//! The Preview pane's multiview layout, kept in the show: how many tiles, their
//! arrangement (a grid, or one large tile with small ones along the bottom), and
//! which output each tile shows (`None`: empty). With no layout in the show the
//! multiview shows every output in an automatic grid, as 0.24.7 did.
//!
//! Pure: the model, its edits and where each tile goes in a pane. The drawing is
//! `ui.rs::multiview_grid`.

use egui::{Rect, pos2, vec2};
use serde::{Deserialize, Deserializer, Serialize};

use crate::show::OutputId;

/// The fewest and most tiles a layout has.
pub const MIN_TILES: usize = 2;
pub const MAX_TILES: usize = crate::gpu::MULTI_SLOTS;

/// How the tiles are arranged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    /// `cols` across, `rows` down, filled row by row.
    Grid { cols: u8, rows: u8 },
    /// One large tile across the top, `small` small ones in a row under it
    /// (a broadcast multiviewer's "1 + N").
    Big { small: u8 },
}

impl Shape {
    /// The choices the layout menu offers.
    pub const CHOICES: [Shape; 11] = [
        Shape::Grid { cols: 2, rows: 1 },
        Shape::Grid { cols: 3, rows: 1 },
        Shape::Grid { cols: 2, rows: 2 },
        Shape::Grid { cols: 3, rows: 2 },
        Shape::Grid { cols: 4, rows: 2 },
        Shape::Grid { cols: 3, rows: 3 },
        Shape::Grid { cols: 4, rows: 3 },
        Shape::Grid { cols: 4, rows: 4 },
        Shape::Big { small: 3 },
        Shape::Big { small: 4 },
        Shape::Big { small: 5 },
    ];

    /// How many tiles (2 to 16, whatever a file says).
    pub fn count(self) -> usize {
        let n = match self {
            Shape::Grid { cols, rows } => usize::from(cols.max(1)) * usize::from(rows.max(1)),
            Shape::Big { small } => 1 + usize::from(small),
        };
        n.clamp(MIN_TILES, MAX_TILES)
    }

    /// Columns of the grid (of the small row, for `Big`), at least one.
    fn cols(self) -> usize {
        match self {
            Shape::Grid { cols, .. } => usize::from(cols.max(1)).min(MAX_TILES),
            Shape::Big { small } => usize::from(small.max(1)).min(MAX_TILES - 1),
        }
    }

    /// What the menu calls it.
    pub fn label(self) -> String {
        match self {
            Shape::Grid { cols, rows } => format!("{cols} × {rows}"),
            Shape::Big { small } => format!("1 large + {small} small"),
        }
    }

    /// The smallest grid offered that holds `n` tiles (4 × 4 past 16).
    pub fn grid_for(n: usize) -> Shape {
        Shape::CHOICES
            .into_iter()
            .filter(|s| matches!(s, Shape::Grid { .. }))
            .find(|s| s.count() >= n)
            .unwrap_or(Shape::Grid { cols: 4, rows: 4 })
    }
}

/// A multiview the user arranged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    pub shape: Shape,
    /// What each tile shows, in order (row by row; for `Big`, the large one
    /// first). Kept as long as `shape.count()` by `tidy`.
    #[serde(default)]
    pub tiles: Vec<Option<OutputId>>,
}

impl Layout {
    /// `shape`, its tiles showing `outputs` in turn (empty past them).
    pub fn new(shape: Shape, outputs: &[OutputId]) -> Layout {
        let mut l = Layout {
            shape,
            tiles: Vec::new(),
        };
        l.set_shape(shape, outputs);
        l
    }

    /// The layout the automatic multiview has for `outputs`: a grid holding
    /// them, each in its turn. Where an edit of the automatic view starts.
    pub fn from_auto(outputs: &[OutputId]) -> Layout {
        Layout::new(Shape::grid_for(outputs.len()), outputs)
    }

    /// Change the shape: tiles that stay keep what they show, those that go are
    /// dropped, and new ones show outputs not yet in a tile, in `outputs`'
    /// order, then nothing.
    pub fn set_shape(&mut self, shape: Shape, outputs: &[OutputId]) {
        self.shape = shape;
        let n = shape.count();
        self.tiles.truncate(n);
        let mut unused = outputs
            .iter()
            .copied()
            .filter(|o| !self.tiles.contains(&Some(*o)))
            .collect::<Vec<_>>()
            .into_iter();
        while self.tiles.len() < n {
            self.tiles.push(unused.next());
        }
    }

