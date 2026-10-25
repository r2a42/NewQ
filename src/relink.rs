//! Finding a show's media again after it has moved, and gathering it in one place.
//!
//! A show remembers where each media file was. Moved to another machine, or
//! with the media in a different folder, those places are gone. These
//! functions list what is missing, find it again by name in a folder you point
//! at, and copy a show's media into one folder so the show can travel.

use crate::show::{CueId, Show};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// How deep into a folder the search goes.
const MAX_DEPTH: usize = 12;

/// Every cue whose media file isn't there, with where it was expected.
pub fn missing(show: &Show) -> Vec<(CueId, PathBuf)> {
    show.cues
        .iter()
        .filter_map(|c| {
            let file = &c.kind.file_media()?.file;
            (!file.as_os_str().is_empty() && !file.is_file()).then(|| (c.id, file.clone()))
        })
        .collect()
}

/// How many trailing path components two paths share (the file name counts).
fn shared_tail(a: &Path, b: &Path) -> usize {
    let parts = |p: &Path| -> Vec<OsString> {
        p.components()
            .filter_map(|c| match c {
                Component::Normal(n) => Some(n.to_os_string()),
                _ => None,
            })
            .collect()
    };
    let (a, b) = (parts(a), parts(b));
    a.iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| x == y)
        .count()
}

/// All the files under `root`, as paths, in a stable order, and whether it
/// stopped for lack of time (`deadline`) before it had looked everywhere. It
/// also stops, early, if `cancel` is set. A folder reached twice (a link back
/// up the tree) is read once.
fn walk(root: &Path, deadline: Instant, cancel: &AtomicBool) -> (Vec<PathBuf>, bool) {
    let mut files = Vec::new();
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut timed_out = false;
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            break;
        }
        let key = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        if !visited.insert(key) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            // `metadata` follows links, which is what a shared media drive usually is.
            match std::fs::metadata(&path) {
                Ok(m) if m.is_file() => files.push(path),
                Ok(m) if m.is_dir() && depth < MAX_DEPTH => stack.push((path, depth + 1)),
                _ => {}
            }
        }
    }
    files.sort();
    (files, timed_out)
}

/// Look for the `wanted` files by name under `root`, for at most `budget`.
/// Where a name turns up more than once, the one whose folders match the
/// original path best wins. Returns what was found, by the path it was wanted
/// at, and whether it ran out of time.
pub fn search_within(
    root: &Path,
    wanted: &[PathBuf],
    budget: Duration,
    cancel: &AtomicBool,
) -> (HashMap<PathBuf, PathBuf>, bool) {
    let (files, timed_out) = walk(root, Instant::now() + budget, cancel);
    let mut by_name: HashMap<OsString, Vec<PathBuf>> = HashMap::new();
    for f in files {
        if let Some(name) = f.file_name() {
            by_name.entry(name.to_os_string()).or_default().push(f);
        }
    }
    let mut found = HashMap::new();
    for want in wanted {
        let Some(candidates) = want.file_name().and_then(|n| by_name.get(n)) else {
            continue;
        };
        if let Some(best) = rank(want, candidates).into_iter().next() {
            found.insert(want.clone(), best);
        }
    }
    (found, timed_out)
}

/// What a search of one folder came to.
#[derive(Debug, Default)]
pub struct FolderFound {
    /// The files found, by the path they were wanted at.
    pub found: HashMap<PathBuf, PathBuf>,
    /// It ran out of time before it had looked everywhere.
    pub timed_out: bool,
    /// It was stopped.
    pub stopped: bool,
}

/// `search_within` on its own thread, so a big folder (or a whole disk) can't
/// freeze the show while it is read.
pub struct FolderSearch {
    done: std::sync::mpsc::Receiver<FolderFound>,
    cancel: Arc<AtomicBool>,
    pub root: PathBuf,
    /// How many files it is looking for.
    pub wanted: usize,
}

impl FolderSearch {
    pub fn start(root: PathBuf, wanted: Vec<PathBuf>, budget: Duration) -> FolderSearch {
        let (tx, done) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (r, c, n) = (root.clone(), cancel.clone(), wanted.len());
        let _ = std::thread::Builder::new()
            .name("folder-search".into())
            .spawn(move || {
                let (found, timed_out) = search_within(&r, &wanted, budget, &c);
                let stopped = c.load(Ordering::Relaxed);
                let _ = tx.send(FolderFound {
                    found,
                    timed_out,
                    stopped,
                });
            });
        FolderSearch {
            done,
            cancel,
            root,
            wanted: n,
        }
    }

