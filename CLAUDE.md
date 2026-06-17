# CLAUDE.md

Essentials for working in this repo. See [README.md](README.md) for the full plan.

## What this is

A Rust CLI + GUI that **compresses NVIDIA replay videos with the GPU (NVENC)**,
preserving the per-game folder structure.
Source: `C:\Users\<you>\Videos\NVIDIA\<Game>\*.mp4` — ~213 files, ~188 GB,
2560×1440 @ 30 fps H.264 ~60 Mbps. Goal: shrink ~6–8× (to ~20–30 GB).

Status: **M0–M1 done, compress pipeline live-verified** — `scan` and `compress`
(NVENC, resumable manifest) work end-to-end. 4 clips compressed; 209 to go.

## Environment (this machine)

- **GPU:** RTX 5070 → NVENC `hevc_nvenc`, `av1_nvenc`, `h264_nvenc` available.
- **ffmpeg:** installed (8.0.1, via winget `Gyan.FFmpeg`) and on `PATH` — call it
  as just `ffmpeg`. Invoked as a subprocess; we do NOT link libav. `ffprobe` is
  available too (same build) for reading codec/duration/bitrate.
- **Rust:** 1.95, **edition 2024**.
- **OS:** Windows 11, shell is **PowerShell** (use PowerShell syntax: `$env:VAR`,
  `$null`, backtick for line-continuation). Watch path backslashes/quoting.

## Commands

The workspace has two binaries (CLI + GUI), so `cargo run` needs `--bin`:

```powershell
cargo build                                   # build
cargo run --bin replayless -- scan        # list source videos + totals (read-only)
cargo run --bin replayless -- setup       # ensure ffmpeg (winget-installs if missing)
cargo run --bin replayless -- compress    # transcode pending (auto-ensures ffmpeg first)
cargo test
cargo clippy --all-targets
cargo fmt
```

## Architecture (see README for full layout)

`main.rs` → `cli.rs` (clap) dispatches to stages, each idempotent and driven by
`manifest.json`:

- `scan.rs` — walk source, group by game folder (a folder counts only if it
  *directly* contains `.mp4`s; this skips NVIDIA's junk folders like `Steam`).
- `encode.rs` — build/run ffmpeg NVENC command; output mirrors game folders;
  skip if output exists & newer than source.
- `compress.rs` — orchestrates encode with bounded concurrency (`--jobs`).
- `manifest.rs` — load/save per-file state (`pending → compressed`),
  the single source of truth for resume.

## Reference encode commands

Validated by a sample test (see README → *Measured sample results*): `cq30` gave
**VMAF 94.6 at 8.4×** on a fat 60 Mbps clip; `cq32` + 8M cap → ~92 / 10.9×.

```sh
# HEVC (default): quality target + bitrate ceiling. NO -hwaccel cuda (see Gotchas).
# Add the fps filter ONLY when the source is >30 fps (see fps-cap rule below).
ffmpeg -y -i IN.mp4 -c:v hevc_nvenc -preset p6 -tune hq \
  -rc vbr -cq 30 -b:v 0 -maxrate 12M -bufsize 24M -vf fps=30 \
  -c:a copy -movflags +faststart OUT.mp4
```

`-cq` is the quality dial (lower = bigger/better, sane 28–34). The `-maxrate` cap
matters because savings are bitrate-dependent: fat ~60 Mbps clips shrink ~8×, but
already-lean ~20 Mbps clips barely shrink — the cap prevents bloat without a
second pass. AV1 NVENC needs higher `cq` to compact (untuned); HEVC is proven.

**Frame-rate cap: halve 60 fps → 30 fps.** Probe each source's frame rate with
`ffprobe` first; if it's >30 fps, add `-vf fps=30` to drop to 30 (this halves the
frames on 60 fps gameplay for extra savings). If the source is already ≤30 fps,
omit the filter entirely — don't re-time clips that don't need it. When a
`scale` is also configured, chain them in one `-vf`, e.g. `-vf scale=1920:1080,fps=30`.

## Conventions

- **Test every new function.** Every new piece of functionality ships with tests
  that cover it — no feature lands untested. Unit-test pure logic (scan grouping,
  manifest transitions, ffmpeg arg building); use fakes/temp dirs for I/O. Run
  `cargo test` before considering work done.
- **Idempotent stages.** Never assume a clean start — 188 GB means crashes
  happen; re-running must only do the remaining work. Manifest is authoritative.
- **Destructive ops are opt-in.** Deleting originals/compressed copies requires an
  explicit flag. Default to keeping everything.
- **Errors:** `anyhow` at the binary boundary, `thiserror` for library-style
  modules. Don't `unwrap()` on I/O, ffmpeg exit codes, or network calls.
- **Logging:** `tracing`; user-facing progress via `indicatif`.

## Commits

Use [Conventional Commits](https://www.conventionalcommits.org/) — every message
starts with a type prefix:

- `feat:` — new functionality
- `fix:` — bug fix
- `refactor:` — behavior-preserving restructure
- `test:` — adding or fixing tests
- `docs:` — docs/comments only (README, CLAUDE.md)
- `chore:` — build, deps, tooling, config

Keep the subject imperative and concise, e.g. `feat: add resumable compress`.

**Before every commit, run `cargo clippy --all-targets` and `cargo fmt` (and
`cargo test`) — a commit must be clippy-clean and formatted.**

## Gotchas

- **GPU decode is broken here.** `-hwaccel cuda` fails on this RTX 5070 / ffmpeg
  8.0.1 (`cuvidCreateDecoder ... CUDA_ERROR_INVALID_VALUE`) and silently falls
  back to CPU decode. Default to CPU decode (omit `-hwaccel`); NVENC *encode*
  still runs at ~4× realtime. Revisit GPU decode later as a speedup.
- **Savings are bitrate-dependent.** Don't promise a flat ratio — fat clips ~8×,
  lean clips ~1.4×. The `-maxrate` cap is what makes output sizes predictable.
- **VMAF on Windows:** ffmpeg's filtergraph parser chokes on the `C:` drive colon
  in `libvmaf log_path` no matter how you escape it. Write the log with a *bare
  relative filename* (no path) from the working dir, then read it back.
- **Use `--jobs 1`.** Measured 2026-06-07: `--jobs 2` ran at 3.86× realtime vs.
  3.9× single-stream — NVENC encode (or the CPU-decode fallback) is already
  saturated by one stream, so extra jobs only add system load without finishing
  faster. The config default is still `2`; pass `--jobs 1` for full runs. For
  scale: the library is ~12.2 h of footage → ~3 h to encode at this rate.
- AV1 saves the most but has weaker old-device/preview playback AND needs higher
  `cq` to actually compact (cq32 *grew* a lean clip in testing) — HEVC is the
  safe default.
- Validate ffmpeg child-process exit codes; a non-zero exit must NOT mark a file
  `compressed` in the manifest.
- PowerShell path quoting: always quote paths with spaces (game names have them).
