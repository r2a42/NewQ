//! Fetching a show version's media on a thread, for as long as the computer is
//! in a show network.
//!
//! `Puller` follows a version (the primary's current one, or one given) and
//! fetches what this computer lacks, a few files at a time, in the order the
//! caller asks for (nearest the playhead first; ruling R14), then the library
//! (R15). It answers "is this file here?" and "how far along is it?" at once
//! from what it already knows: nothing that asks ever waits on a transfer (R13).
//!
//! The order. A new order is taken up when a transfer ends and frees a place
//! (at most once every `reorder_every`, so a stream of small files doesn't
//! re-make it per file). A transfer under way is never put aside for a better
//! ranked file, with one exception: when the file at the top of the newest
//! order isn't under way, every place is taken, and the soonest of them to
//! end still needs more than `URGENT_WAIT`, the lowest ranked transfer (never
//! one nearly done) is put aside, its `.part` kept, so the top one starts now.
//! One such at a time.

use super::fetch::{FetchCtx, FetchError, fetch_object};
use super::http::{self, Timeouts};
use super::local::{Store, SyncPlan};
use super::manifest::{Lookup, Manifest, ObjectKey};
use super::rate::RateCap;
use parking_lot::{Condvar, Mutex};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The longest the top file waits for a place when every place is busy with
/// a long transfer: past this, the lowest ranked one is put aside.
pub const URGENT_WAIT: Duration = Duration::from_secs(30);

/// The rate is the bytes over about this long.
pub const RATE_WINDOW: Duration = Duration::from_secs(10);

/// How fetching behaves.
#[derive(Clone, Debug)]
pub struct PullOptions {
    /// Files fetched at once.
    pub parallel: usize,
    /// Bytes a second for all of them together (None: as fast as it goes).
    pub rate_cap: Option<u64>,
    pub timeouts: Timeouts,
    /// The wait after a failure, doubled each time up to `retry_max`.
    pub retry_wait: Duration,
    pub retry_max: Duration,
    /// How often the followed computer is asked for its current version.
    pub poll: Duration,
    /// A new order is taken up at most this often.
    pub reorder_every: Duration,
    /// See `URGENT_WAIT` (a setting so tests can be quick).
    pub urgent_wait: Duration,
    /// A transfer this far along (a fraction), or this few seconds from its
    /// end, is never put aside.
    pub nearly_done: f64,
    pub nearly_done_secs: f64,
    /// Every computer said it hasn't the file this many times: not found.
    pub not_found_after: u32,
    /// After the disk filled up, the wait before trying again.
    pub disk_retry: Duration,
}

impl Default for PullOptions {
    fn default() -> PullOptions {
        PullOptions {
            parallel: 3,
            rate_cap: None,
            timeouts: Timeouts::default(),
            retry_wait: Duration::from_secs(1),
            retry_max: Duration::from_secs(30),
            poll: Duration::from_secs(3),
            reorder_every: Duration::from_secs(1),
            urgent_wait: URGENT_WAIT,
            nearly_done: 0.9,
            nearly_done_secs: 5.0,
            not_found_after: 3,
            disk_retry: Duration::from_secs(30),
        }
    }
}

/// Which version to fetch for.
#[derive(Clone, Debug, PartialEq)]
pub enum Follow {
    /// Whatever the sources (the primary first) say is current, asked again
    /// every `poll` and whenever a file turns out to have changed.
    Current,
    /// This one.
    Fixed(Manifest),
}

/// What a computer can say about one of the show's files, at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    /// Here, whole and verified.
    Here,
    /// On its way (or waiting its turn): this much of it is here.
    Transferring { done: u64, total: u64 },
    /// No computer has it: missing on the primary too.
    NotFound,
    /// Not a file of the version being followed (or there is none yet).
    NotInShow,
}

/// One file's progress, for the "Media transferring" card.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FileProgress {
    pub done: u64,
    pub total: u64,
    /// Seconds to go at its own rate; None until that is known.
    pub eta_secs: Option<f64>,
}

impl FileProgress {
    pub fn percent(&self) -> f64 {
        percent(self.done, self.total, self.done >= self.total)
    }
}

/// Percent done; exactly 100 only when everything is verified.
fn percent(done: u64, wanted: u64, all_verified: bool) -> f64 {
    if all_verified {
        return 100.0;
    }
    if wanted == 0 {
        return 0.0;
    }
    (done as f64 * 100.0 / wanted as f64).min(99.9)
}

