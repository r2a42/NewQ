//! The timeline editor: one component (`editor`) with its toolbar, ruler,
//! markers, tracks, clips and playhead, hosted as a pane (`pane`: the dock's
//! Timeline pane, and the bottom panel of the new interface's Editor view), as
//! a timeline list's tab (`list_tab`), and as a workspace of its own
//! (`workspace`, with its bin, `bin`); and the timeline rows of the inspector
//! (`control`). Which of the pane and the workspace opens is
//! `Prefs::timeline_layout` (`TimelineAction::SetLayout`).
//!
//! The editor never changes the show while it draws: what it is asked to do
//! comes back as `TimelineAction`s, which `apply` makes after the frame. A drag
//! is shown from `EditorState` while it lasts and asked for once, on release,
//! so it is one undo step.

mod bin;
mod control;
mod keys;
mod lanes;
pub mod list_tab;
mod menus;
mod ruler;
mod toolbar;
mod tracks;
pub mod workspace;

pub use control::{clip_fields, control_fields, describe_control, settings_fields};

use super::{Action, Pane, UiState, cue_label, fmt_time};
use crate::engine::{Engine, RunState, RunView};
pub use crate::prefs::{TimelineFollow, TimelineLayout};
use crate::show::{Animated, ControlAction, CueId, CueKind, CueType, PauseFreeze, Show};
use crate::timeline::edit::{self, Edge, PauseBefore};
use crate::timeline::{TimelineKey, TimelinePoint, TrackId, model};
use egui::{RichText, Ui, Vec2};
use std::path::PathBuf;

/// Where the editor is drawn. Also the salt of its egui ids, so two hosts
/// never share one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Host {
    Pane,
    /// A timeline list's tab, in the cue list's place.
    ListTab,
    /// The workspace, in the panes' place (classic) or as the new interface's
    /// Timeline view.
    Workspace,
}

/// The editor's own state (`UiState::timeline`).
pub struct EditorState {
    /// The editor is wanted (pane or workspace).
    pub open: bool,
    /// Otherwise it follows the selection.
    pub pinned: Option<TimelineKey>,
    /// Points per second (zoom); 0 = fit.
    pub pps: f32,
    pub scroll: Vec2,
    pub snap: bool,
    /// A timeline list's tab shows its cue rows instead of its tracks.
    pub list_rows: bool,
    /// Deleting a clip closes the gap it leaves on its track (the toolbar's
    /// Ripple; Shift+Delete ripples either way).
    pub ripple: bool,
    /// While the dock is drawn (it is out of `UiState::layout` meanwhile):
    /// whether a Timeline pane is in sight (`dock::Layout::in_front`).
    pub pane_in_front: Option<bool>,
    /// The marker last clicked on the ruler (of that timeline): drawn whole on
    /// top, and the one a flag of several at one place shows and acts on.
    marker_pick: Option<(TimelineKey, u64)>,
    /// What is being dragged, and in which host.
    drag: Option<(Host, Drag)>,
    /// A marker whose name is being typed, and the name so far.
    renaming: Option<(Host, u64, String)>,
    /// The zoom last drawn (what "fit" came to), for the zoom buttons.
    shown_pps: f32,
    /// The keyframe lanes open under clips, in the order opened.
    lanes: Vec<(CueId, Lane)>,
    /// A lane just opened, to scroll into view on the next draw.
    reveal: Option<(CueId, Lane)>,
    /// The editor that was clicked last, which the keys act on.
    focus: Option<Host>,
    /// Where on which track an empty spot was right-clicked, for its menu.
    menu_at: Option<(Host, TrackId, f64)>,
    /// A file being chosen for a new clip, answered by the dialog's thread.
    pick: Option<std::sync::mpsc::Receiver<Picked>>,
    /// Each host's editor as last drawn: its timeline, whether the pointer was
    /// over it, and in which pass (`pass`); for the Space key (`space_target`).
    drawn: Vec<(Host, TimelineKey, bool, u64)>,
    /// Counts the main window's frames (`next_pass`, after its keys are read).
    pass: u64,
}

/// A keyframe lane under a clip: its speed (keys that follow its content) or
/// its opacity (keys that follow the timeline's clock).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lane {
    Speed,
    Opacity,
}

/// A file chosen for a new clip: where it goes, and the file (None: cancelled).
type Picked = (TimelineKey, CueType, TrackId, f64, Option<PathBuf>);

