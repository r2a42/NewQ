# Mapping plan: 3D projection mapping, moving screens, canvases, line-up over the network

Written 2026-11-26 against 0.27.2 (`main` at d390cb6). It revises sections 4 (mapping), 5 (line-up),
7 (steps) and 11 (open questions) of `.claude/notes/projection-plan.md` ("the old plan", written
20 Nov against 0.25.0). Everything else in the old plan still stands and is named by its section
number, not copied. Nothing here is built. Statements about the code give a file and line, read on
this date; the few things I could not check are marked "unverified".

Words: *screen* (a 3D stage surface), *display*, *projector*, *canvas* (new, section 4.3),
*lens map* (a lookup picture made off the output path, section 4.1), *live pass* (a moving screen
drawn in the output's own frame, section 4.2), *line-up* (calibration by points).

Contents: 1 the short version · 2 what changed from the old plan · 3 facts checked against the code ·
4 the design (4.1 mapping, 4.2 moving screens, 4.3 canvases, 4.4 line-up over the network,
4.5 streaming) · 5 for the solver agent · 6 steps for the agents · 7 risks and what can't be tried
here · 8 the decided questions, checked again · 9 questions still open

---

## 1. The short version

1. **The old plan's mapping design holds, with one correction and one addition.** The correction:
   the mapped picture is drawn into a picture of its own and then shown by the *existing* compositor
   as an ordinary layer. That one change is what makes the fine-tune, the hand edge blend, sub
   displays, cards, Identify, line-up crosses, streaming and the light simulation all work with code
   that exists, instead of each needing its own branch. The addition is moving screens.
2. **Moving screens: a screen that can move is never in the lens map. It is drawn live, every frame,
   in the projector's own output thread**, over the baked map of everything that stands still, and
   hidden where the room is in front of it. This costs one small draw per moving screen (a screen is
   a few hundred triangles), needs no 3D renderer on the output thread, and is in step with the video
   frame by construction. Re-baking the map each frame is the wrong design and section 4.2 says why
   with numbers.
3. **What moves a screen: a cue first, tracking later.** A new cue ("Move a screen") takes a screen
   to a place over a time, by the shared clock, so every computer agrees to the frame with no new
   network traffic. Tracking (PosiStageNet, OSC) comes after, feeding the same "where the screen is
   now" that the cue feeds.
4. **Where I'd push back.** A cue does not know where the scenery really is. It knows where you said
   it would be. If a stagehand or a winch is half a second late, the picture is half a second ahead of
   the wall. Cue-driven movement is exactly right for *a screen with two or three resting places*
   (which is most theatre) and only roughly right *while the audience watches it travel*. If the
   travelling picture is what you want, that is tracking, it needs hardware I can't test, and it will
   not be good on the first try. Don't promise a client a tracked moving wall on the strength of this
   plan.
5. **The first mapping release is static screens only**, with line-up from the primary and streaming
   working. The moving design is in the shared types from the first commit (the map already leaves
   out anything movable; the frame already has a list of moving screens, empty), so the second
   release adds movement without redesigning the first.
6. **Canvases**: a display is called a canvas when it has no monitor and at least one screen shows
   it. Worked out, never stored. A mapped projector's display says "Shows the stage's screens,
   through Projector 1" and takes no cues.
7. **Line-up over the network**: the primary does the sums and keeps the points. The computer that
   drives the projector only draws crosses where it is told, and stops drawing them by itself five
   seconds after the primary goes quiet. Nothing in progress is written into the show, so the show
   isn't resent at every nudge; *Done* is one ordinary show edit.
8. **Streaming a mapped projector's display** uses the same drawing code as its window. One thing
   follows that the old plan missed: streams are the primary's job, so the primary must be able to
   draw the mapped picture of a projector another computer drives.
9. **Five rounds of at most three agents.** Shippable: after round 2 (mapping, line-up, streaming,
   static screens), round 3 (screens that move on a cue), round 4 (projectors blend themselves;
   tracking in), round 5 (set pieces as screens, imports).
10. **None of it has met a projector.** Everything is proved against pure maths and a software
    graphics card. Before the first mapping release is called done, someone has to stand in front of
    a wall with a projector and a second computer. That is the biggest risk in this plan, bigger than
    any design choice in it.

---

## 2. What changed from the old plan, and why

| Old plan | Now | Why |
|---|---|---|
| 4.2: lens map chosen; moving screens "can be added beside the map later" | Lens map for what stands still **plus** a live pass for movable screens, designed together (4.1, 4.2) | The owner wants moving scenery within a year |
| 4.3: `where` as `Rg32Float`, `how` as `Rgba8Unorm`, 12 bytes a pixel | Both `Rgba8Unorm` (place packed as two 16-bit numbers), 8 bytes a pixel; a third, optional, for depth | No float format to worry about on any card; a third less memory; exact enough for a 16K canvas |
| 4.4: the map pass draws to the swapchain; `OutputJob::Frame` gains `map` | The mapped picture is drawn into a `LayerTexture` target; the window then composites as it always has | One window is one *monitor* with sub displays and cards on it (3.1); reuse beats a second path |
| 4.4: fine-tune drawn by the map pass through its own grid | Fine-tune is the existing mesh warp on the mapped layer | Same result, no new vertex code |
| 2.3: a mapped display ignores its sub displays | Sub displays stay, as flat patches over the mapped picture | It falls out of the layer design and is useful (a surtitle box on a projector) |
| 4.4: streaming a mapped display "refuse in words is fine" | It works, by the shared drawer (4.5) | Owner's decision |
| 4.6: "each computer bakes its own projectors' maps" | Each computer bakes the maps it *draws*: its own windows', plus any it streams or previews | Streams are the primary's alone (3.3) |
| 4.4: a canvas feeding a mapped projector **on this computer** is never `preview_only` | …a mapped projector this computer **draws for a window or a stream** | Same reason |
| 1.1, 4.4: "`delay_ms` is engine-side, nothing touches it" | A mapped projector's own delay is not used; the canvas's is (4.1) | The delay is found by the cue's display (`show.rs:3967`), and a mapped projector has no cues |
| 4.4: streams of a mapped display, no rule for `part_of` | A mapped display is streamed as its own picture even when `part_of` is left in the file (4.5) | `update_streams` skips `part_of` displays (`app.rs:2098`); a blended pair turned mapped keeps it |
| 5.2: the crosses are "an extra layer of the display's group" | Crosses are small sprites in a group of their own, in raw monitor pixels, drawn last, blend off | A layer of the group would go through the map, the fine-tune and the blend; line-up needs raw pixels |
| 5.5: network line-up "a later sub-step (P5b)" | In the first release (4.4) | Owner's decision |
| 5.4: `LinePoint { world, pixel, on, surface }` | Adds the place in the screen's own frame, which is the one kept true (section 5) | A point on a screen that can move |
| 7: P4 + P5 as one round | Two rounds for the first release, moving screens a third | Network line-up and streaming were added; more than three agents' worth |
| 3.3 open question 5: which screens take light, by reach | An explicit kind on the screen, worked out once at rest when left on automatic (section 8) | "By reach" flips as a screen moves through a beam |
| 2.1: "0.25 saves the show and the projectors are lost" | 0.27.2 keeps a projector's unknown fields (`extra`) and asks before saving a newer show | Built since (3.4) |

Kept as written: section 2 (the projector model), 3 (simulation, as built), 4.5's blend maths, 5.1 to
5.3 and 5.6 (the idea of line-up, its UI flow on one computer, the solver), 6 (content modes), 8 (UI
words), 9 (formats), 10 (risks, added to here).

---

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
  219-221). wgpu is 30.0.1 (`Cargo.toml:39`). The old plan's claims about which float formats can be
  sampled under those limits are **unverified** here; this plan avoids the question by using bytes (4.1).
- `LayerTexture::new_target` (`gpu.rs:1389`) makes a texture another pass can draw into and the
  compositor can then show as a layer. A display that shows the stage already works this way
  (`StageLive::output_layer`, `src/stage_live.rs:610`).
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
  `CaptureJob::Groups` (`render.rs:766`) and composites them with **no blend** (`render.rs:995`) into an
  `Rgba8Unorm` texture of the stream's size, reads it back and hands it to FFmpeg, at the stream's rate.
- `update_streams` (`app.rs:2090`) says a stream is "the display's own picture on its own canvas, without
  the shaping a projector needs": no keystone, mesh or blend. It skips sub displays and displays that are
  part of another's picture (`app.rs:2098`).