/// Files and bytes of one tier (used or library), by bytes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tier {
    /// Files that can be had (not those no computer has).
    pub files_wanted: usize,
    pub files_here: usize,
    /// Files no computer has (not in `files_wanted`).
    pub files_not_found: usize,
    pub bytes_wanted: u64,
    /// Bytes of the files here, verified.
    pub bytes_here: u64,
    /// Bytes of files on their way (`.part`s, resumed ones included).
    pub bytes_partial: u64,
}

impl Tier {
    pub fn bytes_done(&self) -> u64 {
        self.bytes_here + self.bytes_partial
    }

    pub fn bytes_left(&self) -> u64 {
        self.bytes_wanted.saturating_sub(self.bytes_done())
    }

    pub fn complete(&self) -> bool {
        self.files_here >= self.files_wanted
    }

    /// By bytes; exactly 100 only when every file is here and verified.
    pub fn percent(&self) -> f64 {
        percent(self.bytes_done(), self.bytes_wanted, self.complete())
    }

    fn add(&mut self, o: &Tier) {
        self.files_wanted += o.files_wanted;
        self.files_here += o.files_here;
        self.files_not_found += o.files_not_found;
        self.bytes_wanted += o.bytes_wanted;
        self.bytes_here += o.bytes_here;
        self.bytes_partial += o.bytes_partial;
    }
}

/// One computer's media progress: what the next stage sends to the primary,
/// and what `Progress::sum` adds up for the whole network.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    pub manifest_id: Option<String>,
    pub used: Tier,
    pub library: Tier,
    /// Bytes a second over about the last `RATE_WINDOW`. None until known.
    pub rate: Option<f64>,
    /// Seconds to go: bytes left over the rate. None while the rate is
    /// unknown or zero; 0 when nothing is left.
    pub eta_secs: Option<f64>,
    /// How many computers this is (1 for one; the sum of them for a sum).
    pub computers: usize,
}

impl Progress {
    /// Both tiers together.
    pub fn all(&self) -> Tier {
        let mut t = self.used.clone();
        t.add(&self.library);
        t
    }

    pub fn percent(&self) -> f64 {
        self.all().percent()
    }

    /// Several computers' progress as one: bytes summed, rates summed, and
    /// the time to go that of the slowest (None if any still busy has none).
    pub fn sum(list: &[Progress]) -> Progress {
        let mut out = Progress::default();
        let mut eta = Some(0.0f64);
        let mut any_rate = false;
        let mut rate = 0.0;
        for p in list {
            out.used.add(&p.used);
            out.library.add(&p.library);
            out.computers += p.computers.max(1);
            if let Some(r) = p.rate {
                any_rate = true;
                rate += r;
            }
            if p.all().bytes_left() > 0 {
                eta = match (eta, p.eta_secs) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    _ => None,
                };
            }
        }
        out.rate = any_rate.then_some(rate);
        out.eta_secs = eta;
        if let Some(first) = list.first()
            && list.iter().all(|p| p.manifest_id == first.manifest_id)
        {
            out.manifest_id = first.manifest_id.clone();
        }
        out
    }
}

/// One transfer under way.
#[derive(Clone, Debug, PartialEq)]
pub struct InFlight {
    pub path: String,
    pub done: u64,
    pub total: u64,
}

/// How the fetching stands, for the status panel. Copied out; never waits on
/// a transfer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PullSnapshot {
    pub progress: Progress,
    /// What the version needed, worked out when it was taken up.
    pub plan: Option<SyncPlan>,
    pub in_flight: Vec<InFlight>,
    /// Paths no computer has.
    pub not_found: Vec<String>,
    /// Paths that failed and are being tried again, and why.
    pub failed: Vec<(String, String)>,
    /// Something stopping all fetching for now (a full disk, no primary).
    pub problem: Option<String>,
    /// Every file wanted is here or can't be had.
    pub idle: bool,
    /// Counts since the start: bytes over the network, transfers started and
    /// put aside, orders taken up, and the most transfers at once.
    pub received: u64,
    pub started: usize,
    pub put_aside: usize,
    pub reorders: usize,
    pub most_at_once: usize,
}

struct Control {
    sources: Vec<String>,
    follow: Follow,
    /// Show paths, nearest the playhead first. None: the version's own order.
    order: Option<Vec<String>>,
    ask_now: bool,
}

/// A transfer as `file_progress` sees it.
#[derive(Clone)]
struct FlightView {
    done: Arc<AtomicU64>,
    started: Instant,
    start_done: u64,
}

/// What `availability` and `file_progress` read.
#[derive(Default)]
struct View {
    target: Option<Arc<(Manifest, Lookup)>>,
    /// Objects wanted and not here: bytes here so far, or None: not to be had.
    waiting: HashMap<ObjectKey, Option<u64>>,
    flights: HashMap<ObjectKey, FlightView>,
}

