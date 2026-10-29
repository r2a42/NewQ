//! Test Mode: a self-running test of this computer's playback, started from the
//! Help menu with no command line. `assets` makes the media ready (free
//! downloads, or generated), `plan` builds the three test shows, `session`
//! runs one while logging and puts the user's show back, `analysis` judges
//! the numbers, and `gui` is the window.

pub mod analysis;
pub mod assets;
pub mod gui;
pub mod plan;
pub mod session;

pub use session::TestMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Short,
    Medium,
    Long,
}

impl Level {
    pub const ALL: [Level; 3] = [Level::Short, Level::Medium, Level::Long];

    pub fn label(self) -> &'static str {
        match self {
            Level::Short => "Short",
            Level::Medium => "Medium",
            Level::Long => "Long",
        }
    }

    pub fn bit(self) -> u8 {
        match self {
            Level::Short => 1,
            Level::Medium => 2,
            Level::Long => 4,
        }
    }
}
