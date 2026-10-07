//! Game metadata extraction, caching, and background retrieval.
//!
//! Prioritizes local on-disk metadata (`~/.config/legendary/metadata/<app>.json`
//! and `installed.json`) which responds in < 1ms without network calls.
//! Only queries `legendary info <app> --json` lazily for settled selection
//! in a single background worker with 10s timeout.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::filesystem;
use crate::models::{Game, GameDetails, ManifestCacheEntry};

pub struct ManifestRequest {
    pub app_name: String,
    pub title: String,
    pub version: Option<String>,
}

pub struct MetadataManager {
    cache: HashMap<String, GameDetails>,
    _tx: Sender<(String, GameDetails)>,
    rx: Receiver<(String, GameDetails)>,
    pending: HashMap<String, bool>,
    protondb_cache: HashMap<String, String>,
    protondb_tx: Sender<(String, String)>,
    protondb_rx: Receiver<(String, String)>,
    protondb_dirty: bool,

    manifest_cache: HashMap<String, ManifestCacheEntry>,
    entitlements_map: HashMap<String, String>,
    pub store_slugs_cache: HashMap<String, String>,
    manifest_tx: Sender<ManifestRequest>,
    manifest_rx: Receiver<(String, Result<ManifestCacheEntry, String>)>,
    active_manifest_target: Arc<Mutex<Option<String>>>,
    slug_lookup_tx: Sender<(String, String, Option<String>)>,
    slug_lookup_rx: Receiver<(String, Result<String, String>)>,
    _cancel_flag: Arc<AtomicBool>,
}

impl MetadataManager {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        let (pdb_tx, pdb_rx) = channel();
        let pdb_cache = load_protondb_cache().unwrap_or_default();
        let manifest_cache = load_manifest_index();
        let entitlements_map = load_entitlements_map();
        let store_slugs_cache = load_store_slugs_cache();

        let (manifest_tx, manifest_rx_req) = channel::<ManifestRequest>();
        let (manifest_res_tx, manifest_rx) =
            channel::<(String, Result<ManifestCacheEntry, String>)>();
        let active_manifest_target = Arc::new(Mutex::new(None::<String>));
        let cancel_flag = Arc::new(AtomicBool::new(false));

        // Spawn single-concurrency manifest worker thread
        {
            let active_target = Arc::clone(&active_manifest_target);
            let cancel = Arc::clone(&cancel_flag);
            let res_tx = manifest_res_tx;
            thread::Builder::new()
                .name("manifest-worker".into())
                .spawn(move || {
                    let agent = ureq::AgentBuilder::new()
                        .timeout(Duration::from_secs(10))
                        .build();
                    while let Ok(req) = manifest_rx_req.recv() {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        // Check if superseded / stale
                        {
                            let cur = active_target.lock().unwrap();
                            if let Some(ref target) = *cur {
                                if target != &req.app_name {
                                    // Stale request dropped
                                    continue;
                                }
                            }
                        }

                        let res = fetch_and_cache_manifest(
                            &req.app_name,
                            &req.title,
                            req.version.as_deref(),
                            &agent,
                        );
                        let _ = res_tx.send((req.app_name, res));
                    }
                })
                .expect("failed to spawn manifest worker");
        }

        let (slug_lookup_tx, slug_rx_req) = channel::<(String, String, Option<String>)>();
        let (slug_res_tx, slug_lookup_rx) = channel::<(String, Result<String, String>)>();
        {
            thread::Builder::new()
                .name("slug-worker".into())
                .spawn(move || {
                    let agent = ureq::AgentBuilder::new()
                        .timeout(Duration::from_secs(5))
                        .build();
                    while let Ok((app_name, _title, ns_opt)) = slug_rx_req.recv() {
                        let res = if let Some(ref ns) = ns_opt {
                            match fetch_product_mappings(&agent) {
                                Ok(mappings) => {
                                    save_store_slugs_cache(&mappings);
                                    mappings
                                        .get(ns)
                                        .map(|s| s.trim_end_matches("/home").to_string())
                                        .ok_or_else(|| "Not found in productmapping".to_string())
                                }
                                Err(e) => Err(e),
                            }
                        } else {
                            Err("No namespace".to_string())
                        };
                        let _ = slug_res_tx.send((app_name, res));
                    }
                })
                .expect("failed to spawn slug worker");
        }

