# The show and the engine

Covers `src/show.rs`, `src/engine.rs`, `src/speed.rs`, `src/fx.rs`, `src/live_text.rs`, `src/shape.rs`, `src/text.rs`.

Moved word for word from the Layout section of `CLAUDE.md` (commit 2589061). Read this before
working in these files, and keep it true when the code changes. A heading that says "Part of"
is a piece of a longer entry that covered several areas.

See also: `remote.md` holds the one line on `src/text.rs` (text cue rendering), in the `src/remote.rs` entry.

- `src/show.rs`: the show data model and `.newq` (de)serialisation, plus the
  group, arm, cue-list and audio-output structure helpers. Cue lists: `Cue::list`
  (0 = main) and `Show::lists`; the cues of a list are kept together in
  `Show::cues`, the main list first (`normalize_lists` repairs a loaded show), so
  `next_top`, `move_cue` and `insert_slot` stay inside a list. The engine's
  `playhead` is the active list's; the other lists' are in `list_playheads`.
- `src/show/compat.rs`: a show from a newer NewQ (R4 and R10 of the 2026-11-26 reviews). Nothing
  in a show stops it loading: a cue whose kind is unknown, or whose kind's settings can't be read
  (an unknown `ControlAction`, `TimelinePoint` or `GroupMode` included), is `CueKind::Unknown`
  (an untagged `Value`, the kind as written, `CueType::Unknown`); `Engine::fire` turns it away
  before the network gate and `start_with` before anything starts, so it is never played or sent;
  `bulk_edit` leaves its kind alone; `normalize_groups` keeps its children its own; the cue list
  and export show `CueKind::unknown_note`. `Transition`, `Fit`, `ContinueMode`, `EffectKind` and
  `CueColor` have an untagged `Other(Kept)`: `Kept` is an interned `&'static Value`, so they stay
  `Copy`/`Eq`/`Hash` and write the value back; they act as `FadeIn`, `Contain`,
  `DoNotContinue`, nothing (neither `is_video` nor `is_sound`) and grey. `Cue::extra` and
  `Show::extra` (`#[serde(flatten)]`) keep unknown fields; fields deeper in a known cue (a
  `MediaCue`'s, an output's) are still dropped. `compat::lenient`/`lenient_vec` read as the
  default the few kinds outside a cue that aren't kept (`wait_cards`, an output's `stream`, a
  cue's `timecode`/`clock`, a timeline's `tracks`). `Show::save` writes `saved_with` (this
  NewQ's version) into every file; in memory it is as read and written only when set, so the
  network hash, undo and the dirty check don't see it. `ui/newer_file.rs`: a file saved by a newer
  NewQ (`compat::newer_than_this`) gets a strip over the window and its path is kept from a plain
  Save (`save_target`) until the show is saved anywhere (`saved`); the recovery copy is beside the
  file, and the protection is in `UiState` (by path), so undo and a recovery restore keep it.
  `net::play::read_show` is the follower's parse (`take_show`, `play_load_kept`).
- `src/engine.rs`: GO, pre-waits, continues, transitions, fades, groups, preload.
  `Engine::fire` takes the cues one firing leads to (continues, a group's children, Start
  cues) from `fire_queue`, never nested; the outermost call counts what it starts
  (`FIRE_TIMES_PER_CUE` starts of one cue, `FIRE_TOTAL_MIN` or twice the show's cues in all,
  R1): past that it clears the queue and says which cue came round, so cues that take no time
  and lead back to each other can't spin the main thread. Every path that starts a cue from
  inside another (control actions, `fire_next`, groups, `go`, `deck_step`, hotkeys, remote and
  network commands) reaches the queue through `fire`, so the count sees it; a timeline's clips
  are started by its run (`timeline_step`, `MAX_STARTS`, `MAX_JUMPS`).

## Part of the `src/app.rs` / `src/ui.rs` entry: group crossfade, shape cues, editing several cues

  Group crossfade: in a "play in order" group a child whose next sibling crossfades in (`crossfade_into_next`) is not faded out by itself, and `advance_groups` starts the next one that long before it ends; a held next cue (`holds`: picture, text, shape, camera) gets the overlap added to its hold (`Engine::started_early` -> `ActiveCue::extra_hold`, which the per-tick hold update adds back), so the group keeps its timing; a video can't, so `cue_duration` subtracts a crossfade into a video, and a next child with a pre-wait doesn't overlap. Shape cues: `show::ShapeCue` (`ShapeKind`), drawn by `shape::render` at the display's size on a thread (`Engine::prepare_shape`, which hands the picture over as `PreparedKind::Text`, like a text cue). Editing several cues: `inspector` wraps `inspector_one` and `spread_edit` makes in the other picked cues what changed (`bulk_edit::spread` compares the cue as JSON before and after, skipping the fields that are a cue's own, `target` and `device` included). A list missing from the file is empty, so a list that appears or empties is a length change and isn't spread; an item with a `kind` (an effect) is addressed as `<index>#<kind>` and changes only the same kind; `start`/`end` reach only the same cue type (`SAME_KIND_ONLY`); `inspector` spreads only on a frame with user input. `Engine::tick` copies the show's layer, geometry, colour, effects, fit, output and hold time into the cues playing (the block under `if let Some(m) = media`), and a shape or typed text is drawn again when `drawing_key` changes (`ImageState::redraw`); what is fixed at start (file, in/out, loop, speed, audio routing, fade-in) is not.

