//! Who this computer is in a show network: its id, name and role, the network
//! it belongs to and the primary it follows (`node.json` in the log folder, per
//! computer and never in the show), and the pure rules for whom it obeys.
//!
//! A computer is `Alone` and switched off until someone turns it on (D1). Once
//! on, an unadopted computer accepts the first primary that adopts it, with
//! nothing typed on it (R3, R11); after that it obeys only that network, and of
//! its primaries the one with the highest `term` (D7). A passcode is optional:
//! its key (a hash, never the passcode) is kept here, and a computer that has
//! one refuses a primary that can't prove it knows it.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The file this computer's identity is kept in.
pub const FILE: &str = "node.json";

/// A computer of a show network: random, made once, kept in `node.json`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u64);

/// A show network: made by the computer that runs it, given to each it adopts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NetworkId(pub u64);

macro_rules! hex_id {
    ($t:ident) => {
        impl std::fmt::Display for $t {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{:016x}", self.0)
            }
        }
        impl std::str::FromStr for $t {
            type Err = String;
            fn from_str(s: &str) -> Result<Self, String> {
                u64::from_str_radix(s.trim(), 16)
                    .map($t)
                    .map_err(|_| format!("not an id: {s:?}"))
            }
        }
        impl Serialize for $t {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.to_string())
            }
        }
        impl<'de> Deserialize<'de> for $t {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }
        impl $t {
            pub fn random() -> $t {
                $t(random_u64())
            }
        }
    };
}
hex_id!(NodeId);
hex_id!(NetworkId);

impl NodeId {
    /// The first few digits, for saying which one in a sentence.
    pub fn short(&self) -> String {
        self.to_string()[..6].to_string()
    }
}

/// 64 random bits without a random-number crate: the standard library's
/// randomly keyed hasher (seeded from the system once per process, then
/// stepped), the time, the process and a counter, through SHA-256.
pub fn random_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(COUNT.fetch_add(1, Ordering::Relaxed));
    let mut s = Sha256::new();
    s.update(h.finish().to_le_bytes());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    s.update(now.as_nanos().to_le_bytes());
    s.update(std::process::id().to_le_bytes());
    let out = s.finalize();
    let mut b = [0u8; 8];
    b.copy_from_slice(&out[..8]);
    // Never 0, which reads as "none" in a sentence.
    u64::from_le_bytes(b).max(1)
}

/// What a computer is for in a show network. A role a later NewQ adds reads
/// as `Alone`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub enum Role {
    /// Not part of a show network (or switched on and waiting to be adopted).
    #[default]
    Alone,
    /// Shows pictures and plays sound for the primary.
    Display,
    /// Runs the show network.
    Primary,
    /// Follows the primary, ready to take over.
    Backup,
}

impl<'de> Deserialize<'de> for Role {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match String::deserialize(d)?.as_str() {
            "Display" => Role::Display,
            "Primary" => Role::Primary,
            "Backup" => Role::Backup,
            _ => Role::Alone,
        })
    }
}

impl Role {
    /// The roles the primary can give another computer from its page.
    pub const GIVEN: [Role; 2] = [Role::Display, Role::Backup];

    pub fn label(self) -> &'static str {
        match self {
            Role::Alone => "Not in the show",
            Role::Display => "Display",
            Role::Primary => "Primary",
            Role::Backup => "Backup",
        }
    }

    /// What the role is, as the rest of a sentence ("a display computer").
    pub fn described(self) -> &'static str {
        match self {
            Role::Alone => "not part of a show network",
            Role::Display => "a display computer",
            Role::Primary => "the primary",
            Role::Backup => "the backup",
        }
    }
}

/// The key a passcode gives: what is kept and compared, never the passcode.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Key(pub [u8; 32]);

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Key(…)")
    }
}

impl Key {
    /// The key for a passcode. The same passcode gives the same key on every
    /// computer, so one typed on a computer before it joins matches the
    /// primary's. Spaces at either end don't count.
    pub fn from_passcode(passcode: &str) -> Key {
        let mut s = Sha256::new();
        s.update(b"NewQ show network passcode\0");
        s.update(passcode.trim().as_bytes());
        let mut k = [0u8; 32];
        k.copy_from_slice(&s.finalize());
        Key(k)
    }

