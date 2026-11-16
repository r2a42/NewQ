//! Computers playing one show together (`.claude/notes/network-plan.md`).
//! Stage 1, the sync trial: a follower's estimate of the shared clock
//! (`clock`), the messages (`msg`), and a simulated network and computers
//! (`sim`) on which `trial` measures how closely engines ticked by the shared
//! clock (`Engine::tick_shared`) agree. Nothing in the app uses it yet.

pub mod clock;
pub mod msg;
#[cfg(test)]
mod sim;
pub mod store;
#[cfg(test)]
mod trial;
