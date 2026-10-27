//! Where the panes of the main window are: docked in a tiled layout that
//! resizes neighbours as one grows or shrinks (so they never overlap), popped
//! out into windows of their own, or hidden. The layout is the user's, kept
//! per computer rather than in the show, since monitors differ.

use egui_dock::{DockState, NodeIndex, TabPath};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A part of the main window that can be moved, popped out or hidden.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Pane {
    Cues,
    Inspector,
    Cart,
    Preview,
    Active,
    Settings,
}

impl Pane {
    pub const ALL: [Pane; 6] = [
        Pane::Cues,
        Pane::Inspector,
        Pane::Cart,
        Pane::Preview,
        Pane::Active,
        Pane::Settings,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Pane::Cues => "Cue list",
            Pane::Inspector => "Inspector",
            Pane::Cart => "Cart",
            Pane::Preview => "Preview",
            Pane::Active => "Active",
            Pane::Settings => "Settings",
        }
    }

    /// The pane a hidden one comes back beside: its own kind of company.
    fn companion(self) -> Option<Pane> {
        match self {
            Pane::Settings => Some(Pane::Cues),
            Pane::Cart => Some(Pane::Inspector),
            Pane::Active => Some(Pane::Preview),
            Pane::Preview => Some(Pane::Active),
            Pane::Cues | Pane::Inspector => None,
        }
    }
}

/// A pane in a window of its own, and where that window was last.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Popout {
    pub pane: Pane,
    /// Outer position and inner size in points, once the window has had them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pos: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<[f32; 2]>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Layout {
    pub dock: DockState<Pane>,
    #[serde(default)]
    pub popouts: Vec<Popout>,
}

impl Default for Layout {
    /// The arrangement NewQ has always had: cues over the inspector on the
    /// left, the preview over what is playing on the right.
    fn default() -> Layout {
        let mut dock = DockState::new(vec![Pane::Cues]);
        let tree = dock.main_surface_mut();
        let [cues, right] = tree.split_right(NodeIndex::root(), 0.66, vec![Pane::Preview]);
        tree.split_below(cues, 0.56, vec![Pane::Inspector]);
        tree.split_below(right, 0.55, vec![Pane::Active]);
        Layout {
            dock,
            popouts: Vec::new(),
        }
    }
}

impl Layout {
    fn docked_path(&self, pane: Pane) -> Option<TabPath> {
        self.dock.find_tab(&pane)
    }

    pub fn is_docked(&self, pane: Pane) -> bool {
        self.docked_path(pane).is_some()
    }

    pub fn is_popped_out(&self, pane: Pane) -> bool {
        self.popouts.iter().any(|p| p.pane == pane)
    }

    /// Showing somewhere, in the window or a window of its own.
    pub fn is_open(&self, pane: Pane) -> bool {
        self.is_docked(pane) || self.is_popped_out(pane)
    }

    pub fn docked_panes(&self) -> Vec<Pane> {
        self.dock.iter_all_tabs().map(|(_, p)| *p).collect()
    }

    /// Show a hidden pane, docked beside its companion if that is docked,
    /// else wherever the focus is. A pane already docked is brought to the
    /// front of its tabs; one popped out stays where it is.
    pub fn open(&mut self, pane: Pane) {
        if self.is_popped_out(pane) {
            return;
        }
        if let Some(path) = self.docked_path(pane) {
            let _ = self.dock.set_active_tab(path);
            return;
        }
        if self.dock.iter_all_tabs().next().is_none() {
            self.dock = DockState::new(vec![pane]);
            return;
        }
        let beside =
            pane.companion()
                .and_then(|c| self.docked_path(c))
                .map(|p| egui_dock::NodePath {
                    surface: p.surface,
                    node: p.node,
                });
        if let Some(node) = beside {
            self.dock.set_focused_node_and_surface(node);
        }
        self.dock.push_to_focused_leaf(pane);
    }

    /// Hide a pane, wherever it is.
    pub fn close(&mut self, pane: Pane) {
        if let Some(path) = self.docked_path(pane) {
            self.dock.remove_tab(path);
        }
        self.popouts.retain(|p| p.pane != pane);
    }

    pub fn toggle(&mut self, pane: Pane) {
        if self.is_open(pane) {
            self.close(pane);
        } else {
            self.open(pane);
        }
    }

    /// Give a pane a window of its own.
    pub fn pop_out(&mut self, pane: Pane) {
        if self.is_popped_out(pane) {
            return;
        }
        if let Some(path) = self.docked_path(pane) {
            self.dock.remove_tab(path);
        }
        self.popouts.push(Popout {
            pane,
            pos: None,
            size: None,
        });
    }

    /// Put a popped-out pane back in the main window.
    pub fn dock_back(&mut self, pane: Pane) {
        if !self.is_popped_out(pane) {
            return;
        }
        self.popouts.retain(|p| p.pane != pane);
        self.open(pane);
    }

