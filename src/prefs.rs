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
    /// Let a show start a streaming server on this computer (a display's Send
    /// out set to run the server in NewQ). Turned on by ticking that box here;
    /// a show opened from somewhere else can't start one before that.
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_hosting: bool,
    /// The monitors Test Mode shows its tests on (each by its make and model, or
    /// the name the system gives it); none = the displays of the show.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub test_displays: Vec<String>,
    /// Let the computer sleep and the display switch off while outputs are
    /// open (NewQ holds it awake by default; see `awake.rs`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub let_computer_sleep: bool,
    /// Draw the output's frame rate on its picture (the Preview pane's checkbox).
    #[serde(default, skip_serializing_if = "is_false")]
    pub fps_on_output: bool,
    /// A presenter-style remote's buttons are acted on (see `clicker`), and the
    /// keys learned for its GO and Back buttons.
    #[serde(default, skip_serializing_if = "is_false")]
    pub clicker_on: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clicker_go: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clicker_back: Option<u32>,
    /// Test Mode plays a heavier lap.
    #[serde(default, skip_serializing_if = "is_false")]
    pub test_heavy: bool,
    /// The audio device Test Mode plays on; none = the show's own outputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_sound: Option<String>,
    /// Test Mode runs with the sound off (everything still mixed).
    #[serde(default, skip_serializing_if = "is_false")]
    pub test_muted: bool,
    /// The new interface (editor, operator and patch views) in place of the panes.
    #[serde(default, skip_serializing_if = "is_false")]
    pub new_layout: bool,
    /// How much the log says (see `logging`).
    #[serde(default, skip_serializing_if = "is_default_level")]
    pub log_level: crate::logging::LogLevel,
}

fn is_default_level(l: &crate::logging::LogLevel) -> bool {
    *l == crate::logging::LogLevel::default()
}

fn is_false(b: &bool) -> bool {
    !*b
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
            allow_hosting: true,
            test_displays: vec!["Dell U2720Q".into()],
            test_sound: Some("USB Interface".into()),
            test_muted: true,
            fps_on_output: true,
            let_computer_sleep: true,
            test_heavy: true,
            clicker_on: true,
            clicker_go: Some(0x51),
            clicker_back: Some(0x49),
            new_layout: true,
            log_level: crate::logging::LogLevel::Debug,
        };
        p.save(&dir).unwrap();
        assert_eq!(Prefs::load(&dir), p);
        std::fs::write(dir.join(FILE), "not json").unwrap();
        assert_eq!(Prefs::load(&dir), Prefs::default());
        // A file from before a setting existed.
        std::fs::write(dir.join(FILE), "{}").unwrap();
        assert!(!Prefs::load(&dir).hw_decode);
        // Defaults are not written, so older versions read the file as before.
        let json = serde_json::to_string(&Prefs::default()).unwrap();
        assert!(!json.contains("test_"), "{json}");
        assert!(!json.contains("new_layout"), "{json}");
        assert!(
            !json.contains("let_computer_sleep") && !json.contains("fps_on_output"),
            "{json}"
        );
        assert!(!json.contains("clicker"), "{json}");
        assert!(!json.contains("log_level"), "{json}");
        // The level is a word in the file, and one this version doesn't know
        // (from a newer one) gives the defaults, not a failure to start.
        assert!(
            serde_json::to_string(&p)
                .unwrap()
                .contains("\"log_level\":\"debug\"")
        );
        std::fs::write(
            dir.join(FILE),
            "{\"log_level\":\"trace\",\"hw_decode\":true}",
        )
        .unwrap();
        assert_eq!(Prefs::load(&dir).log_level, crate::logging::LogLevel::Trace);
        std::fs::write(dir.join(FILE), "{\"log_level\":\"chatty\"}").unwrap();
        assert_eq!(Prefs::load(&dir), Prefs::default());
        let _ = std::fs::remove_dir_all(dir);
    }
}
