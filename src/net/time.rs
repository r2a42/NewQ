//! The shared clock in the app (stage 4a): what time it is on this computer
//! for the show network, and the clock exchange that keeps a follower's
//! estimate of its primary's clock (`clock::SharedClock`).
//!
//! The primary's shared time is its own monotonic clock, counted from the wall
//! clock as it was when this NewQ started (seconds since 2020), so a primary
//! that restarts comes back at about the time it left rather than at zero: a
//! follower's clock never runs back (`SharedClock::now`), and a step back of
//! hours would have it stand still for as long.
//!
//! The exchange goes over UDP, not the session: a request is 16 bytes and a
//! reply 32 (`msg::Msg::ClockAsk`/`ClockReply`), and over TCP a reply could
//! wait behind a large frame (the show) and be taken for a long round trip.
//! The primary answers on its beacon socket (`discover::Beaconer`), whose port
//! every computer already knows (it is the session's port too), so no other
//! port is opened. A follower asks from a socket of its own (`Keeper`), ten
//! times a second until locked and twice a second after.

use super::clock::{Exchange, SharedClock, SharedNow};
use super::msg::Msg;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 2020-01-01 in seconds since 1970: shared times are counted from here, so
/// they stay small enough for a double to keep them to a tenth of a microsecond.
const EPOCH_2020: f64 = 1_577_836_800.0;

/// This computer's clock for the show network, in seconds: monotonic, never
/// back, starting near the wall clock's time since 2020. The primary's is the
/// shared clock; a follower's is the local side of its exchanges.
pub fn local_now() -> f64 {
    static BASE: OnceLock<(Instant, f64)> = OnceLock::new();
    let (at, wall) = BASE.get_or_init(|| {
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64() - EPOCH_2020)
            .unwrap_or(0.0)
            .max(0.0);
        (Instant::now(), wall)
    });
    wall + at.elapsed().as_secs_f64()
}

/// How often a follower asks for the time, before it is locked and after.
pub const ASK_UNLOCKED: Duration = Duration::from_millis(100);
pub const ASK_LOCKED: Duration = Duration::from_millis(500);
/// Round trips remembered for the lead (`ClockHandle::worst_rtt`).
const RTTS: usize = 40;

/// A follower's shared clock and what is known of its exchanges, shared
/// between the keeper's thread and the app (held for microseconds).
#[derive(Default)]
struct Kept {
    clock: SharedClock,
    /// Round trips of the last replies, seconds.
    rtts: VecDeque<f64>,
    /// When the last reply came (local).
    last_reply: Option<f64>,
}

/// The app's (and the hub's) handle on a follower's shared clock.
#[derive(Clone, Default)]
pub struct ClockHandle(Arc<Mutex<Kept>>);

impl ClockHandle {
    /// The shared time now, once there has been an exchange.
    pub fn now(&self) -> Option<SharedNow> {
        self.0.lock().clock.now(local_now())
    }

    /// The longest of the recent round trips (seconds).
    pub fn worst_rtt(&self) -> Option<f64> {
        self.0.lock().rtts.iter().copied().reduce(f64::max)
    }

    /// Start again (another primary).
    pub fn reset(&self) {
        *self.0.lock() = Kept::default();
    }

    fn take(&self, ex: Exchange, t4: f64) {
        let mut k = self.0.lock();
        k.clock.update(ex, t4);
        k.rtts.push_back(ex.delay());
        while k.rtts.len() > RTTS {
            k.rtts.pop_front();
        }
        k.last_reply = Some(t4);
    }

    fn locked(&self) -> bool {
        self.0.lock().clock.locked()
    }

    /// Where this follower's shared clock is now, for carrying it on as the
    /// primary's (a backup taking control, D7). None before any exchange.
    pub fn continued(&self) -> Option<Continued> {
        let k = self.0.lock();
        let local = local_now();
        let now = k.clock.now(local)?;
        Some(Continued {
            local,
            shared: now.time,
            rate: k.clock.rate(),
        })
    }
}

// ---- Take control (stage 5): the shared clock carried on by a new primary ----

/// A primary's shared clock that carries on another's: `shared` at the local
/// time `local`, running at `rate` shared seconds per local second (the old
/// primary's rate as this computer saw it). A backup that takes control keeps
/// the time it had as a follower, so the cues playing everywhere, anchored to
/// shared times, carry on without a jump.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Continued {
    pub local: f64,
    pub shared: f64,
    pub rate: f64,
}

