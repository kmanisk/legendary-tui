//! Small library cache: instant startup, explicit refresh only.
//! `~/.cache/egs/library.json`, written atomically, never polled.

use serde::{Deserialize, Serialize};

use crate::filesystem;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachedGame {
    pub app_name: String,
    pub title: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CacheFile {
    saved_at_unix: u64,
    games: Vec<CachedGame>,
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn load() -> Option<Vec<CachedGame>> {
    let bytes = std::fs::read(filesystem::cache_file()).ok()?;
    let parsed: CacheFile = serde_json::from_slice(&bytes).ok()?;
    if parsed.games.is_empty() {
        return None;
    }
    Some(parsed.games)
}

pub fn save(games: &[CachedGame]) -> Result<(), String> {
    let file = CacheFile {
        saved_at_unix: now_unix(),
        games: games.to_vec(),
    };
    let bytes = serde_json::to_vec_pretty(&file).map_err(|e| format!("cache encode: {e}"))?;
    filesystem::atomic_write(&filesystem::cache_file(), &bytes)
}
