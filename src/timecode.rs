//! MIDI Time Code: reading it, and deciding which cues it fires.
//!
//! MTC arrives as quarter-frame messages, four per frame, each carrying one
//! nibble of the position; eight of them give the whole time (and the frame
//! rate). A "full frame" sysex message says where a generator has just
//! located to. Everything here is plain functions and data, so it is tested
//! with synthesised streams and no MIDI hardware.

use crate::show::CueId;
use serde::{Deserialize, Serialize};
use std::fmt;

/// A position as hours, minutes, seconds and frames. Derives `Ord` so that
/// positions compare in time order whatever the frame rate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timecode {
    pub hours: u8,
    pub minutes: u8,
    pub seconds: u8,
    pub frames: u8,
}

impl Timecode {
    pub fn new(hours: u8, minutes: u8, seconds: u8, frames: u8) -> Timecode {
        Timecode {
            hours,
            minutes,
            seconds,
            frames,
        }
    }

    /// Read "01:02:03:04" (also with `.` or `;` between the parts).
    pub fn parse(text: &str) -> Option<Timecode> {
        let parts: Vec<u8> = text
            .trim()
            .split([':', '.', ';'])
            .map(|p| p.trim().parse().ok())
            .collect::<Option<_>>()?;
        match parts[..] {
            [h, m, s, f] if h < 24 && m < 60 && s < 60 && f < 30 => Some(Timecode::new(h, m, s, f)),
            _ => None,
        }
    }

    /// Frames since 00:00:00:00 at `fps` frames a second (a whole number).
    pub fn frame_number(self, fps: u32) -> u64 {
        ((u64::from(self.hours) * 60 + u64::from(self.minutes)) * 60 + u64::from(self.seconds))
            * u64::from(fps)
            + u64::from(self.frames)
    }

    /// The position `n` frames later, at `fps`, within the day.
    fn plus_frames(self, n: u32, fps: u32) -> Timecode {
        let day = 24 * 3600 * u64::from(fps);
        let total = (self.frame_number(fps) + u64::from(n)) % day;
        let f = u64::from(fps);
        Timecode {
            hours: (total / (f * 3600)) as u8,
            minutes: (total / (f * 60) % 60) as u8,
            seconds: (total / f % 60) as u8,
            frames: (total % f) as u8,
        }
    }
}

impl fmt::Display for Timecode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:02}:{:02}:{:02}:{:02}",
            self.hours, self.minutes, self.seconds, self.frames
        )
    }
}

// Shown in the show file as "01:02:03:04".
impl Serialize for Timecode {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Timecode {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Timecode, D::Error> {
        let text = String::deserialize(d)?;
        Timecode::parse(&text).ok_or_else(|| serde::de::Error::custom("not a timecode"))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rate {
    Fps24,
    #[default]
    Fps25,
    /// 29.97 frames a second, drop frame.
    Fps2997,
    Fps30,
}

impl Rate {
    /// Frames a second as counted on the clock (29.97 counts 30).
    pub fn nominal(self) -> u32 {
        match self {
            Rate::Fps24 => 24,
            Rate::Fps25 => 25,
            Rate::Fps2997 | Rate::Fps30 => 30,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Rate::Fps24 => "24 fps",
            Rate::Fps25 => "25 fps",
            Rate::Fps2997 => "29.97 fps",
            Rate::Fps30 => "30 fps",
        }
    }

    pub const ALL: [Rate; 4] = [Rate::Fps24, Rate::Fps25, Rate::Fps2997, Rate::Fps30];

    /// The two bits that say the rate in a quarter-frame or full-frame message.
    fn bits(self) -> u8 {
        match self {
            Rate::Fps24 => 0,
            Rate::Fps25 => 1,
            Rate::Fps2997 => 2,
            Rate::Fps30 => 3,
        }
    }

    /// Seconds from one frame to the next as the clock really runs (29.97 is
    /// 30000/1001; it is counted without dropping frame numbers).
    pub fn frame_seconds(self) -> f64 {
        match self {
            Rate::Fps24 => 1.0 / 24.0,
            Rate::Fps25 => 1.0 / 25.0,
            Rate::Fps2997 => 1001.0 / 30000.0,
            Rate::Fps30 => 1.0 / 30.0,
        }
    }

