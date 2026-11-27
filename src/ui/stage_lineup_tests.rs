// Line-up on Settings ▸ 3D stage: the panes driven with the mouse and the
// arrow keys in the whole window at 1280×760, in both interfaces, against
// made-up projectors whose true place the test knows; and the pure parts.

use super::*;
use crate::dock::Pane;
use crate::stage3d::Placement;
use crate::ui::SettingsPage;
use crate::ui::stage_edit::{Preset, StageTab, add_surface};
use egui::{Event, Key, Modifiers, PointerButton, RawInput};

const SIZE: Vec2 = vec2(1280.0, 760.0);
const HD: (u32, u32) = (1920, 1080);

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "newq-lineup-ui-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// The scene as the app would make it (`StageLive::begin` does in the app).
fn refresh(show: &Show, st: &mut UiState) {
    st.stage.scene = Arc::new(show.stage3d.scene(show, None));
    st.stage.scene_key = show.stage3d.shape_key();
    st.stage.bounds = render3d::scene_bounds(&st.stage.scene);
}

struct Win {
    ctx: egui::Context,
    e: crate::engine::Engine,
    t: f64,
    spots: Vec<(String, Pos2)>,
}

impl Win {
    /// The window with Settings ▸ 3D stage ▸ Projectors open, the draft
    /// kept in `dir`.
    fn new(modern: bool, show: &mut Show, dir: &std::path::Path) -> (Win, UiState) {
        let mut st = UiState::default();
        st.prefs.new_layout = modern;
        st.layout.open(Pane::Settings);
        st.settings_page = SettingsPage::Stage3d;
        st.stage.tab = StageTab::Projectors;
        st.stage.projectors.lineup.draft_dir = Some(dir.to_path_buf());
        st.stage.projectors.library_dir = Some(dir.join("lib"));
        st.stage.projectors.picked = show.stage3d.projectors.first().map(|p| p.id);
        refresh(show, &mut st);
        let mut w = Win {
            ctx: egui::Context::default(),
            e: crate::engine::Engine::for_tests(),
            t: 1.0,
            spots: Vec::new(),
        };
        w.settle(show, &mut st);
        (w, st)
    }

