//! The media manager in the app: one per NewQ, ticked from the main loop
//! (`App::pass_net`). It decides from the show network's view what this
//! computer is (alone, the primary, or one following a primary) and runs that
//! side; all its slow work is on threads of its own, and `update` only looks
//! at what they left.

use super::words;
use super::{Finder, Positions, StoreFinder, StoreServes};
use crate::media::ffmpeg::Ffmpeg;
use crate::net::hub::NetView;
use crate::net::node::NodeId;
use crate::net::proto::Msg;
use crate::net::store::local::{Policy, VerifyProgress};
use crate::net::store::manifest::{self, Facts, HashCache, HashError, HashProgress, Sources};
use crate::net::store::pull::{Progress, PullOptions};
use crate::net::store::{
    Follow, Manifest, Primary, Puller, ServeLimits, Server, Store, Transfers, paths, space,
};
use crate::show::{Cue, Show};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How a computer keeps versions, as the primary sets it and the computer
/// keeps it (`media-policy.json` beside its log).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PolicySetting {
    pub keep: usize,
    pub replace_all: bool,
    pub replace_over: Option<u64>,
    /// Where the store is kept, if not `<log folder>/media` (set from the
    /// primary per computer: the hook is here, the control not yet).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<PathBuf>,
}

impl Default for PolicySetting {
    fn default() -> PolicySetting {
        PolicySetting::of(&Policy::default())
    }
}

impl PolicySetting {
    pub fn of(p: &Policy) -> PolicySetting {
        PolicySetting {
            keep: p.keep,
            replace_all: p.replace_all,
            replace_over: p.replace_over,
            folder: None,
        }
    }

    pub fn policy(&self) -> Policy {
        Policy {
            keep: self.keep.max(1),
            replace_all: self.replace_all,
            replace_over: self.replace_over,
            ..Policy::default()
        }
    }

    pub fn words(&self) -> String {
        words::policy(&self.policy())
    }

    fn load(dir: &Path) -> PolicySetting {
        std::fs::read(dir.join(POLICY_FILE))
            .ok()
            .and_then(|t| serde_json::from_slice(&t).ok())
            .unwrap_or_default()
    }

    fn save(&self, dir: &Path) {
        if let Ok(text) = serde_json::to_vec_pretty(self)
            && let Err(e) = manifest::write_atomically(&dir.join(POLICY_FILE), &text)
        {
            log::warn!("media: could not keep the versions setting: {e}");
        }
    }
}

/// A used file a computer hasn't yet, and how much of it is there.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lack {
    pub path: String,
    pub done: u64,
    pub total: u64,
}

/// What a computer tells the primary about its media, in its status report
/// each second.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaReport {
    pub progress: Progress,
    /// `SyncPlan::words` of the version it follows.
    pub plan: String,
    /// Paths no computer has.
    pub not_found: Vec<String>,
    pub policy: PolicySetting,
    /// Where its own store is served (others fetch from it too).
    pub serve_port: Option<u16>,
    /// The used files it hasn't yet (at most `MAX_LACKS`).
    pub lacking: Vec<Lack>,
    /// "Checking 40 %" or what the last check found.
    pub verify: Option<String>,
    pub problem: Option<String>,
    /// Room left on the store's disk.
    pub store_free: Option<u64>,
}

/// At most this many lacking files are reported (the rest still count in the
/// percentages).
const MAX_LACKS: usize = 2000;

const POLICY_FILE: &str = "media-policy.json";
const HASH_FILE: &str = "media-hashes.json";
const FACTS_FILE: &str = "media-facts.json";

/// What the manager is doing on this computer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Side {
    #[default]
    Off,
    Primary,
    Follower,
}

/// A library folder as the Media page shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FolderView {
    pub path: PathBuf,
    pub files: usize,
    pub bytes: u64,
    /// Listed and hashed in the version served now.
    pub listed: bool,
}

/// A computer lacking a used file, for the cue list's mark.
#[derive(Clone, Debug, PartialEq)]
pub struct CueLack {
    pub node: NodeId,
    pub name: String,
    pub percent: f32,
}

/// Everything the UI shows of the media manager, published each pass.
#[derive(Clone, Debug, Default)]
pub struct MediaView {
    pub side: Side,
    /// Primary: hashing under way.
    pub hashing: Option<HashProgress>,
    /// Primary: the version served (files, bytes), and the files it lacks.
    pub served: Option<(usize, u64)>,
    pub missing_here: usize,
    pub folders: Vec<FolderView>,
    pub serving: Transfers,
    /// Primary: every computer's progress together, and how many are busy.
    pub sum: Option<Progress>,
    pub busy: usize,
    /// Follower: its own report.
    pub own: Option<MediaReport>,
    /// Primary: by path key, the computers that lack a used file.
    pub lacking: HashMap<String, Vec<CueLack>>,
    /// Primary: the library's files no cue uses yet (the timeline bin offers them).
    pub library_files: Arc<Vec<PathBuf>>,
    pub problem: Option<String>,
    // ---- Take control (stage 5) ----
    /// A primary that took control: files the show uses that the old primary
    /// had and this computer has neither on its disk nor in its store.
    pub lacking_here: usize,
    /// A primary that took control serves from its store.
    pub from_store: bool,
    /// Primary: the id of the version served (the network test waits for its
    /// computers to have it).
    pub current: Option<String>,
}

