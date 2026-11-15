//! Test Mode: a self-running test of this computer's playback, started from the
//! Help menu with no command line. `assets` makes the media ready (free
//! downloads, or generated), `plan` builds the three test shows, `session`
//! runs one while logging and puts the user's show back, `analysis` judges
//! the numbers, and `gui` is the window. Each level comes in two flavours:
//! the cue tests (a lap of cues handing over to each other) and the timeline
//! tests (`plan::build_timeline`, a timeline list and group that `timeline`
//! drives, pressing GO where a pause waits for it).

pub mod analysis;
pub mod assets;
pub mod bundle;
pub mod decode;
pub mod feature;
pub mod full;
pub mod gui;
pub mod plan;
pub mod selftest;
pub mod session;
pub mod timeline;

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

/// What a test plays: a lap of cues, or timelines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Flavour {
    #[default]
    Cues,
    Timeline,
}

impl Flavour {
    pub const ALL: [Flavour; 2] = [Flavour::Cues, Flavour::Timeline];

    /// What the test is called: "Medium", or "Timeline medium".
    pub fn name(self, level: Level) -> String {
        match self {
            Flavour::Cues => level.label().to_string(),
            Flavour::Timeline => format!("Timeline {}", level.label().to_lowercase()),
        }
    }
}
