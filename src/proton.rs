//! Steam and Proton compatibility tool discovery and management.
//!
//! Automatically discovers installed Steam and GE-Proton versions across
//! standard XDG and Steam directories. Supports Auto selection and custom paths.

use std::path::{Path, PathBuf};

use crate::filesystem;

#[derive(Clone, Debug, PartialEq)]
pub struct ProtonVersion {
    pub name: String,
    pub path: PathBuf,
    pub is_ge: bool,
}

/// Dynamically probe known Steam installations.
pub fn steam_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let home = filesystem::home();

    if let Ok(data_home) = std::env::var("XDG_DATA_HOME") {
        roots.push(PathBuf::from(data_home).join("Steam"));
    }
    roots.push(home.join(".local/share/Steam"));
    roots.push(home.join(".steam/steam"));
    roots.push(home.join(".steam/root"));
    roots.push(PathBuf::from("/usr/share/steam"));

    let mut valid = Vec::new();
    for r in roots {
        if r.is_dir() && !valid.contains(&r) {
            valid.push(r);
        }
    }
    valid
}

/// Primary Steam installation directory (e.g. for STEAM_COMPAT_CLIENT_INSTALL_PATH).
pub fn primary_steam_root() -> Option<PathBuf> {
    steam_roots().into_iter().next()
}

/// Search for all installed Proton versions on the system.
pub fn discover_protons() -> Vec<ProtonVersion> {
    let mut versions = Vec::new();
    let roots = steam_roots();

    for root in &roots {
        // 1. compatibilitytools.d (GE-Proton, custom, CachyOS SLR)
        let ct_dir = root.join("compatibilitytools.d");
        scan_compat_dir(&ct_dir, &mut versions);

        // 2. steamapps/common (Valve Proton, Proton Experimental, Proton 9/10)
        let common = root.join("steamapps/common");
        scan_common_dir(&common, &mut versions);
    }

    // Also check global /usr/share/steam/compatibilitytools.d
    scan_compat_dir(
        Path::new("/usr/share/steam/compatibilitytools.d"),
        &mut versions,
    );

    // Sort: GE-Proton first, then Experimental, then numeric versions descending
    versions.sort_by(|a, b| {
        b.is_ge
            .cmp(&a.is_ge)
            .then_with(|| (b.name.contains("Experimental")).cmp(&a.name.contains("Experimental")))
            .then_with(|| b.name.cmp(&a.name))
    });
    versions.dedup_by(|a, b| a.name == b.name);
    versions
}

fn scan_compat_dir(dir: &Path, out: &mut Vec<ProtonVersion>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        let proton_bin = p.join("proton");
        if proton_bin.is_file() {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let is_ge = name.starts_with("GE-Proton");
            out.push(ProtonVersion {
                name,
                path: p,
                is_ge,
            });
        }
    }
}

fn scan_common_dir(dir: &Path, out: &mut Vec<ProtonVersion>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.to_lowercase().contains("proton") {
            let proton_bin = p.join("proton");
            if proton_bin.is_file() {
                out.push(ProtonVersion {
                    name,
                    path: p,
                    is_ge: false,
                });
            }
        }
    }
}

/// Resolve selected Proton to a valid executable and tool path.
pub fn resolve_proton(choice: &str) -> Result<ProtonVersion, String> {
    let choice_trimmed = choice.trim();
    let discovered = discover_protons();

    if choice_trimmed.is_empty() || choice_trimmed.eq_ignore_ascii_case("auto") {
        if let Some(first) = discovered.first() {
            return Ok(first.clone());
        }
        return Err("No compatible Proton runtime was found.\nInstall GE-Proton or Proton via Steam or Settings.".into());
    }

    // Direct path check
    let direct_path = PathBuf::from(choice_trimmed);
    if direct_path.is_dir() && direct_path.join("proton").is_file() {
        let name = direct_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| choice_trimmed.to_string());
        let is_ge = name.starts_with("GE-Proton");
        return Ok(ProtonVersion {
            name,
            path: direct_path,
            is_ge,
        });
    }

    // Name match in discovered
    for v in discovered {
        if v.name == choice_trimmed || v.path.to_string_lossy() == choice_trimmed {
            return Ok(v);
        }
    }

    Err(format!(
        "Proton '{choice}' not found.\nSelect another in Settings → Proton."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_mock_proton() {
        let tmp = std::env::temp_dir().join(format!("egs-proton-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let ct = tmp.join("compatibilitytools.d/GE-Proton99-Test");
        std::fs::create_dir_all(&ct).unwrap();
        std::fs::write(ct.join("proton"), "#!/bin/sh\n").unwrap();

        let mut out = Vec::new();
        scan_compat_dir(&tmp.join("compatibilitytools.d"), &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "GE-Proton99-Test");
        assert!(out[0].is_ge);

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
