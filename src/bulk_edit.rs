//! Editing several cues at once: what was changed in one cue (the one the
//! inspector shows) is made in the others picked too.
//!
//! The change is found by comparing the cue as JSON before and after the
//! inspector drew it, field by field, and the same fields are set in each other
//! cue. A field the other cue doesn't have is left out, so a fade time changed
//! in a video reaches the pictures and sounds picked with it and skips a wait.
//!
//! Three things make it more than a diff:
//! * a cue is saved without the fields that have their default value, so a
//!   missing position, colour correction or motion is read as the default one
//!   (otherwise turning one number of it would copy the whole thing over the
//!   others' own settings);
//! * a list (effects, routing) is compared item by item and only changed where
//!   the other cue has the same item, since two cues' lists are not alike;
//! * the playback settings of a text, shape or camera cue are in a `media`
//!   part of it, and those of a video, picture or sound are the cue itself,
//!   so they are matched up.

use crate::show::{ColorAdjust, Cue, LayerTransform, Motion};
use serde_json::Value;

/// Fields that make a cue its own: they are never copied to the others.
const OWN: &[&str] = &[
    "id",
    "number",
    "name",
    "notes",
    "parent",
    "list",
    "hotkey",
    "midi_note",
    "timecode",
    "clock",
];

/// Fields inside a cue's kind that belong to that one cue (its file, its text).
/// (A fade, stop or control cue's target and a camera's or sound input's device
/// are as much its own: copying them would aim every picked cue at one thing.)
const OWN_IN_KIND: &[&str] = &["file", "text", "target", "device"];

/// Playback settings that mean something different in each sort of cue (a
/// picture's hold time is a video's out point): made only in cues of the same
/// sort, so holding a picture for 5 s doesn't trim the videos picked with it.
const SAME_KIND_ONLY: &[&str] = &["start", "end"];

/// The kinds that are a `MediaCue` themselves; the others hold one as `media`.
const MEDIA_KINDS: &[&str] = &["Video", "Image", "Audio"];

type Path = Vec<String>;

/// Where a change is made.
#[derive(Clone, Debug, PartialEq)]
enum Scope {
    /// A field of the cue itself.
    Cue,
    /// A playback setting (the cue's `MediaCue`, whatever sort of cue it is).
    Media,
    /// Another field of the kind, for a cue of the same kind.
    Kind,
}

#[derive(Clone, Debug)]
struct Item {
    scope: Scope,
    path: Path,
    /// The new value (None: the field is gone, which means its default).
    value: Option<Value>,
}

/// What changed in one cue, ready to be made in others.
#[derive(Clone, Debug)]
pub struct Change {
    /// The kind of the cue it was changed in ("Video", "Shape"…).
    variant: String,
    items: Vec<Item>,
}

/// What a field that is left out of the file stands for, for the fields that
/// are whole structures with a number of settings in them.
fn default_object(key: &str) -> Option<Value> {
    match key {
        "transform" => serde_json::to_value(LayerTransform::default()).ok(),
        "color" => serde_json::to_value(ColorAdjust::default()).ok(),
        "motion" => serde_json::to_value(Motion::default()).ok(),
        _ => None,
    }
}

/// The leaves that differ between `before` and `after`: the path to each and
/// its new value.
fn changes(before: &Value, after: &Value, path: &mut Path, out: &mut Vec<(Path, Option<Value>)>) {
    match (before, after) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, new) in b {
                path.push(k.clone());
                match a.get(k) {
                    Some(old) => changes(old, new, path, out),
                    None => match default_object(k).filter(|_| new.is_object()) {
                        Some(default) => changes(&default, new, path, out),
                        // A list left out of the file is an empty one: one that
                        // appears has grown, which is not spread (below).
                        None if new.is_array() => {
                            changes(&Value::Array(Vec::new()), new, path, out)
                        }
                        None => out.push((path.clone(), Some(new.clone()))),
                    },
                }
                path.pop();
            }
            for (k, old) in a.iter().filter(|(k, _)| !b.contains_key(*k)) {
                path.push(k.clone());
                match default_object(k).filter(|_| old.is_object()) {
                    Some(default) => changes(old, &default, path, out),
                    // A list that became empty has shrunk: not spread either.
                    None if old.is_array() => changes(old, &Value::Array(Vec::new()), path, out),
                    None => out.push((path.clone(), None)),
                }
                path.pop();
            }
        }
        // Lists are compared item by item when they are as long as each other; one
        // that has grown or shrunk is not spread (it would be a different list).
        (Value::Array(a), Value::Array(b)) => {
            if a.len() == b.len() {
                for (i, (old, new)) in a.iter().zip(b).enumerate() {
                    // An item with a kind (an effect) is changed only where the
                    // other cue has the same kind there: a blur's radius is not a
                    // sharpen's amount.
                    path.push(match old.get("kind") {
                        Some(kind) => format!("{i}{KIND_MARK}{kind}"),
                        None => i.to_string(),
                    });
                    changes(old, new, path, out);
                    path.pop();
                }
            }
        }
        _ => {
            if before != after {
                out.push((path.clone(), Some(after.clone())));
            }
        }
    }
}

