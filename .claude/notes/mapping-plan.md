# Mapping plan (revision of projection-plan.md sections 4, 5, 7, 11)

Status: SKELETON, being written. Sections fill in as they are finished.

1. The short version
2. What changed since projection-plan.md, and why
3. Facts checked against today's code
4. The design
   4.1 Mapping (the map pass)
   4.2 Moving screens
   4.3 Canvases
   4.4 Line-up across the network
   4.5 Streaming and recording a mapped display
5. For the solver agent (src/calib/)
6. Steps for the agents (rounds)
7. Risks, and what needs a real projector, room or second computer
8. The decided questions, checked again
9. Questions still open for the owner

---

(Sections 1, 2, 4-9 follow as they are finished; section 3 is saved first because its line
numbers were the costliest to find.)

## 3. Facts checked against today's code (0.27.2, 26 Nov 2026)

Each line is what the code says now, with where. "Old plan" is `projection-plan.md`.

### 3.1 The output path

- An output window's thread is `output_loop` at `src/render.rs:499` (old plan: 451). Per frame it takes
  `OutputJob::Frame { size, groups, blend }` (`render.rs:423`), does one `gpu.composite` and one
  `gpu.submit`, then presents. Still the whole output path.
- **One thread per monitor, not per display.** `update_outputs` builds a list of `OutputGroup`s for each
  monitor's window (the display and its sub displays, `src/app.rs:2824` on), adds the card or Identify
  last (`put_card`, `app.rs:4627`) and sends the list with the display's own blend. The old plan's
  "`OutputJob::Frame` gains `map`" forgot the sub displays and the cards that share the window.
- The window counts a "new picture" by comparing a `Shot` of what it drew (`render.rs:266`,
  `is_new_picture` `render.rs:311`): size, blend, each group's geometry and its layers' stamps. Test Mode's
  shown-frame counts rest on it. The old plan doesn't mention it; a mapped frame must be in the `Shot`.
- `Gpu::composite` (`src/gpu.rs:1732`) writes every layer's uniforms for the caller's `Slot`, then draws
  them and the optional `BlendPass`. Slots: `gpu.rs:1152` (`Window(u8)` 8 of them, `Capture(u8)` 2, `Stage`
  one, `View(host, place)` made on demand through `SlotMaker`, `gpu.rs:1233`). Old plan: 1138-1280.
- The device is asked for `Limits::downlevel_defaults()` at `gpu.rs:234` and `gpu.rs:342` (old plan:
  219-221).
- `LayerTexture::new_target` (`gpu.rs:1389`) makes a texture another pass can draw into and the
  compositor can then show as a layer. A display that shows the stage already works this way
  (`StageLive::output_layer`, `src/stage_live.rs:609`).
- Pictures are uploaded on the "uploads" thread (`src/upload.rs`), and a layer is drawn only once it is
  filled. Unchanged in principle since the old plan; nothing in mapping touches it.

### 3.2 `update_outputs` and the previews' pace

- `update_outputs` is at `src/app.rs:2368` (old plan: 2359).
- The `preview_only` rule is at `app.rs:2538-2540` (old plan: 2518-2531): a display with no window here,
  no stream and `!self.stage.feeds_a_display(id)` has its pictures uploaded only at the previews' pace
  (`app.rs:2550`), else `held_layers` (`app.rs:2594`, `app.rs:4534`). The trap the old plan named is still
  there: a content display with no monitor is exactly `preview_only`.
- `StageLive::feeds_a_display` is at `stage_live.rs:604`; `governed_only` at `stage_live.rs:673`.
- A projector's throw for the light is made by `app.rs::throw_picture` (`app.rs:4731`) from the display's
  group, and `projector_throws` (`app.rs:4705`); built in P2, not as the old plan's 3.2 described (no
  shared "what the window shows" function; the throw is composited on the stage thread through
  `Slot::Stage`).
- `OutputConfig` is at `src/show.rs:703` (old plan: 677). `monitor: None` means "shown only in the main
  window's preview" (`show.rs:746-749`); `computer` at `show.rs:754`; `is_on` at `show.rs:826`, used at
  `app.rs:1924` and `app.rs:4395` (old plan: 1916, 4290).
- `renew_rgba` is at `app.rs:4604` (old plan: 4497).

### 3.3 Streams and recordings

