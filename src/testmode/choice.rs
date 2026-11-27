//! What the Test Mode window asks: a length and what to test. Pure: the ticks
//! and the length give the plan of parts (`plan`, `plan_with` for the 3D
//! stage tick), how long it takes (`total_secs`, `length_words`, `summary`)
//! and why it can't start (`start_refusal`, `network_reason`).
//!
//! The 3D stage tick is not one of the `Parts` this computer keeps: it is off
//! by default and on by itself when the show has a projector or a display
//! that shows the stage (`stage_wanted`, owner's decision 8), each time the
//! window opens.

use super::Level;
use serde::{Deserialize, Serialize};

/// The three things a test can try that this computer remembers (the 3D
/// stage's tick follows the show: `stage_wanted`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parts {
    #[serde(default)]
    pub cue_list: bool,
    #[serde(default)]
    pub timeline: bool,
    #[serde(default)]
    pub network: bool,
}

impl Default for Parts {
    /// What the window starts with: the cue list.
    fn default() -> Parts {
        Parts {
            cue_list: true,
            timeline: false,
            network: false,
        }
    }
}

/// One part of a run, in the order they play.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    CueList,
    Timeline,
    Stage3d,
    Network,
}

impl Part {
    /// What the window and the report call it.
    pub fn name(self) -> &'static str {
        match self {
            Part::CueList => "Cue list",
            Part::Timeline => "Timeline",
            Part::Stage3d => "3D stage",
            Part::Network => "Network",
        }
    }

    /// Its name in file names ("cue-list").
    pub fn slug(self) -> &'static str {
        match self {
            Part::CueList => "cue-list",
            Part::Timeline => "timeline",
            Part::Stage3d => "stage3d",
            Part::Network => "network",
        }
    }

    /// The flavour of the cue and timeline tests.
    pub fn flavour(self) -> Option<super::Flavour> {
        match self {
            Part::CueList => Some(super::Flavour::Cues),
            Part::Timeline => Some(super::Flavour::Timeline),
            Part::Stage3d => Some(super::Flavour::Stage),
            Part::Network => None,
        }
    }
}

/// One part as it is to run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PartPlan {
    pub part: Part,
    pub level: Level,
    /// How long a Medium or Long cue or timeline part plays (0: one lap).
    pub minutes: u32,
    /// Laps of the network part's script.
    pub laps: u32,
}

impl PartPlan {
    /// About how long it plays, in seconds.
    pub fn secs(&self) -> f64 {
        match self.part {
            Part::Network => f64::from(self.laps) * super::network::LAP_SECS,
            _ if self.minutes > 0 => f64::from(self.minutes) * 60.0,
            Part::Stage3d => STAGE_SHORT_SECS,
            _ => SHORT_SECS,
        }
    }
}

/// About how long a Short cue or timeline lap takes, with its getting ready.
const SHORT_SECS: f64 = 60.0;
/// The 3D stage part's five Short legs, with its getting ready.
const STAGE_SHORT_SECS: f64 = 5.0 * super::stage::SHORT_LEG_SECS + 20.0;

/// Whether the 3D stage part is ticked by itself for `show`: it has a
/// projector, or a display that shows the stage (owner's decision 8).
pub fn stage_wanted(show: &crate::show::Show) -> bool {
    !show.stage3d.projectors.is_empty() || show.outputs.iter().any(|o| show.shows_stage(o.id))
}

/// The network part's laps at each length: one, a few, and the soak's ten.
pub fn network_laps(level: Level) -> u32 {
    match level {
        Level::Short => 1,
        Level::Medium => 3,
        Level::Long => super::network::SOAK_LAPS,
    }
}

/// The whole run at a length, in minutes, when a cue list or timeline part is
/// in it (they share what the network part leaves).
fn whole_minutes(level: Level) -> u32 {
    match level {
        Level::Short => 0,
        Level::Medium => 60,
        Level::Long => 480,
    }
}

/// The parts to run, in order: those ticked that can run (`network_ok`),
/// the 3D stage not among them.
#[cfg(test)]
pub fn plan(level: Level, parts: Parts, network_ok: bool) -> Vec<PartPlan> {
    plan_with(level, parts, false, network_ok)
}

