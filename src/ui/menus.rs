//! The menu bar, the same in both interfaces: File, Edit, Cue, Window, Tools,
//! Settings, Help. The menus' names and the items a hint points to are
//! `settings`' `*_MENU` and `MenuItem` constants, so a pointer follows a move.

use super::settings::{
    CUE_MENU, EDIT_MENU, FILE_MENU, HELP_MENU, KEYS_ITEM, SETTINGS_MENU, STATISTICS_ITEM,
    TIMELINE_OPENS_ITEM, TOOLS_MENU, WINDOW_MENU,
};
use super::*;

/// Notes a menu's item and where it is drawn, for the tests that click
/// through the menus (`menu_items`, by "Menu ▸ Item").
fn noted(ui: &Ui, path: &str, r: egui::Response) -> egui::Response {
    #[cfg(test)]
    ui.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<(String, egui::Rect)>>(egui::Id::new("menu_items"))
            .push((path.to_string(), r.rect))
    });
    #[cfg(not(test))]
    let _ = (ui, path);
    r
}

/// A plain item: `label` drawn, noted as "menu ▸ label".
fn item(ui: &mut Ui, menu: &str, label: &str, shortcut: Option<&str>) -> egui::Response {
    item_enabled(ui, true, menu, label, shortcut)
}

fn item_enabled(
    ui: &mut Ui,
    enabled: bool,
    menu: &str,
    label: &str,
    shortcut: Option<&str>,
) -> egui::Response {
    let mut b = egui::Button::new(label);
    if let Some(s) = shortcut {
        b = b.shortcut_text(s);
    }
    let r = ui.add_enabled(enabled, b);
    noted(ui, &format!("{menu} ▸ {label}"), r)
}

/// A small grey heading inside a menu.
fn heading(ui: &mut Ui, words: &str) {
    ui.label(
        RichText::new(words)
            .small()
            .strong()
            .color(Color32::from_gray(130)),
    );
}

/// Every shortcut a menu prints beside an item: the keyboard table
/// (`keys::KEYS`) must have each (a test).
#[cfg(test)]
pub(super) const SHORTCUTS: [&str; 17] = [
    "Ctrl+N",
    "Ctrl+O",
    "Ctrl+S",
    "Ctrl+Shift+S",
    "Ctrl+Z",
    "Ctrl+Y",
    "Ctrl+C",
    "Ctrl+X",
    "Ctrl+V",
    "Ctrl+F",
    "Ctrl+Shift+L",
    "Ctrl+D",
    "Ctrl+↑",
    "Ctrl+↓",
    "Ctrl+,",
    "A",
    "Del",
];

/// The Window menu: which interface, then what that interface can show (the
/// classic one's panes, or the new one's views, timeline, cart and preview
/// windows), and where the timeline editor opens.
fn window_menu(ui: &mut Ui, st: &UiState, actions: &mut Vec<Action>) {
    const W: &str = WINDOW_MENU;
    heading(ui, "Interface");
    for (modern, words, tip) in [
        (
            false,
            "Classic",
            "The panes: cue list, inspector, cart, previews and the rest, docked or in \
             windows of their own",
        ),
        (
            true,
            "New",
            "Editor, Operator and Patch views in place of the panes. The same show; switch \
             back any time.",
        ),
    ] {
        let r = ui
            .radio(st.prefs.new_layout == modern, words)
            .on_hover_text(tip);
        let r = noted(ui, &format!("{W} ▸ Interface ▸ {words}"), r);
        if r.clicked() && st.prefs.new_layout != modern {
            actions.push(Action::SetNewLayout(modern));
            ui.close();
        }
    }
    ui.separator();
    if st.prefs.new_layout {
        new_interface_windows(ui, st, actions);
    } else {
        classic_panes(ui, st, actions);
    }
    ui.separator();
    let r = ui.menu_button(TIMELINE_OPENS_ITEM.label, |ui| {
        timeline_layout_choice(ui, st, actions)
    });
    noted(ui, &TIMELINE_OPENS_ITEM.path(), r.response);
    if !st.prefs.new_layout {
        ui.separator();
        ui.add(
            egui::Label::new(
                RichText::new(
                    "Drag a tab to move it; drag the line between panes to resize them. \
                     Double-click a tab to pop it out.",
                )
                .weak()
                .small(),
            )
            .wrap(),
        );
        if item(ui, W, "Reset layout", None).clicked() {
            actions.push(Action::ResetLayout);
            ui.close();
        }
    }
}

