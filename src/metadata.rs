//! Game metadata extraction, caching, and background retrieval.
//!
//! Prioritizes local on-disk metadata (`~/.config/legendary/metadata/<app>.json`
//! and `installed.json`) which responds in < 1ms without network calls.
//! Only queries `legendary info <app> --json` in a background one-shot thread
//! when manifest/download size is missing.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

use crate::filesystem;
use crate::models::{Game, GameDetails};

pub struct MetadataManager {
    cache: HashMap<String, GameDetails>,
    tx: Sender<(String, GameDetails)>,
    rx: Receiver<(String, GameDetails)>,
    pending: HashMap<String, bool>,
}

impl MetadataManager {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Self {
            cache: HashMap::new(),
            tx,
            rx,
            pending: HashMap::new(),
        }
    }

    /// Try to get details immediately. If not cached, load from disk or schedule background fetch.
    pub fn get_or_load(&mut self, game: &Game) -> Option<GameDetails> {
        if let Some(d) = self.cache.get(&game.app_name) {
            return Some(d.clone());
        }

        // Try cache file on disk
        if let Some(d) = load_cached_details(&game.app_name) {
            if !d.manifest_cached {
                self.request_fetch(&game.app_name);
            }
            self.cache.insert(game.app_name.clone(), d.clone());
            return Some(d);
        }

        // Try parsing legendary's local metadata
        if let Some(d) = load_from_legendary_files(game) {
            if !d.manifest_cached {
                self.request_fetch(&game.app_name);
            }
            save_cached_details(&d);
            self.cache.insert(game.app_name.clone(), d.clone());
            return Some(d);
        }

        // Neither exists; request background fetch
        self.request_fetch(&game.app_name);
        None
    }

    pub fn load_from_cache(&self, app_name: &str) -> Option<&GameDetails> {
        self.cache.get(app_name)
    }

    pub fn request_fetch(&mut self, app_name: &str) {
        if self.pending.contains_key(app_name) {
            return;
        }
        self.pending.insert(app_name.to_string(), true);
        let id = app_name.to_string();
        let tx = self.tx.clone();
        thread::spawn(move || {
            if let Some(details) = fetch_legendary_info(&id) {
                save_cached_details(&details);
                let _ = tx.send((id, details));
            }
        });
    }

    /// Poll for any completed background fetches. Returns true if cache was updated.
    pub fn poll_updates(&mut self) -> bool {
        let mut updated = false;
        while let Ok((id, details)) = self.rx.try_recv() {
            self.pending.remove(&id);
            self.cache.insert(id, details);
            updated = true;
        }
        updated
    }
}

fn cache_file_for(app_name: &str) -> PathBuf {
    filesystem::metadata_cache_dir().join(format!("{app_name}.json"))
}

pub fn load_cached_details(app_name: &str) -> Option<GameDetails> {
    let path = cache_file_for(app_name);
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn save_cached_details(details: &GameDetails) {
    let path = cache_file_for(&details.app_name);
    if let Ok(bytes) = serde_json::to_vec_pretty(details) {
        let _ = filesystem::atomic_write(&path, &bytes);
    }
}

/// Read details from legendary's local disk files without running any subprocess.
pub fn load_from_legendary_files(game: &Game) -> Option<GameDetails> {
    let mut details = GameDetails {
        app_name: game.app_name.clone(),
        title: game.title.clone(),
        installed: game.installed,
        version: game.version.clone(),
        install_path: game.install_path.clone(),
        ..Default::default()
    };

    // Check ~/.config/legendary/installed.json
    if let Ok(bytes) = std::fs::read(filesystem::legendary_installed_file()) {
        if let Ok(items) = serde_json::from_slice::<Vec<serde_json::Value>>(&bytes) {
            for item in items {
                if item.get("app_name").and_then(|v| v.as_str()) == Some(&game.app_name) {
                    if let Some(s) = item.get("install_size").and_then(|v| v.as_u64()) {
                        details.installed_size = Some(s);
                    }
                    if let Some(exe) = item.get("executable").and_then(|v| v.as_str()) {
                        if !exe.is_empty() {
                            details.launch_exe = Some(exe.to_string());
                        }
                    }
                    if details.install_path.is_none() {
                        if let Some(p) = item.get("install_path").and_then(|v| v.as_str()) {
                            details.install_path = Some(p.to_string());
                        }
                    }
                    break;
                }
            }
        }
    }

    // Check ~/.config/legendary/metadata/<app_name>.json
    let meta_path = filesystem::legendary_metadata_dir().join(format!("{}.json", game.app_name));
    if let Ok(bytes) = std::fs::read(&meta_path) {
        if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            parse_legendary_metadata_json(&val, &mut details);
            return Some(details);
        }
    }

    if details.installed {
        Some(details)
    } else {
        None
    }
}

