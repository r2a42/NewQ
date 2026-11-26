//! A show file saved by a newer NewQ than this one (R4): the operator is told,
//! in a strip over the window that doesn't stop anything, and the file is kept
//! as it is. Saving here could lose what this NewQ doesn't know of it
//! (`show::compat`), so Save asks for a new name until the show has been saved
//! somewhere (a "Save as" over the file itself, chosen in the dialog, is the
//! operator's word). The recovery copy is written beside the file, never over
//! it, and a show taken back from it is still the file's: the protection is
//! the file's path, not anything in the show, so undo, a crash and its
//! recovery, and a Test Mode run in between all keep it.

use super::{Action, UiState, settings};
use crate::engine::Engine;
use crate::show::{Show, THIS_NEWQ, compat};
use egui::{RichText, Ui};
use std::path::{Path, PathBuf};

/// The file that was opened, and the NewQ that saved it.
#[derive(Clone, Debug, PartialEq)]
pub struct NewerFile {
    pub path: PathBuf,
    pub saved_with: String,
    /// The strip was closed (the protection stays).
    pub hidden: bool,
}

/// A show was opened from `path` (None: a new show, or one not from a file):
/// note whether a newer NewQ saved it, and say so.
pub fn opened(st: &mut UiState, show: &Show, path: Option<&Path>, engine: &mut Engine) {
    st.newer_file = path.zip(show.saved_with.as_deref()).and_then(|(path, by)| {
        compat::newer_than_this(by).then(|| NewerFile {
            path: path.to_path_buf(),
            saved_with: by.to_string(),
            hidden: false,
        })
    });
    let unknown = show.cues.iter().filter(|c| !c.kind.is_known()).count();
    if let Some(n) = &st.newer_file {
        engine.problem(format!(
            "{} was saved by NewQ {}, newer than this one ({THIS_NEWQ}): Save will ask for a \
             new name, so that it is kept as it is",
            file_name(&n.path),
            n.saved_with
        ));
    }
    if unknown > 0 {
        engine.problem(format!(
            "{unknown} cue(s) of this show can't be read by this NewQ: they are kept, and \
             can't play here"
        ));
    }
}

/// Where a plain Save writes: the show's file, unless that is a file a newer
/// NewQ saved (then nowhere until the operator names one: None, as for a show
/// that has no file yet).
pub fn save_target(st: &UiState) -> Option<PathBuf> {
    let path = st.show_path.as_ref()?;
    (!protects(st, path)).then(|| path.clone())
}

/// Whether `path` is the newer NewQ's file, kept from a plain Save.
pub fn protects(st: &UiState, path: &Path) -> bool {
    st.newer_file.as_ref().is_some_and(|n| n.path == path)
}

/// The show has been saved: whatever happened to the newer file (left where it
/// was, or replaced in a Save as the operator chose), there is nothing more to
/// keep it from.
pub fn saved(st: &mut UiState) {
    st.newer_file = None;
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// The strip over the window, while the show is the newer file's.
pub fn strip(ui: &mut Ui, st: &mut UiState, actions: &mut Vec<Action>) {
    let Some(n) = st.newer_file.clone() else {
        return;
    };
    if n.hidden || st.show_path.as_deref() != Some(n.path.as_path()) {
        return;
    }
    egui::Panel::top("newer_file").show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!(
                    "⚠ {} was saved by NewQ {}, newer than this one ({THIS_NEWQ}). What this \
                     NewQ doesn't know of it is kept, and its cues of kinds this NewQ doesn't \
                     know can't play here. Saving here could still lose some of it, so Save \
                     asks for a new name and the file is kept as it is.",
                    file_name(&n.path),
                    n.saved_with
                ))
                .strong(),
            );
            if ui.button(settings::SAVE_AS_ITEM.label).clicked() {
                actions.push(Action::SaveAs);
            }
            if ui.small_button("✖").on_hover_text("Hide this").clicked()
                && let Some(n) = &mut st.newer_file
            {
                n.hidden = true;
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn newer_show(by: &str) -> Show {
        Show {
            saved_with: Some(by.into()),
            ..Show::default()
        }
    }

    #[test]
    fn a_show_saved_by_a_newer_newq_is_told_of_and_kept_from_a_plain_save() {
        let path = PathBuf::from("/shows/newer.newq");
        let (mut e, mut st) = (Engine::for_tests(), UiState::default());
        st.show_path = Some(path.clone());
        opened(&mut st, &newer_show("99.0.0"), Some(&path), &mut e);
        let n = st.newer_file.clone().expect("noted");
        assert_eq!(
            (n.path.as_path(), n.saved_with.as_str()),
            (path.as_path(), "99.0.0")
        );
        assert!(
            e.messages
                .iter()
                .any(|(_, m)| m.contains("saved by NewQ 99.0.0, newer than this one")),
            "{:?}",
            e.messages
        );
        // The first Save asks where (as Save as does)...
        assert_eq!(save_target(&st), None);
        // ...and once saved elsewhere, Save saves there.
        st.show_path = Some("/shows/mine.newq".into());
        saved(&mut st);
        assert_eq!(save_target(&st), st.show_path);
        // This NewQ's own, an older one's and one from before the field: no word.
        for by in [Some(THIS_NEWQ), Some("0.20.0"), None] {
            let show = Show {
                saved_with: by.map(String::from),
                ..Show::default()
            };
            let mut e = Engine::for_tests();
            opened(&mut st, &show, Some(&path), &mut e);
            assert_eq!(st.newer_file, None, "{by:?}");
            assert!(e.messages.is_empty(), "{by:?}: {:?}", e.messages);
        }
        // A newer show that isn't from a file (a new one): nothing to keep.
        opened(&mut st, &newer_show("99.0.0"), None, &mut e);
        assert_eq!(st.newer_file, None);
    }
}
