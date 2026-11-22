//! The settings page: every setting in one place, in a sidebar of pages
//! grouped by what they are for. The groups above the line are saved in the
//! show; THIS COMPUTER's pages stay on the computer whatever show is open. It
//! takes the place of the cue list while it is open, so the transport (GO,
//! Stop All) and the live preview stay where they are and a setting can be
//! tried while the show runs.

use super::*;

/// A page of the settings. Not saved anywhere (which page is open is not
/// kept), so pages can move and be renamed without touching a file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SettingsPage {
    #[default]
    Show,
    Media,
    Network,
    Displays,
    SubDisplays,
    Stage3d,
    Audio,
    Sync,
    Osc,
    Midi,
    Web,
    Interface,
    ThisComputer,
}

/// What the pages are grouped under in the sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsGroup {
    Show,
    Output,
    Control,
    Computer,
}

impl SettingsGroup {
    pub const ALL: [SettingsGroup; 4] = [
        SettingsGroup::Show,
        SettingsGroup::Output,
        SettingsGroup::Control,
        SettingsGroup::Computer,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SettingsGroup::Show => "SHOW",
            SettingsGroup::Output => "PICTURE AND SOUND",
            SettingsGroup::Control => "REMOTE CONTROL",
            SettingsGroup::Computer => "THIS COMPUTER",
        }
    }

    /// Saved in the show file (the groups above the line), or kept on this
    /// computer.
    pub fn in_show(self) -> bool {
        self != SettingsGroup::Computer
    }

    /// The line under the group's pages, in the sidebar and the Settings
    /// menu: where what is set on them is kept.
    pub fn caption(self) -> Option<&'static str> {
        match self {
            SettingsGroup::Control => Some("Everything above is saved in the show file."),
            SettingsGroup::Computer => Some("Stays on this computer, whatever show is open."),
            SettingsGroup::Show | SettingsGroup::Output => None,
        }
    }
}

/// The caption under a group, small and grey (the sidebar and the menu).
pub(super) fn group_caption(ui: &mut Ui, group: SettingsGroup) {
    if let Some(words) = group.caption() {
        ui.add_space(2.0);
        ui.add(
            egui::Label::new(
                RichText::new(words)
                    .small()
                    .italics()
                    .color(Color32::from_gray(130)),
            )
            .wrap(),
        );
    }
}

impl SettingsPage {
    /// In the order the sidebar lists them.
    pub const ALL: [SettingsPage; 13] = [
        SettingsPage::Show,
        SettingsPage::Media,
        SettingsPage::Network,
        SettingsPage::Displays,
        SettingsPage::SubDisplays,
        SettingsPage::Stage3d,
        SettingsPage::Audio,
        SettingsPage::Sync,
        SettingsPage::Osc,
        SettingsPage::Midi,
        SettingsPage::Web,
        SettingsPage::Interface,
        SettingsPage::ThisComputer,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SettingsPage::Show => "Show",
            SettingsPage::Media => "Media",
            SettingsPage::Network => "Show network",
            SettingsPage::Displays => "Displays",
            SettingsPage::SubDisplays => "Sub displays",
            SettingsPage::Stage3d => "3D stage",
            SettingsPage::Audio => "Audio outputs",
            SettingsPage::Sync => "Sync",
            SettingsPage::Osc => "OSC",
            SettingsPage::Midi => "MIDI",
            SettingsPage::Web => "Web remote",
            SettingsPage::Interface => "Interface",
            SettingsPage::ThisComputer => "This computer",
        }
    }

    pub fn group(self) -> SettingsGroup {
        match self {
            SettingsPage::Show | SettingsPage::Media | SettingsPage::Network => SettingsGroup::Show,
            SettingsPage::Displays
            | SettingsPage::SubDisplays
            | SettingsPage::Stage3d
            | SettingsPage::Audio
            | SettingsPage::Sync => SettingsGroup::Output,
            SettingsPage::Osc | SettingsPage::Midi | SettingsPage::Web => SettingsGroup::Control,
            SettingsPage::Interface | SettingsPage::ThisComputer => SettingsGroup::Computer,
        }
    }

    /// One line under the page's title.
    pub fn blurb(self) -> &'static str {
        match self {
            SettingsPage::Show => {
                "The show's name, how Stop All behaves and how pictures share a display."
            }
            SettingsPage::Media => "The files the cues play, and whether they can all be found.",
            SettingsPage::Network => {
                "Other computers that run this show with this one: find them and give them roles."
            }
            SettingsPage::Displays => {
                "Which monitor each display is on, and how it is shaped, tested and sent out."
            }
            SettingsPage::SubDisplays => {
                "Areas of a display that cues can be sent to as if they were displays."
            }
            SettingsPage::Stage3d => {
                "The venue in 3D and the screens in it, each showing a display's picture."
            }
            SettingsPage::Audio => "Where sound goes: the devices cues can play on.",
            SettingsPage::Sync => "Line the sound up with the picture.",
            SettingsPage::Osc => "Run NewQ from a lighting desk, Companion or other software.",
            SettingsPage::Midi => "Run NewQ from a MIDI controller, or follow MIDI Time Code.",
            SettingsPage::Web => "Run the show from a phone or tablet.",
            SettingsPage::Interface => {
                "Where timelines open, which interface, the previews. Kept on this computer."
            }
            SettingsPage::ThisComputer => {
                "Power, decoding, what a show may start here, the logs. Kept on this computer."
            }
        }
    }
}

/// The words that lead someone to a settings page, or to a section of it:
/// "Settings ▸ This computer ▸ Power". Every pointer the program writes comes
/// from here (or from `Section::path`), so moving a page or a section changes
/// them all in one place.
pub fn path(page: SettingsPage, section: Option<&str>) -> String {
    match section {
        Some(s) => format!("Settings ▸ {} ▸ {s}", page.label()),
        None => format!("Settings ▸ {}", page.label()),
    }
}

/// A titled block of a settings page that something elsewhere points to. Its
/// page is part of it, so a pointer can't name the wrong one; a test draws
/// every page and finds each section's heading on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section {
    pub page: SettingsPage,
    pub name: &'static str,
    /// Kept on this computer rather than in the show. On a page of the show
    /// the heading says so; THIS COMPUTER's pages say it once for all.
    pub this_computer: bool,
}

impl Section {
    /// The heading the page draws.
    pub fn heading(self) -> String {
        if self.this_computer && self.page.group().in_show() {
            format!("{} (this computer)", self.name)
        } else {
            self.name.to_string()
        }
    }

