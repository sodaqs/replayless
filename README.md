<div align="center">

<img src="crates/gui/assets/icon-256.png" alt="Replayless" width="128" height="128" />

# Replayless

**Reclaim hundreds of gigabytes from your NVIDIA ShadowPlay replays.**
GPU-fast HEVC re-encoding that shrinks gameplay clips **8–11×** with no perceptible quality loss — and keeps your per-game folders intact.

[![Build](https://github.com/sodaqs/replayless/actions/workflows/build.yml/badge.svg)](https://github.com/sodaqs/replayless/actions/workflows/build.yml)
[![Latest release](https://img.shields.io/github/v/release/sodaqs/replayless?display_name=tag&logo=github&label=release)](https://github.com/sodaqs/replayless/releases)
![Rust](https://img.shields.io/badge/Rust-2024-CE412B?logo=rust)
![Platform](https://img.shields.io/badge/Windows-x64-0078D6?logo=windows)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

</div>

---

## The problem

NVIDIA ShadowPlay / Instant-Replay records gameplay per game at far higher
bitrates than archived footage needs — and it piles up fast. The library this
was built for:

| | |
|---|---|
| 📁 Location | `C:\Users\<you>\Videos\NVIDIA\<Game>\*.mp4` |
| 🎬 Clips | **213** across ~85 game folders |
| 💾 Size | **~188 GB** |
| 📐 Source | H.264, mostly 2560×1440 @ 30 fps — bitrate **median ~43 Mbps, up to ~93 Mbps** + AAC |

**Replayless** re-encodes them on the GPU (NVENC, HEVC) at a quality target with a
bitrate ceiling — **mirroring the per-game folder structure** — turning that
**~188 GB into ~22–28 GB** with output that's visually transparent (VMAF ≈ 95).

## Highlights

- ⚡ **GPU-accelerated** — NVENC `hevc_nvenc` (or `av1_nvenc`) at ~4× realtime.
- 🎯 **Quality target + bitrate cap** — big wins on fat clips, no bloat on lean ones.
- ♻️ **Resumable** — `manifest.json` tracks every file, so a crash mid-188 GB never restarts from zero.
- 🗂️ **Structure-preserving** — output mirrors `<Game>\<clip>.mp4`.
- 🖥️ **Two front-ends, one core** — a `clap` CLI *and* a native desktop app.
- 🛟 **Safe by default** — nothing touches your originals unless you explicitly opt in.

## Quick start

### Desktop app (recommended)

Download the latest **`replayless.exe`** from the
[Releases](https://github.com/sodaqs/replayless/releases) page and run it: pick
your source and output folders, choose a quality preset, and hit **Start**.
ffmpeg is detected automatically (and offered as a one-click `winget` install if
missing). See the [GUI README](crates/gui/README.md).

### CLI / from source

**Requires:** Windows · an NVIDIA NVENC-capable GPU · Rust 1.95+ (edition 2024) ·
`ffmpeg` on `PATH` (or let the tool install it).

```powershell
cargo run --bin replayless -- scan                # list videos + totals (read-only)
cargo run --bin replayless -- setup               # ensure ffmpeg (winget-installs if missing)
cargo run --bin replayless -- compress --jobs 1   # transcode pending clips
cargo build -p replayless-gui --release           # build the desktop app
```

Full command and flag reference: [CLI README](crates/cli/README.md).

## Measured results

Tested on a representative **fat** clip (Alan Wake, 1304 MB @ 60.6 Mbps, 180 s),
HEVC NVENC, encode ~4× realtime. Quality is VMAF vs. source (100 = identical,
95+ = visually transparent, 90+ = very good, <80 = visible loss):

| Setting | Size | Reduction | VMAF mean | Verdict |
|---|---|---|---|---|
| **HEVC `cq30`** | 154 MB | **8.4×** | **94.6** | Near-transparent — **default** |
| HEVC `cq32` + 8 Mbps cap | 120 MB | 10.9× | 92.0 | Predictable ceiling, still very good |
| HEVC `cq34` | 93 MB | 14× | 88.9 | Visible loss — too aggressive |

> **Savings depend on source bitrate.** Fat clips (~60 Mbps) shrink ~8×; an
> already-lean ~20 Mbps clip only shrank ~1.4× at `cq30` (and AV1 `cq32` actually
> *grew* it). That's why the default pairs a quality target with a **bitrate cap**
> (`cq30` + ~12 Mbps `maxrate`): big wins on fat clips, no bloat on lean ones.

## How it works

```
 ┌─────────┐     ┌──────────────┐     ┌──────────────────┐
 │  scan   │ --> │  compress    │ --> │  (optional)      │
 │ NVIDIA\ │     │  via NVENC   │     │  delete originals│
 └─────────┘     └──────────────┘     └──────────────────┘
       │                │
       └────────────────┘
          manifest.json  (resumable state)
```

1. **Scan** — walk the source root; a folder counts as a "game" only if it
   directly contains video files (this skips NVIDIA's junk folders like `Steam`).
2. **Compress** — run ffmpeg + NVENC into `<out>\<Game>\<clip>.mp4`, skipping any
   output that already exists and is newer than its source (idempotent).
3. **Manifest** — record per-file status (`pending → compressed`) and sizes;
   every stage is restartable.
4. **Cleanup** *(opt-in)* — once compressed, optionally delete originals (off by
   default, gated behind an explicit flag).

## Reference ffmpeg command

The default encode the tool wraps (audio copied; resolution kept native).
**No `-hwaccel cuda`** — full-GPU decode errors on this RTX 5070 / ffmpeg 8.0.1
and falls back to CPU decode anyway; NVENC *encode* still runs ~4× realtime.

```sh
ffmpeg -y -i "INPUT.mp4" \
  -c:v hevc_nvenc -preset p6 -tune hq -rc vbr -cq 30 -b:v 0 \
  -maxrate 12M -bufsize 24M -vf fps=30 \
  -c:a copy -movflags +faststart "OUTPUT.mp4"
```

`-cq` is the quality dial (**lower = better/bigger**, sane 28–34); `-maxrate`
caps fat clips; `-vf fps=30` halves 60 fps gameplay (applied only when the source
is >30 fps). Every knob is exposed as a flag — see the
[CLI README](crates/cli/README.md#options).

## Project structure

A Cargo workspace: a UI-agnostic **core** with two front-ends over it. ffmpeg is
invoked as a subprocess — no libav linkage, no async runtime.

| Crate | What it is | Docs |
|---|---|---|
| **`replayless-core`** | Scan · NVENC encode · resumable manifest · ffmpeg tooling — no UI | [crates/core](crates/core/README.md) |
| **`replayless`** *(CLI)* | Thin `clap` binary with `indicatif` progress | [crates/cli](crates/cli/README.md) |
| **`replayless-gui`** | Native desktop app ([gpui](https://github.com/zed-industries/zed) + [gpui-component](https://github.com/longbridge/gpui-component)) | [crates/gui](crates/gui/README.md) |

## Configuration (`config.toml`)

```toml
source_dir = 'C:\Users\<you>\Videos\NVIDIA'
output_dir = 'C:\Users\<you>\Videos\NVIDIA_compact'
manifest   = './manifest.json'

[encode]
codec   = 'hevc'   # or 'av1' (needs higher cq to compact — untuned)
cq      = 30       # VMAF 94.6 / 8.4× on a fat clip in testing
maxrate = '12M'    # bitrate ceiling so fat clips don't stay huge
fps_cap = 30       # cap frame rate; >30 fps sources (e.g. 60) halved. 0 = off
audio   = 'copy'   # or a bitrate like '128k'
jobs    = 2        # concurrent NVENC sessions (use 1 for full runs)
# scale = '1920x1080'   # uncomment to downscale 1440p → 1080p
```

> A real `config.toml` is machine-specific and git-ignored — commit
> `config.example.toml` as a template. All fields are optional; a missing file
> falls back to built-in defaults.

## Status

CLI + GUI both work end-to-end; the compress pipeline is live-verified. See
[CHANGELOG.md](CHANGELOG.md) for shipped changes. Remaining polish:

- [ ] Per-file ffprobe size-estimate refinement
- [ ] Failed-file summary
- [ ] Preferences persistence (last folders, preset)

## License

Released under the [MIT License](LICENSE). © 2026 Vlad Korotkov.
