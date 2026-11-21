//! The menu bar clicked through with the mouse at 1280×760: every item the
//! menus had before 0.27's reshuffle still asks for the same thing, from its
//! new place; what moved away is gone; both interfaces have the same menus.

use super::*;
use egui::{Event, PointerButton, Pos2, RawInput, Rect, vec2};

const SIZE: egui::Vec2 = vec2(1280.0, 760.0);

/// The menu bar alone, clicked with the mouse; what it asks for is kept.
struct Bar {
    ctx: egui::Context,
    t: f64,
    show: Show,
    st: UiState,
    actions: Vec<Action>,
}

impl Bar {
    fn new(modern: bool) -> Bar {
        let mut show = Show::default();
        let mut e = Engine::for_tests();
        let mut st = UiState::default();
        st.prefs.new_layout = modern;
        apply(Action::AddCue(CueType::Text), &mut show, &mut e, &mut st);
        st.selected = show.cues.first().map(|c| c.id);
        assert!(st.selected.is_some());
        Bar {
            ctx: egui::Context::default(),
            t: 1.0,
            show,
            st,
            actions: Vec::new(),
        }
    }

    fn run(&mut self, events: Vec<Event>) -> egui::FullOutput {
        self.t += 0.05;
        self.ctx
            .data_mut(|d| d.remove::<Vec<(String, Rect)>>(egui::Id::new("menu_items")));
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)),
            time: Some(self.t),
            events,
            ..Default::default()
        };
        let (show, st, actions) = (&self.show, &self.st, &mut self.actions);
        let mut out = self.ctx.run_ui(input, |ui| {
            egui::Panel::top("menu").show(ui, |ui| menu_bar(ui, show, st, actions));
            egui::CentralPanel::default().show(ui, |_| {});
        });
        out.textures_delta.clear();
        out
    }

    fn frame(&mut self, events: Vec<Event>) -> Vec<(String, Rect)> {
        self.run(events);
        self.ctx
            .data(|d| d.get_temp::<Vec<(String, Rect)>>(egui::Id::new("menu_items")))
            .unwrap_or_default()
    }

    fn find(&mut self, path: &str) -> Rect {
        let items = self.frame(vec![]);
        items
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, r)| *r)
            .unwrap_or_else(|| {
                panic!(
                    "no {path} among {:?}",
                    items.iter().map(|i| &i.0).collect::<Vec<_>>()
                )
            })
    }

    fn hover(&mut self, at: Rect) {
        self.frame(vec![Event::PointerMoved(at.center())]);
        self.frame(vec![]);
    }

    fn click(&mut self, at: Rect) {
        let p = at.center();
        let b = |pressed| Event::PointerButton {
            pos: p,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        self.frame(vec![Event::PointerMoved(p)]);
        self.frame(vec![b(true)]);
        self.frame(vec![b(false)]);
        self.frame(vec![]);
    }

    /// Nothing open: a click on the empty window.
    fn close_all(&mut self) {
        self.click(Rect::from_center_size(
            Pos2::new(640.0, 700.0),
            vec2(1.0, 1.0),
        ));
    }

    /// Opens the top menu, goes through its submenus (hovered) and clicks
    /// the item: what that asked for.
    fn reach(&mut self, path: &str) -> Vec<Action> {
        self.close_all();
        self.actions.clear();
        let parts: Vec<&str> = path.split(" ▸ ").collect();
        let top = self.find(parts[0]);
        self.click(top);
        for sub in [&["Cue", "Add"][..], &["Window", "Timeline opens"][..]] {
            if parts.len() > sub.len() && parts[..sub.len()] == *sub {
                let r = self.find(&sub.join(" ▸ "));
                self.hover(r);
            }
        }
        let r = self.find(path);
        self.click(r);
        std::mem::take(&mut self.actions)
    }
}

fn texts(out: &egui::FullOutput) -> Vec<String> {
    out.shapes
        .iter()
        .filter_map(|c| match &c.shape {
            egui::epaint::Shape::Text(t) => Some(t.galley.text().to_string()),
            _ => None,
        })
        .collect()
}

type Wanted = fn(&Action) -> bool;