    /// "Settings ▸ Show ▸ Layers".
    pub fn path(self) -> String {
        path(self.page, Some(self.name))
    }
}

const fn section_of(page: SettingsPage, name: &'static str, this_computer: bool) -> Section {
    Section {
        page,
        name,
        this_computer,
    }
}

pub const NAME_AND_FILE: Section = section_of(SettingsPage::Show, "Name and file", false);
pub const STOP_ALL: Section = section_of(SettingsPage::Show, "Stop All", false);
pub const LAYERS: Section = section_of(SettingsPage::Show, "Layers", false);
pub const FILES: Section = section_of(SettingsPage::Media, "Files", false);
pub const LIBRARY: Section = section_of(
    SettingsPage::Media,
    "Folders to keep on every computer",
    false,
);
/// First on its page: with a Timeline pane open, the classic window's
/// settings have only a little height, and this is the section wanted then.
pub const TIMELINE_EDITOR: Section = section_of(SettingsPage::Interface, "Timeline editor", true);
pub const INTERFACE: Section = section_of(SettingsPage::Interface, "Classic or new", true);
pub const PREVIEWS: Section = section_of(SettingsPage::Interface, "Previews", true);
pub const POWER: Section = section_of(SettingsPage::ThisComputer, "Power", true);
pub const DECODING: Section = section_of(SettingsPage::ThisComputer, "Decoding", true);
/// What a show may start on this computer: a streaming server, programs.
pub const ALLOWED_HERE: Section = section_of(SettingsPage::ThisComputer, "Allowed here", true);
/// Where running a streaming server is allowed (`ALLOWED_HERE`).
pub const STREAMING_SERVER: Section = ALLOWED_HERE;
/// Where letting send cues run programs is allowed (`ALLOWED_HERE`).
pub const PROGRAMS: Section = ALLOWED_HERE;
/// Drawn only while the presenter remote is available (`clicker::AVAILABLE`).
pub const PRESENTER_REMOTE: Section =
    section_of(SettingsPage::ThisComputer, "Presenter remote", true);
pub const RECOVERY_AND_LOGS: Section =
    section_of(SettingsPage::ThisComputer, "Logs and recovery", true);
/// The 3D stage page's tabs, under its view (`stage_edit::StageTab`).
pub const STAGE_VENUE: Section = section_of(SettingsPage::Stage3d, "Venue", false);
pub const STAGE_SCREENS: Section = section_of(SettingsPage::Stage3d, "Screens", false);
pub const STAGE_PROJECTORS: Section = section_of(SettingsPage::Stage3d, "Projectors", false);
pub const STAGE_VIEWPOINTS: Section = section_of(SettingsPage::Stage3d, "Viewpoints", false);
/// The displays that show the stage, each with the way to it on Displays.
pub const STAGE_DISPLAYS: Section = section_of(SettingsPage::Stage3d, "Displays", false);

/// Every section the pages draw, each page's in its order (the tests' list;
/// the presenter remote's is left out while it is switched off). A tab of
/// the 3D stage page counts as drawn by its label, whichever tab is chosen.
#[cfg(test)]
pub const SECTIONS: [Section; 17] = [
    NAME_AND_FILE,
    STOP_ALL,
    LAYERS,
    FILES,
    LIBRARY,
    TIMELINE_EDITOR,
    INTERFACE,
    PREVIEWS,
    POWER,
    DECODING,
    ALLOWED_HERE,
    RECOVERY_AND_LOGS,
    STAGE_VENUE,
    STAGE_SCREENS,
    STAGE_PROJECTORS,
    STAGE_VIEWPOINTS,
    STAGE_DISPLAYS,
];

/// An item of the menu bar that a pointer names: "Tools ▸ Test Mode". The
/// menu draws `label`, so the two can't drift apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuItem {
    pub menu: &'static str,
    pub label: &'static str,
}

impl MenuItem {
    pub fn path(self) -> String {
        format!("{} ▸ {}", self.menu, self.label.trim_end_matches('…'))
    }
}

/// The menus of the menu bar, left to right: `menus::menu_bar` draws these
/// names and the `MenuItem`s below point into them.
pub const FILE_MENU: &str = "File";
pub const EDIT_MENU: &str = "Edit";
pub const CUE_MENU: &str = "Cue";
pub const WINDOW_MENU: &str = "Window";
pub const TOOLS_MENU: &str = "Tools";
pub const SETTINGS_MENU: &str = "Settings";
pub const HELP_MENU: &str = "Help";

pub const MEDIA_MANAGER_ITEM: MenuItem = MenuItem {
    menu: TOOLS_MENU,
    label: "Media manager…",
};
pub const TEST_MODE_ITEM: MenuItem = MenuItem {
    menu: TOOLS_MENU,
    label: "Test Mode…",
};
pub const STATISTICS_ITEM: MenuItem = MenuItem {
    menu: TOOLS_MENU,
    label: "Statistics",
};
/// The Window menu's Timeline tick (the pane's own name).
pub const TIMELINE_ITEM: MenuItem = MenuItem {
    menu: WINDOW_MENU,
    label: "Timeline",
};
/// The Window menu's submenu of where the timeline editor opens.
pub const TIMELINE_OPENS_ITEM: MenuItem = MenuItem {
    menu: WINDOW_MENU,
    label: "Timeline opens",
};
/// The Window menu's Cart tick (the pane's own name).
pub const CART_ITEM: MenuItem = MenuItem {
    menu: WINDOW_MENU,
    label: "Cart",
};
pub const KEYS_ITEM: MenuItem = MenuItem {
    menu: HELP_MENU,
    label: "Keyboard shortcuts…",
};

/// What the sidebar shows beside a page: a count, a state, or a warning.
#[derive(Clone, Debug, PartialEq)]
pub struct Badge {
    pub text: String,
    pub color: Color32,
}

pub(super) fn badge(text: impl Into<String>, color: Color32) -> Option<Badge> {
    Some(Badge {
        text: text.into(),
        color,
    })
}

