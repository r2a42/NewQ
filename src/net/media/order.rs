//! The order the other computers fetch the show's files in (ruling R17),
//! worked out on the primary, which knows the playheads: nearest first.
//!
//! 1. the active list's cues from its playhead forward;
//! 2. each running timeline's clips from its clock forward (what is playing
//!    at the clock first);
//! 3. the other lists from their playheads forward;
//! 4. everything before the playheads, nearest first (the active list's, the
//!    timelines', then the other lists');
//! 5. anything left (a list never visited).
//!
//! A file used several times ranks by its nearest use. The library comes
//! after all of these on its own (the puller does that).

use crate::net::store::paths;
use crate::show::{CueId, Show};
use crate::timeline::TimelineKey;
use std::collections::HashSet;

/// Where the show stands, as the primary sees it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Positions {
    /// The list GO is for, and its playhead.
    pub active_list: u32,
    pub playhead: Option<CueId>,
    /// The other lists' playheads.
    pub others: Vec<(u32, Option<CueId>)>,
    /// The timelines running (or paused part way), with their clocks.
    pub timelines: Vec<(TimelineKey, f64)>,
}

/// Collects paths once each, in the order first met.
struct Ranked {
    seen: HashSet<String>,
    out: Vec<String>,
}

impl Ranked {
    fn push_cue(&mut self, show: &Show, id: CueId) {
        let Some(c) = show.cue(id) else {
            return;
        };
        let Some(m) = c.kind.file_media() else {
            return;
        };
        if m.file.as_os_str().is_empty() {
            return;
        }
        let text = paths::path_text(&m.file);
        if self.seen.insert(paths::path_key(&text)) {
            self.out.push(text);
        }
    }
}

/// The ids of a list's cues in the show's order (groups with their children),
/// split at the playhead: from it on, and before it (nearest first).
fn split(show: &Show, list: u32, playhead: Option<CueId>) -> (Vec<CueId>, Vec<CueId>) {
    let range = show.list_range(list);
    let ids: Vec<CueId> = show.cues[range.clone()].iter().map(|c| c.id).collect();
    let at = playhead
        .and_then(|p| ids.iter().position(|&i| i == p))
        // No playhead (or it is past the end): everything is before it.
        .unwrap_or(if playhead.is_some() { 0 } else { ids.len() });
    let ahead = ids[at..].to_vec();
    let mut behind = ids[..at].to_vec();
    behind.reverse();
    (ahead, behind)
}

/// A timeline's clips split at its clock: what plays there and after (by
/// start), and what is over before it (latest first). `dur` is a cue's length
/// (None: not known, then a clip begun before the clock counts as playing).
fn split_timeline(
    show: &Show,
    key: TimelineKey,
    pos: f64,
    dur: &mut dyn FnMut(&crate::show::Cue) -> Option<f64>,
) -> (Vec<CueId>, Vec<CueId>) {
    let mut lens = |c: &crate::show::Cue| Some(dur(c));
    let mut clips = crate::timeline::model::clips(show, key, &mut lens);
    clips.sort_by(|a, b| a.start.total_cmp(&b.start));
    let (mut ahead, mut behind) = (Vec::new(), Vec::new());
    for c in &clips {
        let over = c.len.is_some_and(|l| c.start + l < pos);
        if over {
            behind.push(c.id);
        } else {
            ahead.push(c.id);
        }
    }
    behind.reverse();
    (ahead, behind)
}

/// The show's used files, nearest the playheads first, as the show names
/// them (the primary's spelling). Every file the show uses is in it.
pub fn order(
    show: &Show,
    at: &Positions,
    dur: &mut dyn FnMut(&crate::show::Cue) -> Option<f64>,
) -> Vec<String> {
    let mut r = Ranked {
        seen: HashSet::new(),
        out: Vec::new(),
    };
    let (active_ahead, active_behind) = split(show, at.active_list, at.playhead);
    let timelines: Vec<(Vec<CueId>, Vec<CueId>)> = at
        .timelines
        .iter()
        .map(|(k, pos)| split_timeline(show, *k, *pos, dur))
        .collect();
    let mut lists: Vec<u32> = show
        .list_ids()
        .into_iter()
        .filter(|l| *l != at.active_list)
        .collect();
    lists.sort_unstable();
    let others: Vec<(Vec<CueId>, Vec<CueId>)> = lists
        .iter()
        .map(|l| {
            let ph = at
                .others
                .iter()
                .find(|(n, _)| n == l)
                .and_then(|(_, p)| *p)
                // A list never visited: from its first cue.
                .or_else(|| show.cues[show.list_range(*l)].first().map(|c| c.id));
            split(show, *l, ph)
        })
        .collect();
    for id in &active_ahead {
        r.push_cue(show, *id);
    }
    for (ahead, _) in &timelines {
        for id in ahead {
            r.push_cue(show, *id);
        }
    }
    for (ahead, _) in &others {
        for id in ahead {
            r.push_cue(show, *id);
        }
    }
    for id in &active_behind {
        r.push_cue(show, *id);
    }
    for (_, behind) in &timelines {
        for id in behind {
            r.push_cue(show, *id);
        }
    }
    for (_, behind) in &others {
        for id in behind {
            r.push_cue(show, *id);
        }
    }
    for c in &show.cues {
        r.push_cue(show, c.id);
    }
    r.out
}

/// What the order depends on, coarsely: the playheads and, per running
/// timeline, how many of its clips have begun (a clip boundary crossed), so
/// it is worked out again only when one of those moves.
pub fn signature(
    show: &Show,
    at: &Positions,
    dur: &mut dyn FnMut(&crate::show::Cue) -> Option<f64>,
) -> Vec<u64> {
    let mut sig = vec![
        u64::from(at.active_list),
        at.playhead.map_or(u64::MAX, |p| p.0),
    ];
    for (l, p) in &at.others {
        sig.push(u64::from(*l));
        sig.push(p.map_or(u64::MAX, |p| p.0));
    }
    for (k, pos) in &at.timelines {
        sig.push(match k {
            TimelineKey::List(n) => u64::from(*n),
            TimelineKey::Group(g) => g.0 | (1 << 63),
        });
        let mut lens = |c: &crate::show::Cue| Some(dur(c));
        let clips = crate::timeline::model::clips(show, *k, &mut lens);
        let begun = clips.iter().filter(|c| c.start <= *pos).count() as u64;
        let over = clips
            .iter()
            .filter(|c| c.len.is_some_and(|l| c.start + l < *pos))
            .count() as u64;
        sig.push(begun);
        sig.push(over);
    }
    sig
}
