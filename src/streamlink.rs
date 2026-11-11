//! The links a streaming server (MediaMTX) stream is watched by, a QR code
//! for them, the computer's network adapters, and a check that the server
//! answers. All of it is pure or small enough to test, apart from the
//! adapter list and the connection check.

use crate::show::ServerProtocol;
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// One network connection of this computer that has an IPv4 address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adapter {
    pub name: String,
    pub ip: IpAddr,
}

impl Adapter {
    /// What the list shows: `Wi-Fi (192.168.1.20)`.
    pub fn label(&self) -> String {
        format!("{} ({})", self.name, self.ip)
    }
}

/// The adapters a phone could reach: IPv4, up, not loopback or link-local.
pub fn adapters() -> Vec<Adapter> {
    let mut found: Vec<Adapter> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback() && !i.is_link_local())
        .filter_map(|i| match i.ip() {
            ip @ IpAddr::V4(_) => Some(Adapter { name: i.name, ip }),
            IpAddr::V6(_) => None,
        })
        .collect();
    found.sort_by(|a, b| (&a.name, a.ip).cmp(&(&b.name, b.ip)));
    found.dedup();
    found
}

/// An adapter name that means this computer alone: a server on it listens on
/// 127.0.0.1 and nothing else on the network can reach it. Test Mode uses it;
/// it is not offered in the list.
pub const LOOPBACK: &str = "this computer only";

/// The address of the adapter called `name`, if this computer has it now.
pub fn adapter_ip(name: &str, list: &[Adapter]) -> Option<IpAddr> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    if name == LOOPBACK {
        return Some(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    }
    list.iter().find(|a| a.name == name).map(|a| a.ip)
}

fn is_this_computer(host: &str) -> bool {
    let h = host.trim().trim_matches(|c| c == '[' || c == ']');
    h.is_empty()
        || h.eq_ignore_ascii_case("localhost")
        || h.parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback() || ip.is_unspecified())
}

/// The address FFmpeg publishes to: the server as typed, except that a server
/// on this computer (127.0.0.1, localhost) is reached by the chosen adapter's
/// address, for a MediaMTX that listens on that adapter only.
pub fn publish_host(host: &str, adapter: &str, list: &[Adapter]) -> String {
    match adapter_ip(adapter, list) {
        Some(ip) if is_this_computer(host) => ip.to_string(),
        _ => host.trim().to_string(),
    }
}

/// The address a viewer's device uses: the chosen adapter's, else the server
/// as typed, else (for a server on this computer) the computer's address on
/// the local network. `None` when there is nothing a phone could reach.
pub fn viewer_host(
    host: &str,
    adapter: &str,
    list: &[Adapter],
    lan: Option<IpAddr>,
) -> Option<String> {
    if let Some(ip) = adapter_ip(adapter, list) {
        return Some(ip.to_string());
    }
    if is_this_computer(host) {
        return lan.map(|ip| ip.to_string());
    }
    Some(host.trim().to_string())
}

fn bracket(host: &str) -> String {
    if host.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

/// The page that plays the stream in a browser (MediaMTX's WebRTC player).
pub fn player_url(host: &str, port: u16, path: &str) -> String {
    let port = if port == 0 {
        crate::show::default_player_port()
    } else {
        port
    };
    format!(
        "http://{}:{port}/{}",
        bracket(host.trim()),
        path.trim().trim_matches('/')
    )
}

/// The link a player such as VLC takes, as the protocol's own URL.
pub fn stream_link(protocol: ServerProtocol, host: &str, port: u16, path: &str) -> String {
    let port = if port == 0 {
        protocol.default_port()
    } else {
        port
    };
    let (host, path) = (bracket(host.trim()), path.trim().trim_matches('/'));
    match protocol {
        ServerProtocol::Rtsp => format!("rtsp://{host}:{port}/{path}"),
        ServerProtocol::Rtmp => format!("rtmp://{host}:{port}/{path}"),
        ServerProtocol::Srt => format!("srt://{host}:{port}?streamid=read:{path}"),
    }
}

/// A QR code as a square of dark (`true`) and light modules, without the quiet
/// zone (the drawing adds it). `None` if the text is too long for a code.
pub fn qr_modules(text: &str) -> Option<(usize, Vec<bool>)> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let width = code.width();
    let dark = code
        .to_colors()
        .into_iter()
        .map(|c| c == qrcode::Color::Dark)
        .collect();
    Some((width, dark))
}