/// The new interface's part of the Window menu: its views (as the strip's
/// tabs), the timeline editor, the cart, and the windows of their own it can
/// have. No pane ticks: there is no dock to put a pane in.
fn new_interface_windows(ui: &mut Ui, st: &UiState, actions: &mut Vec<Action>) {
    const W: &str = WINDOW_MENU;
    let workspace = timeline::workspace::chosen(st);
    heading(ui, "Views");
    for v in modern::View::ALL {
        // As the strip: the Timeline view is there while the editor opens as
        // a workspace.
        if v == modern::View::Timeline && !workspace {
            continue;
        }
        let here = st.modern_view == v && !st.settings_open();
        let r = ui.radio(here, v.label()).on_hover_text(v.hint());
        if noted(ui, &format!("{W} ▸ {}", v.label()), r).clicked() {
            if !here {
                actions.push(Action::ShowView(v));
            }
            ui.close();
        }
    }
    ui.separator();
    // The timeline editor as a panel of the Editor view (with the workspace
    // it is the Timeline view, above).
    if !workspace {
        let title = Pane::Timeline.title();
        let mut shown = st.timeline.open;
        let r = ui
            .checkbox(&mut shown, &title)
            .on_hover_text("The timeline editor, under the cue list in the Editor view");
        if noted(ui, &format!("{W} ▸ {title}"), r).changed() {
            if shown {
                // The panel is in the Editor view: go there to see it.
                if st.modern_view != modern::View::Editor || st.settings_open() {
                    actions.push(Action::ShowView(modern::View::Editor));
                }
                actions.push(Action::Timeline(timeline::TimelineAction::Open(None)));
            } else {
                actions.push(Action::Timeline(timeline::TimelineAction::Close));
            }
            ui.close();
        }
    }
    // The cart: along the foot of the Operator view, or a window of its own.
    let title = Pane::Cart.title();
    let open = st.layout.is_open(Pane::Cart);
    let popped = st.layout.is_popped_out(Pane::Cart);
    ui.horizontal(|ui| {
        let mut shown = open;
        let r = ui.checkbox(&mut shown, &title).on_hover_text(
            "A button for each cue put on the cart, along the foot of the Operator view",
        );
        if noted(ui, &format!("{W} ▸ {title}"), r).changed() {
            if shown && (st.modern_view != modern::View::Operator || st.settings_open()) {
                actions.push(Action::ShowView(modern::View::Operator));
            }
            actions.push(Action::TogglePane(Pane::Cart));
            ui.close();
        }
        if open {
            // The buttons in a column after the names.
            let gap = (ui.min_rect().min.x + 120.0 - ui.cursor().min.x).max(12.0);
            ui.scope(|ui| {
                ui.add_space(gap);
                let (label, tip) = if popped {
                    (
                        "Into the Operator view",
                        "Put it back along the foot of the Operator view",
                    )
                } else {
                    ("Pop out", "Move it to a window of its own")
                };
                let r = ui.small_button(label).on_hover_text(tip);
                if noted(ui, &format!("{W} ▸ {title} ▸ {label}"), r).clicked() {
                    if popped {
                        actions.push(Action::DockBack(Pane::Cart));
                        actions.push(Action::ShowView(modern::View::Operator));
                    } else {
                        actions.push(Action::PopOut(Pane::Cart));
                    }
                    ui.close();
                }
            });
        }
    });
    let r = item(ui, W, NEW_PREVIEW_WINDOW, None).on_hover_text(
        "A Preview in a window of its own, with its own mode (one display, the multiview or \
         the 3D stage) and display",
    );
    if r.clicked() {
        actions.push(Action::NewPreview);
        ui.close();
    }
    // What is in a window of its own now (a pane popped out in the classic
    // interface stays out here too): unticked, its window closes.
    let windows: Vec<Pane> = st
        .layout
        .popouts
        .iter()
        .map(|p| p.pane)
        .filter(|p| *p != Pane::Cart)
        .collect();
    if !windows.is_empty() {
        ui.separator();
        heading(ui, "In windows of their own");
        for pane in windows {
            let title = pane.title();
            let mut shown = true;
            let r = ui
                .checkbox(&mut shown, &title)
                .on_hover_text("Untick to close its window");
            if noted(ui, &format!("{W} ▸ {title}"), r).changed() {
                actions.push(Action::TogglePane(pane));
                ui.close();
            }
        }
    }
}

