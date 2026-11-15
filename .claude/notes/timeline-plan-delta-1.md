# Timeline plan: delta to revision 3 (owner's rulings on section 7)

Read-only investigation only; nothing built or run. Everything in revision 3 not mentioned here stands.

For the P0 agent, section B is the complete list. In short: one enum changes (`RunOut`), four fields are added (`MediaCue::also_on`, `ControlCue::freeze_live`, `Show::main_timeline`, `Clip::also_on`), and one engine stub gains a parameter.

---

## A. Rulings, and the plan text that changes

### 1. Parked picture on the real outputs, with the show-mode lock: YES

No change. Section 2.7 stands; remove item 4 from section 7 and item 3 from 2.13's "needs the owner's yes".

### 2. Resume is a subset of Play; GO continues the run at a pause cue

Revision 3 already matches. For timelines there is no Resume concept:

- `TimelineRun` is the only "play" action. On a paused run it resumes; on a stopped one it runs from the playhead (or from `at`).
- GO at a pause cue continues the run (2.8 transport table). Everything after the pause cue happens where it sits on the timeline.
- No `TimelineResume` action exists and none is added.

One clarification to add to 2.8, under "Existing actions on a timeline group target":

> The old `Resume` action aimed at a timeline group is kept only so old shows still work; it does exactly what `TimelineRun` does. The inspector does not offer "Resume" for a timeline. `Pause`/`Resume` aimed at a single clip stay: they freeze and release that clip while the clock runs (2.2), which is a different thing from resuming the timeline.

### 3. GO does nothing while a standalone timeline is running: CORRECT

No change.

### 4. A clip on one or more displays (replaces "tracks are (display, layer), undecided")

**Design: an extra list on the cue, not a change to `output`.**

- `MediaCue::output` stays the clip's *home* display. New field `MediaCue::also_on: Vec<OutputId>` lists further displays.
- Changing `output` itself into a list would touch every reader of `output` (preload sizing, sync delay, `apply_entry`, dips, the Patch view, paste mapping, Test Mode). The extra list touches only the places below.

**What it costs:**

| Area | Change |
|---|---|
| `Engine::layers(output)` | Also returns a cue's layer when `output` is in its `also_on`. `VideoState`/`ImageState` get an `also: Vec<OutputId>` copied from the show each tick, next to the existing `v.output = m.output` copy, so an edit applies live. |
| Renderer (`app.rs::update_outputs`) | No change expected. Textures are kept per cue instance (`self.textures.entry(l.instance)`), uploaded once per frame serial, and each window already composites its own layer list with its own `Slot`. This is the same situation as `OutputConfig::part_of` today. To be confirmed by a test; I only read the code. |
| Preload sizing (`output_size`, `PrepSig::size`) | Decode size becomes the largest of the clip's displays, so the picture is sharp on the biggest. A text or shape is drawn at the home display's size and stretched on the others. |
| `apply_entry` | "Same display" becomes "shares a display": an entering clip replaces or crossfades a clip on its layer when their display sets intersect. The replaced clip goes from *all* its displays, not only the shared ones. |
| Dips (`begin_dip`) | One matte per display of the clip; the reveal is keyed to the home display's matte. |
| Sync (`video_lag`) | The home display's delay is used for all. |
| `Show::remove_output`, paste mapping | Clean and map `also_on` as `output` is. |
| Chase (`chase.rs`) | Replacement mirrors the intersection rule. |
| Show file | One field, `#[serde(default)]`, not written when empty. Old shows unchanged. |

**Tracks editor.** `TrackId` stays `Picture { output, layer }`, keyed by the *home* display. Section 2.9's rule is confirmed with this addition:

> A clip lives on the track of its home display and layer. If it is also on other displays, it carries a badge ("+2", listing the displays on hover), and a ghost of it (outlined, not draggable) is drawn on the same-layer track of each other display, so the operator sees that those tracks are occupied. Dragging the clip to another display's track changes its home; the "Also on" list is edited in the inspector.

**Who builds it.** Doing this in wave 1 would endanger it: it changes `apply_entry` and `layers`, which P2a and P2b lean on. So:

- **P0 (now):** the field, `remove_output`/paste handling, and `Clip::also_on`. The file format then does not change twice.
- **New package P9, wave 2:** the engine work (see C).
- **P4 (wave 2):** the inspector's "Also on" row.
- **P6 (wave 2):** the badge and ghosts.

Until P9 lands, `also_on` is stored and ignored. P3 must not show the field before then.

### 5. `run_out` default: end, with the default fade (CHANGED)

Replace the table in 2.4 with:

| Value | Behaviour |
|---|---|
| `End` (default, not written) | Ends, fading out. Picture and sound alike. |
| `Loop` | Goes round again from its in point until the timeline ends it. |
| `Hold` | Holds its last frame (sound: silence). |

`Auto` is removed. The fade on End:

- `run_out_fade: None` (default, not written): the cue's own fade-out time, and where that is 0, the default fade (`DEFAULT_FADE_OUT`, 2 s). This is what `MediaCue::manual_fade_out()` already returns.
- `Some(t)`: `t` seconds. `Some(0.0)`: a hard cut.
- The fade finishes as the content ends.

Unchanged: `run_out` is only read for clocked clips (2.3), so old shows behave as today. A clip that already loops for ever never runs out.

### 6. Live inputs at "pause on a frame": the pause cue chooses (CHANGED, ships in the first version)

Replace the "Camera, live sound" row of the freeze table in 2.2 with:

| Kind | Freeze |
|---|---|
| Camera, NDI source, live sound | Per pause cue: `ControlCue::freeze_live`. **Off (default): they continue live.** On: a camera or NDI source holds its last frame, live sound is muted; on resume they are live again at once (nothing buffered is played back). |

Rules that go with it:

- `freeze_live` only matters when `freeze` is `Everything` or `Clips`.
- The transport's Pause button on a timeline leaves live inputs running.
- A clip-level `Pause` cue aimed at a live clip freezes it. Naming the clip is the choice.
- NDI needs nothing separate: an NDI source is a camera cue's device.
- Implementation: `VideoPlayer::set_frozen(bool)` for live players. While frozen it keeps draining the reader and discards frames, so there is no backlog on resume. For live sound, the voice is muted and its input discarded while frozen. Both move from P5 into P2a.
- Remove item 12 from 2.13 and the "optional camera freeze" item from P5.

### 7. Any list, the main one included, can be a timeline (restriction dropped)

I agree, and it is cheap. Your reading has two halves:

- "A timeline cue can sit in any list": already true in revision 3.
- "The main list may be of the timeline type": new. The main list has no `CueList` struct, so it needs one field: `Show::main_timeline: Option<TimelineSettings>`.

Everything already goes through `timeline::settings(show, key)`, so `TimelineKey::List(0)` just reads the new field. Cost: small, all in P0 and P4.

If the owner meant only the first half, the field is harmless: unused, never written.

Replace the last bullet of 2.12 with:

> The main list can be a timeline: `Show::main_timeline`, absent for every existing show. `set_list_timeline(0, on)` works like any other list (and refuses a list that holds groups).

Remove item 9 from section 7.

### 8. Shows with the new control actions do not open in 0.20.3 and earlier: ACCEPTED

No change. Keep the README note.

---

## B. Changes to section 3 (Interfaces) — forward this to the P0 agent

Everything not listed is exactly as in revision 3.

### B1. `RunOut` (in `src/timeline/mod.rs`) — changed

Old:

```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunOut { #[default] Auto, Loop, Hold, End }
```

New:

```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunOut { #[default] End, Loop, Hold }
impl RunOut { pub fn is_end(&self) -> bool { *self == RunOut::End } }
```

### B2. `MediaCue` (in `src/show.rs`) — one field changed, one added

Old:

```rust
#[serde(default, skip_serializing_if = "RunOut::is_auto")] pub run_out: RunOut,
#[serde(default, skip_serializing_if = "Option::is_none")] pub run_out_fade: Option<f64>,
#[serde(default, skip_serializing_if = "is_false")]        pub runs_unseen: bool,
```

New:

```rust
/// What a clip does if its content runs out before the timeline moves on. Default: end, fading.
#[serde(default, skip_serializing_if = "RunOut::is_end")]  pub run_out: RunOut,
/// The fade when it ends that way. None = the cue's own fade-out, or the default fade
/// (`manual_fade_out()`); Some(0.0) = a cut.
#[serde(default, skip_serializing_if = "Option::is_none")] pub run_out_fade: Option<f64>,
#[serde(default, skip_serializing_if = "is_false")]        pub runs_unseen: bool,   // unchanged
/// Further displays this cue is shown on, besides `output` (its home). Empty = only `output`.
#[serde(default, skip_serializing_if = "Vec::is_empty")]   pub also_on: Vec<OutputId>,
```

With `also_on`, P0 must also:

- Initialise it to empty in `MediaCue::new` and every other `MediaCue` literal.
- In `Show::remove_output`: remove the id from every cue's `also_on`.
- In `Show::paste_cues`: map each id through the same display mapping `output` gets; drop ids that have no counterpart; drop one equal to the cue's `output`.
- In `MediaCue::decodes_like`: `also_on` must **not** count as a decoding difference in P0 (it is ignored until P9).
- Tests: default not written; round-trips; `remove_output` cleans it; paste maps it.

### B3. `ControlCue` (in `src/show.rs`) — one field added

Old (revision 3):

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]         pub timeline: Option<TimelineKey>,
#[serde(default, skip_serializing_if = "Option::is_none")]         pub at: Option<TimelinePoint>,
#[serde(default, skip_serializing_if = "PauseFreeze::is_nothing")] pub freeze: PauseFreeze,
#[serde(default, skip_serializing_if = "Vec::is_empty")]           pub clips: Vec<CueId>,
```

New (same four, plus):

```rust
/// A pause that freezes content also freezes live inputs (camera, NDI, live sound).
/// Default false: they carry on live.
#[serde(default, skip_serializing_if = "is_false")]                pub freeze_live: bool,
```

Test: default not written; round-trips.

### B4. `Show` (in `src/show.rs`) — one field added

New:

```rust
/// The main list is a timeline, with these settings. None = an ordinary cue list.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub main_timeline: Option<TimelineSettings>,
```

- Initialise to `None` in `impl Default for Show` and any other `Show` literal.
- Tests: default not written; a 0.20.3 file loads with `None`.

### B5. `src/timeline/model.rs` — behaviour of existing signatures changes, one field added

Signatures are unchanged; these now handle the main list:

```rust
pub fn is_timeline(show: &Show, key: TimelineKey) -> bool;
    // List(0): show.main_timeline.is_some()
pub fn settings(show: &Show, key: TimelineKey) -> Option<&TimelineSettings>;
    // List(0): show.main_timeline.as_ref()
pub fn settings_mut(show: &mut Show, key: TimelineKey) -> Option<&mut TimelineSettings>;
    // List(0): show.main_timeline.get_or_insert_with(Default::default)
pub fn key_of(show: &Show, cue: CueId) -> Option<TimelineKey>;
    // a top-level cue with list == 0 and main_timeline set -> Some(List(0))
pub fn members<'a>(show: &'a Show, key: TimelineKey) -> Vec<&'a Cue>;
    // List(0): the top-level cues of list 0
```

`settle` is unchanged: it only resets a *group's* default settings to `None`. A list's `Some(default)` means "this list is a timeline" and is kept, for the main list too.

`Clip` gains one field:

```rust
pub struct Clip {
    /* … as revision 3 … */
    pub also_on: Vec<OutputId>,     // further displays; the track is the home display's
}
```

`track_of(cue)` is unchanged: `TrackId::Picture { output: m.output, layer: m.layer }`.

Tests to add: every accessor above with `List(0)`; `clips` copies `also_on`.

### B6. `src/timeline/edit.rs` — signatures unchanged, one behaviour change

```rust
pub fn set_list_timeline(show: &mut Show, list: u32, on: bool) -> Result<(), String>;
    // list 0 is allowed: sets or clears show.main_timeline; still refuses a list that holds groups
```

This is P1's to implement; P0 only needs the stub to accept 0.

### B7. Engine stub (`src/engine/timeline.rs`) — one signature changed

Old:

```rust
pub fn timeline_pause(&mut self, show: &Show, key: TimelineKey, freeze: PauseFreeze,
                      clips: &[CueId], waits_for_go: bool);
```

New:

```rust
/// `live`: live inputs among the frozen clips freeze too (camera holds its frame, live sound mutes).
pub fn timeline_pause(&mut self, show: &Show, key: TimelineKey, freeze: PauseFreeze,
                      clips: &[CueId], live: bool, waits_for_go: bool);