    fn from_bits(bits: u8) -> Rate {
        match bits & 3 {
            0 => Rate::Fps24,
            1 => Rate::Fps25,
            2 => Rate::Fps2997,
            _ => Rate::Fps30,
        }
    }
}

/// Assembles the quarter-frame messages of one stream.
#[derive(Default)]
pub struct QuarterFrames {
    nibbles: [u8; 8],
    /// The piece that should come next, if the stream is in step.
    next: Option<u8>,
}

impl QuarterFrames {
    /// Feed the data byte of a quarter-frame message (`F1 dd`). Returns the
    /// position when a run of eight, 0 to 7, completes.
    pub fn push(&mut self, byte: u8) -> Option<(Timecode, Rate)> {
        let piece = (byte >> 4) & 7;
        self.nibbles[usize::from(piece)] = byte & 0x0F;
        // Pieces must arrive in order; a gap or a start in the middle means
        // waiting for the next piece 0.
        let in_step = match self.next {
            Some(n) => piece == n,
            None => piece == 0,
        };
        if !in_step {
            self.next = (piece == 0).then_some(1);
            return None;
        }
        if piece < 7 {
            self.next = Some(piece + 1);
            return None;
        }
        self.next = Some(0);
        let n = &self.nibbles;
        let rate = Rate::from_bits(n[7] >> 1);
        let tc = Timecode {
            hours: ((n[7] & 1) << 4) | n[6],
            minutes: (n[5] << 4) | n[4],
            seconds: (n[3] << 4) | n[2],
            frames: (n[1] << 4) | n[0],
        };
        // The nibbles describe the frame where the run began; it is two frames on now.
        valid(tc, rate).then(|| (tc.plus_frames(2, rate.nominal()), rate))
    }
}

fn valid(tc: Timecode, rate: Rate) -> bool {
    tc.hours < 24 && tc.minutes < 60 && tc.seconds < 60 && u32::from(tc.frames) < rate.nominal()
}

/// A "full frame" message, `F0 7F <device> 01 01 hr mn sc fr F7`: where a
/// generator has located to.
pub fn parse_full_frame(msg: &[u8]) -> Option<(Timecode, Rate)> {
    if msg.len() < 10 || msg[0] != 0xF0 || msg[1] != 0x7F || msg[3] != 0x01 || msg[4] != 0x01 {
        return None;
    }
    let rate = Rate::from_bits(msg[5] >> 5);
    let tc = Timecode {
        hours: msg[5] & 0x1F,
        minutes: msg[6],
        seconds: msg[7],
        frames: msg[8],
    };
    valid(tc, rate).then_some((tc, rate))
}

/// Makes the messages of MIDI Time Code from a start position: a full frame
/// message to say where it begins, then quarter-frame messages, four a frame.
pub struct Generator {
    start: Timecode,
    rate: Rate,
    /// Quarter frames made so far.
    made: u64,
}

impl Generator {
    pub fn new(start: Timecode, rate: Rate) -> Generator {
        Generator {
            start,
            rate,
            made: 0,
        }
    }

    /// Time from one quarter frame to the next.
    pub fn interval(&self) -> f64 {
        self.rate.frame_seconds() / 4.0
    }

    /// The sysex that says the clock is at the start (sent before it runs).
    pub fn full_frame(&self) -> Vec<u8> {
        let t = self.start;
        vec![
            0xF0,
            0x7F,
            0x7F,
            0x01,
            0x01,
            (self.rate.bits() << 5) | (t.hours & 0x1F),
            t.minutes,
            t.seconds,
            t.frames,
            0xF7,
        ]
    }

    /// The next quarter-frame message, `[0xF1, data]`. A run of eight says
    /// the frame where the run began, which is two frames behind when it ends.
    pub fn next_quarter(&mut self) -> [u8; 2] {
        let piece = (self.made % 8) as u8;
        let run = self.made / 8;
        let t = self
            .start
            .plus_frames((run * 2) as u32, self.rate.nominal());
        self.made += 1;
        let data = match piece {
            0 => t.frames & 0xF,
            1 => t.frames >> 4,
            2 => t.seconds & 0xF,
            3 => t.seconds >> 4,
            4 => t.minutes & 0xF,
            5 => t.minutes >> 4,
            6 => t.hours & 0xF,
            _ => (self.rate.bits() << 1) | (t.hours >> 4),
        };
        [0xF1, (piece << 4) | data]
    }
}

/// A position further on than this many seconds is a locate, not playing.
const JUMP_SECONDS: u64 = 1;
/// After a start from nowhere, cues this many frames behind the position still fire.
const START_WINDOW: u64 = 4;

/// Follows the incoming position and says which cues it passes.
#[derive(Default)]
pub struct Follower {
    /// Where the clock last was, and whether a cue exactly there is yet to fire.
    position: Option<(Timecode, Rate)>,
    include_here: bool,
    /// Seconds since a message last arrived.
    silent: f64,
}

impl Follower {
    pub fn position(&self) -> Option<(Timecode, Rate)> {
        self.position
    }

