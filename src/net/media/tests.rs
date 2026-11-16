//! The media manager: the order, the words, and the whole of it over
//! loopback (a primary and two followers with real hubs, a capped rate so
//! the order can be seen). Every wait is on a condition with a generous
//! limit: nothing assumes a fast machine.

use super::manager::{Config, Frame, Manager, Outgoing, Side};
use super::words::{self, Card};
use super::*;
use crate::net::hub::{Hub, HubConfig, NetAction, NetEvent, NetView};
use crate::net::node::{self, NodeId};
use crate::net::store::local::Policy;
use crate::net::store::pull::{Progress, PullOptions, Tier};
use crate::net::store::{Timeouts, paths};
use crate::show::{CueId, CueKind, CueType, GroupMode, Show};
use crate::timeline::TimelineKey;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

// ------------------------------------------------------------------ order

fn file_cue(show: &mut Show, list: u32, name: &str) -> CueId {
    let at = show.list_end(list);
    let mut c = show.make_cue(
        CueType::Video,
        Some(PathBuf::from(format!("/media/{name}"))),
        at,
    );
    c.list = list;
    let id = c.id;
    show.cues.insert(at, c);
    id
}

fn names(order: &[String]) -> Vec<String> {
    order.iter().map(|p| words::file_name(p)).collect()
}

fn no_len(_: &crate::show::Cue) -> Option<f64> {
    None
}

#[test]
fn the_order_is_from_the_playhead_forward_then_behind_nearest_first() {
    let mut show = Show::default();
    show.cues.clear();
    let a = file_cue(&mut show, 0, "a.mp4");
    let _b = file_cue(&mut show, 0, "b.mp4");
    let c = file_cue(&mut show, 0, "c.mp4");
    let _d = file_cue(&mut show, 0, "d.mp4");
    // The same file again later: it ranks by its nearest use.
    let _a2 = file_cue(&mut show, 0, "a.mp4");
    let other = show.add_list("Second");
    let _f = file_cue(&mut show, other, "f.mp4");
    let g = file_cue(&mut show, other, "g.mp4");
    let mut at = Positions {
        active_list: 0,
        playhead: Some(c),
        others: vec![(other, Some(g))],
        timelines: Vec::new(),
    };
    let o = order(&show, &at, &mut no_len);
    assert_eq!(
        names(&o),
        ["c.mp4", "d.mp4", "a.mp4", "g.mp4", "b.mp4", "f.mp4"]
    );
    // Every used file, once each, as the show names it.
    assert_eq!(o.len(), 6);
    assert!(o.contains(&"/media/c.mp4".to_string()));
    // Jumping about: back to the start, then past the end.
    at.playhead = Some(a);
    assert_eq!(
        names(&order(&show, &at, &mut no_len)),
        ["a.mp4", "b.mp4", "c.mp4", "d.mp4", "g.mp4", "f.mp4"]
    );
    at.playhead = None;
    assert_eq!(
        names(&order(&show, &at, &mut no_len))[..4],
        ["g.mp4", "a.mp4", "d.mp4", "c.mp4"]
    );
    // The other list's playhead moves its files.
    at.playhead = Some(c);
    at.others = vec![];
    let o = names(&order(&show, &at, &mut no_len));
    assert_eq!(o[..3], ["c.mp4", "d.mp4", "a.mp4"]);
    assert_eq!(
        o[3..5],
        ["f.mp4", "g.mp4"],
        "from the first cue of a list never visited"
    );
}