```

All other engine signatures in 3.5 are unchanged, and `TimelineAction` in 3.6 is unchanged.

### B8. Compatibility table (2.12) — rows changed or added

| Field | Default (not written) | Note |
|---|---|---|
| `MediaCue::run_out: RunOut` | `End` | Was `Auto`. Only read for clocked clips. |
| `MediaCue::also_on: Vec<OutputId>` | empty | New. Stored from P0, used from P9. |
| `ControlCue::freeze_live: bool` | `false` | New. |
| `Show::main_timeline: Option<TimelineSettings>` | `None` | New. |

---

## C. Changes to the package list

### P0 · Skeleton and data model (wave 0) — still **M**

- All of section B.
- Tests added: those named in B2, B3, B4 and B5.
- Remove from its tests anything that names `RunOut::Auto`.

### P1 · Editing library (wave 1) — unchanged size

- `set_list_timeline(0, ..)` works; test converting the main list both ways and the refusal when it holds groups.
- `new_timeline_list` is unchanged (it still makes a new list; the main list is converted, not created).

### P2a · Engine: run, pause cues, GO-timed transitions (wave 1) — **L**, slightly larger

Scope added (moved in from P5):

- Live inputs under a freezing pause, per `freeze_live`: `VideoPlayer::set_frozen`, and muting plus discarding for live sound.
- A clip-level `Pause` on a live clip freezes it.

Scope changed:

- `run_out` default is End with the fade from `run_out_fade`, else `manual_fade_out()`.

Files added to its ownership:

- `src/media/video.rs`: only `VideoPlayer::update` and the new `set_frozen`.
- `src/media/audio.rs`: only what a live voice needs to discard input while frozen. I have not read that code; the builder must check whether `set_paused` on a live voice already discards.

Tests added or changed:

- `run_out` cases:
  - default: a 3 s clip paused 5 s has faded over its own fade-out;
  - with `fade_out == 0` it fades over 2 s;
  - `Some(0.0)` cuts;
  - `Loop` and `Hold` as before;
  - picture and sound both.
- With the test-pattern camera:
  - a freezing pause with `freeze_live` off: the shown-frame count keeps rising;
  - with it on: it stops and the last frame stays in `Engine::layers`;
  - after GO it rises again at once.
- The transport's pause leaves the camera live.
- A clip `Pause` cue on a camera freezes it.
- Live sound cannot be tested without a device; say so in the hand-back.

### P2b · Engine: seek, parking, opacity-0 (wave 1) — unchanged

- Chase tests cover a main-list timeline (`List(0)`) once.

### P3 · Tracks editor, Option A (wave 1) — unchanged size

- Control inspector: a "Live inputs freeze too" checkbox, shown when the freeze kind is Everything or These clips.
- Clip inspector: `run_out` offers End (default), Loop, Hold; the fade time shows for End.
- Does **not** show `also_on` (not working until P9).
- Tests: the checkbox appears only for freezing pauses; the `run_out` combo has three entries.

### P4 · Standalone Timeline tab (wave 2) — **M**, slightly larger

- "Make this list a timeline" is offered on the main list's tab too.
- The inspector's **"Also on"** row for picture cues: tick boxes for the other displays, written to `also_on`.
- The cue-list row shows "Display 1 +2".
- Tests: converting the main list; the row writes the field and is locked in show mode.

### P9 · A cue on several displays (wave 2, new) — **M**

- Owns `src/engine.rs` and `src/timeline/chase.rs`, so it is serial with P5 on the same lineage: **P9 first, then P5**.
- Scope: the table in A4 (`layers`, live copy each tick, decode size, `apply_entry` intersection, one matte per display, chase).
- `also_on` now counts in `decodes_like` only where it changes the decode size.
- May touch `src/app.rs::update_outputs` only if the test below fails.
- Tests:
  - a cue with `also_on` is in `layers()` of each display, with the same instance;
  - an entering clip that shares one display replaces it everywhere;
  - a clip on other displays only is untouched;
  - editing `also_on` while it plays adds and removes it live;
  - decode size is the larger display's;
  - a dip dips on every display;
  - a renderer test in the style of the `gpu.rs` ones: one instance composited to two targets uploads its texture once.
- Wave 2 then has P4, P6 and the chain P9 → P5: still three at a time.

### P5 · Engine II (wave 2) — smaller, **M**

- Loses the camera freeze (now in P2a).
- Starts from P9's branch.
- Overlap crossfades use the intersection rule.

### P6 · Editor tools II (wave 2) — unchanged size

- The "+N" badge and the ghost clips on the other displays' tracks.
- Dragging to another display's track changes the home display and removes that display from `also_on` if it was there.
- Tests: a ghost is drawn on each other display's track and cannot be dragged.

### P7, P8 — unchanged

- P8's Test Mode timeline part adds a freezing pause with `freeze_live` on, using the test-pattern camera.

### Release contents

- 0.21.0 (wave 1) now includes the live-input choice on pause cues and the new `run_out` default.
- 0.22.0 (wave 2) adds clips on several displays and the main list as a timeline, next to the standalone tab.

---

## D. Where I think a ruling could hurt in a live show

1. **`run_out` default End (ruling 5).** In the stage-manager case this is the default most likely to bite. If the hold lasts longer than a clip that is neither looping nor set to Hold, the picture fades to black, or the music fades out, on its own before anyone presses GO. The fade makes it gentle, but it is still an unplanned blackout. I would at least have the editor warn on any clip with a free-run pause inside it that is left on End ("this clip will fade out if the pause outlasts it"). My own choice of default for pictures would have been Hold.
2. **Several displays, partial overlap (ruling 4).** With the intersection rule, a clip on displays 1 and 2 that is replaced by a clip on display 2 only also disappears from display 1. That is the simplest rule and I recommend it, but it will surprise someone. The alternative (the old clip stays on display 1) means one cue with a different state per display, which the engine has no notion of today.
3. **Several displays of different shapes.** One decode and one fit setting serve all of them, and text is drawn for the home display and stretched on the others. Fine for matching projectors; wrong-looking on a 16:9 screen plus a portrait one.
4. **Live inputs default to continue (ruling 6).** A "pause on a frame" with a camera in the picture will show a frozen background with a live camera over it. That is the ruled default and is defensible; the pause cue's description text should say it in plain words.
5. **Main list as a timeline (ruling 7).** No objection. One consequence: OSC `/go`, the web remote and a presenter remote all act on the main list by default, so on such a show they become Run/resume and do nothing while it is running. That is consistent with ruling 3, but P8 should make the remote feedback say "running".

---

## E. Owner's rulings after this delta (these win over A to D)

1. **The main list is not a timeline.** The owner meant only that a timeline cue can sit in any list. Drop ruling 7's addition: no `Show::main_timeline`, no `TimelineKey::List(0)` handling (B4, the List(0) parts of B5, B6), and the P1/P2b/P4 items that follow from it. Revision 3's rule for list 0 stands.

2. **A clip on several displays: what happens on one display must not affect the others (replaces A4's "replaced everywhere" rule and D2).**
   - A clip on a *higher layer* on one display only covers the multi-display clip on that display. Nothing is replaced; the other displays are untouched. (This is what layers already do with `own_layers`.)
   - A clip entering on the *same layer* on only some of the multi-display clip's displays takes its place on **those displays only**. The multi-display clip keeps playing on its other displays, and ends only when it is on none.
   - So P9 must give a playing cue a per-display state: the set of displays it is still on, and a per-display exit (cut, or a fade/crossfade out on that display alone while it stays at full opacity elsewhere). `Engine::layers(output)` reads that per-display state. Chase (`chase.rs`) mirrors it.
   - The owner accepts that a clip routed to several displays has the same effects, geometry and keyframes on all of them.
   - P9 grows from M to L. Tests to add: a same-layer clip entering on display 2 only leaves the shared clip on display 1 at full opacity and running; a crossfade on display 2 fades it there alone; it ends when replaced on its last display; a higher-layer clip on one display changes nothing in `layers()` of the other.

---

## F. What P0 landed (wave 0), for the packages that build on it

P0 is on the branch as three commits ("Timeline: the show-file fields and the read model", "…the engine's entry points, as stubs", "…the editor's entry points, its pane and layout setting"). Sections B1 to B3, `Clip::also_on` and B7 are in; B4, the `List(0)` parts of B5, and B6 are absent (ruling E1).

Deviations from section 3, which the code now defines:
- `model::members(show: &Show, key: TimelineKey) -> Vec<&Cue>` (elided lifetime; clippy).
- `ui::timeline::Host` also derives `Debug, Hash`. `TimelineRects` has no `Default`.
- Extra derives on `LeadIn`, `Clip`, `Alive`, `Waiting`, `Chase`, `SnapPoints`, `Edge`, `PauseBefore`, `TimelineAction`.
- Extra public items: `PauseFreeze::is_nothing`, `RunOut::is_end`, `ControlCue::remap_timeline(..)`, `model::fade_wake(show, key, cue) -> Option<f64>`.
- `TimelineLayout` lives in `prefs.rs`, re-exported from `ui/timeline.rs`.
- `#![allow(dead_code)]` is on `src/timeline/mod.rs`, `src/engine/timeline.rs`, `src/ui/timeline.rs`, and `Action::Timeline` has `#[allow(dead_code)]`: each package removes the allow for what it makes real; I1 removes what is left.
- `ui.rs::window_menu` filters `Pane::Timeline` out of the Window menu: P3 removes that filter.
- `engine.rs::control_target` has an empty arm for the four timeline actions.