    fn frame(&mut self, show: &mut Show, st: &mut UiState, events: Vec<Event>) {
        self.t += 1.0 / 60.0;
        self.ctx.data_mut(|d| {
            d.remove::<Vec<(String, Rect, bool)>>(egui::Id::new("stage_sources"));
        });
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)),
            time: Some(self.t),
            events,
            ..Default::default()
        };
        let e = &mut self.e;
        let mut out = self
            .ctx
            .run_ui(input, |ui| crate::ui::draw(ui, show, e, st));
        out.textures_delta.clear();
        self.spots = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::epaint::Shape::Text(t) => Some((
                    t.galley.text().to_string(),
                    t.pos + t.galley.rect.center().to_vec2(),
                )),
                _ => None,
            })
            .collect();
    }

    fn settle(&mut self, show: &mut Show, st: &mut UiState) {
        for _ in 0..3 {
            self.frame(show, st, vec![]);
        }
    }

    fn find(&self, name: &str) -> Option<(Rect, bool)> {
        self.ctx
            .data(|d| d.get_temp::<Vec<(String, Rect, bool)>>(egui::Id::new("stage_sources")))
            .unwrap_or_default()
            .into_iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, r, seen)| (r, seen))
    }

    fn rect(&self, name: &str) -> Rect {
        self.find(name)
            .unwrap_or_else(|| panic!("no {name}; words: {:?}", self.words()))
            .0
    }

    fn words(&self) -> Vec<&str> {
        self.spots.iter().map(|(t, _)| t.as_str()).collect()
    }

    fn says(&self, words: &str) -> bool {
        self.spots.iter().any(|(t, _)| t.contains(words))
    }

    fn pointer(
        &mut self,
        show: &mut Show,
        st: &mut UiState,
        at: Pos2,
        pressed: bool,
        m: Modifiers,
    ) {
        self.frame(
            show,
            st,
            vec![Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers: m,
            }],
        );
    }

    fn click_with(&mut self, show: &mut Show, st: &mut UiState, at: Pos2, m: Modifiers) {
        self.frame(show, st, vec![Event::PointerMoved(at)]);
        self.pointer(show, st, at, true, m);
        self.pointer(show, st, at, false, m);
        self.settle(show, st);
    }

    fn click(&mut self, show: &mut Show, st: &mut UiState, at: Pos2) {
        self.click_with(show, st, at, Modifiers::NONE);
    }

    /// Clicks the control `name`, scrolling the tab's content to it first.
    fn press(&mut self, show: &mut Show, st: &mut UiState, name: &str) {
        for _ in 0..40 {
            match self.find(name) {
                Some((_, true)) | None => break,
                Some((r, false)) => {
                    let content = self.find("3D stage ▸ tab content").map(|(c, _)| c);
                    let at = content.map_or(Pos2::new(640.0, 500.0), |c| c.center());
                    let delta = match content {
                        Some(c) if r.max.x > c.max.x => vec2(-80.0, 0.0),
                        Some(c) if r.min.x < c.min.x => vec2(80.0, 0.0),
                        _ if r.center().y > SIZE.y * 0.5 => vec2(0.0, -80.0),
                        _ => vec2(0.0, 80.0),
                    };
                    self.frame(show, st, vec![Event::PointerMoved(at)]);
                    self.frame(
                        show,
                        st,
                        vec![Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            delta,
                            phase: egui::TouchPhase::Move,
                            modifiers: Modifiers::NONE,
                        }],
                    );
                    self.settle(show, st);
                }
            }
        }
        let (r, seen) = self
            .find(name)
            .unwrap_or_else(|| panic!("no {name}; words: {:?}", self.words()));
        assert!(
            seen,
            "{name} in sight: {r:?}, content {:?}",
            self.find("3D stage ▸ tab content")
        );
        self.click(show, st, r.center());
    }

    /// Drags from `from` to `to` with the left button.
    fn drag(&mut self, show: &mut Show, st: &mut UiState, from: Pos2, to: Pos2) {
        self.frame(show, st, vec![Event::PointerMoved(from)]);
        self.pointer(show, st, from, true, Modifiers::NONE);
        let mid = from + (to - from) * 0.5 + vec2(0.0, 9.0);
        self.frame(show, st, vec![Event::PointerMoved(mid)]);
        self.frame(show, st, vec![Event::PointerMoved(to)]);
        self.frame(show, st, vec![Event::PointerMoved(to)]);
        self.pointer(show, st, to, false, Modifiers::NONE);
        self.settle(show, st);
    }

    /// The arrow keys, all in one frame.
    fn keys(&mut self, show: &mut Show, st: &mut UiState, keys: &[(Key, Modifiers)]) {
        let events = keys
            .iter()
            .map(|(key, m)| Event::Key {
                key: *key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: *m,
            })
            .collect();
        self.frame(show, st, events);
        self.settle(show, st);
    }
}

/// A show with `preset`'s screen, a projector where it truly is (aimed at
/// the screen from a little further back, so all of it is in the picture,
/// and turned a little), and the place the operator entered for it (moved
/// and turned from that), which is the show's.
fn rig(preset: Preset) -> (Show, ProjectorId, SurfaceId, Placement) {
    let mut show = Show::default();
    let s = add_surface(&mut show, preset);
    if preset == Preset::AngledWall {
        // Three columns of two panels, each turned 45° from the next: far
        // from flat, every corner in sight of a lens in front.
        show.stage3d.surface_mut(s).unwrap().shape = SurfaceShape::Faceted {
            facets: geometry::angled_wall(3, 2, 2.0, 1.5, 45.0),
        };
    }
    let st = UiState::default();
    let id = super::super::stage_projectors::add_aimed_at(&mut show, &st, s);
    let middle = super::super::stage_projectors::screen_middle(show.stage3d.surface(s).unwrap());
    let aimed = show.projector(id).unwrap().placement;
    let mut truth = aimed;
    for (t, (a, m)) in truth
        .position
        .iter_mut()
        .zip(aimed.position.iter().zip(middle))
    {
        *t += (a - m) * 0.3;
    }
    truth.yaw += 0.6;
    truth.pitch -= 0.4;
    truth.roll += 0.3;
    let mut entered = truth;
    entered.position[0] += 0.30;
    entered.position[1] -= 0.20;
    entered.position[2] += 0.45;
    entered.yaw += 1.5;
    entered.pitch -= 0.8;
    entered.roll += 0.4;
    show.projector_mut(id).unwrap().placement = entered;
    (show, id, s, truth)
}

