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
/// (`received` is when it came, by `local_now`), or None.
pub fn answer(data: &[u8], received: f64) -> Option<Vec<u8>> {
    match Msg::decode(data) {
        Ok(Msg::ClockAsk { seq, t1 }) => Some(
            Msg::ClockReply {
                seq,
                t1,
                t2: received,
                t3: local_now(),
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
                    && let Some(reply) = answer(&buf[..n], local_now())
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
}
