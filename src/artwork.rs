//! Cover artwork retrieval, disk caching, and non-blocking decoding.
//!
//! Artwork is downloaded in a background worker thread via `ureq` (with 5-second timeout),
//! cached deterministically on disk at `~/.cache/legendary-tui/images/<app_name>.img`,
//! and held in a small bounded in-memory cache (up to 16 images).
//! All operations are zero-blocking on the UI thread.

use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use image::DynamicImage;

use crate::filesystem;

const MAX_IMAGE_BYTES: u64 = 15 * 1024 * 1024; // 15 MB safety limit
const MAX_MEMORY_CACHE: usize = 16;

pub struct ArtworkRequest {
    pub app_name: String,
    pub url: String,
    pub force_refresh: bool,
}

pub struct ArtworkResponse {
    pub app_name: String,
    pub result: Result<Arc<DynamicImage>, String>,
}

pub struct ArtworkManager {
    memory_cache: HashMap<String, Arc<DynamicImage>>,
    cache_order: VecDeque<String>,
    pending: HashMap<String, bool>,
    tx_req: Sender<ArtworkRequest>,
    rx_res: Receiver<ArtworkResponse>,
}

impl ArtworkManager {
    pub fn new() -> Self {
        let (tx_req, rx_req) = channel::<ArtworkRequest>();
        let (tx_res, rx_res) = channel::<ArtworkResponse>();

        // Spawn background worker thread
        thread::Builder::new()
            .name("artwork-worker".into())
            .spawn(move || {
                let agent = ureq::AgentBuilder::new()
                    .timeout(Duration::from_secs(5))
                    .build();

                while let Ok(req) = rx_req.recv() {
                    let result = handle_artwork_request(&agent, &req);
                    let _ = tx_res.send(ArtworkResponse {
                        app_name: req.app_name,
                        result,
                    });
                }
            })
            .expect("failed to spawn artwork worker thread");

        Self {
            memory_cache: HashMap::new(),
            cache_order: VecDeque::new(),
            pending: HashMap::new(),
            tx_req,
            rx_res,
        }
    }

    /// Retrieve image from in-memory cache immediately if already present.
    pub fn get_cached(&self, app_name: &str) -> Option<Arc<DynamicImage>> {
        self.memory_cache.get(app_name).cloned()
    }

    /// Request artwork for an application. If in memory cache, returns immediately.
    /// Otherwise schedules background disk load / network download if not already pending.
    pub fn request_artwork(
        &mut self,
        app_name: &str,
        url: Option<&str>,
        force_refresh: bool,
    ) -> Option<Arc<DynamicImage>> {
        if !force_refresh {
            if let Some(img) = self.memory_cache.get(app_name) {
                return Some(Arc::clone(img));
            }
        } else {
            self.invalidate(app_name);
        }

        if self.pending.contains_key(app_name) {
            return None;
        }

        // Even if url is None, check if disk cache exists
        let disk_path = cache_path_for(app_name);
        if !force_refresh && disk_path.exists() {
            self.pending.insert(app_name.to_string(), true);
            let _ = self.tx_req.send(ArtworkRequest {
                app_name: app_name.to_string(),
                url: String::new(),
                force_refresh: false,
            });
            return None;
        }

        if let Some(u) = url {
            if !u.is_empty() {
                self.pending.insert(app_name.to_string(), true);
                let _ = self.tx_req.send(ArtworkRequest {
                    app_name: app_name.to_string(),
                    url: u.to_string(),
                    force_refresh,
                });
            }
        }

        None
    }

    /// Invalidate in-memory and on-disk cached artwork for an app.
    pub fn invalidate(&mut self, app_name: &str) {
        self.memory_cache.remove(app_name);
        self.cache_order.retain(|k| k != app_name);
        let path = cache_path_for(app_name);
        if path.exists() {
            let _ = std::fs::remove_file(path);
        }
    }

    /// Poll for completed artwork requests. Returns true if cache was updated.
    pub fn poll_updates(&mut self) -> bool {
        let mut updated = false;
        while let Ok(resp) = self.rx_res.try_recv() {
            self.pending.remove(&resp.app_name);
            match resp.result {
                Ok(img) => {
                    self.cache_order.retain(|k| k != &resp.app_name);
                    self.cache_order.push_back(resp.app_name.clone());
                    self.memory_cache.insert(resp.app_name, img);
                    while self.memory_cache.len() > MAX_MEMORY_CACHE {
                        if let Some(oldest) = self.cache_order.pop_front() {
                            self.memory_cache.remove(&oldest);
                        }
                    }
                    updated = true;
                }
                Err(_) => {
                    // Mark completed with error; updated so UI stops spinner/loading indicator
                    updated = true;
                }
            }
        }
        updated
    }

    pub fn is_pending(&self, app_name: &str) -> bool {
        self.pending.contains_key(app_name)
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

fn handle_artwork_request(
    agent: &ureq::Agent,
    req: &ArtworkRequest,
) -> Result<Arc<DynamicImage>, String> {
    let cache_file = cache_path_for(&req.app_name);

    if !req.force_refresh && cache_file.exists() {
        if let Ok(bytes) = std::fs::read(&cache_file) {
            if let Ok(img) = image::load_from_memory(&bytes) {
                return Ok(Arc::new(img));
            }
            // Corrupt file on disk, remove it
            let _ = std::fs::remove_file(&cache_file);
        }
    }

    if req.url.is_empty() {
        return Err("No URL provided and no disk cache found".into());
    }

    // Download from URL
    let resp = agent
        .get(&req.url)
        .call()
        .map_err(|e| format!("HTTP request failed: {e}"))?;

    let mut bytes = Vec::new();
    resp.into_reader()
        .take(MAX_IMAGE_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Failed to read response body: {e}"))?;

    if bytes.is_empty() {
        return Err("Empty response body".into());
    }

    // Decode image
    let img =
        image::load_from_memory(&bytes).map_err(|e| format!("Failed to decode image: {e}"))?;

    // Cache to disk
    let _ = filesystem::atomic_write(&cache_file, &bytes);

    Ok(Arc::new(img))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(!mgr.is_pending("nonexistent"));
    }

    #[test]
    fn test_handle_artwork_request_disk_cache() {
        let agent = ureq::AgentBuilder::new().build();
        let app = "test_disk_app";
        let cache_file = cache_path_for(app);
        let img = image::DynamicImage::new_rgb8(2, 2);
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        let bytes = buf.into_inner();
        let _ = filesystem::atomic_write(&cache_file, &bytes);

        let req = ArtworkRequest {
            app_name: app.to_string(),
            url: String::new(),
            force_refresh: false,
        };
        let res = handle_artwork_request(&agent, &req);
        assert!(res.is_ok());
        let _ = std::fs::remove_file(cache_file);
    }
}
