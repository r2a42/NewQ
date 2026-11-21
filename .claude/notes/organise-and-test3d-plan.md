# Where things live, and Test Mode with the 3D stage: a plan

Written against 0.26.0 (`9de5116`). Nothing here is built. Line numbers are that commit's.
Read from the code, not run: where a claim was not checked to the end it says "verify".

## The short version

**Organisation.** The owner is right, and the worst of it is not the menus. It is that nobody
can tell what belongs to the show and what belongs to the computer. The *Media* settings page
is five-sevenths things that have nothing to do with media (power, decoding, permissions), the
status bar sends people to "Settings ▸ General ▸ Power" where Power is not, and the timeline
tells them to turn on "Settings ▸ Show ▸ Layers", which names the sidebar's group and skips the
page (General). The fix:

- **One rule for Settings**: the top of the sidebar is *saved in the show*, the bottom is
  *this computer*. General becomes **Show**; two new pages, **Interface** and **This computer**,
  take every per-computer setting. Nothing changes in any file on disk.
- **Menus**: File, Edit, Cue, Window, **Tools**, Settings, Help. A new Tools menu holds Media
  manager, Test Mode and Statistics (today in File, Help and Window). The Displays menu goes
  (three "Add" items that add a display you cannot see). File loses "Sync…" (it is a settings
  page). The Window menu stops being four things at once.
