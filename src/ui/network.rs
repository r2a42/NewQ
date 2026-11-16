//! Settings ▸ Show network (D1, D9): the switch that puts this computer in a
//! show network, the button that runs one from it, and on the primary a row
//! per computer with what it reported and what can be done to it. Everything
//! is asked of the network's own thread through `Action::Net`; what is shown
//! is the `NetView` it last published, so nothing here waits.

use super::settings::{Badge, badge as make_badge, hint, section, warning};
use super::*;
use crate::net::hub::{NetAction, NetView, PeerView};
use crate::net::node::{NodeId, Role};
use crate::net::proto::{self, Link};

/// The page's fields being typed in.
#[derive(Default)]
pub struct Editing {
    address: String,
    passcode: String,
    /// A computer being renamed, and the name so far.
    renaming: Option<(NodeId, String)>,
}

/// For the tests: where a control of the page was drawn.
#[cfg(test)]
fn note(ui: &Ui, name: impl Into<String>, rect: egui::Rect) {
    let name = name.into();
    // Only what can be seen (not scrolled away) can be clicked.
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

/// The sidebar's badge: nothing while off; on the primary, the computers of
/// the network online of all; elsewhere whether the primary is reached.
pub fn badge(v: &NetView) -> Option<Badge> {
    let weak = Color32::from_gray(140);
    if !v.me.on {
        return None;
    }
    if v.problem.is_some() {
        return make_badge("⚠", STOP_RED);
    }
    if v.is_primary() {
        let ours: Vec<&PeerView> = v.peers.iter().filter(|p| p.ours).collect();
        if ours.is_empty() {
            return make_badge("on", GO_GREEN);
        }
        let online = ours.iter().filter(|p| p.online).count();
        let color = if online == ours.len() {
            GO_GREEN
        } else {
            STOP_RED
        };
        return make_badge(format!("{online}/{}", ours.len()), color);
    }
    match &v.following {
        Some(f) if f.online => make_badge("on", GO_GREEN),
        Some(_) => make_badge("⚠", STOP_RED),
        None => make_badge("…", weak),
    }
}

/// "120 GB free".
pub fn space(bytes: u64) -> String {
    const GB: f64 = 1_000_000_000.0;
    let gb = bytes as f64 / GB;
    if gb >= 100.0 {
        format!("{gb:.0} GB free")
    } else if gb >= 1.0 {
        format!("{gb:.1} GB free")
    } else {
        format!("{:.0} MB free", bytes as f64 / 1_000_000.0)
    }
}

pub fn page(ui: &mut Ui, show: &Show, st: &mut UiState, actions: &mut Vec<Action>) {
    let v = st.net.clone();
    this_computer(ui, &v, st, actions);
    if v.is_primary() {
        super::net_media::primary_summary(ui, &v, st);
        computers(ui, show, &v, st, actions);
        by_address(ui, &v, st, actions);
        passcode(ui, &v, st, actions);
    } else if v.me.on && !v.me.adopted() {
        waiting(ui, &v, st, actions);
    }
    super::net_media::own(ui, st);
}

fn this_computer(ui: &mut Ui, v: &NetView, st: &mut UiState, actions: &mut Vec<Action>) {
    section(ui, "This computer", |ui| {
        let mut on = v.me.on;
        let r = ui.checkbox(&mut on, "This computer is part of a show network");
        note(ui, "switch", r.rect);
        if r.changed() {
            actions.push(Action::Net(NetAction::SetOn(on)));
        }
        if !v.me.on {
            hint(
                ui,
                "Off, NewQ opens nothing on the network. On, it announces itself so that the \
                 computer running the show can find it and give it a role; nothing more needs \
                 doing here. (`newq --join` on the command line does the same.)",
            );
        }
        if !v.me.adopted() && !v.is_primary() {
            let r = ui
                .button("Run the show network from this computer")
                .on_hover_text(
                    "This computer becomes the primary: it finds the others, adds them to the \
                     show and gives them their roles. (`newq --primary` does the same.)",
                );
            note(ui, "become_primary", r.rect);
            if r.clicked() {
                actions.push(Action::Net(NetAction::BecomePrimary));
            }
        }
        if let Some(p) = &v.problem {
            warning(ui, &format!("⚠ {p}"));
        }
        if !v.me.on {
            return;
        }
        // What it is, in a sentence.
        let said = if v.is_primary() {
            RichText::new(format!(
                "This computer runs the show network{}.",
                v.port.map(|p| format!(" (port {p})")).unwrap_or_default()
            ))
            .color(GO_GREEN)
        } else if let Some(f) = &v.following {
            let text = format!(
                "This computer is {} in the show network run from {}{}",
                v.me.role.described(),
                f.name,
                if f.online {
                    "."
                } else {
                    ", which can't be reached now."
                }
            );
            RichText::new(text).color(if f.online { GO_GREEN } else { STOP_RED })
        } else if v.me.role == Role::Primary {
            RichText::new("Starting…").weak()
        } else {
            RichText::new("Waiting for a primary to add this computer.").weak()
        };
        ui.label(said);
        if v.me.adopted() {
            hint(
                ui,
                "Its role and name are set from the primary; nothing needs doing here. It plays \
                 the primary's show in step with it, on the displays and sound outputs given to \
                 it. Switching off leaves the show network.",
            );
            if let Some(s) = &v.status.standing {
                ui.label(RichText::new(s).strong());
            }
        }
        // Start at login (D8): on by itself for a display computer.
        let mut at_login = st.start_at_login;
        let r = ui
            .checkbox(&mut at_login, "Start NewQ when this computer starts")
            .on_hover_text(
                "For a computer out of reach (a display computer in a truss): it comes back by \
                 itself after a power cut. The primary can set this too.",
            );
        note(ui, "start_at_login", r.rect);
        if r.changed() {
            st.start_at_login_request = Some(at_login);
        }
        // Its name: what the others call it.
        ui.horizontal(|ui| {
            ui.label("Name");
            if v.me.adopted() {
                // Named from the primary.
                ui.label(RichText::new(&v.name).strong());
            } else {
                rename_field(ui, v.me.id, &v.name, v, st, actions);
            }
        });
    });
}

/// A name and a way to change it (this computer's, or on the primary another's).
fn rename_field(
    ui: &mut Ui,
    id: NodeId,
    name: &str,
    v: &NetView,
    st: &mut UiState,
    actions: &mut Vec<Action>,
) {
    let editing = st.net_edit.renaming.as_ref().is_some_and(|(r, _)| *r == id);
    if editing {
        let Some((_, text)) = st.net_edit.renaming.as_mut() else {
            return;
        };
        let r = ui.add(egui::TextEdit::singleline(text).desired_width(180.0));
        note(ui, format!("name_field:{id}"), r.rect);
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        let ok = ui.small_button("OK");
        note(ui, format!("name_ok:{id}"), ok.rect);
        if enter || ok.clicked() {
            let text = text.clone();
            st.net_edit.renaming = None;
            actions.push(Action::Net(NetAction::Rename(id, text)));
        } else if ui.small_button("Cancel").clicked() {
            st.net_edit.renaming = None;
        }
    } else {
        ui.label(RichText::new(name).strong());
        let r = ui.small_button("Rename").on_hover_text(if id == v.me.id {
            "What the other computers call this one. Empty: the computer's own name."
        } else {
            "Renamed on that computer, from here. Empty: its own name."
        });
        note(ui, format!("rename:{id}"), r.rect);
        if r.clicked() {
            st.net_edit.renaming = Some((id, name.to_string()));
        }
    }
}

fn computers(ui: &mut Ui, show: &Show, v: &NetView, st: &mut UiState, actions: &mut Vec<Action>) {
    let ours: Vec<&PeerView> = v.peers.iter().filter(|p| p.ours).collect();
    let waiting: Vec<&PeerView> = v
        .peers
        .iter()
        .filter(|p| !p.ours && p.waiting && p.compatible)
        .collect();
    let others: Vec<&PeerView> = v
        .peers
        .iter()
        .filter(|p| !p.ours && !(p.waiting && p.compatible))
        .collect();
    section(ui, "Computers in this show", |ui| {
        own_row(ui, v);
        for p in &ours {
            ui.separator();
            computer_row(ui, show, p, v, st, actions);
        }
        if ours.is_empty() {
            hint(
                ui,
                "No other computer is in the show yet. On each computer that is to take part, \
                 open Settings ▸ Show network and switch it on (or start it with newq --join): \
                 it appears below, ready to add.",
            );
        }
    });
    if !waiting.is_empty() {
        section(ui, "Waiting to join", |ui| {
            for p in &waiting {
                ui.horizontal(|ui| {
                    dot(ui, p.online);
                    ui.label(RichText::new(&p.name).strong());
                    if let Some(a) = p.addr {
                        ui.label(RichText::new(a.to_string()).weak().small());
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let r = ui.add_enabled(
                            !p.adopting,
                            egui::Button::new(if p.adopting {
                                "Adding…"
                            } else {
                                "Add to the show"
                            }),
                        );
                        note(ui, format!("adopt:{}", p.id), r.rect);
                        if r.clicked() {
                            actions.push(Action::Net(NetAction::Adopt(p.id)));
                        }
                    });
                });
                if let Some(t) = &p.problem {
                    warning(ui, &format!("⚠ {t}"));
                }
            }
            hint(
                ui,
                "A computer added becomes a display computer of this show; its role can be \
                 changed afterwards. It remembers this network, and follows no other until it is \
                 released.",
            );
        });
    }
    if !others.is_empty() {
        section(ui, "Other computers on the network", |ui| {
            for p in &others {
                ui.horizontal_wrapped(|ui| {
                    dot(ui, p.online);
                    ui.label(&p.name);
                    let why = p.problem.clone().unwrap_or_else(|| {
                        if p.network.is_some() {
                            "in another show network".into()
                        } else {
                            "not switched on".into()
                        }
                    });
                    ui.label(RichText::new(why).weak().small());
                });
            }
        });
    }
}