impl MediaView {
    /// The status-bar notice (R16), on the primary while anything transfers.
    pub fn notice(&self) -> Option<String> {
        words::notice(self.sum.as_ref()?, self.busy)
    }

    /// Who lacks the file a show path names (primary).
    pub fn lacks(&self, show_path: &Path) -> Option<&Vec<CueLack>> {
        if self.lacking.is_empty() {
            return None;
        }
        self.lacking
            .get(&paths::path_key(&paths::path_text(show_path)))
    }
}

/// A message for one computer of the network.
#[derive(Clone, Debug, PartialEq)]
pub enum Outgoing {
    To(NodeId, Msg),
}

/// How the manager works (tests make it quick and keep it to this computer).
#[derive(Clone, Debug)]
pub struct Config {
    /// Where the hash cache, the facts, the policy and the store are kept.
    pub dir: PathBuf,
    /// Serve on every adapter (else loopback only, for tests).
    pub lan: bool,
    /// Bytes a second the primary sends at most (`NEWQ_MEDIA_RATE`).
    pub serve_rate: Option<u64>,
    pub pull: PullOptions,
    /// How often the primary looks at its files for edits.
    pub watch_every: Duration,
    /// A change to the show's files is acted on once it has rested this long.
    pub settle: Duration,
    /// Library folders are listed again this often (new files in them).
    pub relist_every: Duration,
    /// Put this computer's finder in place for `local_path` (the app; tests
    /// with several managers in one process ask each one's `finder`).
    pub global: bool,
    /// How often the reports, the order and the port are worked out and sent.
    pub every: Duration,
}

impl Config {
    pub fn from_env() -> Config {
        Config {
            dir: crate::net::node::node_dir(),
            lan: true,
            serve_rate: std::env::var("NEWQ_MEDIA_RATE")
                .ok()
                .and_then(|v| v.trim().parse().ok())
                .filter(|v| *v > 0),
            pull: PullOptions::default(),
            watch_every: Duration::from_secs(3),
            settle: Duration::from_millis(800),
            relist_every: Duration::from_secs(30),
            global: true,
            every: Duration::from_millis(1000),
        }
    }
}

/// What `update` is given each pass.
pub struct Frame<'a> {
    pub net: &'a Arc<NetView>,
    pub show: &'a Show,
    /// The show file's folder (for the relative forms of its paths).
    pub show_dir: Option<&'a Path>,
    pub at: Positions,
    /// A cue's length, as the engine knows it.
    pub dur: &'a mut dyn FnMut(&Cue) -> Option<f64>,
    pub ff: Option<&'a Ffmpeg>,
}

/// Drop something whose drop joins a thread, off the caller's thread.
fn drop_later<T: Send + 'static>(t: T) {
    let _ = std::thread::Builder::new()
        .name("media-stop".into())
        .spawn(move || drop(t));
}

/// The media manager.
pub struct Manager {
    cfg: Config,
    side: SideState,
    view: Arc<MediaView>,
    /// The order last received (a follower; kept for the puller to come).
    order: Option<Vec<String>>,
    /// The `NetView` the cue marks were last worked out from.
    seen_net: Option<Arc<NetView>>,
    /// This network has had a backup take control (its term is past the
    /// first): a primary then looks in its store too (stage 5).
    took_control: bool,
}

#[allow(clippy::large_enum_variant)]
enum SideState {
    Off,
    Primary(PrimarySide),
    Follower(FollowerSide),
}

impl Manager {
    pub fn new(cfg: Config) -> Manager {
        Manager {
            cfg,
            side: SideState::Off,
            view: Arc::new(MediaView::default()),
            order: None,
            seen_net: None,
            took_control: false,
        }
    }

    pub fn view(&self) -> Arc<MediaView> {
        self.view.clone()
    }

    /// This computer's report for its status (a follower's; None otherwise).
    pub fn report(&self) -> Option<MediaReport> {
        self.view.own.clone()
    }