/// The badge for `page`: a warning in red when something needs attention, a
/// green mark for what is on and working, grey for a count.
pub fn page_badge(page: SettingsPage, show: &Show, st: &UiState) -> Option<Badge> {
    let weak = Color32::from_gray(140);
    match page {
        SettingsPage::Show | SettingsPage::Interface => None,
        // Beside Power, which it is about.
        SettingsPage::ThisComputer => st.on_battery.then(|| Badge {
            text: "⚠ battery".into(),
            color: STOP_RED,
        }),
        SettingsPage::Media => {
            let missing = st.missing_count(show);
            (missing > 0).then(|| Badge {
                text: format!("⚠ {missing}"),
                color: STOP_RED,
            })
        }
        SettingsPage::Network => super::network::badge(&st.net),
        SettingsPage::Displays => {
            let displays: Vec<&OutputConfig> = show
                .outputs
                .iter()
                .filter(|o| !o.is_sub_display() && o.enabled)
                .collect();
            let trouble = displays.iter().any(|d| {
                matches!(
                    st.display_states.get(&d.id),
                    Some(DisplayState::Missing | DisplayState::OnMainWindowDisplay(_))
                )
            });
            if trouble {
                badge("⚠", STOP_RED)
            } else {
                badge(displays.len().to_string(), weak)
            }
        }
        SettingsPage::SubDisplays => {
            let n = show.outputs.iter().filter(|o| o.is_sub_display()).count();
            (n > 0).then(|| Badge {
                text: n.to_string(),
                color: weak,
            })
        }
        SettingsPage::Stage3d => {
            if matches!(st.stage.venue, super::stage_view::VenueStatus::Failed(_)) {
                return badge("⚠", STOP_RED);
            }
            let n = show.stage3d.surfaces.len();
            (n > 0).then(|| Badge {
                text: n.to_string(),
                color: weak,
            })
        }
        SettingsPage::Audio => {
            if show.audio_outputs.is_empty() {
                return None;
            }
            badge(show.audio_outputs.len().to_string(), weak)
        }
        SettingsPage::Sync => {
            let set = show.sync.audio_ms > 0
                || show.sync.video_ms > 0
                || show.audio_outputs.iter().any(|o| o.delay_ms > 0)
                || show.outputs.iter().any(|o| o.delay_ms > 0);
            set.then(|| Badge {
                text: "set".into(),
                color: PLAYHEAD,
            })
        }
        SettingsPage::Osc => remote_badge(
            show.remote.osc,
            st.remote_status.osc_port.is_some(),
            st.remote_status.osc_error.is_some(),
        ),
        SettingsPage::Midi => remote_badge(
            show.remote.midi,
            st.remote_status.midi_port.is_some(),
            st.remote_status.midi_error.is_some(),
        ),
        SettingsPage::Web => remote_badge(
            show.remote.web,
            st.remote_status.web_port.is_some(),
            st.remote_status.web_error.is_some(),
        ),
    }
}

/// Off shows nothing; on and listening is green; a problem is red.
fn remote_badge(enabled: bool, listening: bool, error: bool) -> Option<Badge> {
    match (enabled, error, listening) {
        (false, _, _) => None,
        (true, true, _) => badge("⚠", STOP_RED),
        (true, false, true) => badge("on", GO_GREEN),
        (true, false, false) => badge("…", Color32::from_gray(140)),
    }
}

/// The settings, in place of the cue list.
pub fn settings_view(
    ui: &mut Ui,
    show: &mut Show,
    engine: &mut Engine,
    st: &mut UiState,
    actions: &mut Vec<Action>,
) {
    egui::Panel::left("settings_nav")
        .resizable(false)
        .exact_size(214.0)
        .show(ui, |ui| sidebar(ui, show, st, actions));
    egui::CentralPanel::default().show(ui, |ui| {
        let page = st.settings_page;
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.heading(page.label());
                ui.label(RichText::new(page.blurb()).weak());
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .button("Close settings")
                    .on_hover_text("Close the settings (Ctrl+,)")
                    .clicked()
                {
                    actions.push(Action::CloseSettings);
                }
            });
        });
        ui.separator();
        // The 3D stage lays itself out: its view pinned at the top, as wide
        // as the window, and only the chosen tab's content scrolls.
        if page == SettingsPage::Stage3d {
            super::stage_edit::page(ui, show, st, actions);
            return;
        }
        // Each page keeps its own place: one opened from far down another
        // starts at its own top, not part-way down at the other's place.
        egui::ScrollArea::vertical()
            .id_salt(("settings_page", page))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // A comfortable width and no more, with room for the frames'
                // edges and the scroll bar so nothing is clipped.
                ui.set_max_width((ui.available_width() - 20.0).clamp(300.0, 640.0));
                match page {
                    SettingsPage::Show => show_page(ui, show, st),
                    SettingsPage::Media => media_page(ui, show, st, actions),
                    SettingsPage::Network => super::network::page(ui, show, st, actions),
                    SettingsPage::Displays => displays_panel(ui, show, engine, st, actions),
                    SettingsPage::SubDisplays => sub_displays_panel(ui, show, st, actions),
                    // Drawn above, without this scroll area.
                    SettingsPage::Stage3d => {}
                    SettingsPage::Audio => audio_panel(ui, show, engine, st, actions),
                    SettingsPage::Sync => sync_page(ui, show, st, actions),
                    SettingsPage::Osc => osc_page(ui, show, st),
                    SettingsPage::Midi => midi_page(ui, show, st, engine),
                    SettingsPage::Web => web_page(ui, show, st),
                    SettingsPage::Interface => interface_page(ui, st, actions),
                    SettingsPage::ThisComputer => computer_page(ui, st),
                }
            });
    });
}

/// For the tests: where a control of a page is, by name, if it is drawn.
#[cfg(test)]
pub(super) fn note_rect(ui: &Ui, name: impl Into<String>, rect: egui::Rect) {
    // Only what is in sight counts (not scrolled out of the page).
    let rect = if ui.clip_rect().contains_rect(rect) {
        rect
    } else {
        egui::Rect::NOTHING
    };
    ui.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<(String, egui::Rect)>>(egui::Id::new("settings_rects"))
            .push((name.into(), rect))
    });
}

/// The pages, grouped, each a row with its status on the right.
fn sidebar(ui: &mut Ui, show: &Show, st: &mut UiState, actions: &mut Vec<Action>) {
    ui.add_space(6.0);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for group in SettingsGroup::ALL {
                if !group.in_show() {
                    ui.add_space(6.0);
                    ui.separator();
                }
                ui.add_space(8.0);
                ui.label(
                    RichText::new(group.label())
                        .small()
                        .strong()
                        .color(Color32::from_gray(130)),
                );
                ui.add_space(2.0);
                for page in SettingsPage::ALL.into_iter().filter(|p| p.group() == group) {
                    let b = page_badge(page, show, st);
                    if nav_row(ui, page.label(), st.settings_page == page, b.as_ref()) {
                        actions.push(Action::OpenSettings(page));
                    }
                }
                group_caption(ui, group);
            }
        });
}