        Self {
            cache: HashMap::new(),
            _tx: tx,
            rx,
            pending: HashMap::new(),
            protondb_cache: pdb_cache,
            protondb_tx: pdb_tx,
            protondb_rx: pdb_rx,
            protondb_dirty: false,
            manifest_cache,
            entitlements_map,
            store_slugs_cache,
            manifest_tx,
            manifest_rx,
            active_manifest_target,
            slug_lookup_tx,
            slug_lookup_rx,
            _cancel_flag: cancel_flag,
        }
    }

    pub fn get_protondb_tier(&self, app_name: &str) -> Option<&str> {
        self.protondb_cache.get(app_name).map(|s| s.as_str())
    }

    pub fn get_cached(&self, app_name: &str) -> Option<GameDetails> {
        self.cache.get(app_name).cloned().map(|mut d| {
            if let Some(m) = self.manifest_cache.get(app_name) {
                merge_manifest_into_details(&mut d, m);
            }
            d
        })
    }

    /// Bulk prefetch ProtonDB ratings concurrently for all games in the library in one go.
    pub fn bulk_prefetch_protondb(&mut self, games: &[Game]) {
        let missing: Vec<(String, String)> = games
            .iter()
            .filter(|g| {
                !self.protondb_cache.contains_key(&g.app_name)
                    && !self.pending.contains_key(&g.app_name)
            })
            .map(|g| (g.app_name.clone(), g.title.clone()))
            .collect();

        if missing.is_empty() {
            return;
        }

        for (id, _) in &missing {
            self.pending.insert(id.clone(), true);
        }

        let num_threads = 16.min(missing.len()).max(1);
        let mut chunks = vec![Vec::new(); num_threads];
        for (idx, item) in missing.into_iter().enumerate() {
            chunks[idx % num_threads].push(item);
        }

        for chunk in chunks {
            if chunk.is_empty() {
                continue;
            }
            let tx = self.protondb_tx.clone();
            thread::spawn(move || {
                for (app_name, title) in chunk {
                    let tier = fetch_protondb_tier(&title).unwrap_or_else(|| "Unknown".into());
                    let _ = tx.send((app_name, tier));
                }
            });
        }
    }

    /// Try to get details immediately. Offline fields load instantly with zero network.
    pub fn get_or_load(&mut self, game: &Game) -> Option<GameDetails> {
        if let Some(mut d) = self.cache.get(&game.app_name).cloned() {
            if d.protondb_tier.is_none() {
                if let Some(tier) = self.protondb_cache.get(&game.app_name) {
                    d.protondb_tier = Some(tier.clone());
                    self.cache.insert(game.app_name.clone(), d.clone());
                }
            }
            if let Some(m) = self.manifest_cache.get(&game.app_name) {
                merge_manifest_into_details(&mut d, m);
            }
            return Some(d);
        }

        // Try cache file on disk
        if let Some(mut d) = load_cached_details(&game.app_name) {
            if d.cover_url.is_none() {
                let meta_path =
                    filesystem::legendary_metadata_dir().join(format!("{}.json", game.app_name));
                if let Ok(bytes) = std::fs::read(&meta_path) {
                    if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                        parse_legendary_metadata_json_with_entitlements(
                            &val,
                            &mut d,
                            Some(&self.entitlements_map),
                        );
                        save_cached_details(&d);
                    }
                }
            }
            if d.protondb_tier.is_none() {
                if let Some(tier) = self.protondb_cache.get(&game.app_name) {
                    d.protondb_tier = Some(tier.clone());
                }
            }
            if let Some(m) = self.manifest_cache.get(&game.app_name) {
                merge_manifest_into_details(&mut d, m);
            }
            self.cache.insert(game.app_name.clone(), d.clone());
            return Some(d);
        }

        // Try parsing legendary's local metadata and entitlements (instant offline)
        if let Some(mut d) = load_from_legendary_files(game, Some(&self.entitlements_map)) {
            if d.protondb_tier.is_none() {
                if let Some(tier) = self.protondb_cache.get(&game.app_name) {
                    d.protondb_tier = Some(tier.clone());
                }
            }
            if let Some(m) = self.manifest_cache.get(&game.app_name) {
                merge_manifest_into_details(&mut d, m);
            }
            save_cached_details(&d);
            self.cache.insert(game.app_name.clone(), d.clone());
            return Some(d);
        }

        None
    }

    pub fn load_from_cache(&self, app_name: &str) -> Option<&GameDetails> {
        self.cache.get(app_name)
    }

    pub fn request_manifest_if_needed(
        &mut self,
        app_name: &str,
        title: &str,
        version: Option<&str>,
    ) {
        if let Some(entry) = self.manifest_cache.get(app_name) {
            let now_sec = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if entry.schema_version == 1
                && now_sec.saturating_sub(entry.fetched_at) <= 30 * 86400
                && (version.is_none() || entry.version.as_deref() == version)
            {
                return;
            }
        }

        if self.pending.contains_key(app_name) {
            return;
        }

        self.pending.insert(app_name.to_string(), true);
        {
            let mut target = self.active_manifest_target.lock().unwrap();
            *target = Some(app_name.to_string());
        }

        if let Some(d) = self.cache.get_mut(app_name) {
            d.manifest_loading = true;
        }

        let _ = self.manifest_tx.send(ManifestRequest {
            app_name: app_name.to_string(),
            title: title.to_string(),
            version: version.map(|s| s.to_string()),
        });
    }

    pub fn resolve_store_slug(
        &mut self,
        app_name: &str,
        title: &str,
        namespace: Option<&str>,
        metadata_slug: Option<&str>,
    ) -> Result<String, bool> {
        if let Some(slug) = metadata_slug {
            let clean = slug.trim().trim_end_matches("/home");
            if !clean.is_empty() {
                return Ok(clean.to_string());
            }
        }

        if let Some(slug) = self.store_slugs_cache.get(app_name) {
            if !slug.is_empty() && slug != "NONE" {
                return Ok(slug.trim_end_matches("/home").to_string());
            }
        }
        if let Some(ns) = namespace {
            if let Some(slug) = self.store_slugs_cache.get(ns) {
                if !slug.is_empty() && slug != "NONE" {
                    return Ok(slug.trim_end_matches("/home").to_string());
                }
            }
        }

        let _ = self.slug_lookup_tx.send((
            app_name.to_string(),
            title.to_string(),
            namespace.map(|s| s.to_string()),
        ));
        Err(true)
    }

    /// Poll for any completed background fetches. Returns true if cache was updated.
    pub fn poll_updates(&mut self) -> bool {
        let mut updated = false;

        while let Ok((app, res)) = self.manifest_rx.try_recv() {
            self.pending.remove(&app);
            if let Ok(entry) = res {
                if let Some(d) = self.cache.get_mut(&app) {
                    merge_manifest_into_details(d, &entry);
                }
                self.manifest_cache.insert(app, entry);
                updated = true;
            } else if let Some(d) = self.cache.get_mut(&app) {
                d.manifest_loading = false;
                updated = true;
            }
        }

        while let Ok((id, details)) = self.rx.try_recv() {
            self.pending.remove(&id);
            self.cache.insert(id, details);
            updated = true;
        }

        while let Ok((id, tier)) = self.protondb_rx.try_recv() {
            self.pending.remove(&id);
            self.protondb_cache.insert(id.clone(), tier.clone());
            self.protondb_dirty = true;
            if let Some(details) = self.cache.get_mut(&id) {
                details.protondb_tier = Some(tier);
                save_cached_details(details);
            }
            updated = true;
        }
        if self.protondb_dirty {
            save_protondb_cache(&self.protondb_cache);
            self.protondb_dirty = false;
        }

        while let Ok((app, res)) = self.slug_lookup_rx.try_recv() {
            if let Ok(slug) = res {
                self.store_slugs_cache.insert(app.clone(), slug.clone());
                if let Some(d) = self.cache.get_mut(&app) {
                    d.store_slug = Some(slug);
                }
                updated = true;
            } else {
                self.store_slugs_cache.insert(app, "NONE".into());
            }
        }

        updated
    }
}

