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

/// `~/.cache/egs/`
pub fn cache_dir() -> PathBuf {
    home().join(".cache/egs")
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

pub fn gather_candidate_directories(default_path: &str) -> Vec<String> {
    let mut dirs = Vec::new();
    let mut seen = std::collections::HashSet::new();

    let mut add = |p: &str| {
        let clean = p.trim().trim_end_matches('/');
        if !clean.is_empty()
            && seen.insert(clean.to_string())
            && std::path::Path::new(clean).is_dir()
        {
            dirs.push(clean.to_string());
        }
    };

    if !default_path.is_empty() {
        add(default_path);
    }
    add("/mnt/Games/EpicGames");
    add("/mnt/Games");
    add("/mnt/Games/Other");
    add("/mnt/Games/NonSteamGames");
    let home_path = home();
    add(&home_path.join("Games").display().to_string());
    add(&home_path
        .join(".local/share/legendary")
        .display()
        .to_string());
    add(&home_path.display().to_string());

    if let Ok(output) = std::process::Command::new("fd")
        .args(["-t", "d", "--max-depth", "2", ".", "/mnt/Games", "/mnt"])
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                let l = line.trim();
                if !l.contains("$RECYCLE") && !l.contains("System Volume") && !l.contains(".git") {
                    add(l);
                }
            }
        }
    }
    dirs
}

pub fn pick_directory_fzf(default_path: &str, title: &str) -> Option<String> {
    let candidates = gather_candidate_directories(default_path);
    let mut child = std::process::Command::new("fzf")
        .args([
            "--prompt",
            "Move destination > ",
            "--header",
            &format!("Select destination directory for {title} (Enter: choose, Esc: cancel)"),
            "--print-query",
            "--bind",
            "enter:accept",
            "--reverse",
            "--height",
            "40%",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .ok()?;

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        for c in candidates {
            let _ = writeln!(stdin, "{c}");
        }
    }

    let output = child.wait_with_output().ok()?;
    if !output.status.success() && output.status.code() == Some(130) {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }
    let chosen = if lines.len() >= 2 {
        lines[1].to_string()
    } else {
        lines[0].to_string()
    };
    Some(chosen)
}
