//! The workspace's bin: what can go on the timeline and what is on it.
//!
//! Three shelves, one shown at a time (so each has the column's height):
//! - New: a text, shape, camera, live sound, fade, send or pause cue, or a
//!   video, picture or sound file (a dialog). A click puts it at the playhead
//!   on a track of its kind with room; a drag puts it where it is let go on a
//!   track (`dropped`, which the tracks call, as they do for files).
//! - Files: each file the timeline plays, dragged onto a track for another
//!   clip of it.
//! - Clips: the timeline's cues in time order; a click picks one, as a click
//!   on its clip does.

use super::tracks::{Ctx, Geo};
use super::*;
use egui::{Rect, Sense};

/// What is dragged from the bin (egui's drag and drop payload).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum BinItem {
    /// A new cue of a type that needs no file.
    New(CueType),
    /// A timeline pause cue.
    Pause,
    /// Another clip of a file the timeline plays.
    File(CueType, PathBuf),
}

impl BinItem {
    fn words(&self) -> String {
        match self {
            BinItem::New(ty) => ty.label().to_string(),
            BinItem::Pause => "Pause".into(),
            BinItem::File(_, f) => file_name(f),
        }
    }
}

/// The new cues the bin offers (type, words, what it is).
const NEW: [(CueType, &str, &str); 6] = [
    (CueType::Text, "Text", "Words on a display"),
    (
        CueType::Shape,
        "Shape",
        "A rectangle, circle or line on a display",
    ),
    (CueType::Camera, "Camera", "A live camera or capture input"),
    (
        CueType::LiveAudio,
        "Live sound",
        "A microphone or line input",
    ),
    (CueType::Fade, "Fade", "A fade cue on the cue track"),
    (
        CueType::Send,
        "Send",
        "An OSC, MIDI or network message on the cue track",
    ),
];

/// The files the bin offers to choose (type, words).
const PICK: [(CueType, &str); 3] = [
    (CueType::Video, "Video…"),
    (CueType::Image, "Image…"),
    (CueType::Audio, "Sound…"),
];

fn file_name(f: &std::path::Path) -> String {
    f.file_name()
        .map_or_else(|| f.display().to_string(), |n| n.to_string_lossy().into())
}

/// Note, for the tests, where a bin item is.
fn note(_ui: &Ui, _name: String, _r: &egui::Response) {
    #[cfg(test)]
    _ui.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<(String, Rect)>>(egui::Id::new("bin_items"))
            .push((_name, _r.rect))
    });
}

/// Which of the bin's three shelves is showing (kept for the session).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Shelf {
    #[default]
    New,
    Files,
    Clips,
}

const SHELF: &str = "ws_bin_shelf";

/// The bin for the timeline `key`: a row of three shelves (new clips, the
/// timeline's files, its clips), the chosen one in the rest of the column,
/// so a short column still shows a useful list.
pub(super) fn draw(
    ui: &mut Ui,
    show: &Show,
    engine: &mut Engine,
    st: &UiState,
    key: TimelineKey,
    actions: &mut Vec<Action>,
) {
    let clips = model::clips(show, key, &mut |c| engine.cue_duration(show, c));
    let pos = engine.timeline(show, key).pos;
    let mut files = files_of(show, key);
    // The media manager's library (in a show network, on the primary): files
    // already on every computer, ready to be used.
    for f in st.media.library_files.iter() {
        if !files.iter().any(|(_, g)| g == f)
            && let Some(ty) = crate::show::guess_cue_type(f)
        {
            files.push((ty, f.clone()));
        }
    }
    let id = egui::Id::new(SHELF);
    let mut shelf: Shelf = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (s, words) in [
            (Shelf::New, "NEW".to_string()),
            (Shelf::Files, format!("FILES {}", files.len())),
            (Shelf::Clips, format!("CLIPS {}", clips.len())),
        ] {
            let r = ui.selectable_label(shelf == s, RichText::new(&words).small().strong());
            note(
                ui,
                format!("shelf {}", words.split(' ').next().unwrap_or("")),
                &r,
            );
            if r.clicked() {
                shelf = s;
            }
        }
    });
    ui.data_mut(|d| d.insert_temp(id, shelf));
    let _shown = egui::ScrollArea::vertical()
        .id_salt(("ws_bin", shelf as u8))
        .auto_shrink([false, false])
        .show(ui, |ui| match shelf {
            Shelf::New => new_items(ui, show, st, key, pos, &clips, actions),
            Shelf::Files => file_items(ui, st, files),
            Shelf::Clips => clip_items(ui, show, st, &clips, actions),
        })
        .inner_rect;
    #[cfg(test)]
    ui.data_mut(|d| d.insert_temp(egui::Id::new("ws_bin"), _shown));
    follow(ui);
}