    /// HMAC-SHA-256 of `parts` under this key.
    pub fn mac(&self, parts: &[&[u8]]) -> [u8; 32] {
        let mut ipad = [0x36u8; 64];
        let mut opad = [0x5cu8; 64];
        for (i, b) in self.0.iter().enumerate() {
            ipad[i] ^= b;
            opad[i] ^= b;
        }
        let mut inner = Sha256::new();
        inner.update(ipad);
        for p in parts {
            inner.update(p);
        }
        let mut outer = Sha256::new();
        outer.update(opad);
        outer.update(inner.finalize());
        let mut out = [0u8; 32];
        out.copy_from_slice(&outer.finalize());
        out
    }

    /// What a computer sends to show it has the key: a MAC of the nonce the
    /// other side sent and of its own id, so a proof can't be sent back to its
    /// maker or replayed on another session.
    pub fn proof(&self, their_nonce: u64, my_id: NodeId) -> String {
        hex(&self.mac(&[
            b"newq-proof",
            &their_nonce.to_le_bytes(),
            &my_id.0.to_le_bytes(),
        ]))
    }

    /// Whether `proof` is what `from`, holding this key, would send for `my_nonce`.
    pub fn checks(&self, proof: &str, my_nonce: u64, from: NodeId) -> bool {
        let want = self.proof(my_nonce, from);
        // The same length, compared without stopping at the first difference.
        want.len() == proof.len()
            && want
                .bytes()
                .zip(proof.bytes())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

impl Serialize for Key {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex(&self.0))
    }
}

impl<'de> Deserialize<'de> for Key {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let b = unhex(&s)
            .filter(|b| b.len() == 32)
            .ok_or_else(|| serde::de::Error::custom("not a passcode key"))?;
        let mut k = [0u8; 32];
        k.copy_from_slice(&b);
        Ok(Key(k))
    }
}

/// This computer in a show network, as `node.json` keeps it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Identity {
    pub id: NodeId,
    /// What it is called; None = the computer's own (host) name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub role: Role,
    /// Switched on: it listens and announces itself. Off, it opens no sockets.
    #[serde(default)]
    pub on: bool,
    /// The show network it belongs to (its own, for a primary).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkId>,
    /// The primary it follows, and what that one was called when last heard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<NodeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_name: Option<String>,
    /// The passcode's key, when the network has one (or, before joining, when
    /// one was typed here: only a primary that has it is accepted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passcode: Option<Key>,
    /// The highest primary term seen (D7): a primary with a lower one is old.
    #[serde(default)]
    pub term: u64,
}

impl Identity {
    /// A computer never seen before: alone, switched off.
    pub fn fresh() -> Identity {
        Identity {
            id: NodeId::random(),
            name: None,
            role: Role::Alone,
            on: false,
            network: None,
            primary: None,
            primary_name: None,
            passcode: None,
            term: 0,
        }
    }

    /// The name to show: the one given, else the computer's own.
    pub fn display_name(&self) -> String {
        self.name.clone().unwrap_or_else(host_name)
    }

    /// Adopted by a primary (a display or backup).
    pub fn adopted(&self) -> bool {
        self.network.is_some() && self.role != Role::Primary
    }
}

/// The computer's own name, as the system gives it.
pub fn host_name() -> String {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        sysinfo::System::host_name()
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "This computer".into())
    })
    .clone()
}

/// The folder `node.json` is in: `NEWQ_NODE_DIR` when set (tests, and several
/// NewQs on one computer), else the log folder.
pub fn node_dir() -> PathBuf {
    match std::env::var_os("NEWQ_NODE_DIR").filter(|v| !v.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => crate::crash::log_dir(),
    }
}

/// Read this computer's identity. A missing file is a new computer; one that
/// can't be read is put aside (`node.unreadable.json`) and a new identity made,
/// alone and off: never a crash. Either way the result is written back, so the
/// id stays the same from then on.
pub fn load(dir: &Path) -> Identity {
    let path = dir.join(FILE);
    let found = match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<Identity>(&bytes) {
            Ok(id) => Some(id),
            Err(e) => {
                log::warn!(
                    "{} can't be read ({e}); this computer starts again outside any show network",
                    path.display()
                );
                let _ = std::fs::rename(&path, dir.join("node.unreadable.json"));
                None
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            log::warn!("{} can't be read: {e}", path.display());
            None
        }
    };
    match found {
        Some(id) => id,
        None => {
            let id = Identity::fresh();
            if let Err(e) = save(dir, &id) {
                log::warn!("could not write {}: {e}", path.display());
            }
            id
        }
    }
}

