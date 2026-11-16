//! What a show version is made of: every media file it names, by size and
//! SHA-256. Made on the primary, on a thread, with a cache so a file that
//! hasn't changed is never read twice.

use super::paths::{self, path_key, path_text};
use crate::show::Show;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

/// One media file of a show version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The path as the show names it (the primary's spelling).
    pub path: String,
    /// The same, relative to the show's folder with `/` between the parts,
    /// when the file is in that folder (a show saved beside its media names
    /// it this way in the file).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rel: Option<String>,
    pub size: u64,
    /// Lower-case hex. Empty for a missing file.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sha256: String,
    /// Nanoseconds since 1970 on the primary's clock (not in the id).
    #[serde(default)]
    pub modified: u64,
    /// The file wasn't there (or couldn't be read) when the manifest was made.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub missing: bool,
    /// A file in the media manager that no cue uses yet: fetched only after
    /// every file the show uses.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub library: bool,
}

impl Entry {
    /// The key its path is compared by (see `paths::path_key`).
    pub fn key(&self) -> String {
        path_key(&self.path)
    }

    /// The name its object has in a store: the digest and the file's own
    /// extension.
    pub fn object(&self) -> ObjectKey {
        ObjectKey {
            sha: self.sha256.clone(),
            ext: paths::extension(&self.path),
        }
    }
}

/// An object in a store: the content's digest and the extension it is kept
/// with (the same bytes under two extensions are two files).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectKey {
    pub sha: String,
    pub ext: String,
}

impl ObjectKey {
    pub fn file_name(&self) -> String {
        if self.ext.is_empty() {
            self.sha.clone()
        } else {
            format!("{}.{}", self.sha, self.ext)
        }
    }

    /// Read back from a file name in the store. None for anything else.
    pub fn from_file_name(name: &str) -> Option<ObjectKey> {
        let (sha, ext) = match name.split_once('.') {
            Some((s, e)) => (s, e),
            None => (name, ""),
        };
        let ext_ok = ext.len() <= 12 && ext.chars().all(|c| c.is_ascii_alphanumeric());
        (paths::is_sha(sha) && ext_ok).then(|| ObjectKey {
            sha: sha.to_string(),
            ext: ext.to_string(),
        })
    }
}

/// A show version's media.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The SHA-256 of `canonical()`: the same media gives the same id.
    pub id: String,
    /// When it was made, milliseconds since 1970 (not in the id).
    pub made: u64,
    /// Sorted by path.
    pub entries: Vec<Entry>,
}

/// What the id is worked out from: the entries without their times.
#[derive(Serialize)]
struct Canon<'a> {
    v: u32,
    entries: Vec<CanonEntry<'a>>,
}

#[derive(Serialize)]
struct CanonEntry<'a> {
    path: &'a str,
    rel: Option<&'a str>,
    size: u64,
    sha256: &'a str,
    missing: bool,
    library: bool,
}

impl Manifest {
    /// A manifest of these entries, sorted, with its id and the time now. A
    /// path named twice is kept once (as used, if either is).
    pub fn new(mut entries: Vec<Entry>) -> Manifest {
        entries.sort_by(|a, b| a.path.cmp(&b.path).then(a.library.cmp(&b.library)));
        entries.dedup_by(|a, b| a.path == b.path);
        let mut m = Manifest {
            id: String::new(),
            made: now_ms(),
            entries,
        };
        m.id = m.work_out_id();
        m
    }

    /// The serialisation the id is the hash of.
    pub fn canonical(&self) -> Vec<u8> {
        let canon = Canon {
            v: 1,
            entries: self
                .entries
                .iter()
                .map(|e| CanonEntry {
                    path: &e.path,
                    rel: e.rel.as_deref(),
                    size: e.size,
                    sha256: &e.sha256,
                    missing: e.missing,
                    library: e.library,
                })
                .collect(),
        };
        serde_json::to_vec(&canon).unwrap_or_default()
    }

    pub fn work_out_id(&self) -> String {
        hex(&Sha256::digest(self.canonical()))
    }

    /// Whether the id matches the content (a manifest from the network or
    /// the disk is checked with this).
    pub fn is_sound(&self) -> bool {
        paths::is_sha(&self.id)
            && self.id == self.work_out_id()
            && self
                .entries
                .iter()
                .all(|e| e.missing || paths::is_sha(&e.sha256))
    }