    /// The finder of a follower (tests ask it instead of `local_path`).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn finder(&self) -> Option<Arc<StoreFinder>> {
        match &self.side {
            SideState::Follower(f) => Some(f.finder.clone()),
            _ => None,
        }
    }

    /// The finder of a primary that took control (tests).
    #[cfg(test)]
    pub fn primary_finder(&self) -> Option<Arc<super::PrimaryFinder>> {
        match &self.side {
            SideState::Primary(p) => p.finder.clone(),
            _ => None,
        }
    }

    /// A follower's store (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn store(&self) -> Option<Store> {
        match &self.side {
            SideState::Follower(f) => f.store.clone(),
            _ => None,
        }
    }

    /// A follower's puller (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn puller(&self) -> Option<Arc<Puller>> {
        match &self.side {
            SideState::Follower(f) => f.puller.clone(),
            _ => None,
        }
    }

    /// One pass: what side this computer is on, what the threads finished,
    /// what to tell the others. Never waits.
    pub fn update(&mut self, f: Frame) -> Vec<Outgoing> {
        let want = if f.net.is_primary() {
            Side::Primary
        } else if f.net.me.on && f.net.me.adopted() && f.net.following.is_some() {
            Side::Follower
        } else {
            Side::Off
        };
        let have = match &self.side {
            SideState::Off => Side::Off,
            SideState::Primary(_) => Side::Primary,
            SideState::Follower(_) => Side::Follower,
        };
        self.took_control = f.net.me.term >= 2;
        if want != have {
            self.switch(want);
        }
        let net_now = f.net.clone();
        let mut out = Vec::new();
        let mut view = MediaView {
            side: want,
            ..MediaView::default()
        };
        match &mut self.side {
            SideState::Off => {}
            SideState::Primary(p) => p.update(&self.cfg, f, &mut out, &mut view),
            SideState::Follower(fo) => {
                let primary = f.net.following.as_ref().map(|x| x.id);
                fo.update(&self.cfg, primary, &mut view);
            }
        }
        // The cue marks, worked out again only when the reports changed.
        if want == Side::Primary {
            let net = net_now;
            let same = self.seen_net.as_ref().is_some_and(|s| Arc::ptr_eq(s, &net));
            if same {
                view.lacking = self.view.lacking.clone();
                view.sum = self.view.sum.clone();
                view.busy = self.view.busy;
            } else {
                let (lacking, sum, busy) = from_reports(&net);
                view.lacking = lacking;
                view.sum = sum;
                view.busy = busy;
                self.seen_net = Some(net);
            }
        }
        self.view = Arc::new(view);
        out
    }

    /// A message from another computer of the network. True if it was the
    /// media manager's.
    pub fn handle(&mut self, from: NodeId, msg: &Msg, net: &NetView) -> bool {
        match msg {
            Msg::MediaServer { port, sources } => {
                if net.following.as_ref().map(|f| f.id) != Some(from) {
                    return true;
                }
                let Some(addr) = net.peer(from).and_then(|p| p.addr) else {
                    return true;
                };
                let mut list = vec![SocketAddr::new(addr, *port).to_string()];
                list.extend(sources.iter().cloned());
                if let SideState::Follower(f) = &mut self.side {
                    f.sources = Some(list);
                }
                true
            }
            Msg::MediaOrder { paths } => {
                if net.following.as_ref().map(|f| f.id) == Some(from) {
                    self.order = Some(paths.clone());
                    if let SideState::Follower(f) = &mut self.side {
                        if let Some(p) = &f.puller {
                            p.set_order(Some(paths.clone()));
                        }
                        f.order = Some(paths.clone());
                    }
                }
                true
            }
            Msg::MediaPolicy {
                keep,
                replace_all,
                replace_over,
            } => {
                if let SideState::Follower(f) = &mut self.side
                    && net.following.as_ref().map(|x| x.id) == Some(from)
                {
                    f.policy.keep = (*keep).max(1);
                    f.policy.replace_all = *replace_all;
                    f.policy.replace_over = *replace_over;
                    f.policy.save(&self.cfg.dir);
                    if let Some(s) = &f.store {
                        s.set_policy(f.policy.policy());
                    }
                }
                true
            }
            Msg::MediaVerify => {
                if let SideState::Follower(f) = &mut self.side
                    && net.following.as_ref().map(|x| x.id) == Some(from)
                {
                    f.start_verify();
                }
                true
            }
            _ => false,
        }
    }

    fn switch(&mut self, to: Side) {
        let mut old = std::mem::replace(&mut self.side, SideState::Off);
        let had_finder = match &old {
            SideState::Follower(_) => true,
            SideState::Primary(p) => p.finder.is_some(),
            SideState::Off => false,
        };
        if had_finder && self.cfg.global {
            super::set_finder(None);
        }
        // ---- Take control (stage 5) ----
        // A follower that takes control keeps its store: the files it has
        // are served from it and played from it.
        let kept = match &mut old {
            SideState::Follower(f) if to == Side::Primary => f.store.take(),
            _ => None,
        };
        drop_later(old);
        self.seen_net = None;
        self.side = match to {
            Side::Off => SideState::Off,
            Side::Primary => {
                SideState::Primary(PrimarySide::new(&self.cfg, kept, self.took_control))
            }
            Side::Follower => {
                let f = FollowerSide::new(&self.cfg, self.order.clone());
                if self.cfg.global {
                    super::set_finder(Some(f.finder.clone() as Arc<dyn Finder>));
                }
                SideState::Follower(f)
            }
        };
    }
}

impl Drop for Manager {
    fn drop(&mut self) {
        let had_finder = match &self.side {
            SideState::Follower(_) => true,
            SideState::Primary(p) => p.finder.is_some(),
            SideState::Off => false,
        };
        if had_finder && self.cfg.global {
            super::set_finder(None);
        }
    }
}

/// The cue marks and the sum of every computer's progress, from the reports.
fn from_reports(net: &NetView) -> (HashMap<String, Vec<CueLack>>, Option<Progress>, usize) {
    let mut lacking: HashMap<String, Vec<CueLack>> = HashMap::new();
    let mut all = Vec::new();
    let mut busy = 0;
    for p in net.peers.iter().filter(|p| p.ours && p.online) {
        let Some(r) = p.status.as_ref().and_then(|s| s.media_progress.as_deref()) else {
            continue;
        };
        if words::busy(&r.progress) {
            busy += 1;
        }
        all.push(r.progress.clone());
        for l in &r.lacking {
            let percent = if l.total == 0 {
                0.0
            } else {
                (l.done as f64 * 100.0 / l.total as f64).min(99.0) as f32
            };
            lacking
                .entry(paths::path_key(&l.path))
                .or_default()
                .push(CueLack {
                    node: p.id,
                    name: p.name.clone(),
                    percent,
                });
        }
        for nf in &r.not_found {
            lacking
                .entry(paths::path_key(nf))
                .or_default()
                .push(CueLack {
                    node: p.id,
                    name: p.name.clone(),
                    percent: -1.0,
                });
        }
    }
    let sum = (!all.is_empty()).then(|| Progress::sum(&all));
    (lacking, sum, busy)
}