impl Default for EditorState {
    fn default() -> EditorState {
        EditorState {
            open: false,
            pinned: None,
            pps: 0.0,
            scroll: Vec2::ZERO,
            // Snapping is what an editor is expected to do; Alt turns it off for a drag.
            snap: true,
            list_rows: false,
            ripple: false,
            pane_in_front: None,
            marker_pick: None,
            drag: None,
            renaming: None,
            shown_pps: 0.0,
            lanes: Vec::new(),
            reveal: None,
            focus: None,
            menu_at: None,
            pick: None,
            drawn: Vec::new(),
            pass: 0,
        }
    }
}

/// A drag in progress, shown as it goes and asked for on release.
#[derive(Clone, Debug, PartialEq)]
enum Drag {
    /// Clips moved in time (and the first, the anchor, perhaps to another track).
    Clips {
        ids: Vec<CueId>,
        /// The time under the pointer when it was pressed.
        grab: f64,
        /// How far they have moved, snapped, and the track the anchor is over.
        dt: f64,
        to: Option<TrackId>,
    },
    /// An edge of a clip, now at `to` (a time on the timeline); the other
    /// clips of the selection (`others`) have that edge moved as far.
    Trim {
        id: CueId,
        edge: Edge,
        to: f64,
        /// Where the held edge was when the drag began.
        from: f64,
        others: Vec<CueId>,
    },
    /// A clip's fade-in (`Edge::Left`) or fade-out handle, now `secs` long.
    Fade { id: CueId, edge: Edge, secs: f64 },
    /// A rubber band over the tracks, from where it was pressed; `add` keeps
    /// the selection there was (Ctrl/Cmd or Shift held).
    Band {
        from: egui::Pos2,
        to: egui::Pos2,
        add: bool,
    },
    /// A key on a lane: the lane's keys as they are now and which is held.
    Key {
        id: CueId,
        lane: Lane,
        keys: Animated,
        i: usize,
        /// Where it was taken, and the key's value then (kept while the
        /// pointer moves only along the lane).
        grab: egui::Pos2,
        v0: f32,
    },
    /// The playhead on the ruler, not yet let go, and the run's `RunView::mark`
    /// when it was taken. Anything else that moves, starts, stops or pauses the
    /// run meanwhile (a remote, a key, a cue) cancels it (`pos` None): its
    /// release then asks for nothing.
    Playhead {
        pos: Option<f64>,
        mark: crate::engine::RunMark,
    },
    /// A marker, now at `to`.
    Marker { id: u64, to: f64, grab: f64 },
}

impl EditorState {
    /// Drop a drag and a marker's renaming half done (after a panic: drawing
    /// one of them may be what fell over). What is shown and how stays.
    pub fn let_go(&mut self) {
        self.drag = None;
        self.renaming = None;
        self.menu_at = None;
    }

    /// The drag in progress in `host`.
    fn drag_in(&self, host: Host) -> Option<&Drag> {
        self.drag
            .as_ref()
            .filter(|(h, _)| *h == host)
            .map(|(_, d)| d)
    }
}

/// The timeline the editor should show: the active list if it is a timeline, else the
/// selected timeline group (or the group of the selected child), else the pinned one.
pub fn target(show: &Show, engine: &Engine, st: &UiState) -> Option<TimelineKey> {
    let list = TimelineKey::List(engine.active_list);
    if model::is_timeline(show, list) {
        return Some(list);
    }
    let selected = st.selected.and_then(|id| {
        let group = TimelineKey::Group(id);
        if model::is_timeline(show, group) {
            Some(group)
        } else {
            model::key_of(show, id).filter(|k| matches!(k, TimelineKey::Group(_)))
        }
    });
    selected.or(st.timeline.pinned.filter(|k| model::is_timeline(show, *k)))
}

/// The timeline rows for the cue at `idx`, at the end of its inspector: a
/// control cue's timeline action, a clip's run-out and lead-in choices. (They
/// need the show and the cue at once, so they edit a copy of the cue's part
/// and put it back if it changed.)
pub(super) fn inspector_rows(ui: &mut Ui, show: &mut Show, idx: usize) {
    if ui.data(|d| d.get_temp::<bool>(egui::Id::new(workspace::ROWS_ABOVE))) == Some(true) {
        return;
    }
    let Some(cue) = show.cues.get(idx) else {
        return;
    };
    let id = cue.id;
    match &cue.kind {
        crate::show::CueKind::Control(c) if c.action.is_timeline() => {
            let mut edited = c.clone();
            ui.separator();
            control_fields(ui, show, id, &mut edited);
            if let crate::show::CueKind::Control(c) = &mut show.cues[idx].kind
                && *c != edited
            {
                *c = edited;
            }
        }
        k if k.media().is_some() && model::key_of(show, id).is_some() => {
            let Some(mut edited) = k.media().cloned() else {
                return;
            };
            ui.separator();
            clip_fields(ui, show, id, &mut edited);
            if let Some(m) = show.cues[idx].kind.media_mut()
                && *m != edited
            {
                *m = edited;
            }
        }
        _ => {}
    }
}

