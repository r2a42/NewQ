//! The media store on a computer that receives a show: objects by digest,
//! the manifests kept, and which one is current.
//!
//! ```text
//! <root>/objects/ab/<sha256>.<ext>       verified content (only ever renamed in whole)
//! <root>/objects/ab/<sha256>.<ext>.part  a fetch under way or broken off (resumed)
//! <root>/manifests/<id>.json             the versions kept
//! <root>/history.json                    the ids that were current, newest last
//! <root>/current                         the current id
//! <root>/pending                         the id a sync is fetching for
//! ```

use super::manifest::{self, Entry, HashError, Lookup, Manifest, ObjectKey};
use super::space::{self, FreeSpace};
use parking_lot::{Condvar, Mutex};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

/// How a computer keeps versions (owner's ruling R4).
#[derive(Clone, Debug, PartialEq)]
pub struct Policy {
    /// How many show versions to keep, the current one included (at least 1).
    /// Every object a kept version names stays, so going back needs no fetch.
    pub keep: usize,
    /// Replace instead of keeping versions, for every file.
    pub replace_all: bool,
    /// Replace files larger than this many bytes (a very large file).
    pub replace_over: Option<u64>,
    /// Room left free on the disk whatever happens (the show, the log, the
    /// system need some).
    pub reserve: u64,
}

impl Default for Policy {
    fn default() -> Policy {
        Policy {
            keep: 5,
            replace_all: false,
            replace_over: None,
            reserve: 512 * 1024 * 1024,
        }
    }
}

impl Policy {
    /// Whether a file of this size is replaced rather than kept per version.
    pub fn replaces(&self, size: u64) -> bool {
        self.replace_all || self.replace_over.is_some_and(|over| size > over)
    }
}

/// A manifest the store holds, with its path lookup made once.
pub struct Kept {
    pub manifest: Manifest,
    lookup: Lookup,
}

impl Kept {
    fn new(manifest: Manifest) -> Kept {
        let lookup = Lookup::new(&manifest);
        Kept { manifest, lookup }
    }

    pub fn find(&self, show_path: &str) -> Option<&Entry> {
        self.lookup.find(&self.manifest, show_path)
    }
}

/// What a computer can say about one of the show's files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readiness {
    /// Here and verified.
    Here,
    /// Named by the show version but not here (yet).
    Missing,
    /// The primary itself didn't have it when the version was made.
    MissingOnPrimary,
    /// Not a file of the current version (or there is none).
    NotInShow,
}

/// What a sync to a manifest would do, worked out before doing it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SyncPlan {
    pub manifest_id: String,
    /// The objects to get, each once.
    pub fetch: Vec<Fetch>,
    /// Bytes to come over the network.
    pub bytes_to_fetch: u64,
    /// Bytes to be written here (copies made locally included).
    pub bytes_to_write: u64,
    /// Files the primary itself lacked: nothing can bring them.
    pub unavailable: Vec<Entry>,
    /// Bytes given back once the sync and its tidy-up are done.
    pub bytes_freed: u64,
    /// Room on the disk now (None: couldn't tell).
    pub free: Option<u64>,
    /// Whether it fits, once what may go first has gone.
    pub enough_room: bool,
    /// Room is short: these go before anything is fetched (old versions of
    /// files set to replace, and what nothing kept names).
    pub free_first: Vec<ObjectKey>,
    pub bytes_freed_first: u64,
    /// Old objects each new one replaces, removed as soon as it is verified.
    pub replaces: HashMap<ObjectKey, Vec<ObjectKey>>,
}

/// One object to get.
#[derive(Clone, Debug, PartialEq)]
pub struct Fetch {
    pub key: ObjectKey,
    pub size: u64,
    /// The show paths that name it (for messages).
    pub paths: Vec<String>,
    /// The same bytes are here under another extension: copied, not fetched.
    pub local: Option<ObjectKey>,
}