    /// The result, once there is one. (A search whose thread died without
    /// one reads as having found nothing.)
    pub fn poll(&self) -> Option<FolderFound> {
        match self.done.try_recv() {
            Ok(f) => Some(f),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(FolderFound::default()),
        }
    }

    pub fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for FolderSearch {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The files that could stand in for `want`, best first: the one whose folders
/// match the original path most, then the shallowest, then in path order.
pub fn rank(want: &Path, candidates: &[PathBuf]) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = candidates.to_vec();
    v.sort();
    v.dedup();
    v.sort_by_key(|c| {
        (
            std::cmp::Reverse(shared_tail(want, c)),
            c.components().count(),
        )
    });
    v
}

// ----- Searching the whole computer ------------------------------------------

/// How long a search of the computer may run before it gives up.
pub const SEARCH_BUDGET: Duration = Duration::from_secs(90);
const SEARCH_DEPTH: usize = 16;

/// Where to look for lost media, most likely places first: beside the show,
/// the usual media folders, then the rest of the home folder and any other
/// disks that are mounted.
pub fn default_roots(show_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(d) = show_dir {
        roots.push(d.to_path_buf());
        if let Some(p) = d.parent() {
            roots.push(p.to_path_buf());
        }
    }
    if let Some(home) = home_dir() {
        for sub in [
            "Movies",
            "Videos",
            "Music",
            "Pictures",
            "Documents",
            "Desktop",
            "Downloads",
        ] {
            roots.push(home.join(sub));
        }
        roots.push(home);
    }
    // Other disks: USB and external drives, network mounts.
    for parent in ["/Volumes", "/media", "/mnt", "/run/media"] {
        if let Ok(entries) = std::fs::read_dir(parent) {
            let mut mounts: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
            mounts.sort();
            roots.extend(mounts);
        }
    }
    if cfg!(windows) {
        for letter in b'D'..=b'Z' {
            roots.push(PathBuf::from(format!("{}:\\", letter as char)));
        }
    }
    let mut seen = HashSet::new();
    roots.retain(|r| r.is_dir() && seen.insert(r.clone()));
    roots
}

fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
}

/// Folders that are never worth searching for media: hidden ones, system
/// ones, caches and application data.
fn skip_dir(name: &str, in_home: bool) -> bool {
    let n = name.to_lowercase();
    n.starts_with('.')
        || matches!(
            n.as_str(),
            "node_modules"
                | "appdata"
                | "$recycle.bin"
                | "system volume information"
                | "windows"
                | "program files"
                | "program files (x86)"
                | "programdata"
                | "proc"
                | "sys"
                | "dev"
                | "__pycache__"
        )
        // On a Mac the home folder's Library is caches and app data.
        || (in_home && n == "library")
}

/// What a running search has found so far.
#[derive(Clone, Debug, Default)]
pub struct SearchState {
    /// Files with a wanted name, by that name.
    pub hits: HashMap<OsString, Vec<PathBuf>>,
    /// Folders looked through.
    pub folders: usize,
    pub done: bool,
    /// It stopped because it ran out of time, not because it had looked everywhere.
    pub timed_out: bool,
}

/// A search of the computer for files by name, on its own thread. Results
/// come in as it goes, so suggestions can appear while it is still looking.
pub struct Search {
    state: Arc<Mutex<SearchState>>,
    cancel: Arc<AtomicBool>,
}

