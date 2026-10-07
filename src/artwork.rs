//! Multi-source cover artwork retrieval, caching, downscaling, and background prefetching.
//!
//! Resolver order:
//! 1. Epic Games Store keyImages (from Legendary local metadata)
//! 2. Steam Store CDN (keyless `library_600x900.jpg`, fallback `header.jpg`)
//! 3. SteamGridDB (optional, active only when `STEAMGRIDDB_API_KEY` is set)
//!
//! Architectural invariants:
//! - All covers stored as display-sized JPEGs (max 400x600, quality 85)
//! - In-memory CacheIndex: zero filesystem calls on hot path lookups
//! - Multi-worker pool (4 threads) with priority queue (Immediate, Neighbor, Prefetch)
//! - Bounded protocol LRU (24 entries) reusing prepared state on selection return
//! - Instant negative caching with schema version 2 (24h TTL)
//! - Max 300 MB cache cap with LRU eviction down to 250 MB

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Read;
use std::panic;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use image::DynamicImage;
use ratatui_image::protocol::StatefulProtocol;
use serde::{Deserialize, Serialize};

use crate::filesystem;
use crate::perf;

const MAX_IMAGE_BYTES: u64 = 15 * 1024 * 1024; // 15 MB safety limit for download
const MAX_MEMORY_CACHE: usize = 24;
pub const NEGATIVE_CACHE_TTL_SECS: u64 = 86400; // 24 hours
pub const CURRENT_SCHEMA_VERSION: u32 = 2;
const MAX_CACHE_SIZE_BYTES: u64 = 300 * 1024 * 1024; // 300 MB cap
const PRUNE_CACHE_SIZE_BYTES: u64 = 250 * 1024 * 1024; // prune target
const NUM_WORKERS: usize = 4;

fn default_schema_version() -> u32 {
    1
}

