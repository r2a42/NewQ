//! The Show network page and the display and sound choices it adds, drawn in
//! the whole window (both interfaces) at 1280×760 and driven with the mouse.
//! The network itself is a `NetView` made up here: what the page asks for is
//! the `NetAction`s it leaves for the app.

use super::*;
use crate::net::hub::{Following, NetView, PeerView};
use crate::net::node::{Identity, NetworkId};
use crate::net::proto::{MonitorReport, StatusReport};
use egui::{Event, PointerButton, Pos2, RawInput, Rect, vec2};
use std::sync::Arc;

const SIZE: egui::Vec2 = vec2(1280.0, 760.0);
const ME: NodeId = NodeId(0x100);
const LEFT: NodeId = NodeId(0x200);
const NEW: NodeId = NodeId(0x300);

struct Win {
    ctx: egui::Context,
    e: Engine,
    t: f64,
}

impl Win {
    fn new() -> Win {
        Win {
            ctx: egui::Context::default(),
            e: Engine::for_tests(),
            t: 1.0,
        }
    }

    fn frame(&mut self, show: &mut Show, st: &mut UiState, events: Vec<Event>) {
        self.t += 1.0 / 60.0;
        self.ctx.data_mut(|d| {
            for name in ["net_rects", "remote_monitors", "plays_on"] {
                d.remove::<Vec<(String, Rect)>>(egui::Id::new(name));
            }
            d.remove::<Vec<(u64, Rect)>>(egui::Id::new("display_monitor_menus"));
        });
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)),
            time: Some(self.t),
            events,
            ..Default::default()
        };
        let e = &mut self.e;
        let mut out = self
            .ctx
            .run_ui(input, |ui| super::super::draw(ui, show, e, st));
        out.textures_delta.clear();
    }

    fn settle(&mut self, show: &mut Show, st: &mut UiState) {
        for _ in 0..3 {
            self.frame(show, st, vec![]);
        }
    }

    fn rects(&self, list: &str) -> Vec<(String, Rect)> {
        self.ctx
            .data(|d| d.get_temp::<Vec<(String, Rect)>>(egui::Id::new(list)))
            .unwrap_or_default()
    }

    fn find(&self, list: &str, name: &str) -> Option<Rect> {
        self.rects(list)
            .into_iter()
            .find(|(n, _)| n == name)
            .map(|(_, r)| r)
    }

    fn click(&mut self, show: &mut Show, st: &mut UiState, at: Rect) {
        let p = at.center();
        let b = |pressed| Event::PointerButton {
            pos: p,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        self.frame(show, st, vec![Event::PointerMoved(p)]);
        self.frame(show, st, vec![b(true)]);
        self.frame(show, st, vec![b(false)]);
        self.settle(show, st);
    }

    /// Click the control `name` of the page, scrolling the page down to it if
    /// it is out of sight.
    fn press(&mut self, show: &mut Show, st: &mut UiState, list: &str, name: &str) {
        for _ in 0..30 {
            match self.find(list, name) {
                Some(r) if r.is_positive() => break,
                Some(_) => {
                    let at = Pos2::new(SIZE.x * 0.6, SIZE.y * 0.5);
                    self.frame(show, st, vec![Event::PointerMoved(at)]);
                    self.frame(
                        show,
                        st,
                        vec![Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            delta: vec2(0.0, -80.0),
                            phase: egui::TouchPhase::Move,
                            modifiers: Default::default(),
                        }],
                    );
                    self.settle(show, st);
                }
                None => break,
            }
        }
        let r = self
            .find(list, name)
            .filter(|r| r.is_positive())
            .unwrap_or_else(|| panic!("{name} isn't in sight; drawn: {:?}", self.rects(list)));
        assert!(
            Rect::from_min_size(Pos2::ZERO, SIZE).contains_rect(r),
            "{name} is in the window: {r:?}"
        );
        self.click(show, st, r);
    }
}

/// The window with the settings open at `page`, in one interface.
fn window(modern: bool, page: SettingsPage, net: NetView) -> (Win, Show, UiState) {
    let mut st = UiState::default();
    st.prefs.new_layout = modern;
    st.layout.open(Pane::Settings);
    st.settings_page = page;
    st.net = Arc::new(net);
    let mut show = Show::default();
    let mut w = Win::new();
    w.settle(&mut show, &mut st);
    (w, show, st)
}

fn monitor(name: &str, w: u32, h: u32) -> MonitorReport {
    MonitorReport {
        identity: name.into(),
        raw: name.into(),
        hardware: None,
        size: (w, h),
        refresh_hz: Some(60.0),
        ..Default::default()
    }
}