#[test]
fn a_running_timeline_comes_after_the_active_list_from_its_clock() {
    let mut show = Show::default();
    show.cues.clear();
    let x = file_cue(&mut show, 0, "x.mp4");
    let other = show.add_list("Timelines");
    let at_end = show.list_end(other);
    let mut group = show.make_cue(CueType::Group, None, at_end);
    group.list = other;
    if let CueKind::Group(g) = &mut group.kind {
        g.mode = GroupMode::Timeline;
    }
    let gid = group.id;
    show.cues.insert(at_end, group);
    let mut kids = Vec::new();
    for (i, start) in [0.0, 5.0, 10.0].into_iter().enumerate() {
        let at_end = show.list_end(other);
        let mut c = show.make_cue(
            CueType::Video,
            Some(PathBuf::from(format!("/media/t{i}.mp4"))),
            at_end,
        );
        c.list = other;
        c.parent = Some(gid);
        c.pre_wait = start;
        kids.push(c.id);
        show.cues.insert(at_end, c);
    }
    assert_eq!(show.children(gid).len(), 3);
    let mut four = |_: &crate::show::Cue| Some(4.0);
    let at = Positions {
        active_list: 0,
        playhead: Some(x),
        others: vec![(other, Some(gid))],
        timelines: vec![(TimelineKey::Group(gid), 6.0)],
    };
    let o = names(&order(&show, &at, &mut four));
    assert_eq!(o, ["x.mp4", "t1.mp4", "t2.mp4", "t0.mp4"]);
    // Its signature moves when a clip begins or ends, not every tick.
    let s1 = order::signature(&show, &at, &mut four);
    let mut later = at.clone();
    later.timelines[0].1 = 6.5;
    assert_eq!(order::signature(&show, &later, &mut four), s1);
    later.timelines[0].1 = 9.5;
    assert_ne!(
        order::signature(&show, &later, &mut four),
        s1,
        "the clip at 5 is over"
    );
}

// ------------------------------------------------------------------ words

#[test]
fn the_card_says_what_it_waits_for() {
    let t = Local::Transferring(Transfer {
        done: 43,
        total: 100,
        eta_secs: Some(130.0),
        known: true,
    });
    assert_eq!(
        words::card(r"C:\Shows\Big Night\Clip.MOV", &t),
        Some(Card {
            heading: "Media transferring".into(),
            name: "Clip.MOV".into(),
            detail: "43 % · about 2 min".into(),
        })
    );
    let c = words::card("/m/a.mp4", &Local::NotFound).unwrap();
    assert_eq!(c.heading, "Media not found");
    assert!(c.detail.contains("/m/a.mp4"));
    assert_eq!(words::card("/m/a.mp4", &Local::Here("/x".into())), None);
    let waiting = words::card("/m/a.mp4", &Local::Transferring(Transfer::WAITING)).unwrap();
    assert_eq!(waiting.detail, "Waiting for the primary's list of media");
    let queued = Local::Transferring(Transfer {
        done: 0,
        total: 2_500_000_000,
        eta_secs: None,
        known: true,
    });
    assert_eq!(
        words::card("/m/a.mp4", &queued).unwrap().detail,
        "Waiting its turn · 2.5 GB"
    );
    // Never "100 %" for a file that isn't whole.
    let nearly = Transfer {
        done: 999,
        total: 1000,
        eta_secs: Some(0.5),
        known: true,
    };
    assert_eq!(words::transfer_line(&nearly), "99 % · nearly there");
}