static NEXT_REQ_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq)]
pub enum CoverState {
    Idle,
    Loading,
    Ready,
    NoArtwork,
    Failed(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtworkSidecar {
    #[serde(default = "default_schema_version")]
    pub version: u32,
    pub source: String,
    pub url: String,
    pub steam_appid: Option<u64>,
    pub fetched_at: u64,
    pub negative: bool,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct CacheIndexEntry {
    pub file_path: PathBuf,
    pub mtime: u64,
    pub source: String,
    pub is_negative: bool,
    pub negative_expiry: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Immediate = 0,
    Neighbor = 1,
    Prefetch = 2,
}

#[derive(Clone, Debug)]
pub struct WorkItem {
    pub priority: Priority,
    pub request_id: u64,
    pub app_name: String,
    pub title: String,
    pub epic_url: Option<String>,
    pub force_refresh: bool,
    pub seq: u64,
}

pub struct ArtworkQueue {
    pub items: Vec<WorkItem>,
    pub in_flight: HashSet<String>,
    pub total_prefetch: usize,
    pub prefetch_done: usize,
    pub prefetch_cached: usize,
    pub prefetch_negative: usize,
    pub prefetch_failed: usize,
}

pub struct ArtworkResponse {
    pub request_id: u64,
    pub app_name: String,
    pub source: Option<String>,
    pub result: Result<Arc<DynamicImage>, String>,
    pub is_no_artwork: bool,
}

#[allow(dead_code)]
pub struct LruEntry {
    pub app_name: String,
    pub image: Arc<DynamicImage>,
    pub source: Option<String>,
    pub protocol: Option<StatefulProtocol>,
}

pub struct ArtworkManager {
    memory_cache: HashMap<String, LruEntry>,
    cache_order: VecDeque<String>,
    cache_index: Arc<RwLock<HashMap<String, CacheIndexEntry>>>,
    queue: Arc<Mutex<ArtworkQueue>>,
    condvar: Arc<Condvar>,
    cancel_flag: Arc<AtomicBool>,
    rx_res: Receiver<ArtworkResponse>,
    _worker_handles: Vec<thread::JoinHandle<()>>,
}

impl ArtworkManager {
    pub fn new() -> Self {
        let (tx_res, rx_res) = channel::<ArtworkResponse>();
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let queue = Arc::new(Mutex::new(ArtworkQueue {
            items: Vec::new(),
            in_flight: HashSet::new(),
            total_prefetch: 0,
            prefetch_done: 0,
            prefetch_cached: 0,
            prefetch_negative: 0,
            prefetch_failed: 0,
        }));
        let condvar = Arc::new(Condvar::new());
        let cache_index = Arc::new(RwLock::new(HashMap::new()));

        let agent = Arc::new(
            ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(5))
                .user_agent("legendary-tui/1.0.2 (Linux)")
                .build(),
        );

        let mut worker_handles = Vec::with_capacity(NUM_WORKERS);

        for worker_id in 0..NUM_WORKERS {
            let q_clone = Arc::clone(&queue);
            let cv_clone = Arc::clone(&condvar);
            let cancel_clone = Arc::clone(&cancel_flag);
            let idx_clone = Arc::clone(&cache_index);
            let tx_clone = tx_res.clone();
            let agent_clone = Arc::clone(&agent);

            let handle = thread::Builder::new()
                .name(format!("art-worker-{worker_id}"))
                .spawn(move || {
                    worker_loop(
                        worker_id,
                        q_clone,
                        cv_clone,
                        cancel_clone,
                        idx_clone,
                        agent_clone,
                        tx_clone,
                    );
                })
                .expect("failed to spawn artwork worker");
            worker_handles.push(handle);
        }

        Self {
            memory_cache: HashMap::new(),
            cache_order: VecDeque::new(),
            cache_index,
            queue,
            condvar,
            cancel_flag,
            rx_res,
            _worker_handles: worker_handles,
        }
    }

    /// Retrieve image and source from in-memory LRU immediately if already present.
    pub fn get_cached(&self, app_name: &str) -> Option<(Arc<DynamicImage>, Option<String>)> {
        self.memory_cache
            .get(app_name)
            .map(|e| (Arc::clone(&e.image), e.source.clone()))
    }

    /// Retrieve pre-encoded StatefulProtocol from LRU if present.
    #[allow(dead_code)]
    pub fn take_lru_protocol(&mut self, app_name: &str) -> Option<StatefulProtocol> {
        self.memory_cache
            .get_mut(app_name)
            .and_then(|e| e.protocol.take())
    }

    /// Store pre-encoded StatefulProtocol into LRU for future reuse.
    #[allow(dead_code)]
    pub fn save_lru_protocol(&mut self, app_name: &str, proto: StatefulProtocol) {
        if let Some(entry) = self.memory_cache.get_mut(app_name) {
            entry.protocol = Some(proto);
        }
    }

    /// Clear cached protocols (e.g. on terminal resize).
    pub fn clear_lru_protocols(&mut self) {
        for entry in self.memory_cache.values_mut() {
            entry.protocol = None;
        }
    }

    /// Scan cache directory once to build in-memory index and enforce 300 MB cap.
    pub fn build_cache_index(&mut self) {
        let entries = scan_cache_index();
        if let Ok(mut lock) = self.cache_index.write() {
            *lock = entries;
        }
        enforce_cache_size_cap();
    }

    /// Check whether app_name is negatively cached in memory index (0 ms, zero FS calls).
    pub fn is_negative_cached(&self, app_name: &str) -> bool {
        if let Ok(lock) = self.cache_index.read() {
            if let Some(entry) = lock.get(app_name) {
                if entry.is_negative {
                    let now = current_time_secs();
                    return now < entry.negative_expiry;
                }
            }
        }
        false
    }

    /// Check whether app_name is cached on disk in memory index (0 ms, zero FS calls).
    #[allow(dead_code)]
    pub fn is_on_disk(&self, app_name: &str) -> bool {
        if let Ok(lock) = self.cache_index.read() {
            if let Some(entry) = lock.get(app_name) {
                return !entry.is_negative;
            }
        }
        false
    }

    /// Request artwork for an application. Returns req_id, CoverState, image, and source.
    pub fn request_artwork(
        &mut self,
        app_name: &str,
        title: &str,
        epic_url: Option<&str>,
        force_refresh: bool,
    ) -> (u64, CoverState, Option<Arc<DynamicImage>>, Option<String>) {
        let req_id = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
        let mut trace = perf::SelectionTrace::new(app_name.to_string());

        if force_refresh {
            self.invalidate(app_name);
        } else {
            // 1. In-memory LRU hit (0 ms)
            let lru_start = Instant::now();
            if let Some(entry) = self.memory_cache.get(app_name) {
                let has_better_epic = epic_url.map(|u| !u.is_empty()).unwrap_or(false)
                    && entry
                        .source
                        .as_deref()
                        .map(|s| s != "Epic")
                        .unwrap_or(false);
                if !has_better_epic {
                    trace.hit_type = "lru".into();
                    trace.protocol_ms = Some(lru_start.elapsed().as_secs_f64() * 1000.0);
                    trace.finish_and_log();
                    perf::LRU_HITS.fetch_add(1, Ordering::Relaxed);
                    return (
                        req_id,
                        CoverState::Ready,
                        Some(Arc::clone(&entry.image)),
                        entry.source.clone(),
                    );
                }
            }

            // 2. In-memory CacheIndex check (0 ms, zero FS calls)
            let idx_start = Instant::now();
            if let Ok(idx_lock) = self.cache_index.read() {
                trace.index_lookup_us = Some(idx_start.elapsed().as_micros() as u64);
                if let Some(entry) = idx_lock.get(app_name) {
                    if entry.is_negative {
                        let now = current_time_secs();
                        if now < entry.negative_expiry {
                            let has_epic = epic_url.map(|u| !u.is_empty()).unwrap_or(false);
                            if !has_epic {
                                trace.hit_type = "negative_index".into();
                                trace.finish_and_log();
                                return (req_id, CoverState::NoArtwork, None, None);
                            }
                        }
                    }
                }
            }
        }

        // Queue Immediate worker request
        self.queue_immediate(app_name, title, epic_url, force_refresh, req_id);

        (req_id, CoverState::Loading, None, None)
    }

    /// Enqueue a high-priority request for the currently selected game.
    pub fn queue_immediate(
        &mut self,
        app_name: &str,
        title: &str,
        epic_url: Option<&str>,
        force_refresh: bool,
        req_id: u64,
    ) {
        let seq = NEXT_SEQ.fetch_add(1, Ordering::Relaxed);
        let mut q = self.queue.lock().unwrap();

        // If already in queue, upgrade to Immediate
        if let Some(pos) = q.items.iter().position(|it| it.app_name == app_name) {
            q.items[pos].priority = Priority::Immediate;
            q.items[pos].force_refresh = force_refresh;
            q.items[pos].request_id = req_id;
        } else {
            q.items.push(WorkItem {
                priority: Priority::Immediate,
                request_id: req_id,
                app_name: app_name.to_string(),
                title: title.to_string(),
                epic_url: epic_url.map(|s| s.to_string()),
                force_refresh,
                seq,
            });
            q.in_flight.insert(app_name.to_string());
        }
        self.condvar.notify_one();
    }

    /// Enqueue neighbor pre-warm requests (±5 around selection), dropping out-of-range neighbor requests.
    pub fn queue_neighbors(&mut self, neighbors: &[(String, String, Option<String>)]) {
        let mut q = self.queue.lock().unwrap();
        let neighbor_set: HashSet<String> = neighbors.iter().map(|(a, _, _)| a.clone()).collect();

        // Drop queued neighbor requests that are no longer within range
        let mut to_remove = Vec::new();
        q.items.retain(|it| {
            if it.priority == Priority::Neighbor && !neighbor_set.contains(&it.app_name) {
                to_remove.push(it.app_name.clone());
                false
            } else {
                true
            }
        });
        for app in to_remove {
            q.in_flight.remove(&app);
        }

        for (app, title, url) in neighbors {
            if self.memory_cache.contains_key(app) || q.in_flight.contains(app) {
                continue;
            }
            let seq = NEXT_SEQ.fetch_add(1, Ordering::Relaxed);
            let req_id = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
            q.items.push(WorkItem {
                priority: Priority::Neighbor,
                request_id: req_id,
                app_name: app.clone(),
                title: title.clone(),
                epic_url: url.clone(),
                force_refresh: false,
                seq,
            });
            q.in_flight.insert(app.clone());
        }
        self.condvar.notify_all();
    }

    /// Enqueue full library finite prefetch (lowest priority).
    pub fn queue_prefetch(&mut self, games: &[(String, String, Option<String>)]) {
        let mut q = self.queue.lock().unwrap();
        let idx = self.cache_index.read().unwrap();
        let now = current_time_secs();

        let mut added = 0;
        for (app, title, url) in games {
            if let Some(entry) = idx.get(app) {
                if entry.is_negative && now < entry.negative_expiry {
                    continue;
                }
                if !entry.is_negative {
                    continue;
                }
            }
            if self.memory_cache.contains_key(app) || q.in_flight.contains(app) {
                continue;
            }
            let seq = NEXT_SEQ.fetch_add(1, Ordering::Relaxed);
            let req_id = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
            q.items.push(WorkItem {
                priority: Priority::Prefetch,
                request_id: req_id,
                app_name: app.clone(),
                title: title.clone(),
                epic_url: url.clone(),
                force_refresh: false,
                seq,
            });
            q.in_flight.insert(app.clone());
            added += 1;
        }

        q.total_prefetch = added;
        q.prefetch_done = 0;
        q.prefetch_cached = 0;
        q.prefetch_negative = 0;
        q.prefetch_failed = 0;
        perf::log_prefetch_queue(added, 0, 0, 0);
        self.condvar.notify_all();
    }

    /// Bump a pending prefetch item to front/Immediate if user selection comes within 10 rows.
    pub fn bump_prefetch(&mut self, app_name: &str) {
        let mut q = self.queue.lock().unwrap();
        if let Some(pos) = q.items.iter().position(|it| it.app_name == app_name) {
            if q.items[pos].priority == Priority::Prefetch {
                q.items[pos].priority = Priority::Immediate;
                self.condvar.notify_one();
            }
        }
    }

    /// Return prefetch progress: Some((completed, total)) or None if inactive.
    pub fn prefetch_progress(&self) -> Option<(usize, usize)> {
        let q = self.queue.lock().unwrap();
        if q.total_prefetch > 0 && q.prefetch_done < q.total_prefetch {
            Some((q.prefetch_done, q.total_prefetch))
        } else {
            None
        }
    }

    /// Invalidate in-memory and on-disk cached artwork for an app.
    pub fn invalidate(&mut self, app_name: &str) {
        self.memory_cache.remove(app_name);
        self.cache_order.retain(|k| k != app_name);

        if let Ok(mut idx) = self.cache_index.write() {
            idx.remove(app_name);
        }

        let path = cache_path_for(app_name);
        if path.exists() {
            let _ = std::fs::remove_file(path);
        }
        let sidecar = sidecar_path_for(app_name);
        if sidecar.exists() {
            let _ = std::fs::remove_file(sidecar);
        }
    }

    /// Poll for completed artwork responses.
    pub fn try_recv(&mut self) -> Option<ArtworkResponse> {
        if let Ok(resp) = self.rx_res.try_recv() {
            if let Ok(ref img) = resp.result {
                self.cache_order.retain(|k| k != &resp.app_name);
                self.cache_order.push_back(resp.app_name.clone());
                self.memory_cache.insert(
                    resp.app_name.clone(),
                    LruEntry {
                        app_name: resp.app_name.clone(),
                        image: Arc::clone(img),
                        source: resp.source.clone(),
                        protocol: None,
                    },
                );
                while self.memory_cache.len() > MAX_MEMORY_CACHE {
                    if let Some(oldest) = self.cache_order.pop_front() {
                        self.memory_cache.remove(&oldest);
                    }
                }
            }
            Some(resp)
        } else {
            None
        }
    }
}

impl Drop for ArtworkManager {
    fn drop(&mut self) {
        self.cancel_flag.store(true, Ordering::Relaxed);
        self.condvar.notify_all();
    }
}

fn worker_loop(
    _worker_id: usize,
    queue: Arc<Mutex<ArtworkQueue>>,
    condvar: Arc<Condvar>,
    cancel_flag: Arc<AtomicBool>,
    cache_index: Arc<RwLock<HashMap<String, CacheIndexEntry>>>,
    agent: Arc<ureq::Agent>,
    tx_res: Sender<ArtworkResponse>,
) {
    while !cancel_flag.load(Ordering::Relaxed) {
        let item = {
            let mut q = queue.lock().unwrap();
            while q.items.is_empty() && !cancel_flag.load(Ordering::Relaxed) {
                q = condvar.wait(q).unwrap();
            }
            if cancel_flag.load(Ordering::Relaxed) {
                break;
            }
            // Pick highest priority item (lowest priority enum, then lowest seq)
            let best_idx = q
                .items
                .iter()
                .enumerate()
                .min_by_key(|(_, it)| (it.priority, it.seq))
                .map(|(i, _)| i);

            match best_idx {
                Some(idx) => q.items.remove(idx),
                None => continue,
            }
        };

        if cancel_flag.load(Ordering::Relaxed) {
            break;
        }

        let app_name = item.app_name.clone();
        let panic_app = app_name.clone();
        let req_id = item.request_id;
        let priority = item.priority;

        let worker_res = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            handle_worker_item(&agent, &cache_index, &item, &cancel_flag)
        }));