fn protondb_cache_file() -> PathBuf {
    filesystem::cache_dir().join("protondb.json")
}

pub fn load_protondb_cache() -> Option<HashMap<String, String>> {
    let bytes = std::fs::read(protondb_cache_file()).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn save_protondb_cache(cache: &HashMap<String, String>) {
    if let Ok(bytes) = serde_json::to_vec_pretty(cache) {
        let _ = filesystem::atomic_write(&protondb_cache_file(), &bytes);
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

pub fn load_manifest_index() -> HashMap<String, ManifestCacheEntry> {
    let mut index = HashMap::new();
    let dir = filesystem::info_cache_dir();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Ok(bytes) = std::fs::read(&path) {
                    if let Ok(item) = serde_json::from_slice::<ManifestCacheEntry>(&bytes) {
                        index.insert(item.app_name.clone(), item);
                    }
                }
            }
        }
    }
    index
}

pub fn save_manifest_entry(app_name: &str, entry: &ManifestCacheEntry) {
    let dir = filesystem::info_cache_dir();
    let _ = filesystem::ensure_dir(&dir);
    let path = dir.join(format!("{app_name}.json"));
    if let Ok(bytes) = serde_json::to_vec_pretty(entry) {
        let _ = filesystem::atomic_write(&path, &bytes);
    }
}