/// What a timeline is called: its group's number and name, or its list's name.
fn timeline_name(show: &Show, key: TimelineKey) -> String {
    model::name(show, key)
}

/// Every timeline in the show: the timeline groups, then the timeline lists.
fn all_timelines(show: &Show) -> Vec<TimelineKey> {
    let groups = show
        .cues
        .iter()
        .map(|c| TimelineKey::Group(c.id))
        .filter(|k| model::is_timeline(show, *k));
    let lists = show
        .lists
        .iter()
        .map(|l| TimelineKey::List(l.id))
        .filter(|k| model::is_timeline(show, *k));
    groups.chain(lists).collect()
}

/// Edits are refused while the show is locked (show mode) or a test runs.
fn can_edit(st: &UiState) -> bool {
    !st.show_mode && !st.test_locked
}

/// THE component: toolbar, ruler, markers, tracks, clips, playhead.
pub fn editor(
    ui: &mut Ui,
    show: &Show,
    engine: &mut Engine,
    st: &mut UiState,
    key: TimelineKey,
    host: Host,
    actions: &mut Vec<Action>,
) {
    // A lane's keys being dragged are shown on a copy of the show with them
    // in, so the clip's length (speed) and lead-in (opacity) follow them.
    let preview = lanes::preview(show, st.timeline.drag_in(host));
    let show = preview.as_ref().unwrap_or(show);
    // Lanes of clips that have gone (deleted, undone) are let go.
    st.timeline
        .lanes
        .retain(|(id, lane)| show.cue(*id).is_some_and(|c| lanes::has(c, *lane)));
    let clips = model::clips(show, key, &mut |c| engine.cue_duration(show, c));
    let tracks = model::tracks(show, key, &clips);
    let run = engine.timeline(show, key);
    // The playhead: the run's, or while the group plays the old way (before
    // the engine runs timelines) where the group has got to.
    let (pos, moving) = match (run.state, key) {
        (RunState::Stopped, TimelineKey::Group(g)) => match engine.status(g) {
            crate::engine::CueStatus::Playing { elapsed, .. } => (elapsed, true),
            _ => (run.pos, false),
        },
        (RunState::Running | RunState::Cueing, _) => (run.pos, true),
        _ => (run.pos, false),
    };
    // What it shows stays when the selection moves off it. (A list's tab
    // shows its list, whatever the pane follows.)
    if host != Host::ListTab {
        st.timeline.pinned = Some(key);
    }
    let ctx = tracks::Ctx {
        show,
        key,
        host,
        clips: &clips,
        tracks: &tracks,
        run: &run,
        pos,
        moving,
        detached: engine.timeline_detached(key),
        waiting: engine.timeline_waiting(key),
        editable: can_edit(st),
        // In show mode only the active list's timeline may be parked from its
        // ruler: parking puts picture on the real outputs (plan 2.7).
        ruler_locked: st.show_mode && key != TimelineKey::List(engine.active_list),
    };
    // The keys go to the editor last clicked in, or the one under the pointer.
    let whole = ui.available_rect_before_wrap();
    if ui.input(|i| i.pointer.primary_pressed() || i.pointer.secondary_pressed()) {
        let inside = ui
            .input(|i| i.pointer.interact_pos())
            .is_some_and(|p| whole.contains(p));
        if inside {
            st.timeline.focus = Some(host);
        } else if st.timeline.focus == Some(host) {
            st.timeline.focus = None;
        }
    }
    let hovered = ui.rect_contains_pointer(whole);
    let pass = st.timeline.pass;
    st.timeline.drawn.retain(|d| d.0 != host);
    st.timeline.drawn.push((host, key, hovered, pass));
    // For the tests: which editors a frame drew.
    #[cfg(test)]
    ui.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<Host>>(egui::Id::new("timeline_editors"))
            .push(host)
    });
    ui.push_id(("timeline_editor", host), |ui| {
        keys::picked(st, actions);
        toolbar::draw(ui, &ctx, engine, st, actions);
        ui.separator();
        tracks::draw(ui, &ctx, engine, st, actions);
        keys::handle(ui, &ctx, st, hovered, actions);
    });
}

/// A new frame of the main window has begun (its keys read): editors drawn
/// before it are no longer under the pointer.
pub(super) fn next_pass(st: &mut UiState) {
    st.timeline.pass += 1;
}

