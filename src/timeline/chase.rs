//! What a timeline has playing at a given time, worked out from the show by
//! playing it through in the mind: the clips alive there and from where, and
//! those still to come. (A stub for now: the engine's seek fills it in.)

use super::TimelineKey;
use super::model::Dur;
use crate::show::{CueId, Curve, GeometryTarget, Show};

/// A clip alive at the chased time, and how to start it.
#[derive(Clone, Debug, PartialEq)]
pub struct Alive {
    pub id: CueId,
    /// Clip time (`elapsed`) to start at.
    pub from: f64,
    /// Lead-in V; content position counts from here.
    pub lead: f64,
    /// Frozen by a clip Pause cue not yet resumed.
    pub frozen: bool,
    pub detached: bool,
    /// A held video that had ended.
    pub at_last_frame: bool,
    /// (now, to, seconds left, curve) from fades in progress or done.
    pub opacity: Option<(f32, f32, f64, Curve)>,
    pub volume_db: Option<(f32, f32, f64, Curve)>,
    /// End values of finished geometry fades.
    pub geometry: Option<GeometryTarget>,
    /// A fade-and-stop in progress.
    pub stop_after: Option<f64>,
}

/// A clip still to come: at a time, or (`at` None) waiting for a fade cue.
#[derive(Clone, Debug, PartialEq)]
pub struct Waiting {
    pub id: CueId,
    pub at: Option<f64>,
    pub lead: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Chase {
    pub alive: Vec<Alive>,
    pub upcoming: Vec<Waiting>,
    pub passed: Vec<CueId>,
}

pub fn chase(_show: &Show, _key: TimelineKey, _pos: f64, _dur: Dur) -> Chase {
    Chase::default()
}