    /// The entries whose file is there to be had.
    pub fn present(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| !e.missing)
    }

    /// The objects it names, each once, with its size.
    pub fn objects(&self) -> HashMap<ObjectKey, u64> {
        self.present().map(|e| (e.object(), e.size)).collect()
    }

    /// The entry for a path the show names, however it is spelled (see
    /// `Lookup`).
    pub fn find(&self, show_path: &str) -> Option<&Entry> {
        Lookup::new(self).find(self, show_path)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Read and check one (the id must match the content).
    pub fn from_json(text: &[u8]) -> Option<Manifest> {
        let m: Manifest = serde_json::from_slice(text).ok()?;
        m.is_sound().then_some(m)
    }
}

/// Finding a manifest's entry for a show path: the same spelling first, then
/// ignoring letter case, then by the path relative to the show's folder (the
/// receiver's copy of the show is in another folder), the longest that fits.
#[derive(Default)]
pub struct Lookup {
    exact: HashMap<String, usize>,
    loose: HashMap<String, usize>,
    rel: HashMap<String, usize>,
}

impl Lookup {
    pub fn new(m: &Manifest) -> Lookup {
        let mut l = Lookup::default();
        for (i, e) in m.entries.iter().enumerate() {
            l.exact.entry(e.key()).or_insert(i);
            l.loose.entry(paths::loose_key(&e.path)).or_insert(i);
            if let Some(r) = &e.rel {
                l.rel.entry(paths::loose_key(r)).or_insert(i);
            }
        }
        l
    }

    pub fn index(&self, show_path: &str) -> Option<usize> {
        if let Some(&i) = self.exact.get(&path_key(show_path)) {
            return Some(i);
        }
        if let Some(&i) = self.loose.get(&paths::loose_key(show_path)) {
            return Some(i);
        }
        if self.rel.is_empty() {
            return None;
        }
        let parts = paths::parts(show_path);
        (0..parts.len()).find_map(|from| {
            let tail = parts[from..].join("/").to_lowercase();
            self.rel.get(&tail).copied()
        })
    }

    pub fn find<'m>(&self, m: &'m Manifest, show_path: &str) -> Option<&'m Entry> {
        self.index(show_path).and_then(|i| m.entries.get(i))
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// A file's modified time, nanoseconds since 1970 (0 if unknown or earlier).
pub fn modified_ns(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos().min(u128::from(u64::MAX)) as u64)
}

/// The media files in a folder and the folders in it (for the library: the
/// media manager's folders), of the kinds a cue can play, sorted. Hidden
/// files and folders are skipped; at most 12 folders deep.
pub fn media_in_folder(folder: &Path) -> Vec<PathBuf> {
    const MAX_DEPTH: usize = 12;
    let mut out = Vec::new();
    let mut stack = vec![(folder.to_path_buf(), 0usize)];
    let mut seen = HashSet::new();
    while let Some((dir, depth)) = stack.pop() {
        if !seen.insert(std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone())) {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = e.path();
            match std::fs::metadata(&path) {
                Ok(m) if m.is_file() && crate::show::guess_cue_type(&path).is_some() => {
                    out.push(path)
                }
                Ok(m) if m.is_dir() && depth < MAX_DEPTH => stack.push((path, depth + 1)),
                _ => {}
            }
        }
    }
    out.sort();
    out
}

/// Every media file a show names, each once, in the show's order.
pub fn media_paths(show: &Show) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    show.cues
        .iter()
        .filter_map(|c| c.kind.file_media())
        .map(|m| m.file.clone())
        .filter(|f| !f.as_os_str().is_empty())
        .filter(|f| seen.insert(f.clone()))
        .collect()
}

/// Reading a file to hash it: how much at a time.
const CHUNK: usize = 1 << 20;

/// Why hashing stopped short.
#[derive(Debug)]
pub enum HashError {
    Cancelled,
    Io(std::io::Error),
}

/// The SHA-256 of what `reader` gives, read a piece at a time (never the
/// whole file in memory). `progress` is told each piece's length.
pub fn hash_reader(
    reader: &mut dyn Read,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64),
) -> Result<String, HashError> {
    let mut h = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(HashError::Cancelled);
        }
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(HashError::Io(e)),
        };
        h.update(&buf[..n]);
        progress(n as u64);
    }
    Ok(hex(&h.finalize()))
}

pub fn hash_file(
    path: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64),
) -> Result<String, HashError> {
    let mut f = std::fs::File::open(path).map_err(HashError::Io)?;
    hash_reader(&mut f, cancel, progress)
}