/// How a try at reaching one port went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    pub what: String,
    pub result: Result<Duration, String>,
}

/// Open a TCP connection to `host:port` and close it again.
pub fn tcp_probe(host: &str, port: u16, timeout: Duration) -> Result<Duration, String> {
    let started = Instant::now();
    // (Windows resolves an empty name to this computer, which then answers.)
    if bracket_less(host).is_empty() {
        return Err("no address for the streaming server".into());
    }
    let addrs: Vec<SocketAddr> = (bracket_less(host), port)
        .to_socket_addrs()
        .map_err(|e| format!("can't find {host}: {e}"))?
        .collect();
    let mut last = format!("no address for {host}");
    for a in addrs {
        match TcpStream::connect_timeout(&a, timeout) {
            Ok(_) => return Ok(started.elapsed()),
            Err(e) => {
                last = match e.kind() {
                    std::io::ErrorKind::ConnectionRefused => {
                        format!("nothing is listening on {a} (is the server running?)")
                    }
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => {
                        format!("no answer from {a} (wrong address, or a firewall)")
                    }
                    _ => format!("{a}: {e}"),
                }
            }
        }
    }
    Err(last)
}

fn bracket_less(host: &str) -> &str {
    host.trim().trim_matches(|c| c == '[' || c == ']')
}

