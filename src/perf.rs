//! Zero-cost performance logging when LEGENDARY_TUI_DEBUG=1 is unset.
//! Writes detailed timing metrics to `/tmp/legendary-tui-perf.log`.

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static DEBUG_ENABLED: OnceLock<bool> = OnceLock::new();
static START_INSTANT: OnceLock<Instant> = OnceLock::new();

pub static LRU_HITS: AtomicU64 = AtomicU64::new(0);
pub static DISK_HITS: AtomicU64 = AtomicU64::new(0);
pub static NETWORK_HITS: AtomicU64 = AtomicU64::new(0);
pub static FIRST_DRAW_LOGGED: AtomicBool = AtomicBool::new(false);

pub fn init_start_time() {
    START_INSTANT.get_or_init(Instant::now);
}

pub fn is_debug() -> bool {
    *DEBUG_ENABLED.get_or_init(|| {
        std::env::var("LEGENDARY_TUI_DEBUG")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

pub fn log_perf(msg: &str) {
    if !is_debug() {
        return;
    }
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/legendary-tui-perf.log")
    {
        let now_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let _ = writeln!(f, "[{:.4}] {}", now_epoch, msg);
    }
}

pub fn log_first_draw() {
    if !is_debug() {
        return;
    }
    if FIRST_DRAW_LOGGED.swap(true, Ordering::Relaxed) {
        return;
    }
    let elapsed = START_INSTANT
        .get()
        .map(|s| s.elapsed())
        .unwrap_or(Duration::ZERO);
    log_perf(&format!(
        "STARTUP: Process start to first draw completed in {:.2} ms",
        elapsed.as_secs_f64() * 1000.0
    ));
}

pub fn log_prefetch_queue(pending: usize, cached: usize, negative: usize, failed: usize) {
    if !is_debug() {
        return;
    }
    log_perf(&format!(
        "PREFETCH_PROGRESS: queue_pending={} | cached={} | negative={} | failed={}",
        pending, cached, negative, failed
    ));
}

#[allow(dead_code)]
pub fn log_selection_stats() {
    if !is_debug() {
        return;
    }
    let lru = LRU_HITS.load(Ordering::Relaxed);
    let disk = DISK_HITS.load(Ordering::Relaxed);
    let net = NETWORK_HITS.load(Ordering::Relaxed);
    log_perf(&format!(
        "SELECTION_STATS: total_lru_hits={} | total_disk_hits={} | total_network_hits={}",
        lru, disk, net
    ));
}

pub struct SelectionTrace {
    pub app_name: String,
    pub start: Instant,
    pub index_lookup_us: Option<u64>,
    pub sidecar_read_us: Option<u64>,
    pub file_read_us: Option<u64>,
    pub decode_ms: Option<f64>,
    pub decode_dim: Option<(u32, u32)>,
    pub decode_format: Option<String>,
    pub resize_ms: Option<f64>,
    pub protocol_ms: Option<f64>,
    pub hit_type: String,
}

impl SelectionTrace {
    pub fn new(app_name: String) -> Self {
        Self {
            app_name,
            start: Instant::now(),
            index_lookup_us: None,
            sidecar_read_us: None,
            file_read_us: None,
            decode_ms: None,
            decode_dim: None,
            decode_format: None,
            resize_ms: None,
            protocol_ms: None,
            hit_type: "unknown".into(),
        }
    }

    pub fn finish_and_log(&self) {
        if !is_debug() {
            return;
        }
        let total_ms = self.start.elapsed().as_secs_f64() * 1000.0;
        let dim_str = self
            .decode_dim
            .map(|(w, h)| format!("{}x{}", w, h))
            .unwrap_or_else(|| "none".into());
        let fmt_str = self.decode_format.as_deref().unwrap_or("none");

        log_perf(&format!(
            "SELECTION: app='{}' | hit={} | total={:.2}ms | index_lookup={}us | sidecar_read={}us | file_read={}us | decode={:.2}ms (fmt:{}, {}) | resize={:.2}ms | protocol={:.2}ms",
            self.app_name,
            self.hit_type,
            total_ms,
            self.index_lookup_us.unwrap_or(0),
            self.sidecar_read_us.unwrap_or(0),
            self.file_read_us.unwrap_or(0),
            self.decode_ms.unwrap_or(0.0),
            fmt_str,
            dim_str,
            self.resize_ms.unwrap_or(0.0),
            self.protocol_ms.unwrap_or(0.0),
        ));
    }
}
