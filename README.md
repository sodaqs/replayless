# video-uploader

A Rust CLI + GUI that takes the NVIDIA ShadowPlay/Instant-Replay recordings in
`C:\Users\<you>\Videos\NVIDIA`, **re-encodes them into compact versions** with
the GPU (NVENC), preserving the per-game folder structure.

---

## The problem

NVIDIA replays are stored per game and are enormous:

| | |
|---|---|
| Location | `C:\Users\<you>\Videos\NVIDIA\<Game Name>\*.mp4` |
| Count | **213 files** across ~85 game folders |
| Total size | **~188 GB** |
| Source format | H.264, mostly **2560×1440 @ 30 fps**; bitrate **median ~43 Mbps, up to ~93 Mbps** (sampled 54 files) + AAC audio |

Those bitrates are wildly higher than needed for archived gameplay. Re-encoding
with HEVC NVENC at a sane quality target cuts the fat clips **~8–11×** with no
perceptible loss.

### Measured sample results (2026-06-07)

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
>
> AV1 NVENC needs higher `cq` values than HEVC to compact well — left as future
> tuning; HEVC already delivers 8–11× at VMAF ≥ 92.

### Expected outcome

At `cq30`, the **~188 GB** collection should land around **~22–28 GB** (fat clips
dominate the total, so they drive most of the savings).

---

## Environment (verified on this machine)

- **GPU:** NVIDIA GeForce RTX 5070 — NVENC `av1_nvenc`, `hevc_nvenc`, `h264_nvenc` all available
- **ffmpeg:** 8.0.1 (on `PATH` as `ffmpeg`)
- **Rust:** 1.95, edition 2024
- **OS:** Windows 11

---

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

1. **Scan** — Recursively walk the source root. A folder is treated as a "game"
   only if it directly contains `.mp4` files (this naturally skips the junk
   folders NVIDIA leaves behind like `Steam`, `Program Files`, `Windowsapps`).
   Produce a work list of `(game, source_path, size)`.

2. **Compress** — For each video, run ffmpeg with NVENC into
   `<out_dir>\<Game Name>\<same-name>.mp4`. Skip if the output already exists and
   is newer than the source (idempotent / resumable). Limit concurrency
   (default **2** simultaneous NVENC sessions).

3. **Manifest** — `manifest.json` records per-file status
   (`pending → compressed`) plus source size and output size.
   Every stage is restartable; re-running only does what's left.

4. **(Optional) Cleanup** — Once files are compressed, optionally delete the
   originals (off by default, gated behind an explicit flag).

---

## Reference ffmpeg commands

