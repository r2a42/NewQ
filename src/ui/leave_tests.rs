//! The questions before leaving the show, clicked with the mouse through the
//! whole window in both interfaces, the engine ticked between the frames as
//! the app does: nothing waits for the answer, and nothing stops before it.

use super::*;
use egui::{Event, PointerButton, RawInput, Rect, vec2};

const SIZE: egui::Vec2 = vec2(1280.0, 760.0);

/// The whole window, with a long wait cue to play.
struct Win {
    ctx: egui::Context,
    t: f64,
    show: Show,
    e: Engine,
    st: UiState,
    wait: CueId,
}

impl Win {
    fn new(modern: bool) -> Win {
        let mut show = Show::default();
        let mut c = show.make_cue(CueType::Wait, None, 0);
        if let CueKind::Wait(w) = &mut c.kind {
            w.duration = 600.0;
        }
        c.name = "Long wait".into();
        let wait = c.id;
        show.cues.push(c);
        let mut st = UiState::default();
        st.prefs.new_layout = modern;
        let mut w = Win {
            ctx: egui::Context::default(),
            t: 1.0,
            show,
            e: Engine::for_tests(),
            st,
            wait,
        };
        for _ in 0..3 {
            w.frame(vec![Event::PointerGone]);
        }
        w
    }

    /// One frame of the main window, then a pass of the engine (0.1 s).
    fn frame(&mut self, events: Vec<Event>) -> egui::FullOutput {
        self.t += 0.05;
        self.ctx
            .data_mut(|d| d.remove::<Vec<(String, Rect)>>(egui::Id::new("menu_items")));
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, SIZE)),
            time: Some(self.t),
            events,
            ..Default::default()
        };
        let (show, e, st) = (&mut self.show, &mut self.e, &mut self.st);
        let mut out = self
            .ctx
            .run_ui(input, |ui| super::super::draw(ui, show, e, st));
        out.textures_delta.clear();
        self.e.tick(&self.show, 0.1);
        out
    }

    /// Where `path` ("File ▸ Quit", "Question ▸ Cancel") was drawn last frame.
    fn at(&mut self, path: &str) -> Rect {
        // (Twice: a window just put up is laid out unseen in its first frame.)
        self.frame(vec![]);
        self.frame(vec![]);
        let mut items: Vec<(String, Rect)> = self
            .ctx
            .data(|d| d.get_temp::<Vec<(String, Rect)>>(egui::Id::new("menu_items")))
            .unwrap_or_default();
        items.extend(
            self.ctx
                .data(|d| d.get_temp::<Vec<(String, Rect)>>(egui::Id::new("question_buttons")))
                .unwrap_or_default(),
        );
        items
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, r)| *r)
            .unwrap_or_else(|| panic!("no {path} among {items:?}"))
    }

    fn click(&mut self, path: &str) {
        let p = self.at(path).center();
        let b = |pressed| Event::PointerButton {
            pos: p,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        self.frame(vec![Event::PointerMoved(p)]);
        self.frame(vec![b(true)]);
        self.frame(vec![b(false)]);
        self.frame(vec![Event::PointerGone]);
    }

    /// File ▸ Quit, with the mouse.
    fn quit_from_the_menu(&mut self) {
        self.click("File");
        self.click("File ▸ Quit");
    }

    fn key(&mut self, key: Key, modifiers: egui::Modifiers) {
        let k = |pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        };
        self.frame(vec![k(true)]);
        self.frame(vec![k(false)]);
    }

    fn words(&mut self) -> Vec<String> {
        let out = self.frame(vec![]);
        out.shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::epaint::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect()
    }

    /// How far the wait cue has got, if it plays.
    fn elapsed(&self) -> Option<f64> {
        match self.e.status(self.wait) {
            crate::engine::CueStatus::Playing { elapsed, .. } => Some(elapsed),
            _ => None,
        }
    }

    fn play(&mut self) {
        self.e.fire(&self.show, self.wait);
        self.frame(vec![]);
        assert!(self.elapsed().is_some());
    }

    /// An edit the window will notice: the show has unsaved changes.
    fn edit(&mut self) {
        self.show.name = "Edited".into();
        self.st.json_stale = true;
        self.frame(vec![]);
        assert!(self.st.dirty);
    }

    /// A few frames with the question up: the engine went on all the while.
    fn plays_on_behind(&mut self, what: &str) {
        let before = self.elapsed().expect("the cue plays");
        for _ in 0..10 {
            self.frame(vec![]);
        }
        let after = self.elapsed().expect("the cue still plays");
        assert!(after >= before + 0.9, "{what}: {before} then {after}");
        assert!(self.st.question.is_some() && !self.st.quit_now, "{what}");
    }
}