struct Shared {
    store: Store,
    opts: PullOptions,
    cap: Option<RateCap>,
    stop: AtomicBool,
    control: Mutex<Control>,
    wake: Condvar,
    snapshot: Mutex<PullSnapshot>,
    view: Mutex<View>,
    received: AtomicU64,
    running: AtomicUsize,
    most: AtomicUsize,
}

/// Fetching for a version on a thread. Dropping it stops it (transfers keep
/// their `.part`).
pub struct Puller {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Puller {
    pub fn start(store: Store, sources: Vec<String>, follow: Follow, opts: PullOptions) -> Puller {
        let shared = Arc::new(Shared {
            store,
            cap: opts.rate_cap.map(RateCap::new),
            opts,
            stop: AtomicBool::new(false),
            control: Mutex::new(Control {
                sources,
                follow,
                order: None,
                ask_now: true,
            }),
            wake: Condvar::new(),
            snapshot: Mutex::new(PullSnapshot::default()),
            view: Mutex::new(View::default()),
            received: AtomicU64::new(0),
            running: AtomicUsize::new(0),
            most: AtomicUsize::new(0),
        });
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("media-pull".into())
            .spawn(move || {
                let s2 = s.clone();
                let _said = crate::crash::OnPanic(move || {
                    s2.snapshot.lock().problem = Some("copying media stopped unexpectedly".into());
                });
                Scheduler::new(s).run();
            })
            .ok();
        Puller { shared, thread }
    }

    pub fn store(&self) -> &Store {
        &self.shared.store
    }

    /// The computers to fetch from, the likeliest first (the primary, then
    /// others that have the files).
    pub fn set_sources(&self, sources: Vec<String>) {
        self.shared.control.lock().sources = sources;
        self.shared.wake.notify_all();
    }

    pub fn set_follow(&self, follow: Follow) {
        let mut c = self.shared.control.lock();
        c.follow = follow;
        c.ask_now = true;
        drop(c);
        self.shared.wake.notify_all();
    }

    /// The files wanted, nearest the playhead first, as the show names them.
    /// Taken up when a transfer ends (see the module's words); the same order
    /// again changes nothing. A file left out isn't started (one under way
    /// finishes; a `.part` stays). None: every used file in the version's order.
    pub fn set_order(&self, order: Option<Vec<String>>) {
        self.shared.control.lock().order = order;
        self.shared.wake.notify_all();
    }

    /// Ask the followed computer for its current version now.
    pub fn check_now(&self) {
        self.shared.control.lock().ask_now = true;
        self.shared.wake.notify_all();
    }

    pub fn snapshot(&self) -> PullSnapshot {
        self.shared.snapshot.lock().clone()
    }

    /// This computer's numbers, to send to the primary.
    pub fn progress(&self) -> Progress {
        self.shared.snapshot.lock().progress.clone()
    }

    /// Here, on its way, or not to be had: from what is known already, never
    /// waiting on a transfer. Cheap enough for every cue every frame.
    pub fn availability(&self, show_path: &str) -> Availability {
        let view = self.shared.view.lock();
        let Some(t) = &view.target else {
            return Availability::NotInShow;
        };
        let Some(e) = t.1.find(&t.0, show_path) else {
            return Availability::NotInShow;
        };
        if e.missing {
            return Availability::NotFound;
        }
        let key = e.object();
        if self.shared.store.size_of(&key) == Some(e.size) {
            return Availability::Here;
        }
        if let Some(f) = view.flights.get(&key) {
            return Availability::Transferring {
                done: f.done.load(Ordering::Relaxed).min(e.size),
                total: e.size,
            };
        }
        match view.waiting.get(&key) {
            Some(None) => Availability::NotFound,
            Some(Some(done)) => Availability::Transferring {
                done: *done,
                total: e.size,
            },
            None => Availability::Transferring {
                done: 0,
                total: e.size,
            },
        }
    }