    /// Show `output` (or nothing) in tile `i`; the same output may be in
    /// several tiles. A tile past the end is ignored.
    pub fn assign(&mut self, i: usize, output: Option<OutputId>) {
        self.tidy();
        if let Some(t) = self.tiles.get_mut(i) {
            *t = output;
        }
    }

    /// Swap what tiles `a` and `b` show.
    pub fn swap(&mut self, a: usize, b: usize) {
        self.tidy();
        let n = self.tiles.len();
        if a < n && b < n {
            self.tiles.swap(a, b);
        }
    }

    /// An output that was deleted: the tiles that showed it are empty.
    pub fn forget(&mut self, output: OutputId) {
        for t in &mut self.tiles {
            if *t == Some(output) {
                *t = None;
            }
        }
    }

    /// Exactly `shape.count()` tiles (a file may hold more or fewer).
    pub fn tidy(&mut self) {
        self.tiles.resize(self.shape.count(), None);
    }

    /// What tile `i` shows: an output that `exists`, else nothing.
    pub fn tile(&self, i: usize, exists: impl Fn(OutputId) -> bool) -> Option<OutputId> {
        self.tiles.get(i).copied().flatten().filter(|o| exists(*o))
    }
}

/// `Show::multiview` from a file: a layout this NewQ can't read (a later one's
/// shape) is no layout, not a show that won't open.
pub fn lenient<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Layout>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value::<Option<Layout>>(v)
        .ok()
        .flatten()
        .map(|mut l| {
            l.tidy();
            l
        }))
}

/// Where one tile goes in the pane: its name line, the cell its picture is
/// fitted in (black bars round the picture), and the line under it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileRects {
    pub head: Rect,
    pub cell: Rect,
    pub foot: Rect,
}

/// How a layout is laid out in a pane.
#[derive(Clone, Copy, Debug)]
pub struct Room {
    /// The pane's top left.
    pub origin: egui::Pos2,
    /// The width in sight.
    pub width: f32,
    /// The height in sight below `origin` (the large tile keeps the small row in it).
    pub visible: f32,
    /// Space between tiles.
    pub gap: f32,
    /// Height of the name line and of the line under a picture.
    pub line: f32,
}