/// This computer runs a network with "Left" in it (two monitors, two sound
/// devices) and "New" waiting to join.
fn primary_view() -> NetView {
    let network = Some(NetworkId(7));
    let me = Identity {
        id: ME,
        name: Some("Booth".into()),
        role: Role::Primary,
        on: true,
        network,
        primary: Some(ME),
        term: 1,
        ..Default::default()
    };
    let left = PeerView {
        id: LEFT,
        name: "Left".into(),
        role: Role::Display,
        version: proto::VERSION.into(),
        compatible: true,
        network,
        ours: true,
        waiting: false,
        online: true,
        connected: true,
        addr: Some("10.0.0.2".parse().unwrap()),
        status: Some(StatusReport {
            name: "Left".into(),
            role: Role::Display,
            version: proto::VERSION.into(),
            network,
            monitors: vec![
                monitor("DELL U2720", 2560, 1440),
                monitor("Projector", 1920, 1080),
            ],
            audio_devices: vec!["Speakers".into(), "USB interface".into()],
            disk_free: Some(120_000_000_000),
            link: Link::Wired,
            ..Default::default()
        }),
        problem: None,
        adopting: false,
        twin: None,
    };
    let new = PeerView {
        id: NEW,
        name: "New".into(),
        role: Role::Alone,
        network: None,
        ours: false,
        waiting: true,
        status: None,
        addr: None,
        ..left.clone()
    };
    NetView {
        me,
        name: "Booth".into(),
        port: Some(47820),
        peers: vec![left, new],
        ..Default::default()
    }
}

#[test]
fn the_page_draws_in_both_interfaces_and_the_switch_turns_it_on() {
    for modern in [false, true] {
        let (mut w, mut show, mut st) = window(modern, SettingsPage::Network, NetView::default());
        assert!(w.find("net_rects", "become_primary").is_some());
        w.press(&mut show, &mut st, "net_rects", "switch");
        assert_eq!(
            st.net_requests,
            vec![NetAction::SetOn(true)],
            "modern {modern}"
        );
        st.net_requests.clear();
        w.press(&mut show, &mut st, "net_rects", "become_primary");
        assert_eq!(st.net_requests, vec![NetAction::BecomePrimary]);
    }
}

#[test]
fn the_primary_lists_its_computers_and_gives_roles() {
    for modern in [false, true] {
        let (mut w, mut show, mut st) = window(modern, SettingsPage::Network, primary_view());
        assert!(
            w.find("net_rects", "become_primary").is_none(),
            "already the primary"
        );
        // A role from the menu.
        w.press(
            &mut show,
            &mut st,
            "net_rects",
            &format!("role_menu:{LEFT}"),
        );
        w.press(
            &mut show,
            &mut st,
            "net_rects",
            &format!("role:{LEFT}:Backup"),
        );
        assert_eq!(
            st.net_requests,
            vec![NetAction::SetRole(LEFT, Role::Backup)],
            "modern {modern}"
        );
        st.net_requests.clear();
        // Identify, release, and the waiting computer added.
        w.press(&mut show, &mut st, "net_rects", &format!("identify:{LEFT}"));
        w.press(&mut show, &mut st, "net_rects", &format!("release:{LEFT}"));
        w.press(&mut show, &mut st, "net_rects", &format!("adopt:{NEW}"));
        assert_eq!(
            st.net_requests,
            vec![
                NetAction::Identify(LEFT),
                NetAction::Release(LEFT),
                NetAction::Adopt(NEW)
            ]
        );
        st.net_requests.clear();
        // Renamed from here.
        w.press(&mut show, &mut st, "net_rects", &format!("rename:{LEFT}"));
        assert!(w.find("net_rects", &format!("name_field:{LEFT}")).is_some());
        w.press(&mut show, &mut st, "net_rects", &format!("name_ok:{LEFT}"));
        assert_eq!(
            st.net_requests,
            vec![NetAction::Rename(LEFT, "Left".into())]
        );
    }
}

#[test]
fn a_computer_in_the_network_shows_whom_it_follows_and_only_the_switch() {
    let mut v = primary_view();
    v.me.role = Role::Display;
    v.me.primary = Some(LEFT);
    v.peers.clear();
    v.following = Some(Following {
        id: LEFT,
        name: "Booth".into(),
        online: true,
        quiet_secs: None,
    });
    for modern in [false, true] {
        let (mut w, mut show, mut st) = window(modern, SettingsPage::Network, v.clone());
        assert!(w.find("net_rects", "switch").is_some());
        for absent in [
            "become_primary",
            "address",
            "passcode",
            "rename:0000000000000100",
        ] {
            assert!(w.find("net_rects", absent).is_none(), "{absent}");
        }
        // Its window is locked (stage 4a) but for leaving: the switch (where
        // the settings aren't greyed with the rest) or the banner's button.
        w.press(&mut show, &mut st, "net_rects", "switch");
        assert!(
            st.net_requests.is_empty() || st.net_requests == vec![NetAction::SetOn(false)],
            "{:?}",
            st.net_requests
        );
        st.net_requests.clear();
        w.press(&mut show, &mut st, "net_rects", "leave_network");
        assert_eq!(st.net_requests, vec![NetAction::SetOn(false)]);
    }
    assert_eq!(badge(&v).map(|b| b.text), Some("on".to_string()));
    v.following.as_mut().unwrap().online = false;
    assert_eq!(badge(&v).map(|b| b.color), Some(STOP_RED));
}