fn both(test: impl Fn(Win, &str)) {
    for modern in [false, true] {
        test(Win::new(modern), if modern { "new" } else { "classic" });
    }
}

#[test]
fn a_quit_while_cues_play_asks_and_stops_nothing_until_it_is_confirmed() {
    both(|mut w, ui| {
        w.play();
        w.quit_from_the_menu();
        assert_eq!(w.st.question, Some(Question::Playing), "{ui}");
        let words = w.words();
        assert!(
            words
                .iter()
                .any(|t| t == "Cues are playing. Quit and stop them?"),
            "{ui}: {words:?}"
        );
        w.plays_on_behind(ui);
        w.click("Question ▸ Quit and stop them");
        // Saved: nothing more to ask. The app stops the cues and ends.
        assert!(w.st.quit_now, "{ui}");
        assert!(w.st.question.is_none(), "{ui}");
    });
}

#[test]
fn cancel_and_esc_leave_everything_playing() {
    both(|mut w, ui| {
        w.play();
        w.quit_from_the_menu();
        w.click("Question ▸ Keep going");
        assert!(w.st.question.is_none() && !w.st.quit_now, "{ui}");
        let at = w.elapsed().expect("still playing");
        w.frame(vec![]);
        assert!(w.elapsed().unwrap() > at, "{ui}: and going on");
        // Esc answers Cancel; it isn't also the panic key (which fades
        // everything out).
        w.st.quit_requested = true;
        w.frame(vec![]);
        assert_eq!(w.st.question, Some(Question::Playing), "{ui}");
        w.key(Key::Escape, egui::Modifiers::NONE);
        assert!(w.st.question.is_none() && !w.st.quit_now, "{ui}");
        assert!(w.elapsed().is_some(), "{ui}");
        assert!(
            w.e.active.iter().all(|a| !a.stopping),
            "{ui}: nothing fades out"
        );
    });
}

#[test]
fn the_close_button_and_ctrl_q_ask_the_same() {
    both(|mut w, ui| {
        w.play();
        // The main window's close button (Alt+F4 too) sets this.
        w.st.quit_requested = true;
        w.frame(vec![]);
        assert_eq!(w.st.question, Some(Question::Playing), "{ui}");
        w.click("Question ▸ Keep going");
        w.key(Key::Q, egui::Modifiers::COMMAND);
        assert_eq!(w.st.question, Some(Question::Playing), "{ui}: Ctrl+Q");
        w.click("Question ▸ Keep going");
        // Show mode asks even with nothing playing.
        w.e.stop_all(0.0);
        w.frame(vec![]);
        w.st.show_mode = true;
        w.quit_from_the_menu();
        assert_eq!(w.st.question, Some(Question::Playing), "{ui}: Show mode");
        assert!(
            w.words().iter().any(|t| t.starts_with("Show mode is on")),
            "{ui}"
        );
        w.click("Question ▸ Quit");
        assert!(w.st.quit_now, "{ui}");
    });
}

#[test]
fn a_saved_show_with_nothing_playing_quits_without_a_question() {
    both(|mut w, ui| {
        // Even with a file dialog open for something else.
        let (_release, wait) = std::sync::mpsc::channel::<Option<PathBuf>>();
        start_dialog_with(&mut w.st, &w.show, DialogFor::ExportCueSheet, move || {
            wait.recv().ok().flatten()
        });
        w.quit_from_the_menu();
        assert!(w.st.quit_now && w.st.question.is_none(), "{ui}");
    });
}