/// The timeline of an editor the pointer was over in the last frame.
pub(super) fn hovered_editor(st: &UiState) -> Option<TimelineKey> {
    st.timeline
        .drawn
        .iter()
        .find(|d| d.2 && d.3 == st.timeline.pass)
        .map(|d| d.1)
}

/// The timeline the Space key plays and pauses, if one is being worked on:
/// the editor that holds the keys (the one last clicked in, while it still
/// shows that timeline; else the one under the pointer, `hovered`), else the
/// timeline list being shown (its tracks or its rows, any view). None: Space
/// is GO.
pub(super) fn space_target(
    show: &Show,
    engine: &Engine,
    st: &UiState,
    hovered: Option<TimelineKey>,
) -> Option<TimelineKey> {
    let focused = st.timeline.focus.and_then(|h| {
        let (_, key, _, _) = *st.timeline.drawn.iter().find(|d| d.0 == h)?;
        let shown = match h {
            Host::ListTab => list_tab::active(show, engine) == Some(key),
            Host::Pane | Host::Workspace => {
                (st.timeline.open || st.layout.is_open(Pane::Timeline))
                    && target(show, engine, st) == Some(key)
            }
        };
        shown.then_some(key)
    });
    focused
        .or(hovered)
        .filter(|k| model::is_timeline(show, *k))
        .or_else(|| list_tab::active(show, engine))
}

/// What Space does to the timeline `key`: running (outside a loop) or
/// cueing, the toolbar's Pause; else what GO does to it (`timeline_go_one`:
/// a stopped one runs, a paused or parked one goes on as GO would at a pause
/// cue, a loop is left as its cue says, nothing in a loop's last pass).
pub(super) fn space_action(show: &Show, engine: &mut Engine, key: TimelineKey) -> TimelineAction {
    let run = engine.timeline(show, key);
    if run.looping.is_none() && matches!(run.state, RunState::Running | RunState::Cueing) {
        TimelineAction::Pause(key)
    } else {
        TimelineAction::Go(key)
    }
}

/// Option A's body: `editor` for `target(..)`, or the empty state.
pub fn pane(
    ui: &mut Ui,
    show: &Show,
    engine: &mut Engine,
    st: &mut UiState,
    actions: &mut Vec<Action>,
) {
    // A Timeline pane left open (or in a window of its own) while the editor
    // opens as a workspace: one place for it, so the pane only points there.
    if workspace::chosen(st) {
        workspace::pointer(ui, show, engine, st, actions);
        return;
    }
    match target(show, engine, st) {
        // The list's own tab has its tracks already: say so rather than draw
        // them twice.
        Some(key)
            if Some(key) == list_tab::active(show, engine)
                && list_tab::shows_tracks(show, engine, st) =>
        {
            #[cfg(test)]
            ui.data_mut(|d| d.insert_temp(egui::Id::new("timeline_in_tab"), true));
            ui.add_space(8.0);
            ui.label(format!(
                "The tracks of {} are in its tab, in the cue list's place.",
                timeline_name(show, key)
            ));
            ui.label(
                RichText::new(
                    "Pick \u{201c}Cue rows\u{201d} there to see its cues as rows, with the tracks here.",
                )
                .weak(),
            );
            // In the panes it would only take room from the cue list's tab.
            if !st.prefs.new_layout
                && ui
                    .button("Close this pane")
                    .on_hover_text("Window ▸ Timeline brings it back")
                    .clicked()
            {
                actions.push(Action::Timeline(TimelineAction::Close));
            }
        }
        Some(key) => editor(ui, show, engine, st, key, Host::Pane, actions),
        None => empty_state(ui, st, actions),
    }
}

/// What the pane shows with no timeline to edit.
fn empty_state(ui: &mut Ui, st: &UiState, actions: &mut Vec<Action>) {
    #[cfg(test)]
    ui.data_mut(|d| d.insert_temp(egui::Id::new("timeline_empty"), true));
    ui.add_space(8.0);
    ui.label("Select a timeline group to see its tracks here.");
    ui.label(
        RichText::new(
            "A timeline group is a group set to \u{201c}Start all together\u{201d}: each cue in it \
             starts at its pre-wait on one clock.",
        )
        .weak(),
    );
    ui.add_space(4.0);
    if ui
        .add_enabled(can_edit(st), egui::Button::new("New timeline group"))
        .on_hover_text("Add an empty timeline group after the selected cue")
        .clicked()
    {
        actions.push(Action::Timeline(TimelineAction::NewTimelineGroup));
    }
}

