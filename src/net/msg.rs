//! The messages between computers: the clock exchange, commands timed by the
//! shared clock, their acknowledgements and a state heartbeat. A compact
//! binary form: `NQ`, a version byte, a kind byte, then the fields in little
//! endian. Later stages add kinds; a reader refuses a version or kind it
//! doesn't know rather than guessing.

use crate::show::CueId;
use crate::timeline::TimelineKey;

const MAGIC: [u8; 2] = *b"NQ";
pub const VERSION: u8 = 1;
/// The most a message may be (a heartbeat's lists are cut to fit).
pub const MAX_LEN: usize = 1200;

#[derive(Clone, Debug, PartialEq)]
pub enum Msg {
    /// A follower asks for the time: `t1` by its own clock.
    ClockAsk { seq: u32, t1: f64 },
    /// The primary's answer: `t1` back, and when it received (`t2`) and sent
    /// (`t3`) by the shared clock.
    ClockReply { seq: u32, t1: f64, t2: f64, t3: f64 },
    /// Do `what` at shared time `at`. Sent again until acknowledged; `seq`
    /// makes a repeat harmless.
    Command { seq: u32, at: f64, what: Command },
    /// A command arrived.
    Ack { seq: u32 },
    /// What a computer is doing, by the shared clock at `time`.
    Heartbeat(Heartbeat),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    /// GO on the cue list being worked.
    Go,
    /// Fire one cue.
    Fire(CueId),
    /// Stop everything.
    StopAll,
    TimelineGo(TimelineKey),
    TimelinePause(TimelineKey),
    /// Put a timeline at `pos` (seconds on its clock).
    TimelineSeek(TimelineKey, f64),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Heartbeat {
    pub node: u16,
    pub time: f64,
    /// Whether its shared clock is locked, and the error it believes (seconds).
    pub locked: bool,
    pub error: f32,
    /// Cues playing: the cue and its own time.
    pub cues: Vec<(CueId, f64)>,
    /// Timelines: which, running or not, and the clock.
    pub runs: Vec<(TimelineKey, bool, f64)>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum BadMsg {
    Short,
    NotOurs,
    Version(u8),
    Kind(u8),
    Long,
}

impl Msg {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Vec::with_capacity(48);
        w.extend_from_slice(&MAGIC);
        w.push(VERSION);
        match self {
            Msg::ClockAsk { seq, t1 } => {
                w.push(1);
                put_u32(&mut w, *seq);
                put_f64(&mut w, *t1);
            }
            Msg::ClockReply { seq, t1, t2, t3 } => {
                w.push(2);
                put_u32(&mut w, *seq);
                for t in [t1, t2, t3] {
                    put_f64(&mut w, *t);
                }
            }
            Msg::Command { seq, at, what } => {
                w.push(3);
                put_u32(&mut w, *seq);
                put_f64(&mut w, *at);
                put_command(&mut w, what);
            }
            Msg::Ack { seq } => {
                w.push(4);
                put_u32(&mut w, *seq);
            }
            Msg::Heartbeat(h) => {
                w.push(5);
                w.extend_from_slice(&h.node.to_le_bytes());
                put_f64(&mut w, h.time);
                w.push(h.locked as u8);
                w.extend_from_slice(&h.error.to_le_bytes());
                // Each list as many as fit (a count byte, then the entries).
                let room = |w: &Vec<u8>| MAX_LEN.saturating_sub(w.len() + 2);
                let n = h.cues.len().min(255).min(room(&w) / 16);
                w.push(n as u8);
                for (id, t) in &h.cues[..n] {
                    put_u64(&mut w, id.0);
                    put_f64(&mut w, *t);
                }
                let n = h.runs.len().min(255).min(room(&w) / 18);
                w.push(n as u8);
                for (key, running, pos) in &h.runs[..n] {
                    put_key(&mut w, key);
                    w.push(*running as u8);
                    put_f64(&mut w, *pos);
                }
            }
        }
        w
    }

    pub fn decode(b: &[u8]) -> Result<Msg, BadMsg> {
        if b.len() > MAX_LEN {
            return Err(BadMsg::Long);
        }
        let mut r = Reader { b, at: 0 };
        if r.take(2)? != MAGIC {
            return Err(BadMsg::NotOurs);
        }
        let v = r.u8()?;
        if v != VERSION {
            return Err(BadMsg::Version(v));
        }
        let msg = match r.u8()? {
            1 => Msg::ClockAsk {
                seq: r.u32()?,
                t1: r.f64()?,
            },
            2 => Msg::ClockReply {
                seq: r.u32()?,
                t1: r.f64()?,
                t2: r.f64()?,
                t3: r.f64()?,
            },
            3 => Msg::Command {
                seq: r.u32()?,
                at: r.f64()?,
                what: r.command()?,
            },
            4 => Msg::Ack { seq: r.u32()? },
            5 => {
                let node = u16::from_le_bytes(r.array()?);
                let time = r.f64()?;
                let locked = r.u8()? != 0;
                let error = f32::from_le_bytes(r.array()?);
                let n = r.u8()?;
                let mut cues = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    cues.push((CueId(r.u64()?), r.f64()?));
                }
                let n = r.u8()?;
                let mut runs = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    runs.push((r.key()?, r.u8()? != 0, r.f64()?));
                }
                Msg::Heartbeat(Heartbeat {
                    node,
                    time,
                    locked,
                    error,
                    cues,
                    runs,
                })
            }
            k => return Err(BadMsg::Kind(k)),
        };
        if r.at != b.len() {
            return Err(BadMsg::Long);
        }
        Ok(msg)
    }
}