#[test]
fn unsaved_changes_are_asked_about_while_the_show_plays_on() {
    both(|mut w, ui| {
        w.edit();
        w.play();
        w.quit_from_the_menu();
        w.click("Question ▸ Quit and stop them");
        assert_eq!(
            w.st.question,
            Some(Question::Unsaved(leave::Leave::Quit)),
            "{ui}"
        );
        assert!(
            w.words()
                .iter()
                .any(|t| t == "Save the changes to this show before quitting?"),
            "{ui}"
        );
        w.plays_on_behind(ui);
        w.click("Question ▸ Cancel");
        assert!(
            w.st.question.is_none() && !w.st.quit_now && w.st.dirty,
            "{ui}"
        );
        w.quit_from_the_menu();
        w.click("Question ▸ Quit and stop them");
        w.click("Question ▸ Don't save");
        assert!(w.st.quit_now, "{ui}");
    });
}

/// Each save waits on its thread until `release` is sent (a slow disk).
fn slow_disk(st: &mut UiState) -> std::sync::mpsc::Sender<()> {
    let (release, wait) = std::sync::mpsc::channel::<()>();
    let wait = std::sync::Mutex::new(wait);
    st.save_hold = Some(std::sync::Arc::new(move || {
        let _ = wait.lock().map(|w| w.recv());
    }));
    release
}

fn save_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("newq_leave_{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn save_then_quit_waits_for_the_save_without_holding_the_show_up() {
    both(|mut w, ui| {
        let d = save_dir(ui);
        let path = d.join("a.newq");
        w.st.show_path = Some(path.clone());
        let release = slow_disk(&mut w.st);
        w.edit();
        w.play();
        w.quit_from_the_menu();
        w.click("Question ▸ Quit and stop them");
        w.click("Question ▸ Save");
        assert_eq!(
            w.st.question,
            Some(Question::Saving(leave::Leave::Quit)),
            "{ui}"
        );
        // The disk is slow: the window and the show go on meanwhile.
        w.plays_on_behind(ui);
        release.send(()).unwrap();
        let t0 = std::time::Instant::now();
        while !w.st.quit_now {
            assert!(t0.elapsed().as_secs() < 10, "{ui}: never quit");
            std::thread::sleep(std::time::Duration::from_millis(5));
            w.frame(vec![]);
        }
        assert_eq!(Show::load(&path).unwrap().name, "Edited", "{ui}");
        let _ = std::fs::remove_dir_all(&d);
    });
}

#[test]
fn a_save_that_fails_does_not_quit() {
    both(|mut w, ui| {
        let d = save_dir(&format!("{ui}_fails"));
        w.st.show_path = Some(d.join("no/such/folder/a.newq"));
        w.edit();
        w.quit_from_the_menu();
        w.click("Question ▸ Save");
        let t0 = std::time::Instant::now();
        while w.st.save_failed.is_none() {
            assert!(t0.elapsed().as_secs() < 10, "{ui}: the save never ended");
            std::thread::sleep(std::time::Duration::from_millis(5));
            w.frame(vec![]);
        }
        w.frame(vec![]);
        // Said in the window, and asked again: nothing was quit.
        assert!(!w.st.quit_now, "{ui}");
        assert_eq!(
            w.st.question,
            Some(Question::Unsaved(leave::Leave::Quit)),
            "{ui}"
        );
        assert!(w.words().iter().any(|t| t == "Save failed"), "{ui}");
        w.click("Question ▸ Cancel");
        w.click("Question ▸ OK");
        assert!(w.st.save_failed.is_none(), "{ui}");
        let _ = std::fs::remove_dir_all(&d);
    });
}

#[test]
fn a_quit_while_a_save_is_still_being_written_waits_for_it() {
    both(|mut w, ui| {
        let d = save_dir(&format!("{ui}_written"));
        let path = d.join("a.newq");
        let release = slow_disk(&mut w.st);
        w.edit();
        apply(
            Action::SaveShowTo(path.clone()),
            &mut w.show,
            &mut w.e,
            &mut w.st,
        );
        w.quit_from_the_menu();
        assert_eq!(
            w.st.question,
            Some(Question::Saving(leave::Leave::Quit)),
            "{ui}"
        );
        release.send(()).unwrap();
        let t0 = std::time::Instant::now();
        while !w.st.quit_now {
            assert!(t0.elapsed().as_secs() < 10, "{ui}: never quit");
            std::thread::sleep(std::time::Duration::from_millis(5));
            w.frame(vec![]);
        }
        // Written, so nothing was left to ask.
        assert_eq!(Show::load(&path).unwrap().name, "Edited", "{ui}");
        let _ = std::fs::remove_dir_all(&d);
    });
}

