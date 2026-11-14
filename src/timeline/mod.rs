//! Timelines: a "start all together" group (not a slide show) or a cue list
//! marked as one, whose clips start at their pre-waits on one clock. This is
//! the data that goes in the show file; `model` reads a timeline out of a show,
//! `edit` and `snap` change one, `chase` works out what plays at a given time.
//! The engine's side is `engine/timeline.rs` and the editor is `ui/timeline.rs`.
//!
//! Everything here is `#[serde(default)]` and left out of the file at its
//! default, so a show that uses no timeline is written as before.

// Most of this is used by the packages that build on it (the editing library,
// the engine's run and the editor); until they land much of it is unused.
#![allow(dead_code)]

pub mod chase;
pub mod edit;
pub mod model;
pub mod snap;

use crate::show::{AudioOutputId, CueColor, CueId, OutputId};
use serde::{Deserialize, Serialize};

/// Which timeline: a timeline group (its clips are its children) or a cue list
/// that is a timeline (its clips are its top-level cues).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TimelineKey {
    Group(CueId),
    List(u32),
}

/// A place on a timeline: a time, a marker, or the start of a clip.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum TimelinePoint {
    Time(f64),
    Marker(u64),
    Cue(CueId),
}

/// What a timeline pause cue does to the content while the clock is stopped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PauseFreeze {
    /// The clips playing go on playing (or looping): only the clock stops.
    #[default]
    Nothing,
    /// Every clip of the timeline holds its frame and its sound stops.
    Everything,
    /// Only the clips the cue names (`ControlCue::clips`) hold.
    Clips,
}

impl PauseFreeze {
    pub fn is_nothing(&self) -> bool {
        *self == PauseFreeze::Nothing
    }
}

/// What a clip whose end is kept by the timeline's clock does when its content
/// runs out before the timeline moves on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunOut {
    /// Ends, fading out over `MediaCue::run_out_fade` (picture and sound alike).
    #[default]
    End,
    /// Goes round again from its in point until the timeline ends it.
    Loop,
    /// Holds its last frame (a sound: silence).
    Hold,
}

impl RunOut {
    pub fn is_end(&self) -> bool {
        *self == RunOut::End
    }
}

/// A row of a timeline. Worked out from the clip (see `model::track_of`),
/// never kept on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TrackId {
    /// Pictures on one display and layer.
    Picture {
        output: Option<OutputId>,
        layer: i32,
    },
    /// Sounds on one audio output.
    Sound { output: Option<AudioOutputId> },
    /// Fade, stop, wait, send, control and timecode cues.
    Cues,
}

/// A named place on a timeline, to jump to. It never holds the clock.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub id: u64,
    pub time: f64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<CueColor>,
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// A timeline's own settings: on a timeline group (`GroupCue::timeline`) or a
/// timeline list (`CueList::timeline`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TimelineSettings {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
    /// Tracks kept though they have no clips, and the order of tracks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tracks: Vec<TrackId>,
    /// Clips that overlap on a track crossfade over the overlap.
    #[serde(default, skip_serializing_if = "is_false")]
    pub crossfade_overlaps: bool,
    /// A clip that is invisible (or silent) when the playhead reaches it waits
    /// for its fade-in before its content starts (see `model::lead_in`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub start_on_fade_in: bool,
}

impl TimelineSettings {
    pub fn is_default(&self) -> bool {
        *self == TimelineSettings::default()
    }

    /// What a timeline made in the editor starts with: both rules on. (An old
    /// group has no settings, so it plays as it always did.)
    pub fn for_editor() -> TimelineSettings {
        TimelineSettings {
            crossfade_overlaps: true,
            start_on_fade_in: true,
            ..TimelineSettings::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_at_their_defaults_are_written_as_nothing_and_the_editors_have_both_flags() {
        let d = TimelineSettings::default();
        assert!(d.is_default());
        assert_eq!(serde_json::to_string(&d).unwrap(), "{}");
        let e = TimelineSettings::for_editor();
        assert!(!e.is_default());
        let json = serde_json::to_string(&e).unwrap();
        assert_eq!(
            json,
            r#"{"crossfade_overlaps":true,"start_on_fade_in":true}"#
        );
        let back: TimelineSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, e);
        let empty: TimelineSettings = serde_json::from_str("{}").unwrap();
        assert!(empty.is_default());
    }

    #[test]
    fn keys_points_tracks_and_markers_round_trip() {
        let s = TimelineSettings {
            markers: vec![
                Marker {
                    id: 1,
                    time: 12.5,
                    name: "Chorus".into(),
                    color: Some(CueColor::Blue),
                },
                Marker {
                    id: 2,
                    time: 30.0,
                    name: "End".into(),
                    color: None,
                },
            ],
            tracks: vec![
                TrackId::Picture {
                    output: Some(OutputId(3)),
                    layer: -1,
                },
                TrackId::Sound { output: None },
                TrackId::Cues,
            ],
            ..TimelineSettings::for_editor()
        };
        let json = serde_json::to_string(&s).unwrap();
        // A marker without a colour doesn't say so.
        assert_eq!(json.matches("color").count(), 1, "{json}");
        assert_eq!(serde_json::from_str::<TimelineSettings>(&json).unwrap(), s);
        for k in [TimelineKey::Group(CueId(7)), TimelineKey::List(2)] {
            let j = serde_json::to_string(&k).unwrap();
            assert_eq!(serde_json::from_str::<TimelineKey>(&j).unwrap(), k);
        }
        for p in [
            TimelinePoint::Time(1.25),
            TimelinePoint::Marker(4),
            TimelinePoint::Cue(CueId(9)),
        ] {
            let j = serde_json::to_string(&p).unwrap();
            assert_eq!(serde_json::from_str::<TimelinePoint>(&j).unwrap(), p);
        }
        for f in [
            PauseFreeze::Nothing,
            PauseFreeze::Everything,
            PauseFreeze::Clips,
        ] {
            let j = serde_json::to_string(&f).unwrap();
            assert_eq!(serde_json::from_str::<PauseFreeze>(&j).unwrap(), f);
        }
        for r in [RunOut::End, RunOut::Loop, RunOut::Hold] {
            let j = serde_json::to_string(&r).unwrap();
            assert_eq!(serde_json::from_str::<RunOut>(&j).unwrap(), r);
        }
        assert!(RunOut::default().is_end() && PauseFreeze::default().is_nothing());
    }
}