pub fn load_entitlements_map() -> HashMap<String, String> {
    let mut map = HashMap::new();
    let path = filesystem::home().join(".config/legendary/entitlements.json");
    if let Ok(bytes) = std::fs::read(path) {
        if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            if let Some(arr) = val.as_array() {
                for item in arr {
                    if let Some(gd) = item.get("grantDate").and_then(|v| v.as_str()) {
                        let date = gd.split('T').next().unwrap_or(gd).to_string();
                        if let Some(ns) = item.get("namespace").and_then(|v| v.as_str()) {
                            map.insert(ns.to_string(), date.clone());
                        }
                        if let Some(cid) = item.get("catalogItemId").and_then(|v| v.as_str()) {
                            map.insert(cid.to_string(), date.clone());
                        }
                        if let Some(en) = item.get("entitlementName").and_then(|v| v.as_str()) {
                            map.insert(en.to_string(), date.clone());
                        }
                    }
                }
            }
        }
    }
    map
}

fn store_slugs_file() -> PathBuf {
    filesystem::cache_dir().join("store_slugs.json")
}

pub fn load_store_slugs_cache() -> HashMap<String, String> {
    let path = store_slugs_file();
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(map) = serde_json::from_slice::<HashMap<String, String>>(&bytes) {
            return map;
        }
    }
    HashMap::new()
}

pub fn save_store_slugs_cache(mappings: &HashMap<String, String>) {
    let path = store_slugs_file();
    if let Ok(bytes) = serde_json::to_vec_pretty(mappings) {
        let _ = filesystem::atomic_write(&path, &bytes);
    }
}

pub fn fetch_product_mappings(agent: &ureq::Agent) -> Result<HashMap<String, String>, String> {
    let resp = agent
        .get("https://store-content-ipv4.ak.epicgames.com/api/content/productmapping")
        .call()
        .map_err(|e| format!("HTTP request failed: {e}"))?;

    let val: serde_json::Value = serde_json::from_reader(resp.into_reader())
        .map_err(|e| format!("JSON parse error: {e}"))?;

    let mut map = HashMap::new();
    if let Some(obj) = val.as_object() {
        for (k, v) in obj {
            if let Some(s) = v.as_str() {
                map.insert(k.clone(), s.to_string());
            }
        }
    }
    Ok(map)
}

pub fn merge_manifest_into_details(details: &mut GameDetails, manifest: &ManifestCacheEntry) {
    if details.download_size.is_none() {
        details.download_size = manifest.download_size;
    }
    if !details.installed && details.installed_size.is_none() {
        details.installed_size = manifest.disk_size;
    }
    if details.launch_exe.is_none() {
        details.launch_exe = manifest.launch_exe.clone();
    }
    if details.version.is_none() {
        details.version = manifest.version.clone();
    }
    if details.prerequisites.is_empty() {
        details.prerequisites = manifest.prerequisites.clone();
    }
    if details.num_files.is_none() {
        details.num_files = manifest.num_files;
    }
    if details.cloud_save_folder.is_none() {
        if let Some(ref f) = manifest.cloud_save_folder {
            details.cloud_save_folder = Some(f.clone());
            details.cloud_saves = Some(true);
        }
    }
    if let Some(ref slug) = manifest.store_slug {
        if details.store_slug.is_none() {
            details.store_slug = Some(slug.clone());
        }
    }
    let is_dup = details
        .description
        .as_deref()
        .map(|d| d.trim().eq_ignore_ascii_case(details.title.trim()))
        .unwrap_or(true);
    if is_dup {
        if let Some(ref desc) = manifest.description {
            if !desc.trim().is_empty() && !desc.trim().eq_ignore_ascii_case(details.title.trim()) {
                details.description = Some(desc.clone());
            }
        }
    }
    details.manifest_cached = true;
    details.manifest_loading = false;
}

