# Media and streams

Covers `src/media/`, `src/ndi.rs`, `src/pdf.rs`, `src/pptx.rs`, `src/slides_export.rs`, `src/stream.rs`, `src/mediamtx.rs`, `src/streamlink.rs`, `src/relink.rs`, `src/recovery.rs`.

Moved word for word from the Layout section of `CLAUDE.md` (commit 2589061). Read this before
working in these files, and keep it true when the code changes. A heading that says "Part of"
is a piece of a longer entry that covered several areas.

- `src/media/`: `ffmpeg.rs` (find and probe), `video.rs`, `audio.rs` (devices,
  routing, mixer), `playback.rs` (looping passes).

- Starting video and audio: FFmpeg is spawned by the reader thread (`video.rs::start`, `spawn_hap`,
  `audio.rs`), not the main loop: a process start took 3 to 4 s now and then on a Mac. A start that
  fails shows as `error()` and no frames. `Engine::update_preload` also preloads the children of
  the group that an auto-follow leads to (`MAX_PRELOAD_CHILDREN`), so a group of layers starts warm.

- `src/ndi.rs`: NDI in and out. The runtime (NDI's own library, not shipped) is
  loaded with `libloading` only inside a helper that is NewQ's own program run as
  `newq --ndi-list|--ndi-receive|--ndi-send` (see `main.rs`), so its faults can't
  take the show down; pictures cross over stdin/stdout. An NDI source is a camera
  cue's device (`ndi:` + the source's name), and `StreamKind::Ndi` is the output.
  The structures follow NDI's SDK 6 headers (not vendored: their licence);
  tests build a fake runtime in C with `cc` (skipped without one) and the pure
  converters are tested, but no real NDI has been tried.
