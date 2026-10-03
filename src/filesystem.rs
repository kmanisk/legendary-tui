//! Filesystem locations. No shelling out, no assumptions beyond $HOME.

use std::path::PathBuf;

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// `~/.config/egs/`
pub fn config_dir() -> PathBuf {
    home().join(".config/egs")
}

/// `~/.cache/egs/library.json`
pub fn cache_file() -> PathBuf {
    home().join(".cache/egs/library.json")
}

/// `~/.local/share/legendary/compatdata/`
pub fn compatdata_dir() -> PathBuf {
    home().join(".local/share/legendary/compatdata")
}

/// Dedicated Epic prefix directory: `~/.local/share/egs/prefixes/`
pub fn prefixes_dir() -> PathBuf {
    home().join(".local/share/egs/prefixes")
}

/// Metadata cache directory: `~/.cache/egs/metadata/`
pub fn metadata_cache_dir() -> PathBuf {
    home().join(".cache/egs/metadata")
}

/// Legendary's metadata directory: `~/.config/legendary/metadata/`
pub fn legendary_metadata_dir() -> PathBuf {
    home().join(".config/legendary/metadata")
}

/// Legendary's installed.json file: `~/.config/legendary/installed.json`
pub fn legendary_installed_file() -> PathBuf {
    home().join(".config/legendary/installed.json")
}

/// `~/.config/rofi/epic-games.list` — owned by the Alt+G Rofi menu.
pub fn rofi_registry() -> PathBuf {
    home().join(".config/rofi/epic-games.list")
}

pub fn ensure_dir(path: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| format!("cannot create {}: {e}", path.display()))
}

/// Atomic file write: temp file + rename, so readers never see halves.
pub fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot promote {}: {e}", path.display()))
}