#[test]
fn the_badge_counts_the_computers_online() {
    assert!(badge(&NetView::default()).is_none(), "off: nothing");
    let mut v = primary_view();
    assert_eq!(badge(&v).map(|b| b.text), Some("1/1".to_string()));
    v.peers[0].online = false;
    let b = badge(&v).unwrap();
    assert_eq!((b.text.as_str(), b.color), ("0/1", STOP_RED));
    assert_eq!(space(120_000_000_000), "120 GB free");
    assert_eq!(space(1_500_000_000), "1.5 GB free");
}

#[test]
fn a_display_is_given_another_computers_monitor() {
    for modern in [false, true] {
        let (mut w, mut show, mut st) = window(modern, SettingsPage::Displays, primary_view());
        let id = show.outputs[0].id;
        let menu = w
            .ctx
            .data(|d| d.get_temp::<Vec<(u64, Rect)>>(egui::Id::new("display_monitor_menus")))
            .unwrap_or_default()
            .into_iter()
            .find(|(d, _)| *d == id.0)
            .expect("the monitor menu")
            .1;
        w.click(&mut show, &mut st, menu);
        w.press(&mut show, &mut st, "remote_monitors", &format!("{LEFT}:1"));
        let d = &show.outputs[0];
        assert_eq!(d.computer, Some(LEFT), "modern {modern}");
        let r = d.monitor.as_ref().unwrap();
        assert_eq!((r.name.as_str(), r.index), ("Projector", 1));
        let (text, color) = display_status(d, &st);
        assert_eq!(text, "On Left · connected · 1920×1080");
        assert_eq!(color, Some(GO_GREEN));
        // Its computer gone: said plainly, and no window is wanted here.
        let mut v = primary_view();
        v.peers[0].online = false;
        st.net = Arc::new(v);
        assert!(
            display_status(&show.outputs[0], &st)
                .0
                .contains("isn't connected now")
        );
        assert!(!show.outputs[0].is_on(ME));
    }
}

#[test]
fn a_sound_output_is_ticked_for_another_computer() {
    for modern in [false, true] {
        let mut v = primary_view();
        v.peers.truncate(1);
        let mut st = UiState::default();
        st.prefs.new_layout = modern;
        st.layout.open(Pane::Settings);
        st.settings_page = SettingsPage::Audio;
        st.net = Arc::new(v);
        let mut show = Show::default();
        let o = show.add_audio_output();
        let mut w = Win::new();
        w.settle(&mut show, &mut st);
        w.press(&mut show, &mut st, "plays_on", &format!("{}:{LEFT}", o.0));
        assert_eq!(show.audio_outputs[0].computers, vec![ME, LEFT]);
        // Taken off this computer: only Left plays it.
        w.press(&mut show, &mut st, "plays_on", &format!("{}:{ME}", o.0));
        assert_eq!(show.audio_outputs[0].computers, vec![LEFT]);
        assert_eq!(show.audio_outputs[0].device_here(ME, true), None);
    }
}

// ---- The media manager's rows (`net_media.rs`) ----

use crate::net::media::manager::{CueLack, FolderView, Lack, MediaReport, MediaView, Side};
use crate::net::proto::Msg;
use crate::net::store::pull::{Progress, Tier};

/// A computer's report: 40 % of 1 GB here, at 20 MB/s.
fn busy_report() -> MediaReport {
    MediaReport {
        progress: Progress {
            manifest_id: Some("m".into()),
            used: Tier {
                files_wanted: 3,
                files_here: 1,
                bytes_wanted: 1_000_000_000,
                bytes_here: 300_000_000,
                bytes_partial: 100_000_000,
                ..Tier::default()
            },
            rate: Some(20_000_000.0),
            eta_secs: Some(30.0),
            computers: 1,
            ..Progress::default()
        },
        plan: "2 files to copy (700 MB).".into(),
        lacking: vec![Lack {
            path: "/media/clip.mp4".into(),
            done: 430,
            total: 1000,
        }],
        serve_port: Some(4000),
        store_free: Some(80_000_000_000),
        ..MediaReport::default()
    }
}

fn primary_media_view() -> (NetView, MediaView) {
    let mut v = primary_view();
    v.peers[0].status.as_mut().unwrap().media_progress = Some(Box::new(busy_report()));
    let sum = Progress::sum(&[busy_report().progress]);
    let mut lacking = std::collections::HashMap::new();
    lacking.insert(
        crate::net::store::paths::path_key("/media/clip.mp4"),
        vec![CueLack {
            node: LEFT,
            name: "Left".into(),
            percent: 43.0,
        }],
    );
    let m = MediaView {
        side: Side::Primary,
        sum: Some(sum),
        busy: 1,
        served: Some((3, 1_000_000_000)),
        lacking,
        ..MediaView::default()
    };
    (v, m)
}