/// A green or grey dot: online or not.
fn dot(ui: &mut Ui, online: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
    ui.painter().circle_filled(
        rect.center(),
        4.5,
        if online {
            GO_GREEN
        } else {
            Color32::from_gray(110)
        },
    );
}

/// The primary itself, first in the list.
fn own_row(ui: &mut Ui, v: &NetView) {
    ui.horizontal(|ui| {
        dot(ui, true);
        ui.label(RichText::new(&v.name).strong());
        ui.label(RichText::new("Primary · this computer").weak());
    });
    ui.label(RichText::new(facts(&v.status, None)).small().weak());
}

/// One computer of the network: name, role, online, its facts and what can
/// be done to it.
fn computer_row(
    ui: &mut Ui,
    show: &Show,
    p: &PeerView,
    v: &NetView,
    st: &mut UiState,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        dot(ui, p.online);
        rename_field(ui, p.id, &p.name, v, st, actions);
        let usable = p.connected;
        ui.add_enabled_ui(usable, |ui| {
            let r = egui::ComboBox::from_id_salt(("net_role", p.id.0))
                .selected_text(p.role.label())
                .width(90.0)
                .show_ui(ui, |ui| {
                    for role in Role::GIVEN {
                        let r = ui.selectable_label(p.role == role, role.label());
                        note(ui, format!("role:{}:{}", p.id, role.label()), r.rect);
                        if r.clicked() && p.role != role {
                            actions.push(Action::Net(NetAction::SetRole(p.id, role)));
                        }
                    }
                })
                .response;
            note(ui, format!("role_menu:{}", p.id), r.rect);
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_enabled_ui(usable, |ui| {
                let r = ui
                    .small_button("Release")
                    .on_hover_text("Take it out of this show network: it goes back to waiting.");
                note(ui, format!("release:{}", p.id), r.rect);
                if r.clicked() {
                    actions.push(Action::Net(NetAction::Release(p.id)));
                }
                let r = ui
                    .small_button("Identify")
                    .on_hover_text("Show its name big on its screen for a few seconds.");
                note(ui, format!("identify:{}", p.id), r.rect);
                if r.clicked() {
                    actions.push(Action::Net(NetAction::Identify(p.id)));
                }
                let r = ui
                    .small_button("Restart")
                    .on_hover_text("Start NewQ again on that computer.");
                note(ui, format!("restart:{}", p.id), r.rect);
                if r.clicked() {
                    actions.push(Action::Net(NetAction::Restart(p.id)));
                }
            });
        });
    });
    // ---- Playing together (stage 4a): in step, clock, start at login ----
    if p.connected
        && let Some(s) = &p.status
    {
        ui.horizontal_wrapped(|ui| {
            let (text, color) = step_words(s);
            let r = ui.label(RichText::new(text).color(color).small());
            note(ui, format!("step:{}", p.id), r.rect);
            let mut at_login = s.start_at_login;
            let r = ui.checkbox(&mut at_login, RichText::new("Starts at login").small());
            note(ui, format!("at_login:{}", p.id), r.rect);
            if r.changed() {
                actions.push(Action::Net(NetAction::StartAtLogin(p.id, at_login)));
            }
        });
        if let Some(w) = blend_warning(show, p, s) {
            warning(ui, &w);
        }
    }
    let state = if !p.online {
        RichText::new("Offline: not heard from")
            .color(STOP_RED)
            .small()
    } else if !p.connected {
        RichText::new("Connecting…").weak().small()
    } else {
        RichText::new(facts(&p.status.clone().unwrap_or_default(), Some(p)))
            .weak()
            .small()
    };
    ui.label(state);
    if let Some(s) = &p.status
        && s.version != proto::VERSION
        && !s.version.is_empty()
    {
        warning(
            ui,
            &format!(
                "NewQ {} there, {} here: the same version on every computer is safest.",
                s.version,
                proto::VERSION
            ),
        );
    }
    if p.status.as_ref().is_some_and(|s| s.link == Link::WiFi) {
        hint(
            ui,
            "On Wi-Fi: it works, but a cable keeps the computers closer in step.",
        );
    }
    if let Some(t) = &p.problem {
        warning(ui, &format!("⚠ {t}"));
    }
    super::net_media::computer_rows(ui, p, st, actions);
}