impl SyncPlan {
    /// Nothing to do.
    pub fn is_empty(&self) -> bool {
        self.fetch.is_empty()
    }

    /// What it means, in a sentence for the person running the show.
    pub fn words(&self) -> String {
        if self.fetch.is_empty() {
            return "All the media is here.".into();
        }
        let mut s = format!(
            "{} file{} to copy ({})",
            self.fetch.len(),
            if self.fetch.len() == 1 { "" } else { "s" },
            space::words(self.bytes_to_fetch)
        );
        if self.bytes_freed > 0 {
            s.push_str(&format!(", {} freed after", space::words(self.bytes_freed)));
        }
        if !self.enough_room {
            s.push_str(&format!(
                ". Not enough room on this computer: {} free",
                space::words(self.free.unwrap_or(0))
            ));
        } else if !self.free_first.is_empty() {
            s.push_str(&format!(
                ". Room is short: {} of older versions go first",
                space::words(self.bytes_freed_first)
            ));
        }
        s.push('.');
        s
    }
}

/// What a tidy-up removed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GcReport {
    pub objects_removed: usize,
    pub bytes_freed: u64,
    pub parts_removed: usize,
    pub manifests_removed: usize,
}

/// What a check of every object found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VerifyReport {
    pub checked: usize,
    /// Objects whose bytes didn't match their name; removed (fetched again by
    /// the next sync).
    pub bad: Vec<ObjectKey>,
    pub cancelled: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VerifyProgress {
    pub files_done: usize,
    pub files_total: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

struct State {
    policy: Policy,
    /// Ids that were current, newest last.
    history: Vec<String>,
    pending: Option<String>,
    manifests: HashMap<String, Arc<Kept>>,
    /// Objects on the disk, verified, with their sizes.
    objects: HashMap<ObjectKey, u64>,
}

struct Inner {
    root: PathBuf,
    free: FreeSpace,
    state: Mutex<State>,
    /// Objects a thread is fetching, copying, checking or removing.
    claims: Mutex<HashSet<ObjectKey>>,
    released: Condvar,
    generation: AtomicU64,
}

/// A receiving computer's store. Cheap to clone; every clone is the same store.
#[derive(Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

/// An object one thread has to itself until this is dropped.
pub struct Claim {
    inner: Arc<Inner>,
    key: ObjectKey,
}

impl Drop for Claim {
    fn drop(&mut self) {
        self.inner.claims.lock().remove(&self.key);
        self.inner.released.notify_all();
    }
}

impl Store {
    /// Open (or make) the store in `root`. `free` tells the room on its disk.
    pub fn open(root: &Path, policy: Policy, free: FreeSpace) -> std::io::Result<Store> {
        std::fs::create_dir_all(root.join("objects"))?;
        std::fs::create_dir_all(root.join("manifests"))?;
        std::fs::create_dir_all(root.join(PARTIAL))?;
        let mut st = State {
            policy: sane(policy),
            history: Vec::new(),
            pending: None,
            manifests: HashMap::new(),
            objects: HashMap::new(),
        };
        let read_id = |name: &str| {
            std::fs::read_to_string(root.join(name))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| super::paths::is_sha(s))
        };
        let mut history: Vec<String> = std::fs::read(root.join("history.json"))
            .ok()
            .and_then(|t| serde_json::from_slice(&t).ok())
            .unwrap_or_default();
        // `current` is written last: if it is newer than the history (a crash
        // between the two), it wins.
        if let Some(cur) = read_id("current")
            && history.last() != Some(&cur)
        {
            history.retain(|h| h != &cur);
            history.push(cur);
        }
        for id in history {
            if let Some(k) = load_manifest(root, &id) {
                st.manifests.insert(id.clone(), Arc::new(k));
                st.history.push(id);
            }
        }
        if let Some(p) = read_id("pending")
            && let Some(k) = load_manifest(root, &p)
        {
            st.manifests.insert(p.clone(), Arc::new(k));
            st.pending = Some(p);
        }
        st.objects = scan_objects(root).0;
        let manifests: Vec<Arc<Kept>> = st.manifests.values().cloned().collect();
        let store = Store {
            inner: Arc::new(Inner {
                root: root.to_path_buf(),
                free,
                state: Mutex::new(st),
                claims: Mutex::new(HashSet::new()),
                released: Condvar::new(),
                generation: AtomicU64::new(0),
            }),
        };
        for k in manifests {
            store.drop_wrong_sizes(&k.manifest);
        }
        Ok(store)
    }

    /// Objects whose length isn't what a version says (a damaged disk, a copy
    /// cut short by hand) are removed, never handed out.
    fn drop_wrong_sizes(&self, m: &Manifest) {
        let wrong: Vec<ObjectKey> = {
            let st = self.inner.state.lock();
            m.present()
                .filter(|e| st.objects.get(&e.object()).is_some_and(|s| *s != e.size))
                .map(Entry::object)
                .collect()
        };
        for key in wrong {
            let _ = std::fs::remove_file(self.object_path(&key));
            self.inner.state.lock().objects.remove(&key);
            self.bump();
        }
    }

    fn bump(&self) {
        self.inner.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Changes whenever an object arrives or goes (to know when to look again).
    pub fn generation(&self) -> u64 {
        self.inner.generation.load(Ordering::SeqCst)
    }

    /// The objects here, with their sizes (a copy).
    pub fn objects(&self) -> HashMap<ObjectKey, u64> {
        self.inner.state.lock().objects.clone()
    }

    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    pub fn policy(&self) -> Policy {
        self.inner.state.lock().policy.clone()
    }

    pub fn set_policy(&self, policy: Policy) {
        self.inner.state.lock().policy = sane(policy);
    }

    /// Room on the store's disk now.
    pub fn free_space(&self) -> Option<u64> {
        (self.inner.free)(&self.inner.root)
    }

    pub fn object_path(&self, key: &ObjectKey) -> PathBuf {
        object_dir(&self.inner.root, key).join(key.file_name())
    }

    /// Where a fetch keeps what has come so far: a separate folder, a name
    /// ending `.part`. Never handed out to be played.
    pub fn part_path(&self, key: &ObjectKey) -> PathBuf {
        self.inner.root.join(PARTIAL).join(part_name(key))
    }

    /// The current version's manifest.
    pub fn current(&self) -> Option<Manifest> {
        self.current_kept().map(|k| k.manifest.clone())
    }

    pub fn current_id(&self) -> Option<String> {
        self.inner.state.lock().history.last().cloned()
    }

    fn current_kept(&self) -> Option<Arc<Kept>> {
        let st = self.inner.state.lock();
        st.history
            .last()
            .and_then(|id| st.manifests.get(id).cloned())
    }

    /// The id a sync is under way (or was broken off) for.
    pub fn pending_id(&self) -> Option<String> {
        self.inner.state.lock().pending.clone()
    }

    /// The versions kept, newest last (the current one is last).
    pub fn kept_ids(&self) -> Vec<String> {
        let st = self.inner.state.lock();
        let keep = st.policy.keep;
        let h = &st.history;
        h[h.len().saturating_sub(keep)..].to_vec()
    }

    /// A manifest the store holds.
    pub fn manifest(&self, id: &str) -> Option<Manifest> {
        self.inner
            .state
            .lock()
            .manifests
            .get(id)
            .map(|k| k.manifest.clone())
    }

    /// Keep a manifest (written to the disk if it is new). Refused if its id
    /// doesn't match its content.
    pub fn add_manifest(&self, m: &Manifest) -> Result<(), String> {
        if !m.is_sound() {
            return Err("the list of media arrived damaged".into());
        }
        if self.inner.state.lock().manifests.contains_key(&m.id) {
            return Ok(());
        }
        let path = self.manifest_path(&m.id);
        manifest::write_atomically(&path, m.to_json().as_bytes())
            .map_err(|e| format!("can't keep the list of media: {e}"))?;
        self.inner
            .state
            .lock()
            .manifests
            .insert(m.id.clone(), Arc::new(Kept::new(m.clone())));
        self.drop_wrong_sizes(m);
        Ok(())
    }

    fn manifest_path(&self, id: &str) -> PathBuf {
        self.inner.root.join("manifests").join(format!("{id}.json"))
    }

    /// Mark the version a sync is fetching for (its objects are then safe
    /// from a tidy-up, even after a restart).
    pub fn begin(&self, m: &Manifest) -> Result<(), String> {
        self.add_manifest(m)?;
        let mut st = self.inner.state.lock();
        st.pending = Some(m.id.clone());
        drop(st);
        manifest::write_atomically(&self.inner.root.join("pending"), m.id.as_bytes())
            .map_err(|e| e.to_string())
    }

    /// Make a kept version the current one (after a sync, or going back to
    /// an earlier one). Its files may not all be here: `missing_for` says.
    pub fn make_current(&self, id: &str) -> Result<(), String> {
        let (history, pending_cleared) = {
            let mut st = self.inner.state.lock();
            if !st.manifests.contains_key(id) {
                return Err("that version isn't kept on this computer".into());
            }
            st.history.retain(|h| h != id);
            st.history.push(id.to_string());
            let cleared = st.pending.as_deref() == Some(id);
            if cleared {
                st.pending = None;
            }
            (st.history.clone(), cleared)
        };
        let root = &self.inner.root;
        let text = serde_json::to_vec(&history).unwrap_or_default();
        manifest::write_atomically(&root.join("history.json"), &text)
            .and_then(|_| manifest::write_atomically(&root.join("current"), id.as_bytes()))
            .map_err(|e| e.to_string())?;
        if pending_cleared {
            let _ = std::fs::remove_file(root.join("pending"));
        }
        Ok(())
    }

    /// Where this computer has the file a show path names, under the current
    /// version. None if it isn't here.
    pub fn resolve(&self, show_path: &str) -> Option<PathBuf> {
        let kept = self.current_kept()?;
        let e = kept.find(show_path).filter(|e| !e.missing)?;
        let key = e.object();
        // Only a whole, verified object: in the index only after its digest
        // matched and it was renamed into place, and the length it should have.
        let size = self.inner.state.lock().objects.get(&key).copied()?;
        (size == e.size).then(|| self.object_path(&key))
    }

    /// Whether the file a show path names is here, under the current version.
    /// Two lookups in memory: cheap enough for every frame.
    pub fn ready(&self, show_path: &str) -> Readiness {
        let st = self.inner.state.lock();
        let Some(kept) = st.history.last().and_then(|id| st.manifests.get(id)) else {
            return Readiness::NotInShow;
        };
        match kept.find(show_path) {
            None => Readiness::NotInShow,
            Some(e) if e.missing => Readiness::MissingOnPrimary,
            Some(e) if st.objects.get(&e.object()) == Some(&e.size) => Readiness::Here,
            Some(_) => Readiness::Missing,
        }
    }

    /// The entries of `m` this computer lacks (those the primary lacked too,
    /// marked `missing`).
    pub fn missing_for(&self, m: &Manifest) -> Vec<Entry> {
        let st = self.inner.state.lock();
        m.entries
            .iter()
            .filter(|e| e.missing || !st.objects.contains_key(&e.object()))
            .cloned()
            .collect()
    }

    /// Whether a verified object is here.
    pub fn has(&self, key: &ObjectKey) -> bool {
        self.inner.state.lock().objects.contains_key(key)
    }

    /// The same bytes (this digest and length) under any extension.
    pub fn any_with(&self, sha: &str, size: u64) -> Option<ObjectKey> {
        let st = self.inner.state.lock();
        let mut found: Vec<&ObjectKey> = st
            .objects
            .iter()
            .filter(|(k, s)| k.sha == sha && **s == size)
            .map(|(k, _)| k)
            .collect();
        found.sort();
        found.first().map(|k| (*k).clone())
    }

    /// The length of an object here.
    pub fn size_of(&self, key: &ObjectKey) -> Option<u64> {
        self.inner.state.lock().objects.get(key).copied()
    }

    /// Remove an object the calling thread has claimed.
    pub fn remove_object_claimed(&self, key: &ObjectKey) {
        let _ = std::fs::remove_file(self.object_path(key));
        self.inner.state.lock().objects.remove(key);
        self.bump();
    }

    /// Note that a verified object has been renamed into place.
    pub fn arrived(&self, key: &ObjectKey, size: u64) {
        self.inner.state.lock().objects.insert(key.clone(), size);
        self.bump();
    }

    /// Have an object to this thread alone, waiting while another has it.
    /// None if `cancel` is set meanwhile.
    pub fn claim(&self, key: &ObjectKey, cancel: &AtomicBool) -> Option<Claim> {
        let mut c = self.inner.claims.lock();
        while c.contains(key) {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return None;
            }
            self.inner
                .released
                .wait_for(&mut c, Duration::from_millis(200));
        }
        c.insert(key.clone());
        Some(Claim {
            inner: self.inner.clone(),
            key: key.clone(),
        })
    }

    /// Have it only if nobody else has it now.
    fn try_claim(&self, key: &ObjectKey) -> Option<Claim> {
        let mut c = self.inner.claims.lock();
        if !c.insert(key.clone()) {
            return None;
        }
        Some(Claim {
            inner: self.inner.clone(),
            key: key.clone(),
        })
    }

    /// Remove an object (not while another thread has it). Its size if it went.
    pub fn remove_object(&self, key: &ObjectKey) -> Option<u64> {
        let _claim = self.try_claim(key)?;
        let size = self.inner.state.lock().objects.get(key).copied()?;
        let path = self.object_path(key);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            // In use (Windows, a file being played): tried again next tidy-up.
            Err(_) => return None,
        }
        self.inner.state.lock().objects.remove(key);
        self.bump();
        Some(size)
    }

    /// The objects nothing may remove: everything the newest version names,
    /// and from the other versions kept, what isn't set to be replaced.
    /// `target` is a version on its way to being current.
    fn protected_by(st: &State, target: Option<&str>) -> HashSet<ObjectKey> {
        let keep = st.policy.keep;
        let mut ids: Vec<&str> = st
            .history
            .iter()
            .map(String::as_str)
            .filter(|h| Some(*h) != target)
            .collect();
        ids.extend(target);
        let ids = &ids[ids.len().saturating_sub(keep)..];
        let mut out = HashSet::new();
        let Some(newest) = ids.last().and_then(|id| st.manifests.get(*id)) else {
            return out;
        };
        out.extend(newest.manifest.present().map(Entry::object));
        // Which paths the newest version replaces (by its own size).
        let replaced_paths: HashSet<String> = newest
            .manifest
            .present()
            .filter(|e| st.policy.replaces(e.size))
            .map(Entry::key)
            .collect();
        for id in &ids[..ids.len() - 1] {
            let Some(k) = st.manifests.get(*id) else {
                continue;
            };
            for e in k.manifest.present() {
                if !st.policy.replaces(e.size) && !replaced_paths.contains(&e.key()) {
                    out.insert(e.object());
                }
            }
        }
        out
    }

    /// What may not be removed now: what the kept versions name, and with a
    /// sync under way, what its version names too.
    fn protected_now(st: &State) -> HashSet<ObjectKey> {
        let mut p = Store::protected_by(st, None);
        if let Some(t) = st.pending.clone() {
            p.extend(Store::protected_by(st, Some(&t)));
        }
        p
    }

    /// What a sync to `m` would need and free, worked out now.
    pub fn plan(&self, m: &Manifest) -> SyncPlan {
        let free = self.free_space();
        let st = self.inner.state.lock();
        let mut plan = SyncPlan {
            manifest_id: m.id.clone(),
            free,
            ..SyncPlan::default()
        };
        let mut by_key: HashMap<ObjectKey, Fetch> = HashMap::new();
        for e in &m.entries {
            if e.missing {
                plan.unavailable.push(e.clone());
                continue;
            }
            let key = e.object();
            if st.objects.contains_key(&key) {
                continue;
            }
            by_key
                .entry(key.clone())
                .or_insert_with(|| Fetch {
                    local: st
                        .objects
                        .iter()
                        .filter(|(k, s)| k.sha == key.sha && **s == e.size)
                        .map(|(k, _)| k)
                        .min()
                        .cloned(),
                    key,
                    size: e.size,
                    paths: Vec::new(),
                })
                .paths
                .push(e.path.clone());
        }
        let mut fetch: Vec<Fetch> = by_key.into_values().collect();
        fetch.sort_by(|a, b| a.paths.cmp(&b.paths));
        plan.bytes_to_write = fetch.iter().map(|f| f.size).sum();
        plan.bytes_to_fetch = fetch
            .iter()
            .filter(|f| f.local.is_none())
            .map(|f| f.size)
            .sum();

        // What is kept once `m` is current.
        let mut after = st.history.clone();
        after.retain(|h| h != &m.id);
        let mut after_state = State {
            policy: st.policy.clone(),
            history: after,
            pending: None,
            manifests: st.manifests.clone(),
            objects: HashMap::new(),
        };
        after_state
            .manifests
            .entry(m.id.clone())
            .or_insert_with(|| Arc::new(Kept::new(m.clone())));
        let keep_after = Store::protected_by(&after_state, Some(&m.id));
        plan.bytes_freed = st
            .objects
            .iter()
            .filter(|(k, _)| !keep_after.contains(*k))
            .map(|(_, s)| *s)
            .sum();

        // Each new object of a replaced path, and the old ones it replaces.
        let new_paths: HashMap<String, &Entry> = m.present().map(|e| (e.key(), e)).collect();
        for k in st.manifests.values() {
            if k.manifest.id == m.id {
                continue;
            }
            for old in k.manifest.present() {
                let Some(new) = new_paths.get(&old.key()) else {
                    continue;
                };
                let (ok, nk) = (old.object(), new.object());
                let replaced = st.policy.replaces(new.size) || st.policy.replaces(old.size);
                if ok != nk && replaced && !keep_after.contains(&ok) && st.objects.contains_key(&ok)
                {
                    let list = plan.replaces.entry(nk).or_default();
                    if !list.contains(&ok) {
                        list.push(ok);
                    }
                }
            }
        }

        // Room: what is written, plus what must stay free.
        let need = plan.bytes_to_write.saturating_add(st.policy.reserve);
        match free {
            None => plan.enough_room = true,
            Some(f) if f >= need || plan.bytes_to_write == 0 => plan.enough_room = true,
            Some(f) => {
                // What may go first: what nothing kept names, then old
                // versions of files set to replace. Never a file the current
                // version plays and keeps.
                let current: HashSet<ObjectKey> = st
                    .history
                    .last()
                    .and_then(|id| st.manifests.get(id))
                    .map(|k| k.manifest.present().map(Entry::object).collect())
                    .unwrap_or_default();
                let replaced_old: HashSet<&ObjectKey> = plan.replaces.values().flatten().collect();
                let mut can: Vec<(bool, u64, ObjectKey)> = st
                    .objects
                    .iter()
                    .filter(|(k, _)| !keep_after.contains(*k))
                    .filter(|(k, _)| !current.contains(*k) || replaced_old.contains(k))
                    .map(|(k, s)| (current.contains(k), *s, k.clone()))
                    .collect();
                // Unused first, then the largest.
                can.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
                let mut have = f;
                for (_, s, k) in can {
                    if have >= need {
                        break;
                    }
                    have = have.saturating_add(s);
                    plan.bytes_freed_first += s;
                    plan.free_first.push(k);
                }
                plan.enough_room = have >= need;
                if !plan.enough_room {
                    plan.free_first.clear();
                    plan.bytes_freed_first = 0;
                }
            }
        }
        plan.fetch = fetch;
        plan
    }

    /// Remove what no kept version needs: objects, broken-off fetches nobody
    /// is resuming or will want, and manifests past the number kept. Never
    /// anything else in the folder, and never what a thread has claimed.
    pub fn gc(&self) -> GcReport {
        let mut report = GcReport::default();
        let (protected, wanted, known) = {
            let mut st = self.inner.state.lock();
            let keep = st.policy.keep;
            let cut = st.history.len().saturating_sub(keep);
            st.history.drain(..cut);
            let protected = Store::protected_now(&st);
            // A part is worth keeping if a kept or pending version names it.
            let mut wanted: HashSet<ObjectKey> = protected.clone();
            for id in st.history.iter().chain(st.pending.iter()) {
                if let Some(k) = st.manifests.get(id) {
                    wanted.extend(k.manifest.present().map(Entry::object));
                }
            }
            // Only the versions kept and the one being fetched stay (a sync
            // that was given up leaves its manifest behind).
            let known: HashSet<String> = st
                .history
                .iter()
                .chain(st.pending.iter())
                .cloned()
                .collect();
            st.manifests.retain(|id, _| known.contains(id));
            (protected, wanted, known)
        };
        // Write the shortened history.
        let history = self.inner.state.lock().history.clone();
        let _ = manifest::write_atomically(
            &self.inner.root.join("history.json"),
            &serde_json::to_vec(&history).unwrap_or_default(),
        );
        if let Ok(rd) = std::fs::read_dir(self.inner.root.join("manifests")) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let Some(id) = name.strip_suffix(".json") else {
                    continue;
                };
                if super::paths::is_sha(id)
                    && !known.contains(id)
                    && std::fs::remove_file(e.path()).is_ok()
                {
                    report.manifests_removed += 1;
                }
            }
        }
        let (on_disk, parts) = scan_objects(&self.inner.root);
        for key in on_disk.keys() {
            if !protected.contains(key)
                && let Some(s) = self.remove_object(key)
            {
                report.objects_removed += 1;
                report.bytes_freed += s;
            }
        }
        for (key, path) in parts {
            if wanted.contains(&key) {
                continue;
            }
            if let Some(_claim) = self.try_claim(&key)
                && std::fs::remove_file(&path).is_ok()
            {
                report.parts_removed += 1;
            }
        }
        report
    }

    /// Look at the disk again (something outside NewQ may have removed files).
    pub fn rescan(&self) {
        let found = scan_objects(&self.inner.root).0;
        self.inner.state.lock().objects = found;
        self.bump();
    }

    /// Read every object again and remove those whose bytes don't match their
    /// name (a damaged disk, or a hand that changed one).
    pub fn verify(&self, cancel: &AtomicBool, progress: &Mutex<VerifyProgress>) -> VerifyReport {
        self.rescan();
        let mut list: Vec<(ObjectKey, u64)> = self
            .inner
            .state
            .lock()
            .objects
            .iter()
            .map(|(k, s)| (k.clone(), *s))
            .collect();
        list.sort();
        {
            let mut p = progress.lock();
            p.files_total = list.len();
            p.bytes_total = list.iter().map(|l| l.1).sum();
        }
        let mut report = VerifyReport::default();
        for (key, _) in list {
            let Some(claim) = self.claim(&key, cancel) else {
                report.cancelled = true;
                return report;
            };
            let path = self.object_path(&key);
            let mut add = |n: u64| progress.lock().bytes_done += n;
            match manifest::hash_file(&path, cancel, &mut add) {
                Ok(sha) if sha == key.sha => {}
                Ok(_) | Err(HashError::Io(_)) => {
                    let _ = std::fs::remove_file(&path);
                    self.inner.state.lock().objects.remove(&key);
                    self.bump();
                    report.bad.push(key.clone());
                }
                Err(HashError::Cancelled) => {
                    report.cancelled = true;
                    return report;
                }
            }
            drop(claim);
            report.checked += 1;
            progress.lock().files_done += 1;
        }
        report
    }
}

