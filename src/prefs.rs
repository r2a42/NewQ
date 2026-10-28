//! Settings that belong to this computer, not to a show: kept as `prefs.json`
//! in the log folder beside the window layout.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Prefs {
    /// Let the graphics hardware decode video where it can (FFmpeg's
    /// `-hwaccel auto`, which falls back to the processor for what it can't).
    #[serde(default)]
    pub hw_decode: bool,
    /// Started in show mode (the show can't be edited), as it was left.
    #[serde(default)]
    pub show_mode: bool,
    /// Let send cues run programs on this computer. Off unless turned on here,
    /// since a show can come from anywhere.
    #[serde(default)]
    pub allow_scripts: bool,
}

const FILE: &str = "prefs.json";

impl Prefs {
    /// The saved settings; the defaults if there are none or they can't be read.
    pub fn load(dir: &Path) -> Prefs {
        std::fs::read_to_string(dir.join(FILE))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!("{FILE}.tmp"));
        std::fs::write(
            &tmp,
            serde_json::to_string(self).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(&tmp, dir.join(FILE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_saved_and_a_missing_or_damaged_file_gives_the_defaults() {
        let dir = std::env::temp_dir().join("newq_test_prefs");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(Prefs::load(&dir), Prefs::default());
        let p = Prefs {
            hw_decode: true,
            show_mode: true,
            allow_scripts: true,
        };
        p.save(&dir).unwrap();
        assert_eq!(Prefs::load(&dir), p);
        std::fs::write(dir.join(FILE), "not json").unwrap();
        assert_eq!(Prefs::load(&dir), Prefs::default());
        // A file from before a setting existed.
        std::fs::write(dir.join(FILE), "{}").unwrap();
        assert!(!Prefs::load(&dir).hw_decode);
        let _ = std::fs::remove_dir_all(dir);
    }
}
