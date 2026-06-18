//! Core library for **Replayless**: scanning the source library, NVENC
//! compression, resumable manifest state, and Google Drive upload.
//!
//! This crate is **UI-agnostic** — it contains no CLI parsing and no GUI code.
//! Front-ends (the `replayless` CLI, the planned gpui desktop app) depend on
//! these modules and drive them. Google Drive upload lives in the sibling
//! `replayless-drive` crate, which builds on top of these.

pub mod compress;
pub mod config;
pub mod encode;
pub mod estimate;
pub mod manifest;
pub mod paths;
pub mod preflight;
pub mod probe;
pub mod probe_cache;
pub mod proc;
pub mod progress;
pub mod scan;
pub mod tooling;
