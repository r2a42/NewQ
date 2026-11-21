# Changelog

What changed in each NewQ release, newest first, in words for the people who run
shows with it. The section for a version is put at the top of that version's
release page, and a release is refused if it has none.

## Unreleased

- **Previews only give way when the displays really suffer.** They used to slow down at every
  single late frame, even one nobody could see (a 25 fps film on a 100 Hz display shows each
  picture for four refreshes), and could end up paused for a whole show. Now only a late frame
  that held back a new picture counts, judged as a share over a few seconds, and the moment after
  a GO or a jump isn't counted. They also come back sooner: a step every two smooth seconds,
  never waiting more than about 20. There is a 60 fps step before 30.
- **New preview settings** in Settings ▸ General ▸ Previews: turn the previews off altogether
  (each preview then says so, with a button to turn them back on), limit their frame rate (now
  with 60 fps too) and their resolution (Full, Half or Quarter, the 3D view included), and choose
  how soon they give way: *Relaxed* (the default, about 5% of new pictures late), *Strict* (about
  1%) or *Off* (they never give way, so on a busy computer the displays may stutter).
- **Projectors in the 3D stage.** Stand projectors in the room to plan a rig before hiring
  anything: *Settings ▸ 3D stage ▸ Projectors*. Add one aimed at a screen and it stands where its
  picture fills it; choose its lens (short throw to very long zoom, or a typed throw ratio and lens
  shift), brightness and pixels; move it and point it at a screen. Each one says how big its
  picture is on the screen, how much of the screen it fills and what spills over, about how many
  lux and nits, and how big a pixel is. *Fit to a screen* moves it or zooms it to fill one, and
  *Which lens from here?* names the lens you need. In the 3D views its light shows as lines (its
  cone and where its picture lands), you can look through its lens, and the pointer reads the
  light on any spot. Keep the projectors you use in *My projectors*. For one projector on one flat
  screen, the Displays page's four corners are still the quicker way. Worked out by sums only: not
  tried with a real projector. A show with projectors still opens in 0.25, which leaves them out
  (and drops them if it saves the show).
- **See the projectors' light in the 3D view.** Tick *Light* over a 3D view and each projector
  throws its display's picture into the room: onto the screens and the set, fading with distance
  and angle, with shadows, adding up where projectors overlap. It throws what the display really
  sends, corners and edge blend included, so you can watch your blend even out the overlap before
  you rig. *Show* switches between the *Picture* (with an automatic or hand-set exposure),
  *Brightness* (nits at full white), *Overlap* (how many projectors light each place) and *Pixel
  size*, each with a colour key. Up to 16 projectors per view; it gives way to the displays like
  every preview. Worked out by sums and drawn on a software graphics card only: not tried with real
  projectors or on a real graphics card.
- **A venue model saved again is shown again.** Export the venue's model over itself (or change
  a texture or materials file beside it) and the 3D stage reads it again a few seconds after the
  file is finished, keeping the old room on screen until the new one is in. Before, it took a
  restart. A file that can't be read is said once and tried again when it changes.
- **A still 3D view no longer has to start over.** With a 3D preview open but nothing moving,
  NewQ let go of the 3D stage after half a minute, so the next change paid the start-up again. It
  now keeps it while any 3D view is on screen or a display shows the stage.

## 0.25.0

- **The 3D stage.** *Settings ▸ 3D stage* puts your screens in a model of the venue (glTF or OBJ;
  for FBX, export it as glTF first) and shows each one with its display's picture as it plays:
  flat screens, curved walls, jumbotrons and angled LED walls, a whole picture or part of it, and
  a blended pair as the one picture it makes. Turn round it, slide and zoom with the mouse, click a
  screen to pick it, and save viewpoints. The Preview pane's *3D* mode shows it too.
  On a show network the display computers draw the stage too, and the venue model goes to them
  with the media (with its data, materials and pictures); until it has come they show the screens
  without the room.
- **Sample venues for the 3D stage.** Without a model of your own you can pick one of seven
  built-in spaces: an arena, a proscenium theatre, a thrust stage, a black box studio, a club, a
  festival main stage and a ballroom, each to real sizes in metres, with the screens such a venue
  usually has (a centre-hung scoreboard, an upstage LED wall, side screens, a cyc, a DJ wall) ready
  to add and a few places to look from.
- **A display can show the 3D stage.** Instead of its cues, a display can show the stage from a
  saved viewpoint, on its monitor and in its stream or recording, at full quality, ahead of the
  previews. A display that shows the stage is never also a picture on its screens, and renaming a
  viewpoint keeps the displays that look from it.