/// One remembered digest.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Known {
    path: String,
    size: u64,
    modified: u64,
    sha256: String,
}

/// Digests by (path, size, modified time), kept in a file, so an unchanged
/// file is hashed once. One record per path: a changed file replaces its own.
#[derive(Default)]
pub struct HashCache {
    file: Option<PathBuf>,
    by_path: HashMap<String, Known>,
    changed: bool,
}

impl HashCache {
    /// A cache kept in `file` (read now if it is there; a damaged one is
    /// started again). None keeps it in memory only.
    pub fn open(file: Option<PathBuf>) -> HashCache {
        let mut by_path = HashMap::new();
        if let Some(f) = &file
            && let Ok(text) = std::fs::read(f)
            && let Ok(list) = serde_json::from_slice::<Vec<Known>>(&text)
        {
            for k in list {
                if paths::is_sha(&k.sha256) {
                    by_path.insert(k.path.clone(), k);
                }
            }
        }
        HashCache {
            file,
            by_path,
            changed: false,
        }
    }

    pub fn get(&self, path: &str, size: u64, modified: u64) -> Option<String> {
        self.by_path
            .get(path)
            .filter(|k| k.size == size && k.modified == modified)
            .map(|k| k.sha256.clone())
    }

    pub fn put(&mut self, path: &str, size: u64, modified: u64, sha256: &str) {
        self.by_path.insert(
            path.to_string(),
            Known {
                path: path.to_string(),
                size,
                modified,
                sha256: sha256.to_string(),
            },
        );
        self.changed = true;
    }

    pub fn len(&self) -> usize {
        self.by_path.len()
    }

    /// Write it out if anything changed (beside the file, then renamed over it).
    pub fn save(&mut self) -> std::io::Result<()> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        if !self.changed {
            return Ok(());
        }
        let mut list: Vec<&Known> = self.by_path.values().collect();
        list.sort_by(|a, b| a.path.cmp(&b.path));
        let text = serde_json::to_vec(&list).map_err(std::io::Error::other)?;
        write_atomically(file, &text)?;
        self.changed = false;
        Ok(())
    }
}

/// Write `bytes` to a file beside `path`, flush it to the disk and rename it
/// over `path`, so a crash never leaves half a file.
pub fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    let tmp = path.with_file_name(name);
    let written = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// How far making a manifest has got.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HashProgress {
    pub files_done: usize,
    pub files_total: usize,
    /// Bytes read so far and to read (files the cache knew don't count).
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// The file being read now.
    pub current: Option<String>,
    /// Bytes actually read and hashed (what the cache didn't save).
    pub bytes_hashed: u64,
}

/// What a manifest is made from.
#[derive(Clone, Debug, Default)]
pub struct Sources {
    /// The files the show uses (`media_paths`), as the show names them.
    pub used: Vec<PathBuf>,
    /// The media manager's files no cue uses yet.
    pub library: Vec<PathBuf>,
    /// The media manager's folders: their media files (`media_in_folder`)
    /// join the library, listed on the hashing thread.
    pub library_folders: Vec<PathBuf>,
    /// The show file's folder, for the relative forms.
    pub show_dir: Option<PathBuf>,
}

/// Make the manifest for these files. A file that isn't there is an entry
/// marked missing. Saves the cache at the end.
pub fn build(
    src: &Sources,
    cache: &Mutex<HashCache>,
    cancel: &AtomicBool,
    progress: &Mutex<HashProgress>,
) -> Result<Manifest, HashError> {
    let mut seen: HashSet<&PathBuf> = src.used.iter().collect();
    let mut files: Vec<(PathBuf, bool)> = src.used.iter().map(|f| (f.clone(), false)).collect();
    let listed: Vec<PathBuf> = src
        .library_folders
        .iter()
        .flat_map(|d| media_in_folder(d))
        .collect();
    for f in src.library.iter().chain(listed.iter()) {
        if cancel.load(Ordering::Relaxed) {
            return Err(HashError::Cancelled);
        }
        if seen.insert(f) {
            files.push((f.clone(), true));
        }
    }
    build_files(&files, src.show_dir.as_deref(), cache, cancel, progress)
}

/// A file's path as text, its size and time if it is there, and the digest
/// the cache knows for those.
type Looked = (String, Option<(u64, u64)>, Option<String>);

