# Timeline mode: implementation plan (revision 3)

Status: plan only, nothing built. Based on `main` at v0.20.3 (branch `claude/beautiful-fermi-9owexf`). This revision replaces the earlier one in full. It includes the owner's later decisions:

- two kinds of pause cue, and clip-level pause/resume;
- keyframed clip speed edited in the timeline;
- loop / hold / end when a clip outruns its slot, with a fade option on End;
- transitions timed to the GO after a free-run pause (the key use);
- a clip at opacity 0 waits for its fade-in by default, with a per-clip opt-out.

Line numbers are for v0.20.3 and will drift; function names are the stable reference.

---

## 1. What the code does today (findings)

### 1.1 Timeline groups in the engine (`/home/user/NewQ/src/engine.rs`)

- **Firing.** `start_with`, `CueKind::Group` arm (~2440): for `GroupMode::Timeline` it calls `self.fire(child)` for every armed child. `fire_one` puts a child with `pre_wait > 0` in `Engine::pending` (`PendingCue { remaining, total, reading }`), otherwise starts it. The group becomes an `ActiveCue` with `ActiveKind::Group(GroupState)`.
- **No timeline clock.** The group's `elapsed` is `+= dt`. Each pending child counts its own `remaining -= dt`. Each started child follows its own clock (audio voice clock for sound and video with sound, `dt` otherwise).
- **Ordering inside a tick.** `tick` collects every pending entry that is due, then starts them all. A control cue among them cannot stop the ones after it in the same tick.
- **End.** `advance_groups` reports a Timeline group done when no child `is_running` (in `pending`, or active and not `held`). The check is by cue id, not instance. An open-ended child keeps the group alive.
- **`together_for`.** Children with `pre_wait == 0` share the display without replacing each other (`ActiveCue::together`). A child with `pre_wait > 0` gets `together = None`, so it replaces or crossfades what is on its own display and layer by its own transition (`apply_entry`). With `Show::own_layers` off (shows from 0.18.0 and before) it replaces everything on the display.
- **`cue_duration`.** Timeline group = max over armed children of `pre_wait + duration`; an open-ended child gives `Some(None)`. Video and audio lengths go through the speed map (`speed_map_cached`, `SpeedMap::time_for`), so a clip's length already follows its keyframed speed.
- **Automatic fade-out.** In `tick`, a cue with `fade_out > 0` starts fading when `duration - elapsed <= fade_out`. It is driven by the cue's own content time, not by any timeline.
- **Opacity.** While no fade is ramping, `tick` sets `opacity.value = m.motion.opacity_at(m.opacity, a.elapsed)`: opacity keyframes are in the cue's own time. A fade cue sets `faded` and ramps `opacity`/`volume` on cues already in `active`; it cannot reach a cue that has not started.
- **Preload.** `update_preload` wants every `pending` cue prepared. A timeline group with 50 video children waiting spawns 50 FFmpegs at GO. `MAX_PRELOAD_CHILDREN` (12) only limits children at the playhead. This must change for real timelines.
- **`load_ahead`** just adds ids to `Engine::loads`; preparing is always at shift 0.
- **Global pause.** `Engine::paused` stops pending countdowns, `advance_groups` and every `elapsed`. `fire_one` un-pauses on any fire.
- **Per-cue pause.** `ControlAction::Pause/Resume` call `hold_cue`, which sets `ActiveCue::hold` on the target and a group's children and pauses their voices. **Existing bug:** it does not touch `pending`, so pausing a timeline group leaves its waiting children counting down and starting.
- **Re-fire.** Firing a group again starts a second run; the first run's waiting children still fire later.
- **`dt`.** `app.rs::pass` does not clamp `dt` (a stall is time that passed) and sets `dt = 0` on the first tick after `!is_active()`. `is_active()` is `active` or `pending` non-empty.

### 1.2 Starting part-way through

- A cue cannot be *started* part-way today. A *playing* cue can be jumped: `Engine::seek(show, instance, to)`.
- The machinery is `prepare_from(cue, info, shift)` (~1825). It already handles:
- steady speed (`shift * rate` of file),
- keyframed speed (`SpeedPlan { map, t0: shift }`, `map.media_at(shift)`),
- loops (`Playback::started_at`, in point unmoved),
- Hap.
- `seek` swaps decoders in once they have a frame (`ActiveCue::seeking`, `apply_seek`) and sets `seek_base` and `elapsed`. `seek_base` is "where in the cue the running decoders began": `elapsed = seek_base + voice clock`. Images just set `elapsed`. Motion, effects and timer text are evaluated from `elapsed`.
- A cue with `hold` set (or global pause) when its decoders become ready shows its first frame and stays silent. Test `seeking_works_while_paused` relies on this.
- Dips: `start_with` calls `begin_dip` first when `reveal` is `None`. A part-way start must bypass it.
- A live `VideoPlayer` (camera) ignores the time passed to `update` and always shows the newest frame. A "held" camera stays live today.

### 1.3 Control cues (`/home/user/NewQ/src/show.rs` ~2232, engine ~1397)

- `ControlCue { action: ControlAction, target: Option<CueId> }`.
- Ten actions in `ControlAction::ALL`. `Engine::control` returns early without a target; `control_target` does the work.
- Arm/Disarm go back to the app as `ShowEdit`. `GoTo` sets a list's playhead.
- Stopping is a separate cue kind (`StopCue { target, fade, use_own_fade }`).
- **Trap:** `testmode/feature.rs` (~3146) asserts the Debug-test show uses every action in `ControlAction::ALL`.
- `ControlCue { .. }` literals exist only in `engine.rs` tests (2) and `show.rs` (1).
- Paste remaps `target` through `CueKind::target_mut`. `bulk_edit::OWN_IN_KIND` lists `"target"` as a cue's own field.

### 1.4 Cue lists

- `Cue::list: u32` (0 = main), `Show::lists: Vec<CueList { id, name }>`. The main list has no `CueList` struct, only `main_list_name`.
- The cues of a list are contiguous in `Show::cues` (`normalize_lists`, `list_range`).
- The engine has `playhead` and `active_list` for the list being worked, and `list_playheads` for the rest. `switch_list`, `go_list`, `sync_lists`, `reset_all` and `take_list_state` manage them.
- A playhead is a `CueId`, never a time.
- Groups do not nest (`normalize_groups` un-parents a group inside a group).

### 1.5 UI

- **Actions.** `Action` is a private enum in `/home/user/NewQ/src/ui.rs` (~610). `draw` collects `Vec<Action>` and `apply(action, show, engine, st)` (~10031) runs them after the frame. `Action::edits_the_show()` gates show mode and Test Mode. Undo is JSON snapshots; `draw` calls `history.end_burst()` on every frame that had actions.
- **Existing timeline view.** `timeline_rows` and `timeline_panel` (~6461–6640): one bar per child, drawn in the inspector under a Timeline group. Dragging emits `Action::SetPreWait` every frame. The playhead line comes from `engine.status(group)`. No tracks, trim, snap or zoom.
- **Reusable bits.** `keyframe_lane(ui, id, &mut Animated, spec, len, playhead)` (~8185) and `speed_inspector` (~7918) already edit `MediaCue::rate` keyframes. Scrub-on-release exists as `Scrub` and `Action::Seek(instance, pos)`.
- **Dock.** `/home/user/NewQ/src/dock.rs`: `Pane` enum (6 variants, `Pane::ALL`, `title`, `companion`), `Layout { dock: DockState<Pane>, popouts }` saved as `layout.json` per computer. `ui.rs::pane_ui` matches on `Pane`. Pop-outs go through `draw_popout`.
- **New interface.** `/home/user/NewQ/src/ui/modern.rs`: `View { Editor, Operator, Patch }`, `UiState::modern_view`. `modern::draw` replaces the transport and dock when `Prefs::new_layout` is on. It reuses `cue_list`, `inspector`, `active_panel` and `output_previews`.
- **Prefs.** `/home/user/NewQ/src/prefs.rs`: `prefs.json` per computer. New fields are `#[serde(default, skip_serializing_if)]`. The test builds a `Prefs` literal, which must gain any new field.
- **UI tests.** `egui::Context::default()`, `ctx.run_ui(RawInput { .. }, |ui| …)`, synthetic pointer events, rects recorded under `cfg(test)` in `ctx.data` (`"cue_rows"`, `"modern_patch_rects"`), then `apply`. See `ListMouse` (ui.rs ~14129) and `drag_in_patch` (modern.rs).
- **Program picture.** `output_previews(ui, show, st)` draws `UiState::previews`, the real outputs' compositions. There is no second compositor for an off-air preview.
- **Module visibility.** Child modules (`src/ui/*.rs`, a new `src/engine/*.rs`) can read their parent's private items. New code can live in new files without widening visibility.

---

## 2. Design decisions

### 2.1 One mechanism for both uses: a timeline run

A timeline is addressed by:

```rust
pub enum TimelineKey { Group(CueId), List(u32) }
```

- `Group`: a `GroupMode::Timeline` group that is not a slide show. Its clips are its children.
- `List`: a cue list with `CueList::timeline = Some(..)`. Its clips are the list's top-level cues. Groups are not allowed in a timeline list in v1: the UI refuses the move, and the engine fires one at its time without chasing into it.

A clip's start is `Cue::pre_wait`. Its length is `Engine::cue_duration`. No new field on `Cue`.

The engine keeps one `TimelineRun` per key, with a real clock (`pos`):

- Waiting clips stay in `Engine::pending`, tagged with the run (`PendingCue::run`, `PendingCue::at`). Their `remaining` is derived from `at - run.pos` instead of counted down. All existing cancel paths (`pending.retain` in stop, fade-and-stop, `stop_all`) and the tests that look at `e.pending` keep working.
- Each tick, a running run moves `pos` forward and starts due clips **one at a time in time order**. If a clip changes the run's state (a Pause, Stop or Jump control cue), the rest stay pending. This is what makes "stop exactly at the pause cue" correct; today's collect-then-start loop cannot do it.
- **Tie rule:** at equal times, control cues go before fade and stop cues, which go before clips. A Pause cue placed exactly at a clip's start, or exactly at a fade cue, stops the clock *before* that clip or fade. No epsilon is needed in the editor.
- Slide-show groups stay on the legacy path untouched.
- All plain Timeline groups, old or new, use the run. The acceptance gate is that the existing engine tests pass unmodified.
- One deliberate behaviour change: re-firing a timeline group cancels the earlier run's still-waiting children. Clips already playing are left to the existing replacement rules.

Run states:

| State | Clock | Clips | Waiting clips |
|---|---|---|---|
| `Stopped { pos }` | still | none | none |
| `Cueing` | still | started held, waiting to be ready | pending, frozen |
| `Running` | advances by `dt` | play | pending, due by `pos` |
| `Paused { freeze, waits_for_go }` | still | per `freeze` | pending, frozen |

### 2.2 Pause cues: two kinds, plus clip-level pause