- **Several previews, each in its own mode.** The Preview pane's header now switches between
  *Single* (one display), *Multiview* and *3D* (the show's 3D stage). *+* in the header, or Window ▸
  *New preview*, opens another Preview beside it with its own mode and display, so you can watch one
  projector big and every display small at once; pop any of them out to a window of its own, for a
  second monitor. They are kept per computer with the window layout. If you used the *Multiview*
  button before, your Preview opens in Multiview as it did.
- **Arrange the multiview yourself.** *Layout…* beside the Preview pane's *Multiview* button picks
  how many tiles (2 to 16, as a grid or one large tile with small ones along the bottom) and which
  display each shows; drag a tile onto another to swap them, or click its name to change it. The
  layout is saved with the show. Shows without one look as they did.
- **The previews make way for the displays.** At the first sign a display is dropping frames, every
  preview in NewQ is drawn less often, then smaller, then paused if need be, and the Preview pane
  says so; they come back once the displays have been smooth for a while. A display that drops a
  frame now and then whatever the previews do (a system hiccup, a laptop lid, a screen without its
  graphics driver) no longer keeps them paused: when pausing them doesn't help, they come straight
  back and the Preview pane says the late frames aren't theirs.
  Settings ▸ General ▸ *Previews* can also keep them at a lower rate on this computer.

## 0.24.7

- **The HD colour bars are the default test pattern.** A display's *Test pattern* now starts with
  the HD bars that broadcast monitors and cameras use (SMPTE RP 219): the 75% bars, then full
  cyan, blue, yellow and red at the ends of a white strip and a grey ramp (for colours that clip,
  banding and crushed blacks or whites), and 100% white beside the black-level strips. Like the
  bars before them they are 4:3 in the middle of any display, with grey either side. The classic
  bars of 0.24.6 are still in the list as *SMPTE colour bars, classic*; a display that showed them
  shows the HD bars now until you choose the classic ones again.

## 0.24.6

- **The number on a test pattern is a setting.** A *Number* checkbox beside a display's (and sub
  display's) *Test pattern* choice turns the display's number on or off. It is on as before, and a
  show only records it when it is off.
- **SMPTE colour bars are the default test pattern.** A display's *Test pattern* now starts with
  SMPTE bars (75% bars, reversed blue row, -I/white/+Q and PLUGE strips, the display's number over
  them), always 4:3 in the middle with grey beside them on a wider display, whatever its shape:
  16:9, 16:10, an ultrawide, an edge-blended pair; a 4:3 projector or narrower is all bars. They are made for a computer's full-range picture: black
  is one black throughout, and the PLUGE strip just above black is 4%. The older *Colour bars and
  number* is still in the list, and a show that chose it keeps it; a show that never chose now
  shows the SMPTE bars. A test pattern this version doesn't know (from a later NewQ) shows the
  SMPTE bars instead of stopping the show from opening.
- **Multiview in the Preview pane.** With more than one display, the *Multiview* button above the
  preview shows all of them at once, each under its name and in its own shape; press it again for
  the single preview and its menu. Kept per computer.

## 0.24.5

- **A sound cue that plays on another computer of the show network no longer restarts over and
  over on this one.** A display computer whose show's sound output plays on the primary only
  refused the sound cue, then started it again part way at every heartbeat for as long as it
  lasted (hundreds of times a lap), and Test Mode said it played less than the primary. Now it
  keeps the cue's time in silence, as playing; a film whose sound plays elsewhere shows its
  pictures without a warning. And whatever the reason, a cue started on a computer to match the
  primary that stops again at once is not started again until the primary starts it anew.
- **NewQ says when a screen has no graphics driver (Windows).** When a graphics chip's driver is
  missing or has failed, Windows runs that screen through its basic display driver and pictures
  on it come a few times a second. NewQ now says so for that screen, with what to do: *Screen 1
  (B173HAN04.9) is run by Windows' basic display driver: install the graphics driver for this
  computer*: in the status bar and the Displays settings, on the primary's row for a display
  computer, and as a problem in Test Mode's report.
- **A display computer starts cleanly.** Opening NewQ Display on Windows showed a white box over
  part of each screen before anything else. Now a screen shows nothing until NewQ has drawn on it
  full screen: from start-up, only the computer's card (its name, screen and address), so you can
  see every projector and screen is on. The card goes when the primary plays its first cue and
  never comes back in that run: between cues the screens are black, even if a computer drops out
  of step for a moment or restarts. *Identify* now shows the computer's name, the screen's number
  and its address on black for a few seconds, then what was there comes back.
- **Films in "start all together" groups end together on every computer of a show network.** A
  film in such a group ended on each computer when that computer ran out of it: a display
  computer without the sound when the picture ran out, the computer playing the sound once it had
  played it out, which for a film whose sound runs on past its picture is a few tenths of a second
  later. What came after the group then started that much apart and was put right on screen. Now
  a film or a sound is over where the shared clock takes it to its length, on every computer.

## 0.24.4

- **A test across the show network that can't start no longer keeps your show away.** When a
  test had sent its show to every computer and then couldn't make its folder or its log (a full
  disk, a folder it may not write), your show stayed put aside: the test's show stayed open and
  locked, and the other computers measured on. Now your show comes back on every computer, the
  computers are told the test is over, and the window says why.
- **A computer that missed the end of a test stops measuring by itself.** One that didn't hear
  the end (its connection blipped then, or the primary was closed) stayed muted, under the
  network load and measuring for up to three hours. Now the primary tells it again while it
  still reports, it stops when the primary's own show comes back or the primary has been out of
  reach for 20 seconds, and a primary closed during a test tells the computers before it goes.
  A test part longer than three hours (a Long test) no longer reads as the computers dropping
  out at three hours: the primary tells them to keep measuring every minute.
- **The network test no longer reads a slow message as the screens apart.** One slow round
  trip to the primary moved "apart on the screens" by half of it for the next 20 seconds, and
  under the network load every heartbeat that waited behind other messages counted as the
  screens that far apart, so a computer in step could read "Out of step". Now the shortest
  recent round trip and the least of the last five seconds' heartbeats are used: a clock that
  is really wrong still shows.
- **A computer that could compare nothing is a problem in the report.** After a lost command a
  computer compared nothing with the primary for the rest of the test, so nothing read apart,
  and it passed. Now not being in step (or comparing nothing while the primary played) for more
  than three seconds at a stretch is a problem, and network.csv no longer counts a clock that
  had lost track (a bound of exactly one second) as one second of error.
- **Each computer's log comes back whole and in its own folder.** A computer's log goes to the
  primary in pieces, so a large one on a slow link no longer gets the computer dropped and its
  log lost; it is read correctly after the log file was put aside during the test; two
  computers of the same name get folders of their own in the zip (names in any script are
  kept); and the window says which computers' logs did not come back instead of saying all are
  inside. A test of several parts with a computer on NewQ 0.24.2 no longer drops its connection
  over and over.
- **A computer joining the show network fetches the media in the right order from the start.**
  It could hear where to fetch from before the order to fetch in, and begin in the media
  list's order.
