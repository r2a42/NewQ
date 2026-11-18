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
    /// This computer took control (stage 5): who from, for the page's note.
    pub took_control_from: Option<String>,
}

/// An address as the page writes it: "10.0.0.2", with its port only when it
/// isn't the usual one ("10.0.0.2, port 47821").
pub fn address_text(ip: std::net::IpAddr, port: Option<u16>) -> String {
    match port.filter(|p| *p != 0 && *p != crate::net::discover::DEFAULT_PORT) {
        Some(p) => format!("{ip}, port {p}"),
        None => ip.to_string(),
    }
}

/// Where computer `id` is: the address its session is connected on, else the
/// one its announcements come from, else (offline) the last one known. The
/// flag says it is a last known address. Pure, for the tests.
pub fn peer_address(v: &NetView, id: NodeId) -> Option<(String, bool)> {
    if let Some((_, a)) = v.reached.iter().find(|(p, _)| *p == id) {
        return Some((address_text(a.ip(), Some(a.port())), false));
    }
    let p = v.peer(id);
    if let Some(a) = p.filter(|p| p.online).and_then(|p| p.addr) {
        return Some((address_text(a, None), false));
    }
    let k = v.me.known.iter().find(|k| k.id == id)?;
    match k.socket() {
        Some(s) => Some((address_text(s.ip(), Some(s.port())), true)),
        None => {
            let ip: std::net::IpAddr = k.addr.as_deref()?.parse().ok()?;
            Some((address_text(ip, None), true))
        }
    }
    .or_else(|| {
        p.and_then(|p| p.addr)
            .map(|a| (address_text(a, None), true))
    })
}

/// This computer's addresses, one per network adapter, best first:
/// "Wired · 192.168.1.10". Pure, for the tests.
pub fn own_addresses(adapters: &[crate::net::paths::Adapter], port: Option<u16>) -> Vec<String> {
    adapters
        .iter()
        .filter(|a| !a.ip.is_link_local())
        .map(|a| format!("{} · {}", a.kind.label(), address_text(a.ip.into(), port)))
        .collect()
}

/// The way this computer reaches computer `id` (or is reached by it): the
/// kind of our adapter its address is on, e.g. "Wired". Pure, for the tests.
pub fn path_to(v: &NetView, id: NodeId) -> Option<crate::net::paths::AdapterKind> {
    use crate::net::paths;
    let ip = v
        .reached
        .iter()
        .find(|(p, _)| *p == id)
        .map(|(_, a)| a.ip())
        .or_else(|| v.peer(id).and_then(|p| p.addr))?;
    let std::net::IpAddr::V4(v4) = ip else {
        return None;
    };
    if v4.is_loopback() {
        return None;
    }
    v.adapters
        .iter()
        .filter(|a| a.holds(v4))
        .min_by_key(|a| paths::rank(a.ip.into(), &v.adapters))
        .map(|a| a.kind)
        .or(Some(paths::AdapterKind::Other))
}