impl Continued {
    /// The shared time at local time `local`.
    pub fn at(&self, local: f64) -> f64 {
        // A rate from a fit is within a thousandth of 1; anything else is
        // not believed.
        let rate = if self.rate.is_finite() && (self.rate - 1.0).abs() < 0.001 {
            self.rate
        } else {
            1.0
        };
        self.shared + (local - self.local) * rate
    }
}

/// What this computer answers as the shared time when it is the primary: its
/// own clock (`local_now`), or, after it took control, the clock it carried on
/// (`Continued`). One per hub, shared with the beacon thread that answers.
#[derive(Clone, Default)]
pub struct PrimaryClock(Arc<Mutex<Option<Continued>>>);

impl PrimaryClock {
    /// The shared time at local time `local`.
    pub fn at(&self, local: f64) -> f64 {
        match *self.0.lock() {
            Some(c) => c.at(local),
            None => local,
        }
    }

    /// The shared time now.
    pub fn now(&self) -> f64 {
        self.at(local_now())
    }

    /// Carry on `c` from now (None: keep what it is). What the followers had
    /// is what counts: it is taken even if it is behind what this computer
    /// answered when it was the primary before.
    pub fn carry_on(&self, c: Option<Continued>) {
        if let Some(c) = c {
            *self.0.lock() = Some(c);
        }
    }

    #[cfg(test)]
    pub fn get(&self) -> Option<Continued> {
        *self.0.lock()
    }
}

/// The follower's side of the exchange: a thread with a socket of its own that
/// asks the primary at `target` for the time and feeds the answers to the clock.
pub struct Keeper {
    stop: Arc<AtomicBool>,
    target: Arc<Mutex<Option<SocketAddr>>>,
}

impl Keeper {
    pub fn start(clock: ClockHandle, target: SocketAddr) -> std::io::Result<Keeper> {
        let any = if target.ip().is_loopback() {
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        } else {
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        };
        let socket = UdpSocket::bind(SocketAddr::new(any, 0))?;
        socket.set_read_timeout(Some(Duration::from_millis(20)))?;
        let stop = Arc::new(AtomicBool::new(false));
        let to = Arc::new(Mutex::new(Some(target)));
        let (stopped, aim) = (stop.clone(), to.clone());
        std::thread::Builder::new()
            .name("net-clock".into())
            .spawn(move || {
                let _said = crate::crash::OnPanic(|| {
                    log::error!("show network: the clock's thread stopped after a fault");
                });
                keep(socket, clock, aim, stopped)
            })?;
        Ok(Keeper { stop, target: to })
    }

    /// Ask another address (the primary was found at another one).
    pub fn aim(&self, target: SocketAddr) {
        *self.target.lock() = Some(target);
    }

    pub fn target(&self) -> Option<SocketAddr> {
        *self.target.lock()
    }
}

impl Drop for Keeper {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn keep(
    socket: UdpSocket,
    clock: ClockHandle,
    target: Arc<Mutex<Option<SocketAddr>>>,
    stop: Arc<AtomicBool>,
) {
    let mut seq: u32 = 0;
    // Asks in flight: seq and when (local), the last few.
    let mut asked: VecDeque<(u32, f64)> = VecDeque::new();
    let mut next = Instant::now();
    let mut buf = [0u8; super::msg::MAX_LEN];
    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        if now >= next {
            let every = if clock.locked() {
                ASK_LOCKED
            } else {
                ASK_UNLOCKED
            };
            next = now + every;
            if let Some(to) = *target.lock() {
                seq = seq.wrapping_add(1);
                let t1 = local_now();
                if socket
                    .send_to(&Msg::ClockAsk { seq, t1 }.encode(), to)
                    .is_ok()
                {
                    asked.push_back((seq, t1));
                    while asked.len() > 16 {
                        asked.pop_front();
                    }
                }
            }
        }
        let Ok((n, _)) = socket.recv_from(&mut buf) else {
            continue;
        };
        let t4 = local_now();
        if let Ok(Msg::ClockReply { seq, t1, t2, t3 }) = Msg::decode(&buf[..n])
            && let Some(i) = asked.iter().position(|(s, t)| *s == seq && *t == t1)
        {
            asked.remove(i);
            clock.take(Exchange { t1, t2, t3, t4 }, t4);
        }
    }
}

