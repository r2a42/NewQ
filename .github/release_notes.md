## Downloads

Everything NewQ needs is in the download, including FFmpeg.

| Platform | File |
|----------|------|
| Windows 10/11 (x64) | `NewQ-{{TAG}}-windows-x64.zip` |
| Linux (x64, glibc 2.35+) | `NewQ-{{TAG}}-linux-x64.tar.gz` |
| macOS 11+ (Apple Silicon and Intel) | `NewQ-{{TAG}}-macos-universal.zip` |

**Windows:** unzip and run `newq.exe`. These builds aren't code-signed yet, so
SmartScreen may warn: choose *More info → Run anyway*.

**Linux:** extract and run `./newq`. Needs ALSA (`libasound2`) and a
Vulkan- or OpenGL-capable graphics driver.

**macOS:** unzip and move `NewQ.app` to Applications. The app isn't notarised
yet, so the first time, right-click it and choose *Open* (or run
`xattr -dr com.apple.quarantine /Applications/NewQ.app`).

`SHA256SUMS.txt` lists checksums for every file.

The bundled FFmpeg is an unmodified LGPL build: from
[BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds) on Windows and
Linux, and compiled from the official FFmpeg release source by
`scripts/build_ffmpeg_macos.sh` on macOS. See `SOURCE.txt` in the download's
`ffmpeg`/`FFmpeg` folder for the exact version and source links.