    /// Time passing with or without messages (the engine's tick).
    pub fn tick(&mut self, dt: f64) {
        self.silent += dt;
    }

    /// Whether messages are arriving (they stop when the source stops).
    pub fn running(&self) -> bool {
        self.position.is_some() && self.silent < 0.5
    }

    /// The source located somewhere (a full frame message): nothing is fired,
    /// but playing on from exactly here will fire a cue set for this very frame.
    pub fn locate(&mut self, tc: Timecode, rate: Rate) {
        self.position = Some((tc, rate));
        self.include_here = true;
        self.silent = 0.0;
    }

    /// The clock is now at `tc`. Returns the cues, from `triggers`, that it has
    /// just reached: those after the last position up to this one when playing
    /// on from it, and a window behind it when starting from nowhere or after
    /// a jump (so playing from the top does fire the first cue).
    pub fn advance(
        &mut self,
        triggers: &[(CueId, Timecode)],
        tc: Timecode,
        rate: Rate,
    ) -> Vec<CueId> {
        let fps = u64::from(rate.nominal());
        let here = tc.frame_number(rate.nominal());
        let fired: Vec<(Timecode, CueId)> = match self.position {
            Some((prev, _))
                if prev <= tc && here - prev.frame_number(rate.nominal()) <= JUMP_SECONDS * fps =>
            {
                triggers
                    .iter()
                    .filter(|(_, t)| (*t > prev || (self.include_here && *t == prev)) && *t <= tc)
                    .map(|(id, t)| (*t, *id))
                    .collect()
            }
            _ => {
                let from = here.saturating_sub(START_WINDOW);
                triggers
                    .iter()
                    .filter(|(_, t)| {
                        let n = t.frame_number(rate.nominal());
                        n >= from && n <= here
                    })
                    .map(|(id, t)| (*t, *id))
                    .collect()
            }
        };
        self.position = Some((tc, rate));
        self.include_here = false;
        self.silent = 0.0;
        let mut fired = fired;
        fired.sort_by_key(|(t, _)| *t);
        fired.into_iter().map(|(_, id)| id).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tc(h: u8, m: u8, s: u8, f: u8) -> Timecode {
        Timecode::new(h, m, s, f)
    }

    /// The eight quarter-frame data bytes that say `t` at `rate`.
    fn quarter_frames(t: Timecode, rate: Rate) -> [u8; 8] {
        let bits = match rate {
            Rate::Fps24 => 0,
            Rate::Fps25 => 1,
            Rate::Fps2997 => 2,
            Rate::Fps30 => 3,
        };
        [
            t.frames & 0xF,
            0x10 | (t.frames >> 4),
            0x20 | (t.seconds & 0xF),
            0x30 | (t.seconds >> 4),
            0x40 | (t.minutes & 0xF),
            0x50 | (t.minutes >> 4),
            0x60 | (t.hours & 0xF),
            0x70 | (bits << 1) | (t.hours >> 4),
        ]
    }

    #[test]
    fn timecodes_read_and_print() {
        assert_eq!(Timecode::parse("01:02:03:04"), Some(tc(1, 2, 3, 4)));
        assert_eq!(Timecode::parse(" 10.00.05.29 "), Some(tc(10, 0, 5, 29)));
        assert_eq!(Timecode::parse("1;2;3;4"), Some(tc(1, 2, 3, 4)));
        for bad in [
            "",
            "1:2:3",
            "1:2:3:4:5",
            "24:00:00:00",
            "0:60:0:0",
            "0:0:60:0",
            "0:0:0:30",
            "a:b:c:d",
        ] {
            assert_eq!(Timecode::parse(bad), None, "{bad:?}");
        }
        assert_eq!(tc(1, 2, 3, 4).to_string(), "01:02:03:04");
        assert!(tc(0, 59, 59, 24) < tc(1, 0, 0, 0));
        assert!(tc(1, 0, 0, 9) < tc(1, 0, 1, 0));
    }

    #[test]
    fn timecodes_are_strings_in_the_show_file() {
        let json = serde_json::to_string(&tc(1, 0, 5, 12)).unwrap();
        assert_eq!(json, "\"01:00:05:12\"");
        assert_eq!(
            serde_json::from_str::<Timecode>(&json).unwrap(),
            tc(1, 0, 5, 12)
        );
        assert!(serde_json::from_str::<Timecode>("\"nonsense\"").is_err());
    }

    #[test]
    fn eight_quarter_frames_give_the_position_two_frames_on() {
        for (t, rate) in [
            (tc(1, 2, 3, 4), Rate::Fps25),
            (tc(0, 0, 0, 0), Rate::Fps24),
            (tc(23, 59, 59, 20), Rate::Fps30),
            (tc(10, 30, 15, 10), Rate::Fps2997),
        ] {
            let mut q = QuarterFrames::default();
            let got: Vec<_> = quarter_frames(t, rate)
                .iter()
                .filter_map(|&b| q.push(b))
                .collect();
            assert_eq!(got.len(), 1, "{t} gives one position");
            assert_eq!(got[0], (t.plus_frames(2, rate.nominal()), rate));
        }
        // The two frames carry into the seconds, and past midnight.
        let mut q = QuarterFrames::default();
        let last = quarter_frames(tc(0, 0, 0, 23), Rate::Fps24);
        assert_eq!(
            last.iter().filter_map(|&b| q.push(b)).next(),
            Some((tc(0, 0, 1, 1), Rate::Fps24))
        );
        let mut q = QuarterFrames::default();
        let last = quarter_frames(tc(23, 59, 59, 28), Rate::Fps30);
        assert_eq!(
            last.iter().filter_map(|&b| q.push(b)).next(),
            Some((tc(0, 0, 0, 0), Rate::Fps30))
        );
    }

    #[test]
    fn a_stream_gives_a_position_every_two_frames() {
        let mut q = QuarterFrames::default();
        let mut positions = Vec::new();
        for start in [0u8, 2, 4, 6] {
            for b in quarter_frames(tc(0, 0, 1, start), Rate::Fps25) {
                positions.extend(q.push(b));
            }
        }
        let frames: Vec<u8> = positions.iter().map(|(t, _)| t.frames).collect();
        assert_eq!(frames, [2, 4, 6, 8]);
    }

    #[test]
    fn a_stream_joined_late_or_with_a_gap_waits_for_the_next_run() {
        let mut q = QuarterFrames::default();
        let msgs = quarter_frames(tc(0, 0, 1, 10), Rate::Fps25);
        // Starting at piece 3: no position until a whole run from piece 0.
        assert!(msgs[3..].iter().all(|&b| q.push(b).is_none()));
        let full = quarter_frames(tc(0, 0, 1, 12), Rate::Fps25);
        assert_eq!(full.iter().filter_map(|&b| q.push(b)).count(), 1);
        // A dropped message spoils the run in progress, not the next one.
        let mut q = QuarterFrames::default();
        let run = quarter_frames(tc(0, 0, 2, 0), Rate::Fps25);
        let got = run
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != 4)
            .filter_map(|(_, &b)| q.push(b))
            .count();
        assert_eq!(got, 0);
        let again = quarter_frames(tc(0, 0, 2, 2), Rate::Fps25);
        assert_eq!(again.iter().filter_map(|&b| q.push(b)).count(), 1);
    }

