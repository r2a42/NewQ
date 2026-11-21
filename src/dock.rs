//! Where the panes of the main window are: docked in a tiled layout that
//! resizes neighbours as one grows or shrinks (so they never overlap), popped
//! out into windows of their own, or hidden. The layout is the user's, kept
//! per computer rather than in the show, since monitors differ.

use egui_dock::{DockState, NodeIndex, TabPath};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// A part of the main window that can be moved, popped out or hidden.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pane {
    Cues,
    Inspector,
    Cart,
    /// A Preview pane, by its instance: there can be several, each with its
    /// own mode and output (`Layout::previews`). 0 is the first, the one there
    /// has always been.
    Preview(u8),
    Active,
    Settings,
    /// The timeline editor (opened under the cue list).
    Timeline,
}

/// The first Preview pane.
pub const PREVIEW: Pane = Pane::Preview(0);

/// Preview hosts that are not panes, with settings of their own in
/// `Layout::previews` and render slots and governor ids of their own: the new
/// interface's Operator view.
pub const OPERATOR_PREVIEW: u8 = 255;
/// And the timeline workspace's monitor (one output, chosen in its own menu).
pub const WORKSPACE_PREVIEW: u8 = 254;
/// Instances below this are Preview panes.
pub const PANE_INSTANCES: u8 = 250;

impl Pane {
    /// One of each kind (the first Preview only): what the Window menu lists.
    pub const ALL: [Pane; 7] = [
        Pane::Cues,
        Pane::Inspector,
        Pane::Cart,
        PREVIEW,
        Pane::Active,
        Pane::Settings,
        Pane::Timeline,
    ];

    pub fn title(self) -> String {
        match self {
            Pane::Preview(n) if n > 0 => format!("Preview {}", u32::from(n) + 1),
            Pane::Cues => "Cue list".into(),
            p => p.name().into(),
        }
    }

    /// The Preview pane's instance, if it is one.
    pub fn preview(self) -> Option<u8> {
        match self {
            Pane::Preview(n) => Some(n),
            _ => None,
        }
    }

    /// The pane a hidden one comes back beside: its own kind of company.
    fn companion(self) -> Option<Pane> {
        match self {
            Pane::Settings | Pane::Timeline => Some(Pane::Cues),
            Pane::Cart => Some(Pane::Inspector),
            Pane::Active => Some(PREVIEW),
            Pane::Preview(_) => Some(Pane::Active),
            Pane::Cues | Pane::Inspector => None,
        }
    }

    /// The name it is written by.
    fn name(self) -> &'static str {
        match self {
            Pane::Cues => "Cues",
            Pane::Inspector => "Inspector",
            Pane::Cart => "Cart",
            Pane::Preview(_) => "Preview",
            Pane::Active => "Active",
            Pane::Settings => "Settings",
            Pane::Timeline => "Timeline",
        }
    }
}

/// Written as before for every pane but a second (third…) Preview, which is
/// `{"Preview": n}`: a layout with one preview reads the same to an older NewQ.
impl Serialize for Pane {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Pane::Preview(n) if *n != 0 => {
                use serde::ser::SerializeMap;
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("Preview", n)?;
                m.end()
            }
            p => s.serialize_str(p.name()),
        }
    }
}

impl<'de> Deserialize<'de> for Pane {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Pane, D::Error> {
        use serde::de::Error;
        let v = serde_json::Value::deserialize(d)?;
        match &v {
            serde_json::Value::String(name) => Pane::ALL
                .into_iter()
                .find(|p| p.name() == name)
                .ok_or_else(|| D::Error::custom(format!("no pane called {name}"))),
            serde_json::Value::Object(m) if m.len() == 1 => m
                .get("Preview")
                .and_then(|n| n.as_u64())
                .and_then(|n| u8::try_from(n).ok())
                .filter(|n| *n < PANE_INSTANCES)
                .map(Pane::Preview)
                .ok_or_else(|| D::Error::custom("not a pane")),
            _ => Err(D::Error::custom("not a pane")),
        }
    }
}

