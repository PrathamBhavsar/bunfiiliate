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
/// Spool end offset: total bytes appended.
pub static SPOOL_BYTES: Gauge = Gauge::new();
/// Spool offset already in Postgres; `bf_spool_lag` is the bytes between the two.
pub static SPOOL_CHECKPOINT: Gauge = Gauge::new();

pub fn render() -> String {
    let mut out = String::new();
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
