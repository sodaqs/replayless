# video-uploader (CLI)

The command-line front-end for the compress pipeline: a thin `clap` binary that
loads config, renders progress with `indicatif`, and drives [`vu-core`](../core).
All real work lives in the core — this crate is argument parsing plus terminal
rendering.

Binary name: **`video-uploader`**. The workspace also builds
`video-uploader-gui`, so `cargo run` needs an explicit `--bin`.

## Commands

| Command | What it does |
|---|---|
| `scan` | Walk the source library, group videos by game, print a totals table. Read-only. |
| `compress` | Transcode pending videos into the output dir. Resumable; auto-runs the ffmpeg pre-flight first. |
| `setup` | Check for `ffmpeg`/`ffprobe`; install via `winget` (`Gyan.FFmpeg`) if missing. |

```powershell
cargo run --bin video-uploader -- scan
cargo run --bin video-uploader -- setup
cargo run --bin video-uploader -- compress --jobs 1
cargo run --bin video-uploader -- compress --dry-run --limit 3
```

## Options

**Global** (`global = true` — valid before or after the subcommand):

| Flag | Meaning |
|---|---|
| `--config <PATH>` | Config file (default: `./config.toml`; missing file → built-in defaults). |
| `-v, --verbose` | Debug-level logging. |

**`compress`** — each flag overrides the matching `config.toml` value; omit to
use the config/default:

| Flag | Default | Meaning |
|---|---|---|
| `--codec <hevc\|av1>` | `hevc` | Video codec. HEVC is the proven default. |
| `--cq <n>` | `30` | Quality (lower = better/bigger; sane 28–34). |
| `--maxrate <rate>` | `12M` | Bitrate ceiling; keeps fat clips from staying huge. |
| `--fps-cap <n>` | `30` | Cap frame rate; sources above it (e.g. 60 fps) are halved. `0` = off. |
| `--scale <WxH>` | — | Optional downscale, e.g. `1920x1080`. |
| `--jobs <n>` | `2` | Concurrent NVENC sessions. (Measured: use `1` for full runs — see CLAUDE.md.) |
| `--dry-run` | off | Print the planned ffmpeg commands; touch nothing (also skips the pre-flight). |
| `--limit <n>` | — | Process only the first N (largest) pending clips — handy for testing. |

## ffmpeg pre-flight

`compress` (unless `--dry-run`) and `setup` both call `tooling::ensure_ffmpeg`
before doing work: a no-op when `ffmpeg`/`ffprobe` are on `PATH`, otherwise a
non-interactive `winget install Gyan.FFmpeg`. If they're still unusable afterward
(UAC dismissed, winget absent, …), the CLI aborts with manual-install guidance
rather than starting a doomed run.

## Modules

| File | Responsibility |
|---|---|
| `main.rs` | `clap` dispatch → core; `tracing` setup; the `ensure_ffmpeg_ready` pre-flight wrapper. |
| `cli.rs` | `clap` derive definitions (`Cli`, `Command`, `CompressArgs`). |
| `ui.rs` | `CliSink` — a `ProgressSink` that renders core `Event`s as `indicatif` bars (an overall bar plus one per in-flight file) with a final size/ratio summary. |

## Configuration

Reads `config.toml` from the working dir (or `--config`). All fields are
optional; a missing file falls back to defaults. See the root
[README](../../README.md#configuration-configtoml) for the full schema. Quick
reference:

```toml
source_dir = 'C:\Users\<you>\Videos\NVIDIA'
output_dir = 'C:\Users\<you>\Videos\NVIDIA_compact'
manifest   = './manifest.json'

[encode]
codec = 'hevc'
cq = 30
maxrate = '12M'
fps_cap = 30
audio = 'copy'
jobs = 2
# scale = '1920x1080'
```

## Dependencies

`vu-core` (all logic), `clap` (parsing), `indicatif` (progress bars),
`anyhow` (errors), `tracing` + `tracing-subscriber` (logging).

## Testing

```powershell
cargo test -p video-uploader
```

`cli.rs` runs clap's own `debug_assert()` lint over the whole command tree;
`ui.rs` unit-tests its basename/ETA formatting helpers.
