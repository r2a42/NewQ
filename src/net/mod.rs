//! Computers playing one show together (`.claude/notes/network-plan.md`).
//! Computers and roles: `node` (identity and the rules), `proto` (frames and
//! messages), `session`, `discover` (the beacon) and `hub` (the thread that
//! runs them). The shared clock (`clock`, `msg`; `sim` and `trial` measure it
//! on a simulated network) and playing together (`play`, `time`) are used by
//! the app (`app/play.rs`). The media store (`store`) is run in the app by the
//! media manager (`media`).

pub mod clock;
pub mod discover;
pub mod hub;
// The network test's measurements (stage 6).
pub mod measure;
pub mod media;
pub mod msg;
pub mod node;
// The computers playing together (stage 4a): the shared clock in the app,
// commands, the heartbeat and show sync.
pub mod play;
pub mod proto;
pub mod session;
#[cfg(test)]
mod sim;
pub mod store;
pub mod time;
#[cfg(test)]
mod trial;