// ---------------------------------------------------------------- primary

/// Digests' companions: what FFprobe read from each file, by digest.
#[derive(Default)]
struct FactsCache {
    file: Option<PathBuf>,
    by_sha: HashMap<String, Facts>,
    changed: bool,
}

impl FactsCache {
    fn open(file: PathBuf) -> FactsCache {
        let by_sha = std::fs::read(&file)
            .ok()
            .and_then(|t| serde_json::from_slice(&t).ok())
            .unwrap_or_default();
        FactsCache {
            file: Some(file),
            by_sha,
            changed: false,
        }
    }

    fn save(&mut self) {
        if !self.changed {
            return;
        }
        if let (Some(f), Ok(text)) = (&self.file, serde_json::to_vec(&self.by_sha)) {
            let _ = manifest::write_atomically(f, &text);
        }
        self.changed = false;
    }
}

/// Making a version: listing, hashing (with the cache), then reading the
/// used files' facts. Dropping it cancels.
struct Making {
    cancel: Arc<AtomicBool>,
    progress: Arc<Mutex<HashProgress>>,
    result: Arc<Mutex<Option<Result<Manifest, String>>>>,
    sig: u64,
}

impl Making {
    fn start(
        src: Sources,
        sig: u64,
        cache: Arc<Mutex<HashCache>>,
        facts: Arc<Mutex<FactsCache>>,
        ff: Option<Ffmpeg>,
    ) -> Making {
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Mutex::new(HashProgress::default()));
        let result = Arc::new(Mutex::new(None));
        let (c, p, r) = (cancel.clone(), progress.clone(), result.clone());
        let spawned = std::thread::Builder::new()
            .name("media-list".into())
            .spawn(move || {
                let r2 = r.clone();
                let _said = crate::crash::OnPanic(move || {
                    *r2.lock() = Some(Err("listing the media stopped unexpectedly".into()));
                });
                let out = manifest::build(&src, &cache, &c, &p)
                    .map_err(|e| match e {
                        HashError::Cancelled => "cancelled".to_string(),
                        HashError::Io(e) => e.to_string(),
                    })
                    .map(|m| add_facts(m, &facts, ff.as_ref(), &c));
                *r.lock() = Some(out);
            });
        if let Err(e) = spawned {
            *result.lock() = Some(Err(e.to_string()));
        }
        Making {
            cancel,
            progress,
            result,
            sig,
        }
    }
}

impl Drop for Making {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The used files' facts, read with FFprobe once per digest.
fn add_facts(
    m: Manifest,
    facts: &Mutex<FactsCache>,
    ff: Option<&Ffmpeg>,
    cancel: &AtomicBool,
) -> Manifest {
    let mut entries = m.entries.clone();
    let mut any = false;
    for e in entries
        .iter_mut()
        .filter(|e| !e.missing && !e.library && !manifest::is_model_part(Path::new(&e.path)))
    {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let known = facts.lock().by_sha.get(&e.sha256).cloned();
        let got = match known {
            Some(f) => Some(f),
            None => ff
                .and_then(|ff| ff.probe(Path::new(&e.path)).ok())
                .map(|info| {
                    let f = Facts::of(&info);
                    let mut c = facts.lock();
                    c.by_sha.insert(e.sha256.clone(), f.clone());
                    c.changed = true;
                    f
                }),
        };
        if got.is_some() {
            any = true;
        }
        e.facts = got;
    }
    facts.lock().save();
    if !any {
        return m;
    }
    Manifest::new(entries)
}

struct PrimarySide {
    primary: Primary,
    server: Option<Server>,
    _watch: crate::net::store::primary::Watch,
    cache: Arc<Mutex<HashCache>>,
    facts: Arc<Mutex<FactsCache>>,
    making: Option<Making>,
    /// The sources' signature of the version served, and when it was made.
    built: Option<u64>,
    built_at: Option<Instant>,
    /// A change seen and when (it is acted on once it rests).
    seen: Option<(u64, Instant)>,
    last_look: Option<Instant>,
    /// The order: its signature, the order, when it was last sent.
    order_sig: Option<(Vec<u64>, Option<String>)>,
    order: Option<Vec<String>>,
    order_due: bool,
    order_sent: Option<Instant>,
    sig_look: Option<Instant>,
    /// Computers that have the order as it is now.
    told: HashSet<NodeId>,
    /// Computers given the port since they connected (never before an order).
    served: HashSet<NodeId>,
    told_at: Option<Instant>,
    order_told_at: Option<Instant>,
    /// The served version's id when its facts were last looked at.
    facts_checked: Option<String>,
    problem: Option<String>,
    summary: (Option<(usize, u64)>, usize, Vec<FolderView>),
    summarised: Option<String>,
    library_files: Arc<Vec<PathBuf>>,
    // ---- Take control (stage 5) ----
    /// The store being opened (a primary that took control, started again).
    store_opening: Option<Opening>,
    /// Its finder, once it has a store (playback finds files there).
    finder: Option<Arc<super::PrimaryFinder>>,
    finder_given: bool,
    lacking_here: usize,
}

impl PrimarySide {
    /// `store`: a follower's that took control, handed over; else, when
    /// `look` (this network has had a takeover), the store beside the log
    /// is opened if there is one.
    fn new(cfg: &Config, store: Option<Store>, look: bool) -> PrimarySide {
        let cache = Arc::new(Mutex::new(HashCache::open(Some(cfg.dir.join(HASH_FILE)))));
        let facts = Arc::new(Mutex::new(FactsCache::open(cfg.dir.join(FACTS_FILE))));
        let primary = Primary::new(cache.clone());
        let bind: SocketAddr = if cfg.lan {
            "0.0.0.0:0".parse().expect("an address")
        } else {
            "127.0.0.1:0".parse().expect("an address")
        };
        let limits = ServeLimits {
            rate_cap: cfg.serve_rate,
            ..ServeLimits::default()
        };
        let (server, problem) = match Server::start(bind, Arc::new(primary.clone()), limits) {
            Ok(s) => (Some(s), None),
            Err(e) => (
                None,
                Some(format!("The media can't be sent to the others: {e}")),
            ),
        };
        let watch = primary.watch(cfg.watch_every);
        let finder = store
            .as_ref()
            .map(|s| Arc::new(super::PrimaryFinder(s.clone())));
        primary.set_store(store.clone());
        let store_opening = if store.is_none() && look {
            let policy = PolicySetting::load(&cfg.dir);
            let root = policy
                .folder
                .clone()
                .unwrap_or_else(|| cfg.dir.join("media"));
            root.join("objects").is_dir().then(|| {
                let p = policy.policy();
                std::thread::Builder::new()
                    .name("media-open".into())
                    .spawn(move || Store::open(&root, p, crate::net::store::space::system()))
                    .ok()
            })
        } else {
            None
        }
        .flatten();
        PrimarySide {
            primary,
            server,
            _watch: watch,
            cache,
            facts,
            making: None,
            built: None,
            built_at: None,
            seen: None,
            last_look: None,
            order_sig: None,
            order: None,
            order_due: false,
            order_sent: None,
            sig_look: None,
            told: HashSet::new(),
            served: HashSet::new(),
            told_at: None,
            order_told_at: None,
            facts_checked: None,
            problem,
            summary: (None, 0, Vec::new()),
            summarised: None,
            library_files: Arc::new(Vec::new()),
            store_opening,
            finder,
            finder_given: false,
            lacking_here: 0,
        }
    }