/// The tiles of `shape` in `room`, in order, and the height they take.
/// `canvas(i)` is tile `i`'s display's shape (an empty tile: 16:9). A row is
/// as tall as its tallest picture at the row's width, at most three quarters of
/// it (a tall sub display is shown narrower, as in the automatic grid).
pub fn arrange(
    shape: Shape,
    room: Room,
    canvas: impl Fn(usize) -> (u32, u32),
) -> (Vec<TileRects>, f32) {
    let Room {
        origin,
        width,
        visible,
        gap,
        line,
    } = room;
    let width = width.max(1.0);
    let n = shape.count();
    let cols = shape.cols();
    let small_w = ((width - gap * (cols as f32 - 1.0)) / cols as f32).max(1.0);
    let natural = |i: usize, w: f32| {
        let (cw, ch) = canvas(i);
        let h = w * ch.max(1) as f32 / cw.max(1) as f32;
        h.min(w * 0.75).max(1.0)
    };
    let mut out = Vec::with_capacity(n);
    let mut y = origin.y;
    let place = |x: f32, y: f32, w: f32, h: f32, out: &mut Vec<TileRects>| {
        out.push(TileRects {
            head: Rect::from_min_size(pos2(x, y), vec2(w, line)),
            cell: Rect::from_min_size(pos2(x, y + line), vec2(w, h)),
            foot: Rect::from_min_size(pos2(x, y + line + h), vec2(w, line)),
        });
    };
    let first_small = match shape {
        Shape::Big { .. } => {
            // The small row first, to know what the large one may take.
            let small_h = (1..n).map(|i| natural(i, small_w)).fold(1.0, f32::max);
            let below = gap + 2.0 * line + small_h;
            let room_h = (visible - 2.0 * line - below).max(60.0);
            let h = natural(0, width).min(room_h);
            place(origin.x, y, width, h, &mut out);
            y += 2.0 * line + h + gap;
            1
        }
        Shape::Grid { .. } => 0,
    };
    let mut i = first_small;
    while i < n {
        let end = (i + cols).min(n);
        let h = (i..end).map(|t| natural(t, small_w)).fold(1.0, f32::max);
        for c in 0..end - i {
            place(
                origin.x + c as f32 * (small_w + gap),
                y,
                small_w,
                h,
                &mut out,
            );
        }
        y += 2.0 * line + h + gap;
        i += cols;
    }
    (out, (y - origin.y - gap).max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[u64]) -> Vec<OutputId> {
        v.iter().map(|&n| OutputId(n)).collect()
    }

    #[test]
    fn shapes_count_two_to_sixteen_tiles() {
        assert_eq!(Shape::Grid { cols: 2, rows: 1 }.count(), 2);
        assert_eq!(Shape::Grid { cols: 3, rows: 3 }.count(), 9);
        assert_eq!(Shape::Grid { cols: 4, rows: 4 }.count(), 16);
        assert_eq!(Shape::Big { small: 4 }.count(), 5);
        // Whatever a file says.
        assert_eq!(Shape::Grid { cols: 0, rows: 0 }.count(), 2);
        assert_eq!(Shape::Grid { cols: 9, rows: 9 }.count(), 16);
        assert_eq!(Shape::Big { small: 40 }.count(), 16);
        for s in Shape::CHOICES {
            assert!((MIN_TILES..=MAX_TILES).contains(&s.count()), "{s:?}");
        }
        assert_eq!(Shape::grid_for(1), Shape::Grid { cols: 2, rows: 1 });
        assert_eq!(Shape::grid_for(5), Shape::Grid { cols: 3, rows: 2 });
        assert_eq!(Shape::grid_for(9), Shape::Grid { cols: 3, rows: 3 });
        assert_eq!(Shape::grid_for(20), Shape::Grid { cols: 4, rows: 4 });
        assert_eq!(Shape::Grid { cols: 3, rows: 2 }.label(), "3 × 2");
        assert_eq!(Shape::Big { small: 3 }.label(), "1 large + 3 small");
    }

    #[test]
    fn a_new_shape_keeps_what_tiles_show_and_fills_new_ones_with_the_rest() {
        let outs = ids(&[1, 2, 3, 4, 5]);
        let mut l = Layout::from_auto(&outs);
        assert_eq!(l.shape, Shape::Grid { cols: 3, rows: 2 });
        assert_eq!(
            l.tiles,
            vec![
                Some(OutputId(1)),
                Some(OutputId(2)),
                Some(OutputId(3)),
                Some(OutputId(4)),
                Some(OutputId(5)),
                None
            ]
        );
        // Fewer tiles: the last go.
        l.set_shape(Shape::Grid { cols: 2, rows: 1 }, &outs);
        assert_eq!(l.tiles, vec![Some(OutputId(1)), Some(OutputId(2))]);
        // More: the outputs not shown, in order, then empty.
        l.assign(0, Some(OutputId(4)));
        l.set_shape(Shape::Grid { cols: 3, rows: 2 }, &outs);
        assert_eq!(
            l.tiles,
            vec![
                Some(OutputId(4)),
                Some(OutputId(2)),
                Some(OutputId(1)),
                Some(OutputId(3)),
                Some(OutputId(5)),
                None
            ]
        );
    }

    #[test]
    fn tiles_are_assigned_and_swapped_and_the_same_output_may_be_in_two() {
        let mut l = Layout::new(Shape::Grid { cols: 2, rows: 2 }, &ids(&[7, 8]));
        assert_eq!(
            l.tiles,
            vec![Some(OutputId(7)), Some(OutputId(8)), None, None]
        );
        l.assign(3, Some(OutputId(7)));
        l.assign(9, Some(OutputId(8))); // past the end: nothing
        assert_eq!(
            l.tiles,
            vec![
                Some(OutputId(7)),
                Some(OutputId(8)),
                None,
                Some(OutputId(7))
            ]
        );
        l.swap(1, 2);
        assert_eq!(
            l.tiles,
            vec![
                Some(OutputId(7)),
                None,
                Some(OutputId(8)),
                Some(OutputId(7))
            ]
        );
        l.swap(0, 99);
        assert_eq!(l.tiles[0], Some(OutputId(7)));
        l.assign(0, None);
        assert_eq!(l.tiles[0], None);
    }

    #[test]
    fn a_deleted_output_leaves_its_tiles_empty() {
        let mut l = Layout::new(Shape::Grid { cols: 3, rows: 1 }, &ids(&[1, 2]));
        l.assign(2, Some(OutputId(2)));
        // Resolved against the outputs there are, before anything is cleared.
        let exists = |o: OutputId| o != OutputId(2);
        assert_eq!(l.tile(0, exists), Some(OutputId(1)));
        assert_eq!(l.tile(1, exists), None);
        assert_eq!(l.tile(2, exists), None);
        l.forget(OutputId(2));
        assert_eq!(l.tiles, vec![Some(OutputId(1)), None, None]);
    }

    #[test]
    fn a_layout_reads_back_and_one_from_a_later_newq_is_no_layout() {
        #[derive(Deserialize)]
        struct S {
            #[serde(default, deserialize_with = "lenient")]
            m: Option<Layout>,
        }
        let l = Layout::new(Shape::Big { small: 3 }, &ids(&[1, 2]));
        let json = serde_json::to_string(&l).unwrap();
        assert_eq!(
            json,
            r#"{"shape":{"big":{"small":3}},"tiles":[1,2,null,null]}"#
        );
        let back: S = serde_json::from_str(&format!(r#"{{"m":{json}}}"#)).unwrap();
        assert_eq!(back.m, Some(l));
        let none: S = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(none.m, None);
        let null: S = serde_json::from_str(r#"{"m":null}"#).unwrap();
        assert_eq!(null.m, None);
        let later: S =
            serde_json::from_str(r#"{"m":{"shape":{"ring":{"n":5}},"tiles":[1]}}"#).unwrap();
        assert_eq!(later.m, None);
        // Too few tiles in the file: made up to the shape's count.
        let short: S =
            serde_json::from_str(r#"{"m":{"shape":{"grid":{"cols":2,"rows":2}},"tiles":[3]}}"#)
                .unwrap();
        assert_eq!(
            short.m.unwrap().tiles,
            vec![Some(OutputId(3)), None, None, None]
        );
    }

    fn room(width: f32) -> Room {
        Room {
            origin: pos2(10.0, 20.0),
            width,
            visible: 2000.0,
            gap: 8.0,
            line: 16.0,
        }
    }

    #[test]
    fn a_grid_is_laid_out_row_by_row_in_the_width() {
        let hd = |_| (1920, 1080);
        let (tiles, height) = arrange(Shape::Grid { cols: 3, rows: 3 }, room(616.0), hd);
        assert_eq!(tiles.len(), 9);
        // 3 across, 200 wide, 8 apart; each cell 16:9.
        for (i, t) in tiles.iter().enumerate() {
            let (r, c) = (i / 3, i % 3);
            assert!((t.cell.width() - 200.0).abs() < 0.01, "{t:?}");
            assert!((t.cell.height() - 112.5).abs() < 0.01, "{t:?}");
            assert!((t.head.min.x - (10.0 + c as f32 * 208.0)).abs() < 0.01);
            let top = 20.0 + r as f32 * (112.5 + 32.0 + 8.0);
            assert!((t.head.min.y - top).abs() < 0.01, "{i}: {t:?}");
            assert_eq!(t.cell.min.y, t.head.max.y);
            assert_eq!(t.foot.min.y, t.cell.max.y);
            assert!(t.cell.right() <= 10.0 + 616.0 + 0.01);
        }
        assert!((height - (3.0 * (112.5 + 32.0) + 2.0 * 8.0)).abs() < 0.01);
        // A tall sub display makes its row taller, up to three quarters of the width.
        let (tiles, _) = arrange(Shape::Grid { cols: 2, rows: 1 }, room(408.0), |i| {
            if i == 1 { (1080, 1920) } else { (1920, 1080) }
        });
        assert!((tiles[0].cell.height() - 150.0).abs() < 0.01, "{tiles:?}");
        assert_eq!(tiles[0].cell.height(), tiles[1].cell.height());
    }

    #[test]
    fn one_large_tile_over_a_row_of_small_ones() {
        let hd = |_| (1920, 1080);
        let (tiles, height) = arrange(Shape::Big { small: 4 }, room(808.0), hd);
        assert_eq!(tiles.len(), 5);
        let big = tiles[0];
        assert!((big.cell.width() - 808.0).abs() < 0.01);
        assert!((big.cell.height() - 808.0 * 9.0 / 16.0).abs() < 0.01);
        for (c, t) in tiles[1..].iter().enumerate() {
            assert!((t.cell.width() - 196.0).abs() < 0.01, "{t:?}");
            assert!(t.head.min.y > big.foot.max.y, "under the large one");
            assert!((t.head.min.x - (10.0 + c as f32 * 204.0)).abs() < 0.01);
        }
        assert!(height > big.cell.height() + 100.0);
        // A short pane: the large one gives way so the small row stays in sight.
        let mut short = room(808.0);
        short.visible = 400.0;
        let (tiles, height) = arrange(Shape::Big { small: 4 }, short, hd);
        assert!(height <= 400.0 + 0.01, "{height}");
        assert!(tiles[0].cell.height() < 808.0 * 9.0 / 16.0);
        assert!(tiles[4].foot.max.y <= 20.0 + 400.0 + 0.01);
    }
}