fn urlencoding_simple(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b'~' {
            out.push(b as char);
        } else if b == b' ' {
            out.push('+');
        } else {
            use std::fmt::Write;
            let _ = write!(out, "%{:02X}", b);
        }
    }
    out
}

pub fn resolve_steam_app_id(title: &str, agent: &ureq::Agent) -> Option<u64> {
    let clean = title.replace(['™', '®', '©'], "").trim().to_string();
    if clean.is_empty() {
        return None;
    }
    let url = format!(
        "https://store.steampowered.com/api/storesearch/?term={}&l=english&cc=US",
        urlencoding_simple(&clean)
    );
    let resp = agent.get(&url).call().ok()?;
    let resp_val: serde_json::Value = serde_json::from_reader(resp.into_reader()).ok()?;
    let items = resp_val.get("items")?.as_array()?;
    let steam_id = items
        .iter()
        .find(|item| {
            item.get("name")
                .and_then(|n| n.as_str())
                .map(|n| n.eq_ignore_ascii_case(&clean))
                .unwrap_or(false)
        })
        .or_else(|| items.first())
        .and_then(|item| item.get("id"))
        .and_then(|id| id.as_u64())?;
    Some(steam_id)
}

pub fn sanitize_description(desc: &str) -> String {
    let mut clean = String::with_capacity(desc.len());
    let mut tag_buf = String::new();
    let mut in_tag = false;

    for ch in desc.chars() {
        if ch == '<' {
            in_tag = true;
            tag_buf.clear();
        } else if ch == '>' {
            in_tag = false;
            let tag_lower = tag_buf.to_ascii_lowercase();
            let tag_name = tag_lower
                .trim_start_matches('/')
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_end_matches('/');
            let is_break = matches!(
                tag_name,
                "br" | "p"
                    | "div"
                    | "li"
                    | "ul"
                    | "ol"
                    | "h1"
                    | "h2"
                    | "h3"
                    | "h4"
                    | "h5"
                    | "h6"
                    | "tr"
            );
            if is_break {
                clean.push('\n');
            }
        } else if in_tag {
            tag_buf.push(ch);
        } else {
            clean.push(ch);
        }
    }

    let decoded = clean
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&#x27;", "'")
        .replace("&#x2F;", "/");

    let mut out = String::with_capacity(decoded.len());
    let mut prev_newlines = 0;

    for line in decoded.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if prev_newlines < 1 && !out.is_empty() {
                out.push('\n');
                prev_newlines += 1;
            }
        } else {
            if !out.is_empty() && prev_newlines == 0 {
                out.push(' ');
            }
            out.push_str(trimmed);
            prev_newlines = 0;
        }
    }

    if out.len() > 4000 {
        let mut truncated = out[..4000].to_string();
        if let Some(last_space) = truncated.rfind(' ') {
            truncated.truncate(last_space);
        }
        truncated.push('…');
        truncated
    } else {
        out
    }
}

pub fn fetch_steam_description(app_id: u64, agent: &ureq::Agent) -> Option<String> {
    let url = format!("https://store.steampowered.com/api/appdetails?appids={app_id}&l=english");
    let resp = agent.get(&url).call().ok()?;
    let resp_val: serde_json::Value = serde_json::from_reader(resp.into_reader()).ok()?;
    let app_obj = resp_val.get(app_id.to_string())?;
    if !app_obj
        .get("success")
        .and_then(|s| s.as_bool())
        .unwrap_or(false)
    {
        return None;
    }
    let data = app_obj.get("data")?;
    let raw_desc = data
        .get("short_description")
        .and_then(|s| s.as_str())
        .filter(|s| !s.trim().is_empty())
        .or_else(|| data.get("detailed_description").and_then(|s| s.as_str()))?;
    let sanitized = sanitize_description(raw_desc);
    if sanitized.is_empty() {
        None
    } else {
        Some(sanitized)
    }
}