#[test]
fn the_primary_shows_each_computers_media_and_sets_its_versions_and_checks() {
    for modern in [false, true] {
        let (v, m) = primary_media_view();
        let (mut w, mut show, mut st) = window(modern, SettingsPage::Network, v);
        st.media = Arc::new(m);
        w.settle(&mut show, &mut st);
        assert!(
            w.find("net_rects", "media_all").is_some(),
            "modern {modern}"
        );
        assert!(w.find("net_rects", &format!("media:{LEFT}")).is_some());
        assert!(
            w.find("net_rects", "media_notice").is_some(),
            "the status bar's notice, modern {modern}"
        );
        w.press(&mut show, &mut st, "net_rects", &format!("verify:{LEFT}"));
        assert_eq!(st.media_requests, vec![(LEFT, Msg::MediaVerify)]);
        st.media_requests.clear();
        w.press(
            &mut show,
            &mut st,
            "net_rects",
            &format!("policy_menu:{LEFT}"),
        );
        w.press(&mut show, &mut st, "net_rects", &format!("policy:{LEFT}:2"));
        assert_eq!(
            st.media_requests,
            vec![(
                LEFT,
                Msg::MediaPolicy {
                    keep: 5,
                    replace_all: true,
                    replace_over: None
                }
            )]
        );
    }
}

#[test]
fn a_follower_shows_its_own_media() {
    let mut v = primary_view();
    v.me.role = Role::Display;
    v.peers.clear();
    v.following = Some(Following {
        id: LEFT,
        name: "Booth".into(),
        online: true,
        quiet_secs: None,
    });
    for modern in [false, true] {
        let (mut w, mut show, mut st) = window(modern, SettingsPage::Network, v.clone());
        st.media = Arc::new(MediaView {
            side: Side::Follower,
            own: Some(busy_report()),
            ..MediaView::default()
        });
        w.settle(&mut show, &mut st);
        assert!(
            w.find("net_rects", "media_own").is_some(),
            "modern {modern}"
        );
        assert!(w.find("net_rects", "media_notice").is_some());
    }
}

#[test]
fn the_media_page_keeps_library_folders_and_the_card_setting() {
    for modern in [false, true] {
        let (mut w, mut show, mut st) = window(modern, SettingsPage::Media, NetView::default());
        show.library_folders = vec!["/Content/Looks".into(), "/Content/Spare".into()];
        st.media = Arc::new(MediaView {
            side: Side::Primary,
            folders: vec![FolderView {
                path: "/Content/Looks".into(),
                files: 12,
                bytes: 3_000_000_000,
                listed: true,
            }],
            ..MediaView::default()
        });
        w.settle(&mut show, &mut st);
        assert!(
            w.find("net_rects", "library_add").is_some(),
            "modern {modern}"
        );
        assert!(w.find("net_rects", "library_folder:1").is_some());
        w.press(&mut show, &mut st, "net_rects", "library_remove:1");
        assert_eq!(show.library_folders, [PathBuf::from("/Content/Looks")]);
    }
}

#[test]
fn the_cards_on_display_computers_are_set_side_by_side_on_the_network_page() {
    for modern in [false, true] {
        let (mut w, mut show, mut st) = window(modern, SettingsPage::Network, primary_view());
        w.press(&mut show, &mut st, "net_rects", "transfer_cards");
        assert!(show.no_transfer_cards, "modern {modern}");
        w.press(&mut show, &mut st, "net_rects", "wait_cards");
        w.press(&mut show, &mut st, "net_rects", "wait_cards:Never (black)");
        assert_eq!(show.wait_cards, crate::show::WaitCards::Never);
        // A follower's come from the primary: not offered there.
        st.net = Arc::new(follower_view(Role::Display));
        w.settle(&mut show, &mut st);
        assert!(w.find("net_rects", "wait_cards").is_none());
        // Alone: offered, for when the show runs in a network.
        st.net = Arc::new(NetView::default());
        w.settle(&mut show, &mut st);
        assert!(w.find("net_rects", "transfer_cards").is_some());
    }
}

