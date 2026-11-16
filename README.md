# comprimo

**Right-click any video → comprimo → get a copy that fits under a size limit.**

*comprimo* is Latin for "I squeeze". It's a free, open-source Windows tool that you install once
and then forget about. It doesn't run in the background, has no tray icon and doesn't start with
Windows. It only shows up in the right-click menu of video files and opens a small progress window
while it works.

```
my clip.mov  (480 MB)   →   right-click → comprimo → default (20 MB)
                        →   my clip_20MB.mp4  (19.1 MB)
```

## Install

1. Download `comprimo.exe` from [Releases](https://github.com/brnbtt/comprimo/releases).
2. Double-click it, set your sizes and press **install**. No admin rights are needed.
   - It copies itself to `%LOCALAPPDATA%\Programs\comprimo`.
   - It downloads a slim ffmpeg build (about 7 MB, one time only).
   - It adds **comprimo** to the right-click menu of video files.
3. You can delete the file you downloaded.

> **Windows 11:** the entry is under **Show more options** (or hold **Shift** while right-clicking).

To uninstall, go to **Settings → Apps → Installed apps → comprimo → Uninstall**.

## The right-click menu

```
comprimo ▸  default (20 MB)
            small (10 MB)
            medium (25 MB)
            large (50 MB)
            ─────────────
            settings…
```

In **settings…** you can change the default size and add, rename, resize or remove presets (changes save automatically). Each
preset shows up in the menu as `name (size)`. Next to each entry, a small 2×2 block meter shows how big it is compared to
the others (1 block for the smallest, 4 for the largest). The menu icons follow your light/dark theme.

The output is saved next to the original as `name_20MB.mp4`. When it's done, **copy file** puts it
on the clipboard so you can paste it straight into a chat, an email or a folder.

If you select several videos, they're queued and compressed one after another.

## The progress window

Progress is shown as the file being *built*: small blocks launch from the bottom-left corner, arc
across with a little spin and stack into a 4-row grid that fills from right to left, column by
column, bottom to top. When the last block lands, a scan runs across the finished grid, popping
each column in turn, and the final size appears as it completes.

## Haptics

comprimo gives dry, tactile sound feedback while it works (you can turn it off in settings → haptics):

| Moment | Cue |
|---|---|
| Compression starts | two quick, dry taps |
| A block lands | a tiny, dry tap, slightly different every time and panned to where it lands |
| A column of 4 fills up | a slightly fuller tap, so the build has a tap-tap-tap-tock rhythm |
| Came out too big, retrying | three quick falling clicks as the extra blocks drop away |
| Done | the finishing scan: one click per column, rising in pitch from left to right, ending in a crisp snap |
| Already small enough | a solid "thunk" and a "snap", like a case closing |
| Installed / updated | a short rising run of clicks that snaps into place |
| Error / cancel | two soft dull thuds / a single low thud |
| File copied | a tiny "tick-tick" |

Every sound is synthesized in code (`src/sound.rs`): no audio files. Run `comprimo sounds` to hear them all.

## Speed

comprimo always takes the fastest route your PC has:

1. **Graphics card first.** If your PC has a hardware video encoder (NVIDIA NVENC, Intel Quick Sync
   or AMD AMF, in that order), comprimo uses it. It's several times faster than the CPU and leaves
   the CPU free.
2. **Otherwise the CPU**, using two-pass x264 at its `fast` preset: quick, and still precise about
   the size.

The graphics card is checked by test-encoding a few frames of the actual video with the actual
settings (about 0.2 s), so missing GPUs and old drivers are caught up front. If a GPU encode fails
midway, comprimo falls back to the CPU.

Measured on an i9-14900K + RTX 4080 SUPER, 1 minute of 1080p60 → 20 MB:

| Route | Time |
|---|---|
| Graphics card (NVENC) | 4.8 s |
| Processor (x264 `fast`) | 10.8 s |

Set `COMPRIMO_ENCODER=x264|nvenc|qsv|amf` to force an encoder.

## How it keeps quality as high as possible

1. **Budget:** the target size (with about 4% kept free for container overhead) is divided by
   the video's length to get a total bitrate.
2. **Audio first:** AAC at 32–160 kbps depending on the budget, and never more than the source's
   own audio bitrate.
3. **Resolution and frame rate:** a low bitrate spread over a high resolution looks blocky, so
   comprimo picks the largest size that still gets enough *bits per pixel*. It steps down
   1440p → 1080p → 720p → 540p → 480p → 360p → 240p. It keeps 60 fps down to 720p before
   dropping to 30 fps. It never upscales, and it handles portrait/rotated phone videos.
4. **H.264** (High profile, `yuv420p`, `+faststart`) so it plays everywhere: on the graphics card with
   its highest-quality multipass settings, or two-pass x264 on the CPU (see [Speed](#speed)).
5. **Verification:** if the result is still over the limit (grainy or noisy footage can throw off
   two-pass rate control), it retries. First it switches to adaptive single-pass encoding, then
   lowers the bitrate, then the resolution.

If a video simply can't fit (for example a 2-hour movie into 10 MB), it tells you instead of
producing something unwatchable.

## ffmpeg

comprimo uses only `ffmpeg.exe`. On install it downloads, in order:

1. **The slim build** from this repo's [`ffmpeg` release](https://github.com/brnbtt/comprimo/releases/tag/ffmpeg)
   (about 7.5 MB download, 19 MB on disk). It's built by [`ffmpeg-slim/build.sh`](ffmpeg-slim/build.sh) and only
   includes what comprimo needs: decoders for common formats (H.264, HEVC, AV1, VP9, ProRes, …),
   x264 + AAC encoding, the NVENC / Quick Sync / AMF hardware encoders and MP4 output.
2. **gyan.dev's essentials build** (`.7z`, about 34 MB) as a fallback.
3. **BtbN's build** (about 200 MB) as a last resort.

If you already have an ffmpeg with libx264 on your `PATH`, it's used when comprimo's own copy is
missing.

## Command line

```
comprimo                                  open setup / settings
comprimo compress [--size MB] <video>     compress (opens progress window)
comprimo install [--quiet]
comprimo uninstall [--quiet]
comprimo sounds                           play every haptic sound
```

Settings are stored in `%LOCALAPPDATA%\Programs\comprimo\config.json`:

```json
{
  "default_mb": 20.0,
  "presets": [
    { "name": "small", "mb": 10.0 },
    { "name": "medium", "mb": 25.0 },
    { "name": "large", "mb": 50.0 }
  ],
  "sounds": true
}
```

Sizes use 1 MB = 1,000,000 bytes, so they stay safely under limits that use either convention.

## Building

Requires [Rust](https://rustup.rs) (stable, MSVC toolchain).

```
cargo build --release                           # → target\release\comprimo.exe
cargo test                                      # unit tests
cargo test --release -- --ignored --nocapture   # real encodes (needs ffmpeg on PATH)
```

The slim ffmpeg is built on Linux (or WSL) with mingw-w64:

```
bash ffmpeg-slim/build.sh                       # → ffmpeg-slim/out/comprimo-ffmpeg-win64.zip
```

### Releasing

| Workflow | Trigger | Result |
|---|---|---|
| `.github/workflows/ffmpeg-slim.yml` | manual, or changes to `build.sh` | Builds the slim ffmpeg and uploads it to the `ffmpeg` release |
| `.github/workflows/release.yml` | pushing a `v*` tag | Tests, builds `comprimo.exe` and creates a release |

Run the ffmpeg workflow once before the first release, so installs can download the slim build.

### Layout

| File | Purpose |
|---|---|
| `src/main.rs` | Command-line dispatch |
| `src/encode.rs` | Planning resolution/bitrate, choosing the encoder, 2-pass encoding and retries |
| `src/gpu.rs` | GPU encoder detection and settings (NVENC, Quick Sync, AMF) |
| `src/probe.rs` | Reads duration/resolution/rotation/audio from `ffmpeg -i` |
| `src/ffmpeg.rs` | Finding or downloading ffmpeg |
| `src/install.rs` | Registry: right-click menu and Apps & Features entry |
| `src/ui.rs` | Progress window and settings window (egui) |
| `src/blocks.rs` | The block-building progress animation |
| `src/theme.rs` | Colors, fonts and custom widgets |
| `src/sound.rs` | Synthesized audio cues |
| `src/icon.rs` | The app icon (a "c" made of blocks) and the menu glyphs, drawn in code |
| `build.rs` | Embeds the icon and version info in the exe |
| `src/win.rs` | Win32 helpers (clipboard, message boxes, job object, blocking game overlays) |

## License

MIT. The ffmpeg build that comprimo downloads is licensed under the GPL (it includes x264); see the
`README.txt` and `LICENSE.txt` inside the download.
