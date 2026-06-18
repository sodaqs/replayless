# replayless-gui

Native desktop front-end for the **Replayless** compress pipeline, built on
[gpui](https://github.com/zed-industries/zed) (Zed's GPU-accelerated UI
framework) and [gpui-component](https://github.com/longbridge/gpui-component)
(inputs, buttons, progress, tables, notifications, themes). The CLI stays fully
usable — this is a **second front-end over the same [core](../core)**, not a
rewrite.

> Build a release exe: `cargo build -p replayless-gui --release`. The app icon is
> embedded into the `.exe` (see [`build.rs`](build.rs)), so it ships as a single
> self-contained executable. Release builds also run in the Windows GUI subsystem
> — no console window flashes behind the app.

## What it does (v1)

- Pick **input** (source) and **output** folders via the native OS folder dialog.
- **Readiness check** before Start: `ffmpeg` / `ffprobe` present, with a one-click
  `winget` install if missing. The check runs off the UI thread, so the window
  opens instantly and the badge fills in a beat later.
- **Pre-flight summary** before anything runs: file count + **size now** +
  **estimated size after** (+ ratio).
- **Quality preset** — Balanced (`cq30`), Smaller (`cq32` + 8M cap), Higher (`cq28`).
- **Jobs** — 1 or 2 concurrent NVENC sessions.
- **Run** with a live **overall progress bar + ETA**, a per-file detail line, and a
  **Cancel** button.

## ffmpeg pre-flight + winget install

Backed by [`core::tooling`](../core):

- `ffmpeg_status()` → checks `ffmpeg -version` and `ffprobe -version` on `PATH`.
- `install_ffmpeg(sink)` → runs `winget install --id Gyan.FFmpeg -e …`, streaming
  output to the sink so the GUI shows live progress.
- `ensure_ffmpeg(sink)` → check-then-install: a no-op when already on `PATH`,
  otherwise installs via winget and returns the re-resolved status.

After an install the status is **re-resolved without an app restart**. The GUI
shows a status badge ("checking…" → ready/missing) plus an **Install ffmpeg**
button; the CLI exposes the same via `replayless setup`.

> Every external-tool spawn (ffmpeg, ffprobe, winget) goes through
> `core::proc::command`, which sets `CREATE_NO_WINDOW` on Windows — so a
> GUI-subsystem build never pops a console window during checks or encodes.

## Size estimation (before / after)

- **Size before** is free — the scan already has per-file sizes.
- **Estimated size after** uses a stored average ratio (**187.6 GB → 28.9 GB ≈
  6.5×** from the real run), refining per file as ffprobe durations come in.

## Threading bridge

The core stays **synchronous and thread-based** (no tokio). The GUI runs it on a
background thread and marshals progress `Event`s onto gpui's executor:

```rust
let (tx, mut rx) = futures::channel::mpsc::unbounded::<Event>();
let cancel = state.cancel.clone();

std::thread::spawn(move || {
    let mut sink = ChannelSink(tx);
    replayless_core::compress::run(&cfg, &ov, &mut sink, &cancel);
});

cx.spawn(async move |cx| {
    while let Some(ev) = rx.next().await {
        // update UI on the foreground executor
    }
}).detach();
```

The same pattern drives the startup checks (ffmpeg + pre-flight) and the folder
picker, so nothing blocking ever runs on the UI thread.

## Crates used (GUI only — CLI/core stay lean)

| Concern | Crate |
|---|---|
| UI framework | `gpui 0.2.2` (crates.io) |
| Widgets + assets | `gpui-component 0.5.1` (crates.io) |
| Channel to UI | `futures` mpsc |
| Folder picker | native OS dialog via gpui `prompt_for_paths` (built-in) |
| Embedded `.exe` icon | `winresource` (build dependency) |

## Folder structure

```
crates/gui/
├── Cargo.toml
├── build.rs                      # embeds assets/icon.ico into the .exe (Windows)
└── src/
    ├── main.rs                   # Entry point — Application::new().run(...); start_checks()
    ├── app.rs                    # AppView (root state + Render) + start_run + install_ffmpeg
    │
    ├── shared/                   # Reusable widgets with no knowledge of business logic
    │   ├── assets.rs             # AssetSource — brand icon + gpui-component Lucide icons
    │   └── components/
    │       ├── card.rs           # card() — bordered rounded container helper
    │       ├── stat.rs           # stat_chip() — label-above / value-below metric widget
    │       └── title_bar.rs      # custom Windows-style caption bar
    │
    └── features/                 # Vertical slices by UI section
        ├── folders/
        │   ├── mod.rs            # Target enum, Preflight struct, compute_preflight()
        │   └── view.rs           # folder_row(), preflight_strip(), pick_folder()
        ├── settings/
        │   ├── mod.rs            # Mode enum, Quality enum (with cq/maxrate/est_ratio)
        │   └── view.rs           # settings_panel() — Mode / Quality / Jobs selectors
        └── progress/
            ├── mod.rs
            ├── model.rs          # RunState, ChannelSink, basename(), fmt_eta()
            └── view.rs           # run_panel() — progress bar + stats + current file + log
```

### Organising principles

- **`shared/components/`** holds only "dumb" widgets that are unaware of any business domain. If a component references `AppView` or feature-specific state, it does not belong here.
- **Feature folders** own both model (data, transitions) and view (rendering) for their slice. `mod.rs` holds pure-Rust types and functions; `view.rs` holds gpui rendering helpers.
- **`AppView`** (in `app.rs`) is the single root entity that aggregates all feature state. Feature view functions receive `&Entity<AppView>` so they can dispatch updates via closures captured in button `on_click` handlers. Intra-crate module cycles are fine in Rust — feature views importing from `app.rs` and vice-versa compiles without issue.
- **Growth threshold**: the structure doc recommends migrating to a cargo workspace (one crate per feature) once the codebase exceeds roughly 3–5 k lines or features become independently deployable.

---

## Code style

### gpui patterns used

| Pattern | Where |
|---|---|
| `Entity<T>` cloned into closures | All `on_click` handlers in feature views |
| `cx.entity()` → clone into sub-functions | `AppView::render()` |
| `FluentBuilder::when(cond, \|b\| b.primary())` | Segmented-control active state |
| `RenderOnce` components from gpui-component | `Progress`, `Button`, `Icon` |
| Channel-based event loop (unbounded mpsc) | `start_run()` in `app.rs` |
| `cx.spawn(async \|cx\| { ... }).detach()` | Startup checks, foreground event drain, folder picker |

### Rendering helpers

Helper functions return `impl IntoElement` (or `impl IntoElement + use<...>` when capturing). They receive extracted theme colors as `Hsla` parameters rather than `&mut Context<AppView>` — colors are pulled once at the top of `AppView::render()` and passed down:

```rust
// In AppView::render():
let muted = theme.muted_foreground;
let border = theme.border;

// Passed to helpers:
folder_row(&view, "Source", self.input_dir.as_ref(), Target::Input, "browse-in", muted, fg)
```

### Theme tokens used

All colors come from `cx.theme()` (`ActiveTheme` from gpui-component). Tokens in use:

| Token | Purpose |
|---|---|
| `theme.background` | Window fill |
| `theme.foreground` | Primary text |
| `theme.muted_foreground` | Labels, secondary text |
| `theme.border` | Card borders, dividers, seg-control borders |
| `theme.secondary` | Card background (slightly elevated) |
| `theme.primary` | Accent — active seg-control button, progress fill |
| `theme.success` | ffmpeg-ready badge, "Done" header |
| `theme.danger` | ffmpeg-missing badge, failed-count stat |
| `theme.progress_bar` | Default progress fill (via gpui-component `Progress`) |

### Segmented controls

Mode / Quality / Jobs selectors are rendered as a group of `Button`s with `ButtonRounded::None` wrapped in a container that supplies the border and rounded corners:

```rust
fn seg_group(border: Hsla) -> Div {
    h_flex()
        .border_1()
        .border_color(border)
        .rounded_md()
        .overflow_hidden()   // clips button corners to the group shape
}
```

The active button calls `.primary()` via `FluentBuilder::when`:

```rust
Button::new(id).label(label).small().rounded(ButtonRounded::None)
    .when(is_active, |b| b.primary())
    .on_click(...)
```

### Progress display

`features/progress/model.rs` — `RunState` accumulates:
- `bytes_out` — sum of `out_bytes` from each `FileFinished` event (shows compression output so far)
- `current_speed`, `current_eta` — per-file realtime factor and remaining seconds from `FileProgress`

`features/progress/view.rs` — `run_panel()` renders:
1. Header text (stage + files done/total, or "Done" in success color)
2. `Progress::new().value(overall * 100.).h(px(16.))` — tall overall bar
3. Stats row: Files · Compressed · Speed · ETA (Failed/Skipped only appear if > 0)
4. Current-file block: name + 6 px mini-bar + `"42% · 3.9× · 1m23s"` detail line
5. Log section: last 8 lines, `text_xs`, muted color