- **The release workflow no longer publishes a Mac download as "signed and notarised" when it
  isn't.** A tagged release without the Developer ID or the notarisation key now fails instead
  of falling back to an ad-hoc signed app (a dry run still builds one ad-hoc as before).
- **Films no longer hold up the rest of NewQ while their frames go to the graphics card.**
  Each new frame (and each still, clock or test pattern) is now handed to a thread of its own
  that writes it to the graphics card, so a moment's wait there (seen on a Mac as the main loop
  held up for 100-180 ms "making the pictures", now and then at a whole minute) no longer stops
  cues, the controls and the show network meanwhile. A cue's first picture is still written at
  once, so a cut goes straight from one picture to the next, never through black, and a display
  computer's waiting card no longer blinks as its words change; the log says when the card was
  slow to take a picture. `NEWQ_UPLOADS_ON_MAIN=1` goes back to the old way.
- **Test Mode can load the show network on purpose.** *More ▸ Load the network while testing*
  (*Light* about 50 Mbit/s, *Heavy* about 400 Mbit/s, or *As fast as it goes*) sends made-up
  data both ways between the primary and each other computer for as long as a test plays
  across the show network, like a large file copy over the network during a show, without
  downloading or saving anything. The running window shows the rate with each computer; the
  report says whether it was on and how much went each way, so a run with it and one without
  can be compared. Off unless chosen; remembered on this computer.
- **A display computer restarted by its primary always comes back.** NewQ started again by the
  primary (*Restart*, a role change), by NewQ Display or at login now waits for the NewQ before
  it to close, however long that takes, instead of giving up after about half a minute with an
  *NewQ is already open* message nobody was there to read, which left the computer without NewQ.
- **Two computers with one identity are noticed.** A computer whose NewQ settings folder was
  copied from another (a cloned disk) has that computer's identity, and the others can't tell
  the two apart. Both now show the warning a second copy of NewQ on one computer already gave,
  and the log says to remove `node.json` on one of them (NewQ makes a new one).
- **Advice for Windows show computers.** The README's display computer section now says how
  to stop Windows Update installing a graphics driver in the middle of a show (it happened in
  a test: the screens blinked): pause updates before a show, and on Windows Pro the policy *Do
  not include drivers with Windows Updates*.