#[test]
fn every_computer_shows_its_address_and_way_and_a_vpn_is_said() {
    use crate::net::paths::Adapter;
    let mut v = primary_view();
    v.adapters = vec![
        Adapter::new(
            "Ethernet",
            "10.0.0.1".parse().unwrap(),
            "255.255.255.0".parse().unwrap(),
        ),
        Adapter::new(
            "utun3",
            "10.8.0.2".parse().unwrap(),
            "255.255.255.0".parse().unwrap(),
        ),
    ];
    // Left's session is on the LAN, its port not the usual one.
    v.reached = vec![(LEFT, "10.0.0.2:47821".parse().unwrap())];
    // An offline computer remembered at its last address.
    let gone = NodeId(0x400);
    v.peers.push(PeerView {
        id: gone,
        name: "Gone".into(),
        online: false,
        connected: false,
        addr: None,
        status: None,
        ..v.peers[0].clone()
    });
    v.me.known = vec![crate::net::node::Known {
        id: gone,
        name: "Gone".into(),
        role: Role::Display,
        addr: Some("10.0.0.9".into()),
        port: crate::net::discover::DEFAULT_PORT,
    }];
    assert_eq!(
        peer_address(&v, LEFT),
        Some(("10.0.0.2, port 47821".to_string(), false))
    );
    assert_eq!(peer_address(&v, gone), Some(("10.0.0.9".to_string(), true)));
    assert_eq!(
        path_to(&v, LEFT),
        Some(crate::net::paths::AdapterKind::Wired)
    );
    assert_eq!(
        own_addresses(&v.adapters, Some(47820)),
        vec!["Wired · 10.0.0.1", "VPN · 10.8.0.2"]
    );
    let vpn = vpn_words(&v).unwrap();
    assert!(
        vpn.contains(
            "A VPN is on: it can slow the show network. NewQ is using the wired network (10.0.0.1)."
        ),
        "{vpn}"
    );
    for modern in [false, true] {
        let (w, _, _) = window(modern, SettingsPage::Network, v.clone());
        let names: Vec<String> = w.rects("net_rects").into_iter().map(|(n, _)| n).collect();
        for want in [
            format!("address:{LEFT}:Wired · 10.0.0.2, port 47821"),
            format!("address:{gone}:last at 10.0.0.9"),
            "address:me:Wired · 10.0.0.1".to_string(),
            "own_address:This computer's addresses: Wired · 10.0.0.1 · VPN · 10.8.0.2".to_string(),
        ] {
            let r = w
                .find("net_rects", &want)
                .unwrap_or_else(|| panic!("{want}: {names:?}"));
            assert!(
                Rect::from_min_size(Pos2::ZERO, SIZE).contains_rect(r) || r == Rect::NOTHING,
                "{want} is in the window"
            );
        }
        assert!(
            names.iter().any(|n| n.starts_with("vpn:A VPN is on")),
            "{names:?}"
        );
    }
    // A computer connected through the VPN is said.
    v.reached = vec![(LEFT, "10.8.0.5:47820".parse().unwrap())];
    assert!(
        vpn_words(&v)
            .unwrap()
            .contains("Left is connected through a VPN")
    );
    // A follower names its primary with the address.
    let mut f = follower_view(Role::Display);
    f.reached = vec![(LEFT, "10.0.0.1:47820".parse().unwrap())];
    for modern in [false, true] {
        let (w, _, _) = window(modern, SettingsPage::Network, f.clone());
        assert!(w.find("net_rects", "said").is_some());
        assert!(
            w.find("net_rects", "primary_at: (10.0.0.1)").is_some(),
            "{:?}",
            w.rects("net_rects")
        );
    }
    // A follower's window is locked: its banner names the primary's address,
    // and says when the primary runs a test that it plays along with.
    for modern in [false, true] {
        let (mut w, mut show, mut st) = locked_window(modern, f.clone());
        assert!(w.find("net_rects", "locked_at: (10.0.0.1)").is_some());
        assert!(w.find("net_rects", "following_test").is_none());
        st.following_test = true;
        w.settle(&mut show, &mut st);
        assert!(w.find("net_rects", "following_test").is_some());
    }
}

#[test]
fn a_cue_whose_file_a_computer_lacks_is_marked_more_strongly_where_it_shows() {
    let (v, m) = primary_media_view();
    for modern in [false, true] {
        let mut st = UiState::default();
        st.prefs.new_layout = modern;
        st.net = Arc::new(v.clone());
        st.media = Arc::new(m.clone());
        let mut show = Show::default();
        let c = show.make_cue(CueType::Video, Some(PathBuf::from("/media/clip.mp4")), 0);
        show.cues.push(c.clone());
        let (strong, tip) = net_media::cue_mark(&show, &st, &c).unwrap();
        assert!(!strong, "its display is on this computer");
        assert!(tip.contains("Left: 43 %"), "{tip}");
        show.outputs[0].computer = Some(LEFT);
        let (strong, tip) = net_media::cue_mark(&show, &st, &c).unwrap();
        assert!(strong);
        assert!(tip.contains("shows this cue's display"));
        // Drawn in the list (both interfaces) without trouble.
        let mut w = Win::new();
        w.settle(&mut show, &mut st);
        // Alone: no mark.
        st.media = Arc::new(MediaView::default());
        assert!(net_media::cue_mark(&show, &st, &c).is_none());
    }
}

// ---- Playing together (stage 4a) ----

/// A computer following "Booth".
fn follower_view(role: Role) -> NetView {
    let mut v = primary_view();
    v.me.role = role;
    v.me.primary = Some(LEFT);
    v.peers.clear();
    v.following = Some(Following {
        id: LEFT,
        name: "Booth".into(),
        online: true,
        quiet_secs: None,
    });
    v.status.standing = Some("Getting ready".into());
    v
}