/// What the editor asks for; made after the frame by `apply`.
#[derive(Clone, Debug, PartialEq)]
pub enum TimelineAction {
    // edits (refused in show mode)
    MoveClips {
        key: TimelineKey,
        ids: Vec<CueId>,
        dt: f64,
        to: Option<TrackId>,
    },
    Trim {
        id: CueId,
        edge: Edge,
        to: f64,
    },
    Split {
        key: TimelineKey,
        ids: Vec<CueId>,
        at: f64,
    },
    Delete {
        key: TimelineKey,
        ids: Vec<CueId>,
        ripple: bool,
    },
    AddClip {
        key: TimelineKey,
        ty: CueType,
        file: Option<PathBuf>,
        track: TrackId,
        at: f64,
    },
    AddControl {
        key: TimelineKey,
        action: ControlAction,
        at: f64,
    },
    AddPauseBefore {
        key: TimelineKey,
        clip: CueId,
        what: PauseBefore,
    },
    AddTrack {
        key: TimelineKey,
        picture: bool,
    },
    RemoveTrack {
        key: TimelineKey,
        track: TrackId,
    },
    /// A picture track above (`up`) or below the next of its display.
    MoveTrack {
        key: TimelineKey,
        track: TrackId,
        up: bool,
    },
    /// Choose a file for a new clip of `ty` at `at` on `track` (a dialog;
    /// its answer comes back as an `AddClip`).
    PickClip {
        key: TimelineKey,
        ty: CueType,
        track: TrackId,
        at: f64,
    },
    /// A clip's fade-in (`Edge::Left`) or fade-out, in seconds.
    SetFade {
        id: CueId,
        edge: Edge,
        secs: f64,
    },
    /// A picture clip's opacity keys (none: a steady opacity of `value`).
    SetOpacity {
        id: CueId,
        opacity: Animated,
    },
    AddMarker {
        key: TimelineKey,
        at: f64,
    },
    MoveMarker {
        key: TimelineKey,
        id: u64,
        to: f64,
    },
    RenameMarker {
        key: TimelineKey,
        id: u64,
        name: String,
    },
    RemoveMarker {
        key: TimelineKey,
        id: u64,
    },
    /// The clipboard's cues as clips, the earliest at `at`, on their tracks.
    Paste {
        key: TimelineKey,
        at: f64,
    },
    /// Copy the clips `ids` and take them off (closing the gap with `ripple`).
    Cut {
        key: TimelineKey,
        ids: Vec<CueId>,
        ripple: bool,
    },
    /// Copies of the clips `ids` just after them.
    Duplicate {
        key: TimelineKey,
        ids: Vec<CueId>,
    },
    SetRate {
        id: CueId,
        rate: Animated,
    },
    NewTimelineList,
    NewTimelineGroup,
    SetListTimeline {
        list: u32,
        on: bool,
    },
    /// Turn on the show's "a picture replaces only what is on its own layer"
    /// (offered by a timeline list's tab, never done unasked).
    UseOwnLayers,
    // transport (allowed in show mode)
    Run(TimelineKey),
    Pause(TimelineKey),
    /// What GO does to this timeline (Space, where it isn't a Pause).
    Go(TimelineKey),
    Stop(TimelineKey),
    /// On release of the playhead.
    Seek {
        key: TimelineKey,
        pos: f64,
    },
    JumpToMarker {
        key: TimelineKey,
        id: u64,
    },
    FreezeClip(CueId, bool),
    // view
    /// Pick these clips (a rubber band); `add` keeps those already picked.
    SelectClips {
        ids: Vec<CueId>,
        add: bool,
    },
    /// Open or close a keyframe lane under a clip.
    ShowLane {
        id: CueId,
        lane: Lane,
        on: bool,
    },
    Open(Option<TimelineKey>),
    Close,
    /// Where the editor opens on this computer: a pane or the workspace (an
    /// editor already open moves there).
    SetLayout(TimelineLayout),
    /// How the view keeps a running playhead in sight, on this computer.
    SetFollow(TimelineFollow),
}

