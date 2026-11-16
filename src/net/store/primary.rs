//! The primary's side: its show's files are served from where they are, never
//! copied. Each is checked against the size and time the manifest has before
//! it is sent; one that has changed is hashed again on a thread and a new
//! version of the manifest is made, which the receivers then fetch.

use super::http::{Provider, Served};
use super::manifest::{self, Entry, HashCache, HashError, Manifest, modified_ns};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Earlier versions still answered for (their files, while unchanged).
const OLDER_KEPT: usize = 5;

#[derive(Default)]
struct State {
    current: Option<Manifest>,
    older: VecDeque<Manifest>,
    /// Digest → the entries (newest version first) that have it.
    by_sha: HashMap<String, Vec<Entry>>,
}

struct Inner {
    state: Mutex<State>,
    cache: Arc<Mutex<HashCache>>,
    /// Paths being hashed again after a change.
    rehashing: Mutex<HashSet<String>>,
    // ---- Take control (stage 5) ----
    /// A primary that was a follower before (it took control): its store,
    /// whose objects stand in for files the show names that aren't on this
    /// computer's disk.
    store: Mutex<Option<super::Store>>,
    /// The digests of entries taken from the store (`inherit`).
    inherited: Mutex<HashSet<String>>,
}

/// The primary's media, as served. Cheap to clone.
#[derive(Clone)]
pub struct Primary {
    inner: Arc<Inner>,
}

impl Primary {
    pub fn new(cache: Arc<Mutex<HashCache>>) -> Primary {
        Primary {
            inner: Arc::new(Inner {
                state: Mutex::new(State::default()),
                cache,
                rehashing: Mutex::new(HashSet::new()),
                store: Mutex::new(None),
                inherited: Mutex::new(HashSet::new()),
            }),
        }
    }

    // ---- Take control (stage 5) ----

    /// The store of a computer that took control (None: files only where
    /// they are).
    pub fn set_store(&self, store: Option<super::Store>) {
        *self.inner.store.lock() = store;
    }

    pub fn store(&self) -> Option<super::Store> {
        self.inner.store.lock().clone()
    }

    /// A version made from the show's paths, with each file that isn't on
    /// this computer's disk taken from what the store knows of it (the old
    /// primary's digest, size and facts), so the other computers keep what
    /// they have. Also returns how many of those this computer hasn't the
    /// bytes of either (their cues show a card here; the others, which have
    /// them, play them).
    pub fn inherit(&self, m: Manifest) -> (Manifest, usize) {
        let Some(store) = self.store() else {
            return (m, 0);
        };
        let mut entries = m.entries.clone();
        let mut taken = 0;
        let mut lacking = 0;
        let mut inherited = self.inner.inherited.lock();
        for e in entries.iter_mut().filter(|e| e.missing && !e.library) {
            let Some(old) = store.find_latest(&e.path) else {
                continue;
            };
            e.size = old.size;
            e.sha256 = old.sha256.clone();
            e.modified = old.modified;
            e.facts = old.facts.clone();
            e.missing = false;
            inherited.insert(e.sha256.clone());
            taken += 1;
            if store.any_with(&e.sha256, e.size).is_none() {
                lacking += 1;
            }
        }
        drop(inherited);
        if taken == 0 {
            return (m, 0);
        }
        (Manifest::new(entries), lacking)
    }

    fn is_inherited(&self, sha: &str) -> bool {
        !sha.is_empty() && self.inner.inherited.lock().contains(sha)
    }

    /// The hash cache (shared with `Hashing` when the show changes).
    pub fn cache(&self) -> Arc<Mutex<HashCache>> {
        self.inner.cache.clone()
    }

    /// Make `m` the version served as current. The same id again changes
    /// nothing but the times it holds.
    pub fn publish(&self, m: Manifest) {
        let mut st = self.inner.state.lock();
        if let Some(old) = st.current.take()
            && old.id != m.id
        {
            st.older.retain(|o| o.id != old.id && o.id != m.id);
            st.older.push_front(old);
            st.older.truncate(OLDER_KEPT);
        }
        st.current = Some(m);
        index(&mut st);
    }

    pub fn current(&self) -> Option<Manifest> {
        self.inner.state.lock().current.clone()
    }

    pub fn current_id(&self) -> Option<String> {
        self.inner
            .state
            .lock()
            .current
            .as_ref()
            .map(|m| m.id.clone())
    }

    /// Whether a changed file is being hashed again (a new version follows).
    pub fn catching_up(&self) -> bool {
        !self.inner.rehashing.lock().is_empty()
    }

    /// Look at every file of the current version (its size and time only) and
    /// hash again those that changed: an edit to a file every computer already
    /// has is noticed without anyone asking for it.
    pub fn check_files(&self) {
        let Some(cur) = self.current() else {
            return;
        };
        for e in &cur.entries {
            let now = std::fs::metadata(&e.path).ok().filter(|m| m.is_file());
            let same = match (&now, e.missing) {
                (None, true) => true,
                (Some(m), false) => m.len() == e.size && modified_ns(m) == e.modified,
                // Taken from the store: not on this disk, and that is no change.
                (None, false) => self.is_inherited(&e.sha256),
                _ => false,
            };
            if !same {
                self.changed(&e.path);
            }
        }
    }

