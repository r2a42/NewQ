//! Where a dragged time or clip snaps to: the edges of the other clips, where
//! their fade-outs start and their lead-ins end, the markers and the playhead.

use super::Marker;
use super::model::{Clip, LeadIn};
use crate::show::CueId;

/// The times things snap to, in order, each once.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SnapPoints(pub Vec<f64>);

/// Clip starts and ends, fade-out starts, ends of lead-ins, markers, the
/// playhead; the clips being moved (`moving`) left out.
pub fn points(
    clips: &[Clip],
    markers: &[Marker],
    playhead: Option<f64>,
    moving: &[CueId],
) -> SnapPoints {
    let mut pts: Vec<f64> = Vec::new();
    for c in clips.iter().filter(|c| !moving.contains(&c.id)) {
        pts.push(c.start);
        if let LeadIn::At(v) = c.lead_in
            && v > 0.0
        {
            pts.push(c.start + v);
        }
        if let Some(len) = c.len {
            let end = c.start + len;
            pts.push(end);
            if c.fade_out > 0.0 && c.fade_out < len {
                pts.push(end - c.fade_out);
            }
        }
    }
    pts.extend(markers.iter().map(|m| m.time));
    pts.extend(playhead);
    pts.retain(|t| t.is_finite());
    pts.sort_by(f64::total_cmp);
    pts.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    SnapPoints(pts)
}

/// The point nearest `t` within `tolerance`, with how far it is.
fn nearest(t: f64, pts: &SnapPoints, tolerance: f64) -> Option<(f64, f64)> {
    if !t.is_finite() || tolerance.is_nan() || tolerance < 0.0 {
        return None;
    }
    // The points are in order: the nearest is one side or the other of where `t` goes.
    let i = pts.0.partition_point(|p| *p < t);
    [i.checked_sub(1), Some(i)]
        .into_iter()
        .flatten()
        .filter_map(|i| pts.0.get(i).copied())
        .map(|p| (p, (p - t).abs()))
        .filter(|(_, d)| *d <= tolerance)
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

/// `t` snapped to the nearest point within `tolerance`: (snapped, the point hit).
pub fn snap(t: f64, pts: &SnapPoints, tolerance: f64) -> (f64, Option<f64>) {
    match nearest(t, pts, tolerance) {
        Some((p, _)) => (p, Some(p)),
        None => (t, None),
    }
}

/// A span's start snapped so that either of its edges meets a point (the
/// nearer wins; the start on a tie): (the start, the point hit).
pub fn snap_span(
    start: f64,
    len: Option<f64>,
    pts: &SnapPoints,
    tolerance: f64,
) -> (f64, Option<f64>) {
    let by_start = nearest(start, pts, tolerance).map(|(p, d)| (p, d, p));
    let by_end = len
        .filter(|l| l.is_finite())
        .and_then(|l| nearest(start + l, pts, tolerance).map(|(p, d)| (p - l, d, p)));
    match (by_start, by_end) {
        (Some(s), Some(e)) if e.1 < s.1 => (e.0, Some(e.2)),
        (Some(s), _) => (s.0, Some(s.2)),
        (None, Some(e)) => (e.0, Some(e.2)),
        (None, None) => (start, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::show::CueType;
    use crate::timeline::TrackId;

    fn clip(id: u64, start: f64, len: Option<f64>, fade_out: f64, lead_in: LeadIn) -> Clip {
        Clip {
            id: CueId(id),
            start,
            len,
            lead_in,
            fade_in: 0.0,
            fade_out,
            track: TrackId::Cues,
            ty: CueType::Video,
            name: String::new(),
            armed: true,
            color: None,
            trim_left: true,
            trim_right: true,
            has_sound: false,
            loops: false,
            speed_keys: false,
            clocked: false,
            also_on: Vec::new(),
        }
    }

    fn marker(time: f64) -> Marker {
        Marker {
            id: 1,
            time,
            name: "M".into(),
            color: None,
        }
    }

    #[test]
    fn the_points_are_edges_fade_outs_lead_ins_markers_and_the_playhead() {
        let clips = [
            clip(1, 2.0, Some(6.0), 1.5, LeadIn::At(1.0)),
            clip(2, 10.0, None, 1.0, LeadIn::None),
            // A fade-out as long as the clip starts where it does: once.
            clip(3, 20.0, Some(2.0), 2.0, LeadIn::UntilFaded),
        ];
        let pts = points(&clips, &[marker(9.0), marker(f64::NAN)], Some(4.25), &[]);
        assert_eq!(pts.0, vec![2.0, 3.0, 4.25, 6.5, 8.0, 9.0, 10.0, 20.0, 22.0]);
        // The clips being moved don't snap to themselves.
        let pts = points(&clips, &[], None, &[CueId(1), CueId(3)]);
        assert_eq!(pts.0, vec![10.0]);
    }

    #[test]
    fn a_time_snaps_to_the_nearest_point_within_the_tolerance() {
        let pts = SnapPoints(vec![1.0, 2.0, 6.5]);
        assert_eq!(snap(1.9, &pts, 0.2), (2.0, Some(2.0)));
        assert_eq!(snap(1.4, &pts, 0.5), (1.0, Some(1.0)), "the nearer of two");
        assert_eq!(snap(1.6, &pts, 0.5), (2.0, Some(2.0)));
        assert_eq!(snap(4.0, &pts, 0.5), (4.0, None), "too far");
        assert_eq!(snap(7.0, &pts, 0.5), (6.5, Some(6.5)), "past the last");
        assert_eq!(snap(0.0, &pts, 1.0), (1.0, Some(1.0)), "before the first");
        assert_eq!(snap(f64::NAN, &pts, 1.0).1, None);
        assert_eq!(snap(2.0, &pts, f64::NAN), (2.0, None));
        assert_eq!(snap(2.0, &pts, -1.0), (2.0, None));
        assert_eq!(snap(2.0, &SnapPoints::default(), 1.0), (2.0, None));
    }

    #[test]
    fn a_span_snaps_by_either_edge_and_to_a_fade_out_start() {
        // The other clip fades out from 8.5 to 10.
        let other = clip(9, 4.0, Some(6.0), 1.5, LeadIn::None);
        let pts = points(&[other], &[], None, &[]);
        // Start near the fade-out's start.
        assert_eq!(snap_span(8.4, Some(3.0), &pts, 0.2), (8.5, Some(8.5)));
        // End near the other's start.
        assert_eq!(snap_span(0.9, Some(3.0), &pts, 0.2), (1.0, Some(4.0)));
        // Both in reach: the nearer edge wins (the end is 0.05 from 10).
        assert_eq!(snap_span(8.35, Some(1.6), &pts, 0.2), (8.4, Some(10.0)));
        // An open-ended span snaps by its start only.
        assert_eq!(snap_span(0.9, None, &pts, 0.2), (0.9, None));
        assert_eq!(snap_span(30.0, Some(1.0), &pts, 0.2), (30.0, None));
    }
}
