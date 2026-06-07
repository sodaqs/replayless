use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Lifecycle of a single source video.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    #[default]
    Pending,
    Compressed,
    Uploaded,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub status: Status,
    pub source_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub output_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub drive_id: Option<String>,
}

/// The resumable state for the whole library, keyed by source path.
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct Manifest {
    pub entries: BTreeMap<String, Entry>,
}

impl Manifest {
    /// Load from disk; a missing file yields an empty manifest.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Manifest::default());
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading manifest {}", path.display()))?;
        serde_json::from_str(&text)
            .with_context(|| format!("parsing manifest {}", path.display()))
    }

    /// Persist atomically (write to a temp file, then rename over the target).
    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self).context("serializing manifest")?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
        Ok(())
    }

    /// True if this source has already been compressed (or further along).
    pub fn is_compressed(&self, key: &str) -> bool {
        matches!(
            self.entries.get(key).map(|e| e.status),
            Some(Status::Compressed) | Some(Status::Uploaded)
        )
    }

    /// Record a successful compression.
    pub fn mark_compressed(&mut self, key: &str, source_bytes: u64, output_bytes: u64) {
        let entry = self.entries.entry(key.to_string()).or_insert(Entry {
            status: Status::Pending,
            source_bytes,
            output_bytes: None,
            drive_id: None,
        });
        entry.status = Status::Compressed;
        entry.source_bytes = source_bytes;
        entry.output_bytes = Some(output_bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let mut m = Manifest::default();
        m.mark_compressed("Game/clip.mp4", 1000, 120);
        let json = serde_json::to_string(&m).unwrap();
        let back: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(m.entries, back.entries);
    }

    #[test]
    fn mark_compressed_sets_status_and_sizes() {
        let mut m = Manifest::default();
        assert!(!m.is_compressed("k"));
        m.mark_compressed("k", 5000, 600);
        assert!(m.is_compressed("k"));
        let e = &m.entries["k"];
        assert_eq!(e.status, Status::Compressed);
        assert_eq!(e.source_bytes, 5000);
        assert_eq!(e.output_bytes, Some(600));
    }

    #[test]
    fn uploaded_counts_as_compressed_for_skip() {
        let mut m = Manifest::default();
        m.mark_compressed("k", 1, 1);
        m.entries.get_mut("k").unwrap().status = Status::Uploaded;
        assert!(m.is_compressed("k")); // already past compression -> skip re-encode
    }

    #[test]
    fn save_then_load_is_stable() {
        let dir = std::env::temp_dir().join(format!("vu_manifest_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("manifest.json");
        let mut m = Manifest::default();
        m.mark_compressed("a/b.mp4", 10, 2);
        m.save(&path).unwrap();
        let loaded = Manifest::load(&path).unwrap();
        assert_eq!(m.entries, loaded.entries);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