fn build_files(
    files: &[(PathBuf, bool)],
    show_dir: Option<&Path>,
    cache: &Mutex<HashCache>,
    cancel: &AtomicBool,
    progress: &Mutex<HashProgress>,
) -> Result<Manifest, HashError> {
    let dir_text = show_dir.map(path_text);
    // A first look at every file, so the totals are known before reading.
    let looked: Vec<Looked> = files
        .iter()
        .map(|(f, _)| {
            let text = path_text(f);
            let meta = std::fs::metadata(f).ok().filter(|m| m.is_file());
            let sm = meta.map(|m| (m.len(), modified_ns(&m)));
            let known = sm.and_then(|(s, t)| cache.lock().get(&text, s, t));
            (text, sm, known)
        })
        .collect();
    {
        let mut p = progress.lock();
        p.files_total = looked.len();
        p.bytes_total = looked
            .iter()
            .filter(|l| l.2.is_none())
            .filter_map(|l| l.1.map(|(s, _)| s))
            .sum();
    }
    let mut entries = Vec::with_capacity(looked.len());
    // Saved now and then, so a long first look at a big library isn't lost
    // to a quit half way.
    let mut saved = std::time::Instant::now();
    for ((f, library), (text, sm, known)) in files.iter().zip(looked) {
        let rel = dir_text
            .as_deref()
            .and_then(|d| paths::relative_to(&text, d));
        let mut entry = Entry {
            path: text.clone(),
            rel,
            size: 0,
            sha256: String::new(),
            modified: 0,
            missing: true,
            library: *library,
        };
        if let Some((size, modified)) = sm {
            let sha = match known {
                Some(s) => Some(s),
                None => {
                    progress.lock().current = Some(text.clone());
                    let mut add = |n: u64| {
                        let mut p = progress.lock();
                        p.bytes_done += n;
                        p.bytes_hashed += n;
                    };
                    match hash_file(f, cancel, &mut add) {
                        Ok(s) => {
                            // Changed while it was read: the digest may be of
                            // neither version, so it is not remembered and the
                            // entry carries the time it had, which the next
                            // look will find different.
                            let after = std::fs::metadata(f).ok();
                            let same = after
                                .as_ref()
                                .is_some_and(|m| m.len() == size && modified_ns(m) == modified);
                            if same {
                                cache.lock().put(&text, size, modified, &s);
                            }
                            Some(s)
                        }
                        Err(HashError::Cancelled) => return Err(HashError::Cancelled),
                        Err(HashError::Io(_)) => None,
                    }
                }
            };
            if let Some(sha) = sha {
                entry.size = size;
                entry.sha256 = sha;
                entry.modified = modified;
                entry.missing = false;
            }
        }
        entries.push(entry);
        if saved.elapsed() > std::time::Duration::from_secs(30) {
            saved = std::time::Instant::now();
            let _ = cache.lock().save();
        }
        let mut p = progress.lock();
        p.files_done += 1;
        p.current = None;
    }
    let _ = cache.lock().save();
    Ok(Manifest::new(entries))
}

/// Making a manifest on a thread. Dropping it cancels.
pub struct Hashing {
    cancel: Arc<AtomicBool>,
    progress: Arc<Mutex<HashProgress>>,
    result: Arc<Mutex<Option<Result<Manifest, String>>>>,
    thread: Option<JoinHandle<()>>,
}

impl Hashing {
    pub fn start(src: Sources, cache: Arc<Mutex<HashCache>>) -> Hashing {
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Mutex::new(HashProgress::default()));
        let result = Arc::new(Mutex::new(None));
        let (c, p, r) = (cancel.clone(), progress.clone(), result.clone());
        let thread = std::thread::Builder::new()
            .name("media-hash".into())
            .spawn(move || {
                let r2 = r.clone();
                let _said = crate::crash::OnPanic(move || {
                    *r2.lock() = Some(Err("making the list of media stopped unexpectedly".into()));
                });
                let out = build(&src, &cache, &c, &p);
                *r.lock() = Some(out.map_err(|e| match e {
                    HashError::Cancelled => "cancelled".to_string(),
                    HashError::Io(e) => e.to_string(),
                }));
            })
            .ok();
        Hashing {
            cancel,
            progress,
            result,
            thread,
        }
    }

    pub fn progress(&self) -> HashProgress {
        self.progress.lock().clone()
    }

    /// The manifest, once made (taken: asks after that give None).
    pub fn take(&self) -> Option<Result<Manifest, String>> {
        self.result.lock().take()
    }

    pub fn is_done(&self) -> bool {
        self.thread.as_ref().is_none_or(|t| t.is_finished())
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for Hashing {
    fn drop(&mut self) {
        self.cancel();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