/// Write the identity, whole or not at all (a temporary file renamed over it).
pub fn save(dir: &Path, id: &Identity) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join("node.json.tmp");
    let json = serde_json::to_vec_pretty(id).map_err(std::io::Error::other)?;
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, dir.join(FILE))
}

/// What the command line asked of this computer's identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Startup {
    /// `--join`: switched on, waiting to be adopted.
    Join,
    /// `--primary`: runs a show network.
    Primary,
}

pub fn startup_flag(args: &[String]) -> Option<Startup> {
    if args.iter().any(|a| a == "--primary") {
        Some(Startup::Primary)
    } else if args.iter().any(|a| a == "--join") {
        Some(Startup::Join)
    } else {
        None
    }
}

/// Act on `--join` or `--primary`: kept in `node.json` like the page's switch
/// and button, so a computer set up from the command line stays so. Returns
/// what to say in the log.
pub fn apply_startup(flag: Startup, dir: &Path) -> String {
    let me = load(dir);
    let next = match flag {
        Startup::Join => switch_on(&me),
        Startup::Primary if me.adopted() => {
            return format!(
                "--primary: this computer is {} of another primary's show network; release it \
                 from there first",
                me.role.described()
            );
        }
        Startup::Primary => become_primary(&me),
    };
    if let Err(e) = save(dir, &next) {
        return format!("could not keep the show network setting: {e}");
    }
    match flag {
        Startup::Join => "--join: this computer is part of a show network".into(),
        Startup::Primary => "--primary: this computer runs the show network".into(),
    }
}

/// Switch on (`--join`, or the page's switch). A computer that is adopted or
/// runs the network stays as it is.
pub fn switch_on(me: &Identity) -> Identity {
    Identity {
        on: true,
        ..me.clone()
    }
}

/// Switch off: a primary keeps its network (switched on again, it runs the same
/// one); any other computer leaves the show network it was in.
pub fn switch_off(me: &Identity) -> Identity {
    if me.role == Role::Primary {
        Identity {
            on: false,
            ..me.clone()
        }
    } else {
        Identity {
            on: false,
            ..release(me)
        }
    }
}

/// Run a show network from this computer: its own network if it already ran
/// one, else a new one. Not offered on a computer another primary adopted.
pub fn become_primary(me: &Identity) -> Identity {
    let network = match (me.role, me.network) {
        (Role::Primary, Some(n)) => n,
        _ => NetworkId::random(),
    };
    Identity {
        role: Role::Primary,
        on: true,
        network: Some(network),
        primary: Some(me.id),
        primary_name: None,
        term: me.term.max(1),
        // A passcode typed while waiting to join was for another network.
        passcode: if me.role == Role::Primary {
            me.passcode
        } else {
            None
        },
        ..me.clone()
    }
}

/// A primary asking this computer to take orders from it.
#[derive(Clone, Debug, PartialEq)]
pub struct Claim {
    pub from: NodeId,
    pub from_name: String,
    pub network: NetworkId,
    pub term: u64,
    /// Whether the primary proved it has this computer's passcode key: None
    /// when this computer has no key to check against.
    pub proof: Option<bool>,
}

/// Why a primary's orders are refused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Refusal {
    /// This computer isn't switched on.
    Off,
    /// It belongs to another show network.
    OtherNetwork,
    /// The passcode is missing or wrong.
    Passcode,
    /// A primary with a higher term has been seen: this one is out of date.
    OldTerm { seen: u64 },
    /// Only a primary gives orders; this would make a primary obey a computer
    /// that isn't one.
    NotPrimary,
    /// The other computer runs a NewQ that speaks another protocol.
    Version,
}

impl Refusal {
    /// A sentence for the page, from the side that was refused.
    pub fn sentence(&self, refuser: &str) -> String {
        match self {
            Refusal::Off => format!("{refuser} isn't switched on to be part of a show network."),
            Refusal::OtherNetwork => {
                format!(
                    "{refuser} belongs to another show network. Release it from that one first."
                )
            }
            Refusal::Passcode => format!(
                "{refuser} needs this show network's passcode, and the one set here doesn't match."
            ),
            Refusal::OldTerm { seen } => format!(
                "{refuser} has been run by a newer primary (turn {seen}) since this one: this \
                 computer is out of date."
            ),
            Refusal::NotPrimary => format!("{refuser} takes orders only from a primary."),
            Refusal::Version => format!("{refuser} runs another version of NewQ."),
        }
    }
}