/// Every item the old menus had (0.26), and where it is now: a menu item
/// clicked here, or (None) a page that has it, tested there.
fn moved() -> Vec<(&'static str, Option<&'static str>, Wanted)> {
    use timeline::TimelineAction as T;
    vec![
        ("File ▸ New show", Some("File ▸ New show"), |a| {
            matches!(a, Action::New)
        }),
        ("File ▸ Open…", Some("File ▸ Open…"), |a| {
            matches!(a, Action::Open)
        }),
        ("File ▸ Save", Some("File ▸ Save"), |a| {
            matches!(a, Action::Save)
        }),
        ("File ▸ Save as…", Some("File ▸ Save as…"), |a| {
            matches!(a, Action::SaveAs)
        }),
        (
            "File ▸ Media manager…",
            Some("Tools ▸ Media manager…"),
            |a| matches!(a, Action::ToggleMedia),
        ),
        ("File ▸ Sync…", Some("Settings ▸ Sync"), |a| {
            matches!(a, Action::OpenSettings(SettingsPage::Sync))
        }),
        (
            "File ▸ Export cue sheet…",
            Some("File ▸ Export cue sheet…"),
            |a| matches!(a, Action::AskFile(DialogFor::ExportCueSheet)),
        ),
        ("File ▸ Quit", Some("File ▸ Quit"), |a| {
            matches!(a, Action::Quit)
        }),
        ("Edit ▸ Copy cue", Some("Edit ▸ Copy cue"), |a| {
            matches!(a, Action::Copy)
        }),
        ("Edit ▸ Cut cue", Some("Edit ▸ Cut cue"), |a| {
            matches!(a, Action::Cut)
        }),
        ("Edit ▸ Find cues", Some("Edit ▸ Find cues"), |a| {
            matches!(a, Action::FocusFind)
        }),
        (
            "Edit ▸ Renumber cues",
            Some("Edit ▸ Renumber cues"),
            |a| matches!(a, Action::Renumber),
        ),
        ("Edit ▸ Show mode", Some("Edit ▸ Show mode"), |a| {
            matches!(a, Action::ToggleShowMode)
        }),
        (
            "Cue ▸ Add Video cue…",
            Some("Cue ▸ Add ▸ Video…"),
            |a| matches!(a, Action::AddCue(CueType::Video)),
        ),
        (
            "Cue ▸ Add Image cue…",
            Some("Cue ▸ Add ▸ Image…"),
            |a| matches!(a, Action::AddCue(CueType::Image)),
        ),
        (
            "Cue ▸ Add Audio cue…",
            Some("Cue ▸ Add ▸ Audio…"),
            |a| matches!(a, Action::AddCue(CueType::Audio)),
        ),
        (
            "Cue ▸ Add Slide show…",
            Some("Cue ▸ Add ▸ Slide show…"),
            |a| matches!(a, Action::AskSlides(None)),
        ),
        (
            "Cue ▸ Add Slide show from PowerPoint…",
            Some("Cue ▸ Add ▸ Slides from a presentation…"),
            |a| matches!(a, Action::AskDeck(None)),
        ),
        (
            "Cue ▸ Add Camera cue",
            Some("Cue ▸ Add ▸ Camera"),
            |a| matches!(a, Action::AddCue(CueType::Camera)),
        ),
        (
            "Cue ▸ Add Live audio cue",
            Some("Cue ▸ Add ▸ Live audio"),
            |a| matches!(a, Action::AddCue(CueType::LiveAudio)),
        ),
        (
            "Cue ▸ Add Control cue",
            Some("Cue ▸ Add ▸ Control"),
            |a| matches!(a, Action::AddCue(CueType::Control)),
        ),
        ("Cue ▸ Add Send cue", Some("Cue ▸ Add ▸ Send"), |a| {
            matches!(a, Action::AddCue(CueType::Send))
        }),
        (
            "Cue ▸ Add Timecode cue",
            Some("Cue ▸ Add ▸ Timecode"),
            |a| matches!(a, Action::AddCue(CueType::Timecode)),
        ),
        ("Cue ▸ Add Text cue", Some("Cue ▸ Add ▸ Text"), |a| {
            matches!(a, Action::AddCue(CueType::Text))
        }),
        (
            "Cue ▸ Add Shape cue",
            Some("Cue ▸ Add ▸ Shape"),
            |a| matches!(a, Action::AddCue(CueType::Shape)),
        ),
        (
            "Cue ▸ Add Clock cue",
            Some("Cue ▸ Add ▸ Clock"),
            |a| matches!(a, Action::AddCue(CueType::Clock)),
        ),
        (
            "Cue ▸ Add Timer cue",
            Some("Cue ▸ Add ▸ Timer"),
            |a| matches!(a, Action::AddCue(CueType::Timer)),
        ),
        ("Cue ▸ Add Fade cue", Some("Cue ▸ Add ▸ Fade"), |a| {
            matches!(a, Action::AddCue(CueType::Fade))
        }),
        ("Cue ▸ Add Stop cue", Some("Cue ▸ Add ▸ Stop"), |a| {
            matches!(a, Action::AddCue(CueType::Stop))
        }),
        ("Cue ▸ Add Wait cue", Some("Cue ▸ Add ▸ Wait"), |a| {
            matches!(a, Action::AddCue(CueType::Wait))
        }),
        (
            "Cue ▸ Add Group cue",
            Some("Cue ▸ Add ▸ Group"),
            |a| matches!(a, Action::AddCue(CueType::Group)),
        ),
        ("Cue ▸ New cue list", Some("Cue ▸ New cue list"), |a| {
            matches!(a, Action::AddList)
        }),
        (
            "Cue ▸ New timeline",
            Some("Cue ▸ 🎞 New timeline"),
            |a| matches!(a, Action::Timeline(T::NewTimelineList)),
        ),
        ("Cue ▸ Show cart", Some("Window ▸ Cart"), |a| {
            matches!(a, Action::TogglePane(Pane::Cart))
        }),
        ("Cue ▸ Disarm", Some("Cue ▸ Disarm"), |a| {
            matches!(a, Action::SetArmed(_, false))
        }),
        (
            "Cue ▸ Wrap in group",
            Some("Cue ▸ Wrap in group"),
            |a| matches!(a, Action::GroupCue(_)),
        ),
        ("Cue ▸ Duplicate", Some("Cue ▸ Duplicate"), |a| {
            matches!(a, Action::Duplicate)
        }),
        ("Cue ▸ Move up", Some("Cue ▸ Move up"), |a| {
            matches!(a, Action::Move(-1))
        }),
        ("Cue ▸ Move down", Some("Cue ▸ Move down"), |a| {
            matches!(a, Action::Move(1))
        }),
        ("Cue ▸ Delete", Some("Cue ▸ Delete"), |a| {
            matches!(a, Action::Delete)
        }),
        // On the Displays and Sub displays pages' "+ Add", clicked by the
        // tests at the end of this file.
        ("Displays ▸ Add display", None, |a| {
            matches!(a, Action::AddDisplay)
        }),
        ("Displays ▸ Add blended projectors", None, |a| {
            matches!(a, Action::AddBlendedPair)
        }),
        ("Displays ▸ Add sub display on", None, |a| {
            matches!(a, Action::AddSubDisplay(_))
        }),
        (
            "Window ▸ New interface",
            Some("Window ▸ Interface ▸ New"),
            |a| matches!(a, Action::SetNewLayout(true)),
        ),
        ("Window ▸ Statistics", Some("Tools ▸ Statistics"), |a| {
            matches!(a, Action::ToggleStats)
        }),
        ("Window ▸ Inspector", Some("Window ▸ Inspector"), |a| {
            matches!(a, Action::TogglePane(Pane::Inspector))
        }),
        (
            "Window ▸ Cue list ▸ Pop out",
            Some("Window ▸ Cue list ▸ Pop out"),
            |a| matches!(a, Action::PopOut(Pane::Cues)),
        ),
        (
            "Window ▸ Timeline ▸ as a workspace",
            Some("Window ▸ Timeline opens ▸ as a workspace"),
            |a| {
                matches!(
                    a,
                    Action::Timeline(T::SetLayout(timeline::TimelineLayout::Workspace))
                )
            },
        ),
        // Settings ▸ Interface ▸ Timeline editor (`timeline/tests/follow.rs`).
        (
            "Window ▸ Timeline ▸ keeps the playhead centred",
            None,
            |a| matches!(a, Action::Timeline(T::SetFollow(_))),
        ),
        (
            "Window ▸ New preview",
            Some("Window ▸ New preview"),
            |a| matches!(a, Action::NewPreview),
        ),
        (
            "Window ▸ Reset layout",
            Some("Window ▸ Reset layout"),
            |a| matches!(a, Action::ResetLayout),
        ),
        (
            "Settings ▸ Open settings…",
            Some("Settings ▸ Open settings…"),
            |a| matches!(a, Action::ToggleSettings),
        ),
        (
            "Settings ▸ This computer",
            Some("Settings ▸ This computer"),
            |a| matches!(a, Action::OpenSettings(SettingsPage::ThisComputer)),
        ),
        (
            "Help ▸ About NewQ…",
            Some("Help ▸ About NewQ…"),
            |a| matches!(a, Action::ToggleAbout),
        ),
        (
            "Help ▸ Test Mode…",
            Some("Tools ▸ Test Mode…"),
            |a| matches!(a, Action::ToggleTestMode),
        ),
        (
            "Help ▸ the keys",
            Some("Help ▸ Keyboard shortcuts…"),
            |a| matches!(a, Action::ToggleKeys),
        ),
        (
            "(new) Open the log folder",
            Some("Help ▸ Open the log folder"),
            |a| matches!(a, Action::OpenLogFolder),
        ),
    ]
}