    #[test]
    fn nonsense_positions_are_dropped() {
        let mut q = QuarterFrames::default();
        // Frame 29 at 25 fps can't happen.
        let mut bad = quarter_frames(tc(0, 0, 0, 0), Rate::Fps25);
        bad[0] = 0xD & 0xF;
        bad[1] = 0x11;
        assert_eq!(bad.iter().filter_map(|&b| q.push(b)).count(), 0);
    }

    #[test]
    fn full_frame_messages_say_where_the_source_located() {
        // F0 7F 7F 01 01 hr mn sc fr F7, with the rate in the top of hr (1 = 25 fps).
        let msg = [0xF0, 0x7F, 0x7F, 0x01, 0x01, 0x20 | 1, 2, 3, 4, 0xF7];
        assert_eq!(parse_full_frame(&msg), Some((tc(1, 2, 3, 4), Rate::Fps25)));
        let at30 = [0xF0, 0x7F, 0x7F, 0x01, 0x01, 0x60 | 10, 0, 0, 29, 0xF7];
        assert_eq!(
            parse_full_frame(&at30),
            Some((tc(10, 0, 0, 29), Rate::Fps30))
        );
        // Not MTC (that is MIDI Show Control), too short, or an impossible time.
        assert_eq!(
            parse_full_frame(&[0xF0, 0x7F, 0x7F, 0x02, 0x01, 0, 0, 0, 0, 0xF7]),
            None
        );
        assert_eq!(
            parse_full_frame(&[0xF0, 0x7F, 0x7F, 0x01, 0x01, 0, 0]),
            None
        );
        assert_eq!(
            parse_full_frame(&[0xF0, 0x7F, 0x7F, 0x01, 0x01, 0x20, 0, 0, 29, 0xF7]),
            None
        );
    }