/// One row of the sidebar. True when it was clicked.
fn nav_row(ui: &mut Ui, label: &str, selected: bool, badge: Option<&Badge>) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::click());
    #[cfg(test)]
    note_rect(ui, format!("page:{label}"), rect);
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().visuals.clone();
        if selected {
            ui.painter()
                .rect_filled(rect, 4.0, visuals.selection.bg_fill);
        } else if response.hovered() {
            ui.painter()
                .rect_filled(rect, 4.0, visuals.widgets.hovered.weak_bg_fill);
        }
        let text_color = if selected {
            visuals.selection.stroke.color
        } else {
            visuals.text_color()
        };
        ui.painter().text(
            rect.left_center() + Vec2::new(10.0, 0.0),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(14.0),
            text_color,
        );
        if let Some(b) = badge {
            ui.painter().text(
                rect.right_center() - Vec2::new(10.0, 0.0),
                egui::Align2::RIGHT_CENTER,
                &b.text,
                egui::FontId::proportional(12.0),
                b.color,
            );
        }
    }
    response.clicked()
}

/// A titled block of a page.
pub(super) fn section(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    // For the tests: every heading drawn, so a pointer's section can be found.
    note_heading(ui, title);
    ui.add_space(6.0);
    ui.label(RichText::new(title).strong());
    ui.add_space(2.0);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        body(ui);
    });
}

/// Notes a heading for the tests, so a pointer's section can be found (a
/// tab's label, which is in sight whichever tab is chosen, counts).
pub(super) fn note_heading(ui: &Ui, title: &str) {
    #[cfg(test)]
    ui.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<String>>(egui::Id::new("settings_sections"))
            .push(title.to_string())
    });
    #[cfg(not(test))]
    let _ = (ui, title);
}

/// A block of a page with no title of its own: a tab's content, which its
/// tab names.
pub(super) fn untitled(ui: &mut Ui, body: impl FnOnce(&mut Ui)) {
    ui.add_space(4.0);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        body(ui);
    });
}

pub(super) fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).weak().small());
}

pub(super) fn warning(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).small().color(PREWAIT));
}

/// Saves this computer's settings (`prefs.json` in the log folder).
fn save_prefs(st: &UiState) {
    if let Some(dir) = &st.layout_dir
        && let Err(e) = st.prefs.save(dir)
    {
        log::warn!("could not save the settings of this computer: {e}");
    }
}

/// The show's own: its name and file, Stop All, layers.
fn show_page(ui: &mut Ui, show: &mut Show, st: &UiState) {
    section(ui, &NAME_AND_FILE.heading(), |ui| {
        egui::Grid::new("settings_show")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut show.name).desired_width(260.0));
                ui.end_row();
                ui.label("File");
                match &st.show_path {
                    Some(p) => ui.label(RichText::new(p.display().to_string()).small()),
                    None => ui.label(RichText::new("Not saved yet").weak()),
                };
                ui.end_row();
            });
        hint(
            ui,
            "The name is the one the file is offered under when saving.",
        );
    });
    section(ui, &STOP_ALL.heading(), |ui| {
        ui.horizontal(|ui| {
            ui.label("Fade out over");
            secs(ui, &mut show.panic_fade);
        });
        hint(
            ui,
            "How long the Stop All button and Esc take to fade everything out. Pressing Esc \
             twice stops everything at once.",
        );
    });
    section(ui, &LAYERS.heading(), |ui| {
        ui.checkbox(
            &mut show.own_layers,
            "A picture replaces only what is on its own layer",
        );
        hint(
            ui,
            "On: a cue that cuts or crossfades in replaces only the cues on its own layer of \
             its display (a title on layer 1 stays over a new picture on layer 0), and the \
             cues a \"start all together\" group starts share the display. Off: it replaces \
             everything on the display, as shows made before this setting did. New shows \
             have it on; a show opened from an older NewQ has it off until you turn it on.",
        );
    });
}

/// How NewQ looks on this computer: where timelines open, which interface,
/// the previews.
fn interface_page(ui: &mut Ui, st: &mut UiState, actions: &mut Vec<Action>) {
    // First, and two rows: with a Timeline pane open, the classic window's
    // settings have only a little height, and this is the section wanted then.
    section(ui, &TIMELINE_EDITOR.heading(), |ui| {
        timeline_layout(ui, st, actions);
        timeline_follow(ui, st, actions);
        hint(
            ui,
            &format!(
                "Where it opens is also in {}. Hover a choice for what it does.",
                TIMELINE_OPENS_ITEM.path()
            ),
        );
    });
    section(ui, &INTERFACE.heading(), |ui| {
        interface_choice(ui, st, actions);
    });
    section(ui, &PREVIEWS.heading(), |ui| {
        preview_settings(ui, st);
        hint(
            ui,
            "The previews in this window (the Preview pane, one display, multiview or 3D stage, and the \
             timeline workspace's monitor) come second to the displays: while a display \
             misses pictures people would see, they are drawn less often, then smaller, \
             then paused, and back up a couple of seconds after the displays are smooth. \
             The Preview pane says so while it happens. The limits here hold them at or \
             below that all the time.",
        );
    });
}

/// The classic window of panes, or the new interface's views.
fn interface_choice(ui: &mut Ui, st: &UiState, actions: &mut Vec<Action>) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Use");
        for (modern, words, tip) in [
            (
                false,
                "the classic interface",
                "The cue list, inspector, previews and the other panes, side by side; any pane \
                 can be popped out to a window of its own",
            ),
            (
                true,
                "the new interface",
                "Editor, Operator and Patch views, one at a time, in place of the panes",
            ),
        ] {
            let r = ui
                .radio(st.prefs.new_layout == modern, words)
                .on_hover_text(format!(
                    "{tip}. The same show either way; switch back any time."
                ));
            #[cfg(test)]
            note_rect(
                ui,
                if modern {
                    "interface:new"
                } else {
                    "interface:classic"
                },
                r.rect,
            );
            if r.clicked() && st.prefs.new_layout != modern {
                actions.push(Action::SetNewLayout(modern));
            }
        }
    });
    hint(
        ui,
        &format!(
            "Also {} ▸ Interface, and the Classic interface button on the new interface's strip.",
            WINDOW_MENU
        ),
    );
}