impl Search {
    pub fn start(roots: Vec<PathBuf>, wanted: HashSet<OsString>, budget: Duration) -> Search {
        let state = Arc::new(Mutex::new(SearchState::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let (s, c) = (state.clone(), cancel.clone());
        if std::thread::Builder::new()
            .name("media-search".into())
            .spawn(move || scan(&roots, &wanted, budget, &c, &s))
            .is_err()
        {
            state.lock().done = true;
        }
        Search { state, cancel }
    }

    pub fn state(&self) -> SearchState {
        self.state.lock().clone()
    }

    pub fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for Search {
    fn drop(&mut self) {
        self.stop();
    }
}

fn scan(
    roots: &[PathBuf],
    wanted: &HashSet<OsString>,
    budget: Duration,
    cancel: &AtomicBool,
    state: &Mutex<SearchState>,
) {
    let deadline = Instant::now() + budget;
    let home = home_dir();
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut timed_out = false;
    'roots: for root in roots {
        let mut stack = vec![(root.clone(), 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            if cancel.load(Ordering::Relaxed) {
                break 'roots;
            }
            if Instant::now() >= deadline {
                timed_out = true;
                break 'roots;
            }
            // A folder reached twice (a root inside another, a link back up) is read once.
            let key = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
            if !visited.insert(key) {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let in_home = home.as_deref() == Some(dir.as_path());
            let mut hits: Vec<(OsString, PathBuf)> = Vec::new();
            for e in entries.flatten() {
                let name = e.file_name();
                let Ok(kind) = e.file_type() else { continue };
                // A link might be a folder or a file: look through it.
                let (is_dir, is_file) = if kind.is_symlink() {
                    std::fs::metadata(e.path())
                        .map(|m| (m.is_dir(), m.is_file()))
                        .unwrap_or((false, false))
                } else {
                    (kind.is_dir(), kind.is_file())
                };
                if is_file && wanted.contains(&name) {
                    hits.push((name, e.path()));
                } else if is_dir
                    && depth < SEARCH_DEPTH
                    && !skip_dir(&name.to_string_lossy(), in_home)
                {
                    stack.push((e.path(), depth + 1));
                }
            }
            let mut st = state.lock();
            st.folders += 1;
            for (name, path) in hits {
                st.hits.entry(name).or_default().push(path);
            }
        }
    }
    let mut st = state.lock();
    st.done = true;
    st.timed_out = timed_out;
}

/// For each wanted path, the files found that could replace it, best first.
pub fn suggestions(
    hits: &HashMap<OsString, Vec<PathBuf>>,
    wanted: &[PathBuf],
) -> HashMap<PathBuf, Vec<PathBuf>> {
    wanted
        .iter()
        .filter_map(|want| {
            let found = hits.get(want.file_name()?)?;
            Some((want.clone(), rank(want, found)))
        })
        .collect()
}

/// Point every cue that used a file at its new place. Returns how many cues changed.
pub fn apply(show: &mut Show, moved: &HashMap<PathBuf, PathBuf>) -> usize {
    let mut n = 0;
    for cue in &mut show.cues {
        if let Some(m) = cue.kind.media_mut()
            && let Some(new) = moved.get(&m.file)
        {
            m.file = new.clone();
            n += 1;
        }
    }
    n
}

/// What copying a show's media together did.
#[derive(Debug, Default, PartialEq)]
pub struct Collected {
    /// Old path → where the file is now (in the folder).
    pub moved: HashMap<PathBuf, PathBuf>,
    pub copied: usize,
    /// Files that were already in the folder.
    pub already_there: usize,
    /// Files that were missing, or couldn't be copied, with the reason.
    pub failed: Vec<(PathBuf, String)>,
}

/// Whether two paths are the same file on disk.
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Where each file goes in `dest`: its own name, or `name_2.ext`, `name_3.ext`…
/// when another file has taken it. The same source always gets the same place.
pub fn plan_collect(dest: &Path, files: &[PathBuf]) -> Vec<(PathBuf, PathBuf)> {
    let mut taken: HashMap<PathBuf, PathBuf> = HashMap::new(); // target → source
    let mut plan: Vec<(PathBuf, PathBuf)> = Vec::new();
    for src in files {
        if plan.iter().any(|(s, _)| s == src) {
            continue;
        }
        let name = src.file_name().map(PathBuf::from).unwrap_or_default();
        let stem = name
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let ext = name
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        let mut n = 1;
        let target = loop {
            let candidate = if n == 1 {
                dest.join(&name)
            } else {
                dest.join(format!("{stem}_{n}{ext}"))
            };
            match taken.get(&candidate) {
                // A file that is already in the folder keeps its place.
                None if !candidate.exists() || same_file(&candidate, src) => break candidate,
                // Something else is there already: leave it alone.
                None => n += 1,
                Some(owner) if owner == src => break candidate,
                Some(_) => n += 1,
            }
        };
        taken.insert(target.clone(), src.clone());
        plan.push((src.clone(), target));
    }
    plan
}

/// Copy the media of the cues in `files` into `dest` (made if needed).
/// `progress` counts files done, for a progress bar.
pub fn collect(dest: &Path, files: &[PathBuf], progress: &Arc<AtomicUsize>) -> Collected {
    let mut out = Collected::default();
    if let Err(e) = std::fs::create_dir_all(dest) {
        out.failed
            .extend(files.iter().map(|f| (f.clone(), format!("{e}"))));
        return out;
    }
    let dest_real = std::fs::canonicalize(dest).unwrap_or_else(|_| dest.to_path_buf());
    let mut unique: Vec<PathBuf> = Vec::new();
    for f in files {
        if !unique.contains(f) {
            unique.push(f.clone());
        }
    }
    // A file already in the folder stays where it is; the others are placed around it.
    let (inside, outside): (Vec<_>, Vec<_>) = unique.iter().cloned().partition(|f| {
        std::fs::canonicalize(f).is_ok_and(|c| c.parent() == Some(dest_real.as_path()))
    });
    for _ in &inside {
        out.already_there += 1;
        progress.fetch_add(1, Ordering::Relaxed);
    }
    let mut all = inside.clone();
    all.extend(outside.iter().cloned());
    for (src, target) in plan_collect(dest, &all) {
        if inside.contains(&src) {
            continue;
        }
        if !src.is_file() {
            out.failed.push((src, "the file is missing".into()));
        } else {
            match std::fs::copy(&src, &target) {
                Ok(_) => {
                    out.copied += 1;
                    out.moved.insert(src, target);
                }
                Err(e) => out.failed.push((src, e.to_string())),
            }
        }
        progress.fetch_add(1, Ordering::Relaxed);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::show::{CueKind, CueType};

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("newq_relink_{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// `search_within`, with all the time it wants.
    fn search_all(root: &Path, wanted: &[PathBuf]) -> HashMap<PathBuf, PathBuf> {
        search_within(
            root,
            wanted,
            Duration::from_secs(60),
            &AtomicBool::new(false),
        )
        .0
    }

    fn touch(p: &Path, content: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    fn show_with(files: &[PathBuf]) -> Show {
        let mut show = Show::default();
        for f in files {
            let c = show.make_cue(CueType::Video, Some(f.clone()), show.cues.len());
            show.cues.push(c);
        }
        let n = show.make_cue(CueType::Wait, None, show.cues.len());
        show.cues.push(n);
        show
    }

    fn file_of(show: &Show, i: usize) -> PathBuf {
        show.cues[i].kind.file_media().unwrap().file.clone()
    }

    #[test]
    fn lists_the_cues_whose_files_are_gone() {
        let d = dir("missing");
        touch(&d.join("here.mp4"), "x");
        let show = show_with(&[d.join("here.mp4"), d.join("gone.mp4")]);
        let m = missing(&show);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].0, show.cues[1].id);
        assert_eq!(m[0].1, d.join("gone.mp4"));
        // A cue with no file yet isn't "missing"; it just isn't set up.
        let mut show = show;
        let blank = show.make_cue(CueType::Video, None, 0);
        show.cues.push(blank);
        assert_eq!(missing(&show).len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn finds_files_by_name_in_a_new_place() {
        let d = dir("search");
        touch(&d.join("new/media/act1/intro.mp4"), "x");
        touch(&d.join("new/media/act2/outro.mp4"), "x");
        touch(&d.join("new/media/tone.wav"), "x");
        let wanted = vec![
            PathBuf::from("/old/show/act1/intro.mp4"),
            PathBuf::from("/old/show/act2/outro.mp4"),
            PathBuf::from("/old/tone.wav"),
            PathBuf::from("/old/nowhere.mp4"),
        ];
        let found = search_all(&d.join("new"), &wanted);
        assert_eq!(found.len(), 3);
        assert_eq!(found[&wanted[0]], d.join("new/media/act1/intro.mp4"));
        assert_eq!(found[&wanted[1]], d.join("new/media/act2/outro.mp4"));
        assert_eq!(found[&wanted[2]], d.join("new/media/tone.wav"));
        assert!(!found.contains_key(&wanted[3]), "not found stays not found");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_folder_search_stops_at_its_time_limit_and_when_told() {
        let d = dir("budget");
        touch(&d.join("a/b/clip.mp4"), "x");
        let want = vec![PathBuf::from("/old/clip.mp4")];
        // No time at all: it gives up at once and says so.
        let (found, timed_out) = search_within(&d, &want, Duration::ZERO, &AtomicBool::new(false));
        assert!(timed_out && found.is_empty());
        // Told to stop: it stops, and that isn't running out of time.
        let stop = AtomicBool::new(true);
        let (found, timed_out) = search_within(&d, &want, Duration::from_secs(60), &stop);
        assert!(!timed_out && found.is_empty());
        // With time and no stop it finds the file.
        let (found, timed_out) =
            search_within(&d, &want, Duration::from_secs(60), &AtomicBool::new(false));
        assert!(!timed_out);
        assert_eq!(found[&want[0]], d.join("a/b/clip.mp4"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_back_up_the_tree_is_not_followed_round_and_round() {
        let d = dir("loop");
        touch(&d.join("a/clip.mp4"), "x");
        std::os::unix::fs::symlink(&d, d.join("a/back")).unwrap();
        let (files, timed_out) = walk(
            &d,
            Instant::now() + Duration::from_secs(60),
            &AtomicBool::new(false),
        );
        assert!(!timed_out);
        assert_eq!(
            files,
            vec![d.join("a/clip.mp4")],
            "the file once, not again through the link at every level"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_folder_search_runs_on_its_own_thread_and_reports_back() {
        let d = dir("threaded");
        touch(&d.join("media/clip.mp4"), "x");
        let want = vec![
            PathBuf::from("/old/clip.mp4"),
            PathBuf::from("/old/gone.mp4"),
        ];
        let s = FolderSearch::start(d.clone(), want.clone(), Duration::from_secs(60));
        assert_eq!((s.root.as_path(), s.wanted), (d.as_path(), 2));
        let t0 = Instant::now();
        let result = loop {
            if let Some(r) = s.poll() {
                break r;
            }
            assert!(t0.elapsed() < Duration::from_secs(10), "it never finished");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(result.found.len(), 1);
        assert_eq!(result.found[&want[0]], d.join("media/clip.mp4"));
        assert!(!result.timed_out && !result.stopped);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_same_name_in_two_folders_picks_the_better_match() {
        let d = dir("ambiguous");
        touch(&d.join("a/act1/loop.mp4"), "1");
        touch(&d.join("a/act2/loop.mp4"), "2");
        touch(&d.join("b/loop.mp4"), "3");
        let want1 = PathBuf::from("/was/act2/loop.mp4");
        let want2 = PathBuf::from("/was/elsewhere/loop.mp4");
        let found = search_all(&d, &[want1.clone(), want2.clone()]);
        assert_eq!(
            found[&want1],
            d.join("a/act2/loop.mp4"),
            "the folder name matches"
        );
        // No folder matches: the shallowest wins, not an arbitrary one.
        assert_eq!(found[&want2], d.join("b/loop.mp4"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn relinking_changes_every_cue_that_used_the_file() {
        let d = dir("apply");
        touch(&d.join("new.mp4"), "x");
        let old = PathBuf::from("/old/a.mp4");
        let mut show = show_with(&[old.clone(), old.clone(), PathBuf::from("/old/b.mp4")]);
        let moved = HashMap::from([(old.clone(), d.join("new.mp4"))]);
        assert_eq!(apply(&mut show, &moved), 2);
        assert_eq!(file_of(&show, 0), d.join("new.mp4"));
        assert_eq!(file_of(&show, 1), d.join("new.mp4"));
        assert_eq!(file_of(&show, 2), PathBuf::from("/old/b.mp4"));
        assert_eq!(missing(&show).len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn collecting_gives_clashing_names_a_number_and_shares_repeats() {
        let dest = Path::new("/dest");
        let files = vec![
            PathBuf::from("/a/clip.mp4"),
            PathBuf::from("/b/clip.mp4"),
            PathBuf::from("/a/clip.mp4"),
            PathBuf::from("/c/clip.mp4"),
            PathBuf::from("/a/tone.wav"),
        ];
        let plan = plan_collect(dest, &files);
        assert_eq!(
            plan,
            vec![
                (
                    PathBuf::from("/a/clip.mp4"),
                    PathBuf::from("/dest/clip.mp4")
                ),
                (
                    PathBuf::from("/b/clip.mp4"),
                    PathBuf::from("/dest/clip_2.mp4")
                ),
                (
                    PathBuf::from("/c/clip.mp4"),
                    PathBuf::from("/dest/clip_3.mp4")
                ),
                (
                    PathBuf::from("/a/tone.wav"),
                    PathBuf::from("/dest/tone.wav")
                ),
            ]
        );
    }

    #[test]
    fn collecting_never_overwrites_other_files_in_the_folder() {
        // Two shows share one media folder: the second must not replace the
        // first's clip.
        let d = dir("collect_keep");
        let dest = d.join("shared");
        touch(&dest.join("clip.mp4"), "first show");
        touch(&d.join("second/clip.mp4"), "second show");
        let src = d.join("second/clip.mp4");
        assert_eq!(
            plan_collect(&dest, std::slice::from_ref(&src)),
            vec![(src.clone(), dest.join("clip_2.mp4"))]
        );
        let got = collect(
            &dest,
            std::slice::from_ref(&src),
            &Arc::new(AtomicUsize::new(0)),
        );
        assert_eq!(got.copied, 1);
        assert_eq!(
            std::fs::read_to_string(dest.join("clip.mp4")).unwrap(),
            "first show",
            "the other show's file is untouched"
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("clip_2.mp4")).unwrap(),
            "second show"
        );
        assert_eq!(got.moved[&src], dest.join("clip_2.mp4"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn collecting_copies_the_files_and_reports_what_it_could_not() {
        let d = dir("collect");
        touch(&d.join("x/clip.mp4"), "one");
        touch(&d.join("y/clip.mp4"), "two");
        touch(&d.join("y/tone.wav"), "tone");
        let dest = d.join("show/media");
        let files = vec![
            d.join("x/clip.mp4"),
            d.join("y/clip.mp4"),
            d.join("y/tone.wav"),
            d.join("y/tone.wav"),
            d.join("lost.mp4"),
        ];
        let progress = Arc::new(AtomicUsize::new(0));
        let got = collect(&dest, &files, &progress);
        assert_eq!(got.copied, 3);
        assert_eq!(got.failed.len(), 1);
        assert_eq!(got.failed[0].0, d.join("lost.mp4"));
        assert_eq!(
            std::fs::read_to_string(dest.join("clip.mp4")).unwrap(),
            "one"
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("clip_2.mp4")).unwrap(),
            "two"
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("tone.wav")).unwrap(),
            "tone"
        );
        assert_eq!(got.moved[&d.join("y/clip.mp4")], dest.join("clip_2.mp4"));
        assert_eq!(progress.load(Ordering::Relaxed), 4, "one per distinct file");
        // The originals are left alone.
        assert!(d.join("x/clip.mp4").is_file());

        // The show's cues now point into the folder, and nothing is missing.
        let mut show = show_with(&files[..4]);
        assert_eq!(apply(&mut show, &got.moved), 4);
        assert!(missing(&show).is_empty());
        // Doing it again changes nothing: the files are already there.
        let again = collect(
            &dest,
            &[dest.join("clip.mp4"), dest.join("clip_2.mp4")],
            &Arc::new(AtomicUsize::new(0)),
        );
        assert_eq!((again.copied, again.already_there), (0, 2));
        assert!(again.moved.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_file_name_that_collides_with_one_already_in_the_folder_is_renamed() {
        let d = dir("collide");
        touch(&d.join("src/clip.mp4"), "incoming");
        touch(&d.join("dest/clip.mp4"), "already here");
        let got = collect(
            &d.join("dest"),
            &[d.join("src/clip.mp4"), d.join("dest/clip.mp4")],
            &Arc::new(AtomicUsize::new(0)),
        );
        assert_eq!((got.copied, got.already_there), (1, 1));
        assert_eq!(
            std::fs::read_to_string(d.join("dest/clip.mp4")).unwrap(),
            "already here",
            "an existing file is never overwritten"
        );
        assert_eq!(
            std::fs::read_to_string(d.join("dest/clip_2.mp4")).unwrap(),
            "incoming"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn only_cues_with_files_are_considered() {
        let show = show_with(&[]);
        assert!(missing(&show).is_empty());
        assert!(matches!(show.cues[0].kind, CueKind::Wait(_)));
    }

    // ----- Searching the computer --------------------------------------------------

    fn names(list: &[&str]) -> HashSet<OsString> {
        list.iter().map(OsString::from).collect()
    }

    /// Wait for a search to finish.
    fn finish(s: &Search) -> SearchState {
        let t0 = Instant::now();
        loop {
            let st = s.state();
            if st.done {
                return st;
            }
            assert!(t0.elapsed().as_secs() < 20, "the search never finished");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_search_finds_wanted_names_wherever_they_are() {
        let d = dir("scan");
        touch(&d.join("a/b/c/intro.mp4"), "x");
        touch(&d.join("z/intro.mp4"), "x");
        touch(&d.join("z/other.wav"), "x");
        touch(&d.join("z/unwanted.mov"), "x");
        let s = Search::start(
            vec![d.clone()],
            names(&["intro.mp4", "other.wav", "absent.png"]),
            SEARCH_BUDGET,
        );
        let st = finish(&s);
        assert!(!st.timed_out);
        assert_eq!(st.hits[&OsString::from("intro.mp4")].len(), 2);
        assert_eq!(
            st.hits[&OsString::from("other.wav")],
            vec![d.join("z/other.wav")]
        );
        assert!(!st.hits.contains_key(&OsString::from("absent.png")));
        assert!(
            !st.hits.contains_key(&OsString::from("unwanted.mov")),
            "only wanted names are kept"
        );
        assert!(st.folders >= 5, "{} folders", st.folders);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_search_skips_hidden_and_system_folders_and_reads_each_folder_once() {
        let d = dir("skip");
        touch(&d.join("media/clip.mp4"), "x");
        touch(&d.join(".cache/clip.mp4"), "x");
        touch(&d.join("node_modules/clip.mp4"), "x");
        touch(&d.join("AppData/clip.mp4"), "x");
        // The same folder given twice, and as a root inside another root.
        let s = Search::start(
            vec![d.clone(), d.join("media"), d.clone()],
            names(&["clip.mp4"]),
            SEARCH_BUDGET,
        );
        let st = finish(&s);
        assert_eq!(
            st.hits[&OsString::from("clip.mp4")],
            vec![d.join("media/clip.mp4")]
        );
        assert!(skip_dir(".git", false) && skip_dir("NODE_MODULES", false));
        assert!(skip_dir("Library", true) && !skip_dir("Library", false));
        assert!(!skip_dir("Music", true) && !skip_dir("act1", false));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_search_stops_when_told_or_out_of_time() {
        let d = dir("stop");
        for i in 0..20 {
            touch(&d.join(format!("f{i}/clip.mp4")), "x");
        }
        // No time at all: it gives up at once and says so.
        let s = Search::start(vec![d.clone()], names(&["clip.mp4"]), Duration::ZERO);
        let st = finish(&s);
        assert!(st.timed_out && st.hits.is_empty());
        // Told to stop: it ends (it may have found some by then).
        let s = Search::start(vec![d.clone()], names(&["clip.mp4"]), SEARCH_BUDGET);
        s.stop();
        let st = finish(&s);
        assert!(!st.timed_out && st.hits.values().map(Vec::len).sum::<usize>() <= 20);
        // Dropping a search stops it too.
        let s = Search::start(vec![d.clone()], names(&["clip.mp4"]), SEARCH_BUDGET);
        drop(s);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn suggestions_are_ranked_best_first() {
        let want = PathBuf::from("/was/act2/loop.mp4");
        let hits = HashMap::from([(
            OsString::from("loop.mp4"),
            vec![
                PathBuf::from("/x/deep/er/loop.mp4"),
                PathBuf::from("/b/loop.mp4"),
                PathBuf::from("/a/act2/loop.mp4"),
                PathBuf::from("/a/act2/loop.mp4"),
            ],
        )]);
        let s = suggestions(&hits, &[want.clone(), PathBuf::from("/was/none.mp4")]);
        assert_eq!(s.len(), 1, "no hit, no suggestion");
        assert_eq!(
            s[&want],
            vec![
                PathBuf::from("/a/act2/loop.mp4"),
                PathBuf::from("/b/loop.mp4"),
                PathBuf::from("/x/deep/er/loop.mp4"),
            ],
            "matching folder first, then shallowest; repeats dropped"
        );
    }

    #[test]
    fn the_places_to_look_start_beside_the_show_and_are_real_folders() {
        let d = dir("roots");
        std::fs::create_dir_all(d.join("shows/hamlet")).unwrap();
        let roots = default_roots(Some(&d.join("shows/hamlet")));
        assert_eq!(roots[0], d.join("shows/hamlet"));
        assert_eq!(roots[1], d.join("shows"));
        assert!(roots.iter().all(|r| r.is_dir()), "{roots:?}");
        let unique: HashSet<_> = roots.iter().collect();
        assert_eq!(unique.len(), roots.len(), "no folder twice");
        // Without a show folder it still has somewhere to look, if there is a home.
        let _ = default_roots(None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