/// Where an item of the classic Window menu is in the new interface's: the
/// panes have no dock to go in there, so their ticks, Pop out and Reset
/// layout are the classic interface's alone (the Editor view has the cue
/// list, inspector and what is playing), and a new preview is a window.
fn in_new_interface(path: &'static str) -> Option<&'static str> {
    match path {
        "Window ▸ Inspector" | "Window ▸ Cue list ▸ Pop out" | "Window ▸ Reset layout" => {
            None
        }
        "Window ▸ New preview" => Some("Window ▸ New preview window"),
        p => Some(p),
    }
}

#[test]
fn every_old_menu_item_is_still_reached_with_the_mouse() {
    for modern in [false, true] {
        let mut bar = Bar::new(modern);
        for (old, new, wanted) in moved() {
            let Some(new) = new else { continue };
            // The interface switch offers the other one.
            if modern && new == "Window ▸ Interface ▸ New" {
                continue;
            }
            let Some(new) = (if modern {
                in_new_interface(new)
            } else {
                Some(new)
            }) else {
                continue;
            };
            let got = bar.reach(new);
            assert!(
                got.iter().any(wanted),
                "modern {modern}: {old}, now {new}: asked for {} other actions",
                got.len()
            );
        }
    }
}

/// The items shown only when there is something for them to act on.
#[test]
fn items_with_something_to_act_on_are_reached_too() {
    let mut bar = Bar::new(false);
    let mut e = Engine::for_tests();
    let wanted = |bar: &mut Bar, path: &str, f: Wanted| {
        let got = bar.reach(path);
        assert!(got.iter().any(f), "{path}: {} other actions", got.len());
    };
    // Undo, then Redo.
    bar.st.history.reset("{}".into());
    bar.st.history.observe("{\"name\":\"changed\"}", 1.0);
    bar.st.history.end_burst();
    wanted(&mut bar, "Edit ▸ Undo", |a| matches!(a, Action::Undo));
    let _ = bar.st.history.undo();
    wanted(&mut bar, "Edit ▸ Redo", |a| matches!(a, Action::Redo));
    // Paste, with a cue copied.
    bar.st.clipboard = bar.show.copy_cues(&[bar.st.selected.unwrap()]);
    wanted(&mut bar, "Edit ▸ Paste cue", |a| {
        matches!(a, Action::Paste)
    });
    // The text cue wrapped in a group: it moves out, the group ungroups.
    let text = bar.st.selected.unwrap();
    apply(Action::GroupCue(text), &mut bar.show, &mut e, &mut bar.st);
    bar.st.selected = Some(text);
    wanted(&mut bar, "Cue ▸ Move out of group", |a| {
        matches!(a, Action::MoveOutOfGroup(_))
    });
    bar.st.selected = bar.show.cue(text).and_then(|c| c.parent);
    assert!(bar.st.selected.is_some());
    wanted(&mut bar, "Cue ▸ Ungroup", |a| {
        matches!(a, Action::Ungroup(_))
    });
    // A pane in a window of its own docks back.
    apply(
        Action::PopOut(Pane::Inspector),
        &mut bar.show,
        &mut e,
        &mut bar.st,
    );
    assert!(bar.st.layout.is_popped_out(Pane::Inspector));
    wanted(&mut bar, "Window ▸ Inspector ▸ Dock back", |a| {
        matches!(a, Action::DockBack(Pane::Inspector))
    });
}

/// The Timeline tick: a pane in the classic interface, the editor's panel
/// in the new one.
#[test]
fn the_window_menus_timeline_tick_opens_the_timeline_in_both_interfaces() {
    for modern in [false, true] {
        let mut bar = Bar::new(modern);
        let got = bar.reach("Window ▸ Timeline");
        assert!(
            got.iter().any(|a| matches!(
                a,
                Action::TogglePane(Pane::Timeline)
                    | Action::Timeline(timeline::TimelineAction::Open(None))
            )),
            "modern {modern}"
        );
    }
}

#[test]
fn the_menu_bar_is_the_same_seven_menus_in_both_interfaces() {
    for modern in [false, true] {
        let mut bar = Bar::new(modern);
        let tops: Vec<String> = bar
            .frame(vec![])
            .into_iter()
            .map(|(p, _)| p)
            .filter(|p| !p.contains('▸'))
            .collect();
        assert_eq!(
            tops,
            ["File", "Edit", "Cue", "Window", "Tools", "Settings", "Help"],
            "modern {modern}"
        );
        // Gone: the Displays menu, File ▸ Sync…, Cue ▸ Show cart, the
        // follow choice and Statistics in the Window menu, Help's key list.
        for (top, there) in [
            ("File", "Quit"),
            ("Cue", "Add"),
            ("Window", if modern { "Patch" } else { "Reset layout" }),
            ("Help", "About NewQ…"),
        ] {
            bar.close_all();
            let r = bar.find(top);
            bar.click(r);
            let out = bar.run(vec![]);
            let words = texts(&out);
            // The menu is open.
            assert!(words.iter().any(|w| w == there), "modern {modern}: {top}");
            for gone in [
                "Sync…",
                "Show cart",
                "keeps the playhead centred",
                "turns the page",
                "Statistics",
                "Media manager…",
                "Test Mode…",
                "Add display",
                "Reorder cue",
            ] {
                assert!(
                    !words.iter().any(|w| w.contains(gone)),
                    "modern {modern}: {gone} still in {top}"
                );
            }
        }
        // The Window menu has what the interface in use can show: the panes,
        // or the views.
        bar.close_all();
        let r = bar.find("Window");
        bar.click(r);
        let words = texts(&bar.run(vec![]));
        let (here, not_here): (&[&str], &[&str]) = if modern {
            (
                &[
                    "Editor",
                    "Operator",
                    "Patch",
                    "Timeline",
                    "Cart",
                    "New preview window",
                ],
                &[
                    "Cue list",
                    "Inspector",
                    "Active",
                    "Preview",
                    "Reset layout",
                    "Pop out",
                ],
            )
        } else {
            (
                &[
                    "Cue list",
                    "Inspector",
                    "Cart",
                    "Preview",
                    "Timeline",
                    "Reset layout",
                ],
                &["Editor", "Operator", "Patch", "New preview window"],
            )
        };
        for w in here {
            assert!(
                words.iter().any(|t| t == w),
                "modern {modern}: no {w} in {words:?}"
            );
        }
        for w in not_here {
            assert!(
                !words.iter().any(|t| t == w),
                "modern {modern}: {w} in {words:?}"
            );
        }
        // The interface switch: the one in use is chosen.
        let other = if modern { "Classic" } else { "New" };
        let switch = bar.reach(&format!("Window ▸ Interface ▸ {other}"));
        assert!(
            switch
                .iter()
                .any(|a| matches!(a, Action::SetNewLayout(m) if *m == !modern)),
            "modern {modern}"
        );
    }
}

#[test]
fn the_settings_menu_shows_both_groups_and_where_each_is_kept() {
    let mut bar = Bar::new(false);
    let r = bar.find("Settings");
    bar.click(r);
    let words = texts(&bar.run(vec![]));
    for want in [
        "SHOW",
        "REMOTE CONTROL",
        "Everything above is saved in the show file.",
        "THIS COMPUTER",
        "Stays on this computer, whatever show is open.",
        "Interface",
        "This computer",
    ] {
        assert!(words.iter().any(|t| t == want), "{want} not in {words:?}");
    }
    // Every page, and all of it in the window.
    let items = bar.frame(vec![]);
    for page in SettingsPage::ALL {
        let path = format!("Settings ▸ {}", page.label());
        let at = items.iter().find(|(p, _)| *p == path);
        let at = at.unwrap_or_else(|| panic!("{path}")).1;
        assert!(at.max.y < SIZE.y, "{path} at {at:?}");
    }
}

/// The whole window at 1280×760, for what the menus send people to.
fn window_frame(
    ctx: &egui::Context,
    t: &mut f64,
    show: &mut Show,
    e: &mut Engine,
    st: &mut UiState,
) {
    *t += 0.05;
    ctx.data_mut(|d| {
        d.remove::<Vec<(u64, Rect)>>(egui::Id::new("display_frames"));
        d.remove::<Vec<(String, Rect)>>(egui::Id::new("page_adds"));
    });
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)),
        time: Some(*t),
        ..Default::default()
    };
    let mut out = ctx.run_ui(input, |ui| super::super::draw(ui, show, e, st));
    out.textures_delta.clear();
}

