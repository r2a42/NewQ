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
2. **Projector simulation (later).** Projectors as positioned cones of light that throw their
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