/// Whether this computer takes orders from `claim`'s primary: its own network's
/// (or, unadopted, anyone's), with the passcode if it has one, and not one
/// older than the newest seen. A primary obeys only a newer primary of its own
/// network (D7: it then becomes the backup).
pub fn judge(me: &Identity, claim: &Claim) -> Result<(), Refusal> {
    if !me.on {
        return Err(Refusal::Off);
    }
    if me.passcode.is_some() && claim.proof != Some(true) {
        return Err(Refusal::Passcode);
    }
    match (me.role, me.network) {
        (_, None) => Ok(()),
        (_, Some(n)) if n != claim.network => Err(Refusal::OtherNetwork),
        (Role::Primary, Some(_)) if claim.term <= me.term => {
            Err(Refusal::OldTerm { seen: me.term })
        }
        (_, Some(_)) if claim.term < me.term => Err(Refusal::OldTerm { seen: me.term }),
        _ => Ok(()),
    }
}

/// This computer once it obeys `claim` (which `judge` allowed): it follows that
/// primary, remembers its term, and a primary that sees a newer one becomes
/// the backup.
pub fn obey(me: &Identity, claim: &Claim) -> Identity {
    let mut next = me.clone();
    if me.network.is_some() {
        next.primary = Some(claim.from);
        next.primary_name = Some(claim.from_name.clone());
        next.term = me.term.max(claim.term);
        if me.role == Role::Primary {
            next.role = Role::Backup;
        }
    }
    next
}

/// This computer adopted into `claim`'s network (which `judge` allowed): it
/// takes the network's passcode key (or none) and the role given, a display
/// unless it was asked to be the backup.
pub fn adopt(
    me: &Identity,
    claim: &Claim,
    key: Option<Key>,
    role: Role,
) -> Result<Identity, Refusal> {
    judge(me, claim)?;
    Ok(Identity {
        role: if role == Role::Backup {
            Role::Backup
        } else {
            Role::Display
        },
        network: Some(claim.network),
        primary: Some(claim.from),
        primary_name: Some(claim.from_name.clone()),
        passcode: key,
        term: me.term.max(claim.term),
        ..me.clone()
    })
}

/// Back to waiting for a primary (still switched on): no network, role,
/// primary or passcode.
pub fn release(me: &Identity) -> Identity {
    Identity {
        role: Role::Alone,
        network: None,
        primary: None,
        primary_name: None,
        passcode: None,
        term: 0,
        ..me.clone()
    }
}

/// The role the primary gives: a display or the backup, nothing else.
pub fn set_role(me: &Identity, role: Role) -> Option<Identity> {
    (me.adopted() && Role::GIVEN.contains(&role)).then(|| Identity { role, ..me.clone() })
}