- **Streams are the primary's alone** (`sends_here`, `app.rs:2099`, `src/app/play.rs:568`). So when a
  follower drives a mapped projector and its display has a stream, it is the *primary* that must draw the
  mapped picture. The old plan's "each computer bakes its own projectors' maps" isn't enough.
- A display's delay (`OutputConfig::delay_ms`) is looked up **by the display a cue is sent to**
  (`Show::video_delay_ms`, `show.rs:3967`). The old plan's 1.1 and 4.4 said mapping doesn't touch it.
  It does: a mapped projector's cues go to the canvas, so the projector's own delay never applies
  (4.1).
- At most two streams at a time (`gpu::CAPTURE_SLOTS`, `gpu.rs:1182`).

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
- `World` in `net/play_tests.rs:114` is simulated engines with no graphics card or app; anything
  that draws can't be tested in it.
- No PosiStageNet code exists (`git grep -i posistage` finds nothing).

---

## 4. The design

### 4.1 Mapping

What "mapped" means is the old plan's 4.1, unchanged: a projector with `mapped: true` and a display.
Every pixel of that display shows the content of the spot on a screen that it lands on. Content
lives on the canvases that feed the screens.

#### The lens map (old 4.3, revised)

Made once per projector and size, off the output path, on the stage-render thread. Per pixel:

- `where`: `Rgba8Unorm`, read with `textureLoad`. The place in the feed's picture, x in the first two
  bytes and y in the last two (16 bits each: one part in 65,536, a quarter of a pixel on a canvas
  16,000 wide).
- `how`: `Rgba8Unorm`. r: the share of light (1 until computed blend, round 4). g: which feed (0 =
  none, black). b: coverage at a screen's edge. a: spare.
- `depth`: `Rgba8Unorm`, **only when the show has a movable screen** (4.2). The distance from the
  lens to the nearest thing that stands still (venue or screen), packed in three bytes over the
  scene's depth range.

8 bytes a pixel without depth: 18 MiB at 1920×1200, 66 MiB at 4K. 12 with depth.

Why not the old plan's `Rg32Float`: it is not filterable under the limits NewQ asks for
(`gpu.rs:234`), so it would be read with `textureLoad` anyway, and whether every card NewQ meets
accepts it as a texture I can't verify here. Bytes are accepted everywhere. Packed values must not
be averaged, so the bake is not multisampled: it is drawn at twice the size and a small resolve pass
picks one sample's place and counts the samples that agree for the coverage.

What is baked: the screens **that cannot move** (4.2), drawn from the lens with a fragment entry
that writes the place in the feed, and the venue as occluders. New code in `render3d/bake.rs` with
its own small shader, using `Renderer`'s mesh cache and the lens matrix the light already uses
(`light::throw_of`, straight from `Projector::frustum`). It does not touch `fs_part` or `fs_lit`,
and it is made inside an error scope like the light (`checked`), so a card that refuses it leaves a
notice and a black projector, never a broken stage view.

The key a map is made for: the scene of static things, the projector's placement and lens, the
feeds' regions, the size, whether depth is wanted. **Nothing a cue can change is in the key.** A GO
never causes a bake. `Projector::fine` and `Projector::line_up` are in `Stage3d`, which `shape_key`
hashes whole (`stage3d/mod.rs:739`); both the scene's key and the map's key must leave those two
out, or every drag of a fine-tune point would rebuild the scene and re-bake the map. (The fine-tune
is applied after the map and the points change nothing until *Done*.)

Who bakes what: each computer bakes the maps it draws. That is a map at the monitor's size for each
mapped projector whose window is here; one at the stream's size for each mapped display it streams
(the primary, 3.3); and one at the throw's size (960 wide) for each mapped projector in a 3D view
with its light on. The first two are **output work**: they go before every preview job and are never
governed. The third is preview work and is governed. A finished map belongs to the app
(`Arc<LensMap>` held by `StageLive`), not to the thread, so the thread's 30-second idle release
(`stage_live.rs:827`) doesn't lose it; a bake that is due counts as work, so the thread is started
for it.

Until a projector's first map lands its display is black, never the flat picture (old 4.3). A map
being replaced stays in use until the new one is ready. A lost graphics device forgets them all
(`StageLive::forget_device`).

The CPU reference (`stage3d/lensmap.rs`, pure): for a pixel, the ray through the lens
(`Projector::ray`), `render3d::pick` against the static scene, the hit's uv through the screen's
region; the same packing functions the shader's are tested against. Compared with the GPU bake pixel
for pixel away from edges, at 64×36, on lavapipe.

#### Drawing it (old 4.4, revised): one drawer, three users

New file `src/mapped.rs` with its own small shader. It is not the uber-shader and should not be.

```rust
pub struct MappedFrame {
    pub target: Arc<LayerTexture>,     // where the mapped picture is drawn (made by the main thread)
    pub map: Arc<LensMap>,             // everything that stands still
    pub feeds: Vec<FeedFrame>,         // at most 4: a canvas's layers on its plain picture, its size
    pub moving: Vec<MovingScreen>,     // 4.2; empty until round 3; far to near
    pub lens: [[f32; 4]; 4],           // the projector's matrix, for the moving screens
    pub shares: bool,                  // false for a stream (4.5)
}

impl Drawer {   // one per thread that draws mapped pictures
    pub fn draw(&mut self, gpu: &Gpu, owner: MapOwner, frame: &MappedFrame) -> Vec<wgpu::CommandBuffer>;
}
```

`draw` does:

1. **Canvas passes.** For each feed, `gpu.composite` of that canvas's layers on its plain picture
   (the geometry `stage_live::feed_groups` already makes) into a texture the drawer keeps. Each feed
   needs uniforms of its own, because one layer can be on two canvases: new `Slot::Map(owner, n)`
   slots, made on demand exactly as `Slot::View` are (`gpu.rs:1233`).
2. **Map pass** into `frame.target`: one triangle over the whole picture. For each pixel, read the
   map at that pixel, sample the feed's canvas at the place it gives (`textureSampleLevel`, no
   derivatives), multiply by coverage and, when `shares`, by `share^(1/gamma)`. When the target is
   the map's size, as it is for a window, that is exactly one map pixel per picture pixel.
3. **Live pass** (round 3) into the same target, over the map pass: section 4.2.

The three users, all calling the same `draw`:

- **A window** (`output_loop`, `render.rs:499`). `OutputJob::Frame` gains
  `mapped: Vec<MappedFrame>` (empty for every display that isn't mapped; one entry for a mapped
  one). The loop runs `draw` for each, then **its existing `gpu.composite`, unchanged**, then one
  `gpu.submit` of all the command buffers together, in order. The main thread puts `frame.target` in
  the window's groups as the mapped display's only layer, with the projector's fine-tune as that
  group's mesh. So with nothing mapped the loop is today's, branch for branch.
- **A stream or recording** (`capture_loop`, `render.rs:930`): section 4.5.
- **The light in a 3D view** (`ThrowJob`, `stage_render.rs:58`): a mapped projector throws its mapped
  picture, drawn by `draw` at the throw's size. This closes the loop the old plan's 3.7 wanted:
  content painted straight on a curved screen, against the same content mapped for a projector and
  thrown back by the simulation, compared on lavapipe.

What the layer design gives without new code:

| Wanted | How it comes |
|---|---|
| Fine-tune (`Projector::fine`, a mesh only) | The mapped layer's `Geometry::mesh`: the existing 48×48-cell mesh warp (`gpu.rs:473`) |
| Hand edge blend, until computed blend | The window's `BlendPass`, as now |
| Sub displays on a mapped projector | Their groups, drawn over the mapped layer, as now |
| Cards, Identify, the fps label, a test pattern | Layers and groups added after, as now (`put_card`, `app.rs:4627`) |
| Line-up crosses | A last group in raw pixels (4.4) |

The price is one more full-size pass per mapped display (the target, then the composite of it) and
one more full-size texture (9 MiB at 1920×1200). On any real card that is small next to decoding
the video. I think that is the right trade and I'd rather defend one extra pass than five special
cases.

**The new-picture count.** `Shot` (`render.rs:266`) must include, for each mapped frame, its feeds'
layers' stamps, its map's identity and its moving screens' matrices. The target layer's own stamp
never changes (it is drawn into, not uploaded), so without this Test Mode would count a mapped
display as never showing a new picture, or as showing one every frame.

**Canvases are output work.** The trap at `app.rs:2538`: a canvas has no window and no stream, so
today it is `preview_only` and its video drops to the previews' rate when the governor steps down.
The rule becomes: *a display that feeds a screen is never `preview_only` while this computer draws,
for a window or a stream, a mapped projector that uses it.* `StageLive` already keeps a set for
"feeds a display that shows the stage" (`needed_by_outputs`, `stage_live.rs:404`); this is a second
reason to be in it. Test: force the governor to Paused, play a video on a canvas, and check the
mapped window's picture still changes every frame.