/// What this computer may do while a show runs, and its logs.
fn computer_page(ui: &mut Ui, st: &mut UiState) {
    section(ui, &POWER.heading(), |ui| {
        let mut keep = !st.prefs.let_computer_sleep;
        let r = ui.checkbox(
            &mut keep,
            "Keep this computer and its displays awake while a display is showing",
        );
        #[cfg(test)]
        note_rect(ui, "keep_awake", r.rect);
        if r.changed() {
            st.prefs.let_computer_sleep = !keep;
            save_prefs(st);
        }
        hint(
            ui,
            "Stops the screen saver, dimming and sleep from interrupting a show during a long \
             hold. Only while a display's window is open: editing a show doesn't hold it. A \
             laptop whose lid is closed keeps showing on an external display only when it is \
             plugged in; on battery it sleeps, and nothing can prevent that.",
        );
        if st.on_battery {
            warning(
                ui,
                "This computer is on battery. Closing the lid would put it to sleep and stop the show.",
            );
        }
    });
    section(ui, &DECODING.heading(), |ui| {
        let mut on = st.prefs.hw_decode;
        let r = ui.checkbox(
            &mut on,
            "Decode video on the graphics hardware where possible",
        );
        #[cfg(test)]
        note_rect(ui, "hw_decode", r.rect);
        if r.changed() {
            st.prefs.hw_decode = on;
            crate::media::video::set_hw_decode(on);
            save_prefs(st);
        }
        hint(
            ui,
            "Takes decoding of 4K and many-layer shows off the processor, at the cost of \
             copying each picture back from the card. Takes effect for cues that start after \
             the change. Try it on this computer with the show you mean to run; a file or card \
             it can't do is decoded by the processor as before.",
        );
    });
    section(ui, &ALLOWED_HERE.heading(), |ui| {
        hint(
            ui,
            "What a show may start on this computer. A show can come from anywhere, so these \
             are kept here, not in the show: each computer says yes for itself.",
        );
        let mut on = st.prefs.allow_hosting;
        let r = ui.checkbox(
            &mut on,
            "Let a show run a streaming server on this computer",
        );
        #[cfg(test)]
        note_rect(ui, "allow_hosting", r.rect);
        if r.changed() {
            st.prefs.allow_hosting = on;
            save_prefs(st);
        }
        hint(
            ui,
            "A display's Send out can run the MediaMTX that comes with NewQ so phones and other \
             computers can watch it. It opens network ports while the stream is on. A show that \
             asks for it starts nothing here until this is on; ticking \"Run the server in NewQ\" \
             turns it on.",
        );
        ui.add_space(4.0);
        let mut on = st.prefs.allow_scripts;
        let r = ui.checkbox(&mut on, "Let send cues run programs on this computer");
        #[cfg(test)]
        note_rect(ui, "allow_scripts", r.rect);
        if r.changed() {
            st.prefs.allow_scripts = on;
            crate::output::set_scripts_allowed(on);
            save_prefs(st);
        }
        warning(
            ui,
            "A program can do anything you can. Leave this off unless you have read the send \
             cues of the show.",
        );
    });
    if crate::clicker::AVAILABLE {
        section(ui, &PRESENTER_REMOTE.heading(), |ui| {
            presenter_remote(ui, st)
        });
    }
    section(ui, &RECOVERY_AND_LOGS.heading(), |ui| {
        ui.label(format!(
            "Unsaved changes are copied to a recovery file every {} seconds, and offered \
             back if NewQ didn't close properly.",
            crate::recovery::INTERVAL.as_secs()
        ));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Log folder");
            ui.label(RichText::new(crate::crash::log_dir().display().to_string()).small());
            if ui.small_button("Open").clicked()
                && let Err(e) = crate::testmode::session::open_folder(&crate::crash::log_dir())
            {
                log::warn!("{e}");
            }
        });
        hint(
            ui,
            "newq.log and newq-crash.log are here: send them if NewQ misbehaves. The log is \
             begun again when it reaches 20 MB (the part before is kept as newq.previous.log), \
             so it can't fill the disk.",
        );
        ui.add_space(4.0);
        log_detail(ui, st);
    });
}

/// A presenter remote's two buttons, learned (switched off for now:
/// `clicker::AVAILABLE`).
fn presenter_remote(ui: &mut Ui, st: &mut UiState) {
    let mut changed = false;
    let mut on = st.prefs.clicker_on;
    if ui
        .checkbox(
            &mut on,
            "Use a presenter remote (a PerfectCue, a slide clicker) as GO and Back",
        )
        .changed()
    {
        st.prefs.clicker_on = on;
        changed = true;
    }
    let rows = [
        (crate::clicker::Button::Go, "GO button", st.prefs.clicker_go),
        (
            crate::clicker::Button::Back,
            "Back button",
            st.prefs.clicker_back,
        ),
    ];
    for (button, label, code) in rows {
        ui.horizontal(|ui| {
            ui.label(format!("{label}:"));
            if st.clicker_learning == Some(button) {
                ui.label(RichText::new("press the button on the remote now…").color(PLAYHEAD));
                if ui.button("Cancel").clicked() {
                    st.clicker_cancel_learning = true;
                }
            } else {
                ui.label(crate::clicker::describe(code));
                if ui.button("Learn").clicked() {
                    st.clicker_learn_request = Some(button);
                }
                if code.is_some() && ui.button("Clear").clicked() {
                    match button {
                        crate::clicker::Button::Go => st.prefs.clicker_go = None,
                        crate::clicker::Button::Back => st.prefs.clicker_back = None,
                    }
                    changed = true;
                }
            }
        });
    }
    if changed {
        save_prefs(st);
    }
    if st.prefs.clicker_on && !st.clicker_status.is_empty() {
        if st.clicker_global {
            ui.label(RichText::new(&st.clicker_status).color(GO_GREEN));
        } else {
            warning(ui, &st.clicker_status);
        }
    }
    hint(
        ui,
        "GO fires the next cue; while everything is paused it resumes instead. Back pauses \
         everything; Back again fades everything out and stops (a second press within a \
         second is ignored, so a double tap can't end the show). The remote is a keyboard to \
         the computer: press Learn, then its button. Only the two learned keys are looked at; \
         nothing typed is kept, and the keys still reach other programs. Learned keys work \
         when NewQ isn't in front on Windows and on a Mac (which asks for the Input \
         Monitoring permission); elsewhere only while a NewQ window has the focus.",
    );
}

