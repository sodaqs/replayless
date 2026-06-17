# vu-core

The **UI-agnostic core** of video-uploader: scan the NVIDIA replay library,
compress clips with the GPU (NVENC), track resumable state in a manifest, and
report progress. Every front-end — the [`video-uploader` CLI](../cli) and the
[gpui desktop GUI](../gui) — depends on this crate and drives it.

> No `clap`, no `gpui`, no HTTP. This crate is pure logic plus subprocess calls
> to `ffmpeg` / `ffprobe` / `winget`; it never links libav. See the root
> [README](../../README.md) for the full project plan and measured encode results.

## Responsibilities

- **Scan** the source tree, grouping `.mp4` / `.mkv` / `.mov` / `.avi` by game folder.
- **Probe** each clip (`ffprobe`) for duration / fps / resolution.
- **Build** the ffmpeg NVENC command and **run** it with live progress + cancellation.
- **Orchestrate** the compress run across N worker threads with bounded concurrency.
- **Persist** per-file state to `manifest.json` so a crash never restarts from zero.
- **Detect / install** ffmpeg via `winget`.

Argument parsing lives in [`crates/cli`](../cli); windowing and widgets live in
[`crates/gui`](../gui).

## Module map

| Module | Responsibility | Key public items |
|---|---|---|
| `config` | Load/validate `config.toml`; sane defaults | `Config`, `EncodeConfig`, `Config::load` |
| `scan` | Walk source → grouped work list; size formatting | `Video`, `collect_videos`, `run`, `human_size` |
| `probe` | `ffprobe` → duration / fps / resolution | `MediaInfo`, `probe` |
| `encode` | Build ffmpeg args; spawn + parse `-progress`; cancel | `build_args`, `run_with_progress`, `EncodeProgress`, `RunOutcome` |
| `compress` | Orchestrate the run: queue, `--jobs`, temp→rename, manifest | `Overrides`, `run`, `ratio` |
| `manifest` | Load/save/update resumable state | `Manifest`, `Entry`, `Status` |
| `tooling` | ffmpeg/ffprobe detection + winget install | `ToolStatus`, `ffmpeg_status`, `ensure_ffmpeg`, `install_ffmpeg` |
| `progress` | Frontend-agnostic events + cooperative cancel | `Event`, `ProgressSink`, `Stage`, `CancelToken`, `NullSink`, `FnSink` |

## Progress & cancellation — the front-end contract

This is the seam that keeps the core UI-free. Stages emit `Event`s into a
`ProgressSink` and poll a `CancelToken` between units of work:

```rust
pub trait ProgressSink: Send {
    fn emit(&mut self, event: Event);
}
```

- **`Event`** is a normalized lifecycle stream:
  `StageStarted` → (`FileStarted` → `FileProgress`\* → `FileFinished` / `FileSkipped` / `FileFailed`)\* → `StageFinished`,
  plus free-form `Log`. Per-file progress is a `fraction` in `0.0..=1.0` (with
  optional `speed` ×realtime and `eta_secs`), so a sink never needs to know what
  the underlying unit is.
- **Ready-made sinks:** `NullSink` discards every event (dry runs, tests);
  `FnSink(closure)` adapts any `FnMut(Event)`. The CLI implements its own
  `indicatif` sink; the GUI forwards events down an mpsc channel.
- **`CancelToken`** is a cheap, clonable `Arc<AtomicBool>`. A front-end calls
  `.cancel()`; the worker checks `.is_cancelled()` between files and mid-encode,
  killing the live ffmpeg child when it trips.

### Threading bridge (the core is synchronous)

There is no async runtime — the core is plain thread-based code. `compress::run`
takes `&mut dyn ProgressSink` and `&CancelToken` and blocks until done, so a GUI
runs it on a background thread and marshals events onto its own executor (see the
bridge example in the root [README](../../README.md#background--threading-bridge)).

## Idempotency & resume

Re-running must only do the work that's left — 188 GB means crashes happen.
Three mechanisms guarantee it:

1. **Manifest skip** — `Manifest::is_compressed(label)` skips files already marked
   done. Keys are forward-slashed paths relative to the source root, so they're
   stable across runs and OSes.
2. **Fresh-output skip** — even with no manifest entry, an output that exists and
   is **not older than** its source is treated as already done.
3. **Temp-file → rename** — each encode writes `<name>.part.mp4` and renames it
   into place only on a clean exit. A killed or crashed ffmpeg leaves a
   `.part.mp4`, never a "fresh" final output that resume would mistake for
   complete. A non-zero ffmpeg exit is an error, and the file is **not** marked
   compressed.

`Manifest::save` is itself atomic (write `manifest.json.tmp`, then rename over the
target). `Status::Uploaded` is retained only to load older manifests and is
treated as `Compressed` everywhere.

## Compress flow (`compress::run`)

1. `effective_encode` = the configured `EncodeConfig` with CLI `Overrides` layered on top.
2. `scan::collect_videos` → drop already-done files (manifest **or** fresh output) → build a list of `Job`s.
3. Sort **largest first** (biggest savings land early, and it's the natural test order); apply `--limit`.
4. `--dry-run`: probe + print the planned `ffmpeg` command per job, then return — no side effects.
5. Otherwise spawn `jobs` scoped worker threads pulling from a shared queue. Each
   job: probe → `build_args` → `run_with_progress` (into the temp file) → rename →
   `mark_compressed` + `save` → emit `FileFinished`. A failure emits `FileFailed`
   and the run continues with the next file.
6. Emit `StageFinished` with totals: ok / skipped / failed plus in/out bytes.

## Dependencies

`anyhow` (errors), `serde` + `serde_json` (manifest), `toml` (config),
`walkdir` (scan). ffmpeg / ffprobe / winget are invoked as subprocesses — no
libav linkage, no async runtime.

## Conventions & testing

Per repo policy, every function ships with tests: pure logic is unit-tested
directly (ffmpeg arg building, manifest transitions, scan grouping, ffprobe
parsing, the output-freshness rule, cancel-token semantics) and I/O uses temp
dirs. Library modules use `anyhow`/`thiserror` and never `unwrap()` on I/O or
ffmpeg exit codes.

```powershell
cargo test -p vu-core
cargo clippy -p vu-core --all-targets
```