/// The new interface's Window ▸ New preview window.
pub(super) const NEW_PREVIEW_WINDOW: &str = "New preview window";

/// The classic interface's part of the Window menu: the panes, which are
/// showing and which are in windows of their own.
fn classic_panes(ui: &mut Ui, st: &UiState, actions: &mut Vec<Action>) {
    const W: &str = WINDOW_MENU;
    // Every kind, with the other Preview panes after the first.
    let mut panes: Vec<Pane> = Vec::new();
    for p in Pane::ALL {
        panes.push(p);
        if p == crate::dock::PREVIEW {
            let more = st.layout.preview_panes();
            panes.extend(more.into_iter().filter(|n| *n != 0).map(Pane::Preview));
        }
    }
    let workspace = timeline::workspace::chosen(st);
    for pane in panes {
        let title = pane.title();
        // The workspace is no pane.
        if pane == Pane::Timeline && workspace {
            let mut shown = st.timeline.open;
            let r = ui
                .checkbox(&mut shown, &title)
                .on_hover_text("The timeline workspace, in the panes' place");
            if noted(ui, &format!("{W} ▸ {title}"), r).changed() {
                actions.push(Action::Timeline(if shown {
                    timeline::TimelineAction::Open(None)
                } else {
                    timeline::TimelineAction::Close
                }));
                ui.close();
            }
            continue;
        }
        let open = st.layout.is_open(pane);
        let popped = st.layout.is_popped_out(pane);
        ui.horizontal(|ui| {
            let mut shown = open;
            let r = ui.checkbox(&mut shown, &title);
            if noted(ui, &format!("{W} ▸ {title}"), r).changed() {
                actions.push(Action::TogglePane(pane));
                ui.close();
            }
            if open {
                // The buttons in a column after the names.
                let gap = (ui.min_rect().min.x + 120.0 - ui.cursor().min.x).max(12.0);
                ui.scope(|ui| {
                    ui.add_space(gap);
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
                    let r = ui.small_button(label).on_hover_text(tip);
                    if noted(ui, &format!("{W} ▸ {title} ▸ {label}"), r).clicked() {
                        actions.push(action);
                        ui.close();
                    }
                });
            }
        });
    }
    let r = item(ui, W, "New preview", None).on_hover_text(
        "One more Preview, with its own mode (one display, the multiview or the 3D stage) and display",
    );
    if r.clicked() {
        actions.push(Action::NewPreview);
        ui.close();
    }
}

/// Window ▸ Timeline opens: where the timeline editor opens on this computer
/// (as `settings::TIMELINE_EDITOR`, which also has how it follows a playhead).
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
            .radio(st.prefs.timeline_layout == layout, words)
            .on_hover_text(tip);
        let r = noted(ui, &format!("{} ▸ {words}", TIMELINE_OPENS_ITEM.path()), r);
        if r.clicked() && st.prefs.timeline_layout != layout {
            actions.push(Action::Timeline(timeline::TimelineAction::SetLayout(
                layout,
            )));
            ui.close();
        }
    }
}

