//! The chase on made-up shows, one test (or a few) for each rule of plan 2.6.
//! Lengths come from a fake `Dur`: a file plays from its in to its out point
//! at its speed, a still is held for its time.

use super::*;
use crate::show::{
    Animated, ControlCue, CueType, GroupMode, Keyframe, OutputId, StopCue, TimelineSettings,
};

struct T {
    show: Show,
    g: CueId,
}

impl T {
    /// A timeline group; `editor`: made in the editor (clips at opacity 0 wait).
    fn new(editor: bool) -> T {
        let mut show = Show::default();
        let mut g = show.make_cue(CueType::Group, None, 0);
        if let CueKind::Group(gc) = &mut g.kind {
            gc.mode = GroupMode::Timeline;
            gc.timeline = editor.then(TimelineSettings::for_editor);
        }
        let id = g.id;
        show.cues.push(g);
        T { show, g: id }
    }

    fn key(&self) -> TimelineKey {
        TimelineKey::Group(self.g)
    }

    fn out(&self) -> OutputId {
        self.show.outputs[0].id
    }

    fn push(&mut self, mut c: Cue, at: f64) -> CueId {
        c.parent = Some(self.g);
        c.pre_wait = at;
        let id = c.id;
        self.show.cues.push(c);
        id
    }

    /// A clip of `ty` at `at`, `len` seconds of content, cut in, no fades.
    fn clip(&mut self, ty: CueType, at: f64, len: f64) -> CueId {
        let path = (ty != CueType::Text && ty != CueType::Shape).then(|| "f.mp4".into());
        let mut c = self.show.make_cue(ty, path, self.show.cues.len());
        let m = c.kind.media_mut().unwrap();
        m.transition = Transition::Cut;
        m.fade_in = 0.0;
        m.fade_out = 0.0;
        m.opacity = 1.0;
        m.volume_db = 0.0;
        m.layer = 0;
        m.start = 0.0;
        m.end = Some(len);
        m.hold_last_frame = false;
        m.loop_media = false;
        m.curve = Curve::Linear;
        self.push(c, at)
    }

    fn video(&mut self, at: f64, len: f64) -> CueId {
        self.clip(CueType::Video, at, len)
    }

    fn m(&mut self, id: CueId) -> &mut MediaCue {
        self.show.cue_mut(id).unwrap().kind.media_mut().unwrap()
    }

    fn fade(&mut self, target: CueId, at: f64, secs: f64, f: impl FnOnce(&mut FadeCue)) -> CueId {
        let mut c = self
            .show
            .make_cue(CueType::Fade, None, self.show.cues.len());
        if let CueKind::Fade(fc) = &mut c.kind {
            fc.target = Some(target);
            fc.duration = secs;
            fc.curve = Curve::Linear;
            fc.opacity = None;
            fc.volume_db = None;
            fc.stop_target = false;
            f(fc);
        }
        self.push(c, at)
    }

    fn control(&mut self, at: f64, c: ControlCue) -> CueId {
        let mut cue = self
            .show
            .make_cue(CueType::Control, None, self.show.cues.len());
        cue.kind = CueKind::Control(c);
        self.push(cue, at)
    }

    fn chase(&self, pos: f64) -> Chase {
        chase(&self.show, self.key(), pos, &mut fake_dur)
    }
}

/// The engine's sum, with every file long enough for its out point.
fn fake_dur(c: &Cue) -> Option<Option<f64>> {
    Some(match &c.kind {
        CueKind::Video(m) | CueKind::Audio(m) => {
            if m.loop_media && m.loop_count == 0 {
                return Some(None);
            }
            let file = (m.end.unwrap_or(10.0) - m.start).max(0.0);
            Some(match m.speed_map() {
                Some(map) => map.time_for(file),
                None => file / m.speed(),
            })
        }
        CueKind::Fade(f) => Some(f.duration),
        CueKind::Wait(w) => Some(w.duration),
        CueKind::Stop(_) | CueKind::Control(_) | CueKind::Send(_) => Some(0.0),
        kind => kind.media().and_then(|m| m.end.filter(|e| *e > 0.0)),
    })
}

