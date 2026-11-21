//! The menu bar, drawn in both interfaces, and the Window and Settings menus
//! the new interface's strip and the tests reach too.

use super::*;

/// The Window menu: which panes are showing, and which are in a window of
/// their own.
pub(super) fn window_menu(ui: &mut Ui, st: &UiState, actions: &mut Vec<Action>) {
    let mut modern = st.prefs.new_layout;
    if ui
        .checkbox(&mut modern, "New interface")
        .on_hover_text(
            "Editor, Operator and Patch views in place of the panes. The same show; \
             switch back any time.",
        )
        .changed()
    {
        actions.push(Action::SetNewLayout(modern));
        ui.close();
    }
    ui.separator();
    let mut stats = st.stats_open;
    if ui
        .checkbox(&mut stats, "Statistics")
        .on_hover_text("A window of live figures: memory, processor, frame rate, slowest frame")
        .changed()
    {
        actions.push(Action::ToggleStats);
        ui.close();
    }
    ui.separator();
    // Every kind, with the other Preview panes after the first.
    let mut panes: Vec<Pane> = Vec::new();
    for p in Pane::ALL {
        panes.push(p);
        if p == crate::dock::PREVIEW {
            let more = st.layout.preview_panes();
            panes.extend(more.into_iter().filter(|n| *n != 0).map(Pane::Preview));
        }
    }
    for pane in panes {
        // The new interface has the timeline editor as a panel of its Editor
        // view, not as a pane, and the workspace is no pane either.
        let workspace = timeline::workspace::chosen(st);
        if pane == Pane::Timeline && (st.prefs.new_layout || workspace) {
            let mut shown = st.timeline.open;
            if ui
                .checkbox(&mut shown, pane.title())
                .on_hover_text(match (workspace, st.prefs.new_layout) {
                    (true, true) => "The timeline workspace, as the Timeline view",
                    (true, false) => "The timeline workspace, in the panes' place",
                    _ => "The timeline editor, under the cue list in the Editor view",
                })
                .changed()
            {
                actions.push(Action::Timeline(if shown {
                    timeline::TimelineAction::Open(None)
                } else {
                    timeline::TimelineAction::Close
                }));
                ui.close();
            }
            timeline_layout_choice(ui, st, actions);
            continue;
        }
        let open = st.layout.is_open(pane);
        let popped = st.layout.is_popped_out(pane);
        ui.horizontal(|ui| {
            let mut shown = open;
            if ui.checkbox(&mut shown, pane.title()).changed() {
                actions.push(Action::TogglePane(pane));
                ui.close();
            }
            if open {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (label, tip, action) = if popped {
                        (
                            "Dock back",
                            "Put it back in the main window",
                            Action::DockBack(pane),
                        )
                    } else {
                        (
                            "Pop out",
                            "Move it to a window of its own",
                            Action::PopOut(pane),
                        )
                    };
                    if ui.small_button(label).on_hover_text(tip).clicked() {
                        actions.push(action);
                        ui.close();
                    }
                });
            }
        });
        if pane == Pane::Timeline {
            timeline_layout_choice(ui, st, actions);
        }
    }
    if ui
        .button("New preview")
        .on_hover_text(
            "One more Preview, with its own mode (one display, the multiview or the 3D stage) and display",
        )
        .clicked()
    {
        actions.push(Action::NewPreview);
        ui.close();
    }
    ui.separator();
    ui.label(
        RichText::new(
            "Drag a tab to move it; drag the line between panes to resize them. Double-click \
             a tab to pop it out.",
        )
        .weak()
        .small(),
    );
    if ui.button("Reset layout").clicked() {
        actions.push(Action::ResetLayout);
        ui.close();
    }
}

