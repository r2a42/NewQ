//! Getting one object from the computers that have it: resume its `.part`,
//! check the digest as the bytes arrive, rename into place only when it
//! matches, and move on to the next computer when one fails.

use super::http::{self, NetError, Timeouts};
use super::local::Store;
use super::manifest::{self, ObjectKey};
use super::rate::{self, RateCap};
use super::space;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Why getting an object failed.
#[derive(Clone, Debug, PartialEq)]
pub enum FetchError {
    Cancelled,
    /// No room: the words say how much was wanted.
    DiskFull(String),
    /// The bytes didn't match (or the file changed where it was): a newer
    /// version of the show is to be asked for.
    Changed(String),
    /// No computer has it.
    NotFound,
    Network(String),
    /// This computer's disk couldn't be written (not for want of room).
    Disk(String),
}

impl FetchError {
    pub fn words(&self) -> String {
        match self {
            FetchError::Cancelled => "stopped".into(),
            FetchError::DiskFull(s)
            | FetchError::Changed(s)
            | FetchError::Network(s)
            | FetchError::Disk(s) => s.clone(),
            FetchError::NotFound => "no computer has this file".into(),
        }
    }
}

/// What one fetch works with.
pub struct FetchCtx<'a> {
    pub store: &'a Store,
    /// "host:port" of the computers that may have it, the likeliest first.
    pub sources: &'a [String],
    pub timeouts: Timeouts,
    pub cap: Option<&'a RateCap>,
    pub stop: &'a AtomicBool,
    /// Bytes of the object here so far (the `.part`'s length).
    pub done: &'a AtomicU64,
    /// Bytes that came over the network (all fetches add to it).
    pub received: &'a AtomicU64,
}

const CHUNK: usize = 256 * 1024;

/// One source's failure.
enum Src {
    Net(NetError),
    Status(u16),
    Mismatch,
    DiskFull(String),
    Disk(String),
}

fn written(e: std::io::Error, part: &Path) -> Src {
    use std::io::ErrorKind::{QuotaExceeded, StorageFull};
    if matches!(e.kind(), StorageFull | QuotaExceeded) {
        Src::DiskFull("this computer's disk is full".into())
    } else {
        Src::Disk(format!("can't write {}: {e}", part.display()))
    }
}