- **After a backup takes control, it says where the sound went and can take it over.** A
  sound output given to the old primary and another computer stayed with the old primary, so
  the new primary played none of it and nothing said so. It still stays (the old primary may
  be there with the sound system plugged in), but the new primary's status bar and *Show
  network* page now say *Sound output "…" is given to Booth, which ran the show before, not to
  this computer*, with **Play it here instead**, which gives it to the new primary in Booth's
  place. The page also says when an output is set to a sound device the new primary hasn't
  (the old primary's), which plays nothing there until another is chosen.
- **Start at login is switched off where an older NewQ had switched it on by itself.** NewQ
  0.24.0 to 0.24.2 set a computer to start at login when it was made a display computer, and
  0.24.3 kept those settings. The first start of this version removes any start-at-login entry
  an older NewQ left, once, and that computer says so for two minutes (in the status bar, and
  on a display computer's screens while they show its card): *Start at login was switched off
  (an older NewQ had switched it on by itself); tick it to have it back*. On the primary that
  computer's tick reads off. A tick you set from now on is never taken away. (A tick set by
  hand in 0.24.3 can't be told apart from one made by itself, so it goes too, once.) **After
  updating, before the next show, tick *Starts at login* again for each display computer that
  should start NewQ by itself** (on the primary's *Show network* page, or on that computer), or
  it won't come back on its own after a power cut or a restart.
- **NewQ no longer stays on the computer's software renderer after the graphics card resets.** A
  Windows laptop whose displays were rearranged mid-test had its graphics driver reset, and NewQ
  took the only thing offered at that moment, *Microsoft Basic Render Driver*, and kept it: a
  picture every 10 to 15 seconds for the rest of the show. NewQ now always picks a graphics card
  over a software renderer, uses a software one only to keep going, looks for the card again
  every few seconds and moves the displays back onto it once it is there. While it draws in
  software it says so in red in the status bar, and a display computer tells the primary: its
  row on the Show network page and the test report say "lost its graphics card and is drawing
  in software: pictures will be very slow". An output window that meets a display change in the
  middle of a frame now skips that frame instead of stopping and starting again.
- **The Preview pane no longer squeezes the picture.** A narrow pane at the edge of the
  window, or a short one, drew the display's picture out of shape (a round Earth looked like
  an egg), and changing the pane's size changed the squeeze. The preview now always keeps the
  display's own shape, as large as the pane allows, with black bars beside it or above and
  below; so do the timeline workspace's monitor, the Operator view's preview and a sub
  display's area editor. A display showing part of another's picture is previewed in that
  part's shape, no longer stretched.
- **Test Mode no longer blames NewQ's mixing for the system calling for sound late.** On a
  Mac the report said *Mixing the sound took up to 144% of the time it has* when NewQ's mixing
  took almost nothing and the system had asked for sound late. The sound's load now counts
  NewQ's mixing over a few buffers, so one call the system held up doesn't read as slow
  mixing, and a dropout that came with the system calling late says so: *the system called
  for sound late, not NewQ's mixing: another program or the sound device's power saving; a
  larger buffer may help*.
- **Cues that follow one another stay together across a show network.** A cue started by the one
  before it (auto-follow, the next cue of a "play in order" group, what follows a group or a
  timeline) now starts on every computer where the one before ended by the shared clock. Each
  computer used to start it at the first frame it drew after the end, so computers drawing at
  different rates drifted a few milliseconds further apart at every cue, until a cue was about
  26 ms off and "put right" (the network test's *in step with the primary again … moved -26 ms*
  lines). A sound or film counts as ended when the shared clock reaches its length (not once its
  sound device has played its last buffers out), and a group ends in the frame its last cue does,
  so what follows is seen in time. It is never placed more than a couple of frames back (less
  than a sound's slack for one with sound): seen later than that, after a stall or a file that
  ran on past its end, it starts from its beginning at that frame, as before, rather than skipping
  its first moments of sound and picture. Playing alone is unchanged.
- **A computer joining the show network copies the media nearest the playhead first, every time.**
  On a busy computer it could begin with the show's first file instead, and the file the show
  was about to play then waited behind it.
- **A later NewQ's new kinds of network message no longer drop the connection.** A message about
  the network test, a playback command or a heartbeat of a kind this NewQ doesn't know is now
  let pass, and so is a kind of timeline, of pause, of control action or of network adapter
  it doesn't know inside one it does, so computers running a newer NewQ from this one onwards
  stay connected to it. A show with such a timeline or pause in a control cue now opens (the
  cue does nothing) rather than being refused.

## 0.24.3

- **A picture on a timeline no longer flashes up a second copy on a display computer** when the
  timeline goes back round a loop (or jumps) and its clips take a while to be ready. While the
  timeline got them ready, the primary told the other computers that the picture it was keeping
  up meanwhile was playing on its own, and a display computer started its own copy of it for a
  couple of seconds. A display computer now never starts a timeline's clip by itself: the
  timeline does.
- **Only NewQ Display opens a display computer's screens.** Opening NewQ itself on a computer
  that has been a display computer now shows its normal window, locked, with a line saying whose
  show it belongs to and two buttons: *Open as a display computer* and *Leave the show network*.
- **Start at login is never switched on by itself.** Adding a display computer no longer sets it
  to start at login. Tick *Starts at login* on its row on the primary, or on the computer itself;
  untick it to remove it. NewQ says where the system lists it (on Windows, Task Manager ▸ Startup
  apps, now named *NewQ Display*; the old *NewQ* entry is removed). An entry 0.24 wrote on a
  display computer is renamed *NewQ Display* the next time NewQ opens there, and one that 0.24.0
  wrote (which started NewQ plainly) still starts it as a display computer.
- **Each screen's card says which screen it is**: *Assistant · Screen 1*, *Screen 2*, numbered as
  the primary numbers that computer's monitors, even when there is only one. Identify shows the
  number too.
- **A display computer that no primary has reached says so** on its card after about 20 seconds,
  and that the firewall on that computer may be blocking NewQ.
- **The network test measures what the screens show.** It compared when each cue began on each
  computer, which a command stamps with the primary's time, so a computer whose clock was wrong
  still read "1.8 ms apart". It now compares each cue's time on the screens, by the computer's
  own clock against the primary's heartbeats, and judges by that (`network.csv` keeps both:
  `shown_apart_ms` and `began_apart_ms`).
- **No more "Clock error up to 2605312 ms".** While a computer's clock isn't locked to the
  primary's, its error isn't a figure: the report says for how long it wasn't locked instead.
  *Use a wire* is no longer said to a computer that is wired.
- **Frames apart are counted at the slowest screen.** A computer with a 165 Hz and a 60 Hz screen
  had 24 ms read as 4 frames; it is counted at 60 Hz now (1.4 frames), and the report says which.
- **No false "1650 frames apart" in the timeline test.** A timeline's loop going round between
  the primary's heartbeat and the display computer's look read as the whole loop apart for a
  moment; a difference must now be seen twice in a row.
- **Each computer's log is one file in the test's zip**: `computers/<name>/newq.log`, the whole
  log since the test began (all its parts), named in the report and in the Test window. A
  display computer makes no zip of its own.
- The test's report says each computer's problem once, and its screens as "Stage-PC screen 1".
- **Only one NewQ opens on a computer.** Opening NewQ again (another copy, NewQ Display, NewQ given
  a show file) brings the NewQ that is open to the front and opens that show in it,
  asking first about unsaved changes (not while something plays). In the second test a forgotten copy left open on the
  primary answered in its name, and a display computer kept its clock with one and then the
  other: that was most of the "out of step" seen.
- **Two NewQs with one identity are said plainly.** If a computer is heard from two NewQs at
  once, its row, its own page and the computers that follow it say so in red, and the display
  computers keep their clock with the NewQ they are connected to, never the other.
- **"Adding…" no longer waits for ever.** When *Add to the show* can't connect within about 10
  seconds, the computer's row says why: each address tried (wired, Wi-Fi, a VPN such as
  Tailscale), and what went wrong. The primary now tries every address the computer was heard
  on, not just one. On a Windows computer it mentions that Windows Firewall may be blocking NewQ.
- **The clock figure is no longer nonsense.** A computer whose clock isn't settled reports *not
  locked* instead of a number (the second test showed 2,605,312 ms). A clock that turns out to
  be a long way off the primary's is put right at once instead of being chased slowly for
  minutes (or days), and what plays on that computer carries on from where it is.

## 0.24.2

- **Test mode is simpler.** One short window: a length (Short, Medium or Long, each saying how
  long it takes), ticks for what to test (*Cue list*, *Timeline*, *Network*: any of them), a tick
  per screen, the sound, and Start. Ticked together the parts run one after another as one test,
  with one result (overall first, then a section per part), one report and one zip; your show
  is put aside once and back once. *Network* is greyed, with the reason, until this computer runs
  a show network with a computer in step; at Long it is the old network soak. The debug test
  (*Check every feature*), the hardware decoding comparison and the download choice are under
  *More*. Your last choice is remembered, including the test you chose before this version.
- **Sound in a show network keeps to the shared clock.** A sound cue's time on every computer is
  now the shared clock's, like a picture's. A sound whose player or sound card started it late
  skips ahead to where it should be (once, as it starts), so two computers' sounds stay within a
  few hundredths of a second of each other. The network test said a computer was "out of step"
  by about 7 frames while a sound played: that was the sound, timed by the sound card, not the
  pictures, which were together. The test now measures pictures and sound separately.
- **A display computer never stutters for the sake of the network.** The network is a reference,
  not something a picture waits for. A small difference from the primary is left alone; a real,
  steady one is put right once, never again and again; and a computer that hears nothing from
  the primary for a while plays on by its own clock.
- **The network test catches a display that stutters.** Each computer now reports how fast each
  of its screens is drawing. A screen drawing far below its refresh rate is a problem in the
  test ("Assistant's monitor 1 drew about 4 frames a second: it should draw 64"), and the Show
  network page says so while it happens. Late frames were missed before when every frame was
  slow; they are counted now.
- **A screen that keeps the graphics card waiting tries another way of drawing.** If an output
  window waits at nearly every frame for the system (seen on a laptop's built-in screen run by
  its other graphics chip), it switches to presenting without waiting for the display, and the
  log says so. The log now also names every graphics card, and how each output window draws.
- The network test's report gave "link unknown" for a computer whose status said "Wired"; it
  now says "wired".
- **Every test on every computer.** On the computer that runs a show network, every test in
  *Help ▸ Test mode* (Short, Medium, Long, the timeline tests and the debug test) now plays on
  every computer of the network, not only the network test. Each computer measures itself
  (memory, processor, frame rate per output, stalls, sound dropouts) and how it keeps in step,
  and the result has a part per computer. While it runs, a line per computer says whether it is in
  step and how many frames a second each of its outputs draws. A test no longer passes while an
  output stutters, a computer stalls, or a computer's measurements never arrive. Waiting for a
  computer that stops answering or never gets in step ends with a plain message, and your show
  comes back everywhere.
- **Choose the screens to test, on every computer.** *Show on* lists every monitor of
  this computer and of every connected computer, grouped by computer with *All* and *None*; all
  the ticked ones are tested at once. This applies to the network test too.
- **Addresses on the Show network page.** Each computer's row shows its address and whether it
  is wired, Wi-Fi or a VPN; this computer's own addresses are listed, and a follower names its
  primary's address. An offline computer shows its last known address.
- **The local network before a VPN.** A computer with a VPN on is reached over the local
  network, not through the VPN, and the page says when a VPN is on.
- **No card between cues.** A display computer's screen with nothing playing stays black once
  the show is ready on it: the waiting card no longer comes back when a cue ends. A new setting,
  *Settings ▸ Show network ▸ Show a card while they wait*, can also show it on screens the show
  doesn't use, or never (black from start-up). It sits beside *Show transfer cards*, which moved
  there from the Media page.

## 0.24.1

- **NewQ Display.** Every download now has a second program, *NewQ Display*: open it on a
  computer with projectors or screens and it becomes a display computer at once, with nothing
  asked. Every monitor goes full screen with a card saying its name and that it is waiting for a
  NewQ primary; on the computer you run the show from it appears under *Waiting to join*, and
  *Add to the show* is the only click. Ctrl+Q (Command-Q on a Mac) quits it. `newq --display`
  does the same from the command line, and a display computer now starts at login this way.
- **Display computers come back by themselves.** One added once finds its primary again after
  either computer restarts, with no click on either. And a show remembers its computers: open it
  on another primary and the display computers it uses that are waiting are added by themselves
  (one still in another primary's show network is never taken).

## 0.24.0

- **The network test.** On the computer that runs a show network, *Help ▸ Test mode* now offers
  *Start the network test* (about 2 minutes) and *Network soak* (about 20). It sends a short show
  of its own and its media to every computer, plays it on every display of every computer (films,
  sound, a timeline with a seek, a pause), and each computer measures how closely it keeps to the
  primary: its clock, how early each GO arrived, how far apart the pictures were (in frames), the
  corrections it made, stalls. The answer is in plain words per computer ("In step: worst 0.4
  frames apart over 2 min"), with one zip to send that holds every computer's measurements and the
  end of its log. Your show comes back on every computer at the end, after Stop, and to a computer
  that dropped out once it is back. It starts with a *Sync flash* (a big frame number, a stepping
  bar and a white flash each second) to film across two projectors with a phone in slow motion.
  The README's *Show network: several computers* is now one guide, step by step.
- **Several computers, first step: the show network.** Settings ▸ Show network finds the other
  computers that are to run a show with this one and gives them roles from the one you run the
  show from (the primary): add a computer to the show, make it a display or the backup, rename it,
  make it show its name, release it. Nothing is ever typed on the other computers beyond switching
  them on (or starting them with `newq --join`). Displays can be put on another computer's monitor
  and sound outputs given to other computers. Off unless you turn it on.
- **The computers play together.** Every computer of a show network plays the whole show from its
  own copy, in step with the primary by a clock they share: GO, a cue fired, stopped or faded,
  Stop All, pause, and every timeline action (run, pause, seek, jump) happen at the same moment on
  each, a moment ahead of when you press so that every computer is ready. Edits on the primary
  reach the others a moment after you make them, without stopping what plays. A computer that
  starts late, restarts or misses something catches up by itself, part way into what is playing.
  The primary alone sends (send cues, MIDI timecode, projector control), listens (OSC, MIDI, the
  web remote, clock triggers) and records or streams. Not yet tried between two real computers:
  measured on simulated networks and on one computer.
- **Display computers.** A computer given the *Display* role has no control window: every monitor
  it has becomes an output, with a quiet card (its name, address, who runs the show and how it is
  getting on) until it is in step, and on any monitor the show doesn't use. *Identify* shows its
  name big on every output. NewQ sets itself to start at login on a display computer, and the
  primary can turn that on or off for any computer, or start NewQ again on it.
- **The other computers' controls are locked.** A computer following a primary shows the show and
  what plays, with a line saying who runs it; *Leave the show network* is the one thing it does.
- **The primary's page** says, for each computer, whether its show is in step and how far its clock
  may be off, and warns when a computer sharing a blended picture is on Wi-Fi or its clock is more
  than half a frame off.
- **Sound in a show network:** an audio output not given to any computer plays on the primary only
  (it played on every computer before).
- In a show network Test Mode offers only the network test; the other tests are off there.
- **The show's media reaches every computer by itself.** In a show network the primary sends every
  file of the show to every computer, nearest the playhead first and following the playhead as you
  jump about, then the folders you add under *Settings ▸ Media ▸ Folders to keep on every computer*
  (a library of content you might use). Rehearsal never waits: a display whose file hasn't arrived
  shows a *Media transferring* card in the cue's place (name, percent, time left), or *Media not
  found*, and the cue keeps its timing; when the file arrives the cue takes it up at the right
  moment. A file is only ever played once all of it is there and checked. The primary shows each
  computer's progress and all of them together, says in its status bar while copying runs that
  playback may be less smooth, marks cues whose file a computer still lacks, and sets each
  computer to keep versions or to replace. Cards can be turned to black for an audience.
- **Take control from the backup.** A computer given the *Backup* role has a *Take control* button
  (with one confirming click) and says when the primary has gone quiet. Pressed, the backup runs
  the show from then on: what plays carries on without a jump, every computer follows it, and the
  old primary becomes the backup when it is there or comes back. Sends, remote control, timecode
  and recordings move with it, and it sends the media from its own copy. Its show isn't saved on
  it yet, so *Save* asks where to keep it. Never automatic.
- **A primary finds its computers again after a restart**, at their last addresses, and *Forget*
  drops a computer that is gone for good.

## 0.23.1

- **Test Mode remembers the test you chose.** The window opens on the test last picked on this
  computer (a level, its timeline version, or the debug test) rather than always on Short.
- **The timeline editor's settings are easy to reach.** In the classic interface with a Timeline
  pane open, the settings page had so little room that the choice of how the view follows a
  running timeline was out of sight. The *Timeline editor* settings are now the first thing on
  Settings ▸ General, in two short rows (hover a choice for what it does), and the follow choice
  is also under *Window ▸ Timeline*, beside pane or workspace.
- **Hotkeys work while an output window has the keyboard.** A cue's hotkey did nothing if you had
  clicked on an output window; it now fires the cue there as it does in the main window, and a
  cue given 0 as its hotkey takes 0 from GO there too.
- **Shorter tooltips for cues at one place.** Hovering a Cues-track marker that stands for several
  cues gave each cue's long description, often taller than the screen. It now gives one short
  line per cue (number, name, time, what it does), at most eight lines and then "and N more";
  one cue's marker still has the whole description. Ruler flags of several markers do the same.
- **Timeline markers at the same time no longer hide each other.** Markers at one time (or too
  close to tell apart) are one flag on the ruler with a count, "×2 Chorus", as cues are on the
  Cues track. Its tooltip lists them, each click picks the next, and a drag moves the one it
  shows; the one hovered or picked is drawn whole on top.
- **A timeline group's inspector knows its Timeline pane is open.** With the tracks editor as a
  pane in the classic interface, the group's inspector offered *Open in timeline editor* even
  while the pane showed its tracks. It now says the tracks are open; if the pane is a tab hidden
  behind another, the button stays and brings it to the front.
- **A control cue's "Do" menu shows every action.** The menu scrolled, so the timeline actions
  (Run, Pause, Stop and Jump a timeline) were out of sight below the others on a smaller
  screen. It now has two columns, the actions on a cue and those on a timeline, all in sight.
- **Dragging the playhead no longer undoes a remote.** If a remote, a key, a cue or GO moved,
  ran, stopped or paused a timeline while you were dragging its playhead on the ruler, letting
  go put the playhead back where you dragged it. Now your drag gives way: its playhead goes at
  once and letting go does nothing.
- **A text or picture that runs out during a timeline pause goes.** On a timeline paused by a
  free-run pause, a text, still or shape whose own time ran out faded away but stayed in the
  active list at 0:00.0 until GO. It now leaves as soon as it has faded out; one set to hold
  stays up, in full, as before.
- **Test Mode reports sound dropouts.** Each time the sound device says it ran out of sound to
  play (a buffer underrun: a click or gap you can hear), the test notes when, counts it in the
  window and in `stats.csv`, and the report says so: tracked if it happened at all, a problem
  above four an hour or three within ten seconds. The report says *No audio dropouts* when there
  were none. The log now says, for each, the device's buffer size and whether NewQ's mixing was
  slow or the system asked for sound late; the Statistics window counts them too.
- **Test Mode reports stalls properly.** Every time the program's main loop was held up for a
  tenth of a second or more (the pictures stand still meanwhile) is listed with its time and what
  the program was doing (the show engine, making the pictures, the test's own measuring): tracked
  if there were a few, a problem for one of a quarter of a second or more, or more than twelve an
  hour. When most of them came just after a whole minute by the clock, the report says that
  something done once a minute is behind them, not the show. The log's stall warning now also
  says how long the main thread itself worked, whether the whole program was held up, which step
  of the show engine (and which cue) took the time, and the slowest picture made.
- **Test Mode no longer holds the show up while it measures.** Looking at NewQ's and
  FFmpeg's memory and processor use is done on a thread of its own, and so is writing the
  measurements file. On Windows that look sometimes took up to 3 seconds, and the show
  (and its timing) stopped meanwhile: clips came up late and laps ran long because of the
  test, not the show. The same goes for the Statistics window and `NEWQ_STATS`.
- **A quieter log.** FFmpeg's "Referenced QT chapter track not found" (said each time a
  QuickTime file with a broken chapter list opens, harmless) is no longer logged as a warning,
  nor given as the reason if such a cue fails.
- **The timeline view follows a running playhead.** While a timeline runs, its tracks
  turn the page as the playhead nears the right-hand edge, and come back to it after a
  jump, a loop going back or a scroll away. Holding a drag keeps the view still until
  you let go; stopped or paused, the view stays where you put it.
- **Or keep the playhead in the middle.** Settings ▸ General ▸ *Timeline editor* now
  chooses how the view follows a running timeline: turning the page, or keeping the
  playhead centred with the tracks creeping under it. Kept for this computer.
- **Space plays and pauses a timeline; 0 is GO.** On a timeline list's tab, or in a
  tracks editor you last clicked in, Space now pauses a running timeline and runs it on
  again (at a pause cue or in a loop it does exactly what GO does). On an ordinary cue
  list Space is GO as before, in edit and show mode alike. The 0 key (top row or keypad)
  is GO everywhere, unless a cue has 0 as its hotkey. Output windows follow the same
  rules.
- **Tidier Cues track and marker flags when zoomed out.** Cue names no longer run over
  each other: each is cut short to fit before the next cue, or left out, and hovering a
  cue shows its whole name, number, time and what it does. The picked or hovered cue's
  name is shown whole. Cues at the same time (or too close to tell apart) are one marker
  marked "×2", and each click on it picks the next of them.

## 0.23.0

- **Timeline tests in Test mode.** *Timeline short*, *Timeline medium* and *Timeline
  long* play a timeline list (and from Medium on a timeline group) made like a short
  programme from the test films and NASA pictures: butted and crossfaded films, stills
  under captions, a picture a fade cue brings up, pauses (GO is pressed for you), a
  freezing pause, a loop, a jump, a picture also on a second display, and in the long
  test heavy films, a mosaic with effects and seeks while it runs. Besides the usual
  figures they check that the timeline's clock keeps real time, that each clip comes up
  on time, how long getting clips ready takes, and that laps match.
- **Timeline workspace.** A timeline can open in a workspace of its own: the tracks
  across the whole window, the picture, a bin of new clips and the timeline's files
  and clips, and the inspector with the clip's timeline settings at the top. Choose
  it, or the pane as before, in Settings ▸ General, the Window menu or the tracks'
  toolbar; in the new interface it is the Timeline view. *Back to cue list* puts it
  away.
- **New stills, texts and shapes on a timeline last 5 seconds** wherever they are
  made (the tracks' menus, a picture dropped on a track, the workspace's bin), as
  they already did in a timeline list, instead of holding for ever.
- **A looping pause looks like one.** The section it plays again is shaded on the
  timeline's ruler and Cues track with an arrow back to its start, the cue is blue
  with 🔁, and its inspector says what a loop does instead of asking what it holds.
- **Time left of a held timeline clip.** A clip set to hold its last frame or loop
  until the timeline moves on shows, in the Active list and the Operator view, the
  time left on the timeline's clock (standing still at a pause) rather than its own.
- **Timelines from a remote.** OSC can run, pause, continue, stop (with the Stop
  cue's 3 s fade, or at once), jump to a time or a marker, and leave a loop on a
  timeline, named by its list's name or its group's cue number:
  `/timeline/Act_1/jump/Interval`, `/timeline/5/stop`.
- **The web remote's timeline transport.** Each timeline list (and a timeline group
  while it plays) gets a card with its clock, what it is doing, Run, Pause and Stop,
  Leave loop while it loops, and a button for each marker. The line above GO says
  when GO will do nothing because a timeline list is running or at its end.
- **Timelines in OSC feedback.** A controller is told each timeline's state
  (running, paused, looping, stopping, at its end…) and its clock, and `/newq/go`
  says what GO will do now.
- **Timelines on the cue sheet.** A timeline's clips have their start time, track and
  other displays in columns of their own, and its markers are listed after it.
- **Test Mode's debug test plays timelines:** two tracks, an overlap crossfade, a
  pause GO ends, a camera held by a pause, a jump to a marker, a clip waiting for
  its fade-in, and Stop cues that fade and cut.

## 0.22.0

- **Changelog.** Each release page now starts with what changed in that version.

Timelines:

- **Timeline groups.** A *Start all together* group is now a timeline you can see
  and work on: double-click it (or *Window ▸ Timeline*) to open its tracks, one row
  per display layer and sound output, with the clips as bars. Drag clips to move
  them, drag their ends to trim, add markers, and click the ruler to park the show
  at any moment with its picture on the displays. Double-clicking a timeline group
  in the cue list now opens its tracks instead of firing it (GO and *Fire* still
  start it). See *Timelines* in the README.
- **Timeline cue lists.** *Cue ▸ New timeline* (or *+ ▸ New timeline* by the list
  tabs) makes a cue list that is a timeline, with a tab of its own showing its
  tracks in the cue list's place; right-click a list's tab to make it a timeline or
  a plain list again. Files dropped on it, cues added to it and cues pasted or
  moved into it become clips at its playhead. GO runs it, carries it on at a pause
  and does nothing while it runs or once it is at its end.
- **Editing in the tracks.** Ctrl+K (Cmd+K) or *✂ Split* cuts clips in two at the
  playhead (a sound split in two may click at the join; the status bar says so).
  Delete takes clips off, and Shift+Delete (or Delete with *Ripple* on) closes the
  gap they leave. Ctrl+C, X and V copy, cut and paste clips: they are pasted at the
  playhead on the tracks they came from, as far apart as they were. Ctrl+D puts a
  copy of the picked clips just after them. Pick clips with a rubber band, trim
  several at once, and drag the square at a clip's corner to set its fade-in or
  fade-out.
- **Adding to a timeline:** right-click an empty spot of a track for a new clip
  there, drop files on a track, use the cue track's + for a pause, stop, fade, send
  or wait cue at the playhead, and *+ Track* for an empty track. Right-click a
  track's name to remove an empty one or move a picture track above or below the
  next.
- **Speed and opacity lanes under a clip:** add, drag and remove keyframes where
  the clip is, and see its length follow its speed.
- **Pause cues.** A *Pause timeline* cue stops the timeline's clock until GO. It
  can let what is playing run on (and then the next crossfade, fade or keyframe
  ramp waits for your GO), hold everything on a frame, or hold only the clips you
  tick; cameras and live sound stay live unless you say otherwise. Fades already
  under way when the clock stops wait where they are and finish after GO; a fade
  you start by hand still runs. *Run*, *Stop* and *Jump in timeline* cues go with
  it, and a clip's own Pause and Resume cues still hold just that clip.
- **Play a section again until GO.** A pause cue can loop the timeline back to a
  marker or a time and play that section round until GO; GO lets the pass finish
  and carries on past the cue (or, if you choose, leaves at once). The jump back
  isn't sample-accurate: for a bed of sound that must not break, loop the sound
  clip itself.
- **The timeline Stop cue fades what the timeline is playing out**, over 3 seconds
  unless you set another time (0 cuts); pressing Stop again during the fade cuts at
  once. The editor's Stop button does the same.
- **Overlapping clips crossfade** on a timeline made in this version: a clip that
  comes in while an earlier one on its track is still playing crossfades over it
  for as long as they overlap, and the tracks draw the crossfade. Clips that butt
  up against each other follow on without a gap of black.
- **Moving the playhead, or a Jump cue, changes the picture all at once**: what was
  up stays until everything at the new place is ready.
- **A clip set to Loop goes round again** if a pause outlasts it; *Hold* keeps its
  last frame; *End* (the default) fades it out. The editor marks a clip left to
  end with ⚠.
- **Clips at opacity 0 wait for their fade-in** on a timeline made in this version,
  so a picture that fades up later is seen from its first frame. A clip can opt out
  (*Already running when it fades in*). Older shows play as they did.
- **On a timeline, opacity, position and effect keyframes follow the timeline's
  clock:** they stand still at a pause and go on from the GO. Speed keyframes
  follow the clip.
- **The transport says what GO will do on a timeline:** "Continue …" and what it
  brings in next, "RUNNING · next pause in 0:12.0", "Finish this pass and
  continue …" while a section loops, "… is at its end" (also in the new interface
  and on the web remote).
- **The tracks editor warns** of a clip that nothing fades up (it never starts)
  and of one that cuts in over the clip under it as it fades up; sound clips show
  their waveform.

Several displays:

- **A picture on several displays.** A picture, video, text, shape or camera cue
  has an *Also on* row in its inspector to show it on other displays too; the cue
  list shows "Display 1 +2" and the tracks a "+2" badge, with a ghost of the clip
  on each other display's track. What comes in on one of those displays replaces
  it there only; its sound plays until it has gone from the last of them. Text and
  shapes are drawn at the size of the cue's own display.

Also:

- **Duplicating a group** now aims the Pause, Fade and Stop cues inside it at the
  copied cues, not at the originals.
- **A show that uses the new timeline cues (Run, Pause, Stop or Jump timeline)
  will not open in NewQ 0.20.3 or older.**

## 0.20.3

- **Nothing is left running behind NewQ (Mac and Linux).** If NewQ crashes or is
  force-quit, a small watcher ends the FFmpegs and streaming server it started. If
  the watcher goes at the same moment, the next NewQ you start ends what was left.
  Only programs NewQ itself started are ever ended.

## 0.20.2

- **Screen capture and capture devices no longer freeze on their first frame
  (Mac).** A camera cue showing the screen or an HDMI-to-USB device stood still;
  a webcam was fine. Fixed.

## 0.20.1

- **Recording to `.mkv` works on a Mac.** The bundled Mac FFmpeg could not write
  the default recording format.
- **The control window keeps drawing when an external display is switched off.**
  It could stop redrawing until the display came back.
- **Test Mode:** the full check is now the *Debug test*, chosen beside Short,
  Medium and Long. It plays a cut of a real film and pictures. Every test leaves
  one zip and nothing else.

## 0.20.0

- **A fault in one part no longer ends the show.** If a part of NewQ meets an
  error, that part is skipped for a moment, the show goes on, and the status bar
  says what happened. An output window whose drawing stops is restarted; a sound
  that fails is dropped without stopping the others.
- **Log detail is a setting** (Settings ▸ General ▸ Recovery and logs), from
  *Errors only* to *Everything*, applied at once. The log is kept to 20 MB.
- **Test Mode's full check:** one button plays every kind of cue and checks that
  each did what it should, then makes one zip to send. `newq --self-test` runs
  the same check with no window.

## 0.19.1

- The MediaMTX streaming server in the downloads now comes from a pinned copy
  kept with NewQ, checked by checksum.

## 0.19.0

- **NewQ can run the streaming server itself** (MediaMTX), with a link and QR
  code for players, a choice of network adapter and a server test.
- **Record a display's picture to a file**, with sound.
- **Blackmagic output** for a display's Send out (needs an FFmpeg with DeckLink).
- **Select, drag, copy and group several cues**, and edit several at once.
- **Layers:** cues on different layers of one display no longer replace each
  other (new shows; older shows play as they did).
- **Shape cues**, and an optional new interface with Editor, Operator and Patch
  views.
- **Group crossfades overlap** and the group keeps its length.

## 0.18.0

- **Send a display's picture to a streaming server** over RTSP, RTMP or SRT.
- **Capture cards:** ask a camera cue's device for a format, and see the modes it
  offers.
- **Blackmagic UltraStudio and DeckLink inputs** (needs an FFmpeg with DeckLink).

Earlier versions: see the commit history.
