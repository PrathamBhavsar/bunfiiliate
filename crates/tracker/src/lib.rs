//! Tracker: validates against an in-memory snapshot, spools every click durably,
//! then batches it into Postgres. No DB call on the request path.

pub mod event;
pub mod geo;
pub mod metrics;
pub mod proxy;
pub mod snapshot;
pub mod webhook;
pub mod writer;

use std::net::SocketAddr;
use std::sync::{Arc, LazyLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, State};
use axum::http::header::{
    ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
    ACCESS_CONTROL_MAX_AGE, ACCESS_CONTROL_REQUEST_HEADERS, CACHE_CONTROL, CONTENT_TYPE, ETAG,
    IF_NONE_MATCH, LOCATION, REFERER, USER_AGENT,
};
use axum::http::{Extensions, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::map_response;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use bf_core::{ClickFacts, FraudRule, default_rules, first_flag};
use bf_db::tracker::{Click, InboundEvent};
use bf_spool::{HEADER_LEN, SpoolConfig, SpoolWriter};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use uuid::Uuid;

use event::{clip, ip_hash, normalize_ua, referrer_host, url_path, with_click};
use geo::Geo;
use proxy::Trust;
use snapshot::{CodeEntry, Snapshot};
use webhook::Source;
use writer::{Error, Queued, Record};

const TRACKER_JS: &[u8] = include_bytes!("../js/tracker.min.js");
const QUEUE_CAPACITY: usize = 10_000;
/// Share of the queue (a tenth) clicks may not take: a click flood must not make Stripe or
/// Paddle deliveries get 503 (they retry, but late conversions delay payouts).
const EVENT_RESERVE_DIVISOR: usize = 10;
const MAX_BODY: usize = 8 * 1024;
const MAX_EVENT_BODY: usize = 256 * 1024;

static TRACKER_JS_ETAG: LazyLock<String> = LazyLock::new(|| {
    let digest = Sha256::digest(TRACKER_JS);
    let hex: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!("\"{hex}\"")
});

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub snapshot: Arc<ArcSwap<Snapshot>>,
    pub spool: SpoolWriter,
    pub queue: mpsc::Sender<Queued>,
    pub ip_salt: Arc<str>,
    /// Which headers may name the client; trusts none until the operator says so.
    pub trust: Arc<Trust>,
    pub geo: Arc<Geo>,
    pub fraud: Arc<[Box<dyn FraudRule>]>,
}

pub struct Tracker {
    pub state: AppState,
    /// Finishes once all `AppState` clones are dropped and the queue is drained.
    pub writer: JoinHandle<()>,
    pub watcher: JoinHandle<()>,
}

/// Open the spool, replay it into Postgres, load the snapshot, start the writer.
pub async fn start(
    pool: PgPool,
    spool: SpoolConfig,
    ip_salt: &str,
    geo: Geo,
) -> Result<Tracker, Error> {
    let spool = SpoolWriter::open(spool).await?;
    let checkpoint = writer::replay(&pool, &spool).await?;
    metrics::SPOOL_BYTES.max(checkpoint);
    metrics::SPOOL_CHECKPOINT.set(checkpoint);
    let (snapshot, watcher) = snapshot::load_and_watch(pool.clone()).await?;
    let (queue, rx) = mpsc::channel(QUEUE_CAPACITY);
    let writer = writer::spawn(pool.clone(), spool.clone(), rx, checkpoint);
    let state = AppState {
        pool,
        snapshot,
        spool,
        queue,
        ip_salt: ip_salt.into(),
        trust: Arc::default(),
        geo: Arc::new(geo),
        fraud: default_rules().into(),
    };
    Ok(Tracker {
        state,
        writer,
        watcher,
    })
}

pub fn router(state: AppState) -> Router {
    // Conversion payloads (Stripe events especially) outgrow the click body limit.
    let events = Router::new()
        .route("/v1/events/{key}", post(generic_event))
        .route("/v1/webhooks/stripe/{key}", post(stripe_webhook))
        .route("/v1/webhooks/paddle/{key}", post(paddle_webhook))
        .layer(DefaultBodyLimit::max(MAX_EVENT_BODY));
    Router::new()
        .route("/t.js", get(tracker_js))
        .route("/r/{code}", get(redirect))
        .route(
            "/c",
            post(beacon).options(preflight).layer(map_response(cors)),
        )
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(readyz))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .merge(events)
        .with_state(state)
}