/// One line of what a computer reported.
fn facts(s: &proto::StatusReport, p: Option<&PeerView>) -> String {
    let mut parts = Vec::new();
    if !s.version.is_empty() {
        parts.push(format!("NewQ {}", s.version));
    }
    if s.link != Link::Unknown {
        parts.push(s.link.label().to_string());
    }
    if let Some(a) = p.and_then(|p| p.addr) {
        parts.push(a.to_string());
    }
    if let Some(b) = s.disk_free {
        parts.push(space(b));
    }
    match s.monitors.len() {
        0 => parts.push("no monitors".into()),
        _ => parts.push(
            s.monitors
                .iter()
                .map(|m| format!("{} {}×{}", m.identity, m.size.0, m.size.1))
                .collect::<Vec<_>>()
                .join(", "),
        ),
    }
    match s.audio_devices.len() {
        0 => parts.push("no sound devices".into()),
        1 => parts.push("1 sound device".into()),
        n => parts.push(format!("{n} sound devices")),
    }
    if let Some(e) = s.clock_error_ms
        && p.is_some()
    {
        parts.push(format!("clock ±{e:.2} ms"));
    }
    parts.join(" · ")
}

/// Whether a computer's playing agrees with the primary's, in words and colour.
fn step_words(s: &proto::StatusReport) -> (String, Color32) {
    if s.in_step && s.clock_locked {
        return ("Show in step".into(), GO_GREEN);
    }
    let text = s
        .standing
        .clone()
        .unwrap_or_else(|| "Not in step yet".into());
    (text, Color32::from_rgb(220, 160, 40))
}