/// Between a list item's place and its kind in a path ("0#\"Blur\"").
const KIND_MARK: char = '#';

/// The item of `items` that path part `k` names: by its place, and when the part
/// gives a kind, only if the item there is of that kind.
fn item<'a>(items: &'a mut [Value], k: &str) -> Option<&'a mut Value> {
    let (at, kind) = match k.split_once(KIND_MARK) {
        Some((at, kind)) => (at, Some(serde_json::from_str::<Value>(kind).ok()?)),
        None => (k, None),
    };
    let slot = items.get_mut(at.parse::<usize>().ok()?)?;
    match kind {
        Some(kind) if slot.get("kind") != Some(&kind) => None,
        _ => Some(slot),
    }
}

/// Whether a path part names a list item rather than a field.
fn is_item(k: &str) -> bool {
    k.split(KIND_MARK)
        .next()
        .is_some_and(|at| at.parse::<usize>().is_ok())
}

/// Make the change at `path` (relative to `root`) if `root` has a place for it.
fn set(root: &mut Value, path: &[String], value: Option<&Value>) -> bool {
    let Some((last, parents)) = path.split_last() else {
        return false;
    };
    let mut at = root;
    for (n, k) in parents.iter().enumerate() {
        let next = path.get(n + 1).map(String::as_str).unwrap_or("");
        at = match at {
            Value::Object(o) => {
                if !o.contains_key(k) {
                    // A field left out of the file is the default one: make it, to
                    // change a part of. Not a list (it is empty: no item to change).
                    if value.is_none() || is_item(next) {
                        return false;
                    }
                    o.insert(k.clone(), Value::Object(Default::default()));
                }
                match o.get_mut(k) {
                    Some(next) => next,
                    None => return false,
                }
            }
            Value::Array(a) => match item(a, k) {
                Some(next) => next,
                None => return false,
            },
            _ => return false,
        };
    }
    // A number stays a number: a field that means something else here is left alone.
    let fits = |old: &Value, v: &Value| {
        old.is_null() || v.is_null() || std::mem::discriminant(old) == std::mem::discriminant(v)
    };
    match at {
        Value::Object(obj) => match value {
            Some(v) => {
                if obj.get(last).is_some_and(|old| !fits(old, v)) {
                    return false;
                }
                obj.insert(last.clone(), v.clone());
                true
            }
            None => {
                obj.remove(last);
                true
            }
        },
        Value::Array(items) => {
            let (Some(slot), Some(v)) = (item(items, last), value) else {
                return false;
            };
            if !fits(slot, v) {
                return false;
            }
            *slot = v.clone();
            true
        }
        _ => false,
    }
}

/// The kind of a cue as JSON: its name and the part of it with the fields.
fn kind_of(value: &mut Value) -> Option<(String, &mut Value)> {
    let (name, inner) = value.get_mut("kind")?.as_object_mut()?.iter_mut().next()?;
    Some((name.clone(), inner))
}

/// What changed between `before` and `after`, or None if nothing did (or nothing
/// that can be spread).
pub fn diff(before: &Cue, after: &Cue) -> Option<Change> {
    let (mut before, mut after) = (
        serde_json::to_value(before).ok()?,
        serde_json::to_value(after).ok()?,
    );
    if before == after {
        return None;
    }
    let variant = kind_of(&mut after)?.0;
    let mut raw = Vec::new();
    changes(&before, &after, &mut Vec::new(), &mut raw);
    let _ = kind_of(&mut before);
    let mut items = Vec::new();
    for (path, value) in raw {
        let Some(first) = path.first() else {
            continue;
        };
        if OWN.contains(&first.as_str()) {
            continue;
        }
        if first != "kind" {
            items.push(Item {
                scope: Scope::Cue,
                path,
                value,
            });
            continue;
        }
        // ["kind", "Video", ..fields..]: the fields are what matter. A change of
        // kind itself is not spread, nor is a cue's own file or text.
        if path.len() < 3 || OWN_IN_KIND.contains(&path[path.len() - 1].as_str()) {
            continue;
        }
        let (scope, rest) = if MEDIA_KINDS.contains(&path[1].as_str()) {
            (Scope::Media, &path[2..])
        } else if path[2] == "media" {
            (Scope::Media, &path[3..])
        } else {
            (Scope::Kind, &path[2..])
        };
        if !rest.is_empty() {
            items.push(Item {
                scope,
                path: rest.to_vec(),
                value,
            });
        }
    }
    (!items.is_empty()).then_some(Change { variant, items })
}

