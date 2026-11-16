//! The media store: every computer of a show network holds all the show's
//! media (owner's ruling R4), found by the same show path the primary uses.
//!
//! - `manifest`: what a show version is made of (path, size, SHA-256), made
//!   on a thread with a hash cache; used files and the library tier.
//! - `local`: a receiving computer's store (objects by digest, versions kept
//!   or replaced, tidy-up, a check of every object, readiness).
//! - `primary`: the primary serves its files from where they are.
//! - `http`: the server (`/manifest/current`, `/manifest/<id>`,
//!   `/object/<sha256>` with `Range`) and the client.
//! - `fetch`: one object, resumed and verified, from whichever computer has it.
//! - `pull`: the long-running fetcher, in the caller's order, with progress.
//!
//! Rulings R13 to R16: a transfer never blocks rehearsal (every question is
//! answered at once from what is known), the order follows the playhead
//! without thrash, the library comes after the used files, and the primary
//! can tell that transfers are running.

// Nothing in the app calls the store yet: the next stage (show sync) wires
// it in.
#![allow(dead_code, unused_imports)]

pub mod fetch;
pub mod http;
pub mod local;
pub mod manifest;
pub mod paths;
pub mod primary;
pub mod pull;
pub mod rate;
pub mod space;

#[cfg(test)]
mod tests;

pub use fetch::{FetchCtx, FetchError, fetch_object};
pub use http::{Provider, ServeLimits, Served, Server, Timeouts, Transfers};
pub use local::{GcReport, Policy, Readiness, Store, SyncPlan, VerifyReport};
pub use manifest::{
    Entry, HashCache, HashProgress, Hashing, Manifest, ObjectKey, Sources, media_in_folder,
    media_paths,
};
pub use primary::Primary;
pub use pull::{
    Availability, FileProgress, Follow, Progress, PullOptions, PullSnapshot, Puller, Tier,
};