- `CaptureThread` is at `render.rs:774`, `capture_loop` at `render.rs:930`. It is given
  `CaptureJob::Groups` (`render.rs:766`) and composites them with **no blend** (`render.rs:999`) into an
  `Rgba8Unorm` texture of the stream's size, reads it back and hands it to FFmpeg, at the stream's rate.
- `update_streams` (`app.rs:2090`) says a stream is "the display's own picture on its own canvas, without
  the shaping a projector needs": no keystone, mesh or blend. It skips sub displays and displays that are
  part of another's picture (`app.rs:2098`).
- **Streams are the primary's alone** (`sends_here`, `app.rs:2100`, `src/app/play.rs:568`). So when a
  follower drives a mapped projector and its display has a stream, it is the *primary* that must draw the
  mapped picture. The old plan's "each computer bakes its own projectors' maps" isn't enough.
- At most two streams at a time (`gpu::CAPTURE_SLOTS`, `gpu.rs:1183`).

### 3.4 The 3D stage

- The stage-render thread: `src/stage_render.rs`. Jobs are `FeedJob` (a display's flat picture, longest
  side `FEED_SIDE` 1024, `stage_render.rs:49`), `ThrowJob` (`stage_render.rs:58`) and `ViewJob`; latest
  wins, merged (`StageJob::merged`, `stage_render.rs:126`). It quits after `IDLE_RELEASE`, 30 s, with no 3D
  view in sight and no display showing the stage (`stage_live.rs:41`, `stage_live.rs:827`). Anything an
  output needs from that thread must keep it, or must not need it again once made.
- `render3d::Renderer` is one per `Gpu`, writes its uniforms per call and submits by itself (area note,
  `render3d/mod.rs`). It can't be used from an output thread. The light (P2) is its own shader module,
  entry `fs_lit` and bind group 3 (`render3d/lit.rs`), not the loop in `fs_part` the old plan's 3.3
  described; so the old plan's 4.5 "a third fragment entry with the simulation's own uniforms" must be
  read against `lit.rs`'s `LightKit`.
- `Stage3d::shape_key` (`src/stage3d/mod.rs:739`) hashes the whole stage; a new scene is made when it
  changes. So **anything that changes every frame must not be in `Stage3d`**: it would rebuild the scene,
  the shadow maps and (in this plan) the lens maps every frame.
- `Surface` (`stage3d/mod.rs:363`) has `gain` and `rear` already; it has **no** `extra` map, and nor has
  `Stage3d` (no `extra` in `stage3d/mod.rs`). `Projector` and `Lens` have one (`stage3d/projector.rs:316`),
  so 0.27.2 keeps a later NewQ's `mapped`, `fine` and `line_up` on a projector through a save, and would
  drop a new field on a screen. (The old plan said 0.25 drops all of it.)
- `Projector` (`projector.rs:289`) has no `mapped`, `fine` or `line_up` yet.
- `src/calib/` doesn't exist on this branch or in the solver agent's worktree yet (looked 26 Nov).

### 3.5 The show network

- `Msg` is at `src/net/proto.rs:217`, tagged by `t`, with `#[serde(other)] Unknown` (`proto.rs:307`).
- The hub hands the app a message it doesn't handle itself only over a heeded session (`hub.rs:1585`,
  `heeded` at `hub.rs:2593`): a follower hears the session it obeys, a primary the sessions it trusts.
  `Hub::tell(to, msg, urgent)` is at `hub.rs:398`.
- The app's handler is `play_message` (`app/play.rs:311`). Each arm a follower obeys is guarded by
  `primary` (`app/play.rs:312-313`: the sender is the primary this computer follows).
- The show is sent whole, 150 ms after a change (`SETTLE`, `app/play.rs:30`; `play_send_show`,
  `app/play.rs:432`), and a follower holds commands while its show hash differs. So **in-progress line-up
  points and a moving screen's position can't be show edits.**
- Remote input, streams, send cues and timecode are the primary's alone (R8, `sends_here`).

### 3.6 Cues and the engine

- A fade cue moves a *cue's picture* (`FadeCue::geometry: GeometryTarget`, `show.rs:2599`), ramped in the
  engine (`Ramp`, `src/engine.rs:108`). Nothing in the show model moves a *screen* today.
- A cue kind this NewQ can't read is `CueKind::Unknown`, kept and never played (`show.rs:1638`,
  `show/compat.rs`). A new field on an existing cue kind would be ignored by an older NewQ, which would
  then play a *different* action; a new kind is the safe way.
- No PosiStageNet code exists (`git grep -i posistage` finds nothing).
