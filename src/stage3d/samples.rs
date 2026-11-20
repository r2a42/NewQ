//! NewQ's own sample venues, made in code (pure): an arena, a proscenium
//! theatre, a thrust stage, a black box, a club, a festival main stage and a
//! ballroom, for a user without a model of their own. No files are shipped:
//! nothing to license, exact metres, and each is a few thousand triangles.
//!
//! Every sample is in metres, Y up, the floor at y = 0 (a theatre's orchestra
//! pit goes below it, `SampleVenue::lowest`), the stage towards -Z and the
//! audience towards +Z, so a screen facing +Z faces them. Seating is stepped
//! tiers, never single chairs. Where the origin is differs (`SampleVenue`'s
//! variants say).
//!
//! `sample_venue` gives the same `LoadedVenue` a model file does (one part
//! per material, plain colours, lit), so the renderer doesn't know the
//! difference. `screens` are the usual screens of each venue, unfed, to add
//! with `Show::add_sample_screens`; `viewpoints` a few places to look from.

use super::geometry::{self, Mat4};
use super::load::{Builder, LoadedVenue, Look};
use super::scene::{IDENTITY, Mesh};
use super::{Placement, SurfaceShape, Venue, Viewpoint};
use serde::{Deserialize, Serialize};

/// A sample venue. Written in a show under the variant's name; a name a later
/// NewQ writes reads as the default (the black box), so the show still loads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub enum SampleVenue {
    /// A 60 x 30 m floor (ice or court sized) in a bowl of lower and upper
    /// tiers, an end stage at -Z, a roof with a truss grid and a centre-hung
    /// rigging point. Origin: the middle of the floor.
    Arena,
    /// A 12 x 7 m proscenium arch, a stage 18 m wide (with wings) and 10 m
    /// deep under a fly tower, an orchestra pit, raked stalls and a circle.
    /// Origin: the middle of the stage's front edge (the setting line), on the
    /// stalls floor.
    Proscenium,
    /// A stage that runs out into the audience, seated on three sides, with
    /// vomitories at its front corners and a stage behind it. Origin: where
    /// the thrust meets the stage behind, on the floor.
    Thrust,
    /// A 15 x 12 m black room, 7 m high, with a pipe grid and a block of
    /// retractable risers. Origin: the middle of the floor.
    #[default]
    BlackBox,
    /// A 22 x 22 m club with a 5 m ceiling: a DJ booth on a riser before a
    /// clear back wall, a dance floor, a bar along one side and columns.
    /// Origin: the middle of the room's floor.
    Club,
    /// An outdoor main stage: a roof on four towers, scaffold wings for the
    /// side screens, a crowd barrier and a front-of-house tent. Origin: the
    /// middle of the stage's front edge, on the ground.
    Festival,
    /// A 30 x 24 m ballroom set for a dinner or a conference: round tables,
    /// a low stage, chandeliers. Origin: the middle of the floor.
    Ballroom,
}

impl<'de> Deserialize<'de> for SampleVenue {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let name = String::deserialize(d)?;
        // Written under the variant's own name, which is also its Debug name.
        Ok(SampleVenue::ALL
            .into_iter()
            .find(|k| format!("{k:?}") == name)
            .unwrap_or_default())
    }
}

impl SampleVenue {
    pub const ALL: [SampleVenue; 7] = [
        SampleVenue::Arena,
        SampleVenue::Proscenium,
        SampleVenue::Thrust,
        SampleVenue::BlackBox,
        SampleVenue::Club,
        SampleVenue::Festival,
        SampleVenue::Ballroom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SampleVenue::Arena => "Arena",
            SampleVenue::Proscenium => "Proscenium theatre",
            SampleVenue::Thrust => "Thrust stage",
            SampleVenue::BlackBox => "Black box studio",
            SampleVenue::Club => "Club",
            SampleVenue::Festival => "Festival main stage",
            SampleVenue::Ballroom => "Ballroom",
        }
    }

    /// The lowest point of the venue, in metres: 0 (the floor) but for the
    /// theatre's orchestra pit.
    pub fn lowest(self) -> f32 {
        match self {
            SampleVenue::Proscenium => -PIT_DEPTH,
            _ => 0.0,
        }
    }
}

/// The sample, made, as `load::load_venue` gives a model file: one part per
/// material, its colour times the venue's brightness.
pub fn sample_venue(kind: SampleVenue, venue: &Venue) -> Result<LoadedVenue, String> {
    let kit = build(kind);
    let mut b = Builder::new(venue.brightness());
    for (name, colour, mesh) in kit.parts {
        b.add(
            name,
            || Look {
                colour,
                texture: None,
            },
            mesh,
            &IDENTITY,
        );
    }
    b.finish(kind.label(), venue)
}

/// The sample as a .glb file's bytes, to change in a 3D program and load
/// back as a model file: metres, Y up, a mesh and material per colour.
pub fn sample_glb(kind: SampleVenue) -> Vec<u8> {
    let kit = build(kind);
    let parts: Vec<super::glb::GlbPart> = kit
        .parts
        .iter()
        .map(|(name, colour, mesh)| super::glb::GlbPart {
            name,
            colour: *colour,
            mesh,
        })
        .collect();
    super::glb::write(&parts)
}

/// The meshes of a sample by material: (name, linear colour, mesh).
pub fn build(kind: SampleVenue) -> Kit {
    let mut k = Kit::default();
    match kind {
        SampleVenue::Arena => arena(&mut k),
        SampleVenue::Proscenium => proscenium(&mut k),
        SampleVenue::Thrust => thrust(&mut k),
        SampleVenue::BlackBox => black_box(&mut k),
        SampleVenue::Club => club(&mut k),
        SampleVenue::Festival => festival(&mut k),
        SampleVenue::Ballroom => ballroom(&mut k),
    }
    k
}

/// A screen a sample usually has, in the venue's own frame.
#[derive(Clone, Debug, PartialEq)]
pub struct StarterScreen {
    pub name: &'static str,
    pub placement: Placement,
    pub shape: SurfaceShape,
}

fn at(position: [f32; 3], yaw: f32) -> Placement {
    Placement {
        position,
        yaw,
        ..Placement::default()
    }
}

/// Facing up (a floor projection): its picture's top is towards -Z.
fn face_up(position: [f32; 3]) -> Placement {
    Placement {
        position,
        pitch: -90.0,
        ..Placement::default()
    }
}

