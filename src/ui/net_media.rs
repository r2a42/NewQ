//! The media manager's parts of the UI (`net::media`): the media rows of the
//! Show network page (every computer's progress, its versions setting and a
//! check of its files), the primary's status-bar notice (R16), a follower's
//! own progress, the cue list's mark on a cue whose file hasn't reached every
//! computer, and the library folders on the Media page (R15, R19). All from
//! what the manager last published (`UiState::media`): nothing here waits.

use super::settings::{hint, section, warning};
use super::*;
use crate::net::hub::{NetView, PeerView};
use crate::net::media::manager::{CueLack, MediaReport, PolicySetting, Side, size_words};
use crate::net::media::words;
use crate::net::node::NodeId;
use crate::net::proto::Msg;

/// For the tests: where a control was drawn (shares the network page's list).
#[cfg(test)]
fn note(ui: &Ui, name: impl Into<String>, rect: egui::Rect) {
    let name = name.into();
    let rect = if ui.clip_rect().contains_rect(rect) {
        rect
    } else {
        egui::Rect::NOTHING
    };
    ui.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<(String, egui::Rect)>>(egui::Id::new("net_rects"))
            .push((name, rect))
    });
}

#[cfg(not(test))]
fn note(_: &Ui, _: impl Into<String>, _: egui::Rect) {}

/// The primary's media section, at the top of its list of computers: all
/// of them together, then what the primary itself serves.
pub fn primary_summary(ui: &mut Ui, v: &NetView, st: &UiState) {
    let m = &st.media;
    if m.side != Side::Primary {
        return;
    }
    section(ui, "Media on every computer", |ui| {
        let ours = v.peers.iter().filter(|p| p.ours).count();
        match &m.sum {
            Some(sum) => {
                let line = words::progress(sum);
                let color = if words::busy(sum) { PREWAIT } else { GO_GREEN };
                let r = ui.label(RichText::new(format!("All together: {line}")).color(color));
                note(ui, "media_all", r.rect);
            }
            None if ours == 0 => {
                ui.label(RichText::new("No other computer is in the show yet.").weak());
            }
            None => {
                ui.label(RichText::new("Waiting for the computers to report.").weak());
            }
        }
        match (&m.hashing, m.served) {
            (Some(h), _) => {
                let pct = if h.bytes_total == 0 {
                    0.0
                } else {
                    h.bytes_done as f64 * 100.0 / h.bytes_total as f64
                };
                ui.label(
                    RichText::new(format!(
                        "Listing the media: {} of {} files · {pct:.0} % read",
                        h.files_done, h.files_total
                    ))
                    .small(),
                );
            }
            (None, Some((files, bytes))) => {
                ui.label(
                    RichText::new(format!(
                        "Sent from here: {files} file{} ({})",
                        if files == 1 { "" } else { "s" },
                        size_words(bytes)
                    ))
                    .small()
                    .weak(),
                );
            }
            (None, None) => {}
        }
        if m.missing_here > 0 {
            warning(
                ui,
                &format!(
                    "⚠ {} file{} the show uses {} missing on this computer, so no computer \
                     can have {}: Settings ▸ Media ▸ media manager.",
                    m.missing_here,
                    if m.missing_here == 1 { "" } else { "s" },
                    if m.missing_here == 1 { "is" } else { "are" },
                    if m.missing_here == 1 { "it" } else { "them" },
                ),
            );
        }
        if let Some(p) = &m.problem {
            warning(ui, &format!("⚠ {p}"));
        }
        hint(
            ui,
            "Every computer gets every file of the show, the ones nearest the playhead first, \
             then the library folders (Settings ▸ Media). A computer plays what it has: where a \
             file hasn't arrived, its display shows a card in the cue's place until it has.",
        );
    });
}

/// A computer's media, under its row on the primary's page.
pub fn computer_rows(ui: &mut Ui, p: &PeerView, st: &mut UiState, actions: &mut Vec<Action>) {
    if st.media.side != Side::Primary || !p.connected {
        return;
    }
    let Some(r) = p.status.as_ref().and_then(|s| s.media_progress.as_deref()) else {
        ui.label(
            RichText::new("Media: waiting for its report")
                .small()
                .weak(),
        );
        return;
    };
    report_lines(ui, r, &format!("media:{}", p.id));
    ui.horizontal_wrapped(|ui| {
        policy_control(ui, p.id, &r.policy, actions);
        let r2 = ui.small_button("Verify").on_hover_text(
            "Read every file it holds again and check it; a damaged one is copied again.",
        );
        note(ui, format!("verify:{}", p.id), r2.rect);
        if r2.clicked() {
            actions.push(Action::MediaTo(p.id, Msg::MediaVerify));
        }
    });
}