/// Get one object into the store. Another thread getting the same one is
/// waited for (and then it is here). The `.part` is kept when stopped or when
/// the network fails, so a later try resumes it.
pub fn fetch_object(ctx: &FetchCtx, key: &ObjectKey, size: u64) -> Result<(), FetchError> {
    let _claim = ctx
        .store
        .claim(key, ctx.stop)
        .ok_or(FetchError::Cancelled)?;
    match ctx.store.size_of(key) {
        Some(s) if s == size => {
            ctx.done.store(size, Ordering::Relaxed);
            return Ok(());
        }
        // Here but not the length wanted: not these bytes after all.
        Some(_) => {
            ctx.store.remove_object_claimed(key);
        }
        None => {}
    }
    let part = ctx.store.part_path(key);
    for dir in [part.parent(), ctx.store.object_path(key).parent()]
        .into_iter()
        .flatten()
    {
        std::fs::create_dir_all(dir).map_err(|e| FetchError::Disk(e.to_string()))?;
    }
    // The same bytes under another extension: a copy here, no network.
    if let Some(other) = ctx.store.any_with(&key.sha, size) {
        return copy_local(ctx, &other, key, size, &part);
    }
    let mut have = std::fs::metadata(&part).map_or(0, |m| m.len());
    if have > size {
        let _ = std::fs::remove_file(&part);
        have = 0;
    }
    let reserve = ctx.store.policy().reserve;
    if let Some(free) = ctx.store.free_space() {
        let need = (size - have).saturating_add(reserve);
        if free < need {
            return Err(FetchError::DiskFull(format!(
                "this computer's disk is too full: {} needed, {} free",
                space::words(size - have),
                space::words(free.saturating_sub(reserve))
            )));
        }
    }
    // What is there already goes through the digest first.
    let mut hasher = Sha256::new();
    if have > 0 && !hash_prefix(&part, have, &mut hasher, ctx.stop)? {
        let _ = std::fs::remove_file(&part);
        have = 0;
        hasher = Sha256::new();
    }
    ctx.done.store(have, Ordering::Relaxed);
    if size == 0 {
        // An empty file: nothing to fetch, only to make.
        std::fs::File::create(&part).map_err(|e| FetchError::Disk(e.to_string()))?;
        if manifest::hex(&hasher.finalize()) != key.sha {
            let _ = std::fs::remove_file(&part);
            return Err(FetchError::Changed(
                "the file changed on the main computer".into(),
            ));
        }
        std::fs::rename(&part, ctx.store.object_path(key))
            .map_err(|e| FetchError::Disk(e.to_string()))?;
        ctx.store.arrived(key, 0);
        return Ok(());
    }
    if ctx.sources.is_empty() {
        return Err(FetchError::Network(
            "no other computer to fetch it from".into(),
        ));
    }
    let mut changed: Option<String> = None;
    let mut not_found = 0;
    let mut last: Option<String> = None;
    for src in ctx.sources {
        match from_source(ctx, src, key, size, &part, &mut have, &mut hasher) {
            Ok(()) => {
                if manifest::hex(&hasher.clone().finalize()) == key.sha {
                    let object = ctx.store.object_path(key);
                    std::fs::rename(&part, &object)
                        .map_err(|e| FetchError::Disk(format!("can't keep the file: {e}")))?;
                    ctx.store.arrived(key, size);
                    return Ok(());
                }
                // Not the bytes wanted: start again from the next computer.
                let _ = std::fs::remove_file(&part);
                have = 0;
                hasher = Sha256::new();
                ctx.done.store(0, Ordering::Relaxed);
                changed = Some(
                    "the copy didn't match the original (it changed, or was damaged on the way)"
                        .into(),
                );
            }
            Err(Src::Net(NetError::Cancelled)) => return Err(FetchError::Cancelled),
            Err(Src::Net(e)) => last = Some(e.words()),
            Err(Src::Status(404)) => not_found += 1,
            Err(Src::Status(410) | Src::Mismatch) => {
                changed = Some("the file changed on the main computer".into());
            }
            Err(Src::Status(s)) => last = Some(NetError::Status(s).words()),
            Err(Src::DiskFull(m)) => {
                // Give the room back: the show needs it more.
                let _ = std::fs::remove_file(&part);
                ctx.done.store(0, Ordering::Relaxed);
                return Err(FetchError::DiskFull(m));
            }
            Err(Src::Disk(m)) => return Err(FetchError::Disk(m)),
        }
    }
    if let Some(c) = changed {
        return Err(FetchError::Changed(c));
    }
    if not_found == ctx.sources.len() {
        return Err(FetchError::NotFound);
    }
    Err(FetchError::Network(last.unwrap_or_else(|| {
        "couldn't reach any computer that has it".into()
    })))
}

/// Feed the first `len` bytes of `part` to the digest. False if they
/// couldn't be read (the part is then started again).
fn hash_prefix(
    part: &Path,
    len: u64,
    hasher: &mut Sha256,
    stop: &AtomicBool,
) -> Result<bool, FetchError> {
    let Ok(f) = std::fs::File::open(part) else {
        return Ok(false);
    };
    let mut f = f.take(len);
    let mut buf = vec![0u8; CHUNK];
    let mut read = 0u64;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err(FetchError::Cancelled);
        }
        match f.read(&mut buf) {
            Ok(0) => return Ok(read == len),
            Ok(n) => {
                hasher.update(&buf[..n]);
                read += n as u64;
            }
            Err(_) => return Ok(false),
        }
    }
}