fn flat(width: f32, height: f32) -> SurfaceShape {
    SurfaceShape::Flat { width, height }
}

/// The venue's usual screens, unfed, in its own frame. Each stands clear of
/// what is behind it (a screen flush on a wall would flicker against it).
pub fn screens(kind: SampleVenue) -> Vec<StarterScreen> {
    let s = |name, placement, shape| StarterScreen {
        name,
        placement,
        shape,
    };
    match kind {
        SampleVenue::Arena => vec![
            s(
                "Centre-hung (4 sides)",
                at([0.0, 16.0, 0.0], 0.0),
                SurfaceShape::Faceted {
                    facets: geometry::jumbotron(4, 8.0, 4.5, true, false),
                },
            ),
            s(
                "Upstage LED wall",
                at([0.0, 7.2, -28.8], 0.0),
                flat(16.0, 9.0),
            ),
            s("IMAG left", at([-15.0, 8.5, -22.5], 20.0), flat(8.0, 4.5)),
            s("IMAG right", at([15.0, 8.5, -22.5], -20.0), flat(8.0, 4.5)),
        ],
        SampleVenue::Proscenium => vec![
            s(
                "Proscenium projection screen",
                at([0.0, 4.55, -1.0], 0.0),
                flat(11.0, 6.19),
            ),
            s(
                "Upstage cyc",
                at([0.0, 4.8, -8.8], 0.0),
                SurfaceShape::Curved {
                    width: 18.0,
                    radius: 14.0,
                    height: 7.0,
                    convex: false,
                },
            ),
        ],
        SampleVenue::Thrust => vec![
            s(
                "Upstage screen",
                at([0.0, 4.4, -6.2], 0.0),
                flat(12.0, 6.75),
            ),
            s("Thrust floor", face_up([0.0, 0.63, 3.5]), flat(8.0, 6.0)),
        ],
        SampleVenue::BlackBox => vec![
            s("Back wall", at([0.0, 3.0, -5.8], 0.0), flat(9.0, 5.06)),
            s("Floor", face_up([0.0, 0.03, -3.0]), flat(6.0, 4.0)),
        ],
        SampleVenue::Club => vec![
            s(
                "DJ wall",
                at([0.0, 3.1, -8.5], 0.0),
                SurfaceShape::Faceted {
                    facets: geometry::angled_wall(5, 3, 1.5, 1.0, 8.0),
                },
            ),
            s("DJ booth front", at([0.0, 1.55, -5.7], 0.0), flat(3.4, 0.9)),
            s(
                "Pillar wrap",
                at([CLUB_COLUMNS[0][0], 2.0, CLUB_COLUMNS[0][1] + 0.42], 0.0),
                SurfaceShape::Curved {
                    width: std::f32::consts::TAU * 0.42,
                    radius: 0.42,
                    height: 2.0,
                    convex: true,
                },
            ),
        ],
        SampleVenue::Festival => vec![
            s(
                "Upstage LED wall",
                at([0.0, 8.0, -13.4], 0.0),
                flat(19.2, 10.8),
            ),
            s("IMAG left", at([-18.25, 10.0, -1.0], 12.0), flat(9.0, 5.06)),
            s(
                "IMAG right",
                at([18.25, 10.0, -1.0], -12.0),
                flat(9.0, 5.06),
            ),
        ],
        SampleVenue::Ballroom => vec![
            s(
                "Centre screen",
                at([0.0, 2.9, -11.5], 0.0),
                flat(6.0, 3.375),
            ),
            s(
                "Side screen left",
                at([-10.0, 2.6, -10.5], 25.0),
                flat(4.0, 2.25),
            ),
            s(
                "Side screen right",
                at([10.0, 2.6, -10.5], -25.0),
                flat(4.0, 2.25),
            ),
        ],
    }
}

/// Places to look from, in the venue's own frame; the first is where the
/// audience sees the screens best.
pub fn viewpoints(kind: SampleVenue) -> Vec<Viewpoint> {
    let v = |name: &str, eye, target| Viewpoint {
        name: name.into(),
        eye,
        target,
        fov_deg: 50.0,
    };
    match kind {
        SampleVenue::Arena => vec![
            v("Front of house", [0.0, 3.0, 10.0], [0.0, 7.0, -28.0]),
            v("Upper tier, far end", [0.0, 15.2, 54.5], [0.0, 6.0, -26.0]),
            v("From the stage", [0.0, 3.6, -20.0], [0.0, 5.0, 30.0]),
        ],
        SampleVenue::Proscenium => vec![
            v("Stalls", [0.0, 2.8, 15.0], [0.0, 4.5, -2.0]),
            v("Circle", [0.0, 9.45, 23.55], [0.0, 4.5, -2.0]),
            v("From the apron", [0.0, 2.7, 0.8], [0.0, 4.0, 20.0]),
        ],
        SampleVenue::Thrust => vec![
            v("Centre block", [0.0, 4.1, 15.3], [0.0, 2.0, 0.0]),
            v("Side block", [-12.35, 4.1, 3.0], [0.0, 2.0, 1.0]),
            v("From the stage", [0.0, 2.3, 5.0], [0.0, 3.0, 18.0]),
        ],
        SampleVenue::BlackBox => vec![
            v("From the risers", [0.0, 2.35, 3.75], [0.0, 1.5, -4.5]),
            v("From the stage", [0.0, 1.7, -4.5], [0.0, 1.5, 3.0]),
            v("Overhead", [5.0, 5.5, -1.0], [0.0, 0.0, -2.0]),
        ],
        SampleVenue::Club => vec![
            v("Dance floor", [0.0, 1.75, 6.0], [0.0, 2.8, -8.5]),
            v("Back of the room", [-6.0, 2.15, 11.5], [0.0, 2.5, -8.0]),
            v("DJ booth", [0.0, 2.7, -7.2], [0.0, 1.6, 8.0]),
        ],
        SampleVenue::Festival => vec![
            v("Front of house", [0.0, 2.2, 36.0], [0.0, 8.0, -10.0]),
            v("At the barrier", [0.0, 1.7, 5.0], [0.0, 8.0, -12.0]),
            v("From the stage", [0.0, 3.8, -4.0], [0.0, 2.0, 40.0]),
        ],
        SampleVenue::Ballroom => vec![
            v("Back of the room", [0.0, 1.7, 11.0], [0.0, 2.6, -11.0]),
            v("At a table", [7.5, 1.2, 3.5], [0.0, 2.6, -11.0]),
            v("From the stage", [0.0, 2.3, -9.5], [0.0, 1.2, 10.0]),
        ],
    }
}

