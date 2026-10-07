//! Filesystem locations. No shelling out, no assumptions beyond $HOME.

use std::path::PathBuf;

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// `~/.config/legendary-tui/`
pub fn config_dir() -> PathBuf {
    let p = home().join(".config/legendary-tui");
    if p.exists() {
        return p;
    }
    let legacy = home().join(".config/egs");
    if legacy.exists() {
        return legacy;
    }
    p
}

/// `~/.cache/legendary-tui/library.json`
pub fn cache_file() -> PathBuf {
    cache_dir().join("library.json")
}

/// `~/.cache/legendary-tui/`
pub fn cache_dir() -> PathBuf {
    home().join(".cache/legendary-tui")
}

/// One-time migration of legacy `~/.cache/egs` into `~/.cache/legendary-tui`.
/// Copies or moves images, metadata, and files so no files need re-downloading.
pub fn migrate_legacy_cache_if_needed() {
    let legacy = home().join(".cache/egs");
    let current = cache_dir();
    if legacy.exists() {
        let _ = ensure_dir(&current);

        // Migrate images directory
        let legacy_images = legacy.join("images");
        let current_images = image_cache_dir();
        if legacy_images.exists() {
            let _ = ensure_dir(&current_images);
            if let Ok(entries) = std::fs::read_dir(&legacy_images) {
                for entry in entries.flatten() {
                    let dest = current_images.join(entry.file_name());
                    if !dest.exists() {
                        let _ = std::fs::rename(entry.path(), &dest)
                            .or_else(|_| std::fs::copy(entry.path(), &dest).map(|_| ()));
                    }
                }
            }
        }

        // Migrate metadata directory
        let legacy_meta = legacy.join("metadata");
        let current_meta = metadata_cache_dir();
        if legacy_meta.exists() {
            let _ = ensure_dir(&current_meta);
            if let Ok(entries) = std::fs::read_dir(&legacy_meta) {
                for entry in entries.flatten() {
                    let dest = current_meta.join(entry.file_name());
                    if !dest.exists() {
                        let _ = std::fs::rename(entry.path(), &dest)
                            .or_else(|_| std::fs::copy(entry.path(), &dest).map(|_| ()));
                    }
                }
            }
        }

        // Migrate root cache files
        for name in &["library.json", "updates.json", "protondb.json"] {
            let src = legacy.join(name);
            let dst = current.join(name);
            if src.exists() && !dst.exists() {
                let _ =
                    std::fs::rename(&src, &dst).or_else(|_| std::fs::copy(&src, &dst).map(|_| ()));
            }
        }
    }
}

/// `~/.local/share/legendary/compatdata/`
pub fn compatdata_dir() -> PathBuf {
    home().join(".local/share/legendary/compatdata")
}

/// Dedicated Epic prefix directory: `~/.local/share/legendary-tui/prefixes/`
pub fn prefixes_dir() -> PathBuf {
    let p = home().join(".local/share/legendary-tui/prefixes");
    if p.exists() {
        return p;
    }
    let legacy = home().join(".local/share/egs/prefixes");
    if legacy.exists() {
        return legacy;
    }
    p
}

/// Metadata cache directory: `~/.cache/legendary-tui/metadata/`
pub fn metadata_cache_dir() -> PathBuf {
    cache_dir().join("metadata")
}

/// Manifest & game info cache directory: `~/.cache/legendary-tui/info/`
pub fn info_cache_dir() -> PathBuf {
    cache_dir().join("info")
}

/// Available disk space in bytes for the filesystem containing `path`.
pub fn available_disk_space(path: &std::path::Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    #[repr(C)]
    struct Statvfs {
        f_bsize: std::os::raw::c_ulong,
        f_frsize: std::os::raw::c_ulong,
        f_blocks: u64,
        f_bfree: u64,
        f_bavail: u64,
        f_files: u64,
        f_ffree: u64,
        f_favail: u64,
        f_fsid: std::os::raw::c_ulong,
        f_flag: std::os::raw::c_ulong,
        f_namemax: std::os::raw::c_ulong,
        __f_spare: [std::os::raw::c_int; 6],
    }

    extern "C" {
        fn statvfs(path: *const std::os::raw::c_char, buf: *mut Statvfs) -> std::os::raw::c_int;
    }

    let mut check_path = path.to_path_buf();
    while !check_path.exists() {
        if !check_path.pop() {
            return None;
        }
    }

    let c_path = CString::new(check_path.as_os_str().as_bytes()).ok()?;
    let mut stat: Statvfs = unsafe { std::mem::zeroed() };
    let res = unsafe { statvfs(c_path.as_ptr(), &mut stat) };
    if res == 0 {
        let frsize = if stat.f_frsize > 0 {
            stat.f_frsize as u64
        } else {
            stat.f_bsize as u64
        };
        Some(stat.f_bavail * frsize)
    } else {
        None
    }
}

/// Artwork image cache directory: `~/.cache/legendary-tui/images/`
pub fn image_cache_dir() -> PathBuf {
    cache_dir().join("images")
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
    if cfg!(test) || !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return None;
    }
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