fn alive(ch: &Chase, id: CueId) -> Option<&Alive> {
    ch.alive.iter().find(|a| a.id == id)
}

fn upcoming(ch: &Chase, id: CueId) -> Option<&Waiting> {
    ch.upcoming.iter().find(|w| w.id == id)
}

fn passed(ch: &Chase, id: CueId) -> bool {
    ch.passed.contains(&id)
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

fn keys(pairs: &[(f64, f32)]) -> Animated {
    Animated {
        value: 0.0,
        keys: pairs
            .iter()
            .map(|&(t, v)| Keyframe {
                t,
                v,
                curve: Curve::Linear,
            })
            .collect(),
    }
}

#[test]
fn a_file_clip_under_the_playhead_starts_part_way_in_and_one_ended_or_to_come_does_not() {
    let mut t = T::new(false);
    let a = t.video(2.0, 5.0);
    let s = t.clip(CueType::Audio, 1.0, 3.0);
    let later = t.video(9.0, 1.0);
    let ch = t.chase(3.5);
    let v = alive(&ch, a).unwrap();
    assert!(near(v.from, 1.5) && v.lead == 0.0 && !v.frozen && !v.at_last_frame);
    assert!(v.opacity.is_none() && v.volume_db.is_none() && !v.faded);
    assert!(near(alive(&ch, s).unwrap().from, 2.5));
    assert_eq!(upcoming(&ch, later).unwrap().at, Some(9.0));
    // The sound has ended at 4; the video at 7.
    let ch = t.chase(5.0);
    assert!(passed(&ch, s) && alive(&ch, a).is_some());
    assert!(passed(&t.chase(7.0), a));
    // A file whose length is not known yet is taken to be playing (the one at
    // 9 has cut the other away).
    let ch = chase(&t.show, t.key(), 50.0, &mut |_| None);
    assert!(alive(&ch, later).is_some() && alive(&ch, s).is_some() && passed(&ch, a));
}

#[test]
fn a_loop_for_ever_is_alive_until_replaced_and_set_plays_count_whole() {
    let mut t = T::new(false);
    let lp = t.video(0.0, 2.0);
    t.m(lp).loop_media = true;
    t.m(lp).loop_count = 0;
    let ch = t.chase(100.0);
    assert!(
        near(alive(&ch, lp).unwrap().from, 100.0),
        "the engine wraps it into its pass"
    );
    let next = t.video(50.0, 1.0);
    let ch = t.chase(100.0);
    assert!(
        passed(&ch, lp) && passed(&ch, next),
        "cut away at 50, and that one ended"
    );
}

#[test]
fn a_video_that_holds_its_last_frame_stays_on_it_until_something_replaces_it() {
    let mut t = T::new(false);
    let v = t.video(1.0, 2.0);
    t.m(v).hold_last_frame = true;
    let ch = t.chase(10.0);
    let a = alive(&ch, v).unwrap();
    assert!(a.at_last_frame && near(a.from, 2.0));
    // With a fade-out of its own it fades away instead.
    t.m(v).fade_out = 0.5;
    assert!(passed(&t.chase(10.0), v));
    t.m(v).fade_out = 0.0;
    let b = t.video(8.0, 5.0);
    let ch = t.chase(10.0);
    assert!(passed(&ch, v) && alive(&ch, b).is_some());
}

#[test]
fn a_still_text_shape_or_camera_is_held_for_its_time_or_until_replaced() {
    let mut t = T::new(false);
    let img = t.clip(CueType::Image, 1.0, 3.0);
    let txt = t.clip(CueType::Text, 0.0, 10.0);
    t.m(txt).layer = 1;
    let cam = t.clip(CueType::Camera, 2.0, 0.0);
    t.m(cam).end = None;
    t.m(cam).layer = 2;
    let ch = t.chase(3.0);
    assert!(near(alive(&ch, img).unwrap().from, 2.0));
    assert!(near(alive(&ch, txt).unwrap().from, 3.0));
    assert!(
        near(alive(&ch, cam).unwrap().from, 1.0),
        "run-for counts from its start"
    );
    let ch = t.chase(500.0);
    assert!(passed(&ch, img) && passed(&ch, txt));
    assert!(alive(&ch, cam).is_some(), "open-ended");
}

#[test]
fn a_later_picture_on_the_same_layer_replaces_by_its_transition() {
    let mut t = T::new(false);
    let out = t.out();
    let a = t.video(0.0, 20.0);
    let b = t.video(5.0, 20.0);
    t.m(b).transition = Transition::Crossfade;
    t.m(b).fade_in = 2.0;
    // Half way through the crossfade both are up: one fading out and going, one in.
    let ch = t.chase(6.0);
    let x = alive(&ch, a).unwrap();
    let (now, to, left, _) = x.opacity.unwrap();
    assert!((now - 0.5).abs() < 1e-4 && to == 0.0 && near(left, 1.0));
    assert!(near(x.stop_after.unwrap(), 1.0));
    let (now, to, left, _) = alive(&ch, b).unwrap().opacity.unwrap();
    assert!((now - 0.5).abs() < 1e-4 && to == 1.0 && near(left, 1.0));
    assert!(
        !alive(&ch, b).unwrap().faded,
        "its own fade-in, no fade cue"
    );
    // After it, only the new one, at its own level.
    let ch = t.chase(7.5);
    assert!(passed(&ch, a));
    assert!(alive(&ch, b).unwrap().opacity.is_none());
    // A cut replaces at once; another layer, or another display, is left.
    t.m(b).transition = Transition::Cut;
    assert!(
        passed(&t.chase(5.0), a),
        "exactly at its time it has happened"
    );
    t.m(b).layer = 1;
    assert!(alive(&t.chase(6.0), a).is_some());
    t.m(b).layer = 0;
    t.m(b).output = None;
    assert!(alive(&t.chase(6.0), a).is_some());
    t.m(b).output = Some(out);
    // Fading in on top leaves what is under it.
    t.m(b).transition = Transition::FadeIn;
    t.m(b).fade_in = 1.0;
    assert!(alive(&t.chase(9.0), a).is_some());
    // A dip clears the display half way through it, and the clip's time counts from there.
    t.m(b).transition = Transition::DipToBlack;
    t.m(b).fade_in = 2.0;
    let ch = t.chase(5.5);
    assert!(
        alive(&ch, a).is_some() && upcoming(&ch, b).is_some(),
        "under the matte"
    );
    let ch = t.chase(7.0);
    assert!(passed(&ch, a) && near(alive(&ch, b).unwrap().from, 1.0));
}

#[test]
fn an_old_show_replaces_every_layer_and_clips_starting_together_never_replace_each_other() {
    let mut t = T::new(false);
    let a = t.video(0.0, 20.0);
    let b = t.video(0.0, 20.0);
    let c = t.video(4.0, 20.0);
    t.m(c).layer = 3;
    let ch = t.chase(5.0);
    assert!(alive(&ch, a).is_some() && alive(&ch, b).is_some() && alive(&ch, c).is_some());
    t.show.own_layers = false;
    let ch = t.chase(5.0);
    assert!(passed(&ch, a) && passed(&ch, b), "any layer of the display");
    assert!(alive(&ch, c).is_some());
    // Two at 0 in an old show: each replaces what came before it (the later wins).
    t.show.cue_mut(c).unwrap().armed = false;
    let ch = t.chase(5.0);
    assert!(passed(&ch, a) && alive(&ch, b).is_some());
    assert!(
        !passed(&ch, c) && upcoming(&ch, c).is_none(),
        "disarmed: left out"
    );
}

#[test]
fn a_clips_own_fade_in_in_progress_goes_on_and_a_sound_fades_in_its_volume() {
    let mut t = T::new(false);
    let v = t.video(2.0, 10.0);
    t.m(v).transition = Transition::FadeIn;
    t.m(v).fade_in = 4.0;
    t.m(v).opacity = 0.8;
    let s = t.clip(CueType::Audio, 2.0, 10.0);
    t.m(s).fade_in = 4.0;
    t.m(s).volume_db = -6.0;
    let ch = t.chase(3.0);
    let (now, to, left, _) = alive(&ch, v).unwrap().opacity.unwrap();
    assert!((now - 0.2).abs() < 1e-4 && (to - 0.8).abs() < 1e-6 && near(left, 3.0));
    let (now, to, left, _) = alive(&ch, s).unwrap().volume_db.unwrap();
    let want = MIN_DB + (-6.0 - MIN_DB) * 0.25;
    assert!((now - want).abs() < 1e-3 && to == -6.0 && near(left, 3.0));
    assert!(
        alive(&ch, s).unwrap().opacity.is_none(),
        "a sound has no picture to fade"
    );
    assert!(alive(&t.chase(7.0), v).unwrap().opacity.is_none());
}

#[test]
fn a_fade_cue_done_leaves_its_end_values_and_one_in_progress_goes_on_from_where_it_is() {
    let mut t = T::new(false);
    let v = t.video(0.0, 30.0);
    t.fade(v, 2.0, 4.0, |f| {
        f.opacity = Some(0.0);
        f.volume_db = Some(-20.0);
        f.geometry.x = Some(0.5);
    });
    let ch = t.chase(3.0);
    let a = alive(&ch, v).unwrap();
    let (now, to, left, curve) = a.opacity.unwrap();
    assert!((now - 0.75).abs() < 1e-4 && to == 0.0 && near(left, 3.0) && curve == Curve::Linear);
    let (now, to, _, _) = a.volume_db.unwrap();
    assert!((now + 5.0).abs() < 1e-3 && to == -20.0);
    let g = a.geometry.unwrap();
    assert!((g.now.x.unwrap() - 0.125).abs() < 1e-4 && g.to.x == Some(0.5));
    assert!(g.now.y.is_none() && near(g.left, 3.0));
    assert!(a.faded);
    let ch = t.chase(10.0);
    let a = alive(&ch, v).unwrap();
    assert_eq!(a.opacity.map(|o| (o.0, o.1, o.2)), Some((0.0, 0.0, 0.0)));
    assert_eq!(a.geometry.unwrap().now.x, Some(0.5));
    assert!(a.faded && a.stop_after.is_none());
    // A second fade starts from where the first left it.
    t.fade(v, 8.0, 2.0, |f| f.opacity = Some(1.0));
    let (now, to, _, _) = alive(&t.chase(9.0), v).unwrap().opacity.unwrap();
    assert!((now - 0.5).abs() < 1e-4 && to == 1.0);
}

#[test]
fn a_fade_that_stops_its_target_and_a_stop_cue_end_it_or_keep_it_from_starting() {
    let mut t = T::new(false);
    let v = t.video(0.0, 30.0);
    t.fade(v, 2.0, 4.0, |f| f.stop_target = true);
    let ch = t.chase(3.0);
    assert!(near(alive(&ch, v).unwrap().stop_after.unwrap(), 3.0));
    assert!(passed(&t.chase(6.0), v));
    // A stop cue: the clip goes at once (its fade taken as done)...
    let mut t = T::new(false);
    let v = t.video(0.0, 30.0);
    let mut c = t.show.make_cue(CueType::Stop, None, t.show.cues.len());
    c.kind = CueKind::Stop(StopCue {
        target: Some(v),
        fade: 5.0,
        use_own_fade: false,
    });
    t.push(c.clone(), 4.0);
    assert!(alive(&t.chase(3.9), v).is_some() && passed(&t.chase(4.5), v));
    // ...and one aimed at a clip still to come means it never comes.
    let later = t.video(8.0, 2.0);
    let mut c2 = t.show.make_cue(CueType::Stop, None, t.show.cues.len());
    c2.kind = CueKind::Stop(StopCue {
        target: Some(later),
        fade: 0.0,
        use_own_fade: false,
    });
    t.push(c2, 5.0);
    let ch = t.chase(6.0);
    assert!(passed(&ch, later) && upcoming(&ch, later).is_none());
    // A stop of everything takes what is up, and what comes later still comes.
    let mut t = T::new(false);
    let v = t.video(0.0, 30.0);
    let later = t.video(8.0, 2.0);
    let mut c = t.show.make_cue(CueType::Stop, None, t.show.cues.len());
    c.kind = CueKind::Stop(StopCue {
        target: None,
        fade: 0.0,
        use_own_fade: false,
    });
    t.push(c, 4.0);
    let ch = t.chase(6.0);
    assert!(passed(&ch, v) && upcoming(&ch, later).is_some());
}

#[test]
fn control_cues_passed_are_not_done_again_but_a_clips_pause_and_resume_are_played_through() {
    let mut t = T::new(false);
    let v = t.video(1.0, 20.0);
    let other = t.video(30.0, 1.0);
    let start = t.control(
        2.0,
        ControlCue {
            action: ControlAction::Start,
            target: Some(other),
            ..ControlCue::default()
        },
    );
    let pause = t.control(
        3.0,
        ControlCue {
            action: ControlAction::Pause,
            target: Some(v),
            ..ControlCue::default()
        },
    );
    let ch = t.chase(5.0);
    assert!(passed(&ch, start) && passed(&ch, pause));
    assert!(upcoming(&ch, other).is_some(), "the Start is not done");
    let a = alive(&ch, v).unwrap();
    assert!(
        a.frozen && a.detached && near(a.from, 2.0),
        "held where it was paused"
    );
    let resume = t.control(
        6.0,
        ControlCue {
            action: ControlAction::Resume,
            target: Some(v),
            ..ControlCue::default()
        },
    );
    let ch = t.chase(8.0);
    let a = alive(&ch, v).unwrap();
    assert!(
        !a.frozen && a.detached && near(a.from, 4.0),
        "three seconds held: {}",
        a.from
    );
    assert!(upcoming(&t.chase(5.5), resume).is_some());
    // Its clock still ends it where its slot ends.
    assert!(passed(&t.chase(21.5), v));
}

#[test]
fn a_timeline_pause_passed_changes_nothing_one_at_the_playhead_has_happened_and_zero_is_the_top() {
    let mut t = T::new(false);
    let v = t.video(0.0, 10.0);
    let p = t.control(
        2.0,
        ControlCue {
            action: ControlAction::TimelinePause,
            ..ControlCue::default()
        },
    );
    let w = t.clip(CueType::Video, 2.0, 1.0);
    t.m(w).layer = 1;
    let ch = t.chase(2.0);
    assert!(
        passed(&ch, p),
        "a pause exactly at the playhead has happened"
    );
    assert!(
        alive(&ch, w).is_some_and(|a| a.from == 0.0),
        "so has a clip there"
    );
    assert!(near(alive(&ch, v).unwrap().from, 2.0));
    assert!(upcoming(&t.chase(1.0), p).is_some());
    // The top: everything to come, what is at 0 too, in time order.
    let ch = t.chase(0.0);
    assert!(ch.alive.is_empty() && ch.passed.is_empty());
    let at: Vec<Option<f64>> = ch.upcoming.iter().map(|w| w.at).collect();
    assert_eq!(at, vec![Some(0.0), Some(2.0), Some(2.0)]);
}

#[test]
fn send_wait_and_timecode_cues_are_never_started_by_a_seek() {
    let mut t = T::new(false);
    let mut w = t.show.make_cue(CueType::Wait, None, t.show.cues.len());
    if let CueKind::Wait(wc) = &mut w.kind {
        wc.duration = 10.0;
    }
    let w = t.push(w, 1.0);
    let tc = t.show.make_cue(CueType::Timecode, None, t.show.cues.len());
    let tc = t.push(tc, 1.0);
    let send = t.show.make_cue(CueType::Send, None, t.show.cues.len());
    let send = t.push(send, 1.0);
    let ch = t.chase(3.0);
    assert!(ch.alive.is_empty());
    assert!(passed(&ch, w) && passed(&ch, tc) && passed(&ch, send));
}

#[test]
fn a_clip_whose_opacity_keys_rise_late_waits_until_then_and_its_content_counts_from_there() {
    let mut t = T::new(true);
    let v = t.video(2.0, 5.0);
    t.m(v).motion.opacity = Some(keys(&[(0.0, 0.0), (4.0, 0.0), (6.0, 1.0)]));
    // Inside the lead-in: waiting for its place, start plus the lead-in.
    let ch = t.chase(4.0);
    assert_eq!(
        upcoming(&ch, v),
        Some(&Waiting {
            id: v,
            at: Some(6.0),
            lead: 4.0
        })
    );
    // In the visible part: its own time from its start, the lead-in given.
    let ch = t.chase(7.0);
    let a = alive(&ch, v).unwrap();
    assert!(near(a.from, 5.0) && near(a.lead, 4.0));
    assert!(a.opacity.is_none(), "its keyframes give its opacity");
    // Its content runs from the lead-in: 5 s of it end at 2 + 4 + 5.
    assert!(alive(&t.chase(10.9), v).is_some() && passed(&t.chase(11.1), v));
    // Already running when it fades in: no lead-in.
    t.m(v).runs_unseen = true;
    let a = t.chase(6.5);
    let a = alive(&a, v).unwrap();
    assert!(near(a.from, 4.5) && a.lead == 0.0);
    assert!(passed(&t.chase(7.1), v), "it ended at 2 + 5");
    // An old timeline (no settings): it plays from its start as ever.
    let mut old = T::new(false);
    let v = old.video(2.0, 5.0);
    old.m(v).motion.opacity = Some(keys(&[(0.0, 0.0), (4.0, 0.0), (6.0, 1.0)]));
    assert!(alive(&old.chase(4.0), v).is_some_and(|a| a.lead == 0.0 && near(a.from, 2.0)));
}

#[test]
fn a_clip_at_opacity_0_waits_for_the_fade_cue_that_raises_it_and_a_silent_sound_too() {
    let mut t = T::new(true);
    let v = t.video(1.0, 5.0);
    t.m(v).opacity = 0.0;
    let s = t.clip(CueType::Audio, 1.0, 5.0);
    t.m(s).volume_db = MIN_DB;
    // Nothing raises them yet: they never start.
    let ch = t.chase(3.0);
    assert!(passed(&ch, v) && passed(&ch, s) && ch.alive.is_empty());
    t.fade(v, 4.0, 2.0, |f| f.opacity = Some(1.0));
    t.fade(s, 4.0, 2.0, |f| f.volume_db = Some(0.0));
    let ch = t.chase(3.0);
    assert_eq!(upcoming(&ch, v).map(|w| (w.at, w.lead)), Some((None, 3.0)));
    assert_eq!(upcoming(&ch, s).map(|w| w.at), Some(None));
    // After the fade: started at the fade, from its in point, fading up from 0.
    let ch = t.chase(5.0);
    let a = alive(&ch, v).unwrap();
    assert!(near(a.lead, 3.0) && near(a.from, 4.0) && a.faded);
    let (now, to, left, _) = a.opacity.unwrap();
    assert!((now - 0.5).abs() < 1e-4 && to == 1.0 && near(left, 1.0));
    let b = alive(&ch, s).unwrap();
    assert!(near(b.lead, 3.0) && b.volume_db.is_some_and(|v| v.1 == 0.0));
    // Exactly at the fade it has come in and the fade has begun.
    let a = t.chase(4.0);
    let a = alive(&a, v).unwrap();
    assert!(near(a.from, a.lead) && a.opacity.is_some_and(|o| o.0 == 0.0 && near(o.2, 2.0)));
    // Its 5 s of content end 5 s after the fade.
    assert!(passed(&t.chase(9.1), v));
}

#[test]
fn a_list_timeline_is_chased_too() {
    let mut show = Show::default();
    let l = show.add_list("Timeline");
    show.lists.last_mut().unwrap().timeline = Some(TimelineSettings::default());
    let mut c = show.make_cue(CueType::Video, Some("f.mp4".into()), show.cues.len());
    c.list = l;
    c.pre_wait = 2.0;
    let m = c.kind.media_mut().unwrap();
    m.start = 0.0;
    m.end = Some(4.0);
    let id = c.id;
    show.cues.push(c);
    show.normalize_lists();
    let ch = chase(&show, TimelineKey::List(l), 3.0, &mut fake_dur);
    assert!(alive(&ch, id).is_some_and(|a| near(a.from, 1.0)));
}