Content is free-running by default.

**Timeline pause cue** (`ControlAction::TimelinePause`). It always stops the *clock* at the cue; no later clip, fade or transition starts. `ControlCue::freeze` says what happens to content:

| `freeze` | Meaning |
|---|---|
| `Nothing` (default) | Content free-runs. Clips that are playing keep playing or looping. The "hold before the next transition while the loop keeps going" case. |
| `Everything` | Pause on a frame: every clip of the timeline holds its frame, sound stops. |
| `Clips` | Only the clips in `ControlCue::clips` freeze; the rest free-run. |

Both scopes of "pause on a frame" are planned, as confirmed: the whole timeline (`Everything`) or the clips the cue names (`Clips`).

In all three, the next GO (or a Run cue, or the editor's Run) restarts the clock and un-freezes what that cue froze.

**Clip pause without stopping the clock.** The existing `Pause` and `Resume` control actions with `target` = a clip, placed on the cue track. The clip freezes and the timeline carries on. A clip frozen this way stays frozen until its own Resume; a GO does not release it. The editor also offers "Freeze clip / Resume clip" on a playing clip's menu.

**The transport's Pause button** on a timeline is `Everything` (as global pause is today). Free-run pauses only come from cues, so they are statically known from the show; sections 2.3 and 2.4 use that.

**How "freeze" is done per kind:**

| Kind | Freeze |
|---|---|
| Video | `ActiveCue::hold` (existing): frame held, voice paused. Resumes from that frame. |
| Audio | `hold`: voice paused, resumes from the same sample. |
| Image, text, shape | `hold` stops `elapsed`. Plain stills look the same; motion and effect keyframes stop; a timer text stops counting. |
| Clock text | Keeps showing wall time. |
| Camera, live sound | **Stay live** in v1 (a live player ignores its clock). Only their "run for" time stops. Freezing a camera's last frame is an optional item in P5. |
| Looping clip | Holds within its pass and resumes there. |

### 2.3 After a free-run pause: transitions are timed to the GO

This is the key use (a stage manager calls the end of a song; the track or loop runs as long as needed; GO makes the next transition happen).

**Rule.** After a resume, everything scheduled runs from the resumed clock, regardless of where the free-running content is:

- the next clip's start and its crossfade-in,
- fade and stop cues,
- each clip's own scheduled fade-out and end.

A clip whose content has left the clock is *detached*: its content position is no longer tied to the timeline.

**Clock-driven clips.** A clip is *clocked* when either:

- a free-run pause cue, or a Pause of that clip, lies inside its span (start to end inclusive, known from the show when it starts), or
- it becomes detached at run time (an operator's "Freeze clip").

For a clocked clip the engine drives its end from the timeline:

- Its scheduled end is `ends_at = start + length` in timeline time.
- Its own automatic fade-out begins when `run.pos >= ends_at - fade_out` and it ends at `ends_at`.
- While the clock is paused, neither happens. The content-driven fade-out and end in `tick` are switched off for it.
- Its own content running out is handled by 2.4.

Clips that are not clocked behave exactly as today (content-driven). Old shows have no pause cues inside clips, so nothing changes for them.

**How this is built in the editor.** Three patterns, all with a Pause cue on the cue track:

| What the operator wants on GO | How it is laid out |
|---|---|
| The next clip crossfades in over the running one | Pause cue at the next clip's start (same time; the tie rule puts the pause first). |
| The running clip fades out by its own fade-out | Pause cue at the start of the clip's fade-out (`end - fade_out`). |
| A fade or stop cue fires | Pause cue at the fade cue's time. |

To make this easy:

- Clips draw their fade-in and fade-out as handles, and the fade-out's start is a snap point.
- A clip's menu has **"Pause before this clip comes in"** and **"Pause before this clip fades out"**. They add a `TimelinePause` (free-run) at exactly that time.
- A marker's menu has "Add Pause cue here".
- While paused, the editor shows what GO will do: "GO: crossfade to sunset.mp4".
- A detached clip is drawn hatched and marked "free-running".

The model case needs nothing exotic: a looping clip, a Pause cue at the next clip's start, and the next clip with a crossfade transition on the same track.

### 2.4 A clip that outruns its slot: loop, hold or end

When a clocked clip's content reaches its end before the timeline moves on, the per-clip field `MediaCue::run_out` decides. It is shown in the inspector as "If it runs out before the timeline moves on":

| Value | Behaviour |
|---|---|
| `Auto` (default, not written) | Video holds its last frame; sound ends with its own fade-out. |
| `Loop` | Goes round again from its in point until the timeline ends it. |
| `Hold` | Holds its last frame (sound: silence). |
| `End` | Ends, **fading out** over `MediaCue::run_out_fade`. |

**The fade on End** (confirmed requirement):

- `run_out_fade: Option<f64>`.
- `None` (default, not written) means the clip's own fade-out time (`fade_out`), finishing as the content ends, as an ordinary cue ends today.
- `Some(t)` is a fade of `t` seconds finishing as the content ends.
- `Some(0.0)` is a hard cut.

**Interaction with the clip's own loop setting:**

- A clip that already loops for ever never runs out, so `run_out` is ignored.
- A loop with a set number of plays runs out after them, then `run_out` applies.

**Implementation.**

- `Loop` is decided when the clip starts: a clocked clip with `run_out == Loop` is spawned as a looping playback and ended by the clock. This is seamless, because the readers see the passes as one run.
- A clip that becomes detached only at run time (operator freeze) and was not spawned looping falls back to `Hold`.
- `Hold` uses the existing `held` state, ended by the clock.
- `End` uses the existing content-driven fade-out with the chosen time.

### 2.5 A clip at opacity 0 waits for its fade-in

**Rule (default for timelines made in the editor).** A clip that is invisible when the playhead reaches it is not started. NewQ may preload it. Its content starts from its beginning at the moment its fade-in begins.

**Scope and compatibility.**

- The rule is on per timeline: `TimelineSettings::start_on_fade_in` (default `false`, not written).
- Timelines created by the editor set it `true`.
- An old group has no settings, so old shows behave exactly as today.
- The timeline's inspector has the checkbox "Clips at opacity 0 wait for their fade-in".
- The per-clip opt-out is `MediaCue::runs_unseen: bool` (default `false`, not written), shown as "Already running when it fades in". Turn it on for a clip or loop that should be rolling and appear at an arbitrary point.

**What "invisible" and "fade-in begins" mean.** Define the clip's *lead-in* `V`, in the clip's own time:

| Case | Invisible? | Fade-in begins (`V`) |
|---|---|---|
| The cue's own transition fade-in (`fade_in > 0`) | No. The ramp starts at the clip's start. | `V = 0`: the clip starts at its place as today. The transition never defers. |
| Opacity keyframes (`Motion` opacity track) that are 0 from the start | Yes, while every key so far is 0. | `V` = the time of the last zero key before the opacity rises (the key the rise starts from). Known from the show. |
| Static opacity 0 (`opacity == 0`, no opacity keys) | Yes. | When a **fade cue** that targets the clip (or its group) with an opacity above 0 fires. The engine starts the clip at that moment, then applies the fade. `V` = that time minus the clip's start. |
| Audio clip at silence (`volume_db <= MIN_DB`) | Treated the same way. | When a fade cue raises its volume. |
| Anything else | No. | `V = 0`. |

**What starts at `V`.**

- The clip's **content** starts from its in point (file clips, loops from the loop's start, a camera is opened, a timer text counts from 0).
- The clip's **own time** (`elapsed`) is `V`, not 0. Opacity, motion and effect keyframes therefore stay where the user drew them.
- Implementation: decoders are prepared at file offset 0 and the cue is started with `seek_base = elapsed = V`.
- The clip enters by its own transition at `V`, not at its start. A Cut therefore replaces its layer at `V`.
- A video's **sound** follows the same start as its picture, as the owner expected. If the sound should lead, turn the option off for that clip.
- Stills, text and shapes have no content to defer; the rule is harmless for them.

**Keyframed speed.** Speed keys are in the clip's own time like every other key, so the lead-in counts. The file position at clip time `t >= V` is `start + M(t) - M(V)`, where `M` is `SpeedMap::media_at`. `prepare_from` gains a `lead` argument for this. With a steady speed this is `(t - V) * rate`.

**Length.** The clip's bar is `V` plus the content's length. `cue_duration` adds the lead-in when the rule applies. The editor draws the lead-in hatched ("waiting to fade in").

**While waiting.** The clip is a run-tagged pending entry.

- With keyframes, `at = start + V`.
- With static opacity 0, it has no time (`waits_for_fade`) and is started by the fade cue.
- It is preloaded once the playhead has reached its start, subject to the preload cap.
- If the timeline ends or is stopped first, it never starts.

**Seek.** See the table in 2.6: a seek into the lead-in leaves the clip waiting; a seek into the visible part starts it at the content position counted from `V`.

A pure helper gives all of this to the engine, the chase and the editor:

```rust
pub enum LeadIn { None, At(f64), UntilFaded }
pub fn lead_in(show: &Show, key: TimelineKey, cue: &Cue) -> LeadIn;
```

### 2.6 Seek: the rule for each kind

`timeline_seek(key, pos)` computes a *chase*: a pure simulation of the timeline from 0 to `pos` (`src/timeline/chase.rs`), giving the clips alive at `pos` and each one's content offset. It then starts those held and leaves the rest pending.

| Kind | Rule when the playhead lands inside or after it |
|---|---|
| Video, audio file | Start at clip time `from = pos - start` via `prepare_from` (in point plus offset; speed applied). |
| Keyframed speed | Same; `prepare_from` maps `from` through the `SpeedMap` to the file position. Clip length is `SpeedMap::time_for`. |
| Loop, set plays | `from` across the passes (`Playback::started_at`). |
| Loop, for ever | `from` modulo the pass. Alive until replaced or stopped. |
| Video ended before `pos` with `hold_last_frame`, not yet replaced | Start at its last frame, held. |
| Image, text, shape | Start with `elapsed = from`. Motion and effects follow. Open-ended (no hold time) is alive until replaced or stopped. |
| Timer text | `elapsed = from`, so the count is right. Clock text shows now. |
| Camera, live sound | Open live now; `elapsed = from` so "run for" ends on time. |
| **Clip with a lead-in (2.5), `pos` inside the lead-in** | Not started. It waits, preloaded, and starts at `V` (or at its fade cue). |
| **Clip with a lead-in, `pos` in the visible part** | Started at clip time `from = pos - start`; content position counted from `V` (`M(from) - M(V)`). For a fade-triggered clip, `V` comes from the first fade cue on the timeline that raised it before `pos`. |
| Replacement | A picture is not alive if a later clip on the same display and layer started before `pos` and replaced it (cut: at once; crossfade: after its fade). With `own_layers` off: any later picture on the display. Mirrors `apply_entry`. |
| Clip's own fade-in in progress | Start at the interpolated level and ramp the rest. |
| Clip's own automatic fade-out in progress | Full level, then fades over what is left. Same simplification as today's scrub (`restart_auto_fade`). |
| Transitions | Chase-started clips cut in together (`together`). No dip or crossfade is replayed, apart from an in-progress fade-in. |
| Fade cue finished before `pos` | Target gets its end values (opacity, volume, geometry). `stop_target`: target not alive. |
| Fade cue in progress | Target starts at the interpolated value and ramps the remainder. The curve restarts over the remainder: a small shape error, accepted. |
| Stop cue passed | Target (or everything) not alive. A stop fade still in progress is treated as finished. |
| Control cues passed | **Not executed.** No Start, GoTo, Arm or Disarm of other things. Exception: `Pause`/`Resume` of a *clip* are simulated so the clip's state is right (next rows). |
| Timeline Pause cue passed | Skipped. The run keeps the state it had: Running stays Running, otherwise it is parked frozen. |
| Clip frozen by a `Pause` cue at `Tp`, no `Resume` before `pos` | Started at `from = Tp - start`, frozen, waiting for its Resume. |
| Clip frozen then resumed (`Tp` to `Tr`) | `from = (pos - start) - (Tr - Tp)`, detached. |
| Free-run pauses | No effect on the chase. Timeline time is the reference; how long an operator once waited is not part of it. A seek re-attaches every clip. |
| Seek while the run is frozen or parked | Re-chase; everything stays held. Earlier detachment is forgotten. |
| Send cues (OSC, MIDI, projector, script), Timecode cues, Wait cues | Never fired by a seek. A Timecode cue spanning `pos` is not started (v1). |
| Cue exactly at `pos` | Counts as passed, so GO from a marker on a pause cue runs. Exception: `pos == 0` behaves as a start from the top. |
| Disarmed clips | Skipped, as everywhere. |

**The hard engine problems, with recommendations:**

1. **Releasing together.** Cold decoders become ready at different times; a FFmpeg start took 3 to 4 s now and then on a Mac. Recommendation: the `Cueing` state. Seek-started clips are started held, show their first frame and stay silent. The clock stays still until all are `started` or `CUE_LIMIT` (2 s) has passed, then all are released in one tick. A parked timeline is already cued, so GO from a parked playhead is instant.
2. **Order within a tick** (pause exactly at the cue). Solved by the run loop and tie rule in 2.1.
3. **Two time bases per clip.** Clip time (keyframes) and content time differ after a lead-in, a freeze or a free-run pause. Recommendation: keep `elapsed` as clip time everywhere, and carry the difference in `seek_base` and the `lead` given to `prepare_from`. Nothing else in the engine needs to know.
4. **Too many decoders.** Preload only run-tagged pending clips within `PRELOAD_HORIZON` (10 s) of `run.pos`, plus waiting lead-in clips, capped by `MAX_PRELOAD_CHILDREN`.
5. **Seek flash.** Old clips are removed and new ones appear as they become ready. Wave 1 accepts a brief partly-built picture. P5 improves it: a clip alive before and after is jumped with the existing `Seek` swap, and removed clips are kept until the new set is cued.
6. **Jump loops are not seamless.** A Jump back restarts decoders. P5 preloads a Jump cue's chase set ahead (`Prepared` gains a `shift`). Until then a jump has a visible start delay.
7. **Clock source.** `pos` advances by frame `dt`; clips follow audio clocks. Over an hour-long timeline, clips started far apart can drift against each other by the audio device's ppm error. Recommendation: document it and leave it. A stall delays clip starts by the stall, as today.
8. **Abutting clips.** A clip ending at `t` and the next on the same track starting at `t` leaves a tick or two of black. P5: an outgoing clip with a successor on its track within 50 ms is held until the successor's entry replaces it.

### 2.7 The picture at the playhead when stopped

- **Cheapest correct way: no preview renderer.** Parking *is* the seek chase with the run left frozen. Clips under the playhead are started held on the real outputs. The existing compositor and `output_previews` show them. GO releases them.
- **Cost:** one FFmpeg (plus a paused voice) per video or audio clip under the playhead, spawned once on release, not while dragging. The main loop runs at full rate while parked, as it does for a paused cue today.
- The editor keeps the dragged playhead in UI state and emits one `Seek` on release.
- **Consequence the owner must accept:** the parked picture goes to the real outputs, as in WATCHOUT. Guard: in show mode the ruler of a timeline that is not the active list's is locked (status-bar note). A group timeline can be parked only in edit mode. `timeline_seek` takes `show_picture: bool` so the UI can move a playhead without output.
- A true off-air preview would need a second compositor path and second decoders. Not recommended.

### 2.8 Addressing, and how pause meets the existing transport

- **Addressing.** `ControlCue::timeline: Option<TimelineKey>`.
- `None` means the timeline this cue is on (its group, or its timeline list).
- `Some(Group(id))` and `Some(List(n))` name another one (auxiliary timelines).
- A cue not on a timeline with `None` logs a problem.
- **Position.** `ControlCue::at: Option<TimelinePoint>`, where `TimelinePoint` is `Time(f64)`, `Marker(u64)` or `Cue(CueId)` (that clip's start). Run uses it as "from"; Jump requires it.
- **New actions:** `TimelineRun`, `TimelinePause`, `TimelineStop`, `TimelineJump`. They need no `target`.
- **Existing actions on a timeline group target** are routed to its run:
- `Pause`: freeze everything, clock too. This fixes the pending-children bug.
- `Resume`: run on.
- `Start`: from the top.
- `Reset`: stop and forget.
- On a single clip they stay clip-level.
- **Markers** are labels and jump targets only. They never hold.

Transport rules:

| Event | Standalone timeline tab (active list is a timeline) | Timeline group in a cue list |
|---|---|---|
| GO, run Stopped or parked | Run from the playhead. | Normal GO. Firing the group always starts from 0, discarding a parked state. |
| GO, run Paused by a pause cue | Resume. | If the group was started from the active list and is waiting at its own pause cue (`waits_for_go`), GO resumes it **before** firing anything else. Same precedent as a slide deck (`deck_step`). Another list's GO does not. |
| GO, run Running | No-op, logged. The GO area shows "RUNNING · next pause in m:ss". Never a restart. | Normal GO (next cue). |
| GO at the end | No-op: "the timeline is at its end". The playhead stays at the end. | Not applicable (the group has ended). |
| Global Pause (P) | Freezes everything. On resume, a timeline paused by a cue stays paused (`hold` is separate from `Engine::paused` already). | Same. |
| STOP ALL / Esc | Run becomes `Stopped` with `pos` kept. No picture until parked again or GO, which cues from cold. | Run removed with the group. |
| Reset all | `pos = 0`. | Not applicable. |
| Firing a clip on its own (hotkey, cart, OSC) | Starts now, ignoring its `pre_wait` and any lead-in. | As today (honours `pre_wait`). |
| Continues on clips | Ignored in a timeline list (`fire_next` guard). | As today (children never chain). |

### 2.9 Tracks

`TrackId` is derived from the clip, never stored on it:

- **Picture:** `(MediaCue::output, MediaCue::layer)`. Recommendation differs slightly from "a track is a layer": the engine replaces per display *and* layer, so one track per (display, layer). With one display it reads "V2 · Layer 1" exactly as approved.
- **Sound:** `MediaCue::audio_output`. A video's own sound is a badge on its picture clip, not a second clip.
- **Cues:** one thin track for fade, stop, wait, send, control and timecode cues.
- Empty tracks and track order live in `TimelineSettings::tracks`.
- Dragging a clip to another track sets `layer`/`output` or `audio_output`. `Engine::tick` already copies layer and output into a playing cue, so it takes effect live.

**Overlapping clips crossfade** is opt-in per timeline: `TimelineSettings::crossfade_overlaps`, on for timelines made in the editor, off (absent) for old groups. Old shows play exactly as before and no cue field is rewritten. The engine (P5) derives the crossfade length from the overlap when the later clip starts:

- Picture: the entry becomes `Crossfade { time: overlap }` and the earlier clip's own fade-out is skipped.
- Sound: the later clip fades in and the earlier fades out over the overlap.

Until P5, the "crossfade to the next clip on GO" pattern uses the next clip's own crossfade transition, which works from Wave 1.

### 2.10 Speed keyframes in the timeline

- Reuse `MediaCue::rate` (`Animated`) and `speed.rs`. No new data.
- The editor gets a speed lane on a selected video or audio clip: the keyframe line drawn across the clip's width. It edits a copy and emits `SetRate(id, Animated)`.
- The clip's length on the timeline follows the speed map automatically, because clip length is `cue_duration`.
- Trimming with speed:
- Right-trim: `end = start + map.media_at(new_len)` (steady: `new_len * rate`), less the lead-in where there is one.
- Left-trim: moves `start` and `pre_wait` together. Disabled for keyframed speed in v1 (the keyframes would need re-timing).
- Trim is disabled for looping clips.
- Split at the playhead on a clip with speed, motion or effect keyframes shifts the second half's keys by the split offset (`Animated` key shift, pure and tested).
- `MIN_RATE` is 0.25, so speed cannot reach zero. Stopping a clip is the clip Pause cue, not a keyframe.

### 2.11 Layouts

One component, `ui::timeline::editor(..)`, hosted four ways. The setting is `Prefs::timeline_layout: TimelineLayout { Pane, Workspace }`, in Settings ▸ General and as a toggle in the editor toolbar.

| | Classic interface | New interface |
|---|---|---|
| **A · Pane** (default) | `Pane::Timeline` in the dock, opened split below the cue list. Pops out like any pane. | A resizable bottom panel in the Editor view, above the inspector. |
| **B · Workspace** | While the editor is open, a workspace replaces the dock: bin, program monitor, inspector, tracks full width. A "Back to cue list" button closes it. | A fourth `View::Timeline`. |

- For a timeline list tab, the cue list pane draws the tracks editor in place of rows.
- The Timeline pane follows the selected timeline group.
- The component takes an id salt so two hosts never share egui ids.
- Selection is the existing `UiState::selected`/`multi`, so the inspector edits the picked clip unchanged.

### 2.12 Show-file compatibility

Every field is `#[serde(default)]`, omitted at its default, with an "old file loads" and "default not written" test.

| Field | Default (not written) | Note |
|---|---|---|
| `CueList::timeline: Option<TimelineSettings>` | `None` | `Some` marks a timeline list. Inner fields are omitted individually, so it is `"timeline": {}` at minimum. |
| `GroupCue::timeline: Option<TimelineSettings>` | `None` | Set back to `None` when it becomes default (`TimelineSettings::is_default`), so an untouched group is written as before. |
| `TimelineSettings::markers: Vec<Marker>` | empty | `Marker { id: u64, time: f64, name: String, color: Option<CueColor> }`. |
| `TimelineSettings::tracks: Vec<TrackId>` | empty | Extra or empty tracks, and order. |
| `TimelineSettings::crossfade_overlaps: bool` | `false` | Opt-in; the editor sets it on new timelines. |
| `TimelineSettings::start_on_fade_in: bool` | `false` | Opt-in; the editor sets it on new timelines. Old groups keep today's behaviour. |
| `MediaCue::runs_unseen: bool` | `false` | Per-clip opt-out of 2.5. Only read on a timeline with `start_on_fade_in`. |
| `MediaCue::run_out: RunOut` | `Auto` | Only read for clocked clips. |
| `MediaCue::run_out_fade: Option<f64>` | `None` | The fade on End. |
| `ControlCue::timeline: Option<TimelineKey>` | `None` | |
| `ControlCue::at: Option<TimelinePoint>` | `None` | |
| `ControlCue::freeze: PauseFreeze` | `Nothing` | |
| `ControlCue::clips: Vec<CueId>` | empty | |
| `ControlAction::{TimelineRun, TimelinePause, TimelineStop, TimelineJump}` | n/a | Written only when used. |
| `Prefs::timeline_layout` (`prefs.json`) | `Pane` | Per computer. |
| `Pane::Timeline` (`layout.json`) | n/a | Per computer. An older NewQ reading it falls back to the default layout. |

- No new field on `Cue`. Start is `pre_wait`; the track is derived.
- "Default-on for new timelines" is done with flags on the timeline's settings, not with cue-field defaults. That is what lets the defaults stay unwritten and old shows stay as they are.
- A timeline made by the editor is therefore written with `"crossfade_overlaps": true, "start_on_fade_in": true`.
- **Not forward compatible:** a show that uses a new control action fails to open in 0.20.3 and earlier (unknown enum variant). Older versions silently drop the other new fields on save. Say so in the README.
- `paste_cues`, `duplicate` and `remove_list` must remap or clear `ControlCue::timeline`, `at` (`Cue`) and `clips`, as `target` is today.
- Add `"timeline"`, `"at"` and `"clips"` to `bulk_edit::OWN_IN_KIND`.
- The main list cannot be a timeline in v1 (it has no `CueList`). Everything goes through `timeline::settings(show, key)`, so adding it later is one field.

### 2.13 Where I think the approved design will bite

1. **GO becomes context-dependent in a cue list** (it resumes a paused timeline cue before firing the next cue). I think this is right and matches the slide deck, but the operator must see it: the standing-by area should read "GO resumes 4 · Opening sequence".
2. **GO does nothing while a standalone timeline is running.** That is safe against double presses, but an operator used to a cue stack will press it expecting "next". The transport must show it is running and when the next pause is.
3. **Parking puts picture on the real outputs.** See 2.7 for the guard. Needs the owner's yes.
4. **A free-run pause breaks sync by design.** After resuming, a detached clip's sound and picture no longer line up with later clips. The editor shows it; it cannot be fixed.
5. **The opacity-0 rule has a sharp edge with Cut.** A waiting clip enters by its own transition at `V`. A Cut at opacity 0 clears its layer and then fades up from black. For "fade up over what is there", the clip belongs on a higher track or needs a fade-in transition. The editor should warn when a waiting clip with Cut shares a track with a clip under it.
6. **Static opacity 0 with no fade cue never starts.** That is the rule as stated, but it looks like a broken clip. The editor should mark it "waits for a fade cue".
7. **Not frame-accurate.** Clip starts are quantised to the tick (about 16 ms). A split audio clip will click or gap at the join; abutting pictures need the P5 gapless rule. Splitting sound should carry a warning.
8. **Jump-back loops glitch** until P5's preloaded jumps, and even then depend on FFmpeg start time.
9. **Shows with `own_layers` off:** tracks mean little, since every picture replaces everything on the display. Creating a timeline in such a show should offer to turn it on, never do it silently.
10. **A show-long timeline holds its place only in memory.** After a crash and restart the playhead is at 0. Markers plus "Jump to marker" are the recovery path; put it in the README.
11. **Process count.** Parked clips, waiting lead-in clips and the 10 s horizon can mean a dozen FFmpegs on a busy timeline. The cap is `MAX_PRELOAD_CHILDREN`; beyond it clips start cold.
12. **Cameras don't freeze** in "pause on a frame" (v1). Say so in the cue's description text.

---

## 3. Interfaces (land in Wave 0, then frozen)

Parallel packages code against these. Changing one after Wave 0 needs the integration package of that wave.

### 3.1 Data model: `/home/user/NewQ/src/timeline/mod.rs` (new; types re-exported for `show.rs`)

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TimelineKey { Group(CueId), List(u32) }

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum TimelinePoint { Time(f64), Marker(u64), Cue(CueId) }

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PauseFreeze { #[default] Nothing, Everything, Clips }

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunOut { #[default] Auto, Loop, Hold, End }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TrackId {
  Picture { output: Option<OutputId>, layer: i32 },
  Sound { output: Option<AudioOutputId> },
  Cues,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Marker { pub id: u64, pub time: f64, pub name: String,
  #[serde(default, skip_serializing_if = "Option::is_none")] pub color: Option<CueColor> }

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TimelineSettings {
  #[serde(default, skip_serializing_if = "Vec::is_empty")] pub markers: Vec<Marker>,
  #[serde(default, skip_serializing_if = "Vec::is_empty")] pub tracks: Vec<TrackId>,
  #[serde(default, skip_serializing_if = "is_false")]     pub crossfade_overlaps: bool,
  #[serde(default, skip_serializing_if = "is_false")]     pub start_on_fade_in: bool,
}
impl TimelineSettings {
  pub fn is_default(&self) -> bool;
  pub fn for_editor() -> TimelineSettings;   // both flags on
}
```

`show.rs` additions (the only edits there):

```rust
// CueList
#[serde(default, skip_serializing_if = "Option::is_none")] pub timeline: Option<TimelineSettings>,
// GroupCue
#[serde(default, skip_serializing_if = "Option::is_none")] pub timeline: Option<TimelineSettings>,
// MediaCue
#[serde(default, skip_serializing_if = "RunOut::is_auto")]   pub run_out: RunOut,
#[serde(default, skip_serializing_if = "Option::is_none")]   pub run_out_fade: Option<f64>,
#[serde(default, skip_serializing_if = "is_false")]          pub runs_unseen: bool,
// ControlCue (existing: action, target)
#[serde(default, skip_serializing_if = "Option::is_none")] pub timeline: Option<TimelineKey>,
#[serde(default, skip_serializing_if = "Option::is_none")] pub at: Option<TimelinePoint>,
#[serde(default, skip_serializing_if = "PauseFreeze::is_nothing")] pub freeze: PauseFreeze,
#[serde(default, skip_serializing_if = "Vec::is_empty")] pub clips: Vec<CueId>,
// ControlAction: + TimelineRun, TimelinePause, TimelineStop, TimelineJump
impl ControlAction {
  pub const ALL: [ControlAction; 10];        // unchanged (Test Mode asserts on it)
  pub const TIMELINE: [ControlAction; 4];    // the four new ones
  pub fn is_timeline(self) -> bool;          // needs_target() is false for these
}
```

### 3.2 Read model: `/home/user/NewQ/src/timeline/model.rs` (pure; Wave 0 implements it for real)

```rust
/// How long a cue's content is, if known: the engine's own length sum, or a fake in tests.
pub type Dur<'a> = &'a mut dyn FnMut(&Cue) -> Option<Option<f64>>;

pub enum LeadIn { None, At(f64), UntilFaded }

pub struct Clip {
  pub id: CueId, pub start: f64,
  pub len: Option<f64>,            // lead-in included; None = open-ended or not known yet
  pub lead_in: LeadIn,             // the hatched "waiting to fade in" part
  pub fade_in: f64, pub fade_out: f64,   // for the handles and the snap points
  pub track: TrackId, pub ty: CueType, pub name: String,
  pub armed: bool, pub color: Option<CueColor>,
  pub trim_left: bool, pub trim_right: bool,   // what the editor may drag
  pub has_sound: bool, pub loops: bool, pub speed_keys: bool,
  pub clocked: bool,               // a free-run pause or clip pause lies inside it
}

pub fn is_timeline(show: &Show, key: TimelineKey) -> bool;
pub fn key_of(show: &Show, cue: CueId) -> Option<TimelineKey>;   // the timeline a cue is on
pub fn members<'a>(show: &'a Show, key: TimelineKey) -> Vec<&'a Cue>;   // show order
pub fn settings(show: &Show, key: TimelineKey) -> Option<&TimelineSettings>;
pub fn settings_mut(show: &mut Show, key: TimelineKey) -> Option<&mut TimelineSettings>; // makes Some
pub fn settle(show: &mut Show, key: TimelineKey);   // a group's default settings back to None
pub fn track_of(cue: &Cue) -> TrackId;
pub fn lead_in(show: &Show, key: TimelineKey, cue: &Cue) -> LeadIn;      // section 2.5
pub fn clocked(show: &Show, key: TimelineKey, cue: &Cue, len: Option<f64>) -> bool;  // section 2.3
pub fn clips(show: &Show, key: TimelineKey, dur: Dur) -> Vec<Clip>;
pub fn tracks(show: &Show, key: TimelineKey, clips: &[Clip]) -> Vec<TrackId>;
  // pictures by display order then layer high to low, sounds by output order, Cues last
pub fn length(clips: &[Clip]) -> Option<f64>;       // None if any clip is open-ended
pub fn resolve(show: &Show, key: TimelineKey, p: TimelinePoint) -> Option<f64>;
pub fn track_label(show: &Show, t: TrackId) -> String;   // "V2 · Layer 1", "A1 · Main", "Cues"
/// The order of things due at the same time: control, then fade and stop, then clips.
pub fn due_order(cue: &Cue) -> u8;
```

### 3.3 Edit operations: `/home/user/NewQ/src/timeline/edit.rs`, `snap.rs` (P1)

All take `&mut Show`, are pure, and return `bool` (changed) unless stated.

```rust
pub enum Edge { Left, Right }
pub enum PauseBefore { ClipIn, ClipFadeOut }
pub fn move_clips(show: &mut Show, key: TimelineKey, ids: &[CueId], dt: f64, to: Option<TrackId>) -> bool;
  // dt applied to all, clamped so none starts before 0; `to` moves the anchor (ids[0])
  // to that track and the others by the same track step where the kind allows
pub fn trim(show: &mut Show, id: CueId, edge: Edge, to: f64, dur: Dur) -> bool;    // `to` = timeline time
pub fn split(show: &mut Show, key: TimelineKey, ids: &[CueId], at: f64, dur: Dur) -> Vec<CueId>; // new halves
pub fn delete(show: &mut Show, key: TimelineKey, ids: &[CueId], ripple: bool, dur: Dur) -> Vec<CueId>; // removed
pub fn add_clip(show: &mut Show, key: TimelineKey, ty: CueType, file: Option<PathBuf>,
              track: TrackId, at: f64) -> Option<CueId>;
pub fn add_control(show: &mut Show, key: TimelineKey, action: ControlAction, at: f64) -> Option<CueId>;
pub fn add_pause_before(show: &mut Show, key: TimelineKey, clip: CueId, what: PauseBefore, dur: Dur)
              -> Option<CueId>;   // a free-run TimelinePause at exactly that time
pub fn add_track(show: &mut Show, key: TimelineKey, picture: bool) -> Option<TrackId>;
pub fn remove_track(show: &mut Show, key: TimelineKey, t: TrackId) -> bool;      // only if empty
pub fn add_marker(show: &mut Show, key: TimelineKey, at: f64, name: &str) -> Option<u64>;
pub fn move_marker(show: &mut Show, key: TimelineKey, id: u64, to: f64) -> bool;
pub fn rename_marker(show: &mut Show, key: TimelineKey, id: u64, name: &str) -> bool;
pub fn remove_marker(show: &mut Show, key: TimelineKey, id: u64) -> bool;
pub fn set_rate(show: &mut Show, id: CueId, rate: Animated) -> bool;             // sanitised
pub fn sort_by_start(show: &mut Show, key: TimelineKey);   // stable; called by the ops above
pub fn new_timeline_list(show: &mut Show, name: &str) -> u32;        // TimelineSettings::for_editor()
pub fn new_timeline_group(show: &mut Show, at: Slot) -> CueId;       // TimelineSettings::for_editor()
pub fn set_list_timeline(show: &mut Show, list: u32, on: bool) -> Result<(), String>; // refuses groups

// snap.rs
pub struct SnapPoints(pub Vec<f64>);
pub fn points(clips: &[Clip], markers: &[Marker], playhead: Option<f64>, moving: &[CueId]) -> SnapPoints;
  // clip starts and ends, fade-out starts, ends of lead-ins, markers, the playhead
pub fn snap(t: f64, pts: &SnapPoints, tolerance: f64) -> (f64, Option<f64>);  // (snapped, the point hit)
pub fn snap_span(start: f64, len: Option<f64>, pts: &SnapPoints, tolerance: f64) -> (f64, Option<f64>);
```

### 3.4 Chase: `/home/user/NewQ/src/timeline/chase.rs` (P2b, pure)

```rust
pub struct Alive {
  pub id: CueId,
  pub from: f64,                    // clip time (`elapsed`) to start at
  pub lead: f64,                    // lead-in V; content position counts from here
  pub frozen: bool,                 // frozen by a clip Pause cue not yet resumed
  pub detached: bool,
  pub at_last_frame: bool,          // a held video that had ended
  pub opacity: Option<(f32, f32, f64, Curve)>,  // (now, to, seconds left, curve) from fades in progress or done
  pub volume_db: Option<(f32, f32, f64, Curve)>,
  pub geometry: Option<GeometryTarget>,         // end values of finished geometry fades
  pub stop_after: Option<f64>,      // a fade-and-stop in progress
}
pub struct Waiting { pub id: CueId, pub at: Option<f64>, pub lead: f64 }   // at None = waits for a fade cue
pub struct Chase { pub alive: Vec<Alive>, pub upcoming: Vec<Waiting>, pub passed: Vec<CueId> }
pub fn chase(show: &Show, key: TimelineKey, pos: f64, dur: Dur) -> Chase;
```

### 3.5 Engine API: `/home/user/NewQ/src/engine/timeline.rs` (new child module; stubs in Wave 0, real in P2a and P2b)

```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RunState { Stopped, Cueing, Running, Paused { freeze: PauseFreeze, waits_for_go: bool } }

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunView { pub key: TimelineKey, pub state: RunState, pub pos: f64,
                   pub next_pause: Option<f64>, pub length: Option<f64>,
                   pub on_go: Option<CueId> }   // what GO will bring in or fade, for the read-out

impl Engine {
  /// A timeline's state; `Stopped` at 0 when it has never been touched.
  pub fn timeline(&mut self, show: &Show, key: TimelineKey) -> RunView;
  pub fn timelines(&self) -> Vec<TimelineKey>;                 // those not Stopped
  /// Run from where it is: Stopped/parked -> cue then run; Paused -> resume; Running -> no-op.
  pub fn timeline_run(&mut self, show: &Show, key: TimelineKey);
  /// Stop the clock. `clips` only matters for `PauseFreeze::Clips`.
  pub fn timeline_pause(&mut self, show: &Show, key: TimelineKey, freeze: PauseFreeze,
                        clips: &[CueId], waits_for_go: bool);
  /// Stop its clips (fade seconds; 0 = cut), keep the playhead where it is.
  pub fn timeline_stop(&mut self, show: &Show, key: TimelineKey, fade: f64);
  /// Move the playhead. Keeps Running running; otherwise parks, with the picture
  /// (clips started held) when `show_picture`, without when not.
  pub fn timeline_seek(&mut self, show: &Show, key: TimelineKey, pos: f64, show_picture: bool);
  /// The clips of a run that are free-running, and those waiting to fade in, for the editor.
  pub fn timeline_detached(&self, key: TimelineKey) -> Vec<CueId>;
  pub fn timeline_waiting(&self, key: TimelineKey) -> Vec<CueId>;
}
```

Internal items added in `engine.rs` (not public API, but later packages rely on the names):

- `struct StartAt { from: f64, lead: f64, hold: bool, run: Option<u64>, together: Option<u64>, chase: Option<Alive> }` (`Default` = today's start).
- `start_with(show, id, reveal, StartAt)`.
- `prepare_from(cue, info, shift, lead)`: file position `M(shift) - M(lead)`.
- `fire_queue: VecDeque<(CueId, StartAt)>`.
- `PendingCue { .., run: Option<u64>, at: f64, waits_for_fade: bool, start: StartAt }`.
- `ActiveCue { .., run: Option<u64>, ends_at: Option<f64>, clocked: bool, detached: bool }`.

Named hook points in `engine.rs`. Each is a one- or two-line call into `engine/timeline.rs`:

| Hook | Where | Calls |
|---|---|---|
| H1 | `Engine` struct, `for_tests`, `new` | `runs: timeline::Runs` |
| H2 | `start_with`, Group arm, Timeline non-slides | `self.timeline_begin(show, TimelineKey::Group(id), instance)` in place of firing the children |
| H3 | `tick`, before the pending loop | `self.timeline_tick(show, dt)`. The pending loop skips entries with `run.is_some()`. |
| H4 | `tick`, after the `active.retain_mut` | `self.timeline_after_tick(show)` (clocked ends, group `elapsed = pos`, run finished) |
| H5 | `tick`, the automatic fade-out and completion | skipped for a `clocked` cue; `run_out` decides at the content's end |
| H6 | `advance_groups` | a group whose run is not `Running` is never done |
| H7 | `control`, `control_target`, `hold_cue` | `self.timeline_control(show, cue, c)`; Pause/Resume/Start/Reset on a timeline group |
| H8 | `go`, `go_list` | `if self.timeline_go(show, list) { return; }` after `deck_step` |
| H9 | `stop_all`, `reset_all`, `stop_instance`, `forget_lists`, `sync_lists`, `take_list_state`/`restore_list_state` | `self.timeline_stopped(..)` |
| H10 | `update_preload` | horizon filter on run-tagged pending; `wanted.extend(self.timeline_wanted(show))` |
| H11 | `fire_next`, `fire_one` | timeline-list guards (no continues; no pre-wait when fired alone) |
| H12 | `is_active` | true while a run is `Cueing` or `Running` |
| H13 | `status` | unchanged logic; a run-tagged pending reports `PreWait { remaining: at - pos }` |
| H14 | `start_with`, Fade arm | `self.timeline_wake(show, &targets)`: start clips waiting for this fade, before the ramp is applied |
| H15 | `cue_duration` | adds a clip's lead-in when 2.5 applies |

### 3.6 UI: `/home/user/NewQ/src/ui/timeline.rs` (new child module; stubs in Wave 0, real in P3)

```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimelineLayout { #[default] Pane, Workspace }      // lives in prefs.rs

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Host { Pane, ListTab, Workspace }                  // also the egui id salt

#[derive(Default)]
pub struct EditorState {              // UiState::timeline
  pub open: bool,                   // the editor is wanted (pane or workspace)
  pub pinned: Option<TimelineKey>,  // otherwise it follows the selection
  pub pps: f32,                     // points per second (zoom); 0 = fit
  pub scroll: Vec2,
  pub snap: bool, pub ripple: bool,
  /* private: drag, playhead drag, speed-lane clip */
}

/// The timeline the editor should show: the active list if it is a timeline, else the
/// selected timeline group (or the group of the selected child), else the pinned one.
pub fn target(show: &Show, engine: &Engine, st: &UiState) -> Option<TimelineKey>;

/// THE component: toolbar, ruler, markers, tracks, clips, playhead.
pub fn editor(ui: &mut Ui, show: &Show, engine: &mut Engine, st: &mut UiState,
            key: TimelineKey, host: Host, actions: &mut Vec<Action>);

/// Option A's body: `editor` for `target(..)`, or the empty state.
pub fn pane(ui: &mut Ui, show: &Show, engine: &mut Engine, st: &mut UiState, actions: &mut Vec<Action>);

/// The control cue's timeline fields, called from the inspector's Control arm.
pub fn control_fields(ui: &mut Ui, show: &Show, cue_id: CueId, c: &mut ControlCue);
/// A clip's timeline rows in the media inspector: run_out, its fade, runs_unseen.
pub fn clip_fields(ui: &mut Ui, show: &Show, cue_id: CueId, m: &mut MediaCue);
/// A timeline's own settings (the two flags), in the group's or list's inspector.
pub fn settings_fields(ui: &mut Ui, s: &mut TimelineSettings);
pub fn describe_control(show: &Show, cue: &Cue, c: &ControlCue) -> String;   // cue-list row text

pub enum TimelineAction {
  // edits (refused in show mode)
  MoveClips { key: TimelineKey, ids: Vec<CueId>, dt: f64, to: Option<TrackId> },
  Trim { id: CueId, edge: Edge, to: f64 },
  Split { key: TimelineKey, ids: Vec<CueId>, at: f64 },
  Delete { key: TimelineKey, ids: Vec<CueId>, ripple: bool },
  AddClip { key: TimelineKey, ty: CueType, file: Option<PathBuf>, track: TrackId, at: f64 },
  AddControl { key: TimelineKey, action: ControlAction, at: f64 },
  AddPauseBefore { key: TimelineKey, clip: CueId, what: PauseBefore },
  AddTrack { key: TimelineKey, picture: bool },
  RemoveTrack { key: TimelineKey, track: TrackId },
  AddMarker { key: TimelineKey, at: f64 },
  MoveMarker { key: TimelineKey, id: u64, to: f64 },
  RenameMarker { key: TimelineKey, id: u64, name: String },
  RemoveMarker { key: TimelineKey, id: u64 },
  SetRate { id: CueId, rate: Animated },
  NewTimelineList, NewTimelineGroup, SetListTimeline { list: u32, on: bool },
  // transport (allowed in show mode)
  Run(TimelineKey), Pause(TimelineKey), Stop(TimelineKey),
  Seek { key: TimelineKey, pos: f64 },          // on release of the playhead
  JumpToMarker { key: TimelineKey, id: u64 },
  FreezeClip(CueId, bool),
  // view
  Open(Option<TimelineKey>), Close, SetLayout(TimelineLayout),
}
impl TimelineAction { pub fn edits_the_show(&self) -> bool; }
pub(super) fn apply(a: TimelineAction, show: &mut Show, engine: &mut Engine, st: &mut UiState);
```

In `ui.rs`:

- `Action::Timeline(timeline::TimelineAction)`.
- One arm in `apply`.
- One line in `edits_the_show`: `Action::Timeline(a) => a.edits_the_show()`.

Edits commit on **release** (one action, one undo step). The drag is previewed from `EditorState`.

Test rects under `cfg(test)`, stored at `egui::Id::new(("timeline_rects", host))`:

```rust
pub struct TimelineRects { pub ruler: Rect, pub clips: Vec<(CueId, Rect)>,
  pub tracks: Vec<(TrackId, Rect)>, pub markers: Vec<(u64, Rect)>, pub playhead_x: Option<f32>,
  pub x0: f32, pub pps: f32 }
```

---

## 4. Work packages

Rules for every package:

- Own worktree and branch; one feature per commit.
- Before handing back: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `NEWQ_REQUIRE_MEDIA=1 cargo test`.
- Touch only the files and functions listed. A needed change elsewhere is reported, not made.
- Use `git grep`, not `grep -r`.

File contention across waves:

| File | W0 | W1 | I1 | W2 | I2 | W3 | I3 |
|---|---|---|---|---|---|---|---|
| `src/show.rs` | P0 | none | fixes | none | fixes | P8 (`ALL` only) | none |
| `src/engine.rs` | P0 (H1 only) | P2a, then P2b | fixes | P5 | fixes | none | none |
| `src/ui.rs` | P0 (Action variant) | P3 | I1 | P4 | I2 | P7 (`draw`, `menu_bar`); P8 (remote handler only) | I3 |
| `src/ui/modern.rs` | none | P3 | none | P4 | none | P7 | none |

Wave 1 runs three agents at a time: P1, P3 and the engine chain. The engine work is two packages on one branch lineage, **P2a then P2b**, because both own `engine.rs`. P2b starts from P2a's branch when P2a passes its checks.

### Wave 0 (one agent, serial)

#### P0 · Skeleton and data model · **M**

**Goal.** Everything in section 3 exists and compiles. The data model and read model are real. Engine and UI entry points are stubs that do nothing. No visible change in the app.

**Owns.**

- `src/timeline/mod.rs`, `src/timeline/model.rs` (new, real, including `lead_in`, `clocked`, `due_order`).
- `src/timeline/{edit,snap,chase}.rs` (new, signatures with trivial bodies).
- `src/engine/timeline.rs` (new, stubs) and hook H1 in `engine.rs`.
- `src/ui/timeline.rs` (new, stubs) and the three `ui.rs` lines for `Action::Timeline`.
- `src/show.rs` (the fields in 3.1; `label`/`description` for the new actions; `paste_cues`/`duplicate`/`remove_list` remapping; `target_mut` untouched).
- `src/bulk_edit.rs` (`OWN_IN_KIND`).
- `src/prefs.rs` (`timeline_layout`, plus the test's literal).
- `src/dock.rs` (`Pane::Timeline`, title "Timeline", companion `Cues`, in `ALL`, **not** in `Layout::default`).
- `src/main.rs` (`mod timeline;`).
- `ui.rs::pane_ui` gets a `Pane::Timeline => timeline::pane(..)` arm.
- Fix the three `ControlCue { .. }` literals with `..Default::default()`.

**Must not** show the new control actions or the pane in any menu yet.

**Tests.**

- A 0.20.3 show file loads.
- Each new field at its default is absent from the JSON (show, prefs).
- Each new field round-trips.
- A group whose settings became default is written with no `timeline` key (`settle`).
- An editor-made timeline is written with both flags.
- Paste and duplicate remap `timeline`, `at` and `clips`.
- Removing a list clears keys that named it.
- `model`:
- `track_of` for every cue kind;
- `tracks` order;
- `clips` with a fake `Dur`;
- `key_of` for a child, a list cue and a plain cue;
- `resolve` for time, marker and cue;
- `lead_in` for each row of the table in 2.5, with the flag off and with `runs_unseen`;
- `clocked` with a pause cue inside, at the clip's start, at its end, and outside;
- `due_order`.
- An old `layout.json` loads.
- `testmode::feature` test still passes (`ALL` unchanged).

**Accept.** The three checks pass; the app runs headless and looks the same.

**Traps.** Show-file convention (default plus not-written plus test). `ControlAction::ALL` is asserted on by Test Mode. `src/timecode.rs` already exists, so do not confuse names. `MediaCue` derives `PartialEq` and is compared in `decodes_like`/`PrepSig`; check the new fields do not make a preload be thrown away needlessly.

### Wave 1 (three agents at a time, after P0 is merged)

#### P1 · Editing library · **M**

**Goal.** All of 3.3, pure, on `&mut Show`.

**Owns.** `src/timeline/edit.rs`, `src/timeline/snap.rs`. Nothing else.

**Rules to implement.**

- Move clamps the whole selection at 0.
- Track change only between tracks of the clip's kind.
- Trim per kind, as in 2.10:
- file clips: in/out through steady speed or the speed map, less any lead-in;
- held kinds (image, text, shape, camera, live sound): `end`;
- fade and wait: `duration`;
- no left-trim with keyframed speed;
- no trim on loops.
- Minimum length 0.05 s. Never invert `start`/`end`.
- Split:
- `Show::duplicate`-style new id;
- first half keeps the fade-in and loses the fade-out;
- second half gets a cut and no fade-in;
- keys of `rate`, `motion` and effects on the second half are shifted and re-anchored;
- refuses loops.
- Ripple delete closes the gap on the deleted clips' tracks only, and moves markers after the gap only if every track rippled.
- `sort_by_start` is stable and keeps a group's children directly after the group and a list's cues inside the list.
- `set_list_timeline(on)` refuses a list that holds groups.
- New timelines get `TimelineSettings::for_editor()`.
- `add_pause_before`: a free-run `TimelinePause` at the clip's start (`ClipIn`) or at `end - fade_out` (`ClipFadeOut`), named for the clip.
- Snap points include fade-out starts and the ends of lead-ins.

**Tests** (unit, fake `Dur`).

- Each op on a group timeline and on a list timeline.
- Clamping at 0.
- Trim maths at speed 2.0 and with a two-key ramp, compared to `SpeedMap`; and with a lead-in.
- Split keeps total length; keys shifted.
- Ripple with and without markers.
- `add_pause_before` lands on the exact time for both kinds.
- Snap: nearest point, tolerance, ignoring the moving clips' own edges, span snapping by either edge, snapping to a fade-out start.
- Undo-safety: every op leaves a show that survives `normalize_groups` and `normalize_lists` unchanged.

**Accept.** No engine or UI dependency (`cargo test timeline::` runs without media).

**Traps.** A `clamp` with inverted or NaN bounds from a show panics (see `ui.rs::within`); use `max`/`min` chains. Slide-show groups are not timelines (`relay_slides` re-times their children every frame): `is_timeline` must say no.

#### P2a · Engine: the run, pause cues, GO-timed transitions · **L**

**Goal.** 2.1, 2.2, 2.3, 2.4 and the transport rules of 2.8, for both key kinds, starting from the top only (no seek yet):

- the run clock and ordered starts with the tie rule;
- the four control actions and the routed old ones;
- both pause kinds, both freeze scopes, and clip freeze;
- GO rules;
- clocked clips: clock-driven fade-out and end, detached marking;
- `run_out` (Loop, Hold, End with `run_out_fade`);
- the preload horizon.

`TimelineJump` and `at` on Run are accepted but log "not yet" until P2b.

**Owns.** `src/engine.rs` (hooks H2 to H13, `StartAt`, the `PendingCue`/`ActiveCue` fields), `src/engine/timeline.rs`. Nothing else.

**Implementation notes.**

- `timeline_begin` at 0 must go through the fire queue. This keeps instance order, `active` order and the `together_for` behaviour exactly as now.
- Run loop per tick:

```text
target = pos + dt
repeat:
    take the earliest run-tagged pending with at <= target (ties by due_order, then show order)
    set pos = max(pos, at)
    start it
    stop if the run is no longer Running
else:
    pos = target
```

- `TimelinePause` from a cue on its own timeline sets `waits_for_go = true`.
- Freeze sets `hold` on the chosen clips. They are remembered by instance so the resume releases only those.
- A clip is marked `clocked` at start from `timeline::clocked(..)`, or later when it is frozen on its own.
- For a clocked clip, each tick while Running:
- fade-out starts when `pos >= ends_at - fade_out`;
- the clip ends at `ends_at`.
- While the clock is paused, neither happens.
- A clocked `Loop` clip is spawned looping (passes: none) with its end left to the clock.
- A list run has an instance number (for `together`) but no `ActiveCue`. A group run keeps its `ActiveKind::Group` cue, with `hold` while not Running.
- Use `Option<Instant>` / `checked_sub`, never `Instant::now() - d`.

**Tests** (`Engine::for_tests()`, with real media where marked).

- Every existing engine test passes unmodified. This is the regression gate.
- A Pause cue at T with a clip at T + 0.001 in the same big tick: the clip is not started; `pos == T`.
- A Pause cue at exactly a clip's start: the clip is not started until GO, then starts at once.
- Both pause kinds:
- free-run leaves a child's `elapsed` advancing while pending stays frozen;
- `Everything` holds it;
- `Clips` holds only the named ones.
- **The stage-manager case** (with media):
- a looping clip, a free-run Pause cue at the next clip's start, the next clip with a crossfade;
- wait several loop lengths: the loop is still playing and nothing else has started;
- GO: the next clip starts and the loop is faded over the crossfade time.
- A Pause cue at a clip's fade-out start: no fade while paused for longer than the clip's remaining content; after GO it fades over `fade_out` and ends.
- A Pause cue at a fade cue: the fade runs from the GO.
- `run_out`, with a 3 s clip paused 5 s:
- `Loop` is still playing with a pass number above 1;
- `Hold` is held;
- `End` has faded over `run_out_fade` (and over the clip's own fade-out when `None`, and cut at `Some(0.0)`);
- `Auto` is a held video or an ended sound.
- A clip frozen by a clip Pause cue ends by the clock, and a GO does not release it.
- A clip with no pause cue inside it ends by its content exactly as before.
- GO resumes:
- a group waiting at its own pause cue is resumed by `go()` before the playhead cue fires;
- `go_list` on another list does not resume it.
- Standalone list: GO runs, GO while running is a no-op, GO while paused resumes, GO at the end does nothing. Continues on clips are ignored. A clip fired alone ignores its `pre_wait`.
- Global pause then resume leaves a cue-paused run paused.
- `stop_all` gives `Stopped` with `pos` kept; `reset_all` gives 0.
- Control `Pause` on a timeline group now freezes its waiting children. This is the bug's regression test.
- Run/Pause/Stop of another timeline by key.
- Preload: a run with 30 waiting video clips prepares only those within the horizon.
- Re-fire cancels the earlier run's waiting children.

**Accept.** The above. `newq --self-test` still exits 0. No change in `testmode` expectations.

**Traps.**

- `together` is `None` for a child with `pre_wait > 0` on purpose.
- `is_running` is by cue id.
- `dt` is 0 on the first tick after idle and unclamped after a stall.
- `fire_one` un-pauses the global pause.
- `pass_len` with a keyframed speed is in file seconds.
- `play_on`/devamp must not be broken by clock-driven ends.
- `crossfade_into_next` is for Playlist groups only; do not reuse it.
- Slide shows stay on the old path.
- Say in the hand-back that sound was only tested with the fake device.

#### P2b · Engine: seek, parking, the opacity-0 rule · **L** (after P2a, same lineage)

**Goal.** 2.5, 2.6 and 2.7: the chase, start-from-offset, the cueing gate, parking with the picture, Jump and Run-from, and clips that wait for their fade-in.

Not in P2b (they are P5): overlap crossfades, gapless joins, preloaded jumps, smooth seek.

**Owns.** `src/engine.rs` (hooks H14, H15; `start_with`'s offset path; `prepare_from`'s `lead`), `src/engine/timeline.rs`, `src/timeline/chase.rs`.

**Implementation notes.**

- `start_with` with `StartAt.from > 0`, `lead > 0` or `chase`:
- skip `begin_dip`;
- take the shift-0 preload only when `from == lead` (content at its beginning);
- otherwise `prepare_from(cue, info, from, lead)`;
- set `elapsed = seek_base = from`;
- `duration` is the whole clip's, lead-in included (`cue_duration`), not the shifted `Prepared::duration`;
- a chase start enters as `Cut` with `together = run`; a lead-in start enters by the clip's own transition;
- levels from `Alive`.
- A start that has to wait for ffprobe keeps its `StartAt` in the `PendingCue`.
- Lead-in clips:
- with keyframes, pending at `at = start + V` with `StartAt { from: V, lead: V }`;
- with static opacity 0 (or a silent audio clip), pending with `waits_for_fade`;
- H14 starts a waiting clip when a fade cue that raises it fires, with `V = pos - start`, before the ramp loop runs.
- `Cueing`: release when every clip of the run is `started` or gone, or after `CUE_LIMIT`.
- Guard against more than 8 jumps in a tick.

**Tests.**

- Chase unit tests (pure): one for every row of the table in 2.6.
- With media:
- seek into a video: `elapsed` near `pos - start`, and the frame's pts matches;
- at speed 2 and with a keyframed ramp: file position equals `SpeedMap::media_at`;
- a looping clip;
- parked: a frame is in `Engine::layers`, voice paused, `elapsed` still; then run moves it;
- cueing releases two clips on the same tick.
- Jump to a time, a marker and a cue. A Jump cue to its own time does not spin.
- Seeking past a Pause cue leaves the run in the state it had.
- Opacity-0 rule (with media):
- opacity keys 0 until 4 s then rising: nothing is in `active` for the clip before 4 s; at 4 s the first frame's pts is the in point and `elapsed` is 4;
- the same with `runs_unseen`: started at the clip's start, and at 4 s the pts is 4 s in;
- the same on a group with no settings (an old show): started at the clip's start;
- static opacity 0 plus a fade cue at 6 s: started at 6 s from the in point, then fades up;
- static opacity 0 and no fade cue: never started, and the run still ends;
- a silent audio clip woken by a fade cue;
- with a keyframed speed: the file position at clip time `t` is `M(t) - M(V)`;
- the group's length includes the lead-in;
- seek into the lead-in: waiting and preloaded; seek into the visible part: content position counted from `V`.

**Accept.** The above, and every P2a test still passes.

**Traps.**

- `own_layers` off replaces everything; chase must mirror it.
- FFmpeg is spawned by reader threads; a failed start shows as `error()` with no frames.
- A camera can only be open once (`camera_in_use`).
- `Engine::tick` copies layer, geometry, output and hold time from the show into playing cues; do not fight it.
- A fade cue sets `faded`, which stops live level edits and the keyframed opacity. A clip woken by a fade keeps that behaviour.
- `tick` writes `opacity.value` from the keyframes only when not ramping; a lead-in start must not flash at the base opacity first.

#### P3 · Tracks editor and Option A · **L**

**Goal.** The one component, hosted as the dock pane (classic) and the Editor view's bottom panel (new interface), on group timelines.

**Tools in P3.**

- Ruler with adaptive ticks (reuse `timeline_step`).
- Zoom: Ctrl/Cmd + wheel, toolbar buttons, fit.
- Horizontal and vertical scroll.
- Track headers.
- Clips drawn with:
- name, colour, armed state, open-ended arrow;
- loop and speed badges;
- fade-in and fade-out handles;
- the lead-in hatched ("waiting to fade in"), and "waits for a fade cue" when nothing raises it;
- the detached hatch ("free-running").
- Click, Ctrl-click and Shift-click selection through the existing selection (`Action::Select`, `ToggleSelect`).
- Move one clip or the selection in time with snap (clip edges, fade-out starts, markers, playhead; Alt disables).
- Trim both edges.
- Markers drawn, added from the toolbar at the playhead, dragged and renamed; "Add Pause cue here".
- Clip menu: "Pause before this clip comes in", "Pause before this clip fades out", "Freeze clip / Resume clip".
- Playhead from `engine.timeline(..)`.
- Click or drag on the ruler moves a ghost playhead and emits `Seek` on release.
- Transport in the toolbar: Run, Pause, Stop, time read-out, next pause, and "GO: …" from `RunView::on_go` while paused.
- Control cue inspector fields: action list `ALL` + `TIMELINE`, timeline picker, point picker, freeze kind (free-run / everything / these clips), clip list.
- Clip inspector rows (`clip_fields`): `run_out`, its fade time when End, and "Already running when it fades in".
- Timeline inspector rows (`settings_fields`): the two flags.
- "Open in timeline editor" on a timeline group; double-click a timeline group row.
- Window menu item for the pane.
- `Prefs::timeline_layout` stored; the Workspace value falls back to Pane until P7.

**Owns.**

- `src/ui/timeline.rs` and `src/ui/timeline/{tracks,ruler,toolbar,control}.rs`.
- `src/ui.rs`, only:
- the `apply` arm;
- the inspector's Control arm (call `timeline::control_fields`);
- the cue row's Control description (call `describe_control`);
- one call to `clip_fields` in the media inspector;
- one call to `settings_fields` and the "Open" button in the group's inspector;
- the `menu_bar` Window item;
- the `UiState::timeline` field.
- `src/ui/modern.rs`: the `editor()` bottom panel only.
- `src/dock.rs`: `Layout::open` special case that splits `Pane::Timeline` below the Cues node.

**Leaves alone.** The old `timeline_panel` in the inspector; I1 decides its fate.

**Tests** (UI style of `ListMouse`/`drag_in_patch`, using `TimelineRects`).

- The pane draws for a group timeline, and the empty state for none.
- Dragging a clip emits exactly one `MoveClips` on release with the snapped `dt`.
- Alt-drag does not snap.
- Dragging an edge emits `Trim`.
- A ruler drag emits no action while moving and one `Seek` on release.
- Ctrl+wheel changes `pps` and keeps the time under the pointer.
- Clicking a clip selects it (the inspector follows).
- The clip menu's "Pause before this clip comes in" emits `AddPauseBefore`.
- A clip with a lead-in draws a longer bar than its content.
- Show mode: edit drags emit nothing; transport still works.
- Control inspector: choosing `TimelinePause` shows the freeze choice and hides the cue picker; "these clips" shows the clip list.
- The pane opens below the cue list and pops out (`draw_popout`).
- The new interface's Editor view draws with the panel.

**Accept.** The above, plus a headless run under xvfb with a screenshot of the pane on a timeline group (see CLAUDE.md "Running the app headless").

**Traps.**

- Selection: `multi` only counts while `selected == multi_for`. Go through the actions, never write the fields.
- Letters and digits are cue hotkeys; editor shortcuts need a modifier and must be consumed before `keyboard()`'s typing check.
- `apply` early-returns in show mode and under `test_locked` via `edits_the_show`.
- A pop-out has its own egui context, so keep no state in `ctx.data` that the main window needs.
- `spread_edit` only wraps `inspector`; the new fields spread like any other cue field.
- egui ids must include `Host`.
- `clamp` panics on inverted bounds.

#### I1 · Integration 1 (one agent, after P1, P2a, P2b and P3 merge) · **S**

- Merge order: P1, P2a+P2b, P3.
- Wire-up fixes.
- End-to-end tests in `ui.rs` tests:
- drag a clip, the show changes, undo restores it in one step;
- release on the ruler, the engine is parked with a layer up;
- a Run action runs it;
- "Pause before this clip comes in", then run, then GO: the clip comes in on the GO.
- Decide the old inspector `timeline_panel`: recommend keeping it read-only with the "Open in timeline editor" button.
- README section and keyboard-table rows.
- CLAUDE.md layout entry.
- Headless run with screenshots of both interfaces.

**This is the minimum shippable release (0.21.0): "Timeline cues".** It gives:

- a timeline group in a cue list with the tracks pane in both interfaces;
- move, trim, snap, zoom, markers;
- seek and the parked picture;
- Run/Pause/Stop/Jump control cues with both pause kinds and both freeze scopes, and clip freeze;
- transitions timed to the GO after a free-run pause, with loop / hold / end-with-fade;
- clips at opacity 0 waiting for their fade-in;
- GO resuming a paused timeline cue.

Known limits to state in the release notes:

- no automatic crossfade of overlapping clips (use the next clip's crossfade transition);
- abutting clips can flash;
- no standalone tab;
- no split, ripple or speed lane (speed keyframes are edited in the inspector as now).

### Wave 2 (three agents in parallel)

#### P4 · Standalone Timeline tab · **M**

**Goal.** A cue list of type Timeline: made from the tab bar's "+" menu and the Cue menu, or converted (a list holding groups is refused).

- Its tab shows the tracks editor in place of rows (`Host::ListTab`).
- GO, Pause and Stop in both transports drive it, with the "RUNNING · next pause in m:ss", "GO resumes …" and "GO: …" read-outs.
- `apply(Action::Go)` keeps the selection when the list is a timeline.
- Files dropped on the window while a timeline tab is active become clips at the playhead on a fitting track.
- `MoveToList` refuses groups into a timeline list.
- The offer to turn on `own_layers` is shown, never applied silently.

**Owns.**

- `src/ui.rs`: `cue_list_tabs`, the top of `cue_list`, `transport`, the `Action::Go`/`AddFiles`/`MoveToList` arms, the `menu_bar` Cue menu.
- `src/ui/modern.rs`: `strip`, `operator`.
- `src/ui/timeline/list_tab.rs` (new).

**Not** `src/ui/timeline/tracks.rs` (P6's) or `engine.rs`.

**Tests.**

- Tab draws tracks.
- GO on a timeline list sends the engine running.
- The read-outs.
- Convert refusal.
- Dropped files become clips.
- Show mode locks edits.
- Operator view shows the timeline state.

**Traps.** `engine.playhead` is meaningless for a timeline list (`next_cue` in modern.rs assumes it). `restore` (undo) and `sync_lists` paths. `GoList(n)` is by tab position.

#### P5 · Engine II: crossfades, joins, jumps · **M/L**

**Goal.**

- 2.9: overlap crossfades when `crossfade_overlaps`, for picture and sound.
- Gapless abutting clips.
- Preloaded Jump targets (`Prepared::shift`, matched in `take_prepared`).
- Smooth seek: reuse `Seek` for clips alive on both sides; keep the old set until the new one is cued.
- A clip that becomes detached at run time with `run_out == Loop` restarts from its in point (preloaded as its end nears) instead of falling back to Hold.
- Optional: freeze a camera's last frame (a `VideoPlayer::set_frozen` that keeps draining).

**Owns.** `src/engine.rs`, `src/engine/timeline.rs`, `src/timeline/chase.rs`. If the camera freeze is done, `src/media/video.rs` (`update`/`set_frozen` only).

**Tests.**

- Overlap of 1 s on one track: both layers present with opacities crossing, and no own fade-out on the first. With the flag off, behaviour is as today.
- Overlap crossfade after a free-run pause: it runs from the GO.
- Abutting pictures: the layer stack is never empty across the join.
- A Jump cue within the horizon has its chase set prepared, and the jump's first frame arrives within a tick or two.
- A looping clip with keyframed speed under `run_out` Loop.
- A clip frozen by the operator with `run_out` Loop goes round again.

**Traps.** `started_early`/`extra_hold` is the existing pattern for holding a picture longer. `crossfade_into_next` is for Playlist groups only.

#### P6 · Editor tools II and the speed lane · **L**

**Goal.** The rest of the tool list.

- Drag between tracks, with a ghost and only onto fitting tracks.
- Multi-select move across tracks.
- Rubber-band selection.
- Split at the playhead (Ctrl/Cmd+K), with the audio-join warning.
- Ripple delete (Shift+Delete, and the toolbar's ripple toggle).
- Add or remove a track; reorder tracks.
- Add control, pause, fade or send cues from a "+" on the cue track at the playhead.
- Marker menu: rename, delete, "jump here".
- Dragging a clip's fade handles sets `fade_in`/`fade_out`.
- The **speed lane**: a clip-width keyframe line built on `keyframe_lane`'s logic, emitting `SetRate`. The clip's bar length updates as the keys move. A read-out shows speed at the playhead and the resulting length.
- An opacity lane on the same pattern, so a lead-in can be drawn where the clip is.
- The warnings of 2.13 (a waiting clip with Cut over another clip; a clip that nothing fades up).
- Waveform (from `Engine::waveforms`) on sound clips at `Host::Workspace` heights.

**Owns.** `src/ui/timeline/tracks.rs`, `src/ui/timeline/toolbar.rs`, `src/ui/timeline/lanes.rs` (new), `src/ui/timeline.rs` (apply arms for the remaining actions), and `src/timeline/edit.rs` for bug fixes only.

**Not** `ui.rs`.

**Tests.**

- Each tool by mouse through `TimelineRects`: a track drag emits `MoveClips { to: Some(..) }`; split and ripple emit their actions and change the show.
- Speed lane:
- a double-click adds a key;
- a drag moves it;
- the clip's rect gets shorter when speed rises;
- removing the last key leaves a steady speed.
- Opacity lane: moving the first rising key moves the end of the hatched lead-in.
- Shortcuts are consumed and do not fire a cue hotkey.

**Traps.** `keyframe_lane` takes `&mut Animated` and allocates the full width: work on a copy inside a child `Ui` at the clip's rect. `Animated::sanitize`. `MIN_RATE`/`MAX_RATE`. The editor takes `&Show`.

#### I2 · Integration 2 · **S**

Merge order: P5, P6, P4. Cross tests (standalone tab plus the tools plus detached and waiting marks), README, headless screenshots. Release 0.22.0, "Timeline tab".

### Wave 3 (two or three agents in parallel)

#### P7 · Option B workspace and the layout setting · **L**

**Goal.** `ui/timeline_workspace.rs::draw(ui, show, engine, st, actions)`, made of:

- **Bin** (left): the timeline's media, with "+ Text · Shape · Camera · Fade · Send · Pause". Items drag onto tracks, with a payload like `DraggedCues`.
- **Program monitor**: `output_previews`.
- **Inspector**: the existing `inspector`.
- **Tracks** across the full width: `editor(.., Host::Workspace)`.

Hosting:

- New interface: `View::Timeline` in the strip.
- Classic: replaces `dock_area` while `st.timeline.open` and the layout is Workspace, with a breadcrumb back.

Also the setting in Settings ▸ General and the toolbar toggle (`SetLayout`). All four cells of the matrix in 2.11 work.

**Owns.**

- `src/ui/timeline_workspace.rs` (new).
- `src/ui/timeline/bin.rs` (new).
- `src/ui/modern.rs`: `View`, `draw`.
- `src/ui/settings.rs`: one row.
- `src/ui.rs`: only the `draw()` branch and the `menu_bar` Window items.

**Tests.**

- Each of the four combinations draws the editor exactly once.
- Switching the setting moves it.
- A bin drag emits `AddClip` with the track and time under the pointer.
- The setting is saved in prefs and absent at its default.
- A pop-out of the pane plus the Workspace layout does not draw two editors with one id.

**Traps.** The settings page takes the view's place while open. `modern_view` is not saved. The test in modern.rs draws every `View::ALL`.

#### P8 · Remote, feedback, export, Test Mode · **M**

**Goal.**

- OSC `/timeline/<list name|cue number>/{run,pause,stop,jump <seconds|marker>}`.
- The web remote's GO and state for a timeline list.
- OSC feedback of a run's state and position.
- Cue sheet export with a start-time column for timeline lists and markers listed.
- Test Mode's Debug test gains a timeline part covering the four new actions, both pause kinds, a GO-timed crossfade, a clip waiting for its fade-in, and a seek. Only then are the four actions folded into `ControlAction::ALL`.
- `--self-test` covers it too.

**Owns.** `src/remote.rs`, `src/web.rs`, `web_page.html`, `src/feedback.rs`, `src/export.rs`, `src/testmode/feature.rs`, `src/testmode/full.rs`, `src/ui.rs` (only the remote command handler around `control_target(.., "Remote")`), and `src/show.rs` (only the `ALL` constant, last).

**Tests.** The parsers (pure, as the existing ones). Feedback `snapshot`/`changes`. Export text. `feature::judge` on made-up notes for the new part. The self-test passes on CI's three systems.

**Traps.** `feature.rs` asserts on every action. The test show copy has `own_layers` on, no streams and no feedback. A part's later fires are loaded ahead (`Engine::load_ahead`).

#### I3 · Integration 3 and release · **S**

Merge, README (both layouts, the remote commands), CLAUDE.md, screenshots of the four layout combinations, release 0.23.0 through the release skill.

---

## 5. Minimum shippable first wave

Wave 0 + Wave 1 + I1 (packages P0, P1, P2a, P2b, P3, I1), about **M + M + L + L + L + S**. It works end to end for use (a), includes the stage-manager case and the opacity-0 default, and has every engine capability the standalone tab needs. Waves 2 and 3 add reach, not foundations.

Why the opacity-0 rule and the GO-timed transitions are in the first release, not later: both set what a *new* timeline does by default. Shipping editor-made timelines first and changing their defaults a release later would leave shows that behave differently depending on the version that made them.

Smaller cuts are not releasable: Wave 0 alone is invisible, and the editor without the engine chain would be "stage 1 on today's engine", which cannot honour a pause cue at its exact time.

Wave 1's length is set by the engine chain (two L packages in a row). P1 and P3 finish sooner; the third slot is free for an early start on P4's design notes only, not code, since P4 owns `ui.rs` regions P3 is editing.

## 6. Size summary

| Package | Wave | Size |
|---|---|---|
| P0 Skeleton and data model | 0 | M |
| P1 Editing library | 1 | M |
| P2a Engine: run, pause cues, GO-timed transitions | 1 | L |
| P2b Engine: seek, parking, opacity-0 rule | 1 (after P2a) | L |
| P3 Tracks editor, Option A | 1 | L |
| I1 Integration, release 1 | 1 | S |
| P4 Standalone tab | 2 | M |
| P5 Engine II | 2 | M/L |
| P6 Tools II, speed and opacity lanes | 2 | L |
| I2 Integration, release 2 | 2 | S |
| P7 Option B, layout setting | 3 | L |
| P8 Remote, feedback, export, Test Mode | 3 | M |
| I3 Integration, release 3 | 3 | S |

## 7. Decisions for the owner before building

Already settled by the owner and built into this plan: both pause kinds; both freeze scopes; loop / hold / end with a fade on End; transitions timed to the GO; the opacity-0 default with a per-clip opt-out.

Still open:

1. A picture track is a (display, layer) pair, not a layer alone (2.9).
2. GO resumes a timeline cue waiting at its own pause cue before firing the next cue (2.8).
3. GO does nothing while a standalone timeline is running; it stays at its end when finished (2.8).
4. The parked picture goes to the real outputs; rulers of timelines that are not the active list are locked in show mode (2.7).
5. `run_out` default is: video holds its last frame, sound ends with its own fade-out (2.4).
6. In the opacity-0 rule, the cue's own fade-in transition never defers the start; only opacity keyframes at 0 or a static opacity of 0 do. A video's sound starts with its picture (2.5).
7. A waiting clip enters by its own transition when it becomes visible, so a Cut clears its layer then (2.5, 2.13).
8. Cameras and live sound stay live in "pause on a frame" in the first version (2.2).
9. The main list cannot be a timeline in the first version (2.12).
10. Shows using the new control actions do not open in 0.20.3 and earlier (2.12).

### Critical Files for Implementation

- /home/user/NewQ/src/engine.rs
- /home/user/NewQ/src/show.rs
- /home/user/NewQ/src/ui.rs
- /home/user/NewQ/src/ui/modern.rs
- /home/user/NewQ/src/dock.rs