#[test]
fn a_follower_is_locked_with_a_line_saying_who_runs_the_show() {
    for role in [Role::Display, Role::Backup] {
        for modern in [false, true] {
            let mut st = UiState::default();
            st.prefs.new_layout = modern;
            st.net = Arc::new(follower_view(role));
            let mut show = Show::default();
            let c = show.make_cue(crate::show::CueType::Wait, None, 0);
            show.cues.push(c);
            let mut w = Win::new();
            w.e.playhead = show.cues.first().map(|c| c.id);
            w.settle(&mut show, &mut st);
            let r = w
                .find("net_rects", "locked_banner")
                .expect("the banner is drawn");
            assert!(Rect::from_min_size(Pos2::ZERO, SIZE).contains_rect(r));
            // GO's key does nothing here: the show is run from the primary.
            for key in [egui::Key::Space, egui::Key::Num0] {
                let ev = |pressed| Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: Default::default(),
                };
                w.frame(&mut show, &mut st, vec![ev(true), ev(false)]);
            }
            w.settle(&mut show, &mut st);
            assert!(w.e.active.is_empty() && w.e.pending.is_empty());
            // An edit that got through (a field written directly) is put back.
            let name = show.cues[0].name.clone();
            show.cues[0].name = "Edited here".into();
            for _ in 0..30 {
                w.frame(&mut show, &mut st, vec![]);
            }
            assert_eq!(show.cues[0].name, name);
            assert!(st.net_requests.is_empty());
        }
    }
}

#[test]
fn the_primary_page_says_each_computer_is_in_step_and_sets_it_up() {
    for modern in [false, true] {
        let mut v = primary_view();
        if let Some(s) = v.peers[0].status.as_mut() {
            s.clock_error_ms = Some(0.05);
            s.clock_locked = true;
            s.in_step = true;
        }
        let (mut w, mut show, mut st) = window(modern, SettingsPage::Network, v);
        assert!(w.find("net_rects", &format!("step:{LEFT}")).is_some());
        w.press(&mut show, &mut st, "net_rects", &format!("at_login:{LEFT}"));
        assert_eq!(st.net_requests, vec![NetAction::StartAtLogin(LEFT, true)]);
        st.net_requests.clear();
        w.press(&mut show, &mut st, "net_rects", &format!("restart:{LEFT}"));
        assert_eq!(st.net_requests, vec![NetAction::Restart(LEFT)]);
        st.net_requests.clear();
        // This computer's own tick.
        w.press(&mut show, &mut st, "net_rects", "start_at_login");
        assert_eq!(st.start_at_login_request, Some(true));
    }
    let (text, color) = super::step_words(&StatusReport {
        standing: Some("Getting the show".into()),
        ..Default::default()
    });
    assert_eq!(text, "Getting the show");
    assert_ne!(color, GO_GREEN);
}

#[test]
fn a_blended_picture_on_wifi_or_a_loose_clock_is_warned_of() {
    let v = primary_view();
    let left = &v.peers[0];
    let mut show = Show::default();
    show.outputs.clear();
    let mut d = crate::show::OutputConfig::new(crate::show::OutputId(1), "D".into());
    d.computer = Some(LEFT);
    show.outputs.push(d);
    let mut s = left.status.clone().unwrap();
    s.clock_error_ms = Some(20.0);
    // No blend: nothing to say.
    assert_eq!(super::blend_warning(&show, left, &s), None);
    show.outputs[0].blend.right = 0.1;
    assert!(super::blend_warning(&show, left, &s).is_some_and(|w| w.contains("20 ms")));
    s.clock_error_ms = Some(0.1);
    assert_eq!(
        super::blend_warning(&show, left, &s),
        None,
        "wired and close"
    );
    s.link = Link::WiFi;
    assert!(super::blend_warning(&show, left, &s).is_some_and(|w| w.contains("Wi-Fi")));
    // A part of a picture another computer shows counts too.
    show.outputs[0].blend = Default::default();
    s.link = Link::Wired;
    s.clock_error_ms = Some(12.0);
    let mut other = crate::show::OutputConfig::new(crate::show::OutputId(2), "E".into());
    other.part_of = Some(crate::show::PartOf {
        display: crate::show::OutputId(1),
        area: [0.5, 0.0, 0.5, 1.0],
    });
    show.outputs.push(other);
    assert!(super::blend_warning(&show, left, &s).is_some());
}

// ---- Take control (stage 5) ----

/// The locked window of a computer following "Booth", drawn.
fn locked_window(modern: bool, v: NetView) -> (Win, Show, UiState) {
    let mut st = UiState::default();
    st.prefs.new_layout = modern;
    st.net = Arc::new(v);
    let mut show = Show::default();
    let c = show.make_cue(crate::show::CueType::Wait, None, 0);
    show.cues.push(c);
    let mut w = Win::new();
    w.settle(&mut show, &mut st);
    (w, show, st)
}