/// The new clips: a file to choose, or a cue that needs none.
fn new_items(
    ui: &mut Ui,
    show: &Show,
    st: &UiState,
    key: TimelineKey,
    pos: f64,
    clips: &[model::Clip],
    actions: &mut Vec<Action>,
) {
    let push = |actions: &mut Vec<Action>, a| actions.push(Action::Timeline(a));
    ui.label(
        RichText::new(format!("click: at {} · drag: onto a track", fmt_time(pos)))
            .weak()
            .small(),
    );
    ui.add_enabled_ui(can_edit(st), |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        ui.horizontal_wrapped(|ui| {
            for (ty, words) in PICK {
                let r = ui
                    .add(egui::Button::new(RichText::new(words).small()))
                    .on_hover_text("Choose a file for a new clip at the playhead");
                note(ui, words.into(), &r);
                if r.clicked() {
                    let track = list_tab::fitting_track(show, key, ty, pos, clips);
                    push(
                        actions,
                        TimelineAction::PickClip {
                            key,
                            ty,
                            track,
                            at: pos,
                        },
                    );
                }
            }
            for (ty, words, what) in NEW {
                let r = item(ui, words, what, BinItem::New(ty));
                if r.clicked() {
                    let track = list_tab::fitting_track(show, key, ty, pos, clips);
                    push(
                        actions,
                        TimelineAction::AddClip {
                            key,
                            ty,
                            file: None,
                            track,
                            at: pos,
                        },
                    );
                }
            }
            let r = item(
                ui,
                "Pause",
                "A pause cue: the timeline stops there until GO",
                BinItem::Pause,
            );
            if r.clicked() {
                push(
                    actions,
                    TimelineAction::AddControl {
                        key,
                        action: ControlAction::TimelinePause,
                        at: pos,
                    },
                );
            }
        });
    });
}

/// Each file the timeline plays, once, with the type of cue it makes.
fn files_of(show: &Show, key: TimelineKey) -> Vec<(CueType, PathBuf)> {
    let mut files: Vec<(CueType, PathBuf)> = Vec::new();
    for c in model::members(show, key) {
        let Some(m) = c.kind.file_media() else {
            continue;
        };
        if m.file.as_os_str().is_empty() || files.iter().any(|(_, f)| *f == m.file) {
            continue;
        }
        if let Some(ty) = crate::show::guess_cue_type(&m.file) {
            files.push((ty, m.file.clone()));
        }
    }
    files
}

/// The timeline's files, each dragged onto a track for another clip of it.
fn file_items(ui: &mut Ui, st: &UiState, files: Vec<(CueType, PathBuf)>) {
    if files.is_empty() {
        ui.label(RichText::new("No files on this timeline yet").weak());
        return;
    }
    ui.label(
        RichText::new("drag onto a track for another clip")
            .weak()
            .small(),
    );
    ui.add_enabled_ui(can_edit(st), |ui| {
        for (ty, f) in files {
            let words = format!("{} · {}", ty.label(), file_name(&f));
            let tip = format!("{}\nDrag onto a track for another clip of it", f.display());
            item(ui, &words, &tip, BinItem::File(ty, f));
        }
    });
}

