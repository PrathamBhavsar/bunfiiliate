//! Tracker metrics, served by `bf_rt::metrics` on `BF_METRICS_ADDR` (default 127.0.0.1:9100).

use bf_rt::metrics::{Counter, Gauge, Histogram, counter, gauge, gauge_value, histogram};

/// Spool write (group-commit ack) latency per accepted record, microseconds.
pub static INGEST: Histogram<9> = Histogram::new(
    [100, 250, 500, 1_000, 2_500, 5_000, 10_000, 50_000, 250_000],
    1e6,
);
/// Records per DB batch.
pub static BATCH: Histogram<7> = Histogram::new([1, 5, 10, 50, 100, 500, 1000], 1.0);
/// Requests answered 503 because the writer queue was full.
pub static SHED: Counter = Counter::new();
/// Records skipped because Postgres rejected them for good (bad data, not an outage).
pub static DEAD_LETTERED: Counter = Counter::new();
/// Program rows left out of a snapshot because a stored secret failed to decrypt.
pub static SNAPSHOT_DECRYPT_FAIL: Counter = Counter::new();
/// Spool end offset: total bytes appended.
pub static SPOOL_BYTES: Gauge = Gauge::new();
/// Spool offset already in Postgres; `bf_spool_lag` is the bytes between the two.
pub static SPOOL_CHECKPOINT: Gauge = Gauge::new();

/// Spool free bytes on its filesystem, sampled each second.
pub static SPOOL_FREE_BYTES: Gauge = Gauge::new();
/// Total bytes of spool segments on disk.
pub static SPOOL_DISK_BYTES: Gauge = Gauge::new();
/// 1 while the spool disk is (nearly) full and the tracker sheds.
pub static SPOOL_SHEDDING: Gauge = Gauge::new();
/// Requests shed because the spool disk is full, by route (indexed like `SHED_ROUTES`).
static SPOOL_SHED: [Counter; 3] = [Counter::new(), Counter::new(), Counter::new()];
const SHED_ROUTES: [&str; 3] = ["c", "r", "events"];

pub fn spool_shed(route: &str) {
    if let Some(i) = SHED_ROUTES.iter().position(|r| *r == route) {
        SPOOL_SHED[i].inc();
    }
}

pub fn spool_report(free: u64, bytes: u64, shedding: bool) {
    SPOOL_FREE_BYTES.set(free);
    SPOOL_DISK_BYTES.set(bytes);
    SPOOL_SHEDDING.set(shedding as u64);
}

/// v1 (deprecated) requests by matched route, indexed like `V1_ROUTES`.
static V1_REQUESTS: [Counter; 4] = [
    Counter::new(),
    Counter::new(),
    Counter::new(),
    Counter::new(),
];
const V1_ROUTES: [&str; 4] = [
    "/c",
    "/v1/events/{key}",
    "/v1/webhooks/stripe/{key}",
    "/v1/webhooks/paddle/{key}",
];

pub fn v1_request(route: &str) {
    if let Some(i) = V1_ROUTES.iter().position(|r| *r == route) {
        V1_REQUESTS[i].inc();
    }
}

pub fn render() -> String {
    let mut out = String::new();
    out.push_str(
        "# HELP bf_tracker_v1_requests_total Requests to deprecated v1 routes.\n\
         # TYPE bf_tracker_v1_requests_total counter\n",
    );
    for (route, c) in V1_ROUTES.iter().zip(&V1_REQUESTS) {
        out.push_str(&format!(
            "bf_tracker_v1_requests_total{{route=\"{route}\"}} {}\n",
            c.get()
        ));
    }
    histogram(
        &mut out,
        "bf_ingest_latency_seconds",
        "Spool write latency per accepted record.",
        &INGEST,
    );
    histogram(&mut out, "bf_batch_size", "Records per DB batch.", &BATCH);
    counter(
        &mut out,
        "bf_shed_total",
        "Requests shed with 503, writer queue full.",
        &SHED,
    );
    counter(
        &mut out,
        "bf_dead_lettered_total",
        "Spool records skipped after Postgres rejected them permanently.",
        &DEAD_LETTERED,
    );
    counter(
        &mut out,
        "bf_snapshot_decrypt_fail_total",
        "Program rows skipped by a snapshot load: secret failed to decrypt.",
        &SNAPSHOT_DECRYPT_FAIL,
    );
    out.push_str(
        "# HELP bf_spool_shed_total Requests shed because the spool disk is full.\n\
         # TYPE bf_spool_shed_total counter\n",
    );
    for (route, c) in SHED_ROUTES.iter().zip(&SPOOL_SHED) {
        out.push_str(&format!(
            "bf_spool_shed_total{{route=\"{route}\"}} {}\n",
            c.get()
        ));
    }
    gauge(
        &mut out,
        "bf_spool_free_bytes",
        "Free bytes on the spool filesystem.",
        &SPOOL_FREE_BYTES,
    );
    gauge(
        &mut out,
        "bf_spool_disk_bytes",
        "Bytes of spool segments on disk.",
        &SPOOL_DISK_BYTES,
    );
    gauge(
        &mut out,
        "bf_spool_shedding",
        "1 while the spool disk is full and the tracker sheds.",
        &SPOOL_SHEDDING,
    );
    gauge(
        &mut out,
        "bf_spool_bytes",
        "Total bytes appended to the spool.",
        &SPOOL_BYTES,
    );
    gauge_value(
        &mut out,
        "bf_spool_lag",
        "Spool bytes not yet checkpointed into Postgres.",
        SPOOL_BYTES.get().saturating_sub(SPOOL_CHECKPOINT.get()),
    );
    out
}