fn sane(mut p: Policy) -> Policy {
    p.keep = p.keep.max(1);
    p
}

fn object_dir(root: &Path, key: &ObjectKey) -> PathBuf {
    root.join("objects").join(&key.sha[..2])
}

fn load_manifest(root: &Path, id: &str) -> Option<Kept> {
    let text = std::fs::read(root.join("manifests").join(format!("{id}.json"))).ok()?;
    let m = Manifest::from_json(&text)?;
    (m.id == id).then(|| Kept::new(m))
}

/// The objects on the disk (by their names, with their sizes) and the
/// broken-off fetches.
type Scanned = (HashMap<ObjectKey, u64>, Vec<(ObjectKey, PathBuf)>);

fn scan_objects(root: &Path) -> Scanned {
    let mut objects = HashMap::new();
    let mut parts = Vec::new();
    let Ok(dirs) = std::fs::read_dir(root.join("objects")) else {
        return (objects, parts);
    };
    for d in dirs.flatten() {
        let dname = d.file_name().to_string_lossy().into_owned();
        if dname.len() != 2 || !d.path().is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(d.path()) else {
            continue;
        };
        for f in files.flatten() {
            let name = f.file_name().to_string_lossy().into_owned();
            let Ok(meta) = f.metadata() else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            if let Some(k) = ObjectKey::from_file_name(&name)
                && k.sha.starts_with(&dname)
            {
                objects.insert(k, meta.len());
            }
        }
    }
    if let Ok(files) = std::fs::read_dir(root.join(PARTIAL)) {
        for f in files.flatten() {
            let name = f.file_name().to_string_lossy().into_owned();
            if let Some(k) = part_key(&name) {
                parts.push((k, f.path()));
            }
        }
    }
    (objects, parts)
}

