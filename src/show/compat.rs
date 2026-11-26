//! Reading a show that a newer NewQ wrote (R4, R10 of the 2026-11-26 reviews).
//!
//! A newer NewQ can write kinds and fields this one doesn't know. None of them
//! stops a show from being read: a cue of a kind this NewQ doesn't know (or
//! one whose settings it can't read) is `CueKind::Unknown`, which keeps the
//! kind as it was written and can't play; a transition, fit, continue mode,
//! effect or colour it doesn't know is the `Other` of its enum, which keeps the
//! value (a `Kept`) and acts as a stated fallback; a cue's and the show's own
//! fields it doesn't know are kept in their `extra`. Everything kept is written
//! back as it was read. What can't be kept (fields it doesn't know deeper in a
//! cue of a kind it knows, and the few kinds read as their default by
//! `lenient`) is why a show from a newer NewQ is saved under a new name first
//! (`newer_than_this`, `UiState::newer_file`).

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::sync::Mutex;

/// The NewQ that is running, as it writes itself into the files it saves
/// (`Show::saved_with`).
pub const THIS_NEWQ: &str = env!("CARGO_PKG_VERSION");

/// The names `CueKind`'s variants are written under (a test keeps it so): a
/// `CueKind::Unknown` of one of these is a cue of a kind this NewQ knows whose
/// settings it can't read.
pub const KNOWN_KINDS: [&str; 14] = [
    "Video",
    "Image",
    "Audio",
    "Fade",
    "Stop",
    "Wait",
    "Group",
    "Text",
    "Shape",
    "Camera",
    "Send",
    "LiveAudio",
    "Control",
    "Timecode",
];

/// A value a newer NewQ wrote that this one doesn't know, kept so that it is
/// written back as it was read. Kept values are interned: the same value read
/// twice is the same `Kept`, so it is `Copy` and the enums that hold one stay
/// `Copy` (and `Eq` and `Hash` by the one copy). The store grows only by the
/// distinct values this NewQ didn't know, of which a show has a handful (a
/// show network sending the same show again adds nothing).
#[derive(Clone, Copy)]
pub struct Kept(&'static Value);

static KEPT: Mutex<Vec<&'static Value>> = Mutex::new(Vec::new());

impl Kept {
    pub fn new(v: Value) -> Kept {
        let mut store = KEPT.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&k) = store.iter().find(|k| ***k == v) {
            return Kept(k);
        }
        let k: &'static Value = Box::leak(Box::new(v));
        store.push(k);
        Kept(k)
    }

    /// What it is called: the variant's name as the newer NewQ wrote it.
    pub fn name(self) -> String {
        kind_name(self.0)
    }
}

/// The name of an externally tagged enum's value: the string itself, or the
/// one key of an object. Anything else is shown as its JSON.
pub fn kind_name(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Object(m) if m.len() == 1 => m.keys().next().cloned().unwrap_or_default(),
        other => other.to_string(),
    }
}

impl PartialEq for Kept {
    fn eq(&self, other: &Kept) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}

impl Eq for Kept {}

impl std::hash::Hash for Kept {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        std::ptr::hash(self.0, h);
    }
}

impl std::fmt::Debug for Kept {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Kept({})", self.0)
    }
}

impl Serialize for Kept {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Kept {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Kept, D::Error> {
        let v = Value::deserialize(d)?;
        log::warn!("show: {v} isn't known to this NewQ (a newer one wrote it); it is kept");
        Ok(Kept::new(v))
    }
}

/// A field whose value this NewQ may not know, read as its default if so (and
/// then not kept: see the module's note). For the few kinds outside a cue whose
/// value isn't kept, so that one of them never stops the show from loading.
pub fn lenient<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: serde::de::DeserializeOwned + Default,
{
    let value = Value::deserialize(d)?;
    Ok(T::deserialize(&value).unwrap_or_else(|e| {
        log::warn!("show: {value} isn't known to this NewQ ({e}); its default is used");
        T::default()
    }))
}

/// `lenient` for a list: the items this NewQ doesn't know are left out.
pub fn lenient_vec<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let items = Vec::<Value>::deserialize(d)?;
    Ok(items
        .into_iter()
        .filter_map(|v| {
            T::deserialize(&v)
                .map_err(|e| log::warn!("show: {v} isn't known to this NewQ ({e}); left out"))
                .ok()
        })
        .collect())
}

/// A version as three numbers (a missing one is 0; anything after a `-` or
/// `+` is left out), or None if it isn't one.
fn numbers(version: &str) -> Option<[u64; 3]> {
    let core = version.trim().split(['-', '+']).next()?;
    let mut out = [0u64; 3];
    let mut parts = core.split('.');
    for slot in &mut out {
        match parts.next() {
            Some(p) => *slot = p.parse().ok()?,
            None => break,
        }
    }
    parts.next().is_none().then_some(out)
}

