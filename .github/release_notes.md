## Downloads

| Platform | File | FFmpeg |
|----------|------|--------|
| Windows 10/11 (x64) | `NewQ-{{TAG}}-windows-x64.zip` | Included |
| Linux (x64, glibc 2.35+) | `NewQ-{{TAG}}-linux-x64.tar.gz` | Included |
| macOS 11+ (Apple Silicon and Intel) | `NewQ-{{TAG}}-macos-universal.zip` | Install with `brew install ffmpeg` |

**Windows:** unzip and run `newq.exe`. These builds aren't code-signed yet, so
SmartScreen may warn: choose *More info → Run anyway*.

**Linux:** extract and run `./newq`. Needs ALSA (`libasound2`) and a
Vulkan- or OpenGL-capable graphics driver.

**macOS:** unzip, move `NewQ.app` to Applications, and install FFmpeg with
[Homebrew](https://brew.sh): `brew install ffmpeg`. The app isn't signed or
notarised yet, so the first time, right-click it and choose *Open* (or run
`xattr -dr com.apple.quarantine /Applications/NewQ.app`).

`SHA256SUMS.txt` lists checksums for every file.

The bundled FFmpeg is an unmodified LGPL build from
[BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds); see
`ffmpeg/SOURCE.txt` in the download for version and source details.
