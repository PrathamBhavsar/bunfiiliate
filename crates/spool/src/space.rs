//! Disk-space guard for the spool. Samples free space; above the high-water mark the spool
//! reports "shedding" and callers answer 503 per route; it clears below the low-water mark
//! (hysteresis, so it does not flap around one threshold).

use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::{SpoolWriter, list_segments, read_checkpoint};

#[derive(Debug, Clone)]
pub struct SpaceConfig {
    /// Start shedding when the filesystem is this percent used.
    pub high_pct: u64,
    /// Stop shedding below this percent used.
    pub low_pct: u64,
    /// Also shed when fewer free bytes than this remain.
    pub min_free_bytes: u64,
    /// Optional cap on the total bytes of spool segments.
    pub max_bytes: Option<u64>,
}

impl Default for SpaceConfig {
    fn default() -> Self {
        Self {
            high_pct: 85,
            low_pct: 70,
            min_free_bytes: 256 * 1024 * 1024,
            max_bytes: None,
        }
    }
}

impl SpaceConfig {
    /// `BF_SPOOL_HIGH_WATER_PCT`, `BF_SPOOL_LOW_WATER_PCT`, `BF_SPOOL_MIN_FREE_BYTES`,
    /// `BF_SPOOL_MAX_BYTES`. Unparsable values fall back to the default.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let num = |k: &str| get(k).and_then(|v| v.trim().parse::<u64>().ok());
        let d = Self::default();
        let high = num("BF_SPOOL_HIGH_WATER_PCT")
            .unwrap_or(d.high_pct)
            .min(100);
        // The low mark can never sit above the high one.
        let low = num("BF_SPOOL_LOW_WATER_PCT").unwrap_or(d.low_pct).min(high);
        Self {
            high_pct: high,
            low_pct: low,
            min_free_bytes: num("BF_SPOOL_MIN_FREE_BYTES").unwrap_or(d.min_free_bytes),
            max_bytes: num("BF_SPOOL_MAX_BYTES"),
        }
    }
}

/// `(free_bytes, total_bytes)` of the filesystem holding `path` (what an unprivileged writer can use).
pub fn free_space(path: &Path) -> io::Result<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated path and `st` a valid out pointer.
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let frag = st.f_frsize as u64;
    Ok((st.f_bavail as u64 * frag, st.f_blocks as u64 * frag))
}

/// Ok -> Shedding at the high-water mark, back to Ok below the low-water mark.
#[derive(Debug, Clone)]
pub struct SpaceGuard {
    cfg: SpaceConfig,
    shedding: Arc<AtomicBool>,
}

impl SpaceGuard {
    pub fn new(cfg: SpaceConfig) -> Self {
        Self {
            cfg,
            shedding: Arc::default(),
        }
    }

    /// Share the flag the writer sets on ENOSPC.
    pub fn for_writer(cfg: SpaceConfig, w: &SpoolWriter) -> Self {
        Self {
            cfg,
            shedding: Arc::clone(w.shedding_flag()),
        }
    }

    pub fn shedding(&self) -> bool {
        self.shedding.load(Ordering::SeqCst)
    }

    /// Feed one sample. `spool_bytes` is the total size of the segments. Returns true when
    /// this call moved Ok -> Shedding.
    pub fn observe(&self, free: u64, total: u64, spool_bytes: u64) -> bool {
        let used_pct = total
            .saturating_sub(free)
            .saturating_mul(100)
            .checked_div(total)
            .unwrap_or(0);
        let over_cap = self.cfg.max_bytes.is_some_and(|m| spool_bytes > m);
        let hot = used_pct >= self.cfg.high_pct || free < self.cfg.min_free_bytes || over_cap;
        if hot {
            return !self.shedding.swap(true, Ordering::SeqCst);
        }
        let cool = used_pct < self.cfg.low_pct
            && free >= self.cfg.min_free_bytes
            && self.cfg.max_bytes.is_none_or(|m| spool_bytes <= m);
        if cool {
            self.shedding.store(false, Ordering::SeqCst);
        }
        false
    }
}

/// Total bytes of spool segments in `dir`.
pub fn segment_bytes(dir: &Path) -> u64 {
    list_segments(dir)
        .map(|s| s.iter().map(|m| m.file_len).sum())
        .unwrap_or(0)
}

/// Sample once a second. On the transition to Shedding, collect segments below the
/// checkpoint to free space. Gauges are updated through `report`.
pub fn spawn_sampler(
    writer: SpoolWriter,
    cfg: SpaceConfig,
    report: fn(free: u64, bytes: u64, shedding: bool),
) -> tokio::task::JoinHandle<()> {
    let guard = SpaceGuard::for_writer(cfg, &writer);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tick.tick().await;
            let dir = writer.dir().to_owned();
            let bytes = segment_bytes(&dir);
            let Ok((free, total)) = free_space(&dir) else {
                continue;
            };
            if guard.observe(free, total, bytes) {
                free_below_checkpoint(&writer);
            }
            report(free, bytes, guard.shedding());
        }
    })
}

fn free_below_checkpoint(writer: &SpoolWriter) {
    if let Ok(cp) = read_checkpoint(writer.dir()) {
        let _ = writer.gc(cp);
    }
}