pub fn parse_legendary_metadata_json(val: &serde_json::Value, details: &mut GameDetails) {
    if let Some(meta) = val.get("metadata") {
        if let Some(dev) = meta.get("developer").and_then(|v| v.as_str()) {
            if !dev.is_empty() {
                details.developer = Some(dev.to_string());
            }
        }
        if let Some(desc) = meta.get("description").and_then(|v| v.as_str()) {
            if !desc.is_empty() {
                details.description = Some(desc.to_string());
            }
        }
        if let Some(date) = meta
            .get("creationDate")
            .or_else(|| meta.get("releaseDate"))
            .and_then(|v| v.as_str())
        {
            // Trim ISO timestamp e.g. 2023-12-31T13:18:40.206Z -> 2023-12-31
            let d = date.split('T').next().unwrap_or(date);
            details.release_date = Some(d.to_string());
        }
        if let Some(cats) = meta.get("categories").and_then(|v| v.as_array()) {
            let mut genres = Vec::new();
            for c in cats {
                if let Some(p) = c.get("path").and_then(|v| v.as_str()) {
                    if p != "games" && p != "applications" && p != "public" {
                        genres.push(p.to_string());
                    }
                }
            }
            if genres.is_empty() {
                // If only generic categories exist, use them
                for c in cats {
                    if let Some(p) = c.get("path").and_then(|v| v.as_str()) {
                        genres.push(p.to_string());
                    }
                }
            }
            details.genres = genres;
        }
        if let Some(custom) = meta.get("customAttributes") {
            if let Some(publ) = custom
                .get("publisherName")
                .and_then(|v| v.get("value"))
                .and_then(|v| v.as_str())
            {
                if !publ.is_empty() {
                    details.publisher = Some(publ.to_string());
                }
            }
            if details.description.is_none() {
                if let Some(short) = custom
                    .get("shortDescription")
                    .and_then(|v| v.get("value"))
                    .and_then(|v| v.as_str())
                {
                    if !short.is_empty() {
                        details.description = Some(short.to_string());
                    }
                }
            }
        }
    }
}