**A slow projector's delay.** `delay_ms` is found by the display a cue is sent to
(`show.rs:3967`). A mapped projector has no cues, so its own delay does nothing; the **canvas's**
delay is the one that counts, and it holds back everything on that canvas for every projector that
shows it. That is the same as a blended pair today (their cues are on the wide picture). The mapped
projector's delay row is greyed: "Set the delay on the canvas (Cyc content)." Two projectors of one
screen that are *differently* slow can't be fixed this way; that would need frames held back in each
window's thread, and it is not planned.

**Latency: none added.** Content and mapping are drawn in the same thread, in the same frame, from
the same job. Two projectors in two windows are as much in step as two windows are today.

**Quality** (old 4.4's note stands): content is sampled without mip maps, so a canvas with far more
pixels than the projector resolves will shimmer. The pixel-size read-out says what size is right.

**Limits, said in words in the UI**: at most 4 canvases per projector; at most 8 windows on one
computer (`WINDOW_SLOTS`, `gpu.rs:1180`), as today.

#### How it sits with the hand tools

The old plan's table in 4.5 stands, with one change: the display's `corners` and `mesh` are still
not read while mapped, and the fine-tune is still `Projector::fine`, but it is now drawn by the
compositor's mesh code, not by the map pass.

### 4.2 Moving screens

#### What moves a screen: a cue first, then tracking; one place they both write to

The engine gets one new piece of state: **where each movable screen is now** (`Engine::screen_at(id)
-> Option<Placement>`, None meaning "at home", the placement in the show). It is run-time state, as
a playing cue's opacity is. It is **never written into `Show::stage3d`**: that would change
`shape_key` (`stage3d/mod.rs:739`), rebuild the scene and re-bake every map each frame, and on a show
network resend the show and hold every follower's commands meanwhile (3.5).

Two things write to it, in this order of building:

1. **A cue: "Move a screen"** (round 3). A new cue kind, `CueKind::Screen`, with the screen, where
   to (each of X, Y, Z, turn, tilt, roll optional; what isn't given is left), a time and a curve. It
   is the fade cue's idea (`GeometryTarget`, `show.rs:2609`) pointed at a screen, ramped by the
   engine's existing `Ramp` (`engine.rs:108`). A new kind, not a new field on the fade cue: an older
   NewQ reads a kind it doesn't know as `CueKind::Unknown`, keeps it and never plays it, which is the
   rule (CLAUDE.md, `show/compat.rs`); a new field on a fade cue would be ignored and the fade
   played as something else. The inspector has a *Use where it is now* button, so nobody types six
   numbers.
2. **Tracking** (round 4): PosiStageNet first, then OSC. Section "Later: tracking" below.

A screen is **movable** when some cue of that kind names it (or, later, a tracker is bound to it).
Worked out from the show (`Show::movable(surface)`), never a tick box, so nobody can forget it. It
changes only when the show is edited, which is the only time a re-bake is welcome.

Where the screen is must outlive the cue that moved it: the wall stays open after the eight seconds
are over. So:

- The primary's heartbeat (`PlayState`, `net/play.rs:226`) gains `screens` (each moved screen: where
  it is, and where it is heading and by when, on the shared clock), `#[serde(default)]` and left out
  when empty. A computer that joins late, or a backup that takes control, has it from the last
  heartbeat. A move itself needs no new message: it is a cue, anchored on the shared clock like any
  other, and each computer ramps it by that clock.
- **A follower that disagrees with the heartbeat** about a screen follows R21: the picture never
  jumps at the primary. A computer that has only just joined takes the heartbeat's places at once
  (nothing was right before). Otherwise it acts only on a difference seen at several heartbeats in a
  row, as `net_correct` does for cues (`engine/net.rs:462`), and then eases to the primary's place
  over half a second. I have read `PlayState` but not `net_correct`'s body; round 3's agent A reads
  it first and says if this doesn't fit.
- *Stop all* does not move scenery back. Real scenery doesn't jump when the sound stops.
- Opening a show puts every screen at home. After a restart mid-show the screens are at home until
  told otherwise, so the 3D stage's Screens tab shows, for each movable screen, **Now at** and a
  *Put at ▸* menu of the places its cues name (a jump, no ramp). That is also the rehearsal tool.
  It is an input from outside the engine, so on a primary it goes through the gate as a
  `play::Command` like any other (`engine/net.rs`), and followers do it at the same shared time.
- Movement is straight from where it is to the target, each number on its own. A revolve is a turn
  number that keeps counting (720 is twice round). Curved tracks are a row of cues that follow one
  another; a real path editor is not planned.

Not planned at all: a *projector* that moves (on a moving truss). Every map and every line-up
assumes projectors are bolted down.

#### How the picture stays frame-safe while a screen moves

Three ways were on the table.

**A. Re-bake the lens map every frame on the stage thread. Rejected.**

- Testable on lavapipe, but only its picture, not its timing, and the timing is what is wrong
  with it.
- Size: 1920×1200 is 2.3 million pixels, baked at twice the size and resolved: about 9 million
  fragments and 18 MiB written per projector per frame, plus the whole venue drawn as occluders (the
  loader allows 8 million triangles, `MAX_TRIANGLES`, `stage3d/load.rs:25`). Six projectors is six of
  those, sixty times a second, on the thread that also draws every 3D view.
- Lateness: the stage thread is "latest wins, when it gets to it". Its result reaches the output
  thread at least one frame late, and by a varying amount.
- The part that kills it: two projectors lighting one screen would get their new maps on different
  frames, so the picture **tears along the overlap** for as long as the screen moves. That is the
  old plan's own argument against the async route (its 1.4).

**B. A live 3D pass in the output thread for moving screens, beside the map. Chosen**, in a form
that keeps the 3D renderer off the output thread.

A screen is a small mesh: a flat screen is two triangles, a curved one a few hundred, and the mesh
doesn't change when the screen moves, only its matrix does. So the live pass is not "3D rendering"
in the sense the old plan feared. For each moving screen it is one draw call:

```
vertex:    position on the picture = lens matrix × the screen's matrix this frame × corner
fragment:  fade   = how near my uv is to the screen's edge, over one pixel   (derivatives: first)
           colour = the canvas at my uv
           hidden = the baked depth at this pixel is nearer than me          (the room is in front)
           out    = hidden ? nothing : colour × fade
```

- The mesh is uploaded once (when the show changes), the matrix comes with the frame
  (`MovingScreen { mesh, matrix, feed, region }` in `MappedFrame`), computed on the main thread from
  `Engine::screen_at` at the same tick the video frames are taken from. **Position and content are
  in the same job**, so they can't disagree, and two projectors can't disagree with each other.
- The venue is never drawn on the output thread. What it hides is already in the map's `depth`
  picture, baked once. A moving screen in front of a static one covers it, because the live pass is
  drawn after the map pass and is nearer. Where the screen has moved away from, the map shows
  whatever stands behind. Two moving screens are sorted far to near on the main thread (no depth
  buffer on the output; two set pieces passing *through* each other would be wrong, and can't
  happen on a real stage).
- Edges: no multisampling. The fragment fades over the last pixel from its uv's distance to the
  screen's edge. That needs derivatives, so it is worked out first, before anything that depends on
  the depth; nothing is discarded before it.
- Lavapipe can test all of it: the live pass's picture against the CPU reference
  (`stage3d/lensmap.rs` with the screen at that placement) at any position, and two projectors'
  frames from one tick against each other.
- Cost on the output path: per moving screen one uniform write and one draw of a few hundred
  triangles; per pixel one extra `textureLoad`. No new texture per frame, no wait on any other
  thread, nothing whose cost grows with the venue. A screen that is movable but standing still costs
  the same as one that is moving: there is no "start of a move" on the card at all, so nothing can
  hitch at a GO.
- Pipelines: `mapped.rs` owns them. `render3d::Renderer` is not used (it is one per `Gpu` and
  submits by itself, 3.4).

**C. Draw every screen live and have no lens map. Considered, rejected.** It is tempting, since B
shows a screen is cheap to draw, and it would be as testable on lavapipe as B. But the map is still the right home for everything that is costly
or comes from elsewhere: what the venue hides, the computed blend's shares (which need every other
projector and a shadow test per pixel), set pieces with big meshes as screens (old 6.3), and maps
made by other people's camera calibration (old 9.5, MPCDI). With the map, the static cost on the
output path is one lookup whatever the room is. Keep both.