    /// A file's progress with its time to go (None when it isn't on its way).
    pub fn file_progress(&self, show_path: &str) -> Option<FileProgress> {
        let Availability::Transferring { done, total } = self.availability(show_path) else {
            return None;
        };
        let view = self.shared.view.lock();
        let t = view.target.clone()?;
        let key = t.1.find(&t.0, show_path)?.object();
        let eta_secs = view.flights.get(&key).and_then(|f| {
            let secs = f.started.elapsed().as_secs_f64();
            let got = done.saturating_sub(f.start_done) as f64;
            (secs >= 1.0 && got > 0.0).then(|| (total - done) as f64 / (got / secs))
        });
        Some(FileProgress {
            done,
            total,
            eta_secs,
        })
    }
}

impl Drop for Puller {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.wake.notify_all();
        // The scheduler tells its transfers to stop and doesn't wait for them.
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Item {
    size: u64,
    paths: Vec<String>,
    library: bool,
    /// Times every computer said it hasn't it.
    not_found: u32,
    failures: u32,
    next_try: Option<Instant>,
    unobtainable: bool,
    last_error: Option<String>,
    /// Bytes of its `.part` when its last transfer ended (or at the start).
    done: u64,
}

struct Flight {
    stop: Arc<AtomicBool>,
    view: FlightView,
    stopping: bool,
    handle: JoinHandle<Result<(), FetchError>>,
}

struct Scheduler {
    s: Arc<Shared>,
    target: Option<Arc<(Manifest, Lookup)>>,
    plan: Option<SyncPlan>,
    items: HashMap<ObjectKey, Item>,
    /// The ranking from the order taken up: the files asked for, then the
    /// library.
    used: Vec<ObjectKey>,
    library: Vec<ObjectKey>,
    order: Option<Vec<String>>,
    order_at: Option<Instant>,
    flights: HashMap<ObjectKey, Flight>,
    /// The transfer put aside for an urgent one, until it has stopped.
    aside: Option<ObjectKey>,
    urgent_at: Option<Instant>,
    order_pending: bool,
    /// The newest order asked for and its ranking (not yet taken up).
    asked: Option<Ranking>,
    next_ask: Option<Instant>,
    disk_wait: Option<Instant>,
    problem: Option<String>,
    tidied: bool,
    started: usize,
    put_aside: usize,
    reorders: usize,
    samples: VecDeque<(Instant, u64)>,
    /// The objects here, refreshed when the store's generation moves.
    here: HashMap<ObjectKey, u64>,
    here_gen: Option<u64>,
    last_publish: Option<Instant>,
}

impl Scheduler {
    fn new(s: Arc<Shared>) -> Scheduler {
        Scheduler {
            s,
            target: None,
            plan: None,
            items: HashMap::new(),
            used: Vec::new(),
            library: Vec::new(),
            order: None,
            order_at: None,
            flights: HashMap::new(),
            aside: None,
            urgent_at: None,
            order_pending: false,
            asked: None,
            next_ask: None,
            disk_wait: None,
            problem: None,
            tidied: false,
            started: 0,
            put_aside: 0,
            reorders: 0,
            samples: VecDeque::new(),
            here: HashMap::new(),
            here_gen: None,
            last_publish: None,
        }
    }

    fn run(mut self) {
        loop {
            if self.s.stop.load(Ordering::Relaxed) {
                for f in self.flights.values() {
                    f.stop.store(true, Ordering::Relaxed);
                }
                return;
            }
            let (sources, follow, order, ask_now) = {
                let mut c = self.s.control.lock();
                let ask = std::mem::take(&mut c.ask_now);
                (c.sources.clone(), c.follow.clone(), c.order.clone(), ask)
            };
            self.follow(&sources, &follow, ask_now);
            let ended = self.reap();
            self.refresh_here();
            self.take_order(order, ended, &sources);
            self.start(&sources);
            self.publish(ended);
            let mut c = self.s.control.lock();
            if !self.s.stop.load(Ordering::Relaxed) && !c.ask_now {
                self.s.wake.wait_for(&mut c, Duration::from_millis(100));
            }
        }
    }

    fn refresh_here(&mut self) {
        let g = self.s.store.generation();
        if self.here_gen != Some(g) {
            self.here = self.s.store.objects();
            self.here_gen = Some(g);
        }
    }

    fn here(&self, k: &ObjectKey) -> bool {
        match (self.here.get(k), self.items.get(k)) {
            (Some(s), Some(it)) => *s == it.size,
            (Some(_), None) => true,
            _ => false,
        }
    }

    /// Take up the version to fetch for.
    fn follow(&mut self, sources: &[String], follow: &Follow, ask_now: bool) {
        let now = Instant::now();
        match follow {
            Follow::Fixed(m) => {
                if self.target.as_ref().is_none_or(|t| t.0.id != m.id) {
                    self.take_target(m.clone());
                }
            }
            Follow::Current => {
                let due = ask_now || self.next_ask.is_none_or(|t| now >= t);
                if !due {
                    return;
                }
                self.next_ask = Some(now + self.s.opts.poll);
                let got = sources.iter().find_map(|src| {
                    http::get_manifest(src, "current", &self.s.opts.timeouts, &self.s.stop).ok()
                });
                match got {
                    Some(m) => {
                        if self.problem.as_deref() == Some(NO_PRIMARY) {
                            self.problem = None;
                        }
                        if self.target.as_ref().is_none_or(|t| t.0.id != m.id) {
                            self.take_target(m);
                        }
                    }
                    None if self.target.is_none() => self.problem = Some(NO_PRIMARY.into()),
                    None => {}
                }
            }
        }
    }