/// The previews' settings on this computer: on or off, their most pictures a
/// second and size, and how much the displays may miss before they give way.
fn preview_settings(ui: &mut Ui, st: &mut UiState) {
    use crate::preview_pace::{Cap, SizeCap, Strictness};
    let before = (
        st.prefs.previews_off,
        st.prefs.preview_cap,
        st.prefs.preview_size,
        st.prefs.preview_strictness,
        st.prefs.fps_on_output,
    );
    egui::Grid::new("settings_previews")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Previews");
            ui.horizontal(|ui| {
                let _on = ui
                    .selectable_value(&mut st.prefs.previews_off, false, "On")
                    .on_hover_text("Previews are drawn, as well as the displays leave room for.");
                let _off = ui
                    .selectable_value(&mut st.prefs.previews_off, true, "Off")
                    .on_hover_text(
                        "No preview pictures at all; each preview says so, with a button to turn them back on. The displays are not affected.",
                    );
                #[cfg(test)]
                {
                    note_rect(ui, "previews:on", _on.rect);
                    note_rect(ui, "previews:off", _off.rect);
                }
            });
            ui.end_row();
            ui.label("Frame rate");
            egui::ComboBox::from_id_salt("settings_preview_cap")
                .selected_text(st.prefs.preview_cap.label())
                .show_ui(ui, |ui| {
                    for c in Cap::ALL {
                        ui.selectable_value(&mut st.prefs.preview_cap, c, c.label())
                            .on_hover_text(c.words());
                    }
                });
            ui.end_row();
            ui.label("Resolution");
            egui::ComboBox::from_id_salt("settings_preview_size")
                .selected_text(st.prefs.preview_size.label())
                .show_ui(ui, |ui| {
                    for s in SizeCap::ALL {
                        ui.selectable_value(&mut st.prefs.preview_size, s, s.label())
                            .on_hover_text(s.words());
                    }
                });
            ui.end_row();
            ui.label("Give way");
            ui.horizontal(|ui| {
                for s in Strictness::ALL {
                    ui.selectable_value(&mut st.prefs.preview_strictness, s, s.label())
                        .on_hover_text(s.words());
                }
            });
            ui.end_row();
            ui.label("Frame rate on the display");
            let _fps = ui
                .checkbox(
                    &mut st.prefs.fps_on_output,
                    "Write it in the picture's top left corner",
                )
                .on_hover_text(
                    "Writes how many pictures a second each display draws in the top left corner of its picture, while something is playing. Off by default: don't leave it on in a show.",
                );
            #[cfg(test)]
            note_rect(ui, "fps_on_output", _fps.rect);
            ui.end_row();
        });
    // What the choices made mean, one sentence each.
    let off = st.prefs.previews_off;
    hint(
        ui,
        if off {
            "Off: no preview pictures at all; the displays are not affected."
        } else {
            "On: previews are drawn, as well as the displays leave room for."
        },
    );
    hint(ui, st.prefs.preview_cap.words());
    hint(ui, st.prefs.preview_size.words());
    hint(ui, st.prefs.preview_strictness.words());
    let after = (
        st.prefs.previews_off,
        st.prefs.preview_cap,
        st.prefs.preview_size,
        st.prefs.preview_strictness,
        st.prefs.fps_on_output,
    );
    if after != before {
        save_prefs(st);
    }
}

/// How the timeline editor's view keeps a running playhead in sight.
fn timeline_follow(ui: &mut Ui, st: &UiState, actions: &mut Vec<Action>) {
    use super::timeline::TimelineFollow as F;
    const HOW: &str = "Either way you can scroll and zoom while it runs (in the middle, it \
         comes back to the playhead), nothing moves while you hold a drag, and a stopped or \
         paused timeline stays where you put it.";
    ui.horizontal_wrapped(|ui| {
        ui.label("While it runs, the view");
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
                .radio(st.prefs.timeline_follow == follow, words)
                .on_hover_text(format!("{tip}. {HOW}"));
            // For the tests: where it is, if it is in sight (not scrolled out).
            #[cfg(test)]
            let seen = if ui.clip_rect().contains_rect(r.rect) {
                r.rect
            } else {
                egui::Rect::NOTHING
            };
            #[cfg(test)]
            ui.data_mut(|d| {
                d.get_temp_mut_or_default::<Vec<(bool, egui::Rect)>>(egui::Id::new(
                    "settings_timeline_follow",
                ))
                .push((follow == F::Centre, seen))
            });
            if r.clicked() && st.prefs.timeline_follow != follow {
                actions.push(Action::Timeline(
                    super::timeline::TimelineAction::SetFollow(follow),
                ));
            }
        }
    });
}

/// Where the timeline editor opens: a pane or a workspace of its own, in
/// either interface. Kept for this computer; an editor open moves.
fn timeline_layout(ui: &mut Ui, st: &UiState, actions: &mut Vec<Action>) {
    use super::timeline::TimelineLayout as L;
    let where_ = format!(
        "Where a timeline opens when you double-click a timeline group, tick {} or go to a \
         timeline list's tab. The workspace takes the cue list's place until Back to cue list; \
         in the new interface it is the Timeline view. The {} menu and the Workspace button of \
         the tracks' toolbar switch it too.",
        TIMELINE_ITEM.path(),
        TIMELINE_ITEM.menu
    );
    let pane = if st.prefs.new_layout {
        ("a panel", "A panel under the cue list (Editor view)")
    } else {
        ("a pane", "A pane, beside the cue list and the other panes")
    };
    ui.horizontal_wrapped(|ui| {
        ui.label("It opens as");
        for (layout, words, tip) in [
            (L::Pane, pane.0, pane.1),
            (
                L::Workspace,
                "a workspace",
                "A workspace of its own: bin, picture, inspector and the tracks across the window",
            ),
        ] {
            let r = ui
                .radio(st.prefs.timeline_layout == layout, words)
                .on_hover_text(format!("{tip}. {where_}"));
            #[cfg(test)]
            let seen = if ui.clip_rect().contains_rect(r.rect) {
                r.rect
            } else {
                egui::Rect::NOTHING
            };
            #[cfg(test)]
            ui.data_mut(|d| {
                d.get_temp_mut_or_default::<Vec<(bool, egui::Rect)>>(egui::Id::new(
                    "settings_timeline_layout",
                ))
                .push((layout == L::Workspace, seen))
            });
            if r.clicked() && st.prefs.timeline_layout != layout {
                actions.push(Action::Timeline(
                    super::timeline::TimelineAction::SetLayout(layout),
                ));
            }
        }
    });
}