/// `plan`, with the 3D stage part if `stage` (after the timeline, before
/// the network).
pub fn plan_with(level: Level, parts: Parts, stage: bool, network_ok: bool) -> Vec<PartPlan> {
    let mut v = Vec::new();
    let net = (parts.network && network_ok).then(|| PartPlan {
        part: Part::Network,
        level,
        minutes: 0,
        laps: network_laps(level),
    });
    let played: Vec<Part> = [
        (parts.cue_list, Part::CueList),
        (parts.timeline, Part::Timeline),
        (stage, Part::Stage3d),
    ]
    .into_iter()
    .filter_map(|(on, p)| on.then_some(p))
    .collect();
    // Medium and Long are the whole run's length: the network part's laps
    // first, the rest shared out, never under 20 minutes a part.
    let left = f64::from(whole_minutes(level)) - net.map_or(0.0, |n| n.secs() / 60.0);
    let minutes = if level == Level::Short || played.is_empty() {
        0
    } else {
        ((left / played.len() as f64).round() as u32).max(20)
    };
    for part in played {
        v.push(PartPlan {
            part,
            level,
            minutes,
            laps: 1,
        });
    }
    v.extend(net);
    v
}

/// About how long the whole plan takes, in seconds.
pub fn total_secs(plan: &[PartPlan]) -> f64 {
    plan.iter().map(PartPlan::secs).sum()
}

/// "about 4 minutes", "about an hour", "about 8 hours".
pub fn length_words(secs: f64) -> String {
    let minutes = (secs / 60.0).round().max(1.0) as u32;
    if minutes < 55 {
        format!(
            "about {minutes} minute{}",
            if minutes == 1 { "" } else { "s" }
        )
    } else if minutes < 90 {
        "about an hour".into()
    } else {
        let hours = (f64::from(minutes) / 60.0).round() as u32;
        format!("about {hours} hours")
    }
}

/// The drop-down's words for a length with these ticks: "Short · about 4 minutes".
#[cfg(test)]
pub fn level_item(level: Level, parts: Parts, network_ok: bool) -> String {
    level_item_with(level, parts, false, network_ok)
}

/// `level_item`, with the 3D stage part if `stage`.
pub fn level_item_with(level: Level, parts: Parts, stage: bool, network_ok: bool) -> String {
    let p = plan_with(level, parts, stage, network_ok);
    if p.is_empty() {
        return level.label().to_string();
    }
    format!("{} · {}", level.label(), length_words(total_secs(&p)))
}

/// The line under the drop-down: "Cue list, then timeline, then the network
/// check: about 4 minutes".
pub fn summary(plan: &[PartPlan]) -> String {
    let words: Vec<&str> = plan
        .iter()
        .enumerate()
        .map(|(i, p)| match (i, p.part) {
            (0, Part::CueList) => "Cue list",
            (0, Part::Timeline) => "Timeline",
            (0, Part::Stage3d) => "The 3D stage",
            (0, Part::Network) => "The network check",
            (_, Part::CueList) => "cue list",
            (_, Part::Timeline) => "timeline",
            (_, Part::Stage3d) => "the 3D stage",
            (_, Part::Network) => "the network check",
        })
        .collect();
    if words.is_empty() {
        return String::new();
    }
    let what = match plan.first() {
        Some(p) if plan.len() == 1 && p.part != Part::Network && p.minutes > 0 => {
            format!("{}, round and round", words[0])
        }
        Some(p) if plan.len() == 1 && p.part == Part::Network && p.laps > 1 => {
            format!("{}, {} laps", words[0], p.laps)
        }
        _ => words.join(", then "),
    };
    format!("{what}: {}", length_words(total_secs(plan)))
}

/// The parts named in `NEWQ_TEST_PARTS` ("cue,timeline,network", any order,
/// any case); nothing named: the network alone (what the developers' start
/// always did). The 3D stage ("stage", "3d") is `parse_stage`.
pub fn parse_parts(text: &str) -> Parts {
    parse_ticks(text).0
}

/// Whether `NEWQ_TEST_PARTS` names the 3D stage part.
pub fn parse_stage(text: &str) -> bool {
    parse_ticks(text).1
}

fn parse_ticks(text: &str) -> (Parts, bool) {
    let mut stage = false;
    let mut p = Parts {
        cue_list: false,
        timeline: false,
        network: false,
    };
    for word in text.split([',', ' ', '+']).map(str::trim) {
        match word.to_lowercase().as_str() {
            "cue" | "cues" | "cue-list" | "cuelist" => p.cue_list = true,
            "timeline" => p.timeline = true,
            "network" | "net" => p.network = true,
            "stage" | "3d" | "stage3d" | "3d-stage" => stage = true,
            _ => {}
        }
    }
    if !(p.cue_list || p.timeline || p.network || stage) {
        p.network = true;
    }
    (p, stage)
}

/// What a part plays, for the window's "What does this do?": the test's own
/// words without their first sentence (its length, which the run decides).
pub fn what_it_does(p: &PartPlan) -> String {
    let text = match p.part.flavour() {
        Some(super::Flavour::Stage) => super::stage::WHAT_IT_DOES,
        Some(f) => f.summary(p.level),
        None => {
            "Plays a short show of its own on every monitor of every computer and measures how closely each keeps to this one: the clock, each GO, the pictures, corrections and stalls. It opens with a Sync flash you can film in slow motion across two screens. Its media is sent to every computer first."
        }
    };
    let text = match p.part {
        Part::Network | Part::Stage3d => text,
        _ => text.split_once(". ").map_or(text, |(_, rest)| rest),
    };
    format!("{}: {text}", p.part.name())
}