/// Run `legendary info <app_name> --json` once in background to retrieve manifest sizes.
pub fn fetch_legendary_info(app_name: &str) -> Option<GameDetails> {
    let out = std::process::Command::new("legendary")
        .args(["info", app_name, "--json"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout_str = std::str::from_utf8(&out.stdout).ok()?;
    let json_start = stdout_str.find('{')?;
    let json_end = stdout_str.rfind('}')?;
    if json_start > json_end {
        return None;
    }
    let val: serde_json::Value = serde_json::from_str(&stdout_str[json_start..=json_end]).ok()?;
    let mut details = GameDetails {
        app_name: app_name.to_string(),
        title: val
            .get("game")
            .and_then(|g| g.get("title"))
            .and_then(|v| v.as_str())
            .unwrap_or(app_name)
            .to_string(),
        ..Default::default()
    };

    if let Some(game) = val.get("game") {
        if let Some(v) = game.get("version").and_then(|v| v.as_str()) {
            details.version = Some(v.to_string());
        }
        if let Some(cs) = game.get("cloud_saves_supported").and_then(|v| v.as_bool()) {
            details.cloud_saves = Some(cs);
        }
        if let Some(f) = game.get("cloud_save_folder").and_then(|v| v.as_str()) {
            if !f.is_empty() {
                details.cloud_save_folder = Some(f.to_string());
            }
        }
        if let Some(cmd) = game.get("command_line").and_then(|v| v.as_str()) {
            if !cmd.is_empty() {
                details.command_line = Some(cmd.to_string());
            }
        }
        if let Some(lo) = game.get("launch_options") {
            if let Some(s) = lo.as_str() {
                if !s.is_empty() {
                    details.extra_launch_options = Some(s.to_string());
                }
            } else if let Some(arr) = lo.as_array() {
                let opts: Vec<String> = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect();
                if !opts.is_empty() {
                    details.extra_launch_options = Some(opts.join(" "));
                }
            }
        }
        if let Some(is_dlc) = game.get("is_dlc").and_then(|v| v.as_bool()) {
            details.is_dlc = is_dlc;
        }
        if let Some(dlcs) = game.get("owned_dlc").and_then(|v| v.as_array()) {
            details.owned_dlc = dlcs
                .iter()
                .filter_map(|v| {
                    v.as_str()
                        .or_else(|| v.get("title").and_then(|t| t.as_str()))
                        .map(|s| s.to_string())
                })
                .collect();
        }
        if let Some(gd) = game.get("grant_date").and_then(|v| v.as_str()) {
            let d = gd.split('T').next().unwrap_or(gd);
            details.grant_date = Some(d.to_string());
        }
    }

    if let Some(inst) = val.get("install") {
        if !inst.is_null() {
            details.installed = true;
            if let Some(p) = inst.get("install_path").and_then(|v| v.as_str()) {
                details.install_path = Some(p.to_string());
            }
            if let Some(s) = inst.get("disk_size").and_then(|v| v.as_u64()) {
                details.installed_size = Some(s);
            }
            if let Some(plt) = inst.get("platform").and_then(|v| v.as_str()) {
                details.platform = Some(plt.to_string());
            }
            if let Some(dlcs) = inst.get("installed_dlc").and_then(|v| v.as_array()) {
                details.installed_dlc = dlcs
                    .iter()
                    .filter_map(|v| {
                        v.as_str()
                            .or_else(|| v.get("title").and_then(|t| t.as_str()))
                            .map(|s| s.to_string())
                    })
                    .collect();
            }
        }
    }

    if let Some(man) = val.get("manifest") {
        if !man.is_null() {
            if let Some(dl) = man.get("download_size").and_then(|v| v.as_u64()) {
                details.download_size = Some(dl);
            }
            if details.installed_size.is_none() {
                if let Some(dk) = man.get("disk_size").and_then(|v| v.as_u64()) {
                    details.installed_size = Some(dk);
                }
            }
            if let Some(exe) = man.get("launch_exe").and_then(|v| v.as_str()) {
                details.launch_exe = Some(exe.to_string());
            }
            if let Some(bid) = man.get("build_id").and_then(|v| v.as_str()) {
                details.build_id = Some(bid.to_string());
            }
            if let Some(prereq) = man.get("prerequisites") {
                if let Some(p) = prereq.get("path").and_then(|v| v.as_str()) {
                    if !p.is_empty() {
                        details.prerequisites.push(p.to_string());
                    }
                }
                if let Some(n) = prereq.get("name").and_then(|v| v.as_str()) {
                    if !n.is_empty() && !details.prerequisites.contains(&n.to_string()) {
                        details.prerequisites.push(n.to_string());
                    }
                }
            }
        }
    }

    // Complement with local metadata description/developer if available
    let meta_path = filesystem::legendary_metadata_dir().join(format!("{app_name}.json"));
    if let Ok(bytes) = std::fs::read(&meta_path) {
        if let Ok(mval) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            parse_legendary_metadata_json(&mval, &mut details);
        }
    }

    details.manifest_cached = true;
    Some(details)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_meta_json() {
        let json_str = r#"{
            "app_name": "test_game",
            "app_title": "Test Game",
            "metadata": {
                "developer": "Acme Studios",
                "description": "A thrilling test adventure.",
                "creationDate": "2024-05-10T12:00:00.000Z",
                "categories": [
                    { "path": "games" },
                    { "path": "action" },
                    { "path": "rpg" }
                ],
                "customAttributes": {
                    "publisherName": { "type": "STRING", "value": "Acme Pub" }
                }
            }
        }"#;
        let val: serde_json::Value = serde_json::from_str(json_str).unwrap();
        let mut d = GameDetails::default();
        parse_legendary_metadata_json(&val, &mut d);
        assert_eq!(d.developer.as_deref(), Some("Acme Studios"));
        assert_eq!(d.publisher.as_deref(), Some("Acme Pub"));
        assert_eq!(d.release_date.as_deref(), Some("2024-05-10"));
        assert_eq!(d.genres, vec!["action", "rpg"]);
        assert_eq!(
            d.description.as_deref(),
            Some("A thrilling test adventure.")
        );
    }

    #[test]
    fn parse_legendary_info_rich() {
        let raw = r#"[Core] INFO: Trying to re-use existing login session...
{
  "game": {
    "app_name": "3e02273b543f4ff0a1c24d3b534a9ac3",
    "title": "Hell Let Loose",
    "version": "643.1183087",
    "platform_versions": { "Windows": "643.1183087" },
    "grant_date": "2025-01-02T18:13:57.370Z",
    "cloud_saves_supported": false,
    "cloud_save_folder": null,
    "is_dlc": false,
    "launch_options": ["-dx11"],
    "command_line": null,
    "owned_dlc": []
  },
  "install": {
    "platform": "Windows",
    "version": "643.1183087",
    "disk_size": 72270173415,
    "install_path": "/mnt/Games/EpicGames/HellLetLooseG0WU4",
    "installed_dlc": ["DLC_1"]
  },
  "manifest": {
    "launch_exe": "Launch_HLL.exe",
    "build_id": "2iHOBO3GZ0GladcZ7pJrJg",
    "prerequisites": {
      "path": "EasyAntiCheat/EasyAntiCheat_EOS_Setup.exe"
    },
    "disk_size": 72270173415,
    "download_size": 39746389618
  }
}
"#;
        let json_start = raw.find('{').unwrap();
        let json_end = raw.rfind('}').unwrap();
        let val: serde_json::Value = serde_json::from_str(&raw[json_start..=json_end]).unwrap();

        let mut details = GameDetails {
            app_name: "3e02273b543f4ff0a1c24d3b534a9ac3".into(),
            ..Default::default()
        };

        if let Some(game) = val.get("game") {
            if let Some(v) = game.get("version").and_then(|v| v.as_str()) {
                details.version = Some(v.to_string());
            }
            if let Some(cs) = game.get("cloud_saves_supported").and_then(|v| v.as_bool()) {
                details.cloud_saves = Some(cs);
            }
            if let Some(lo) = game.get("launch_options") {
                if let Some(arr) = lo.as_array() {
                    let opts: Vec<String> = arr
                        .iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect();
                    details.extra_launch_options = Some(opts.join(" "));
                }
            }
            if let Some(gd) = game.get("grant_date").and_then(|v| v.as_str()) {
                let d = gd.split('T').next().unwrap_or(gd);
                details.grant_date = Some(d.to_string());
            }
        }
        if let Some(inst) = val.get("install") {
            details.installed = true;
            if let Some(plt) = inst.get("platform").and_then(|v| v.as_str()) {
                details.platform = Some(plt.to_string());
            }
            if let Some(dlcs) = inst.get("installed_dlc").and_then(|v| v.as_array()) {
                details.installed_dlc = dlcs
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect();
            }
        }
        if let Some(man) = val.get("manifest") {
            if let Some(dl) = man.get("download_size").and_then(|v| v.as_u64()) {
                details.download_size = Some(dl);
            }
            if let Some(exe) = man.get("launch_exe").and_then(|v| v.as_str()) {
                details.launch_exe = Some(exe.to_string());
            }
            if let Some(bid) = man.get("build_id").and_then(|v| v.as_str()) {
                details.build_id = Some(bid.to_string());
            }
            if let Some(prereq) = man.get("prerequisites") {
                if let Some(p) = prereq.get("path").and_then(|v| v.as_str()) {
                    details.prerequisites.push(p.to_string());
                }
            }
        }

        assert_eq!(details.version.as_deref(), Some("643.1183087"));
        assert_eq!(details.cloud_saves, Some(false));
        assert_eq!(details.extra_launch_options.as_deref(), Some("-dx11"));
        assert_eq!(details.grant_date.as_deref(), Some("2025-01-02"));
        assert_eq!(details.platform.as_deref(), Some("Windows"));
        assert_eq!(details.installed_dlc, vec!["DLC_1"]);
        assert_eq!(details.launch_exe.as_deref(), Some("Launch_HLL.exe"));
        assert_eq!(details.build_id.as_deref(), Some("2iHOBO3GZ0GladcZ7pJrJg"));
        assert_eq!(
            details.prerequisites,
            vec!["EasyAntiCheat/EasyAntiCheat_EOS_Setup.exe"]
        );
        assert_eq!(details.download_size, Some(39746389618));
    }
}
