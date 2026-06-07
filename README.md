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

## Module layout (planned)

```
src/
  main.rs          # CLI entrypoint, dispatch            [done]
  cli.rs           # clap definitions                    [done]
  config.rs        # load/validate config.toml           [done]
  manifest.rs      # load/save/update resumable state    [done]
  scan.rs          # walk source -> grouped work list    [done]
  encode.rs        # build & run ffmpeg NVENC commands    [done]
  probe.rs         # ffprobe: duration/fps/resolution     [done]
  compress.rs      # orchestrate compress, --jobs pool    [done]
  drive/
    mod.rs         # high-level: ensure folder, upload, dedup, mark manifest
    auth.rs        # .env load, loopback consent, refresh-token grant
    upload.rs      # resumable chunked upload protocol
```

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
- [ ] **M2 — Drive auth:** `dotenvy` + `.env`/`.env.example`, `auth` command
      (loopback OAuth consent, PKCE, `drive.file` scope), exchange code → refresh
      token and **auto-write it into `.env`**, plus an in-memory access-token
      refresh used by later runs.
- [ ] **M3 — Upload:** `upload` command — ensure `DRIVE_ROOT_FOLDER`/per-game
      folder tree (create + cache IDs), **skip** files already present in the
      target folder (dedup by name + manifest), **sequential** resumable chunked
      upload with `Content-Range`/`308` handling + backoff, mark manifest
      `uploaded` with the Drive file id.
- [ ] **M4 — Orchestration:** `run` pipelines compress→upload, retries/backoff,
      `status` report, full resume.
- [ ] **M5 — Polish:** `--dry-run`, optional cleanup flags, verification
      (size/duration sanity check), docs.

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
