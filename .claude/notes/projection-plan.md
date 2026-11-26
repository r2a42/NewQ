# The 3D stage after 0.25.0: projector simulation, mapping and line-up (plan)

Written 2026-11-20 against the tree that shipped 0.25.0. Step P1 is built (see "Built so far"
below); the rest isn't. Statements about
existing code name the file they were read in; the few taken from CLAUDE.md without opening the
file say "per CLAUDE.md". Statements about other products say where they come from and how sure
they are.

Where the nine things asked for are:

| Asked for | Section |
|---|---|
| 1. Projector model in the show file | 2 |
| 2. Simulation rendering | 3 |
| 3. UI | 8 |
| 4. Mapping | 4 |
| 5. Calibration | 5 |
| 6. Content mapping modes | 6 |
| 7. Stages and releases | 7 |
| 8. Formats and interop | 9 |
| 9. Open questions | 11 |

Words used below:

- **screen**: a `stage3d::Surface` (what the UI already calls a screen).
- **display**: a `show::OutputConfig`.
- **projector**: the new thing, a physical projector standing in the room.
- **lens map**: a per-projector lookup picture made once and used every frame (section 4).
- **line-up**: calibration by point alignment (section 5).

---

## 0. The short version

1. **A projector is a new object in `Show::stage3d`** (position, aim, lens, lumens, the display it
   is), not new fields on `OutputConfig`. A show with no projectors is written exactly as 0.25 writes
   it. 0.25 opens a later show, ignores the projectors and plays it with the hand-made keystone,
   mesh and blend, which mapping never overwrites. (A show that was built mapped from the start
   has no hand-made setup to fall back on: in 0.25 its projectors' displays are simply black.)
2. **Simulation is one more loop in the existing uber-shader** (`src/render3d/shader.wgsl`): up to
   16 projectors per view, each with a shadow-map layer and a picture layer, summed in physical
   units inside the fragment shader and turned into display values at the end. It is preview work:
   it goes through the governor like every 3D view does today.
3. **The numbers come from a pure module** (`stage3d/light.rs`), the way `blend.rs` is the
   reference for the blend shader. The lux, nit and pixel-size read-out under the pointer is
   computed on the CPU from a `pick::Hit`; the shader is tested against the same functions.
4. **Mapping does not go through the stage-render thread.** A mapped display is drawn in its own
   output thread, in step with its frame, as two passes: the content canvas, then one full-screen
   pass through a **lens map** (for every projector pixel: which content, where in it, how bright).
   The lens map is made off the output path when the room, the projector or the line-up changes.
   The cost per frame doesn't depend on how complicated the room is.
5. **Line-up is a pure solver** (`src/calib/`, f64, no GPU, tested on made-up projectors) plus a
   small UI: click a spot on the model, nudge a cross on the real surface until it sits there. Four
   points with a known lens are enough; more points also find the lens.
6. **For one flat screen the corner pin that exists today stays the recommended way.** Mapping
   earns its keep on curved and faceted screens, set pieces and several projectors. The plan keeps
   that path untouched and never makes a small theatre place a projector in 3D to get a picture.
7. **Six shippable steps over about three releases** (section 7). Three agents can work in
   parallel in each round if new work goes in new files; the collision files are listed.

---

## Built so far

### P1, projectors in the room (branch `projectors-p1`, 2026-11-20)

Owner's rulings taken: simulation first; up to 16 projectors per view; read-outs in lux and nits
only; generic lens and brightness presets plus "My projectors" per computer, no products list;
plain words; the corner pin stays the way for one flat screen, and the Projectors section says so.

What a user can do: add a projector (aimed at a screen, from My projectors, or blank); choose its
display, pixels, lens (five presets in words, or a typed throw ratio, zoom range and lens shift)
and lumens; type where it stands and its turn, or *Point at* a screen; read the picture's size on
the screen its middle lands on, how much of the screen it fills, what spills over which edge,
about how many lux and nits, the distance and the pixel size; *Fit to a screen* (move, or zoom from
here); *Which lens from here?* (and *Use that lens*); keep it in My projectors. In the 3D views:
pick a projector by its body, see every projector's cone and its picture's outline as lines (the
picked one brighter), look through a lens (*Viewpoint ▸ Through …*, letterboxed, lens shift in the
projection), and read the light under the pointer.

Code: `stage3d/projector.rs` (model and throw calculator), `stage3d/light.rs` (section 3.1's sums,
with `lit_by`'s shadow test), `render3d/camera.rs` (`Camera::shift`/`aspect`, `Frustum`,
`picture_in`, `clip_to_near`), `render3d/mod.rs` (`Line`, `render_with_lines`), `stage_render.rs`
and `stage_live.rs` (lines on preview jobs only, hashed into the signature), `ui/stage_projectors.rs`
(the section, the worker, the view's lines and read-out), small hooks in `ui/stage_edit.rs` and
`ui/stage_view.rs`. The contract types for P2 are in: `scene::Throw` and `Scene::throws` (always
empty for now).

Changes from the plan:

- **The cone and outline are drawn on the GPU, not with egui's painter** (8.2 said the painter):
  `Renderer::render_with_lines`, a `LineList` pipeline made in an error scope like the grid and
  axes, drawn last with depth compare `Always`, so lines show through walls as 8.2 accepted. This
  keeps the lines inside the governed preview (they come with a view's `ViewRequest`, so a
  governed-down view draws none) and makes them testable on lavapipe. The near plane is the
  GPU's clipper; `camera::clip_to_near` exists for CPU drawing but isn't used yet.
- **No drag handles** (8.2's three handles): P1 places projectors by typed numbers, *Point at* and
  *Fit to a screen*. Handles can come with P2 or P5.
- **Ray tests are off the main thread.** `render3d::pick` transforms every vertex per ray, so an
  outline (24 rays a side) on a big venue would stall the main loop. Footprints and the light
  under the pointer are made on a `projector-sums` worker (latest wins); the view shows the cone
  alone until the outline lands, and asks to be drawn again while answers are due. The row
  read-outs test only the screens (small) on the main thread.
- `Projector::fine`, `mapped` and `line_up` are left out (P4, P5), as asked; a later NewQ's fields
  are kept by a flattened `extra` map on `Projector` and `Lens`, so they survive a save here.
- `fit_to` takes the box round any shape (flat, curved, faceted), its front face, the lens on its
  axis; a `rear` screen is fitted from behind. "Fills" is exact for a flat screen (the picture's
  quadrilateral clipped to the screen) and not given for other shapes.
- *Point at* puts the picture's middle (not the lens axis) on the screen's middle, solved by a few
  steps for a shifted lens.
- Leaving a look-through view (drag, wheel, double-click, Esc) orbits from the lens's position; the
  orbit has no shift, so the picture jumps a little for a shifted lens.
- Section 1.3's first point is already right in CLAUDE.md (the manifest paragraph names the
  venue); its second (render3d's `scene.rs` is a re-export) is now corrected.

Not tested here: any real GPU, projector, light meter or room. The lux and nits follow section
3.1's first-order model and say "about".

### P2, light (branch `projectors-p2`, 2026-11-20)

What a user can do: tick *Light* over any 3D view (it appears when the show has projectors) and
each shown projector, up to 16, throws its display's picture into the room: onto the screens and
the venue, falling off with distance and angle, shadowed by whatever is in the way (other
projectors' bodies included), adding up where pictures overlap. *Show* picks *Picture* (with *Auto
exposure*, or the nits shown as white by hand), *Brightness* (nits at full white in five bands),
*Overlap* (one, two, three or more) or *Pixel size* (five bands in mm); each colour view has a key.
What is thrown is what the display sends: its keystone, mesh, part of a wide picture and edge
blend, so a blended pair's overlap evens out as the blend is set. A projector with no display
throws white; a screen no projector reaches keeps its own picture (an LED wall).

Code: `stage3d/light.rs` (`throw_of`, `throw_uv`, `throw_lux`, bands, `auto_white`), `render3d/lit.rs`
and `light.wgsl` (the light kit, its shader module, shadow maps), `render3d/mod.rs` (`render_lit`,
`LightView`, `Options::light`/`white_nits`, `checked` lifted out), `stage_render.rs` (`ThrowJob`,
the throws' array), `stage_live.rs` (`ProjectorThrow`, `ThrowPicture`, `light_up`, the lit scene),
`app.rs` (`projector_throws`, `throw_picture`), `ui/stage_view.rs` (the toggle, the bar, the
legend, notices), `stage_projectors::auto_white`. `scene::Part::catch` (a screen's gain and side).

Changes from the plan:

- **The light has its own shader module, entry point (`fs_lit`) and bind group 3**, not a loop in
  `fs_part` and bindings in group 0 (3.3): a GPU that refuses any of it (the module, the lit
  pipelines, the shadow pipeline) leaves the light out with a log line and a notice in the view,
  and the plain view is untouched. `checked` now pushes an Internal scope as well as Validation:
  wgpu classes a backend's shader compiler failing (what DirectX 12 did) as Internal, which the
  0.25 helper did not catch.
- **Shadow maps reuse `vs_part`** with the frame uniform's matrix swapped per layer (16 slices of
  one buffer), no new vertex code. They are kept per size (1024, or 512 when the view is governed
  below full size) and drawn again only when a throw's lens or any part's mesh or transform changes.
  Bias is in the lookup only (a push along the normal, in proportion to depth, and a relative depth
  slack); no pipeline depth bias.
- **Brightness is at full white**, not of the picture (so it agrees with the pointer's read-out),
  and so before any edge blend: an overlap reads as the sum of both, the hover text says *Picture*
  shows blends. *Exposure* belongs to the Picture view (8.2 put it under Brightness).
- 3.7's blend check is in: `stage_render_tests::a_blended_pairs_overlap_is_as_bright_as_either_alone`
  (two white throws overlapping a quarter, matched blends: within 3% of one alone, in light, all
  across; twice without the blends). Every screen shape's normals point out of its CCW front
  (`light_tests`), so the shader's front and the sums' agree. A lit view that doesn't change isn't
  drawn again (`stage_live_tests`).
- **Which screens take light** is decided per screen on the CPU: a screen whose box meets any
  throw's frustum is lit by projectors (a front screen only from its front), any other keeps its
  feed (the plan's rule, 3.3; no `emits` field yet, open question 5).
- **The throw is drawn on the stage thread, not from a function shared with the window's path**:
  `app.rs::throw_picture` takes the display's group from `update_outputs` (the same layers and
  geometry the window gets) with `warp = pin` (not where the window sits on its monitor), its sub
  displays by their areas, and its blend. A display that shows the stage throws nothing (never drawn
  from itself). The throw layer is 960 × 540 at full scale (16:9 always; the projector's own shape is
  in its matrix).
- **A display that shows the stage is drawn without light in P2** (3.6 offered it as a
  `StageSource` option): no show field, no cost on an output.
- The throws' matrix is made from `Projector::frustum` directly (not `Camera::view_projection`,
  which clamps the angle), and the aspect comes from `stage_projectors::shape_of`, as P1's lines,
  so the light sits inside the outline.
- naga is a dev-dependency so a test writes the light's shader as HLSL (5.1, 6.0) and MSL; that
  proves naga's writers take it, not that DirectX's or Metal's compilers do.
- More than 16 shown projectors: the first 16 are lit and the view says so in words (3.5).

Not tested here: any real GPU (only lavapipe), Windows' DirectX 12 or a Mac's Metal compiling the
shader, shadow acne on real drivers, the cost on a weak card, any projector, light meter or room.

### P3, the line-up solver (worktree branch, 2026-11-26)

Nothing a user sees yet: `src/calib/`, pure f64, hand-written (no `nalgebra`), `pub(crate)` and
`#[allow(dead_code)]` until P5. `solve(&Problem) -> Result<Solution, Refusal>`; `project(placement,
lens, pixels, world) -> Option<[f32; 2]>` (the very sum the solver fits, for the overlay and each
point's error); `fit_rigid(from, to) -> Option<(rotation, translation, rms)>` (for lining up a screen
and a tracker's frame later). Conventions in `calib/mod.rs`'s header; points are room metres and
picture pixels ((0, 0) the top-left corner, y down), the caller turns a screen's own-frame points into
room positions; the picture's size is an argument (no display, window, scene or GPU).

Method as 5.3: starts (the placement now, a flat set's homography, the 3x4 DLT for six or more not
flat), each 30 Levenberg-Marquardt rounds, the best to at most 100 (`Solution::capped` says if the cap
was hit; no clock). A ring of 192 places facing the points is tried as well only when the fit is over
a pixel per point and there is no placement to start from or no linear guess (four or five points not
on one surface). The ladder runs from the top down (the highest rung the count and `Freedom` allow,
then lower); each rung is tried for 30 rounds and kept only if its lens numbers are inside the bounds
and pinned to 2% (their standard deviation for a pixel's error per point, from `jᵀj`), and only the
kept one is refined to the end; a `Note` says in words why a rung wasn't used. Leave-one-out (five or
more points, one a pixel or more out) refits without each of the three points furthest off, 10 rounds
each, and names one that is 3 px and five times the others' error off.

Changes from the plan:

- **A lens number that wants to leave its bounds isn't a refusal**: that rung isn't used and the words
  say so ("The throw came out beyond what a lens can be, so it is as you entered it: check the
  points."); the pose is still solved with the lens as entered. The new mapping plan (section 5,
  item 5) asks for a refusal; P5 can turn `Note::AtLimit` into one if the owner prefers.
- **The ladder is 5.3's, not the brief's**: the throw at 6 points, the shift at 8 (the brief said all
  of the lens at six). **Aspect (an anamorphic lens) is a fourth rung**, only when the caller frees it
  (`Freedom::aspect`); 5.3 never solves it. The solved aspect is kept within `Lens::aspect`'s 0.2 to
  10; an entered one (the pixels') within `sane_aspect`'s 0.1 to 20, as drawn.
- **The shift is often kept as entered**: in 60 random projectors (8 to 12 points, depths ±30%), 31
  kept the entered shift because the points didn't pin it to 2% (long lenses most: shift and a small
  turn look alike). A wrong shift then leaves a few pixels and moves the pose; the words ask for it
  ("If the lens is shifted, enter roughly how much"). The owner may want a looser bar or the UI to ask
  for the shift early.
- **Four points always carry "a point in the wrong place can't be told from the others"**: a test
  showed a point moved 15 px left the fit under 1.5 px, so the fit alone can't warn.
- **A left-to-right flipped picture is refused** (`Refusal::Mirrored`: a mirror rig, or the
  projector's own rear setting): looked for only when the fit is over 5 px RMS, by solving the flipped
  points too. Upside down (both flipped) is a turn and solves. **Not caught on a flat set**: flipped
  points on one screen fit exactly with the projector behind it, so the UI must check the answer is
  on the side of the screen it expects.
- `Refusal::Behind` (no start has every point in front) is a guard: no test reaches it through
  `solve`, and by design it is nearly unreachable.
- `LinePoint`, `LineUp`, `Solution` are serde types in `calib` (f64 `world`, lenient enums, `extra`
  kept); the mapping plan's show types are f32 `at` plus `surface` in `projector.rs`, which can use or
  replace them. `points_key` (FNV-1a of the points as f32, and the picture's size) lets a follower
  tell a sent answer is for its points; JSON doesn't give f64s back bit for bit, f32s yes.
- Deterministic on one computer (no clock, threads or hashing; fixed loop order). Only the primary
  solves, so nothing more is promised.

Cost, measured in the test build (`opt-level = 1`, one test thread, a busy 4-core cloud machine;
release, opt-level 3, not measured): a nudge from the last answer with the lens still as entered took
0.7 to 1.9 ms with 12 points (the slow end is a fit over 5 px, which also looks for a flipped picture)
and about 1.1 ms with 36 points (one of them bad). The slowest of 60 cold solves (no start) was about
3.5 ms. "Well under a millisecond" needs the release build; if it isn't, an analytic Jacobian (one
evaluation per round instead of 2 per unknown) is the next step.

Tests (`calib/tests.rs`, made-up projectors with `LensPreset`'s throws): the projection is
`in_picture`'s and `ray`'s (to f32 rounding, a 12.8:1 picture too); exact data recovers 1e-6 m and
1e-6 rad (and 1e-6 of the lens) for 4 points with the lens, 6 (throw), 8 (shift), 12 (aspect), flat
screens, rear projection, straight down and up; 0.5 and 2 px of noise within four of the solver's own
standard deviations, errors in proportion; one bad point named; refusals in words; no input panics;
determinism; a stored f32 answer reprojects within 0.1 px of the f64 one; a nudge gives a cold solve's
answer; 60 random projectors with a fixed seed, each solved exactly (29) or keeping the shift as
entered only because its standard deviation at the truth is over the bar (31).

Not tested here: any real projector, room, or operator's nudging.

---

## 1. What the code is today (the facts the design rests on)

### 1.1 The show model

- `src/stage3d/mod.rs`: `Stage3d { venue, surfaces, viewpoints, units }`, every field
  `#[serde(default)]` and unwritten at its default. `Placement { position, yaw, pitch, roll }` is
  `T * Ry(yaw) * Rx(pitch) * Rz(roll)`, metres and degrees, right-handed, Y up.
- Enums that a later NewQ may extend already carry `#[serde(untagged)] Unknown(serde_json::Value)`
  (`SurfaceShape`, `Feed`), and plain enums are read with `stage3d::lenient` or `lenient_option`.
- `git grep deny_unknown_fields -- src` finds nothing. So 0.25 **ignores** a struct field it
  doesn't know and **drops it when it saves**. An `Unknown(Value)` variant is kept on save; an
  unknown field is not. This is true of the whole show file, not just the stage.
- `Surface { id, name, visible, placement, shape, feed }`; `Feed::Display { output, region }`;
  `SurfaceShape::{Flat, Curved, Faceted}`. `Stage3d::scene(show, venue)` makes the renderer's
  `Scene`: venue parts lit, screens unlit `Paint::Output`, and a screen fed by a display that
  shows the stage is left unfed (`Show::shows_stage`).
- `Stage3d::shape_key()` hashes the whole `Stage3d` as JSON, so anything added to it changes the
  scene key by itself.
- `OutputConfig` (`src/show.rs:677`) has `corners`, `mesh`, `part_of`, `picture`, `blend`,
  `monitor`, `computer`, `within`/`area` (sub displays), `source` (shows the stage), `delay_ms`.
  `EdgeBlend { left, right, top, bottom, gamma }`. `Show::whole_picture(output)` is the display a
  projector of a blended group is `part_of`.
- `delay_ms` is applied in the engine as a cue's `video_lag` (`src/engine.rs:3164`,
  `Show::video_delay_ms`), not in the draw path, so nothing in this plan touches it.

### 1.2 Drawing

- `src/render3d/mod.rs`: `Renderer::render(gpu, &Target, scene, camera, outputs, &Options)`. One
  render pass, reversed Z (`Depth32Float`, clear 0), bind groups 0 (frame), 1 (part, dynamic
  offset), 2 (picture). It writes its uniforms per call and submits through `Gpu::submit` itself.
  Meshes are cached by `Part::mesh_key`, nothing is culled, both faces are lit.
- `src/render3d/shader.wgsl`: `fs_part` samples its one picture "in uniform control flow" and
  works in display values, with lighting in linear light. A comment in `vs_axes` records that
  Windows' DirectX 12 refused a pipeline that wrote a vector component at a run-time index. New
  shader code must be as plain.
- `src/gpu.rs:219-221`: the device is asked for `Limits::downlevel_defaults()` (raised to the
  adapter's resolution) and only the BC feature. In wgpu-types 30 that is 4 bind groups, 16
  sampled textures and 16 samplers per stage, 256 array layers and a 16 KiB uniform binding. There
  are **no texture binding arrays**, no 16-bit normalised formats and no filterable 32-bit float.
  The design below fits inside these.
- `src/stage_render.rs`: the "stage-render" thread. `FeedJob`s first (a display's layers on its
  plain canvas, "no keystone, mesh or edge blend", longest side `FEED_SIDE` = 1024, format
  `Rgba8Unorm` in display values), then `ViewJob`s, displays before previews. Latest wins.
- `src/stage_live.rs`: `StageLive::send` builds jobs, draws a view again only when its
  `signature` changed or something plays, and lets previews wait while displays are behind.
  `output_layer` gives a display that shows the stage a target texture as its only layer.
- `src/render.rs:451` `output_loop`: an output window's thread takes `OutputJob::Frame { size,
  groups, blend }` and does exactly one `gpu.composite(...)` (layers, then the `BlendPass`) and
  one `Gpu::submit`, then presents. That is the whole output path.
- `src/gpu.rs`: `Geometry { canvas, warp, pin, mesh, slice }`. The mesh warp is at most 9 by 9
  lines (`mesh::MAX_LINES`) drawn as 48 by 48 cells (`MESH_CELLS`) in the vertex shader. It is a
  smooth hand warp and can't carry a mapping with edges in it.
- `src/app.rs:2359` `update_outputs`: builds each display's geometry from `corners`, `mesh` and
  `part_of`, and its layers from `engine.layers(source)` or from the stage.

### 1.3 Two places where CLAUDE.md is behind the code

- It says the network media manifest doesn't know the venue file. It does:
  `src/net/store/manifest.rs:350-371` adds the venue model and the files it reads
  (`stage3d::load::venue_files`), and `src/net/store/tests.rs:1511` tests that a venue reaches a
  computer and loads from its store. `relink.rs` has no mention of the venue (grep).
- It calls `render3d/scene.rs` a copy of `stage3d::scene`. It is now a seven-line re-export.

Both should be corrected when the first step of this plan lands.

### 1.4 What the async route would cost (why mapping needs its own path)

A display that shows the stage today gets a texture drawn by another thread, when that thread gets
to it. For a 3D view on a monitor that is fine. For a mapped projector it would mean:

- at least one extra frame of delay, and a varying one;
- two projectors of one blend showing different video frames in their overlap, because all views
  are drawn one after another by one thread;
- content at 1024 pixels unless the feeds are made full size, which then makes the previews pay.

So mapping is drawn in the output's own thread (section 4).

---

## 2. Projectors in the show file

### 2.1 Where they live

`Stage3d` gets one new field:

```rust
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub projectors: Vec<Projector>,
```

Nothing is added to `OutputConfig`. Reasons:

- The stage stays unwritten while empty, and a show without projectors is byte-for-byte what 0.25
  writes (tested, as the existing stage tests do).
- 0.25 ignores `projectors` and still plays the show from `corners`, `mesh`, `part_of` and
  `blend`, which mapping **never changes**. That is a fallback only where such a setup was made
  before mapping was switched on; a show built mapped-first has cues on content displays and
  nothing on the projectors' own, so 0.25 shows black there. If 0.25 saves the show the
  projectors are lost; that is the general rule for newer data and should be said in the
  changelog.
- One projector is one display. A flag on the projector (`mapped`) says the stage drives that
  display. Switching it off gives back the hand-made setup exactly as it was.

### 2.2 The types (new file `src/stage3d/projector.rs`)

```rust
pub struct ProjectorId(pub u64);          // from Show::alloc_id, in load's next-id guard

pub struct Projector {
    pub id: ProjectorId,
    pub name: String,                      // "Projector 1"
    visible: bool,                         // default true, unwritten
    pub placement: Placement,              // where the lens is, and its aim
    pub lens: Lens,                        // unwritten at default
    lumens: f32,                           // default 5000, unwritten at default
    pub display: Option<OutputId>,         // the display this projector is
    resolution: Option<(u32, u32)>,        // only when it has no display yet
    model: Option<String>,                 // the preset it was made from (words only)
    mapped: bool,                          // step P4: the stage drives its display
    fine: Option<crate::mesh::Mesh>,       // step P4: a hand fine-tune on top of the mapping
    line_up: Option<LineUp>,               // step P5: points and the last solve
}

pub struct Lens {
    throw: f32,            // distance / picture width; default 1.5
    zoom: Option<[f32; 2]>,// the lens' range of throw ratios (for the slider and warnings)
    shift: [f32; 2],       // picture moved right/up, as a share of its width/height
    aspect: Option<f32>,   // only when it differs from the display's pixels (anamorphic)
    gamma: f32,            // default 2.2; today's EdgeBlend::gamma is the same idea
}
```

Conventions, to be written in the module header and tested:

- **The lens looks along the projector's own -Z, +X is the picture's right, +Y its top.** That is
  a glTF camera's convention, so importing a camera node is direct (section 9.3), and `render3d`'s `look_at`
  serves. With the sample venues' convention (stage towards -Z, audience +Z,
  `src/stage3d/samples.rs:7-8`), a projector at yaw 0 in the audience points at the stage, and faces
  a screen at yaw 0. Yaw, pitch and roll mean what `Placement` already says.
- **Throw ratio** is distance over picture width, as on every projector data sheet. Internally
  the picture at 1 m is `w1 = 1 / throw` wide and `h1 = w1 / aspect` high.
- **Lens shift** is the picture's centre moved off the lens axis, as a share of the picture's
  width (x, right) and height (y, up). 0.5 up puts the picture's bottom edge on the axis. The
  frustum at 1 m is `left = (sx - 0.5) w1`, `right = (sx + 0.5) w1`, `bottom = (sy - 0.5) h1`,
  `top = (sy + 0.5) h1`. Manufacturers quote shift in different ways (WATCHOUT 7's manual gives
  it as a percentage of the image width, read 2026-11-20); the UI shows a percentage and a line
  of help, and line-up finds it anyway.
- **Aspect** comes from the display's monitor resolution where there is one, else `resolution`,
  else 16:9. Pixels are square unless `aspect` says otherwise.
- **Non-finite or silly numbers** are clamped on use (throw 0.1 to 20, lumens 0 to 200,000,
  shift -3 to 3), as `Venue::brightness()` and `Camera::sane` do. Show data must never reach a
  `clamp` with inverted bounds (CLAUDE.md, `ui.rs::within`).

Screens get two optional numbers for the light sums (both unwritten at default):

```rust
// on Surface
gain: f32,               // default 1.0: a matt white screen
rear: bool,              // a rear-projection screen: lit from behind, seen from the front
```

### 2.3 Rules that keep the model honest

- **At most one projector per display.** `Show::sanitize_stage` drops the `display` of a second
  one, with a log line.
- **A projector's display is a real display**: not a sub display (`is_sub_display`), not a display
  that shows the stage (`shows_stage`; the same loop `Stage3d::scene` already refuses).
- `Stage3d::forget_missing_outputs` (called from `remove_output`, `load`, `use_displays_as`) also
  clears a projector's `display`.
- **Blend groups need no new type.** Today a group is the displays that are `part_of` one whole
  picture. In simulation each projector throws what its window shows (its part, its keystone, its
  blend), so the overlap and the blend are simply what the light does. In mapping the group is
  "the projectors that light the same screen", found from geometry.
- **Sub displays** stay 2D. A mapped display ignores its sub displays' areas in the mapped
  picture (they are cut-outs of a flat picture and have no meaning on a surface). The UI says so
  when a display with sub displays is switched to mapped.

### 2.4 A projector library: generic presets yes, a products list no

Worth doing, small, pure:

- **Lens presets** in plain words: ultra short (0.25), short (0.5), standard zoom (1.2 to 2.0),
  long zoom (2.0 to 4.0), very long (4.0 to 7.0).
- **Brightness steps**: 3,000, 5,000, 10,000, 20,000, 30,000 lumens.
- **Resolutions**: 1280x800, 1920x1080, 1920x1200, 3840x2160, 4096x2160.
- **"My projectors"**: save a projector's lens, lumens and resolution under a name, kept per
  computer beside `layout.json` (not in the show), and offered in the Add menu.

Not worth doing now: a list of real makes and models. I have no source of throw ratios and shift
ranges that I can vouch for, and wrong numbers are worse than none. If the owner wants one, the
honest routes are GDTF files (section 9.4, unverified for projector lens data) or a hand-kept table
with a source per row.

### 2.5 The throw calculator (pure, in `projector.rs`)

These are what make step P1 useful on its own:

- `picture_at(distance) -> (width, height)` and `distance_for(width)`.
- `fit_to(surface) -> Placement`: stand square to a flat screen at the distance that fills it,
  with the lens shift it has. For a curved screen, on the arc's axis.
- `lens_for(position, surface) -> throw`: "I can only hang it here; which lens do I hire?"
- `pixel_size_at(point)`: millimetres per pixel on the surface.
- `footprint(scene) -> Vec<[f32; 3]>`: where the picture's outline lands (rays against the scene
  with `render3d::pick`), for the outline drawn on the surfaces.

---

## 3. Simulation in `render3d` (stage 2)

### 3.1 The light sums (new pure file `src/stage3d/light.rs`)

The model is deliberately simple and stated in the file's header:

- A projector gives `Φ` lumens at full white, spread evenly over its picture.
- On a plane square to the lens axis at depth `z` along the axis, the illuminance is
  `E0 = Φ / (w1 * h1 * z²)` lux. That is the inverse square law.
- On a real surface with normal `n`, for the ray direction `l` (surface to lens) and the axis
  `a`: `E = E0 * max(n·l, 0) / max(a·(-l), ε)`. The first cosine is the angle of incidence; the
  second undoes the plane's own obliquity off the axis. (Derivation: the same pencil of light
  covers `dA` on the square plane and `dA * cos θa / cos θi` on the surface.)
- The pixel's signal becomes light as `signal^gamma` per channel.
- A screen's luminance is `L = E * gain / π` nits (candela per square metre), gain 1 being a
  matt white surface. A venue part uses its material's colour as its reflectance.
- A rear-projection screen (`rear`) uses `|n·l|` and is seen from the other side; a front screen
  lit from behind shows nothing.
- Light from several projectors **adds** in linear light.

A worked check for the tests and the docs: 10,000 lumens on a 16:9 picture 6 m wide (20.25 m²)
is 494 lux, and 157 nits at gain 1. For scale: DCI cinema white is 48 nits (from memory; to be
checked before it is printed in the UI).

What the model leaves out, and the UI must not pretend otherwise: lens vignetting (real pictures
are dimmer at the edges), a screen's gain falling off with viewing angle, colour differences
between projectors, black level (a projector showing black still gives light), light bouncing
round the room, and the fact that quoted lumens are optimistic. The read-out says "about".

Functions: `illuminance(projector, point, normal) -> f32`, `luminance(...)`, `lit_by(scene,
projectors, point) -> Vec<(ProjectorId, lux)>` (with a shadow test by `pick` from the lens),
`pixel_size`, `blend_share` (section 4.5). All f32 in, f64 inside where it matters, no GPU.

### 3.2 What a projector throws: its window's picture

The honest picture to throw is **what leaves the lens**, which is what the display's window
shows: its layers through its `part_of` area, its corners or mesh, and its edge blend. That is
not what a `FeedJob` is today (the whole picture, flat, no blend). So:

- A new job kind on the stage thread, `ThrowJob { projector, groups, blend, size }`, drawn like a
  feed but with the display's **real** `Geometry` and its `BlendPass`, into **one layer of a 2D
  array texture** ("the throws"), `Rgba8Unorm`, one layer per simulated projector.
- `app.rs::update_outputs` already builds every display's `OutputGroup` with that geometry (the
  `groups` map at `app.rs:2453`). The part that collects "what this display's window shows" (its
  own group and its sub displays') should be made a function and used for both the window and the
  throw. A display with no window here (unassigned, or another computer's) uses the same
  geometry with no screen placement, the `None => pin` arm that exists.
- Size: longest side 960 at full quality, scaled by the governor like `feed_size`, never under
  256. Every layer is the same size; the projector's own aspect is in its matrix, so a picture
  stretched to fill the layer comes out right on the wall.
- A mapped projector (step P4) throws its **mapped** picture, drawn by the same map pass at this
  small size. So the simulation shows what mapping will do, and that closes a loop that CI can
  test (section 3.7).

This means the simulation shows the user's edge blend working, or not: set the blend widths, and
the bright band in the overlap fades. That is most of the value of stage 2 for a two-projector
theatre.

### 3.3 The shader: one loop, no new passes over the scene

`Scene` gets `throws: Vec<Throw>` (neutral type in `stage3d/scene.rs`): view-projection matrix
(off-axis, from the lens), eye, axis, `E0` factor (`Φ / (w1 h1)`), gamma, layer index, on/off.
Projectors belong in `Scene`, not `Options`: `Options` is `Copy` and is part of
`stage_live::signature`, and stays small (a mode and an exposure are added to it).

Bindings, all inside the downlevel limits:

- group 0 (frame) grows: binding 1 a uniform array of 16 `Throw`s (about 2.5 KiB of 16 KiB),
  binding 2 the throws (`texture_2d_array<f32>`), binding 3 its sampler, binding 4 the shadow
  maps (`texture_depth_2d_array`), binding 5 a comparison sampler. Three sampled textures in all
  with the part's picture. No fourth bind group is needed, which leaves one spare.

In `fs_part`, when light is on:

```
sum = ambient term (a dim headlight so the room is still readable)
for i in 0 .. count:                       // count <= 16, a uniform
    clip = throw[i].view_proj * world
    inside = clip inside the frustum and in front
    shadow = textureSampleCompareLevel(shadows, cmp, uv, i, depth)   // 2x2 PCF by the sampler
    signal = textureSampleLevel(throws, samp, uv, i, 0.0)
    sum += inside * shadow * pow(signal.rgb, gamma_i) * E0_i / z² * cosines
colour = albedo_or_gain * sum / π          // nits per channel
out = to_display(colour * exposure)        // or a false colour, by mode
```

Details that matter:

- **`textureSampleLevel` and `textureSampleCompareLevel` only.** They need no derivatives, so they
  are legal inside the loop and under `inside`. Plain `textureSample` would fail validation there.
- **The sum is made in float inside the shader**, so overlap never clips before it is counted,
  although the target stays `Rgba8Unorm`. False colour is computed from the float sum before it is
  written. No float render target and no additive passes over an 8-million-triangle venue.
- A screen (`lit == false`) with light on shows projected light times its gain instead of its
  feed painted on. With light off it is 0.25's previz, unchanged. A screen that no projector
  reaches is dark, which is the point. LED walls are screens with no projector: they keep their
  own picture, so a surface needs to know which it is. Rule: a screen that some projector's
  frustum reaches is lit by projectors; one that none reaches shows its feed as now (an LED wall).
  An explicit `emits: bool` on the surface would be clearer; see the open questions.
- **Index the uniform array with the loop counter only for reading.** That is ordinary WGSL, but
  given the DirectX 12 note in `vs_axes`, the step includes a test that runs the shader through
  naga's HLSL and Metal writers if `wgpu::naga` exposes them in this build (to be checked; if not,
  the Windows and Mac CI runs are the only proof).

### 3.4 Shadow maps

- One layer of a `Depth32Float` array per projector, 1024 square (4 MiB each, 64 MiB for 16).
  With the governor's scale under 1 the previews may use 512.
- A projector is the ideal shadow caster: its frustum **is** the light's frustum, so there is no
  cascade or fitting problem. Same reversed-Z convention as the main pass (near 0.1 m, far from
  the scene's bounds).
- Drawn with a depth-only pipeline over the same cached meshes (venue and screens).
- **Drawn again only when the scene key or that projector's matrix changes.** A still room costs
  nothing per frame. Dragging one projector redraws one layer.
- Bias: the venue is drawn two-sided and wound either way (`shader.wgsl` lights both faces), so
  back-face tricks are out. Use a slope-scaled depth bias in the pipeline plus a small offset
  along the normal in the lookup. Shadow acne on lavapipe versus real cards is a known risk.
- A see-through venue (x-ray) still casts shadows. A venue part that is hidden doesn't.

### 3.5 How many projectors, at what cost

- **Per frame**: 16 loop turns per shaded sample, two texture reads each, most turns ended early
  by the frustum test. No extra geometry passes. On a real card at preview sizes this should be
  small next to the venue's own triangle cost; **I have no measurement**, only lavapipe, where it
  will be slow and the governor will shrink the view.
- **On change**: one depth pass over the scene per moved projector.
- **Every drawn frame while something plays**: one small composite per projector for its throw.
  This is the real recurring cost: 16 projectors is 16 composites of up to 960 pixels wide on the
  stage thread, each its own submit (the one-`Slot::Stage` rule in `stage_render.rs`). Throws are
  only made for views that are being drawn, so a governed-down or hidden preview makes none.
- **More than 16**: P2 simulates the first 16 that are switched on and says so in words. The
  later answer is a per-part list (each `Part` carries the indices of the projectors whose
  frusta meet its bounds, the arrays grow to 64 layers at 512). That is a contained change to the
  part uniform. Whether it is needed depends on the owner's answer about rig sizes.

### 3.6 Rules kept from 0.25

- One uber-shader: the loop is in `fs_part`; the depth-only pipeline reuses `vs_part`.
- Every submit through `Gpu::submit`; the renderer still writes its uniforms per call.
- **Governor**: the simulation is drawn only by `stage_view::draw`, which already asks
  `preview_budget(PreviewId::Stage(host))`. Throws and shadow maps are made only as part of a job
  that has a view to draw, at the job's scale. No new preview kind is introduced. The look-through
  view (section 8.2) is the same `PreviewId::Stage` view with a different camera.
- **Displays before previews** stays as `StageJob::merged` has it. A display whose source is the
  stage may show the simulation (useful: send the lit previz to a client's monitor); that is a
  `StageSource` option, drawn ungoverned as today.
- Outputs never read any of it.

### 3.7 What lavapipe CI can test

Pixel tests in `src/render3d/tests.rs`, in the style of the existing four-colour test:

- A white projector square to a wall: lit inside the frustum, dark outside, edges where
  `Projector::footprint` says.
- The four-colour picture arrives the right way up and the right way round, front and rear.
- A box between projector and wall leaves a shadow where a `pick` ray from the lens is blocked.
- Two projectors overlapping: the overlap is the sum of the two alone (in linear light), and with
  a matched pair of `EdgeBlend`s it is flat to within a tolerance. This is `blend.rs`'s promise,
  tested for the first time in 3D.
- Falloff: the same projector at two distances, ratio `(d1/d2)²`; a tilted wall, the cosine.
- Lens shift moves the picture by the share it says.
- At a grid of sample points, the shader's value against `light.rs` (the reference pattern of
  `gpu.rs::the_edges_fade_as_the_blend_maths_says`).
- False-colour modes: a known lux lands in the right band.
- The round trip, once P4 exists: content painted straight on a curved screen (previz) against
  the same content mapped for a projector and thrown back by the simulation.

Pure tests need no GPU: `light.rs`, `projector.rs`, frustum and clipping maths.

Cannot be tested here: speed on any real card, depth precision and acne on real drivers, whether
the numbers match a light meter, HLSL and Metal compilation beyond what CI's runners do.

---

## 4. Mapping (stage 3)

### 4.1 What "mapped" means

A projector with `mapped: true` and a display: that display's window no longer shows its flat
picture through `part_of` and corners. Instead every projector pixel shows the content of the
spot on the screen that it lands on. Content lives on **screens** (their feeds), as in previz.
The projector's own display then has no content of its own; cues are sent to the displays that
feed the screens. (Those are often displays with no monitor: "shown only in the main window's
preview", `OutputConfig::monitor` None, usually with a `picture` size. They are the content
canvases, and 0.25 already draws them.)

### 4.2 Two ways to draw it, and the choice

**A live 3D pass per frame in the output thread**: draw the screens from the projector's lens
with depth, sampling the content canvases. Needs meshes, a depth buffer and either the venue as
occluders or a baked occlusion mask on every output thread, every frame. Handles moving screens.

**A lens map, made once**: for every projector pixel store which feed, the place in it, and a
brightness share. Per frame the output thread draws its content canvas and then one full-screen
pass that looks the content up through the map.

**Choice: the lens map.** Reasons:

- The output thread stays a compositor. No 3D code, meshes or depth buffers on the path that must
  never miss a frame. Its cost is fixed: one more pass, whatever the room looks like.
- Occlusion, several screens, edges, blend shares and (later) "as seen from a seat", domes and
  lens distortion are all just different maps. The output path never changes again.
- A map can be made by a **CPU reference** (rays through `pick`) at small sizes, so the GPU bake
  is tested against pure maths on lavapipe.
- A map made by someone else's calibration (section 9.5, MPCDI) could be dropped in later.

What it gives up: **screens that move during the show** (automation, tracked objects). Every
screen in NewQ's model is static today. If moving scenery is wanted later, the live pass can be
added beside the map for those screens only; nothing here blocks it.

### 4.3 The lens map

Per mapped projector, at the projector's pixel size:

- `where`: `Rg32Float`, the place in the feed's picture (0..1). Read with `textureLoad` (a 32-bit
  float texture isn't filterable under the downlevel limits, and exactness matters more).
  Half-float would not do: near 1.0 it resolves 1/2048, coarser than a pixel of a 4K canvas.
- `how`: `Rgba8Unorm`: r the brightness share (blend, 1 until P6), g which feed (0 = none,
  black), b coverage at the screen's edge (antialiasing, from baking at twice the size), a free.

Memory: 12 bytes a pixel, about 28 MiB for 1920x1200 and about 100 MiB for 4K, per projector, on
the computer that drives it. If that proves too much, a half-size `where` with a four-tap
interpolation guarded by the feed index is the fallback. Say so in the step; don't build it first.

Also carried: for each feed, the rectangle of its picture this projector actually uses, so the
canvas pass can be scissored to it.

**Made where**: on the stage-render thread, by `render3d` drawing the screens from the
projector's lens with a fragment entry that writes "place in the feed" instead of colour, and the
venue as depth-only occluders. It is the same vertex path, cameras and mesh cache; the off-axis
projection is the one the simulation uses. A `MapJob` is **output work**: it goes before every
preview job and is never governed. It runs only when its key changes: the scene key, the
projector, the feeds' regions, the fine-tune off. Dragging a line-up point re-bakes one
projector; that is a single small render.

**Until the first map lands** the display shows black, never the raw flat picture (a flash of an
unmapped picture across a set is worse than a moment of black). When a map is replaced the old
one stays in use until the new one is ready. On a lost device the maps are made again
(`StageLive::forget_device`).

**CPU reference**: `src/stage3d/lensmap.rs`, pure: for a pixel, the ray through the lens, `pick`
against the scene, the hit's uv through the surface's region. Used by tests at 64x36 and
compared with the GPU bake pixel for pixel away from edges.

### 4.4 The output path

`OutputJob::Frame` gains `map: Option<MappedFrame>` (`Arc<LensMap>` plus the feeds' groups). When
it is None the loop is exactly today's. When it is set, `output_loop` does:

1. **Canvas pass(es)**: for each feed the map uses (at most 4 per projector; more is refused in
   words), composite that display's layers on its plain canvas into a texture the thread keeps,
   scissored to the rectangle the map needs. This is `gpu.composite` with the geometry
   `stage_live::feed_groups` already makes. Each feed needs its own uniform slot: either new
   `Slot::Map(window, n)` slots made on demand the way `Slot::View(host, place)` are
   (`src/gpu.rs:1138-1280`: a slot is a uniform buffer and bind group per layer; the fixed ones
   are indexed by `Slot::index`, `View` ones are made when first drawn by `SlotMaker` into
   `LayerTexture::extra`), or a submit between feeds as `stage_render.rs` does. Prefer the
   slots: one submit per frame, as now.

**Content displays become output work.** This is the trap in today's code:
`app.rs:2518-2531` calls a display `preview_only` when it has no window, no stream and
`!stage.feeds_a_display(id)`, and then uploads its pictures only when
`preview_pace::uploads_due` says so, else `held_layers`. A content canvas with no monitor is
exactly that. Left alone, a mapped projector's video would drop to the previews' rate, or
freeze, whenever the governor steps down. So P4 must extend that test: **a display that feeds a
screen which a mapped projector on this computer lights is never `preview_only`** (a
`feeds_a_mapped_projector(id)` beside `feeds_a_display`), with a test that forces the governor
to Paused and checks the mapped output's content still changes every frame.
2. **Map pass** to the swapchain: a grid drawn through the projector's own fine-tune mesh
   (`Projector::fine`, a `mesh::Mesh` drawn by the existing mesh vertex code; the display's
   `corners` and `mesh` are not read while mapped, see 4.5), whose fragment reads the lens
   map at the pre-fine-tune pixel, samples the canvas there with `textureSampleLevel`, and
   multiplies by `share^(1/gamma)` and the coverage. With no fine-tune the grid is one quad and
   the lookup is exactly one map texel per pixel.
3. Submit, present. One `Gpu::submit`, as today.

New file `src/map_pass.rs` holds the pipeline and the shader (small: it is not the uber-shader
and should not be; the output path wants the least code that can go wrong). `render.rs` gets the
branch; `gpu.rs` the slot.

**Cost on the output path**: one extra render pass and one extra texture the size of the part of
the content the projector uses. The canvas pass draws the same layers the window would have
drawn anyway. Several projectors of one screen on one computer each draw their own scissored
part, which is what `part_of` costs today.

**Latency**: none added. The content is composited and mapped in the same frame, in the same
thread, from the same job. Projectors in separate windows stay as in step as they are today.

**Quality notes**: content is sampled bilinearly with no mip maps, so a canvas with far more
pixels than the projector resolves on a surface will shimmer. The pixel-size read-out (P1) tells
the user what canvas size is right; a mip chain per frame is a later option with a real cost.

**What is not affected**: `delay_ms` (engine-side), streams and recordings of the *content*
displays (they capture the flat canvas, which is what you want to record), the previews (they
show the content displays and the stage). A stream of a mapped projector's own display would
show the mapped picture; `CaptureThread` needs the same branch or should refuse in words. Decide
in the step; refusing is fine.

### 4.5 Blend from geometry (step P6)

Where several mapped projectors light the same spot, each gets a share of the light:

- For projector `i` at a spot, `d_i` is how far the spot's pixel is from the edge of `i`'s
  picture (and from the edge of what `i` can reach: a shadow's edge or the screen's edge), as a
  share of the picture. The share is `d_i^p / Σ d_j^p` over the projectors that reach the spot,
  `p` about 2. Shares add to one everywhere, fall to zero at each picture's edge, and are smooth.
  This is the standard distance-to-edge weighting from the multi-projector literature (Raskar and
  others, late 1990s; from memory).
- The signal is multiplied by `share^(1/gamma)`, exactly as `blend.rs::signal` does, with the
  projector's `Lens::gamma`.
- Optionally the share also evens out brightness (a projector twice as far gives a quarter of the
  light): divide by each one's `E` from `light.rs`. Off by default; it costs brightness.

It is baked into the lens map's `how.r` on the stage thread by a third fragment entry that loops
over the other projectors with the simulation's own uniforms and shadow maps. The pure reference
is `light.rs::blend_share`; tests: shares add to one, and the simulated overlap of two mapped
projectors is flat. It needs every projector in the show, on whatever computer, but only as
numbers from the show, so each computer bakes its own projectors' shares and they agree.

**How this sits with the hand tools**, for a mapped display:

| Setting | When not mapped | When mapped |
|---|---|---|
| `part_of` | which part of the wide picture | not used (geometry decides); left in the file |
| `corners` / `mesh` | the keystone or warp | not used; left in the file. The fine-tune is `Projector::fine` |
| `blend` (edges) | the edge fade | not used once P6 computes shares; before P6, still applied |
| `blend.gamma` | the fade's gamma | read as the lens gamma unless `Lens::gamma` is set |
| `picture` | the wide picture's size | the content canvas's size, as now |

"Left in the file" matters twice: switching mapping off restores the old look, and 0.25 opens the
show and plays it the old way (where there was an old way). The mapped fine-tune therefore has
its own field on the projector (`fine: Option<mesh::Mesh>`, flat to begin with), and the
display's `corners` and `mesh` are neither read nor written while it is mapped. The UI reuses the
existing mesh editor on it.

### 4.6 The show network

- Each computer draws its own projectors: `OutputConfig::computer` and `is_on(me)` already decide
  whose window a display is (`app.rs:4284-4290`). The map pass is in that window's thread.
- **The content is there on every computer.** `is_on` is used in only two places outside tests
  (`app.rs:1916` for the display states, `app.rs:4290` for windows) and nowhere in the engine:
  every computer's engine plays every display's cues, and the comment at `app.rs:4288` says
  another computer's display "is still previewed in the main window". So a follower has the
  layers of a content display whoever "owns" it; what it needs is the output-work rule of 4.4,
  so those layers are uploaded at full rate.
- Each computer bakes its own lens maps on its own stage thread. The inputs are the show (which
  followers have: `app/play.rs` show sync, per CLAUDE.md), the parametric screens (in the show) and the venue
  model for occlusion, which is already distributed (`manifest.rs:350-371`).
- A model used as a screen (section 6.3) must be added to the manifest the same way.
- A follower without the venue yet bakes without occlusion (the screens are in the show, so the
  map is right except where the room would shadow it) and bakes again when the venue arrives.
  The rule of 4.3 holds here too: black until that first map exists, never the flat picture.
  Cards and "not in step" behave as today.
- Shares (P6) are computed from the show alone, so two computers agree without talking.
  Rasterising differences between cards show only at shadow edges, by a fraction of a pixel.
- Line-up of a follower's projector is done from the primary (section 5.5).
- Not solved here: frame-exact lock between computers' outputs. That is the network's existing
  job (shared clock, R21); mapping adds nothing to it.

---

## 5. Line-up (calibration)

### 5.1 The idea, in the user's words

"Click a spot on the model that you can find in the real room. A cross appears in the projector.
Move the cross until it sits on that spot. Do it for four spots. NewQ works out where the
projector really is."

disguise calls this QuickCal and WATCHOUT 7 calls it virtual and reality points (section 9).

### 5.2 The UI flow

In Settings ▸ 3D stage, on a projector's row: **Line up…**. The page becomes two panes:

- **Left: the model**, in the look-through view of that projector. Click to drop a numbered
  point. It snaps to the nearest screen corner, facet corner or venue vertex within a few pixels
  (corners are what people can find in the room); hold a key to place it freely on the surface.
- **Right: the projector's picture**, a rectangle in its pixel shape, with the same numbers.
  Drag a number, or pick it and use the arrow keys (one pixel; Shift ten; Alt a tenth), while
  looking at the real surface. Arrow keys matter most: the operator is watching the wall.
- The real projector meanwhile shows black with every point as a small ring and the active one
  as a cross with its number, plus (a tick) the model's outlines as the current solve throws
  them. These are drawn on the CPU into one RGBA overlay texture and sent as an extra layer of
  the display's group, like the fps label and cards (`app.rs:4497` `renew_rgba`). No new GPU path.
- Under both: a line of plain words. "Lined up to within about 2 pixels." or "Point 5 disagrees
  with the others by 14 pixels. Check it, or switch it off." or "These points are all on one flat
  screen, so the lens can't be worked out from them. Using the lens you entered (1.5)."
- **Done** writes the projector's placement and lens (one undo step) and keeps the points.
  **Cancel** restores. **Back to before line-up** stays available afterwards (`LineUp::before`).

After a bump on the truss: open Line up, nudge the points that have moved, done. The points are
the durable thing, which is why they are saved.

Works for a projector that isn't mapped too: line-up then only makes the simulation true.

### 5.3 The solver (new pure module `src/calib/`)

Model: pixel `x = K [R | t] X` with `K` from throw, shift, aspect and the pixel size. No lens
distortion in the first version (disguise's own documentation says QuickCal doesn't handle it
either; only its camera-based OmniCal does).

Unknowns, and how many points they need:

| Points | What is solved | Notes |
|---|---|---|
| under 4 | nothing | the picture follows as a preview only |
| 4 or 5 | position and aim (6 numbers) | lens as entered. Points may all be on one flat screen |
| 6 or more | + throw ratio (7) | only if the points aren't nearly flat; else the throw stays as entered |
| 8 or more, not flat | + lens shift (9) | |

The ladder is automatic, by a conditioning test on the Jacobian, and the words under the panes
say which rung was used and why. Lens shift is the principal point; if it is wrong and not
solved, the pose comes out wrong, so the UI asks for it when it can't be found ("Is the lens
shifted? Roughly how much?") and warns when the answer matters.

Method:

1. **Starting guesses**, all tried, best kept:
   - the projector's current placement (the user put it roughly there; this is usually enough);
   - points on one plane: a homography by normalised DLT, decomposed with the known `K` into a
     rotation and position, both sign choices, the one in front of the lens kept;
   - six or more points not on a plane: the 3x4 matrix by normalised DLT, split into `K`, `R`,
     `t` by RQ, as a guess only.
2. **Refinement** by Levenberg-Marquardt on the reprojection error in pixels, over the free
   numbers of the rung, rotation as a small turn applied to the current one. Central-difference
   Jacobian in f64; nine unknowns at most.
3. **Report**: RMS and each point's error in pixels; the point whose removal improves the fit
   most, if it stands out; refusal in words for points in a line, repeated points, or a solution
   behind the lens.

**Pure-Rust options** (checked on crates.io 2026-11-20):

- **Hand-written, no dependency** (recommended). The problem is tiny and fixed-size: a Jacobi
  eigen-solve for the DLT's null vector (12x12 at most), a 3x3 polar decomposition, a 9x9
  Cholesky for the LM step. About 400 lines plus tests. The repo has no linear algebra crate
  today (`Cargo.toml`), and `render3d/maths.rs` and `stage3d/geometry.rs` are hand-written too.
- **`nalgebra` 0.35.0** (Apache-2.0, pure Rust, rust-version 1.89): SVD, QR and Cholesky ready
  made. A large dependency for three small routines; check the repo's toolchain against 1.89.
- **`levenberg-marquardt` 0.15.0** (MIT, rust-cv) sits on nalgebra; I did not check which
  nalgebra version it wants, and a mismatch would pull in two.
- **`lambda-twist` 0.7.0** (rust-cv, a P3P solver) is MPL-2.0, a different licence family from
  the rest of the tree; not needed given a starting guess.
- `opencv` crates are bindings to C++, not pure Rust; ruled out by the same reasoning as `ufbx`
  in the FBX verdict.

**Tests, all on made-up data**: random projectors (pose, throw, shift) and random points on the
sample venues' screens; exact data recovers the projector to rounding; half a pixel and two
pixels of noise recover it within stated bounds; four coplanar points with the true lens; six
coplanar points keep the entered throw and say so; one bad point is named; collinear points are
refused; rear projection (the projector behind the screen) solves; the rung chosen is the one
the table says. A property test runs a few hundred cases with a fixed seed.

### 5.4 Saving and restoring

```rust
pub struct LineUp {
    points: Vec<LinePoint>,           // { world: [f32;3], pixel: [f32;2], on: bool, surface: Option<SurfaceId> }
    solved: Option<Solved>,           // { rms_px, rung, the pixel size it was done at }
    before: Option<(Placement, Lens)>,// what "Back to before line-up" restores
}
```

All on the projector, all `#[serde(default)]`, unwritten when empty. Points are stored in world
metres and projector pixels. If the display's resolution changes, the points are scaled and the
UI says the line-up should be checked.

### 5.5 Across a show network

The projector being lined up may be another computer's. The overlay (crosses and outlines) is
show state that must reach it at once while the operator nudges. Two options:

- put the in-progress points in the show and rely on the existing show sync (`app/play.rs:30`
  `SETTLE` = 150 ms after a change; the whole show JSON, per CLAUDE.md): simple, but slow to the eye and it resends the show on every nudge;
- a small `proto::Msg::LineUp { projector, points, active }` sent with `Hub::tell(.., urgent)` (per CLAUDE.md),
  read as `Msg::Unknown` by an older NewQ.

The second is right; it is a later sub-step (P5b). The first version of line-up works on the
computer that owns the display, or on the primary for its own displays.

### 5.6 When the model is wrong

Real screens are never exactly where the drawing says. Three answers, in order of effort:

1. More points spread the error (the solve is least squares).
2. The **fine-tune** (section 4.5): a hand mesh warp on top of the mapping, per projector.
3. Later: let line-up move a *screen* instead of the projector ("the projector is right, the
   screen is 4 cm lower"), solving the screen's placement from the same kind of points.

### 5.7 Later: with a camera

Structured light (the projector shows coded stripes, a camera photographs them, every projector
pixel gets a place on the surface) is how disguise's OmniCal and tools such as VIOSO and Scalable
work. NewQ already has camera cues, so frames are available. It needs: camera calibration, sync
between pattern and capture, decoding, robust fitting, and a lot of testing in real rooms with
real cameras, none of which can be done in a cloud session. **Mark it later, and prefer reading
other tools' results (MPCDI, section 9.5) over writing our own.** The lens-map design is what makes
that import possible.

---

## 6. Content mapping modes

### 6.1 Laid on the screen (UV), first

What `Feed::Display { output, region }` already is: the display's picture laid flat on the
screen by the screen's own UVs (flat, along the arc of a curved screen, per facet). It is what
previz draws, so the mapped projector shows exactly what the 3D view promised. Ships with P4.

### 6.2 As seen from a seat (camera projection), second

Content made for one eye point, thrown onto whatever is there: the picture looks right from that
seat and stretches over the geometry. One optional field on the feed:

```rust
Feed::Display { output, region, seen_from: Option<String> }   // a viewpoint's name
```

0.25 ignores the field and lays the picture on by UV: a sane fallback. In the shader the picture
coordinate becomes "this point seen by that viewpoint's camera" instead of the mesh's uv. It
applies in previz, in the simulation and in the lens map by the same few lines, because all three
start from the world position. A **straight-on** variant (parallel rays, for a facade) is the
same code with another matrix. Step P7.

### 6.3 A model as a screen, with its own UVs, second

Projection mapping proper: the content is painted for a set piece's UV layout. Needs a new shape:

```rust
SurfaceShape::Model { part: String }       // a named part of the venue model
```

0.25 reads it as `SurfaceShape::Unknown` and keeps it. `load.rs` merges parts per material
today, so "a part" needs a stable name from the file (material or object name; the `gltf` crate
is built with `names`). The user picks it in the 3D view: "Use this as a screen". A separate
model file per screen can come later; in that case it must join the network manifest as the
venue does. Step P7.

### 6.4 Domes and spheres, later

Fisheye or equirectangular content on a dome: another way of turning a world direction into a
picture coordinate, so another lens-map flavour and a `SurfaceShape::Dome`. Nothing in the
design blocks it. Not before someone has a dome.

### 6.5 Order

UV first (it is free), then seen-from and model screens together (P7), domes later.

---

## 7. Steps, releases and who builds what

### 7.1 The steps

Sizes: S about a day of an agent, M two to four, L about a week. They are guesses.

**P1. Projectors in the room** (M). *No light yet.*
- Delivers: add a projector, choose a lens and brightness, stand it somewhere, aim it at a
  screen; see its cone as lines and its picture's outline on the surfaces; look through its
  lens; read picture size, distance, pixel size and lux at the pointer; "Fit to this screen" and
  "Which lens from here?". A theatre can answer "will the picture fill the cyc from the balcony
  rail, and how bright?" before hiring anything.
- Code: `stage3d/projector.rs`, `stage3d/light.rs` (new, pure); `Stage3d::projectors`; off-axis
  projection in `render3d/camera.rs` (`Camera` gains a shift and a fixed aspect); the UI
  (section 8).
- Risk: low. Everything but the look-through is pure.

**P2. Light** (L).
- Delivers: Light on/off in the 3D view. Projectors throw their displays' pictures; shadows,
  overlap, falloff. Views: Picture, Brightness, Overlap, Pixel size. The user sees their edge
  blend working in the overlap.
- Code: `render3d` (throws, shadow maps, the loop), `stage_render.rs` (`ThrowJob`),
  `stage_live.rs`, the window-picture function in `app.rs`.
- Risk: shadow quality, WGSL on DirectX 12 and Metal, cost on weak cards (governed).

**P3. The solver** (M). *Pure; ships inside P5 but can be built on day one.*
- Code: `src/calib/` only.

**P4. Mapping** (L).
- Delivers: tick "Mapped" on a projector and its display shows the screens' content from its own
  position: curved and faceted screens come out straight with no hand warp. Fine-tune on top.
  Hand edge blend still applies.
- Code: `stage3d/lensmap.rs` (reference), the bake in `render3d` and `stage_render.rs`,
  `src/map_pass.rs`, the branch in `render.rs::output_loop`, `OutputJob`, slots in `gpu.rs`,
  `app.rs::update_outputs` (the mapped frame, and the `preview_only` rule of 4.4 with its test).
- Risk: the highest. It changes the output path (behind an `Option`, so unmapped displays run
  the old code exactly). Memory on 4K. Must be soak-tested by Test Mode with a mapped display.

**P5. Line-up** (M, plus S for the network message).
- Delivers: the four-point (and more) alignment, with words for the fit.
- Code: `ui/stage_lineup.rs` (new), the overlay layer, `LineUp` in `projector.rs`.
- Risk: only testable by mouse-driven UI tests and synthetic data. The feel (nudging while
  watching a wall) cannot be judged here at all.

**P6. Blend from geometry** (M).
- Delivers: mapped projectors that overlap fade into each other by themselves.
- Code: the share bake (`render3d`, `stage_render.rs`), `light.rs::blend_share`.
- Risk: visible banding or seams on real projectors (gamma, black level) that no test here sees.

**P7. More ways to map** (M). Seen-from, straight-on, a model part as a screen.

**P8. Getting data in and out** (S to M). Projectors from a model's cameras; projectors in the
.glb export; then MVR (section 9.4).

**Later, not planned in detail**: more than 16 simulated projectors; black-level compensation;
lens distortion; camera-assisted line-up; MPCDI; domes; moving screens.

### 7.2 Releases

| Release | Steps | The headline for a user |
|---|---|---|
| 0.26.0 | P1 + P2 | "See your projectors' light in the room before you rig." |
| 0.27.0 | P4 + P5 (P3 inside) | "Line up a projector on a curved screen or a set in four clicks." |
| 0.28.0 | P6 + P7 | "Projectors blend themselves; map onto set pieces." |
| 0.28.x / 0.29 | P8 | imports |

P1 alone could go out as a 0.25.x if 0.26 runs long: it is useful and low-risk. P4 without P5
is usable only by someone willing to type positions, so they ship together. P4+P5 do **not**
depend on P2: mapping needs the model and the bake, not the light. If the owner cares more about
mapping than about pretty simulation, 0.27's work can come first and P2 second. See the open
questions.

### 7.3 Three agents, without collisions

The collision files are: `src/stage3d/mod.rs`, `src/stage3d/scene.rs`, `src/render3d/mod.rs` with
`shader.wgsl`, `src/show.rs`, `src/app.rs` (`update_outputs`), `src/stage_live.rs`,
`src/stage_render.rs`, `src/render.rs`, `src/ui/stage_edit.rs`, `src/ui/stage_view.rs`. Rule:
**new work in new files; one named owner per collision file per round; a first "contract" commit
that adds the shared types and nothing else.**

**Round 1 (0.26.0)**

- Contract commit (agent A, first hour): `Projector`, `Lens` in `stage3d/projector.rs`;
  `Stage3d::projectors`; `scene::Throw` and `Scene::throws`; `Camera`'s shift and aspect.
  Signatures only where bodies aren't ready.
- **Agent A, model and sums**: owns `stage3d/mod.rs`, `stage3d/projector.rs`, `stage3d/light.rs`,
  `stage3d/tests.rs`, `show.rs`. Then the integration in `stage_live.rs` and `app.rs`.
- **Agent B, renderer**: owns `render3d/*` and `stage_render.rs`. Off-axis camera, depth-only
  pipeline, the arrays, the loop, `ThrowJob`, the pixel tests.
- **Agent C, solver then UI**: `src/calib/` first (touches nothing else). Then
  `ui/stage_projectors.rs` (new: the list, the rows, the calculator, the gizmos, the read-out),
  with small hooks that C alone makes in `ui/stage_edit.rs` and `ui/stage_view.rs`.

**Round 2 (0.27.0)**

- Contract commit: `LensMap`, `MappedFrame`, `MapJob`, `Projector::mapped`, `LineUp`.
- **Agent A, the bake**: `stage3d/lensmap.rs`, the bake entry in `render3d`, `MapJob` in
  `stage_render.rs`, `stage_live.rs`.
- **Agent B, the output path**: `src/map_pass.rs`, `render.rs`, `gpu.rs` slots, `app.rs`. B can
  build and test against a made-up lens map (an identity map, a flip, a known warp) before A's
  bake exists.
- **Agent C, line-up**: `ui/stage_lineup.rs`, the overlay drawing (a new pure file), hooks in
  `ui/stage_edit.rs`.

**Round 3 (0.28.0)**: A the share bake; B seen-from and model screens (`stage3d/mod.rs`,
`load.rs`, shader); C imports (`load.rs` cameras after B's `load.rs` change lands, `glb.rs`, a
new `stage3d/mvr.rs`). B and C share `load.rs` here: order them.

Every round ends with: the paragraph in `.claude/notes/areas/stage3d.md` (once CLAUDE.md's), `.claude/notes/stage3d-plan.md` updated, README,
CHANGELOG lines, the old-show and default-unwritten tests, and a headless run of the app with a
sample venue and two projectors, with screenshots looked at.

### 7.4 What can't be tested here, all in one place

- Any real GPU: speed, memory, driver quirks, HLSL and Metal shader compilation beyond CI.
- Any real projector: whether lumens, gamma and blend look right; black level; the fit of a
  line-up; lens distortion; what the fine-tune is needed for in practice.
- Any real room: whether models match buildings; how people find points.
- Two real computers driving projectors of one blend.
- A camera, for anything in 5.7.

Everything above is tested by pure maths, by lavapipe pixel tests and by the simulate-what-you-map
round trip. Each release note should say "tried only in simulation" until someone has stood in
front of a wall, as CLAUDE.md does for every untried path.

---

## 8. UI, in plain language

### 8.1 Settings ▸ 3D stage

The page today has the view at the top, then Venue, Screens, Viewpoints, and the displays that
show the stage (`stage_edit.rs`: `venue_section`, `surfaces_section`, `viewpoints_section`,
`sources_section`). Add a **Projectors** section after Screens:

- **Add a projector ⏷**: "Aimed at a screen…" (pick the screen and a lens; it is stood at the
  right distance, square on: one click to a sensible start), "From my projectors" (saved
  presets), "Blank".
- Each row: name; **Shows** (which display; only real displays, none that shows the stage);
  **Lens**: "Distance for a 1 m wide picture" with the throw ratio beside it in grey, a zoom
  slider when the lens has a range; **Lens shift** up/down and left/right in percent;
  **Brightness** in lumens; **Stands at** X, Y, Z (the page's existing `placement_rows`, in the
  stage's units); **Aimed at** a screen from a menu, or turn/tilt/roll.
- Read-outs on the row, from `projector.rs` and `light.rs`: "Picture on Screen 1: 5.8 m by
  3.3 m, fills 96% of it. About 520 lux, about 165 nits. One pixel is about 3 mm."
- Buttons: **Fit to screen**, **Which lens from here?**, **Look through**, **Line up…** (P5),
  a **Mapped** tick (P4) with one line under it: "This display shows the screens as this
  projector sees them, instead of its own picture."
- Warnings in words, not colours alone: "The picture is bigger than the screen: 40 cm spills
  over the left edge." "The seats in row C are in the beam." (that one needs audience geometry;
  later.) "This lens can't zoom that far."

### 8.2 In the 3D view

`stage_view.rs` has an overlay bar with Viewpoint, See all, Grid, X-ray, Edit…. Add:

- **Light: Off / On** (P2).
- **Show ⏷**: Picture, Brightness, Overlap, Pixel size. Each false-colour view has a small
  legend in the corner with real numbers ("50, 100, 200, 400 nits"; "1, 2, 3+ projectors";
  "1, 2, 5, 10 mm per pixel"). Brightness mode has an **Exposure** slider and "Auto".
- **Viewpoint ⏷** gains "Through Projector 1…": the look-through view, letterboxed to the
  projector's shape, with the lens shift in the projection.
- Projectors are small box parts in the scene, so `pick` finds them (a `part_projector` beside
  `part_surface`). Selected, a projector shows its cone as lines and its picture's outline on
  the surfaces. These are drawn with egui's painter over the picture using `Camera::project`
  (lines need clipping at the near plane: a pure function in `camera.rs`). No depth, so lines
  show through walls; that is acceptable and even helpful for a first version.
- **Handles** (edit only, not in show mode, as `editable` decides): drag the body to slide it
  over the floor (or along a truss height with a modifier), a vertical arrow for height, and an
  **aim dot** where the lens axis meets the scene, which can be dragged over surfaces. Three
  handles; no rotate rings. Numbers stay in the page for exactness.
- Under the pointer (hover, edit or not): "Projector 1 + Projector 2 · about 410 lux · about
  130 nits here · 1 pixel ≈ 3 mm". From `light.rs` on the CPU; never read back from the card.

### 8.3 The Preview pane's 3D mode

View only, as now, with Light, Show and the viewpoint menu (including the look-through views).
Editing stays on the Settings page ("Edit…" opens it). The pane goes through the governor as it
does today; nothing new to wire.

### 8.4 The small theatre's path

Someone with one projector and a flat screen should never have to meet any of this. The Displays
page's corner pin stays the first thing offered. The 3D stage's projectors are reached only from
Settings ▸ 3D stage. If a user ticks Mapped for a single flat screen, a line says: "For one flat
screen, the four corners on the Displays page do the same job with less work."

---

## 9. Formats and interop

What I know, what I looked up today, and what I don't know.

### 9.1 disguise

- Looked up (help.disguise.one, 2026-11-20): projectors are set up by "position, rotation and
  lens qualities"; they are matched to reality by manual calibration or **QuickCal**. QuickCal
  uses reference points, with a stated minimum of six and a suggestion of ten to fifteen per
  projector; it does not handle lens distortion. **OmniCal** is the camera-based system and does.
- Not known: any documented file format for projector positions or calibration that another
  program could read. disguise projects are its own format. The pages I found mention exporting
  a reshaped *mesh* from OmniCal, nothing about projector data. **I would not plan an import
  from disguise.**

### 9.2 WATCHOUT

- Looked up (docs.dataton.com, WATCHOUT 7, 2026-11-20): a 3D mapping projector has Eye, Target,
  Roll, horizontal and vertical lens shift in percent, and a width/distance ratio. Calibration
  pairs virtual points with reality points; at least six virtual points are needed.
- A VIOSO page describes bringing calibration into WATCHOUT 7 through **MPCDI**, and a Dataton
  knowledge-base article covers MPCDI in WATCHOUT 6.4 and later.
- Not known: WATCHOUT 7's show file layout. I would not plan to read it.
- Useful all the same: the three lens numbers above are the same three this plan uses, so a user
  can type a WATCHOUT projector's numbers into NewQ by hand. Eye/Target/Roll is worth offering as
  an alternative way of typing an aim.

### 9.3 glTF cameras: worth it, small

- glTF 2.0 has camera nodes (perspective: vertical field of view, aspect ratio, near, far) with
  the node's transform, looking along -Z, which is why the projector's convention above matches.
  `load.rs` ignores cameras today (the plan note lists them as ignored). The `gltf` crate is
  already a dependency.
- A perspective camera has **no lens shift**. Imported projectors come in on-axis; line-up fixes
  that.
- Offer: after loading a venue with cameras, "This model has 3 cameras. Add them as projectors /
  as viewpoints." Blender, Cinema 4D and others export cameras. About a day, in P8.
- And the other way: `glb.rs` (ours) can write projectors as camera nodes in "Export as .glb".

### 9.4 MVR and GDTF: plausible, later

- Looked up (the MVR spec on GitHub, version 1.6, DIN SPEC 15801, first 100,000 characters read):
  an MVR file is a zip with `GeneralSceneDescription.xml`. It has **`Projector`** and
  **`VideoScreen`** objects. A `Matrix` is 4x3, right-handed, **Z up, millimetres**. Geometry is
  3DS (deprecated) or glTF. A `Projector` has `Projections` with a `Source` (NDI, File, CITP,
  CaptureDevice) and a `ScaleHandeling`, and a `GDTFSpec`/`GDTFMode`.
- The Projector node itself carries **no throw ratio or lens shift**. The spec's example names a
  GDTF mode `Projector@ThrowRatio1_7_to_2_2`, which suggests the lens is described in the GDTF
  file. I have not read the GDTF spec for how, and won't guess.
- NewQ already has `zip` and `roxmltree` (`Cargo.toml`), and the glTF loader. So reading an MVR
  for **where the projectors and screens are** (and the venue geometry inside it) is a moderate
  job. Lighting designers export MVR from Vectorworks, Capture, grandMA3 and others (from
  memory), so it is the likeliest way a real venue drawing reaches NewQ.
- Worth it, after glTF cameras, and only for positions at first. Test files must come from real
  programs, the same caution as the FBX verdict.

### 9.5 MPCDI: the door to camera calibration, later

- MPCDI is a VESA standard for exchanging projector warp and blend data. Known from memory and
  partly confirmed today (Unreal's nDisplay docs, a VIOSO page): a container with an XML
  description, per-projector warp data as PFM float images and blend maps as PNG; version 2
  exists; profiles include a 2D one. **I have not read the specification** and could not find it
  freely today; the layout must be confirmed from the document before anything is built.
- Why it matters: a 2D-profile MPCDI warp is, in substance, a lens map (per projector pixel, a
  place in the content, plus a blend picture). If NewQ can load one into the same structure the
  map pass uses, then VIOSO, Scalable and similar camera systems can line NewQ up, and NewQ
  never has to write structured light itself.
- Plan: nothing now. Keep the lens map's layout close to "a float picture of places and a blend
  picture" so the import stays easy. That is already what section 4.3 describes.

### 9.6 OBJ, FBX

OBJ has no cameras. FBX has, and stays refused for the reasons in the FBX verdict.

---

## 10. Risks, plainly

1. **The output path is touched (P4).** Mitigation: the branch is an `Option`; the unmapped path
   is byte-for-byte today's; Test Mode gets a mapped display in its Debug test; the map pass is
   its own tiny pipeline.
2. **Nothing has met a real projector.** The model is first-order physics. Expect the first real
   use to find gamma and black-level surprises in blends. Keep the hand blend usable beside the
   computed one until someone reports otherwise.
3. **Memory for 4K lens maps** (about 100 MiB each). Stated, with a fallback.
4. **Shader portability.** One refusal on DirectX 12 has already happened in this shader. Loops
   over uniform arrays and array textures are ordinary, but untried here.
5. **Scope.** disguise is a company's decade. This plan gets to "static screens, point line-up,
   computed blend" and stops. It does not include tracking, moving scenery, camera calibration,
   colour matching or content rendering in 3D (Notch-style). Those are separate decisions.
6. **Ease of use.** 3D placement is the hardest UI in the program. The plan's defences are:
   "Aimed at a screen" as the way to add a projector, three handles only, words with every
   number, and line-up doing the precise work so nobody types coordinates off a drawing.

---

## 11. Open questions for the owner

1. **Order: light first or mapping first?** P2 (simulation) and P4+P5 (mapping and line-up) don't
   depend on each other. Simulation helps planning and selling a design; mapping changes what
   comes out of the projectors. Which should 0.26 be?
2. **How big a rig is the target for the first version?** 16 simulated projectors in one view is
   the first limit. Is that enough for the arena case for now, or should the 64 version be in the
   first plan?
3. **Static screens only?** The lens map assumes nothing moves during the show. Is moving
   scenery or tracked objects wanted within the next year? It changes P4's design if so.
4. **Is one content canvas per screen the right mental model?** In mapped mode, cues go to the
   displays that feed the screens (often displays with no monitor), and the projectors' own
   displays carry no cues. Should NewQ make that explicit, for instance by naming such displays
   "canvases" in the UI?
5. **LED walls beside projection**: should a screen say what it is (LED, front projection, rear
   projection) rather than NewQ guessing from whether a projector reaches it? I recommend yes, as
   a three-way choice on the screen, default "works it out".
6. **A dependency for the solver?** Hand-written maths (recommended, about 400 lines) or
   `nalgebra`?
7. **Line-up over the network** needs a new message so crosses move at once on another computer.
   Acceptable in 0.27, or is line-up on the computer that owns the display enough at first?
8. **A projector products list**: generic lens and brightness presets plus "My projectors" is
   what I recommend. Is a list of real models wanted enough to maintain one, and from what
   source?
9. **Which imports matter to you?** glTF cameras are cheap. MVR is moderate and is what lighting
   people send. MPCDI opens camera calibration by other tools. Rank them, or name the program
   your venues' drawings come from.
10. **Read-out units**: lux and nits, or also foot-candles and foot-lamberts for US users (the
    stage's `LengthUnit` already has feet)?
11. **Streaming or recording a mapped projector's own display**: refuse in words (recommended),
    or make it work?
12. **Real hardware**: is there a projector, a second computer and a wall that someone can try
    0.26 and 0.27 on before they are called done? Everything here is otherwise "tried only in
    simulation".
13. **Should the mapped fine-tune (`Projector::fine`) be a mesh only**, or also offer four
    corners? A mesh covers both; one tool is simpler.
14. **Ambient light**: is a "house lights at N lux" setting wanted in the simulation, so washed-out
    pictures show as washed out? Small, but one more number for users to not know.

---

## Appendix: new files and types by step

| Step | New files | Changed files (owner in brackets) |
|---|---|---|
| P1 | `stage3d/projector.rs`, `stage3d/light.rs`, `ui/stage_projectors.rs` | `stage3d/mod.rs`, `show.rs` (A); `render3d/camera.rs` (B); `ui/stage_edit.rs`, `ui/stage_view.rs` (C) |
| P2 | none | `stage3d/scene.rs` (contract); `render3d/mod.rs`, `shader.wgsl`, `render3d/tests.rs`, `stage_render.rs` (B); `stage_live.rs`, `app.rs` (A) |
| P3 | `calib/mod.rs`, `calib/linear.rs`, `calib/tests.rs` | `main.rs` (mod line) |
| P4 | `stage3d/lensmap.rs`, `map_pass.rs` | `render3d/mod.rs`, `stage_render.rs`, `stage_live.rs` (A); `render.rs`, `gpu.rs`, `app.rs` (B) |
| P5 | `ui/stage_lineup.rs`, `stage3d/lineup_overlay.rs` | `stage3d/projector.rs`, `ui/stage_edit.rs` (C); later `net/proto.rs`, `net/hub.rs` |
| P6 | none | `stage3d/light.rs`, `render3d/*`, `stage_render.rs` (A) |
| P7 | none | `stage3d/mod.rs`, `stage3d/load.rs`, `shader.wgsl`, `net/store/manifest.rs` (B) |
| P8 | `stage3d/mvr.rs` | `stage3d/load.rs`, `stage3d/glb.rs`, `ui/stage_edit.rs` (C) |

Sources looked up on 2026-11-20:

- disguise QuickCal and OmniCal: https://help.disguise.one/workflows/calibration-projection/quickcal-ref-points ,
  https://help.disguise.one/workflows/calibration-projection/omnical-overview ,
  https://help.disguise.one/designer/stage-setup/projector-simulation/projector-simulation-overview
- WATCHOUT 7 3D mapping projectors: https://docs.dataton.com/watchout-7-new/watchout/displays-and-outputs/3d-mapping-projectors.html ,
  https://docs.dataton.com/watchout-7/3d/mapping.html
- WATCHOUT and MPCDI: https://docs.vioso.com/guide/mediaservers/dataton/watchout-v.7 ,
  https://knowledge.dataton.com/knowledge/using-mpcdi-in-watchout
- MVR 1.6: https://github.com/mvrdevelopment/spec/blob/main/mvr-spec.md
- crates.io (`cargo info`): nalgebra 0.35.0, levenberg-marquardt 0.15.0, lambda-twist 0.7.0