#[test]
fn the_backup_takes_control_with_one_confirming_click() {
    for modern in [false, true] {
        let (mut w, mut show, mut st) = locked_window(modern, follower_view(Role::Backup));
        assert!(w.find("net_rects", "locked_banner").is_some());
        // The first click only asks.
        w.press(&mut show, &mut st, "net_rects", "take_control");
        assert!(st.net_requests.is_empty(), "modern {modern}");
        assert!(w.find("net_rects", "take_control").is_none());
        // The question and the banner's own words don't overlap.
        let ask = w
            .find("net_rects", "take_control_ask")
            .expect("the question");
        let banner = w.find("net_rects", "locked_banner").unwrap();
        assert!(!ask.intersects(banner), "{ask:?} {banner:?}");
        assert!(Rect::from_min_size(Pos2::ZERO, SIZE).contains_rect(ask));
        // Cancel goes back; asked again and confirmed, it is asked of the network.
        w.press(&mut show, &mut st, "net_rects", "take_control_cancel");
        assert!(st.net_requests.is_empty());
        w.press(&mut show, &mut st, "net_rects", "take_control");
        w.press(&mut show, &mut st, "net_rects", "take_control_yes");
        assert_eq!(
            st.net_requests,
            vec![NetAction::TakeControl],
            "modern {modern}"
        );
        assert!(
            w.find("net_rects", "take_control").is_some(),
            "back to the button"
        );
    }
    // A display computer's window has no such button (it is hidden anyway).
    let (w, _, _) = locked_window(false, follower_view(Role::Display));
    assert!(w.find("net_rects", "take_control").is_none());
}

/// A display computer opened as NewQ (not NewQ Display) shows its window,
/// locked, says what it is and offers display mode or leaving; its start at
/// login can be set there though the rest of the window is locked.
#[test]
fn a_display_computer_opened_as_newq_offers_display_mode_or_leaving() {
    for modern in [false, true] {
        let (mut w, mut show, mut st) = locked_window(modern, follower_view(Role::Display));
        let screen = Rect::from_min_size(Pos2::ZERO, SIZE);
        let said = w
            .find("net_rects", "paired_display")
            .expect("the display computer's line");
        assert!(screen.contains_rect(said), "{said:?}");
        w.press(&mut show, &mut st, "net_rects", "open_as_display");
        assert!(st.open_as_display_request, "modern {modern}");
        assert!(st.net_requests.is_empty());
        w.press(&mut show, &mut st, "net_rects", "banner_start_at_login");
        assert_eq!(st.start_at_login_request, Some(true));
        w.press(&mut show, &mut st, "net_rects", "leave_network");
        assert_eq!(st.net_requests, vec![NetAction::SetOn(false)]);
        // None of it overlaps the banner's first line.
        let banner = w.find("net_rects", "locked_banner").unwrap();
        let open = w.find("net_rects", "open_as_display").unwrap();
        assert!(!open.intersects(banner) && screen.contains_rect(open));
    }
    // A backup has no such line, but its own start at login.
    let (mut w, mut show, mut st) = locked_window(false, follower_view(Role::Backup));
    assert!(w.find("net_rects", "paired_display").is_none());
    w.press(&mut show, &mut st, "net_rects", "banner_start_at_login");
    assert_eq!(st.start_at_login_request, Some(true));
    assert_eq!(
        super::at_login_label(true),
        "Start as a display computer when this computer starts"
    );
}

#[test]
fn the_backup_says_when_the_primary_has_gone_quiet() {
    let mut v = follower_view(Role::Backup);
    assert_eq!(quiet_words(&v), None, "heard: nothing to say");
    let f = v.following.as_mut().unwrap();
    f.online = false;
    f.quiet_secs = Some(12);
    assert_eq!(
        quiet_words(&v).as_deref(),
        Some("The primary (Booth) has not been heard for 12 s.")
    );
    for modern in [false, true] {
        let (w, _, _) = locked_window(modern, v.clone());
        let r = w
            .find("net_rects", "quiet_primary")
            .expect("the warning is drawn");
        assert!(
            Rect::from_min_size(Pos2::ZERO, SIZE).contains_rect(r),
            "modern {modern}: {r:?}"
        );
        assert!(w.find("net_rects", "take_control").is_some());
    }
}

#[test]
fn a_computer_gone_for_good_is_forgotten_and_a_takeover_is_said() {
    let mut v = primary_view();
    v.peers[0].online = false;
    v.peers[0].connected = false;
    for modern in [false, true] {
        let (mut w, mut show, mut st) = window(modern, SettingsPage::Network, v.clone());
        assert!(
            w.find("net_rects", "forget:0000000000000100").is_none(),
            "not this computer"
        );
        w.press(&mut show, &mut st, "net_rects", &format!("forget:{LEFT}"));
        assert_eq!(
            st.net_requests,
            vec![NetAction::Forget(LEFT)],
            "modern {modern}"
        );
        // Online: no Forget (Release is the way).
        st.net = Arc::new(primary_view());
        w.settle(&mut show, &mut st);
        assert!(w.find("net_rects", &format!("forget:{LEFT}")).is_none());
        // After taking control: unsaved here, Save asks where, and the page says so.
        st.show_path = Some(PathBuf::from("/shows/old.newq"));
        st.net = Arc::new(follower_view(Role::Backup));
        st.took_control();
        assert_eq!(st.show_path, None);
        assert!(st.dirty);
        st.net = Arc::new(primary_view());
        w.settle(&mut show, &mut st);
        assert!(w.find("net_rects", "took_control_note").is_some());
        assert_eq!(
            st.net_edit.took_control_from.as_deref(),
            Some("Booth"),
            "modern {modern}"
        );
    }
}

