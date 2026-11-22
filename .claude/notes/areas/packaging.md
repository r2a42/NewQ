# Packaging, signing and pins

Covers `.github/workflows/release.yml`, `scripts/package_release.sh`, `scripts/ffmpeg_pins.txt`, `scripts/mediamtx_pins.txt`, `assets/`, `build.rs`.

Moved word for word from the Layout section of `CLAUDE.md` (commit 2589061). Read this before
working in these files, and keep it true when the code changes. A heading that says "Part of"
is a piece of a longer entry that covered several areas.

- Mac signing: `release.yml` imports a Developer ID certificate into a temporary keychain when the
  secrets `MACOS_CERT_P12`, `MACOS_CERT_PASSWORD` (and, to notarise, `APPLE_API_KEY_P8`,
  `APPLE_API_KEY_ID`, `APPLE_API_ISSUER_ID`) exist; `package_release.sh` then signs ffmpeg, ffprobe
  and the app (hardened runtime, `assets/macos/entitlements.plist`), notarises with `notarytool`,
  staples and zips. Without the secrets it ad-hoc signs as before. A step's own `env` can't be used in
  its `if`, hence the `signing` step's outputs. Works: used for every release since 0.16.0 (signing,
  notarising and stapling took 83-94 s on 0.26.0 and 0.27.0, not the 30 minutes this once said).
  The whole Release workflow took 16-21 minutes on those two; the Mac job's time was its *Build* step,
  the Apple and Intel builds one after the other, which `release.yml` now builds as two parallel halves.
  When Apple's servers are down, dispatch `release.yml` itself with `tag` and `mac_unsigned: true`:
  the Mac download is ad-hoc signed and `scripts/release_notes.sh` (used by `prepare` and `publish`)
  puts the matching macOS paragraph on the page (`<!--mac-signed-->`/`<!--mac-adhoc-->` blocks in
  `.github/release_notes.md`). 0.24.2 was released that way.
- Windows signing: the `Sign newq.exe` step in `release.yml` runs only with the secrets
  `WINDOWS_CERT_PFX` and `WINDOWS_CERT_PASSWORD`. Untested, and a placeholder: new certificates
  are hardware-backed and can't be exported as a .pfx (SignPath or a cloud service is the way).

## Part of the `src/remote.rs` entry (a run of other files' notes): the logo

  `assets/logo/`: the logo (SVGs, PNGs, `newq.ico`, `newq.icns`, the raw 128 px
  window icon `src/branding.rs` embeds, and the 1280x640 GitHub social preview).
  Change the artwork in `scripts/make_logo_assets.py` and re-run it (it needs
  fontTools, node with playwright and ImageMagick; see its header), then commit
  what it writes. `build.rs` puts `newq.ico` in the Windows executable; the
  packaging script puts `newq.icns` in the Mac bundle and the icon and a
  `.desktop` file (`assets/linux/`) in the Linux download.

## Part of the `src/remote.rs` entry (a run of other files' notes): the bundled MediaMTX

  `scripts/mediamtx_pins.txt` pins the bundled version, its mirror release and SHA-256s; `package_release.sh::bundle_mediamtx`
  fetches it from this repository's mirror (a universal `lipo` binary on the Mac, signed with the others).

## CI and releases (moved from CLAUDE.md; CLAUDE.md keeps a short version)

Since 0.27.0 each system's CI job is split into tests and self-test, sharing a cache that
`warm-cache.yml` fills from `main`, and `flaky-hunt.yml` runs the suite nightly; the text below is
as it was before that.

- CI runs fmt, clippy, tests and the real-media tests on Ubuntu, Windows and
  macOS, once per change: on pull requests (a newer push cancels the older run)
  and on `main` after a merge. A rebase merge of a pull request that CI passed on
  has the same tree, so its `main` run skips the builds (the `gate` job) and still
  counts for `Cut release`; a merge after `main` moved on is tested in full.
  Slow steps are capped (`apt` once stalled for 40 minutes). A branch has no CI until it has a pull request. Changes
  that only touch `*.md` or `LICENSE-*` skip CI; `scripts/check_main_ci.sh` (used by
  `Cut release`) accepts such commits on top of a commit CI passed on. Occasionally the macOS runner never starts and the job is cancelled
  after 15 minutes with no steps run; that's not a failure of the change. Re-run it.
- Linux CI installs `mesa-vulkan-drivers` and sets `WGPU_BACKEND=vulkan` and `NEWQ_REQUIRE_GPU=1` (via
  `$GITHUB_ENV`, Linux only) for the tests and the self-test.
- **Releasing:** follow `.claude/skills/release/SKILL.md` (the whole sequence, the traps,
  and how to keep going without being poked). In short: bump `version` in `Cargo.toml` (and `Cargo.lock`) in a PR, merge,
  and wait for CI to pass on `main`. Then, as the repository owner, comment
  `/release vX.Y.Z dry-run` and then `/release vX.Y.Z` on any issue or PR. The
  `Cut release` workflow checks the tag against `Cargo.toml`, tags `main` and
  starts the `Release` workflow, which builds and publishes the downloads.
  (It can also be run from the Actions tab, or by the owner's token.)
  A suffix (`v0.3.0-rc1`) makes it a pre-release.
- **FFmpeg for Windows and Linux is pinned to one build** (`scripts/ffmpeg_pins.txt`):
  a dated BtbN build, copied into a release of this repo (`ffmpeg-<version>`,
  not a NewQ release) so it can't change or vanish upstream, and checked against
  its SHA-256 while packaging. Nothing moves it but a deliberate change: to get a
  newer FFmpeg, run `scripts/update_ffmpeg_pins.sh <BtbN autobuild tag>`, commit,
  merge, run the **Mirror FFmpeg** workflow (owner only), then a release dry run.
- **MediaMTX is mirrored the same way** (`scripts/mediamtx_pins.txt`): MediaMTX's own release builds,
  copied into a release of this repo (`mediamtx-<version>`) by the **Mirror MediaMTX** workflow
  (`scripts/mirror_mediamtx.sh`, owner only), which checks each file against the pins and the pins
  against MediaMTX's `checksums.sha256`; packaging downloads from the mirror. To move to a newer one:
  change `version`, `release` and the SHA-256s, commit, merge, run Mirror MediaMTX, then a release dry
  run. `scripts/mirror_mediamtx.sh --check` verifies without uploading.
- Cloud sessions can't push tags (the git proxy only accepts branches), which
  is why releases go through that comment.
- **Claude can't post the comment, but can run the workflow.** The GitHub
  comment tool rewrites slash commands in comments Claude posts (`/release`
  becomes `·/·r·elease`), so the workflow skips them. That looks deliberate, so
  don't work around it. The same workflow also has a manual trigger
  (`workflow_dispatch`, inputs `tag` and `dry_run`) that accepts only the
  repository owner's account. A session with the owner's token (set up with
  `/web-setup`) can start it with `actions_run_trigger` `run_workflow` on
  `cut-release.yml`. Do the dry run first, then the real one, and check the
  result: the `Release` workflow publishes the downloads.
- Large tool outputs: page GitHub list calls (`per_page` of a few).