**Computed blend on a moving screen** (round 4): the share at a spot depends on where the spot is,
which changes. For moving screens it is worked out in the live pass's fragment from the other
projectors' matrices (a short loop, at most 8), by the same distance-to-edge rule as the baked
shares (old 4.5), without the shadow test. Before round 4, moving screens take the hand blend like
everything else.

**The 3D views and the light** follow the same positions, as preview work. `Stage3d::scene` is not
made again per frame: the views get the moved screens' matrices beside the scene (a small list
applied to those parts' transforms when the job is built), so `scene_key` is untouched. A lit view's
shadow maps are redrawn while a screen moves (`shadow_key` includes transforms, `render3d/lit.rs`);
that is governed preview work and never reaches an output.

#### How late may the picture be?

A screen moving at 0.5 m/s (brisk for scenery) covers 8 mm in one 60 Hz frame. A projector pixel on
a typical screen is about 3 mm. So **one frame late is two or three pixels of slip**, seen as a soft
edge while it moves and gone when it stops. More than two frames is a visible lag.

- **Cue-driven: zero frames late, by design.** There is no measurement to be late with. The error
  is between the cue and the real scenery, and no software removes that.
- **Tracked: NewQ should add at most one frame** on a computer that hears the tracker itself (the
  position is read at the tick the frame is made from), two on one that has it relayed. That is the
  target for round 4. The tracker's own delay (often 20 to 60 ms) and the projector's (one to three
  frames) are on top and are usually bigger than NewQ's; the cure for those is a "look ahead by N
  ms" setting per tracker that runs the last known speed forward. Without it a tracked wall at
  0.5 m/s will trail by several centimetres. This cannot be tuned without the real hardware.

#### Later: tracking (round 4, outline only)

- **PosiStageNet first.** From memory, to be confirmed against its specification before anything
  is built: it is what tracking and automation systems send to media servers, and it is UDP
  multicast. Its parser is pure (testable here against made-up packets; a capture from a
  real system is needed before it is trusted). If it is multicast as I remember, **every computer can
  hear it directly**, so followers don't wait for the primary. That is the reason to do it before OSC.
- **OSC second** (`/newq/screen/<name>/position x y z` and `/rotation`), through `remote.rs`'s parser.
  Remote input is the primary's alone (R8, 3.5), so the primary relays positions to the others in an
  urgent, latest-wins message, read as `Unknown` by an older NewQ. R21 holds: a follower uses the
  newest position it has and never waits for one.
- A tracker's numbers are in the tracker's own room coordinates. Matching them to the stage's is a
  small fit from a few points, which is the solver's core (section 5).
- A screen follows a cue *or* a tracker, chosen on the screen; if both arrive, the tracker wins while
  its data is fresh and the cue's place is used when it goes quiet for more than a second.

#### What ships when

- **First mapping release (rounds 1 and 2): static screens.** `MappedFrame::moving` exists and is
  empty; the bake already leaves out anything `Show::movable` names (nothing yet); line-up points
  are stored in the screen's own frame. Nothing is built that movement will have to undo.
- **Second (round 3): screens that move on a cue**, in the projectors, the 3D views and the light.
- **Third (round 4): tracking**, and computed blend.

I'd argue against putting movement in the first release. Static mapping changes the output path and
has never met a projector; it should be tried on a wall before anything is stacked on it.

#### What can't be tested here (moving screens)

Whether a real winch or wagon is where a cue says it is; any tracker or automation feed; total lag
with a real projector; the look-ahead setting; how a moving edge looks without multisampling on a
real card. What *can*: the live pass against the CPU reference at any placement (lavapipe), two
projectors agreeing on a moving screen in the same frame, the engine's ramp and the late joiner on a
test-owned clock (`World` in `net/play_tests.rs`), and that a GO never causes a bake.

### 4.3 Canvases

**What a canvas is.** A display is a canvas when all of these hold (`Show::is_canvas(id)`, worked
out, never stored):

- it is a whole display: not a sub display, not a part of another display's picture, not a display
  that shows the stage;
- it has no monitor (`OutputConfig::monitor` is None, `show.rs:749`);
- at least one screen shows it (`Feed::Display { output, .. }`).

Why not stored: a stored flag can disagree with the facts (a "canvas" with a monitor plugged in; a
screen pointed at something that isn't one). Why "no monitor" is part of it: an LED wall is a
display with a monitor that a screen also shows, and calling the thing your LED processor is plugged
into a "canvas" would be wrong. That display keeps the word *display* and gains a line: "Also on the
stage: LED wall". (This is the one place I've narrowed your wording; it is question 1 in section 9.)

The edge cases:

| Case | Called | Notes |
|---|---|---|
| No monitor, feeds a screen | **canvas** | the usual mapped set-up |
| No monitor, feeds nothing | display ("shown only in the previews"), as today | becomes a canvas the moment a screen shows it |
| Has a monitor, feeds a screen | display, "Also on the stage: …" | an LED wall; a monitor in the wings |
| A mapped projector's own display | display, "Shows the stage's screens, through Projector 1" | takes no cues; can't feed a screen (`feed_choices` leaves it out; `sanitize_stage` clears such a feed) |
| A part of another's picture, a sub display | never a canvas | the canvas is the whole picture (`Show::whole_picture`) |
| A display that shows the stage | never a canvas | already can't feed a screen (`stage_edit.rs:328`) |

**What makes a display "a mapped projector's display"**: also worked out
(`Show::mapped_projector(display) -> Option<&Projector>`): a projector with `mapped` set names it.
`Projector::mapped` *is* stored: it is the user's choice, and switching it off must give back the
hand-made keystone exactly.

**Where the word appears.** One function makes a display's name for every menu and list
(`ui::display_label(show, id)`: "Cyc content (canvas)", "Projector 1 (shows the stage's screens)",
or the plain name), and every place that lists displays calls it, so a new menu can't forget. The
places today:

- **Settings ▸ Displays** (`ui.rs`, the page around `ui.rs:5838`):
  - a canvas's row has the tag *Canvas* beside its name and a line "Shown on: Cyc, Portal left"
    (each a button to that screen);
  - its monitor menu's first entry reads "No monitor (a canvas for the stage's screens)" when a
    screen shows it, and "No monitor (previews only)" when none does;
  - *+ Add* (`ui.rs:5844`) gains **Canvas**: a display with no monitor and a picture size, which then
    asks which screen shows it. So nobody has to learn that a canvas is "a display you didn't plug
    in";
  - a mapped projector's display: its *Shows* row (`SHOWS_ROW`, `ui.rs:6092`) reads "The stage's
    screens, through Projector 1", greyed, with a button to the projector. Its corners, mesh and
    part-of controls are greyed with one line: "Mapped: the 3D stage decides this. Fine-tune it on
    the projector." If cues are still sent to it: "3 cues are sent to this display. They aren't seen
    while it is mapped."
- **The cue inspector's Display menu** (`ui.rs:8959`): canvases are listed as "Cyc content (canvas)".
  Mapped projectors' displays are listed last, greyed: "Projector 1 (shows the stage's screens)".
  The row's label stays *Display*.
- **The Preview pane's tile and display pickers** (`ui.rs:4770`) and the multiview's: the same
  labels, from the same function.
- **Patch view** (`ui/modern.rs:664`): a canvas's node has the subtitle "canvas → Cyc"; a mapped
  projector's node "shows the stage's screens" and no port to drop a cue on.
