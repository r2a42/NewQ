//! The store over loopback: real sockets on free ports, in temporary folders.
//! Nothing here assumes a fast computer: waits are on conditions with
//! generous limits, and timings are only ever checked from below.

use super::http::{NetError, get};
use super::local::Readiness;
use super::manifest::{self, HashProgress};
use super::space::FreeSpace;
use super::*;
use parking_lot::Mutex;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const LIMIT: Duration = Duration::from_secs(180);

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("newq_store_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < LIMIT, "gave up waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Bytes that differ with the seed (xorshift).
fn bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

fn sha(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    manifest::hex(&Sha256::digest(data))
}

fn put(path: &Path, data: &[u8]) {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(path, data).unwrap();
}

fn loopback() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

fn text(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// The primary: its manifest of `used` (and `library`), served on a free port.
struct Main {
    primary: Primary,
    server: Server,
    manifest: Manifest,
}

impl Main {
    fn new(used: &[PathBuf], library: &[PathBuf], show_dir: Option<&Path>) -> Main {
        Main::with(used, library, show_dir, ServeLimits::default())
    }

    fn with(
        used: &[PathBuf],
        library: &[PathBuf],
        show_dir: Option<&Path>,
        limits: ServeLimits,
    ) -> Main {
        let cache = Arc::new(Mutex::new(HashCache::open(None)));
        let src = Sources {
            used: used.to_vec(),
            library: library.to_vec(),
            library_folders: Vec::new(),
            show_dir: show_dir.map(Path::to_path_buf),
        };
        let m = manifest::build(
            &src,
            &cache,
            &AtomicBool::new(false),
            &Mutex::new(HashProgress::default()),
        )
        .unwrap();
        let primary = Primary::new(cache);
        primary.publish(m.clone());
        let server = Server::start(loopback(), Arc::new(primary.clone()), limits).unwrap();
        Main {
            primary,
            server,
            manifest: m,
        }
    }

    fn addr(&self) -> String {
        format!("127.0.0.1:{}", self.server.port)
    }

    /// Make a new version from these files and serve it.
    fn republish(&mut self, used: &[PathBuf], library: &[PathBuf]) {
        let src = Sources {
            used: used.to_vec(),
            library: library.to_vec(),
            ..Sources::default()
        };
        let m = manifest::build(
            &src,
            &self.primary.cache(),
            &AtomicBool::new(false),
            &Mutex::new(HashProgress::default()),
        )
        .unwrap();
        self.primary.publish(m.clone());
        self.manifest = m;
    }
}

fn no_reserve() -> Policy {
    Policy {
        reserve: 0,
        ..Policy::default()
    }
}

fn unlimited() -> FreeSpace {
    Arc::new(|_: &Path| None)
}

fn store(dir: &Path, policy: Policy) -> Store {
    Store::open(&dir.join("store"), policy, unlimited()).unwrap()
}

fn quick() -> PullOptions {
    PullOptions {
        timeouts: Timeouts {
            connect: Duration::from_secs(5),
            idle: Duration::from_secs(5),
        },
        retry_wait: Duration::from_millis(50),
        retry_max: Duration::from_millis(500),
        poll: Duration::from_millis(200),
        disk_retry: Duration::from_millis(500),
        ..PullOptions::default()
    }
}

fn wait_idle(p: &Puller) -> PullSnapshot {
    wait_for("the puller to finish", || p.snapshot().idle);
    p.snapshot()
}

fn read_resolved(s: &Store, path: &Path) -> Vec<u8> {
    let at = s
        .resolve(&text(path))
        .unwrap_or_else(|| panic!("{} isn't here", path.display()));
    std::fs::read(at).unwrap()
}

// ----- Fakes ------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Fake {
    /// The right length, the wrong bytes.
    Corrupt,
    /// Says the whole length, sends half, closes.
    HalfThenClose,
    /// Says the whole length, sends half, then nothing (a pulled plug).
    HalfThenStall,
}

/// A computer that answers every object request badly with `data`.
fn fake(kind: Fake, data: Vec<u8>) -> (String, Arc<AtomicBool>) {
    let listener = TcpListener::bind(loopback()).unwrap();
    let port = listener.local_addr().unwrap().port();
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    listener.set_nonblocking(true).unwrap();
    std::thread::spawn(move || {
        while !stop2.load(Ordering::Relaxed) {
            let Ok((mut s, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            let data = data.clone();
            let stop3 = stop2.clone();
            std::thread::spawn(move || {
                s.set_nonblocking(false).unwrap();
                let mut head = Vec::new();
                let mut b = [0u8; 1024];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut b) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&b[..n]),
                    }
                }
                let h = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    data.len()
                );
                let _ = s.write_all(h.as_bytes());
                match kind {
                    Fake::Corrupt => {
                        let bad: Vec<u8> = data.iter().map(|b| b ^ 0x5a).collect();
                        let _ = s.write_all(&bad);
                    }
                    Fake::HalfThenClose => {
                        let _ = s.write_all(&data[..data.len() / 2]);
                    }
                    Fake::HalfThenStall => {
                        let _ = s.write_all(&data[..data.len() / 2]);
                        while !stop3.load(Ordering::Relaxed) {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                    }
                }
            });
        }
    });
    (format!("127.0.0.1:{port}"), stop)
}

// ----- Syncing ------------------------------------------------------------------------------