## Part of the `src/app.rs` / `src/ui.rs` entry: layers, copy and paste

  Layers: `MediaCue::layer` (higher on top; `Engine::layers` sorts by it); with `Show::own_layers` (new shows; a file without it, from 0.18.0 and before, has it off and replaces everything on the display as it did) `apply_entry` replaces or fades only the cues on the entering cue's own layer and display (`layer_no`, `ActiveCue::own_layer`), a dip's matte replaces all. The children a "start all together" group fires share the display (`together`): the group records them in `Engine::together_for`, read at each start attempt (a start can wait for its file and come back) and removed when the start ends, so a child fired on its own later replaces as usual. Copy and paste: `UiState::clipboard`, `Show::copy_cues` and `Show::paste_cues` (new ids, display/audio output mapped to the pasting show's); Ctrl+C/X/V are `Event::Copy/Cut/Paste`, handled in `keyboard` after the typing check.

- `src/speed.rs`: keyframed playback speed. `SpeedMap` integrates a cue's speed
  (`MediaCue::rate`, an `Animated`) into file time and back; `Stretcher` is a
  WSOLA time stretcher the audio reader runs along a `SpeedPlan`; the video
  reader (`video.rs::read_at_speed`) picks frames by the same plan. A steady
  speed still uses FFmpeg's `atempo`/`setpts`. Loops follow the plan too: the readers
  see the passes as one run of frames, and `ActiveCue::pass_len` is then in the
  file's seconds (`pass_time` maps the cue's time into them).
- Animated geometry: `show::Motion` (six optional `Animated` tracks on
  `MediaCue::motion`, evaluated in `Engine::tick`); a fade cue's `geometry` ramps an
  `ActiveCue::live_transform` (`LiveTransform`) that then wins over the tracks.
  Fade cue `matrix` targets set `VoiceShared::xp_target/xp_rate` (up to
  `audio::MAX_FADEABLE` crosspoints, kept in `Matrix::keys` order); `mix_voice`
  moves the gains per buffer. Silent crosspoints are kept in the matrix for this.

- `src/fx.rs`: cue effects. `show::Effect` (kind and up to four `Animated`
  numbers, each a value or keyframes) is on `MediaCue::effects`. `fx::VideoFx::at`
  turns the picture effects into the numbers in the compositor shader (blur,
  sharpen, pixelate, negative, vignette in `gpu.rs`, one pass, a test renders them);
  the sound effects (`AudioChain`: biquad EQ/filter, compressor, echo, freeverb)
  are built when a voice is, kept in step with the show by `FxShared` atomics
  that `Engine::tick` sets from the cue's time, and run per frame in
  `audio.rs::mix_voice`. `render.rs::FrameTimer` and the mixer callback's load
  feed the status-bar warnings.

## Part of the `src/remote.rs` entry (a run of other files' notes): clock and timer text

  `src/live_text.rs`: clock and speaker timer text (the engine redraws the
  picture as the shown time changes; the renderer re-uploads on `version`).
