//! Where a dragged time or clip snaps to. (Stubs for now: the editing library
//! fills them in.)

use super::Marker;
use super::model::Clip;
use crate::show::CueId;

/// The times things snap to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SnapPoints(pub Vec<f64>);

/// Clip starts and ends, fade-out starts, ends of lead-ins, markers, the
/// playhead; the clips being moved (`moving`) left out.
pub fn points(
    _clips: &[Clip],
    _markers: &[Marker],
    _playhead: Option<f64>,
    _moving: &[CueId],
) -> SnapPoints {
    SnapPoints(Vec::new())
}

/// `t` snapped to the nearest point within `tolerance`: (snapped, the point hit).
pub fn snap(t: f64, _pts: &SnapPoints, _tolerance: f64) -> (f64, Option<f64>) {
    (t, None)
}

/// A span's start snapped so that either of its edges meets a point.
pub fn snap_span(
    start: f64,
    _len: Option<f64>,
    _pts: &SnapPoints,
    _tolerance: f64,
) -> (f64, Option<f64>) {
    (start, None)
}