#[test]
fn a_primary_that_took_control_says_what_media_it_lacks() {
    for modern in [false, true] {
        let (v, mut m) = primary_media_view();
        m.from_store = true;
        m.lacking_here = 2;
        let (mut w, mut show, mut st) = window(modern, SettingsPage::Network, v);
        st.media = Arc::new(m);
        w.settle(&mut show, &mut st);
        assert!(
            w.find("net_rects", "media_from_store").is_some(),
            "modern {modern}"
        );
        assert!(w.find("net_rects", "media_lacking_here").is_some());
    }
}

/// A display computer whose built-in screen draws 4 frames a second says so
/// on the primary's page; one drawing at its rate says nothing.
#[test]
fn a_monitor_drawing_slowly_is_said_on_the_page() {
    let mut s = proto::StatusReport {
        monitors: vec![
            monitor("B173HAN04.9", 1920, 1080),
            monitor("DISPLAY2", 1920, 1080),
        ],
        ..Default::default()
    };
    assert!(super::slow_drawing("Assistant", &s).is_empty());
    s.monitors[0].refresh_hz = Some(64.0);
    s.monitors[0].drawing_fps = Some(4.0);
    s.monitors[1].drawing_fps = Some(60.0);
    assert_eq!(
        super::slow_drawing("Assistant", &s),
        vec!["⚠ Assistant's monitor 1 (B173HAN04.9) is drawing 4 frames a second: it should draw 64. Its pictures stutter".to_string()]
    );
}

#[test]
fn a_computer_drawing_in_software_is_said_on_the_page() {
    let s = proto::StatusReport {
        software_drawing: Some(crate::adapters::SoftwareDrawing {
            adapter: "Microsoft Basic Render Driver".into(),
            lost: true,
        }),
        ..Default::default()
    };
    let said = super::slow_drawing("WNO-2", &s);
    assert_eq!(said.len(), 1);
    assert!(
        said[0].starts_with(
            "⚠ WNO-2 lost its graphics card and is drawing in software (Microsoft Basic Render Driver): pictures will be very slow"
        ),
        "{said:?}"
    );
}

/// Two NewQs with one identity (second test): the badge turns red and the page
/// draws the warning in both interfaces; a follower whose clock isn't settled
/// says "clock not locked" rather than a figure.
#[test]
fn two_newqs_with_one_identity_and_an_unlocked_clock_are_said() {
    let mut v = primary_view();
    v.peers[0].twin = Some("Two copies of NewQ answer as Left".into());
    assert_eq!(badge(&v).map(|b| b.text), Some("⚠".to_string()));
    for modern in [false, true] {
        let (mut w, mut show, mut st) = window(modern, SettingsPage::Network, v.clone());
        w.settle(&mut show, &mut st);
    }
    let mut own = primary_view();
    own.twin = Some("Another copy of NewQ with this computer's identity is open".into());
    assert_eq!(badge(&own).map(|b| b.text), Some("⚠".to_string()));
    let mut s = primary_view().peers[0].status.clone().unwrap();
    s.primary = Some(ME);
    s.clock_locked = false;
    s.clock_error_ms = None;
    let p = primary_view().peers[0].clone();
    assert!(facts(&s, Some(&p)).contains("clock not locked"));
    s.clock_locked = true;
    s.clock_error_ms = Some(0.05);
    let said = facts(&s, Some(&p));
    assert!(said.contains("clock ±0.05 ms") && !said.contains("not locked"));
}

/// The one-time start-at-login clean-up removed an older NewQ's entry: the
/// computer says so beside its tick for a while, until it is ticked again.
#[test]
fn a_computer_whose_start_at_login_was_cleaned_up_says_so() {
    let now = std::time::Instant::now();
    let mut st = UiState::default();
    assert_eq!(super::login_note(&st, now), None, "nothing removed");
    st.login_note_since = Some(now);
    assert_eq!(
        super::login_note(&st, now),
        Some(crate::autostart::SWITCHED_OFF)
    );
    assert!(super::login_note(&st, now + super::LOGIN_NOTE_FOR).is_none());
    st.start_at_login = true;
    assert!(super::login_note(&st, now).is_none(), "ticked again");
    for modern in [false, true] {
        let (mut w, mut show, mut st) = locked_window(modern, follower_view(Role::Display));
        st.login_note_since = Some(std::time::Instant::now());
        w.settle(&mut show, &mut st);
        assert!(
            w.find("net_rects", "banner_login_note").is_some(),
            "modern {modern}"
        );
        st.start_at_login = true;
        w.settle(&mut show, &mut st);
        assert!(w.find("net_rects", "banner_login_note").is_none());
    }
}