- `src/media/hap.rs`: Hap frames (section header, Snappy, chunked); FFmpeg only
  demuxes (`-c copy`, `video.rs::spawn_hap`) and the texture data goes to
  `LayerTexture::new_bc/upload_bc` (BC1/BC3, Hap Q's YCoCg is shader mode 2). Needs
  `TEXTURE_COMPRESSION_BC` (`video::plays_as_hap`; `Gpu::new` sets the flag); a
  speed is in the frames' times. `media/convert.rs` makes Hap copies; `bench.rs` is
  `newq --bench-video` (headless; `Gpu::headless`). Tests compare our decode to
  FFmpeg's own for every format.

- `src/pdf.rs`: draws a PDF's pages as 1920-pixel PNGs with `hayro` (pure Rust, no unsafe, MIT/Apache; the
  PDF's own fonts, Foxit substitutes for the standard 14; no encrypted PDFs). A page that panics is an error
  naming it, not an abort. Compared against poppler on a LibreOffice-made deck: the same pictures.
- `src/pptx.rs` and `src/slides_export.rs`: slides from a presentation. `pptx.rs` reads the slide
  order, hidden slides and first transition (advance and fade) from the XML (pure, tested);
  `slides_export.rs` makes pictures: a PDF through `pdf.rs` (no program needed), PowerPoint (PowerShell
  COM), PowerPoint/Keynote (AppleScript), LibreOffice (makes a PDF that `pdf.rs` draws), or
  `NEWQ_SLIDE_EXPORTER`; `match_timings` pairs pictures with slides.
  `ui.rs` (`start_deck_dialog`, `start_deck_import`, `poll_deck`) runs it on a thread with Cancel and
  `Action::ImportSlides` -> `Show::add_slides_timed`. Not tried against a real office program.
  A deck with no advance times imports with `SlideShow::advance = Advance::OnGo`: the group fires only
  its first slide (`GroupState::deck`), `Engine::deck_step` fires the next or previous child (a fresh
  instance, which crossfades over the older ones: `apply_entry` only fades out layers started before it),
  and `Engine::go` steps it before it fires anything else; past the last slide the deck lets go
  (`DeckStep::AtEnd`). `ui::remote_button` is the one handler for the presenter remote's buttons and for the
  show-mode arrow keys (`keyboard`, and `app.rs::handle_output_key` for the output windows, which
  maps the winit key to egui's and calls `ui::output_key`; hotkeys in both through `ui::hotkey_cue`).

## Part of the `src/remote.rs` entry (a run of other files' notes): streams, MediaMTX, links

  `src/stream.rs`: sending a display's picture out through an FFmpeg process
  (argument building, `Sink`); `render.rs::CaptureThread` draws the picture off
  screen at the stream's rate and feeds it. A test streams to a file through
  real FFmpeg; the Linux virtual camera itself can't be tested here.
  `StreamKind::Server` (RTSP/RTMP/SRT to MediaMTX and the like) takes its H.264 encoder from
  `stream::encoder` (libx264, then VideoToolbox, NVENC, QSV, AMF: listed *and* proven by a short
  test encode, once per run), and `missing_for` checks the muxer and protocol (the Mac bundle has
  no libsrt, so no SRT). The Mac FFmpeg is built with network on and the `mpegts,rtsp,rtp,flv`
  muxers for this; `package_release.sh::verify_ffmpeg` checks them. Tests publish to FFmpeg's own
  RTSP/RTMP listeners. A hardware encoder must be checked for its keyframe spacing, not just that it
  encodes: AMF's `lowlatency` usage sent one keyframe ever (`ffprobe -show_entries packet=flags`).
  A stream's FFmpeg must never be waited on without a limit: `Sink::finish` kills it after
  `finish_patience(kind)` (3 s; a Custom output gets 60 s for closing a file), `CaptureThread::stop`
  ends it through a `Killer` when the thread is stuck writing to it (a hung server fills the pipe),
  and on Windows it is in a kill-on-close job (`mediamtx::tie_to_us`); on Linux and a Mac the
  watcher in `reaper.rs` ends it after a crash. The stress tests (`mediamtx/stress.rs`, `gpu.rs::stress_stop_…`) are
  `#[ignore]`d; run them with `NEWQ_MEDIAMTX_DIR` and `NEWQ_FFMPEG_DIR` set and `--ignored --nocapture`.
  sysinfo only fills `Process::cmd()` when asked (`ProcessRefreshKind::with_cmd`); without it a
  "no FFmpeg left" check passes for the wrong reason.
  `src/mediamtx.rs`: NewQ runs MediaMTX itself (`StreamKind::Server::host_here`): `locate` (next to the
  executable, `NEWQ_MEDIAMTX_DIR`, `third_party/mediamtx`, PATH), `Hosting::config` writes its YAML (listen
  address of the chosen adapter, per-protocol ports; publishing only from loopback and the adapter, read for all,
  `overridePublisher: no`), `Hosting::conflict` finds two ways in on one port, `Hosted` is the child process
  (killed on drop; Linux `PR_SET_PDEATHSIG`, a Windows job object, so a crash leaves none; its last lines say why it
  stopped; output appended to `<log folder>/mediamtx/mediamtx.log`). Each run has a folder named for its pid with
  `mediamtx.yml` and `server.pid`; `stop_leftovers` kills a server whose NewQ is gone (or is us) and only if the pid
  is still the MediaMTX exe. `Runner` is the worker thread (start/stop/retry every 10 s; status in `Status`).
  `plan`/`align` make every display that runs its own server agree on one adapter and port set (first wins).
  `app.rs::update_hosting` works out the plan each frame, settles a change for 1 s, refuses a missing adapter and a
  computer without `Prefs::allow_hosting`, and sets `UiState::hosting`. Real-server tests need
  `NEWQ_MEDIAMTX_DIR` (they pick their own ports; WebRTC's and RTSP-over-UDP's are settings for that).
  `src/streamlink.rs`: network adapters (`if-addrs`), the player/protocol links, the QR modules (`qrcode`),
  and `probe_server` (TCP connects; SRT is UDP so unchecked). The UI is `ui.rs::server_share_panel`.
  An adapter can't make FFmpeg bind its outgoing socket (RTSP/RTMP/SRT have no such option), so it picks the
  listen address, the publish address for a local server, and the address in the links. Tests: the real
  server and a real publish/play-back run when `NEWQ_MEDIAMTX_DIR` (or PATH) has `mediamtx`, else skip.

## Part of the `src/remote.rs` entry (a run of other files' notes): relinking media

  `src/relink.rs`: finding moved media and copying it into one folder. The venue model
  (`venue_model`: a file, never a sample) is one of the show's files: `missing_files` (cues' and the
  model; for `check_media`, the searches), `apply_venue`, `Action::RelinkVenue`/`DialogFor::LocateVenue`,
  `linked_words`; the files it reads beside it (`VenueSide`, `load::venue_files` listed again by
  `refresh_missing`'s thread only when the model changes, `UiState::venue_side`) are listed
  (`venue_missing_among`, `UiState::missing_count` for the badges) but can't be relinked;
  `collect_with_venue` copies them to their places under the model's copy, never renamed.

## Part of the `src/remote.rs` entry (a run of other files' notes): autosave copies, cameras, waveforms

  `src/recovery.rs`: autosave copies. `src/media/camera.rs`: camera cues'
  device lists and ffmpeg input: `CaptureFormat` (asked for as `-pixel_format`, `-vcodec` or `-input_format`
  per system), the device's modes (`list_modes`, run on a thread by `Engine::query_camera_modes`; parsers for
  dshow, v4l2, avfoundation and decklink output, tested on sample text only) and Blackmagic `decklink:` devices
  (listed only when the FFmpeg in use has the `decklink` input: NewQ's bundled one can't, the SDK terms stop us
  shipping one; `CameraCue::mode_code` is the `-format_code`). Automatic names the format the device offers (`resolve_format`), never waiting: `modes_known` answers from its cache and asks on a thread, the answer used from the next preload (it is in the camera's `PrepSig`). None of it has met a real capture card. A display's Send out can also go to a Blackmagic device (`StreamKind::DeckLink`: `-pix_fmt uyvy422 [-s WxH] [-r N000/1001] -f decklink <device>`, devices from `camera::list_decklink_outputs`; same FFmpeg-build limit, also untried on hardware). A display's Send out can record to a file (`StreamKind::Record`: `record_path`, `crop_filter`, `hevc_args`; H.265 encoder found by `hevc_encoder`; closing the file gets the Custom-sized `finish_patience`, and `App::exiting` stops every stream so a quit closes it; the file is made first by `reserve_record_file`, so names never collide; inputs are stamped with `-use_wallclock_as_timestamps` and the output is `-fps_mode cfr`). Its sound is a tap on an audio device (`AudioOut::tap`: `TapSlot` is try-locked in the audio callback, never blocking it; each tap has an id so a recording ending cannot clear its replacement, and a device that comes back at the same rate keeps it) that `stream::AudioPump` hands to FFmpeg over a local TCP socket as f32le (FFmpeg connects), encoded to AAC; silence fills a quiet device so FFmpeg never waits. `src/media/waveform.rs`: peaks for the
  inspector's waveform.
  The `NewQ test pattern` device stands in for a camera in tests.