pub fn fetch_and_cache_manifest(
    app_name: &str,
    title: &str,
    version: Option<&str>,
    agent: &ureq::Agent,
) -> Result<ManifestCacheEntry, String> {
    let mut details = fetch_legendary_info(app_name)
        .ok_or_else(|| format!("Failed to fetch legendary info for {app_name}"))?;

    let is_dup = details
        .description
        .as_deref()
        .map(|d| d.trim().eq_ignore_ascii_case(title.trim()))
        .unwrap_or(true);
    if is_dup {
        if let Some(steam_id) = resolve_steam_app_id(title, agent) {
            if let Some(steam_desc) = fetch_steam_description(steam_id, agent) {
                details.description = Some(steam_desc);
            }
        }
    }

    let now_sec = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let entry = ManifestCacheEntry {
        schema_version: 1,
        app_name: app_name.to_string(),
        version: details
            .version
            .clone()
            .or_else(|| version.map(|v| v.to_string())),
        download_size: details.download_size,
        disk_size: details.installed_size,
        launch_exe: details.launch_exe.clone(),
        prerequisites: details.prerequisites.clone(),
        num_files: details.num_files,
        cloud_save_folder: details.cloud_save_folder.clone(),
        description: details.description.clone(),
        store_slug: details.store_slug.clone(),
        fetched_at: now_sec,
    };

    save_manifest_entry(app_name, &entry);
    Ok(entry)
}

/// Read details from legendary's local disk files without running any subprocess.
pub fn load_from_legendary_files(
    game: &Game,
    entitlements: Option<&HashMap<String, String>>,
) -> Option<GameDetails> {
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
        if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            let item_opt = if let Some(items) = val.as_array() {
                items
                    .iter()
                    .find(|i| i.get("app_name").and_then(|v| v.as_str()) == Some(&game.app_name))
            } else if let Some(map) = val.as_object() {
                map.get(&game.app_name)
            } else {
                None
            };
            if let Some(item) = item_opt {
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
                if details.version.is_none() {
                    if let Some(v) = item.get("version").and_then(|v| v.as_str()) {
                        if !v.is_empty() {
                            details.version = Some(v.to_string());
                        }
                    }
                }
            }
        }
    }

    // Check ~/.config/legendary/metadata/<app_name>.json
    let meta_path = filesystem::legendary_metadata_dir().join(format!("{}.json", game.app_name));
    if let Ok(bytes) = std::fs::read(&meta_path) {
        if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            parse_legendary_metadata_json_with_entitlements(&val, &mut details, entitlements);
            return Some(details);
        }
    }

    Some(details)
}

pub fn parse_legendary_metadata_json(val: &serde_json::Value, details: &mut GameDetails) {
    parse_legendary_metadata_json_with_entitlements(val, details, None);
}

pub fn parse_legendary_metadata_json_with_entitlements(
    val: &serde_json::Value,
    details: &mut GameDetails,
    entitlements: Option<&HashMap<String, String>>,
) {
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
            if let Some(tp) = custom
                .get("ThirdPartyManagedApp")
                .and_then(|v| v.get("value"))
                .and_then(|v| v.as_str())
            {
                if !tp.is_empty() {
                    details.third_party_managed = Some(tp.to_string());
                }
            }
            if let Some(cs_val) = custom
                .get("CloudSaveFolder")
                .and_then(|v| v.get("value"))
                .and_then(|v| v.as_str())
            {
                if !cs_val.is_empty() {
                    details.cloud_saves = Some(true);
                    details.cloud_save_folder = Some(cs_val.to_string());
                } else {
                    details.cloud_saves = Some(false);
                }
            } else if custom.is_object() && details.cloud_saves.is_none() {
                details.cloud_saves = Some(false);
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
        if let Some(slug) = meta.get("productSlug").and_then(|v| v.as_str()) {
            let clean = slug.trim().trim_end_matches("/home");
            if !clean.is_empty() && details.store_slug.is_none() {
                details.store_slug = Some(clean.to_string());
            }
        }
        if details.platform.is_none() {
            if let Some(rel) = meta.get("releaseInfo").and_then(|v| v.as_array()) {
                for r in rel {
                    if let Some(plat_arr) = r.get("platform").and_then(|v| v.as_array()) {
                        let plats: Vec<String> = plat_arr
                            .iter()
                            .filter_map(|p| p.as_str().map(|s| s.to_string()))
                            .collect();
                        if !plats.is_empty() {
                            details.platform = Some(plats.join(", "));
                            break;
                        }
                    }
                }
            }
        }
        if details.owned_dlc.is_empty() {
            if let Some(dlcs) = meta.get("dlcItemList").and_then(|v| v.as_array()) {
                let list: Vec<String> = dlcs
                    .iter()
                    .filter_map(|d| {
                        d.get("title")
                            .and_then(|t| t.as_str())
                            .map(|s| s.to_string())
                    })
                    .collect();
                if !list.is_empty() {
                    details.owned_dlc = list;
                }
            }
        }
        if let Some(imgs) = meta.get("keyImages").and_then(|v| v.as_array()) {
            if let Some(url) = select_best_game_image(imgs) {
                details.cover_url = Some(url);
            }
        }
        if details.grant_date.is_none() {
            if let Some(entitlements) = entitlements {
                let ns = meta.get("namespace").and_then(|v| v.as_str());
                let id = meta.get("id").and_then(|v| v.as_str());
                if let Some(ns) = ns {
                    if let Some(gd) = entitlements.get(ns) {
                        details.grant_date = Some(gd.clone());
                    }
                }
                if details.grant_date.is_none() {
                    if let Some(id) = id {
                        if let Some(gd) = entitlements.get(id) {
                            details.grant_date = Some(gd.clone());
                        }
                    }
                }
            }
        }
    }

    if details.version.is_none() {
        if let Some(assets) = val.get("asset_infos").and_then(|v| v.as_object()) {
            for (_k, v) in assets {
                if let Some(bv) = v.get("build_version").and_then(|b| b.as_str()) {
                    if !bv.is_empty() {
                        details.version = Some(bv.to_string());
                        break;
                    }
                }
            }
        }
    }
}