/// What a Preview pane shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub enum PreviewMode {
    /// One output, chosen in the pane's menu.
    #[default]
    Single,
    /// Every output at once, or the show's multiview layout.
    Multiview,
    /// The show's 3D stage (`ui::stage_view`).
    Stage,
}

/// A mode from a later NewQ that this one doesn't know reads as Single.
impl<'de> Deserialize<'de> for PreviewMode {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<PreviewMode, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        Ok(PreviewMode::ALL
            .into_iter()
            .find(|m| serde_json::to_value(m).ok().as_ref() == Some(&v))
            .unwrap_or_default())
    }
}

impl PreviewMode {
    pub const ALL: [PreviewMode; 3] = [
        PreviewMode::Single,
        PreviewMode::Multiview,
        PreviewMode::Stage,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PreviewMode::Single => "One display",
            PreviewMode::Multiview => "Multiview",
            PreviewMode::Stage => "3D stage",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            PreviewMode::Single => "One display, chosen in the menu beside",
            PreviewMode::Multiview => "Every display at once, or the show's multiview layout",
            PreviewMode::Stage => {
                "The show's 3D stage, with each display's picture on its surfaces"
            }
        }
    }
}

/// One preview's own settings, kept with the layout (per computer, not in the
/// show): a Preview pane's, or the Operator view's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PreviewPane {
    #[serde(default)]
    pub mode: PreviewMode,
    /// The output shown in Single mode (the first when unset or gone).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<crate::show::OutputId>,
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

/// Written as `LayoutFile`, so that 0.24 reads the file too.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(from = "LayoutFile", into = "LayoutFile")]
pub struct Layout {
    pub dock: DockState<Pane>,
    pub popouts: Vec<Popout>,
    /// Each preview's mode and output, by instance (`Pane::Preview(n)`, and
    /// `OPERATOR_PREVIEW`).
    pub previews: BTreeMap<u8, PreviewPane>,
    /// 0.24's Multiview setting has been taken into `previews`
    /// (`take_old_multiview`), once. 0.24 writes the file without it, so
    /// after a downgrade its setting is taken again.
    pub took_multiview: bool,
}

