//! Which way to reach another computer when there are several: a computer on
//! a wired network that also runs a VPN (or has Wi-Fi on) announces itself on
//! every adapter, and the primary hears it from each address in turn. The show
//! network wants the plain local network: a private address on the same
//! subnet as one of this computer's physical adapters, wired before Wi-Fi; a
//! VPN or tunnel last, and never while a local path exists (a VPN can add
//! tens of milliseconds and send the traffic through another country);
//! link-local (169.254.x.x, a cable with no address given) last of all.
//!
//! Pure apart from `own_adapters`: the hub chooses each computer's address
//! with `best` (sessions, the shared clock and the media all use it), and the
//! page names the adapter a connection uses with `kind_of`.

use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr};

/// What kind of network an adapter is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AdapterKind {
    Wired,
    WiFi,
    /// A VPN or another tunnel.
    Vpn,
    /// An adapter whose name doesn't say (a Mac's "en0" is wired or Wi-Fi).
    #[default]
    Other,
}

impl AdapterKind {
    /// For the page: "Wired", "Wi-Fi", "VPN", "Network".
    pub fn label(self) -> &'static str {
        match self {
            AdapterKind::Wired => "Wired",
            AdapterKind::WiFi => "Wi-Fi",
            AdapterKind::Vpn => "VPN",
            AdapterKind::Other => "Network",
        }
    }
}

/// The adapter names VPN and tunnel software uses (lower case, matched as a
/// start of the name or, for the longer ones, anywhere in it).
const TUNNEL_STARTS: [&str; 9] = [
    "utun",
    "ppp",
    "tun",
    "tap",
    "wg",
    "ipsec",
    "gpd",
    "zt",
    "tailscale",
];
const TUNNEL_WORDS: [&str; 16] = [
    "vpn",
    "tap-windows",
    "wireguard",
    "wintun",
    "pangp",
    "palo alto",
    "anyconnect",
    "cisco",
    "fortinet",
    "forticlient",
    "openvpn",
    "nordlynx",
    "tailscale",
    "zerotier",
    "globalprotect",
    "juniper",
];

/// The kind of an adapter, from its name: "Ethernet", "enp3s0" wired;
/// "Wi-Fi", "wlan0" Wi-Fi; "utun3", "wg0", "TAP-Windows Adapter",
/// "PANGP Virtual Ethernet Adapter" a VPN (checked first: several of those
/// call themselves Ethernet).
pub fn kind_of(name: &str) -> AdapterKind {
    let n = name.trim().to_ascii_lowercase();
    if TUNNEL_STARTS.iter().any(|s| n.starts_with(s)) || TUNNEL_WORDS.iter().any(|w| n.contains(w))
    {
        return AdapterKind::Vpn;
    }
    match crate::net::proto::Link::from_adapter_name(&n) {
        crate::net::proto::Link::Wired => AdapterKind::Wired,
        crate::net::proto::Link::WiFi => AdapterKind::WiFi,
        crate::net::proto::Link::Unknown => AdapterKind::Other,
    }
}

/// One IPv4 adapter of this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adapter {
    pub name: String,
    pub ip: Ipv4Addr,
    pub mask: Ipv4Addr,
    pub kind: AdapterKind,
}

impl Adapter {
    pub fn new(name: &str, ip: Ipv4Addr, mask: Ipv4Addr) -> Adapter {
        Adapter {
            name: name.to_string(),
            ip,
            mask,
            kind: kind_of(name),
        }
    }

    /// Whether `ip` is on this adapter's own subnet.
    pub fn holds(&self, ip: Ipv4Addr) -> bool {
        let m = u32::from(self.mask);
        m != 0 && u32::from(self.ip) & m == u32::from(ip) & m
    }
}

/// This computer's IPv4 adapters that are up (not loopback).
pub fn own_adapters() -> Vec<Adapter> {
    let mut v: Vec<Adapter> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback())
        .filter_map(|i| match i.addr {
            if_addrs::IfAddr::V4(a) => Some(Adapter::new(&i.name, a.ip, a.netmask)),
            _ => None,
        })
        .collect();
    v.sort_by(|a, b| (rank_own(a), &a.name).cmp(&(rank_own(b), &b.name)));
    v
}

fn rank_own(a: &Adapter) -> u8 {
    if a.ip.is_link_local() {
        return 9;
    }
    match a.kind {
        AdapterKind::Wired => 0,
        AdapterKind::WiFi => 1,
        AdapterKind::Other => 2,
        AdapterKind::Vpn => 8,
    }
}

/// Whether a VPN or tunnel is up on this computer.
pub fn vpn_on(own: &[Adapter]) -> bool {
    own.iter().any(|a| a.kind == AdapterKind::Vpn)
}

/// The adapter of ours that `ip` (one of our addresses) belongs to.
pub fn adapter_of(ip: IpAddr, own: &[Adapter]) -> Option<&Adapter> {
    match ip {
        IpAddr::V4(v4) => own.iter().find(|a| a.ip == v4),
        IpAddr::V6(_) => None,
    }
}

/// How good a way to reach another computer at `ip` is (lower is better):
/// this computer itself; the subnet of a wired, a Wi-Fi, another physical
/// adapter of ours; a private address; anything else; a VPN's subnet or a
/// VPN-like range (100.64/10); link-local.
pub fn rank(ip: IpAddr, own: &[Adapter]) -> u8 {
    let v4 = match ip {
        IpAddr::V4(v4) => v4,
        IpAddr::V6(v6) if v6.is_loopback() => return 0,
        IpAddr::V6(_) => return 6,
    };
    if v4.is_loopback() {
        return 0;
    }
    if v4.is_link_local() {
        return 9;
    }
    // The subnet of one of ours: its kind says how good it is.
    if let Some(a) = own
        .iter()
        .filter(|a| a.holds(v4))
        .min_by_key(|a| rank_own(a))
    {
        return match a.kind {
            AdapterKind::Wired => 1,
            AdapterKind::WiFi => 2,
            AdapterKind::Other => 3,
            AdapterKind::Vpn => 8,
        };
    }
    let o = v4.octets();
    if o[0] == 100 && (64..128).contains(&o[1]) {
        // Carrier-grade NAT's range, which Tailscale and others use.
        return 7;
    }
    if v4.is_private() { 4 } else { 5 }
}

