//! The questions asked in the window itself before the show is left (quit,
//! New, Open, a show file dropped or handed over) and at start-up (the changes
//! a crashed session left). They are drawn by egui, never a system dialog:
//! the main thread runs the show (`App::pass` ticks the engine, which chooses
//! the video frames, runs fades and pre-waits and sends network heartbeats),
//! so nothing on it may wait for a person. While a question is up the show
//! plays on, the keys still work, and Esc answers Cancel.
//!
//! One question at a time (`UiState::question`); a button only notes its
//! answer (`UiState::answer`), which `settle` acts on at the end of the main
//! window's frame, so it works on a follower (whose actions are dropped) and
//! from a pop-out. A quit that has been agreed to sets `UiState::quit_now`,
//! which the app acts on (`App::render_control`).

use super::*;

/// What was asked for that leaves the show behind.
#[derive(Clone, Debug, PartialEq)]
pub enum Leave {
    Quit,
    New,
    Open,
    /// Open this show file (dropped on the window, handed over by a second
    /// NewQ, or chosen while the show was being edited).
    OpenFile(PathBuf),
}

impl Leave {
    /// "Save the changes to this show before …?"
    fn before(&self) -> &'static str {
        match self {
            Leave::Quit => "quitting",
            Leave::New => "starting a new show",
            Leave::Open | Leave::OpenFile(_) => "opening another show",
        }
    }
}

/// The question up in the window.
#[derive(Clone, Debug, PartialEq)]
pub enum Question {
    /// A quit while cues play or in Show mode: quit and stop them?
    Playing,
    /// The show has changes that aren't saved: save them first, leave them,
    /// or stay?
    Unsaved(Leave),
    /// Waiting for a save before going on with `then`: one already being
    /// written when it was asked for, or the one Save started here (which,
    /// if it fails or its file dialog is cancelled, asks again).
    Saving(Leave),
    /// A session that didn't close properly left changes that weren't saved,
    /// in this recovery copy: restore them?
    Recover(PathBuf),
}

/// What a button of the question said.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Answer {
    /// Quit (and stop what plays), or restore the recovered changes.
    Go,
    Save,
    DontSave,
    Cancel,
}

/// Ask to leave the show for `then`: at once when there is nothing to ask,
/// else by putting the question up. While a question is up, another request
/// is ignored.
pub(super) fn ask(then: Leave, show: &mut Show, engine: &mut Engine, st: &mut UiState) {
    if st.question.is_some() {
        return;
    }
    // During a test the show is the test's, which is put away unsaved and the
    // user's own comes back: nothing to ask about (and a Save would put the
    // test show in a file of the user's). Its cues play, but a quit is asked
    // for by a person in front of a test they started.
    if st.test_locked {
        if then == Leave::Quit {
            st.quit_now = true;
        }
        return;
    }
    if then == Leave::Quit && (engine.is_active() || st.show_mode) {
        st.question = Some(Question::Playing);
        return;
    }
    unsaved_or_go(then, show, engine, st);
}

/// Whether a quit now would ask anything (a Mac's app menu Quits without
/// asking when it wouldn't, `macwin::ask_before_quitting`).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn quit_would_ask(engine: &Engine, st: &UiState) -> bool {
    !st.test_locked
        && (st.question.is_some()
            || engine.is_active()
            || st.show_mode
            || st.dirty
            || st.saving.is_some()
            || st.save_next.is_some())
}

/// The unsaved-changes question, if there are any; else on with `then`.
fn unsaved_or_go(then: Leave, show: &mut Show, engine: &mut Engine, st: &mut UiState) {
    // (A file dialog open for something else doesn't hold a quit up.)
    if st.saving.is_some() || st.save_next.is_some() {
        st.question = Some(Question::Saving(then));
    } else if st.dirty {
        st.question = Some(Question::Unsaved(then));
    } else {
        go(then, show, engine, st);
    }
}

/// A save being written or waiting to be, or a file dialog open (a Save As
/// choosing where to save).
fn saving_or_asking(st: &UiState) -> bool {
    st.saving.is_some() || st.save_next.is_some() || st.dialog.is_some()
}

