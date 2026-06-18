//! On-disk cache of ffprobe results, so re-opening a folder doesn't re-spawn one
//! `ffprobe` per file (hundreds of subprocess launches). Keyed by source path; an
//! entry is reused only while the file's size **and** mtime are unchanged, so an
//! edited/replaced clip is transparently re-probed.

use std::collections::BTreeMap;
use std::fs::Metadata;
use std::path::Path;
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::estimate::ProbeFacts;
use crate::probe::MediaInfo;

/// One cached probe result plus the file fingerprint it was taken from.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CachedProbe {
    /// File mtime in milliseconds since the Unix epoch (freshness check).
    pub mtime_ms: u64,
    /// File size in bytes (freshness check).
    pub size: u64,
    pub duration_secs: f64,
    pub fps: f64,
}

/// Probe cache keyed by source path (as a lossy string, for portable JSON keys).
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct ProbeCache {
    pub entries: BTreeMap<String, CachedProbe>,
}

/// File mtime as milliseconds since the Unix epoch, or `0` if unavailable.
pub fn mtime_ms(meta: &Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl ProbeCache {
    /// Load from disk; a missing (or unreadable/corrupt) file yields an empty
    /// cache — the cache is an optimization, never a source of truth.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Persist atomically (temp file + rename), creating the parent dir if needed.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let json = serde_json::to_string_pretty(self).context("serializing probe cache")?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
        Ok(())
    }

    /// Cached facts for `key`, but only if the fingerprint (`size` + `mtime_ms`)
    /// still matches — otherwise `None` so the caller re-probes.
    pub fn get(&self, key: &str, size: u64, mtime_ms: u64) -> Option<ProbeFacts> {
        let c = self.entries.get(key)?;
        (c.size == size && c.mtime_ms == mtime_ms).then_some(ProbeFacts {
            duration_secs: c.duration_secs,
            fps: c.fps,
            src_bytes: size,
        })
    }

    /// Store a fresh probe result for `key`.
    pub fn insert(&mut self, key: String, size: u64, mtime_ms: u64, info: &MediaInfo) {
        self.entries.insert(
            key,
            CachedProbe {
                mtime_ms,
                size,
                duration_secs: info.duration_secs,
                fps: info.fps,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> MediaInfo {
        MediaInfo {
            duration_secs: 180.0,
            fps: 60.0,
            width: 2560,
            height: 1440,
        }
    }

    #[test]
    fn get_returns_facts_only_on_exact_fingerprint() {
        let mut cache = ProbeCache::default();
        cache.insert("Game/clip.mp4".into(), 1000, 42, &info());

        // Matching size + mtime → hit, with src_bytes set from the queried size.
        let hit = cache.get("Game/clip.mp4", 1000, 42).unwrap();
        assert_eq!(hit.src_bytes, 1000);
        assert!((hit.duration_secs - 180.0).abs() < 1e-9);
        assert!((hit.fps - 60.0).abs() < 1e-9);

        // Any fingerprint drift → miss.
        assert_eq!(cache.get("Game/clip.mp4", 1001, 42), None); // size changed
        assert_eq!(cache.get("Game/clip.mp4", 1000, 43), None); // mtime changed
        assert_eq!(cache.get("other.mp4", 1000, 42), None); // unknown key
    }

    #[test]
    fn missing_file_loads_as_empty() {
        let cache = ProbeCache::load(Path::new("does-not-exist-probe-cache.json"));
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = std::env::temp_dir().join(format!("vu_probe_cache_{}", std::process::id()));
        let path = dir.join("nested").join("probe-cache.json");
        let mut cache = ProbeCache::default();
        cache.insert("a/b.mp4".into(), 12_345, 999, &info());

        cache.save(&path).unwrap(); // also creates the nested dir
        let loaded = ProbeCache::load(&path);
        assert_eq!(cache.entries, loaded.entries);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
