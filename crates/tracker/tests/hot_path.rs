use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use bf_spool::{SpoolConfig, SpoolWriter};
use sqlx::PgPool;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tower::ServiceExt;
use tracker::snapshot::Snapshot;
use tracker::writer::Queued;
use tracker::{AppState, router, start};
use uuid::Uuid;

const SALT: &str = "test-salt";
const CHROME: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                      (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

fn spool_dir() -> PathBuf {
    std::env::temp_dir().join(format!("bf-tracker-test-{}", Uuid::now_v7()))
}

async fn seed(pool: &PgPool, code: &str) {
    let (ws, user, aff, prog) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let q = |sql: &'static str| sqlx::query(sql);
    q("INSERT INTO workspaces (id, name, slug) VALUES ($1, 'w', 'w')")
        .bind(ws)
        .execute(pool)
        .await
        .unwrap();
    q(
        "INSERT INTO users (id, workspace_id, email, password_hash, role) \
       VALUES ($1, $2, 'a@x.io', 'h', 'affiliate')",
    )
    .bind(user)
    .bind(ws)
    .execute(pool)
    .await
    .unwrap();
    q("INSERT INTO affiliates (id, workspace_id, user_id) VALUES ($1, $2, $3)")
        .bind(aff)
        .bind(ws)
        .bind(user)
        .execute(pool)
        .await
        .unwrap();
    q(
        "INSERT INTO programs (id, workspace_id, name, currency, public_key, webhook_secret) \
       VALUES ($1, $2, 'p', 'USD', 'pk', 's')",
    )
    .bind(prog)
    .bind(ws)
    .execute(pool)
    .await
    .unwrap();
    add_code(pool, code).await;
}

async fn add_code(pool: &PgPool, code: &str) {
    sqlx::query(
        "INSERT INTO referral_codes (code, affiliate_id, program_id, landing_url) \
         SELECT $1, a.id, p.id, 'https://example.com/pricing?x=1' FROM affiliates a, programs p",
    )
    .bind(code)
    .execute(pool)
    .await
    .unwrap();
}

fn click_req(i: usize) -> Request<Body> {
    // Alternate /r redirects with beacons; beacons go out as text/plain like sendBeacon may.
    if i.is_multiple_of(2) {
        Request::get("/r/abc")
            .header("user-agent", CHROME)
            .header("referer", "https://Blog.Example.org/post?id=1")
            .header("cf-ipcountry", "de")
            .header("x-forwarded-for", "203.0.113.9, 10.0.0.1")
            .body(Body::empty())
            .unwrap()
    } else {
        beacon(&format!(
            r#"{{"code":"abc","vid":"v{i}","url":"https://example.com/a?b=1","ref":"https://t.co"}}"#
        ))
    }
}

fn beacon(body: &str) -> Request<Body> {
    Request::post("/c")
        .header("content-type", "text/plain;charset=UTF-8")
        .header("user-agent", CHROME)
        .body(Body::from(body.to_owned()))
        .unwrap()
}

async fn fire(app: &Router, n: usize) {
    let mut set = JoinSet::new();
    for i in 0..n {
        let app = app.clone();
        set.spawn(async move { (i, app.oneshot(click_req(i)).await.unwrap().status()) });
    }
    while let Some(r) = set.join_next().await {
        let (i, status) = r.unwrap();
        let want = if i.is_multiple_of(2) {
            StatusCode::FOUND
        } else {
            StatusCode::NO_CONTENT
        };
        assert_eq!(status, want, "request {i}");
    }
}

async fn clicks(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM clicks")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn wait_clicks(pool: &PgPool, n: i64) {
    for _ in 0..200 {
        if clicks(pool).await == n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("clicks never reached {n}, now {}", clicks(pool).await);
}

fn spool_end(dir: &Path) -> u64 {
    bf_spool::replay(dir, 0)
        .unwrap()
        .map(|r| r.end_offset())
        .last()
        .unwrap_or(0)
}

/// State with a queue nobody drains: records get spooled but never reach Postgres.
async fn undrained(pool: &PgPool, dir: &Path, cap: usize) -> (AppState, mpsc::Receiver<Queued>) {
    let (queue, rx) = mpsc::channel(cap);
    let state = AppState {
        pool: pool.clone(),
        snapshot: Arc::new(ArcSwap::from_pointee(Snapshot::load(pool).await.unwrap())),
        spool: SpoolWriter::open(SpoolConfig::new(dir)).await.unwrap(),
        queue,
        ip_salt: SALT.into(),
    };
    (state, rx)
}

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    code: String,
    country: Option<String>,
    device: Option<String>,
    os: Option<String>,
    browser: Option<String>,
    referrer_host: Option<String>,
    landing_path: Option<String>,
    ip_hash: Option<Vec<u8>>,
}

#[sqlx::test(migrations = "../db/migrations")]
async fn thousand_clicks_thousand_rows(pool: PgPool) {
    seed(&pool, "abc").await;
    let dir = spool_dir();
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT)
        .await
        .unwrap();
    let app = router(t.state.clone());

    fire(&app, 1000).await;
    wait_clicks(&pool, 1000).await;

    // Checkpoint catches up to the whole spool, despite out-of-order handler completion.
    let end = spool_end(&dir);
    for _ in 0..100 {
        if t.state.spool.read_checkpoint().unwrap() == end {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(t.state.spool.read_checkpoint().unwrap(), end);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(clicks(&pool).await, 1000);
    std::fs::remove_dir_all(&dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn click_fields_are_normalized(pool: PgPool) {
    seed(&pool, "abc").await;
    let dir = spool_dir();
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT)
        .await
        .unwrap();
    let app = router(t.state.clone());

    let res = app.oneshot(click_req(0)).await.unwrap();
    assert_eq!(res.status(), StatusCode::FOUND);
    let loc = res.headers()["location"].to_str().unwrap().to_owned();
    let id = loc
        .strip_prefix("https://example.com/pricing?x=1&bf_click=")
        .unwrap();
    wait_clicks(&pool, 1).await;

    let row: Row = sqlx::query_as(
        "SELECT id, code, country::text AS country, device, os, browser, referrer_host, \
                landing_path, ip_hash FROM clicks",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.id.to_string(), id);
    assert_eq!(row.id.get_version_num(), 7);
    assert_eq!(row.code, "abc");
    assert_eq!(row.country.as_deref(), Some("DE"));
    assert_eq!(row.device.as_deref(), Some("pc"));
    assert_eq!(row.os.as_deref(), Some("Windows 10"));
    assert_eq!(row.browser.as_deref(), Some("Chrome"));
    assert_eq!(row.referrer_host.as_deref(), Some("blog.example.org"));
    assert_eq!(row.landing_path.as_deref(), Some("/pricing"));
    assert_eq!(
        row.ip_hash.unwrap(),
        tracker::event::ip_hash(SALT, "203.0.113.9")
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn crash_before_flush_replays_exactly_once(pool: PgPool) {
    seed(&pool, "abc").await;
    let dir = spool_dir();

    // Run 1: 1000 clicks are acked and spooled, then the process dies before any DB flush.
    {
        let (state, _rx) = undrained(&pool, &dir, 2000).await;
        fire(&router(state), 1000).await;
        assert_eq!(clicks(&pool).await, 0);
    }
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Run 2: startup replays the spool.
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT)
        .await
        .unwrap();
    assert_eq!(clicks(&pool).await, 1000);
    assert_eq!(t.state.spool.read_checkpoint().unwrap(), spool_end(&dir));
    drop(t);

    // Crash after the DB flush but before the checkpoint moved: replay again, no dupes.
    bf_spool::write_checkpoint(&dir, 0).unwrap();
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT)
        .await
        .unwrap();
    assert_eq!(clicks(&pool).await, 1000);
    assert_eq!(t.state.spool.read_checkpoint().unwrap(), spool_end(&dir));
    std::fs::remove_dir_all(&dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn unknown_code_is_404(pool: PgPool) {
    seed(&pool, "abc").await;
    let dir = spool_dir();
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT)
        .await
        .unwrap();
    let app = router(t.state.clone());

    let res = app
        .clone()
        .oneshot(Request::get("/r/nope").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let res = app
        .clone()
        .oneshot(beacon(r#"{"code":"nope","vid":"v"}"#))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let res = app.oneshot(beacon("not json")).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(clicks(&pool).await, 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn beacon_click_field_is_accepted(pool: PgPool) {
    seed(&pool, "abc").await;
    let dir = spool_dir();
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT)
        .await
        .unwrap();
    let app = router(t.state.clone());
    let send = |body: &'static str| {
        let app = app.clone();
        async move { app.oneshot(beacon(body)).await.unwrap().status() }
    };

    // `/r` already recorded a click-only visit: acknowledged, not recorded again.
    let click = r#"{"code":null,"click":"0190c1d2-0000-7000-8000-000000000000","vid":"v"}"#;
    assert_eq!(send(click).await, StatusCode::NO_CONTENT);
    // A referral code records a click, with or without a click id beside it.
    let both = r#"{"code":"abc","click":"c1","vid":"v","url":"https://example.com/","ref":null}"#;
    assert_eq!(send(both).await, StatusCode::NO_CONTENT);
    let code = r#"{"code":"abc","click":null,"vid":"v"}"#;
    assert_eq!(send(code).await, StatusCode::NO_CONTENT);
    wait_clicks(&pool, 2).await;
    // Neither code nor click, or an unknown code, is rejected.
    assert_eq!(send(r#"{"vid":"v"}"#).await, StatusCode::BAD_REQUEST);
    assert_eq!(
        send(r#"{"code":"nope","click":"c1"}"#).await,
        StatusCode::NOT_FOUND
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(clicks(&pool).await, 2);
    std::fs::remove_dir_all(&dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn full_queue_sheds_load_with_503(pool: PgPool) {
    seed(&pool, "abc").await;
    let dir = spool_dir();
    let (state, _rx) = undrained(&pool, &dir, 1).await;
    let app = router(state);

    let first = app.clone().oneshot(click_req(0)).await.unwrap();
    assert_eq!(first.status(), StatusCode::FOUND);
    let second = app.clone().oneshot(click_req(0)).await.unwrap();
    assert_eq!(second.status(), StatusCode::SERVICE_UNAVAILABLE);
    let third = app.oneshot(click_req(1)).await.unwrap();
    assert_eq!(third.status(), StatusCode::SERVICE_UNAVAILABLE);
    // The shed requests never reached the spool.
    assert_eq!(bf_spool::replay(&dir, 0).unwrap().count(), 1);
    std::fs::remove_dir_all(&dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn snapshot_reloads_on_notify(pool: PgPool) {
    seed(&pool, "abc").await;
    let dir = spool_dir();
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT)
        .await
        .unwrap();
    let app = router(t.state.clone());
    let get = |code: &str| {
        Request::get(format!("/r/{code}"))
            .body(Body::empty())
            .unwrap()
    };

    assert_eq!(
        app.clone().oneshot(get("fresh")).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    add_code(&pool, "fresh").await;
    sqlx::query("SELECT pg_notify('bf_config', '')")
        .execute(&pool)
        .await
        .unwrap();
    for _ in 0..100 {
        if app.clone().oneshot(get("fresh")).await.unwrap().status() == StatusCode::FOUND {
            std::fs::remove_dir_all(&dir).ok();
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("snapshot never picked up the new code");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn static_endpoints(pool: PgPool) {
    let dir = spool_dir();
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT)
        .await
        .unwrap();
    let app = router(t.state.clone());

    let res = app
        .clone()
        .oneshot(Request::get("/t.js").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(
        res.headers()["cache-control"]
            .to_str()
            .unwrap()
            .contains("max-age")
    );
    let etag = res.headers()["etag"].to_str().unwrap().to_owned();
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], include_bytes!("../js/tracker.min.js"));
    let res = app
        .clone()
        .oneshot(
            Request::get("/t.js")
                .header("if-none-match", etag)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);

    let pre = Request::options("/c")
        .header("origin", "https://shop.example")
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", "content-type")
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(pre).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert_eq!(res.headers()["access-control-allow-origin"], "*");
    assert_eq!(
        res.headers()["access-control-allow-headers"],
        "content-type"
    );

    for (path, want) in [("/healthz", StatusCode::OK), ("/readyz", StatusCode::OK)] {
        let res = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), want, "{path}");
    }
    std::fs::remove_dir_all(&dir).ok();
}