    fn sources(show: &Show, show_dir: Option<&Path>) -> (Sources, u64) {
        use std::hash::{Hash, Hasher};
        let used = manifest::media_paths(show);
        let src = Sources {
            used,
            library: Vec::new(),
            library_folders: show.library_folders.clone(),
            show_dir: show_dir.map(Path::to_path_buf),
        };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        src.used.hash(&mut h);
        src.library_folders.hash(&mut h);
        src.show_dir.hash(&mut h);
        (src, h.finish())
    }

    fn update(&mut self, cfg: &Config, f: Frame, out: &mut Vec<Outgoing>, view: &mut MediaView) {
        let now = Instant::now();
        // ---- Take control (stage 5): the store, once open, stands in ----
        if let Some(h) = &self.store_opening
            && h.is_finished()
            && let Some(h) = self.store_opening.take()
        {
            match h.join() {
                Ok(Ok(store)) => {
                    self.primary.set_store(Some(store.clone()));
                    self.finder = Some(Arc::new(super::PrimaryFinder(store)));
                    // Made again with the store's files.
                    self.built = None;
                }
                Ok(Err(e)) => log::warn!("media: the store couldn't be opened: {e}"),
                Err(_) => log::warn!("media: opening the store failed"),
            }
        }
        if !self.finder_given
            && let Some(fi) = &self.finder
        {
            self.finder_given = true;
            if cfg.global {
                super::set_finder(Some(fi.clone() as Arc<dyn Finder>));
            }
        }
        // The show's files: looked at about once a second (it walks the cues).
        let look = self
            .last_look
            .is_none_or(|t| now.duration_since(t) >= cfg.every.min(Duration::from_secs(1)));
        if look {
            self.last_look = Some(now);
            let (src, sig) = Self::sources(f.show, f.show_dir);
            let relist = !f.show.library_folders.is_empty()
                && self
                    .built_at
                    .is_some_and(|t| now.duration_since(t) >= cfg.relist_every);
            // A file edited since: its facts are read again.
            let cur_id = self.primary.current_id();
            let lacks_facts = cur_id.is_some()
                && cur_id != self.facts_checked
                && self.primary.current().is_some_and(|m| {
                    m.entries.iter().any(|e| {
                        !e.missing
                            && !e.library
                            && e.facts.is_none()
                            && !manifest::is_model_part(Path::new(&e.path))
                    })
                });
            if cur_id.is_some() && !lacks_facts {
                self.facts_checked = cur_id.clone();
            }
            let changed = self.built != Some(sig) || relist || lacks_facts;
            let making_it = self.making.as_ref().is_some_and(|m| m.sig == sig) && !relist;
            if changed && !making_it {
                match self.seen {
                    Some((s, at)) if s == sig => {
                        if now.duration_since(at) >= cfg.settle
                            || relist
                            || lacks_facts
                            || self.built.is_none()
                        {
                            self.seen = None;
                            if lacks_facts {
                                self.facts_checked = cur_id;
                            }
                            // A newer list replaces one being made.
                            self.making = Some(Making::start(
                                src,
                                sig,
                                self.cache.clone(),
                                self.facts.clone(),
                                f.ff.cloned(),
                            ));
                        }
                    }
                    _ => self.seen = Some((sig, now)),
                }
            } else {
                self.seen = None;
            }
        }
        // A version made: served from now on.
        let made = self
            .making
            .as_ref()
            .and_then(|m| m.result.lock().take().map(|r| (m.sig, r)));
        if let Some((sig, r)) = made {
            self.making = None;
            match r {
                Ok(manifest) => {
                    // What the old primary had and this disk hasn't: from the
                    // store (stage 5).
                    let (manifest, lacking) = self.primary.inherit(manifest);
                    self.lacking_here = lacking;
                    self.primary.publish(manifest);
                    self.built = Some(sig);
                    self.built_at = Some(now);
                    self.order_due = true;
                    self.problem = None;
                }
                Err(e) if e == "cancelled" => {}
                Err(e) => {
                    self.problem = Some(format!("The list of media couldn't be made: {e}"));
                    self.built = Some(sig);
                    self.built_at = Some(now);
                }
            }
        }
        // A changed file hashed again by the watch: a new version served.
        let cur_id = self.primary.current_id();
        if cur_id != self.summarised
            && let Some(cur) = self.primary.current()
        {
            self.summary = summarise(&cur, &f.show.library_folders);
            self.library_files = Arc::new(
                cur.present()
                    .filter(|e| e.library)
                    .map(|e| PathBuf::from(&e.path))
                    .collect(),
            );
            self.summarised = cur_id;
            self.order_due = true;
        }
        self.update_order(cfg, f.show, &f.at, f.dur, now);
        self.tell(cfg, f.net, now, out);
        view.hashing = self.making.as_ref().map(|m| m.progress.lock().clone());
        view.served = self.summary.0;
        view.current = self.primary.current_id();
        view.library_files = self.library_files.clone();
        view.missing_here = self.summary.1;
        view.folders = f
            .show
            .library_folders
            .iter()
            .map(|p| {
                self.summary
                    .2
                    .iter()
                    .find(|v| &v.path == p)
                    .cloned()
                    .unwrap_or(FolderView {
                        path: p.clone(),
                        ..FolderView::default()
                    })
            })
            .collect();
        view.serving = self
            .server
            .as_ref()
            .map(Server::transfers)
            .unwrap_or_default();
        view.problem = self.problem.clone();
        view.lacking_here = self.lacking_here;
        view.from_store = self.finder.is_some();
    }