#[test]
fn new_with_unsaved_changes_asks_in_the_window_then_starts_the_new_show() {
    both(|mut w, ui| {
        w.edit();
        w.click("File");
        w.click("File ▸ New show");
        assert_eq!(
            w.st.question,
            Some(Question::Unsaved(leave::Leave::New)),
            "{ui}"
        );
        assert_eq!(w.show.name, "Edited", "{ui}: nothing left yet");
        w.click("Question ▸ Don't save");
        assert!(w.st.question.is_none() && !w.st.quit_now, "{ui}");
        assert!(w.show.cues.is_empty(), "{ui}: the new show");
    });
}

#[test]
fn a_show_dropped_with_unsaved_changes_asks_first() {
    let mut w = Win::new(false);
    let d = save_dir("dropped");
    let other = d.join("other.newq");
    Show {
        name: "Other".into(),
        ..Show::default()
    }
    .save(&other)
    .unwrap();
    w.edit();
    apply(
        Action::AddFiles(vec![other.clone()]),
        &mut w.show,
        &mut w.e,
        &mut w.st,
    );
    assert_eq!(
        w.st.question,
        Some(Question::Unsaved(leave::Leave::OpenFile(other)))
    );
    w.click("Question ▸ Don't save");
    assert_eq!(w.show.name, "Other", "{:?}", w.e.messages);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_crashed_sessions_changes_are_offered_in_the_window() {
    both(|mut w, ui| {
        let d = save_dir(&format!("{ui}_recover"));
        let rec = d.join("recovery.newq");
        Show {
            name: "Recovered".into(),
            ..Show::default()
        }
        .save(&rec)
        .unwrap();
        w.play();
        w.st.question = Some(Question::Recover(rec.clone()));
        // The autosave keeps off the copy while it is offered (`App`), a
        // test doesn't start over it, and a panic doesn't drop it.
        assert!(w.st.offering_recovery(), "{ui}");
        assert!(w.st.test_refusal_base().is_some(), "{ui}");
        w.st.after_panic();
        assert!(w.st.offering_recovery(), "{ui}");
        w.plays_on_behind(ui);
        // Editing keys wait for the answer; the show's keys don't.
        let cues = w.show.cues.len();
        w.st.selected = Some(w.wait);
        w.key(Key::D, egui::Modifiers::COMMAND);
        assert_eq!(
            w.show.cues.len(),
            cues,
            "{ui}: no duplicate behind the question"
        );
        w.click("Question ▸ Restore them");
        assert_eq!(w.show.name, "Recovered", "{ui}");
        assert!(w.st.question.is_none(), "{ui}");
        w.frame(vec![]);
        assert!(w.st.dirty, "{ui}: the restored changes aren't saved");
        let _ = std::fs::remove_dir_all(&d);
    });
}

#[test]
fn a_pop_out_window_shows_the_question_too() {
    let mut w = Win::new(false);
    w.play();
    w.st.question = Some(Question::Playing);
    let ctx = egui::Context::default();
    let mut t = 0.0;
    let mut frame = |w: &mut Win, events: Vec<Event>| {
        t += 0.05;
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 600.0))),
            time: Some(t),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            draw_popout(ui, Pane::Active, &mut w.show, &mut w.e, &mut w.st)
        });
        out.textures_delta.clear();
    };
    for _ in 0..3 {
        frame(&mut w, vec![]);
    }
    let at = ctx
        .data(|d| d.get_temp::<Vec<(String, Rect)>>(egui::Id::new("question_buttons")))
        .unwrap_or_default()
        .into_iter()
        .find(|(p, _)| p == "Question ▸ Keep going")
        .expect("the question in the pop-out")
        .1
        .center();
    frame(&mut w, vec![Event::PointerMoved(at)]);
    frame(
        &mut w,
        vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        }],
    );
    frame(
        &mut w,
        vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }],
    );
    // Answered there, acted on by the main window's next frame.
    w.frame(vec![]);
    assert!(w.st.question.is_none() && !w.st.quit_now);
    assert!(w.elapsed().is_some());
}

