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
        // Switching off is leaving.
        w.press(&mut show, &mut st, "net_rects", "switch");
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
        w.press(&mut show, &mut st, "net_rects", "transfer_cards");
        assert!(show.no_transfer_cards);
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