    fn take_target(&mut self, m: Manifest) {
        let store = self.s.store.clone();
        if let Err(e) = store
            .add_manifest(&m)
            .and_then(|_| store.make_current(&m.id))
        {
            self.problem = Some(e);
            return;
        }
        let plan = store.plan(&m);
        // Room is short: what may go first goes now (ruling R4).
        for k in &plan.free_first {
            store.remove_object(k);
        }
        if !plan.enough_room {
            self.problem = Some(plan.words());
        }
        let mut items: HashMap<ObjectKey, Item> = HashMap::new();
        for e in m.present() {
            let key = e.object();
            if let Some(it) = items.get_mut(&key) {
                it.paths.push(e.path.clone());
                it.library &= e.library;
                continue;
            }
            let old = self.items.remove(&key);
            // A `.part` left from before (another run, a file put aside)
            // counts as done already.
            let done = old.map_or_else(
                || std::fs::metadata(store.part_path(&key)).map_or(0, |md| md.len().min(e.size)),
                |o| o.done,
            );
            items.insert(
                key,
                Item {
                    size: e.size,
                    paths: vec![e.path.clone()],
                    library: e.library,
                    not_found: 0,
                    failures: 0,
                    next_try: None,
                    unobtainable: false,
                    last_error: None,
                    done,
                },
            );
        }
        self.items = items;
        // Transfers for files no longer in the show stop (their parts stay).
        for (k, f) in &mut self.flights {
            if !self.items.contains_key(k) {
                f.stop.store(true, Ordering::Relaxed);
                f.stopping = true;
            }
        }
        let lookup = Lookup::new(&m);
        self.target = Some(Arc::new((m, lookup)));
        self.plan = Some(plan);
        self.tidied = false;
        self.rank();
        self.last_publish = None;
    }

    /// Work out `used` and `library` from the order taken up and the version.
    fn rank(&mut self) {
        let (used, library) = self.ranking(self.order.as_deref());
        self.used = used;
        self.library = library;
    }

    fn ranking(&self, order: Option<&[String]>) -> (Vec<ObjectKey>, Vec<ObjectKey>) {
        let (mut used, mut library) = (Vec::new(), Vec::new());
        let Some(t) = &self.target else {
            return (used, library);
        };
        let (m, lookup) = (&t.0, &t.1);
        let mut seen = HashSet::new();
        match order {
            Some(paths) => {
                for p in paths {
                    if let Some(e) = lookup.find(m, p).filter(|e| !e.missing)
                        && seen.insert(e.object())
                    {
                        used.push(e.object());
                    }
                }
            }
            None => {
                for e in m.present().filter(|e| !e.library) {
                    if seen.insert(e.object()) {
                        used.push(e.object());
                    }
                }
            }
        }
        for e in m.present().filter(|e| e.library) {
            if seen.insert(e.object()) {
                library.push(e.object());
            }
        }
        (used, library)
    }

    /// Whether a file may be started (or go on) now.
    fn open(&self, k: &ObjectKey, now: Instant) -> bool {
        let Some(it) = self.items.get(k) else {
            return false;
        };
        !it.unobtainable
            && !self.here(k)
            && (self.flights.contains_key(k) || it.next_try.is_none_or(|t| now >= t))
    }

    /// The best file that may start now (the library only once the used ones
    /// are settled).
    fn first_open(
        &self,
        used: &[ObjectKey],
        library: &[ObjectKey],
        now: Instant,
    ) -> Option<ObjectKey> {
        if let Some(k) = used.iter().find(|k| self.open(k, now)) {
            return Some(k.clone());
        }
        if used.iter().all(|k| self.settled(k)) {
            return library.iter().find(|k| self.open(k, now)).cloned();
        }
        None
    }

    fn settled(&self, k: &ObjectKey) -> bool {
        self.here(k) || self.items.get(k).is_none_or(|it| it.unobtainable)
    }

    /// The files to fetch, best first: the used ones, then (once those are
    /// all here or can't be had) the library.
    fn ranked(&self, used: &[ObjectKey], library: &[ObjectKey]) -> Vec<ObjectKey> {
        let now = Instant::now();
        let mut out: Vec<ObjectKey> = used.iter().filter(|k| self.open(k, now)).cloned().collect();
        if used.iter().all(|k| self.settled(k)) {
            out.extend(library.iter().filter(|k| self.open(k, now)).cloned());
        }
        out
    }