/// Clicks a page's "+ Add" (or an item of it) where it was last drawn.
fn page_click(
    ctx: &egui::Context,
    t: &mut f64,
    show: &mut Show,
    e: &mut Engine,
    st: &mut UiState,
    path: &str,
) {
    let adds = ctx
        .data(|d| d.get_temp::<Vec<(String, Rect)>>(egui::Id::new("page_adds")))
        .unwrap_or_default();
    let at = adds
        .iter()
        .find(|(p, _)| p == path)
        .unwrap_or_else(|| panic!("no {path} among {adds:?}"))
        .1
        .center();
    for ev in [
        Event::PointerMoved(at),
        Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        },
        Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        },
    ] {
        *t += 0.05;
        ctx.data_mut(|d| {
            d.remove::<Vec<(u64, Rect)>>(egui::Id::new("display_frames"));
            d.remove::<Vec<(String, Rect)>>(egui::Id::new("page_adds"));
        });
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)),
            time: Some(*t),
            events: vec![ev],
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| super::super::draw(ui, show, e, st));
        out.textures_delta.clear();
    }
    window_frame(ctx, t, show, e, st);
}

#[test]
fn a_sub_display_is_added_from_its_page_with_the_mouse() {
    let ctx = egui::Context::default();
    let mut t = 1.0;
    let mut show = Show::default();
    let mut e = Engine::for_tests();
    let mut st = UiState::default();
    show.add_display();
    let name = show.outputs[0].name.clone();
    apply(
        Action::OpenSettings(SettingsPage::SubDisplays),
        &mut show,
        &mut e,
        &mut st,
    );
    for _ in 0..3 {
        window_frame(&ctx, &mut t, &mut show, &mut e, &mut st);
    }
    page_click(
        &ctx,
        &mut t,
        &mut show,
        &mut e,
        &mut st,
        "Sub displays ▸ + Add",
    );
    page_click(
        &ctx,
        &mut t,
        &mut show,
        &mut e,
        &mut st,
        &format!("Sub displays ▸ + Add ▸ {name}"),
    );
    assert_eq!(
        show.outputs.iter().filter(|o| o.is_sub_display()).count(),
        1
    );
}