/// Half a frame at 60 Hz (ms): a clock further off than this can show a
/// blended picture's halves a frame apart.
const HALF_FRAME_MS: f64 = 1000.0 / 120.0;

/// A plain warning when computer `p` shares a blended picture (an edge blend,
/// or a part of a picture another computer shows too) and is on Wi-Fi or its
/// clock is more than half a frame off (ruling R2).
pub fn blend_warning(show: &Show, p: &PeerView, s: &proto::StatusReport) -> Option<String> {
    let mine = |o: &crate::show::OutputConfig| o.computer == Some(p.id);
    let shares = show.outputs.iter().filter(|o| o.enabled).any(|o| {
        let blended = o.blend != crate::show::EdgeBlend::default();
        let part = show.part_of(o.id).and_then(|pt| show.output(pt.display));
        let across = part.is_some_and(|src| mine(src) != mine(o));
        let shown_elsewhere = show
            .outputs
            .iter()
            .any(|x| show.part_of(x.id).is_some_and(|pt| pt.display == o.id) && mine(x) != mine(o));
        (mine(o) && blended) || ((mine(o) || part.is_some_and(mine)) && (across || shown_elsewhere))
    });
    if !shares {
        return None;
    }
    if s.link == Link::WiFi {
        return Some(format!(
            "⚠ {} shares a blended picture and is on Wi-Fi: its half may be a frame or more \
             apart from the other's. A cable keeps them together.",
            p.name
        ));
    }
    s.clock_error_ms.filter(|e| *e > HALF_FRAME_MS).map(|e| {
        format!(
            "⚠ {} shares a blended picture and its clock may be {e:.0} ms off: its half may \
             be a frame apart from the other's.",
            p.name
        )
    })
}