// ----- Materials ---------------------------------------------------------------

/// Linear colours, muted: the screens should be what stands out.
type Rgba = [f32; 4];
const CONCRETE: Rgba = [0.20, 0.20, 0.19, 1.0];
const ARENA_FLOOR: Rgba = [0.30, 0.30, 0.31, 1.0];
const SEATS_BLUE: Rgba = [0.05, 0.08, 0.16, 1.0];
const SEATS_RED: Rgba = [0.22, 0.03, 0.04, 1.0];
const SEATS_GREY: Rgba = [0.07, 0.07, 0.08, 1.0];
const WALL_GREY: Rgba = [0.22, 0.22, 0.23, 1.0];
const THEATRE_WALL: Rgba = [0.12, 0.08, 0.06, 1.0];
const STAGE_DECK: Rgba = [0.06, 0.05, 0.045, 1.0];
const MASKING: Rgba = [0.02, 0.02, 0.02, 1.0];
const METAL: Rgba = [0.35, 0.36, 0.38, 1.0];
const CEILING_DARK: Rgba = [0.05, 0.05, 0.055, 1.0];
const GOLD: Rgba = [0.45, 0.33, 0.12, 1.0];
const GRASS: Rgba = [0.09, 0.13, 0.05, 1.0];
const WOOD: Rgba = [0.22, 0.13, 0.07, 1.0];
const CANVAS: Rgba = [0.55, 0.55, 0.52, 1.0];
const CLUB_WALL: Rgba = [0.04, 0.03, 0.06, 1.0];
const CLUB_FLOOR: Rgba = [0.06, 0.06, 0.07, 1.0];
const COPPER: Rgba = [0.33, 0.13, 0.06, 1.0];
const CARPET: Rgba = [0.16, 0.04, 0.05, 1.0];
const BEIGE: Rgba = [0.42, 0.35, 0.25, 1.0];
const CREAM: Rgba = [0.50, 0.48, 0.44, 1.0];
const LINEN: Rgba = [0.60, 0.60, 0.58, 1.0];

/// A venue's meshes by material, in the order first used.
#[derive(Default)]
pub struct Kit {
    pub parts: Vec<(&'static str, Rgba, Mesh)>,
}

impl Kit {
    /// The mesh of a material, made on first use.
    fn m(&mut self, name: &'static str, colour: Rgba) -> &mut Mesh {
        let at = match self.parts.iter().position(|p| p.0 == name) {
            Some(i) => i,
            None => {
                self.parts.push((name, colour, Mesh::default()));
                self.parts.len() - 1
            }
        };
        &mut self.parts[at].2
    }