- **Settings ▸ 3D stage**: the Screens tab's *Shows* menu ends with "New canvas…"; the Projectors
  tab's *Mapped* tick keeps the old plan's line ("This display shows the screens as this projector
  sees them, instead of its own picture."); the Displays tab lists canvases and what they feed.
- **README**: the 3D stage and Displays sections, and one paragraph "Canvases". **CLAUDE.md's words
  rule** gains: *canvas* (a display with no monitor that a screen shows; worked out, never a kind
  that is stored). Every pointer is made by `settings::path`, as the rule says.
- The word is free: `git grep -i canvas -- src/ui.rs src/ui` finds no label a user sees today, only
  debug text. In the code `canvas` already means "a display's picture and its size"
  (`Geometry::canvas`, `gpu.rs:1683`), which agrees.

### 4.4 Line-up across the network

The idea and the two-pane UI are the old plan's 5.1 and 5.2. What follows is what changes when the
projector is another computer's, and it is built so that **one computer alone runs the same code**
(the "other computer" is then itself), so the common case is the tested case.

#### Who does what

| | The primary (where the operator sits) | The computer that drives the projector |
|---|---|---|
| Holds the points while lining up | yes, as UI state, never in the show | no |
| Solves | yes, at every nudge (pure maths, well under a millisecond) | never |
| Draws crosses on the real projector | sends where | draws them where told |
| Stores the result | *Done* = one edit of the show: placement, lens, the points, "before" | gets it by ordinary show sync |

Only one computer ever solves, so the solver needs no agreement between machines.

#### The messages

One new `Msg` variant, `Msg::LineUp(Box<LineUpMsg>)`, in a new file `src/net/lineup.rs`:

```rust
#[serde(tag = "k", rename_all = "snake_case")]
pub enum LineUpMsg {
    /// Primary -> the display's computer: show exactly this. The whole state, every time.
    Show(Box<Overlay>),
    /// Primary -> that computer: line-up is over.
    End { session: u64 },
    /// Back: this is shown, on a monitor of this size.
    Seen { session: u64, seq: u64, pixels: (u32, u32) },
    #[serde(other)]
    Unknown,
}

pub struct Overlay {
    pub session: u64,            // a random number per line-up, so a stale message is passed by
    pub seq: u64,                // counts up; an older one than the last is dropped
    pub display: OutputId,
    pub pixels: (u32, u32),      // the size the points are in
    pub marks: Vec<Mark>,        // { n, at: [f32; 2], on } in raw monitor pixels
    pub active: Option<u8>,
    pub outline: Option<Vec<Vec<[f32; 2]>>>,  // the model as the present solve throws it; None: keep the last
    pub under: Under,            // Black | Dimmed (the picture under the crosses); lenient
}
```

- **Whole state, latest wins.** No deltas, so a lost or late message costs nothing and a reconnect
  is just "send it again". A nudge sends a `Show` at once with `Hub::tell(Some(computer), .., true)`
  (urgent: written ahead of other frames, `hub.rs:398`), about a kilobyte. With nothing changing the
  primary repeats it once a second.
- **An older NewQ** reads `Msg::LineUp` as `Msg::Unknown` and a later kind inside it as
  `LineUpMsg::Unknown`; neither closes the session (the rule in CLAUDE.md). The primary then gets no
  `Seen`, and says so in words: "Stage-left runs NewQ 0.27.2, which can't show line-up crosses.
  Update it, or line this projector up on that computer."
- **`Seen`** tells the primary the crosses are up, and the monitor's real size. If that isn't the
  size the points are in, the UI says so before the operator wastes ten minutes.

#### Trust

Nothing new is trusted.

- The hub only hands the app a message from a **heeded** session (`hub.rs:1585`, `heeded` at
  `hub.rs:2593`), and `tell` only writes to heeded sessions (`hub.rs:2123`). With a passcode that
  means a proved one.
- In `play_message` (`app/play.rs:311`) the `Show` and `End` arms are guarded by `if primary`, as
  `Play` and `ShowPart` are: only the primary this computer follows can put crosses on its
  projector. The follower also checks the display is one of its own (`is_on`, `show.rs:826`).
- `Seen` is taken by a primary only from the computer that drives that display, for the session it
  started; anything else is passed by.
- What a wrongdoer on a network *without* a passcode could do with this is blank a projector for
  five seconds at a time. They can already send it a different show. No new hole; say so in the
  area note.

#### On the projector's computer

A small pure state machine (`lineup::Shown`): `take(msg, now)`, `tick(now)`, `overlay()`. It holds
the last `Overlay` and when it came.

- **The lease.** If no `Show` for the session has come for 5 s, the overlay is dropped and the
  display goes back to its picture. Measured as `shown_at.elapsed() > LEASE`, never by subtracting
  from an `Instant` (the hard rule). So a primary that crashes, a pulled cable or a takeover can
  never leave crosses on a projector in front of an audience.
- A change of primary, of role, or leaving the network drops it at once.
- **Drawing.** While an overlay is held, that monitor's window is sent, in place of its usual list:
  black (or the usual list dimmed, by `under`), then one last group in **raw monitor pixels** with
  the window's edge blend **off**. Crosses and rings are small sprite layers (one little texture per
  number, made once) placed by their `LayerTransform`, so **a nudge changes a few numbers and
  uploads nothing**. The old plan's whole-screen overlay texture would be 9 MB re-uploaded per nudge
  at 1920×1200, 33 MB at 4K, thirty times a second with a held arrow key. The model's outline is a
  whole-screen texture, redrawn only when a new `outline` comes (the primary sends one at most five
  times a second). Both are drawn by a pure file, `stage3d/lineup_overlay.rs`.
- It goes through none of the map, the fine-tune, the keystone or the blend. Line-up finds where the
  *lens* is; the fine-tune is what's left over afterwards.

#### On the primary

- Line-up is refused in Show mode, in words. Outside it, starting line-up on a projector that is
  showing something asks first ("This blanks Projector 2 while you line it up").
- The points in progress are UI state. They are also written to `lineup-draft.json` in the node
  folder at each change, so a restart offers "Carry on lining up Projector 2?". Small, and it saves
  a get-in's worth of clicking.
- *Done* writes placement, lens and `LineUp` (old 5.4) as one undo step, sends `End`, and the show
  sync does the rest: the other computer gets the show, its map's key changes, it bakes, and the
  mapped picture moves to the new solve. `Projector::line_up` is left out of the scene's key and the
  map's (4.1), so storing points alone rebuilds and re-bakes nothing.
- *Cancel* sends `End` and changes nothing.

#### If the link drops in the middle

- The projector's computer: the lease runs out, crosses gone, picture back. (If it lost the primary
  altogether, its usual "not in step" behaviour applies; nothing new.)
- The primary: `NetView` says the computer isn't connected; the line-up panes show "Lost the link to
  Stage-left. The crosses there aren't following. Your points are kept." Nothing is lost: when the
  session is back the next `Show` puts everything up again.
- A backup takes control mid line-up: the new primary has no draft, the old one's lease runs out on
  the projector. The operator starts line-up again on the new primary; the stored points from the
  last *Done* are there.

#### Testing it on loopback

- `net/lineup.rs`'s `Shown` and the primary's sender are pure and take `now` as an argument: every
  lease and repeat test runs on a clock the test owns (timing-test rule 4).
- `net/hub_tests.rs`, two hubs on loopback with a passcode: the primary `tell`s a `Show`; the
  follower's `take_events` gives it; the follower's `Seen` reaches the primary.
- The same file's `Stranger` (a raw socket that says what it likes, `hub_tests.rs:1770`): a
  stranger that claims the primary's id and sends `LineUp` to the follower is never forwarded; one
  that sends `Seen` to the primary likewise.
- `Hub::broadcast_json` (`hub.rs:390`) sends `{"t":"line_up","k":"something_new"}` and
  `{"t":"something_newer"}`: both are read, neither closes the session.
- Dropping the link: close the primary's hub mid line-up, advance the follower's test clock past the
  lease, the overlay is gone; start the hub again, send, it is back.
- With the app: a mouse-driven UI test places four points on a sample venue and nudges with the
  arrow keys, against a made-up projector whose true pose the test knows, with the "other computer"
  being a `Shown` in the same process; the solve lands within the stated pixels and *Done* writes
  one undo step.

### 4.5 Streaming and recording a mapped projector's display

**What the stream shows.** Today a stream is a display's picture "without the shaping a projector
needs": no keystone, no mesh, no edge blend (`app.rs:2087`, `render.rs:995`). For a mapped display
the same rule gives: **the mapped picture, without the fine-tune, without the hand blend and without
computed shares** (`MappedFrame::shares = false`). That is "what this projector is asked to show",
which is what you want to record or to watch from the production desk. It is not what you'd want if
the stream *is* the cable to the projector; nor is today's stream for an unmapped one. Question 2 in
section 9.

**The path.** `CaptureJob::Groups(groups)` (`render.rs:766`) becomes
`CaptureJob::Picture { groups, mapped: Vec<MappedFrame> }`. `capture_loop` keeps a `Drawer` and,
each time it draws, runs `Drawer::draw` for each mapped frame and then its existing `gpu.composite`,
submitting them together with the copy to the read-back buffer (`render.rs:1016`). It is the same
few lines as in `output_loop`, and both call one helper in `mapped.rs`, so there is one piece of
code to get right.

- **A mapped display is streamed as its own picture whatever `part_of` says.** `update_streams`
  skips a display that is part of another's picture (`app.rs:2098`), and a blended pair turned
  mapped keeps `part_of` in the file (it is what switching *Mapped* off goes back to). The filter
  becomes "not a part of another's picture, unless mapped".