async fn tracker_js(headers: HeaderMap) -> Response {
    let etag = TRACKER_JS_ETAG.as_str();
    let cache = [(CACHE_CONTROL, "public, max-age=86400"), (ETAG, etag)];
    if header(&headers, IF_NONE_MATCH.as_str()) == Some(etag) {
        return (StatusCode::NOT_MODIFIED, cache).into_response();
    }
    let js = [(CONTENT_TYPE, "application/javascript; charset=utf-8")];
    (cache, js, TRACKER_JS).into_response()
}

async fn readyz(State(s): State<AppState>) -> StatusCode {
    let db = sqlx::query("SELECT 1").execute(&s.pool).await.is_ok();
    if db && !s.queue.is_closed() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

async fn redirect(
    State(s): State<AppState>,
    Path(code): Path<String>,
    headers: HeaderMap,
    ext: Extensions,
) -> Response {
    let snap = s.snapshot.load_full();
    let Some(entry) = snap.codes.get(&code) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let referrer = header(&headers, REFERER.as_str());
    let click = s.click(
        &code,
        entry,
        None,
        referrer,
        &entry.landing_url,
        &headers,
        &ext,
    );
    let location = with_click(&entry.landing_url, &click.id.to_string());
    let Ok(location) = HeaderValue::try_from(location) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    match ingest(s, Record::Click(click)).await {
        Ok(()) => (StatusCode::FOUND, [(LOCATION, location)]).into_response(),
        Err(status) => status.into_response(),
    }
}

/// `{code, click, vid, url, ref}` from tracker.js. `code` is a `?ref=` referral code;
/// `click` is the id `/r` already recorded.
#[derive(Deserialize)]
struct Beacon {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    click: Option<String>,
    #[serde(default)]
    vid: String,
    #[serde(default)]
    url: String,
    #[serde(default, rename = "ref")]
    referrer: Option<String>,
}

/// The body is parsed whatever its Content-Type: sendBeacon may send text/plain.
async fn beacon(
    State(s): State<AppState>,
    headers: HeaderMap,
    ext: Extensions,
    body: Bytes,
) -> StatusCode {
    let Ok(b) = serde_json::from_slice::<Beacon>(&body) else {
        return StatusCode::BAD_REQUEST;
    };
    // A click id alone was recorded by `/r`: recording again would double count.
    let Some(code) = b.code.as_deref().filter(|c| !c.is_empty()) else {
        return match b.click {
            Some(_) => StatusCode::NO_CONTENT,
            None => StatusCode::BAD_REQUEST,
        };
    };
    let snap = s.snapshot.load_full();
    let Some(entry) = snap.codes.get(code) else {
        return StatusCode::NOT_FOUND;
    };
    let click = s.click(
        code,
        entry,
        Some(&b.vid),
        b.referrer.as_deref(),
        &b.url,
        &headers,
        &ext,
    );
    match ingest(s, Record::Click(click)).await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(status) => status,
    }
}