/// The Tools menu: what to run before a show and while it plays.
fn tools_menu(ui: &mut Ui, st: &UiState, actions: &mut Vec<Action>) {
    let t = TOOLS_MENU;
    let r = item(ui, t, settings::MEDIA_MANAGER_ITEM.label, None).on_hover_text(
        "Every file the show uses: which are missing, finding them again, and copying them \
         all into one folder",
    );
    if r.clicked() {
        actions.push(Action::ToggleMedia);
        ui.close();
    }
    let r = item(ui, t, settings::TEST_MODE_ITEM.label, None).on_hover_text(
        "Plays a show of its own to find out how this computer (or the show network) copes, \
         then puts your show back",
    );
    if r.clicked() {
        actions.push(Action::ToggleTestMode);
        ui.close();
    }
    let mut stats = st.stats_open;
    let r = ui
        .checkbox(&mut stats, STATISTICS_ITEM.label)
        .on_hover_text("A window of live figures: memory, processor, frame rate, slowest frame");
    if noted(ui, &STATISTICS_ITEM.path(), r).changed() {
        actions.push(Action::ToggleStats);
        ui.close();
    }
}

/// The Settings menu: the same pages as the sidebar, in the same groups,
/// each group's caption saying where what is set there is kept.
fn settings_menu(ui: &mut Ui, st: &UiState, actions: &mut Vec<Action>) {
    let label = if st.settings_open() {
        "Close settings"
    } else {
        "Open settings…"
    };
    if item(ui, SETTINGS_MENU, label, Some("Ctrl+,")).clicked() {
        actions.push(Action::ToggleSettings);
        ui.close();
    }
    for group in settings::SettingsGroup::ALL {
        // As the sidebar: a wider gap between the show's pages and this
        // computer's.
        if !group.in_show() {
            ui.add_space(6.0);
        }
        ui.separator();
        heading(ui, group.label());
        for page in SettingsPage::ALL.into_iter().filter(|p| p.group() == group) {
            let on = st.settings_open() && st.settings_page == page;
            let r = ui.selectable_label(on, page.label());
            if noted(ui, &format!("{SETTINGS_MENU} ▸ {}", page.label()), r).clicked() {
                actions.push(Action::OpenSettings(page));
                ui.close();
            }
        }
        // As the sidebar: what is saved in the show, what stays here.
        settings::group_caption(ui, group);
    }
}

/// The Cue menu's Add submenu, by what a cue plays.
fn add_menu(ui: &mut Ui, actions: &mut Vec<Action>) {
    let m = format!("{CUE_MENU} ▸ Add");
    let add = |ui: &mut Ui, ty: CueType, dots: &str, actions: &mut Vec<Action>| {
        let words = format!("{}{dots}", ty.label());
        let r = ui.button(format!("{} {words}", ty.icon()));
        if noted(ui, &format!("{m} ▸ {words}"), r).clicked() {
            actions.push(Action::AddCue(ty));
            ui.close();
        }
    };
    heading(ui, "From a file");
    for ty in [CueType::Video, CueType::Image, CueType::Audio] {
        add(ui, ty, "…", actions);
    }
    let r = ui.button("🖼 Slide show…").on_hover_text(
        "Any number of pictures, shown one after another on one display, with a crossfade",
    );
    if noted(ui, &format!("{m} ▸ Slide show…"), r).clicked() {
        actions.push(Action::AskSlides(None));
        ui.close();
    }
    let r = ui.button("📊 Slides from a presentation…").on_hover_text(
        "Takes the slides of a PowerPoint, Keynote, OpenDocument or PDF file as pictures, \
         with the file's own timing. A PDF (Save as PDF in any presentation program) needs \
         no other program.",
    );
    if noted(ui, &format!("{m} ▸ Slides from a presentation…"), r).clicked() {
        actions.push(Action::AskDeck(None));
        ui.close();
    }
    for (title, kinds) in [
        ("Live", &[CueType::Camera, CueType::LiveAudio][..]),
        (
            "On screen",
            &[
                CueType::Text,
                CueType::Shape,
                CueType::Clock,
                CueType::Timer,
            ][..],
        ),
        (
            "Control and timing",
            &[
                CueType::Fade,
                CueType::Stop,
                CueType::Wait,
                CueType::Control,
                CueType::Send,
                CueType::Timecode,
            ][..],
        ),
    ] {
        ui.separator();
        heading(ui, title);
        for ty in kinds {
            add(ui, *ty, "", actions);
        }
    }
    ui.separator();
    add(ui, CueType::Group, "", actions);
}

