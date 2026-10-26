//! Tracker: validates against an in-memory snapshot, spools every click durably,
//! then batches it into Postgres. No DB call on the request path.

pub mod event;
pub mod snapshot;
pub mod writer;

use std::net::SocketAddr;
use std::sync::{Arc, LazyLock};
use std::time::{SystemTime, UNIX_EPOCH};

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
use bf_db::tracker::Click;
use bf_spool::{HEADER_LEN, SpoolConfig, SpoolWriter};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use uuid::Uuid;

use event::{clip, country, ip_hash, normalize_ua, referrer_host, url_path, with_click};
use snapshot::{CodeEntry, Snapshot};
use writer::{Error, Queued};

const TRACKER_JS: &[u8] = include_bytes!("../js/tracker.min.js");
const QUEUE_CAPACITY: usize = 10_000;
const MAX_BODY: usize = 8 * 1024;

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
}

pub struct Tracker {
    pub state: AppState,
    /// Finishes once all `AppState` clones are dropped and the queue is drained.
    pub writer: JoinHandle<()>,
    pub watcher: JoinHandle<()>,
}

/// Open the spool, replay it into Postgres, load the snapshot, start the writer.
pub async fn start(pool: PgPool, spool: SpoolConfig, ip_salt: &str) -> Result<Tracker, Error> {
    let spool = SpoolWriter::open(spool).await?;
    let checkpoint = writer::replay(&pool, &spool).await?;
    let (snapshot, watcher) = snapshot::load_and_watch(pool.clone()).await?;
    let (queue, rx) = mpsc::channel(QUEUE_CAPACITY);
    let writer = writer::spawn(pool.clone(), spool.clone(), rx, checkpoint);
    let state = AppState {
        pool,
        snapshot,
        spool,
        queue,
        ip_salt: ip_salt.into(),
    };
    Ok(Tracker {
        state,
        writer,
        watcher,
    })
}

pub fn router(state: AppState) -> Router {
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
    match ingest(s, click).await {
        Ok(()) => (StatusCode::FOUND, [(LOCATION, location)]).into_response(),
        Err(status) => status.into_response(),
    }
}

/// `{code, vid, url, ref}` from tracker.js.
#[derive(Deserialize)]
struct Beacon {
    code: String,
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
    let snap = s.snapshot.load_full();
    let Some(entry) = snap.codes.get(&b.code) else {
        return StatusCode::NOT_FOUND;
    };
    let click = s.click(
        &b.code,
        entry,
        Some(&b.vid),
        b.referrer.as_deref(),
        &b.url,
        &headers,
        &ext,
    );
    match ingest(s, click).await {
        Ok(()) => StatusCode::NO_CONTENT,
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
        let peer = ext
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip().to_string());
        let ip = header(headers, "cf-connecting-ip")
            .map(str::to_owned)
            .or_else(|| {
                let xff = header(headers, "x-forwarded-for")?;
                Some(xff.split(',').next()?.trim().to_owned())
            })
            .or(peer);
        let visitor = visitor.filter(|v| !v.is_empty());
        Click {
            id,
            program_id: entry.program_id,
            affiliate_id: entry.affiliate_id,
            code: code.to_owned(),
            visitor_id: visitor.map_or_else(|| id.to_string(), |v| clip(v, 64)),
            ts_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as i64),
            ip_hash: ip.map(|ip| ip_hash(&self.ip_salt, &ip)),
            country: header(headers, "cf-ipcountry").and_then(country),
            device: ua.device,
            os: ua.os,
            browser: ua.browser,
            referrer_host: referrer.and_then(referrer_host).map(|h| clip(&h, 255)),
            landing_path: Some(clip(&url_path(landing), 512)),
        }
    }
}

/// Reserve queue room (503 when full), spool the click (waits for the group-commit
/// ack), then hand it to the DB writer. Runs detached so a dropped client connection
/// cannot strand a spooled record outside the queue and stall the checkpoint.
async fn ingest(state: AppState, click: Click) -> Result<(), StatusCode> {
    let task = tokio::spawn(async move {
        let permit = state
            .queue
            .try_reserve()
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        let payload = serde_json::to_vec(&click).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let end = state.spool.write(&payload).await.map_err(|e| {
            tracing::error!(error = %e, "spool write failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        let start = end - HEADER_LEN as u64 - payload.len() as u64;
        permit.send(Queued { start, end, click });
        Ok(())
    });
    task.await.unwrap_or(Err(StatusCode::INTERNAL_SERVER_ERROR))
}