/// Select the best cover/poster image URL from an array of Epic keyImages.
/// Prioritizes portrait box art (DieselGameBoxTall / OfferImageTall),
/// then thumbnail, then wide box art, then background/hero, then any valid image URL.
pub fn select_best_game_image(images: &[serde_json::Value]) -> Option<String> {
    let mut best_url: Option<String> = None;
    let mut best_rank: i32 = -1;

    for img in images {
        let url = match img.get("url").and_then(|v| v.as_str()) {
            Some(u) if u.starts_with("http://") || u.starts_with("https://") => u,
            _ => continue,
        };
        let img_type = img.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let lower = img_type.to_lowercase();

        let rank: i32 = match img_type {
            "DieselGameBoxTall" => 10,
            "OfferImageTall" => 9,
            "DieselStoreFrontVertical" => 8,
            _ if lower.contains("tall") || lower.contains("vertical") => 7,
            "Thumbnail" => 6,
            _ if lower.contains("thumb") => 6,
            "DieselGameBox" => 5,
            "DieselGameBoxWide" => 4,
            "OfferImageWide" => 3,
            _ if lower.contains("wide") || lower.contains("box") => 2,
            "Featured" => 1,
            _ if lower.contains("hero")
                || lower.contains("background")
                || lower.contains("featured") =>
            {
                1
            }
            _ => 0,
        };

        if rank > best_rank {
            best_rank = rank;
            best_url = Some(url.to_string());
        }
    }

    best_url
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
        if let Some(ea) = game.get("external_activation").and_then(|v| v.as_str()) {
            if !ea.is_empty() {
                details.external_activation = Some(ea.to_string());
            }
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

/// Fetch ProtonDB compatibility tier by searching game title on Steam, then querying ProtonDB.
pub fn fetch_protondb_tier(title: &str) -> Option<String> {
    // 1. Clean the title: remove (R), (TM), etc.
    let clean_title = title.replace(['™', '®', '©'], "").trim().to_string();

    if clean_title.is_empty() {
        return None;
    }

    // 2. Query Steam storesearch
    let search_out = std::process::Command::new("curl")
        .args([
            "-s",
            "-m",
            "4",
            "-G",
            "https://store.steampowered.com/api/storesearch/",
            "--data-urlencode",
            &format!("term={clean_title}"),
            "--data-urlencode",
            "l=english",
            "--data-urlencode",
            "cc=US",
        ])
        .output()
        .ok()?;

    if !search_out.status.success() {
        return None;
    }

    let search_json: serde_json::Value = serde_json::from_slice(&search_out.stdout).ok()?;
    let items = search_json.get("items")?.as_array()?;
    if items.is_empty() {
        return None;
    }

    let steam_id = items
        .iter()
        .find(|item| {
            item.get("name")
                .and_then(|n| n.as_str())
                .map(|n| n.eq_ignore_ascii_case(&clean_title))
                .unwrap_or(false)
        })
        .or_else(|| items.first())
        .and_then(|item| item.get("id"))
        .and_then(|id| id.as_u64())?;

    // 3. Query ProtonDB summary
    let proton_out = std::process::Command::new("curl")
        .args([
            "-s",
            "-m",
            "4",
            &format!("https://www.protondb.com/api/v1/reports/summaries/{steam_id}.json"),
        ])
        .output()
        .ok()?;

    if !proton_out.status.success() {
        return None;
    }

    let proton_json: serde_json::Value = serde_json::from_slice(&proton_out.stdout).ok()?;
    let raw_tier = proton_json.get("tier")?.as_str()?;

    let lower = raw_tier.to_lowercase();
    let tier = match lower.as_str() {
        "platinum" => "Platinum",
        "gold" => "Gold",
        "silver" => "Silver",
        "bronze" => "Bronze",
        "borked" => "Borked",
        "native" => "Native",
        _ => raw_tier,
    };

    Some(tier.to_string())
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

    #[test]
    fn test_select_best_game_image() {
        let json = serde_json::json!([
            {
                "type": "DieselGameBox",
                "url": "https://cdn.example.com/box.jpg"
            },
            {
                "type": "DieselGameBoxTall",
                "url": "https://cdn.example.com/tall.jpg"
            },
            {
                "type": "OfferImageWide",
                "url": "https://cdn.example.com/wide.jpg"
            }
        ]);
        let selected = super::select_best_game_image(json.as_array().unwrap());
        assert_eq!(
            selected.as_deref(),
            Some("https://cdn.example.com/tall.jpg")
        );
    }

    #[test]
    fn test_parse_dishonored_fixture() {
        let json_str = r#"{
            "app_name": "d2da64bd4c4e422da4b1a88041492a3a",
            "metadata": {
                "namespace": "766e1b59c32c447093add5efbc1a4858",
                "id": "19884196ffca4ad0b56486a76773e5f9",
                "developer": "Arkane Studios",
                "releaseInfo": [
                    { "platform": ["Windows"] }
                ],
                "customAttributes": {
                    "CloudSaveFolder": {
                        "type": "STRING",
                        "value": "{UserDir}/My Games/Dishonored/DishonoredGame/SaveData/{EpicID}/"
                    }
                }
            },
            "asset_infos": {
                "Windows": {
                    "build_version": "1.6"
                }
            }
        }"#;

        let val: serde_json::Value = serde_json::from_str(json_str).unwrap();
        let mut details = GameDetails {
            app_name: "d2da64bd4c4e422da4b1a88041492a3a".into(),
            title: "Dishonored - Definitive Edition".into(),
            ..Default::default()
        };

        let mut entitlements = HashMap::new();
        entitlements.insert(
            "766e1b59c32c447093add5efbc1a4858".into(),
            "2022-12-31".into(),
        );

        parse_legendary_metadata_json_with_entitlements(&val, &mut details, Some(&entitlements));

        assert_eq!(details.version.as_deref(), Some("1.6"));
        assert_eq!(details.platform.as_deref(), Some("Windows"));
        assert_eq!(details.cloud_saves, Some(true));
        assert_eq!(
            details.cloud_save_folder.as_deref(),
            Some("{UserDir}/My Games/Dishonored/DishonoredGame/SaveData/{EpicID}/")
        );
        assert_eq!(details.grant_date.as_deref(), Some("2022-12-31"));
    }

    #[test]
    fn test_sanitize_description() {
        let html = "<p>Dishonored is an <b>immersive</b> first-person action game &amp; stealth thriller.</p><br/><br/>Features &quot;all DLC&quot; &amp; graphics!";
        let clean = sanitize_description(html);
        assert_eq!(
            clean,
            "Dishonored is an immersive first-person action game & stealth thriller.\nFeatures \"all DLC\" & graphics!"
        );

        // Long text capping
        let long = "word ".repeat(1000);
        let sanitized = sanitize_description(&long);
        assert!(sanitized.len() <= 4004);
        assert!(sanitized.ends_with('…'));
    }

    #[test]
    fn test_store_slug_resolution_order() {
        let mut mgr = MetadataManager::new();
        mgr.store_slugs_cache
            .insert("app_mapped".into(), "mapped-slug/home".into());

        // 1. Metadata slug takes highest priority and strips /home
        let res1 = mgr.resolve_store_slug("app_mapped", "Title", None, Some("meta-slug/home"));
        assert_eq!(res1, Ok("meta-slug".to_string()));

        // 2. Cache mapping used when metadata slug absent, strips /home
        let res2 = mgr.resolve_store_slug("app_mapped", "Title", None, None);
        assert_eq!(res2, Ok("mapped-slug".to_string()));
    }
}