#[test]
fn times_policies_and_progress_read_plainly() {
    assert_eq!(words::time_left(30.0), "less than a minute");
    assert_eq!(words::time_left(14.0 * 60.0), "about 14 min");
    assert_eq!(words::time_left(80.0 * 60.0), "about 1 h 20 min");
    assert_eq!(words::time_left(f64::NAN), "working out the time left");
    assert_eq!(words::policy(&Policy::default()), "Keeps 5 versions");
    let replace = Policy {
        replace_all: true,
        ..Policy::default()
    };
    assert_eq!(
        words::policy(&replace),
        "Replaces (keeps no older versions)"
    );
    let big = Policy {
        replace_over: Some(4_000_000_000),
        ..Policy::default()
    };
    assert_eq!(
        words::policy(&big),
        "Keeps 5 versions · replaces files over 4.0 GB"
    );
    let one = |here: u64, partial: u64, wanted: u64, eta: Option<f64>| Progress {
        manifest_id: Some("m".into()),
        used: Tier {
            files_wanted: 2,
            files_here: usize::from(here == wanted),
            files_not_found: 0,
            bytes_wanted: wanted,
            bytes_here: here,
            bytes_partial: partial,
        },
        library: Tier::default(),
        rate: Some(45_000_000.0),
        eta_secs: eta,
        computers: 1,
    };
    let a = one(500_000_000, 120_000_000, 1_000_000_000, Some(14.0 * 60.0));
    assert_eq!(
        words::progress(&a),
        "62 % · 380 MB left · 45 MB/s · about 14 min"
    );
    let b = one(0, 0, 1_000_000_000, None);
    let sum = Progress::sum(&[a.clone(), b]);
    assert_eq!(sum.computers, 2);
    assert_eq!(
        words::notice(&sum, 2).unwrap(),
        "Media transferring to 2 computers · 31 % · working out the time left · playback may \
         be less smooth meanwhile"
    );
    let done = Progress {
        used: Tier {
            files_wanted: 2,
            files_here: 2,
            bytes_wanted: 10,
            bytes_here: 10,
            ..Tier::default()
        },
        ..Progress::default()
    };
    assert_eq!(words::notice(&done, 0), None);
    assert_eq!(words::progress(&done), "All here · 2 files");
}

#[test]
fn alone_a_path_is_itself() {
    // (No finder is put in place by any test: managers in tests keep theirs.)
    let p = Path::new("/Shows/clip.mp4");
    assert_eq!(local_path(p), Local::Here(p.to_path_buf()));
}

#[test]
fn a_card_is_drawn_dark_with_its_words_and_black_when_asked() {
    let card = Card {
        heading: "Media transferring".into(),
        name: "clip.mp4".into(),
        detail: "43 % · about 2 min".into(),
    };
    let img = card::draw(&card, (640, 360), false);
    assert_eq!((img.width, img.height), (640, 360));
    let px = |x: u32, y: u32| &img.data[((y * 640 + x) * 4) as usize..][..4];
    assert_eq!(px(2, 2), card::BACKGROUND);
    // Something lighter than the card where the words are.
    let lit = img
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[0] > 120 && p[1] > 120)
        .count();
    assert!(lit > 200, "{lit} light pixels");
    let black = card::draw(&card, (64, 36), true);
    assert!(
        black
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [0, 0, 0, 255])
    );
}

// --------------------------------------------------------------- loopback

