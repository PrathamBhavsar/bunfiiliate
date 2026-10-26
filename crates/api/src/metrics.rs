//! API/worker metrics, served by `bf_rt::metrics` on `BF_METRICS_ADDR` (default 127.0.0.1:9101).

use bf_rt::metrics::{Counter, Gauge, Histogram, counter, gauge, histogram};

/// Outbox rows not yet dispatched, sampled each dispatch tick.
pub static OUTBOX_DEPTH: Gauge = Gauge::new();
/// Webhook deliveries that got a non-2xx answer or no answer.
pub static WEBHOOK_FAILURES: Counter = Counter::new();
/// Inbound events the worker claimed.
pub static EVENTS_PROCESSED: Counter = Counter::new();
/// Tick durations, microseconds.
pub static ROLLUP_TICK: Histogram<8> = Histogram::new(
    [
        1_000, 5_000, 25_000, 100_000, 500_000, 1_000_000, 5_000_000, 30_000_000,
    ],
    1e6,
);
pub static APPROVE_TICK: Histogram<8> = Histogram::new(
    [
        1_000, 5_000, 25_000, 100_000, 500_000, 1_000_000, 5_000_000, 30_000_000,
    ],
    1e6,
);

pub fn render() -> String {
    let mut out = String::new();
    gauge(
        &mut out,
        "bf_outbox_depth",
        "Outbox rows waiting for dispatch.",
        &OUTBOX_DEPTH,
    );
    counter(
        &mut out,
        "bf_webhook_delivery_failures_total",
        "Webhook deliveries that failed.",
        &WEBHOOK_FAILURES,
    );
    counter(
        &mut out,
        "bf_ingest_events_processed_total",
        "Inbound events processed by the worker.",
        &EVENTS_PROCESSED,
    );
    histogram(
        &mut out,
        "bf_rollup_tick_seconds",
        "Analytics rollup tick duration.",
        &ROLLUP_TICK,
    );
    histogram(
        &mut out,
        "bf_approve_tick_seconds",
        "Auto-approve tick duration.",
        &APPROVE_TICK,
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_render_has_every_series() {
        let out = render();
        for name in [
            "bf_outbox_depth",
            "bf_webhook_delivery_failures_total",
            "bf_ingest_events_processed_total",
            "bf_rollup_tick_seconds_count",
            "bf_approve_tick_seconds_count",
        ] {
            assert!(out.contains(name), "missing {name}");
        }
    }
}