/// The folder broken-off fetches are kept in, away from the objects, so no
/// path to an object can ever lead to incomplete bytes.
const PARTIAL: &str = "partial";

/// A partial file's name: `<sha256>_<ext>.part` (never a media extension).
fn part_name(key: &ObjectKey) -> String {
    if key.ext.is_empty() {
        format!("{}.part", key.sha)
    } else {
        format!("{}_{}.part", key.sha, key.ext)
    }
}

fn part_key(name: &str) -> Option<ObjectKey> {
    let stem = name.strip_suffix(".part")?;
    let (sha, ext) = stem.split_once('_').unwrap_or((stem, ""));
    let k = ObjectKey {
        sha: sha.to_string(),
        ext: ext.to_string(),
    };
    (ObjectKey::from_file_name(&k.file_name()).as_ref() == Some(&k)).then_some(k)
}

/// A receiving computer serves what it has too, so any computer that has an
/// object can pass it on.
impl super::http::Provider for Store {
    fn current_manifest(&self) -> Option<Manifest> {
        self.current()
    }

    fn manifest(&self, id: &str) -> Option<Manifest> {
        Store::manifest(self, id)
    }

    fn object(&self, sha: &str) -> super::http::Served {
        let found = {
            let st = self.inner.state.lock();
            let mut keys: Vec<(&ObjectKey, &u64)> =
                st.objects.iter().filter(|(k, _)| k.sha == sha).collect();
            keys.sort();
            keys.first().map(|(k, s)| ((*k).clone(), **s))
        };
        match found {
            Some((key, size)) => super::http::Served::File {
                path: self.object_path(&key),
                size,
            },
            None => super::http::Served::NotHere,
        }
    }
}
