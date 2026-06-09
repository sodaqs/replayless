# video-uploader

A Rust CLI that takes the NVIDIA ShadowPlay/Instant-Replay recordings in
`C:\Users\fruit\Videos\NVIDIA`, **re-encodes them into compact versions** with
the GPU (NVENC), and **uploads them to Google Drive**, preserving the per-game
folder structure.

---

## The problem

NVIDIA replays are stored per game and are enormous:

| | |
|---|---|
| Location | `C:\Users\fruit\Videos\NVIDIA\<Game Name>\*.mp4` |
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

At `cq30`, the **~188 GB** collection should land around **~22–28 GB** before
upload (fat clips dominate the total, so they drive most of the savings).

---

## Environment (verified on this machine)

- **GPU:** NVIDIA GeForce RTX 5070 — NVENC `av1_nvenc`, `hevc_nvenc`, `h264_nvenc` all available
- **ffmpeg:** 8.0.1 (on `PATH` as `ffmpeg`)
- **Rust:** 1.95, edition 2024
- **OS:** Windows 11

---

## How it works (pipeline)

```
 ┌─────────┐     ┌──────────────┐     ┌────────────┐     ┌──────────────┐
 │  scan   │ --> │  compress    │ --> │  upload    │ --> │  (optional)  │
 │ NVIDIA\ │     │  via NVENC   │     │ to Drive   │     │  delete src  │
 └─────────┘     └──────────────┘     └────────────┘     └──────────────┘
       │                │                   │
       └────────────────┴───────────────────┘
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

3. **Upload** — Push each compact file to Google Drive under a root folder
   (e.g. `NVIDIA Replays`), creating one sub-folder per game. Use **resumable
   uploads** so large files survive flaky connections. Mark uploaded in the
   manifest.

4. **Manifest** — `manifest.json` records per-file status
   (`pending → compressed → uploaded`) plus the source hash/mtime/size and the
   Drive file id. Every stage is restartable; re-running only does what's left.

5. **(Optional) Cleanup** — Once a file is verified uploaded, optionally delete
   the local compact copy and/or the original (off by default, gated behind an
   explicit flag).

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
- `-movflags +faststart` puts the index at the front so Drive can stream-preview.
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
  upload               Upload compressed videos to Google Drive.
  run                  Full pipeline: scan -> compress -> upload.
  status               Show manifest progress (counts, sizes, what's left).
  auth                 Run the Google OAuth flow and cache the token.

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
  --delete-compressed-after-upload
  --delete-source-after-upload
```

---

## Configuration (`config.toml`)

```toml
source_dir   = 'C:\Users\fruit\Videos\NVIDIA'
output_dir   = 'C:\Users\fruit\Videos\NVIDIA_compact'
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

Google Drive **auth lives in `.env`**, not `config.toml` (see below):

```dotenv
# .env  (git-ignored; copy from .env.example)
GOOGLE_CLIENT_ID=xxxx.apps.googleusercontent.com
GOOGLE_CLIENT_SECRET=xxxx
GOOGLE_REFRESH_TOKEN=          # written automatically by `auth`
DRIVE_ROOT_FOLDER=NVIDIA Replays
```

> A real `config.toml` and `.env` are **secrets / machine-specific** and are
> git-ignored. Commit `config.example.toml` and `.env.example` as templates.

---

## Google Drive auth (via `.env`)

**Why OAuth (not a service account):** the target is your *personal* Google Drive,
and service accounts can't write to personal Drive (they have separate 0-quota
storage). So we authenticate **as you** with OAuth 2.0 and keep the secrets in
`.env`.

**One-time Google Cloud setup:**
1. Create a project at <https://console.cloud.google.com/>.
2. **Enable** the *Google Drive API*.
3. Configure the OAuth consent screen (External; add your Google account as a test
   user).
4. Create an **OAuth client ID → Desktop app**. Copy its **client ID + secret**
   into `.env` (`GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET`).

**Get a refresh token:** run `video-uploader auth`. It starts a localhost loopback
server, opens your browser for consent (scope `drive.file`, `access_type=offline`,
PKCE), catches the redirect, exchanges the code, and **writes `GOOGLE_REFRESH_TOKEN`
back into `.env`** automatically. (Loopback — not the deprecated copy-paste/OOB
flow.)

**Scope:** only `https://www.googleapis.com/auth/drive.file` — the app can see and
manage **only files it creates** (least privilege; it never touches the rest of
your Drive).

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
| `.env` loading | `dotenvy` | M2 |
| HTTP (Drive REST) | `reqwest` (**blocking**, rustls) | M2/M3 |
| Loopback consent | `tiny_http` + `webbrowser` | M2 |
| PKCE | `sha2` + `base64` | M2 |