    /// Take up a newer order: when a place is free (a transfer ended), at most
    /// once every `reorder_every`; or at once for an urgent file.
    fn take_order(&mut self, order: Option<Vec<String>>, ended: bool, sources: &[String]) {
        let p = self.s.opts.parallel.max(1);
        if order != self.order {
            let busy = self.flights.len() >= p;
            let soon = self
                .order_at
                .is_some_and(|t| t.elapsed() < self.s.opts.reorder_every);
            if (ended || !busy) && !soon {
                self.order = order.clone();
                self.order_at = Some(Instant::now());
                self.reorders += 1;
                self.rank();
            }
        }
        self.urgent(order.clone(), sources);
        // A newer order not yet taken up keeps the puller from being idle
        // only if it wants something not settled.
        self.order_pending = order != self.order && {
            if self.asked.as_ref().is_none_or(|(o, _, _)| *o != order) {
                let (u, l) = self.ranking(order.as_deref());
                self.asked = Some((order.clone(), u, l));
            }
            self.asked
                .as_ref()
                .is_some_and(|(_, u, l)| !u.iter().chain(l.iter()).all(|k| self.settled(k)))
        };
    }

    /// The one exception to never putting a transfer aside (see the module).
    fn urgent(&mut self, order: Option<Vec<String>>, _sources: &[String]) {
        let p = self.s.opts.parallel.max(1);
        if self.aside.is_some() || self.flights.len() < p || self.disk_wait.is_some() {
            return;
        }
        // Looked at twice a second at most (the newest order's ranking is
        // worked out again only when it changes).
        if self
            .urgent_at
            .is_some_and(|t| t.elapsed() < Duration::from_millis(500))
        {
            return;
        }
        self.urgent_at = Some(Instant::now());
        if order != self.order && self.asked.as_ref().is_none_or(|(o, _, _)| *o != order) {
            let (u, l) = self.ranking(order.as_deref());
            self.asked = Some((order.clone(), u, l));
        }
        let now = Instant::now();
        let top = if order == self.order {
            self.first_open(&self.used, &self.library, now)
        } else {
            let (_, u, l) = self.asked.as_ref().expect("worked out above");
            self.first_open(u, l, now)
        };
        let Some(top) = top else {
            return;
        };
        if self.flights.contains_key(&top) {
            return;
        }
        // The soonest a place frees, at the rates measured.
        let mut soonest = f64::INFINITY;
        for (k, f) in &self.flights {
            if f.stopping {
                return; // a place is freeing already
            }
            let total = self.items.get(k).map_or(0, |i| i.size);
            match self.secs_left(f, total) {
                Some(s) => soonest = soonest.min(s),
                // Too early to tell: wait for a measure.
                None if f.view.started.elapsed() < Duration::from_secs(2) => return,
                None => {}
            }
        }
        if soonest <= self.s.opts.urgent_wait.as_secs_f64() {
            return;
        }
        // Take up this order now, and put aside its lowest ranked transfer.
        if order != self.order {
            self.order = order;
            self.order_at = Some(Instant::now());
            self.reorders += 1;
            self.rank();
        }
        let ranked = self.ranked(&self.used.clone(), &self.library.clone());
        let rank_of = |k: &ObjectKey| ranked.iter().position(|r| r == k).unwrap_or(usize::MAX);
        let victim = self
            .flights
            .iter()
            .filter(|(k, _)| **k != top)
            .filter(|(k, f)| !self.nearly_done(f, self.items.get(*k).map_or(0, |i| i.size)))
            .max_by_key(|(k, _)| (rank_of(k), (*k).clone()))
            .map(|(k, _)| k.clone());
        if let Some(k) = victim
            && let Some(f) = self.flights.get_mut(&k)
        {
            f.stop.store(true, Ordering::Relaxed);
            f.stopping = true;
            self.aside = Some(k);
            self.put_aside += 1;
        }
    }

    /// Seconds a transfer still needs at its measured rate (None: no measure).
    fn secs_left(&self, f: &Flight, total: u64) -> Option<f64> {
        let done = f.view.done.load(Ordering::Relaxed);
        let secs = f.view.started.elapsed().as_secs_f64();
        let got = done.saturating_sub(f.view.start_done) as f64;
        if secs < 1.0 || got <= 0.0 {
            return None;
        }
        Some(total.saturating_sub(done) as f64 / (got / secs))
    }

    fn nearly_done(&self, f: &Flight, total: u64) -> bool {
        let done = f.view.done.load(Ordering::Relaxed);
        if total == 0 || done as f64 >= self.s.opts.nearly_done * total as f64 {
            return true;
        }
        self.secs_left(f, total)
            .is_some_and(|s| s < self.s.opts.nearly_done_secs)
    }