fn from_source(
    ctx: &FetchCtx,
    src: &str,
    key: &ObjectKey,
    size: u64,
    part: &Path,
    have: &mut u64,
    hasher: &mut Sha256,
) -> Result<(), Src> {
    if *have == size {
        // All here already (an earlier try stopped just before its check).
        return Ok(());
    }
    let mut resp = http::get(
        src,
        &format!("/object/{}", key.sha),
        *have,
        &ctx.timeouts,
        ctx.stop,
    )
    .map_err(Src::Net)?;
    let fresh = match resp.status {
        206 if *have > 0 => {
            let range = resp
                .headers
                .get("content-range")
                .cloned()
                .unwrap_or_default();
            let expect = format!("bytes {}-", *have);
            if !range.starts_with(&expect) || !range.ends_with(&format!("/{size}")) {
                return Err(Src::Mismatch);
            }
            false
        }
        200 => {
            if resp.length().is_some_and(|l| l != size) {
                return Err(Src::Mismatch);
            }
            true
        }
        // Theirs is shorter than the part: not the same file.
        416 => {
            let _ = std::fs::remove_file(part);
            *have = 0;
            *hasher = Sha256::new();
            ctx.done.store(0, Ordering::Relaxed);
            return Err(Src::Mismatch);
        }
        s => return Err(Src::Status(s)),
    };
    let mut file = if fresh || *have == 0 {
        *have = 0;
        *hasher = Sha256::new();
        ctx.done.store(0, Ordering::Relaxed);
        std::fs::File::create(part).map_err(|e| written(e, part))?
    } else {
        std::fs::OpenOptions::new()
            .append(true)
            .open(part)
            .map_err(|e| written(e, part))?
    };
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = resp.read(&mut buf, ctx.stop).map_err(Src::Net)?;
        if n == 0 {
            break;
        }
        if *have + n as u64 > size {
            return Err(Src::Mismatch);
        }
        if !rate::take(ctx.cap, n as u64, ctx.stop) {
            return Err(Src::Net(NetError::Cancelled));
        }
        file.write_all(&buf[..n]).map_err(|e| written(e, part))?;
        hasher.update(&buf[..n]);
        *have += n as u64;
        ctx.done.store(*have, Ordering::Relaxed);
        ctx.received.fetch_add(n as u64, Ordering::Relaxed);
    }
    file.flush().map_err(|e| written(e, part))?;
    file.sync_all().map_err(|e| written(e, part))?;
    if *have != size {
        return Err(Src::Net(NetError::Broken("it ended early".into())));
    }
    Ok(())
}

/// The same bytes are here under another name: copy them (checked too).
fn copy_local(
    ctx: &FetchCtx,
    from: &ObjectKey,
    to: &ObjectKey,
    size: u64,
    part: &Path,
) -> Result<(), FetchError> {
    let disk = |r: Src| match r {
        Src::DiskFull(m) => FetchError::DiskFull(m),
        Src::Disk(m) => FetchError::Disk(m),
        _ => FetchError::Disk("can't copy".into()),
    };
    if let Some(free) = ctx.store.free_space()
        && free < size.saturating_add(ctx.store.policy().reserve)
    {
        return Err(FetchError::DiskFull(
            "this computer's disk is too full".into(),
        ));
    }
    let mut src = std::fs::File::open(ctx.store.object_path(from))
        .map_err(|e| FetchError::Disk(e.to_string()))?;
    let mut out = std::fs::File::create(part).map_err(|e| disk(written(e, part)))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    loop {
        if ctx.stop.load(Ordering::Relaxed) {
            return Err(FetchError::Cancelled);
        }
        let n = src
            .read(&mut buf)
            .map_err(|e| FetchError::Disk(e.to_string()))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])
            .map_err(|e| disk(written(e, part)))?;
        hasher.update(&buf[..n]);
        done += n as u64;
        ctx.done.store(done, Ordering::Relaxed);
    }
    out.sync_all().map_err(|e| disk(written(e, part)))?;
    drop(out);
    if done != size || manifest::hex(&hasher.finalize()) != to.sha {
        let _ = std::fs::remove_file(part);
        return Err(FetchError::Changed(
            "the copy here didn't match (damaged on this disk)".into(),
        ));
    }
    std::fs::rename(part, ctx.store.object_path(to))
        .map_err(|e| FetchError::Disk(e.to_string()))?;
    ctx.store.arrived(to, size);
    Ok(())
}