    /// Work the order out again when the playheads or the files moved, and
    /// send it at most about once a second.
    fn update_order(
        &mut self,
        cfg: &Config,
        show: &Show,
        at: &Positions,
        dur: &mut dyn FnMut(&Cue) -> Option<f64>,
        now: Instant,
    ) {
        let look = self
            .sig_look
            .is_none_or(|t| now.duration_since(t) >= Duration::from_millis(250));
        if !look && !self.order_due {
            return;
        }
        self.sig_look = Some(now);
        let sig = super::order::signature(show, at, dur);
        let id = self.primary.current_id();
        let key = (sig, id);
        if self.order_sig.as_ref() != Some(&key) || self.order_due {
            let order = super::order(show, at, dur);
            self.order_sig = Some(key);
            self.order_due = false;
            if self.order.as_ref() != Some(&order) {
                self.order = Some(order);
                // Everyone is told again (below), at most once a second.
                self.told.clear();
            }
        }
        let _ = cfg;
    }

    /// The port, the other sources and the order, to the computers that
    /// haven't them (a new one at once, the rest after a change), at most
    /// about once a second; the port every few seconds whatever happens.
    fn tell(&mut self, cfg: &Config, net: &NetView, now: Instant, out: &mut Vec<Outgoing>) {
        let Some(port) = self.server.as_ref().map(|s| s.port) else {
            return;
        };
        let ours: Vec<&crate::net::hub::PeerView> =
            net.peers.iter().filter(|p| p.ours && p.connected).collect();
        self.told.retain(|id| ours.iter().any(|p| p.id == *id));
        let again = self
            .told_at
            .is_none_or(|t| now.duration_since(t) >= cfg.every.max(Duration::from_millis(200)) * 3);
        let soon = self
            .order_sent
            .is_none_or(|t| now.duration_since(t) >= cfg.every);
        let cur = self.primary.current_id();
        // Computers that have all the used files of this version, as sources.
        let full: Vec<(NodeId, String)> = ours
            .iter()
            .filter_map(|p| {
                let r = p.status.as_ref()?.media_progress.as_deref()?;
                let addr = p.addr?;
                let port = r.serve_port?;
                (r.progress.manifest_id == cur && cur.is_some() && r.progress.used.complete())
                    .then(|| (p.id, SocketAddr::new(addr, port).to_string()))
            })
            .collect();
        // The order again to everyone now and then (a computer that started
        // afresh), or at once to one not told since it changed.
        let order_again = self.order_told_at.is_none_or(|t| {
            now.duration_since(t) >= cfg.every.max(Duration::from_millis(200)) * 10
        });
        self.served.retain(|id| ours.iter().any(|p| p.id == *id));
        for p in &ours {
            let (order, server) = sends(Sends {
                told: self.told.contains(&p.id),
                served: self.served.contains(&p.id),
                has_order: self.order.is_some(),
                soon,
                again,
                order_again,
            });
            // The order ahead of the port: a follower starts fetching when it
            // hears of the port, and may read the two in separate passes.
            if order && let Some(o) = &self.order {
                out.push(Outgoing::To(p.id, Msg::MediaOrder { paths: o.clone() }));
                self.told.insert(p.id);
                self.order_sent = Some(now);
            }
            if server {
                let sources: Vec<String> = full
                    .iter()
                    .filter(|(id, _)| *id != p.id)
                    .map(|(_, a)| a.clone())
                    .collect();
                out.push(Outgoing::To(p.id, Msg::MediaServer { port, sources }));
                self.served.insert(p.id);
            }
        }
        if order_again {
            self.order_told_at = Some(now);
        }
        if again {
            self.told_at = Some(now);
        }
    }
}

/// What the primary knows of one computer at a pass (`PrimarySide::tell`).
#[derive(Clone, Copy, Debug)]
pub(super) struct Sends {
    /// It has the order as it is now.
    pub told: bool,
    /// It has been given the port since it connected.
    pub served: bool,
    pub has_order: bool,
    /// The order may go (none went in the last `every`).
    pub soon: bool,
    /// The port goes to everyone again (every few seconds).
    pub again: bool,
    /// The order goes to everyone again.
    pub order_again: bool,
}

/// Whether the order and the port go to a computer this pass. A computer
/// never hears of the port before it has an order (it would fetch in the
/// manifest's order meanwhile): one never given the port gets the order at
/// once, ahead of it; one whose order changed gets the order at most about
/// once a second (`soon`) and the port only every few seconds (`again`).
pub(super) fn sends(s: Sends) -> (bool, bool) {
    let first = !s.served;
    let order = s.has_order && ((!s.told && (s.soon || first)) || s.order_again);
    let told = s.told || order;
    let server = told && (s.again || (first || (!s.told && s.soon)));
    (order, server)
}

/// The version's numbers for the pages: files and bytes, the files missing
/// here, and each library folder's.
fn summarise(m: &Manifest, folders: &[PathBuf]) -> (Option<(usize, u64)>, usize, Vec<FolderView>) {
    let present: Vec<_> = m.present().collect();
    let total = (present.len(), present.iter().map(|e| e.size).sum());
    let missing = m.entries.iter().filter(|e| e.missing && !e.library).count();
    let views = folders
        .iter()
        .map(|dir| {
            let prefix = paths::path_key(&paths::path_text(dir));
            let inside: Vec<_> = present
                .iter()
                .filter(|e| {
                    let k = e.key();
                    k.len() > prefix.len()
                        && k.starts_with(&prefix)
                        && matches!(k.as_bytes()[prefix.len()], b'/' | b'\\')
                })
                .collect();
            FolderView {
                path: dir.clone(),
                files: inside.len(),
                bytes: inside.iter().map(|e| e.size).sum(),
                listed: true,
            }
        })
        .collect();
    (Some(total), missing, views)
}

// --------------------------------------------------------------- follower

type Opening = std::thread::JoinHandle<std::io::Result<Store>>;

struct Verifying {
    cancel: Arc<AtomicBool>,
    progress: Arc<Mutex<VerifyProgress>>,
    result: Arc<Mutex<Option<String>>>,
}

struct FollowerSide {
    finder: Arc<StoreFinder>,
    opening: Option<Opening>,
    store: Option<Store>,
    puller: Option<Arc<Puller>>,
    server: Option<Server>,
    /// The primary's server and the other computers that have the files.
    sources: Option<Vec<String>>,
    sources_given: Option<Vec<String>>,
    order: Option<Vec<String>>,
    policy: PolicySetting,
    verifying: Option<Verifying>,
    verified: Option<String>,
    problem: Option<String>,
    last_report: Option<Instant>,
    report: Option<MediaReport>,
    primary: Option<NodeId>,
}

impl FollowerSide {
    fn new(cfg: &Config, order: Option<Vec<String>>) -> FollowerSide {
        let policy = PolicySetting::load(&cfg.dir);
        let root = policy
            .folder
            .clone()
            .unwrap_or_else(|| cfg.dir.join("media"));
        let p = policy.policy();
        let opening = std::thread::Builder::new()
            .name("media-open".into())
            .spawn(move || Store::open(&root, p, crate::net::store::space::system()))
            .ok();
        FollowerSide {
            finder: Arc::new(StoreFinder::default()),
            opening,
            store: None,
            puller: None,
            server: None,
            sources: None,
            sources_given: None,
            order,
            policy,
            verifying: None,
            verified: None,
            problem: None,
            last_report: None,
            report: None,
            primary: None,
        }
    }