/// Whether a show that says it was saved by NewQ `saved_with` comes from a
/// newer NewQ than this one. One that says something this NewQ can't read as a
/// version is taken as newer: a later NewQ could write it so, and taking it as
/// newer only costs a "Save as" where a plain save would have done.
pub fn newer_than_this(saved_with: &str) -> bool {
    newer_than(saved_with, THIS_NEWQ)
}

fn newer_than(saved_with: &str, this: &str) -> bool {
    match (numbers(saved_with), numbers(this)) {
        (Some(a), Some(b)) => a > b,
        _ => true,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::show::*;
    use serde_json::{Value, json};

    /// A show as a newer NewQ might write it: a cue of a kind this NewQ doesn't
    /// know, a video with a transition, fit, effect, continue mode and colour it
    /// doesn't know, and fields it doesn't know at every level.
    pub(crate) fn newer_show() -> Value {
        let mut show = Show::default();
        let mut cue = show.make_cue(CueType::Video, Some("a.mp4".into()), 0);
        cue.continue_mode = ContinueMode::AutoFollow;
        cue.color = Some(CueColor::Red);
        let m = cue.kind.media_mut().unwrap();
        m.transition = Transition::Crossfade;
        m.fit = Fit::Cover;
        m.effects = vec![Effect::new(EffectKind::Blur)];
        show.cues.push(cue);
        let mut v = serde_json::to_value(&show).unwrap();
        let c = &mut v["cues"][0];
        c["continue_mode"] = json!("AutoBounce");
        c["color"] = json!("Teal");
        c["new_cue_field"] = json!(7);
        let m = &mut c["kind"]["Video"];
        m["transition"] = json!({"Wipe": {"angle": 30.0}});
        m["fit"] = json!("Tile");
        m["effects"][0]["kind"] = json!("Glow");
        m["sparkle"] = json!(true);
        v["cues"].as_array_mut().unwrap().push(json!({
            "id": 900,
            "number": "7.5",
            "name": "Hologram rig",
            "notes": "",
            "pre_wait": 0.0,
            "continue_mode": "DoNotContinue",
            "future": {"x": 1},
            "kind": {"Hologram": {"rig": "A", "level": 0.5}}
        }));
        v["new_show_setting"] = json!({"on": true});
        v["saved_with"] = json!("99.0.0");
        v
    }

    /// The input without what this NewQ can't keep: a field it doesn't know
    /// inside the settings of a cue of a kind it knows.
    fn kept_part(mut v: Value) -> Value {
        v["cues"][0]["kind"]["Video"]
            .as_object_mut()
            .unwrap()
            .remove("sparkle");
        v
    }

    #[test]
    fn a_show_from_a_newer_newq_loads_and_is_written_back_as_it_was_read() {
        let input = newer_show();
        let show: Show = serde_json::from_str(&input.to_string()).expect("the newer show loads");
        assert_eq!(show.cues.len(), 2);
        assert_eq!(show.saved_with.as_deref(), Some("99.0.0"));
        assert_eq!(show.extra["new_show_setting"], json!({"on": true}));
        // The video is still a video, and plays with stated fallbacks.
        let video = &show.cues[0];
        assert!(video.kind.is_known() && video.kind.unknown_note().is_none());
        let m = video.kind.media().expect("a video");
        assert!(matches!(m.transition, Transition::Other(k) if k.name() == "Wipe"));
        assert!(matches!(m.fit, Fit::Other(k) if k.name() == "Tile"));
        assert!(matches!(m.effects[0].kind, EffectKind::Other(k) if k.name() == "Glow"));
        assert!(!m.effects[0].kind.is_video() && !m.effects[0].kind.is_sound());
        assert!(
            m.sound_chain().is_empty(),
            "an effect it doesn't know does nothing"
        );
        assert!(matches!(video.continue_mode, ContinueMode::Other(_)));
        assert_ne!(video.continue_mode, ContinueMode::AutoFollow);
        assert!(matches!(video.color, Some(CueColor::Other(_))));
        assert_eq!(video.extra["new_cue_field"], json!(7));
        // The cue of a kind it doesn't know keeps its number, name and place,
        // and is marked.
        let holo = &show.cues[1];
        assert!(!holo.kind.is_known());
        assert_eq!((holo.id, holo.number.as_str()), (CueId(900), "7.5"));
        assert_eq!(holo.name, "Hologram rig");
        assert_eq!(holo.kind.cue_type(), CueType::Unknown);
        assert_eq!(
            holo.kind.unknown_note().unwrap(),
            "Hologram cue, made in a newer NewQ: can't play here"
        );
        // Written back: all of it but what can't be kept, the unknown cue
        // exactly (compared as JSON: serde_json writes an object's keys in
        // order, so a field this NewQ doesn't know may move within its object).
        let back = serde_json::to_value(&show).unwrap();
        assert_eq!(back["cues"][1], input["cues"][1]);
        assert_eq!(back, kept_part(input.clone()));
        // And read again by this NewQ, the same.
        let again: Show = serde_json::from_value(back.clone()).unwrap();
        assert_eq!(serde_json::to_value(&again).unwrap(), back);
    }

    #[test]
    fn a_newer_show_saved_here_keeps_its_unknown_cue_and_says_which_newq_saved_it() {
        let dir = std::env::temp_dir().join(format!("newq_test_compat_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("newer.newq");
        let input = newer_show();
        std::fs::write(&path, serde_json::to_string_pretty(&input).unwrap()).unwrap();
        let show = Show::load(&path).expect("opens");
        assert_eq!(show.cues.len(), 2);
        assert!(show.next_id > 900, "the unknown cue's id is counted");
        let out = dir.join("out.newq");
        show.save(&out).unwrap();
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
        assert_eq!(written["saved_with"], json!(THIS_NEWQ));
        assert_eq!(written["cues"][1], input["cues"][1], "written back as read");
        assert_eq!(written["new_show_setting"], input["new_show_setting"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cue_it_cant_read_does_nothing_when_fired_or_gone_to() {
        let mut show: Show = serde_json::from_value(newer_show()).unwrap();
        let holo = CueId(900);
        let mut e = crate::engine::Engine::for_tests();
        e.fire(&show, holo);
        e.tick_shared(&show, 1.0);
        assert!(e.active.is_empty());
        assert!(
            e.messages
                .iter()
                .any(|(_, m)| m.contains("can't play here")),
            "{:?}",
            e.messages
        );
        // GO on it.
        e.playhead = Some(holo);
        e.go(&show);
        e.tick_shared(&show, 2.0);
        assert!(e.active.iter().all(|a| a.cue_id != holo));
        // A group with a mode this NewQ doesn't know: the group can't play,
        // and its child stays its own.
        let mut group = show.make_cue(CueType::Group, None, 0);
        let mut child = show.make_cue(CueType::Wait, None, 0);
        child.parent = Some(group.id);
        let gid = group.id;
        let mut gv = serde_json::to_value(&group).unwrap();
        gv["kind"]["Group"]["mode"] = json!("Shuffle");
        group = serde_json::from_value(gv.clone()).unwrap();
        assert!(!group.kind.is_known());
        assert_eq!(
            group.kind.unknown_note().unwrap(),
            "Group cue with settings from a newer NewQ: can't play here"
        );
        show.cues.push(group);
        show.cues.push(child);
        show.normalize_groups();
        assert_eq!(
            show.cues.last().unwrap().parent,
            Some(gid),
            "still its child"
        );
        assert_eq!(serde_json::to_value(show.cue(gid).unwrap()).unwrap(), gv);
        e.fire(&show, gid);
        e.tick_shared(&show, 3.0);
        assert!(
            e.active.is_empty(),
            "neither the group nor its child played"
        );
        // A control cue whose action it doesn't know is one it can't read, so
        // it never does some other action in its place.
        let ctl = show.make_cue(CueType::Control, None, 0);
        let mut cv = serde_json::to_value(&ctl).unwrap();
        cv["kind"]["Control"]["action"] = json!("Teleport");
        let ctl: Cue = serde_json::from_value(cv.clone()).unwrap();
        assert!(!ctl.kind.is_known());
        assert_eq!(serde_json::to_value(&ctl).unwrap(), cv);
    }

    #[test]
    fn a_transition_and_fit_it_doesnt_know_play_as_a_fade_in_letterboxed() {
        let mut show = Show::default();
        let mut c = show.make_cue(CueType::Text, None, 0);
        let mut v = serde_json::to_value(&c).unwrap();
        v["kind"]["Text"]["media"]["transition"] = json!("Wipe");
        v["kind"]["Text"]["media"]["fit"] = json!("Tile");
        v["kind"]["Text"]["media"]["fade_in"] = json!(0.0);
        c = serde_json::from_value(v.clone()).unwrap();
        assert!(
            c.kind.is_known(),
            "a text cue with a newer transition is still one"
        );
        assert_eq!(serde_json::to_value(&c).unwrap(), v);
        let fit = c.kind.media().unwrap().fit;
        assert_eq!(
            crate::gpu::fit_rect((100, 50), (200, 200), fit),
            crate::gpu::fit_rect((100, 50), (200, 200), Fit::Contain)
        );
        let id = c.id;
        show.cues.push(c);
        let mut e = crate::engine::Engine::for_tests();
        e.fire(&show, id);
        let t0 = std::time::Instant::now();
        let mut now = 0.0;
        while !e.active.iter().any(|a| a.cue_id == id && a.started) {
            assert!(t0.elapsed().as_secs() < 30, "the text cue never started");
            now += 0.02;
            e.tick_shared(&show, now);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn the_known_kinds_are_the_names_cue_kinds_are_written_under() {
        let mut show = Show::default();
        let mut names = Vec::new();
        for ty in [
            CueType::Video,
            CueType::Image,
            CueType::Audio,
            CueType::Fade,
            CueType::Stop,
            CueType::Wait,
            CueType::Group,
            CueType::Text,
            CueType::Shape,
            CueType::Camera,
            CueType::Send,
            CueType::LiveAudio,
            CueType::Control,
            CueType::Timecode,
        ] {
            let c = show.make_cue(ty, None, 0);
            names.push(super::kind_name(&serde_json::to_value(&c.kind).unwrap()));
        }
        assert_eq!(names, super::KNOWN_KINDS);
        // Asked for, an Unknown cue is never made.
        assert!(show.make_cue(CueType::Unknown, None, 0).kind.is_known());
    }

    #[test]
    fn an_ordinary_show_is_written_as_before_and_its_file_says_which_newq_wrote_it() {
        let mut show = Show::default();
        for ty in [
            CueType::Video,
            CueType::Text,
            CueType::Group,
            CueType::Control,
        ] {
            let c = show.make_cue(ty, None, show.cues.len());
            show.cues.push(c);
        }
        let plain = serde_json::to_string_pretty(&show).unwrap();
        assert!(
            !plain.contains("saved_with") && !plain.contains("extra"),
            "{plain}"
        );
        let dir = std::env::temp_dir().join(format!("newq_test_plain_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("plain.newq");
        show.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        // The file is the show as it was written before, with one line more.
        let line = format!("\n  \"saved_with\": \"{THIS_NEWQ}\",");
        assert!(text.contains(&line), "{text}");
        assert_eq!(text.replacen(&line, "", 1), plain);
        // A file from before (no such line) reads with none, and a file from
        // this NewQ reads as from this one.
        let old: Show = serde_json::from_str(&plain).unwrap();
        assert_eq!(old.saved_with, None);
        assert_eq!(
            Show::load(&path).unwrap().saved_with.as_deref(),
            Some(THIS_NEWQ)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn versions_compare_by_their_numbers() {
        use super::newer_than;
        assert!(newer_than("0.28.0", "0.27.1"));
        assert!(newer_than("0.27.10", "0.27.9"));
        assert!(newer_than("1.0", "0.99.99"));
        assert!(!newer_than("0.27.1", "0.27.1"));
        assert!(!newer_than("0.27.1-beta", "0.27.1"));
        assert!(!newer_than("0.9.0", "0.27.1"));
        // One it can't read as a version is taken as newer.
        assert!(newer_than("someday", "0.27.1"));
        assert!(!super::newer_than_this(THIS_NEWQ));
    }

    #[test]
    fn a_value_it_doesnt_know_is_kept_once_however_often_it_is_read() {
        let a: Transition = serde_json::from_str("\"Wipe\"").unwrap();
        let b: Transition = serde_json::from_str("\"Wipe\"").unwrap();
        let c: Transition = serde_json::from_str("\"Iris\"").unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(serde_json::to_string(&a).unwrap(), "\"Wipe\"");
        // The ones it knows read as themselves.
        for t in Transition::ALL {
            let j = serde_json::to_string(&t).unwrap();
            assert_eq!(serde_json::from_str::<Transition>(&j).unwrap(), t);
        }
        // Kinds outside a cue that aren't kept read as their default.
        let s: Show = serde_json::from_value(json!({"version": 2, "name": "x",
            "outputs": [], "cues": [], "next_id": 1, "wait_cards": "Sometimes",
            "lists": [{"id": 1, "name": "T", "timeline": {"tracks": [{"Light": 3}, "Cues"]}}]}))
        .unwrap();
        assert_eq!(s.wait_cards, WaitCards::default());
        let tl = s.lists[0].timeline.as_ref().unwrap();
        assert_eq!(tl.tracks, vec![crate::timeline::TrackId::Cues]);
    }
}