/// Where `room` truly is in the projector's picture.
fn truly(show: &Show, id: ProjectorId, truth: &Placement, room: [f32; 3]) -> [f32; 2] {
    let p = show.projector(id).unwrap();
    calib::project(truth, &p.lens, HD, room).expect("in front of the lens")
}

/// The screen's corners in the room.
fn room_corners(show: &Show, s: SurfaceId) -> Vec<[f32; 3]> {
    let screen = show.stage3d.surface(s).unwrap();
    let m = screen.placement.matrix();
    corners(&screen.shape)
        .into_iter()
        .map(|c| geometry::point(&m, c))
        .collect()
}

/// Starts lining up the first projector from its row, saying yes to the
/// blanking question.
fn start_by_mouse(w: &mut Win, show: &mut Show, st: &mut UiState) {
    let name = show.stage3d.projectors[0].name.clone();
    w.press(show, st, &format!("Line up {name}"));
    assert!(st.line_up.is_none(), "it asks first");
    assert!(w.says(&blanks_words(&name)), "{:?}", w.words());
    w.press(show, st, &format!("{BLANK_YES} {name}"));
    assert!(st.line_up.is_some(), "lining up; {:?}", w.words());
    assert!(w.find("Line-up ▸ model").is_some());
}

/// Drops a point on the model near `room` (a few points off it, so the
/// snap is what puts it there).
fn drop_point(w: &mut Win, show: &mut Show, st: &mut UiState, room: [f32; 3], towards: [f32; 3]) {
    let rect = w.rect("Line-up ▸ model");
    let p = show
        .projector(st.line_up.as_ref().unwrap().projector)
        .unwrap()
        .clone();
    let camera = model_camera(show, st, &st.stage.projectors.lineup, &p);
    let at = model_pos(&camera, rect, room).expect("in front");
    let mid = model_pos(&camera, rect, towards).expect("in front");
    // A few points in from the corner, onto the screen.
    let into = (mid - at).normalized() * 4.0;
    assert!(rect.contains(at + into), "{at:?} in {rect:?}");
    w.click(show, st, at + into);
}

/// Moves point `i`'s cross to `target` (pixels): dragged near it, then
/// the arrow keys the rest of the way (tens, ones and tenths).
fn nudge_to(w: &mut Win, show: &mut Show, st: &mut UiState, i: usize, target: [f32; 2]) {
    let pic = w.rect("Line-up ▸ picture");
    let scale = pic.width() / HD.0 as f32;
    let pos = |px: [f32; 2]| pic.min + vec2(px[0], px[1]) * scale;
    let now = st.line_up.as_ref().unwrap().points[i].pixel;
    // Dragged to a pixel or so of it (a point of the pane is several).
    w.drag(show, st, pos(now), pos(target));
    let d = st.line_up.as_ref().unwrap();
    assert_eq!(d.active, Some(i), "the dragged point is the one picked");
    let now = d.points[i].pixel;
    assert!(
        (now[0] - target[0]).abs() < 4.0 && (now[1] - target[1]).abs() < 4.0,
        "dragged to {now:?}, near {target:?}"
    );
    // The keys for the rest.
    let mut keys = Vec::new();
    for (k, (minus, plus)) in [
        (Key::ArrowLeft, Key::ArrowRight),
        (Key::ArrowUp, Key::ArrowDown),
    ]
    .into_iter()
    .enumerate()
    {
        let mut left = target[k] - now[k];
        let key = if left < 0.0 { minus } else { plus };
        left = left.abs();
        for (step, m) in [
            (10.0, Modifiers::SHIFT),
            (1.0, Modifiers::NONE),
            (0.1, Modifiers::ALT),
        ] {
            let n = if step < 1.0 {
                (left / step).round()
            } else {
                (left / step).floor()
            };
            for _ in 0..n as usize {
                keys.push((key, m));
            }
            left -= n * step;
        }
    }
    w.keys(show, st, &keys);
    let now = st.line_up.as_ref().unwrap().points[i].pixel;
    assert!(
        (now[0] - target[0]).abs() < 0.06 && (now[1] - target[1]).abs() < 0.06,
        "the arrow keys took it to {now:?}, for {target:?}"
    );
}

fn close(a: &Placement, b: &Placement, metres: f32, degrees: f32) -> bool {
    (0..3).all(|k| (a.position[k] - b.position[k]).abs() < metres)
        && (a.yaw - b.yaw).abs() < degrees
        && (a.pitch - b.pitch).abs() < degrees
        && (a.roll - b.roll).abs() < degrees
}