/// `span` free ports in a row.
fn free_range(span: u16) -> u16 {
    for _ in 0..200 {
        let base = 20_000 + (node::random_u64() % 40_000) as u16;
        let ok = crate::net::discover::ports(base, span).all(|p| {
            std::net::UdpSocket::bind(("127.0.0.1", p)).is_ok()
                && std::net::TcpListener::bind(("127.0.0.1", p)).is_ok()
        });
        if ok {
            return base;
        }
    }
    panic!("no free ports");
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "newq-media-{name}-{}-{}",
        std::process::id(),
        node::random_u64()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Bytes that differ per file and per seed.
fn bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

/// One NewQ: its hub, its media manager and the show it has open.
struct Node {
    hub: Hub,
    mgr: Manager,
    show: Show,
    dir: PathBuf,
    at: Positions,
}

impl Node {
    fn id(&self) -> NodeId {
        self.hub.view().me.id
    }

    /// One pass of the app's: messages in, the manager, messages out, the report.
    fn pass(&mut self) {
        let v = self.hub.view();
        for ev in self.hub.take_events() {
            if let NetEvent::Message { from, msg } = ev {
                self.mgr.handle(from, &msg, &v);
            }
        }
        let out = self.mgr.update(Frame {
            net: &v,
            show: &self.show,
            show_dir: None,
            at: self.at.clone(),
            dur: &mut |_| None,
            ff: None,
        });
        for o in out {
            match o {
                Outgoing::To(id, m) => self.hub.send_to(id, m),
            }
        }
        self.hub.set_media(self.mgr.report());
    }

    /// Where this follower has the file a show path names.
    fn local(&self, path: &Path) -> Local {
        match self.mgr.finder() {
            Some(f) => f.local(path),
            None => Local::Here(path.to_path_buf()),
        }
    }
}

struct Rig {
    base: u16,
    dirs: Vec<PathBuf>,
}

impl Rig {
    fn new() -> Rig {
        Rig {
            base: free_range(4),
            dirs: Vec::new(),
        }
    }

    fn node(&mut self, name: &str, rate: Option<u64>, parallel: usize) -> Node {
        let dir = scratch(name);
        self.dirs.push(dir.clone());
        self.node_in(name, dir, rate, parallel)
    }

    fn node_in(&mut self, name: &str, dir: PathBuf, rate: Option<u64>, parallel: usize) -> Node {
        let mut me = node::load(&dir);
        me.name = Some(name.to_string());
        node::save(&dir, &me).unwrap();
        let hub = Hub::start(
            HubConfig {
                dir: dir.clone(),
                port_base: self.base,
                port_span: 4,
                lan: false,
            },
            || {},
        );
        let cfg = Config {
            dir: dir.clone(),
            lan: false,
            serve_rate: rate,
            pull: PullOptions {
                parallel,
                timeouts: Timeouts {
                    connect: Duration::from_secs(5),
                    idle: Duration::from_secs(10),
                },
                retry_wait: Duration::from_millis(100),
                retry_max: Duration::from_millis(1000),
                poll: Duration::from_millis(300),
                disk_retry: Duration::from_millis(500),
                ..PullOptions::default()
            },
            watch_every: Duration::from_millis(300),
            settle: Duration::from_millis(200),
            relist_every: Duration::from_secs(3600),
            global: false,
            every: Duration::from_millis(200),
        };
        Node {
            hub,
            mgr: Manager::new(cfg),
            show: Show::default(),
            dir,
            at: Positions::default(),
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        for d in &self.dirs {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

/// Run every node's passes until `f` holds (up to a minute and a half).
fn until(nodes: &mut [&mut Node], what: &str, mut f: impl FnMut(&mut [&mut Node]) -> bool) {
    let began = Instant::now();
    loop {
        for n in nodes.iter_mut() {
            n.pass();
        }
        if f(nodes) {
            return;
        }
        assert!(
            began.elapsed() < Duration::from_secs(90),
            "waited too long for {what}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A primary and followers it has adopted.
fn network(p: &mut Node, others: &mut [&mut Node]) {
    p.hub.ask(NetAction::BecomePrimary);
    for o in others.iter() {
        o.hub.ask(NetAction::SetOn(true));
    }
    let ids: Vec<NodeId> = others.iter().map(|o| o.id()).collect();
    let mut adopted = std::collections::HashSet::new();
    let began = Instant::now();
    loop {
        let v = p.hub.view();
        for id in &ids {
            if let Some(peer) = v.peer(*id)
                && peer.waiting
                && peer.connected
                && adopted.insert(*id)
            {
                p.hub.ask(NetAction::Adopt(*id));
            }
        }
        let all = ids.iter().all(|id| {
            v.peer(*id)
                .is_some_and(|x| x.ours && x.connected && x.status.is_some())
        }) && others
            .iter()
            .all(|o| o.hub.view().following.as_ref().is_some_and(|f| f.online));
        if all {
            return;
        }
        assert!(
            began.elapsed() < Duration::from_secs(60),
            "the computers weren't adopted: {v:#?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The same show on every node (until show sync is in): cues of these files,
/// in this order, and the library folders.
fn same_show(nodes: &mut [&mut Node], files: &[PathBuf], library: &[PathBuf]) {
    let mut show = Show::default();
    show.cues.clear();
    for f in files {
        let at = show.cues.len();
        let c = show.make_cue(CueType::Video, Some(f.clone()), at);
        show.cues.push(c);
    }
    show.library_folders = library.to_vec();
    for n in nodes.iter_mut() {
        n.show = show.clone();
        n.at = Positions {
            active_list: 0,
            playhead: show.cues.first().map(|c| c.id),
            ..Positions::default()
        };
    }
}

fn cue_of(show: &Show, file: &Path) -> CueId {
    show.cues
        .iter()
        .find(|c| c.kind.file_media().is_some_and(|m| m.file == file))
        .unwrap()
        .id
}

/// The paths a follower has, in the order they arrived (looked at each pass).
#[derive(Default)]
struct Arrivals(Vec<PathBuf>);

impl Arrivals {
    fn note(&mut self, n: &Node, files: &[PathBuf]) {
        for f in files {
            if !self.0.contains(f) && n.local(f).is_here() {
                self.0.push(f.clone());
            }
        }
    }
}

#[test]
fn files_arrive_in_playhead_order_then_the_library_and_progress_reaches_the_primary() {
    let mut rig = Rig::new();
    let media = scratch("show-media");
    rig.dirs.push(media.clone());
    // Five clips the show uses, a library folder of two more.
    let files: Vec<PathBuf> = (0..5)
        .map(|i| {
            let f = media.join(format!("clip{i}.mp4"));
            std::fs::write(&f, bytes(60_000, i)).unwrap();
            f
        })
        .collect();
    let lib = media.join("library");
    std::fs::create_dir_all(&lib).unwrap();
    for i in 0..2 {
        std::fs::write(lib.join(format!("spare{i}.mov")), bytes(30_000, 100 + i)).unwrap();
    }
    // 100 KB/s from the primary, one file at a time: the order shows.
    let mut p = rig.node("Booth", Some(100_000), 1);
    let mut a = rig.node("Left", None, 1);
    let mut b = rig.node("Right", None, 1);
    network(&mut p, &mut [&mut a, &mut b]);
    same_show(
        &mut [&mut p, &mut a, &mut b],
        &files,
        std::slice::from_ref(&lib),
    );
    // The playhead on the fourth clip.
    p.at.playhead = Some(cue_of(&p.show, &files[3]));
    let mut seen_a = Arrivals::default();
    let mut seen_b = Arrivals::default();
    let spares: Vec<PathBuf> = (0..2).map(|i| lib.join(format!("spare{i}.mov"))).collect();
    // A follower's card never hands out a partial file: whenever it says
    // Here, the file is whole and right.
    let want: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|f| (f.clone(), std::fs::read(f).unwrap()))
        .collect();
    until(&mut [&mut p, &mut a, &mut b], "every file on both", |n| {
        let (p, rest) = n.split_first_mut().unwrap();
        let _ = p;
        for (i, f) in rest.iter().enumerate() {
            let seen = if i == 0 { &mut seen_a } else { &mut seen_b };
            seen.note(f, &files);
            for file in &files {
                if let Local::Here(at) = f.local(file) {
                    assert_eq!(std::fs::read(&at).unwrap(), want[file], "a whole file");
                }
            }
        }
        rest.iter().all(|f| {
            f.mgr
                .report()
                .is_some_and(|r| r.progress.all().complete() && r.progress.library.files_here == 2)
        })
    });
    // Nearest the playhead first: the fourth and fifth, then back from it.
    for seen in [&seen_a, &seen_b] {
        let order: Vec<String> = seen
            .0
            .iter()
            .map(|f| words::file_name(&paths::path_text(f)))
            .collect();
        assert_eq!(
            order,
            [
                "clip3.mp4",
                "clip4.mp4",
                "clip2.mp4",
                "clip1.mp4",
                "clip0.mp4"
            ],
            "arrivals"
        );
    }
    // The library is there too, as objects of the store.
    let store = a.mgr.store().unwrap();
    let m = store.current().unwrap();
    for s in &spares {
        let e = m.find(&paths::path_text(s)).unwrap();
        assert!(e.library);
        assert!(store.has(&e.object()));
    }
    // The primary hears of both, and adds them up.
    until(
        &mut [&mut p, &mut a, &mut b],
        "the sum on the primary",
        |n| {
            let v = n[0].mgr.view();
            v.sum
                .as_ref()
                .is_some_and(|s| s.computers == 2 && s.all().complete())
                && v.notice().is_none()
        },
    );
    let v = p.mgr.view();
    assert_eq!(v.side, Side::Primary);
    let sum = v.sum.clone().unwrap();
    assert_eq!(sum.used.files_here, 10);
    assert_eq!(sum.library.files_here, 4);
    assert!(v.lacking.is_empty());
    assert_eq!(v.folders.len(), 1);
    assert_eq!(v.folders[0].files, 2);
    assert_eq!(v.folders[0].bytes, 60_000);
}

#[test]
fn a_jump_reorders_without_fetching_again_and_marks_follow_the_reports() {
    let mut rig = Rig::new();
    let media = scratch("jump-media");
    rig.dirs.push(media.clone());
    let files: Vec<PathBuf> = (0..6)
        .map(|i| {
            let f = media.join(format!("j{i}.mp4"));
            std::fs::write(&f, bytes(50_000, 10 + i)).unwrap();
            f
        })
        .collect();
    let mut p = rig.node("Booth", Some(60_000), 1);
    let mut a = rig.node("Left", None, 1);
    network(&mut p, &mut [&mut a]);
    same_show(&mut [&mut p, &mut a], &files, &[]);
    let mut seen = Arrivals::default();
    // While the first file is on its way, the primary's marks say so.
    until(&mut [&mut p, &mut a], "a mark on the primary", |n| {
        n[0].mgr
            .view()
            .lacks(&files[1])
            .is_some_and(|l| !l.is_empty())
    });
    until(&mut [&mut p, &mut a], "the first file", |n| {
        seen.note(n[1], &files);
        !seen.0.is_empty()
    });
    assert_eq!(seen.0[0], files[0]);
    // A jump to the last file: it comes next (the one in flight finishes).
    p.at.playhead = Some(cue_of(&p.show, &files[5]));
    until(&mut [&mut p, &mut a], "every file", |n| {
        seen.note(n[1], &files);
        seen.0.len() == files.len()
    });
    let at5 = seen.0.iter().position(|f| *f == files[5]).unwrap();
    assert!(at5 <= 2, "the jumped-to file came next: {:?}", seen.0);
    // Nothing was fetched twice.
    let puller = a.mgr.puller().unwrap();
    let total: u64 = 6 * 50_000;
    until(&mut [&mut p, &mut a], "the puller's count", |_| {
        puller.snapshot().received >= total
    });
    assert_eq!(puller.snapshot().received, total);
    until(&mut [&mut p, &mut a], "the marks to clear", |n| {
        n[0].mgr.view().lacking.is_empty()
    });
}

#[test]
fn files_added_and_changed_reach_the_followers_kept_as_versions_or_replaced() {
    let mut rig = Rig::new();
    let media = scratch("change-media");
    rig.dirs.push(media.clone());
    let mut files: Vec<PathBuf> = (0..2)
        .map(|i| {
            let f = media.join(format!("v{i}.mp4"));
            std::fs::write(&f, bytes(20_000, 50 + i)).unwrap();
            f
        })
        .collect();
    let mut p = rig.node("Booth", None, 2);
    let mut keep = rig.node("Keeps", None, 2);
    let mut repl = rig.node("Replaces", None, 2);
    network(&mut p, &mut [&mut keep, &mut repl]);
    same_show(&mut [&mut p, &mut keep, &mut repl], &files, &[]);
    let all_here = |n: &Node, files: &[PathBuf]| files.iter().all(|f| n.local(f).is_here());
    until(
        &mut [&mut p, &mut keep, &mut repl],
        "both files on both",
        |n| all_here(n[1], &files) && all_here(n[2], &files),
    );
    // The primary sets the second one to replace.
    let rid = repl.id();
    p.hub.send_to(
        rid,
        crate::net::proto::Msg::MediaPolicy {
            keep: 5,
            replace_all: true,
            replace_over: None,
        },
    );
    until(
        &mut [&mut p, &mut keep, &mut repl],
        "the policy to be kept",
        |n| n[2].mgr.report().is_some_and(|r| r.policy.replace_all),
    );
    assert!(
        std::fs::read_to_string(repl.dir.join("media-policy.json"))
            .unwrap()
            .contains("\"replace_all\": true")
    );
    // A file added to the show on the primary (and on the others: show sync
    // isn't in yet) reaches them.
    let added = media.join("added.mp4");
    std::fs::write(&added, bytes(15_000, 77)).unwrap();
    files.push(added.clone());
    same_show(&mut [&mut p, &mut keep, &mut repl], &files, &[]);
    until(&mut [&mut p, &mut keep, &mut repl], "the added file", |n| {
        n[1].local(&added).is_here() && n[2].local(&added).is_here()
    });
    // A file changed on the primary: a new version; the old object stays on
    // the computer that keeps versions and goes from the one that replaces.
    let old_key = |n: &Node| {
        let m = n.mgr.store().unwrap().current().unwrap();
        m.find(&paths::path_text(&files[0])).unwrap().object()
    };
    let (k_old, r_old) = (old_key(&keep), old_key(&repl));
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&files[0], bytes(20_000, 999)).unwrap();
    let new_bytes = std::fs::read(&files[0]).unwrap();
    until(
        &mut [&mut p, &mut keep, &mut repl],
        "the new version",
        |n| {
            [1, 2].iter().all(|&i| match n[i].local(&files[0]) {
                Local::Here(at) => std::fs::read(at).is_ok_and(|b| b == new_bytes),
                _ => false,
            })
        },
    );
    until(
        &mut [&mut p, &mut keep, &mut repl],
        "the old one to go where it is replaced",
        |n| !n[2].mgr.store().unwrap().has(&r_old),
    );
    assert!(keep.mgr.store().unwrap().has(&k_old), "kept as a version");
}

#[test]
fn a_follower_that_restarts_resumes() {
    let mut rig = Rig::new();
    let media = scratch("resume-media");
    rig.dirs.push(media.clone());
    let big = media.join("big.mp4");
    std::fs::write(&big, bytes(400_000, 5)).unwrap();
    let files = vec![big.clone()];
    let mut p = rig.node("Booth", Some(80_000), 1);
    let mut a = rig.node("Left", None, 1);
    network(&mut p, &mut [&mut a]);
    same_show(&mut [&mut p, &mut a], &files, &[]);
    until(
        &mut [&mut p, &mut a],
        "part of the file",
        |n| matches!(n[1].local(&big), Local::Transferring(t) if t.known && t.done > 100_000),
    );
    // NewQ on that computer stops and starts again (the same folder).
    let dir = a.dir.clone();
    drop(a);
    let mut a = rig.node_in("Left", dir, None, 1);
    same_show(&mut [&mut a], &files, &[]);
    a.at = p.at.clone();
    until(&mut [&mut p, &mut a], "the whole file", |n| {
        n[1].local(&big).is_here()
    });
    let got = a.mgr.puller().unwrap().snapshot().received;
    assert!(got < 380_000, "resumed, not fetched again: {got} bytes");
    match a.local(&big) {
        Local::Here(at) => assert_eq!(std::fs::read(at).unwrap(), std::fs::read(&big).unwrap()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn alone_the_manager_does_nothing() {
    let mut rig = Rig::new();
    let mut n = rig.node("Alone", None, 1);
    for _ in 0..5 {
        n.pass();
        std::thread::sleep(Duration::from_millis(20));
    }
    let v = n.mgr.view();
    assert_eq!(v.side, Side::Off);
    assert!(n.mgr.report().is_none());
    assert!(!n.dir.join("media").exists(), "no store made");
    assert!(!n.dir.join("media-hashes.json").exists());
    let _: &NetView = &n.hub.view();
    let _ = Arc::new(());
}