    fn cues(times: &[Timecode]) -> Vec<(CueId, Timecode)> {
        times
            .iter()
            .enumerate()
            .map(|(i, t)| (CueId(i as u64 + 1), *t))
            .collect()
    }

    /// Play from `start` to `end` two frames at a time, collecting what fires.
    fn play(
        f: &mut Follower,
        triggers: &[(CueId, Timecode)],
        from: Timecode,
        frames: u32,
        rate: Rate,
    ) -> Vec<CueId> {
        let mut fired = Vec::new();
        let mut t = from;
        for _ in 0..frames / 2 {
            t = t.plus_frames(2, rate.nominal());
            fired.extend(f.advance(triggers, t, rate));
        }
        fired
    }

    #[test]
    fn playing_through_fires_each_cue_once_in_order() {
        let triggers = cues(&[tc(1, 0, 2, 0), tc(1, 0, 1, 0), tc(1, 0, 4, 13)]);
        let mut f = Follower::default();
        f.locate(tc(1, 0, 0, 0), Rate::Fps25);
        let fired = play(&mut f, &triggers, tc(1, 0, 0, 0), 6 * 25, Rate::Fps25);
        assert_eq!(
            fired,
            [CueId(2), CueId(1), CueId(3)],
            "time order, each once"
        );
        // Nothing more as the clock keeps running past them.
        let more = play(&mut f, &triggers, tc(1, 0, 6, 0), 100, Rate::Fps25);
        assert!(more.is_empty());
    }

    #[test]
    fn a_cue_on_the_frame_the_source_started_from_fires() {
        let triggers = cues(&[tc(1, 0, 0, 0)]);
        // Located there, then played: fires.
        let mut f = Follower::default();
        f.locate(tc(1, 0, 0, 0), Rate::Fps25);
        assert!(f.advance(&triggers, tc(1, 0, 0, 2), Rate::Fps25) == vec![CueId(1)]);
        assert!(f.advance(&triggers, tc(1, 0, 0, 4), Rate::Fps25).is_empty());
        // No locate message (a source that only sends quarter frames): the
        // first position is two frames on, and the window behind it still fires it.
        let mut f = Follower::default();
        assert_eq!(
            f.advance(&triggers, tc(1, 0, 0, 2), Rate::Fps25),
            vec![CueId(1)]
        );
        assert!(f.advance(&triggers, tc(1, 0, 0, 4), Rate::Fps25).is_empty());
    }

    #[test]
    fn locating_past_cues_does_not_fire_them() {
        let triggers = cues(&[tc(1, 0, 1, 0), tc(1, 0, 30, 0), tc(1, 0, 40, 0)]);
        let mut f = Follower::default();
        f.locate(tc(1, 0, 0, 0), Rate::Fps25);
        play(&mut f, &triggers, tc(1, 0, 0, 0), 2 * 25, Rate::Fps25);
        // Jump to 35 s: the cue at 30 s was skipped, not played.
        assert!(
            f.advance(&triggers, tc(1, 0, 35, 0), Rate::Fps25)
                .is_empty()
        );
        // Playing on from there reaches the cue at 40 s.
        let fired = play(&mut f, &triggers, tc(1, 0, 35, 0), 6 * 25, Rate::Fps25);
        assert_eq!(fired, [CueId(3)]);
    }