/// The one line said when a VPN is in the way: on this computer, on another,
/// or a connection going through one. None when there is none. Pure, for the
/// tests.
pub fn vpn_words(v: &NetView) -> Option<String> {
    use crate::net::paths::AdapterKind;
    let mut lines = Vec::new();
    let lan = v
        .adapters
        .iter()
        .find(|a| a.kind != AdapterKind::Vpn && !a.ip.is_link_local());
    if crate::net::paths::vpn_on(&v.adapters) {
        lines.push(match lan {
            Some(a) => format!(
                "A VPN is on: it can slow the show network. NewQ is using the {} network ({}).",
                a.kind.label().to_lowercase(),
                a.ip
            ),
            None => "A VPN is on and there is no local network: the show network goes \
                     through the VPN, which can slow it."
                .to_string(),
        });
    }
    for p in v.peers.iter().filter(|p| p.ours) {
        let through = path_to(v, p.id) == Some(AdapterKind::Vpn)
            || p.status.as_ref().and_then(|s| s.path) == Some(AdapterKind::Vpn);
        if through {
            lines.push(format!(
                "{} is connected through a VPN: it can be slow. Give it a cable on the same \
                 network as this computer.",
                p.name
            ));
        } else if p.status.as_ref().is_some_and(|s| s.vpn_on) {
            lines.push(format!(
                "A VPN is on on {}: it can slow the show network there.",
                p.name
            ));
        }
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
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
    if v.problem.is_some() || v.twin.is_some() || v.peers.iter().any(|p| p.twin.is_some()) {
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

/// The start-at-login tick's words: a display computer comes back as one.
pub fn at_login_label(display: bool) -> &'static str {
    if display {
        "Start as a display computer when this computer starts"
    } else {
        "Start NewQ when this computer starts"
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

pub fn page(ui: &mut Ui, show: &mut Show, st: &mut UiState, actions: &mut Vec<Action>) {
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
    // The show's own settings for its display computers (a follower's come
    // from the primary).
    if !v.me.adopted() || v.is_primary() {
        display_cards(ui, show);
    }
    super::net_media::own(ui, st);
}

/// What display computers' screens show when there is nothing to play: the
/// waiting card, and the "Media transferring" card in a cue's place. Both
/// are the show's, so they follow it to every computer.
fn display_cards(ui: &mut Ui, show: &mut Show) {
    section(ui, "Cards on display computers' screens", |ui| {
        ui.horizontal(|ui| {
            ui.label("Show a card while they wait");
            let r = egui::ComboBox::from_id_salt("wait_cards")
                .selected_text(show.wait_cards.label())
                .width(300.0)
                .show_ui(ui, |ui| {
                    for w in crate::show::WaitCards::ALL {
                        let r = ui.selectable_label(show.wait_cards == w, w.label());
                        note(ui, format!("wait_cards:{}", w.label()), r.rect);
                        if r.clicked() {
                            show.wait_cards = w;
                        }
                    }
                })
                .response;
            note(ui, "wait_cards", r.rect);
        });
        hint(
            ui,
            "The waiting card shows a display computer's name, address and how it stands, so it \
             can be found and set up. Once the show is ready on it, a screen with nothing \
             playing is black between cues, whatever is chosen here. \"Never\": black from \
             the start (a computer no show has added yet still shows it). Identify always \
             shows the name.",
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
        // Another NewQ with this identity, here or on a computer it follows.
        if let Some(t) = &v.twin {
            warning(ui, &format!("⚠ {t}"));
        }
        if let Some(t) = v
            .following
            .as_ref()
            .and_then(|f| v.peer(f.id))
            .and_then(|p| p.twin.as_ref())
        {
            warning(ui, &format!("⚠ {t}"));
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
            // The primary's address, so it can be found (or typed in) by hand.
            let at = peer_address(v, f.id)
                .map(|(a, last)| {
                    if last {
                        format!(" (last at {a})")
                    } else {
                        format!(" ({a})")
                    }
                })
                .unwrap_or_default();
            note(ui, format!("primary_at:{at}"), egui::Rect::NOTHING);
            let text = format!(
                "This computer is {} in the show network run from {}{at}{}",
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
        let r = ui.label(said);
        note(ui, "said", r.rect);
        // Its addresses, to read out to whoever sets up the other computers.
        let port = v.port;
        let mine = own_addresses(&v.adapters, port);
        let text = match mine.len() {
            0 => "This computer has no network address yet: plug in a cable or join a network."
                .to_string(),
            1 => format!("This computer's address: {}", mine[0]),
            _ => format!("This computer's addresses: {}", mine.join(" · ")),
        };
        let r = ui.label(RichText::new(&text).small());
        note(ui, format!("own_address:{text}"), r.rect);
        if let Some(w) = vpn_words(v) {
            for l in w.lines() {
                warning(ui, &format!("⚠ {l}"));
                note(ui, format!("vpn:{l}"), egui::Rect::NOTHING);
            }
        }
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
        // Start at login (D8): only when ticked, here or on the primary.
        let display = v.me.role == Role::Display;
        let mut at_login = st.start_at_login;
        let r = ui
            .checkbox(&mut at_login, at_login_label(display))
            .on_hover_text(
                "For a computer out of reach (a display computer in a truss): it comes back by \
                 itself after a power cut. The primary can set this too, from this computer's \
                 row.",
            );
        note(ui, "start_at_login", r.rect);
        if r.changed() {
            st.start_at_login_request = Some(at_login);
        }
        if st.start_at_login {
            hint(
                ui,
                &crate::autostart::where_words(std::env::consts::OS, display),
            );
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
        // ---- Take control (stage 5) ----
        if let Some(from) = &st.net_edit.took_control_from {
            let r = ui.label(
                RichText::new(format!(
                    "This computer took control from {from}. The show it runs is the one {from} \
                     last sent and isn't saved on this computer yet: Save asks where to keep it."
                ))
                .color(Color32::from_rgb(220, 160, 40)),
            );
            note(ui, "took_control_note", r.rect);
        }
        let backups = ours.iter().filter(|p| p.role == Role::Backup).count();
        if backups > 1 {
            hint(
                ui,
                &format!(
                    "{backups} backups: whichever takes control first runs the show, and the \
                     others follow it."
                ),
            );
        }
        for p in &ours {
            ui.separator();
            computer_row(ui, show, p, v, st, actions);
        }
        if ours.is_empty() {
            hint(
                ui,
                "No other computer is in the show yet. On each computer that is to take part, \
                 open NewQ Display (beside NewQ), or open Settings ▸ Show network and switch it \
                 on: it appears below, ready to add.",
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
                if let Some(t) = &p.twin {
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
    let mine = own_addresses(&v.adapters, v.port);
    ui.horizontal_wrapped(|ui| {
        dot(ui, true);
        ui.label(RichText::new(&v.name).strong());
        for a in &mine {
            let r = ui.label(RichText::new(a).monospace());
            note(ui, format!("address:me:{a}"), r.rect);
        }
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
    // Its address (on a line of its own: the row above is full at 1280 wide).
    if let Some((a, last)) = peer_address(v, p.id) {
        let text = match (last, path_to(v, p.id)) {
            (true, _) => format!("last at {a}"),
            (false, Some(k)) => format!("{} · {a}", k.label()),
            (false, None) => a,
        };
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            let r = ui.label(RichText::new(&text).monospace());
            note(ui, format!("address:{}:{text}", p.id), r.rect);
        });
    }
    // ---- Playing together (stage 4a): in step, clock, start at login ----
    if p.connected
        && let Some(s) = &p.status
    {
        ui.horizontal_wrapped(|ui| {
            let (text, color) = step_words(s);
            let r = ui.label(RichText::new(text).color(color).small());
            note(ui, format!("step:{}", p.id), r.rect);
            let mut at_login = s.start_at_login;
            let r = ui
                .checkbox(&mut at_login, RichText::new("Starts at login").small())
                .on_hover_text(format!(
                    "NewQ starts by itself when {} starts{}, so it comes back after a power cut. \
                     Set only here or on that computer, never by itself; untick to remove it \
                     (listed there as \"{}\").",
                    p.name,
                    if p.role == Role::Display {
                        ", as a display computer"
                    } else {
                        ""
                    },
                    crate::autostart::name(p.role == Role::Display)
                ));
            note(ui, format!("at_login:{}", p.id), r.rect);
            if r.changed() {
                actions.push(Action::Net(NetAction::StartAtLogin(p.id, at_login)));
            }
        });
        if let Some(w) = blend_warning(show, p, s) {
            warning(ui, &w);
        }
        for w in slow_drawing(&p.name, s) {
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
    ui.horizontal_wrapped(|ui| {
        ui.label(state);
        // ---- Take control (stage 5): a computer gone for good is forgotten ----
        if !p.online {
            let r = ui.small_button("Forget").on_hover_text(
                "This computer is gone for good: stop looking for it. If it comes back, it \
                 appears here again.",
            );
            note(ui, format!("forget:{}", p.id), r.rect);
            if r.clicked() {
                actions.push(Action::Net(NetAction::Forget(p.id)));
            }
        }
    });
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
    if let Some(t) = &p.twin {
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
    } else if p.is_some() && s.primary.is_some() && !s.clock_locked {
        // A follower's clock not settled reports no figure.
        parts.push("clock not locked".into());
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

/// A plain warning for each of a computer's monitors whose window draws at
/// under half its display's rate (R21: in time, but stuttering).
pub fn slow_drawing(name: &str, s: &proto::StatusReport) -> Vec<String> {
    s.monitors
        .iter()
        .enumerate()
        .filter(|(_, m)| m.drawing_slowly())
        .map(|(i, m)| {
            format!(
                "⚠ {name}'s monitor {} ({}) is drawing {:.0} frames a second: it should draw {:.0}. Its pictures stutter",
                i + 1,
                m.identity,
                m.drawing_fps.unwrap_or(0.0),
                m.refresh_hz.unwrap_or(60.0)
            )
        })
        .collect()
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
    /// This computer took control (stage 5): the show it holds is the one the
    /// old primary last sent. It isn't saved here: it reads as unsaved and
    /// Save asks where to keep it (not over a show file this computer had
    /// open before it followed).
    pub fn took_control(&mut self) {
        self.net_edit.took_control_from = Some(
            self.net
                .took_control_from
                .clone()
                .or_else(|| self.net.following.as_ref().map(|f| f.name.clone()))
                .or_else(|| self.net.me.primary_name.clone())
                .unwrap_or_else(|| "the old primary".into()),
        );
        self.show_path = None;
        // Unlike any show: unsaved until saved.
        self.saved_json = Some(String::new());
        self.dirty = true;
    }

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
pub fn locked_banner(ui: &mut Ui, st: &mut UiState, actions: &mut Vec<Action>) {
    let net = st.net.clone();
    let v = &*net;
    let who = v
        .following
        .as_ref()
        .map_or_else(|| "The primary".to_string(), |f| f.name.clone());
    // Its address too: what a follower can be asked for over the phone.
    let at = v
        .following
        .as_ref()
        .and_then(|f| peer_address(v, f.id))
        .map(|(a, _)| format!(" ({a})"))
        .unwrap_or_default();
    ui.horizontal_wrapped(|ui| {
        let what = match v.me.role {
            Role::Backup => "this computer is its backup, following its show",
            Role::Display => "this computer is one of its display computers",
            _ => "this computer follows its show",
        };
        let r = ui.label(
            RichText::new(format!(
                "🔒 {who}{at} runs the show: {what}, and its controls are locked here."
            ))
            .strong(),
        );
        note(ui, "locked_banner", r.rect);
        note(ui, format!("locked_at:{at}"), egui::Rect::NOTHING);
        // "In step" says nothing true while the primary isn't heard.
        let heard = v.following.as_ref().is_none_or(|f| f.online) && quiet_words(v).is_none();
        if let Some(s) = v.status.standing.as_ref().filter(|_| heard) {
            ui.label(RichText::new(s).weak());
        }
    });
    // A test the primary runs (every test plays on every computer).
    if st.following_test {
        let r = ui.label(
            RichText::new(format!(
                "{who} is running a test: this computer plays along and measures itself. The results are on {who}."
            ))
            .color(Color32::LIGHT_BLUE),
        );
        note(ui, "following_test", r.rect);
    }
    // A display computer opened as NewQ rather than NewQ Display (only that
    // starts display mode): say so, and offer display mode now.
    if v.me.role == Role::Display {
        ui.horizontal_wrapped(|ui| {
            let r = ui.label(format!(
                "Opened as NewQ, so this window shows and this computer plays only the displays \
                 {who}'s show gives it. NewQ Display opens it as a display computer: a window on \
                 every screen, no controls."
            ));
            note(ui, "paired_display", r.rect);
            let r = ui.button("Open as a display computer").on_hover_text(
                "Every screen of this computer becomes an output and this window hides, as \
                     when NewQ Display opens it. Ctrl+Q (Command-Q on a Mac) quits.",
            );
            note(ui, "open_as_display", r.rect);
            if r.clicked() {
                st.open_as_display_request = true;
            }
        });
    }
    // Start at login, set here or from the primary (never by itself).
    ui.horizontal_wrapped(|ui| {
        let display = v.me.role == Role::Display;
        let mut at_login = st.start_at_login;
        let r = ui
            .checkbox(&mut at_login, at_login_label(display))
            .on_hover_text(crate::autostart::where_words(std::env::consts::OS, display));
        note(ui, "banner_start_at_login", r.rect);
        if r.changed() {
            st.start_at_login_request = Some(at_login);
        }
        if st.start_at_login {
            ui.label(
                RichText::new(crate::autostart::where_words(std::env::consts::OS, display))
                    .weak()
                    .small(),
            );
        }
    });
    // A second row: whether the primary is heard, and what can be done here.
    ui.horizontal(|ui| {
        // ---- Take control (stage 5): the primary gone quiet ----
        if let Some(text) = quiet_words(v) {
            let r = ui.label(RichText::new(text).color(STOP_RED).strong());
            note(ui, "quiet_primary", r.rect);
        } else if v.following.as_ref().is_some_and(|f| !f.online) {
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
            if v.me.role == Role::Backup {
                take_control_button(ui, &who, actions);
            }
        });
    });
}

/// "The primary (Booth) has not been heard for 12 s." once it has gone quiet.
pub fn quiet_words(v: &NetView) -> Option<String> {
    let f = v.following.as_ref()?;
    let secs = f.quiet_secs?;
    Some(format!(
        "The primary ({}) has not been heard for {secs} s.",
        f.name
    ))
}

/// The backup's "Take control", with its one-click confirmation (kept in the
/// window's memory, so the banner needs nothing of its own).
fn take_control_button(ui: &mut Ui, who: &str, actions: &mut Vec<Action>) {
    let id = egui::Id::new("net_confirm_take");
    let asking = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    if asking {
        let r = ui.button("Cancel");
        note(ui, "take_control_cancel", r.rect);
        if r.clicked() {
            ui.data_mut(|d| d.insert_temp(id, false));
        }
        let r = ui
            .add(
                egui::Button::new(
                    RichText::new("Yes, take control")
                        .strong()
                        .color(Color32::WHITE),
                )
                .fill(STOP_RED),
            )
            .on_hover_text(format!(
                "This computer runs the show from now on; {who} becomes the backup if it is \
                 there or comes back."
            ));
        note(ui, "take_control_yes", r.rect);
        if r.clicked() {
            ui.data_mut(|d| d.insert_temp(id, false));
            actions.push(Action::Net(NetAction::TakeControl));
        }
        let r = ui.label(RichText::new("Run the show from this computer?").strong());
        note(ui, "take_control_ask", r.rect);
    } else {
        let r = ui
            .add(
                egui::Button::new(RichText::new("Take control").strong().color(Color32::WHITE))
                    .fill(Color32::from_rgb(200, 120, 20)),
            )
            .on_hover_text(
                "Run the show from this computer: for when the primary has stopped. What plays \
                 carries on; the other computers follow this one.",
            );
        note(ui, "take_control", r.rect);
        if r.clicked() {
            ui.data_mut(|d| d.insert_temp(id, true));
        }
    }
}
