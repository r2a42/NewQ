//! Computers playing one show together (`.claude/notes/network-plan.md`).
//! Computers and roles: `node` (identity and the rules), `proto` (frames and
//! messages), `session`, `discover` (the beacon) and `hub` (the thread that
//! runs them). The shared clock (`clock`, `msg`; `sim` and `trial` measure it
//! on a simulated network) is not used by the app yet. The media store
//! (`store`) is run in the app by the media manager (`media`).

// Not used by the app yet (the sync trial, stage 1).
#[cfg_attr(not(test), allow(dead_code))]
pub mod clock;
pub mod discover;
pub mod hub;
pub mod media;
#[cfg_attr(not(test), allow(dead_code))]
pub mod msg;
pub mod node;
pub mod proto;
pub mod session;
#[cfg(test)]
mod sim;
pub mod store;
#[cfg(test)]
mod trial;