impl Change {
    /// `target` with the change made in it, or None if none of it applies.
    pub fn apply(&self, target: &Cue) -> Option<Cue> {
        let before = serde_json::to_value(target).ok()?;
        let mut value = before.clone();
        let mut any = false;
        for item in &self.items {
            any |= match item.scope {
                Scope::Cue => set(&mut value, &item.path, item.value.as_ref()),
                Scope::Media | Scope::Kind => {
                    let Some((name, inner)) = kind_of(&mut value) else {
                        continue;
                    };
                    let same_kind_only = item
                        .path
                        .first()
                        .is_some_and(|f| SAME_KIND_ONLY.contains(&f.as_str()));
                    if item.scope == Scope::Kind || same_kind_only {
                        name == self.variant
                            && if item.scope == Scope::Kind || MEDIA_KINDS.contains(&name.as_str())
                            {
                                set(inner, &item.path, item.value.as_ref())
                            } else {
                                inner
                                    .get_mut("media")
                                    .is_some_and(|m| set(m, &item.path, item.value.as_ref()))
                            }
                    } else if MEDIA_KINDS.contains(&name.as_str()) {
                        set(inner, &item.path, item.value.as_ref())
                    } else {
                        inner
                            .get_mut("media")
                            .is_some_and(|m| set(m, &item.path, item.value.as_ref()))
                    }
                }
            };
        }
        if !any || value == before {
            return None;
        }
        serde_json::from_value(value).ok()
    }
}