fn put_u32(w: &mut Vec<u8>, v: u32) {
    w.extend_from_slice(&v.to_le_bytes());
}
fn put_u64(w: &mut Vec<u8>, v: u64) {
    w.extend_from_slice(&v.to_le_bytes());
}
fn put_f64(w: &mut Vec<u8>, v: f64) {
    w.extend_from_slice(&v.to_le_bytes());
}

fn put_key(w: &mut Vec<u8>, key: &TimelineKey) {
    match key {
        TimelineKey::Group(id) => {
            w.push(0);
            put_u64(w, id.0);
        }
        TimelineKey::List(n) => {
            w.push(1);
            put_u64(w, *n as u64);
        }
    }
}

fn put_command(w: &mut Vec<u8>, c: &Command) {
    match c {
        Command::Go => w.push(1),
        Command::Fire(id) => {
            w.push(2);
            put_u64(w, id.0);
        }
        Command::StopAll => w.push(3),
        Command::TimelineGo(k) => {
            w.push(4);
            put_key(w, k);
        }
        Command::TimelinePause(k) => {
            w.push(5);
            put_key(w, k);
        }
        Command::TimelineSeek(k, pos) => {
            w.push(6);
            put_key(w, k);
            put_f64(w, *pos);
        }
    }
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], BadMsg> {
        let s = self.b.get(self.at..self.at + n).ok_or(BadMsg::Short)?;
        self.at += n;
        Ok(s)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], BadMsg> {
        let s = self.take(N)?;
        let mut a = [0u8; N];
        a.copy_from_slice(s);
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8, BadMsg> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, BadMsg> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    fn u64(&mut self) -> Result<u64, BadMsg> {
        Ok(u64::from_le_bytes(self.array()?))
    }
    fn f64(&mut self) -> Result<f64, BadMsg> {
        Ok(f64::from_le_bytes(self.array()?))
    }
    fn key(&mut self) -> Result<TimelineKey, BadMsg> {
        match self.u8()? {
            0 => Ok(TimelineKey::Group(CueId(self.u64()?))),
            1 => {
                let n = self.u64()?;
                Ok(TimelineKey::List(
                    u32::try_from(n).map_err(|_| BadMsg::Kind(1))?,
                ))
            }
            k => Err(BadMsg::Kind(k)),
        }
    }
    fn command(&mut self) -> Result<Command, BadMsg> {
        Ok(match self.u8()? {
            1 => Command::Go,
            2 => Command::Fire(CueId(self.u64()?)),
            3 => Command::StopAll,
            4 => Command::TimelineGo(self.key()?),
            5 => Command::TimelinePause(self.key()?),
            6 => Command::TimelineSeek(self.key()?, self.f64()?),
            k => return Err(BadMsg::Kind(k)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<Msg> {
        let g = TimelineKey::Group(CueId(77));
        let l = TimelineKey::List(3);
        vec![
            Msg::ClockAsk { seq: 1, t1: 12.5 },
            Msg::ClockReply {
                seq: u32::MAX,
                t1: 12.5,
                t2: 1e9 + 0.123_456_789,
                t3: -0.0,
            },
            Msg::Command {
                seq: 9,
                at: 3600.25,
                what: Command::Go,
            },
            Msg::Command {
                seq: 10,
                at: 1.0,
                what: Command::Fire(CueId(u64::MAX)),
            },
            Msg::Command {
                seq: 11,
                at: 1.0,
                what: Command::StopAll,
            },
            Msg::Command {
                seq: 12,
                at: 2.0,
                what: Command::TimelineGo(g),
            },
            Msg::Command {
                seq: 13,
                at: 2.0,
                what: Command::TimelinePause(l),
            },
            Msg::Command {
                seq: 14,
                at: 2.0,
                what: Command::TimelineSeek(l, 61.5),
            },
            Msg::Ack { seq: 14 },
            Msg::Heartbeat(Heartbeat {
                node: 2,
                time: 99.0,
                locked: true,
                error: 0.000_25,
                cues: vec![(CueId(1), 3.5), (CueId(2), 0.0)],
                runs: vec![(g, true, 12.0), (l, false, 0.5)],
            }),
            Msg::Heartbeat(Heartbeat::default()),
        ]
    }

    #[test]
    fn every_message_comes_back_as_it_went() {
        for m in all() {
            let b = m.encode();
            assert!(b.len() <= MAX_LEN);
            assert_eq!(Msg::decode(&b), Ok(m.clone()), "{m:?}");
        }
        // Small: a clock exchange fits in a few dozen bytes.
        assert_eq!(Msg::ClockAsk { seq: 1, t1: 0.0 }.encode().len(), 16);
        assert_eq!(
            Msg::ClockReply {
                seq: 1,
                t1: 0.0,
                t2: 0.0,
                t3: 0.0
            }
            .encode()
            .len(),
            32
        );
    }

    #[test]
    fn what_is_not_a_message_of_ours_is_refused() {
        let good = Msg::Ack { seq: 1 }.encode();
        assert_eq!(Msg::decode(&[]), Err(BadMsg::Short));
        assert_eq!(Msg::decode(b"XX\x01\x04\0\0\0\0"), Err(BadMsg::NotOurs));
        let mut v = good.clone();
        v[2] = 9;
        assert_eq!(Msg::decode(&v), Err(BadMsg::Version(9)));
        let mut k = good.clone();
        k[3] = 200;
        assert_eq!(Msg::decode(&k), Err(BadMsg::Kind(200)));
        assert_eq!(Msg::decode(&good[..good.len() - 1]), Err(BadMsg::Short));
        let mut extra = good;
        extra.push(0);
        assert_eq!(Msg::decode(&extra), Err(BadMsg::Long));
        // Every cut of every message is refused, never misread or a panic.
        for m in all() {
            let b = m.encode();
            for n in 0..b.len() {
                assert!(Msg::decode(&b[..n]).is_err());
            }
        }
    }

    #[test]
    fn a_heartbeat_too_big_is_cut_to_fit() {
        let h = Heartbeat {
            cues: (0..200).map(|i| (CueId(i), i as f64)).collect(),
            runs: (0..200)
                .map(|i| (TimelineKey::List(i), true, 1.0))
                .collect(),
            ..Default::default()
        };
        let b = Msg::Heartbeat(h).encode();
        assert!(b.len() <= MAX_LEN, "{}", b.len());
        let Ok(Msg::Heartbeat(back)) = Msg::decode(&b) else {
            panic!("not read back");
        };
        assert!(!back.cues.is_empty() && back.cues.len() < 200);
    }
}