/// What the window knows of the show network for the Network tick.
#[derive(Clone, Debug, Default)]
pub struct NetFacts {
    /// This computer runs the show network.
    pub primary: bool,
    /// Its computers connected now, by name, and whether each is in step.
    pub computers: Vec<(String, bool)>,
}

/// Why the Network tick can't be ticked, in one short line; None: it can.
pub fn network_reason(n: &NetFacts) -> Option<String> {
    if !n.primary || n.computers.is_empty() {
        return Some("Needs another computer in the show network.".into());
    }
    if n.computers.iter().any(|(_, in_step)| *in_step) {
        return None;
    }
    let names: Vec<&str> = n.computers.iter().map(|(n, _)| n.as_str()).collect();
    Some(format!("Waiting for {} to be in step.", or_list(&names)))
}

fn or_list(names: &[&str]) -> String {
    joined(names, "or")
}

/// "Left", "Left and Right", "Left, Right and Rear".
pub fn and_list(names: &[&str]) -> String {
    joined(names, "and")
}

fn joined(names: &[&str], word: &str) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} {word} {last}", rest.join(", ")),
    }
}

/// Why Start can't be pressed with this plan, in plain words; None: it can.
/// `screens` is how many screens are offered (none: the Preview pane) and
/// `ticked` how many of them are ticked.
pub fn start_refusal(plan: &[PartPlan], screens: usize, ticked: usize) -> Option<&'static str> {
    if plan.is_empty() {
        Some("Tick at least one thing to test.")
    } else if screens > 0 && ticked == 0 {
        Some("Tick at least one monitor to show the test on.")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticks(cue_list: bool, timeline: bool, network: bool) -> Parts {
        Parts {
            cue_list,
            timeline,
            network,
        }
    }

    #[test]
    fn the_ticks_and_the_length_make_the_plan_in_order() {
        let all = ticks(true, true, true);
        let p = plan(Level::Short, all, true);
        let order: Vec<Part> = p.iter().map(|p| p.part).collect();
        assert_eq!(order, [Part::CueList, Part::Timeline, Part::Network]);
        assert!(p.iter().all(|p| p.level == Level::Short && p.minutes == 0));
        assert_eq!(p[2].laps, 1);
        // The network unticked, or ticked but not possible here, is left out.
        assert_eq!(plan(Level::Short, all, false).len(), 2);
        assert_eq!(plan(Level::Short, ticks(true, true, false), true).len(), 2);
        assert!(plan(Level::Short, ticks(false, false, true), false).is_empty());
        assert!(plan(Level::Long, ticks(false, false, false), true).is_empty());
        // Longer: more network laps, the Long one the soak's ten.
        assert_eq!(plan(Level::Medium, all, true)[2].laps, 3);
        assert_eq!(
            plan(Level::Long, all, true)[2].laps,
            super::super::network::SOAK_LAPS
        );
    }

    #[test]
    fn medium_and_long_are_the_whole_runs_length_shared_by_the_parts() {
        let one = plan(Level::Medium, ticks(true, false, false), false);
        assert_eq!(one[0].minutes, 60, "alone, a Medium cue list is an hour");
        let two = plan(Level::Medium, ticks(true, true, false), false);
        assert_eq!((two[0].minutes, two[1].minutes), (30, 30));
        let three = plan(Level::Medium, ticks(true, true, true), true);
        // Three laps of the network (about 6 minutes), the rest shared.
        assert_eq!(three[0].minutes, 27);
        let long = plan(Level::Long, ticks(false, true, false), false);
        assert_eq!(long[0].minutes, 480);
        for p in [one, two, three, long] {
            let hours = total_secs(&p) / 3600.0;
            assert!(hours >= 0.9, "{p:?}");
        }
    }

    #[test]
    fn the_time_is_said_in_plain_words() {
        let all = ticks(true, true, true);
        let p = plan(Level::Short, all, true);
        assert_eq!(
            summary(&p),
            "Cue list, then timeline, then the network check: about 4 minutes"
        );
        assert_eq!(
            summary(&plan(Level::Short, ticks(true, false, false), false)),
            "Cue list: about 1 minute"
        );
        assert_eq!(
            summary(&plan(Level::Medium, ticks(true, false, false), false)),
            "Cue list, round and round: about an hour"
        );
        assert_eq!(
            summary(&plan(Level::Long, ticks(false, false, true), true)),
            "The network check, 10 laps: about 19 minutes"
        );
        assert_eq!(
            summary(&plan(Level::Long, all, true)),
            "Cue list, then timeline, then the network check: about 8 hours"
        );
        assert_eq!(summary(&[]), "");
        assert_eq!(
            level_item(Level::Short, all, true),
            "Short · about 4 minutes"
        );
        assert_eq!(
            level_item(Level::Medium, all, true),
            "Medium · about an hour"
        );
        assert_eq!(level_item(Level::Long, all, false), "Long · about 8 hours");
        assert_eq!(
            level_item(Level::Long, ticks(false, false, false), true),
            "Long"
        );
        assert_eq!(length_words(90.0), "about 2 minutes");
    }

    #[test]
    fn the_network_tick_says_why_it_cant_be_ticked() {
        let alone = NetFacts::default();
        assert_eq!(
            network_reason(&alone).as_deref(),
            Some("Needs another computer in the show network.")
        );
        let none_joined = NetFacts {
            primary: true,
            computers: Vec::new(),
        };
        assert!(network_reason(&none_joined).is_some());
        let waiting = NetFacts {
            primary: true,
            computers: vec![("Left".into(), false), ("Right".into(), false)],
        };
        assert_eq!(
            network_reason(&waiting).as_deref(),
            Some("Waiting for Left or Right to be in step.")
        );
        let one_in = NetFacts {
            primary: true,
            computers: vec![("Left".into(), true), ("Right".into(), false)],
        };
        assert_eq!(network_reason(&one_in), None);
        // A follower is no primary.
        let follower = NetFacts {
            primary: false,
            computers: vec![("Main".into(), true)],
        };
        assert!(network_reason(&follower).is_some());
    }

    #[test]
    fn the_developers_start_reads_the_parts_it_is_given() {
        assert_eq!(parse_parts(""), ticks(false, false, true));
        assert_eq!(parse_parts("cue,network"), ticks(true, false, true));
        assert_eq!(parse_parts("Timeline + CUE"), ticks(true, true, false));
        assert_eq!(parse_parts("nonsense"), ticks(false, false, true));
    }

    #[test]
    fn the_3d_stage_part_comes_after_the_timeline_and_follows_the_show() {
        let p = plan_with(Level::Short, ticks(true, true, true), true, true);
        let order: Vec<Part> = p.iter().map(|p| p.part).collect();
        assert_eq!(
            order,
            [Part::CueList, Part::Timeline, Part::Stage3d, Part::Network]
        );
        assert_eq!(
            plan(Level::Short, ticks(true, false, false), false).len(),
            1
        );
        let alone = plan_with(Level::Short, ticks(false, false, false), true, false);
        assert_eq!(alone.len(), 1);
        assert_eq!(summary(&alone), "The 3D stage: about 4 minutes");
        assert_eq!(alone[0].part.slug(), "stage3d");
        assert_eq!(
            alone[0].part.flavour(),
            Some(crate::testmode::Flavour::Stage)
        );
        // Medium shares the hour.
        let two = plan_with(Level::Medium, ticks(true, false, false), true, false);
        assert_eq!((two[0].minutes, two[1].minutes), (30, 30));
        assert!(what_it_does(&alone[0]).starts_with("3D stage: Plays the cue list's lap"));
        // The developers' start.
        assert!(parse_stage("cue,stage") && !parse_stage("cue"));
        assert_eq!(parse_parts("3d"), ticks(false, false, false));
        // On by itself for a show with a projector or a display showing the stage.
        let mut show = crate::show::Show::default();
        assert!(!stage_wanted(&show));
        show.add_surface(None);
        assert!(!stage_wanted(&show), "a screen alone isn't enough");
        show.outputs[0].source = Some(crate::stage3d::StageSource::default());
        assert!(stage_wanted(&show));
        show.outputs[0].source = None;
        show.add_projector();
        assert!(stage_wanted(&show));
        // What this computer keeps reads as before: no 3D tick in it.
        let old: Parts =
            serde_json::from_str("{\"cue_list\":true,\"timeline\":false,\"network\":true}")
                .unwrap();
        assert_eq!(old, ticks(true, false, true));
    }

    #[test]
    fn start_says_why_it_cant_be_pressed() {
        let p = plan(Level::Short, Parts::default(), false);
        assert_eq!(
            start_refusal(&[], 2, 1),
            Some("Tick at least one thing to test.")
        );
        assert_eq!(
            start_refusal(&p, 2, 0),
            Some("Tick at least one monitor to show the test on.")
        );
        assert_eq!(start_refusal(&p, 2, 1), None);
        // No screen but the main window's: the Preview pane shows it.
        assert_eq!(start_refusal(&p, 0, 0), None);
    }
}
