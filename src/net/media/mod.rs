//! The media manager: the store (`net::store`) run in the app (stage 2b's
//! media half; rulings R4, R13 to R20 of `.claude/notes/network-plan.md`).
//!
//! - On the primary (`manager::Manager`, primary side): the show's media and
//!   the library folders are listed and hashed on a thread whenever they
//!   change (a hash cache beside the log keeps a restart from reading them
//!   again), with what FFprobe reads from each used file (its length, so a
//!   computer that hasn't the file yet keeps the cue's timing); the version is
//!   published and served, and the files are watched for edits. The order the
//!   others fetch in is worked out here for everyone (`order`), from the
//!   playheads and the running timelines.
//! - On every other computer of the network: a store beside the log, a
//!   `Puller` following the primary (and the computers that have the files),
//!   and a server of its own store for the others.
//! - `local_path`: where this computer has the file a show path names. Alone
//!   or on the primary it is the path itself; on another computer of the
//!   network it is the store's verified object, or why it isn't here yet.
//!   **A partial file is never handed out** (R20): only `Store::resolve`'s
//!   answer is.
//!
//! Nothing here waits on a disk hash, a transfer or the network on the main
//! thread; every answer is from memory.

pub mod card;
pub mod manager;
pub mod order;
pub mod words;

#[cfg(test)]
mod tests;

pub use manager::{Manager, MediaReport, MediaView};
pub use order::{Positions, order};

use super::store::{Availability, Puller, Store, paths};
use parking_lot::RwLock;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Where this computer has the file a show names.
#[derive(Clone, Debug, PartialEq)]
pub enum Local {
    /// Here, whole and verified: play this.
    Here(PathBuf),
    /// On its way (or waiting its turn): a card says so in the cue's place.
    Transferring(Transfer),
    /// No computer has it (missing on the primary too).
    NotFound,
}

impl Local {
    pub fn here(&self) -> Option<&Path> {
        match self {
            Local::Here(p) => Some(p),
            _ => None,
        }
    }

    pub fn is_here(&self) -> bool {
        matches!(self, Local::Here(_))
    }
}

/// A file on its way: how far along it is (None: not known yet, the list of
/// media hasn't come from the primary).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transfer {
    pub done: u64,
    pub total: u64,
    /// Seconds to go at its own rate, once known.
    pub eta_secs: Option<f64>,
    /// The list of media is known (else everything is "waiting").
    pub known: bool,
}

impl Transfer {
    pub const WAITING: Transfer = Transfer {
        done: 0,
        total: 0,
        eta_secs: None,
        known: false,
    };

    /// Percent, by bytes (never 100 until it is here).
    pub fn percent(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        (self.done as f64 * 100.0 / self.total as f64).min(99.0)
    }
}

/// Answers `local_path` on a computer that takes its media from a primary.
pub trait Finder: Send + Sync {
    fn local(&self, show_path: &Path) -> Local;
    /// What the primary read from the file (its length), if it has said.
    fn facts(&self, show_path: &Path) -> Option<super::store::manifest::Facts>;
    /// Changes whenever an answer of `local` may have changed (a file came,
    /// a new version was taken up).
    fn generation(&self) -> u64;
}

/// Whether a finder is in place (read first, so that alone it costs one load).
static ACTIVE: AtomicBool = AtomicBool::new(false);
static FINDER: RwLock<Option<Arc<dyn Finder>>> = RwLock::new(None);

/// Where this computer has the file `path` names. Alone or on the primary:
/// the path itself, at the cost of one atomic load and a copy of the path.
pub fn local_path(path: &Path) -> Local {
    if !ACTIVE.load(Ordering::Acquire) {
        return Local::Here(path.to_path_buf());
    }
    match finder() {
        Some(f) => f.local(path),
        None => Local::Here(path.to_path_buf()),
    }
}

/// The finder in place, if this computer takes its media from a primary.
pub fn finder() -> Option<Arc<dyn Finder>> {
    if !ACTIVE.load(Ordering::Acquire) {
        return None;
    }
    FINDER.read().clone()
}

/// Put a finder in place (a computer joining a show network as a follower),
/// or take it away (None: back to the paths themselves).
pub fn set_finder(f: Option<Arc<dyn Finder>>) {
    let on = f.is_some();
    *FINDER.write() = f;
    ACTIVE.store(on, Ordering::Release);
}

/// The finder of a follower: the puller's knowledge and the store's verified
/// objects. Until the puller is running (the primary's server not yet heard
/// of), everything is on its way.
#[derive(Default)]
pub struct StoreFinder {
    inner: RwLock<Option<Arc<Puller>>>,
    /// Bumped when the puller is replaced.
    swaps: std::sync::atomic::AtomicU64,
}

impl StoreFinder {
    pub fn set_puller(&self, p: Option<Arc<Puller>>) {
        *self.inner.write() = p;
        self.swaps.fetch_add(1, Ordering::SeqCst);
    }

    fn puller(&self) -> Option<Arc<Puller>> {
        self.inner.read().clone()
    }
}

/// What a store and its puller say about a show path.
pub fn local_in(puller: &Puller, show_path: &Path) -> Local {
    let text = paths::path_text(show_path);
    match puller.availability(&text) {
        // Only the store's verified object, never anything else.
        Availability::Here => match puller.store().resolve(&text) {
            Some(p) => Local::Here(p),
            None => Local::Transferring(Transfer::WAITING),
        },
        Availability::Transferring { done, total } => {
            let eta_secs = puller.file_progress(&text).and_then(|f| f.eta_secs);
            Local::Transferring(Transfer {
                done,
                total,
                eta_secs,
                known: true,
            })
        }
        Availability::NotFound => Local::NotFound,
        Availability::NotInShow => Local::Transferring(Transfer::WAITING),
    }
}

impl Finder for StoreFinder {
    fn local(&self, show_path: &Path) -> Local {
        match self.puller() {
            Some(p) => local_in(&p, show_path),
            None => Local::Transferring(Transfer::WAITING),
        }
    }

    fn facts(&self, show_path: &Path) -> Option<super::store::manifest::Facts> {
        let p = self.puller()?;
        let m = p.store().current()?;
        m.find(&paths::path_text(show_path))?.facts.clone()
    }

    fn generation(&self) -> u64 {
        let swaps = self.swaps.load(Ordering::SeqCst);
        let Some(p) = self.puller() else {
            return swaps;
        };
        let store = p.store();
        let id = store.current_id().unwrap_or_default();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        use std::hash::{Hash, Hasher};
        (swaps, store.generation(), id).hash(&mut h);
        h.finish()
    }
}

/// A follower's store, served to the other computers (so they can fetch from
/// it as well as from the primary).
pub struct StoreServes(pub Store);

impl super::store::Provider for StoreServes {
    fn current_manifest(&self) -> Option<super::store::Manifest> {
        self.0.current()
    }

    fn manifest(&self, id: &str) -> Option<super::store::Manifest> {
        self.0.manifest(id)
    }

    fn object(&self, sha: &str) -> super::store::Served {
        let found = self
            .0
            .objects()
            .into_iter()
            .filter(|(k, _)| k.sha == sha)
            .min_by(|a, b| a.0.cmp(&b.0));
        match found {
            Some((k, size)) => super::store::Served::File {
                path: self.0.object_path(&k),
                size,
            },
            None => super::store::Served::NotHere,
        }
    }
}