async fn generic_event(
    State(s): State<AppState>,
    Path(key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    conversion(Source::Generic, s, &key, &headers, &body).await
}

async fn stripe_webhook(
    State(s): State<AppState>,
    Path(key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    conversion(Source::Stripe, s, &key, &headers, &body).await
}

async fn paddle_webhook(
    State(s): State<AppState>,
    Path(key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    conversion(Source::Paddle, s, &key, &headers, &body).await
}

/// Verify the signature, then spool the event (waits for the fsync ack). The tracker
/// does no business logic: the worker turns `inbound_events` into conversions.
async fn conversion(
    source: Source,
    s: AppState,
    key: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> StatusCode {
    let snap = s.snapshot.load_full();
    let Some(program) = snap.programs.get(key) else {
        return StatusCode::NOT_FOUND;
    };
    let Some(secret) = source.secret(program) else {
        return StatusCode::BAD_REQUEST;
    };
    if let Err(e) = source.verify(headers, body, secret) {
        tracing::debug!(error = ?e, source = source.name(), "webhook rejected");
        return StatusCode::BAD_REQUEST;
    }
    // jsonb cannot hold a NUL escape; one such event would wedge the batch writer.
    let Ok(payload) = serde_json::from_slice::<Value>(body) else {
        return StatusCode::BAD_REQUEST;
    };
    if body.windows(6).any(|w| w == b"\\u0000") {
        return StatusCode::BAD_REQUEST;
    }
    let Some(external_id) = source.external_id(headers, &payload) else {
        return StatusCode::BAD_REQUEST;
    };
    let event = InboundEvent {
        id: Uuid::now_v7(),
        program_id: program.program_id,
        source: source.name().to_owned(),
        external_id,
        payload,
        received_ms: now_ms(),
    };
    match ingest(s, Record::Event(event)).await {
        Ok(()) => source.accepted(),
        Err(status) => status,
    }
}

async fn preflight(headers: HeaderMap) -> impl IntoResponse {
    let allow = headers
        .get(ACCESS_CONTROL_REQUEST_HEADERS)
        .cloned()
        .unwrap_or(HeaderValue::from_static("content-type"));
    (
        StatusCode::NO_CONTENT,
        [
            (
                ACCESS_CONTROL_ALLOW_METHODS,
                HeaderValue::from_static("POST, OPTIONS"),
            ),
            (ACCESS_CONTROL_ALLOW_HEADERS, allow),
            (ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("86400")),
        ],
    )
}

async fn cors(mut res: Response) -> Response {
    res.headers_mut()
        .insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    res
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

impl AppState {
    #[allow(clippy::too_many_arguments)]
    fn click(
        &self,
        code: &str,
        entry: &CodeEntry,
        visitor: Option<&str>,
        referrer: Option<&str>,
        landing: &str,
        headers: &HeaderMap,
        ext: &Extensions,
    ) -> Click {
        let id = Uuid::now_v7();
        let ua = normalize_ua(header(headers, USER_AGENT.as_str()).unwrap_or(""));
        let peer = ext.get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip());
        let ip = self.trust.client_ip(headers, peer).map(|ip| ip.to_string());
        let visitor = visitor.filter(|v| !v.is_empty());
        let ip_hash = ip.as_deref().map(|ip| ip_hash(&self.ip_salt, ip));
        let ts_ms = now_ms();
        // O(1) in-memory rules; a flagged click is still recorded.
        let fraud_reason = first_flag(
            &self.fraud,
            &ClickFacts {
                ip_hash: ip_hash.as_deref(),
                ua: header(headers, USER_AGENT.as_str()).unwrap_or(""),
                device: ua.device.as_deref(),
                code,
                ts_ms,
            },
        );
        Click {
            id,
            program_id: entry.program_id,
            affiliate_id: entry.affiliate_id,
            code: code.to_owned(),
            visitor_id: visitor.map_or_else(|| id.to_string(), |v| clip(v, 64)),
            ts_ms,
            country: self.geo.country(self.trust.country(headers), ip.as_deref()),
            ip_hash,
            device: ua.device,
            os: ua.os,
            browser: ua.browser,
            referrer_host: referrer.and_then(referrer_host).map(|h| clip(&h, 255)),
            landing_path: Some(clip(&url_path(landing), 512)),
            fraud_reason: fraud_reason.map(str::to_owned),
        }
    }
}

/// Reserve queue room (503 when full), spool the record (waits for the group-commit
/// ack), then hand it to the DB writer. Runs detached so a dropped client connection
/// cannot strand a spooled record outside the queue and stall the checkpoint.
async fn ingest(state: AppState, record: Record) -> Result<(), StatusCode> {
    let started = Instant::now();
    let task = tokio::spawn(async move {
        // Clicks leave the last tenth of the queue to signed conversion events.
        let reserve = state.queue.max_capacity() / EVENT_RESERVE_DIVISOR;
        let full = matches!(record, Record::Click(_)) && state.queue.capacity() <= reserve;
        let permit = state
            .queue
            .try_reserve()
            .ok()
            .filter(|_| !full)
            .ok_or_else(|| {
                metrics::SHED.inc();
                StatusCode::SERVICE_UNAVAILABLE
            })?;
        let payload = serde_json::to_vec(&record).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let end = state.spool.write(&payload).await.map_err(|e| {
            tracing::error!(error = %e, "spool write failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        let start = end - HEADER_LEN as u64 - payload.len() as u64;
        metrics::SPOOL_BYTES.max(end);
        permit.send(Queued { start, end, record });
        Ok(())
    });
    let result = task.await.unwrap_or(Err(StatusCode::INTERNAL_SERVER_ERROR));
    if result.is_ok() {
        metrics::INGEST.observe_since(started);
    }
    result
}