/// The best of a computer's `candidates` (the addresses it was heard from);
/// of equally good ones the first, so a chosen address isn't swapped for an
/// equal one.
pub fn best(candidates: &[IpAddr], own: &[Adapter]) -> Option<IpAddr> {
    candidates
        .iter()
        .enumerate()
        .min_by_key(|(i, ip)| (rank(**ip, own), *i))
        .map(|(_, ip)| *ip)
}

/// Whether a connection to `now` should give way to `better`: it is a VPN's
/// or another clearly worse way (a local path has appeared).
pub fn worse(now: IpAddr, better: IpAddr, own: &[Adapter]) -> bool {
    let (a, b) = (rank(now, own), rank(better, own));
    b < a && a >= 4
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ad(name: &str, ip: &str, mask: &str) -> Adapter {
        Adapter::new(name, ip.parse().unwrap(), mask.parse().unwrap())
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn adapters_are_told_apart_by_name_vpns_first() {
        for (n, k) in [
            ("Ethernet", AdapterKind::Wired),
            ("enp3s0", AdapterKind::Wired),
            ("Wi-Fi", AdapterKind::WiFi),
            ("wlan0", AdapterKind::WiFi),
            ("en0", AdapterKind::Other),
            ("utun4", AdapterKind::Vpn),
            ("ppp0", AdapterKind::Vpn),
            ("tun0", AdapterKind::Vpn),
            ("wg0", AdapterKind::Vpn),
            ("ipsec0", AdapterKind::Vpn),
            ("TAP-Windows Adapter V9", AdapterKind::Vpn),
            ("WireGuard Tunnel", AdapterKind::Vpn),
            ("Wintun Userspace Tunnel", AdapterKind::Vpn),
            ("PANGP Virtual Ethernet Adapter", AdapterKind::Vpn),
            ("Cisco AnyConnect Secure Mobility Client", AdapterKind::Vpn),
            (
                "Fortinet SSL VPN Virtual Ethernet Adapter",
                AdapterKind::Vpn,
            ),
            ("Ethernet 3 (NordLynx)", AdapterKind::Vpn),
            ("tailscale0", AdapterKind::Vpn),
        ] {
            assert_eq!(kind_of(n), k, "{n}");
        }
        assert!(vpn_on(&[ad("utun3", "10.8.0.2", "255.255.255.0")]));
        assert!(!vpn_on(&[ad("Ethernet", "192.168.1.5", "255.255.255.0")]));
    }

    #[test]
    fn the_local_network_wins_over_a_vpn_wired_over_wifi_and_link_local_comes_last() {
        let own = [
            ad("utun3", "10.8.0.2", "255.255.255.0"),
            ad("Wi-Fi", "192.168.50.7", "255.255.255.0"),
            ad("Ethernet", "192.168.1.10", "255.255.255.0"),
        ];
        // The same computer heard on its VPN address and its LAN one: the LAN.
        let heard = [ip("10.8.0.9"), ip("192.168.1.206")];
        assert_eq!(best(&heard, &own), Some(ip("192.168.1.206")));
        // Wired subnet before Wi-Fi's.
        assert_eq!(
            best(&[ip("192.168.50.20"), ip("192.168.1.20")], &own),
            Some(ip("192.168.1.20"))
        );
        // Any private address before a VPN subnet; the VPN before link-local.
        assert!(rank(ip("172.16.4.4"), &own) < rank(ip("10.8.0.9"), &own));
        assert!(rank(ip("10.8.0.9"), &own) < rank(ip("169.254.3.3"), &own));
        assert!(rank(ip("100.101.1.1"), &own) > rank(ip("8.8.8.8"), &own));
        // This computer itself (two NewQs on one machine) is best.
        assert_eq!(rank(ip("127.0.0.1"), &own), 0);
        // Equal ones: the first is kept.
        assert_eq!(
            best(&[ip("192.168.1.30"), ip("192.168.1.20")], &own),
            Some(ip("192.168.1.30"))
        );
        assert_eq!(best(&[], &own), None);
        // A connection over the VPN gives way to the LAN; a wired one doesn't
        // move for a Wi-Fi one (nor the other way: both are local).
        assert!(worse(ip("10.8.0.9"), ip("192.168.1.206"), &own));
        assert!(!worse(ip("192.168.50.20"), ip("192.168.1.20"), &own));
        assert!(!worse(ip("192.168.1.20"), ip("10.8.0.9"), &own));
    }

    #[test]
    fn our_own_adapters_are_sorted_wired_first_and_found_by_address() {
        let own = [
            ad("utun3", "10.8.0.2", "255.255.255.0"),
            ad("Ethernet", "192.168.1.10", "255.255.255.0"),
        ];
        assert_eq!(
            adapter_of(ip("192.168.1.10"), &own).map(|a| a.kind),
            Some(AdapterKind::Wired)
        );
        assert!(adapter_of(ip("192.168.1.11"), &own).is_none());
        assert!(own[1].holds("192.168.1.200".parse().unwrap()));
        assert!(!own[1].holds("192.168.2.200".parse().unwrap()));
        assert_eq!(AdapterKind::Vpn.label(), "VPN");
    }
}