/// The lines of one computer's report.
fn report_lines(ui: &mut Ui, r: &MediaReport, id: &str) {
    let busy = words::busy(&r.progress);
    let line = format!("Media: {}", words::progress(&r.progress));
    let resp = ui.label(
        RichText::new(line)
            .small()
            .color(if busy { PREWAIT } else { GO_GREEN }),
    );
    note(ui, id, resp.rect);
    let mut facts = Vec::new();
    if !r.plan.is_empty() {
        facts.push(r.plan.clone());
    }
    if let Some(f) = r.store_free {
        facts.push(super::network::space(f));
    }
    if !facts.is_empty() {
        ui.label(RichText::new(facts.join(" · ")).small().weak());
    }
    if !r.not_found.is_empty() {
        let names: Vec<String> = r.not_found.iter().map(|p| words::file_name(p)).collect();
        warning(
            ui,
            &format!(
                "⚠ Not found on any computer: {}",
                names.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
                    + if names.len() > 5 { "…" } else { "" }
            ),
        );
    }
    if let Some(v) = &r.verify {
        ui.label(RichText::new(v).small());
    }
    if let Some(p) = &r.problem {
        warning(ui, &format!("⚠ {p}"));
    }
}

/// "Keeps versions / Replaces": the computer's setting, changed from here and
/// kept on that computer.
fn policy_control(ui: &mut Ui, id: NodeId, p: &PolicySetting, actions: &mut Vec<Action>) {
    let mut next = p.clone();
    let mode = if p.replace_all {
        2
    } else if p.replace_over.is_some() {
        1
    } else {
        0
    };
    let mut chosen = mode;
    let labels = ["Keeps versions", "Replaces very large files", "Replaces"];
    let r = egui::ComboBox::from_id_salt(("media_policy", id.0))
        .selected_text(labels[mode])
        .width(170.0)
        .show_ui(ui, |ui| {
            for (i, l) in labels.iter().enumerate() {
                let r = ui.selectable_label(mode == i, *l);
                note(ui, format!("policy:{id}:{i}"), r.rect);
                if r.clicked() {
                    chosen = i;
                }
            }
        })
        .response
        .on_hover_text(
            "Keeps versions: an older version of a changed file stays, so going back needs no \
             copying. Replaces: only the newest is kept, for a computer short of room.",
        );
    note(ui, format!("policy_menu:{id}"), r.rect);
    match chosen {
        0 => {
            next.replace_all = false;
            next.replace_over = None;
        }
        1 => {
            next.replace_all = false;
            next.replace_over = next.replace_over.or(Some(4_000_000_000));
        }
        _ => next.replace_all = true,
    }
    if !next.replace_all {
        ui.label(RichText::new("keep").small());
        let mut keep = next.keep.max(1);
        let r = ui.add(egui::DragValue::new(&mut keep).range(1..=50));
        note(ui, format!("policy_keep:{id}"), r.rect);
        if r.changed() {
            next.keep = keep;
        }
    }
    if let Some(over) = next.replace_over.filter(|_| !next.replace_all) {
        ui.label(RichText::new("over").small());
        let mut gb = words::gb(over);
        let r = ui.add(
            egui::DragValue::new(&mut gb)
                .range(0.1..=10_000.0)
                .speed(0.1)
                .suffix(" GB"),
        );
        if r.changed() {
            next.replace_over = Some((gb * 1e9) as u64);
        }
    }
    if next != *p {
        actions.push(Action::MediaTo(
            id,
            Msg::MediaPolicy {
                keep: next.keep.max(1),
                replace_all: next.replace_all,
                replace_over: next.replace_over,
            },
        ));
    }
}

/// On a computer following a primary: its own media (on its Show network page).
pub fn own(ui: &mut Ui, st: &UiState) {
    if st.media.side != Side::Follower {
        return;
    }
    section(ui, "Media on this computer", |ui| {
        match &st.media.own {
            Some(r) => {
                report_lines(ui, r, "media_own");
                ui.label(RichText::new(r.policy.words()).small().weak());
            }
            None => {
                ui.label(RichText::new("Opening the media store…").weak());
            }
        }
        hint(
            ui,
            "The primary sends every file of the show here, nearest the playhead first, and \
             decides how many versions this computer keeps.",
        );
    });
}

/// The status bar: on the primary while anything transfers (R16), on a
/// follower its own progress.
pub fn status(ui: &mut Ui, st: &UiState) {
    let m = &st.media;
    match m.side {
        Side::Primary => {
            if let Some(text) = m.notice() {
                let r = ui
                    .label(RichText::new(format!("↓ {text}")).color(PREWAIT))
                    .on_hover_text(
                        "The other computers are being sent the show's media. Each plays what it \
                         has; a display whose file hasn't arrived shows a card in the cue's place. \
                         Copying uses the disks and the network, so playback may be less smooth \
                         until it is done. Settings ▸ Show network has each computer's progress.",
                    );
                note(ui, "media_notice", r.rect);
            }
        }
        Side::Follower => {
            if let Some(r) = &m.own
                && words::busy(&r.progress)
            {
                let r = ui
                    .label(
                        RichText::new(format!("↓ Media: {}", words::progress(&r.progress)))
                            .color(PREWAIT),
                    )
                    .on_hover_text("The show's media is being copied to this computer.");
                note(ui, "media_notice", r.rect);
            }
        }
        Side::Off => {}
    }
}

