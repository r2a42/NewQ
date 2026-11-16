//! Finding the other computers (D3): a beacon broadcast once a second on every
//! adapter, to each port of the show network's small range, and to the same
//! ports on this computer (loopback), so several NewQs on one computer find
//! each other; plus beacons sent straight to addresses added by hand, for
//! networks that drop broadcasts. No mDNS, nothing to install.
//!
//! Each NewQ listens on the first port of the range it can have (UDP for
//! beacons and TCP for sessions on the same number), so a second NewQ on the
//! same computer takes the next one.

use super::proto::{self, Beacon};
use crossbeam_channel::Sender;
use parking_lot::Mutex;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The first port of the range, unless `NEWQ_NET_PORT` says another.
pub const DEFAULT_PORT: u16 = 47820;

/// How many ports the range has: as many NewQs as can share one computer.
pub const PORTS: u16 = 8;

/// How often a beacon goes out.
pub const EVERY: Duration = Duration::from_secs(1);

/// The ports a show network uses from `base`.
pub fn ports(base: u16, span: u16) -> impl Iterator<Item = u16> + Clone {
    (0..span).filter_map(move |i| base.checked_add(i))
}

/// Where each beacon is sent: every port of the range at each broadcast
/// address and on this computer, and the addresses added by hand (each port
/// of the range when none was given).
pub fn targets(
    broadcasts: &[Ipv4Addr],
    base: u16,
    span: u16,
    manual: &[SocketAddr],
    lan: bool,
) -> Vec<SocketAddr> {
    let mut out = Vec::new();
    let mut hosts: Vec<IpAddr> = vec![IpAddr::V4(Ipv4Addr::LOCALHOST)];
    if lan {
        hosts.extend(broadcasts.iter().map(|b| IpAddr::V4(*b)));
    }
    for h in hosts {
        out.extend(ports(base, span).map(|p| SocketAddr::new(h, p)));
    }
    for m in manual {
        if m.port() == 0 {
            out.extend(ports(base, span).map(|p| SocketAddr::new(m.ip(), p)));
        } else {
            out.push(*m);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The broadcast address of each IPv4 adapter that is up (not loopback).
pub fn broadcast_addresses() -> Vec<Ipv4Addr> {
    let mut out: Vec<Ipv4Addr> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback())
        .filter_map(|i| match i.addr {
            if_addrs::IfAddr::V4(v4) => Some(v4.broadcast.unwrap_or_else(|| {
                // No broadcast address given: work it out from the netmask.
                Ipv4Addr::from(u32::from(v4.ip) | !u32::from(v4.netmask))
            })),
            _ => None,
        })
        .filter(|b| !b.is_loopback() && !b.is_unspecified())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The adapter that has address `ip`, if any (its name says wired or Wi-Fi).
pub fn adapter_with(ip: IpAddr) -> Option<String> {
    if_addrs::get_if_addrs()
        .ok()?
        .into_iter()
        .find(|i| i.ip() == ip)
        .map(|i| i.name)
}

/// A beacon heard, and where from.
#[derive(Clone, Debug)]
pub struct Heard {
    pub beacon: Beacon,
    pub from: SocketAddr,
}

/// The beacon thread: sends ours, hears the others'.
pub struct Beaconer {
    stop: Arc<AtomicBool>,
}

impl Beaconer {
    /// Start announcing `content` (kept up to date by the hub) from `socket`,
    /// heard beacons going to `heard`.
    pub fn start(
        socket: UdpSocket,
        base: u16,
        span: u16,
        lan: bool,
        content: Arc<Mutex<Beacon>>,
        manual: Arc<Mutex<Vec<SocketAddr>>>,
        heard: Sender<super::hub::Event>,
    ) -> std::io::Result<Beaconer> {
        socket.set_read_timeout(Some(Duration::from_millis(200)))?;
        let _ = socket.set_broadcast(true);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        std::thread::Builder::new()
            .name("net-beacon".into())
            .spawn(move || {
                let mut buf = vec![0u8; proto::MAX_BEACON];
                let mut next = Instant::now();
                let mut broadcasts = Vec::new();
                let mut looked: Option<Instant> = None;
                let mut warned = false;
                while !stopped.load(Ordering::Relaxed) {
                    let now = Instant::now();
                    if now >= next {
                        next = now + EVERY;
                        // The adapters, looked at again now and then (a cable
                        // plugged in, Wi-Fi joined).
                        if lan && looked.is_none_or(|t| t.elapsed() > Duration::from_secs(10)) {
                            broadcasts = broadcast_addresses();
                            looked = Some(now);
                        }
                        let data = proto::encode_beacon(&content.lock());
                        let manual = manual.lock().clone();
                        let mut failed = 0;
                        for to in targets(&broadcasts, base, span, &manual, lan) {
                            if socket.send_to(&data, to).is_err() {
                                failed += 1;
                            }
                        }
                        if failed > 0 && !warned {
                            log::debug!("show network: {failed} beacons couldn't be sent");
                            warned = true;
                        }
                    }
                    // (An error is a timeout, or on Windows a beacon sent to a
                    // port nobody listens on being refused: nothing to do.)
                    let Ok((n, from)) = socket.recv_from(&mut buf) else {
                        continue;
                    };
                    // A follower asking for the shared time (`net::time`):
                    // answered at once, on this thread, so the reply waits on
                    // nothing else.
                    let received = super::time::local_now();
                    if let Some(reply) = super::time::answer(&buf[..n], received) {
                        let _ = socket.send_to(&reply, from);
                        continue;
                    }
                    if let Some(beacon) = proto::decode_beacon(&buf[..n])
                        && heard
                            .send(super::hub::Event::Heard(Heard { beacon, from }))
                            .is_err()
                    {
                        break;
                    }
                }
            })?;
        Ok(Beaconer { stop })
    }
}

impl Drop for Beaconer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Read "192.168.1.20", "booth.local" or "10.0.0.5:47821" as an address; no
/// port given is port 0 (every port of the range).
pub fn parse_address(text: &str) -> Result<SocketAddr, String> {
    use std::net::ToSocketAddrs;
    let t = text.trim();
    if t.is_empty() {
        return Err("Type the other computer's address.".into());
    }
    if let Ok(a) = t.parse::<SocketAddr>() {
        return Ok(a);
    }
    if let Ok(ip) = t.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, 0));
    }
    let with_port = if t.contains(':') {
        t.to_string()
    } else {
        format!("{t}:0")
    };
    with_port
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.find(|a| a.is_ipv4()))
        .ok_or_else(|| format!("{t} isn't an address this computer can find."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beacons_go_to_every_port_on_each_adapter_and_this_computer() {
        let b = [Ipv4Addr::new(192, 168, 1, 255)];
        let t = targets(&b, 47820, 3, &[], true);
        assert_eq!(t.len(), 6);
        assert!(t.contains(&"127.0.0.1:47822".parse().unwrap()));
        assert!(t.contains(&"192.168.1.255:47820".parse().unwrap()));
        // Only this computer when the network isn't wanted (tests).
        let t = targets(&b, 47820, 2, &[], false);
        assert_eq!(
            t,
            vec![
                "127.0.0.1:47820".parse().unwrap(),
                "127.0.0.1:47821".parse().unwrap()
            ]
        );
        // An address added by hand: its own port, or every port of the range.
        let m = [
            "10.0.0.5:0".parse().unwrap(),
            "10.0.0.6:9000".parse().unwrap(),
        ];
        let t = targets(&[], 47820, 2, &m, true);
        assert!(t.contains(&"10.0.0.5:47821".parse().unwrap()));
        assert!(t.contains(&"10.0.0.6:9000".parse().unwrap()));
        assert!(!t.contains(&"10.0.0.6:47820".parse().unwrap()));
    }

    #[test]
    fn the_range_stops_at_the_last_port() {
        assert_eq!(ports(65534, 8).count(), 2);
    }

    #[test]
    fn addresses_typed_by_hand() {
        assert_eq!(
            parse_address(" 192.168.1.20 "),
            Ok("192.168.1.20:0".parse().unwrap())
        );
        assert_eq!(
            parse_address("10.0.0.5:47821"),
            Ok("10.0.0.5:47821".parse().unwrap())
        );
        assert_eq!(
            parse_address("localhost:5"),
            Ok("127.0.0.1:5".parse().unwrap())
        );
        assert!(parse_address("").is_err());
        assert!(parse_address("no such host.invalid").is_err());
    }
}