    /// Take in the transfers that have ended. True if any did.
    fn reap(&mut self) -> bool {
        let ended: Vec<ObjectKey> = self
            .flights
            .iter()
            .filter(|(_, f)| f.handle.is_finished())
            .map(|(k, _)| k.clone())
            .collect();
        let now = Instant::now();
        for key in &ended {
            let Some(f) = self.flights.remove(key) else {
                continue;
            };
            if self.aside.as_ref() == Some(key) {
                self.aside = None;
            }
            let done = f.view.done.load(Ordering::Relaxed);
            let result = f.handle.join().unwrap_or_else(|_| {
                Err(FetchError::Disk("the transfer stopped unexpectedly".into()))
            });
            let opts = &self.s.opts;
            let Some(it) = self.items.get_mut(key) else {
                continue;
            };
            it.done = done.min(it.size);
            let backoff = |n: u32| {
                let w = opts
                    .retry_wait
                    .saturating_mul(1u32 << n.saturating_sub(1).min(16));
                now + w.min(opts.retry_max)
            };
            match result {
                Ok(()) => {
                    it.last_error = None;
                    // A file arrived: whatever stopped fetching has passed.
                    if self.problem.as_deref() != Some(NO_PRIMARY) {
                        self.problem = None;
                    }
                    // Replaced files' old versions go once the new one is here.
                    if let Some(olds) = self.plan.as_ref().and_then(|p| p.replaces.get(key)) {
                        for old in olds {
                            self.s.store.remove_object(old);
                        }
                    }
                }
                Err(FetchError::Cancelled) => {}
                Err(FetchError::DiskFull(m)) => {
                    it.last_error = Some(m.clone());
                    self.problem = Some(m);
                    self.disk_wait = Some(now + opts.disk_retry);
                    for f in self.flights.values_mut() {
                        f.stop.store(true, Ordering::Relaxed);
                        f.stopping = true;
                    }
                }
                Err(FetchError::NotFound) => {
                    it.not_found += 1;
                    it.unobtainable = it.not_found >= opts.not_found_after;
                    it.next_try = Some(backoff(it.not_found));
                    it.last_error = Some(FetchError::NotFound.words());
                    // A newer version may say why.
                    self.next_ask = Some(now);
                }
                Err(FetchError::Changed(m)) => {
                    it.failures += 1;
                    it.next_try = Some(backoff(it.failures));
                    it.last_error = Some(m);
                    self.next_ask = Some(now);
                }
                Err(FetchError::Network(m) | FetchError::Disk(m)) => {
                    it.failures += 1;
                    it.next_try = Some(backoff(it.failures));
                    it.last_error = Some(m);
                }
            }
        }
        !ended.is_empty()
    }

    /// Fill the free places from the ranking taken up.
    fn start(&mut self, sources: &[String]) {
        let now = Instant::now();
        if let Some(t) = self.disk_wait {
            if now < t || !self.flights.is_empty() {
                return;
            }
            self.disk_wait = None;
            if self.problem.as_deref().is_some_and(|p| p.contains("disk")) {
                self.problem = None;
            }
        }
        let p = self.s.opts.parallel.max(1);
        if self.flights.len() >= p {
            return;
        }
        let ranked = self.ranked(&self.used.clone(), &self.library.clone());
        for k in ranked {
            if self.flights.len() >= p {
                break;
            }
            if !self.flights.contains_key(&k) {
                self.launch(k, sources);
            }
        }
    }

    fn launch(&mut self, key: ObjectKey, sources: &[String]) {
        let Some(it) = self.items.get(&key) else {
            return;
        };
        let size = it.size;
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicU64::new(it.done));
        let (s, st, dn, k) = (self.s.clone(), stop.clone(), done.clone(), key.clone());
        let sources = sources.to_vec();
        let handle = std::thread::Builder::new()
            .name("media-fetch".into())
            .spawn(move || {
                let n = s.running.fetch_add(1, Ordering::SeqCst) + 1;
                s.most.fetch_max(n, Ordering::SeqCst);
                let s2 = s.clone();
                let _down = crate::crash::OnPanic(move || {
                    s2.running.fetch_sub(1, Ordering::SeqCst);
                });
                let ctx = FetchCtx {
                    store: &s.store,
                    sources: &sources,
                    timeouts: s.opts.timeouts,
                    cap: s.cap.as_ref(),
                    stop: &st,
                    done: &dn,
                    received: &s.received,
                };
                let r = fetch_object(&ctx, &k, size);
                s.running.fetch_sub(1, Ordering::SeqCst);
                r
            });
        let Ok(handle) = handle else {
            return;
        };
        self.started += 1;
        let view = FlightView {
            start_done: done.load(Ordering::Relaxed),
            done,
            started: Instant::now(),
        };
        self.flights.insert(
            key,
            Flight {
                stop,
                view,
                stopping: false,
                handle,
            },
        );
    }

