//! Computers playing one show together (`.claude/notes/network-plan.md`).
//! Computers and roles: `node` (identity and the rules), `proto` (frames and
//! messages), `session`, `discover` (the beacon) and `hub` (the thread that
//! runs them). The shared clock (`clock`, `msg`; `sim` and `trial` measure it
//! on a simulated network) and the media store (`store`) are not used by the
//! app yet.

// Not used by the app yet (the sync trial, stage 1).
#[cfg_attr(not(test), allow(dead_code))]
pub mod clock;
pub mod discover;
pub mod hub;
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