- The main thread makes the stream's own target (at the stream's size) and puts it in `groups` as
  the display's one layer, stretched; `update_streams` (`app.rs:2090`) builds the `MappedFrame` from
  the same parts `update_outputs` uses for the window.
- **The map for a stream is baked at the stream's size**, on the computer that streams. That is the
  primary (3.3), whoever drives the projector. Its size comes from the projector's own pixels
  (`stage_projectors::shape_of`), since the primary may not have that monitor.
- The canvases the stream's map uses are output work on the streaming computer (4.1's rule already
  says "for a window or a stream").
- A stream's slot is `Slot::Capture(n)` (`gpu.rs:1162`); its canvases use `Slot::Map` with the
  capture's owner number, so a window and a stream of the same projector never share uniforms.
- The stream draws at its own rate from what it was last given, as now. A moving screen in a stream
  is as fresh as the last job the main loop sent, which is every pass while something plays.

**Tests.** A made-up map (a flip) through `capture_loop` into a real FFmpeg writing a short file,
read back with the test media's tools and checked the right way round (real time: this is FFmpeg);
the same frame through `output_loop`'s helper into a texture, compared with the stream's picture;
a pure test of "which maps does this computer make" (`mapped::wanted(show, me, streams)`): a primary
with a stream on a follower's mapped display wants that map at the stream's size, the follower wants
it at the monitor's, a computer with neither wants none; and a GPU test that a map for a display
that isn't on this computer draws through `capture_loop`. (`World` in `net/play_tests.rs` has no
graphics card, so it can't hold this test.)

---

## 5. For the solver agent (`src/calib/`)

You are building old-plan 5.3 as written. Nothing in this revision changes the maths. These are the
things the rest of the plan needs from you; none should cost more than an hour.

1. **Only the primary ever solves.** Followers draw crosses and receive the result in the show. So
   bit-for-bit agreement between machines or systems is **not** needed. Don't spend time on it.
2. **No dependence on a display, a window, the scene or the GPU.** The picture's size in pixels is an
   argument. In a network the primary lines up a projector whose monitor it doesn't have.
3. **It is called at every nudge, on the main thread.** With nine unknowns and a few dozen points it
   must return in well under a millisecond and must never loop without a limit. Cap the
   Levenberg-Marquardt steps and say in the report if the cap was hit.
4. **Export the forward function**: `project(placement, lens, pixels, world) -> Option<[f32; 2]>`
   (None behind the lens). The overlay's outline and each point's error are drawn with it, and it
   must be the same function the solver minimises, so they can't disagree.
5. **In and out are the show's types** (`stage3d::Placement`, `projector::Lens`), f32 in the file,
   f64 inside. Test that a solved result, stored and read back, re-projects the points within a
   tenth of a pixel of the f64 answer. The clamping getters on `Lens` (throw 0.1 to 20, shift -3 to
   3) are the bounds; a solve that wants to leave them is a refusal in words, not a clamp.
6. **Points on screens are stored in the screen's own frame.** The old 5.4 had
   `LinePoint { world, pixel, on, surface }`. With movable screens `world` goes stale. So:
   `LinePoint { at: [f32; 3], surface: Option<SurfaceId>, pixel, on }` where `at` is in the screen's
   own frame when `surface` is set, and in the room's otherwise (a venue corner). The **caller**
   turns points into room positions before calling you; the solver itself sees only room positions
   and pixels. Line-up is done with screens at home, and the UI refuses a point on a screen that
   isn't.
7. **Keep the rigid fit separate and public.** "Find the rotation and position that take these
   points onto those" is needed twice more: lining up a *screen* instead of the projector (old 5.6,
   item 3), and matching a tracker's coordinates to the stage's (4.2). A
   `fit_rigid(from: &[[f64; 3]], to: &[[f64; 3]]) -> Option<(rotation, translation, rms)>` beside the
   projector solve is enough; nothing uses it until round 4.
8. **`LineUp`, `LinePoint`, `Solved`** are show types and live in `stage3d/projector.rs`, owned by
   round 1's agent C, not in `calib`. `calib` takes slices and returns plain structs. If you have
   already put serde types in `calib`, leave them; agent C will use or move them.
9. **Rear projection and mirrored pictures** (a projector behind its screen, a mirror rig) must
   solve; the old plan's test list has the first. Add a mirrored case or refuse it in words.

---

## 6. Steps for the agents

Rules for every round, as CLAUDE.md has them: at most three agents; each reads the area files named
in its brief; new work in new files; **one owner per collision file per round**; a first "contract"
commit of shared types and nothing else; each agent commits as it goes on its own branch and runs
`scripts/check.sh quick <its areas>`, never the full suite; the integrator alone runs
`scripts/check.sh full`, once. Sizes: S about a day of an agent, M two to four, L about a week.
They are guesses.

Collision files, the ones to watch: `src/app.rs` (`update_outputs`, `update_streams`),
`src/render.rs` (`output_loop` and `capture_loop` are both in it: always one owner), `src/gpu.rs`,
`src/stage_live.rs`, `src/stage_render.rs`, `src/render3d/mod.rs`, `src/stage3d/mod.rs`,
`src/stage3d/projector.rs`, `src/show.rs`, `src/engine.rs`, `src/net/proto.rs`, `src/app/play.rs`,
`src/ui.rs`, `src/ui/stage_edit.rs`, `src/ui/stage_projectors.rs`, `src/ui/modern.rs`. `README.md`,
`CHANGELOG.md` and the area notes are merged by the integrator from each agent's lines.

The solver (old P3, `src/calib/`) is being built now and touches nothing below. It is needed from
round 2.

### Round 1: the map and the drawer

Every agent reads `areas/stage3d.md` and `areas/compositor-and-displays.md`; C also
`areas/previews-and-ui.md` and `areas/show-and-engine.md`.

**Contract commit** (agent B, before the others start):
- `src/mapped.rs`: `LensMap` (the textures, size, feeds, key), `MappedFrame`, `FeedFrame`,
  `MovingScreen`, `MapOwner`, `Drawer::draw`'s signature; the packing functions (`pack_place`,
  `unpack_place`, `pack_depth`, `unpack_depth`), pure, with their tests.
- `gpu::Slot::Map(u8, u8)`.
- `Projector::{mapped, fine, line_up}` and `LineUp`, `LinePoint`, `Solved` (section 5, point 6), all
  `#[serde(default)]` and unwritten at their defaults; `Show::is_canvas`, `Show::mapped_projector`,
  `Show::movable` (false for now); `extra` maps on `Surface` and `Stage3d` (3.4), so later fields
  survive an older save.
- `StageLive::lens_map(projector, size, use) -> Option<Arc<LensMap>>` and `MapUse { Window, Stream,
  Preview }`, returning None for now; `StageLive::feeds_output(id) -> bool` (a canvas a mapped
  projector drawn here uses: A fills it in `stage_live.rs`, B calls it in the `preview_only` rule);
  `mapped::wanted(show, me, streams) -> Vec<MapWanted>` (pure: which maps this computer makes).
- `stage3d/lensmap.rs`: `reference(scene, projector, size) -> CpuMap`, signature only.

**Agent A, the bake** (L). Owns `stage3d/lensmap.rs` (new), `render3d/bake.rs` and its shader (new),
`render3d/mod.rs`, `stage_render.rs`, `stage_live.rs`.
- Delivers: the CPU reference; the GPU bake (twice the size, resolved, in an error scope); `MapJob`
  on the stage thread, output work before previews, preview maps governed; keys (nothing a cue can
  change); the thread kept while a bake is due; maps forgotten with a lost device; black until the
  first map; a follower without its venue bakes without occluders and again when the venue lands
  (old 4.6).
- Tests: packing round trips (pure); GPU bake against the reference pixel for pixel away from edges,
  flat, curved and faceted screens, a box in the way, front and rear (lavapipe); a still scene bakes
  once; a moved projector re-bakes one map; `stage_live_tests`: a bake wanted starts the thread and
  the map outlives the thread's idle release.

**Agent B, the drawer and the output path** (L). Owns `mapped.rs` and its shader (new), `render.rs`,
`gpu.rs`, `app.rs`.
- Delivers: `Drawer` (canvas passes, map pass); `OutputJob::Frame::mapped`; `Shot` with the mapped
  frame; `CaptureJob::Picture` and the stream path (4.5); `update_outputs` building the frames, the
  mapped display's layer and fine-tune geometry; the `preview_only` rule; `update_streams`.