#[test]
fn four_points_on_a_flat_screen_line_a_projector_up_by_mouse_and_arrow_keys() {
    for modern in [false, true] {
        let (mut show, id, s, truth) = rig(Preset::Flat);
        let entered = show.projector(id).unwrap().clone();
        let dir = scratch(&format!("flat-{modern}"));
        let (mut w, mut st) = Win::new(modern, &mut show, &dir);
        start_by_mouse(&mut w, &mut show, &mut st);
        let corners = room_corners(&show, s);
        let middle =
            super::super::stage_projectors::screen_middle(show.stage3d.surface(s).unwrap());
        for c in &corners {
            drop_point(&mut w, &mut show, &mut st, *c, middle);
        }
        let d = st.line_up.clone().unwrap();
        assert_eq!(d.points.len(), 4, "{:?}", w.words());
        // Each snapped to its corner exactly, kept in the screen's own frame.
        let screen = show.stage3d.surface(s).unwrap().clone();
        let own = super::corners(&screen.shape);
        for (pt, c) in d.points.iter().zip(&own) {
            assert_eq!(pt.surface, Some(s));
            assert_eq!(pt.at, *c, "snapped to the corner");
        }
        // Nothing in the show changed yet.
        assert_eq!(show.projector(id).unwrap(), &entered);
        for (i, c) in corners.iter().enumerate() {
            let target = truly(&show, id, &truth, *c);
            nudge_to(&mut w, &mut show, &mut st, i, target);
        }
        assert!(
            w.says("Lined up to within half a pixel."),
            "{:?}",
            w.words()
        );
        assert!(w.says("Using the lens you entered"), "{:?}", w.words());
        // Solved again only as the points changed, not every frame.
        let solves = st.stage.projectors.lineup.solves;
        w.settle(&mut show, &mut st);
        assert_eq!(st.stage.projectors.lineup.solves, solves);
        // The outline is the solve's.
        let d = st.line_up.clone().unwrap();
        assert!(d.outline.as_ref().is_some_and(|o| !o.is_empty()));
        // Done: one step of the undo history.
        assert!(!st.history.can_undo(), "nothing to undo before Done");
        w.press(&mut show, &mut st, "Line-up ▸ Done");
        assert!(st.line_up.is_none(), "{:?}", w.words());
        let p = show.projector(id).unwrap().clone();
        assert!(
            close(&p.placement, &truth, 0.01, 0.1),
            "{:?} is where it truly is, {truth:?}",
            p.placement
        );
        assert_eq!(p.lens, entered.lens, "four points keep the lens");
        let lu = p.line_up.clone().unwrap();
        assert!(lu.is_current());
        assert_eq!(lu.points.len(), 4);
        assert_eq!(lu.pixels, HD);
        let before = lu.before.clone().unwrap();
        assert_eq!(before.placement, entered.placement);
        assert_eq!(before.lens, entered.lens);
        assert!(!dir.join(DRAFT_FILE).exists(), "the draft file went");
        assert!(st.history.can_undo());
        w.frame(
            &mut show,
            &mut st,
            vec![Event::Key {
                key: Key::Z,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::COMMAND,
            }],
        );
        w.settle(&mut show, &mut st);
        assert_eq!(
            show.projector(id).unwrap(),
            &entered,
            "one undo is all of it"
        );
        assert!(!st.history.can_undo(), "and it was one step");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn eight_points_on_a_faceted_screen_find_the_pose_and_name_a_bad_point() {
    for modern in [false, true] {
        let (mut show, id, s, truth) = rig(Preset::AngledWall);
        let dir = scratch(&format!("faceted-{modern}"));
        let (mut w, mut st) = Win::new(modern, &mut show, &dir);
        start_by_mouse(&mut w, &mut show, &mut st);
        let rect = w.rect("Line-up ▸ model");
        let p = show.projector(id).unwrap().clone();
        let camera = model_camera(&show, &st, &st.stage.projectors.lineup, &p);
        let middle =
            super::super::stage_projectors::screen_middle(show.stage3d.surface(s).unwrap());
        // Eight corners the lens sees well apart from each other.
        let mut chosen: Vec<[f32; 3]> = Vec::new();
        let mut at: Vec<Pos2> = Vec::new();
        for c in room_corners(&show, s) {
            let Some(pos) = model_pos(&camera, rect, c) else {
                continue;
            };
            if rect.shrink(12.0).contains(pos)
                && at.iter().all(|a| a.distance(pos) > 1.5 * SNAP_PT)
                && truly(&show, id, &truth, c)
                    .iter()
                    .zip([HD.0, HD.1])
                    .all(|(v, n)| *v > 0.0 && *v < n as f32)
            {
                at.push(pos);
                chosen.push(c);
            }
        }
        assert!(
            chosen.len() >= 8,
            "{} corners to choose from of {}: {at:?} in {rect:?}",
            chosen.len(),
            room_corners(&show, s).len()
        );
        chosen.truncate(8);
        for c in &chosen {
            drop_point(&mut w, &mut show, &mut st, *c, middle);
        }
        let d = st.line_up.clone().unwrap();
        assert_eq!(d.points.len(), 8, "{:?}", w.words());
        for (i, c) in chosen.iter().enumerate() {
            let target = truly(&show, id, &truth, *c);
            nudge_to(&mut w, &mut show, &mut st, i, target);
        }
        let words = w.words().join("\n");
        // Eight points not on one flat surface: the throw (and the shift)
        // are tried; the rung used is said either way, by name.
        assert!(
            words.contains("Worked out the throw")
                || (words.contains("Using the lens you entered")
                    && words.contains("The points don't pin the throw down")),
            "{words}"
        );
        assert!(
            words.contains("Lined up to within half a pixel."),
            "{words}"
        );
        // One point dragged well off is named.
        let pic = w.rect("Line-up ▸ picture");
        let scale = pic.width() / HD.0 as f32;
        let pos = |px: [f32; 2]| pic.min + vec2(px[0], px[1]) * scale;
        let good = st.line_up.as_ref().unwrap().points[2].pixel;
        let bad = [good[0] + 60.0, good[1] - 40.0];
        w.drag(&mut show, &mut st, pos(good), pos(bad));
        assert!(w.says("Point 3 is"), "{:?}", w.words());
        assert!(w.says("px off: is it the right spot?"), "{:?}", w.words());
        // Switched off, the others line it up again.
        w.press(&mut show, &mut st, "Line-up ▸ Point 3 ▸ On");
        assert!(!st.line_up.as_ref().unwrap().points[2].on);
        assert!(!w.says("Point 3 is"), "{:?}", w.words());
        w.press(&mut show, &mut st, "Line-up ▸ Done");
        let p = show.projector(id).unwrap().clone();
        assert!(
            close(&p.placement, &truth, 0.02, 0.15),
            "{:?} is where it truly is, {truth:?}",
            p.placement
        );
        let lu = p.line_up.unwrap();
        assert_eq!(lu.points.len(), 8, "the switched-off point is kept");
        assert!(!lu.points[2].on);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// A line-up with four points placed (not lined up), started by mouse.
fn four_placed(modern: bool, name: &str) -> (Show, Win, UiState, ProjectorId, PathBuf) {
    let (mut show, id, s, _) = rig(Preset::Flat);
    let dir = scratch(name);
    let (mut w, mut st) = Win::new(modern, &mut show, &dir);
    start_by_mouse(&mut w, &mut show, &mut st);
    let middle = super::super::stage_projectors::screen_middle(show.stage3d.surface(s).unwrap());
    for c in room_corners(&show, s) {
        drop_point(&mut w, &mut show, &mut st, c, middle);
    }
    (show, w, st, id, dir)
}

#[test]
fn cancel_leaves_the_show_exactly_as_it_was() {
    for modern in [false, true] {
        let (mut show, mut w, mut st, _, dir) = four_placed(modern, &format!("cancel-{modern}"));
        let before = serde_json::to_string(&show).unwrap();
        // Moved about and nudged, then cancelled.
        let pic = w.rect("Line-up ▸ picture");
        w.drag(
            &mut show,
            &mut st,
            pic.center(),
            pic.center() + vec2(30.0, 10.0),
        );
        w.keys(&mut show, &mut st, &[(Key::ArrowRight, Modifiers::SHIFT)]);
        assert!(
            dir.join(DRAFT_FILE).exists(),
            "the draft is kept as it goes"
        );
        w.press(&mut show, &mut st, "Line-up ▸ Cancel");
        assert!(st.line_up.is_none());
        assert_eq!(serde_json::to_string(&show).unwrap(), before);
        assert!(!st.history.can_undo(), "nothing to undo");
        assert!(!dir.join(DRAFT_FILE).exists(), "the draft file went");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn back_to_before_line_up_puts_it_back_exactly() {
    for modern in [false, true] {
        let (mut show, id, s, truth) = rig(Preset::Flat);
        let entered = show.projector(id).unwrap().clone();
        let dir = scratch(&format!("back-{modern}"));
        let (mut w, mut st) = Win::new(modern, &mut show, &dir);
        start_by_mouse(&mut w, &mut show, &mut st);
        let corners = room_corners(&show, s);
        let middle =
            super::super::stage_projectors::screen_middle(show.stage3d.surface(s).unwrap());
        for c in &corners {
            drop_point(&mut w, &mut show, &mut st, *c, middle);
        }
        // Lined up near the truth (to a few pixels: enough to move it).
        let pic = w.rect("Line-up ▸ picture");
        let scale = pic.width() / HD.0 as f32;
        let pos = |px: [f32; 2]| pic.min + vec2(px[0], px[1]) * scale;
        for (i, c) in corners.iter().enumerate() {
            let now = st.line_up.as_ref().unwrap().points[i].pixel;
            let to = pos(truly(&show, id, &truth, *c));
            w.drag(&mut show, &mut st, pos(now), to);
        }
        w.press(&mut show, &mut st, "Line-up ▸ Done");
        assert_ne!(show.projector(id).unwrap().placement, entered.placement);
        // From the projector's row, any time after.
        let name = entered.name.clone();
        w.press(&mut show, &mut st, &format!("{BACK} {name}"));
        let p = show.projector(id).unwrap();
        assert_eq!(p.placement, entered.placement, "exactly where it was");
        assert_eq!(p.lens, entered.lens);
        let lu = p.line_up.as_ref().unwrap();
        assert_eq!(lu.points.len(), 4, "the points are kept");
        assert!(!lu.is_current());
        assert!(lu.before.is_some(), "and it can be done again");
        // And from the panes, in the middle of lining it up again.
        start_by_mouse(&mut w, &mut show, &mut st);
        assert_eq!(st.line_up.as_ref().unwrap().points.len(), 4, "carried on");
        let pic = w.rect("Line-up ▸ picture");
        w.drag(
            &mut show,
            &mut st,
            pic.center(),
            pic.center() + vec2(-20.0, 0.0),
        );
        w.press(&mut show, &mut st, "Line-up ▸ Back");
        assert!(st.line_up.is_none());
        let p = show.projector(id).unwrap();
        assert_eq!(p.placement, entered.placement);
        assert_eq!(p.lens, entered.lens);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn show_mode_refuses_line_up_and_puts_one_in_progress_away() {
    for modern in [false, true] {
        let (mut show, mut w, mut st, id, dir) = four_placed(modern, &format!("show-{modern}"));
        let points = st.line_up.as_ref().unwrap().points.clone();
        let session = st.line_up.as_ref().unwrap().session;
        st.show_mode = true;
        w.settle(&mut show, &mut st);
        assert!(st.line_up.is_none(), "put away: no crosses in Show mode");
        assert!(w.says(SHOW_MODE_WORDS), "{:?}", w.words());
        assert!(dir.join(DRAFT_FILE).exists(), "kept");
        // Refused from the row too.
        let name = show.projector(id).unwrap().name.clone();
        w.press(&mut show, &mut st, &format!("Line up {name}"));
        assert!(st.line_up.is_none());
        assert!(st.stage.projectors.lineup.asking.is_none());
        start(&show, &mut st, id);
        assert!(st.line_up.is_none(), "refused however it is asked");
        // Out of Show mode it carries on, in a new session.
        st.show_mode = false;
        w.settle(&mut show, &mut st);
        w.press(&mut show, &mut st, "Line-up ▸ Carry on");
        let d = st.line_up.as_ref().unwrap();
        assert_eq!(d.points, points);
        assert_ne!(d.session, session);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn a_line_up_in_progress_survives_a_restart() {
    for modern in [false, true] {
        let (mut show, mut w, mut st, id, dir) = four_placed(modern, &format!("restart-{modern}"));
        let pic = w.rect("Line-up ▸ picture");
        w.drag(
            &mut show,
            &mut st,
            pic.center(),
            pic.center() + vec2(25.0, -15.0),
        );
        let d = st.line_up.clone().unwrap();
        drop((w, st));
        // A new run of NewQ, the same show.
        let (mut w, mut st) = Win::new(modern, &mut show, &dir);
        let name = show.projector(id).unwrap().name.clone();
        assert!(
            w.says(&format!("Carry on lining up {name}?")),
            "{:?}",
            w.words()
        );
        w.press(&mut show, &mut st, "Line-up ▸ Carry on");
        let back = st.line_up.clone().expect("carried on");
        assert_eq!(back.points, d.points);
        assert_eq!(back.pixels, d.pixels);
        assert_eq!(back.before, d.before);
        assert_eq!(back.active, d.active);
        assert_ne!(back.session, d.session, "a new session");
        assert!(w.find("Line-up ▸ model").is_some());
        // Another show isn't offered it; Forget it takes it away.
        let mut other = Show {
            name: "Another".into(),
            ..Default::default()
        };
        let (w2, _) = Win::new(modern, &mut other, &dir);
        assert!(!w2.says("Carry on lining up"), "{:?}", w2.words());
        w.press(&mut show, &mut st, "Line-up ▸ Cancel");
        let (mut w3, mut st3) = Win::new(modern, &mut show, &dir);
        assert!(!w3.says("Carry on lining up"));
        // Forget it.
        start(&show, &mut st3, id);
        w3.settle(&mut show, &mut st3);
        drop((w3, st3));
        let (mut w4, mut st4) = Win::new(modern, &mut show, &dir);
        assert!(w4.says("Carry on lining up"));
        w4.press(&mut show, &mut st4, "Line-up ▸ Forget");
        assert!(!dir.join(DRAFT_FILE).exists());
        assert!(st4.line_up.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// The pure parts.

#[test]
fn the_arrow_keys_step_one_ten_and_a_tenth_and_leave_other_keys() {
    let key = |key, modifiers| Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    };
    let mut events = vec![
        key(Key::ArrowRight, Modifiers::NONE),
        key(Key::ArrowRight, Modifiers::SHIFT),
        key(Key::ArrowUp, Modifiers::ALT),
        key(Key::ArrowDown, Modifiers::NONE),
        key(Key::Space, Modifiers::NONE),
        Event::Key {
            key: Key::ArrowLeft,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: Modifiers::NONE,
        },
    ];
    let d = arrows(&mut events);
    assert_eq!(d[0], 11.0);
    assert!((d[1] - 0.9).abs() < 1e-6, "{d:?}");
    assert_eq!(events.len(), 2, "Space and a key let go are left");
}

#[test]
fn corners_are_each_screens_own_once() {
    assert_eq!(corners(&SurfaceShape::default()).len(), 4);
    // A curved wall's edge, top and bottom.
    let curved = Preset::Curved.shape();
    let n = corners(&curved).len();
    assert!(n > 8 && n.is_multiple_of(2), "{n}");
    // Facets that touch share their corners: a 6 x 3 wall has 7 x 4.
    assert_eq!(corners(&Preset::AngledWall.shape()).len(), 28);
}

#[test]
fn the_outline_is_each_screens_edge_where_the_lens_throws_it() {
    let (show, id, s, _) = rig(Preset::Flat);
    let p = show.projector(id).unwrap();
    let o = outline(&show.stage3d, &p.placement, &p.lens, HD);
    assert_eq!(o.len(), 1, "one run round the screen");
    assert_eq!(o[0].len(), 5, "four corners and back");
    let want: Vec<[f32; 2]> = room_corners(&show, s)
        .into_iter()
        .map(|c| calib::project(&p.placement, &p.lens, HD, c).unwrap())
        .collect();
    for v in &o[0] {
        assert!(
            want.iter()
                .any(|w| (w[0] - v[0]).abs() < 1e-3 && (w[1] - v[1]).abs() < 1e-3),
            "{v:?} is a corner"
        );
    }
    // Behind the lens: nothing.
    let mut away = p.placement;
    away.yaw += 180.0;
    assert!(outline(&show.stage3d, &away, &p.lens, HD).is_empty());
}

#[test]
fn a_picture_flipped_on_one_flat_screen_is_refused_as_mirrored() {
    let (show, id, s, truth) = rig(Preset::Flat);
    let p = show.projector(id).unwrap().clone();
    let screen = show.stage3d.surface(s).unwrap().clone();
    let m = screen.placement.matrix();
    let points = |flip: bool| -> Vec<LinePoint> {
        corners(&screen.shape)
            .into_iter()
            .map(|c| {
                let [x, y] = truly(&show, id, &truth, geometry::point(&m, c));
                let x = if flip { HD.0 as f32 - x } else { x };
                LinePoint::on_screen(s, c, [x, y])
            })
            .collect()
    };
    let mut draft = LineUpDraft::new(id, HD, 1);
    draft.points = points(false);
    let sol = solve(&show, &p, &draft, None).expect("solves");
    assert!(close(&sol.placement(), &truth, 1e-3, 1e-2));
    draft.points = points(true);
    let r = solve(&show, &p, &draft, None);
    assert_eq!(r.err(), Some(Refusal::Mirrored), "flipped is refused");
    // A rear screen is lit from behind: there the right way round is
    // behind it.
    let mut rear = show.clone();
    rear.stage3d.surface_mut(s).unwrap().rear = true;
    draft.points = points(false);
    let r = solve(&rear, &p, &draft, None);
    assert!(
        rear.kind_of(rear.stage3d.surface(s).unwrap()) != ScreenKind::Rear
            || r == Err(Refusal::Mirrored),
        "{r:?}"
    );
}

#[test]
fn a_new_point_starts_where_the_lens_throws_it() {
    let (show, id, s, _) = rig(Preset::Flat);
    let p = show.projector(id).unwrap().clone();
    let draft = LineUpDraft::new(id, HD, 1);
    let c = room_corners(&show, s)[0];
    let middle = super::super::stage_projectors::screen_middle(show.stage3d.surface(s).unwrap());
    let off_middle = [
        (c[0] + middle[0]) / 2.0,
        (c[1] + middle[1]) / 2.0,
        middle[2],
    ];
    let px = first_pixel(&show, &p, &Panes::default(), &draft, off_middle);
    let want = calib::project(&p.placement, &p.lens, HD, off_middle).unwrap();
    assert_eq!(px, want);
    // A corner a hair outside the picture is put on its edge.
    let px = first_pixel(&show, &p, &Panes::default(), &draft, c);
    assert!(
        px.iter()
            .zip([1920.0, 1080.0])
            .all(|(v, n)| (0.0..=n).contains(v))
    );
    assert_ne!(px, [960.0, 540.0]);
    // Off the picture: its middle.
    let behind = [0.0, 0.0, 1e4];
    assert_eq!(
        first_pixel(&show, &p, &Panes::default(), &draft, behind),
        [960.0, 540.0]
    );
}

/// Writes a show with a sample venue, a screen and a projector, and a
/// line-up of it in progress, to try by hand (or headless):
/// `NEWQ_LINEUP_SHOW=/abs/show.newq NEWQ_LINEUP_NODE=/abs/node cargo test
/// write_a_line_up_show -- --ignored`, then run NewQ with that
/// `NEWQ_NODE_DIR` on that show: Settings ▸ 3D stage offers to carry on.
#[test]
#[ignore]
fn write_a_line_up_show() {
    let (Some(path), Some(node)) = (
        std::env::var_os("NEWQ_LINEUP_SHOW"),
        std::env::var_os("NEWQ_LINEUP_NODE"),
    ) else {
        return;
    };
    let path = PathBuf::from(path);
    let (mut show, id, s, truth) = rig(Preset::Flat);
    show.name = "Line-up".into();
    crate::ui::stage_edit::use_sample(
        &mut show,
        crate::stage3d::samples::SampleVenue::Proscenium,
        false,
    );
    std::fs::write(&path, serde_json::to_string_pretty(&show).unwrap()).unwrap();
    let mut draft = LineUpDraft::new(id, HD, 7);
    let screen = show.stage3d.surface(s).unwrap().clone();
    let m = screen.placement.matrix();
    let p = show.projector(id).unwrap().clone();
    for (i, c) in corners(&screen.shape).into_iter().enumerate() {
        let room = geometry::point(&m, c);
        // The first three where they truly are; the last still where the
        // projector as entered throws it.
        let px = if i < 3 {
            truly(&show, id, &truth, room)
        } else {
            calib::project(&p.placement, &p.lens, HD, room).unwrap()
        };
        draft.points.push(LinePoint::on_screen(s, c, px));
    }
    draft.active = Some(3);
    draft.before = Some(Before {
        placement: p.placement,
        lens: p.lens.clone(),
        extra: Default::default(),
    });
    let mut panes = Panes {
        draft_dir: Some(PathBuf::from(node)),
        ..Default::default()
    };
    panes.show = Some((Some(path), show.name.clone()));
    panes.write(&draft);
}
