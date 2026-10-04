//! Legendary backend. All state comes from machine-readable JSON;
//! human-readable output is never parsed. Game ids travel as argv items,
//! never inside shell strings.

use std::collections::HashMap;
use std::process::Command;

#[derive(Clone, Debug)]
pub struct InstalledInfo {
    pub version: String,
    pub path: String,
}

fn capture(args: &[&str]) -> Result<String, String> {
    let out = Command::new("legendary")
        .args(args)
        .output()
        .map_err(|e| format!("cannot run legendary: {e} (is it installed?)"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "legendary {} failed: {}",
            args.join(" "),
            err.trim()
        ));
    }
    String::from_utf8(out.stdout).map_err(|e| format!("legendary output not UTF-8: {e}"))
}

/// (app_name, app_title) for the whole library.
pub fn library() -> Result<Vec<(String, String)>, String> {
    let text = capture(&["list", "--json"])?;
    let items: Vec<serde_json::Value> =
        serde_json::from_str(&text).map_err(|e| format!("library JSON: {e}"))?;
    let mut out = Vec::new();
    for item in items {
        let id = item.get("app_name").and_then(|v| v.as_str()).unwrap_or("");
        let title = item.get("app_title").and_then(|v| v.as_str()).unwrap_or("");
        if id.is_empty() || title.is_empty() {
            continue;
        }
        out.push((id.to_string(), title.to_string()));
    }
    if out.is_empty() {
        return Err("library empty (login valid? run `legendary auth`)".into());
    }
    Ok(out)
}

/// app_name -> InstalledInfo for installed games.
pub fn installed_map() -> Result<HashMap<String, InstalledInfo>, String> {
    let text = capture(&["list-installed", "--json"])?;
    let items: Vec<serde_json::Value> =
        serde_json::from_str(&text).map_err(|e| format!("installed JSON: {e}"))?;
    let mut map = HashMap::new();
    for item in items {
        let id = item.get("app_name").and_then(|v| v.as_str()).unwrap_or("");
        if id.is_empty() {
            continue;
        }
        map.insert(
            id.to_string(),
            InstalledInfo {
                version: item
                    .get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
                    .to_string(),
                path: item
                    .get("install_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            },
        );
    }
    Ok(map)
}

/// Legendary's own default base dir (`install_dir` in its config.ini).
pub fn legendary_default_dir() -> Option<String> {
    let text =
        std::fs::read(crate::filesystem::home().join(".config/legendary/config.ini")).ok()?;
    let text = String::from_utf8_lossy(&text);
    let mut in_main = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_main = line == "[Legendary]";
            continue;
        }
        if in_main {
            if let Some(v) = line.strip_prefix("install_dir") {
                let v = v.trim().trim_start_matches(['=', ' ', '\t']).trim();
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

/// Check online for game updates using legendary. Returns map of app_name -> available_version.
pub fn check_installed_updates() -> Result<HashMap<String, String>, String> {
    let text = capture(&["list-installed", "--check-updates", "--csv"])?;
    let mut map = HashMap::new();
    for line in text.lines().skip(1) {
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() >= 5 {
            let app_name = parts[0].trim();
            let avail_ver = parts[3].trim();
            let has_update = parts[4].trim().eq_ignore_ascii_case("true");
            if has_update && !app_name.is_empty() {
                map.insert(app_name.to_string(), avail_ver.to_string());
            }
        }
    }
    Ok(map)
}

/// Uninstall a game cleanly via Legendary non-interactively.
/// Passes `-y` to avoid terminal prompts and verifies the result.
pub fn uninstall_game(app_name: &str) -> Result<(), String> {
    let out = Command::new("legendary")
        .args(["-y", "uninstall", app_name])
        .output()
        .map_err(|e| format!("cannot run legendary: {e}"))?;

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    if stdout.contains("ERROR:")
        || stderr.contains("ERROR:")
        || stdout.contains("Aborting")
        || stderr.contains("Aborting")
        || !out.status.success()
    {
        let err = if !stderr.trim().is_empty() {
            stderr.trim()
        } else if !stdout.trim().is_empty() {
            stdout.trim()
        } else {
            "legendary uninstall failed"
        };
        return Err(err.to_string());
    }

    // Verify it is no longer in installed_map
    if let Ok(map) = installed_map() {
        if map.contains_key(app_name) {
            return Err(
                "Legendary reported success, but game is still present in installed registry."
                    .into(),
            );
        }
    }

    Ok(())
}