Things later packages must take into account:
1. **`Dur` and the lead-in (P2b).** `model::clips()` treats `Dur` as the content length and adds the lead-in itself. If `Engine::cue_duration` also adds the lead-in for every cue, passing it as `Dur` counts it twice. Add the lead-in only in `cue_duration`'s Group arm, or give `clips` a content-only length.
2. **`bulk_edit`.** `OWN_WHOLE_IN_KIND = ["timeline","at","clips"]` keeps those from spreading; `freeze` and `freeze_live` still spread.
3. **`remove_list`** clears a control cue's `timeline` to `None` ("the timeline this cue is on") and a lost `Marker` point with it.
4. **`duplicate`** remaps `timeline`, `at`, `clips` inside a copied group but not `target` (today's behaviour): a duplicated group's clip `Pause` cue still targets the original clip. To be fixed in I1 (remap `target` inside a duplicated group).
5. **Lead-in rule as built:** the transition is never a reason to defer; opacity keys at 0 or a still opacity of 0 defer whatever the transition (so a clip whose opacity keys stay at 0 until 4 s gets `LeadIn::At(4.0)`); keys that never rise, or a still 0, give `UntilFaded`; a sound at `volume_db <= MIN_DB` waits for a fade cue; invisible means opacity `<= 0.0`.
6. **`clocked`:** only armed cues on the same timeline count (`TimelinePause` with `timeline` None or this key; `Pause` targeting the clip). `Everything`, and `Clips` naming the clip, do not clock it. Inclusive span, 1e-6 tolerance.
7. **Tracks:** labels "V{layer+1} · Layer {layer}" (display name in front unless the show has one display), "A{n} · name"; order: pictures by display index then layer high to low, then sounds, then `Cues`; `tracks()` always includes `Cues`.
8. **`Clip::has_sound`** is true for every video (the model cannot probe). Trim is allowed for file, held, fade and wait cues that do not loop; no left-trim with keyframed speed. `length()` skips disarmed clips.
9. **`members()`** is structural only (it does not check `is_timeline`).
10. **Preload:** `decodes_like` ignores `run_out`, `run_out_fade`, `runs_unseen`, `also_on`. If P2a spawns `run_out == Loop` clips as looping, it must add `run_out` (for clocked clips) to the preload signature.

Real after P0: `timeline/mod.rs`, `timeline/model.rs`, and in `edit.rs` `new_timeline_list`, `new_timeline_group`, `set_list_timeline`. Stubs: the rest of `edit.rs`, `snap.rs`, `chase.rs`, the engine's run/pause/stop/seek/detached/waiting, and in `ui/timeline.rs` `editor`, `control_fields`, `clip_fields`, `settings_fields`, `apply`, `pane` (an empty-state label).

## G. How the wave-1 agents share this machine

Disk is small. Every worktree builds into the ONE target folder: set `CARGO_TARGET_DIR=/home/user/NewQ/target` for every cargo command. Builds then wait on each other's lock ("Blocking waiting for file lock"): that is expected, wait for it. Do not create another target folder, and do not run `scripts/cross_clippy.sh` (integration does). The test media is not in git: `ln -s /home/user/NewQ/test_media test_media` in the worktree's root before running tests. Other agents' tests run at the same time, so a timing test may fail once from load; re-run that one test alone before believing it.

---

## H. What P1 landed (the editing library), for P2b, P6 and I1

`src/timeline/edit.rs` and `src/timeline/snap.rs` are real, with wave 0's signatures unchanged. Added: `edit::MIN_LEN: f64 = 0.05` and `pub(crate) fn edit::shift_keys(a: &mut Animated, by: f64)`.

Behaviour worth knowing:
- `set_list_timeline` refuses list 0 and a list that does not exist ("There is no such list").
- `sort_by_start` breaks a tie in start time by `model::due_order` (control, then fade and stop, then clips): a pause added at a clip's start comes before the clip in the show.
- `split`: only media cues; refused inside a lead-in, outside the clip and on loops; clears `hold_last_frame` on the first half; the second half gets `runs_unseen = true` when it would otherwise wait for a fade-in.
- `add_marker` ids come from `show.alloc_id()` and are never reused.
- `add_track`: a picture track goes one layer above the highest on the first picture track's display; a sound track on the first audio output without one (`None` first).
- `remove_track` refuses the Cues track. `trim` finds the timeline with `model::key_of` and refuses a cue on none.
- Ripple delete moves a marker only if every track that had clips rippled, by the least any of them moved before it.
- A move's "same track step" is an index step in `model::tracks` order (so it can cross displays); a clip whose target row is of another kind stays put.
- Trim never shortens `fade_in`/`fade_out` to fit.

Open items:
1. **P2b: `Engine::cue_duration` and keyframed speed with a lead-in.** `trim` and `split` put the file position at clip time `t` at `in + M(t) − M(V)` (plan 2.5). `cue_duration` maps the file length with `time_for` from clip time 0, leaving out `M(V)`. They agree at a steady speed or with no lead-in. P2b makes `cue_duration` and `prepare_from` use the same rule (with F1).
2. **I1:** remove `#![allow(dead_code)]` from `src/timeline/mod.rs` once P3's editor calls `edit` and `snap`.
3. **I1 or P6: a split leaves fade cues aimed at the first half.** A fade-triggered clip split after its fade leaves the second half at opacity 0; retargeting needs a rule.

---

## I. What P2a landed (engine: run, pause cues, GO-timed transitions), for P2b and I1

One commit, "Timeline: the engine's run, pause cues and GO-timed transitions": `src/engine.rs` hooks, `src/engine/timeline.rs` (real), `src/engine/timeline/tests.rs` (23 tests), and in `src/media/video.rs` a `frozen` field, `set_frozen`, `update`.

Signatures as built:
- `start_with(show, id, reveal, at: StartAt)`; `start_at(show, id, at)`; `start` uses the default.
- `StartAt { run: Option<u64>, together: Option<u64> }` so far. **P2b adds** `from`, `lead`, `hold`, `chase`.
- `PendingCue` has private `run`, `at`, `start: StartAt` (no `waits_for_fade` yet: P2b). `ActiveCue` has private `run`, `ends_at`, `clocked`, `detached`, `live_frozen`. `MatteState` has `start: StartAt`. `ListState` has `runs`.
- `together_for`/`together_with` are gone; `StartAt.together` replaces them.
- `VideoPlayer::set_frozen(&mut self, bool)` (live players only). `audio.rs` is unchanged: a frozen live voice keeps running at gain 0.
- `timeline_seek` and `timeline_waiting` are still stubs (seek logs "not built yet"); `timeline_detached` is real. The file-level `allow(dead_code)` is gone; items only the editor calls carry their own.

For P2b:
- Runs live in `Runs.map` (`TimelineKey` -> `Run { instance, state, pos, frozen, pause_clips, pause_live }`).
- `timeline_begin(show, key, instance, from)` lists the waiting clips (`comes_after` decides which); `timeline_step(show, key, dt)` moves the clock and starts what it reaches; `timeline_adopt` tags a started clip, sets `clocked` and `ends_at` (from `timeline_clip`; no lead-in yet), and freezes it if the run is paused.
- To add: the `Cueing` state (unused, marked allowed), a real `timeline_seek`, `timeline_waiting`, Jump and `ControlCue::at`, hooks H14 and H15.
- The chase plugs into `timeline_start`: for a list it starts only what comes after the playhead today; for a group it fires from the top.
- Tests go in `src/engine/timeline/tests.rs`.
- **Also for P2b (asked by P3):** in `engine.rs` add `pub use timeline::{RunState, RunView};` so the UI can name them.

Behaviour as built (for the README and the owner):
- A clock-driven fade-out or end does not start while the clock stands (that is what holds a fade at a pause placed on its start).
- A clocked still, text, shape or camera ends by the clock alone, not by its own hold time.
- `TimelineStop` cuts (a control cue has no fade field).
- A `Pause` aimed at a live cue freezes it only when the cue is named itself, not through its group.
- GO resuming a run also lifts the show's global pause.
- A stopped list timeline run starts only what comes after the playhead plus clips exactly on it; a pause cue exactly there counts as passed (until P2b's seek).
- Preload looks ahead `PRELOAD_HORIZON` (10 s), at most `MAX_PRELOAD_CHILDREN`.
- A clocked `Loop` clip is played as an endless loop, and the preload uses the same looped version.

## J. What P3 landed (tracks editor, Option A), for I1 and wave 2

Two commits. New: `src/ui/timeline/{tracks,ruler,toolbar,control,tests}.rs`; `src/ui/timeline.rs` holds the editor, `pane`, `target`, `apply`. `UiState::timeline: timeline::EditorState` (manual `Default`, `snap` starts true). `pub(super) fn inspector_rows(ui, &mut Show, idx)` is called at the end of the inspector's right column (it edits a clone and writes back only on change), because `control_fields`/`clip_fields` cannot be called where the plan said.

The pane splits the dock **root** (full width along the bottom), not the cue list's node. In the new interface it is a resizable bottom panel in the Editor view.

For I1:
1. Replace `run_of` (it parses `{:?}` of `RunView::state`) with the real types once `engine.rs` exports `RunState`/`RunView`.
2. Double-click on a timeline group's row to open the editor is not done: it lives in `cue_list` and double-click fires the selected cue today. Decide, or leave the inspector button and Window menu as the ways in.
3. `UiState::after_panic` should clear `st.timeline`'s drag and renaming state.
4. README section, keyboard-table rows, CLAUDE.md entries (`engine/timeline.rs`, `ui/timeline/`, `timeline/`), CHANGELOG line.
5. The module-level `allow(dead_code)` in `ui/timeline.rs` and `timeline/mod.rs`: remove what is now called.
6. Known look issues: in the short new-interface panel the Cues row needs a vertical scroll; the inspector's timeline rows sit below the fold when the pane is open.
7. Machine trap: `target/debug/newq` can be another worktree's binary even when cargo says it is fresh; `touch src/main.rs` and rebuild before a screenshot.

---

## K. What P2b landed (seek, parking, jumps, the wait-for-fade-in rule)

Two commits: "Timeline: the chase, what plays at a given time on a timeline" (`src/timeline/chase.rs`, `chase/tests.rs`) and "Timeline: seek, parking, jumps and clips that wait for their fade-in" (`engine.rs`, `engine/timeline.rs`, its tests). Full suite: 982 passed.

As built:
- `prepare_from(cue, info, shift, lead)`: file position `M(shift) − M(lead)`. `take_prepared(cue, t0)`. `cue_duration` (H15): with keyframed speed and a lead-in the content length is `time_for(M(V) + file) − V`; only the Group arm adds the lead-in.
- `StartAt` has `from`, `lead`, `hold`, `chase: Option<Alive>`, `part_way()`; it is `Clone`, not `Copy`. `PendingCue.waits_for_fade`; `ActiveCue.lead`, `file_from`.
- `timeline_begin(show, key, instance)`; a start from a later position goes through `timeline_place`. `timeline_clip(show, cue, lead)`. `timeline_adopt(show, active, &at, clip)`.
- `timeline_seek`, `timeline_waiting` are real. `Run` has `parked`, `cue_since`, `unstarted`. Constants `CUE_LIMIT = 2 s`, `MAX_JUMPS = 8`, `LAST_FRAME = 0.2 s`.
- `chase::Alive.geometry` is `Option<GeometryRamp { now, to, left, curve }>`; `Alive.faded: bool`; a `Level` alias.
- `engine.rs`: `pub use timeline::{RunState, RunView};` (with `allow(unused_imports)` until the UI uses it).
- States: a seek while Running or Cueing goes to Cueing (released when every clip is started or after `CUE_LIMIT`); otherwise the run parks as `Paused { Everything, waits_for_go: true }`. `RunView` has no `parked` flag.
- A seek to exactly 0 parks with nothing on screen (0 means "from the top"); a cue exactly at `pos` counts as passed.
- Parking: one FFmpeg (and a paused voice) per video/audio clip under the playhead, started on each `timeline_seek` and only then; each seek ends the previous seek's clips at once; preloads within 10 s (at most 12) are kept between seeks.

Left for later:
- I1: replace the editor's `run_of` Debug parsing with `RunState`/`RunView`; hatch the real `timeline_waiting`; decide whether the toolbar says "Parked" (needs a flag on `RunView`); the ruler seeking to a hair above 0 if the first frame should show at 0.
- P5: a running seek shows a partly-built picture for a moment (old clips go at once, new ones appear as decoders are ready).
- A fade cue that wakes a clip whose file is not read yet misses it (rare: the preload horizon reads files well before).
- Not modelled by the chase: fade cues' matrix crosspoints, a fade on a clip while it is held, motion keyframes under a geometry fade.
- One P2a test was made to wait in real time for a clip's end (it failed under full-suite load).

## L. Owner's rulings after wave 1's engine work (these win over anything earlier)

1. **`run_out` default stays End with the fade.** The integrated fade on a clip that runs out is by design; no prompt, no automatic Hold when a pause is added. The editor's ⚠ stays.
2. **On a timeline, keyframes follow the timeline's clock, not the content.** A fade the operator controls (timed to GO) is done with a discrete fade time or with opacity keyframes, with a pause cue before it: nothing moves at the pause, and the fade or ramp starts on GO. So for a clip in a timeline run, its opacity, geometry (`Motion`) and effect keyframes are evaluated at the clip's *timeline* time (`run.pos − clip start`, less any lead-in as the tracks draw it), which stands still while the clock is paused, even while the content free-runs or loops. Speed keyframes stay tied to the content. Off a timeline nothing changes. This is package **P2c** (engine), in the first release.
   The model is Disguise's: the timeline waits at each change. A cut, a crossfade to the next clip, a fade cue, a clip's own fade-out and a keyframe ramp all hold at a pause and go on GO.
3. **"Loop this section": a third kind of pause cue (wave 2, with P5's jumps).** When the timeline reaches the cue it jumps back to a section start (a marker or a time earlier on the timeline) and plays the section again, until GO. Everything in the section replays together, in sync. On GO: **default, finish the current pass and carry straight on past the cue** (the intro / looping middle / ending case); option on the cue: leave at once. The jump is not sample-accurate (clips are restarted); say so in the cue's description, and that a continuous bed of sound is better as a per-clip loop.
4. **Split and fade cues:** a fade cue stays aimed at the first half of a split clip; the second half starts visible (no objection from the owner to this suggestion).
5. Undecided, asked of the owner: whether the timeline Stop cue gets a fade time (today it cuts).

## M. What P2c landed (keyframes on a timeline follow its clock, ruling L2)

One commit, "Timeline: keyframes on a timeline follow the timeline's clock": `src/engine.rs`, `src/engine/timeline.rs`, `src/timeline/chase.rs` and the tests beside them. No show-file field.

As built:
- `ActiveCue` has private `clip_zero: Option<f64>` (where on its run's clock the clip's own time is 0) and `clock_time: f64` (`pos − clip_zero`, never below 0). `pub fn ActiveCue::key_time()` is `clock_time` for a timeline's clip and `elapsed` for any other cue, so a cue fired off a timeline (or a timeline's clip fired on its own) reads its keys exactly as before.
- `timeline::zero_of(cue)`: the clip's start (`pre_wait`), plus half its fade for a picture that dips in (the dip starts it at its midpoint): the chase's `Clip::zero`. The lead-in needs nothing extra: a clip's own time already counts from its start, lead-in included (`elapsed = V` when its content starts).
- `Engine::timeline_clock_time(cue, run)` (in `engine/timeline.rs`) gives both when `start_with` makes a run's media cue; `tick` sets `clock_time` from `Runs::clocks()` each pass. A clip whose run has gone (its group fired again) goes on counting by `dt` from where it was, so it doesn't jump.
- Read at `key_time()`: the motion transform and opacity keys in `tick`, `VideoFx::at` and `Voice::set_effects` (picture and sound effects), the opacity a part-way start begins at, a fade cue's geometry starting point, and `chase_levels`' geometry base. `elapsed` stays the content's time for everything else: speed keys (`SpeedMap` in the readers, `pass_time`), file positions, a timer text's count, hold times, the content-driven fade-out and end.
- Chase: a geometry fade now starts from where the motion keys have the picture at that clock time (`geometry_base(m, t − zero)`; was the bare transform), and the first one keeps the fields it doesn't name where the keys had them (still tracks), as the engine's `LiveTransform` does. `Alive.geometry.now`/`to` then have all five fields. The "motion keyframes under a geometry fade" gap in K is closed.
- No keyframed volume exists in the model (`volume_db` is a plain number; `Motion` has opacity and geometry only), so nothing to do for sound levels.

Behaviour to know (for the README and the owner):
- A clip frozen by a `Pause` of its own while the clock runs holds its frame, but its opacity, motion and effect keys go on with the clock (Disguise's model; the plan's 2.2 table said they stop). A seek gives the same.
- Scrubbing a timeline clip on its bar moves its content, not its keys.
- What still moves at a free-run pause: Param ramps already going when the clock stops (a clip's own transition fade-in, a fade cue in progress, a crossfade's fade-out, a run-out fade). Ruling L2's model says those hold too; they are not keyframes and were not part of P2c. Holding them means not advancing `opacity`/`volume`/`live_transform` for a run's clip while its clock stands (and the fade cue's own `ActiveKind::Fade` time): a separate small package if wanted.
- The matte of a dip runs its half by its own time; a dip that starts just before a pause still reaches its midpoint during the pause.

Tests: `src/engine/timeline/tests.rs` (P2c section: opacity, geometry and blur keys standing through a free-run pause on a loop and ramping after GO; speed keys following the content through a pause, checked against FFmpeg's frame; a cue off any timeline and a timeline clip fired on its own reading keys at their own time, held and resumed; a parked seek into the middle of a ramp equal to playing there with a wait at the pause and a clip frozen part of the way), `src/timeline/chase/tests.rs` (`a_geometry_fade_starts_from_where_the_motion_keys_have_the_picture_by_the_clock`). Full suite: 988 passed, 5 ignored.

## O. What P2d landed (the Stop cue's fade; ramps wait while the clock stands)

Two commits: "Timeline: the Stop cue fades the timeline out, 3 seconds unless set" and "Timeline: fades under way wait while the timeline's clock stands" (`src/show.rs`, `src/engine.rs`, `src/engine/timeline.rs`, its tests, `src/ui/timeline/control.rs`, one line in `src/ui/timeline.rs`).

The Stop fade (owner's ruling, after L5):
- `ControlCue::fade: Option<f64>` (`#[serde(default)]`, not written when None), `ControlCue::stop_fade()` (None or not finite = `show::TIMELINE_STOP_FADE`, 3.0; negative = 0). A field of its own: `StopCue::fade` is on another struct. It is read only for `TimelineStop`; bulk edit spreads it like `freeze`.
- `timeline_stop(show, key, fade)` (signature unchanged): the run is `Stopped` at once (clock stands, pending cancelled, frozen clips let go), its clips and a dip's matte on its way to one (`MatteState::start.run`, which the cut used to miss) fade with `stop_instance(i, fade)` (S-curve), and the run is over when they have gone. No new `RunState`: a Stopped run with `stopping` clips is "fading".
- A second Stop (cue, editor, route) while those clips fade cuts at once.
- GO or Run during the fade does what it does on a stopped run (a list runs from its playhead, a group from its top); `clear_clips(instance, fading)` leaves the old run's fading clips fading, and the clips coming in replace them on their layer as usual.
- The editor's Stop button uses `TIMELINE_STOP_FADE` too. `timeline_route`'s TimelineStop (a control aimed at a timeline group) uses the default. Stop-all and panic unchanged.
- Inspector: a "Fade out" row for a timeline Stop (`note_row "fade"`; writing 3.0 stores None). The cue-list text adds " · fades out over 3.0 s" or " · cuts".

Ramps wait while the clock stands (L2, what M left):
- `Ramp` has `run: Option<u64>` and `since: Option<f64>`. `Param::follow(Some((run, pos)))` tags the ramp just started; `Param::step(dt, &ClockSteps)` moves a tagged ramp only as far as its run's clock moved this tick (first step: from `since`, at most the move), and an untagged one by `dt`. `Runs::steps(before, dt)` is per run that is not Stopped: `pos` and `moved` (clamped to 0..dt). A Stopped or gone run's ramps move by `dt`, so nothing is ever stuck. `Engine::run_clock(run)` gives the pair for a run that is not stopped.
- Tagged: a run clip's own fade-in (and what a seek's chase left going), a fade cue the run fired (opacity, volume, geometry, its stop-target hold), its own `ActiveKind::Fade` time, a crossfade out by an entering run clip, a dip's matte and the outgoing content under it, the matte's reveal, the clock fade-out and a run-out fade begun while the clock runs (`clock_end(a, m, on_clock, clock)`), and the content-driven auto fade-out begun while the run's clock runs.
- Not tagged (always move): anything begun by hand (active panel fade-out/stop, `stop_instance`), a Stop or Fade cue not fired by the run (a cue list outside the timeline, or a timeline cue fired on its own), the Stop fade, a run-out fade begun while the clock stands (the content runs out during the pause: ruling 5's fade to black stays), cues on no timeline.
- A tagged ramp also fixes the old overshoot: a fade cue started part way through a tick moved a whole `dt` on it; it now moves from where the clock was when it started.
- Content that runs out while a tagged level ramp waits is `held` (last frame, or silence) instead of ending, and the ramp finishes after GO (then it goes as a stopping/auto-faded held clip does).
- Not done: matrix crosspoint fades (`Voice::fade_crosspoint` runs in the audio callback by rate; holding needs a pause flag in `VoiceShared`). They still move during a pause. The matte's `REVEAL_TIMEOUT` still counts real time.
- Chase/seek: the chase works from the clock, so a seek to a time now matches playing there through a pause (tested for a fade cue's opacity and x).

Tests (engine/timeline/tests.rs, "The timeline Stop fades" and "Fades under way wait"): stop at 3 s by default, opacity part way at 1.5 s, run over after 3 s, the clock stands; 0 cuts; a second Stop (cue and editor) cuts; Run during the fade re-cues over the fading picture; a sound's level fades; a fade cue half done stays so through a free-run pause and finishes after GO, equal to a seek there; a hand fade-out and a main-list fade cue move during the pause, a cue on no timeline is unchanged; a video's fade-in waits while its content plays on; a run-out fade under way waits on the last frame and finishes after GO. show.rs: default 3 s, not written, old cue loads, round trip.

---

## N. What I1 landed (integration 1)

Commits starting "Timeline:" and one "Docs:" on top of section L's notes; engine files touched only to drop
`allow` lines that the editor's calls made unneeded (`engine.rs` `pub use`, and in `engine/timeline.rs`
on `is_pause_of`, `timeline`, `next_pause_from`, `timeline_detached`; `timelines` keeps its allow).

- **The editor's run:** `run_of`'s Debug parsing is gone; `tracks::Ctx::run` is the engine's `RunView`,
  matched on `RunState`. A clip in `timeline_waiting` is hatched blue (`WAITING`) over its lead-in (all of
  it when it waits for a fade cue) and noted "waiting to fade in" (the static lead-in now says "waits to
  fade in").
- **Parked:** `toolbar::state_text` says "Parked · Run or GO plays from here" for
  `Paused { Everything, waits_for_go: true }` when no armed pause cue of the timeline sits at the
  playhead. A guess: a pause cue elsewhere that names this timeline also reads as parked. **Wanted from
  the engine:** a `parked: bool` on `RunView` (the `Run` has one).
- **Seek to the start:** `TimelineAction::Seek` and `JumpToMarker` go to `seek_to(pos)` = at least
  `FIRST_FRAME` (1 ms), since the engine parks with nothing at exactly 0. A ruler tap (press and release
  in one frame) now seeks too (`ruler.rs`, `resp.clicked()`); before, only a press seen over two frames did.
- **Double-click** on a timeline group's row opens it in the editor (`Open(Some(key))`) instead of firing
  it; every other row still fires (`FireSelected`). Double-click-to-fire was never in the README; GO,
  the inspector's Fire and the right-click menu still fire a group.
- `UiState::after_panic` calls `EditorState::let_go` (drag and marker renaming).
- **`allow(dead_code)`:** the module-level ones in `timeline/mod.rs` and `ui/timeline.rs` are gone, and
  `show.rs`'s `unused_imports` (`TrackId` dropped from the re-export; `Marker` re-exported only under
  `cfg(test)`). What stays, each on its item: `Host::ListTab`/`Workspace` (P4, P7), `EditorState::ripple`
  (P6), the `TimelineAction` variants `Split`, `Delete`, `AddClip`, `AddTrack`, `RemoveTrack`, `SetRate`
  (P6), `NewTimelineList`, `SetListTimeline` (P4), `SetLayout` (P7) (their `apply` arms are real);
  `Engine::timelines` (only tests call it; P4/P8 transport and feedback); and `#[allow(dead_code)]` on
  `pub mod chase;` because `chase::Clip::entry` is written and never read (**engine's file: use it or
  drop it, then remove that allow**). The test helper `frame_apply` was deleted.
- **Duplicate** (`Show::duplicate`): a fade, stop or control cue inside a duplicated group whose `target`
  is a cue of the group now aims at that cue's copy (as `paste_cues` did); one aimed outside, or a cue
  duplicated alone, keeps its aim. Test `duplicating_a_group_aims_the_cues_in_it_at_the_copies`.
  (Noticed, not changed: `duplicate` and `paste_cues` set a child's parent to the top group only;
  groups don't nest, so it is harmless today.)
- **Split (L4):** `edit::starts_visible`: a second half that would be unseen by itself (opacity 0, or
  opacity keys all at 0; a sound at `MIN_DB`) gets the level of the last fade cue aimed at the first half
  at or before the split (else the first such fade, else 1.0 / 0 dB), its opacity keys dropped. Fade cues
  are not retargeted. Test in `edit.rs`.
- **End-to-end tests** (`ui/timeline/tests.rs`, plan I1's list): a clip dragged in the whole window
  (`Draw::Full`) moves and one Undo restores it; letting go on the ruler parks with both pictures up;
  Run from the toolbar runs it and a "Pause before this clip comes in" holds the clock until GO brings
  the clip in; a click at the ruler's start parks with the first picture up; a one-frame tap seeks.
- **Look (headless, 1280×760 window):** the new interface's timeline panel is 250 high so three tracks
  and the Cues row fit; a thin bar at the right of the lanes shows when tracks overflow (the wheel
  scrolls them). The group inspector's "Overlapping clips crossfade" is disabled with a "not working
  yet" hover, since nothing reads `crossfade_overlaps` before P5.
- Docs: README "Timelines" section and three keyboard-table rows, CLAUDE.md layout entry, CHANGELOG
  lines (with the 0.20.3 note).

Left (not I1's, or not cheap):
- The old inspector `timeline_panel` (bars you drag in the group's inspector) is still in the group's
  inspector; not decided (plan I1 recommended read-only with the "Open" button).
- Transports don't say what GO will do on a paused timeline: the main transport reads "NEXT · 8 ·
  Blackout" while GO would run the timeline on (P4's read-outs).
- In the classic layout with the Timeline pane open, the inspector's timeline rows are below the fold
  (the pane splits the dock root); in the new interface on a 760-high window the cue list is left about
  two rows.
- The toolbar's items move right as the state text grows ("Running · next pause in …"), so Snap/Marker/
  zoom shift while it runs.
- The control cue's icon (🎛) draws as a box-like glyph in the cue list (font coverage; older than timelines).
- Editor adds clips only through the cue list (no AddClip/AddTrack/split from the editor: P6).

## P. What I1b landed (the last polish before the first timeline release)

Commits "Timeline:" and one "Docs:" on top of N. Full suite: 1012 passed, 5 ignored.

- **`RunView.parked`** (the run's own flag: a seek that parks, or a pause while cueing) and
  **`RunView.fading`** (Stopped, its clips or a dip's matte still fading out). `toolbar::state_text(run,
  pos)` reads them: "Parked · Run or GO plays from here", "Stopping · fading out". The guess from pause
  cues at the playhead is gone.
- **`chase::Clip::entry`** removed (the Enter event's time is the same); `pub mod chase;` has no allow.
- **What GO does is one engine decision:** `Engine::go_plan(show) -> GoPlan` (`Slide { deck, slide }`,
  `Timeline { key, does: TimelineGo, then }`, `Cue(Option<CueId>)`); `TimelineGo` is Start (a stopped
  timeline list), Resume (at a pause), Play (parked), Running and Cueing (a list: GO does nothing).
  `go` follows the same pieces: `deck_at` (shared with `deck_step`), `timeline_go_for` (which
  `timeline_go` acts on), `playhead_top` (shared with `go_playhead`). `GoPlan::describe` gives
  "Continue 1 · Opening" / "then bring in 5 · Still" (`then` is `on_go_of`, the toolbar's `on_go`);
  the words are `timeline::model::on_go_text` (the toolbar's `go_text` adds "GO: ") and
  `model::name`. Read out by the classic transport (`ui::go_readout`), the new interface's header (fixed
  width `HEADER_NEXT_WIDTH`, cut short, hover for all) and Operator view (THEN then lists the next
  cue first), and the web remote (`"go"` in the state JSON; the page shows it in place of the next cue).
  OSC feedback's `/newq/playhead` is the playhead cue itself, not GO's action: left as it is.
- **Toolbar:** the state and GO texts come last, truncated (hover for the whole); the time is padded.
  Test `the_toolbar_buttons_stay_put_as_the_run_state_text_changes` (fails on the old layout).
- **The flaky test** (`seeking_past_a_pause_cue_or_onto_one…`): not an engine race. A text is drawn on
  a thread; going on from a park cues the run until it is up; the test gave it three 2 ms ticks.
  Reproduced (9 of 40 runs with four busy loops), now waits with 0-ticks for the drawing and for
  Running, and checks the clock is still at 9; 40 of 40 under the same load.
- **Matrix crosspoint fades hold at a pause:** `VoiceShared::xp_hold` (an `AtomicBool` the mixer reads;
  no lock, no allocation), `Voice::hold_crosspoints/crosspoints_held/has_crosspoints`.
  `Engine::timeline_hold_crosspoints` (after each tick) holds the voices a run's fade cue with `matrix`
  targets aims at while that run is Paused or Cueing; it clears when the clock moves, the run stops or
  goes, or the fade ends. The rate was set at the start, so the fade takes its time in all.
  A crosspoint fade begun by hand on a voice such a fade also aims at waits with it (not separated).
  Also: a run's fade cue counts its own time from where its run's clock was when it began
  (`ActiveCue::fade_from`); one begun part way through a tick counted the whole tick and ended up to a
  frame early. Tests: the mixer (`…stands_while_held`) and the engine on the fake device, mixed in real
  time (`a_matrix_crosspoint_fade_waits_at_a_free_run_pause_and_goes_on_after_go`).
- **Docs:** README (toolbar Stop and the Stop cue fade over 3 s / the cue's time, 0 cuts, a second Stop
  cuts, GO or Run during the fade; fades under way wait at a pause, hand fades run; a clip frozen by its
  own Pause follows its keys; the transports' GO read-out), CHANGELOG lines, CLAUDE.md entry.
- **Looked at headless** (1280×760 window, both interfaces; screenshots in the session scratchpad
  `i1b/`): stopped, running, waiting at the pause ("Continue 1 · Opening"), parked ("Play 1 · Opening
  from its playhead / then 5 · Dim the red"), the Stop fade, the Stop cue's "Fade out 3.00 s" row, the
  Operator view at a pause. Fixed from what was seen: the toolbar said "Stopped" during the fade, and
  the new interface's view tabs moved with the read-out.

Still to know (not I1b's):
- No "then …" when the next thing on a paused timeline is a control cue (a Stop or Jump): `on_go`
  skips control cues; the read-out is just "Continue …".
- A still on a timeline whose own time passes its end while the clock keeps it reads "0:08.0 / 0:08.0
  (-0:00.0)" in the active panel (cosmetic). In the look show the free-run pause let the 10 s video and
  the still run out before the Stop (by design, the ⚠ case), so the Stop's fade had little to fade.
- The Stop cue's "Fade out" row is below the fold of the classic inspector with the Timeline pane open
  (as N said for the timeline rows); scroll the inspector.
- The matte's `REVEAL_TIMEOUT` still counts real time (O).

## Q. What P9 landed (a cue on several displays)

Commits "Displays: a picture on several displays, replaced on each alone" (engine, chase, one helper in
`show.rs`) and "Docs: what P9 landed". No show-file change (`also_on` was stored since P0).

As built (owner's ruling E2, not A4's "replaced everywhere"):
- `MediaCue::displays(show)`: the home (`output`), then each of `also_on` the show has; one that is the
  home or listed twice counts once, one the show lacks is ignored. Every reader goes through it.
- `ActiveCue::screens: Vec<displays::Screen>` (`src/engine/displays.rs`): per display `{ output, exit:
  Option<Param>, gone }`, set at start for a picture and for a dip's matte, kept in step with the show each
  tick (`displays::sync`, next to `v.output = m.output`), so editing `also_on` while it plays adds and drops
  a display at once; a display it was replaced on stays gone while it is listed. Empty: `output()` alone.
- `Engine::layers(output)` reads `level_on(output)`: the cue's opacity times its exit level there.
- `apply_entry(instance, e)` (the `output` parameter is gone): what enters replaces on *its own* displays
  (`showing()`), per cue `o`: `cut_from` (a cut, a dip's reveal) and `crossfade_from`. Off all of `o`'s
  displays (and leaving none alone), it is removed or `fade_out_and_stop`ped exactly as before, so a cue
  on one display behaves as it always did. Off some: those are gone (cut) or get an exit ramp 1 -> 0 over
  the crossfade, tagged to the entering clip's run like every other ramp (`Param::follow`), stepped in
  `tick` while the show isn't paused (a control cue's hold of the cue doesn't stop it: the hold is the
  other displays' business). A cue gone on every display ends (`gone_everywhere`, no follow-on, like a
  replaced cue). Layers mode off: the layer test is skipped, as before; the display rule is the same.
- Sound (one voice): plays while the picture is fully on some display; when the last full display goes
  and some are still fading (`leave_sound`), the cue is `stopping`, its volume ramps to silence over the
  longest exit left, and it ends then.
- Dips: one matte instance on all the entering cue's displays. `begin_dip` fades and marks stopping only
  content wholly under the matte; a cue also on other displays is left alone and the reveal cuts it from
  the dipped displays.
- Fade cues, Stop cues, hand fades/stops, a timeline Stop and `clear_clips` act on the whole cue (all its
  displays).
- `crossfade_into_next` (play-in-order groups) needs the same display set, not just the same home.
- Decode size: `picture_size(m)` = the home's size, or the largest (by area) *open* window among
  `also_on` (`output_sizes`); a display with no window open doesn't count. Text and shapes are drawn at the
  home's size and stretched on the others. `decodes_like` still ignores `also_on`: the preload's `size`
  carries the only decode difference. Sync (`video_lag`) is the home display's. Geometry and fit are the
  cue's own, applied against each display's size by its window (open question in D3, decided this way).
- Chase: `Clip::screens` (`Shown { output, gone, exit: Track }`) with the same `cut_from`/
  `crossfade_from`/`leave_sound`; `Alive::off` (displays it was replaced on) and `Alive::leaving`
  (display, `Level`); `chase_levels` sets the screens from them.
- Renderer: no change. `app.rs::update_outputs` keys textures by instance and uploads once per frame
  serial; each window composites with its own slot, as for `part_of`. Read, not tested (no GPU test was
  written for it).
- Paste and display removal were already right (P0): tested in `show.rs`.

Tests: `src/engine/displays/tests.rs` (15, text cues, no media): one instance on each display; unknown and
home displays ignored; a single-display cue unchanged; a cut on display 2 leaves it on 1 at full opacity
and running, then a cut on 1 ends it; a crossfade on 2 fades it there alone, then one on 1 fades it (and
its sound) as usual; crossfades on both at once end it with the longer; a higher layer on 2 changes
nothing on 1; layers mode off; a shared cue entering replaces on each of its displays; a fade cue, a hand
fade and a Stop cue act on every display; live `also_on` edits; a dip on two displays; the decode size;
a free-run pause holding a per-display crossfade until GO; a seek giving what playing gives.
`src/timeline/chase/tests.rs` (2): per-display replacement, a higher layer, passing on no display, the
sound fading with the longer fade.

Left / not tried:
- No real displays: all through `Engine::layers` (two output windows of one instance were not looked at).
- The inspector's "Also on" row (P4) and the editor's badge and ghosts (P6) are not P9's.
- A cue held by a control cue that leaves its last full display unfreezes to fade its sound (as
  `fade_out_and_stop` does).