    /// Note where a popped-out pane's window is, for next time.
    pub fn set_popout_geometry(&mut self, pane: Pane, pos: Option<[f32; 2]>, size: [f32; 2]) {
        if let Some(p) = self.popouts.iter_mut().find(|p| p.pane == pane) {
            if pos.is_some() {
                p.pos = pos;
            }
            p.size = Some(size);
        }
    }

    pub fn reset(&mut self) {
        *self = Layout::default();
    }

    /// Whether the numbers hang together: every pane at most once across the
    /// window and the popped-out ones. A file that says otherwise is not
    /// trusted.
    pub fn is_valid(&self) -> bool {
        let mut seen = std::collections::HashSet::new();
        let docked = self.docked_panes();
        let popped = self.popouts.iter().map(|p| p.pane);
        docked.into_iter().chain(popped).all(|p| seen.insert(p)) && self.dock.surfaces_count() == 1
    }

    /// The layout saved in `dir`, or the default if there is none or it makes
    /// no sense.
    pub fn load(dir: &Path) -> Layout {
        std::fs::read_to_string(dir.join(FILE))
            .ok()
            .and_then(|t| Layout::from_json(&t))
            .filter(Layout::is_valid)
            // Never start with nothing to see. The cue list itself is not
            // needed: the cart alone is a fine arrangement.
            .filter(|l| Pane::ALL.iter().any(|p| l.is_open(*p)))
            .unwrap_or_default()
    }

    /// Read a layout saved as JSON. The docking library keeps the size of
    /// each tile, which starts out infinite and which JSON writes as `null`
    /// and can't read back; they are worked out afresh every frame, so they are
    /// put right here. Anything else that doesn't parse is no layout.
    pub fn from_json(text: &str) -> Option<Layout> {
        fn fix(v: &mut serde_json::Value) {
            match v {
                serde_json::Value::Object(map) => {
                    for (key, child) in map.iter_mut() {
                        if (key == "min" || key == "max")
                            && let serde_json::Value::Object(point) = child
                        {
                            for axis in ["x", "y"] {
                                if point.get(axis).is_some_and(|n| n.is_null()) {
                                    point.insert(axis.into(), 0.0.into());
                                }
                            }
                        }
                        fix(child);
                    }
                }
                serde_json::Value::Array(items) => items.iter_mut().for_each(fix),
                _ => {}
            }
        }
        let mut value: serde_json::Value = serde_json::from_str(text).ok()?;
        fix(&mut value);
        serde_json::from_value(value).ok()
    }

    /// Write the layout to `dir`, by way of a temporary file so a crash
    /// can't leave half of one.
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

const FILE: &str = "layout.json";

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(mut v: Vec<Pane>) -> Vec<Pane> {
        v.sort_by_key(|p| p.title());
        v
    }

    #[test]
    fn the_default_is_the_arrangement_there_has_always_been() {
        let l = Layout::default();
        assert!(l.is_valid());
        assert_eq!(
            sorted(l.docked_panes()),
            sorted(vec![
                Pane::Cues,
                Pane::Inspector,
                Pane::Preview,
                Pane::Active
            ])
        );
        // The cart and the settings are not shown until asked for.
        assert!(!l.is_open(Pane::Cart) && !l.is_open(Pane::Settings));
        assert!(l.popouts.is_empty());
    }

    #[test]
    fn opening_and_closing_a_pane_docks_it_beside_its_companion_and_hides_it() {
        let mut l = Layout::default();
        l.open(Pane::Settings);
        assert!(l.is_docked(Pane::Settings));
        // Beside the cue list: the same leaf.
        let (a, b) = (
            l.dock.find_tab(&Pane::Settings).unwrap(),
            l.dock.find_tab(&Pane::Cues).unwrap(),
        );
        assert_eq!(a.node, b.node, "the settings are a tab of the cue list");
        l.open(Pane::Cart);
        let (c, i) = (
            l.dock.find_tab(&Pane::Cart).unwrap(),
            l.dock.find_tab(&Pane::Inspector).unwrap(),
        );
        assert_eq!(c.node, i.node, "the cart is a tab of the inspector");
        l.close(Pane::Settings);
        assert!(!l.is_open(Pane::Settings) && l.is_valid());
        // Toggling.
        l.toggle(Pane::Settings);
        assert!(l.is_open(Pane::Settings));
        l.toggle(Pane::Settings);
        assert!(!l.is_open(Pane::Settings));
    }

    #[test]
    fn opening_what_is_already_docked_does_not_duplicate_it() {
        let mut l = Layout::default();
        l.open(Pane::Cues);
        l.open(Pane::Cues);
        assert_eq!(
            l.docked_panes()
                .iter()
                .filter(|p| **p == Pane::Cues)
                .count(),
            1
        );
        assert!(l.is_valid());
    }