    /// Bring the snapshot and the view up to date (a few times a second, and
    /// whenever a transfer ended).
    fn publish(&mut self, ended: bool) {
        let now = Instant::now();
        let received = self.s.received.load(Ordering::Relaxed);
        self.samples.push_back((now, received));
        while self.samples.len() > 2
            && self
                .samples
                .get(1)
                .is_some_and(|(t, _)| now.duration_since(*t) >= RATE_WINDOW)
        {
            self.samples.pop_front();
        }
        if !ended
            && self
                .last_publish
                .is_some_and(|t| now.duration_since(t) < Duration::from_millis(250))
        {
            // Only the live numbers of the transfers move meanwhile, and the
            // view reads those itself.
            return;
        }
        self.last_publish = Some(now);
        let rate = match (self.samples.front(), self.samples.back()) {
            (Some((t0, b0)), Some((t1, b1)))
                if t1.duration_since(*t0) >= Duration::from_secs(1) =>
            {
                Some((b1 - b0) as f64 / t1.duration_since(*t0).as_secs_f64())
            }
            _ => None,
        };
        let mut snap = PullSnapshot {
            plan: self.plan.clone(),
            problem: self.problem.clone(),
            received,
            started: self.started,
            put_aside: self.put_aside,
            reorders: self.reorders,
            most_at_once: self.s.most.load(Ordering::SeqCst),
            ..PullSnapshot::default()
        };
        let mut prog = Progress {
            manifest_id: self.target.as_ref().map(|t| t.0.id.clone()),
            computers: 1,
            ..Progress::default()
        };
        let mut waiting = HashMap::new();
        let ranked_set: HashSet<&ObjectKey> = self.used.iter().chain(self.library.iter()).collect();
        let mut all_settled = true;
        for (k, it) in &self.items {
            let tier = if it.library {
                &mut prog.library
            } else {
                &mut prog.used
            };
            let path = it.paths.first().cloned().unwrap_or_default();
            if self.here(k) {
                tier.files_wanted += 1;
                tier.files_here += 1;
                tier.bytes_wanted += it.size;
                tier.bytes_here += it.size;
                continue;
            }
            if it.unobtainable {
                tier.files_not_found += 1;
                snap.not_found.push(path);
                waiting.insert(k.clone(), None);
                continue;
            }
            let flight = self.flights.get(k);
            let done = flight
                .map_or(it.done, |f| f.view.done.load(Ordering::Relaxed))
                .min(it.size);
            tier.files_wanted += 1;
            tier.bytes_wanted += it.size;
            tier.bytes_partial += done;
            waiting.insert(k.clone(), Some(done));
            if ranked_set.contains(k) {
                all_settled = false;
            }
            if let Some(f) = flight
                && !f.stopping
            {
                snap.in_flight.push(InFlight {
                    path: path.clone(),
                    done,
                    total: it.size,
                });
            }
            if let Some(e) = &it.last_error {
                snap.failed.push((path, e.clone()));
            }
        }
        if let Some(t) = &self.target {
            for e in t.0.entries.iter().filter(|e| e.missing) {
                let tier = if e.library {
                    &mut prog.library
                } else {
                    &mut prog.used
                };
                tier.files_not_found += 1;
                snap.not_found.push(e.path.clone());
            }
        }
        snap.not_found.sort();
        snap.in_flight.sort_by(|a, b| a.path.cmp(&b.path));
        let left = prog.all().bytes_left();
        prog.rate = rate;
        prog.eta_secs = if left == 0 {
            Some(0.0)
        } else {
            rate.filter(|r| *r > 0.0).map(|r| left as f64 / r)
        };
        snap.progress = prog;
        // Not idle while a newer order waits to be taken up.
        snap.idle =
            self.target.is_some() && all_settled && self.flights.is_empty() && !self.order_pending;
        if snap.idle && !self.tidied {
            self.tidied = true;
            self.s.store.gc();
        }
        {
            let mut v = self.s.view.lock();
            v.target = self.target.clone();
            v.waiting = waiting;
            v.flights = self
                .flights
                .iter()
                .map(|(k, f)| (k.clone(), f.view.clone()))
                .collect();
        }
        *self.s.snapshot.lock() = snap;
    }
}

const NO_PRIMARY: &str = "can't reach the main computer";

/// An order and the files it ranks (used, library).
type Ranking = (Option<Vec<String>>, Vec<ObjectKey>, Vec<ObjectKey>);