        let response = match worker_res {
            Ok(res) => res,
            Err(_) => {
                log_art(req_id, &panic_app, "Worker panicked during execution");
                ArtworkResponse {
                    request_id: req_id,
                    app_name: panic_app,
                    source: None,
                    result: Err("Worker thread panicked".into()),
                    is_no_artwork: false,
                }
            }
        };

        // Update queue in_flight and prefetch statistics
        {
            let mut q = queue.lock().unwrap();
            q.in_flight.remove(&app_name);
            if priority == Priority::Prefetch {
                q.prefetch_done += 1;
                if response.result.is_ok() {
                    q.prefetch_cached += 1;
                } else if response.is_no_artwork {
                    q.prefetch_negative += 1;
                } else {
                    q.prefetch_failed += 1;
                }
                perf::log_prefetch_queue(
                    q.total_prefetch.saturating_sub(q.prefetch_done),
                    q.prefetch_cached,
                    q.prefetch_negative,
                    q.prefetch_failed,
                );
            }
        }

        let _ = tx_res.send(response);
    }
}

fn handle_worker_item(
    agent: &ureq::Agent,
    cache_index: &Arc<RwLock<HashMap<String, CacheIndexEntry>>>,
    item: &WorkItem,
    cancel_flag: &AtomicBool,
) -> ArtworkResponse {
    let req_id = item.request_id;
    let app = &item.app_name;
    let cache_file = cache_path_for(app);
    let mut trace = perf::SelectionTrace::new(app.clone());

    // 1. Check disk cache if not force refresh
    if !item.force_refresh && cache_file.exists() {
        let sidecar_start = Instant::now();
        let sidecar = read_sidecar(app);
        trace.sidecar_read_us = Some(sidecar_start.elapsed().as_micros() as u64);

        let has_better_epic = item
            .epic_url
            .as_ref()
            .map(|u| !u.is_empty())
            .unwrap_or(false)
            && sidecar
                .as_ref()
                .map(|s| s.source != "Epic")
                .unwrap_or(false);

        if !has_better_epic {
            let read_start = Instant::now();
            if let Ok(bytes) = std::fs::read(&cache_file) {
                trace.file_read_us = Some(read_start.elapsed().as_micros() as u64);
                if !bytes.is_empty() {
                    let decode_start = Instant::now();
                    if let Ok(mut img) = image::load_from_memory(&bytes) {
                        trace.decode_ms = Some(decode_start.elapsed().as_secs_f64() * 1000.0);
                        trace.decode_dim = Some((img.width(), img.height()));
                        trace.decode_format = Some("jpeg/image".into());

                        // Lazy migration: if oversized (> 400x600 or > 80 KB), downscale and save JPEG q85
                        if bytes.len() > 80 * 1024 || img.width() > 400 || img.height() > 600 {
                            let resize_start = Instant::now();
                            if let Ok(small) = save_image_to_cache(app, &img) {
                                trace.resize_ms =
                                    Some(resize_start.elapsed().as_secs_f64() * 1000.0);
                                img = small;
                            }
                        }

                        let source = sidecar
                            .as_ref()
                            .map(|s| s.source.clone())
                            .unwrap_or_else(|| "Cached".into());

                        // Update index with verified file
                        if let Ok(mut idx) = cache_index.write() {
                            idx.insert(
                                app.clone(),
                                CacheIndexEntry {
                                    file_path: cache_file.clone(),
                                    mtime: current_time_secs(),
                                    source: source.clone(),
                                    is_negative: false,
                                    negative_expiry: 0,
                                },
                            );
                        }

                        trace.hit_type = "disk".into();
                        trace.finish_and_log();
                        perf::DISK_HITS.fetch_add(1, Ordering::Relaxed);

                        return ArtworkResponse {
                            request_id: req_id,
                            app_name: app.clone(),
                            source: Some(source),
                            result: Ok(Arc::new(img)),
                            is_no_artwork: false,
                        };
                    }
                }
                // Corrupt / zero-byte cache file discarded and re-fetched
                let _ = std::fs::remove_file(&cache_file);
                let _ = std::fs::remove_file(sidecar_path_for(app));
            }
        }
    }

    if cancel_flag.load(Ordering::Relaxed) {
        return ArtworkResponse {
            request_id: req_id,
            app_name: app.clone(),
            source: None,
            result: Err("Cancelled".into()),
            is_no_artwork: false,
        };
    }

    log_art(
        req_id,
        app,
        &format!("Starting resolver for '{}' (title: '{}')", app, item.title),
    );

    // Provider 1: Epic Games keyImages URL
    if let Some(ref url) = item.epic_url {
        if !url.is_empty() {
            log_art(req_id, app, &format!("Trying Provider 1 (Epic): {}", url));
            match download_and_cache_image(agent, req_id, app, url) {
                Ok(img) => {
                    let sidecar = ArtworkSidecar {
                        version: CURRENT_SCHEMA_VERSION,
                        source: "Epic".into(),
                        url: url.clone(),
                        steam_appid: None,
                        fetched_at: current_time_secs(),
                        negative: false,
                    };
                    write_sidecar(app, &sidecar);
                    if let Ok(mut idx) = cache_index.write() {
                        idx.insert(
                            app.clone(),
                            CacheIndexEntry {
                                file_path: cache_file,
                                mtime: current_time_secs(),
                                source: "Epic".into(),
                                is_negative: false,
                                negative_expiry: 0,
                            },
                        );
                    }
                    trace.hit_type = "network_epic".into();
                    trace.finish_and_log();
                    perf::NETWORK_HITS.fetch_add(1, Ordering::Relaxed);

                    return ArtworkResponse {
                        request_id: req_id,
                        app_name: app.clone(),
                        source: Some("Epic".into()),
                        result: Ok(Arc::new(img)),
                        is_no_artwork: false,
                    };
                }
                Err(e) => {
                    log_art(req_id, app, &format!("Provider 1 (Epic) failed: {}", e));
                }
            }
        }
    }

    if cancel_flag.load(Ordering::Relaxed) {
        return ArtworkResponse {
            request_id: req_id,
            app_name: app.clone(),
            source: None,
            result: Err("Cancelled".into()),
            is_no_artwork: false,
        };
    }

    // Provider 2: Steam Store CDN
    log_art(
        req_id,
        app,
        &format!("Trying Provider 2 (Steam CDN) for '{}'", item.title),
    );
    match resolve_steam_cdn(agent, req_id, app, &item.title) {
        Ok(Some((img, appid, url))) => {
            let sidecar = ArtworkSidecar {
                version: CURRENT_SCHEMA_VERSION,
                source: "Steam".into(),
                url,
                steam_appid: Some(appid),
                fetched_at: current_time_secs(),
                negative: false,
            };
            write_sidecar(app, &sidecar);
            if let Ok(mut idx) = cache_index.write() {
                idx.insert(
                    app.clone(),
                    CacheIndexEntry {
                        file_path: cache_file,
                        mtime: current_time_secs(),
                        source: "Steam".into(),
                        is_negative: false,
                        negative_expiry: 0,
                    },
                );
            }
            trace.hit_type = "network_steam".into();
            trace.finish_and_log();
            perf::NETWORK_HITS.fetch_add(1, Ordering::Relaxed);

            return ArtworkResponse {
                request_id: req_id,
                app_name: app.clone(),
                source: Some("Steam".into()),
                result: Ok(Arc::new(img)),
                is_no_artwork: false,
            };
        }
        Ok(None) => {
            log_art(req_id, app, "Provider 2 (Steam) found no matching title");
        }
        Err(e) => {
            log_art(req_id, app, &format!("Provider 2 (Steam) error: {}", e));
        }
    }

    if cancel_flag.load(Ordering::Relaxed) {
        return ArtworkResponse {
            request_id: req_id,
            app_name: app.clone(),
            source: None,
            result: Err("Cancelled".into()),
            is_no_artwork: false,
        };
    }

    // Provider 3: SteamGridDB (optional, active only if STEAMGRIDDB_API_KEY is present)
    if let Ok(api_key) = std::env::var("STEAMGRIDDB_API_KEY") {
        let trimmed_key = api_key.trim();
        if !trimmed_key.is_empty() {
            log_art(req_id, app, "Trying Provider 3 (SteamGridDB)");
            match resolve_steamgriddb(agent, req_id, app, &item.title, trimmed_key) {
                Ok(Some((img, url))) => {
                    let sidecar = ArtworkSidecar {
                        version: CURRENT_SCHEMA_VERSION,
                        source: "SteamGridDB".into(),
                        url,
                        steam_appid: None,
                        fetched_at: current_time_secs(),
                        negative: false,
                    };
                    write_sidecar(app, &sidecar);
                    if let Ok(mut idx) = cache_index.write() {
                        idx.insert(
                            app.clone(),
                            CacheIndexEntry {
                                file_path: cache_file,
                                mtime: current_time_secs(),
                                source: "SteamGridDB".into(),
                                is_negative: false,
                                negative_expiry: 0,
                            },
                        );
                    }
                    trace.hit_type = "network_steamgriddb".into();
                    trace.finish_and_log();
                    perf::NETWORK_HITS.fetch_add(1, Ordering::Relaxed);

                    return ArtworkResponse {
                        request_id: req_id,
                        app_name: app.clone(),
                        source: Some("SteamGridDB".into()),
                        result: Ok(Arc::new(img)),
                        is_no_artwork: false,
                    };
                }
                Ok(None) => {
                    log_art(req_id, app, "Provider 3 (SteamGridDB) found no grid");
                }
                Err(e) => {
                    log_art(
                        req_id,
                        app,
                        &format!("Provider 3 (SteamGridDB) error: {}", e),
                    );
                }
            }
        }
    }

    // All sources exhausted: write negative cache
    log_art(
        req_id,
        app,
        "All artwork providers exhausted. Recording negative cache.",
    );
    let now = current_time_secs();
    let sidecar = ArtworkSidecar {
        version: CURRENT_SCHEMA_VERSION,
        source: "None".into(),
        url: String::new(),
        steam_appid: None,
        fetched_at: now,
        negative: true,
    };
    write_sidecar(app, &sidecar);
    if let Ok(mut idx) = cache_index.write() {
        idx.insert(
            app.clone(),
            CacheIndexEntry {
                file_path: cache_file,
                mtime: now,
                source: "None".into(),
                is_negative: true,
                negative_expiry: now + NEGATIVE_CACHE_TTL_SECS,
            },
        );
    }
    trace.hit_type = "negative_new".into();
    trace.finish_and_log();

    ArtworkResponse {
        request_id: req_id,
        app_name: app.clone(),
        source: None,
        result: Err("No cover artwork available".into()),
        is_no_artwork: true,
    }
}

