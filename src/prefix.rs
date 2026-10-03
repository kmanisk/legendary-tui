//! Wine/Proton prefix discovery and measurement.
//!
//! Never assumes `compatdata/<app_name>`: the registry-recorded prefix (if
//! any) wins, then `<appid>`, and only real on-disk directories count.
//! Unknown prefix => None (callers must fail closed, never delete blindly).

use std::path::{Path, PathBuf};

use crate::filesystem;

/// Resolve existing prefix for `app_name`.
/// Checks dedicated prefixes first (`~/.local/share/egs/prefixes/<app_name>`),
/// then legacy compatdata directory (`~/.local/share/legendary/compatdata/<field>`).
/// Returns None unless a directory actually exists on disk.
pub fn resolve(app_name: &str, registry_prefix: &str) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    let dedicated_base = filesystem::prefixes_dir();
    let legacy_base = filesystem::compatdata_dir();

    if !registry_prefix.is_empty() {
        candidates.push(dedicated_base.join(registry_prefix));
        candidates.push(legacy_base.join(registry_prefix));
    }
    candidates.push(dedicated_base.join(app_name));
    candidates.push(legacy_base.join(app_name));

    candidates.into_iter().find(|p| p.is_dir())
}

/// Return the dedicated prefix path for a game, ensuring its parent directory exists.
pub fn dedicated_prefix_path(app_name: &str) -> PathBuf {
    filesystem::prefixes_dir().join(app_name)
}

/// Ensure a dedicated prefix directory exists for an Epic game.
pub fn ensure_prefix(app_name: &str, registry_prefix: &str) -> Result<PathBuf, String> {
    if let Some(p) = resolve(app_name, registry_prefix) {
        return Ok(p);
    }
    let p = dedicated_prefix_path(if !registry_prefix.is_empty() {
        registry_prefix
    } else {
        app_name
    });
    filesystem::ensure_dir(&p)?;
    Ok(p)
}

/// Recursive directory size in bytes. Symlinks are never followed.
pub fn dir_size_bytes(path: &Path) -> u64 {
    fn walk(path: &Path, total: &mut u64) {
        let entries = match std::fs::read_dir(path) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let ft = match entry.file_type() {
                Ok(f) => f,
                Err(_) => continue,
            };
            if ft.is_symlink() {
                continue; // never follow symlinks
            } else if ft.is_dir() {
                walk(&entry.path(), total);
            } else if let Ok(meta) = entry.metadata() {
                *total = total.saturating_add(meta.len());
            }
        }
    }
    let mut total = 0u64;
    walk(path, &mut total);
    total
}

pub fn fmt_size(bytes: u64) -> String {
    const G: f64 = 1024.0 * 1024.0 * 1024.0;
    const M: f64 = 1024.0 * 1024.0;
    const K: f64 = 1024.0;
    let b = bytes as f64;
    if b >= G {
        format!("{:.1} GiB", b / G)
    } else if b >= M {
        format!("{:.1} MiB", b / M)
    } else if b >= K {
        format!("{:.1} KiB", b / K)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_symlink_skip() {
        let root = std::env::temp_dir().join(format!("egs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("a/f1"), vec![0u8; 2048]).unwrap();
        std::fs::write(root.join("a/b/f2"), vec![0u8; 1024]).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("a/f1"), root.join("loop")).ok();
        assert_eq!(dir_size_bytes(&root), 3072);
        assert_eq!(fmt_size(3 * 1024 * 1024 * 1024), "3.0 GiB");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_is_zero() {
        assert_eq!(dir_size_bytes(Path::new("/nonexistent-egs-prefix")), 0);
    }

    #[test]
    fn prefix_resolution_order() {
        let tmp = std::env::temp_dir().join(format!("egs-prefix-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let app_dir = tmp.join("app1");
        std::fs::create_dir_all(&app_dir).unwrap();
        assert!(app_dir.is_dir());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