    /// `check_files` every `every` on a thread, until the `Watch` is dropped.
    pub fn watch(&self, every: std::time::Duration) -> Watch {
        let stop = Arc::new(AtomicBool::new(false));
        let (me, st) = (self.clone(), stop.clone());
        let thread = std::thread::Builder::new()
            .name("media-watch".into())
            .spawn(move || {
                let step = std::time::Duration::from_millis(100);
                let mut waited = std::time::Duration::ZERO;
                while !st.load(std::sync::atomic::Ordering::Relaxed) {
                    std::thread::sleep(step);
                    waited += step;
                    if waited >= every {
                        waited = std::time::Duration::ZERO;
                        me.check_files();
                    }
                }
            })
            .ok();
        Watch { stop, thread }
    }

    /// A file of the show has changed: hash it again on a thread and publish
    /// a version with its new digest (or marked missing).
    fn changed(&self, path: &str) {
        if !self.inner.rehashing.lock().insert(path.to_string()) {
            return;
        }
        let me = self.clone();
        let path = path.to_string();
        let spawned = std::thread::Builder::new()
            .name("media-rehash".into())
            .spawn({
                let path = path.clone();
                move || {
                    let me2 = me.clone();
                    let p2 = path.clone();
                    let _done = crate::crash::OnPanic(move || {
                        me2.inner.rehashing.lock().remove(&p2);
                    });
                    me.rehash(&path);
                    me.inner.rehashing.lock().remove(&path);
                }
            });
        if spawned.is_err() {
            self.inner.rehashing.lock().remove(&path);
        }
    }

    fn rehash(&self, path: &str) {
        let file = PathBuf::from(path);
        let never = AtomicBool::new(false);
        // Until it stays the same while it is read (a file being written).
        let looked = loop {
            let Some(before) = std::fs::metadata(&file).ok().filter(|m| m.is_file()) else {
                break None;
            };
            let (size, modified) = (before.len(), modified_ns(&before));
            if let Some(sha) = self.inner.cache.lock().get(path, size, modified) {
                break Some((size, modified, sha));
            }
            // A file still being written is waited for, not read again and
            // again.
            std::thread::sleep(std::time::Duration::from_millis(500));
            let still = std::fs::metadata(&file)
                .ok()
                .is_some_and(|m| m.len() == size && modified_ns(&m) == modified);
            if !still {
                continue;
            }
            match manifest::hash_file(&file, &never, &mut |_| {}) {
                Ok(sha) => {
                    let after = std::fs::metadata(&file).ok();
                    if after.is_some_and(|m| m.len() == size && modified_ns(&m) == modified) {
                        let mut c = self.inner.cache.lock();
                        c.put(path, size, modified, &sha);
                        let _ = c.save();
                        break Some((size, modified, sha));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                Err(HashError::Io(_)) => break None,
                Err(HashError::Cancelled) => return,
            }
        };
        let st = self.inner.state.lock();
        let Some(cur) = st.current.clone() else {
            return;
        };
        let mut entries = cur.entries.clone();
        let mut touched = false;
        for e in entries.iter_mut().filter(|e| e.path == path) {
            touched = true;
            // What was read from the file before isn't true of it now (the
            // media manager reads it again).
            if looked.as_ref().is_none_or(|(_, _, sha)| *sha != e.sha256) {
                e.facts = None;
            }
            match &looked {
                Some((size, modified, sha)) => {
                    e.size = *size;
                    e.modified = *modified;
                    e.sha256 = sha.clone();
                    e.missing = false;
                }
                None => {
                    e.size = 0;
                    e.modified = 0;
                    e.sha256.clear();
                    e.missing = true;
                }
            }
        }
        if !touched {
            return;
        }
        let next = Manifest::new(entries);
        drop(st);
        if next.id == cur.id {
            // Only its time changed: the same version, with the new time.
            let mut st = self.inner.state.lock();
            if let Some(c) = &mut st.current
                && c.id == next.id
            {
                *c = next;
                index(&mut st);
            }
        } else {
            self.publish(next);
        }
    }
}

fn index(st: &mut State) {
    let mut by_sha: HashMap<String, Vec<Entry>> = HashMap::new();
    for m in st.current.iter().chain(st.older.iter()) {
        for e in m.present() {
            by_sha.entry(e.sha256.clone()).or_default().push(e.clone());
        }
    }
    st.by_sha = by_sha;
}

impl Provider for Primary {
    fn current_manifest(&self) -> Option<Manifest> {
        self.current()
    }

    fn manifest(&self, id: &str) -> Option<Manifest> {
        let st = self.inner.state.lock();
        st.current
            .iter()
            .chain(st.older.iter())
            .find(|m| m.id == id)
            .cloned()
    }

    fn object(&self, sha: &str) -> Served {
        let candidates = self
            .inner
            .state
            .lock()
            .by_sha
            .get(sha)
            .cloned()
            .unwrap_or_default();
        let mut changed = false;
        for e in candidates {
            let file = PathBuf::from(&e.path);
            match std::fs::metadata(&file) {
                Ok(m) if m.is_file() && m.len() == e.size && modified_ns(&m) == e.modified => {
                    return Served::File {
                        path: file,
                        size: e.size,
                    };
                }
                // Taken from the store (stage 5): its object, if it is here.
                Err(_) if self.is_inherited(&e.sha256) => {
                    if let Some(store) = self.store()
                        && let Some(key) = store.any_with(&e.sha256, e.size)
                    {
                        return Served::File {
                            path: store.object_path(&key),
                            size: e.size,
                        };
                    }
                }
                _ => {
                    // Changed or gone since the version was made.
                    changed = true;
                    self.changed(&e.path);
                }
            }
        }
        if changed {
            Served::Changed
        } else {
            Served::NotHere
        }
    }
}

/// The primary's look at its files; dropping it stops the look.
pub struct Watch {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