    fn start_verify(&mut self) {
        let Some(store) = self.store.clone() else {
            return;
        };
        if self.verifying.is_some() {
            return;
        }
        let v = Verifying {
            cancel: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(Mutex::new(VerifyProgress::default())),
            result: Arc::new(Mutex::new(None)),
        };
        let (c, p, r) = (v.cancel.clone(), v.progress.clone(), v.result.clone());
        let puller = self.puller.clone();
        let spawned = std::thread::Builder::new()
            .name("media-verify".into())
            .spawn(move || {
                let r2 = r.clone();
                let _said = crate::crash::OnPanic(move || {
                    *r2.lock() = Some("The check stopped unexpectedly".into());
                });
                let rep = store.verify(&c, &p);
                let text = if rep.cancelled {
                    "The check was stopped".to_string()
                } else if rep.bad.is_empty() {
                    format!("Checked {} files: all sound", rep.checked)
                } else {
                    format!(
                        "Checked {} files: {} damaged, removed and being copied again",
                        rep.checked,
                        rep.bad.len()
                    )
                };
                *r.lock() = Some(text);
                if let Some(p) = puller {
                    p.check_now();
                }
            });
        if spawned.is_ok() {
            self.verifying = Some(v);
        }
    }

    fn update(&mut self, cfg: &Config, primary: Option<NodeId>, view: &mut MediaView) {
        let now = Instant::now();
        if self.primary != primary {
            // Another primary: what the old one said goes.
            self.primary = primary;
            self.sources = None;
        }
        if let Some(h) = &self.opening
            && h.is_finished()
            && let Some(h) = self.opening.take()
        {
            match h.join() {
                Ok(Ok(store)) => {
                    let bind: SocketAddr = if cfg.lan {
                        "0.0.0.0:0".parse().expect("an address")
                    } else {
                        "127.0.0.1:0".parse().expect("an address")
                    };
                    self.server = Server::start(
                        bind,
                        Arc::new(StoreServes(store.clone())),
                        ServeLimits::default(),
                    )
                    .ok();
                    self.store = Some(store);
                }
                Ok(Err(e)) => {
                    self.problem = Some(format!("The media store couldn't be opened: {e}"))
                }
                Err(_) => self.problem = Some("Opening the media store failed".into()),
            }
        }
        // The primary's server heard of: fetching starts (or its sources move).
        if let (Some(store), Some(src)) = (&self.store, &self.sources)
            && self.sources_given.as_ref() != Some(src)
        {
            match &self.puller {
                Some(p) => p.set_sources(src.clone()),
                None => {
                    // The order known so far goes in with it: set after the
                    // thread starts, the first file could be the version's
                    // first rather than the playhead's (a busy machine).
                    let p = Arc::new(Puller::start_ordered(
                        store.clone(),
                        src.clone(),
                        Follow::Current,
                        self.order.clone(),
                        cfg.pull.clone(),
                    ));
                    self.finder.set_puller(Some(p.clone()));
                    self.puller = Some(p);
                }
            }
            self.sources_given = Some(src.clone());
        }
        let done = self.verifying.as_ref().and_then(|v| v.result.lock().take());
        if let Some(done) = done {
            self.verified = Some(done);
            self.verifying = None;
        }
        let due = self
            .last_report
            .is_none_or(|t| now.duration_since(t) >= cfg.every.min(Duration::from_secs(1)));
        if due {
            self.last_report = Some(now);
            self.report = Some(self.make_report());
        }
        view.own = self.report.clone();
        view.problem = self.problem.clone();
    }