- Works against made-up maps until A's bake lands: identity, a flip, a known warp, two feeds.
- Tests: the map pass against the made-up maps (lavapipe, pixel for pixel); the fine-tune by the
  existing mesh test's method; a display that isn't mapped draws exactly what it drew before (a
  picture compared before and after the change); the governor forced to Paused while a canvas plays:
  the mapped window's `Shot` still changes each frame; the stream of a mapped display through a real
  FFmpeg (4.5); two windows drawing one canvas don't share uniforms (the `Slot::Map` test, in the
  style of the `part_of` one).

**Agent C, the model, the rules and the words** (M). Owns `stage3d/projector.rs`, `stage3d/mod.rs`,
`show.rs`, `ui.rs`, `ui/modern.rs`, `ui/stage_projectors.rs`, `ui/stage_edit.rs`.
- Delivers: the rules (a mapped projector's display takes no feed and no stage source; at most 4
  canvases; `sanitize_stage`); canvases everywhere section 4.3 lists; *+ Add ▸ Canvas* and "New
  canvas…"; the *Mapped* tick with its lines (and the small-theatre line, old 8.4); the screen's kind
  (section 8); CLAUDE.md's words rule and the README paragraphs.
- Tests: an old show loads and nothing new is written at its defaults; a later NewQ's fields on a
  screen survive a save; mouse-driven: add a canvas, point a screen at it, tick Mapped, read the
  Displays row, the cue inspector's menu and the Patch node.

**After round 1**: it can be merged and is safe (nothing changes unless *Mapped* is ticked). A user
who types a projector's position gets a mapped picture, with fine-tune, and can stream it. I would
not announce it: without line-up it is a demonstration.

### Round 2: line-up, and the proof

Every agent reads `areas/stage3d.md`; B also `areas/network.md`; C also `areas/testmode.md`.

**Contract commit** (agent B): `net/lineup.rs` types (4.4) and `Msg::LineUp`; `lineup::Shown` and
`lineup::Sender` signatures; `stage3d/lineup_overlay.rs` signatures (sprites, outline raster);
`UiState::line_up` (the draft); `stage_live::ThrowPicture::mapped: Option<MappedFrame>` (B fills it
in `app.rs::throw_picture`, C draws it in `stage_render.rs`).

**Agent A, line-up on the page** (M to L). Owns `ui/stage_lineup.rs` (new),
`stage3d/lineup_overlay.rs` (new), `ui/stage_projectors.rs`, `ui/stage_edit.rs`.
- Delivers: old 5.2's two panes; snapping to corners; arrow keys (1, 10, a tenth of a pixel); the
  solve at every nudge through `calib`; the words for the fit and the rung; *Done*, *Cancel*, *Back
  to before line-up*; the draft file; refusal in Show mode. A README keyboard-table row if a key is
  added (`ui::keys::KEYS` must match).
- Tests: mouse-driven against a made-up projector whose pose the test knows (four points on one flat
  screen; eight on a faceted one; one bad point is named); *Done* is one undo step; pure tests of
  the overlay's drawing.

**Agent B, line-up over the network** (M). Owns `net/lineup.rs` (new), `net/proto.rs`,
`app/play.rs`, `app.rs`, `net/hub_tests.rs`.
- Delivers: the messages, the lease, the sender's repeat; the `play_message` arms; the overlay's
  groups in `update_outputs` (raw pixels, last, blend off, sprites); the words for a lost link and
  for an older NewQ; one path for alone and networked; and, as `app.rs`'s owner, the mapped frame in
  `throw_picture` for C.
- Tests: section 4.4's loopback list, all of it.

**Agent C, the proof** (M to L). Owns `stage_render.rs`, `stage_live.rs`, `src/testmode/` (a new
`testmode/mapped.rs` and its hooks), `ui.rs`.
- Delivers: a mapped projector throws its mapped picture in the light (`ThrowJob` with a
  `MappedFrame`); the round-trip test; the fine-tune's editor (the existing mesh editor pointed at
  `Projector::fine`); **a Test Mode leg with a mapped display**, beside the 3D stage part
  (`testmode/stage.rs`): the Short lap on a canvas, a mapped projector on the last ticked monitor,
  judged against the baseline leg by the same line as the other 3D legs; a headless run of the app
  (xvfb, a sample venue, two mapped projectors on one curved screen) with screenshots looked at.
- Tests: the round trip on lavapipe (content laid straight on a curved screen, against the same
  content mapped and thrown back, within a tolerance away from edges); the Test Mode leg's judge
  (pure); the soak run itself, with the mapped display, as `soak.rs` does others.

**After round 2: the first mapping release.** "Line up a projector on a curved screen or a set in
four clicks, from the control desk, and stream what it shows." Static screens; hand edge blend.

### Round 3: screens that move on a cue

Every agent reads `areas/stage3d.md`; A also `areas/show-and-engine.md` and `areas/network.md`.

**Contract commit** (agent A): `CueKind::Screen(ScreenCue)` and `compat::KNOWN_KINDS`;
`Engine::screen_at`; `PlayState::screens`; `Show::movable` made real; `LensMap::depth`;
`MovingScreen`'s final fields; `StageLive::send` gains `screens: &[(SurfaceId, Placement)]` (B passes
them from the engine in `update_outputs`, C uses them in `stage_live.rs`).

**Agent A, the cue and the engine** (L). Owns `show.rs`, `show/compat.rs`, `engine.rs` (new work in
`engine/screens.rs`), `engine/net.rs`, `net/play.rs`, `ui.rs` (the inspector hook; the cue's page in
a new `ui/screen_cue.rs`).
- Delivers: the cue, its ramp by `Ramp`, on a timeline too; where the screens are, kept after the
  cue; the heartbeat; *Put at*; an older NewQ keeps the cue and doesn't play it.
- Tests, all on test-owned clocks: the ramp by `Engine::tick_shared`; three engines in `World`
  (`net/play_tests.rs`) agree on a screen's place at every tick; a late joiner and a backup taking
  control have it; *Stop all* leaves it; an older reader keeps it (`show/compat` tests).

**Agent B, the live pass** (M). Owns `mapped.rs`, `render.rs`, `app.rs`.
- Delivers: the live pass (4.2); the moving list from the engine each pass, sorted; `Shot`; the
  stream.
- Tests (lavapipe): a moved flat, curved and faceted screen against the CPU reference at that
  placement; hidden behind a baked box; in front of a static screen; two frames for two projectors
  built from one tick put the screen in the same place on both; a made-up job with 16 moving screens
  draws.

**Agent C, the bake, the views and the page** (M to L). Owns `stage3d/lensmap.rs`, `render3d/bake.rs`,
`render3d/mod.rs`, `stage_live.rs`, `stage_render.rs`, `stage3d/mod.rs`, `ui/stage_edit.rs`.
- Delivers: movable screens left out of the map and `depth` baked when there are any; the 3D views
  and the light following the screens without a new scene; *Now at* and *Put at* on the Screens tab;
  a Test Mode leg variant with a screen moving.
- Tests: a GO of a screen cue bakes nothing (a bake counter added to `stage_counts.rs` doesn't move);
  the reference with a movable screen; `stage_live_tests`: a moving screen redraws a view and makes
  no new scene.

**After round 3**: "Screens that move on a cue: the picture goes with them."

### Round 4: projectors blend themselves; tracking in

**An outline, not a brief.** Rounds 4 and 5 should be written out properly (contract commit, tests,
sizes checked) only after round 2 has met a real wall: what that finds will change them. What is
fixed now is who owns what, so the first three rounds don't build towards a collision.

- **Agent A, computed blend** (M to L): old 4.5 as written, read against `render3d/lit.rs` as built;
  shares baked into `how.r`; the live pass's shares for moving screens. Owns `stage3d/light.rs`,
  `render3d/bake.rs`, `mapped.rs`. Tests: shares add to one (pure); two mapped projectors' simulated
  overlap is flat (lavapipe); the hand blend is ignored once shares are on, and comes back when
  *Mapped* is switched off.
- **Agent B, tracking** (L): a new `src/tracking/` (PosiStageNet parser, pure; the listener, watched
  by `reaper` rules where a process is involved, none here); OSC addresses in `remote.rs`; the
  relay message; look-ahead; a tracker bound to a screen; the coordinate fit through
  `calib::fit_rigid`. Owns `remote.rs`, `engine/screens.rs`, `net/proto.rs`, `app/play.rs`. Tests:
  made-up packets; a `World` where the primary hears a tracker and two followers follow within the
  stated frames; a tracker going quiet falls back.
- **Agent C, a model's part as a screen** (M; old 6.3): owns `stage3d/mod.rs`, `stage3d/load.rs`,
  `net/store/manifest.rs`, `ui/stage_edit.rs`. Tests: old 7.1's for P7, and the bake and the live
  pass with a mesh that isn't one of NewQ's own shapes.
- Contract commit: `LensMap` shares on; `tracking` types and the relay `Msg`; `SurfaceShape::Model`.
  Area files: `stage3d.md` for all; `remote.md` and `network.md` for B.

**After round 4**: "Mapped projectors fade into each other by themselves" and "screens can follow a
tracking system" (the second said as *experimental* until tried with real hardware).

### Round 5

Old P7's rest (content as seen from a seat) and old P8 (projectors from a model's cameras, MVR), as
the old plan's 7.1 and 7.3 (round 3) have them, with that plan's owners: one agent on the shader and
`stage3d/mod.rs`, one on `load.rs`, `glb.rs` and a new `stage3d/mvr.rs`, ordered where they share
`load.rs`. Unchanged by this revision, and an outline like round 4.

