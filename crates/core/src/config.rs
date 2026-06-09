use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// Top-level configuration, loaded from `config.toml` (all fields optional).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub source_dir: PathBuf,
    pub output_dir: PathBuf,
    pub manifest: PathBuf,
    pub encode: EncodeConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct EncodeConfig {
    pub codec: String,
    pub cq: u32,
    pub maxrate: String,
    /// Cap output frame rate; sources above this (e.g. 60 fps) are halved to it.
    /// `0` disables the cap.
    pub fps_cap: u32,
    pub audio: String,
    pub jobs: usize,
    pub scale: Option<String>,
}

/// `%USERPROFILE%\Videos\<subdir>` (or `$HOME/Videos/<subdir>`), with a relative
/// fallback. Derived at runtime so no username is hard-coded into the binary.
fn default_videos_dir(subdir: &str) -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("Videos").join(subdir)
}

impl Default for Config {
    fn default() -> Self {
        Self {
            source_dir: default_videos_dir("NVIDIA"),
            output_dir: default_videos_dir("NVIDIA_compact"),
            manifest: PathBuf::from("manifest.json"),
            encode: EncodeConfig::default(),
        }
    }
}

impl Default for EncodeConfig {
    fn default() -> Self {
        Self {
            codec: "hevc".into(),
            cq: 30,
            maxrate: "12M".into(),
            fps_cap: 30,
            audio: "copy".into(),
            jobs: 2,
            scale: None,
        }
    }
}

impl Config {
    /// Load config from `path` (or `./config.toml`). Missing file -> defaults.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let path = path.unwrap_or_else(|| Path::new("config.toml"));
        if !path.exists() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing config {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sensible() {
        let cfg = Config::default();
        assert_eq!(cfg.encode.codec, "hevc");
        assert_eq!(cfg.encode.cq, 30);
        assert_eq!(cfg.encode.maxrate, "12M");
        assert_eq!(cfg.encode.fps_cap, 30);
        assert_eq!(cfg.encode.jobs, 2);
        assert!(cfg.encode.scale.is_none());
    }

    #[test]
    fn partial_toml_fills_missing_with_defaults() {
        let cfg: Config = toml::from_str("[encode]\ncq = 34\nfps_cap = 0\n").unwrap();
        assert_eq!(cfg.encode.cq, 34); // overridden
        assert_eq!(cfg.encode.fps_cap, 0); // overridden (cap disabled)
        assert_eq!(cfg.encode.codec, "hevc"); // defaulted
        assert_eq!(cfg.encode.maxrate, "12M"); // defaulted
    }

    #[test]
    fn missing_file_yields_defaults() {
        let cfg = Config::load(Some(Path::new("does-not-exist.toml"))).unwrap();
        assert_eq!(cfg.encode.cq, 30);
    }
}
