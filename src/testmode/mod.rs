//! Test Mode: a self-running test of this computer's playback, started from the
//! Help menu with no command line. See `assets` for the media, and the
//! README for what the three levels do.

pub mod assets;
pub mod plan;

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

/// Everything Test Mode keeps (a field of the interface state).
#[derive(Default)]
pub struct TestMode {
    pub open: bool,
}
