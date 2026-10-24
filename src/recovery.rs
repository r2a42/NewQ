//! Recovery files: a copy of the show with unsaved changes, written every so
//! often, so a crash or power cut loses seconds of work rather than a session.
//!
//! A saved show's copy sits next to it (`show.newq` → `show.newq.recovery`),
//! so relative media paths still mean the same thing; an untitled show's goes
//! in the temporary folder. It is deleted when the show is saved, or when its
//! changes are deliberately discarded. If one is still there at the next start,
//! the program was not closed properly.

use crate::show::Show;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// How often a changed show is written out.
pub const INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

pub fn path_for(show_path: Option<&Path>) -> PathBuf {
    match show_path {
        Some(p) => {
            let mut name = p.file_name().unwrap_or_default().to_os_string();
            name.push(".recovery");
            p.with_file_name(name)
        }
        None => std::env::temp_dir().join("newq-untitled.newq.recovery"),
    }
}

pub fn write(show: &Show, show_path: Option<&Path>) -> anyhow::Result<()> {
    show.save(&path_for(show_path))
}

pub fn clear(show_path: Option<&Path>) {
    let _ = std::fs::remove_file(path_for(show_path));
}

/// Put a recovery copy aside (as `<name>.recovery.old`, replacing an earlier
/// one) rather than deleting it: it is declined when the question is answered
/// "No", but also when it couldn't be asked, and then it must not be lost.
pub fn set_aside(rec: &Path) {
    let mut name = rec.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    let _ = std::fs::rename(rec, rec.with_file_name(name));
}

/// A recovery file that is worth offering: it exists, and the saved show
/// (if there is one on disk) hasn't been saved since it was written.
pub fn pending(show_path: Option<&Path>) -> Option<PathBuf> {
    let rec = path_for(show_path);
    let rec_time = modified(&rec)?;
    match show_path.and_then(modified) {
        Some(saved) if saved >= rec_time => None,
        _ => Some(rec),
    }
}

fn modified(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::show::CueType;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("newq_recovery_{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_copy_sits_next_to_the_show() {
        let p = path_for(Some(Path::new("/shows/hamlet.newq")));
        assert_eq!(p, Path::new("/shows/hamlet.newq.recovery"));
        let u = path_for(None);
        assert!(u.starts_with(std::env::temp_dir()), "{u:?}");
        assert!(u.to_string_lossy().ends_with(".recovery"));
    }

    #[test]
    fn a_written_copy_is_offered_until_the_show_is_saved() {
        let d = dir("offer");
        let path = d.join("a.newq");
        let mut show = Show::default();
        show.save(&path).unwrap();
        assert_eq!(pending(Some(&path)), None, "nothing to recover yet");

        // Unsaved edits are written to the recovery file...
        std::thread::sleep(std::time::Duration::from_millis(50));
        let cue = show.make_cue(CueType::Wait, None, 0);
        show.cues.push(cue);
        write(&show, Some(&path)).unwrap();
        let rec = pending(Some(&path)).expect("a copy to offer");
        // ...which loads as the edited show, not the saved one.
        assert_eq!(Show::load(&rec).unwrap().cues.len(), 1);
        assert_eq!(Show::load(&path).unwrap().cues.len(), 0);

        // Saving the show afterwards makes the copy stale; clearing removes it.
        std::thread::sleep(std::time::Duration::from_millis(50));
        show.save(&path).unwrap();
        assert_eq!(pending(Some(&path)), None);
        write(&show, Some(&path)).unwrap();
        clear(Some(&path));
        assert!(!rec.exists());
        // Clearing what isn't there is fine.
        clear(Some(&path));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_declined_copy_is_kept_aside_not_deleted() {
        let d = dir("aside");
        let path = d.join("a.newq");
        Show::default().save(&path).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        write(&Show::default(), Some(&path)).unwrap();
        let rec = pending(Some(&path)).unwrap();
        set_aside(&rec);
        assert!(!rec.exists() && pending(Some(&path)).is_none());
        assert!(d.join("a.newq.recovery.old").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_untitled_show_is_always_offered_its_copy() {
        clear(None);
        assert_eq!(pending(None), None);
        write(&Show::default(), None).unwrap();
        assert_eq!(pending(None), Some(path_for(None)));
        clear(None);
        assert_eq!(pending(None), None);
    }
}