    fn make_report(&self) -> MediaReport {
        let mut r = MediaReport {
            policy: self.policy.clone(),
            serve_port: self.server.as_ref().map(|s| s.port),
            problem: self.problem.clone(),
            ..MediaReport::default()
        };
        r.verify = match (&self.verifying, &self.verified) {
            (Some(v), _) => {
                let p = v.progress.lock().clone();
                let pct = if p.bytes_total == 0 {
                    0.0
                } else {
                    p.bytes_done as f64 * 100.0 / p.bytes_total as f64
                };
                Some(format!("Checking every file · {pct:.0} %"))
            }
            (None, Some(t)) => Some(t.clone()),
            _ => None,
        };
        if let Some(store) = &self.store {
            r.store_free = store.free_space();
        }
        let Some(p) = &self.puller else {
            if self.store.is_some() && r.problem.is_none() {
                r.plan = "Waiting for the primary".into();
            }
            return r;
        };
        let snap = p.snapshot();
        r.progress = snap.progress.clone();
        // (The plan is worked out when a version is taken up: once all is
        // here it would still say what was to copy.)
        r.plan = match &snap.plan {
            Some(_) if !words::busy(&snap.progress) => "All the media is here.".into(),
            Some(pl) => pl.words(),
            None => "Waiting for the primary's list of media".into(),
        };
        r.not_found = snap.not_found.clone();
        if r.problem.is_none() {
            r.problem = snap.problem.clone();
        }
        if let Some(m) = p.store().current() {
            for e in m.entries.iter().filter(|e| !e.library && !e.missing) {
                if r.lacking.len() >= MAX_LACKS {
                    break;
                }
                match p.availability(&e.path) {
                    crate::net::store::Availability::Here
                    | crate::net::store::Availability::NotFound
                    | crate::net::store::Availability::NotInShow => {}
                    crate::net::store::Availability::Transferring { done, total } => {
                        r.lacking.push(Lack {
                            path: e.path.clone(),
                            done,
                            total,
                        })
                    }
                }
            }
        }
        r
    }
}

impl Drop for FollowerSide {
    fn drop(&mut self) {
        if let Some(v) = &self.verifying {
            v.cancel.store(true, Ordering::Relaxed);
        }
        self.finder.set_puller(None);
    }
}

/// Bytes, for the pages.
pub fn size_words(bytes: u64) -> String {
    space::words(bytes)
}