#[test]
fn blended_projectors_are_added_from_the_displays_page_with_the_mouse() {
    let ctx = egui::Context::default();
    let mut t = 1.0;
    let mut show = Show::default();
    let mut e = Engine::for_tests();
    let mut st = UiState::default();
    show.add_display();
    let before = show.outputs.len();
    apply(
        Action::OpenSettings(SettingsPage::Displays),
        &mut show,
        &mut e,
        &mut st,
    );
    for _ in 0..3 {
        window_frame(&ctx, &mut t, &mut show, &mut e, &mut st);
    }
    page_click(&ctx, &mut t, &mut show, &mut e, &mut st, "Displays ▸ + Add");
    page_click(
        &ctx,
        &mut t,
        &mut show,
        &mut e,
        &mut st,
        "Displays ▸ + Add ▸ Blended projectors",
    );
    assert_eq!(show.outputs.len(), before + 3);
}

#[test]
fn a_display_added_on_its_page_is_brought_into_sight() {
    for modern in [false, true] {
        let ctx = egui::Context::default();
        let mut t = 1.0;
        let mut show = Show::default();
        let mut e = Engine::for_tests();
        let mut st = UiState::default();
        st.prefs.new_layout = modern;
        for _ in 0..6 {
            show.add_display();
        }
        apply(
            Action::OpenSettings(SettingsPage::Displays),
            &mut show,
            &mut e,
            &mut st,
        );
        for _ in 0..3 {
            window_frame(&ctx, &mut t, &mut show, &mut e, &mut st);
        }
        let frames = |ctx: &egui::Context| {
            ctx.data(|d| d.get_temp::<Vec<(u64, Rect)>>(egui::Id::new("display_frames")))
                .unwrap_or_default()
        };
        let last = show.outputs.last().unwrap().id.0;
        let below = frames(&ctx).iter().find(|f| f.0 == last).unwrap().1;
        assert!(
            below.min.y > SIZE.y,
            "modern {modern}: already in sight {below:?}"
        );
        let before = show.outputs.len();
        page_click(&ctx, &mut t, &mut show, &mut e, &mut st, "Displays ▸ + Add");
        page_click(
            &ctx,
            &mut t,
            &mut show,
            &mut e,
            &mut st,
            "Displays ▸ + Add ▸ Display",
        );
        assert_eq!(show.outputs.len(), before + 1, "modern {modern}");
        let added = show.outputs.last().unwrap().id.0;
        // It scrolls there smoothly, in a fraction of a second.
        for _ in 0..20 {
            window_frame(&ctx, &mut t, &mut show, &mut e, &mut st);
        }
        let at = frames(&ctx).iter().find(|f| f.0 == added).unwrap().1;
        assert!(
            at.min.y > 0.0 && at.max.y < SIZE.y,
            "modern {modern}: the new display at {at:?}"
        );
        assert!(st.reveal_display.get().is_none());
    }
}