These are the encode commands the tool wraps, validated by the sample test above.
Audio is copied (it's tiny next to video). Resolution is kept native by default.
Note: **no `-hwaccel cuda`** — full-GPU decode threw `CUDA_ERROR_INVALID_VALUE` on
this RTX 5070 / ffmpeg 8.0.1 and fell back to CPU decode anyway. CPU decode is
reliable and NVENC encoding still runs at ~4× realtime.

**HEVC, quality + bitrate cap (default):**
```sh
ffmpeg -y -i "INPUT.mp4" \
  -c:v hevc_nvenc -preset p6 -tune hq -rc vbr -cq 30 -b:v 0 \
  -maxrate 12M -bufsize 24M -vf fps=30 \
  -c:a copy -movflags +faststart "OUTPUT.mp4"
```
`cq30` gave VMAF 94.6 at 8.4× on a fat clip; the `maxrate` ceiling stops any
single clip from staying huge without hurting the typical case. The `-vf fps=30`
caps frame rate at 30 — applied **only when the source is >30 fps** (e.g. 60 fps
clips halve their frames for extra savings); clips already ≤30 fps skip the filter.

**HEVC, smaller (predictable ~10–11×):** drop `cq` quality and tighten the cap —
`-cq 32 -maxrate 8M -bufsize 16M` (VMAF ~92 in testing).

- `-cq` is the quality knob: **lower = higher quality / bigger file** (sane range 28–34).
- `-movflags +faststart` puts the index at the front for faster playback start.
- To shrink further, add `-vf scale=1920:1080` to downscale 1440p → 1080p.
- Re-enabling GPU decode (`-hwaccel cuda`) is a future optimization if the cuvid
  error is resolved — it would cut encode time further.

---

## CLI design

```
video-uploader <command> [options]

Commands:
  scan                 List source videos grouped by game; print totals. No changes.
  compress             Transcode pending videos into the output dir.

Global options:
  --config <path>      Config file (default: ./config.toml)
  --dry-run            Show what would happen; touch nothing.
  -v, --verbose        More logging.

Compress options:
  --codec <hevc|av1>   Default: hevc
  --cq <n>             Quality (default: 30; sane 28-34)
  --maxrate <rate>     Bitrate ceiling (default: 12M; stops fat clips staying huge)
  --fps-cap <n>        Cap frame rate (default: 30; >30 fps sources halved; 0 = off)
  --scale <WxH>        Optional downscale, e.g. 1920x1080
  --jobs <n>           Concurrent NVENC sessions (default: 2)

Cleanup (opt-in, destructive):
  --delete-source-after-compress
```

---

## Configuration (`config.toml`)

```toml
source_dir   = 'C:\Users\<you>\Videos\NVIDIA'
output_dir   = 'C:\Users\<you>\Videos\NVIDIA_compact'
manifest     = './manifest.json'

[encode]
codec        = 'hevc'   # or 'av1' (needs higher cq to compact — untuned)
cq           = 30       # VMAF 94.6 / 8.4x on a fat clip in testing
maxrate      = '12M'    # bitrate ceiling so fat clips don't stay huge
fps_cap      = 30       # cap frame rate; >30 fps sources (e.g. 60) halved. 0 = off
audio        = 'copy'   # or a bitrate like '128k'
jobs         = 2        # concurrent NVENC sessions
# scale      = '1920x1080'   # uncomment to downscale

```

> A real `config.toml` is **machine-specific** and is git-ignored. Commit
> `config.example.toml` as a template.

---

## Proposed crates

| Concern | Crate | Status |
|---|---|---|
| CLI parsing | `clap` (derive) | in use |
| Recursive scan | `walkdir` | in use |
| Config / manifest | `serde`, `serde_json`, `toml` | in use |
| Errors | `anyhow` | in use |
| Logging | `tracing`, `tracing-subscriber` | in use |
| Progress bars | `indicatif` | in use |

ffmpeg is invoked as a subprocess (`std::process::Command`); we do **not** link
libav.

---

## Module layout (realized — workspace)

```
Cargo.toml                 # [workspace] members = core, cli, gui
crates/
  core/   (lib "vu_core")  # UI-agnostic logic — no clap, no gpui
    src/
      lib.rs               # pub module surface
      config.rs            # load/validate config.toml            [done]
      manifest.rs          # load/save/update resumable state      [done]
      scan.rs              # walk source -> grouped work list       [done]
      probe.rs             # ffprobe: duration/fps/resolution        [done]
      encode.rs            # build args; spawn ffmpeg + -progress     [done]
      compress.rs          # orchestrate compress, --jobs, temp→rename [done]
      tooling.rs           # ffmpeg/ffprobe detection + winget install  [done]
      progress.rs          # Event / ProgressSink / CancelToken        [done]
  cli/    (bin "video-uploader")
    src/
      main.rs              # clap dispatch -> core                 [done]
      cli.rs               # clap definitions                       [done]
      ui.rs                # indicatif ProgressSink for the CLI      [done]
  gui/    (bin "video-uploader-gui") # gpui app -> core (channel sink)
```

---

## Desktop GUI (gpui + gpui-component)

A native desktop front-end for the **compress** pipeline, built on
[gpui](https://github.com/zed-industries/zed) (Zed's GPU-accelerated UI
framework) and [gpui-component](https://github.com/longbridge/gpui-component)
(60+ widgets: inputs, buttons, progress, virtualized tables, notifications,
modals, themes). The CLI stays fully usable — the GUI is a **second front-end
over the same core**, not a rewrite.

### What it does (v1 scope)

- Pick **input** (source) and **output** folders via the native OS folder dialog.
- **Readiness check** before Start: `ffmpeg`/`ffprobe` present (offers a `winget`
  install if missing).
- **Pre-flight summary** before anything runs: file count + **size now** +
  **estimated size after** (+ ratio).
- **Quality preset** — Balanced (`cq30`), Smaller (`cq32`+8M), Higher (`cq28`).
- **Jobs** — 1 or 2 concurrent NVENC sessions.
- **Run** with a live **overall progress bar + ETA**, a per-file detail, and a
  **Cancel** button.

### Architecture: workspace with a shared core

```
video-uploader/                # workspace root
  Cargo.toml                   # [workspace] members = ["crates/*"]
  crates/
    core/                      # compress logic — no UI, no clap, no gpui
      scan.rs  probe.rs  encode.rs  compress.rs
      manifest.rs  config.rs  tooling.rs
      progress.rs              # ProgressSink trait + Event enum + CancelToken
    cli/                       # thin clap binary -> core (indicatif sink)
      main.rs  cli.rs  ui.rs
    gui/                       # gpui app -> core (channel sink)
      main.rs  app.rs  features/  shared/
```

### ffmpeg pre-flight + winget install

`core::tooling`:
- `ffmpeg_status()` → checks `ffmpeg -version` and `ffprobe -version` on `PATH`.
- `install_ffmpeg(sink)` → runs `winget install --id Gyan.FFmpeg -e …`, streaming
  output to the sink so the GUI shows live progress.
- `ensure_ffmpeg(sink)` → check-then-install: a no-op when already on `PATH`,
  otherwise installs via winget and returns the re-resolved status.
- After install, **re-resolve** ffmpeg without an app restart.

Both front-ends use this: the GUI shows a status badge + "Install ffmpeg" button,
and the CLI exposes `video-uploader setup` plus an automatic pre-flight before
`compress` (skipped for `--dry-run`) that installs ffmpeg if it's missing and
aborts with manual-install guidance if it still isn't usable.

### Size estimation (before / estimated after)

- **Size before** is free — `scan` already has per-file sizes.
- **Estimated size after** uses a stored average ratio (**187.6 GB → 28.9 GB ≈
  6.5×** from the real run), refining per file as ffprobe durations come in.

### Background / threading bridge

Core stays **synchronous and thread-based** (no tokio). The GUI marshals it onto
gpui's executor:

```rust
let (tx, mut rx) = futures::channel::mpsc::unbounded::<Event>();
let cancel = state.cancel.clone();

std::thread::spawn(move || {
    let mut sink = ChannelSink(tx);
    vu_core::compress::run(&cfg, &ov, &mut sink, &cancel);
});

cx.spawn(async move |cx| {
    while let Some(ev) = rx.next().await {
        // update UI
    }
}).detach();
```

### Added crates (gui only — CLI/core stay lean)

| Concern | Crate |
|---|---|
| UI framework | `gpui 0.2.2` (crates.io) |
| Widgets + assets | `gpui-component 0.5.1` (crates.io) |
| Channel to UI | `futures` mpsc |
| Folder picker | native OS dialog via gpui `prompt_for_paths` (built-in) |

---

## Milestones

- [x] **M0 — Scaffold:** CLI skeleton (`clap`), config loading, logging, `scan`
      prints the work list and totals. *Done — `scan` reports 213 files / 187.6 GB
      across 29 game folders; 8 unit tests pass.*
- [x] **M1 — Compress:** ffmpeg NVENC wrapper, output dir mirroring game folders,
      skip-if-exists, progress bars, `--jobs` concurrency, manifest writes.
      *Done — verified end-to-end: 2 clips 12.2 GB → 1.4 GB (8.9×); manifest
      records status/sizes; re-run skips completed files. `--dry-run` and
      `--limit N` supported. 28 unit tests pass.*
- [x] **M6 — Core refactor:** *Done.* Split into a Cargo workspace
      (`crates/core` lib + `crates/cli` bin); added
      `progress::{Event, ProgressSink, CancelToken}`; switched encode to
      `spawn()` with `-progress` parsing, cancellation, and **temp-file → rename**
      (closes the interrupted-partial bug — a killed ffmpeg leaves only a
      `*.part.mp4`, never a "fresh" final output); re-wired the CLI onto an
      `indicatif` sink; 54 tests pass; clippy + fmt clean; single-file compress
      live-verified (9.5 MB → 2.9 MB, no leftover temp).
- [x] **M7 — GUI spike + main window:** *Done.* `crates/gui` builds on crates.io
      `gpui 0.2.2` + `gpui-component 0.5.1`; the window opens and runs on this
      machine (RTX 5070 / Win11). Main view has the ffmpeg readiness banner
      (`core::tooling`: detection + non-interactive `winget install Gyan.FFmpeg`,
      tested) and source/output folder rows with native OS folder dialog. Quality
      presets and jobs selector wired up. Live compress run with progress panel.
- [ ] **M8 — Polish:** pre-flight size estimate refinement (per-file ffprobe),
      failed-file summary, prefs persistence (last folders, preset), bundle a
      Windows `.exe`.

---

## Risks & decisions

- **NVENC session limit** — consumer GeForce historically caps concurrent NVENC
  sessions (recent drivers raise this). Default `--jobs 2`; make it configurable.
- **Codec vs. compatibility** — HEVC is the safe default; AV1 wins on size but
  some old players/previews struggle. Configurable per run.
- **Quality target** — validated: `cq30` + 12M cap = VMAF 94.6 at 8.4× on a fat
  clip. `cq` is the dial (28–34); the `maxrate` cap keeps lean clips from bloating.
  Re-sample a couple of clips if defaults change before mass-encoding 188 GB.
- **Idempotency first** — every stage must be safe to re-run; the manifest is the
  source of truth so a crash mid-188 GB never restarts from zero.
- **Destructive ops are opt-in** — nothing deletes originals unless explicitly
  flagged.
