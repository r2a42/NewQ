# 0.25.0: previews and the 3D stage (plan)

Owner, 2026-11-20:
- "NewQ should scale from the simplest slideshow to an arena style electronica fest."
- The end goal is a choice of previz, projector simulation and full disguise-style mapping (or all three). Stages are fine.
- Formats: whatever disguise and WATCHOUT use, plus newer open formats.
- The 3D view also works as an output (to a monitor or stream).

## The preview pane (0.25.0)

The Preview pane has three modes, toggled in its header: **Single** (as now), **Multiview**
(configurable 2 to 16 tiles, arrangeable, kept in the show), and **3D**. There can be several Preview
panes, each popped out to its own window, each with its own mode. The only limit is what the hardware
can draw.

Previews give way first: when the outputs are at risk, previews draw fewer frames or at a lower
resolution, never the outputs (the governor, pure and tested).

## The 3D stage, in stages

1. **Previz (0.25.0).**
   - Load a venue model.
   - Place surfaces in it: flat, curved (part of a cylinder), or faceted (several flat faces, such as a
     jumbotron box or an LED wall of angled panels).
   - Each surface shows a display's picture, or a part of it. A blended projector group shows as its
     whole picture: the display that its projectors are `part_of`.
   - Orbit, pan and zoom; saved viewpoints.
   - The 3D view can be a display's source, so it can go to a monitor or a stream.
2. **Projector simulation (later).** Step P1 of `projection-plan.md` is built: projectors stood in
   the room, the throw calculator, lines for their cones and outlines, the look through a lens,
   lux and nits by sums. The light itself (P2) is next. Projectors as positioned cones of light that throw their
   display's picture onto the venue and the surfaces: overlaps, blends, shadows, brightness falloff.
3. **Mapping (later).** The 3D model drives the real outputs: each projector's picture is computed
   from its position and lens and the surface's UVs, as disguise does. This needs calibration (point
   alignment), which is a stage of its own.

## Formats
- disguise imports **OBJ** and **FBX**.
- WATCHOUT 7 imports **OBJ, FBX, glTF/GLB, 3DS** (to be confirmed).
- **Plan:**
  - 0.25.0: glTF/GLB (open, current, textures, metres, Y-up) and OBJ+MTL (universal). Both have
    pure-Rust readers (`gltf`, `tobj`).
  - FBX: binary FBX 7.x through a pure-Rust reader if one is good enough (`fbxcel-dom` to be judged);
    otherwise tell the user to export glTF. 3DS and Collada later.
- **FBX verdict (2026-11-20): not in 0.25.0.** `load_venue` refuses a `.fbx` (or a file starting
  `Kaydara FBX Binary`/`; FBX`) with `load::FBX_WORDS`: "Export the model as glTF (.glb) from your 3D
  program … or as OBJ." Why `fbxcel-dom` 0.0.10 (MIT/Apache, last release Feb 2023, ~7k downloads,
  pre-0.1) isn't good enough yet:
  - It reads binary FBX 7.4/7.5 into objects, meshes (polygon vertex indices, layer elements for
    normals and UVs, a `triangulate_each` that needs our own triangulator) and materials, but has
    **no model transforms**: no `Lcl Translation/Rotation/Scaling`, no pre/post-rotation, pivots,
    offsets, rotation order or geometric transforms. We would hand-write FBX's transform stack
    (`T * Roff * Rp * Rpre * R * Rpost⁻¹ * Rp⁻¹ * Soff * Sp * S * Sp⁻¹`, per node, with six rotation
    orders and inherit types) and read `GlobalSettings` (UpAxis/FrontAxis/CoordAxis and their signs,
    UnitScaleFactor in cm) from raw properties. Getting that wrong puts a venue's parts in the wrong
    places, which is worse than refusing.
  - ASCII FBX and FBX 6 are not read at all (only binary 7.x).
  - Nothing to test against: no exporter here writes FBX, and a hand-made test file only checks our
    own reading of the format. It wants real files from 3ds Max, Maya, Cinema 4D and Blender.
  - Every program that writes FBX also writes glTF or OBJ (Blender, Maya, 3ds Max, C4D, SketchUp Pro),
    so the cost to the user is one export.
  - When it is taken up: a few days with real files; or the transform stack upstreamed into
    `fbxcel-dom`. Alternatives looked at: `ufbx` (C, MIT, excellent) would need `cc` and unsafe FFI;
    Assimp (BSD, C++) far too big.
- **What 0.25.0's loaders read** (`src/stage3d/load.rs`):
  - glTF 2.0 (.gltf with data URIs or files beside it, .glb): every mesh of the default scene (else
    the first, else the root nodes) with node transforms baked into the positions, normals by the
    inverse transpose, winding turned under a mirror; triangles, strips and fans (points and lines
    left out, with a note); base colour factor and base colour texture (PNG/JPEG) on the primitive's
    `texCoord` set, the factor multiplied into the picture. Skins, animation, morph targets, lights,
    cameras and other material channels are ignored. `KHR_draco_mesh_compression`, `KHR_mesh_quantization` and
    `EXT_meshopt_compression` are refused in words; other required extensions are refused unless
    they only change looks (`GLTF_IGNORABLE`). Views and accessors are checked against the buffers
    before reading.
  - OBJ + MTL: objects and groups, `Kd` (+ `d`) and `map_Kd` (a `\` path and options before the
    name are coped with; the picture is shown as it is, not times `Kd`), v flipped. A missing MTL or
    picture loads grey with a warning.
  - Units and up axis are the venue's settings (`LengthUnit`, `UpAxis`), applied as the part's
    transform, so changing them needs no reload; brightness is baked in, so it does.
  - Parts are merged per material. Over 2M triangles a warning, over 8M the rest left out; pictures
    over 4096 px made smaller.

## Code layout (agreed between the agents)
- `src/stage3d/`: the model in the show (`Stage3d`, unwritten when empty), surfaces' geometry (pure),
  and the loaders.
- `src/render3d.rs` (or `src/render3d/`): the wgpu renderer, on neutral types: `Mesh`, `Paint`, `Part`,
  `Camera`; a depth buffer; renders to any texture view. The venue is lit; screens are unlit (their
  picture at full brightness).
- **Integration (after the multiview work lands):**
  - the Preview pane's 3D mode, the editing UI and several Preview panes;
  - the display pictures as textures (each display's composite drawn off screen, like
    `CaptureThread`'s);
  - the 3D view as a display source.
