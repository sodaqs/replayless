# Changelog

All notable changes to Replayless are documented here.

Format follows [Keep a Changelog](https://keepachangelog.com/) and the entries
are generated from [Conventional Commits](https://www.conventionalcommits.org/).

## [0.1.0] - 2026-06-17


### Chores

- Add .env.example and move Drive config to .env (2fc367a)
- Remove hard-coded username from paths (a5b8d65)
- Add Windows CI and release workflows for GUI (96a5428)
- Rename gui-windows workflow to build (ea7572d)

### Documentation

- Add project plan (README) and CLAUDE.md (28e8f3a)
- Mark M1 (compress) done in README and CLAUDE (57ccc9f)
- Plan Google Drive upload with .env-based OAuth (ff92ec0)
- Mark M2 (Drive auth) done; trim .env.example (23fd2ff)
- Mark M3 (Drive upload) done (9a02160)
- Native folder picker + mark M7 done (db609d6)
- Add READMEs for cli and core crates (f31f627)

### Features

- Scaffold video-uploader CLI with read-only scan command (0ac71fb)
- Add compress stage with NVENC transcode and resumable manifest (8801582)
- Add Drive auth via .env (loopback OAuth + PKCE) (4022517)
- Add Drive upload with resumable chunked transfer (6fa083b)
- Split into workspace with shared core; progress/cancel + encode hardening (b74155d)
- Add gpui GUI crate (window spike) + ffmpeg tooling module (244e3e0)
- Folder-picker dialog, ffmpeg banner, and folder rows (7fc22c3)
- Mode selector, Start/Cancel, and live run progress (65195d9)
- Pre-flight size estimate and ffmpeg winget Install button (096505a)
- Set window title to "Replayless" and a 16:9 min size (427b6f1)
- Encode quality presets and jobs control (b3a6439)
- Embed application icon in the Windows exe (a218d1f)
- Modular GUI with custom title bar; remove Drive upload crate (3ccd58f)
- Hide console window on release builds (90b2cc1)

### Fixes

- Open the native OS folder dialog for Browse (64f3109)
- Don't spawn console windows for ffmpeg/ffprobe/winget on Windows (d48b91f)
- Run ffmpeg and pre-flight checks off the UI thread at launch (7f097c1)

### Other

- Generate release changelog with git-cliff (c029509)
- Trim release notes to features/fixes and publish as replayless.exe (90da35c)

### Refactor

- Extract Google Drive integration into a vu-drive crate (93ea6b7)
- Rebrand video-uploader to Replayless (85e24c6)