/// How much the log says: a setting of this computer, used at once.
fn log_detail(ui: &mut Ui, st: &mut UiState) {
    use crate::logging::LogLevel;
    let forced = crate::logging::forced();
    let mut level = forced.unwrap_or(st.prefs.log_level);
    ui.horizontal(|ui| {
        ui.label("Log detail");
        ui.add_enabled_ui(forced.is_none(), |ui| {
            egui::ComboBox::from_id_salt("log-detail")
                .selected_text(level.label())
                .show_ui(ui, |ui| {
                    for l in LogLevel::ALL {
                        ui.selectable_value(&mut level, l, l.label())
                            .on_hover_text(l.summary());
                    }
                });
        });
    });
    if forced.is_none() && level != st.prefs.log_level {
        st.prefs.log_level = level;
        crate::logging::set_level(level);
        if let Some(dir) = &st.layout_dir
            && let Err(e) = st.prefs.save(dir)
        {
            log::warn!("could not save the settings of this computer: {e}");
        }
    }
    hint(ui, level.summary());
    if forced.is_some() {
        warning(
            ui,
            "Set by NEWQ_LOG in the environment for this run; the choice here is used without it.",
        );
    } else if level >= LogLevel::Debug {
        hint(
            ui,
            "Used from now on, with no restart. Put it back to Normal once the fault is found: \
             the fuller levels write a great deal during a show.",
        );
    }
}

/// The show's files: whether they are all here, and the folders every
/// computer of a show network keeps.
fn media_page(ui: &mut Ui, show: &mut Show, st: &mut UiState, actions: &mut Vec<Action>) {
    let files = show
        .cues
        .iter()
        .filter_map(|c| c.kind.file_media())
        .filter(|m| !m.file.as_os_str().is_empty())
        .count()
        + relink::venue_files(show, &st.venue_side).len();
    let missing = st.missing_count(show);
    section(ui, &FILES.heading(), |ui| {
        ui.horizontal(|ui| {
            if files == 0 {
                ui.label(RichText::new("No cue plays a file yet.").weak());
            } else if missing == 0 {
                ui.label(RichText::new(format!("All {files} files are in place.")).color(GO_GREEN));
            } else {
                ui.label(
                    RichText::new(format!("{missing} of {files} files can't be found."))
                        .color(STOP_RED),
                );
            }
        });
        ui.add_space(4.0);
        if ui
            .button("Open the Media manager…")
            .on_hover_text("Find moved files, or gather every file into one folder.")
            .clicked()
        {
            actions.push(Action::ToggleMedia);
        }
        hint(
            ui,
            "Files are saved in the show relative to it, so a show and its media can be moved \
             together.",
        );
    });
    super::net_media::library(ui, show, st, actions);
}