/// A new name; an empty one is the computer's own name again.
pub fn rename(me: &Identity, name: &str) -> Identity {
    let name = name.trim();
    Identity {
        name: (!name.is_empty()).then(|| name.to_string()),
        ..me.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "newq-node-{name}-{}-{}",
            std::process::id(),
            random_u64()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn on() -> Identity {
        switch_on(&Identity::fresh())
    }

    fn claim(from: u64, network: u64, term: u64, proof: Option<bool>) -> Claim {
        Claim {
            from: NodeId(from),
            from_name: format!("P{from}"),
            network: NetworkId(network),
            term,
            proof,
        }
    }

    #[test]
    fn ids_are_hex_and_random() {
        let a = NodeId::random();
        let b = NodeId::random();
        assert_ne!(a, b);
        assert_eq!(a.to_string().len(), 16);
        assert_eq!(a.to_string().parse::<NodeId>(), Ok(a));
        assert_eq!(
            serde_json::to_string(&NodeId(0xab)).unwrap(),
            "\"00000000000000ab\""
        );
        assert!("zz".parse::<NodeId>().is_err());
    }

    #[test]
    fn the_identity_is_kept_and_read_back() {
        let dir = temp("roundtrip");
        let first = load(&dir);
        assert_eq!(first.role, Role::Alone);
        assert!(!first.on, "off by default");
        assert!(dir.join(FILE).exists(), "the new id is kept at once");
        assert_eq!(load(&dir), first, "the same id each start");
        let mut me = become_primary(&first);
        me.passcode = Some(Key::from_passcode("abc"));
        me.name = Some("Booth".into());
        save(&dir, &me).unwrap();
        assert_eq!(load(&dir), me);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unreadable_or_odd_file_is_a_computer_alone_never_a_crash() {
        let dir = temp("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE), b"{ not json").unwrap();
        let me = load(&dir);
        assert_eq!(me.role, Role::Alone);
        assert!(!me.on);
        assert!(dir.join("node.unreadable.json").exists(), "put aside");
        assert_eq!(load(&dir), me, "and the new one kept");
        // A file from a newer NewQ with things this one doesn't know: read
        // what it can.
        std::fs::write(
            dir.join(FILE),
            br#"{"id":"00000000000000aa","role":"Display","on":true,"later":5}"#,
        )
        .unwrap();
        let me = load(&dir);
        assert_eq!((me.id, me.role, me.on), (NodeId(0xaa), Role::Display, true));
        // A role this NewQ doesn't know isn't a crash either.
        std::fs::write(
            dir.join(FILE),
            br#"{"id":"00000000000000aa","role":"Lighting"}"#,
        )
        .unwrap();
        let me = load(&dir);
        assert_eq!((me.id, me.role), (NodeId(0xaa), Role::Alone));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_first_primary_wins_and_another_network_is_refused() {
        let me = on();
        assert_eq!(
            judge(&Identity::fresh(), &claim(1, 10, 1, None)),
            Err(Refusal::Off)
        );
        assert_eq!(judge(&me, &claim(1, 10, 1, None)), Ok(()));
        let me = adopt(&me, &claim(1, 10, 1, None), None, Role::Display).unwrap();
        assert_eq!(me.role, Role::Display);
        assert_eq!(me.network, Some(NetworkId(10)));
        assert_eq!(me.primary, Some(NodeId(1)));
        assert_eq!(
            judge(&me, &claim(2, 11, 5, None)),
            Err(Refusal::OtherNetwork)
        );
        assert_eq!(
            adopt(&me, &claim(2, 11, 5, None), None, Role::Display),
            Err(Refusal::OtherNetwork)
        );
        // Asked to be the primary, it is a display: only the backup can be asked for.
        let b = adopt(&on(), &claim(1, 10, 1, None), None, Role::Primary).unwrap();
        assert_eq!(b.role, Role::Display);
        let b = adopt(&on(), &claim(1, 10, 1, None), None, Role::Backup).unwrap();
        assert_eq!(b.role, Role::Backup);
    }

    #[test]
    fn a_passcode_is_needed_once_there_is_one() {
        let key = Key::from_passcode(" opening night ");
        assert_eq!(key, Key::from_passcode("opening night"));
        assert_ne!(key, Key::from_passcode("Opening night"));
        // Adopted with a passcode: a primary without it, or with a wrong one, is refused.
        let me = adopt(&on(), &claim(1, 10, 1, None), Some(key), Role::Display).unwrap();
        assert_eq!(judge(&me, &claim(1, 10, 1, None)), Err(Refusal::Passcode));
        assert_eq!(
            judge(&me, &claim(1, 10, 1, Some(false))),
            Err(Refusal::Passcode)
        );
        assert_eq!(judge(&me, &claim(1, 10, 1, Some(true))), Ok(()));
        // A passcode typed before joining: only a primary that has it adopts.
        let mut waiting = on();
        waiting.passcode = Some(key);
        assert_eq!(
            judge(&waiting, &claim(1, 10, 1, None)),
            Err(Refusal::Passcode)
        );
        assert_eq!(judge(&waiting, &claim(1, 10, 1, Some(true))), Ok(()));
    }

    #[test]
    fn proofs_check_the_key_the_nonce_and_who_sent_it() {
        let key = Key::from_passcode("x");
        let p = key.proof(42, NodeId(7));
        assert!(key.checks(&p, 42, NodeId(7)));
        assert!(!key.checks(&p, 43, NodeId(7)), "another session's nonce");
        assert!(!key.checks(&p, 42, NodeId(8)), "sent by another computer");
        assert!(!Key::from_passcode("y").checks(&p, 42, NodeId(7)));
        assert!(!key.checks("", 42, NodeId(7)));
        let json = serde_json::to_string(&key).unwrap();
        assert_eq!(serde_json::from_str::<Key>(&json).unwrap(), key);
        assert!(serde_json::from_str::<Key>("\"abcd\"").is_err());
    }

    #[test]
    fn release_sends_it_back_to_waiting() {
        let me = adopt(
            &on(),
            &claim(1, 10, 3, None),
            Some(Key::from_passcode("p")),
            Role::Display,
        )
        .unwrap();
        let free = release(&me);
        assert_eq!(free.role, Role::Alone);
        assert!(free.on, "still switched on");
        assert_eq!(
            (free.network, free.primary, free.passcode),
            (None, None, None)
        );
        // Any primary may adopt it again.
        assert_eq!(judge(&free, &claim(2, 11, 1, None)), Ok(()));
        // Switching off leaves the network; a primary keeps its own.
        let off = switch_off(&me);
        assert!(!off.on && off.network.is_none());
        let p = become_primary(&on());
        let p_off = switch_off(&p);
        assert!(!p_off.on && p_off.network == p.network && p_off.role == Role::Primary);
        assert_eq!(
            become_primary(&p_off).network,
            p.network,
            "on again: the same network"
        );
    }

    #[test]
    fn a_higher_term_wins_and_an_old_primary_is_ignored() {
        let me = adopt(&on(), &claim(1, 10, 2, None), None, Role::Display).unwrap();
        assert_eq!(me.term, 2);
        // A newer primary of the same network (the backup that took over).
        let c = claim(3, 10, 3, None);
        assert_eq!(judge(&me, &c), Ok(()));
        let me = obey(&me, &c);
        assert_eq!((me.primary, me.term), (Some(NodeId(3)), 3));
        // The old primary comes back with its old term: ignored.
        assert_eq!(
            judge(&me, &claim(1, 10, 2, None)),
            Err(Refusal::OldTerm { seen: 3 })
        );
        // The old primary itself, seeing the newer one, becomes the backup;
        // it never obeys one of its own term or older.
        let mut old = become_primary(&on());
        old.term = 2;
        let n = old.network.unwrap().0;
        assert_eq!(
            judge(&old, &claim(9, n, 2, None)),
            Err(Refusal::OldTerm { seen: 2 })
        );
        let newer = claim(9, n, 3, None);
        assert_eq!(judge(&old, &newer), Ok(()));
        let now = obey(&old, &newer);
        assert_eq!(
            (now.role, now.term, now.primary),
            (Role::Backup, 3, Some(NodeId(9)))
        );
    }

    #[test]
    fn the_primary_gives_only_display_and_backup_and_names() {
        let me = adopt(&on(), &claim(1, 10, 1, None), None, Role::Display).unwrap();
        assert_eq!(set_role(&me, Role::Backup).unwrap().role, Role::Backup);
        assert!(set_role(&me, Role::Primary).is_none());
        assert!(set_role(&me, Role::Alone).is_none());
        assert!(set_role(&on(), Role::Display).is_none(), "not adopted");
        assert_eq!(
            rename(&me, " Truss left ").name.as_deref(),
            Some("Truss left")
        );
        assert_eq!(rename(&me, "  ").name, None);
    }

    #[test]
    fn the_command_line_asks_to_join_or_to_run_the_network() {
        let a = |v: &[&str]| startup_flag(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(a(&[]), None);
        assert_eq!(a(&["--join"]), Some(Startup::Join));
        assert_eq!(a(&["show.newq", "--primary"]), Some(Startup::Primary));
        let dir = temp("flags");
        apply_startup(Startup::Join, &dir);
        let me = load(&dir);
        assert!(me.on && me.role == Role::Alone);
        apply_startup(Startup::Primary, &dir);
        let p = load(&dir);
        assert_eq!((p.role, p.id), (Role::Primary, me.id));
        // An adopted computer isn't made a primary by the command line.
        let adopted = adopt(&me, &claim(1, 10, 1, None), None, Role::Display).unwrap();
        save(&dir, &adopted).unwrap();
        assert!(apply_startup(Startup::Primary, &dir).contains("release it"));
        assert_eq!(load(&dir).role, Role::Display);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