    pub fn triangles(&self) -> usize {
        self.parts.iter().map(|p| p.2.triangles()).sum()
    }
}

// ----- Pieces ------------------------------------------------------------------

const UP: [f32; 3] = [0.0, 1.0, 0.0];
const DOWN: [f32; 3] = [0.0, -1.0, 0.0];

/// A flat four-cornered face, wound so its front (CCW) and its normal are on
/// the side `hint` points to. Corners in order round the face; a face with no
/// area is left out, as is either half that has none.
fn face(m: &mut Mesh, q: [[f32; 3]; 4], hint: [f32; 3]) {
    // Newell's normal: the polygon's, for its corners' order.
    let mut n = [0.0f32; 3];
    for i in 0..4 {
        let (a, b) = (q[i], q[(i + 1) % 4]);
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    if geometry::dot(n, n) < 1e-12 {
        return;
    }
    let mut q = q;
    if geometry::dot(n, hint) < 0.0 {
        q.reverse();
        n = n.map(|v| -v);
    }
    let n = geometry::normalize(n);
    for tri in [[0, 1, 2], [0, 2, 3]] {
        let [a, b, c] = tri.map(|i| q[i]);
        let area = geometry::cross(geometry::sub(b, a), geometry::sub(c, a));
        if geometry::dot(area, area) < 1e-12 {
            continue;
        }
        let base = m.positions.len() as u32;
        m.positions.extend([a, b, c]);
        m.normals.extend([n; 3]);
        m.uvs.extend([[0.0, 0.0], [0.0, 1.0], [1.0, 1.0]]);
        m.indices.extend([base, base + 1, base + 2]);
    }
}

/// A flat convex polygon, fanned from its middle, facing `hint`.
fn polygon(m: &mut Mesh, pts: &[[f32; 3]], hint: [f32; 3]) {
    if pts.len() < 3 {
        return;
    }
    let k = pts.len() as f32;
    let c = [0, 1, 2].map(|i| pts.iter().map(|p| p[i]).sum::<f32>() / k);
    for i in 0..pts.len() {
        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
        // A triangle, as a face whose last two corners are one.
        face(m, [c, a, b, b], hint);
    }
}

/// An upright box, every face outwards. The bottom is left out (it stands on
/// something).
fn cuboid(m: &mut Mesh, min: [f32; 3], max: [f32; 3]) {
    let [x0, y0, z0] = min;
    let [x1, y1, z1] = max;
    face(
        m,
        [[x0, y1, z0], [x1, y1, z0], [x1, y1, z1], [x0, y1, z1]],
        UP,
    );
    face(
        m,
        [[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]],
        [0.0, 0.0, 1.0],
    );
    face(
        m,
        [[x0, y0, z0], [x1, y0, z0], [x1, y1, z0], [x0, y1, z0]],
        [0.0, 0.0, -1.0],
    );
    face(
        m,
        [[x1, y0, z0], [x1, y0, z1], [x1, y1, z1], [x1, y1, z0]],
        [1.0, 0.0, 0.0],
    );
    face(
        m,
        [[x0, y0, z0], [x0, y0, z1], [x0, y1, z1], [x0, y1, z0]],
        [-1.0, 0.0, 0.0],
    );
    if y0 > 0.0 {
        face(
            m,
            [[x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]],
            DOWN,
        );
    }
}

/// A box of `size` (width, height, depth) standing on its base's middle,
/// placed by `place` (a turn and a move): every face outwards.
fn block(m: &mut Mesh, place: &Mat4, size: [f32; 3]) {
    let [w, h, d] = size.map(|v| v / 2.0);
    let h = h * 2.0;
    let p = |x: f32, y: f32, z: f32| geometry::point(place, [x, y, z]);
    let dir = |v: [f32; 3]| geometry::direction(place, v);
    let faces = [
        (
            [p(-w, h, -d), p(w, h, -d), p(w, h, d), p(-w, h, d)],
            dir(UP),
        ),
        (
            [p(-w, 0.0, -d), p(w, 0.0, -d), p(w, 0.0, d), p(-w, 0.0, d)],
            dir(DOWN),
        ),
        (
            [p(-w, 0.0, d), p(w, 0.0, d), p(w, h, d), p(-w, h, d)],
            dir([0.0, 0.0, 1.0]),
        ),
        (
            [p(-w, 0.0, -d), p(w, 0.0, -d), p(w, h, -d), p(-w, h, -d)],
            dir([0.0, 0.0, -1.0]),
        ),
        (
            [p(w, 0.0, -d), p(w, 0.0, d), p(w, h, d), p(w, h, -d)],
            dir([1.0, 0.0, 0.0]),
        ),
        (
            [p(-w, 0.0, -d), p(-w, 0.0, d), p(-w, h, d), p(-w, h, -d)],
            dir([-1.0, 0.0, 0.0]),
        ),
    ];
    for (q, hint) in faces {
        face(m, q, hint);
    }
}

/// An upright prism of `sides` round a vertical axis at `centre` (x, z), from
/// `y0` to `y1`: a column, a table, a tent pole.
fn prism(m: &mut Mesh, centre: [f32; 2], radius: f32, y0: f32, y1: f32, sides: u32) {
    let pt = |i: u32, y: f32| {
        let a = std::f32::consts::TAU * i as f32 / sides as f32;
        [
            centre[0] + radius * a.cos(),
            y,
            centre[1] + radius * a.sin(),
        ]
    };
    for i in 0..sides {
        let mid = std::f32::consts::TAU * (i as f32 + 0.5) / sides as f32;
        face(
            m,
            [pt(i, y0), pt(i + 1, y0), pt(i + 1, y1), pt(i, y1)],
            [mid.cos(), 0.0, mid.sin()],
        );
    }
    let top: Vec<[f32; 3]> = (0..sides).map(|i| pt(i, y1)).collect();
    polygon(m, &top, UP);
    if y0 > 0.0 {
        let bottom: Vec<[f32; 3]> = (0..sides).map(|i| pt(i, y0)).collect();
        polygon(m, &bottom, DOWN);
    }
}

/// A bar along one axis, from `a` to `b` (which differ in that axis only),
/// `t` thick: a truss, a pipe, a batten.
fn bar(m: &mut Mesh, a: [f32; 3], b: [f32; 3], t: f32) {
    let h = t / 2.0;
    let mut min = [0.0; 3];
    let mut max = [0.0; 3];
    for i in 0..3 {
        if (a[i] - b[i]).abs() > 1e-6 {
            min[i] = a[i].min(b[i]);
            max[i] = a[i].max(b[i]);
        } else {
            min[i] = a[i] - h;
            max[i] = a[i] + h;
        }
    }
    // Hung (above the floor), its underside is drawn too.
    cuboid(m, min, max);
}

/// A rectangle in a horizontal plane, facing up or down.
fn rect_y(m: &mut Mesh, x: [f32; 2], z: [f32; 2], y: f32, up: bool) {
    face(
        m,
        [
            [x[0], y, z[0]],
            [x[1], y, z[0]],
            [x[1], y, z[1]],
            [x[0], y, z[1]],
        ],
        if up { UP } else { DOWN },
    );
}

/// A rectangle in the plane x = `x`, facing +X (`plus`) or -X.
fn rect_x(m: &mut Mesh, x: f32, z: [f32; 2], y: [f32; 2], plus: bool) {
    face(
        m,
        [
            [x, y[0], z[0]],
            [x, y[0], z[1]],
            [x, y[1], z[1]],
            [x, y[1], z[0]],
        ],
        [if plus { 1.0 } else { -1.0 }, 0.0, 0.0],
    );
}

/// A rectangle in the plane z = `z`, facing +Z (`plus`) or -Z.
fn rect_z(m: &mut Mesh, z: f32, x: [f32; 2], y: [f32; 2], plus: bool) {
    face(
        m,
        [
            [x[0], y[0], z],
            [x[1], y[0], z],
            [x[1], y[1], z],
            [x[0], y[1], z],
        ],
        [0.0, 0.0, if plus { 1.0 } else { -1.0 }],
    );
}

/// A room's walls seen from inside: x from `x[0]` to `x[1]`, z likewise,
/// from the floor to `height`.
fn walls(m: &mut Mesh, x: [f32; 2], z: [f32; 2], height: f32) {
    let y = [0.0, height];
    rect_x(m, x[0], z, y, true);
    rect_x(m, x[1], z, y, false);
    rect_z(m, z[0], x, y, true);
    rect_z(m, z[1], x, y, false);
}

// ----- Seating: a stepped profile swept along a path ---------------------------

/// A line in plan (x, z) that seating rows follow: each point is a centre and
/// an outward direction, and a row `off` metres out is at
/// `centre + (r + off) * normal`. A rounded rectangle's corners are arcs of one radius about
/// their centres, so every row out is the same shape, larger.
#[derive(Clone, Debug)]
struct Path {
    pts: Vec<([f32; 2], [f32; 2])>,
    closed: bool,
    r: f32,
}

/// Points on an arc about `c`, from `from` to `to` degrees (0 = +X, 90 = +Z).
fn push_arc(pts: &mut Vec<([f32; 2], [f32; 2])>, c: [f32; 2], from: f32, to: f32, seg: u32) {
    for i in 0..=seg {
        let t = (from + (to - from) * i as f32 / seg as f32).to_radians();
        pts.push((c, [t.cos(), t.sin()]));
    }
}

impl Path {
    /// Round a rectangle whose corners' centres are at (±a, ±b).
    fn rounded_rect(a: f32, b: f32, r: f32, seg: u32) -> Path {
        let mut pts = Vec::new();
        for (c, from) in [
            ([a, b], 0.0),
            ([-a, b], 90.0),
            ([-a, -b], 180.0),
            ([a, -b], 270.0),
        ] {
            push_arc(&mut pts, c, from, from + 90.0, seg);
        }
        Path {
            pts,
            closed: true,
            r,
        }
    }

    /// Three sides of a rounded rectangle, open towards -Z: from (-a, `z0`)
    /// up the left side, round the front corners at z = b, and back down the
    /// right side.
    fn u_shape(a: f32, b: f32, z0: f32, r: f32, seg: u32) -> Path {
        let mut pts = vec![([-a, z0], [-1.0, 0.0])];
        push_arc(&mut pts, [-a, b], 180.0, 90.0, seg);
        push_arc(&mut pts, [a, b], 90.0, 0.0, seg);
        pts.push(([a, z0], [1.0, 0.0]));
        Path {
            pts,
            closed: false,
            r,
        }
    }

    /// Rows curved about `c` (behind the stage), `half` degrees either side
    /// of +Z.
    fn fan(c: [f32; 2], r: f32, half: f32, seg: u32) -> Path {
        let mut pts = Vec::new();
        push_arc(&mut pts, c, 90.0 + half, 90.0 - half, seg);
        Path {
            pts,
            closed: false,
            r,
        }
    }

    /// Straight rows from x = -`half` to `half` at z, going back towards +Z.
    fn straight(half: f32, z: f32) -> Path {
        Path {
            pts: vec![([-half, z], [0.0, 1.0]), ([half, z], [0.0, 1.0])],
            closed: false,
            r: 0.0,
        }
    }

    fn at(&self, k: usize, off: f32, y: f32) -> [f32; 3] {
        let (c, n) = self.pts[k];
        [
            c[0] + (self.r + off) * n[0],
            y,
            c[1] + (self.r + off) * n[1],
        ]
    }

    fn segments(&self) -> Vec<(usize, usize)> {
        let n = self.pts.len();
        let mut s: Vec<(usize, usize)> = (0..n.saturating_sub(1)).map(|i| (i, i + 1)).collect();
        if self.closed && n > 2 {
            s.push((n - 1, 0));
        }
        s
    }

    /// Outwards, between two points.
    fn outward(&self, a: usize, b: usize) -> [f32; 3] {
        let (na, nb) = (self.pts[a].1, self.pts[b].1);
        [na[0] + nb[0], 0.0, na[1] + nb[1]]
    }

    /// Along the path at an end: into the path at the start, out of it at the
    /// end.
    fn along(&self, from: usize, to: usize) -> [f32; 3] {
        geometry::sub(self.at(to, 0.0, 0.0), self.at(from, 0.0, 0.0))
    }
}

/// Rows of seating: a stepped block whose fronts follow a path.
#[derive(Clone, Copy, Debug)]
struct Rake {
    /// How far out the first row's front is.
    first: f32,
    rows: u32,
    /// Front to back of a row.
    depth: f32,
    /// One row's tread above the one before.
    rise: f32,
    /// The first row's tread.
    first_height: f32,
    /// The block's underside: 0 on the floor, above it for a balcony.
    floor: f32,
    /// Close the back with a wall.
    back: bool,
}

impl Rake {
    fn end(&self) -> f32 {
        self.first + self.rows as f32 * self.depth
    }

    fn top(&self) -> f32 {
        self.first_height + self.rows.saturating_sub(1) as f32 * self.rise
    }
}

/// A rake along a path: each row's tread and riser, the back, the underside
/// of a raised block and, on an open path, the stepped ends.
fn rake(m: &mut Mesh, p: &Path, k: &Rake) {
    let segs = p.segments();
    let last = p.pts.len() - 1;
    for i in 0..k.rows {
        let o0 = k.first + i as f32 * k.depth;
        let o1 = o0 + k.depth;
        let y = k.first_height + i as f32 * k.rise;
        let below = if i == 0 { k.floor } else { y - k.rise };
        for &(a, b) in &segs {
            let out = p.outward(a, b);
            face(
                m,
                [
                    p.at(a, o0, y),
                    p.at(b, o0, y),
                    p.at(b, o1, y),
                    p.at(a, o1, y),
                ],
                UP,
            );
            face(
                m,
                [
                    p.at(a, o0, below),
                    p.at(b, o0, below),
                    p.at(b, o0, y),
                    p.at(a, o0, y),
                ],
                out.map(|v| -v),
            );
        }
        if !p.closed {
            for (e, hint) in [(0, p.along(1, 0)), (last, p.along(last - 1, last))] {
                face(
                    m,
                    [
                        p.at(e, o0, k.floor),
                        p.at(e, o1, k.floor),
                        p.at(e, o1, y),
                        p.at(e, o0, y),
                    ],
                    hint,
                );
            }
        }
    }
    if k.back {
        band(m, p, k.end(), k.floor, k.top(), true);
    }
    if k.floor > 0.0 {
        for &(a, b) in &segs {
            face(
                m,
                [
                    p.at(a, k.first, k.floor),
                    p.at(b, k.first, k.floor),
                    p.at(b, k.end(), k.floor),
                    p.at(a, k.end(), k.floor),
                ],
                DOWN,
            );
        }
    }
}

/// An upright band `off` out along a path, from `y0` to `y1`, facing out or
/// in.
fn band(m: &mut Mesh, p: &Path, off: f32, y0: f32, y1: f32, outwards: bool) {
    for (a, b) in p.segments() {
        let out = p.outward(a, b);
        face(
            m,
            [
                p.at(a, off, y0),
                p.at(b, off, y0),
                p.at(b, off, y1),
                p.at(a, off, y1),
            ],
            if outwards { out } else { out.map(|v| -v) },
        );
    }
}

/// The flat shape a closed path makes `off` out, at height y.
fn disc(m: &mut Mesh, p: &Path, off: f32, y: f32, up: bool) {
    let pts: Vec<[f32; 3]> = (0..p.pts.len()).map(|k| p.at(k, off, y)).collect();
    polygon(m, &pts, if up { UP } else { DOWN });
}

// ----- The venues --------------------------------------------------------------

/// Arena: the floor's half sizes (x, z) and the corners' radius.
const ARENA_HALF: [f32; 2] = [15.0, 30.0];
const ARENA_CORNER: f32 = 8.5;
/// From the floor's edge to the building's outer wall.
const ARENA_WALL: f32 = 31.3;
const ARENA_ROOF: f32 = 24.0;

fn arena(k: &mut Kit) {
    let path = Path::rounded_rect(
        ARENA_HALF[0] - ARENA_CORNER,
        ARENA_HALF[1] - ARENA_CORNER,
        ARENA_CORNER,
        12,
    );
    // The event floor, its corners rounded as an ice rink's are.
    disc(k.m("Floor", ARENA_FLOOR), &path, 0.0, 0.0, true);
    // Lower bowl: 18 rows, 25 degrees; a 1.2 m wall to the floor.
    let lower = Rake {
        first: 1.0,
        rows: 18,
        depth: 0.85,
        rise: 0.4,
        first_height: 1.2,
        floor: 0.0,
        back: false,
    };
    // The gangway between the floor and the first row.
    {
        let f = k.m("Concourse", CONCRETE);
        for (a, b) in path.segments() {
            face(
                f,
                [
                    path.at(a, 0.0, 0.0),
                    path.at(b, 0.0, 0.0),
                    path.at(b, lower.first, 0.0),
                    path.at(a, lower.first, 0.0),
                ],
                UP,
            );
        }
    }
    let seats = k.m("Seating", SEATS_BLUE);
    rake(seats, &path, &lower);
    // The concourse ring between the tiers, and the upper tier above it.
    let concourse = Rake {
        first: lower.end(),
        rows: 1,
        depth: 2.2,
        rise: 0.0,
        first_height: lower.top(),
        floor: 0.0,
        back: false,
    };
    rake(k.m("Concourse", CONCRETE), &path, &concourse);
    let upper = Rake {
        first: concourse.end(),
        rows: 14,
        depth: 0.9,
        rise: 0.55,
        first_height: lower.top() + 2.0,
        floor: 0.0,
        back: false,
    };
    rake(k.m("Seating", SEATS_BLUE), &path, &upper);
    // The building: a wall round it all and the roof.
    band(
        k.m("Walls", WALL_GREY),
        &path,
        ARENA_WALL,
        0.0,
        ARENA_ROOF,
        false,
    );
    disc(
        k.m("Ceiling", CEILING_DARK),
        &path,
        ARENA_WALL,
        ARENA_ROOF,
        false,
    );
    // The roof's truss grid, every 6 m, 4 m under the roof.
    let metal = k.m("Metal", METAL);
    let y = ARENA_ROOF - 4.0;
    let mut x = -30.0;
    while x <= 30.0 {
        bar(metal, [x, y, -48.0], [x, y, 48.0], 0.4);
        x += 6.0;
    }
    let mut z = -48.0;
    while z <= 48.0 {
        bar(metal, [-30.0, y - 0.4, z], [30.0, y - 0.4, z], 0.4);
        z += 6.0;
    }
    // The centre-hung's rigging point: a frame under the grid and its
    // chains down to the screen's top (the screen is a starter screen).
    bar(metal, [-4.5, 18.8, 0.0], [4.5, 18.8, 0.0], 0.3);
    bar(metal, [0.0, 18.8, -4.5], [0.0, 18.8, 4.5], 0.3);
    for (x, z) in [(-3.0, -3.0), (3.0, -3.0), (-3.0, 3.0), (3.0, 3.0)] {
        bar(metal, [x, 18.25, z], [x, y - 0.6, z], 0.08);
    }
    // The end stage: deck, a drape behind it closing off the end seats, and
    // a front truss.
    cuboid(
        k.m("Stage", STAGE_DECK),
        [-10.0, 0.0, -29.0],
        [10.0, 1.8, -15.0],
    );
    cuboid(
        k.m("Masking", MASKING),
        [-16.0, 0.0, -29.6],
        [16.0, 15.0, -29.5],
    );
    let metal = k.m("Metal", METAL);
    bar(metal, [-11.0, 14.0, -16.0], [11.0, 14.0, -16.0], 0.5);
    bar(metal, [-11.0, 14.0, -28.0], [11.0, 14.0, -28.0], 0.5);
    // The mix position on the floor.
    cuboid(
        k.m("Stage", STAGE_DECK),
        [-3.0, 0.0, 11.0],
        [3.0, 0.6, 15.0],
    );
}

/// The proscenium theatre's pit, below the stalls floor.
const PIT_DEPTH: f32 = 2.5;

fn proscenium(k: &mut Kit) {
    const DECK: f32 = 1.0;
    const HOUSE: [f32; 2] = [-14.0, 14.0];
    const HOUSE_BACK: f32 = 30.0;
    const HOUSE_HIGH: f32 = 15.0;
    const FLY: f32 = 25.0;
    // The stage (ending behind the arch, so no face lies in the arch's), its
    // apron through the opening, and the stage house (fly tower).
    let deck = k.m("Stage", STAGE_DECK);
    cuboid(deck, [-9.0, 0.0, -10.0], [9.0, DECK, -0.6]);
    cuboid(deck, [-6.0, 0.0, -0.6], [6.0, DECK, 1.2]);
    let w = k.m("Walls", THEATRE_WALL);
    rect_x(w, -9.0, [-10.0, -0.6], [0.0, FLY], true);
    rect_x(w, 9.0, [-10.0, -0.6], [0.0, FLY], false);
    rect_z(w, -10.0, [-9.0, 9.0], [0.0, FLY], true);
    rect_z(w, -0.6, [-9.0, 9.0], [HOUSE_HIGH, FLY], false);
    rect_y(
        k.m("Ceiling", CEILING_DARK),
        [-9.0, 9.0],
        [-10.0, -0.6],
        FLY,
        false,
    );
    // The proscenium wall, with its 12 x 7 m opening over the deck.
    let arch = k.m("Proscenium", [0.16, 0.11, 0.07, 1.0]);
    cuboid(arch, [HOUSE[0], 0.0, -0.6], [-6.0, HOUSE_HIGH, 0.0]);
    cuboid(arch, [6.0, 0.0, -0.6], [HOUSE[1], HOUSE_HIGH, 0.0]);
    cuboid(arch, [-6.0, DECK + 7.0, -0.6], [6.0, HOUSE_HIGH, 0.0]);
    // Masking: two sets of legs and borders, and a black backdrop.
    let black = k.m("Masking", MASKING);
    for z in [-2.5, -5.5] {
        cuboid(black, [-8.8, DECK, z - 0.05], [-6.3, 8.8, z]);
        cuboid(black, [6.3, DECK, z - 0.05], [8.8, 8.8, z]);
        cuboid(black, [-8.8, 7.6, z - 0.05], [8.8, 9.2, z]);
    }
    cuboid(black, [-8.8, DECK, -9.65], [8.8, 12.0, -9.6]);
    // Flown battens and the grid.
    let metal = k.m("Metal", METAL);
    let mut z = -1.5;
    while z > -9.5 {
        bar(metal, [-8.0, 13.5, z], [8.0, 13.5, z], 0.06);
        z -= 1.0;
    }
    let mut x = -8.0;
    while x <= 8.0 {
        bar(metal, [x, 22.0, -9.8], [x, 22.0, -0.8], 0.15);
        x += 2.0;
    }
    // The orchestra pit, in front of the apron.
    let pit = k.m("Pit", [0.045, 0.04, 0.035, 1.0]);
    rect_y(pit, [-6.0, 6.0], [1.2, 4.2], -PIT_DEPTH, true);
    rect_z(pit, 1.2, [-6.0, 6.0], [-PIT_DEPTH, 0.0], true);
    rect_z(pit, 4.2, [-6.0, 6.0], [-PIT_DEPTH, 0.0], false);
    rect_x(pit, -6.0, [1.2, 4.2], [-PIT_DEPTH, 0.0], true);
    rect_x(pit, 6.0, [1.2, 4.2], [-PIT_DEPTH, 0.0], false);
    cuboid(k.m("Wood", WOOD), [-6.0, 0.0, 4.2], [6.0, 0.9, 4.35]);
    // The auditorium: floor round the pit, walls, ceiling.
    let floor = k.m("Floor", CARPET);
    rect_y(floor, [HOUSE[0], -6.0], [0.0, 4.2], 0.0, true);
    rect_y(floor, [6.0, HOUSE[1]], [0.0, 4.2], 0.0, true);
    rect_y(floor, HOUSE, [4.2, HOUSE_BACK], 0.0, true);
    let w = k.m("Walls", THEATRE_WALL);
    rect_x(w, HOUSE[0], [0.0, HOUSE_BACK], [0.0, HOUSE_HIGH], true);
    rect_x(w, HOUSE[1], [0.0, HOUSE_BACK], [0.0, HOUSE_HIGH], false);
    rect_z(w, HOUSE_BACK, HOUSE, [0.0, HOUSE_HIGH], false);
    rect_y(
        k.m("Ceiling", CREAM),
        HOUSE,
        [0.0, HOUSE_BACK],
        HOUSE_HIGH,
        false,
    );
    // Raked stalls, curved about a point behind the stage, and the circle.
    let seats = k.m("Seating", SEATS_RED);
    rake(
        seats,
        &Path::fan([0.0, -6.0], 11.6, 26.0, 26),
        &Rake {
            first: 0.0,
            rows: 20,
            depth: 0.9,
            rise: 0.15,
            first_height: 0.15,
            floor: 0.0,
            back: true,
        },
    );
    rake(
        seats,
        &Path::fan([0.0, -6.0], 25.5, 23.0, 24),
        &Rake {
            first: 0.0,
            rows: 11,
            depth: 0.9,
            rise: 0.45,
            first_height: 6.5,
            floor: 5.5,
            back: true,
        },
    );
}

fn thrust(k: &mut Kit) {
    const DECK: f32 = 0.6;
    // The room.
    let (x, z, high) = ([-18.5, 18.5], [-8.0, 21.0], 11.0);
    rect_y(k.m("Floor", CONCRETE), x, z, 0.0, true);
    let w = k.m("Walls", THEATRE_WALL);
    walls(w, x, z, high);
    rect_y(k.m("Ceiling", CEILING_DARK), x, z, high, false);
    // The stage behind and the thrust: three sides of a rounded rectangle
    // (corners' centres at x ±3, z 6, radius 2), 10 m wide, 8 m out.
    let deck = k.m("Stage", STAGE_DECK);
    cuboid(deck, [-8.0, 0.0, -6.0], [8.0, DECK, 0.0]);
    let mut slab = Path::u_shape(3.0, 6.0, 0.0, 2.0, 8);
    slab.closed = true;
    band(deck, &slab, 0.0, 0.0, DECK, true);
    disc(deck, &slab, 0.0, DECK, true);
    // Seating on three sides, past a 1.5 m gangway.
    let rows = Rake {
        first: 1.5,
        rows: 12,
        depth: 0.9,
        rise: 0.42,
        first_height: 0.45,
        floor: 0.0,
        back: true,
    };
    rake(
        k.m("Seating", SEATS_RED),
        &Path::u_shape(3.0, 6.0, 0.3, 2.0, 8),
        &rows,
    );
    // Vomitories through the seating at the front corners.
    let black = k.m("Masking", MASKING);
    for side in [-1.0f32, 1.0] {
        let n = [
            side * std::f32::consts::FRAC_1_SQRT_2,
            std::f32::consts::FRAC_1_SQRT_2,
        ];
        let c = [3.0 * side + 6.5 * n[0], 6.0 + 6.5 * n[1]];
        let place = geometry::mul(
            &geometry::translation([c[0], 0.0, c[1]]),
            &geometry::rotation_y(45.0 * side),
        );
        block(black, &place, [2.0, 2.4, 6.0]);
    }
    // Black masking upstage and the lighting grid.
    cuboid(black, [-8.0, 0.0, -6.5], [8.0, 9.0, -6.4]);
    let metal = k.m("Metal", METAL);
    let mut gx = -16.0;
    while gx <= 16.0 {
        bar(metal, [gx, 9.5, -7.0], [gx, 9.5, 20.0], 0.06);
        gx += 2.0;
    }
    let mut gz = -6.0;
    while gz <= 20.0 {
        bar(metal, [-17.0, 9.44, gz], [17.0, 9.44, gz], 0.06);
        gz += 2.0;
    }
}

fn black_box(k: &mut Kit) {
    let (x, z, high) = ([-7.5, 7.5], [-6.0, 6.0], 7.0);
    rect_y(k.m("Floor", [0.035, 0.035, 0.035, 1.0]), x, z, 0.0, true);
    walls(k.m("Walls", MASKING), x, z, high);
    rect_y(k.m("Ceiling", [0.03, 0.03, 0.032, 1.0]), x, z, high, false);
    // The pipe grid at 6.2 m, every 1.5 m both ways.
    let metal = k.m("Metal", [0.12, 0.12, 0.13, 1.0]);
    let mut gz = -5.25;
    while gz <= 5.3 {
        bar(metal, [-7.3, 6.2, gz], [7.3, 6.2, gz], 0.05);
        gz += 1.5;
    }
    let mut gx = -6.75;
    while gx <= 6.8 {
        bar(metal, [gx, 6.15, -5.8], [gx, 6.15, 5.8], 0.05);
        gx += 1.5;
    }
    // Retractable risers, pulled out: six rows to the back wall.
    rake(
        k.m("Seating", SEATS_GREY),
        &Path::straight(5.5, 0.6),
        &Rake {
            first: 0.0,
            rows: 6,
            depth: 0.9,
            rise: 0.3,
            first_height: 0.3,
            floor: 0.0,
            back: false,
        },
    );
}

/// The club's columns (x, z).
const CLUB_COLUMNS: [[f32; 2]; 4] = [[-8.3, -2.0], [-8.3, 6.0], [7.6, -2.0], [7.6, 6.0]];

fn club(k: &mut Kit) {
    let (x, z, high) = ([-11.0, 11.0], [-9.0, 13.0], 5.0);
    rect_y(k.m("Floor", CLUB_FLOOR), x, z, 0.0, true);
    walls(k.m("Walls", CLUB_WALL), x, z, high);
    rect_y(k.m("Ceiling", MASKING), x, z, high, false);
    // The dance floor, raised a little; the DJ's riser and desk.
    cuboid(
        k.m("Dance floor", [0.10, 0.09, 0.11, 1.0]),
        [-7.0, 0.0, -5.0],
        [7.0, 0.05, 6.0],
    );
    let deck = k.m("Stage", STAGE_DECK);
    cuboid(deck, [-4.0, 0.0, -9.0], [4.0, 1.0, -5.0]);
    cuboid(deck, [-1.8, 1.0, -6.5], [1.8, 2.1, -5.8]);
    // A raised area at the back.
    cuboid(deck, [-11.0, 0.0, 10.0], [-2.0, 0.45, 13.0]);
    // The bar along the right-hand wall, and the shelves behind it.
    let bar_top = k.m("Bar", COPPER);
    cuboid(bar_top, [8.4, 0.0, -2.0], [9.4, 1.1, 10.0]);
    cuboid(bar_top, [10.5, 0.0, -2.0], [11.0, 2.4, 10.0]);
    // Columns.
    let col = k.m("Columns", [0.08, 0.08, 0.09, 1.0]);
    for c in CLUB_COLUMNS {
        prism(col, c, 0.35, 0.0, high, 12);
    }
    // A square of truss over the dance floor.
    let metal = k.m("Metal", METAL);
    for (a, b) in [
        ([-5.0, 4.3, -3.0], [5.0, 4.3, -3.0]),
        ([-5.0, 4.3, 4.0], [5.0, 4.3, 4.0]),
        ([-5.0, 4.0, -3.0], [-5.0, 4.0, 4.0]),
        ([5.0, 4.0, -3.0], [5.0, 4.0, 4.0]),
    ] {
        bar(metal, a, b, 0.3);
    }
}

fn festival(k: &mut Kit) {
    rect_y(
        k.m("Ground", GRASS),
        [-60.0, 60.0],
        [-35.0, 80.0],
        0.0,
        true,
    );
    // The stage and its roof on four towers.
    cuboid(
        k.m("Stage", STAGE_DECK),
        [-12.0, 0.0, -14.0],
        [12.0, 2.0, 0.0],
    );
    let metal = k.m("Metal", METAL);
    for x in [-13.5, 12.5] {
        for z in [-15.0, -1.0] {
            cuboid(metal, [x, 0.0, z], [x + 1.0, 20.0, z + 1.0]);
        }
    }
    bar(metal, [-12.0, 17.0, -0.8], [12.0, 17.0, -0.8], 0.5);
    cuboid(
        k.m("Roof", [0.05, 0.05, 0.05, 1.0]),
        [-14.0, 20.0, -15.5],
        [14.0, 21.2, 0.5],
    );
    // The backdrop and the scaffold wings that carry the side screens.
    let black = k.m("Masking", MASKING);
    cuboid(black, [-12.0, 2.0, -13.8], [12.0, 20.0, -13.7]);
    cuboid(black, [-22.0, 0.0, -6.0], [-14.5, 16.0, -2.0]);
    cuboid(black, [14.5, 0.0, -6.0], [22.0, 16.0, -2.0]);
    // The crowd barrier and two delay towers.
    let metal = k.m("Metal", METAL);
    cuboid(metal, [-15.0, 0.0, 3.0], [15.0, 1.2, 3.25]);
    for x in [-12.75, 11.25] {
        cuboid(metal, [x, 0.0, 45.0], [x + 1.5, 10.0, 46.5]);
    }
    // Front of house: a platform under a tent.
    cuboid(k.m("Platform", WOOD), [-5.0, 0.0, 33.0], [5.0, 0.5, 39.0]);
    let tent = k.m("Tent", CANVAS);
    cuboid(tent, [-6.0, 3.8, 32.0], [6.0, 4.0, 40.0]);
    for x in [-5.8, 5.8] {
        for z in [32.2, 39.8] {
            prism(tent, [x, z], 0.05, 0.0, 3.8, 6);
        }
    }
}

fn ballroom(k: &mut Kit) {
    let (x, z, high) = ([-15.0, 15.0], [-12.0, 12.0], 6.0);
    rect_y(k.m("Floor", CARPET), x, z, 0.0, true);
    walls(k.m("Walls", BEIGE), x, z, high);
    rect_y(k.m("Ceiling", CREAM), x, z, high, false);
    cuboid(
        k.m("Stage", STAGE_DECK),
        [-6.0, 0.0, -12.0],
        [6.0, 0.6, -8.0],
    );
    cuboid(
        k.m("Masking", MASKING),
        [-7.0, 0.0, -11.9],
        [7.0, 6.0, -11.8],
    );
    // Round tables for ten, 1.8 m across, five by three.
    for tx in [-10.0, -5.0, 0.0, 5.0, 10.0] {
        for tz in [-4.0, 1.0, 6.0] {
            prism(k.m("Tables", LINEN), [tx, tz], 0.9, 0.0, 0.75, 16);
            let chairs = k.m("Chairs", [0.38, 0.28, 0.12, 1.0]);
            for i in 0..8 {
                let a = std::f32::consts::TAU * (i as f32 + 0.5) / 8.0;
                let (cx, cz) = (tx + 1.25 * a.cos(), tz + 1.25 * a.sin());
                cuboid(
                    chairs,
                    [cx - 0.22, 0.0, cz - 0.22],
                    [cx + 0.22, 0.47, cz + 0.22],
                );
            }
        }
    }
    // Chandeliers.
    for cx in [-8.0, 0.0, 8.0] {
        let c = k.m("Chandeliers", GOLD);
        prism(c, [cx, 0.0], 0.8, 4.4, 5.0, 8);
        prism(c, [cx, 0.0], 0.03, 5.0, high, 6);
    }
}

#[cfg(test)]
mod tests;
