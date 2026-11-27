# The 3D stage, projectors and the 3D renderer

Covers `src/stage3d/`, `src/calib/`, `src/stage_render.rs`, `src/stage_live.rs`, `src/stage_counts.rs`, `src/render3d/`, `src/ui/stage_view.rs`, `src/ui/stage_edit.rs`, `src/ui/stage_projectors.rs`.

Moved word for word from the Layout section of `CLAUDE.md` (commit 2589061). Read this before
working in these files, and keep it true when the code changes. A heading that says "Part of"
is a piece of a longer entry that covered several areas.

- The 3D stage (0.25.0; plan and the FBX verdict: `.claude/notes/stage3d-plan.md`). `src/stage3d/`:
  `mod.rs` `Show::stage3d` (`Stage3d`: `venue: Option<Venue>`, `surfaces`, `viewpoints`, `units`;
  unwritten while empty, every field at its default too). Metres, degrees, right-handed, Y up; a
  `Placement` is `T * Ry(yaw) * Rx(pitch) * Rz(roll)`. `Surface` (`SurfaceId` from `alloc_id`, in
  `load`'s next-id guard) has a `SurfaceShape` (Flat, Curved: `width` along the arc, concave unless
  `convex`; Faceted: `Facet`s with their own `uv` region) and a `Feed` (None or a display's
  `region`, `[x, y, w, h]` as `PartOf::area`); `Show::whole_picture` is what a blended projector's
  screen shows. `forget_missing_feeds` runs in `remove_output`, `load` and `use_displays_as`; the
  venue's path is kept relative like cues' files. `Stage3d::scene(show, Option<&LoadedVenue>)` makes
  the renderer's `Scene`: venue lit with `Venue::matrix` (units, up axis, placement), screens unlit
  `Paint::Output` with the feed and piece regions composed, `UNFED_GREY` without one (or for a display
  that isn't there). `scene.rs` the neutral types the renderer draws (`Mesh`, `Paint`, `Part`,
  `Camera`, `Scene`; conventions in its header: CCW front faces, UV (0,0) top-left, column-major
  `m[col][row]`, the renderer applies a region). `geometry.rs` pure meshes (`surface_mesh` ->
  `Piece`s; `arc`, `jumbotron`, `angled_wall` presets; sizes and counts clamped, `MAX_FACETS`) and the
  matrix sums. `load.rs` `load_venue` (worker thread; `catch_unwind` as a last guard): glTF/GLB via
  `gltf` (no `import`: buffers and pictures read here, views/accessors checked first) and OBJ+MTL via
  `tobj`; merged per material, node transforms baked, brightness baked (`Venue::needs_reload`),
  `WARN_TRIANGLES`/`MAX_TRIANGLES`, `MAX_TEXTURE`; FBX refused in words (`FBX_WORDS`). A material's
  triangles are cut into parts of at most `MAX_PART_VERTICES` (4M; the card's buffers are 256 MB
  under `downlevel_defaults`) by `split_mesh`, which keeps only used corners (`Builder::part_vertices`
  smaller in tests). Every file is found through `load_venue_with(path, venue, locate)` (`Found`
  Here/Arriving/Missing; `Unread::Arriving` is never a failure): side files by `side_path` (textual,
  the model's separator; refuses absolute, drive, UNC, URL, `..`), read by `read_capped` (regular
  files only; a glTF buffer at most its byteLength, `MAX_PICTURE_FILE`, `MAX_MATERIALS_FILE`); OBJ
  through `tobj::load_obj_buf`; `texture_tries`/`texture_file` (Missing beats Arriving: only one name
  is in a primary's list). `venue_files(path)` lists those files (a .glb's JSON chunk only; a name
  not there is listed too, so the primary's list says it is missing). Network: `manifest::media_paths`
  adds the venue, `manifest::build` adds `venue_files` on the hashing thread, `Puller` ranking puts
  used files the order doesn't name after it, `add_facts` skips `is_model_part`. Tests make their
  models in the test (`load_tests.rs`; `net/store/tests.rs::a_venue_model_and_its_files_reach_…`).
  In the Media manager (`relink.rs`, below). Lenient reading: `SurfaceShape::Unknown`/`Feed::Unknown`
  (`#[serde(untagged)]` variants keep a later NewQ's value and write it back; drawn as nothing),
  `lenient`/`lenient_option` for `LengthUnit`, `UpAxis`, `OutputConfig::source`.
  `OutputConfig::source: Option<StageSource { viewpoint }>` (unwritten when None): a display that
  shows the stage; `Show::stage_source` (the `whole_picture`'s, never a sub display's; the one rule
  for app.rs and `shows_stage`), `stage_source_refused` (the page's reason for a projector or sub
  display), `sanitize_stage` (in `load`, `take_show`, `play_load_kept`: a sub display's source
  cleared, duplicate viewpoint names renamed), `rename_viewpoint` (sources follow, a taken name
  refused; the page types into a draft). `Show::shows_stage` (itself or its `whole_picture`) makes `scene` leave a screen
  fed by it unfed (`UNFED_GREY`): the stage is never drawn from itself (sampling the target being
  drawn is a wgpu error). `Show::stage_camera` (the viewpoint, else all of the bounds from in front),
  `outputs_in(scene)`, `Stage3d::part_surface` (a picked part's screen: venue parts come first, lit).
  Integration: `src/stage_render.rs` the "stage-render" thread (`StageThread`, `StageDrawer`;
  latest-wins `StageJob`s merged so no view is lost; `crash::OnPanic`): `FeedJob`s first (each
  display's layers composited off screen, Rgba8Unorm, through the one `gpu::Slot::Stage`, one
  `Gpu::submit` each before the next display's uniforms are written: a layer can be on two displays),
  then `ViewJob`s drawn by `render3d::Renderer` into a `LayerTexture::new_target` (RENDER_ATTACHMENT;
  `render_view`, `mark_drawn`); `feed_size` (1024 longest, times the governor's scale, at least 256).
  `src/stage_live.rs` (`App::stage`): `begin` (venue on a "venue-load" thread through `locate` =
  `net::media::local_path`, a brightness change waits `RELOAD_SETTLE`, a failure isn't retried until
  the venue, `media_generation` or its files change; the load thread lists the model's files
  (`venue_file_list`: itself and `venue_files`) and `stamp_of` them (located path, length, mtime) before
  reading (`Read`); `VenueState::watch` stats that list on a "venue-watch" thread every `WATCH_EVERY`
  (2 s; `wake_at` only while the thread runs) and two looks in a row that agree and differ from the read
  make it `stale`, read again as a brightness change is (the old model kept until the new lands; a new
  `scene_key` makes `update_outputs` redraw the control window and pop-outs), a venue on its way is `VenueStatus::Arriving`, looked at
  every `ARRIVING_LOOK`; `stage_seen` compared each pass, `shape_key` (hashed through a writer) only
  on a change; the scene made again only when its key changes, into `UiState::stage` with
  `stage_bounds`: the venue's box and the screens), `output_layer` (a display showing the
  stage gets its target as its only layer, `Fit::Stretch`, so window, keystone, blend, streams and
  `part_of` projectors work unchanged), `send` (`StageOutput { seen, due }`: one no window or stream
  shows is preview work at the previews' pace; the previews' `ViewRequest`s; each drawn again only
  when its signature (with `feeds_key`, the stage's pictures' layers) changed or something plays;
  displays first (`ViewJob::display`, `StageJob::merged` sorts them, `displays_done`), previews wait
  while a job waits behind one whose displays are being drawn (`set_behind` in tests); a thread that stopped is noticed every pass, never busy, and
  restarted by `wake_at` after `RESTART_EVERY`; `IDLE_RELEASE` (30 s) with no 3D view in sight and no
  display showing the stage quits the thread and with it all it kept on the card (`stage_view::draw` pushes
  `StageShared::in_sight` every frame it is drawn, asking or not, and asks a repaint every `IN_SIGHT_EVERY`,
  10 s, so a still pane keeps it and its target); feeds by
  `feed_groups`: the whole picture's layers on its canvas with no keystone, mesh or blend, sub
  displays in their `area`), `landed` (the screens showing it and the control window redraw),
  `forget_device` (in `watch_gpu`'s `stopping`). `src/stage_counts.rs` (pulled in by `#[path]` from `stage_render.rs`): `stage_render::totals()` returns `StageTotals`, relaxed atomics counted through `CountTo` (default `TOTALS`; a test's `Counters::for_test()` counts into both). It holds running sums (jobs, feeds, throws; `views` (previews, and displays only the previews show) and `displays` (shown in a window or stream) with their processor time and `frames_by_ms` bands (`FRAME_MS_BANDS`); missing pictures; `light_left_out` views; shadows; the thread's starts, stops, `restarts` (only after a stop) and idle releases; venue loads and failures) and levels (`cpu_us_worst` since start, `throws_left_out` past `MAX_THROWS`, the last venue's ms, parts and triangles). `since` subtracts the sums and keeps the levels, and `worst_band_ms` on a `since` gives the worst frame between two looks to a band. `StageDrawer::counting` counts in `draw`, `StageLive` counts the thread's life in `send`/`keep_thread` and the projectors left out in `light_up`, and the venue-load thread counts its read. `feeds_a_display` keeps those displays' uploads off
  the previews' pace (not when every stage display is preview work, `governed_only`).
  `App::stage_busy` keeps the loop coming round; `wake_at` brings it back. `src/ui/stage_view.rs`:
  `StageShared` (`UiState::stage`: scene, bounds, `VenueStatus`, `requests`, `targets` by `PreviewId`,
  `actions` drained into `ui::draw`/`draw_popout`, `picked`) and `draw(ui, &Show, st, &mut
  StageViewState, PreviewId)`: orbit/pan/dolly/frame/pick, overlay (viewpoints, See all, Grid, X-ray,
  Edit…), asks `preview_budget` each frame and pushes a request only when it may draw (or has no
  picture), the governor's scale through `Quality::scaled_by`; shows its target through the compositor
  (`preview::callback`, pace None). Hosted by a Preview pane in 3D mode (`PreviewId::Stage(host)`,
  state in `UiState::preview_stage`) and by Settings ▸ 3D stage.
  `src/ui/stage_edit.rs`: Settings ▸ 3D stage (`SettingsPage::Stage3d`), which lays itself out
  (`settings_view` hands it the page without the 640 pt scroll column): `page` picks an
  `Arrangement` by the room (Stacked: the view, `SETTINGS_VIEW`, pinned on top at `view_height`,
  the `grip` under it dragging `StageShared::view_height`, then the tabs; SideBySide on a page
  under `STACKED_FROM` high and `SIDE_BY_SIDE_FROM` wide, the classic window's: the view on the
  left at `view_width`; Cramped: all in one scroll area), then `StageTab` tabs (Venue, Screens,
  Projectors, Viewpoints, Displays; `StageShared::tab`, for the run; each a `settings::STAGE_*`
  `Section`, its label noted by `settings::note_heading`) and only the chosen tab's content in a
  `ScrollArea::both` salted by tab (`tab_scroll`; sections drawn in `settings::untitled` frames).
  A new pick (`StageShared::tab_saw`, screens and projectors) opens its tab once. Test notes
  "3D stage ▸ view", "▸ view size", "▸ tab ▸ <label>", "▸ tab content". It edits the show directly
  as other pages do; `StageEdit` (`Action::Stage`, an edit
  of the show) for what a view asks; `Preset` (Flat, Curved, Jumbotron, AngledWall), `add_surface`
  (fed by `first_feed`), `feed_choices` (no display that shows the stage). A display is set to show
  the stage on Settings ▸ Displays: its *Shows* (`ui.rs::shared_picture_editor`) draws
  `stage_edit::StageItems::items` (`stage_choices`, `stage_source_words`; greyed with
  `Show::stage_source_refused`'s reason); `ShowPartOf` clears a `source`; the page's `sources_section`
  (the Displays tab, `settings::STAGE_DISPLAYS`) lists them, each a button to it there (`UiState::reveal_display`). Samples: *Use a sample
  venue* (`StageEdit::UseSample`, with the usual screens and viewpoints), *Export as .glb…*. Tests: `stage_render_tests.rs` (GPU: a four-colour picture
  the right way round on a screen and on a display showing the stage; the thread), `stage_live_tests.rs`
  (venue loading with a made OBJ; a display's picture drawn and not again while still),
  `ui/stage_view_tests.rs` (governor, pick, drag), `ui/stage_edit_tests.rs`. Tried headless under xvfb
  (lavapipe): the view, the page, a display showing the stage; never on a real GPU or display.
  Sample venues: `samples.rs` (pure) makes seven in code (`SampleVenue` Arena, Proscenium, Thrust,
  BlackBox, Club, Festival, Ballroom; `ALL`, `label`, lenient: an unknown name reads as BlackBox). In
  the show `Venue::sample` (`"sample": "Arena"`, `path` empty and unwritten); `load_venue` makes it
  instead of reading the path (`sample_venue`, through `load::Builder`, so the same `LoadedVenue`:
  a part per material, plain lit colours, brightness baked); `Venue::matrix` is the placement alone
  (metres, Y up); `needs_reload` compares `sample`. All: floor y = 0 (the pit to `lowest()`), stage
  towards -Z, audience +Z, origin per variant's doc. Pieces: `face` (winds to a hint, Newell normal,
  flat normals, no zero-area halves), `cuboid`, `block`, `prism`, `bar`, and seating as a stepped
  `Rake` swept along a `Path` (rounded rectangle, U, fan, straight). `screens(kind)` are
  `StarterScreen`s (name, placement, shape) added unfed by `Show::add_sample_screens` (ids from
  `alloc_id`; follows the venue's position and yaw, not pitch or roll); `viewpoints(kind)` and
  `Stage3d::add_sample_viewpoints` (by the venue's matrix, once per name). Tests in
  `samples/tests.rs` (budget, size, winding, screens inside, facing and seen from the first viewpoint
  by a ray test); the ignored `draw_the_samples` rasterises each into `NEWQ_SAMPLE_PICTURES`.
  `glb.rs` (pure) writes parts as a .glb (mesh, node and `baseColorFactor` material per part;
  `samples::sample_glb`), read back through `load_venue` part for part in a test (so each sample's
  materials have colours of their own). Not offered in the UI yet.
  Projectors (steps P1 and P2 of `.claude/notes/projection-plan.md`). `stage3d/projector.rs`:
  `Stage3d::projectors` (`Projector`: `ProjectorId` from `alloc_id`, in `load`'s next-id guard, and
  `Show::number_projectors` after it for a missing or doubled id; `Lens { throw, zoom, shift, aspect, gamma }`,
  `lumens`, `display`, `resolution`, `model`; every field but `id` defaulted and unwritten at it, and
  `#[serde(flatten)] extra` on both keeps a later NewQ's fields; read through clamping getters, `Lens::zoom`
  sorted and finite). Conventions in its header: the lens looks down its own -Z, throw = distance / width,
  shift a share of the picture (0.5 up: the bottom edge on the axis). The throw calculator: `frustum`,
  `picture_at`, `distance_for`, `ray(u, v)`, `in_picture`, `camera` (the look-through `Camera`), `aim_at`
  (the picture's middle, not the axis, onto a point; roll kept), `fit_to` (square on, the box round the
  shape, a `rear` screen from behind), `throw_for`, `on_flat` (corners, `fills` by polygon clipping,
  `spill`), `pixel_size_at`, `footprint` (rays `FOOTPRINT_INSET` inside the edge) and `cone`;
  `LensPreset` (plain-word lenses, `for_throw`), `BRIGHTNESS`, `RESOLUTIONS`. `Show::add_projector`,
  `projector_display_refused` (sub display, shows the stage, taken), `sanitize_projectors` (in
  `sanitize_stage`), `forget_missing_outputs` clears a gone display. `stage3d/light.rs` (pure): lux by the
  inverse square law and the two cosines (`illuminance`, `Facing`), `luminance` (lux × gain / π),
  `lit_by` (shadow test by `pick`). `Surface::gain`/`rear` (unwritten at 1/false). `Stage3d::scene` puts
  a lit `projector_body` (wholly behind the lens) per shown projector after the screens:
  `projectors_from`, `part_projector`; `part_surface` stops before them. `Part::catch` (`Catch { gain,
  front_only }`: a screen's gain and side for the light; the venue's default). `Scene::throws` is filled
  only in `stage_live`'s `lit_scene`. UI: `src/ui/stage_projectors.rs` (`section_ui`, the Projectors tab of Settings ▸ 3D
  stage, `CORNER_PIN_WORDS`; `add_aimed_at`; rows with `readout_words`, *Fit to a screen*, `which_lens`;
  "My projectors" in `my-projectors.json` in the log folder, `ProjectorsUi::library_dir` for tests);
  `ProjectorsUi` (`StageShared::projectors`) runs a `projector-sums` worker (latest wins; footprints and
  the light under the pointer, `ask_here`/`Here`, shown only in the view that asked, `here_in`; `waiting` makes a view repaint) so no venue ray test is
  on the main thread; `view_lines` gives each `ViewRequest` its lines (cached by key). `stage_view`:
  bodies picked by click, `StageViewState::through` (*Viewpoint ▸ Through …*; drag, wheel, double-click
  or Esc leave to an orbit from the lens), `letterbox`. Tests: `projector_tests.rs`, `light_tests.rs`,
  `render3d/tests.rs` (lines, cone and outline, look-through), `ui/stage_projectors_tests.rs` (mouse:
  add aimed, point at, which lens, keep; the worker; pick and look through); the ignored
  `write_a_show_with_two_projectors` (`NEWQ_PROJECTOR_SHOW`) makes a show to try by hand. Tried
  headless under xvfb (lavapipe) with the proscenium sample and two projectors; never a real GPU,
  projector, light meter or room.
  The light (P2): `StageViewState::light`/`light_view` (`render3d::LightView` Picture/Brightness/Overlap/
  PixelSize)/`auto_white`/`white_nits`, view state only (not the show, not saved); *Light* in the view's
  bar when the show has projectors, `light_bar` (Show menu `LIGHT_VIEWS`, *Auto exposure* =
  `stage_projectors::auto_white`, rounded to whole nits since the slider rounds), `legend`/`draw_legend`,
  `light_notices` (refused GPU, more than `MAX_THROWS`). `light.rs` adds `MAX_THROWS` (16), `throw_of`
  (matrix straight from `Projector::frustum`, not through `Camera`, whose angle is clamped), `throw_uv`/
  `throw_lux` (the shader's sum on the CPU, tests compare), `band` with `BRIGHTNESS_STEPS`/
  `PIXEL_STEPS_MM`/`SCALE`/`OVERLAP` (the shader is given them), `auto_white`, `ROOM_LIGHT`. App:
  `app.rs::projector_throws` (only when a request has its light on; shape by `stage_projectors::shape_of`,
  as the lines) and `throw_picture` (the display's group with `warp = pin`, its sub displays by
  `corner_pin(area)`, `blend.clamped()`; None for a display that shows the stage; a mapped projector's by
  `mapped_throw`: its own group shows its `MapUse::Preview` frame's target, asked for at `throw_size(1.0)` by
  `App::lens_maps(gpu, light)`, with `ThrowPicture::mapped` set and slots made for `MapOwner::Throw(n)`,
  `n` its place among the throws; none until the map is made; those canvases stay preview work) ->
  `StageLive::send(.., projectors: &[ProjectorThrow], ..)` -> `light_up` (`lit_scene` made again only on
  `lit_key`; a key of the pictures, `groups_key`, xored into a lit view's signature, so a blend width or a
  still redraws it) -> `ThrowJob`s only in a job with a lit view, `throw_size(lit scale)` (960 wide 16:9,
  the governor's scale). `stage_render`: `Throws` (an `Rgba8Unorm` array of `layers_for` layers, a D2 view per layer),
  each throw composited through `Slot::Stage` with its own `BlendPass` (set, composite, submit, one at a
  time) after the displays' views (`draw_throws`), `StageJob::merged` keeps newer throws by layer; `render_lit` gets the array for a lit view. A
  display that shows the stage is drawn unlit. Tests: `light_tests.rs`, `render3d/lit.rs` (packing,
  `box_meets`, naga writes HLSL 5.1/6.0 and MSL: naga is a dev-dependency), `render3d/tests.rs` (four
  colours the right way round front and rear, a box's shadow and no acne, two throws add and match
  `throw_lux`, the three colour views, a refused shader still draws, an LED wall keeps its picture),
  `stage_render_tests.rs` (blend on the throw's layer, thrown the right way round, a blended pair's
  overlap as bright as either alone in light), `light_tests` (every screen shape's normals out of its
  CCW front, which `fs_lit`'s side relies on), `stage_live_tests.rs`
  (`blended_pair_show`, the ignored `write_the_lights_demonstration_show` with `NEWQ_DEMO_SHOW`; what a
  window sends, `light_up`, only a lit view lit), `ui/stage_view_tests.rs` (`Driven`: Light and the views
  by mouse, notices, legends). Tried headless under xvfb (lavapipe); never a real GPU, Mac, Windows or
  projector.
  Lens maps (mapping plan 4.1, round 1 agent A; the drawer and the output path are `mapped.rs`,
  `compositor-and-displays.md`). `stage3d/lensmap.rs` (pure): `still_scene` (the views' scene without
  the projectors' bodies, `projectors_from`, and without any screen `Show::movable` names), `still_key`
  (each part's mesh key, transform, output and region, lit, side), `map_key` (still key, placement,
  lens, aspect, size, depth: never `Projector::fine`/`line_up`, nothing a cue changes),
  `lens_matrix`/`tile_matrix` (the light's `throw_of` lens; a tile's part of its frustum),
  `feeds_seen` (fed screens whose box meets the lens, `lit::box_meets`, scene order, at most
  `MAX_FEEDS`; past them a screen is only in the way), `part_feed`, `SCREEN_SLACK` (a fed screen wins
  up to 0.1% of its distance behind anything else: a screen on a wall), a front screen
  (`Catch::front_only`) met from behind shows nothing; `reference`/`reference_with(.., aspect)`: a ray
  through each pixel's middle (`Projector::ray`), `pick` among fed screens and among the rest, the
  hit's uv by barycentrics through `part_region`. `render3d/bake.rs` + `bake.wgsl` (its own module,
  after `mapped::WGSL_UNPACK`; never `fs_part`/`fs_lit`): `Renderer::bake(gpu, &Bake { scene,
  projector, aspect, size, key }) -> Result<LensMap, String>`, the renderer's meshes
  (`ensure_mesh`), one pipeline (fed screens write packed place and how, everything else "none";
  reversed depth, `Greater`; fed screens' clip z pushed by `SCREEN_SLACK`) into twice-size scratch,
  then `fs_resolve` (majority feed of the four samples, the average of their places, their share as
  coverage) into `LensMap::texture`s; in tiles of `TILE` (1024, at most half the card's largest
  texture; `bake_in_tiles` in tests), a `Gpu::submit` each; inside validation, internal and
  out-of-memory scopes; `BakeKit` made through `checked`, a refusal sets `bake_refused` for good
  (`refuse_bake` in tests). Memory: 8 bytes a map pixel (17.6 MiB at 1920×1200, 63 MiB at 4K), and
  while baking at most 48 MiB of scratch, let go after. `stage_render`: `MapJob { projector, aspect,
  size, key, scene, output }` in `StageJob::maps` (`merged` keeps the newer of a projector and size),
  output maps baked first of all, preview maps after the previews; each `Baked` pushed at once to the
  `BakedSlot` given by `StageThread::spawn_with` (`StageDrawer::baked_into`); counted
  (`maps_baked`, `maps_refused`). `stage_live`: `lens_map(projector, size, use_)` (aspect
  `Projector::aspect(size)`, or its own pixels' for `Preview`) and `lens_map_shaped(.., aspect, ..)`:
  the call is the ask (`MapEntry` by projector, size and shape, `MapAt`: `want`, `in_flight`,
  `refused`, `asked_at`, let go after `MAP_KEPT`; a `give_lens_map` map is asked for as any other and
  never baked), None until the first map, the old map until the new lands; a refusal landing is said
  once from `begin` (`Said::Problem`, "stays black"); `begin` makes
  `still`/`still_key` with the scene; the scene's key is `stage_key_of` (`shape_key` with fine and
  line-up cleared; `stage_seen` kept when only those changed); `send` -> `maps_due` (output maps
  always, preview maps only with a lit request this pass and not while `displays_behind`; a due map
  keeps `wanted_at` and starts the thread, an up-to-date one doesn't), `in_flight` cleared when the
  thread is found stopped; `take_baked` in `begin`'s asks, `send` and `landed` (a map landing makes
  `landed` true); `feeds_output` (`feeds_of_outputs`: the canvases a window's or stream's map's
  projector sees, the whole pictures and sub displays, from the ask on); `forget_device` clears the
  maps and the slot. Not yet: `depth` (round 3), the 3D view's light through a map (round 2), the
  app's calls (agent B). Tests: `stage3d/lensmap_tests.rs` (pure), `render3d/bake_tests.rs` (lavapipe,
  64×36, against the reference away from edges: feed exact, coverage 1, place within 3e-5 plus 2% of
  a pixel's step; flat, curved both ways, angled wall, jumbotron, a box in the way, a screen on a
  wall, front and rear, two canvases, tiles of 10; a refused bake; naga HLSL 5.1/6.0 and MSL),
  `stage_render_map_tests.rs`, `stage_live_map_tests.rs` (thread started for a bake and the map
  outliving the idle release; a still scene and a fine-tune or line-up bake nothing again; a moved
  projector re-bakes its own; a preview map waits for a lit view; a lost device; a venue landing
  later). Lavapipe only; never a real GPU or projector.
  Round 2 (agent C): **the bake agrees with the model.** A map's canvases are
  `lensmap::feeds_for(show, p)` = `Show::mapped_feeds(p).used` (what the projector's row says it
  maps), worked out by `StageLive::begin` with the scene into `map_feeds` (by projector), carried
  in `MapJob::feeds`/`Bake::feeds`, hashed into `map_key` (which hashes
  `Projector::without_tuning()` whole, so a renamed or redisplayed projector re-bakes), and used
  by `feeds_output`; `feeds_seen` is gone; `reference`/`reference_with` take the feeds. An LED
  screen (`Catch::led`, set from `Show::kind_of` in `Stage3d::scene`) is unfed in `still_scene`
  (only in the way) and never `lit::reached` (keeps its picture in the light). Not covered: a
  screen the window's or stream's frustum meets but `reaches_screen` (the projector's own
  `resolution` shape) doesn't is in the way, black through that projector.
  **A mapped projector in the light**: `ThrowJob::mapped: Option<MappedFrame>` (from
  `ThrowPicture::mapped`, which `app.rs::throw_picture` fills with the `MapUse::Preview` map at
  the throw's size); `draw_throws` runs `mapped::draw_mapped` (`StageDrawer::mapped`, made at the
  first, `MapOwner::Throw(layer)`, frame 0) and submits its buffers with that throw's composite;
  `light_up` keys the lit view on the frame's map stamp and its canvases' layers (stamps
  included) and makes their `Slot::Map`s on the main thread. Tests: `stage_render_mapped_tests.rs`
  (the round trip on lavapipe: a gradient laid straight on a concave curved screen against the
  same content mapped and thrown back, worked out as (mapped - black) / (white - black) through
  the projector's gamma, where the white throw is bright five pixels round: mean 0.003, 95th
  percentile 0.006, worst 0.009 in display values, lines 0.01/0.02/0.05; the content thrown
  unmapped is 0.041, so the test tells a map from none; a plain throw makes no drawer),
  `stage_live_map_tests.rs::a_baked_maps_canvases_are_the_ones_the_model_says_it_maps` (an LED
  screen showing a canvas, five canvases, a 4:3 window), `lensmap_tests` (LED in the way, feeds),
  `render3d/tests.rs::an_led_screen_in_a_beam_keeps_its_own_picture`. End to end:
  `stage_live_e2e_tests.rs` (ignored; `NEWQ_MAPPED_E2E`) and `scripts/mapped_e2e.sh`: a known
  picture on a canvas, `soak::mapped_stage` (two mapped projectors on one curved screen in the
  Proscenium sample), the left projector's display and the canvas recorded with
  `StreamKind::Record`, the real app run under Xvfb, a late frame of each against
  `lensmap::reference` at the recording's size (the canvas's recording is the codec's floor).
  The fine-tune's editor is `ui.rs::fine_tune_editor` (below, previews-and-ui).

- `src/calib/`: the line-up solver (step P3 of `.claude/notes/projection-plan.md`, section 5.3). Pure
  f64, hand-written (no linear algebra crate, the owner's choice), `pub(crate)`, `#[allow(dead_code)]`
  on `mod calib` in `main.rs` until P5 wires it in. Conventions in `mod.rs`'s header: points in metres
  (`RoomPoint::world`) and **pixels** of the projector's picture (`RoomPoint::pixel`, (0, 0) its top-left
  corner, (W, H) its bottom-right, y down: `in_picture`'s `(u, v)` times `Problem::pixels`); the lens
  (`LensFit`) and pose (`Pose`, `Placement`'s yaw/pitch/roll in f64) as `projector.rs`'s; point numbers
  in words are index + 1, switched-off points counted. `model.rs` `project` is `in_picture` in f64
  (pinned by `the_projection_is_the_projectors_own` against `in_picture` and `ray`), `refine` is
  Levenberg-Marquardt on the error in pixels (central-difference Jacobian, a small turn in the lens's
  frame, Marquardt's diagonal damping, `MAX_ROUNDS`, no clock; a step putting a point behind the lens is
  refused), `spreads` the standard deviations for a pixel's error per point. `start.rs` the starting
  guesses: a flat set's homography, the 3x4 DLT (six or more, not flat; RQ split with `D = diag(1,-1,-1)`
  for the -Z lens and the downward v), a 64-direction ring of places facing the points (Horn; only
  when the others leave the points over a pixel out and there is no start or no linear guess); plus
  `Problem::start` (the placement now). `linalg.rs` 3-vectors, row-major 3x3s, `from_euler`/`to_euler`
  (straight up or down: roll 0, all yaw), cyclic Jacobi, Cholesky, `nearest_rotation` (Horn), `rq`.
  `solve(&Problem) -> Result<Solution, Refusal>`: checks first (`Refusal`: too few, no picture, not a
  number, same spot, same pixel, in a line, in a line in the picture, three of four flat points in a
  line, behind; after the fit, mirrored: a left-to-right flipped picture, over 5 px, not on a flat set), then the ladder, top down (`Rung`: Pose at 4, Throw at 6 not flat, Shift at 8 not flat, Aspect only
  when `Freedom::aspect`; a rung is kept only inside the lens's bounds and with each number's sd under
  2%, else a `Note` says why), each rung judged after 30 rounds, only the kept one refined on), then leave-one-out of the three
  points furthest off, with five or more points and one a pixel or more out (`Outlier`, named when it is
  3 px and five times the others' error off). `Solution::words`/`Refusal::words` are the operator's
  sentences; `placement()` and `lens_onto(&Lens, pixels)` apply it, `solved()` is what the show keeps.
  **One set of line-up types** (round 2's contract): the kept ones are the show's
  (`projector::{LineUp, LinePoint, Solved, Before, Rung}`; `calib` has no `LineUp`/`LinePoint`/`Rung` of
  its own, `calib::Rung` is the show's, ordered by `PartialOrd` with a later `Unknown(Value)` above or
  below none). The solver takes `RoomPoint`s (room metres); `to_room(stage, &LinePoint)` is the one way
  from a kept point (screen frame through the screen's `Placement`, as the scene places it) and
  `to_screen` the way back; `room_points`/`Problem::for_line_up` refuse a point on a screen that is
  gone (`Refusal::NoScreen`, a switched-off one is passed by). `projector::points_key` (FNV-1a of each
  kept point's `at`, screen, pixel and `on`, f32 bits, and the picture size) is the one fingerprint:
  `LineUp::points_key`/`is_current`, carried as `Problem::key` into `Solution::points_key`; worked out
  from the kept numbers, so a moved screen or another system's last bit leaves a line-up current. Every
  type is serde with defaults and lenient enums (`Unknown`). `project(placement, lens, pixels, world)` is the forward sum on the show's types
  (what the solver fits); `fit_rigid(from, to)` a rigid fit (Horn) for later. `Solution::capped`: the
  refinement hit `MAX_ROUNDS` (100; starts get 30). Deterministic on one computer; solve once and send the answer (another
  system's libm may differ in the last bit). Tests: `calib/tests.rs` (made-up projectors with
  `LensPreset`'s throws; exact data to 1e-6 m and rad; noise bounded by four of the solver's own sds).

- `src/render3d/`: the 3D stage's renderer (0.25.0 previz, `.claude/notes/stage3d-plan.md`; drawn on
  `stage_render`'s thread, `#![allow(dead_code)]` for what isn't used yet). `scene.rs` re-exports `stage3d::scene`'s neutral types (`Mesh`, `Paint`,
  `Part`, `Camera`, `Scene`); conventions: right-handed, Y up, metres, uv (0,0) the picture's
  top-left, `Paint::Output::region` (left, top, width, height) like `PartOf::area`, nothing culled.
  `Camera::shift`/`aspect` (a projector's lens): `frustum` (off-axis, letterboxed to the target's shape;
  `maths::frustum_reversed`), `picture_in`, `clip_to_near`; `project` and `ray` use the same frustum.
  `render_with_lines` draws `Line`s last, over everything (a `LineList` pipeline made through `checked`;
  `FrameStats::lines`), for a preview's projector cones: `ViewRequest`/`ViewJob::lines`, hashed into
  `stage_live::signature`; a display's job has none. `lit.rs` the light (P2; all optional, `checked`,
  which now pushes Validation and Internal scopes: a backend's shader compiler failing is Internal):
  `render_lit(.., throw_pictures: Option<&TextureView>)`, used when `Options::light` isn't Off and the
  scene has throws; a `LightKit` made at the first lit view (its own module `shader.wgsl` + `light.wgsl`,
  entry `fs_lit`, bind group 3: `LightsUniform` (vec4s only, 16 `ThrowUniform`s), the pictures array, a
  sampler, a `Depth32Float` shadow array (`SHADOW_SIDE` 1024, `SHADOW_SIDE_LOW` 512 below full
  scale, kept per side; `layers_for` the throws, a power of two up to 16, as the pictures' array) and a comparison sampler (`GreaterEqual`: reversed depth). Shadow maps by a
  depth-only pipeline over `vs_part` with the frame uniform's `view_proj` swapped per layer
  (`shadow_frames`), drawn only when `shadow_key` (every part's mesh and transform, the lens) changes
  (`FrameStats::shadows_drawn`); lookup pushed along the normal (`NORMAL_PUSH`) and taken `DEPTH_SLACK`
  nearer. An unlit part reaches the light only when `lit::reached` (`box_meets` on `GpuMesh::bounds`), else
  it keeps its picture (an LED wall); flags.z/w carry gain and side. A refusal sets `light_refused` (never
  tried again; `FrameStats::light_left_out`); `refuse_light` in tests uses a broken shader. `pick::Hit::normal` (towards the ray), `front`/`front_normal` (the CCW front: the light sums use a screen's own).
  `Renderer` (one per `Gpu`): `render(gpu, &Target{view, format, size}, scene, camera, outputs: &dyn
  Fn(OutputId) -> Option<&TextureView>, &Options) -> FrameStats` (draw calls, triangles, missing pictures)
  submits through `Gpu::submit` itself (its uniforms are written per call, so two renders can't share a
  submit). One uber-shader (`shader.wgsl`), colours in display values like the compositor's: lit = headlight
  from the eye + sky/ground ambient, Lambert in linear light, both faces, `lift` adds grey to dark colours;
  unlit = full brightness; a display with no picture is dark grey with an edge; optional grid (fragment-drawn
  1 m / 5 m lines) and axes; `venue_opacity` < 1 is the x-ray (lit parts drawn last, blended). Reversed Z
  (Depth32Float, clear 0, `Greater`). `Quality { msaa, scale }`: 4× where the adapter offers it for the
  format and depth (`samples_for`), a scale below 1 draws smaller and scales up (`blit.wgsl`); a preview
  passes its quality through `Quality::scaled_by(governor)` (multisampling off below 1), a 3D view that is a
  display's source doesn't; a frame not drawn is no call at all (the target keeps its picture); targets are
  kept per `Target::owner` (the view) and size, an owner's other sizes dropped on a resize, so the
  governor's steps don't churn and a resize makes no pile. `FrameStats` has `cpu_time` and `samples_shaded`. Meshes are
  kept by `mesh_key`, pictures by `key`, let go after `FORGET_AFTER` unused (looked over every `TIDY_EVERY`,
  2 s, while drawing) or at once by `keep_only` when the stage-render thread's scenes change; bad meshes
  (stray indices, no normals or uvs, NaNs) are mended or skipped; one bigger than the device's
  `max_buffer_size` is skipped and remembered (`too_big`, `limit_buffers` in tests). `camera.rs` (pure): `Camera::{sane, view, projection,
  project, ray}`, `Orbit` (`orbit`/`pan`/`dolly` from mouse deltas, `frame`/`frame_scene` zoom to fit,
  `from_camera`); `pick.rs` (pure): `pick(scene, ray, wanted)` -> `Hit { part, point, distance }`. GPU
  tests in `render3d/tests.rs` (lavapipe under `WGPU_BACKEND=vulkan`; `NEWQ_RENDER3D_PNG=<path>` writes the
  little stage of `a_little_stage_draws`). No real GPU or display tried.

- Mapping, round 1's model and words (`.claude/notes/mapping-plan.md` 4.1, 4.3, 8; tried only in
  simulation). Contract (B): `Projector::{mapped, fine, line_up}` with `LineUp`, `LinePoint` (`at`
  in the screen's own frame when `surface` is set), `Solved`, `Rung`, `Before`; `extra` on
  `Surface` and `Stage3d`; `Show::is_canvas` (a whole display, no monitor, no source, not a mapped
  projector's, that a screen shows: worked out, never stored), `mapped_projector`, `movable` (false
  until round 3). A `fine`/`line_up` this NewQ can't read is kept in `extra` under its own name by
  `ProjectorFile` (`Projector`'s serde is `from`/`into` it: a new field goes there too; the
  attributes on `Projector`'s own fields do nothing) and written back unchanged, and is no
  fine-tune or line-up here (a later `mapped` likewise: not mapped here; `LineUp::solved`/`before`
  are read strictly, so a later shape of either keeps the whole line-up as written); `Rung::Unknown(Value)` keeps a later rung. `Stage3d::shape_key` and
  `stage_projectors::free_display` (a new projector's display) passes over canvases while another
  display is free. `stage_projectors::key_of` hash `Projector::without_tuning` (no `fine`, no `line_up`), so a
  dragged point rebuilds no scene, map or footprint. The screen's kind: `Surface::kind:
  Option<SurfaceKind>` (`Projection`/`Led`, untagged `Unknown` kept; None, left out, is "works it
  out"), `rear` as before; `chosen_kind`/`choose_kind`, `KIND_CHOICES`, `ScreenKind {Front, Rear,
  Led}`; `Show::kind_of`/`screen_kind`: as chosen, else projection when any projector
  `reaches_screen` (the light's `lit::box_meets` on `Surface::boxes`, at rest, the projector's own
  `resolution`), else LED. Not yet used by the light (`lit::reached`) or the bake: round 1's A.
  `Show::mapped_feeds(projector)` -> `MappedFeeds { used, left_out, screens }`: the displays of the
  visible, known-shape, unmovable projection screens it reaches, in the screens' order, each once,
  at most `mapped::MAX_FEEDS` used (the rest left out: black through it, said on its row; the file
  is never changed for it). The rules: `mapped_refused` (`MappedRefused::NoDisplay`,
  `OnAScreen(screens)`; words in `stage_projectors::mapped_refusal_words`);
  `projector_display_refused` refuses a display a screen shows for a mapped projector;
  `stage_source_refused` gives `MAPPED_SHOWS_NO_STAGE`; `sanitize_stage` clears a screen fed by a
  mapped projector's display (the tick is kept); `feed_choices` and `first_feed` leave those
  displays out. `Show::add_canvas` ("Canvas N", no monitor, 1920 × 1080 picture),
  `new_canvas_for(surface)`, `screens_showing`, `canvases`. UI: the Screens tab's *Kind* row
  (`kind_row`, "Works it out: projection, front") and *Shows ▸ New canvas…* (`NEW_CANVAS`,
  `feed_rows` returns it, made after the loop); the Displays tab's `canvases_list`; the
  Projectors tab's *Mapped* tick (`mapped_rows`: greyed with the reason, *Give Screen 1 a new
  canvas, and map it*, "Maps Cyc onto Cyc screen.", the four-canvas warning,
  `ONE_FLAT_SCREEN_WORDS` for one flat screen, `MAPPED_WORDS`). Tests: `projector_tests.rs`
  (kind, defaults unwritten, feeds, rules, scene key, a later fine/line-up kept),
  `stage3d/tests.rs` (a later kind and field on a screen kept), and the mouse-driven
  `ui/stage_edit_tests.rs::a_canvas_is_added_a_screen_shows_it_and_a_mapped_projector_reads_as_one_everywhere`
  (both interfaces, 1280×760; `Win::see`, `press_add`).

- Moving screens, round 3's contract and the cue (agent A; tried only in simulation). What moves a
  screen is `CueKind::Screen` (`show-and-engine.md`); `Show::movable` is real (a screen cue names
  it) and `Show::movable_screens` answers for many at once (use it in loops, `still_scene`). Where
  screens are: `Engine::moved_screens()` -> `StageLive::send(.., projectors, screens, fresh)` (the
  app passes it) and `UiState::screens_now` (each frame, for the Screens tab's *Now at*; *Put at*
  is `Action::PutScreen(screen, place)`, places from `Show::screen_places`). `mapped::MovingScreen`
  is one per piece: `surface, mesh, mesh_key, matrix` (placement now times the piece's transform,
  as `Stage3d::scene`), `feed: Option<u8>` (0-based in `MappedFrame::feeds`, as
  `lensmap::part_feed`; None black but in the way), `region`, `front_only`. `LensMap::depth` was
  already there (round 1).

- Line-up on the page (mapping plan round 2, agent A; old plan 5.2; tried only in simulation).
  `src/ui/stage_lineup.rs`: `LineUpDraft` (`UiState::line_up`, the contract's) and the panes'
  own state `Panes` (`ProjectorsUi::lineup`: the last `Solve`, `solves` counted for tests, the
  left view's `orbit`, the drag, `want_focus`, `asking`, `offer`, `note`, `draft_dir` for tests).
  `row_buttons` (end of `stage_projectors::row`): *Line up…* (`LINE_UP`; a projector with a
  display asks first, `blanks_words`, *Line it up* / *Not now*), *Back to before line-up* while
  the placement or lens differs from `LineUp::before`, the last line-up in words. `start` (refused
  by `refused`: `SHOW_MODE_WORDS`, `LOCKED_WORDS` on a follower, `TEST_WORDS`): a new random
  session, the stored points carried on (scaled, with a note, when the picture's size changed),
  `before` = the line-up's own or where it stands. `stage_edit::page` calls `tend` first (puts a
  draft away, kept in its file and offered, in Show mode, on a follower, in Test Mode, when its
  projector or show is gone), then `panes` while there is a draft, else `offer` ("Carry on lining
  up Projector 1?", *Carry on* takes a new session, *Forget it*). `panes`: Done / Cancel / Back /
  *Its picture, dimmed* (`Under`), `link_words` and the refusal as warnings, `PANES_HINT` (only on
  a page 420 pt high or more; else the heading's hover), the two panes, the solve's words, the
  points' grid (number, screen, pixel, error, *On*, *Remove*). Left (`model_pane`): its own governed
  3D view `LINEUP_VIEW` (`PreviewId::Other("stage-lineup", 0)`: `preview_budget`, `in_sight`,
  `requests`, no lines) through the lens at the show's placement (`model_camera`; drag orbits, the
  wheel's own events (not a page's smooth scroll) dolly, double-click back), the lens frame
  letterboxed, every screen corner a dot, the points; a click (`click_model`) picks a point within
  `PICK_PT` or drops one on a screen (`render3d::pick` over the unlit parts before the projectors:
  screens only, no venue ray test on the main thread, so no venue corners), snapped to `corners`
  (each piece's mesh corners in the screen's own frame, a shared one once) within `SNAP_PT` unless
  Alt, kept by `LinePoint::on_screen` (`calib::to_screen` when free); it starts at `first_pixel`
  (the last answer's projection, else the show's; a hair off the picture clamped, far off the
  middle). Right (`picture_pane`): the picture's shape (`picture_rect`), the outline, the marks;
  drag a mark (offset kept), click to pick; the pane takes keyboard focus (`request_focus`, a focus
  lock filter for the arrows), so `ui.rs::keyboard` sees `egui_wants_keyboard_input` and leaves the
  arrows (and Space/GO, hotkeys) alone while it has it; `arrows` reads them (1, Shift 10, Alt 0.1;
  repeats count). `solve_if_due` solves only when `solve_key` changes (points key, pixels, the
  projector's placement, lens and resolution, the used screens' placement and shape), from the
  last answer's pose (`Problem::start`) with the lens as entered; `wrong_side`: every point on one
  flat screen and the answer on the side it isn't lit from (`Show::kind_of`: behind a front or LED
  one, in front of a rear one) is `Refusal::Mirrored`. The outline (`outline`: each visible
  screen's mesh boundary, chained, through `calib::project`, cut behind the lens, at most
  `MAX_OUTLINE_PIECES`; empty on a refusal) is set with `outline_rev` counted when it changes.
  *Done* (`done`): `end_burst`, placement and `lens_onto`, the `LineUp` kept (its `extra` and first
  `before` kept, `solved()`), observe, `end_burst`: one undo step. *Back* (`back_to_before`):
  placement and lens exactly `before`, `solved` cleared, points kept. The draft file
  (`DRAFT_FILE`, `lineup-draft.json` in `net::node::node_dir`, `DraftFile` with the show's path
  and name) is written (tmp + rename) whenever the serialised draft changes and removed by Done,
  Cancel, Back and *Forget it*. `stage3d/lineup_overlay.rs` (pure, the projector's side): `sprite`
  (ring, number in a built-in 5x7 figure font, Active yellow with a cross, Off grey, straight
  alpha), `sprite_transform`, `outline_picture` (lines `OUTLINE_WIDTH` per 1080 rows, clipped
  Liang-Barsky before stepping). Tests: `ui/stage_lineup_tests.rs` (whole window by mouse and
  keys at 1280x760, both interfaces: four points on a flat screen to the true pose and one undo
  step; eight on an angled faceted wall with the rung named and a bad point named and switched
  off; Cancel and Back exact; Show mode refuses and puts away; the draft survives a restart; pure:
  arrows, corners, outline, mirrored, first pixel), `lineup_overlay` tests. Not yet: `tend` called
  each pass by the app (only the page calls it), a point on a screen that isn't at home (round 3).