/// `target` with what changed between `before` and `after` made in it, or
/// None if nothing of it applies.
#[cfg(test)]
pub fn spread(before: &Cue, after: &Cue, target: &Cue) -> Option<Cue> {
    diff(before, after)?.apply(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::show::*;

    fn cues() -> Vec<Cue> {
        let mut show = Show::default();
        let mut out = Vec::new();
        for ty in [
            CueType::Wait,
            CueType::Wait,
            CueType::Image,
            CueType::Video,
            CueType::Audio,
            CueType::Shape,
            CueType::Text,
        ] {
            let c = show.make_cue(ty, None, 0);
            out.push(c);
        }
        out
    }

    #[test]
    fn a_changed_duration_reaches_the_other_waits_and_leaves_their_names_alone() {
        let all = cues();
        let before = all[0].clone();
        let mut after = before.clone();
        after.name = "renamed".into();
        if let CueKind::Wait(w) = &mut after.kind {
            w.duration = 12.5;
        }
        after.pre_wait = 3.0;
        let moved = spread(&before, &after, &all[1]).unwrap();
        match &moved.kind {
            CueKind::Wait(w) => assert_eq!(w.duration, 12.5),
            _ => panic!("still a wait"),
        }
        assert_eq!(moved.pre_wait, 3.0);
        assert_eq!(moved.name, all[1].name, "a name is its own");
        assert_eq!(moved.id, all[1].id);
        assert_eq!(moved.number, all[1].number);
    }

    #[test]
    fn a_change_to_a_picture_cue_reaches_video_and_sound_cues_with_the_same_field() {
        let all = cues();
        let before = all[2].clone();
        let mut after = before.clone();
        let m = after.kind.media_mut().unwrap();
        m.fade_in = 2.5;
        m.layer = 4;
        m.volume_db = -9.0;
        // The file is each cue's own.
        m.file = "other.png".into();
        for target in [&all[3], &all[4]] {
            let moved = spread(&before, &after, target).unwrap();
            let m = moved.kind.media().unwrap();
            assert_eq!(m.fade_in, 2.5);
            assert_eq!(m.volume_db, -9.0);
            assert_eq!(m.layer, 4);
            assert_eq!(m.file, target.kind.media().unwrap().file, "the file stays");
        }
        // A wait has no such fields: nothing of it applies, and it stays a wait.
        assert!(spread(&before, &after, &all[0]).is_none());
        // Nothing changed, nothing to spread.
        assert!(spread(&before, &before, &all[3]).is_none());
    }

    #[test]
    fn the_playback_settings_reach_text_shape_and_camera_cues_and_back() {
        let all = cues();
        // From a video to a shape and a text.
        let before = all[3].clone();
        let mut after = before.clone();
        let m = after.kind.media_mut().unwrap();
        m.layer = 3;
        m.fade_out = 4.0;
        for target in [&all[5], &all[6]] {
            let moved = spread(&before, &after, target).unwrap();
            let m = moved.kind.media().unwrap();
            assert_eq!((m.layer, m.fade_out), (3, 4.0));
        }
        // From a shape to a video.
        let before = all[5].clone();
        let mut after = before.clone();
        after.kind.media_mut().unwrap().layer = -2;
        let moved = spread(&before, &after, &all[3]).unwrap();
        assert_eq!(moved.kind.media().unwrap().layer, -2);
        // A colour is a shape's own thing: it goes to another shape, not to a text.
        let mut after = before.clone();
        if let CueKind::Shape(s) = &mut after.kind {
            s.color = [1, 2, 3, 255];
            s.shape = ShapeKind::Star;
        }
        let mut other = all[5].clone();
        other.id = CueId(900);
        let moved = spread(&before, &after, &other).unwrap();
        assert!(
            matches!(&moved.kind, CueKind::Shape(s) if s.color == [1, 2, 3, 255] && s.shape == ShapeKind::Star)
        );
        assert!(spread(&before, &after, &all[6]).is_none());
    }

    #[test]
    fn a_field_set_back_to_its_default_is_set_back_in_the_others_too() {
        let all = cues();
        let mut before = all[3].clone();
        before.kind.media_mut().unwrap().layer = 6;
        let mut after = before.clone();
        after.kind.media_mut().unwrap().layer = 0;
        let mut target = all[4].clone();
        target.kind.media_mut().unwrap().layer = 6;
        let moved = spread(&before, &after, &target).unwrap();
        assert_eq!(moved.kind.media().unwrap().layer, 0);
        // Disarming is a change like another.
        let mut off = all[0].clone();
        off.armed = false;
        let moved = spread(&all[0], &off, &all[1]).unwrap();
        assert!(!moved.armed);
    }

    #[test]
    fn changing_one_number_of_the_geometry_leaves_the_others_own_position_alone() {
        let all = cues();
        // The cue in the inspector has the default geometry; another has its own.
        let a = all[3].clone();
        let mut b = all[4].clone();
        {
            let t = &mut b.kind.media_mut().unwrap().transform;
            t.x = 0.3;
            t.scale_x = 0.5;
        }
        let mut turned = a.clone();
        turned.kind.media_mut().unwrap().transform.rotation = 30.0;
        let moved = spread(&a, &turned, &b).unwrap();
        let t = moved.kind.media().unwrap().transform;
        assert_eq!((t.rotation, t.x, t.scale_x), (30.0, 0.3, 0.5));
        // The other way: the cue has geometry of its own and the target has none.
        let mut c = a.clone();
        c.kind.media_mut().unwrap().transform.x = 0.2;
        let mut scaled = c.clone();
        scaled.kind.media_mut().unwrap().transform.scale_y = 2.0;
        let moved = spread(&c, &scaled, &all[4]).unwrap();
        let t = moved.kind.media().unwrap().transform;
        assert_eq!(
            (t.scale_y, t.x),
            (2.0, 0.0),
            "the scale reaches it, the position is not copied"
        );
        // Setting the cue's geometry back to the default sets the number back, not the lot.
        let mut reset = c.clone();
        reset.kind.media_mut().unwrap().transform.x = 0.0;
        let moved = spread(&c, &reset, &b).unwrap();
        let t = moved.kind.media().unwrap().transform;
        assert_eq!((t.x, t.scale_x), (0.0, 0.5));
        // Colour correction and motion are the same.
        let mut tinted = a.clone();
        tinted.kind.media_mut().unwrap().color.brightness = 0.25;
        let mut other = all[4].clone();
        other.kind.media_mut().unwrap().color.contrast = 1.4;
        let moved = spread(&a, &tinted, &other).unwrap();
        let c = moved.kind.media().unwrap().color;
        assert_eq!((c.brightness, c.contrast), (0.25, 1.4));
    }

    #[test]
    fn a_list_is_changed_item_by_item_and_only_where_the_other_cue_has_that_item() {
        let all = cues();
        let fx = |kind: EffectKind| Effect::new(kind);
        let mut a = all[3].clone();
        a.kind.media_mut().unwrap().effects = vec![fx(EffectKind::Blur)];
        let mut b = all[4].clone();
        b.kind.media_mut().unwrap().effects = vec![fx(EffectKind::Sharpen)];
        let mut c = all[2].clone();
        c.kind.media_mut().unwrap().effects = vec![fx(EffectKind::Blur)];
        let mut tweaked = a.clone();
        tweaked.kind.media_mut().unwrap().effects[0].p[0] = Animated::new(7.0);
        // A blur's setting is not a sharpen's: that cue is left as it is...
        assert!(
            spread(&a, &tweaked, &b).is_none(),
            "a sharpen took a blur's setting"
        );
        // ...and reaches the cue with a blur there.
        let moved = spread(&a, &tweaked, &c).unwrap();
        let e = &moved.kind.media().unwrap().effects;
        assert_eq!(e.len(), 1, "no effect is added or replaced");
        assert!(matches!(e[0].kind, EffectKind::Blur));
        assert_eq!(e[0].p[0], Animated::new(7.0));
        // A list that grew is not copied.
        let mut more = a.clone();
        more.kind
            .media_mut()
            .unwrap()
            .effects
            .push(fx(EffectKind::Blur));
        assert!(spread(&a, &more, &b).is_none());
    }

    #[test]
    fn the_first_effect_added_or_the_last_taken_away_leaves_the_others_lists_alone() {
        let all = cues();
        let fx = |kind: EffectKind| Effect::new(kind);
        // The cue edited has no effects (none in its file); the other has two.
        let a = all[3].clone();
        let mut b = all[2].clone();
        b.kind.media_mut().unwrap().effects =
            vec![fx(EffectKind::Sharpen), fx(EffectKind::Vignette)];
        let mut added = a.clone();
        added.kind.media_mut().unwrap().effects = vec![fx(EffectKind::Blur)];
        assert!(
            spread(&a, &added, &b).is_none(),
            "the other's effects were replaced"
        );
        // And taking the edited cue's only effect away doesn't strip the other's.
        assert!(
            spread(&added, &a, &b).is_none(),
            "the other's effects were deleted"
        );
        // The same for the sound's routing.
        let mut routed = a.clone();
        routed.kind.media_mut().unwrap().matrix = vec![Crosspoint {
            from: 0,
            to: 1,
            db: -6.0,
        }];
        assert!(spread(&a, &routed, &b).is_none());
        // A change elsewhere in the same edit still gets through.
        let mut both = added.clone();
        both.pre_wait = 2.0;
        assert_eq!(spread(&a, &both, &b).unwrap().pre_wait, 2.0);
    }

    #[test]
    fn a_cues_target_device_and_hold_time_stay_its_own() {
        let mut show = Show::default();
        let fade = show.make_cue(CueType::Fade, None, 0);
        let other_fade = show.make_cue(CueType::Fade, None, 0);
        let mut aimed = fade.clone();
        if let CueKind::Fade(f) = &mut aimed.kind {
            f.target = Some(CueId(99));
        }
        assert!(
            spread(&fade, &aimed, &other_fade).is_none(),
            "a fade was re-aimed"
        );
        let cam = show.make_cue(CueType::Camera, None, 0);
        let other_cam = show.make_cue(CueType::Camera, None, 0);
        let mut moved = cam.clone();
        if let CueKind::Camera(c) = &mut moved.kind {
            c.device = "Another camera".into();
        }
        assert!(
            spread(&cam, &moved, &other_cam).is_none(),
            "a camera was moved"
        );
        // A picture held for 5 s doesn't trim a video to 5 s, but another picture
        // is held for 5 s too.
        let picture = show.make_cue(CueType::Image, None, 0);
        let video = show.make_cue(CueType::Video, None, 0);
        let other_picture = show.make_cue(CueType::Image, None, 0);
        let mut held = picture.clone();
        held.kind.media_mut().unwrap().end = Some(5.0);
        assert!(
            spread(&picture, &held, &video).is_none(),
            "the video was trimmed"
        );
        assert_eq!(
            spread(&picture, &held, &other_picture)
                .unwrap()
                .kind
                .media()
                .unwrap()
                .end,
            Some(5.0)
        );
    }
}