/// The timeline's clips in time order; a click picks one.
fn clip_items(
    ui: &mut Ui,
    show: &Show,
    st: &UiState,
    clips: &[model::Clip],
    actions: &mut Vec<Action>,
) {
    let mut sorted: Vec<&model::Clip> = clips.iter().collect();
    sorted.sort_by(|a, b| a.start.total_cmp(&b.start));
    if sorted.is_empty() {
        ui.label(RichText::new("Nothing on this timeline yet").weak());
    }
    let picked = st.selection(show);
    ui.spacing_mut().item_spacing.y = 1.0;
    ui.spacing_mut().button_padding.y = 1.0;
    for c in sorted {
        let Some(cue) = show.cue(c.id) else {
            continue;
        };
        let words = format!("{}  {}", fmt_time(c.start), cue_label(cue));
        let r = ui
            .add(
                egui::Button::selectable(picked.contains(&c.id), words)
                    .truncate()
                    .min_size(egui::vec2(ui.available_width(), 0.0)),
            )
            .on_hover_text(format!("{} · pick it", c.ty.label()));
        note(ui, format!("clip {}", cue.number), &r);
        if r.clicked() {
            actions.push(super::tracks::click_select(ui, c.id));
        }
    }
}

/// A bin entry that is clicked or dragged.
fn item(ui: &mut Ui, words: &str, tip: &str, what: BinItem) -> egui::Response {
    let r = ui
        .add(
            egui::Button::new(RichText::new(words).small())
                .truncate()
                .sense(Sense::click_and_drag()),
        )
        .on_hover_text(tip);
    note(ui, what.words(), &r);
    r.dnd_set_drag_payload(what);
    r
}

/// What is dragged from the bin, by the pointer.
fn follow(ui: &Ui) {
    let Some(what) = egui::DragAndDrop::payload::<BinItem>(ui.ctx()) else {
        return;
    };
    let Some(p) = ui.ctx().pointer_latest_pos() else {
        return;
    };
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::new("bin_drag"),
    ));
    let text = painter.layout_no_wrap(
        format!("+ {}", what.words()),
        egui::FontId::proportional(13.0),
        egui::Color32::WHITE,
    );
    let at = p + egui::vec2(14.0, 6.0);
    let r = Rect::from_min_size(at, text.size()).expand(4.0);
    painter.rect_filled(r, 4.0, egui::Color32::from_rgb(40, 70, 120));
    painter.galley(at, text, egui::Color32::WHITE);
}

/// A bin item let go over a track becomes a clip (or a pause) there, at the
/// time under the pointer, on a track of its kind.
pub(super) fn dropped(
    ui: &Ui,
    ctx: &Ctx,
    geo: Geo,
    bars: &[(TrackId, Rect)],
    actions: &mut Vec<Action>,
) {
    let Some(what) = egui::DragAndDrop::payload::<BinItem>(ui.ctx()) else {
        return;
    };
    let Some(p) = ui.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    let Some(&(track, bar)) = bars.iter().find(|(_, r)| r.contains(p)) else {
        return;
    };
    let at = geo.t(p.x).max(0.0);
    // Where it would go, while it is held there.
    ui.painter().vline(
        p.x,
        bar.y_range(),
        egui::Stroke::new(2.0, super::super::PLAYHEAD),
    );
    if !ui.input(|i| i.pointer.any_released()) || !ctx.editable {
        return;
    }
    egui::DragAndDrop::clear_payload(ui.ctx());
    let key = ctx.key;
    let made = match &*what {
        BinItem::Pause => TimelineAction::AddControl {
            key,
            action: ControlAction::TimelinePause,
            at,
        },
        BinItem::New(ty) => TimelineAction::AddClip {
            key,
            ty: *ty,
            file: None,
            track: keys::fitting_track(ctx, *ty, track),
            at,
        },
        BinItem::File(ty, f) => TimelineAction::AddClip {
            key,
            ty: *ty,
            file: Some(f.clone()),
            track: keys::fitting_track(ctx, *ty, track),
            at,
        },
    };
    actions.push(Action::Timeline(made));
}