fn sync_page(ui: &mut Ui, show: &mut Show, st: &mut UiState, actions: &mut Vec<Action>) {
    if let Some(rx) = &st.sync_clip {
        match rx.try_recv() {
            Ok(Ok(path)) => {
                st.sync_clip = None;
                actions.push(Action::AddSyncCue(path));
            }
            Ok(Err(e)) => {
                st.sync_clip = None;
                st.sync_note = format!("Couldn't make the test clip: {e}");
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => st.sync_clip = None,
        }
    }
    let delay = |ui: &mut Ui, ms: &mut u32| {
        ui.add(
            egui::DragValue::new(ms)
                .range(0..=MAX_DELAY_MS)
                .speed(1.0)
                .suffix(" ms"),
        )
        .changed()
    };
    section(ui, "Everything", |ui| {
        ui.label(
            "Hold back whichever comes first. If the picture is ahead of the sound, delay the \
             picture; if the sound is ahead, delay the sound.",
        );
        egui::Grid::new("sync_all").num_columns(2).show(ui, |ui| {
            ui.label("Sound delay");
            delay(ui, &mut show.sync.audio_ms);
            ui.end_row();
            ui.label("Picture delay");
            delay(ui, &mut show.sync.video_ms);
            ui.end_row();
        });
        hint(
            ui,
            "These apply to every display and audio output. A delay changes cues as they start.",
        );
    });
    if !show.audio_outputs.is_empty() || !show.outputs.is_empty() {
        section(ui, "One at a time", |ui| {
            ui.label("On top of that, for one display or audio output (a slow projector, a Bluetooth speaker):");
            egui::Grid::new("sync_outputs")
                .num_columns(2)
                .show(ui, |ui| {
                    for o in &mut show.audio_outputs {
                        ui.label(format!("Sound: {}", o.name));
                        delay(ui, &mut o.delay_ms);
                        ui.end_row();
                    }
                    for o in &mut show.outputs {
                        ui.label(format!("Picture: {}", o.name));
                        delay(ui, &mut o.delay_ms);
                        ui.end_row();
                    }
                });
        });
    }
    section(ui, "Test it", |ui| {
        ui.horizontal(|ui| {
            let making = st.sync_clip.is_some();
            if ui
                .add_enabled(!making, egui::Button::new("Add a sync test cue"))
                .on_hover_text(
                    "A looping clip that flashes white and beeps together once a second. Play \
                     it and adjust until the flash and the beep land together.",
                )
                .clicked()
            {
                actions.push(Action::MakeSyncClip);
            }
            if making {
                ui.spinner();
            }
            if ui.button("Reset all delays").clicked() {
                show.sync = SyncSettings::default();
                for o in &mut show.audio_outputs {
                    o.delay_ms = 0;
                }
                for o in &mut show.outputs {
                    o.delay_ms = 0;
                }
            }
        });
        if !st.sync_note.is_empty() {
            ui.label(RichText::new(&st.sync_note).color(STOP_RED).small());
        }
    });
}

/// A line saying how a listener is doing: a problem, what it is doing, or
/// that it is starting.
fn listener_status(
    ui: &mut Ui,
    error: &Option<String>,
    working: Option<String>,
    starting: Option<&str>,
) {
    match (working, error) {
        (_, Some(e)) => {
            ui.label(RichText::new(format!("⚠ {e}")).color(STOP_RED));
        }
        (Some(text), None) => {
            ui.label(RichText::new(text).color(GO_GREEN));
        }
        (None, None) => {
            if let Some(text) = starting {
                ui.label(RichText::new(text).weak());
            }
        }
    }
}

fn osc_page(ui: &mut Ui, show: &mut Show, st: &UiState) {
    let status = &st.remote_status;
    let r = &mut show.remote;
    section(ui, "OSC over UDP", |ui| {
        ui.checkbox(&mut r.osc, "Listen for OSC");
        if r.osc {
            ui.horizontal(|ui| {
                ui.label("Port");
                ui.add(
                    egui::DragValue::new(&mut r.osc_port)
                        .range(1..=65535)
                        .speed(1.0),
                );
            });
            listener_status(
                ui,
                &status.osc_error,
                status
                    .osc_port
                    .map(|p| format!("Listening on UDP port {p}")),
                Some("Starting…"),
            );
            warning(
                ui,
                "Anyone who can reach this port on your network can control the show.",
            );
        }
    });
    section(ui, "Feedback", |ui| {
        ui.label(
            RichText::new(
                "Send the show's state to a controller (a TouchOSC page, a desk): the playhead, \
                 which cues are playing and how far on they are.",
            )
            .weak(),
        );
        ui.horizontal(|ui| {
            ui.label("Send to");
            ui.add(
                egui::TextEdit::singleline(&mut r.osc_feedback)
                    .hint_text("192.168.1.20:9000 (empty = off)")
                    .desired_width(220.0),
            );
        });
        if !r.osc_feedback.trim().is_empty() {
            if let Some(e) = &st.feedback_error {
                warning(ui, e);
            }
            ui.label(
                RichText::new(
                    "/newq/playhead <number> <name>   /newq/paused <0|1>   /newq/running <n>\n\
                     /newq/cue/<number>/status <idle|prewait|playing>\n\
                     /newq/cue/<number>/progress <0 to 1>",
                )
                .monospace()
                .small(),
            );
        }
    });
    if r.osc {
        section(ui, "What it understands", |ui| {
            ui.label(
                RichText::new(
                    "/go  /panic  /stop  /pause  /resume\n/playhead/next  /playhead/previous\n\
                     /cue/<number>/start  go  select  stop  fade",
                )
                .monospace()
                .small(),
            );
        });
    }
}

fn midi_page(ui: &mut Ui, show: &mut Show, st: &UiState, engine: &Engine) {
    let status = &st.remote_status;
    let r = &mut show.remote;
    section(ui, "MIDI input", |ui| {
        ui.checkbox(&mut r.midi, "Listen to a MIDI input");
        if r.midi {
            egui::Grid::new("settings_midi")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Input");
                    egui::ComboBox::from_id_salt("midi_port")
                        .selected_text(r.midi_port.clone().unwrap_or_else(|| "Choose…".into()))
                        .width(240.0)
                        .show_ui(ui, |ui| {
                            for p in &st.midi_ports {
                                ui.selectable_value(&mut r.midi_port, Some(p.clone()), p);
                            }
                        });
                    ui.end_row();
                    ui.label("Channel");
                    let text = r
                        .midi_channel
                        .map_or("Any".to_string(), |c| (c + 1).to_string());
                    egui::ComboBox::from_id_salt("midi_channel")
                        .selected_text(text)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut r.midi_channel, None, "Any");
                            for c in 0..16u8 {
                                ui.selectable_value(
                                    &mut r.midi_channel,
                                    Some(c),
                                    (c + 1).to_string(),
                                );
                            }
                        });
                    ui.end_row();
                    ui.label("Show control device");
                    let text = r.msc_device.map_or("Any".to_string(), |d| d.to_string());
                    egui::ComboBox::from_id_salt("msc_device")
                        .selected_text(text)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut r.msc_device, None, "Any");
                            for d in 0..=111u8 {
                                ui.selectable_value(&mut r.msc_device, Some(d), d.to_string());
                            }
                        })
                        .response
                        .on_hover_text(
                            "Act only on MIDI Show Control sent to this device id (and to \
                             everyone, 127), when several machines share a MIDI line.",
                        );
                    ui.end_row();
                });
            listener_status(
                ui,
                &status.midi_error,
                status
                    .midi_port
                    .as_ref()
                    .map(|p| format!("Listening to {p}")),
                None,
            );
            hint(
                ui,
                "Note on N: fire the cue given that note, else cue N · Program change N: select \
                 cue N\nMIDI Show Control: GO, STOP, RESUME, LOAD, ALL OFF, RESET (cue numbers work)",
            );
        }
    });
    if r.midi {
        section(ui, "MIDI Time Code", |ui| {
            ui.checkbox(&mut r.mtc, "Follow MIDI Time Code")
                .on_hover_text("Fire cues that have a timecode as the incoming time reaches it.");
            if r.mtc {
                match engine.timecode.position() {
                    Some((tc, rate)) if engine.timecode.running() => ui
                        .label(RichText::new(format!("{tc}  ·  {}", rate.label())).color(GO_GREEN)),
                    Some((tc, _)) => ui.label(RichText::new(format!("{tc}  ·  stopped")).weak()),
                    None => ui.label(RichText::new("Waiting for timecode…").weak()),
                };
            }
        });
    }
}

fn web_page(ui: &mut Ui, show: &mut Show, st: &UiState) {
    let status = &st.remote_status;
    let r = &mut show.remote;
    section(ui, "Web remote", |ui| {
        ui.checkbox(&mut r.web, "Serve the web remote")
            .on_hover_text("Serves a page that runs the show from a browser on this network.");
        if r.web {
            egui::Grid::new("settings_web")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Port");
                    ui.add(
                        egui::DragValue::new(&mut r.web_port)
                            .range(1..=65535)
                            .speed(1.0),
                    );
                    ui.end_row();
                    ui.label("PIN");
                    ui.add(
                        egui::TextEdit::singleline(&mut r.web_pin)
                            .password(true)
                            .desired_width(90.0)
                            .hint_text("none"),
                    );
                    ui.end_row();
                });
            // What the page can send: no spaces or accented letters.
            let clean = crate::web::clean_pin(&r.web_pin);
            if clean != r.web_pin {
                r.web_pin = clean;
            }
            let host = crate::web::lan_address()
                .map_or("<this computer>".to_string(), |ip| ip.to_string());
            listener_status(
                ui,
                &status.web_error,
                status
                    .web_port
                    .map(|p| format!("Open http://{host}:{p} on a phone")),
                Some("Starting…"),
            );
            warning(
                ui,
                "GO, Stop All, Pause, the playhead and a button for every cue. Anyone on your \
                 network who can reach the port (and has the PIN, if you set one) can run the \
                 show. The PIN is saved in the show file.",
            );
        }
    });
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