    #[test]
    fn rewinding_and_playing_again_fires_the_cues_again() {
        let triggers = cues(&[tc(1, 0, 1, 0)]);
        let mut f = Follower::default();
        f.locate(tc(1, 0, 0, 0), Rate::Fps25);
        assert_eq!(
            play(&mut f, &triggers, tc(1, 0, 0, 0), 2 * 25, Rate::Fps25),
            [CueId(1)]
        );
        // Back to the start (a locate), and again.
        f.locate(tc(1, 0, 0, 0), Rate::Fps25);
        assert_eq!(
            play(&mut f, &triggers, tc(1, 0, 0, 0), 2 * 25, Rate::Fps25),
            [CueId(1)]
        );
        // A backwards step while running re-syncs without firing anything.
        assert!(
            f.advance(&triggers, tc(1, 0, 0, 10), Rate::Fps25)
                .is_empty()
        );
    }

    #[test]
    fn stopping_and_resuming_in_place_does_not_fire_twice() {
        let triggers = cues(&[tc(1, 0, 1, 0)]);
        let mut f = Follower::default();
        f.locate(tc(1, 0, 0, 0), Rate::Fps25);
        let first = play(&mut f, &triggers, tc(1, 0, 0, 0), 2 * 25, Rate::Fps25);
        assert_eq!(first, [CueId(1)]);
        let stopped_at = f.position().unwrap().0;
        // The source stops for a while...
        f.tick(0.4);
        assert!(f.running());
        f.tick(0.4);
        assert!(!f.running(), "no messages for half a second means stopped");
        // ...and resumes where it was.
        assert!(
            f.advance(&triggers, stopped_at.plus_frames(2, 25), Rate::Fps25)
                .is_empty()
        );
        assert!(f.running());
    }

    #[test]
    fn the_clock_wrapping_past_midnight_is_a_jump() {
        let triggers = cues(&[tc(0, 0, 0, 5)]);
        let mut f = Follower::default();
        f.advance(&triggers, tc(23, 59, 59, 20), Rate::Fps25);
        // Backwards in the tuple order: resyncs, and the window behind it fires.
        assert_eq!(
            f.advance(&triggers, tc(0, 0, 0, 6), Rate::Fps25),
            vec![CueId(1)]
        );
    }

    #[test]
    fn rates_count_frames_as_the_clock_does() {
        assert_eq!(tc(0, 0, 1, 0).frame_number(24), 24);
        assert_eq!(tc(0, 0, 1, 0).frame_number(30), 30);
        assert_eq!(tc(0, 1, 0, 0).frame_number(Rate::Fps2997.nominal()), 1800);
        assert_eq!(Rate::Fps2997.label(), "29.97 fps");
    }

    #[test]
    fn the_generators_messages_are_read_back_by_our_own_decoder() {
        for rate in Rate::ALL {
            let start = tc(1, 59, 58, 4);
            let mut g = Generator::new(start, rate);
            assert_eq!(parse_full_frame(&g.full_frame()), Some((start, rate)));
            let mut q = QuarterFrames::default();
            let mut seen = Vec::new();
            for _ in 0..8 * 40 {
                let m = g.next_quarter();
                assert_eq!(m[0], 0xF1);
                assert!(m[1] < 0x80);
                if let Some(p) = q.push(m[1]) {
                    seen.push(p);
                }
            }
            // A run ends 7 quarter frames in and says the position two frames on
            // from where it began; the next run is two frames after that.
            assert_eq!(seen.len(), 40, "{rate:?}");
            for (n, (t, r)) in seen.iter().enumerate() {
                assert_eq!(*r, rate);
                assert_eq!(
                    *t,
                    start.plus_frames(2 * n as u32 + 2, rate.nominal()),
                    "{n}"
                );
            }
            // It crossed a minute and an hour boundary without a wrong digit.
            assert!(seen.iter().any(|(t, _)| t.hours == 2));
        }
    }

    #[test]
    fn a_quarter_frame_comes_every_quarter_of_a_frame() {
        let g = Generator::new(tc(0, 0, 0, 0), Rate::Fps25);
        assert!((g.interval() - 0.01).abs() < 1e-12);
        let g = Generator::new(tc(0, 0, 0, 0), Rate::Fps2997);
        assert!((g.interval() - 1001.0 / 120000.0).abs() < 1e-12);
    }
}