fn cue_menu(ui: &mut Ui, show: &Show, st: &UiState, actions: &mut Vec<Action>) {
    const C: &str = CUE_MENU;
    let r = ui.menu_button("Add", |ui| add_menu(ui, actions));
    noted(ui, &format!("{C} ▸ Add"), r.response);
    ui.separator();
    let r = item_enabled(ui, !st.show_mode, C, "New cue list", None).on_hover_text(
        "Another list of cues with a playhead of its own, in a tab above the cue list",
    );
    if r.clicked() {
        actions.push(Action::AddList);
        ui.close();
    }
    let r = item_enabled(ui, !st.show_mode, C, "🎞 New timeline", None).on_hover_text(
        "A cue list of the timeline type, in a tab of its own: its cues are clips on tracks, \
         on one clock, and GO runs it",
    );
    if r.clicked() {
        actions.push(Action::Timeline(timeline::TimelineAction::NewTimelineList));
        ui.close();
    }
    if let Some(id) = st.selected
        && let Some(c) = show.cue(id)
    {
        ui.separator();
        let arm = if c.armed { "Disarm" } else { "Arm" };
        if item(ui, C, arm, Some("A")).clicked() {
            actions.push(Action::SetArmed(id, !c.armed));
            ui.close();
        }
        let (label, action) = match (&c.kind, c.parent) {
            (CueKind::Group(_), _) => ("Ungroup", Action::Ungroup(id)),
            (_, None) => ("Wrap in group", Action::GroupCue(id)),
            (_, Some(_)) => ("Move out of group", Action::MoveOutOfGroup(id)),
        };
        if item(ui, C, label, None).clicked() {
            actions.push(action);
            ui.close();
        }
    }
    ui.separator();
    for (label, key, action) in [
        ("Duplicate", "Ctrl+D", Action::Duplicate),
        ("Move up", "Ctrl+↑", Action::Move(-1)),
        ("Move down", "Ctrl+↓", Action::Move(1)),
        ("Delete", "Del", Action::Delete),
    ] {
        if item(ui, C, label, Some(key)).clicked() {
            actions.push(action);
            ui.close();
        }
    }
}

fn file_menu(ui: &mut Ui, actions: &mut Vec<Action>) {
    const F: &str = FILE_MENU;
    for (label, key, action) in [
        ("New show", "Ctrl+N", Action::New),
        ("Open…", "Ctrl+O", Action::Open),
        ("Save", "Ctrl+S", Action::Save),
        ("Save as…", "Ctrl+Shift+S", Action::SaveAs),
    ] {
        if item(ui, F, label, Some(key)).clicked() {
            actions.push(action);
            ui.close();
        }
    }
    ui.separator();
    let r = item(ui, F, "Export cue sheet…", None)
        .on_hover_text("The cue list as a web page to print, or as a CSV for a spreadsheet.");
    if r.clicked() {
        actions.push(Action::AskFile(DialogFor::ExportCueSheet));
        ui.close();
    }
    ui.separator();
    if item(ui, F, "Quit", None).clicked() {
        actions.push(Action::Quit);
        ui.close();
    }
}