/// The primary's side: the answer to a datagram that is a clock request
/// (`received` is when it came, by `local_now`), by the shared clock `clock`
/// gives, or None.
pub fn answer(data: &[u8], received: f64, clock: &PrimaryClock) -> Option<Vec<u8>> {
    match Msg::decode(data) {
        Ok(Msg::ClockAsk { seq, t1 }) => Some(
            Msg::ClockReply {
                seq,
                t1,
                t2: clock.at(received),
                t3: clock.now(),
            }
            .encode(),
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_local_clock_runs_forward_from_about_the_wall_clock() {
        let a = local_now();
        let b = local_now();
        assert!(b >= a);
        // Somewhere after 2020 and before 2100.
        assert!(a > 0.0 && a < 80.0 * 365.25 * 86400.0, "{a}");
    }

    #[test]
    fn a_follower_locks_to_a_socket_that_answers() {
        // The primary's side: a socket answering as the beacon thread does.
        let primary = UdpSocket::bind("127.0.0.1:0").unwrap();
        primary
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let addr = primary.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let answering = std::thread::spawn(move || {
            let mut buf = [0u8; 64];
            while !stopping.load(Ordering::Relaxed) {
                if let Ok((n, from)) = primary.recv_from(&mut buf)
                    && let Some(reply) = answer(&buf[..n], local_now(), &PrimaryClock::default())
                {
                    let _ = primary.send_to(&reply, from);
                }
            }
        });
        let clock = ClockHandle::default();
        let keeper = Keeper::start(clock.clone(), addr).unwrap();
        let began = Instant::now();
        while !clock.now().is_some_and(|n| n.locked) && began.elapsed() < Duration::from_secs(60) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let now = clock.now().expect("an exchange");
        assert!(now.locked, "not locked in a minute");
        // Same computer, same clock: the error is the loopback's noise.
        let err = (now.time - local_now()).abs();
        assert!(err < 0.01, "{err}");
        assert!(clock.worst_rtt().is_some());
        drop(keeper);
        stop.store(true, Ordering::Relaxed);
        answering.join().unwrap();
    }

    #[test]
    fn a_clock_carried_on_keeps_its_time_and_its_rate() {
        let c = Continued {
            local: 100.0,
            shared: 5000.0,
            rate: 1.0 + 20e-6,
        };
        assert_eq!(c.at(100.0), 5000.0);
        assert!((c.at(200.0) - (5100.0 + 100.0 * 20e-6)).abs() < 1e-9);
        // A rate no fit gives is not believed.
        let wild = Continued { rate: 1.5, ..c };
        assert_eq!(wild.at(200.0), 5100.0);
        // A primary's clock is its own until it carries another's on.
        let p = PrimaryClock::default();
        assert_eq!(p.at(42.0), 42.0);
        p.carry_on(None);
        assert_eq!(p.get(), None);
        p.carry_on(Some(c));
        assert_eq!(p.at(100.0), 5000.0);
        // And the answer to a follower is by it.
        let ask = Msg::ClockAsk { seq: 3, t1: 1.0 }.encode();
        let reply = answer(&ask, 100.0, &p).unwrap();
        match Msg::decode(&reply).unwrap() {
            Msg::ClockReply { seq, t1, t2, t3 } => {
                assert_eq!((seq, t1, t2), (3, 1.0, 5000.0));
                assert!(t3 >= t2 - 1.0, "{t3}");
            }
            m => panic!("{m:?}"),
        }
    }

    /// A follower's clock handle carried on is where its clock is now.
    #[test]
    fn a_follower_clock_carried_on_starts_where_it_is() {
        let h = ClockHandle::default();
        assert!(h.continued().is_none(), "no exchange yet");
        let l = local_now();
        // The primary 1000 s ahead of this computer, 2 ms each way.
        h.take(
            Exchange {
                t1: l,
                t2: l + 1000.002,
                t3: l + 1000.002,
                t4: l + 0.004,
            },
            l + 0.004,
        );
        let c = h.continued().unwrap();
        assert!((c.shared - c.local - 1000.0).abs() < 0.001, "{c:?}");
        assert!((c.rate - 1.0).abs() < 1e-9);
    }
}
