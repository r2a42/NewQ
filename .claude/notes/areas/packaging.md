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
  its `if`, hence the `signing` step's outputs. Works: used for every release since 0.16.0 (the
  notarisation wait is about 30 minutes).
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