/// Leave the show as asked: nothing is left to ask.
fn go(then: Leave, show: &mut Show, engine: &mut Engine, st: &mut UiState) {
    st.question = None;
    // Deliberately leaving this show behind (saved or not): nothing to recover.
    recovery::clear(st.show_path.as_deref());
    match then {
        Leave::Quit => st.quit_now = true,
        Leave::New => load_into(Show::default(), None, show, engine, st),
        Leave::Open => start_dialog(st, show, DialogFor::OpenShow),
        Leave::OpenFile(path) => match Show::load(&path) {
            Ok(new) => load_into(new, Some(path), show, engine, st),
            Err(e) => engine.log(format!("Could not open show: {e:#}")),
        },
    }
}

/// Act on the answer given (from any window), and go on once a save the
/// question waits for has finished. At the end of the main window's frame,
/// after `UiState::dirty` is up to date.
pub(super) fn settle(show: &mut Show, engine: &mut Engine, st: &mut UiState) {
    let Some(q) = st.question.clone() else {
        st.answer = None;
        return;
    };
    let answer = st.answer.take();
    match (q, answer) {
        (Question::Recover(rec), Some(Answer::Go)) => {
            st.question = None;
            restore(&rec, show, engine, st);
        }
        (Question::Recover(rec), Some(_)) => {
            st.question = None;
            // Kept, out of the way.
            recovery::set_aside(&rec);
        }
        (_, Some(Answer::Cancel)) => st.question = None,
        (Question::Playing, Some(Answer::Go)) => {
            st.question = None;
            unsaved_or_go(Leave::Quit, show, engine, st);
        }
        (Question::Unsaved(then), Some(Answer::DontSave)) => go(then, show, engine, st),
        (Question::Unsaved(then), Some(Answer::Save)) => {
            match st.show_path.clone() {
                Some(path) => start_save(show, engine, st, path),
                // Where to? On the file dialog's thread; its answer is a
                // `SaveShowTo`, saved as any other.
                None => start_dialog(st, show, DialogFor::SaveShow),
            }
            st.question = Some(Question::Saving(then));
        }
        (Question::Saving(then), None) => {
            if saving_or_asking(st) {
                return;
            }
            // Written as it is now: on. Otherwise the save failed (and said
            // so), its dialog was cancelled, or it was edited meanwhile: ask.
            if serde_json::to_string(show).ok() == st.saved_json {
                go(then, show, engine, st);
            } else {
                st.question = Some(Question::Unsaved(then));
            }
        }
        _ => {}
    }
}

/// Take back the changes a crashed session left in `rec`.
fn restore(rec: &Path, show: &mut Show, engine: &mut Engine, st: &mut UiState) {
    match Show::load(rec) {
        Ok(recovered) => {
            // The show on disk stays the saved one, so the title shows the edits.
            st.set_saved(show);
            *show = recovered;
            engine.forget_lists();
            engine.playhead = show.cues.first().map(|c| c.id);
            st.selected = engine.playhead;
            st.json_stale = true;
        }
        Err(e) => {
            engine.problem(format!("Could not read the recovery file: {e:#}"));
            recovery::set_aside(rec);
        }
    }
}

/// While a question is up, the keys that run the show (GO, the hotkeys,
/// pause, the arrows) still work behind it; those that edit, save or open
/// wait for the answer.
pub(super) fn keys_behind(st: &UiState, actions: &mut Vec<Action>) {
    if st.question.is_some() {
        actions.retain(|a| {
            !a.edits_the_show()
                && !matches!(
                    a,
                    Action::Save | Action::SaveAs | Action::Quit | Action::Copy | Action::FocusFind
                )
        });
    }
}

/// Where the question's buttons were drawn (tests: "Question ▸ label").
fn noted(ui: &Ui, label: &str, r: egui::Response) -> egui::Response {
    #[cfg(test)]
    ui.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<(String, egui::Rect)>>(egui::Id::new("question_buttons"))
            .push((format!("Question ▸ {label}"), r.rect))
    });
    #[cfg(not(test))]
    let _ = (ui, label);
    r
}