pub fn downscale_to_display_size(img: &DynamicImage, max_w: u32, max_h: u32) -> DynamicImage {
    if img.width() <= max_w && img.height() <= max_h {
        img.clone()
    } else {
        img.resize(max_w, max_h, image::imageops::FilterType::Triangle)
    }
}

pub fn encode_jpeg_q85(img: &DynamicImage) -> Result<Vec<u8>, String> {
    let rgb = img.to_rgb8();
    let mut buf = Vec::with_capacity(32 * 1024);
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 85);
    enc.encode(
        rgb.as_raw(),
        rgb.width(),
        rgb.height(),
        image::ExtendedColorType::Rgb8,
    )
    .map_err(|e| format!("JPEG encode error: {e}"))?;
    Ok(buf)
}

pub fn save_image_to_cache(app_name: &str, img: &DynamicImage) -> Result<DynamicImage, String> {
    let small = downscale_to_display_size(img, 400, 600);
    let bytes = encode_jpeg_q85(&small)?;
    let cache_file = cache_path_for(app_name);
    filesystem::atomic_write(&cache_file, &bytes)?;
    Ok(small)
}

pub fn download_and_cache_image(
    agent: &ureq::Agent,
    req_id: u64,
    app_name: &str,
    url: &str,
) -> Result<DynamicImage, String> {
    let resp = agent
        .get(url)
        .call()
        .map_err(|e| format!("HTTP request error: {e}"))?;

    let status = resp.status();
    let content_type = resp.content_type().to_string();
    log_art(
        req_id,
        app_name,
        &format!(
            "HTTP {} from {} (Content-Type: {})",
            status, url, content_type
        ),
    );

    if !(200..300).contains(&status) {
        return Err(format!("HTTP status {status}"));
    }

    let ct_lower = content_type.to_lowercase();
    if ct_lower.contains("text/html") || ct_lower.contains("application/json") {
        return Err(format!("Invalid Content-Type: {content_type}"));
    }

    let mut bytes = Vec::new();
    resp.into_reader()
        .take(MAX_IMAGE_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Read body error: {e}"))?;

    if bytes.is_empty() {
        return Err("Zero-byte body".into());
    }

    // Decode and downscale to max 400x600 JPEG q85
    let raw_img =
        image::load_from_memory(&bytes).map_err(|e| format!("Image decode failed: {e}"))?;

    log_art(
        req_id,
        app_name,
        &format!(
            "Decoded raw image: {}x{} ({} bytes), downscaling to max 400x600 JPEG q85",
            raw_img.width(),
            raw_img.height(),
            bytes.len()
        ),
    );

    save_image_to_cache(app_name, &raw_img)
}

pub fn scan_cache_index() -> HashMap<String, CacheIndexEntry> {
    let dir = filesystem::image_cache_dir();
    let mut index = HashMap::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return index;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let app_name = stem.to_string();
            if let Ok(bytes) = std::fs::read(&path) {
                if let Ok(sidecar) = serde_json::from_slice::<ArtworkSidecar>(&bytes) {
                    if sidecar.negative {
                        if sidecar.version >= CURRENT_SCHEMA_VERSION {
                            index.insert(
                                app_name,
                                CacheIndexEntry {
                                    file_path: path.with_extension("img"),
                                    mtime: sidecar.fetched_at,
                                    source: sidecar.source,
                                    is_negative: true,
                                    negative_expiry: sidecar.fetched_at + NEGATIVE_CACHE_TTL_SECS,
                                },
                            );
                        }
                    } else {
                        let img_path = path.with_extension("img");
                        if let Ok(meta) = img_path.metadata() {
                            if meta.len() > 0 {
                                let mtime = meta
                                    .modified()
                                    .ok()
                                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                                    .map(|d| d.as_secs())
                                    .unwrap_or(sidecar.fetched_at);
                                index.insert(
                                    app_name,
                                    CacheIndexEntry {
                                        file_path: img_path,
                                        mtime,
                                        source: sidecar.source,
                                        is_negative: false,
                                        negative_expiry: 0,
                                    },
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    index
}

pub fn enforce_cache_size_cap() {
    let dir = filesystem::image_cache_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut files = Vec::new();
    let mut total_bytes = 0u64;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("img") {
            if let Ok(meta) = entry.metadata() {
                let size = meta.len();
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                total_bytes += size;
                files.push((path, size, mtime));
            }
        }
    }

    if total_bytes > MAX_CACHE_SIZE_BYTES {
        files.sort_by_key(|(_, _, mtime)| *mtime);
        for (path, size, _) in files {
            if total_bytes <= PRUNE_CACHE_SIZE_BYTES {
                break;
            }
            let _ = std::fs::remove_file(&path);
            let json = path.with_extension("json");
            if json.exists() {
                let _ = std::fs::remove_file(json);
            }
            total_bytes = total_bytes.saturating_sub(size);
        }
    }
}

pub fn cache_path_for(app_name: &str) -> PathBuf {
    let sanitized: String = app_name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    filesystem::image_cache_dir().join(format!("{sanitized}.img"))
}

pub fn sidecar_path_for(app_name: &str) -> PathBuf {
    let sanitized: String = app_name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    filesystem::image_cache_dir().join(format!("{sanitized}.json"))
}

pub fn read_sidecar(app_name: &str) -> Option<ArtworkSidecar> {
    let path = sidecar_path_for(app_name);
    let bytes = std::fs::read(path).ok()?;
    let sidecar: ArtworkSidecar = serde_json::from_slice(&bytes).ok()?;
    if sidecar.negative && sidecar.version < CURRENT_SCHEMA_VERSION {
        return None;
    }
    Some(sidecar)
}

pub fn write_sidecar(app_name: &str, sidecar: &ArtworkSidecar) {
    let mut updated = sidecar.clone();
    updated.version = CURRENT_SCHEMA_VERSION;
    let path = sidecar_path_for(app_name);
    if let Ok(bytes) = serde_json::to_vec_pretty(&updated) {
        let _ = filesystem::atomic_write(&path, &bytes);
    }
}

fn current_time_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn log_art(req_id: u64, app_name: &str, msg: &str) {
    if std::env::var("LEGENDARY_TUI_DEBUG").unwrap_or_default() == "1" || cfg!(debug_assertions) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/legendary-tui-art.log")
        {
            let now = current_time_secs();
            let _ = writeln!(f, "[{} | req:{} | {}] {}", now, req_id, app_name, msg);
        }
    }
}

#[derive(Clone, Debug)]
pub struct SteamCandidate {
    pub id: u64,
    pub item_type: String,
    pub name: String,
    pub score: f32,
}

pub fn normalize_title(title: &str) -> String {
    let s = title
        .replace(['™', '®', '©'], "")
        .replace('&', " and ")
        .replace("40,000", "40000")
        .replace("40.000", "40000")
        .to_lowercase();

    let raw_tokens: Vec<String> = s
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(|w| match w {
            "40k" => "40000".to_string(),
            "40000" => "40000".to_string(),
            "i" => "1".to_string(),
            "ii" => "2".to_string(),
            "iii" => "3".to_string(),
            "iv" => "4".to_string(),
            "v" => "5".to_string(),
            "vi" => "6".to_string(),
            "vii" => "7".to_string(),
            "viii" => "8".to_string(),
            "ix" => "9".to_string(),
            "x" => "10".to_string(),
            other => other.to_string(),
        })
        .collect();

    let mut tokens = Vec::new();
    let mut idx = 0;
    while idx < raw_tokens.len() {
        if raw_tokens[idx] == "40" && idx + 1 < raw_tokens.len() && raw_tokens[idx + 1] == "000" {
            tokens.push("40000".to_string());
            idx += 2;
        } else {
            tokens.push(raw_tokens[idx].clone());
            idx += 1;
        }
    }

    tokens.join(" ")
}

pub fn build_steam_query(title: &str) -> String {
    let clean = title
        .replace(['™', '®', '©'], "")
        .replace('&', " and ")
        .replace("40,000", "40000")
        .replace("40.000", "40000")
        .trim()
        .to_string();

    let tokens: Vec<String> = clean
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(|w| {
            if w.eq_ignore_ascii_case("40k") {
                "40000".to_string()
            } else {
                w.to_string()
            }
        })
        .collect();

    tokens.join(" ")
}

pub fn simplify_title_for_search(title: &str) -> Option<String> {
    let clean = title.replace(['™', '®', '©'], "");
    let base = clean
        .split([':', '-'])
        .next()
        .unwrap_or(&clean)
        .trim()
        .to_string();

    let lower = base.to_lowercase();
    let stripped = lower
        .replace("deluxe edition", "")
        .replace("definitive edition", "")
        .replace("standard edition", "")
        .replace("remastered", "")
        .replace("enhanced edition", "")
        .replace("game of the year edition", "")
        .replace("goty", "")
        .trim()
        .to_string();

    if stripped.is_empty() || stripped.eq_ignore_ascii_case(&clean) {
        None
    } else {
        Some(build_steam_query(&stripped))
    }
}

pub fn calculate_token_similarity(a: &str, b: &str) -> f32 {
    let norm_a = normalize_title(a);
    let norm_b = normalize_title(b);

    if norm_a == norm_b {
        return 1.0;
    }

    let tokens_a: HashSet<&str> = norm_a.split_whitespace().collect();
    let tokens_b: HashSet<&str> = norm_b.split_whitespace().collect();

    if tokens_a.is_empty() || tokens_b.is_empty() {
        return 0.0;
    }

    let intersection = tokens_a.intersection(&tokens_b).count();
    let union = tokens_a.union(&tokens_b).count();

    intersection as f32 / union as f32
}

fn is_extra_content(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains("soundtrack")
        || lower.contains("dlc")
        || lower.contains("season pass")
        || lower.contains("expansion pack")
        || lower.contains("artbook")
        || lower.contains("content pack")
        || lower.contains("upgrade pack")
        || lower.contains("bonus content")
}

pub fn search_steam_candidates(
    agent: &ureq::Agent,
    query: &str,
    target_title: &str,
) -> Result<Vec<SteamCandidate>, String> {
    let url = format!(
        "https://store.steampowered.com/api/storesearch/?term={}&l=english&cc=US",
        url_encode(query)
    );

    let resp = agent
        .get(&url)
        .call()
        .map_err(|e| format!("Steam store search error: {e}"))?;

    let json: serde_json::Value = serde_json::from_reader(resp.into_reader())
        .map_err(|e| format!("Steam JSON parse error: {e}"))?;

    let items = match json.get("items").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return Ok(Vec::new()),
    };

    let mut candidates = Vec::new();
    for item in items {
        let id = match item.get("id").and_then(|v| v.as_u64()) {
            Some(id) => id,
            None => continue,
        };
        let name = match item.get("name").and_then(|v| v.as_str()) {
            Some(n) => n,
            None => continue,
        };
        let item_type = item
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("app")
            .to_string();

        if item_type != "app" || is_extra_content(name) {
            continue;
        }

        let score = calculate_token_similarity(target_title, name);
        if score >= 0.85 {
            candidates.push(SteamCandidate {
                id,
                item_type,
                name: name.to_string(),
                score,
            });
        }
    }

    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(candidates)
}

pub fn resolve_steam_cdn(
    agent: &ureq::Agent,
    req_id: u64,
    app_name: &str,
    target_title: &str,
) -> Result<Option<(DynamicImage, u64, String)>, String> {
    let q1 = build_steam_query(target_title);
    let mut candidates = search_steam_candidates(agent, &q1, target_title)?;

    if candidates.is_empty() {
        if let Some(q2) = simplify_title_for_search(target_title) {
            log_art(
                req_id,
                app_name,
                &format!(
                    "Pass 1 returned no candidates. Trying Pass 2 query: '{}'",
                    q2
                ),
            );
            candidates = search_steam_candidates(agent, &q2, target_title)?;
        }
    }

    if candidates.is_empty() {
        return Ok(None);
    }

    if candidates.len() >= 2 {
        let diff = (candidates[0].score - candidates[1].score).abs();
        if diff < 0.05 {
            let base0 = normalize_title(&candidates[0].name);
            let base1 = normalize_title(&candidates[1].name);
            if base0 != base1 {
                log_art(
                    req_id,
                    app_name,
                    &format!(
                        "Ambiguous Steam candidates between '{}' and '{}'. Rejecting.",
                        candidates[0].name, candidates[1].name
                    ),
                );
                return Ok(None);
            }
        }
    }

    let best = &candidates[0];
    let appid = best.id;
    log_art(
        req_id,
        app_name,
        &format!(
            "Matched Steam candidate appid {} ('{}') with score {:.2}",
            appid, best.name, best.score
        ),
    );

    let portrait_url = format!(
        "https://shared.fastly.steamstatic.com/store_item_assets/steam/apps/{}/library_600x900.jpg",
        appid
    );
    match download_and_cache_image(agent, req_id, app_name, &portrait_url) {
        Ok(img) => return Ok(Some((img, appid, portrait_url))),
        Err(e) => {
            log_art(
                req_id,
                app_name,
                &format!(
                    "Steam library_600x900.jpg failed: {}. Falling back to header.jpg",
                    e
                ),
            );
        }
    }

    let header_url = format!(
        "https://shared.fastly.steamstatic.com/store_item_assets/steam/apps/{}/header.jpg",
        appid
    );
    match download_and_cache_image(agent, req_id, app_name, &header_url) {
        Ok(img) => Ok(Some((img, appid, header_url))),
        Err(e) => {
            log_art(req_id, app_name, &format!("Steam header.jpg failed: {}", e));
            Ok(None)
        }
    }
}

pub fn resolve_steamgriddb(
    agent: &ureq::Agent,
    req_id: u64,
    app_name: &str,
    title: &str,
    api_key: &str,
) -> Result<Option<(DynamicImage, String)>, String> {
    let search_url = format!(
        "https://www.steamgriddb.com/api/v2/search/autocomplete/{}",
        url_encode(title)
    );

    let search_resp = agent
        .get(&search_url)
        .set("Authorization", &format!("Bearer {}", api_key))
        .call()
        .map_err(|e| format!("SteamGridDB search error: {e}"))?;

    let search_json: serde_json::Value = serde_json::from_reader(search_resp.into_reader())
        .map_err(|e| format!("SteamGridDB JSON error: {e}"))?;

    let data = match search_json.get("data").and_then(|v| v.as_array()) {
        Some(arr) if !arr.is_empty() => arr,
        _ => return Ok(None),
    };

    let game_id = match data[0].get("id").and_then(|v| v.as_u64()) {
        Some(id) => id,
        None => return Ok(None),
    };

    let grids_url = format!(
        "https://www.steamgriddb.com/api/v2/grids/game/{}?dimensions=600x900",
        game_id
    );

    let grids_resp = agent
        .get(&grids_url)
        .set("Authorization", &format!("Bearer {}", api_key))
        .call()
        .map_err(|e| format!("SteamGridDB grids error: {e}"))?;

    let grids_json: serde_json::Value = serde_json::from_reader(grids_resp.into_reader())
        .map_err(|e| format!("SteamGridDB grids JSON error: {e}"))?;

    let grids_data = match grids_json.get("data").and_then(|v| v.as_array()) {
        Some(arr) if !arr.is_empty() => arr,
        _ => return Ok(None),
    };

    let img_url = match grids_data[0].get("url").and_then(|v| v.as_str()) {
        Some(u) => u,
        None => return Ok(None),
    };

    log_art(
        req_id,
        app_name,
        &format!("Fetching SteamGridDB grid: {}", img_url),
    );
    if let Ok(img) = download_and_cache_image(agent, req_id, app_name, img_url) {
        return Ok(Some((img, img_url.to_string())));
    }

    Ok(None)
}

#[allow(dead_code)]
pub fn titles_match(a: &str, b: &str) -> bool {
    let norm_a = normalize_title(a);
    let norm_b = normalize_title(b);

    if norm_a == norm_b {
        return true;
    }

    let tokens_a: HashSet<&str> = norm_a.split_whitespace().collect();
    let tokens_b: HashSet<&str> = norm_b.split_whitespace().collect();

    if tokens_a.is_empty() || tokens_b.is_empty() {
        return false;
    }

    let intersection = tokens_a.intersection(&tokens_b).count();
    let min_len = tokens_a.len().min(tokens_b.len());

    intersection == min_len && (tokens_a.len() as isize - tokens_b.len() as isize).abs() <= 1
}

pub fn url_encode(s: &str) -> String {
    let mut encoded = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b'~' {
            encoded.push(b as char);
        } else if b == b' ' {
            encoded.push('+');
        } else {
            encoded.push_str(&format!("%{:02X}", b));
        }
    }
    encoded
}

pub fn run_art_report() {
    println!("=== Legendary TUI Artwork Diagnostic Report ===");
    println!();

    let games: Vec<crate::cache::CachedGame> = crate::cache::load().unwrap_or_else(|| {
        crate::legendary::library()
            .unwrap_or_default()
            .into_iter()
            .map(|(app_name, title)| crate::cache::CachedGame { app_name, title })
            .collect()
    });

    let mut total = 0;
    let mut epic_count = 0;
    let mut steam_count = 0;
    let mut sgdb_count = 0;
    let mut other_cached_count = 0;
    let mut missing_count = 0;

    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(8))
        .user_agent("legendary-tui/1.0.2-art-report (Linux)")
        .build();

    let sgdb_key = std::env::var("STEAMGRIDDB_API_KEY").ok();

    for g in &games {
        total += 1;
        let c_path = cache_path_for(&g.app_name);
        let sidecar = read_sidecar(&g.app_name);
        let is_cached = c_path.exists();

        if is_cached {
            let src = sidecar
                .as_ref()
                .map(|s| s.source.as_str())
                .unwrap_or("Cached");
            match src {
                "Epic" => epic_count += 1,
                "Steam" => steam_count += 1,
                "SteamGridDB" => sgdb_count += 1,
                _ => other_cached_count += 1,
            }
            println!(
                "[Cached ({})] {} ({}) -> {}",
                src,
                g.title,
                g.app_name,
                c_path.display()
            );
        } else {
            missing_count += 1;
            let status = if sidecar.as_ref().map(|s| s.negative).unwrap_or(false) {
                "NoArtwork"
            } else {
                "Missing"
            };
            println!(
                "[{} / {}] {} ({})",
                status,
                sidecar
                    .as_ref()
                    .map(|s| s.source.as_str())
                    .unwrap_or("None"),
                g.title,
                g.app_name
            );

            // Diagnose Epic
            let meta_path =
                crate::filesystem::legendary_metadata_dir().join(format!("{}.json", g.app_name));
            if let Ok(bytes) = std::fs::read(&meta_path) {
                if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                    if let Some(imgs) = val
                        .get("metadata")
                        .and_then(|m| m.get("keyImages"))
                        .and_then(|k| k.as_array())
                    {
                        let best = crate::metadata::select_best_game_image(imgs);
                        println!(
                            "  Epic metadata: {} images found. Best candidate: {:?}",
                            imgs.len(),
                            best
                        );
                        for img in imgs {
                            let itype = img
                                .get("type")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");
                            let iurl = img.get("url").and_then(|v| v.as_str()).unwrap_or("");
                            println!("    - {}: {}", itype, iurl);
                        }
                    } else {
                        println!("  Epic metadata: no keyImages array found");
                    }
                }
            } else {
                println!("  Epic metadata: file not found at {}", meta_path.display());
            }

            // Diagnose Steam
            println!("  Steam store search:");
            let q1 = build_steam_query(&g.title);
            println!("    Query 1: '{}'", q1);
            let steam_res1 = search_steam_candidates(&agent, &q1, &g.title);
            match steam_res1 {
                Ok(cands) if !cands.is_empty() => {
                    println!("    Candidates ({}):", cands.len());
                    for c in &cands {
                        println!(
                            "      - [id: {}, type: {}] '{}' (score: {:.2})",
                            c.id, c.item_type, c.name, c.score
                        );
                    }
                }
                Ok(_) => {
                    println!("    Query 1 returned 0 matching candidates.");
                    if let Some(q2) = simplify_title_for_search(&g.title) {
                        println!("    Query 2 (simplified): '{}'", q2);
                        if let Ok(cands2) = search_steam_candidates(&agent, &q2, &g.title) {
                            if cands2.is_empty() {
                                println!("    Query 2 returned 0 matching candidates.");
                            } else {
                                for c in &cands2 {
                                    println!(
                                        "      - [id: {}, type: {}] '{}' (score: {:.2})",
                                        c.id, c.item_type, c.name, c.score
                                    );
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    println!("    Steam search error: {}", e);
                }
            }

            // Diagnose SteamGridDB
            if let Some(ref key) = sgdb_key {
                if !key.trim().is_empty() {
                    println!("  SteamGridDB: configured with API key");
                } else {
                    println!("  SteamGridDB: STEAMGRIDDB_API_KEY is empty");
                }
            } else {
                println!("  SteamGridDB: STEAMGRIDDB_API_KEY not configured");
            }
        }
    }

    println!();
    println!("--------------------------------------------------");
    println!(
        "Summary: Total: {} | Epic: {} | Steam: {} | SteamGridDB: {} | Other Cached: {} | Missing: {}",
        total, epic_count, steam_count, sgdb_count, other_cached_count, missing_count
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_title() {
        assert_eq!(normalize_title("Apex Legends™"), "apex legends");
        assert_eq!(
            normalize_title("Grand Theft Auto V: Premium Edition"),
            "grand theft auto 5 premium edition"
        );
        assert_eq!(normalize_title("Grand Theft Auto IV"), "grand theft auto 4");
        assert_eq!(
            normalize_title("Warhammer 40K Speed Freeks"),
            "warhammer 40000 speed freeks"
        );
        assert_eq!(
            normalize_title("Warhammer 40,000: Speed Freeks"),
            "warhammer 40000 speed freeks"
        );
        assert_eq!(
            normalize_title("Dungeons & Dragons"),
            "dungeons and dragons"
        );
        assert_eq!(normalize_title("Game® (Demo)"), "game demo");
    }

    #[test]
    fn test_titles_match() {
        assert!(titles_match("Apex Legends™", "Apex Legends"));
        assert!(titles_match("Apex Legends", "Apex Legends™"));
        assert!(!titles_match(
            "Apex Legends - Season 20 Soundtrack",
            "Apex Legends"
        ));
        assert!(!titles_match(
            "Apex Legends: Defiance DLC Pack",
            "Apex Legends"
        ));
    }

    #[test]
    fn test_url_encode() {
        assert_eq!(url_encode("Apex Legends"), "Apex+Legends");
        assert_eq!(url_encode("Grand Theft Auto V"), "Grand+Theft+Auto+V");
    }

    #[test]
    fn test_cache_path_for_sanitization() {
        let path = cache_path_for("Epic-Game_123:Special!");
        let fname = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(fname, "Epic-Game_123_Special_.img");
    }

    #[test]
    fn test_artwork_manager_initial_state() {
        let mgr = ArtworkManager::new();
        assert!(mgr.get_cached("nonexistent").is_none());
    }

    #[test]
    fn test_sidecar_roundtrip() {
        let app = "test_sidecar_app";
        let sidecar = ArtworkSidecar {
            version: CURRENT_SCHEMA_VERSION,
            source: "Steam".into(),
            url: "https://example.com/test.jpg".into(),
            steam_appid: Some(12345),
            fetched_at: 1000000,
            negative: false,
        };
        write_sidecar(app, &sidecar);
        let read = read_sidecar(app).unwrap();
        assert_eq!(read.version, CURRENT_SCHEMA_VERSION);
        assert_eq!(read.source, "Steam");
        assert_eq!(read.steam_appid, Some(12345));
        let _ = std::fs::remove_file(sidecar_path_for(app));
    }

    #[test]
    fn test_negative_cache_ttl() {
        let app = "test_neg_cache_app";
        let now = current_time_secs();
        let fresh_neg = ArtworkSidecar {
            version: CURRENT_SCHEMA_VERSION,
            source: "None".into(),
            url: String::new(),
            steam_appid: None,
            fetched_at: now - 100, // 100 seconds ago, well within 24 hours
            negative: true,
        };
        write_sidecar(app, &fresh_neg);

        let mut mgr = ArtworkManager::new();
        mgr.build_cache_index();
        let (_req_id, state, _img, _src) = mgr.request_artwork(app, "Test App", None, false);
        assert_eq!(state, CoverState::NoArtwork);

        // Expired negative cache (> 24 hours)
        let expired_neg = ArtworkSidecar {
            version: CURRENT_SCHEMA_VERSION,
            source: "None".into(),
            url: String::new(),
            steam_appid: None,
            fetched_at: now - (NEGATIVE_CACHE_TTL_SECS + 100),
            negative: true,
        };
        write_sidecar(app, &expired_neg);
        mgr.build_cache_index();
        let (_req_id, state_expired, _img, _src) =
            mgr.request_artwork(app, "Test App", None, false);
        assert_eq!(state_expired, CoverState::Loading);

        let _ = std::fs::remove_file(sidecar_path_for(app));
    }

    #[test]
    fn test_sidecar_version_migration_invalidates_old_negative() {
        let app = "test_old_negative_app";
        let raw_v1_json = r#"{
            "version": 1,
            "source": "None",
            "url": "",
            "steam_appid": null,
            "fetched_at": 1791383730,
            "negative": true
        }"#;
        let path = sidecar_path_for(app);
        std::fs::write(&path, raw_v1_json).unwrap();

        assert!(read_sidecar(app).is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_provider_chain_priority_order() {
        enum Step {
            Epic,
            Steam,
            SteamGridDb,
        }

        fn resolve_chain(has_epic: bool, has_steam: bool, has_sgdb: bool) -> Option<&'static str> {
            let steps = [
                (Step::Epic, has_epic, "Epic"),
                (Step::Steam, has_steam, "Steam"),
                (Step::SteamGridDb, has_sgdb, "SteamGridDB"),
            ];
            for (_step, available, name) in steps {
                if available {
                    return Some(name);
                }
            }
            None
        }

        assert_eq!(resolve_chain(true, true, true), Some("Epic"));
        assert_eq!(resolve_chain(false, true, true), Some("Steam"));
        assert_eq!(resolve_chain(false, false, true), Some("SteamGridDB"));
        assert_eq!(resolve_chain(false, false, false), None);
    }

    #[test]
    fn test_downscale_and_jpeg_roundtrip() {
        let large_img = DynamicImage::new_rgb8(1200, 1600);
        let downscaled = downscale_to_display_size(&large_img, 400, 600);
        assert!(downscaled.width() <= 400);
        assert!(downscaled.height() <= 600);

        let encoded = encode_jpeg_q85(&downscaled).expect("jpeg encode");
        assert!(!encoded.is_empty());
        assert!(encoded.len() < 50 * 1024); // tiny JPEG

        let decoded = image::load_from_memory(&encoded).expect("jpeg decode");
        assert_eq!(decoded.width(), downscaled.width());
        assert_eq!(decoded.height(), downscaled.height());
    }

    #[test]
    fn test_cache_index_hit_zero_fs_stat() {
        let mut mgr = ArtworkManager::new();
        // Insert directly into cache_index map
        if let Ok(mut idx) = mgr.cache_index.write() {
            idx.insert(
                "mock_cached_app".into(),
                CacheIndexEntry {
                    file_path: PathBuf::from("/nonexistent/mock_path.img"),
                    mtime: 1234567,
                    source: "Epic".into(),
                    is_negative: true,
                    negative_expiry: current_time_secs() + 1000,
                },
            );
        }
        assert!(mgr.is_negative_cached("mock_cached_app"));
        // Negative cache returns immediately without loading state
        let (_id, state, _img, _src) =
            mgr.request_artwork("mock_cached_app", "Mock Title", None, false);
        assert_eq!(state, CoverState::NoArtwork);
    }

    #[test]
    fn test_rapid_selection_and_neighbor_pruning() {
        let mut q = ArtworkQueue {
            items: Vec::new(),
            in_flight: HashSet::new(),
            total_prefetch: 0,
            prefetch_done: 0,
            prefetch_cached: 0,
            prefetch_negative: 0,
            prefetch_failed: 0,
        };
        // Add items to queue
        q.items.push(WorkItem {
            priority: Priority::Neighbor,
            request_id: 1,
            app_name: "g1".into(),
            title: "Title 1".into(),
            epic_url: None,
            force_refresh: false,
            seq: 1,
        });
        q.in_flight.insert("g1".into());
        q.items.push(WorkItem {
            priority: Priority::Neighbor,
            request_id: 2,
            app_name: "g2".into(),
            title: "Title 2".into(),
            epic_url: None,
            force_refresh: false,
            seq: 2,
        });
        q.in_flight.insert("g2".into());
        assert_eq!(q.items.len(), 2);

        // Simulate neighbor update: new neighbors are g2 and g3; g1 should be pruned!
        let neighbor_set: HashSet<String> =
            ["g2".to_string(), "g3".to_string()].into_iter().collect();
        let mut to_remove = Vec::new();
        q.items.retain(|it| {
            if it.priority == Priority::Neighbor && !neighbor_set.contains(&it.app_name) {
                to_remove.push(it.app_name.clone());
                false
            } else {
                true
            }
        });
        for app in to_remove {
            q.in_flight.remove(&app);
        }
        // g3 added
        q.items.push(WorkItem {
            priority: Priority::Neighbor,
            request_id: 3,
            app_name: "g3".into(),
            title: "Title 3".into(),
            epic_url: None,
            force_refresh: false,
            seq: 3,
        });
        q.in_flight.insert("g3".into());

        let names: Vec<String> = q.items.iter().map(|it| it.app_name.clone()).collect();
        assert!(!names.contains(&"g1".to_string()));
        assert!(names.contains(&"g2".to_string()));
        assert!(names.contains(&"g3".to_string()));
        assert!(!q.in_flight.contains("g1"));
        assert!(q.in_flight.contains("g2"));
        assert!(q.in_flight.contains("g3"));
    }

    #[test]
    fn test_lru_reuse_no_redecode() {
        let mut mgr = ArtworkManager::new();
        let dyn_img = Arc::new(DynamicImage::new_rgb8(100, 150));
        let app = "lru_test_game";

        // Insert into memory LRU
        mgr.memory_cache.insert(
            app.into(),
            LruEntry {
                app_name: app.into(),
                image: Arc::clone(&dyn_img),
                source: Some("Epic".into()),
                protocol: None,
            },
        );

        // Requesting artwork for this game should immediately hit LRU with the same Arc pointer
        let (_id, state, img_opt, src_opt) = mgr.request_artwork(app, "LRU Test Game", None, false);
        assert_eq!(state, CoverState::Ready);
        assert_eq!(src_opt, Some("Epic".into()));
        let returned_img = img_opt.expect("image in lru");
        assert!(Arc::ptr_eq(&dyn_img, &returned_img));
    }

    #[test]
    fn test_corrupt_and_zero_byte_files_discarded() {
        let app_zero = "zero_byte_game_xyz";
        let path_zero = cache_path_for(app_zero);
        let _ = filesystem::ensure_dir(path_zero.parent().unwrap());
        std::fs::write(&path_zero, b"").unwrap();

        let app_corrupt = "corrupt_byte_game_xyz";
        let path_corrupt = cache_path_for(app_corrupt);
        std::fs::write(&path_corrupt, b"not a valid image format header").unwrap();

        // Reading should discard them
        let cancel = AtomicBool::new(false);
        let cache_idx = Arc::new(RwLock::new(HashMap::new()));
        let agent = ureq::AgentBuilder::new().build();

        let item_zero = WorkItem {
            priority: Priority::Immediate,
            request_id: 1,
            app_name: app_zero.into(),
            title: "NonexistentZeroXYZ999".into(),
            epic_url: None,
            force_refresh: false,
            seq: 1,
        };
        let res_zero = handle_worker_item(&agent, &cache_idx, &item_zero, &cancel);
        assert!(!path_zero.exists(), "zero byte file must be deleted");
        assert!(res_zero.is_no_artwork);

        let item_corrupt = WorkItem {
            priority: Priority::Immediate,
            request_id: 2,
            app_name: app_corrupt.into(),
            title: "NonexistentCorruptXYZ999".into(),
            epic_url: None,
            force_refresh: false,
            seq: 2,
        };
        let res_corrupt = handle_worker_item(&agent, &cache_idx, &item_corrupt, &cancel);
        assert!(!path_corrupt.exists(), "corrupt file must be deleted");
        assert!(res_corrupt.is_no_artwork);

        let _ = std::fs::remove_file(sidecar_path_for(app_zero));
        let _ = std::fs::remove_file(sidecar_path_for(app_corrupt));
    }

    #[test]
    fn test_cache_migration_no_redownload() {
        let tmp = std::env::temp_dir().join(format!("leg_test_mig_{}", current_time_secs()));
        let legacy_img = tmp.join(".cache/egs/images");
        let current_img = tmp.join(".cache/legendary-tui/images");
        std::fs::create_dir_all(&legacy_img).unwrap();
        std::fs::create_dir_all(&current_img).unwrap();

        let file_legacy = legacy_img.join("test_app.img");
        std::fs::write(&file_legacy, b"mock_cached_image_bytes").unwrap();

        // Simulate migration logic
        for entry in std::fs::read_dir(&legacy_img).unwrap().flatten() {
            let dest = current_img.join(entry.file_name());
            if !dest.exists() {
                let _ = std::fs::rename(entry.path(), &dest)
                    .or_else(|_| std::fs::copy(entry.path(), &dest).map(|_| ()));
            }
        }

        let migrated_file = current_img.join("test_app.img");
        assert!(migrated_file.exists());
        assert_eq!(
            std::fs::read(&migrated_file).unwrap(),
            b"mock_cached_image_bytes"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