/// The cue list's mark for a cue whose file hasn't reached every computer:
/// whether a computer that shows the cue's display lacks it (the stronger
/// mark), and the hover text.
pub fn cue_mark(show: &Show, st: &UiState, c: &Cue) -> Option<(bool, String)> {
    if st.media.side != Side::Primary {
        return None;
    }
    let m = c.kind.file_media()?;
    if m.file.as_os_str().is_empty() {
        return None;
    }
    let lacks: &Vec<CueLack> = st.media.lacks(&m.file)?;
    if lacks.is_empty() {
        return None;
    }
    let shows_it: Vec<NodeId> = m
        .displays(show)
        .iter()
        .filter_map(|d| show.outputs.iter().find(|o| o.id == *d))
        .filter_map(|o| o.computer)
        .collect();
    let strong = lacks.iter().any(|l| shows_it.contains(&l.node));
    let mut tip = String::from("Not on every computer yet:");
    for l in lacks {
        let what = if l.percent < 0.0 {
            "not found".to_string()
        } else {
            format!("{:.0} %", l.percent.floor())
        };
        let shows = if shows_it.contains(&l.node) {
            " (shows this cue's display)"
        } else {
            ""
        };
        tip.push_str(&format!("\n{}: {what}{shows}", l.name));
    }
    Some((strong, tip))
}

/// The Media page's library: the folders whose media goes to every computer
/// whether a cue uses it or not.
pub fn library(ui: &mut Ui, show: &mut Show, st: &UiState, actions: &mut Vec<Action>) {
    section(ui, "Folders to keep on every computer", |ui| {
        let mut remove = None;
        for (i, dir) in show.library_folders.iter().enumerate() {
            let view = st.media.folders.iter().find(|f| &f.path == dir);
            ui.horizontal(|ui| {
                ui.label(RichText::new(dir.display().to_string()).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let r = ui.small_button("Remove");
                    note(ui, format!("library_remove:{i}"), r.rect);
                    if r.clicked() {
                        remove = Some(i);
                    }
                });
            });
            let line = match view {
                Some(v) if v.listed => format!(
                    "{} file{} · {}",
                    v.files,
                    if v.files == 1 { "" } else { "s" },
                    size_words(v.bytes)
                ),
                _ if st.media.side == Side::Primary => match &st.media.hashing {
                    Some(h) => format!(
                        "Being listed: {} of {} files read",
                        h.files_done, h.files_total
                    ),
                    None => "Listed once the show network runs it".into(),
                },
                _ => "Listed when this computer runs a show network".into(),
            };
            let r = ui.label(RichText::new(line).small().weak());
            note(ui, format!("library_folder:{i}"), r.rect);
        }
        if let Some(i) = remove {
            show.library_folders.remove(i);
        }
        if show.library_folders.is_empty() {
            ui.label(RichText::new("No folders yet.").weak());
        }
        let r = ui.button("Add folder…").on_hover_text(
            "A folder of content you may use: in a show network it is copied to every \
             computer after the files the show uses, so a file is already everywhere when you \
             reach for it.",
        );
        note(ui, "library_add", r.rect);
        if r.clicked() {
            actions.push(Action::AskFile(DialogFor::LibraryFolder));
        }
        if let Some(h) = st.media.hashing.as_ref().filter(|h| h.bytes_total > 0) {
            ui.add(
                egui::ProgressBar::new(h.bytes_done as f32 / h.bytes_total.max(1) as f32)
                    .desired_width(240.0)
                    .text(format!(
                        "reading {} of {} files",
                        h.files_done, h.files_total
                    )),
            );
        }
        hint(
            ui,
            "In a show network, every computer gets the show's files first, nearest the \
             playhead first, then these folders, so the content is in place at load-in. A \
             folder is read once; NewQ remembers what it read, so a restart doesn't read it \
             again.",
        );
        let mut cards = !show.no_transfer_cards;
        let r = ui.checkbox(&mut cards, "Show transfer cards on the displays");
        note(ui, "transfer_cards", r.rect);
        if r.changed() {
            show.no_transfer_cards = !cards;
        }
        hint(
            ui,
            "Where a cue's file hasn't reached a computer yet, its display shows \"Media \
             transferring\" in the cue's place. Off: black instead (for a show in front of an \
             audience).",
        );
    });
}

/// The missing-media manager's note in a show network: a file missing here
/// is missing everywhere.
pub fn missing_note(ui: &mut Ui, st: &UiState, missing: usize) {
    if st.media.side == Side::Primary && missing > 0 {
        warning(
            ui,
            "This computer runs the show network: a file missing here can't reach any other \
             computer either. Their displays show \"Media not found\" in its place until it is \
             found here.",
        );
    }
}