    #[test]
    fn a_popped_out_pane_leaves_the_window_and_comes_back() {
        let mut l = Layout::default();
        l.pop_out(Pane::Preview);
        assert!(l.is_popped_out(Pane::Preview) && !l.is_docked(Pane::Preview));
        assert!(l.is_open(Pane::Preview) && l.is_valid());
        // Popping out again, or opening it, changes nothing.
        l.pop_out(Pane::Preview);
        l.open(Pane::Preview);
        assert_eq!(l.popouts.len(), 1);
        // Its window's place is remembered.
        l.set_popout_geometry(Pane::Preview, Some([100.0, 50.0]), [640.0, 480.0]);
        assert_eq!(l.popouts[0].pos, Some([100.0, 50.0]));
        l.set_popout_geometry(Pane::Preview, None, [700.0, 500.0]);
        assert_eq!(
            (l.popouts[0].pos, l.popouts[0].size),
            (Some([100.0, 50.0]), Some([700.0, 500.0]))
        );
        // Docking it back puts it beside the active list again, once.
        l.dock_back(Pane::Preview);
        assert!(l.is_docked(Pane::Preview) && l.popouts.is_empty() && l.is_valid());
        let (p, a) = (
            l.dock.find_tab(&Pane::Preview).unwrap(),
            l.dock.find_tab(&Pane::Active).unwrap(),
        );
        assert_eq!(p.node, a.node, "back beside its companion");
    }

    #[test]
    fn closing_a_popped_out_pane_hides_it() {
        let mut l = Layout::default();
        l.pop_out(Pane::Active);
        l.close(Pane::Active);
        assert!(!l.is_open(Pane::Active) && l.popouts.is_empty());
    }

    #[test]
    fn an_empty_window_can_be_filled_again() {
        let mut l = Layout::default();
        for p in Pane::ALL {
            l.close(p);
        }
        assert!(l.docked_panes().is_empty());
        l.open(Pane::Cues);
        assert_eq!(l.docked_panes(), vec![Pane::Cues]);
        l.open(Pane::Inspector);
        assert!(l.is_valid() && l.is_docked(Pane::Inspector));
        // Everything popped out leaves the window empty too, and docking back works.
        let mut l = Layout::default();
        for p in [Pane::Cues, Pane::Inspector, Pane::Preview, Pane::Active] {
            l.pop_out(p);
        }
        assert!(l.docked_panes().is_empty() && l.popouts.len() == 4);
        l.dock_back(Pane::Cues);
        assert!(l.is_docked(Pane::Cues) && l.is_valid());
    }

    #[test]
    fn reset_puts_everything_back() {
        let mut l = Layout::default();
        l.pop_out(Pane::Preview);
        l.open(Pane::Settings);
        l.close(Pane::Inspector);
        l.reset();
        assert_eq!(
            sorted(l.docked_panes()),
            sorted(vec![
                Pane::Cues,
                Pane::Inspector,
                Pane::Preview,
                Pane::Active
            ])
        );
        assert!(l.popouts.is_empty());
    }

    #[test]
    fn a_layout_survives_saving_and_a_bad_file_is_ignored() {
        let dir = std::env::temp_dir().join("newq_test_layout");
        let _ = std::fs::remove_dir_all(&dir);
        // Nothing saved: the default.
        assert!(Layout::load(&dir).is_docked(Pane::Inspector));
        let mut l = Layout::default();
        l.open(Pane::Settings);
        l.pop_out(Pane::Preview);
        l.set_popout_geometry(Pane::Preview, Some([10.0, 20.0]), [800.0, 600.0]);
        l.close(Pane::Inspector);
        l.save(&dir).unwrap();
        let back = Layout::load(&dir);
        assert_eq!(sorted(back.docked_panes()), sorted(l.docked_panes()));
        assert_eq!(back.popouts, l.popouts);
        assert!(back.is_valid());
        // Rubbish is the default, not a crash.
        std::fs::write(dir.join(FILE), "not json").unwrap();
        assert!(Layout::load(&dir).is_docked(Pane::Inspector));
        // So is a layout with a pane twice, and one with nothing open.
        let mut twice = Layout::default();
        twice.popouts.push(Popout {
            pane: Pane::Cues,
            pos: None,
            size: None,
        });
        assert!(!twice.is_valid());
        std::fs::write(dir.join(FILE), serde_json::to_string(&twice).unwrap()).unwrap();
        assert!(Layout::load(&dir).popouts.is_empty());
        let mut nothing = Layout::default();
        for p in Pane::ALL {
            nothing.close(p);
        }
        std::fs::write(dir.join(FILE), serde_json::to_string(&nothing).unwrap()).unwrap();
        assert!(Layout::load(&dir).is_docked(Pane::Cues));
        // The cart without the cue list is allowed, and so is the list in a
        // window of its own.
        let mut cart_only = Layout::default();
        cart_only.close(Pane::Cues);
        cart_only.open(Pane::Cart);
        cart_only.pop_out(Pane::Inspector);
        std::fs::write(dir.join(FILE), serde_json::to_string(&cart_only).unwrap()).unwrap();
        let loaded = Layout::load(&dir);
        assert!(!loaded.is_open(Pane::Cues) && loaded.is_docked(Pane::Cart));
        assert!(loaded.is_popped_out(Pane::Inspector));
        std::fs::remove_dir_all(&dir).ok();
    }
}