/// `layout.json` as written. 0.24 reads `dock` and `popouts` (and ignores the
/// rest), and gives up on the whole file, and writes its default over it, at
/// a pane it doesn't know (a second Preview, `{"Preview": n}`) or one twice.
/// So those two hold the first Preview pane only, as 0.24 had it; the whole
/// arrangement, the other Preview panes with it, is in `panes` (written only
/// when it differs) and `more_popouts`.
#[derive(Serialize, Deserialize)]
struct LayoutFile {
    dock: DockState<Pane>,
    #[serde(default)]
    popouts: Vec<Popout>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    panes: Option<DockState<Pane>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    more_popouts: Vec<Popout>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    previews: BTreeMap<u8, PreviewPane>,
    #[serde(default, skip_serializing_if = "is_false")]
    took_multiview: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A pane 0.24 doesn't know.
fn later_pane(p: Pane) -> bool {
    matches!(p, Pane::Preview(n) if n != 0)
}

impl From<LayoutFile> for Layout {
    fn from(f: LayoutFile) -> Layout {
        let mut popouts = f.popouts;
        popouts.extend(f.more_popouts);
        Layout {
            dock: f.panes.unwrap_or(f.dock),
            popouts,
            previews: f.previews,
            took_multiview: f.took_multiview,
        }
    }
}

impl From<Layout> for LayoutFile {
    fn from(l: Layout) -> LayoutFile {
        let later: Vec<Pane> = l
            .dock
            .iter_all_tabs()
            .map(|(_, p)| *p)
            .filter(|p| later_pane(*p))
            .collect();
        let (dock, panes) = if later.is_empty() {
            (l.dock, None)
        } else {
            let mut older = l.dock.clone();
            // With the first Preview closed, the first other one stands in for
            // it (so a layout of a second preview alone isn't empty to 0.24).
            let first_open =
                l.dock.find_tab(&PREVIEW).is_some() || l.popouts.iter().any(|p| p.pane == PREVIEW);
            let mut stand_in = None;
            if !first_open
                && let Some((_, tab)) = older.iter_all_tabs_mut().find(|(_, p)| later_pane(**p))
            {
                stand_in = Some(*tab);
                *tab = PREVIEW;
            }
            for p in later.into_iter().filter(|p| Some(*p) != stand_in) {
                if let Some(path) = older.find_tab(&p) {
                    older.remove_tab(path);
                }
            }
            (older, Some(l.dock))
        };
        let (more_popouts, popouts) = l.popouts.into_iter().partition(|p| later_pane(p.pane));
        LayoutFile {
            dock,
            popouts,
            panes,
            more_popouts,
            previews: l.previews,
            took_multiview: l.took_multiview,
        }
    }
}

impl Default for Layout {
    /// The arrangement NewQ has always had: cues over the inspector on the
    /// left, the preview over what is playing on the right.
    fn default() -> Layout {
        let mut dock = DockState::new(vec![Pane::Cues]);
        let tree = dock.main_surface_mut();
        let [cues, right] = tree.split_right(NodeIndex::root(), 0.66, vec![PREVIEW]);
        tree.split_below(cues, 0.56, vec![Pane::Inspector]);
        tree.split_below(right, 0.55, vec![Pane::Active]);
        Layout {
            dock,
            popouts: Vec::new(),
            previews: BTreeMap::new(),
            took_multiview: false,
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

    /// In sight: popped out, or docked as the tab in front of its node (not
    /// behind another tab).
    pub fn in_front(&self, pane: Pane) -> bool {
        self.is_popped_out(pane)
            || self.docked_path(pane).is_some_and(|p| {
                self.dock[p.node_path()]
                    .get_leaf()
                    .is_some_and(|l| l.active == p.tab)
            })
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
        // The timeline goes along the bottom of the window, under the cue list
        // and everything beside it, as a video editor's tracks do: it wants width.
        if pane == Pane::Timeline
            && let Some(cues) = self.docked_path(Pane::Cues)
        {
            self.dock.split(
                egui_dock::NodePath {
                    surface: cues.surface,
                    node: NodeIndex::root(),
                },
                egui_dock::Split::Below,
                0.6,
                egui_dock::Node::leaf_with(vec![pane]),
            );
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
        self.forget_closed_previews();
    }

    /// The Preview panes showing, docked or in windows of their own, by instance.
    pub fn preview_panes(&self) -> Vec<u8> {
        let mut v: Vec<u8> = self
            .dock
            .iter_all_tabs()
            .map(|(_, p)| *p)
            .chain(self.popouts.iter().map(|p| p.pane))
            .filter_map(Pane::preview)
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Preview `n`'s settings (the defaults if it has none yet).
    pub fn preview(&self, n: u8) -> PreviewPane {
        self.previews.get(&n).copied().unwrap_or_default()
    }

    /// Preview `n`'s settings, to change.
    pub fn preview_mut(&mut self, n: u8) -> &mut PreviewPane {
        self.previews.entry(n).or_default()
    }

    /// Forget the settings of the Preview panes that are gone (a tab's close
    /// button takes it away without asking `close`), but never the first's
    /// nor the other hosts'. The instances forgotten.
    pub fn forget_closed_previews(&mut self) -> Vec<u8> {
        let open = self.preview_panes();
        let gone: Vec<u8> = self
            .previews
            .keys()
            .copied()
            .filter(|n| *n != 0 && *n < PANE_INSTANCES && !open.contains(n))
            .collect();
        for n in &gone {
            self.previews.remove(n);
        }
        gone
    }

    /// 0.24's Multiview button was one setting for the Preview pane and the
    /// Operator view (`Prefs::multiview`): on, they show the multiview, unless
    /// a mode of their own is already kept. Once per layout (`took_multiview`:
    /// `Prefs::multiview` is written again, for 0.24, and must not overrule a
    /// mode chosen since). Whether the layout changed.
    pub fn take_old_multiview(&mut self, on: bool) -> bool {
        if self.took_multiview {
            return false;
        }
        self.took_multiview = true;
        if on {
            for n in [0, OPERATOR_PREVIEW] {
                self.previews.entry(n).or_insert(PreviewPane {
                    mode: PreviewMode::Multiview,
                    output: None,
                });
            }
        }
        true
    }

    /// What 0.24's Multiview setting would be now (`Prefs::multiview`): the
    /// first Preview pane shows the multiview.
    pub fn old_multiview(&self) -> bool {
        self.preview(0).mode == PreviewMode::Multiview
    }

    /// The lowest Preview instance not showing (a closed one's number is free
    /// again), with fresh settings unless it is the first.
    fn free_preview(&mut self) -> Option<Pane> {
        let open = self.preview_panes();
        let n = (0..PANE_INSTANCES).find(|n| !open.contains(n))?;
        if n != 0 {
            self.previews.insert(n, PreviewPane::default());
        }
        Some(Pane::Preview(n))
    }

    /// One more Preview pane, docked beside the first docked preview (side by
    /// side, sharing its space) or, with none docked, where a preview goes.
    /// None when there are as many as there can be.
    pub fn new_preview(&mut self) -> Option<Pane> {
        let pane = self.free_preview()?;
        let beside = self
            .dock
            .iter_all_tabs()
            .find_map(|(_, p)| p.preview().map(|_| *p))
            .and_then(|p| self.docked_path(p));
        match beside {
            Some(path) => {
                self.dock.split(
                    egui_dock::NodePath {
                        surface: path.surface,
                        node: path.node,
                    },
                    egui_dock::Split::Right,
                    0.5,
                    egui_dock::Node::leaf_with(vec![pane]),
                );
            }
            None => self.open(pane),
        }
        Some(pane)
    }

    /// One more Preview pane, in a window of its own (where there is no dock:
    /// the new interface).
    pub fn new_preview_window(&mut self) -> Option<Pane> {
        let pane = self.free_preview()?;
        self.pop_out(pane);
        Some(pane)
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
        let mut layout = std::fs::read_to_string(dir.join(FILE))
            .ok()
            .and_then(|t| Layout::from_json(&t))
            .filter(Layout::is_valid)
            // Never start with nothing to see. The cue list itself is not
            // needed: the cart alone is a fine arrangement.
            .filter(|l| !l.docked_panes().is_empty() || !l.popouts.is_empty())
            .unwrap_or_default();
        layout.forget_impossible_places();
        layout.forget_closed_previews();
        // Whatever tab was in front when it was saved (the settings page, say),
        // the cue list is what a show opens to.
        if let Some(path) = layout.docked_path(Pane::Cues) {
            let _ = layout.dock.set_active_tab(path);
        }
        layout
    }

    /// A pop-out window's remembered place that can't be one (a hand-edited
    /// file: a size of nothing or of miles, a position off any screen) is
    /// forgotten, so the window opens at its usual size instead of failing
    /// to open at all.
    fn forget_impossible_places(&mut self) {
        const SIZE: std::ops::RangeInclusive<f32> = 100.0..=16_384.0;
        const FAR: f32 = 100_000.0;
        for p in &mut self.popouts {
            p.size = p.size.filter(|[w, h]| SIZE.contains(w) && SIZE.contains(h));
            p.pos = p.pos.filter(|[x, y]| x.abs() <= FAR && y.abs() <= FAR);
        }
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
            sorted(vec![Pane::Cues, Pane::Inspector, PREVIEW, Pane::Active])
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
        l.pop_out(PREVIEW);
        assert!(l.is_popped_out(PREVIEW) && !l.is_docked(PREVIEW));
        assert!(l.is_open(PREVIEW) && l.is_valid());
        // Popping out again, or opening it, changes nothing.
        l.pop_out(PREVIEW);
        l.open(PREVIEW);
        assert_eq!(l.popouts.len(), 1);
        // Its window's place is remembered.
        l.set_popout_geometry(PREVIEW, Some([100.0, 50.0]), [640.0, 480.0]);
        assert_eq!(l.popouts[0].pos, Some([100.0, 50.0]));
        l.set_popout_geometry(PREVIEW, None, [700.0, 500.0]);
        assert_eq!(
            (l.popouts[0].pos, l.popouts[0].size),
            (Some([100.0, 50.0]), Some([700.0, 500.0]))
        );
        // Docking it back puts it beside the active list again, once.
        l.dock_back(PREVIEW);
        assert!(l.is_docked(PREVIEW) && l.popouts.is_empty() && l.is_valid());
        let (p, a) = (
            l.dock.find_tab(&PREVIEW).unwrap(),
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
        for p in [Pane::Cues, Pane::Inspector, PREVIEW, Pane::Active] {
            l.pop_out(p);
        }
        assert!(l.docked_panes().is_empty() && l.popouts.len() == 4);
        l.dock_back(Pane::Cues);
        assert!(l.is_docked(Pane::Cues) && l.is_valid());
    }

    #[test]
    fn reset_puts_everything_back() {
        let mut l = Layout::default();
        l.pop_out(PREVIEW);
        l.open(Pane::Settings);
        l.close(Pane::Inspector);
        l.reset();
        assert_eq!(
            sorted(l.docked_panes()),
            sorted(vec![Pane::Cues, Pane::Inspector, PREVIEW, Pane::Active])
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
        l.pop_out(PREVIEW);
        l.set_popout_geometry(PREVIEW, Some([10.0, 20.0]), [800.0, 600.0]);
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

    #[test]
    fn a_layout_from_before_the_timeline_pane_loads_and_one_with_it_round_trips() {
        let dir = std::env::temp_dir().join(format!("newq_dock_timeline_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // What 0.20.3 writes: the timeline is not in it, and it is not opened.
        let old = serde_json::to_string(&Layout::default()).unwrap();
        assert!(!old.contains("Timeline"), "{old}");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE), &old).unwrap();
        let back = Layout::load(&dir);
        assert!(back.is_valid() && !back.is_open(Pane::Timeline));
        assert_eq!(
            sorted(back.docked_panes()),
            sorted(Layout::default().docked_panes())
        );
        // Opened, it docks along the bottom, under the cue list and the panes
        // beside it, and is saved and read back.
        let under_the_rest = |l: &Layout| {
            let root = NodeIndex::root();
            let tl = l.dock.find_tab(&Pane::Timeline).unwrap().node;
            let mut cues = l.dock.find_tab(&Pane::Cues).unwrap().node;
            while cues.parent().is_some_and(|p| p != root) {
                cues = cues.parent().unwrap();
            }
            let vertical = matches!(l.dock.main_surface()[root], egui_dock::Node::Vertical(_));
            vertical && tl == root.right() && cues == root.left()
        };
        let mut l = Layout::default();
        l.open(Pane::Timeline);
        assert!(under_the_rest(&l) && l.is_valid());
        assert_eq!(sorted(l.docked_panes()).len(), 5);
        // Shut and opened again, it goes back there.
        l.close(Pane::Timeline);
        l.open(Pane::Timeline);
        assert!(under_the_rest(&l) && l.is_valid());
        l.save(&dir).unwrap();
        assert!(Layout::load(&dir).is_docked(Pane::Timeline));
        l.pop_out(Pane::Timeline);
        l.save(&dir).unwrap();
        assert!(Layout::load(&dir).is_popped_out(Pane::Timeline));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_pop_out_window_place_that_cannot_be_is_forgotten() {
        let dir = std::env::temp_dir().join("newq_test_layout_places");
        let _ = std::fs::remove_dir_all(&dir);
        let mut l = Layout::default();
        l.pop_out(PREVIEW);
        l.set_popout_geometry(PREVIEW, Some([10.0, 20.0]), [800.0, 600.0]);
        l.pop_out(Pane::Active);
        // Off any screen, and no size at all. (JSON has no NaN or infinity,
        // and a number too big for a float is no layout at all.)
        l.set_popout_geometry(Pane::Active, Some([1e9, -1e9]), [0.0, 1e9]);
        l.save(&dir).unwrap();
        let back = Layout::load(&dir);
        let find = |p: Pane| back.popouts.iter().find(|q| q.pane == p).copied().unwrap();
        assert_eq!(
            (find(PREVIEW).pos, find(PREVIEW).size),
            (Some([10.0, 20.0]), Some([800.0, 600.0]))
        );
        assert_eq!(
            (find(Pane::Active).pos, find(Pane::Active).size),
            (None, None)
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_saved_layout_opens_to_the_cue_list_not_the_tab_that_was_in_front() {
        let dir = std::env::temp_dir().join(format!("newq_dock_front_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut l = Layout::default();
        l.open(Pane::Settings);
        // The settings page is in front when it is saved.
        l.save(&dir).unwrap();
        let back = Layout::load(&dir);
        assert!(back.is_docked(Pane::Settings) && back.is_docked(Pane::Cues));
        let active = |l: &Layout, pane: Pane| -> bool {
            let path = l.docked_path(pane).unwrap();
            match &l.dock[path.surface][path.node] {
                egui_dock::Node::Leaf(leaf) => leaf.tabs.get(leaf.active.0) == Some(&pane),
                _ => false,
            }
        };
        assert!(
            active(&l, Pane::Settings),
            "the test needs Settings in front"
        );
        assert!(active(&back, Pane::Cues));
        assert!(!active(&back, Pane::Settings));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn several_previews_each_a_tab_of_its_own_with_its_own_settings() {
        let mut l = Layout::default();
        let second = l.new_preview().unwrap();
        assert_eq!(second, Pane::Preview(1));
        assert_eq!(second.title(), "Preview 2");
        assert!(l.is_docked(PREVIEW) && l.is_docked(second) && l.is_valid());
        // Side by side, not behind each other: both in front.
        assert!(l.in_front(PREVIEW) && l.in_front(second));
        assert_ne!(
            l.dock.find_tab(&PREVIEW).unwrap().node,
            l.dock.find_tab(&second).unwrap().node
        );
        l.preview_mut(1).mode = PreviewMode::Multiview;
        let third = l.new_preview().unwrap();
        assert_eq!(third, Pane::Preview(2));
        assert_eq!(l.preview_panes(), vec![0, 1, 2]);
        assert_eq!(l.preview(1).mode, PreviewMode::Multiview);
        assert_eq!(l.preview(2).mode, PreviewMode::Single);
        // Popped out, it keeps its mode, and comes back.
        l.pop_out(second);
        assert!(l.is_popped_out(second) && l.is_valid());
        assert_eq!(l.preview(1).mode, PreviewMode::Multiview);
        l.dock_back(second);
        assert!(l.is_docked(second));
        assert_eq!(l.preview(1).mode, PreviewMode::Multiview);
        // Closed, its settings go and its number is free for the next.
        l.close(second);
        assert!(!l.previews.contains_key(&1));
        assert_eq!(l.new_preview(), Some(Pane::Preview(1)));
        assert_eq!(l.preview(1).mode, PreviewMode::Single);
        // A tab's close button takes the tab without `close`: the settings go
        // at the next look.
        let path = l.dock.find_tab(&third).unwrap();
        l.preview_mut(2).mode = PreviewMode::Stage;
        l.dock.remove_tab(path);
        assert_eq!(l.forget_closed_previews(), vec![2]);
        // The first keeps its settings when closed, for when it is shown again;
        // and so do the hosts that aren't panes.
        l.preview_mut(0).mode = PreviewMode::Stage;
        l.preview_mut(OPERATOR_PREVIEW).mode = PreviewMode::Multiview;
        l.close(PREVIEW);
        assert_eq!(l.preview(0).mode, PreviewMode::Stage);
        assert_eq!(l.preview(OPERATOR_PREVIEW).mode, PreviewMode::Multiview);
        // In a window of its own, where there is no dock.
        let mut l = Layout::default();
        let w = l.new_preview_window().unwrap();
        assert_eq!(w, Pane::Preview(1));
        assert!(l.is_popped_out(w) && l.is_valid());
        // With no preview docked, a new one goes beside the active list.
        let mut l = Layout::default();
        l.close(PREVIEW);
        assert_eq!(l.new_preview(), Some(PREVIEW));
        assert_eq!(
            l.dock.find_tab(&PREVIEW).unwrap().node,
            l.dock.find_tab(&Pane::Active).unwrap().node
        );
    }

    #[test]
    fn a_layout_with_one_preview_is_written_as_before_and_more_round_trip() {
        let dir = std::env::temp_dir().join(format!("newq_dock_previews_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // The first Preview is written "Preview", as 0.24 does (and reads).
        let one = serde_json::to_string(&Layout::default()).unwrap();
        assert!(one.contains("\"Preview\""), "{one}");
        assert!(!one.contains("previews"), "nothing new written: {one}");
        // A layout 0.24 wrote loads as one Preview, in Single mode.
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE), &one).unwrap();
        let back = Layout::load(&dir);
        assert_eq!(back.preview_panes(), vec![0]);
        assert_eq!(back.preview(0).mode, PreviewMode::Single);
        // Two, one popped out, with their modes and outputs.
        let mut l = Layout::default();
        let second = l.new_preview().unwrap();
        l.preview_mut(1).mode = PreviewMode::Multiview;
        l.preview_mut(0).output = Some(crate::show::OutputId(7));
        l.pop_out(second);
        let text = serde_json::to_string(&l).unwrap();
        assert!(text.contains("{\"Preview\":1}"), "{text}");
        l.save(&dir).unwrap();
        let back = Layout::load(&dir);
        assert!(back.is_valid() && back.is_popped_out(second) && back.is_docked(PREVIEW));
        assert_eq!(back.previews, l.previews);
        // A mode from a later NewQ reads as Single; a preview number out of
        // range is no layout.
        let later = text.replace("\"Multiview\"", "\"Hologram\"");
        assert_eq!(
            Layout::from_json(&later).unwrap().preview(1).mode,
            PreviewMode::Single
        );
        assert!(Layout::from_json(&text.replace("{\"Preview\":1}", "{\"Preview\":251}")).is_none());
        // A second Preview alone (the first closed) is something to see.
        let mut l = Layout::default();
        let second = l.new_preview().unwrap();
        for p in Pane::ALL {
            l.close(p);
        }
        assert_eq!(l.docked_panes(), vec![second]);
        l.save(&dir).unwrap();
        assert_eq!(Layout::load(&dir).docked_panes(), vec![second]);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 0.24's layout types and its `Layout::load`, copied from 0.24's
    /// `dock.rs`: to check that 0.24 reads what this NewQ writes.
    mod v024 {
        use egui_dock::DockState;
        use serde::Deserialize;

        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
        pub enum Pane {
            Cues,
            Inspector,
            Cart,
            Preview,
            Active,
            Settings,
            Timeline,
        }

        pub const ALL: [Pane; 7] = [
            Pane::Cues,
            Pane::Inspector,
            Pane::Cart,
            Pane::Preview,
            Pane::Active,
            Pane::Settings,
            Pane::Timeline,
        ];

        #[derive(Debug, Deserialize)]
        pub struct Popout {
            pub pane: Pane,
            #[serde(default)]
            pub pos: Option<[f32; 2]>,
            #[serde(default)]
            pub size: Option<[f32; 2]>,
        }

        #[derive(Debug, Deserialize)]
        pub struct Layout {
            pub dock: DockState<Pane>,
            #[serde(default)]
            pub popouts: Vec<Popout>,
        }

        impl Layout {
            pub fn docked_panes(&self) -> Vec<Pane> {
                self.dock.iter_all_tabs().map(|(_, p)| *p).collect()
            }
        }

        /// What 0.24 makes of `text`: None where it would give up on it, start
        /// with its default arrangement and write that over the file.
        pub fn load(text: &str) -> Option<Layout> {
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
            let l: Layout = serde_json::from_value(value).ok()?;
            // `is_valid`: every pane at most once, one surface.
            let mut seen = std::collections::HashSet::new();
            let docked = l.docked_panes();
            let valid = docked
                .iter()
                .copied()
                .chain(l.popouts.iter().map(|p| p.pane))
                .all(|p| seen.insert(p))
                && l.dock.surfaces_count() == 1;
            let open = |p: &Pane| docked.contains(p) || l.popouts.iter().any(|q| q.pane == *p);
            (valid && ALL.iter().any(open)).then_some(l)
        }
    }

    #[test]
    fn what_this_newq_writes_is_read_by_0_24_with_one_preview_where_it_was() {
        let check = |l: &Layout, what: &str| -> v024::Layout {
            let text = serde_json::to_string(l).unwrap();
            let old = v024::load(&text).unwrap_or_else(|| panic!("{what}: 0.24 gives up: {text}"));
            // No empty tile left where a later preview was.
            for node in old.dock.main_surface().iter() {
                if let Some(leaf) = node.get_leaf() {
                    assert!(!leaf.is_empty(), "{what}: {text}");
                }
            }
            // And this NewQ reads all of it back.
            let back = Layout::from_json(&text).unwrap();
            assert!(back.is_valid(), "{what}");
            assert_eq!(
                sorted(back.docked_panes()),
                sorted(l.docked_panes()),
                "{what}"
            );
            assert_eq!(back.popouts, l.popouts, "{what}");
            assert_eq!(back.previews, l.previews, "{what}");
            assert_eq!(back.took_multiview, l.took_multiview, "{what}");
            old
        };
        let old = check(&Layout::default(), "the default");
        assert_eq!(old.docked_panes().len(), 4);
        // Two previews side by side, a third in a window of its own.
        let mut l = Layout::default();
        l.new_preview().unwrap();
        let third = l.new_preview().unwrap();
        l.pop_out(third);
        l.preview_mut(1).mode = PreviewMode::Multiview;
        l.took_multiview = true;
        let old = check(&l, "three previews");
        let panes = old.docked_panes();
        assert_eq!(
            panes.iter().filter(|p| **p == v024::Pane::Preview).count(),
            1
        );
        assert!(panes.contains(&v024::Pane::Cues) && panes.contains(&v024::Pane::Active));
        assert!(old.popouts.is_empty(), "{:?}", old.popouts);
        // The first one popped out, the others docked: 0.24 has it popped out.
        let mut l = Layout::default();
        l.new_preview().unwrap();
        l.pop_out(PREVIEW);
        l.set_popout_geometry(PREVIEW, Some([10.0, 20.0]), [640.0, 480.0]);
        let old = check(&l, "the first popped out");
        assert!(!old.docked_panes().contains(&v024::Pane::Preview));
        assert_eq!(old.popouts.len(), 1);
        assert_eq!(old.popouts[0].pane, v024::Pane::Preview);
        assert_eq!(
            (old.popouts[0].pos, old.popouts[0].size),
            (Some([10.0, 20.0]), Some([640.0, 480.0]))
        );
        // The first closed: the second stands in for it.
        let mut l = Layout::default();
        l.new_preview().unwrap();
        l.close(PREVIEW);
        let old = check(&l, "the first closed");
        assert_eq!(
            old.docked_panes()
                .iter()
                .filter(|p| **p == v024::Pane::Preview)
                .count(),
            1
        );
        // A second preview alone.
        let mut l = Layout::default();
        let second = l.new_preview().unwrap();
        for p in Pane::ALL {
            l.close(p);
        }
        assert_eq!(l.docked_panes(), vec![second]);
        let old = check(&l, "a second preview alone");
        assert_eq!(old.docked_panes(), vec![v024::Pane::Preview]);
        // A layout with one preview is written exactly as 0.24 wrote it.
        let text = serde_json::to_string(&Layout::default()).unwrap();
        assert!(
            !text.contains("panes") && !text.contains("more_popouts"),
            "{text}"
        );
    }
}
