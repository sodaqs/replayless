//! Core library for **video-uploader**: scanning the source library, NVENC
//! compression, resumable manifest state, and Google Drive upload.
//!
//! This crate is **UI-agnostic** — it contains no CLI parsing and no GUI code.
//! Front-ends (the `video-uploader` CLI, the planned gpui desktop app) depend on
//! these modules and drive them. Google Drive upload lives in the sibling
//! `vu-drive` crate, which builds on top of these.

pub mod compress;
pub mod config;
pub mod encode;
pub mod manifest;
pub mod probe;
pub mod progress;
pub mod scan;
pub mod tooling;