/// The whole window at 1280×760 driven with the mouse, for what a menu item
/// changes in it.
struct Win {
    ctx: egui::Context,
    t: f64,
    show: Show,
    e: Engine,
    st: UiState,
}

impl Win {
    /// The new interface on a show with two cues on the cart; `set` puts it
    /// in the state to start from.
    fn new(set: fn(&mut Win)) -> Win {
        let mut show = Show::default();
        for (n, on_cart) in [("Thunder", true), ("Doorbell", true), ("Wait", false)] {
            let mut c = show.make_cue(CueType::Wait, None, show.cues.len());
            c.name = n.into();
            c.cart = on_cart;
            show.cues.push(c);
        }
        let mut st = UiState::default();
        st.prefs.new_layout = true;
        let mut w = Win {
            ctx: egui::Context::default(),
            t: 1.0,
            show,
            e: Engine::for_tests(),
            st,
        };
        set(&mut w);
        for _ in 0..3 {
            w.frame(vec![Event::PointerGone]);
        }
        w
    }

    fn frame(&mut self, events: Vec<Event>) -> egui::FullOutput {
        self.t += 0.05;
        self.ctx
            .data_mut(|d| d.remove::<Vec<(String, Rect)>>(egui::Id::new("menu_items")));
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)),
            time: Some(self.t),
            events,
            ..Default::default()
        };
        let (show, e, st) = (&mut self.show, &mut self.e, &mut self.st);
        let mut out = self
            .ctx
            .run_ui(input, |ui| super::super::draw(ui, show, e, st));
        out.textures_delta.clear();
        out
    }

    fn items(&mut self) -> Vec<(String, Rect)> {
        self.frame(vec![]);
        self.ctx
            .data(|d| d.get_temp::<Vec<(String, Rect)>>(egui::Id::new("menu_items")))
            .unwrap_or_default()
    }

    fn at(&mut self, path: &str) -> Rect {
        let items = self.items();
        items
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, r)| *r)
            .unwrap_or_else(|| panic!("no {path} among {items:?}"))
    }

    fn click(&mut self, at: Rect) {
        let p = at.center();
        let b = |pressed| Event::PointerButton {
            pos: p,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        self.frame(vec![Event::PointerMoved(p)]);
        self.frame(vec![b(true)]);
        self.frame(vec![b(false)]);
        self.frame(vec![]);
    }

    /// The Window menu opened: every item in it, its submenu's too.
    fn window_items(&mut self) -> Vec<String> {
        let r = self.at("Window");
        self.click(r);
        let r = self.at("Window ▸ Timeline opens");
        self.frame(vec![Event::PointerMoved(r.center())]);
        let items = self.items();
        self.frame(vec![Event::PointerGone]);
        let mut paths: Vec<String> = items
            .into_iter()
            .map(|(p, _)| p)
            .filter(|p| p.starts_with("Window ▸ "))
            .collect();
        paths.dedup();
        paths
    }

    /// What the main window shows with the pointer away, and the windows of
    /// their own it has.
    fn drawn(&mut self) -> (Vec<String>, Vec<Pane>) {
        self.frame(vec![Event::PointerGone]);
        let words = texts(&self.frame(vec![]));
        (
            words,
            self.st.layout.popouts.iter().map(|p| p.pane).collect(),
        )
    }

    /// Clicks `path` in the Window menu (through its submenu).
    fn choose(&mut self, path: &str) {
        let r = self.at("Window");
        self.click(r);
        if path.starts_with("Window ▸ Timeline opens ▸ ") {
            let r = self.at("Window ▸ Timeline opens");
            self.frame(vec![Event::PointerMoved(r.center())]);
            self.frame(vec![]);
        }
        let r = self.at(path);
        self.click(r);
    }
}