/// Look for the streaming server: its publishing port (RTSP and RTMP are TCP;
/// SRT is UDP, which can't be asked without a handshake, so it is left out)
/// and its web player's port. A stream that isn't running yet still shows
/// the server is up.
pub fn probe_server(
    protocol: ServerProtocol,
    host: &str,
    port: u16,
    player_port: u16,
) -> Vec<Probe> {
    // (Windows takes about two seconds to give up on a closed port.)
    let timeout = Duration::from_millis(3500);
    let mut out = Vec::new();
    let port = if port == 0 {
        protocol.default_port()
    } else {
        port
    };
    if protocol == ServerProtocol::Srt {
        out.push(Probe {
            what: format!("SRT port {port}"),
            result: Err("SRT is UDP: it can't be checked until a stream is sent".into()),
        });
    } else {
        out.push(Probe {
            what: format!("{} port {port}", protocol.label()),
            result: tcp_probe(host, port, timeout),
        });
    }
    let pp = if player_port == 0 {
        crate::show::default_player_port()
    } else {
        player_port
    };
    out.push(Probe {
        what: format!("web player port {pp}"),
        result: tcp_probe(host, pp, timeout),
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, TcpListener};

    fn wifi() -> Vec<Adapter> {
        vec![
            Adapter {
                name: "eth0".into(),
                ip: Ipv4Addr::new(10, 0, 0, 5).into(),
            },
            Adapter {
                name: "wlan0".into(),
                ip: Ipv4Addr::new(192, 168, 1, 20).into(),
            },
        ]
    }

    #[test]
    fn the_player_link_is_the_web_player_of_the_stream() {
        assert_eq!(
            player_url("192.168.1.20", 8889, "newq"),
            "http://192.168.1.20:8889/newq"
        );
        assert_eq!(
            player_url(" host ", 0, "/live/show/"),
            "http://host:8889/live/show"
        );
        assert_eq!(player_url("fe80::1", 8889, "a"), "http://[fe80::1]:8889/a");
    }

    #[test]
    fn stream_links_follow_the_protocol() {
        assert_eq!(
            stream_link(ServerProtocol::Rtsp, "h", 0, "n"),
            "rtsp://h:8554/n"
        );
        assert_eq!(
            stream_link(ServerProtocol::Rtmp, "h", 1936, "n"),
            "rtmp://h:1936/n"
        );
        assert_eq!(
            stream_link(ServerProtocol::Srt, "h", 0, "n"),
            "srt://h:8890?streamid=read:n"
        );
    }

    #[test]
    fn an_adapter_is_found_by_name_and_empty_means_none() {
        let l = wifi();
        assert_eq!(
            adapter_ip("wlan0", &l),
            Some(Ipv4Addr::new(192, 168, 1, 20).into())
        );
        assert_eq!(
            adapter_ip(" eth0 ", &l),
            Some(Ipv4Addr::new(10, 0, 0, 5).into())
        );
        assert_eq!(adapter_ip("", &l), None);
        assert_eq!(adapter_ip("gone0", &l), None);
    }

    #[test]
    fn a_local_server_is_reached_by_the_chosen_adapter_and_a_remote_one_is_not_changed() {
        let l = wifi();
        assert_eq!(publish_host("127.0.0.1", "wlan0", &l), "192.168.1.20");
        assert_eq!(publish_host("localhost", "eth0", &l), "10.0.0.5");
        assert_eq!(publish_host("media.local", "wlan0", &l), "media.local");
        assert_eq!(publish_host("127.0.0.1", "", &l), "127.0.0.1");
        // An adapter that has gone leaves the address as typed.
        assert_eq!(publish_host("127.0.0.1", "gone0", &l), "127.0.0.1");
    }

    #[test]
    fn viewers_get_an_address_a_phone_can_reach() {
        let l = wifi();
        let lan: Option<IpAddr> = Some(Ipv4Addr::new(172, 16, 0, 9).into());
        assert_eq!(
            viewer_host("127.0.0.1", "wlan0", &l, lan).as_deref(),
            Some("192.168.1.20")
        );
        assert_eq!(
            viewer_host("127.0.0.1", "", &l, lan).as_deref(),
            Some("172.16.0.9")
        );
        assert_eq!(viewer_host("localhost", "", &l, None), None);
        assert_eq!(
            viewer_host("media.local", "", &l, lan).as_deref(),
            Some("media.local")
        );
    }

    #[test]
    fn a_qr_code_is_square_and_has_the_finder_squares() {
        let (w, dark) = qr_modules("http://192.168.1.20:8889/newq").unwrap();
        assert_eq!(dark.len(), w * w);
        assert!(w >= 21 && (w - 21) % 4 == 0, "{w}");
        // The top-left finder pattern: a dark ring, light ring, dark centre.
        let at = |x: usize, y: usize| dark[y * w + x];
        assert!(at(0, 0) && at(6, 0) && at(0, 6) && at(6, 6));
        assert!(!at(1, 1) && !at(5, 5) && at(3, 3));
        // The same text always makes the same code.
        assert_eq!(qr_modules("http://192.168.1.20:8889/newq").unwrap().1, dark);
        // Too much for any code.
        assert!(qr_modules(&"x".repeat(8000)).is_none());
    }

    #[test]
    fn a_probe_finds_a_listening_port_and_reports_a_closed_one() {
        let open = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = open.local_addr().unwrap().port();
        assert!(tcp_probe("127.0.0.1", port, Duration::from_secs(2)).is_ok());
        let closed = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        assert!(tcp_probe("127.0.0.1", closed, Duration::from_secs(2)).is_err());
        assert!(tcp_probe("", 80, Duration::from_millis(200)).is_err());
    }

    #[test]
    fn the_server_check_names_each_port_and_leaves_srt_unchecked() {
        let open = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = open.local_addr().unwrap().port();
        let r = probe_server(ServerProtocol::Rtsp, "127.0.0.1", port, port);
        assert_eq!(r.len(), 2);
        assert!(r.iter().all(|p| p.result.is_ok()), "{r:?}");
        assert!(r[0].what.starts_with("RTSP"));
        let s = probe_server(ServerProtocol::Srt, "127.0.0.1", 0, port);
        assert!(s[0].result.is_err() && s[1].result.is_ok());
    }

    #[test]
    fn this_computers_adapters_can_be_listed() {
        for a in adapters() {
            assert!(a.ip.is_ipv4() && !a.ip.is_loopback());
            assert!(a.label().contains(&a.name));
        }
    }
}

#[cfg(test)]
mod qr_picture {
    /// Writes the code as the UI draws it (with the quiet zone) when
    /// NEWQ_QR_PNG names a file, so another program can try reading it.
    #[test]
    fn write_the_code_for_a_reader() {
        let Ok(path) = std::env::var("NEWQ_QR_PNG") else {
            return;
        };
        let (n, dark) = super::qr_modules("http://192.168.1.20:8889/newq").unwrap();
        let (quiet, cell) = (3usize, 8usize);
        let side = (n + 2 * quiet) * cell;
        let mut img = image::GrayImage::from_pixel(side as u32, side as u32, image::Luma([255]));
        for y in 0..n {
            for x in 0..n {
                if dark[y * n + x] {
                    for dy in 0..cell {
                        for dx in 0..cell {
                            img.put_pixel(
                                (((x + quiet) * cell) + dx) as u32,
                                (((y + quiet) * cell) + dy) as u32,
                                image::Luma([0]),
                            );
                        }
                    }
                }
            }
        }
        img.save(path).unwrap();
    }
}
