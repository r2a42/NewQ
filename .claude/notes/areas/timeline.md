# Timelines

Covers `src/timeline/`, `src/engine/timeline.rs`, `src/engine/displays.rs`, `src/ui/timeline.rs`, `src/ui/timeline/`.

Moved word for word from the Layout section of `CLAUDE.md` (commit 2589061). Read this before
working in these files, and keep it true when the code changes. A heading that says "Part of"
is a piece of a longer entry that covered several areas.

- Timelines (plan and rulings: `.claude/notes/timeline-plan.md` and `timeline-plan-delta-1.md`, later
  sections win; section U is I2's). A timeline is a "start all together" group that is not a slide show
  (`TimelineKey::Group`), or a list with `CueList::timeline` (`List(n)`, never list 0). `src/timeline/`:
  `mod.rs` the file's types (`TimelineSettings` with `start_on_fade_in`/`crossfade_overlaps`, both on in
  `for_editor()`, off for an old group; `Marker`, `TimelinePoint`, `RunOut`, `PauseFreeze`, `TrackId`;
  all `#[serde(default)]`, unwritten at their default, so old shows are unchanged; the four
  `ControlAction::Timeline*` variants are what 0.20.3 can't read); `model.rs` the pure read model
  (`clips` with each clip's lead-in and `clocked` (a free-run pause or its own Pause inside it; a loop
  pause doesn't count), `tracks`, `ghosts`, `cuts_over`, `lead_in`, `fade_wake`, `key_of`; `members` is
  structural only; `Dur` is the engine's `cue_duration`, and only its Group arm adds the lead-in);
  `edit.rs` pure edits that say what they changed (move, trim, split, ripple delete, add clip/control/
  pause-before/track/marker, `set_fade`, `set_opacity`, `move_track` (swaps the layers of two picture
  tracks), `place_at`, `paste_clips`, `duplicate_clips`; `split` leaves fade cues on the first half and
  makes the second visible, `starts_visible`; a move onto a display in `also_on` takes it out of the
  list; `add_clip` gives a new still, text, shape, clock or timer `STILL_LEN`, wherever it is made); `snap.rs`; `chase.rs` works out what plays at a time (seek, parking, Jump; `overlap` and
  `crossed_by` for overlap crossfades, per display), pure, its own tests.
  `src/engine/timeline.rs`: a `Run` per timeline in `Runs.map` (clock `pos`, `RunState`
  Stopped/Cueing/Running/Paused, `RunView` for the UI, `pub use`d by `engine.rs`); waiting clips are
  `PendingCue`s tagged with the run; at equal times control cues go first (`model::due_order`), so a
  pause stops the clock before anything after it; a clocked clip's end and fade-out come from the clock
  (`ActiveCue::ends_at`, `run_out` when its content runs out first; a detached clip set to Loop goes
  round, `timeline_go_round`); a seek cues for up to `CUE_LIMIT` while running, else parks (paused,
  picture on the outputs; a seek to exactly 0 parks with nothing up, so the editor seeks to
  `engine::FIRST_FRAME`, as a remote's jump does; `Run::parked`, `RunView.parked`); the picture changes all at once
  (`Run::outgoing`, `timeline_unseen`, `timeline_settle`; clips alive on both sides are jumped in place,
  `timeline_carry`); jumps are got ready ahead (`timeline_jump_wanted`, `Prepared::fits`); overlaps
  crossfade (`timeline_overlap`, `ActiveCue::crossed_by`) and abutting pictures are held over the join
  (`timeline_joins`, `JOIN_GAP`, `ActiveCue::joined`); a loop pause (`ControlCue::loop_from`,
  `leave_at_once`, `is_loop()`) is `Run::looping` (`timeline_loop`, `timeline_leave_loop`);
  jumps are `MAX_JUMPS` a frame (`may_jump`), a step starts `MAX_STARTS` at most, and a list's start
  from the top nests inside itself (clips at 0 that stop and run it again) at most
  `FIRE_TIMES_PER_CUE` deep (`Runs::list_starts`, a depth, not a count per frame);
  `timeline_waiting`/`timeline_detached` feed the editor's hatching. The active panels show a
  clocked clip set to Hold or Loop on the clock (`ActiveCue::panel_times`); one set to End by its own time. A clip's keys read at
  `ActiveCue::key_time()` (its run's clock less `clip_zero`; `elapsed` off a timeline). Ramps a run
  starts are tagged (`Ramp.run`, `Param::follow`) and move only as far as its clock does
  (`Runs::steps`); a run's fade cue keeps its clock's time (`fade_from`), and its matrix crosspoints are
  held in the mixer while the clock stands (`timeline_hold_crosspoints` -> `Voice::hold_crosspoints`,
  an atomic); untagged (by hand, off the timeline, a Stop's fade) move with show time. The Stop cue
  fades the run out over `ControlCue::fade` (`stop_fade()`: None = `TIMELINE_STOP_FADE`, 3 s; 0
  cuts; a second Stop cuts; `RunView.fading` meanwhile, "Stopping · fading out"). What GO does is
  `Engine::go_plan` (`GoPlan`, `TimelineGo`: Start, Resume, Play, Running, Cueing, LeaveLoop, AtEnd (a
  list stopped at its end, `Run::at_end`, until a clip is after the playhead); `go` and `timeline_go`
  follow the same `timeline_go_for`), which the transports, the new interface and the web remote's `go`
  read out (`GoPlan::describe`, words from `model::on_go_text`). Tests in `engine/timeline/tests.rs`
  play real media (`color_red.png`, the test-pattern camera); live sound under a freezing pause is
  untested (no device).
  `src/engine/displays.rs`: a picture on several displays (`MediaCue::also_on`; read only through
  `MediaCue::displays(show)`, the home first). `ActiveCue::screens` holds per display `{ output, exit,
  gone }`, kept in step with the show each tick (`displays::sync`); `Engine::layers(output)` uses
  `level_on(output)`; `apply_entry` replaces or crossfades a cue only on the entering cue's displays
  (`cut_from`, `crossfade_from`), so it stays on the others; gone from all, it ends
  (`gone_everywhere`); its one voice plays until the last full display goes (`leave_sound`). Fades, Stop
  cues and a timeline Stop act on the whole cue. The decode size is the home's or the largest open window
  among `also_on` (`picture_size`); text and shapes are drawn at the home's size. Tests in
  `engine/displays/tests.rs` (text cues, no media); no real displays tried.
  `src/ui/timeline.rs` and `src/ui/timeline/`: the tracks editor (`editor`, hosted as `Pane::Timeline`
  in the dock, as a bottom panel of the new interface's Editor view, as a timeline list's tab, and as
  the workspace; `Host` salts its ids); it takes `&Show` and asks for `TimelineAction`s that `apply` makes after the
  frame (edits refused in show mode and tests by `edits_the_show`); a drag lives in `EditorState` and is
  asked for once on release (one undo step). `tracks.rs` clips, rows, drags, ghosts, the overlap's
  crossfade mark, a loop pause's section (`loop_sections`, shaded on the Cues track and the ruler) and the
  vertical-overflow bar (a lane just opened is scrolled into view,
  `EditorState::reveal`; fade handles only on the hovered or picked clip; a moving run's playhead is
  kept in view by `tracks::follow` (`Prefs::timeline_follow`: `Page`, or `Centre`, which overrides a user
  scroll; `TimelineAction::SetFollow`, Settings ▸ Interface), not while a drag or the scroll bar is held; the Cues
  track's no-time cues are `tracks::chips`, narrowed to the next, those nearer than `MIN_CHIP` one chip
  whose click picks each in turn, names `fitted` to the next and drawn whole on top when hovered or
  picked, `cue_labels`; the ruler's flags are cut to the next the same way); `ruler.rs` ticks, markers (grouped
  into one "×N" flag by the Cues track's `tracks::chips`, generic over the id; a click picks each in
  turn, `EditorState::marker_pick`),
  the playhead (a ruler tap or drag seeks on release; a drag is cancelled when the run's
  `RunView::mark` changes meanwhile, `Drag::Playhead { pos: None }`; locked in show mode for a group); `toolbar.rs`
  transport and read-outs (`state_text` from `RunState` and `RunView.parked`; two rows under
  `ONE_ROW_W`; the state text comes last on its row so the buttons don't move); `control.rs` the
  inspector rows (`inspector_rows` at the end of the inspector, `settings_fields`); `menus.rs` the clip,
  track and cue-track "+" menus; `lanes.rs` the speed and opacity keyframe lanes under a clip
  (`EditorState::lanes`; a key drag is drawn from a copy of the show with the keys in, `lanes::preview`,
  so the bar's length and lead-in follow; both on the timeline's time axis, speed keys following the
  content and opacity keys the clock, ruling L2); `keys.rs` Ctrl/Cmd+K (split at the playhead), dropped
  files and the new-clip file dialog. `ui::keyboard` runs before anything is drawn, so the editor that
  holds the keys (`EditorState::focus`, last clicked, or hovered) swaps the `Action::Delete`, `Cut` and
  `Duplicate` it already made for its own (`TimelineAction::Delete`/`Cut`/`Duplicate`) when every picked
  cue is one of its clips, and `Paste` for `TimelineAction::Paste` (at the playhead, `paste_into`) when
  the clipboard holds no group and nothing outside the timeline is picked; Space is `Action::Space` (the
  hovered editor's timeline), `ui::space_key`: `timeline::space_target` (the focused editor still showing
  its timeline, `EditorState::drawn`, else the hovered one, else `list_tab::active`) gets
  `space_action` (Pause while running outside a loop or cueing, else `TimelineAction::Go` =
  `timeline_go_one`), none is GO; `0` is GO (`ui::go_key`; output windows too); and an `Action::AddFiles` for
  `AddClip`s when the files were let go over a track (exactly one handles a drop). A clip on several
  displays has a "+N" badge and ghosts. Double-clicking a timeline group's row opens it rather than
  firing it. `list_tab.rs`: a timeline list's tab: `cue_list` calls `list_tab::draw` after the tabs, which
  draws the editor (`Host::ListTab`, it doesn't pin) in place of the rows unless
  `EditorState::list_rows` (set on going to the tab while the classic Timeline pane is open, so the pane
  shows the tracks); the pane otherwise says the tracks are in the tab. `add_cue`/`AddFiles` on the active
  timeline list go to `list_tab::add_clips` (at the run's `pos`, in turn where lengths are known,
  `fitting_track`); a paste there or a `MoveToList` into a timeline
  list lands at its playhead (`edit::place_at`); groups are refused there (`NO_GROUPS`: MoveToList,
  GroupCue, Paste, slides, `NewTimelineGroup`). `ui::go_readout` is `list_tab::go_readout` (`go_plan` plus
  the run's next pause or loop); GO on a timeline list keeps the selection; `modern::next_cue` is None
  there and the Operator view adds `operator_state`. Tabs: `+` menu and the tab's right-click
  (`SetListTimeline`, which then `sync_lists`). Layout B, `workspace.rs` (`Host::Workspace`): chosen by
  `Prefs::timeline_layout` (`workspace::chosen`; `TimelineAction::SetLayout` saves it and moves an open
  editor; reached from Settings ▸ Interface (its first section, two rows: the classic window's settings are
  short while a Timeline pane is open), Window ▸ Timeline opens (the follow mode only on the Interface page) and
  the toolbar's `Workspace`); classic `ui::draw` draws it in `dock_area`'s place while `st.timeline.open` and no
  docked Settings (`replaces_panes`); the new interface's `View::Timeline` (in the strip only for B;
  leaving it is `Close`); `Open` goes to it (and `SelectList` into a timeline list does), the tab then
  keeps its rows (`shows_tracks` false) and its Tracks button opens it; a Timeline pane under B only
  points to it (`workspace::pointer`). `workspace::showing` (the inspector's "tracks are open" instead
  of its Open button) counts a classic Timeline pane only in front of its tabs (`dock::Layout::in_front`,
  read through `EditorState::pane_in_front` while the dock is drawn). Bar (Back = `Close`, `Show as a pane`), tracks across the
  bottom (`TRACKS_SHARE`), left column the monitor (its own paint of `PreviewOutput`, a resizable top panel,
  `monitor_height`) over `bin.rs` (shelves `Shelf` New/Files/Clips, one shown: new clips click to the
  playhead or drag a `BinItem` onto a track: `bin::dropped`, called by `tracks.rs` next to
  `keys::dropped`; the timeline's files; its clips), the inspector in the
  middle with `inspector_rows` first (`ROWS_ABOVE` skips them inside; edits spread as
  `ui::inspector`'s, `user_touched`). The inspector's two columns need about 940 pt, which is why
  the monitor isn't in the middle. Tests in `timeline/tests/workspace.rs` (1280×760, both
  interfaces). The inspector's "Also on" row writes `MediaCue::also_on`
  (home never in it); `bulk_edit::WHOLE` spreads it as one value and `tidy` drops the target's own
  display. Tests drive it with the mouse (`TimelineRects`, `Draw::Full` for the whole window and undo,
  the list tab's `Win`; cfg(test) notes such as `timeline_clip_text` and `timeline_state`); tried
  headless under xvfb in both interfaces.