/// Under the Window menu's Timeline: where the timeline editor opens on this
/// computer (as `settings::TIMELINE_EDITOR`).
fn timeline_layout_choice(ui: &mut Ui, st: &UiState, actions: &mut Vec<Action>) {
    use timeline::TimelineLayout as L;
    let pane = if st.prefs.new_layout {
        "as a panel under the cue list"
    } else {
        "as a pane"
    };
    for (layout, words, tip) in [
        (
            L::Pane,
            pane,
            "The timeline's tracks beside the cue list and the other panes",
        ),
        (
            L::Workspace,
            "as a workspace",
            "A bin, the picture, the inspector and the tracks across the whole window, in \
             place of the cue list until Back to cue list",
        ),
    ] {
        let r = ui
            .indent(("timeline_layout", layout == L::Pane), |ui| {
                ui.radio(st.prefs.timeline_layout == layout, words)
                    .on_hover_text(tip)
            })
            .inner;
        if r.clicked() && st.prefs.timeline_layout != layout {
            actions.push(Action::Timeline(timeline::TimelineAction::SetLayout(
                layout,
            )));
            ui.close();
        }
    }
    // And how its view follows a running playhead (as the same section).
    use timeline::TimelineFollow as F;
    ui.indent("timeline_follow_label", |ui| {
        ui.label(RichText::new("While it runs, the view").weak())
    });
    for (follow, words, tip) in [
        (
            F::Page,
            "turns the page",
            "Turns the page as the playhead nears the edge",
        ),
        (
            F::Centre,
            "keeps the playhead centred",
            "Keeps the playhead in the middle",
        ),
    ] {
        let r = ui
            .indent(("timeline_follow", follow == F::Page), |ui| {
                ui.radio(st.prefs.timeline_follow == follow, words)
                    .on_hover_text(tip)
            })
            .inner;
        #[cfg(test)]
        ui.data_mut(|d| {
            d.get_temp_mut_or_default::<Vec<(bool, egui::Rect)>>(egui::Id::new(
                "window_timeline_follow",
            ))
            .push((follow == F::Centre, r.rect))
        });
        if r.clicked() && st.prefs.timeline_follow != follow {
            actions.push(Action::Timeline(timeline::TimelineAction::SetFollow(
                follow,
            )));
            ui.close();
        }
    }
}

/// The Settings menu: the same pages as the sidebar, in the same groups.
fn settings_menu(ui: &mut Ui, st: &UiState, actions: &mut Vec<Action>) {
    let label = if st.settings_open() {
        "Close settings"
    } else {
        "Open settings…"
    };
    if ui
        .add(egui::Button::new(label).shortcut_text("Ctrl+,"))
        .clicked()
    {
        actions.push(Action::ToggleSettings);
        ui.close();
    }
    for group in settings::SettingsGroup::ALL {
        ui.separator();
        ui.label(
            RichText::new(group.label())
                .small()
                .strong()
                .color(Color32::from_gray(130)),
        );
        for page in SettingsPage::ALL.into_iter().filter(|p| p.group() == group) {
            let on = st.settings_open() && st.settings_page == page;
            if ui.selectable_label(on, page.label()).clicked() {
                actions.push(Action::OpenSettings(page));
                ui.close();
            }
        }
        // As the sidebar: what is saved in the show, what stays here.
        settings::group_caption(ui, group);
    }
}