/// A button of the question. It never takes the keyboard: Space and Enter
/// stay the show's (GO), and a button with the focus would take them.
fn button(ui: &mut Ui, label: &str, strong: bool) -> bool {
    let text = if strong {
        RichText::new(label).strong()
    } else {
        RichText::new(label)
    };
    let r = ui.add(egui::Button::new(text).sense(egui::Sense::CLICK));
    noted(ui, label, r).clicked()
}

/// Draw the question, if one is up, over the window it is drawn in (the main
/// window and each pop-out). Esc answers Cancel. Before `keyboard`, so that
/// Esc is not also the panic key.
pub(super) fn window(ctx: &egui::Context, engine: &Engine, st: &mut UiState) {
    #[cfg(test)]
    ctx.data_mut(|d| d.remove::<Vec<(String, egui::Rect)>>(egui::Id::new("question_buttons")));
    save_failed_window(ctx, st);
    let Some(q) = st.question.clone() else { return };
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
        st.answer = Some(Answer::Cancel);
    }
    let mut answer = None;
    egui::Modal::new(egui::Id::new("leave_question")).show(ctx, |ui| {
        ui.set_max_width(420.0);
        let (title, said) = match &q {
            Question::Playing if engine.is_active() => (
                "Quit NewQ?",
                "Cues are playing. Quit and stop them?".to_string(),
            ),
            Question::Playing => (
                "Quit NewQ?",
                "Show mode is on. Quit NewQ and close the displays?".to_string(),
            ),
            Question::Unsaved(then) => (
                "Unsaved changes",
                format!("Save the changes to this show before {}?", then.before()),
            ),
            Question::Saving(_) => (
                "Saving…",
                "The show is being written to the disk. NewQ goes on when it has been.".to_string(),
            ),
            Question::Recover(_) => (
                "Recover unsaved changes?",
                "NewQ didn't close properly last time and kept a copy of the changes that \
                 hadn't been saved. Restore them?"
                    .to_string(),
            ),
        };
        ui.heading(title);
        ui.add_space(4.0);
        ui.label(said);
        if engine.is_active() && !matches!(q, Question::Playing) {
            ui.label(RichText::new("The show plays on meanwhile.").weak().small());
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| match &q {
            Question::Playing => {
                let go = if engine.is_active() {
                    "Quit and stop them"
                } else {
                    "Quit"
                };
                if button(ui, go, false) {
                    answer = Some(Answer::Go);
                }
                if button(ui, "Keep going", true) {
                    answer = Some(Answer::Cancel);
                }
            }
            Question::Unsaved(_) => {
                if button(ui, "Save", true) {
                    answer = Some(Answer::Save);
                }
                if button(ui, "Don't save", false) {
                    answer = Some(Answer::DontSave);
                }
                if button(ui, "Cancel", false) {
                    answer = Some(Answer::Cancel);
                }
            }
            Question::Saving(_) => {
                ui.spinner();
                if button(ui, "Cancel", false) {
                    answer = Some(Answer::Cancel);
                }
            }
            Question::Recover(_) => {
                if button(ui, "Restore them", true) {
                    answer = Some(Answer::Go);
                }
                if button(ui, "Not now", false) {
                    answer = Some(Answer::DontSave);
                }
            }
        });
    });
    if answer.is_some() {
        st.answer = answer;
        ctx.request_repaint();
    }
    if matches!(q, Question::Saving(_)) {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

/// A save that failed, said in the window until the operator has read it
/// (`tell_save_failed`). Not modal: the show goes on, and so may the editing.
fn save_failed_window(ctx: &egui::Context, st: &mut UiState) {
    let Some(why) = st.save_failed.clone() else {
        return;
    };
    egui::Window::new("Save failed")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_TOP, Vec2::new(0.0, 80.0))
        .show(ctx, |ui| {
            ui.set_max_width(460.0);
            ui.label("The show could not be saved, so the changes are not on the disk yet:");
            ui.label(RichText::new(why).monospace().small());
            if button(ui, "OK", true) {
                st.save_failed = None;
            }
        });
}

#[cfg(test)]
#[path = "leave_tests.rs"]
mod tests;
