//! Conventional locations for Replayless's per-output bookkeeping.
//!
//! State lives in a hidden `.replayless` folder under the **output** directory,
//! so a given output library carries its own resumable manifest and probe cache.
//! Centralized here so the GUI's run path and the pre-flight estimator agree on
//! where the manifest is (calibration reads the same file the run writes).

use std::path::{Path, PathBuf};

/// The `.replayless` data directory under an output library.
pub fn data_dir(output: &Path) -> PathBuf {
    output.join(".replayless")
}

/// The resumable manifest path for an output library.
pub fn manifest_path(output: &Path) -> PathBuf {
    data_dir(output).join("manifest.json")
}

/// The probe-cache path for an output library.
pub fn probe_cache_path(output: &Path) -> PathBuf {
    data_dir(output).join("probe-cache.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_live_under_output_replayless() {
        let out = Path::new(r"C:\Videos\NVIDIA_compact");
        assert_eq!(
            data_dir(out),
            PathBuf::from(r"C:\Videos\NVIDIA_compact\.replayless")
        );
        assert_eq!(
            manifest_path(out),
            PathBuf::from(r"C:\Videos\NVIDIA_compact\.replayless\manifest.json")
        );
        assert_eq!(
            probe_cache_path(out),
            PathBuf::from(r"C:\Videos\NVIDIA_compact\.replayless\probe-cache.json")
        );
    }
}