### What each round's tests must include, in one place

| Kind of test | Round 1 | Round 2 | Round 3 |
|---|---|---|---|
| Pure CPU reference against lavapipe output | the bake; the map pass | the round trip through the light | the live pass at moved placements |
| Mouse-driven UI | canvases, the Mapped tick | line-up, the fine-tune | the screen cue's inspector, *Put at* |
| Network | pure: which maps each computer makes (a primary streaming a follower's projector) | loopback hubs: the line-up messages, strangers, a dropped link | three engines agree; late joiner; takeover |
| Both new shaders through naga's HLSL and MSL writers | bake, map pass | | live pass |
| Test Mode | none (not reachable by a user yet) | a leg with a mapped display, and the soak | the leg with a screen moving |

Every round ends as the old plan's 7.3 says: the area notes, this file's "built so far", README,
CHANGELOG lines in words for people who run shows, the old-show and default-unwritten tests, and a
headless run with screenshots looked at.

---

## 7. Risks, plainly

1. **Nothing here has met a projector.** Still the old plan's risk 2, and still the largest. The
   first real use will find things no test here can: gamma, black level, a lens that isn't a
   pinhole, a model that doesn't match the building.
2. **The output path is touched** (round 1). The defence is the layer design: with nothing mapped,
   `output_loop` runs today's code with an empty list in front of it, and a test compares the
   picture before and after. A mapped display adds passes; Test Mode gets a leg that measures it
   (round 2), on whatever machine it is run on.
3. **Cue-driven movement is a promise about scenery NewQ can't keep.** Section 1, point 4. Say it in
   the README where the cue is described: "NewQ moves the picture as the cue says. It doesn't know
   where the scenery is."
4. **A moving edge isn't smoothed** beyond a one-pixel fade, and a card may round a moving screen's
   edge differently from the baked map's edge beside it by a fraction of a pixel. Seen only on a
   real projector.
5. **Memory.** A 4K mapped projector is 66 MiB of map (100 with depth) plus its target and its
   canvases, on the computer that draws it; a primary that streams two mapped displays bakes two
   more at the stream's size. Stated in the UI's words when a map can't be made.
6. **Shaders.** The bake and the drawer are small and plain, but DirectX 12 has refused one plain
   thing already (old 1.2). Each is made in an error scope, a refusal blacks the projector and says
   why, and the naga HLSL/MSL test that `lit.rs` has is repeated for both. That proves naga writes
   them, not that the real compilers take them: CI on Windows and Mac is the only other proof, and
   what CI's runners have for a graphics card I have not checked.
7. **The primary bakes for streams.** A primary with a weak card and several mapped streams does
   work the old plan didn't count. It is bounded (two streams) and off unless a stream is on.
8. **Line-up's feel.** Nudging a cross while watching a wall, over a network, can't be judged here.
   The numbers say it will follow within a frame or two on a wired network; a hand on an arrow key
   may disagree.
9. **Scope.** Four rounds to get to moving, tracked, self-blending mapping is a lot stacked on
   something untried. The plan is cut so that you can stop after round 2 and have a finished thing.

### What can only be tried with real things

- **A projector and a wall**: whether a four-point line-up lands; how big the leftover error is and
  whether the fine-tune mesh is the right tool for it; gamma and black level in a blend; the edge of
  a screen; lens distortion on a short-throw lens (not modelled at all).
- **A second computer**: line-up from the primary on a real network, wired and Wi-Fi; two computers'
  projectors in one overlap staying in step (the network's existing job, R21, but never looked at
  through a lens map); a stream of a follower's projector made by the primary.
- **A real room**: whether the drawing matches the building; which points people can actually find.
- **A real graphics card on each system**: speed and memory of the extra passes; Windows' and a
  Mac's shader compilers; 4K.
- **Scenery that moves, and anything that tracks it**: all of round 3's honesty and all of round 4's
  tracking.

Each release note should say "tried only in simulation" for the parts that were, as CLAUDE.md asks.

---

## 8. The decided questions, checked again

- **Hand-written solver, no `nalgebra`**: still right. One addition (section 5, point 7): keep the
  rigid fit as its own public function.
- **The fine-tune is a mesh only**: still right, and now cheaper than the old plan thought: the
  mapped picture is a layer, and the mesh warp that exists draws it. One thing to know: that mesh is
  at most 9 by 9 lines and smooth (`mesh::MAX_LINES`). It will take up "the screen sags 3 cm in the
  middle"; it will not take up "this flat is 10 cm from where the drawing says", which is a job for
  more line-up points or for lining up the screen (old 5.6).
- **No real projector products list**: still right.
- **Lux and nits only**: still right.
- **LED, front or rear, "worked out by default": this one I'd now change in one respect.** Today the
  light decides per frame: a screen any projector's beam reaches is projector-lit, any other keeps
  its own picture as an LED wall (`lit::reached`, `render3d/lit.rs`). Once screens move, an LED wall
  that travels through a beam would flip to "projection" for those frames, and a mapped projector
  would start mapping onto it. So: the screen gets an explicit kind (**Projection, front /
  Projection, rear / LED or monitor**), and "Works it out" stays the default but is **worked out
  once, from where things stand at rest**, never per frame, and the row says what it came to
  ("Works it out: projection, front"). A mapped projector never maps onto a screen that is LED.
  In the file: `Surface::rear` (`stage3d/mod.rs:380`) stays as it is; one new optional field says
  LED or not, absent meaning "work it out". Round 1, agent C.

The old open questions this revision doesn't touch: 2 (16 simulated projectors: unchanged), 9 (which
imports: round 5, still yours to rank), 14 (house lights in the simulation: not planned).

---

## 9. Questions still open for the owner

**Answered by the owner, 26 Nov 2026:** (1) canvases are only displays with no monitor that a
screen shows; (2) a stream of a mapped projector's display is for recording and watching (no
fine-tune or blend); (3) moving scenery is not yet known: round 3 (cue-driven) as planned,
PosiStageNet first in round 4; (4) no real projector, wall or second computer yet: a mapping
release is marked "tried only in simulation" in its notes and changelog.

The questions as they were put:

Only the ones I can't settle from the code.

1. **Is a display that has a monitor *and* is shown on a screen a canvas?** You said "the displays
   that feed screens". I've made it "…and have no monitor", so an LED wall's display stays a
   display. *My recommendation: keep the narrower meaning.* If you'd rather every display on a
   screen is a canvas, it is a one-line change to `Show::is_canvas` and to the words.
2. **What should a stream of a mapped projector's display be for?** I've made it the mapped picture
   without fine-tune or blend, matching how every other stream leaves out the keystone: right for
   recording and for watching. If you mean to *feed a projector* from the stream (over NDI, say),
   it needs the fine-tune and the blend in it, and so do today's unmapped streams. *My
   recommendation: as planned now, and a separate "send exactly what the projector gets" tick later
   if someone needs it.*
3. **Which is your moving scenery: a few resting places, or a picture that must stick while the
   audience watches it travel?** And what moves it: hands, a winch with a controller (which make?),
   a tracking system? This decides whether round 3 is the end of the matter for you or round 4's
   tracking is the real goal, and whether PosiStageNet or OSC comes first. *My recommendation
   without an answer: round 3 as planned, PosiStageNet first in round 4.*
4. **Is there a projector, a wall and a second computer to try round 2 on before it is released?**
   (The old plan's question 12, still unanswered, and it matters more now.) *My recommendation: don't
   call the first mapping release done until one curved or angled surface has been lined up for real,
   once alone and once from a primary.*

---

## Built so far

Nothing. When a round lands, say here what a user can do, the code, and what changed from this
plan, as the old plan's "Built so far" does.