impl TimelineAction {
    /// Whether it changes the show, and so is refused in show mode.
    pub fn edits_the_show(&self) -> bool {
        use TimelineAction as A;
        match self {
            A::MoveClips { .. }
            | A::Trim { .. }
            | A::Split { .. }
            | A::Delete { .. }
            | A::AddClip { .. }
            | A::AddControl { .. }
            | A::AddPauseBefore { .. }
            | A::AddTrack { .. }
            | A::RemoveTrack { .. }
            | A::MoveTrack { .. }
            | A::PickClip { .. }
            | A::SetFade { .. }
            | A::SetOpacity { .. }
            | A::AddMarker { .. }
            | A::MoveMarker { .. }
            | A::RenameMarker { .. }
            | A::RemoveMarker { .. }
            | A::Paste { .. }
            | A::Cut { .. }
            | A::Duplicate { .. }
            | A::SetRate { .. }
            | A::NewTimelineList
            | A::NewTimelineGroup
            | A::SetListTimeline { .. }
            | A::UseOwnLayers => true,
            A::Run(_)
            | A::Pause(_)
            | A::Go(_)
            | A::Stop(_)
            | A::Seek { .. }
            | A::JumpToMarker { .. }
            | A::FreezeClip(..)
            | A::SelectClips { .. }
            | A::ShowLane { .. }
            | A::Open(_)
            | A::Close
            | A::SetLayout(_)
            | A::SetFollow(_) => false,
        }
    }
}

/// Where the editor seeks for a time picked on it. The engine takes a seek to
/// exactly 0 as "from the top" and parks with nothing up; the operator who
/// clicks the very start wants its first frame, so a hair past it.
fn seek_to(pos: f64) -> f64 {
    pos.max(crate::engine::FIRST_FRAME)
}