> **Synchronous by choice:** uploads run sequentially with blocking `reqwest`
> (one file at a time — simplest, gentle on Drive's rate limits, no tokio). This
> matches the thread-based compress stage. We hand-roll the OAuth refresh + the
> resumable-upload protocol rather than pulling in the heavier `google-drive3` /
> `yup-oauth2`, which keeps full control over chunking and retries for the large
> files here.

ffmpeg is invoked as a subprocess (`std::process::Command`); we do **not** link
libav.

---

## Module layout (realized — workspace as of M6)

```
Cargo.toml                 # [workspace] members = core, cli
crates/
  core/  (lib "vu_core")   # UI-agnostic logic — no clap, no gpui
    src/
      lib.rs               # pub module surface
      config.rs            # load/validate config.toml            [done]
      manifest.rs          # load/save/update resumable state      [done]
      scan.rs              # walk source -> grouped work list       [done]
      probe.rs             # ffprobe: duration/fps/resolution        [done]
      encode.rs            # build args; spawn ffmpeg + -progress     [done]
      compress.rs          # orchestrate compress, --jobs, temp→rename [done]
      progress.rs          # Event / ProgressSink / CancelToken        [done]
      drive/
        mod.rs             # ensure folder, upload, dedup, mark manifest [done]
        auth.rs            # .env load, loopback consent, refresh token   [done]
        upload.rs          # resumable chunked upload (+ progress cb)      [done]
  cli/   (bin "video-uploader")
    src/
      main.rs              # clap dispatch -> core                  [done]
      cli.rs               # clap definitions                       [done]
      ui.rs                # indicatif ProgressSink for the CLI      [done]
  gui/   (bin, planned M7+) # gpui app -> core (channel sink)
```

---

## Desktop GUI (gpui + gpui-component) — planned

A native desktop front-end for the **compress → upload** flow, built on
[gpui](https://github.com/zed-industries/zed) (Zed's GPU-accelerated UI
framework) and [gpui-component](https://github.com/longbridge/gpui-component)
(60+ widgets: inputs, buttons, progress, virtualized tables, notifications,
modals, themes). The CLI stays fully usable — the GUI is a **second front-end
over the same core**, not a rewrite.

### What it does (v1 scope)

- **Choose the operation** — **Compress only**, **Upload only**, or
  **Compress + Upload**. The rest of the UI adapts: required folders, readiness
  checks, and the pre-flight summary all depend on the chosen mode.
- Pick **input** (source) and/or **output** folders via the in-app folder picker
  (input is only needed when compressing).
- **Readiness checks** before Start: `ffmpeg`/`ffprobe` present (Compress / Both —
  offer a `winget` install if missing); Drive auth available in `.env`
  (Upload / Both).
- **Pre-flight summary** before anything runs, mode-dependent: compress → file
  count + **size now** + **estimated size after** (+ ratio); upload → count +
  **bytes to send** to Drive.
- **Run** the selected work with a live **overall progress bar + ETA**, a
  per-file list, and a **Cancel** button. Upload reuses the existing resumable
  pipeline. *(Drive auth is done once via the CLI `auth` command, which writes
  the refresh token into `.env`; the GUI reads it.)*

### Architecture: workspace with a shared core

Refactor the single binary into a Cargo **workspace** so the heavy gpui
dependency never touches the CLI build:

```
video-uploader/                # workspace root
  Cargo.toml                   # [workspace] members = ["crates/*"]
  crates/
    core/                      # all logic — no UI, no clap, no gpui
      scan.rs  probe.rs  encode.rs  compress.rs
      manifest.rs  config.rs  tooling.rs  estimate.rs
      progress.rs              # ProgressSink trait + Event enum + CancelToken
      drive/{mod,auth,upload}.rs
    cli/                       # thin clap binary -> core (indicatif sink)
      main.rs  cli.rs
    gui/                       # gpui app -> core (channel sink)
      main.rs  app.rs  state.rs  views/...
```

Today's `src/*.rs` move almost verbatim into `crates/core`; `main.rs` becomes
`crates/cli`. The only behavioral change to core is the progress/cancel refactor
below — **which the CLI benefits from too.**

### Core refactor (prerequisite — benefits CLI + GUI)

Two things in the current code block a responsive GUI:

1. **Progress is hard-wired to `indicatif`/`println!`.** Introduce a
   frontend-agnostic sink in `core::progress` (kept **sync** — no async deps leak
   into core):

   ```rust
   pub enum Event {
       Scan      { files: usize, total_bytes: u64 },
       Estimate  { est_output_bytes: u64, ratio: f32 },
       FileStart { key: String, src_bytes: u64, index: usize, total: usize },
       FileProgress { key: String, done_secs: f64, dur_secs: f64, speed: f32 },
       FileDone   { key: String, out_bytes: u64 },
       FileFailed { key: String, error: String },
       UploadProgress { key: String, sent: u64, total: u64 },
       UploadDone { key: String, drive_id: String },
       Done { compressed: u64, failed: u64, in_bytes: u64, out_bytes: u64 },
   }

   pub trait ProgressSink: Send { fn emit(&mut self, ev: Event); }
   ```

   `compress::run` / `drive::run` take `&mut dyn ProgressSink` instead of
   printing. CLI implements it with `indicatif`; GUI forwards events to a channel.

2. **Encode is blocking and uncancellable** (`Command::output()`). Switch to
   `Command::spawn()` to gain:
   - **Cancellation** — thread a `CancelToken` (`Arc<AtomicBool>`) through the job
     loop; on cancel, `child.kill()` the running ffmpeg and stop pulling jobs.
   - **Per-file progress + ETA** — add `-progress pipe:1 -nostats` and parse the
     `out_time_us=` / `speed=` lines ffmpeg streams, combined with the ffprobe
     duration, to drive a real per-file bar and remaining-time estimate.

   > Dovetails with the already-filed hardening task (encode to a temp path,
   > rename on success): a killed ffmpeg must never leave a "fresh" partial that
   > resume mistakes for done.

### ffmpeg pre-flight + winget install

New `core::tooling`:

- `ffmpeg_status()` → checks `ffmpeg -version` and `ffprobe -version` on `PATH`,
  plus the winget shim dir (`%LOCALAPPDATA%\Microsoft\WinGet\Links`) in case
  `PATH` wasn't refreshed in the running process.
- `install_ffmpeg(sink)` → runs
  `winget install --id Gyan.FFmpeg -e --source winget --accept-package-agreements --accept-source-agreements`,
  streaming stdout/stderr to the sink so the GUI shows live progress.
- After install, **re-resolve** ffmpeg without an app restart (fall back to the
  `WinGet\Links` shim path if the running process's `PATH` is stale).

GUI flow: a small **banner** — ✅ "ffmpeg ready" / ⬇️ "Not found — Install with
winget" (button) / ⏳ streaming install log / ❌ error with winget output. Start
stays disabled until ffmpeg resolves. This banner applies to **Compress /
Compress + Upload** only — **Upload-only** skips ffmpeg entirely and instead
requires Drive auth in `.env` (its own banner).

> **Caveats:** winget may trigger a **UAC** prompt and is absent on very old
> Windows builds — surface a clear message + a manual-install link as fallback.

### Size estimation (before / estimated after)

`core::estimate`:

- **Size before** is free — `scan` already has per-file sizes.
- **Estimated size after** uses the `maxrate` cap as the dominant signal:
  `est_video ≈ min(src_video_bitrate, maxrate) / 8 × duration`; audio adds `copy`
  ≈ source audio. Per file `est_out = min(src_bytes, est_video + est_audio)` —
  fat clips collapse toward the cap, lean clips stay near original (no bloat),
  matching observed behavior.
- **Two-phase for responsiveness:** show an instant rough total from a stored
  average ratio (our real run was **187.6 GB → 28.9 GB ≈ 6.5×**), then refine
  **per file as ffprobe durations stream in** on a background task.

### GUI structure (gpui)

**Bootstrap** (per gpui-component docs — *lock exact APIs to the pinned gpui
revision during the M7 spike; gpui's API churns*):

```rust
let app = gpui_platform::application().with_assets(gpui_component_assets::Assets);
app.run(|cx| {
    gpui_component::init(cx);
    cx.open_window(WindowOptions::default(), |window, cx| {
        let view = cx.new(|cx| AppView::new(window, cx));
        cx.new(|cx| Root::new(view.into(), window, cx))   // Root wraps the tree
    }).unwrap();
});
```

**State entity** (`gui::state::AppState`, a gpui `Entity`):

| Field | Purpose |
|---|---|
| `mode` | Compress / Upload / Both — drives gating, summary, and the worker |
| `input_dir`, `output_dir` | chosen folders (input only needed for compress/both) |
| `settings` | codec, cq, maxrate, fps_cap, jobs, scale (preset or advanced) |
| `ffmpeg` | Unknown / Installing(log) / Ready / Error (compress/both) |
| `drive` | auth status from `.env`: Ready / Missing (upload/both) |
| `preflight` | files, bytes_before, est_bytes_after, ratio, or bytes_to_upload |
| `phase` | Idle / Scanning / Estimating / Ready / Compressing / Uploading / Done / Error |
| `files` | per-file status, %, size in/out (virtualized table) |
| `overall` | done/total, bytes, ETA, speed |
| `cancel` | `CancelToken` shared with the worker |
| `log` | recent core/winget output |

**Views** (gpui-component widgets):

- Folder row — two read-only inputs + **Browse** buttons (open the in-app
  folder-picker `Dialog`).
- **Mode selector** — a segmented control: **Compress** / **Upload** /
  **Compress + Upload**. Switching it shows/hides the input-folder row, the
  ffmpeg banner, and the Drive banner, and reshapes the pre-flight card.
- Settings — a **preset** dropdown (Balanced `cq30` / Smaller `cq32`+8M / Higher
  `cq28`) with an **Advanced** expander for raw knobs (compress/both only).
- **Readiness banners** — ffmpeg (compress/both, above) and **Drive auth**
  (upload/both): ✅ "Drive connected" / ❌ "Not authorized — run `auth` in the
  CLI" with steps.
- **Pre-flight card** (mode-dependent) — compress: "213 files · 187.6 GB →
  ~28.9 GB (~6.5×)"; upload: "47 files · 6.1 GB to upload".
- **Start / Cancel** — Start gated per mode: Compress → input + output + ffmpeg;
  Upload → output + Drive; Both → all of them.
- **Overall progress** bar + `12 / 213 · ~1h 42m left · 3.9× realtime`.
- **Per-file table** — virtualized list with per-file bars.
- **Upload section** — mirrors compress (overall + per-file send progress).
- **Log panel** + completion/error **toasts** (notifications).

**Folder picker — in-app `Dialog` (gpui-component).** Folder selection happens
in a modal built on gpui-component's `Dialog`, so it matches the app theme and
pulls in no extra crate (gpui-component has **no native OS file dialog** — only
in-app modals, so we render the browser ourselves):

- an editable **path `Input`** — type/paste a path to jump straight there;
- an **Up / breadcrumb** row, and at the top level on Windows a **drive
  selector** (enumerate `C:\`, `D:\`, … by probing `A:`–`Z:`);
- a **virtualized `List`** of sub-folders (folder icon + name; click to enter,
  Select to choose), read via `std::fs::read_dir` (directories only, sorted);
- **Cancel / Select** footer; the chosen `PathBuf` flows into `input_dir` /
  `output_dir`.

gpui-component's `Dialog` / `AlertDialog` is reused for confirmations (e.g.
overwrite warnings), and `Notification` for done/error toasts.

### Background / threading bridge

Core stays **synchronous and thread-based** (no tokio — consistent with the
existing "sync by choice" decision). The GUI marshals it onto gpui's executor:

```rust
// on Start:
let (tx, rx) = smol::channel::unbounded::<Event>();
let cancel = state.cancel.clone();
let mode = state.mode;                          // Compress | Upload | Both
let (cfg, ov, opts) = state.to_run_inputs();

cx.background_spawn(async move {                 // blocking pipeline OFF the UI thread
    let mut sink = ChannelSink(tx);             // impl ProgressSink
    if mode.does_compress() && !cancel.is_cancelled() {
        core::compress::run(&cfg, &ov, &mut sink, &cancel);
    }
    if mode.does_upload() && !cancel.is_cancelled() {
        core::drive::run(&cfg, &opts, &mut sink, &cancel);
    }
}).detach();

cx.spawn(async move |this, cx| {                // drain events ON the UI thread
    while let Ok(ev) = rx.recv().await {
        this.update(cx, |state, cx| { state.apply(ev); cx.notify() })?;
    }
    Ok(())
}).detach();
```

Dropping a `Task` cancels it, but we use an explicit `CancelToken` so the worker
can kill the live ffmpeg child deterministically and finish cleanup.

### Persistence

- **Manifest** moves next to the output set —
  `<output_dir>\.video-uploader\manifest.json` — so each output folder is
  self-describing and independently resumable (the CLI gains a matching default).
- **App prefs** (last-used folders, preset) in the OS config dir via `directories`.

### Added crates (gui only — CLI/core stay lean)

| Concern | Crate |
|---|---|
| UI framework | `gpui`, `gpui_platform` (git, `zed-industries/zed`) |
| Widgets + assets | `gpui-component`, `gpui-component-assets` (git, `longbridge`) |
| Channel to UI | `smol` (or `futures` mpsc) |
| Folder picker | in-app gpui-component `Dialog` + `std::fs` (no extra crate) |
| Config dir / prefs | `directories` |
| ETA formatting | reuse `human_size`; `humantime` for durations |

> **Pin gpui + gpui-component to matching git revisions.** gpui's API changes
> often (e.g. `Model`→`Entity`, render signatures) and gpui-component tracks a
> specific gpui rev — pin both in the workspace `Cargo.toml` and bump together.

### Risks / prove early

- **gpui builds & opens a window on this RTX 5070 / Windows 11 box** — the single
  biggest unknown; the GUI work starts with a throwaway "hello window" spike.
- **API churn** — keep gpui-touching code thin and isolated in `gui::views`.
- **winget UAC / absence** — always offer a manual path + clear messaging.
- **ETA accuracy** — the first estimate is rough; it tightens once the first
  file or two report real `speed=` from ffmpeg.

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
- [x] **M2 — Drive auth:** `dotenvy` + `.env`/`.env.example`, `auth` command
      (loopback OAuth consent, PKCE, `drive.file` scope), exchange code → refresh
      token and **auto-write it into `.env`**, plus an in-memory access-token
      refresh used by later runs. *Code complete; 35 unit tests (incl. RFC 7636
      PKCE vector), clippy clean. Live browser consent pending your Google Cloud
      OAuth client in `.env`.*
- [x] **M3 — Upload:** `upload` command — ensure `DRIVE_ROOT_FOLDER`/per-game
      folder tree (create + cache IDs), **skip** files already present in the
      target folder (dedup by name + manifest), **sequential** resumable chunked
      upload (16 MiB, `Content-Range`/`308` + retry/backoff), mark manifest
      `uploaded` with the Drive file id. *Done & live-verified: uploaded a
      234 MB clip to Drive (folder tree created, manifest marked `uploaded` with
      the file id), and dedup correctly excludes it on re-run. 44 unit tests,
      clippy clean.*
- [ ] **M4 — Orchestration:** `run` pipelines compress→upload, retries/backoff,
      `status` report, full resume.
- [ ] **M5 — Polish:** `--dry-run`, optional cleanup flags, verification
      (size/duration sanity check), docs.

### Desktop GUI

- [x] **M6 — Core refactor:** *Done.* Split into a Cargo workspace
      (`crates/core` lib + `crates/cli` bin); added
      `progress::{Event, ProgressSink, CancelToken}`; switched encode to
      `spawn()` with `-progress` parsing, cancellation, and **temp-file → rename**
      (closes the interrupted-partial bug — a killed ffmpeg leaves only a
      `*.part.mp4`, never a "fresh" final output); re-wired the CLI onto an
      `indicatif` sink; added per-chunk upload progress. 54 tests pass; clippy +
      fmt clean; single-file compress live-verified (9.5 MB → 2.9 MB, no leftover
      temp).
- [ ] **M7 — GUI spike:** *Window proven.* `crates/gui` builds on crates.io
      `gpui 0.2.2` + `gpui-component 0.5.1` (no Zed-monorepo git dep); a window
      opens **and runs** on this machine (RTX 5070 / Win11) — DirectX init OK.
      `core::tooling` (ffmpeg/ffprobe detection + non-interactive
      `winget install Gyan.FFmpeg`) added with tests. **Remaining:** the in-app
      folder-picker `Dialog` and the ffmpeg/Drive readiness banners wired into
      the GUI.
- [ ] **M8 — Compress UI:** mode selector (Compress active; Upload/Both wired in
      M9), pre-flight summary (size before / estimated after), Start/Cancel,
      overall + per-file progress with ETA, log panel + toasts, prefs persistence
      (last folders, mode, preset).
- [ ] **M9 — Upload + modes + packaging:** enable **Upload-only** and
      **Compress + Upload** modes (Drive readiness banner, mode-aware pre-flight,
      gating, and worker dispatch) over the existing pipeline (reads `.env`);
      failed-file summary; bundle a Windows `.exe` (icon, assets) for one-click use.

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
- **Secrets** — OAuth credentials/token never get committed (see `.gitignore`).
- **Destructive ops are opt-in** — nothing deletes originals unless explicitly
  flagged *and* the upload is verified.