fn by_address(ui: &mut Ui, v: &NetView, st: &mut UiState, actions: &mut Vec<Action>) {
    section(ui, "Add by address", |ui| {
        ui.horizontal(|ui| {
            let r = ui.add(
                egui::TextEdit::singleline(&mut st.net_edit.address)
                    .hint_text("192.168.1.20")
                    .desired_width(200.0),
            );
            note(ui, "address", r.rect);
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            let add = ui.button("Add");
            note(ui, "add_address", add.rect);
            if (enter || add.clicked()) && !st.net_edit.address.trim().is_empty() {
                actions.push(Action::Net(NetAction::AddAddress(std::mem::take(
                    &mut st.net_edit.address,
                ))));
            }
        });
        for m in &v.manual {
            ui.horizontal(|ui| {
                ui.label(&m.text);
                if let Some(n) = &m.note {
                    ui.label(RichText::new(n).weak().small());
                }
                if ui.small_button("✕").on_hover_text("Forget it").clicked() {
                    actions.push(Action::Net(NetAction::RemoveAddress(m.text.clone())));
                }
            });
        }
        hint(
            ui,
            "Computers are found by themselves on most networks. Where they aren't (a network \
             that drops broadcasts), type the other computer's address here.",
        );
    });
}

fn passcode(ui: &mut Ui, v: &NetView, st: &mut UiState, actions: &mut Vec<Action>) {
    section(ui, "Passcode (optional)", |ui| {
        passcode_field(ui, v, st, actions);
        hint(
            ui,
            "Only for a network that carries more than one show. With a passcode, a computer \
             of this show obeys no primary that doesn't have it. The computers already in the \
             show are given it at once.",
        );
    });
}

fn waiting(ui: &mut Ui, v: &NetView, st: &mut UiState, actions: &mut Vec<Action>) {
    section(ui, "Joining", |ui| {
        ui.label(
            "The first primary that adds this computer runs it from then on; nothing needs \
             typing here.",
        );
        ui.add_space(4.0);
        ui.label(RichText::new("If the show network has a passcode:").small());
        passcode_field(ui, v, st, actions);
        hint(
            ui,
            "With a passcode typed here, only a primary that has the same one can add this \
             computer.",
        );
    });
}

fn passcode_field(ui: &mut Ui, v: &NetView, st: &mut UiState, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        let r = ui.add(
            egui::TextEdit::singleline(&mut st.net_edit.passcode)
                .password(true)
                .hint_text(if v.me.passcode.is_some() {
                    "(set)"
                } else {
                    "none"
                })
                .desired_width(160.0),
        );
        note(ui, "passcode", r.rect);
        let set = ui.add_enabled(
            !st.net_edit.passcode.trim().is_empty(),
            egui::Button::new("Set"),
        );
        note(ui, "set_passcode", set.rect);
        if set.clicked() {
            actions.push(Action::Net(NetAction::SetPasscode(Some(std::mem::take(
                &mut st.net_edit.passcode,
            )))));
        }
        if v.me.passcode.is_some() && ui.button("Remove").clicked() {
            actions.push(Action::Net(NetAction::SetPasscode(None)));
        }
    });
    if v.me.passcode.is_some() {
        ui.label(RichText::new("A passcode is set.").color(GO_GREEN).small());
    }
}