- **Words**: *display* (a picture's destination in the show), *monitor* (the thing plugged in),
  *screen* (a surface in the 3D stage), *audio output*. "Output" alone, "3D", "previz" and
  three spellings of Test Mode go.
- **The new interface** has holes: the Window menu ticks panes it does not draw, and the cart
  has no place in any of its views. Fixed in the same pass.
- I would **not** rename Window to View, and **not** move the 3D stage out of Settings yet.
  Both cost more than they give today.

**Test Mode and the 3D stage.** They do not meet at all, and in the usual case Test Mode
quietly *removes* the 3D work: ticking monitors replaces the show's displays, so every screen
in the stage goes grey, the projectors throw nothing and a display that showed the stage is
gone. The test then says "all good" about a show it did not run. Nothing crashes and
everything is put back afterwards. The plan:

1. **Every test keeps the stage.** The screens and projectors are re-pointed at the test's
   displays, so open 3D views show the test's pictures, lit if the light is on.
2. **Every report says what the previews did**: time at each level, how often they gave way,
   whether the displays' missed pictures were theirs. This needs two small counters first.
3. **A "3D stage" tick** beside Cue list / Timeline / Network: a sample venue when the show has
   none, two projectors with the light, a turning view in the test window, a display showing
   the stage, and the same lap with previews off, so the report can say in one sentence whether
   3D costs the displays on this computer.
4. **The debug test** gets the 3D checks that mean something on a software graphics card:
   does it load, does it draw, is the light built or refused cleanly. No timings there.

Thirteen steps, about half small. Three agents can work at once if the organisation work stays
in one lane (nearly all of it is in `src/ui.rs`).

---

# Part A: organisation

## A1. What exists today

### The menu bar (`ui.rs:1938 menu_bar`, drawn in both interfaces, `ui.rs:1147`)

| Menu | Item | Leads to | Notes |
|---|---|---|---|
| File (1940) | New, Open, Save, Save as | the usual; Ctrl+N/O/S/Shift+S | |
| | Media… (1966) | the "Media" window (`media_window`, 3935) | also Settings ▸ Media ▸ "Open the media manager…" (`settings.rs:703`). Three names for one window |
| | Sync… (1969) | Settings ▸ Sync | a settings page, in File |
| | Export cue sheet… | file dialog | |
| | Quit | | |
| Edit (1991) | Undo, Redo, Copy/Cut/Paste cue, Find cues, Renumber cues | | |
| | Show mode / Edit mode… (2057) | Ctrl+Shift+L | also the transport's big button (2353) and the new strip's "Lock editing" (`modern.rs:226`): two names |
| Cue (2077) | 5 file cues, 13 other "Add … cue" | | 18 adds in one flat list |
| | New cue list, New timeline | | also the tabs' "+" (2740) |
| | Show/Hide cart (2141) | `TogglePane(Cart)` | also in Window. Does nothing in the new interface (below) |
| | Arm, Ungroup/Wrap/Move out, Duplicate, Move up/down, Delete | selected cue | also the right-click menu (`cue_context_menu`, 4304) |
| Displays (2207) | Add display, Add blended projectors, Add sub display on ▸ | `show.add_display()` and nothing else (12770) | the display appears on a settings page that is not opened. Same three adds are on the Displays page's "+ Add" (6260) and Sub displays' "+ Add" (7543) |
| Window (1715) | New interface ☐ | `Prefs::new_layout` | also the strip's "Classic interface" button (`modern.rs:211`) |
| | Statistics ☐ | a floating window (`testmode/gui.rs:132`) | a tool, between an interface switch and a pane list |
| | Cue list, Inspector, Cart, Preview (+ Preview 2…), Active, Settings, Timeline ☐ with Pop out / Dock back | the dock | in the new interface these tick a dock that is not drawn |
| | under Timeline: 2 radios (pane / workspace) + 2 radios (page / centred) (1836) | prefs | the same four radios as Settings ▸ General (`settings.rs:371`) and, for layout, the tracks' toolbar |
| | New preview, Reset layout | | also the Preview header's "+" (5405) |
| Settings (1907) | Open settings… (Ctrl+,) and every page by group | the sidebar's pages | a copy of the sidebar. Useful: one click to a page |
| Help (2227) | About NewQ… | window (3890) | |
| | Test mode… | the "Test mode" window (`testmode/gui.rs:52`) | the only way in |
| | a list of 9 keys, "Drag media files…", "N cues" | text in a menu | the README's table has about three times as many keys |

### Settings (`ui/settings.rs`; groups at 26-46, pages 50-93)

"Saved in" was read from what each section writes: `show.*` or `st.prefs.*`.

| Group | Page | Sections | Saved in |
|---|---|---|---|
| SHOW | General (368) | Timeline editor; Previews | **computer** |
| | | Show (name, file); Stop All; Layers | show |
| | | Recovery and logs (log folder, log level) | **computer** |
| | Media (679) | Files (and the media manager's button); Folders to keep on every computer | show |
| | | Streaming server; Programs; Power; Presenter remote (hidden); Decoding (716-888) | **computer**, and none of it is media |
| | Show network (`ui/network.rs`) | Cards; This computer (role); Computers; Passcode; Joining; media rows | show (cards) and **computer** (role, passcode: `node.json`) |
| PICTURE AND SOUND | Displays (`ui.rs:6251`) | each display: monitor, Shows, size, Send out, Edge blend, Test pattern | show |
| | Sub displays (7540) | areas of a display | show |
| | 3D stage (`ui/stage_edit.rs:347`) | View; Venue; Screens; Projectors; Viewpoints; Displays that show the stage (931) | show ("My projectors" on the computer) |
| | Audio outputs (5999), Sync (889) | | show |
| REMOTE CONTROL | OSC, MIDI, Web remote | | show |

Badges (`page_badge`, 139): the battery warning is on General (143), the Power setting it is
about is on Media (760).

### Panes, views and windows

| Thing | Where | How you get there |
|---|---|---|
| Classic dock: Cue list, Inspector, Cart, Preview(n), Active, Settings, Timeline (`dock.rs:13`) | `ui.rs:1614 dock_area` | Window menu; a tab's right-click "Pop out to a window" (1578) or double-click |
| Preview pane modes "Single / Multiview / 3D" (`dock.rs:159`) | header of `output_previews` (5381) | the pane's own switch; "+" for another; "Layout…" opens the Multiview layout window (5228); "Show on the display" fps tick (a computer pref, in a pane header, 5436) |
| New interface: Editor, Operator, Patch, Timeline (`modern.rs:18`), Settings as a fifth tab (`modern.rs:201`) | `modern.rs:111 strip` | Window ▸ New interface. **Patch exists only here. The cart exists only in the classic dock** (no `Cart` in `modern.rs`; popping it out to its own window from the Window menu probably still works, since pop-outs are not tied to the interface: verify) |
| Timeline workspace | `ui/timeline/workspace.rs` | Window ▸ Timeline radio, Settings ▸ General, the tracks' toolbar "Workspace", a timeline tab's Tracks button |
| 3D view | a Preview pane in "3D" mode; the top of Settings ▸ 3D stage (300 px high in a 640 px column, and it scrolls away with the page: `stage_edit.rs:349-354`, `settings.rs:268`) | the overlay's "Edit…" opens the settings page (`stage_view.rs:644`) |
| Media window, About, Statistics, Test mode, Multiview layout, Slides import, Leave show mode? | floating egui windows | File, Help, Window, Help, the Preview header, Cue menu, Edit |
| Right-click menus | cue rows (4304), dock tabs (1578), multiview tiles (4901), timeline clips/tracks (`timeline/menus.rs`) | |
| Status bar (`ui.rs:2389`) | ffmpeg, battery, graphics, network, media, awake, audio, late frames, clock, timecode | hover texts point into Settings |

### Pointers written into the program

About 25 strings say "Settings ▸ …", "Window ▸ …", "Help ▸ …"
(`git grep -n 'Settings ▸\|Help ▸\|Window ▸\|Window menu' -- src`). Two are already wrong:

- `ui.rs:2426` "Settings ▸ General ▸ Power turns this off": Power is on Media (`settings.rs:760`).
- `ui/timeline/list_tab.rs:88` "Turn on Settings ▸ Show ▸ Layers": SHOW is the sidebar's group; the page is General. Every other pointer is page ▸ section.

### Words

| Meaning | Words in use today |
|---|---|
| An `OutputConfig` | "display" (menus, settings), "output" (Preview: "Which output to preview here" 5472, multiview "Every output" 5244, the inspector's row "Output" 9304, the previews' notices "to keep the outputs smooth"), "Display N" |
| A physical monitor | "monitor" (Displays page), "screen" (Test Mode: "Tick at least one screen", `choice.rs:292`; display computers' cards "Name · Screen N") |
| A `Surface` in the 3D stage | "screen" |
| The 3D stage | "3D stage" (settings), "3D" (Preview mode), "the stage" (hints), "previz" (notes, not the UI) |
| Test Mode | "Test mode…" (menu, window title), "Test Mode" (README, reports, test shows' names) |
| The media window | "Media…", "Media", "the media manager" |
| Show mode | "Show mode / Edit mode", "SHOW MODE / EDIT MODE", "Lock editing / Unlock editing" |

## A2. The problems, by how much they hurt someone running a show

1. **Nobody can tell what travels with the show.** Per-computer settings sit on General, Media,
   Show network, in the Window menu and in a Preview pane's header, mixed with the show's. A
   designer who builds at home and opens the file at the venue finds Decoding, Power, the
   previews' limits and "may run a streaming server" back at their defaults and no page that
   says why. The "(this computer)" in section titles is the only clue and it is not on every
   one (Recovery and logs).
2. **The pointers lie.** Two today (above). Every move makes more unless they stop being
   hand-typed.
3. **Menu items that seem to do nothing.** Displays ▸ Add display adds one nobody sees. In the
   new interface Window ▸ Inspector and Cue ▸ Show cart tick and untick a dock that is not
   drawn (verify by running; `window_menu` special-cases only Timeline, 1752).
4. **Tools are in four menus.** Before a show you want: are my files here (File ▸ Media…), is
   this computer up to it (Help ▸ Test mode…), what is it doing now (Window ▸ Statistics),
   where are the logs (Settings ▸ General, at the bottom). Help is the last place anyone looks
   for the thing they should run before every show.
5. **Displays are spread over four pages and a menu.** "What does this display show" is set on
   Displays (its cues, or part of another) and at the bottom of 3D stage (the stage). A
   projector's display is chosen on a third section.
6. **The same thing has three names** (table above). The inspector says a cue goes to an
   "Output", the page that makes them is "Displays", the test asks for "screens".
7. **The Window menu is four things**: an interface switch, a tool, the panes, and four radio
   buttons of timeline preferences. The Cue menu is 18 adds, then lists, a pane toggle, arming,
   grouping and moving in one column.
8. **The 3D stage is edited through a keyhole**: a 640 px column with the view at the top that
   scrolls away as you reach Projectors. It will get worse with line-up and mapping (P4, P5 of
   `projection-plan.md`).
9. **Help is thin.** A stale short key list, a cue count, no way to the log folder.

## A3. The proposed structure

### Words (the rule for all UI text, README and CHANGELOG)

- **Display**: where pictures go in the show. Never "output" for it. The inspector's row
  becomes "Display"; the previews say "displays".
- **Monitor**: the physical thing. Test Mode says "Show the test on" and ticks monitors.
- **Screen**: only a surface in the 3D stage. (Display computers' cards keep "Screen N" for
  now: owner's decision 6.)
- **Audio output**: sound. "Output" alone is not used.
- **3D stage**: always in full. The Preview pane's modes read "One display · Multiview · 3D stage".
- **Test Mode**, **Media manager**, **Show mode** (the new strip's button says "Show mode" too).

Code names (`OutputConfig`, `OutputId`, `Slot`) do not change. Nor does any file: these are labels.

### Menus

```
File      New show · Open… · Save · Save as… | Export cue sheet… | Quit
Edit      (as now)
Cue       Add ▸ (From a file: Video, Picture, Sound, Slide show, Slides from a presentation |
                 Live: Camera, Live sound | On screen: Text, Shape, Clock, Timer |
                 Control: Fade, Stop, Wait, Control, Send, Timecode | Group)
          New cue list · New timeline | Arm · Wrap in group… · Duplicate · Move up/down · Delete
Window    Interface: ◉ Classic ○ New |
          classic: the panes with Pop out / Dock back · New preview · Reset layout
          new:     Editor · Operator · Patch · Timeline · Cart · New preview window
          Timeline opens ▸ as a pane / as a workspace
Tools     Media manager… · Test Mode… · Statistics
Settings  Open settings… (Ctrl+,) and the pages (as now, in the new groups)
Help      About NewQ… · Keyboard shortcuts… · Open the log folder
```

- **Tools is new.** Later homes: line-up (P5), "find the ceiling" (`next-ideas.md`).
- **Displays is cut.** Its three adds are already on the pages. If the owner wants a quick add
  kept, it must open Settings ▸ Displays with the new display in view.
- **Sync… leaves File.** It is in Settings twice already (sidebar and menu).
- **Show cart leaves Cue** (it is a pane: Window).
- **The follow-mode radios leave the Window menu** (Settings ▸ Interface has them; the layout
  choice stays as a two-item submenu because "where does it open" is a window question).
- **Help's key list becomes a window** drawn from one table in code; a test checks the
  README's table against it.
- The cue right-click menu is fine as it is.
- Keyboard shortcuts: **none change.**

### Settings

```
SHOW                    Show (was General) · Media · Show network
PICTURE AND SOUND       Displays · Sub displays · 3D stage · Audio outputs · Sync
REMOTE CONTROL          OSC · MIDI · Web remote
   — everything above is saved in the show file —
THIS COMPUTER           Interface · This computer
   — stays on this computer, whatever show is open —
```

| Page | Holds | Came from |
|---|---|---|
| Show | Name and file; Stop All; Layers | General |
| Media | Files; Folders to keep on every computer | Media (less five sections) |
| Interface | Classic or new; Timeline editor (opens as, follows); Previews (on/off, most a second, size, how strict; "write the rate on the display") | General; Window menu; the Preview pane's header |
| This computer | Power (with the battery badge); Decoding; Allowed here: run a streaming server, let send cues run programs; Presenter remote (still hidden); Logs and recovery | Media; General |

- Show network keeps its "This computer" section (the role is the point of the page) with one
  line: "Which computer does what is kept on each computer, not in the show."
- **Displays ▸ Shows** gains "The 3D stage, from <viewpoint>" as a choice beside "Its own
  cues" and "Part of …". The section at the bottom of 3D stage becomes a line that names those
  displays and links there. One place to answer "what does this display show".
- **3D stage**: stays a settings page for now, but the view is pinned at the top (not in the
  scroll area), the page may use the full width, and the sections become tabs under the view:
  Venue · Screens · Projectors · Viewpoints. When line-up arrives it should become a workspace
  like the timeline's; not now (decision 5).
- `SettingsPage` and `SettingsGroup` are not serialised (no `Serialize` on them,
  `settings.rs:9, 26`), so the regroup touches **no file format**: `layout.json`, prefs, shows
  and `node.json` are untouched. The cost is tests that name `SettingsPage::General` for the
  timeline settings (`ui/timeline/tests/follow.rs:369`, `tests/workspace.rs:222`), and the pointers.
- **Pointers come from one function**: `settings::path(page, Some("Power"))` gives "Settings ▸
  This computer ▸ Power"; section titles are constants the page and the pointer both use, and a
  test draws each page and checks every constant is on the page it claims.

### Both interfaces

One menu bar, same words. The Window menu shows what the interface in use can do (panes, or
views). The cart becomes reachable in the new interface (a panel of the Operator view, where
someone firing spot effects wants it; verify there is room at 1280x760), or its menu item is
hidden there. The strip's "Lock editing" becomes "Show mode". "Classic interface" stays on the
strip: it is the way home.

### What to cut or hide, in one list

Displays menu · File ▸ Sync… · Cue ▸ Show cart · the follow radios in the Window menu · Help's
key list and cue count in the menu · the fps tick in every Preview header (to Interface ▸
Previews) · "output" as a word · the Window menu's pane ticks in the new interface.

### Migration

- Files: nothing changes. No show field, no prefs field, no `layout.json` field.
- Muscle memory: three things move menus (Media…, Test mode…, Statistics). CHANGELOG gets a
  short "Where things went" table; the README's Test Mode, Media and Settings sections and the
  keyboard table are updated in the same commits as the code.
- Strings elsewhere that say "Help ▸ Test mode" (`ui.rs:11829`) and the `Settings ▸` pointers
  in `testmode/`, `net/node.rs`, `ui/network.rs`, `ui/net_media.rs` move to the helper.
- CLAUDE.md's Layout section is updated per step (it names pages and menus).

---

# Part B: Test Mode and the 3D stage

## B1. What happens today when a show with a 3D stage is tested

Every test show starts as a clone of the user's show with the cues and lists removed
(`plan.rs:330, 433, 1081`; the debug test and the network test likewise through
`use_displays_as`, `full.rs:339`, `network.rs:204`). So `Show::stage3d` (venue, screens,
projectors, viewpoints) is in the test show.

**Monitors ticked (the usual case; Start needs one whenever any is offered,
`choice.rs:288`).** `use_displays_as` (`plan.rs:261`) clears `show.outputs`, adds new displays
with **new ids** (`alloc_id` never reuses, `show.rs:3427`) and calls
`forget_missing_feeds` (`plan.rs:270`), which sets every screen's feed to None and every
projector's display to None (`stage3d/mod.rs:557-568`). `OutputConfig::source` goes with the
old displays. So during the test:

- the venue still loads and any open 3D view still draws it, with **every screen grey**;
- the light, if on, has projectors with no picture (`app.rs:4672`: `picture: None`);
- **a display that showed the stage does not exist**, so the heaviest 3D work a show can ask
  for (a full-rate 3D view on a real monitor) is not run;
- the report says nothing about any of it.

Nothing breaks. The test is simply of a lighter show than the one that will run, and says so
nowhere. That is the real gap.

**No monitor offered (a laptop alone).** `displays` is empty, `use_displays_as` returns at
once (`plan.rs:262`) and the user's displays stay, `source` included. New test cues go to the
show's first display. If that display shows the stage, its only layer is the stage's picture
(`stage_live::output_layer`), so the test's pictures are never seen on it, and a screen fed by
it is unfed (`Show::shows_stage`). Test Mode still passes: "reached the screen" means the
layer was in `Engine::layers`, not that it was drawn. **This is the one place today's code is
wrong rather than thin.** (Reasoned from `use_displays_as` and CLAUDE.md's account of
`output_layer`; not run. The first thing B4 does is a test that shows it.)

**Across a show network.** The test show carries `stage3d.venue`, so `manifest::media_paths`
lists the venue model and its side files and every follower fetches them before the test can
start (`Fleet::media_ready`, `MEDIA_LIMIT`). Harmless, slow with a big model, and unexplained.
(Read from CLAUDE.md's description of the manifest; verify in `testmode/network.rs::build`.)

**Afterwards.** `begin_test`/`end_test` (`ui.rs:566-631`) swap the whole `Show`, so feeds,
projectors' displays and `source` all come back exactly. Not handled, and to verify:

- `begin_test` does not clear `geometry_edit`, `area_edit` or `multiview_editor`. Corners
  being lined up when a test starts keep the id of a display that is gone for the test.
- `Layout::previews[..].output` (per computer) names the user's displays, missing during the
  test. `output_previews` falls back through `preview_index` (5384); check it never writes the
  fallback into the layout, or the user's pane comes back on a different display.
- `StageViewState::through` (a projector) and `StageShared::picked` survive because projectors
  are cloned with their ids. Fine.
- The venue is not reloaded on the swap as long as `venue` is unchanged (`stage_live::begin`
  compares `stage_seen`); a test that swapped in a sample venue would reload twice.

**What is measured today.** Stalls, late frames (every late frame, `render::LATE`), sound
dropouts, memory. Not: what the preview governor did (it only logs to `newq.log`,
`app.rs:3154-3160`; `take_news` is consumed there and there is no history), how long 3D frames
took (`FrameStats::cpu_time` exists per view, `render3d/mod.rs:227`, and is not kept), whether
the stage-render thread was restarted (`stage_live.rs:797`, not counted), whether the light
was refused, how long the venue took to load, or the displays' *seen* misses
(`render::shown_totals`), which is the number the governor acts on and the one the audience
sees.

## B2. Recommendations, question by question

### Should a test include the 3D stage when the show has one?

**Yes, always, without a tick**, by keeping it rather than adding to it. `use_displays_as`
should re-point instead of forget: the user's displays in order map to the test's displays in
order (more old than new: the rest map to the first; a sub display to its parent's), and
screens' feeds follow the map. Projectors follow it too, but **a display is one projector at
most** (`projector_display_refused`, `projector.rs:941`; `sanitize_projectors` enforces it on
a show sent to other computers), so the first projector that maps to a display keeps it and
any other gets none and throws no picture; the report says how many. Then whatever 3D views the user has
open show the test's pictures on the screens, lit if their light is on: the load the show
will have on this computer with this layout. A display that showed the stage is **not** kept
by this (the ticked monitors are for the test's pictures); that is the 3D part's job. With no
monitor ticked, `source` is cleared on the test show's first display so the test can be seen.

Load this adds: only what is already open. A 3D view costs feed composites (1024 px, one per
fed display), the scene at the pane's size (4x multisampled at full scale), and with the light
a 960x540 composite and a 1024 shadow map per projector (the map only when something moves).
All of it gives way through the governor, so the honest measure is not "how long did 3D take"
but **"did the displays miss pictures, and how far did the previews have to give way"**.

Measured in every test from then on:

| Measure | From | New? |
|---|---|---|
| Seen misses and new pictures per display window | `render::shown_totals()` | exists, not sampled |
| Governor: seconds at each level, steps down and up, each `News` | a tally in `preview_pace.rs` | **new** |
| 3D views drawn, displays-showing-the-stage drawn, processor time per 3D frame (mean, worst) | counters beside `StageThread::done` | **new** |
| Stage-render thread restarts; light refused; projectors past the 16 limit | `stage_live.rs`, `render3d` | **new counters** |
| Venue load time, parts, triangles | the venue-load thread | **new** |
| Previews' settings; which previews were open, their modes, light on or off | `Prefs`, `Layout::previews`, `UiState::preview_stage` | exists, not recorded |

The graphics card's own time is not measurable here (`FrameStats` says so) and should not be
claimed.

### Should Test Mode have a 3D part?

**Yes, as a fourth tick, off by default, on by default when the show has a display that shows
the stage or a projector.** `Parts::stage3d` (`#[serde(default)]`, so `Prefs::test_pick` from
0.26 reads), `Part::Stage3d` ("3D stage", slug `stage3d`), `stage` in `NEWQ_TEST_PARTS`.

What it plays, on the test's pictures (the cue-list show's first lap is enough content):

- The user's stage if it has screens; else the Proscenium sample with its starter screens,
  fed by the test's displays, and two projectors aimed at the main screen from
  `add_aimed_at`, said in the report ("your show has no 3D stage: a sample was used").
- One projector per test display (the rule above): two ticked monitors give an overlapping
  pair, one gives one projector. (Whether a display with no window can feed a throw was not
  read: `app.rs::throw_picture`, verify before promising a pair on one monitor.)
- **A view in the Test Mode window itself**, turning slowly on a scripted orbit (so the load
  does not depend on which panes the user happens to have open; it is a preview like any
  other and goes through the governor as `PreviewId::Other`).
- Legs of equal length, the same content each: **(1) previews off** (the baseline; a new
  held-by-the-test flag that `App::pace_previews` treats as it does `Prefs::previews_off`.
  `PreviewGovernor::force` is `#[cfg(test)]` only and would be undone by the next `observe`
  anyway), **(2) 3D view, light off**, **(3) light on**,
  **(4) a display shows the stage**: on the last ticked monitor when two or more are ticked;
  with one, drawn without a window and the report says it was "for the previews only, at
  their pace". Short: 45 s a leg. Medium and Long: the legs go round for the part's share of
  the time (memory growth on the card is the thing to catch there).
- The governor starts each leg afresh (a new `PreviewGovernor::reset()`: level Full, nothing
  learned), or a floor learned in one leg decides the next. The user's own governor state is
  not put back afterwards; it relearns. A Short leg is long enough to see the previews give
  way (a step a second at most) but not to see them settle: its waits run to 20 s and its
  learning to 30-60 s. So Short answers "does 3D cost the displays"; "where do the previews
  settle" is Medium's.

Pass and fail, as the report would say it:

- **All good.** "With the 3D stage open and its light on, the displays missed no more pictures
  than without it (0.1% against 0.1%). The previews drew at full rate throughout."
- **Worth knowing** (does not count against the verdict). "The previews gave way 6 times, as
  far as 15 pictures a second, and the displays stayed smooth. This computer cannot draw the
  3D view at full rate while a show plays; the show itself is not affected." · "This graphics
  card would not draw the projectors' light; the 3D view shows the pictures without it." ·
  "The venue took 41 seconds to load."
- **Problem.** "With the 3D stage open the displays missed 4% of their pictures; with previews
  off, 0.3%. On this computer the 3D view costs the show. Turn previews to Strict or Off for
  the performance (Settings ▸ Interface ▸ Previews)." · "The display that shows the stage drew
  14 pictures a second on a 60 Hz monitor." · "The 3D drawing stopped and had to be started
  again 3 times." · "Memory on the graphics card grew 400 MB over the hour with the 3D stage."

The comparison is between legs of the same run, so a slow computer is judged against itself.
Thresholds reuse the governor's own (`Strictness::share`: 1% and 5%) so the test and the
governor never disagree about what "smooth" is. With the computer's previews set to never
give way (`share()` is None) the test judges by the 5% line and says the previews were not
allowed to give way.

`projection-plan.md` P4 says mapping "must be soak-tested by Test Mode with a mapped display":
this part is where that leg goes when P4 lands.

### The governor: where its account goes

It should be in **every** test's report, not only the 3D part's: the previews are open in all
of them.

- **`report.txt`**, a part "Previews" after the findings: the settings in force; the previews
  open at the start (and that they are not re-read if the user changes panes mid-test); a
  small table of seconds at each level (Full, 60, 30, 15, 5, half size, quarter size, paused);
  steps down and up; each thing the governor said ("Previews back: the displays' missed
  pictures aren't theirs"); and one sentence of judgement from `analysis`.
- **`previews.csv`**: a row per change of level: seconds into the test, level, the worst
  display's seen misses and new pictures in the window, stalls, and the news if any.
- **`stats.csv`**: three columns at the end, `preview_level` (0 Full … 7 Paused),
  `seen_missed_permille` (worst window since the last row), `stage_ms` (worst 3D frame since
  the last row). The reader already takes a file with fewer columns ("its missing columns read
  as blanks", `stats.rs:394`). `stats::Inputs` is built in about ten places (`app.rs:2347,
  3962`, `app/play.rs:1303`, `net/measure_local.rs:80, 257`, `testmode/session.rs:2220`,
  `full.rs:707`, `decode.rs:477` and tests): add one `previews: stats::Previews` field with
  `Default` and one `Previews::now()` so each site gains a line. Verify whether `Inputs`
  crosses the network in `NetTest::Local`; if so the field is `#[serde(default)]`.
- **`events.log`**: each step, through `session::FindingLog`, so it sits in time with the cues.
- **Across a network**: each follower's tally in its `computers.csv` row. Later; the primary's
  is the one that matters first.

The tally is fed where `take_news` is already taken (`App::pace_previews`); Test Mode reads a
snapshot at the start and the end and subtracts, as it does with `crumbs::stall_count`. No
second taker.

Findings: time below Full with the displays smooth is **Worth knowing**; "not theirs" is
**Worth knowing** and names the likely causes the governor already knows (a lid, a basic
display driver); the displays missing over the line *while the previews were already paused*
is a **Problem** that says previews are not the cause.

### The debug test and CI

`selftest.rs` runs the `Runner` on an `Engine` with no window and no `Gpu`; `full.rs` runs it
inside the app. CI's Linux job installs no Vulkan driver and never sets `NEWQ_REQUIRE_GPU`
(`ci.yml` names neither), so the GPU tests there probably skip today (check a CI log for the
runner's adapters before relying on this); Windows (WARP) and the Mac should have one. Software cards make any frame time meaningless, so: **correctness only.**

| Check | Where it can run | Verdict |
|---|---|---|
| A sample venue builds and round-trips through `.glb` and `load_venue` | everywhere (pure): a `Part` "3D stage" in `feature.rs` | pass/fail |
| The test show's screens are fed and `Stage3d::scene` paints them with the display's picture; projector sums give a picture that fills its screen | everywhere (pure) | pass/fail |
| A display set to show the stage reaches the screen: the stage drew for it (`displays_done` rose, `missing_pictures == 0`) while a cue played | in the app (`full.rs`: `Ask::StageOn/StageOff`, modelled on `Ask::RecordOn`, `full.rs:504`); in `--self-test` only when `Gpu::headless()` gives a card, else "not checked: no graphics card here" | pass/fail/not checked |
| The light pipeline is built, or refused cleanly (`light_refused` set, the frame still drawn, no panic, no device loss) | same | pass; "refused" is a pass with a note |
| A picture thrown the right way round | **not here.** `stage_render_tests.rs` and `render3d/tests.rs` already read pixels back; duplicating that in the self-test buys nothing | leave to `cargo test` |

And make those `cargo test` checks real on Linux: install `mesa-vulkan-drivers` in CI, set
`WGPU_BACKEND=vulkan` and `NEWQ_REQUIRE_GPU=1` for the Linux test job so a missing adapter
fails instead of skipping. Risk: lavapipe is slow (cap the job; the 3D tests are small) and
some GPU tests may never have run there, so the first run may find real failures. That is the
point.

### Test show building and the stage

Covered in B1. The changes:

1. `use_displays_as` re-points feeds and projectors (`plan::carry_stage`, pure, tested with a
   show that has more displays than the test, fewer, sub displays, a projector on a blended
   pair's side).
2. When no monitor is ticked: clear `source` on the first display of the test show.
3. `begin_test` also ends corner and area edits and closes the multiview layout window.
4. A test that `Layout::previews` is the same after a test as before.
5. Network test: decide whether the venue goes to the followers (decision 9).
6. Nothing in the user's show needs restoring by hand: the swap does it. Keep it that way; the
   3D part must build its sample venue on the *clone*.

---

# Steps for implementation agents

Sizes: S about a day of an agent, M two to four, L about a week. Guesses.

**The collision rule.** `src/ui.rs` is 18,000 lines and nearly every Part A step is in it, so
Part A is **one lane, in order**. Part B has two lanes that stay out of `ui.rs`. At most three
agents: lane A, lane B1 (counters, app wiring), lane B2 (`testmode/`). One owner of `app.rs`
per wave, named below. Each step lands working on its own.

**Docs.** Each step adds its own line under CHANGELOG's Unreleased (append-only: a rebase
sorts it). README and CLAUDE.md: lane A owns everything except the Test Mode sections and
CLAUDE.md's `src/testmode/` paragraphs, which lane B2 owns; lane B1 hands its sentences to
lane B2. Nobody else edits those files in a wave.

**Words in `preview_pace.rs`.** Its notices are matched by tests in `ui.rs` (16272, 16281),
so lane A changes those strings (A1: only inside `notice`, `words` and `News::words`); B1
touches no string.

| Wave | Lane A (`ui.rs`, `ui/*`) | Lane B1 | Lane B2 |
|---|---|---|---|
| 1 | A1 words and pointers | B1 governor tally (**owns `app.rs`**) | B2 stage counters |
| 2 | A2 settings regroup | B3 reports (**owns `app.rs`**) | B4 tests keep the stage |
| 3 | A3 menus | B6 debug test and CI | B5 the 3D part (**owns `app.rs`**) |
| 4 | A4 displays and the stage, then A5 new interface | | (B5 continues) |
| 5 | A6 3D stage page, A7 sweep | | |

### A1. Words and pointers (S)

- Files: `ui/settings.rs` (new `path(page, section)`, section-title constants), `ui.rs`,
  `ui/modern.rs`, `dock.rs` (`PreviewMode::label`), `ui/timeline/{list_tab,toolbar,workspace}.rs`,
  `ui/network.rs`, `ui/net_media.rs`, `ui/stage_view.rs`, `net/node.rs`, and only the pointer
  and title strings in `testmode/{gui,fleet,full,network}.rs`, `choice.rs:292`.
  In `preview_pace.rs` only the strings (see the rule above).
- Delivers: the glossary applied to labels and hovers; every pointer made by the helper; the
  two wrong ones right; "Test Mode", "Media manager", "One display · Multiview · 3D stage".
- Tests: one that draws each settings page and finds every section constant on the page its
  pointer names; update tests that match on old words (`ui.rs:15941-16281`, `network_ui_tests`).
- Risk: tests that find widgets by label. Lands alone first because it touches strings in
  files the B lanes own later.

### A2. Settings regroup (M)

- Files: `ui/settings.rs`, the few callers in `ui.rs` (status bar hover 2426, Preview header
  5436, `previews_off_line` 4846), `ui/timeline/tests/{follow,workspace}.rs`.
- Delivers: Show, Interface, This computer; Media slimmed; the two captions in the sidebar and
  the Settings menu; the battery badge on This computer; the fps tick moved.
- Tests: each moved setting is on its new page and saves to the same prefs field (mouse tests
  as `ui/network_tests.rs` does); a prefs file and a show from 0.26 load unchanged (no format
  change: assert the written JSON is byte-identical before and after a round trip).
- Risk: the classic window's settings are short with a Timeline pane open, which is why the
  timeline section was first on General; keep it first on Interface.

### A3. Menus (M)

- Files: new `src/ui/menus.rs` (move `menu_bar`, `window_menu`, `settings_menu`,
  `timeline_layout_choice` out of `ui.rs`, then change them), new `src/ui/keys.rs` (the
  shortcut table and its window), `ui.rs` (calls, `UiState` flag for the window), README.
- Delivers: Tools; File and Cue trimmed, Cue ▸ Add grouped; Displays gone; Help's window.
- Tests: draw the menu bar in both interfaces and click through each top menu
  (`network_ui_tests.rs` style, 1280x760); every `Action` the old menus could push is still
  reachable from a menu or a page (a table test); the README key table equals `keys.rs`.
- Risk: moving code and changing it in one step hides mistakes: two commits, move then change.

### A4. Displays and the stage in one place (S)

- Files: `ui.rs` `displays_panel` (the "Shows" row, 6504 on), `ui/stage_edit.rs`
  `sources_section`, `ui/stage_edit_tests.rs`.
- Delivers: Displays ▸ Shows ▸ "The 3D stage, from …" using `stage_source_refused` for the
  reason it can't; the 3D stage page's section becomes a list with a link.
- Tests: mouse-set a display to the stage from the Displays page; a refused one says why.

### A5. The new interface says the same things (M)

- Files: `ui/menus.rs` (Window menu by interface), `ui/modern.rs` (cart panel in Operator,
  "Show mode"), tests in `modern.rs`.
- Delivers: no menu item that does nothing; the cart reachable; Patch named in the Window menu.
- Tests: in the new interface every Window item changes what is drawn; the cart fires a cue
  (`cart_buttons_fire_and_stop_cues` again through the Operator view).
- Risk: room in the Operator view at 1280x760.

### A6. The 3D stage page (M)

- Files: `ui/stage_edit.rs`, `ui/stage_projectors.rs`, `ui/settings.rs` (this page lays itself
  out: no 640 px cap, its own scroll), their tests.
- Delivers: the view pinned, tabs under it, the page as wide as the window allows.
- Tests: existing mouse tests re-aimed at tabs; the view's rect is the same before and after
  scrolling the tab's content.
- Risk: `stage_projectors_tests.rs` (552 lines) finds things by position. After A4 (same file).

### A7. Sweep (S)

README sections (Settings, Test Mode, Media, the 3D stage, Keyboard), CHANGELOG "Where things
went", CLAUDE.md Layout, a last `git grep` for the old words. Screenshots under xvfb of both
interfaces for the owner.

### B1. The governor keeps a tally (S)

- Files: `preview_pace.rs` (pure `Tally { secs_at: [f64; 8], down, up, news: Vec<(at, News)> }`,
  `PreviewGovernor::tally()`, counted in `observe`), `app.rs::pace_previews` (pass it on). No strings, no `ui.rs`.
- Tests: in `preview_pace.rs` with its simulated pressure: seconds add up to the time run; a
  step down and back is one of each; Off and forced are told apart.
- Risk: none to the outputs (`render::pace_tests` must still pass untouched).

### B2. The stage counts what it did (S)

- Files: `stage_render.rs` (totals as atomics: views, displays, cpu µs sum and worst, light
  refused, throws left out), `stage_live.rs` (restarts, venue load ms, parts, triangles),
  `stage_render_tests.rs`, `stage_live_tests.rs`. Public `stage_render::totals()`.
- Tests: draw twice, totals rise by two; `panic_thread` then a restart counts one; a refused
  light (`refuse_light`) is counted and the frame still lands.
- Risk: none; nothing reads them yet.

### B3. Every test reports the previews and the stage (M)

- Files: `stats.rs` (`Previews`, three columns, reader), every `Inputs` site (**`app.rs`,
  `app/play.rs`, `net/measure_local.rs`** one line each), new `testmode/previews.rs` (the
  recorder, `previews.csv`, the report part), `testmode/session.rs`, `full.rs` (sampling only),
  `analysis.rs` (`analyse_previews`), `sequence.rs` (`file_names`), `bundle.rs` if it lists files.
- Delivers: the report part, the CSV, the findings, in all tests. Needs B1 and B2.
- Tests: `analysis` on made-up tallies (smooth at Full; gave way and smooth; paused and still
  missing; not theirs); an old `stats.csv` still reads; a sequence of two parts gathers both
  `previews.csv` files; an engine-level run writes the part with "no previews open".
- Risk: `Inputs` over the network (verify, `#[serde(default)]`).

### B4. Tests keep the stage (M)

- Files: `testmode/plan.rs` (`carry_stage` inside `use_displays_as`, so no call site changes;
  the line on what the stage was pointed at goes in `Builder::notes`, which the report already
  prints, so **not** `session.rs`, which B3 owns this wave), `testmode/network.rs` only for
  decision 9, `ui.rs` **only** `begin_test` (566-603, a few lines; rebase on lane A before
  merging).
- Tests: first, one that shows today's fault (no monitor ticked, first display shows the
  stage); `plan.rs`: more, fewer and equal displays; sub displays; two projectors mapping to one
  display (one keeps it, `sanitize_projectors` changes nothing after); no
  monitor ticked clears `source` on the first display; the user's show is equal before and
  after a begin/end (extend `ui.rs:13383`'s test with a stage, feeds, a source and an open
  corner edit); `Layout::previews` unchanged.
- Risk: a re-pointed screen with a `region` made for another display's shape shows an odd crop;
  acceptable in a test, say so in the report line.

### B5. The 3D stage part (L)

- Files: `testmode/choice.rs` (`Parts::stage3d`, `Part::Stage3d`, `plan`, `summary`,
  `parse_parts`), new `testmode/stage.rs` (the script of legs, the orbit, the judge, `stage.csv`,
  the report part), `plan.rs` (`build_stage`: sample venue, projectors, the stage display),
  `session.rs`, `sequence.rs`, `gui.rs` (the tick, the view in the window, the live lines),
  **`app.rs`** (hand the test's view request to `stage_live`; the held-by-the-test flag in
  `pace_previews`), `preview_pace.rs` (`reset`, a few lines). Needs B1-B4.
- Delivers: the tick and the four legs; the sentences of B2.
- Tests: `choice.rs` (old `test_pick` reads; the default when the show has a stage display);
  the judge on made-up leg results (each sentence above from numbers); `plan.rs` (a show with
  no stage gets the sample on the clone only; with one keeps the user's); an engine-level run
  of the script at 2x as the timeline test does; GPU test (skipped without a card) that the
  window's view and the stage display both draw; mouse test of the tick at 1280x760 in both
  interfaces (`network_ui_tests.rs` pattern).
- Risk: holding the previews off from a test must be undone on every way out (`not_launched`, a
  panic, Stop): put it in the same restore path as the show. The sample venue swap reloads the
  venue twice. Never run on a real card, like the rest of the 3D stage: say so in the README.

### B6. The debug test and CI (M)

- Files: `testmode/feature.rs` (the `Part`, `Check::{VenueLoads, ScreensFed, StageDrawn,
  LightBuilt}`, `Ask::StageOn/Off`), `full.rs` (`obey`, the judge reading `stage_render::totals`),
  `selftest.rs` (the same with `Gpu::headless()` or "not checked"), `.github/workflows/ci.yml`,
  `.claude/hooks/session-start.sh` if the driver is wanted in cloud sessions. Needs B2.
- Tests: `judge` on made-up notes for each new check (the file's existing pattern, 3171 on);
  the plan test that every part is used; the self-test itself on three systems.
- Risk: the Linux job gets slower and may show GPU tests that never passed there; cap it and
  fix or mark what it finds. Shares `full.rs` with B3: B3 lands first (wave 2 before wave 3).

---

# Decisions for the owner

1. **A Tools menu** for Media manager, Test Mode and Statistics, and Test Mode out of Help.
   *Recommended: yes.* The alternative that costs least is leaving all three where they are.
2. **Cut the Displays menu.** *Recommended: yes*; or keep one item, "Add display…", that opens
   the Displays page on the new display.
3. **Settings split by "saved in the show" and "this computer"**, with General renamed Show
   and two new pages. *Recommended: yes; this is the step that matters most.*
4. **Keep the menu called Window**, not View. *Recommended: keep.* It has been Window since
   the dock; five hovers and the README say "Window ▸ Timeline".
5. **The 3D stage stays a settings page** (wider, view pinned, tabs) until line-up needs a
   workspace. *Recommended: stay.* If you already find the page unworkable, say so and A6
   becomes a workspace (L instead of M).
6. **"Screen" means a surface in the 3D stage only**; Test Mode says "monitor". And the
   display computers' cards, which say "Screen 2" on a monitor? *Recommended: Test Mode
   changes now; the cards stay until someone trips on them.*
7. **The cart in the new interface**: a panel in the Operator view, or only as a window of its
   own.
   *Recommended: the panel, if it fits at 1280x760.*
8. **The 3D part's tick**: off by default, on by itself when the show has a projector or a
   display that shows the stage. *Recommended.* Not part of every test: it adds two minutes to
   a Short test for people with no stage.
9. **Network tests and the venue model**: send it to the other computers with the test show
   (as happens today, by accident) or leave it out unless the 3D part is ticked.
   *Recommended: leave it out unless ticked; say so in the report.*
10. **Linux CI gets a software graphics card and must use it.** *Recommended: yes.* It will
    make CI a little slower and may turn up failures that have been skipping.
11. **What counts as a Problem in the 3D part**: the displays missing more pictures with 3D
    than without, by the governor's own lines (1% Strict, 5% Relaxed, whichever the computer is
    set to; 5% when it is set never to give way). Previews merely giving way is "worth
    knowing", never a Problem. *Recommended.*