fn edit_menu(ui: &mut Ui, st: &UiState, actions: &mut Vec<Action>) {
    const E: &str = EDIT_MENU;
    if item_enabled(ui, st.history.can_undo(), E, "Undo", Some("Ctrl+Z")).clicked() {
        actions.push(Action::Undo);
        ui.close();
    }
    if item_enabled(ui, st.history.can_redo(), E, "Redo", Some("Ctrl+Y")).clicked() {
        actions.push(Action::Redo);
        ui.close();
    }
    ui.separator();
    let has_cue = st.selected.is_some();
    if item_enabled(ui, has_cue, E, "Copy cue", Some("Ctrl+C")).clicked() {
        actions.push(Action::Copy);
        ui.close();
    }
    if item_enabled(ui, has_cue && !st.show_mode, E, "Cut cue", Some("Ctrl+X")).clicked() {
        actions.push(Action::Cut);
        ui.close();
    }
    let can_paste = !st.clipboard.is_empty() && !st.show_mode;
    if item_enabled(ui, can_paste, E, "Paste cue", Some("Ctrl+V")).clicked() {
        actions.push(Action::Paste);
        ui.close();
    }
    ui.separator();
    if item(ui, E, "Find cues", Some("Ctrl+F")).clicked() {
        actions.push(Action::FocusFind);
        ui.close();
    }
    let r = item_enabled(ui, !st.show_mode, E, "Renumber cues", None)
        .on_hover_text("Number the cues 1, 2, 3 down the list (a group's children 3.1, 3.2)");
    if r.clicked() {
        actions.push(Action::Renumber);
        ui.close();
    }
    let mode = if st.show_mode {
        "Edit mode…"
    } else {
        "Show mode"
    };
    let r = item(ui, E, mode, Some("Ctrl+Shift+L")).on_hover_text(
        "In show mode nothing in the show can be edited by accident: GO, stopping, pausing \
         and the rest of running a show still work. Leaving it asks first.",
    );
    if r.clicked() {
        actions.push(Action::ToggleShowMode);
        ui.close();
    }
}

fn help_menu(ui: &mut Ui, st: &UiState, actions: &mut Vec<Action>) {
    const H: &str = HELP_MENU;
    if item(ui, H, "About NewQ…", None).clicked() {
        actions.push(Action::ToggleAbout);
        ui.close();
    }
    let r = item(ui, H, KEYS_ITEM.label, None)
        .on_hover_text("Every key NewQ answers to, in a window you can keep open");
    if r.clicked() {
        if !st.show_keys {
            actions.push(Action::ToggleKeys);
        }
        ui.close();
    }
    let r = item(ui, H, "Open the log folder", None).on_hover_text(format!(
        "{}: newq.log and newq-crash.log are here, to send if NewQ misbehaves",
        crate::crash::log_dir().display()
    ));
    if r.clicked() {
        actions.push(Action::OpenLogFolder);
        ui.close();
    }
}

pub(super) fn menu_bar(ui: &mut Ui, show: &Show, st: &UiState, actions: &mut Vec<Action>) {
    egui::containers::menu::MenuBar::new().ui(ui, |ui| {
        let r = ui.menu_button(FILE_MENU, |ui| file_menu(ui, actions));
        noted(ui, FILE_MENU, r.response);
        let r = ui.menu_button(EDIT_MENU, |ui| edit_menu(ui, st, actions));
        noted(ui, EDIT_MENU, r.response);
        let r = ui.menu_button(CUE_MENU, |ui| cue_menu(ui, show, st, actions));
        noted(ui, CUE_MENU, r.response);
        let r = ui.menu_button(WINDOW_MENU, |ui| window_menu(ui, st, actions));
        noted(ui, WINDOW_MENU, r.response);
        let r = ui.menu_button(TOOLS_MENU, |ui| tools_menu(ui, st, actions));
        noted(ui, TOOLS_MENU, r.response);
        let r = ui.menu_button(SETTINGS_MENU, |ui| settings_menu(ui, st, actions));
        noted(ui, SETTINGS_MENU, r.response);
        let r = ui.menu_button(HELP_MENU, |ui| help_menu(ui, st, actions));
        noted(ui, HELP_MENU, r.response);
    });
}

#[cfg(test)]
#[path = "menus_tests.rs"]
mod tests;
