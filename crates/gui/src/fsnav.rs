//! Pure filesystem-navigation helpers for the in-app folder picker.

use std::path::{Path, PathBuf};

/// Immediate sub-directories of `dir`, sorted case-insensitively by name.
/// Unreadable entries are skipped; an unreadable `dir` yields an empty list.
pub fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.path())
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort_by_key(|p| display_name(p).to_lowercase());
    out
}

/// The parent directory, if any.
pub fn parent(dir: &Path) -> Option<PathBuf> {
    dir.parent().map(Path::to_path_buf)
}

/// A display label for a path: its final component, or the whole path for a
/// root like `C:\`.
pub fn display_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

/// Available drive roots on Windows (`C:\`, `D:\`, …), found by probing A–Z.
#[cfg(windows)]
pub fn drive_roots() -> Vec<PathBuf> {
    (b'A'..=b'Z')
        .map(|c| PathBuf::from(format!("{}:\\", c as char)))
        .filter(|p| p.exists())
        .collect()
}

/// On non-Windows, the single filesystem root.
#[cfg(not(windows))]
pub fn drive_roots() -> Vec<PathBuf> {
    vec![PathBuf::from("/")]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subdirs_lists_only_dirs_sorted_case_insensitively() {
        let base = std::env::temp_dir().join(format!("vu_fsnav_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("Zeta")).unwrap();
        std::fs::create_dir_all(base.join("alpha")).unwrap();
        std::fs::write(base.join("file.txt"), b"x").unwrap();

        let names: Vec<String> = subdirs(&base).iter().map(|p| display_name(p)).collect();
        assert_eq!(names, vec!["alpha", "Zeta"]); // dirs only, case-insensitive order

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn display_name_uses_final_component() {
        assert_eq!(display_name(Path::new(r"C:\Videos\NVIDIA")), "NVIDIA");
    }

    #[test]
    fn parent_of_nested_is_one_up() {
        assert_eq!(parent(Path::new(r"C:\a\b")), Some(PathBuf::from(r"C:\a")));
    }
}