/// A word in the status bar while this computer is in a show network.
pub fn status(ui: &mut Ui, st: &UiState) {
    let v = &st.net;
    if !v.me.on {
        return;
    }
    let (text, color, tip) = if v.is_primary() {
        let ours = v.peers.iter().filter(|p| p.ours).count();
        let online = v.peers.iter().filter(|p| p.ours && p.online).count();
        (
            format!("Show network: primary · {online}/{ours} computers"),
            if online == ours { GO_GREEN } else { STOP_RED },
            "This computer runs the show network. Settings ▸ Show network.".to_string(),
        )
    } else if let Some(f) = &v.following {
        (
            format!("Show network: {} · run from {}", v.me.role.label(), f.name),
            if f.online { GO_GREEN } else { STOP_RED },
            if f.online {
                format!(
                    "This computer is {} of the show run from {}.",
                    v.me.role.described(),
                    f.name
                )
            } else {
                format!("{} can't be reached now.", f.name)
            },
        )
    } else {
        (
            "Show network: waiting for a primary".to_string(),
            Color32::from_gray(150),
            "Switched on: the first primary that adds this computer runs it.".to_string(),
        )
    };
    ui.label(RichText::new(text).color(color))
        .on_hover_text(tip);
}

/// This computer's name, big, while the primary asked it to show it.
pub fn identify(ctx: &egui::Context, st: &UiState) {
    let Some(until) = st.net.identify_until else {
        return;
    };
    if std::time::Instant::now() >= until {
        return;
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(200));
    egui::Area::new(egui::Id::new("net_identify"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(32.0)
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(&st.net.name)
                            .size(64.0)
                            .strong()
                            .color(PLAYHEAD),
                    );
                    ui.label(RichText::new(st.net.me.role.label()).size(24.0));
                });
        });
}

#[cfg(test)]
#[path = "network_tests.rs"]
mod tests;

// ---- Playing together (stage 4a): the show from the primary ----

impl UiState {
    /// The show as the control window last serialised it (a primary sends it).
    pub fn show_json(&self) -> Option<&str> {
        self.json.as_deref()
    }

    /// A follower has been given the primary's show:
    /// it is the saved show from here (nothing to save, no recovery copy), the
    /// undo history starts from it, and a selection of a cue that has gone is
    /// let go. What plays is left alone (the engine copies the live fields of
    /// a playing cue from the show each tick).
    pub fn took_synced_show(&mut self, show: &Show, engine: &mut Engine) {
        // As this computer has it (its files where they are here).
        let json = serde_json::to_string(show).unwrap_or_default();
        engine.sync_lists(show);
        self.saved_json = Some(json.clone());
        self.history.reset(json.clone());
        self.json = Some(json);
        self.json_stale = false;
        if self.selected.is_some_and(|id| show.cue(id).is_none()) {
            self.selected = engine.playhead.filter(|id| show.cue(*id).is_some());
        }
        self.forget_multi();
        self.capture_hotkey = None;
    }
}

/// A computer that follows a primary (a display computer's hidden window, a
/// backup's): its controls are locked, the show is run from the primary.
pub fn locked(st: &UiState) -> bool {
    st.net.me.on && st.net.me.adopted()
}

/// The line across the top of a follower's window: who runs the show, where
/// this computer stands, and the one thing that can be done here.
pub fn locked_banner(ui: &mut Ui, st: &UiState, actions: &mut Vec<Action>) {
    let v = &st.net;
    let who = v
        .following
        .as_ref()
        .map_or_else(|| "The primary".to_string(), |f| f.name.clone());
    ui.horizontal_wrapped(|ui| {
        let what = if v.me.role == Role::Backup {
            "this computer is its backup, following its show"
        } else {
            "this computer follows its show"
        };
        let r = ui.label(
            RichText::new(format!(
                "🔒 {who} runs the show: {what}, and its controls are locked here."
            ))
            .strong(),
        );
        note(ui, "locked_banner", r.rect);
        if let Some(s) = &v.status.standing {
            ui.label(RichText::new(s).weak());
        }
        if v.following.as_ref().is_some_and(|f| !f.online) {
            ui.label(
                RichText::new(format!("{who} can't be reached: playing on by its clock."))
                    .color(STOP_RED),
            );
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let r = ui
                .button("Leave the show network")
                .on_hover_text("Switch this computer out of the show network: it plays alone.");
            note(ui, "leave_network", r.rect);
            if r.clicked() {
                actions.push(Action::Net(NetAction::SetOn(false)));
            }
        });
    });
}