/// Where a system dialog (`rfd`) may be shown, and why that is not on the
/// main thread while the show runs. A dialog anywhere else waits on the
/// thread that ticks the engine: every picture holds its frame and every
/// fade and wait stands still until someone answers it.
const DIALOGS_ALLOWED_IN: [(&str, &str, &str); 9] = [
    (
        "src/ui.rs",
        "start_dialog",
        "its `ask` runs on start_dialog_with's thread",
    ),
    ("src/ui.rs", "start_slide_dialog", "on a file-dialog thread"),
    ("src/ui.rs", "start_deck_dialog", "on a file-dialog thread"),
    ("src/ui.rs", "media_dialog", "builds one, shows nothing"),
    ("src/ui.rs", "show_dialog", "builds one, shows nothing"),
    (
        "src/ui/timeline/keys.rs",
        "pick_file",
        "on a file-dialog thread",
    ),
    // The exceptions: nothing plays yet, or NewQ is ending.
    (
        "src/app.rs",
        "open_main_window",
        "NewQ could not start, and exits",
    ),
    (
        "src/crash.rs",
        "install_in",
        "a fatal panic, before NewQ ends",
    ),
    (
        "src/main.rs",
        "hold_node_folder",
        "before anything runs: another NewQ is open",
    ),
];

/// What shows a system dialog, or makes one.
fn is_dialog(line: &str) -> bool {
    let shows = [
        concat!("rfd", "::"),
        concat!(".pick_", "file"),
        concat!(".pick_", "folder"),
        concat!(".save_", "file("),
    ];
    shows.iter().any(|s| line.contains(s))
}

/// The function a line starts, if it starts one.
fn fn_name(line: &str) -> Option<&str> {
    let t = line.trim_start();
    let t = t
        .strip_prefix("pub(super) ")
        .or_else(|| t.strip_prefix("pub(crate) "))
        .or_else(|| t.strip_prefix("pub "))
        .unwrap_or(t);
    let t = t.strip_prefix("fn ")?;
    Some(&t[..t.find(['(', '<'])?])
}

fn rust_files(dir: &Path, into: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let p = entry.path();
        if p.is_dir() {
            rust_files(&p, into);
        } else if p.extension().is_some_and(|e| e == "rs") {
            into.push(p);
        }
    }
}

#[test]
fn no_system_dialog_is_shown_on_the_thread_that_runs_the_show() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    let mut found = Vec::new();
    let mut stray = Vec::new();
    for f in files {
        let name = f
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if name.ends_with("_tests.rs") || name.ends_with("/tests.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap().replace("\r\n", "\n");
        let mut in_fn = "";
        for (n, line) in text.lines().enumerate() {
            // A file's own tests come last.
            if line.starts_with("mod tests {") {
                break;
            }
            if let Some(f) = fn_name(line) {
                in_fn = f;
            }
            if !is_dialog(line) || line.trim_start().starts_with("//") {
                continue;
            }
            if DIALOGS_ALLOWED_IN
                .iter()
                .any(|(file, func, _)| *file == name && *func == in_fn)
            {
                found.push((name.clone(), in_fn.to_string()));
            } else {
                stray.push(format!("{name}:{}: in {in_fn}: {}", n + 1, line.trim()));
            }
        }
    }
    assert!(
        stray.is_empty(),
        "a system dialog outside the dialog threads (ask in the window, `ui::leave`, \
         or open it with `start_dialog`):\n{}",
        stray.join("\n")
    );
    // Each allowed place still shows one (else the list is out of date).
    for (file, func, _) in DIALOGS_ALLOWED_IN {
        assert!(
            found.iter().any(|(f, g)| f == file && g == func),
            "{file} {func} shows no dialog any more: take it off the list"
        );
    }
}