pub(super) fn menu_bar(ui: &mut Ui, show: &Show, st: &UiState, actions: &mut Vec<Action>) {
    egui::containers::menu::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("File", |ui| {
            if ui
                .add(egui::Button::new("New show").shortcut_text("Ctrl+N"))
                .clicked()
            {
                actions.push(Action::New);
            }
            if ui
                .add(egui::Button::new("Open…").shortcut_text("Ctrl+O"))
                .clicked()
            {
                actions.push(Action::Open);
            }
            if ui
                .add(egui::Button::new("Save").shortcut_text("Ctrl+S"))
                .clicked()
            {
                actions.push(Action::Save);
            }
            if ui
                .add(egui::Button::new("Save as…").shortcut_text("Ctrl+Shift+S"))
                .clicked()
            {
                actions.push(Action::SaveAs);
            }
            ui.separator();
            if ui.button(MEDIA_MANAGER_ITEM.label).clicked() {
                actions.push(Action::ToggleMedia);
            }
            if ui
                .button("Sync…")
                .on_hover_text("Line the sound up with the picture.")
                .clicked()
            {
                actions.push(Action::OpenSettings(SettingsPage::Sync));
                ui.close();
            }
            if ui
                .button("Export cue sheet…")
                .on_hover_text(
                    "The cue list as a web page to print, or as a CSV for a spreadsheet.",
                )
                .clicked()
            {
                actions.push(Action::AskFile(DialogFor::ExportCueSheet));
            }
            ui.separator();
            if ui.button("Quit").clicked() {
                actions.push(Action::Quit);
            }
        });
        ui.menu_button("Edit", |ui| {
            if ui
                .add_enabled(
                    st.history.can_undo(),
                    egui::Button::new("Undo").shortcut_text("Ctrl+Z"),
                )
                .clicked()
            {
                actions.push(Action::Undo);
            }
            if ui
                .add_enabled(
                    st.history.can_redo(),
                    egui::Button::new("Redo").shortcut_text("Ctrl+Y"),
                )
                .clicked()
            {
                actions.push(Action::Redo);
            }
            ui.separator();
            let has_cue = st.selected.is_some();
            if ui
                .add_enabled(has_cue, egui::Button::new("Copy cue").shortcut_text("Ctrl+C"))
                .clicked()
            {
                actions.push(Action::Copy);
                ui.close();
            }
            if ui
                .add_enabled(
                    has_cue && !st.show_mode,
                    egui::Button::new("Cut cue").shortcut_text("Ctrl+X"),
                )
                .clicked()
            {
                actions.push(Action::Cut);
                ui.close();
            }
            if ui
                .add_enabled(
                    !st.clipboard.is_empty() && !st.show_mode,
                    egui::Button::new("Paste cue").shortcut_text("Ctrl+V"),
                )
                .clicked()
            {
                actions.push(Action::Paste);
                ui.close();
            }
            ui.separator();
            if ui
                .add(egui::Button::new("Find cues").shortcut_text("Ctrl+F"))
                .clicked()
            {
                actions.push(Action::FocusFind);
                ui.close();
            }
            if ui
                .add_enabled(!st.show_mode, egui::Button::new("Renumber cues"))
                .on_hover_text(
                    "Number the cues 1, 2, 3 down the list (a group's children 3.1, 3.2)",
                )
                .clicked()
            {
                actions.push(Action::Renumber);
                ui.close();
            }
            if ui
                .add(
                    egui::Button::new(if st.show_mode {
                        "Edit mode…"
                    } else {
                        "Show mode"
                    })
                    .shortcut_text("Ctrl+Shift+L"),
                )
                .on_hover_text(
                    "In show mode nothing in the show can be edited by accident: GO, \
                     stopping, pausing and the rest of running a show still work. Leaving it \
                     asks first.",
                )
                .clicked()
            {
                actions.push(Action::ToggleShowMode);
                ui.close();
            }
        });
        ui.menu_button("Cue", |ui| {
            for ty in [CueType::Video, CueType::Image, CueType::Audio] {
                if ui
                    .button(format!("{} Add {} cue…", ty.icon(), ty.label()))
                    .clicked()
                {
                    actions.push(Action::AddCue(ty));
                }
            }
            if ui.button("🖼 Add Slide show…").clicked() {
                actions.push(Action::AskSlides(None));
            }
            if ui
                .button("📊 Add Slide show from PowerPoint…")
                .on_hover_text(
                    "Takes the slides of a PowerPoint, Keynote, OpenDocument or PDF file as pictures, with the file's own timing. A PDF (Save as PDF in any presentation program) needs no other program.",
                )
                .clicked()
            {
                actions.push(Action::AskDeck(None));
            }
            ui.separator();
            for ty in [
                CueType::Camera,
                CueType::LiveAudio,
                CueType::Control,
                CueType::Send,
                CueType::Timecode,
                CueType::Text,
                CueType::Shape,
                CueType::Clock,
                CueType::Timer,
                CueType::Fade,
                CueType::Stop,
                CueType::Wait,
                CueType::Group,
            ] {
                if ui
                    .button(format!("{} Add {} cue", ty.icon(), ty.label()))
                    .clicked()
                {
                    actions.push(Action::AddCue(ty));
                }
            }
            ui.separator();
            if ui
                .add_enabled(!st.show_mode, egui::Button::new("New cue list"))
                .on_hover_text(
                    "Another list of cues with a playhead of its own, in a tab above the cue list",
                )
                .clicked()
            {
                actions.push(Action::AddList);
            }
            if ui
                .add_enabled(!st.show_mode, egui::Button::new("🎞 New timeline"))
                .on_hover_text(
                    "A cue list of the timeline type, in a tab of its own: its cues are clips \
                     on tracks, on one clock, and GO runs it",
                )
                .clicked()
            {
                actions.push(Action::Timeline(timeline::TimelineAction::NewTimelineList));
            }
            if ui
                .button(if st.layout.is_open(Pane::Cart) {
                    "Hide cart"
                } else {
                    "Show cart"
                })
                .clicked()
            {
                actions.push(Action::TogglePane(Pane::Cart));
            }
            if let Some(id) = st.selected
                && let Some(c) = show.cue(id)
            {
                if ui
                    .add(
                        egui::Button::new(if c.armed { "Disarm" } else { "Arm" })
                            .shortcut_text("A"),
                    )
                    .clicked()
                {
                    actions.push(Action::SetArmed(id, !c.armed));
                }
                match (&c.kind, c.parent) {
                    (CueKind::Group(_), _) => {
                        if ui.button("Ungroup").clicked() {
                            actions.push(Action::Ungroup(id));
                        }
                    }
                    (_, None) => {
                        if ui.button("Wrap in group").clicked() {
                            actions.push(Action::GroupCue(id));
                        }
                    }
                    (_, Some(_)) => {
                        if ui.button("Move out of group").clicked() {
                            actions.push(Action::MoveOutOfGroup(id));
                        }
                    }
                }
            }
            ui.separator();
            if ui
                .add(egui::Button::new("Duplicate").shortcut_text("Ctrl+D"))
                .clicked()
            {
                actions.push(Action::Duplicate);
            }
            if ui
                .add(egui::Button::new("Move up").shortcut_text("Ctrl+↑"))
                .clicked()
            {
                actions.push(Action::Move(-1));
            }
            if ui
                .add(egui::Button::new("Move down").shortcut_text("Ctrl+↓"))
                .clicked()
            {
                actions.push(Action::Move(1));
            }
            if ui
                .add(egui::Button::new("Delete").shortcut_text("Del"))
                .clicked()
            {
                actions.push(Action::Delete);
            }
        });
        ui.menu_button("Displays", |ui| {
            if ui.button("Add display").clicked() {
                actions.push(Action::AddDisplay);
            }
            if ui
                .button("Add blended projectors")
                .on_hover_text(
                    "Two projectors that overlap and share one wide picture, with the \
                     overlap faded so it doesn't show",
                )
                .clicked()
            {
                actions.push(Action::AddBlendedPair);
            }
            ui.menu_button("Add sub display on", |ui| {
                add_sub_display_menu(ui, show, actions);
            });
        });
        ui.menu_button("Window", |ui| window_menu(ui, st, actions));
        ui.menu_button("Settings", |ui| settings_menu(ui, st, actions));
        ui.menu_button("Help", |ui| {
            if ui.button("About NewQ…").clicked() {
                actions.push(Action::ToggleAbout);
                ui.close();
            }
            if ui.button(TEST_MODE_ITEM.label).clicked() {
                actions.push(Action::ToggleTestMode);
                ui.close();
            }
            ui.separator();
            ui.label(RichText::new("Keyboard").strong());
            for (k, d) in [
                ("Space", "GO (a timeline: play / pause)"),
                ("0", "GO"),
                ("Esc", "Fade out all (press twice: stop now)"),
                ("P", "Pause / resume"),
                ("A", "Arm / disarm the selected cue"),
                ("↑ / ↓", "Move selection and playhead"),
                ("Ctrl+↑ / Ctrl+↓", "Reorder cue"),
                ("Ctrl+D", "Duplicate cue"),
                ("Del", "Delete cue"),
            ] {
                ui.horizontal(|ui| {
                    ui.monospace(format!("{k:<16}"));
                    ui.label(d);
                });
            }
            ui.separator();
            ui.label("Drag media files onto the window to add cues.");
            ui.label(format!("{} cues", show.cues.len()));
        });
    });
}