/// Make what the editor asked for. (`ui::apply` has already refused an edit
/// in show mode or during a test.)
pub(super) fn apply(a: TimelineAction, show: &mut Show, engine: &mut Engine, st: &mut UiState) {
    use TimelineAction as A;
    // The edits that need lengths get them from the engine, on the show as it
    // was before the edit.
    let before = || show.clone();
    match a {
        A::MoveClips { key, ids, dt, to } => {
            edit::move_clips(show, key, &ids, dt, to);
        }
        A::Trim { id, edge, to } => {
            let was = before();
            edit::trim(show, id, edge, to, &mut |c| engine.cue_duration(&was, c));
        }
        A::Split { key, ids, at } => {
            let was = before();
            let made = edit::split(show, key, &ids, at, &mut |c| engine.cue_duration(&was, c));
            // Clips start on the engine's tick (plan 2.13, 7): a sound cut in
            // two may click or leave a tiny gap where its halves meet.
            let sound = made
                .iter()
                .filter_map(|id| show.cue(*id))
                .any(|c| matches!(c.kind, CueKind::Audio(_) | CueKind::Video(_)));
            let n = made.len();
            let s = if n == 1 { "" } else { "s" };
            if n == 0 {
                engine.log("Nothing to split there: put the playhead inside a clip");
            } else if sound {
                engine.log(format!(
                    "Split {n} clip{s}: a sound split in two may click or gap a little at the join"
                ));
            } else {
                engine.log(format!("Split {n} clip{s}"));
            }
        }
        A::Delete { key, ids, ripple } => {
            let was = before();
            let gone = edit::delete(show, key, &ids, ripple, &mut |c| {
                engine.cue_duration(&was, c)
            });
            for id in &gone {
                engine.stop_cue(*id, 0.0);
            }
            if st.selected.is_some_and(|s| gone.contains(&s)) {
                st.selected = None;
                st.forget_multi();
            }
        }
        A::AddClip {
            key,
            ty,
            file,
            track,
            at,
        } => {
            if let Some(id) = edit::add_clip(show, key, ty, file, track, at) {
                super::apply(Action::Select(id), show, engine, st);
            }
        }
        A::AddControl { key, action, at } => {
            if let Some(id) = edit::add_control(show, key, action, at) {
                super::apply(Action::Select(id), show, engine, st);
            }
        }
        A::AddPauseBefore { key, clip, what } => {
            let was = before();
            if edit::add_pause_before(show, key, clip, what, &mut |c| engine.cue_duration(&was, c))
                .is_some()
            {
                engine.log("Added a pause cue: GO runs the timeline on from it");
            }
        }
        A::AddTrack { key, picture } => {
            edit::add_track(show, key, picture);
        }
        A::RemoveTrack { key, track } => {
            edit::remove_track(show, key, track);
        }
        A::MoveTrack { key, track, up } => {
            edit::move_track(show, key, track, up);
        }
        A::PickClip { key, ty, track, at } => keys::pick_file(st, key, ty, track, at),
        A::SetFade { id, edge, secs } => {
            let was = before();
            edit::set_fade(show, id, edge, secs, &mut |c| engine.cue_duration(&was, c));
        }
        A::SetOpacity { id, opacity } => {
            edit::set_opacity(show, id, opacity);
        }
        A::AddMarker { key, at } => {
            let n = model::settings(show, key).map_or(0, |s| s.markers.len()) + 1;
            edit::add_marker(show, key, at, &format!("Marker {n}"));
        }
        A::MoveMarker { key, id, to } => {
            edit::move_marker(show, key, id, to);
        }
        A::RenameMarker { key, id, name } => {
            edit::rename_marker(show, key, id, &name);
        }
        A::RemoveMarker { key, id } => {
            edit::remove_marker(show, key, id);
        }
        A::Paste { key, at } => paste_into(show, engine, st, key, at),
        A::Cut { key, ids, ripple } => {
            super::apply(Action::Copy, show, engine, st);
            // As the cue list's cut: what was cut comes back with its ids.
            let kept = !st.clipboard.is_empty();
            apply(A::Delete { key, ids, ripple }, show, engine, st);
            st.clipboard_cut = kept;
        }
        A::Duplicate { key, ids } => {
            let was = before();
            let made =
                edit::duplicate_clips(show, key, &ids, &mut |c| engine.cue_duration(&was, c));
            pick(show, engine, st, &made);
        }
        A::SetRate { id, rate } => {
            edit::set_rate(show, id, rate);
        }
        A::NewTimelineList => {
            let n = show.lists.iter().filter(|l| l.timeline.is_some()).count() + 1;
            let id = edit::new_timeline_list(show, &format!("Timeline {n}"));
            super::apply(Action::SelectList(id), show, engine, st);
            st.timeline.list_rows = false;
            if !show.own_layers {
                engine.log(
                    "New timeline: this show replaces everything on a display; its tab offers \
                     to keep pictures on their own layers",
                );
            }
        }
        A::NewTimelineGroup if list_tab::is_timeline_list(show, engine.active_list) => {
            engine.log(list_tab::NO_GROUPS);
        }
        A::NewTimelineGroup => {
            let slot = show.insert_slot(st.selected, true, engine.active_list);
            let id = edit::new_timeline_group(show, slot);
            if let Some(i) = show.cue_index(id) {
                show.cues[i].name = "Timeline".into();
            }
            super::select_new(show, st, engine, None, id);
            st.timeline.pinned = Some(TimelineKey::Group(id));
        }
        A::SetListTimeline { list, on } => {
            match edit::set_list_timeline(show, list, on) {
                Ok(()) if on => st.timeline.list_rows = false,
                Ok(()) => {}
                Err(e) => engine.log(e),
            }
            // A run of a list that is no longer a timeline is let go.
            engine.sync_lists(show);
        }
        A::UseOwnLayers => {
            show.own_layers = true;
            engine.log("A picture now replaces only what is on its own layer of its display");
        }
        A::Run(key) => engine.timeline_run(show, key),
        A::Go(key) => engine.timeline_go_one(show, key),
        // The editor's Pause holds everything (as the transport's Pause does)
        // and leaves live inputs live; its Run carries on.
        A::Pause(key) => {
            engine.timeline_pause(show, key, PauseFreeze::Everything, &[], false, false)
        }
        // As a timeline Stop cue does: a fade, and a second Stop cuts.
        A::Stop(key) => engine.timeline_stop(show, key, crate::show::TIMELINE_STOP_FADE),
        A::Seek { key, pos } => engine.timeline_seek(show, key, seek_to(pos), true),
        A::JumpToMarker { key, id } => {
            if let Some(t) = model::resolve(show, key, TimelinePoint::Marker(id)) {
                engine.timeline_seek(show, key, seek_to(t), true);
            }
        }
        A::FreezeClip(id, on) => {
            let action = if on {
                ControlAction::Pause
            } else {
                ControlAction::Resume
            };
            engine.control_target(show, action, id, "the timeline editor");
        }
        A::SelectClips { ids, add } => {
            let mut all = if add { st.selection(show) } else { Vec::new() };
            for id in ids {
                if !all.contains(&id) {
                    all.push(id);
                }
            }
            if let Some(&first) = all.first() {
                super::select(show, st, engine, Some(first));
                if all.len() > 1 {
                    st.select_many(all, first);
                }
            }
        }
        A::ShowLane { id, lane, on } => {
            st.timeline.lanes.retain(|l| *l != (id, lane));
            if on {
                st.timeline.lanes.push((id, lane));
                st.timeline.reveal = Some((id, lane));
            }
        }
        A::Open(key) => {
            st.timeline.open = true;
            if key.is_some() {
                st.timeline.pinned = key;
            }
            if workspace::chosen(st) {
                // The workspace takes the panes' place (or is the new
                // interface's Timeline view); the settings give it back.
                if st.prefs.new_layout {
                    st.modern_view = super::modern::View::Timeline;
                }
                if st.settings_open() {
                    super::apply(Action::CloseSettings, show, engine, st);
                }
            } else if !st.prefs.new_layout {
                // The new interface has its own place for it (the Editor view's
                // bottom panel); the panes get the Timeline pane.
                st.layout.open(Pane::Timeline);
            }
        }
        A::Close => {
            st.timeline.open = false;
            if st.prefs.new_layout {
                if st.modern_view == super::modern::View::Timeline {
                    st.modern_view = super::modern::View::Editor;
                }
            } else {
                st.layout.close(Pane::Timeline);
            }
        }
        A::SetFollow(follow) => {
            st.prefs.timeline_follow = follow;
            if let Some(dir) = &st.layout_dir
                && let Err(e) = st.prefs.save(dir)
            {
                log::warn!("could not save the settings of this computer: {e}");
            }
        }
        // Kept for this computer; an editor that is open moves to the new place.
        A::SetLayout(layout) => {
            let open =
                st.timeline.open || (!st.prefs.new_layout && st.layout.is_open(Pane::Timeline));
            let key = target(show, engine, st);
            st.prefs.timeline_layout = layout;
            if let Some(dir) = &st.layout_dir
                && let Err(e) = st.prefs.save(dir)
            {
                log::warn!("could not save the settings of this computer: {e}");
            }
            apply(A::Close, show, engine, st);
            // Back from the workspace to a timeline list's tracks: they are
            // in its tab, not in a pane beside it.
            if layout == TimelineLayout::Pane
                && key.is_some()
                && key == list_tab::active(show, engine)
            {
                st.timeline.list_rows = false;
            } else if open {
                apply(A::Open(key), show, engine, st);
            }
        }
    }
}