/// In the new interface the Window menu has no item that does nothing:
/// from each of a few starting points, every item but the ones already
/// chosen (the interface in use, the view showing) changes the window or
/// opens or closes a window of its own.
#[test]
fn every_window_item_in_the_new_interface_changes_what_is_drawn() {
    type Set = fn(&mut Win);
    let starts: [(&str, Set); 4] = [
        ("as it starts", |_| {}),
        ("in Operator with the cart and a preview window", |w| {
            w.st.modern_view = modern::View::Operator;
            w.st.layout.open(Pane::Cart);
            w.st.layout.new_preview_window();
        }),
        ("with the cart in a window and the timeline open", |w| {
            w.st.layout.open(Pane::Cart);
            w.st.layout.pop_out(Pane::Cart);
            let (show, e, st) = (&mut w.show, &mut w.e, &mut w.st);
            apply(
                Action::Timeline(timeline::TimelineAction::Open(None)),
                show,
                e,
                st,
            );
        }),
        ("with the workspace chosen, in Patch", |w| {
            w.st.prefs.timeline_layout = timeline::TimelineLayout::Workspace;
            w.st.modern_view = modern::View::Patch;
        }),
    ];
    let mut tried = std::collections::BTreeSet::new();
    for (start, set) in starts {
        let paths = Win::new(set).window_items();
        for path in paths {
            let mut w = Win::new(set);
            // The ones that show what is chosen, and the submenu's button.
            let layout = w.st.prefs.timeline_layout;
            let chosen = path == "Window ▸ Interface ▸ New"
                || path == "Window ▸ Timeline opens"
                || path == format!("Window ▸ {}", w.st.modern_view.label())
                || (path.ends_with("as a panel under the cue list")
                    && layout == timeline::TimelineLayout::Pane)
                || (path.ends_with("as a workspace")
                    && layout == timeline::TimelineLayout::Workspace);
            if chosen {
                continue;
            }
            let before = w.drawn();
            assert_eq!(before, w.drawn(), "{start}: not still before {path}");
            w.choose(&path);
            let after = w.drawn();
            assert!(before != after, "{start}: {path} changed nothing");
            tried.insert(path);
        }
    }
    for want in [
        "Window ▸ Interface ▸ Classic",
        "Window ▸ Editor",
        "Window ▸ Operator",
        "Window ▸ Patch",
        "Window ▸ Timeline",
        "Window ▸ Cart",
        "Window ▸ Cart ▸ Pop out",
        "Window ▸ Cart ▸ Into the Operator view",
        "Window ▸ New preview window",
        "Window ▸ Timeline opens ▸ as a workspace",
        "Window ▸ Timeline opens ▸ as a panel under the cue list",
    ] {
        assert!(tried.contains(want), "{want} not tried: {tried:?}");
    }
    // A preview in a window of its own is listed, and unticked it closes.
    assert!(
        tried.iter().any(|p| p.starts_with("Window ▸ Preview")),
        "{tried:?}"
    );
}