#[test]
fn a_whole_show_arrives_and_is_found_by_the_primarys_paths() {
    let dir = scratch("full");
    let show = dir.join("primary").join("My Show");
    let a = show.join("media").join("clip one.mp4");
    let b = show.join("sound").join("fête été.WAV");
    let c = dir.join("elsewhere").join("still.png");
    let empty = show.join("empty.txt");
    let gone = show.join("not there.mov");
    put(&a, &bytes(1, 700_000));
    put(&b, &bytes(2, 120_000));
    put(&c, &bytes(3, 5_000));
    put(&empty, b"");
    let main = Main::new(
        &[a.clone(), b.clone(), c.clone(), empty.clone(), gone.clone()],
        &[],
        Some(&show),
    );
    let s = store(&dir, no_reserve());
    let p = Puller::start(s.clone(), vec![main.addr()], Follow::Current, quick());
    let snap = wait_idle(&p);
    for f in [&a, &b, &c, &empty] {
        assert_eq!(
            read_resolved(&s, f),
            std::fs::read(f).unwrap(),
            "{}",
            f.display()
        );
        assert_eq!(p.availability(&text(f)), Availability::Here);
        assert_eq!(s.ready(&text(f)), Readiness::Here);
    }
    // The decoders look at the extension.
    let at = s.resolve(&text(&b)).unwrap();
    assert_eq!(at.extension().unwrap(), "wav");
    assert!(at.starts_with(s.root().join("objects")));
    assert_eq!(s.resolve(&text(&gone)), None);
    assert_eq!(p.availability(&text(&gone)), Availability::NotFound);
    assert_eq!(s.ready(&text(&gone)), Readiness::MissingOnPrimary);
    assert_eq!(p.availability("/no/such/file.mp4"), Availability::NotInShow);
    assert_eq!(snap.not_found, vec![text(&gone)]);
    assert_eq!(snap.progress.used.percent(), 100.0);
    assert_eq!(snap.progress.used.files_not_found, 1);
    assert_eq!(snap.received, 825_000);
    assert_eq!(s.current_id(), Some(main.manifest.id.clone()));
    // The receiver's copy of the show, saved beside its media, is elsewhere.
    let mine = dir
        .join("receiver")
        .join("My Show")
        .join("media")
        .join("clip one.mp4");
    assert_eq!(read_resolved(&s, &mine), std::fs::read(&a).unwrap());
    // Asked again, nothing moves.
    assert!(s.plan(&main.manifest).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_interrupted_fetch_resumes_and_nothing_comes_twice() {
    let dir = scratch("resume");
    let a = dir.join("show").join("big.mov");
    let data = bytes(4, 2_000_000);
    put(&a, &data);
    let main = Main::new(std::slice::from_ref(&a), &[], None);
    let s = store(&dir, no_reserve());
    let slow = PullOptions {
        rate_cap: Some(400_000),
        ..quick()
    };
    let p = Puller::start(s.clone(), vec![main.addr()], Follow::Current, slow);
    wait_for(
        "some of it",
        || matches!(p.availability(&text(&a)), Availability::Transferring { done, .. } if done >= 300_000),
    );
    let first = {
        drop(p);
        // The scheduler is gone; its transfer stops at its next read.
        let key = main.manifest.entries[0].object();
        let part = s.part_path(&key);
        wait_for("the transfer to stop", || {
            let l1 = std::fs::metadata(&part).map_or(0, |m| m.len());
            std::thread::sleep(Duration::from_millis(400));
            l1 == std::fs::metadata(&part).map_or(0, |m| m.len())
        });
        std::fs::metadata(&part).unwrap().len()
    };
    assert!(first > 0 && first < 2_000_000, "{first}");
    assert_eq!(
        s.resolve(&text(&a)),
        None,
        "a partial file is never handed out"
    );
    // A crash-like restart: the store opened again from the disk.
    let s2 = Store::open(s.root(), no_reserve(), unlimited()).unwrap();
    assert_eq!(s2.resolve(&text(&a)), None);
    let p = Puller::start(s2.clone(), vec![main.addr()], Follow::Current, quick());
    // The `.part` counts as done from the start.
    wait_for("the progress to be known", || {
        p.snapshot().progress.used.bytes_wanted > 0
    });
    assert!(p.snapshot().progress.used.bytes_done() >= first);
    let snap = wait_idle(&p);
    assert_eq!(read_resolved(&s2, &a), data);
    assert_eq!(snap.received, 2_000_000 - first, "only the rest came");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn damaged_bytes_are_refused_and_a_good_computer_supplies_them() {
    let dir = scratch("corrupt");
    let a = dir.join("show").join("clip.mp4");
    let data = bytes(5, 300_000);
    put(&a, &data);
    let main = Main::new(std::slice::from_ref(&a), &[], None);
    let (bad, stop) = fake(Fake::Corrupt, data.clone());
    // Only the bad one: never here.
    let s = store(&dir, no_reserve());
    let p = Puller::start(
        s.clone(),
        vec![bad.clone()],
        Follow::Fixed(main.manifest.clone()),
        quick(),
    );
    wait_for("a failure to be told", || !p.snapshot().failed.is_empty());
    let why = &p.snapshot().failed[0].1;
    assert!(why.contains("didn't match"), "{why}");
    assert_eq!(s.resolve(&text(&a)), None);
    drop(p);
    // The bad one first, then the primary: the primary's bytes win.
    let p = Puller::start(s.clone(), vec![bad, main.addr()], Follow::Current, quick());
    wait_idle(&p);
    assert_eq!(read_resolved(&s, &a), data);
    stop.store(true, Ordering::Relaxed);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_computer_that_dies_mid_file_is_followed_by_one_that_finishes_it() {
    for kind in [Fake::HalfThenClose, Fake::HalfThenStall] {
        let dir = scratch(if kind == Fake::HalfThenClose {
            "dies"
        } else {
            "stalls"
        });
        let a = dir.join("show").join("clip.mp4");
        let data = bytes(6, 1_000_000);
        put(&a, &data);
        let main = Main::new(std::slice::from_ref(&a), &[], None);
        let (dying, stop) = fake(kind, data.clone());
        let s = store(&dir, no_reserve());
        let opts = PullOptions {
            timeouts: Timeouts {
                connect: Duration::from_secs(5),
                idle: Duration::from_secs(1),
            },
            ..quick()
        };
        let p = Puller::start(
            s.clone(),
            vec![dying, main.addr()],
            Follow::Fixed(main.manifest.clone()),
            opts,
        );
        wait_idle(&p);
        assert_eq!(read_resolved(&s, &a), data);
        // The second computer sent only the rest.
        assert_eq!(main.server.bytes_sent(), 500_000);
        stop.store(true, Ordering::Relaxed);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn a_receiver_passes_on_what_it_has() {
    let dir = scratch("relay");
    let a = dir.join("show").join("clip.mp4");
    put(&a, &bytes(7, 200_000));
    let main = Main::new(std::slice::from_ref(&a), &[], None);
    let first = Store::open(&dir.join("one"), no_reserve(), unlimited()).unwrap();
    let p = Puller::start(first.clone(), vec![main.addr()], Follow::Current, quick());
    wait_idle(&p);
    drop(p);
    let relay = Server::start(loopback(), Arc::new(first.clone()), ServeLimits::default()).unwrap();
    drop(main);
    // The primary is gone: the second gets it all from the first.
    let second = Store::open(&dir.join("two"), no_reserve(), unlimited()).unwrap();
    let p = Puller::start(
        second.clone(),
        vec![format!("127.0.0.1:{}", relay.port)],
        Follow::Current,
        quick(),
    );
    wait_idle(&p);
    assert_eq!(read_resolved(&second, &a), bytes(7, 200_000));
    let _ = std::fs::remove_dir_all(&dir);
}

// ----- Versions -----------------------------------------------------------------------------

#[test]
fn versions_are_kept_and_going_back_needs_no_fetch() {
    let dir = scratch("versions");
    let a = dir.join("show").join("a.mp4");
    let b = dir.join("show").join("b.png");
    put(&a, &bytes(8, 100_000));
    put(&b, &bytes(9, 10_000));
    let mut main = Main::new(&[a.clone(), b.clone()], &[], None);
    let v1 = main.manifest.clone();
    let s = store(&dir, no_reserve());
    let p = Puller::start(s.clone(), vec![main.addr()], Follow::Current, quick());
    wait_idle(&p);
    put(&a, &bytes(10, 100_000));
    main.republish(&[a.clone(), b.clone()], &[]);
    let v2 = main.manifest.clone();
    assert_ne!(v1.id, v2.id);
    p.check_now();
    wait_for("the new version", || {
        let snap = p.snapshot();
        snap.idle && snap.progress.manifest_id.as_deref() == Some(v2.id.as_str())
    });
    assert_eq!(read_resolved(&s, &a), bytes(10, 100_000));
    assert_eq!(s.kept_ids(), vec![v1.id.clone(), v2.id.clone()]);
    drop(p);
    // Back to the first: everything is here.
    let plan = s.plan(&v1);
    assert!(plan.is_empty(), "{plan:?}");
    s.make_current(&v1.id).unwrap();
    assert_eq!(read_resolved(&s, &a), bytes(8, 100_000));
    assert_eq!(s.gc().objects_removed, 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn replace_mode_drops_the_old_file_once_the_new_one_is_verified() {
    let dir = scratch("replace");
    let a = dir.join("show").join("huge.mov");
    let small = dir.join("show").join("small.png");
    put(&a, &bytes(11, 200_000));
    put(&small, &bytes(12, 500));
    let mut main = Main::new(&[a.clone(), small.clone()], &[], None);
    let v1 = main.manifest.clone();
    let policy = Policy {
        replace_over: Some(100_000),
        ..no_reserve()
    };
    let s = store(&dir, policy);
    let p = Puller::start(s.clone(), vec![main.addr()], Follow::Current, quick());
    wait_idle(&p);
    let old_key = v1.find(&text(&a)).unwrap().object();
    put(&a, &bytes(13, 200_000));
    put(&small, &bytes(14, 500));
    main.republish(&[a.clone(), small.clone()], &[]);
    p.check_now();
    wait_for("the new version", || {
        let snap = p.snapshot();
        snap.idle && snap.progress.manifest_id.as_deref() == Some(main.manifest.id.as_str())
    });
    assert!(!s.has(&old_key), "the old huge file is gone");
    assert!(!s.object_path(&old_key).exists());
    // The small one is kept per version as usual.
    let old_small = v1.find(&text(&small)).unwrap().object();
    assert!(s.has(&old_small));
    // Going back needs the huge one fetched again.
    assert_eq!(s.plan(&v1).bytes_to_fetch, 200_000);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A disk of `capacity` bytes holding what is under `root`.
fn disk_of(capacity: Arc<AtomicU64>) -> FreeSpace {
    fn used(p: &Path) -> u64 {
        let Ok(rd) = std::fs::read_dir(p) else {
            return 0;
        };
        rd.flatten()
            .map(|e| match e.metadata() {
                Ok(m) if m.is_dir() => used(&e.path()),
                Ok(m) => m.len(),
                Err(_) => 0,
            })
            .sum()
    }
    Arc::new(move |root: &Path| {
        let mut r = root;
        while r.file_name().is_some_and(|n| n != "store") {
            r = r.parent()?;
        }
        Some(capacity.load(Ordering::SeqCst).saturating_sub(used(r)))
    })
}

#[test]
fn when_room_is_short_replace_frees_the_old_file_before_fetching() {
    for replace in [true, false] {
        let dir = scratch(if replace {
            "short_replace"
        } else {
            "short_keep"
        });
        let a = dir.join("show").join("huge.mov");
        put(&a, &bytes(15, 60_000));
        let mut main = Main::new(std::slice::from_ref(&a), &[], None);
        let capacity = Arc::new(AtomicU64::new(u64::MAX));
        let policy = Policy {
            replace_all: replace,
            ..no_reserve()
        };
        let s = Store::open(&dir.join("store"), policy, disk_of(capacity.clone())).unwrap();
        let p = Puller::start(s.clone(), vec![main.addr()], Follow::Current, quick());
        wait_idle(&p);
        // Room for one copy and a half (manifests count too).
        capacity.store(95_000, Ordering::SeqCst);
        put(&a, &bytes(16, 60_000));
        main.republish(std::slice::from_ref(&a), &[]);
        p.check_now();
        if replace {
            wait_for("the new version", || {
                let snap = p.snapshot();
                snap.idle && snap.progress.manifest_id.as_deref() == Some(main.manifest.id.as_str())
            });
            let plan = p.snapshot().plan.unwrap();
            assert_eq!(plan.free_first.len(), 1, "{plan:?}");
            assert!(plan.words().contains("Room is short"), "{}", plan.words());
            assert_eq!(read_resolved(&s, &a), bytes(16, 60_000));
        } else {
            wait_for("the problem to be told", || {
                p.snapshot()
                    .problem
                    .is_some_and(|w| w.contains("Not enough room"))
            });
            assert!(matches!(
                p.availability(&text(&a)),
                Availability::Transferring { .. }
            ));
            drop(p);
            // What is there stays: going back, the old version plays.
            let kept = s.kept_ids();
            assert_eq!(kept.len(), 2);
            s.make_current(&kept[0]).unwrap();
            assert_eq!(read_resolved(&s, &a), bytes(15, 60_000));
            let _ = std::fs::remove_dir_all(&dir);
            continue;
        }
        drop(p);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn a_full_disk_stops_fetching_cleanly_and_leaves_what_is_there() {
    let dir = scratch("full_disk");
    let a = dir.join("show").join("a.mp4");
    let b = dir.join("show").join("b.mp4");
    put(&a, &bytes(17, 50_000));
    let mut main = Main::new(std::slice::from_ref(&a), &[], None);
    let free = Arc::new(AtomicU64::new(u64::MAX));
    let f2 = free.clone();
    let space: FreeSpace = Arc::new(move |_: &Path| Some(f2.load(Ordering::SeqCst)));
    let s = Store::open(&dir.join("store"), no_reserve(), space).unwrap();
    let p = Puller::start(s.clone(), vec![main.addr()], Follow::Current, quick());
    wait_idle(&p);
    free.store(0, Ordering::SeqCst);
    put(&b, &bytes(18, 50_000));
    main.republish(&[a.clone(), b.clone()], &[]);
    p.check_now();
    wait_for("the full disk to be told", || {
        p.snapshot()
            .problem
            .is_some_and(|w| w.contains("room") || w.contains("disk"))
    });
    assert_eq!(read_resolved(&s, &a), bytes(17, 50_000));
    assert_eq!(s.resolve(&text(&b)), None);
    // Room again: it carries on by itself.
    free.store(u64::MAX, Ordering::SeqCst);
    wait_for("the rest", || {
        p.availability(&text(&b)) == Availability::Here
    });
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_changed_on_the_primary_mid_transfer_ends_up_as_its_new_bytes() {
    let dir = scratch("changed");
    let a = dir.join("show").join("clip.mp4");
    put(&a, &bytes(19, 600_000));
    // The primary reads slowly, so the edit lands in what it sends.
    let limits = ServeLimits {
        rate_cap: Some(200_000),
        ..ServeLimits::default()
    };
    let main = Main::with(std::slice::from_ref(&a), &[], None, limits);
    let _watch = main.primary.watch(Duration::from_millis(300));
    let first_id = main.manifest.id.clone();
    let s = store(&dir, no_reserve());
    let p = Puller::start(s.clone(), vec![main.addr()], Follow::Current, quick());
    wait_for(
        "some of it",
        || matches!(p.availability(&text(&a)), Availability::Transferring { done, .. } if done >= 100_000),
    );
    // Edited in place: same length, other bytes, a later time.
    let mut f = std::fs::OpenOptions::new().write(true).open(&a).unwrap();
    f.write_all(&bytes(20, 600_000)).unwrap();
    f.set_modified(std::time::SystemTime::now() + Duration::from_secs(10))
        .unwrap();
    drop(f);
    wait_for("the new bytes", || {
        s.current_id().is_some_and(|id| id != first_id) && p.snapshot().idle
    });
    assert_eq!(read_resolved(&s, &a), bytes(20, 600_000));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn two_pullers_on_one_store_fetch_each_file_once() {
    let dir = scratch("twice");
    let files: Vec<PathBuf> = (0..4)
        .map(|i| dir.join("show").join(format!("f{i}.mp4")))
        .collect();
    for (i, f) in files.iter().enumerate() {
        put(f, &bytes(30 + i as u64, 250_000));
    }
    let main = Main::new(&files, &[], None);
    let s = store(&dir, no_reserve());
    let p1 = Puller::start(s.clone(), vec![main.addr()], Follow::Current, quick());
    let p2 = Puller::start(s.clone(), vec![main.addr()], Follow::Current, quick());
    wait_idle(&p1);
    wait_idle(&p2);
    assert_eq!(p1.snapshot().received + p2.snapshot().received, 1_000_000);
    for (i, f) in files.iter().enumerate() {
        assert_eq!(read_resolved(&s, f), bytes(30 + i as u64, 250_000));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ----- Never a partial file -----------------------------------------------------------------

#[test]
fn no_path_to_incomplete_bytes_is_ever_handed_out() {
    let dir = scratch("never_partial");
    let files: Vec<PathBuf> = (0..5)
        .map(|i| dir.join("show").join(format!("clip {i}.mov")))
        .collect();
    let sizes: Vec<usize> = (0..5).map(|i| 150_000 + i * 70_000).collect();
    for (i, f) in files.iter().enumerate() {
        put(f, &bytes(40 + i as u64, sizes[i]));
    }
    let main = Main::new(&files, &[], None);
    let s = store(&dir, no_reserve());
    let done = Arc::new(AtomicBool::new(false));
    let seen = Arc::new(AtomicUsize::new(0));
    // Something asking all the time, as a player would.
    let asker = {
        let (s, files, sizes, done, seen) = (
            s.clone(),
            files.clone(),
            sizes.clone(),
            done.clone(),
            seen.clone(),
        );
        std::thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                for (i, f) in files.iter().enumerate() {
                    if let Some(at) = s.resolve(&text(f)) {
                        let name = at.to_string_lossy().into_owned();
                        assert!(!name.ends_with(".part"), "{name}");
                        let len = std::fs::metadata(&at).map(|m| m.len()).unwrap_or(0);
                        assert_eq!(len, sizes[i] as u64, "{name} handed out incomplete");
                        seen.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        })
    };
    let slow = PullOptions {
        rate_cap: Some(300_000),
        parallel: 2,
        ..quick()
    };
    // Interrupted, re-ordered (a file put aside), opened again, finished.
    let p = Puller::start(s.clone(), vec![main.addr()], Follow::Current, slow.clone());
    wait_for("a start", || p.snapshot().received > 100_000);
    p.set_order(Some(files.iter().rev().map(|f| text(f)).collect()));
    std::thread::sleep(Duration::from_millis(1500));
    drop(p);
    let s2 = Store::open(s.root(), no_reserve(), unlimited()).unwrap();
    let p = Puller::start(s2.clone(), vec![main.addr()], Follow::Current, slow);
    wait_idle(&p);
    done.store(true, Ordering::Relaxed);
    asker.join().expect("the asker found a partial file");
    assert!(seen.load(Ordering::Relaxed) > 0);
    for (i, f) in files.iter().enumerate() {
        assert_eq!(read_resolved(&s2, f), bytes(40 + i as u64, sizes[i]));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_object_of_the_wrong_length_found_at_start_up_is_not_handed_out() {
    let dir = scratch("wrong_len");
    let a = dir.join("show").join("clip.mp4");
    put(&a, &bytes(50, 100_000));
    let main = Main::new(std::slice::from_ref(&a), &[], None);
    let s = store(&dir, no_reserve());
    let p = Puller::start(s.clone(), vec![main.addr()], Follow::Current, quick());
    wait_idle(&p);
    drop(p);
    let at = s.resolve(&text(&a)).unwrap();
    // Cut short behind NewQ's back.
    std::fs::OpenOptions::new()
        .write(true)
        .open(&at)
        .unwrap()
        .set_len(40_000)
        .unwrap();
    let s2 = Store::open(s.root(), no_reserve(), unlimited()).unwrap();
    assert_eq!(s2.resolve(&text(&a)), None);
    assert_eq!(s2.ready(&text(&a)), Readiness::Missing);
    let p = Puller::start(s2.clone(), vec![main.addr()], Follow::Current, quick());
    wait_idle(&p);
    assert_eq!(read_resolved(&s2, &a), bytes(50, 100_000));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_check_of_every_object_drops_damaged_ones() {
    let dir = scratch("verify");
    let a = dir.join("show").join("clip.mp4");
    let b = dir.join("show").join("pic.png");
    put(&a, &bytes(51, 80_000));
    put(&b, &bytes(52, 8_000));
    let main = Main::new(&[a.clone(), b.clone()], &[], None);
    let s = store(&dir, no_reserve());
    let p = Puller::start(s.clone(), vec![main.addr()], Follow::Current, quick());
    wait_idle(&p);
    drop(p);
    let at = s.resolve(&text(&a)).unwrap();
    let mut data = std::fs::read(&at).unwrap();
    data[1234] ^= 1;
    std::fs::write(&at, &data).unwrap();
    let report = s.verify(&AtomicBool::new(false), &Mutex::new(Default::default()));
    assert_eq!(report.checked, 2);
    assert_eq!(report.bad.len(), 1);
    assert_eq!(s.resolve(&text(&a)), None);
    assert!(s.resolve(&text(&b)).is_some());
    let p = Puller::start(s.clone(), vec![main.addr()], Follow::Current, quick());
    wait_idle(&p);
    assert_eq!(read_resolved(&s, &a), bytes(51, 80_000));
    let _ = std::fs::remove_dir_all(&dir);
}

// ----- Tidying ------------------------------------------------------------------------------

#[test]
fn a_tidy_up_removes_only_what_nothing_kept_names() {
    let dir = scratch("gc");
    let a = dir.join("show").join("a.mp4");
    put(&a, &bytes(60, 10_000));
    let mut main = Main::new(std::slice::from_ref(&a), &[], None);
    let policy = Policy {
        keep: 2,
        ..no_reserve()
    };
    let s = store(&dir, policy);
    let mut ids = Vec::new();
    for i in 0..3 {
        if i > 0 {
            put(&a, &bytes(60 + i, 10_000));
            main.republish(std::slice::from_ref(&a), &[]);
        }
        let p = Puller::start(
            s.clone(),
            vec![main.addr()],
            Follow::Fixed(main.manifest.clone()),
            quick(),
        );
        wait_idle(&p);
        ids.push(main.manifest.id.clone());
    }
    // The first version is past the two kept: its manifest and object went.
    assert_eq!(s.kept_ids(), ids[1..].to_vec());
    assert!(
        !s.root()
            .join("manifests")
            .join(format!("{}.json", ids[0]))
            .exists()
    );
    // Things the tidy-up must weigh.
    let orphan = ObjectKey {
        sha: sha(b"orphan"),
        ext: "mp4".into(),
    };
    put(&s.object_path(&orphan), b"orphan");
    let stale = ObjectKey {
        sha: sha(b"stale"),
        ext: "mov".into(),
    };
    put(&s.part_path(&stale), b"sta");
    let busy = ObjectKey {
        sha: sha(b"busy"),
        ext: "mov".into(),
    };
    put(&s.part_path(&busy), b"bu");
    let stranger = s.root().join("objects").join("notes.txt");
    put(&stranger, b"not ours");
    s.rescan();
    let claim = s.claim(&busy, &AtomicBool::new(false)).unwrap();
    let report = s.gc();
    assert_eq!(report.objects_removed, 1, "{report:?}");
    assert_eq!(report.parts_removed, 1, "{report:?}");
    assert!(!s.object_path(&orphan).exists());
    assert!(!s.part_path(&stale).exists());
    assert!(s.part_path(&busy).exists(), "a part in use stays");
    assert!(stranger.exists(), "nothing that isn't the store's");
    drop(claim);
    assert_eq!(read_resolved(&s, &a), bytes(62, 10_000));
    let _ = std::fs::remove_dir_all(&dir);
}

// ----- Order ---------------------------------------------------------------------------------

#[test]
fn jumping_about_hard_fetches_nothing_twice_and_loses_nothing() {
    let dir = scratch("jumping");
    let files: Vec<PathBuf> = (0..12)
        .map(|i| dir.join("show").join(format!("cue {i}.mp4")))
        .collect();
    let mut total = 0;
    for (i, f) in files.iter().enumerate() {
        let n = 120_000 + i * 15_000;
        total += n as u64;
        put(f, &bytes(70 + i as u64, n));
    }
    let main = Main::new(&files, &[], None);
    let s = store(&dir, no_reserve());
    let opts = PullOptions {
        rate_cap: Some(500_000),
        urgent_wait: Duration::from_millis(500),
        nearly_done_secs: 0.1,
        ..quick()
    };
    let p = Puller::start(s.clone(), vec![main.addr()], Follow::Current, opts);
    // Many new orders a second for a while.
    let mut seed = 7u64;
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(4) {
        let mut order: Vec<String> = files.iter().map(|f| text(f)).collect();
        for i in (1..order.len()).rev() {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            order.swap(i, (seed % (i as u64 + 1)) as usize);
        }
        // Now and then only part of the show.
        if seed.is_multiple_of(3) {
            order.truncate(5);
        }
        p.set_order(Some(order));
        std::thread::sleep(Duration::from_millis(30));
    }
    p.set_order(Some(files.iter().map(|f| text(f)).collect()));
    let snap = wait_idle(&p);
    assert_eq!(
        snap.received, total,
        "nothing came twice, every part resumed"
    );
    assert!(snap.most_at_once <= 3, "{}", snap.most_at_once);
    assert!(snap.reorders <= 4 + 2 + 30, "{}", snap.reorders);
    for (i, f) in files.iter().enumerate() {
        assert_eq!(
            read_resolved(&s, f),
            bytes(70 + i as u64, 120_000 + i * 15_000)
        );
    }
    assert_eq!(snap.progress.used.percent(), 100.0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_urgent_small_file_starts_promptly_when_huge_ones_fill_every_place() {
    let dir = scratch("urgent");
    let huge: Vec<PathBuf> = (0..3)
        .map(|i| dir.join("show").join(format!("huge {i}.mov")))
        .collect();
    for (i, f) in huge.iter().enumerate() {
        put(f, &bytes(80 + i as u64, 3_000_000));
    }
    let small = dir.join("show").join("small.png");
    put(&small, &bytes(90, 10_000));
    let mut all = huge.clone();
    all.push(small.clone());
    let main = Main::new(&all, &[], None);
    let s = store(&dir, no_reserve());
    let opts = PullOptions {
        rate_cap: Some(60_000),
        urgent_wait: Duration::from_secs(3),
        nearly_done_secs: 1.0,
        ..quick()
    };
    let p = Puller::start(s.clone(), vec![main.addr()], Follow::Current, opts);
    p.set_order(Some(huge.iter().map(|f| text(f)).collect()));
    wait_for("every place busy", || p.snapshot().in_flight.len() == 3);
    let mut order: Vec<String> = vec![text(&small)];
    order.extend(huge.iter().map(|f| text(f)));
    p.set_order(Some(order));
    wait_for("the small file", || {
        p.availability(&text(&small)) == Availability::Here
    });
    let snap = p.snapshot();
    assert_eq!(snap.put_aside, 1);
    assert!(snap.most_at_once <= 3);
    // The one put aside kept its part and goes on later.
    wait_for("three huge ones again", || {
        p.snapshot().in_flight.len() == 3
    });
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_stream_of_tiny_files_remakes_the_order_about_once_a_second() {
    let dir = scratch("tiny");
    let files: Vec<PathBuf> = (0..80)
        .map(|i| dir.join("show").join(format!("t{i}.png")))
        .collect();
    for (i, f) in files.iter().enumerate() {
        put(f, &bytes(100 + i as u64, 2_000));
    }
    let main = Main::new(&files, &[], None);
    let s = store(&dir, no_reserve());
    let opts = PullOptions {
        rate_cap: Some(40_000),
        ..quick()
    };
    let p = Puller::start(s.clone(), vec![main.addr()], Follow::Current, opts);
    let t = Instant::now();
    let mut n = 0;
    while !p.snapshot().idle {
        assert!(t.elapsed() < LIMIT);
        // A new order every 20 ms (the playhead creeping on).
        let mut order: Vec<String> = files.iter().map(|f| text(f)).collect();
        order.rotate_left(n % files.len());
        n += 1;
        p.set_order(Some(order));
        std::thread::sleep(Duration::from_millis(20));
    }
    let secs = t.elapsed().as_secs_f64();
    let snap = p.snapshot();
    assert!(snap.files_done_all(), "{snap:?}");
    assert!(n > 50);
    assert!(
        (snap.reorders as f64) <= secs + 2.0,
        "{} orders in {secs:.1} s",
        snap.reorders
    );
    let _ = std::fs::remove_dir_all(&dir);
}

impl PullSnapshot {
    fn files_done_all(&self) -> bool {
        self.progress.used.complete() && self.progress.library.complete()
    }
}

// ----- Library ------------------------------------------------------------------------------

#[test]
fn the_library_comes_after_the_used_files_and_costs_nothing_once_used() {
    let dir = scratch("library");
    let used: Vec<PathBuf> = (0..2)
        .map(|i| dir.join("show").join(format!("u{i}.mp4")))
        .collect();
    let folder = dir.join("show").join("library");
    let lib: Vec<PathBuf> = (0..3).map(|i| folder.join(format!("l{i}.mov"))).collect();
    for (i, f) in used.iter().chain(lib.iter()).enumerate() {
        put(f, &bytes(110 + i as u64, 100_000));
    }
    put(&folder.join("readme.txt"), b"not media");
    put(&folder.join(".hidden.mp4"), b"hidden");
    assert_eq!(media_in_folder(&folder), lib);
    let cache = Arc::new(Mutex::new(HashCache::open(None)));
    let src = Sources {
        used: used.clone(),
        library_folders: vec![folder.clone()],
        ..Sources::default()
    };
    let m = manifest::build(
        &src,
        &cache,
        &AtomicBool::new(false),
        &Mutex::new(HashProgress::default()),
    )
    .unwrap();
    assert_eq!(m.entries.iter().filter(|e| e.library).count(), 3);
    let primary = Primary::new(cache.clone());
    primary.publish(m.clone());
    let server = Server::start(
        loopback(),
        Arc::new(primary.clone()),
        ServeLimits::default(),
    )
    .unwrap();
    let addr = format!("127.0.0.1:{}", server.port);
    let s = store(&dir, no_reserve());
    let opts = PullOptions {
        rate_cap: Some(400_000),
        ..quick()
    };
    let p = Puller::start(s.clone(), vec![addr.clone()], Follow::Current, opts);
    let t = Instant::now();
    loop {
        assert!(t.elapsed() < LIMIT);
        let snap = p.snapshot();
        let lib_started = lib.iter().any(|f| match p.availability(&text(f)) {
            Availability::Here => true,
            Availability::Transferring { done, .. } => done > 0,
            _ => false,
        });
        if lib_started {
            for f in &used {
                assert_eq!(
                    p.availability(&text(f)),
                    Availability::Here,
                    "used before library"
                );
            }
        }
        if snap.idle {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let snap = p.snapshot();
    assert_eq!(snap.progress.library.files_here, 3);
    assert_eq!(snap.progress.used.files_here, 2);
    assert_eq!(snap.progress.percent(), 100.0);
    let received = snap.received;
    // A cue now uses a library file: same bytes, nothing to fetch.
    let src = Sources {
        used: vec![used[0].clone(), used[1].clone(), lib[0].clone()],
        library_folders: vec![folder.clone()],
        ..Sources::default()
    };
    let m2 = manifest::build(
        &src,
        &cache,
        &AtomicBool::new(false),
        &Mutex::new(HashProgress::default()),
    )
    .unwrap();
    assert_ne!(m2.id, m.id);
    assert!(!m2.find(&text(&lib[0])).unwrap().library);
    assert!(s.plan(&m2).is_empty());
    primary.publish(m2.clone());
    p.check_now();
    wait_for("the new version", || {
        let snap = p.snapshot();
        snap.idle && snap.progress.manifest_id.as_deref() == Some(m2.id.as_str())
    });
    assert_eq!(p.snapshot().received, received);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_library_file_put_in_a_cue_jumps_the_queue() {
    let dir = scratch("jump_queue");
    let folder = dir.join("lib");
    let lib: Vec<PathBuf> = (0..6).map(|i| folder.join(format!("l{i}.mov"))).collect();
    for (i, f) in lib.iter().enumerate() {
        put(f, &bytes(120 + i as u64, 200_000));
    }
    let cue = dir.join("show").join("cue.mp4");
    put(&cue, &bytes(130, 200_000));
    let mut main = Main::new(std::slice::from_ref(&cue), &lib, None);
    let s = store(&dir, no_reserve());
    let opts = PullOptions {
        rate_cap: Some(300_000),
        parallel: 1,
        ..quick()
    };
    let p = Puller::start(s.clone(), vec![main.addr()], Follow::Current, opts);
    wait_for("the library to start", || {
        p.snapshot()
            .in_flight
            .iter()
            .any(|f| f.path.contains("lib"))
    });
    // The last library file is now used by a cue.
    main.republish(&[cue.clone(), lib[5].clone()], &lib);
    p.check_now();
    wait_for("it to arrive", || {
        p.availability(&text(&lib[5])) == Availability::Here
    });
    let here = lib[..5]
        .iter()
        .filter(|f| p.availability(&text(f)) == Availability::Here)
        .count();
    assert!(
        here <= 3,
        "the used file came before the rest of the library ({here})"
    );
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

// ----- Progress -----------------------------------------------------------------------------

#[test]
fn progress_is_by_bytes_with_a_sensible_time_to_go() {
    let dir = scratch("progress");
    let a = dir.join("show").join("clip.mp4");
    let data = bytes(140, 1_600_000);
    put(&a, &data);
    let main = Main::new(std::slice::from_ref(&a), &[], None);
    let s = store(&dir, no_reserve());
    // A part left from before: the first 400 kB, which count as done.
    let key = main.manifest.entries[0].object();
    put(&s.part_path(&key), &data[..400_000]);
    let rate = 200_000u64;
    let opts = PullOptions {
        rate_cap: Some(rate),
        ..quick()
    };
    let p = Puller::start(s.clone(), vec![main.addr()], Follow::Current, opts);
    wait_for("a rate", || p.snapshot().progress.rate.is_some());
    let first = p.snapshot().progress;
    assert!(first.used.bytes_done() >= 400_000, "{first:?}");
    assert_eq!(first.used.bytes_wanted, 1_600_000);
    // After a few seconds the estimate is near bytes left over the cap.
    wait_for("a steady rate", || {
        let pr = p.snapshot().progress;
        pr.used.bytes_done() >= 900_000 || pr.used.complete()
    });
    let pr = p.snapshot().progress;
    if !pr.used.complete() {
        let left = pr.used.bytes_left() as f64;
        let eta = pr.eta_secs.expect("an estimate");
        let expect = left / rate as f64;
        assert!(
            eta >= expect * 0.6 && eta <= expect * 2.5,
            "eta {eta:.1} s, expected about {expect:.1} s"
        );
        assert!(pr.percent() < 100.0);
        let file = p.file_progress(&text(&a)).unwrap();
        assert_eq!(file.total, 1_600_000);
        assert!(file.percent() < 100.0);
    }
    let snap = wait_idle(&p);
    assert_eq!(snap.progress.percent(), 100.0);
    assert_eq!(snap.progress.eta_secs, Some(0.0));
    assert_eq!(snap.received, 1_200_000);
    assert_eq!(p.file_progress(&text(&a)), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn several_computers_progress_adds_up() {
    let t = |wanted, here, partial| Tier {
        files_wanted: 2,
        files_here: usize::from(here == wanted) * 2,
        files_not_found: 0,
        bytes_wanted: wanted,
        bytes_here: here,
        bytes_partial: partial,
    };
    let a = Progress {
        manifest_id: Some("x".into()),
        used: t(1000, 500, 100),
        library: t(1000, 1000, 0),
        rate: Some(10.0),
        eta_secs: Some(40.0),
        computers: 1,
    };
    let b = Progress {
        manifest_id: Some("x".into()),
        used: t(1000, 1000, 0),
        library: t(1000, 0, 0),
        rate: Some(20.0),
        eta_secs: Some(50.0),
        computers: 1,
    };
    let sum = Progress::sum(&[a.clone(), b.clone()]);
    assert_eq!(sum.computers, 2);
    assert_eq!(sum.used.bytes_done(), 1600);
    assert_eq!(sum.all().bytes_wanted, 4000);
    assert_eq!(sum.rate, Some(30.0));
    assert_eq!(sum.eta_secs, Some(50.0));
    assert_eq!(sum.manifest_id.as_deref(), Some("x"));
    assert!(sum.percent() < 100.0);
    let unknown = Progress {
        eta_secs: None,
        rate: None,
        ..b.clone()
    };
    assert_eq!(Progress::sum(&[a, unknown]).eta_secs, None);
    // A complete computer and its JSON for the primary.
    let done = Progress {
        used: t(1000, 1000, 0),
        library: t(0, 0, 0),
        eta_secs: Some(0.0),
        ..b
    };
    let json = serde_json::to_string(&done).unwrap();
    let back: Progress = serde_json::from_str(&json).unwrap();
    assert_eq!(back, done);
    assert_eq!(back.used.percent(), 100.0);
    // Every byte there but not verified: not 100.
    let unverified = Tier {
        files_here: 1,
        bytes_here: 500,
        bytes_partial: 500,
        ..t(1000, 0, 0)
    };
    assert!(unverified.percent() < 100.0);
}

// ----- The primary --------------------------------------------------------------------------

#[test]
fn the_primary_says_while_files_go_out_and_honours_its_cap() {
    let dir = scratch("serving");
    let a = dir.join("show").join("clip.mp4");
    put(&a, &bytes(150, 600_000));
    let limits = ServeLimits {
        rate_cap: Some(400_000),
        ..ServeLimits::default()
    };
    let main = Main::with(std::slice::from_ref(&a), &[], None, limits);
    assert!(!main.server.transfers().running);
    let s = store(&dir, no_reserve());
    let t = Instant::now();
    let p = Puller::start(s.clone(), vec![main.addr()], Follow::Current, quick());
    wait_for("files going out", || main.server.transfers().running);
    let tr = main.server.transfers();
    assert!(tr.computers <= 1 && tr.bytes_left <= 600_000, "{tr:?}");
    wait_idle(&p);
    // 600 kB at 400 kB a second (less the quarter second of credit).
    assert!(
        t.elapsed() >= Duration::from_millis(1000),
        "{:?}",
        t.elapsed()
    );
    wait_for("the notice to clear", || !main.server.transfers().running);
    assert_eq!(main.server.transfers().bytes_left, 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_server_turns_away_what_it_shouldnt_answer() {
    let dir = scratch("server");
    let a = dir.join("show").join("clip.mp4");
    put(&a, &bytes(160, 1_000));
    let main = Main::new(std::slice::from_ref(&a), &[], None);
    let never = AtomicBool::new(false);
    let t = Timeouts::default();
    let status = |path: &str, from: u64| match get(&main.addr(), path, from, &t, &never) {
        Ok(r) => r.status,
        Err(e) => panic!("{e:?}"),
    };
    let sha = &main.manifest.entries[0].sha256;
    assert_eq!(status(&format!("/object/{sha}"), 0), 200);
    assert_eq!(status(&format!("/object/{sha}"), 10), 206);
    assert_eq!(status(&format!("/object/{sha}"), 1_000), 416);
    assert_eq!(status("/object/../../etc/passwd", 0), 400);
    assert_eq!(status(&format!("/object/{}", "0".repeat(64)), 0), 404);
    assert_eq!(status("/manifest/current", 0), 200);
    assert_eq!(status("/manifest/nonsense", 0), 400);
    assert_eq!(status("/elsewhere", 0), 404);
    let mut r = get(&main.addr(), &format!("/object/{sha}"), 990, &t, &never).unwrap();
    assert_eq!(
        r.read_all(100, &never).unwrap(),
        bytes(160, 1_000)[990..].to_vec()
    );
    // Nobody listening.
    let gone = {
        let l = TcpListener::bind(loopback()).unwrap();
        l.local_addr().unwrap().port()
    };
    assert!(matches!(
        get(
            &format!("127.0.0.1:{gone}"),
            "/manifest/current",
            0,
            &t,
            &never
        ),
        Err(NetError::Unreachable(_))
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

// ----- Manifests and the hash cache ---------------------------------------------------------

#[test]
fn the_hash_cache_saves_reading_an_unchanged_file_again() {
    let dir = scratch("cache");
    let a = dir.join("show").join("clip.mp4");
    put(&a, &bytes(170, 300_000));
    let cache_file = dir.join("hashes.json");
    let src = Sources {
        used: vec![a.clone()],
        ..Sources::default()
    };
    let never = AtomicBool::new(false);
    let run = |cache: &Mutex<HashCache>| {
        let progress = Mutex::new(HashProgress::default());
        let m = manifest::build(&src, cache, &never, &progress).unwrap();
        (m, progress.lock().bytes_hashed)
    };
    let cache = Mutex::new(HashCache::open(Some(cache_file.clone())));
    let (m1, read1) = run(&cache);
    assert_eq!(read1, 300_000);
    let (m2, read2) = run(&cache);
    assert_eq!((read2, &m2.id), (0, &m1.id));
    // Kept on disk: a restart reads nothing either.
    let cache = Mutex::new(HashCache::open(Some(cache_file.clone())));
    assert_eq!(cache.lock().len(), 1);
    let (_, read3) = run(&cache);
    assert_eq!(read3, 0);
    // A new time: read again (same bytes, same id).
    let f = std::fs::OpenOptions::new().write(true).open(&a).unwrap();
    f.set_modified(std::time::SystemTime::now() + Duration::from_secs(5))
        .unwrap();
    drop(f);
    let (m4, read4) = run(&cache);
    assert_eq!((read4, &m4.id), (300_000, &m1.id));
    // Cancelled: no manifest.
    let stop = AtomicBool::new(true);
    put(&a, &bytes(171, 300_000));
    let r = manifest::build(&src, &cache, &stop, &Mutex::new(HashProgress::default()));
    assert!(matches!(r, Err(manifest::HashError::Cancelled)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hashing_on_a_thread_reports_progress_and_can_be_cancelled() {
    let dir = scratch("hashing");
    let files: Vec<PathBuf> = (0..3).map(|i| dir.join(format!("f{i}.mp4"))).collect();
    for (i, f) in files.iter().enumerate() {
        put(f, &bytes(180 + i as u64, 200_000));
    }
    let cache = Arc::new(Mutex::new(HashCache::open(None)));
    let src = Sources {
        used: files.clone(),
        ..Sources::default()
    };
    let h = Hashing::start(src.clone(), cache.clone());
    wait_for("the hashing", || h.is_done());
    let m = h.take().unwrap().unwrap();
    assert_eq!(m.entries.len(), 3);
    let p = h.progress();
    assert_eq!((p.files_done, p.files_total, p.bytes_done), (3, 3, 600_000));
    // Dropping one under way cancels it and waits for its thread.
    let fresh = Arc::new(Mutex::new(HashCache::open(None)));
    drop(Hashing::start(src, fresh));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn manifests_check_their_id_and_old_ones_read() {
    let e = |p: &str, data: &[u8]| Entry {
        path: p.into(),
        rel: None,
        size: data.len() as u64,
        sha256: sha(data),
        modified: 7,
        missing: false,
        library: false,
    };
    let m = Manifest::new(vec![
        e("/b.mp4", b"b"),
        e("/a.mp4", b"a"),
        e("/a.mp4", b"a"),
    ]);
    assert_eq!(m.entries.len(), 2);
    assert_eq!(m.entries[0].path, "/a.mp4");
    let json = m.to_json();
    assert!(
        !json.contains("library") && !json.contains("missing"),
        "{json}"
    );
    assert_eq!(Manifest::from_json(json.as_bytes()), Some(m.clone()));
    // Times aren't in the id.
    let mut later = m.clone();
    later.entries[0].modified = 99;
    later.made += 1000;
    assert_eq!(later.work_out_id(), m.id);
    // A changed entry no longer matches its id.
    let tampered = json.replace(&sha(b"a"), &sha(b"z"));
    assert_eq!(Manifest::from_json(tampered.as_bytes()), None);
}

#[test]
fn paths_from_other_systems_find_their_files() {
    let dir = scratch("paths");
    let data = b"windows bytes".to_vec();
    let entries = vec![
        Entry {
            path: r"C:\Shows\Big Night\Act 1\Clip.MOV".into(),
            rel: Some("Act 1/Clip.MOV".into()),
            size: data.len() as u64,
            sha256: sha(&data),
            modified: 0,
            missing: false,
            library: false,
        },
        Entry {
            path: "/Users/me/Shows/Big Night/sound/été.wav".into(),
            rel: Some("sound/été.wav".into()),
            size: 3,
            sha256: sha(b"abc"),
            modified: 0,
            missing: false,
            library: false,
        },
    ];
    let m = Manifest::new(entries);
    let s = store(&dir, no_reserve());
    s.add_manifest(&m).unwrap();
    s.make_current(&m.id).unwrap();
    let k1 = m
        .find(r"C:\Shows\Big Night\Act 1\Clip.MOV")
        .unwrap()
        .object();
    assert_eq!(k1.ext, "mov");
    put(&s.object_path(&k1), &data);
    let k2 = m
        .find("/Users/me/Shows/Big Night/sound/été.wav")
        .unwrap()
        .object();
    put(&s.object_path(&k2), b"abc");
    s.rescan();
    for asked in [
        r"C:\Shows\Big Night\Act 1\Clip.MOV",
        "c:/shows/big night/act 1/clip.mov",
        "/home/stage/Big Night/Act 1/Clip.MOV",
        r"D:\Copy\Act 1\Clip.MOV",
    ] {
        let at = s.resolve(asked).unwrap_or_else(|| panic!("{asked}"));
        assert_eq!(std::fs::read(at).unwrap(), data, "{asked}");
    }
    assert!(s.resolve(r"C:\Users\x\Desktop\sound\été.wav").is_some());
    assert!(
        s.resolve("/Users/me/Shows/Big Night/Act 2/Clip.MOV")
            .is_none()
    );
    assert_eq!(s.missing_for(&m), Vec::<Entry>::new());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_primary_notices_a_file_edited_after_everyone_has_it() {
    let dir = scratch("watch");
    let a = dir.join("show").join("clip.mp4");
    let gone = dir.join("show").join("later.png");
    put(&a, &bytes(190, 50_000));
    let main = Main::new(&[a.clone(), gone.clone()], &[], None);
    let _watch = main.primary.watch(Duration::from_millis(200));
    let s = store(&dir, no_reserve());
    let p = Puller::start(s.clone(), vec![main.addr()], Follow::Current, quick());
    wait_idle(&p);
    assert_eq!(p.availability(&text(&gone)), Availability::NotFound);
    put(&a, &bytes(191, 70_000));
    put(&gone, &bytes(192, 1_000));
    wait_for("the edit and the new file", || {
        p.availability(&text(&gone)) == Availability::Here
            && s.resolve(&text(&a))
                .is_some_and(|at| std::fs::read(at).unwrap() == bytes(191, 70_000))
    });
    let _ = std::fs::remove_dir_all(&dir);
}