/// Paste the clipboard onto the timeline `key` as clips, the earliest at
/// `at` (the playhead), each on the track it had. A group is refused.
pub(super) fn paste_into(
    show: &mut Show,
    engine: &mut Engine,
    st: &mut UiState,
    key: TimelineKey,
    at: f64,
) {
    if st
        .clipboard
        .iter()
        .any(|c| matches!(c.kind, CueKind::Group(_)))
    {
        engine.log("A group can't be pasted onto a timeline: its cues are clips on one clock");
        return;
    }
    let copies = std::mem::take(&mut st.clipboard);
    let made = edit::paste_clips(
        show,
        key,
        &copies,
        at,
        st.clipboard_epoch == st.show_epoch,
        st.clipboard_cut,
    );
    st.clipboard = copies;
    if made.is_empty() {
        return;
    }
    // Only the first paste of a cut puts the cues back as they were.
    st.clipboard_cut = false;
    pick(show, engine, st, &made);
    let n = made.len();
    engine.log(format!(
        "Pasted {n} clip{} at {}",
        if n == 1 { "" } else { "s" },
        super::fmt_time(at)
    ));
}

/// Make `ids` the selection (the last one selected).
fn pick(show: &Show, engine: &mut Engine, st: &mut UiState, ids: &[CueId]) {
    let Some(&last) = ids.last() else {
        return;
    };
    super::select(show, st, engine, Some(last));
    if ids.len() > 1 {
        st.select_many(ids.to_vec(), last);
    }
}

/// Where the editor drew things, for tests to aim the mouse at; stored at
/// `egui::Id::new(("timeline_rects", host))`.
#[cfg(test)]
#[derive(Clone, Debug)]
pub struct TimelineRects {
    pub ruler: egui::Rect,
    pub clips: Vec<(CueId, egui::Rect)>,
    pub tracks: Vec<(TrackId, egui::Rect)>,
    pub markers: Vec<(u64, egui::Rect)>,
    /// The ghosts of clips on the other displays they are also on.
    pub ghosts: Vec<(CueId, egui::Rect)>,
    /// The keyframe lanes open, and the lane's keys' centres.
    pub lanes: Vec<(CueId, Lane, egui::Rect)>,
    pub keys: Vec<(CueId, Lane, egui::Pos2)>,
    /// The fade handles drawn.
    pub fades: Vec<(CueId, Edge, egui::Rect)>,
    pub playhead_x: Option<f32>,
    /// The sideways scroll bar's thumb (none when it all fits).
    pub hscroll: Option<egui::Rect>,
    pub x0: f32,
    pub pps: f32,
}

/// Note, for the tests, that an inspector row was drawn.
#[cfg(test)]
pub(super) fn note_row(ui: &Ui, row: &'static str) {
    ui.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<&'static str>>(egui::Id::new("timeline_rows"))
            .push(row)
    });
}

#[cfg(not(test))]
fn note_row(_ui: &Ui, _row: &'static str) {}

#[cfg(test)]
mod tests;
