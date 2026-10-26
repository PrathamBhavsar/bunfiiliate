//! Minimal Prometheus text registry: static atomics, no allocation on the hot path.
//! Rendering allocates, but only when `/metrics` is scraped.

use std::fmt::Write;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use axum::Router;
use axum::http::{HeaderMap, StatusCode, header::AUTHORIZATION};
use axum::routing::get;

pub struct Counter(AtomicU64);

impl Counter {
    pub const fn new() -> Self {
        Self(AtomicU64::new(0))
    }

    pub fn inc(&self) {
        self.add(1);
    }

    pub fn add(&self, n: u64) {
        self.0.fetch_add(n, Relaxed);
    }

    pub fn get(&self) -> u64 {
        self.0.load(Relaxed)
    }
}

impl Default for Counter {
    fn default() -> Self {
        Self::new()
    }
}

/// An unsigned gauge; also usable as a monotonic high-water mark via `max`.
pub struct Gauge(AtomicU64);

impl Gauge {
    pub const fn new() -> Self {
        Self(AtomicU64::new(0))
    }

    pub fn set(&self, v: u64) {
        self.0.store(v, Relaxed);
    }

    pub fn max(&self, v: u64) {
        self.0.fetch_max(v, Relaxed);
    }

    pub fn get(&self) -> u64 {
        self.0.load(Relaxed)
    }
}

impl Default for Gauge {
    fn default() -> Self {
        Self::new()
    }
}

/// Fixed-bucket histogram over integer observations. `bounds` are inclusive upper
/// bounds in raw units; `scale` converts raw units to the exposed unit (1e6 for
/// microseconds -> seconds, 1 for plain counts).
pub struct Histogram<const N: usize> {
    bounds: [u64; N],
    scale: f64,
    /// Per-bucket (non-cumulative) counts; the last slot is `+Inf`.
    buckets: [AtomicU64; N],
    inf: AtomicU64,
    sum: AtomicU64,
}

impl<const N: usize> Histogram<N> {
    pub const fn new(bounds: [u64; N], scale: f64) -> Self {
        Self {
            bounds,
            scale,
            buckets: [const { AtomicU64::new(0) }; N],
            inf: AtomicU64::new(0),
            sum: AtomicU64::new(0),
        }
    }

    pub fn observe(&self, v: u64) {
        let slot = self.bounds.iter().position(|&b| v <= b);
        match slot {
            Some(i) => self.buckets[i].fetch_add(1, Relaxed),
            None => self.inf.fetch_add(1, Relaxed),
        };
        self.sum.fetch_add(v, Relaxed);
    }

    pub fn observe_since(&self, start: std::time::Instant) {
        self.observe(start.elapsed().as_micros() as u64);
    }

    pub fn count(&self) -> u64 {
        self.buckets.iter().map(|b| b.load(Relaxed)).sum::<u64>() + self.inf.load(Relaxed)
    }
}

fn head(out: &mut String, name: &str, help: &str, kind: &str) {
    let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} {kind}");
}

pub fn counter(out: &mut String, name: &str, help: &str, c: &Counter) {
    head(out, name, help, "counter");
    let _ = writeln!(out, "{name} {}", c.get());
}

pub fn gauge(out: &mut String, name: &str, help: &str, g: &Gauge) {
    gauge_value(out, name, help, g.get());
}

pub fn gauge_value(out: &mut String, name: &str, help: &str, v: u64) {
    head(out, name, help, "gauge");
    let _ = writeln!(out, "{name} {v}");
}

pub fn histogram<const N: usize>(out: &mut String, name: &str, help: &str, h: &Histogram<N>) {
    head(out, name, help, "histogram");
    let mut cum = 0;
    for (b, n) in h.bounds.iter().zip(&h.buckets) {
        cum += n.load(Relaxed);
        let _ = writeln!(out, "{name}_bucket{{le=\"{}\"}} {cum}", *b as f64 / h.scale);
    }
    cum += h.inf.load(Relaxed);
    let _ = writeln!(out, "{name}_bucket{{le=\"+Inf\"}} {cum}");
    let _ = writeln!(out, "{name}_sum {}", h.sum.load(Relaxed) as f64 / h.scale);
    let _ = writeln!(out, "{name}_count {cum}");
}

/// Router serving `render()` at `GET /metrics`; with `token`, requires `Authorization: Bearer <token>`.
pub fn router(render: fn() -> String, token: Option<String>) -> Router {
    Router::new().route(
        "/metrics",
        get(move |headers: HeaderMap| async move {
            let want = token.map(|t| format!("Bearer {t}"));
            let got = headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok());
            if want.is_some() && got != want.as_deref() {
                return (StatusCode::UNAUTHORIZED, String::new());
            }
            (StatusCode::OK, render())
        }),
    )
}

/// Serve `/metrics` on `BF_METRICS_ADDR` (default `default_addr`, loopback) until the
/// task is dropped. Set `BF_METRICS_TOKEN` to require a bearer token. Never on the
/// public tracker or API port. Failures are logged: metrics must not take the process down.
pub async fn serve(default_addr: &str, render: fn() -> String) {
    let addr = std::env::var("BF_METRICS_ADDR").unwrap_or_else(|_| default_addr.to_owned());
    let token = std::env::var("BF_METRICS_TOKEN")
        .ok()
        .filter(|t| !t.is_empty());
    let run = async {
        let addr: SocketAddr = addr.parse()?;
        let listener = tokio::net::TcpListener::bind(addr).await?;
        tracing::info!(%addr, "metrics listening");
        axum::serve(listener, router(render, token)).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    };
    if let Err(e) = run.await {
        tracing::error!(error = %e, addr, "metrics server stopped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[test]
    fn metrics_histogram_is_cumulative() {
        static H: Histogram<2> = Histogram::new([1_000, 10_000], 1e6);
        H.observe(500);
        H.observe(5_000);
        H.observe(50_000);
        let mut out = String::new();
        histogram(&mut out, "bf_x_seconds", "x", &H);
        assert!(out.contains("bf_x_seconds_bucket{le=\"0.001\"} 1"));
        assert!(out.contains("bf_x_seconds_bucket{le=\"0.01\"} 2"));
        assert!(out.contains("bf_x_seconds_bucket{le=\"+Inf\"} 3"));
        assert!(out.contains("bf_x_seconds_count 3"));
        assert!(out.contains("bf_x_seconds_sum 0.0555"));
    }

    #[tokio::test]
    async fn metrics_route_requires_bearer_token() {
        let app = router(|| "bf_up 1\n".into(), Some("s3".into()));
        let get = |auth: Option<&str>| {
            let mut r = Request::get("/metrics");
            if let Some(a) = auth {
                r = r.header(AUTHORIZATION, a);
            }
            app.clone().oneshot(r.body(Body::empty()).unwrap())
        };
        assert_eq!(get(None).await.unwrap().status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            get(Some("Bearer nope")).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            get(Some("Bearer s3")).await.unwrap().status(),
            StatusCode::OK
        );
    }
}
