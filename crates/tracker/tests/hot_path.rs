use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use bf_spool::{SpoolConfig, SpoolWriter};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use sqlx::PgPool;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tower::ServiceExt;
use tracker::geo::Geo;
use tracker::proxy::Trust;
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
            .header("cf-connecting-ip", "203.0.113.9")
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
        trust: Default::default(),
        geo: Arc::new(Geo::disabled()),
        fraud: bf_core::default_rules().into(),
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
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT, Geo::disabled())
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
    let mut t = start(pool.clone(), SpoolConfig::new(&dir), SALT, Geo::disabled())
        .await
        .unwrap();
    t.state.trust = Arc::new(Trust {
        cf: true,
        proxies: vec![],
    });
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
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT, Geo::disabled())
        .await
        .unwrap();
    assert_eq!(clicks(&pool).await, 1000);
    assert_eq!(t.state.spool.read_checkpoint().unwrap(), spool_end(&dir));
    drop(t);

    // Crash after the DB flush but before the checkpoint moved: replay again, no dupes.
    bf_spool::write_checkpoint(&dir, 0).unwrap();
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT, Geo::disabled())
        .await
        .unwrap();
    assert_eq!(clicks(&pool).await, 1000);
    assert_eq!(t.state.spool.read_checkpoint().unwrap(), spool_end(&dir));
    std::fs::remove_dir_all(&dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn nul_bytes_and_null_landing_do_not_wedge_pipeline(pool: PgPool) {
    seed(&pool, "abc").await;
    sqlx::query(
        "INSERT INTO referral_codes (code, affiliate_id, program_id) \
         SELECT 'nolanding', a.id, p.id FROM affiliates a, programs p",
    )
    .execute(&pool)
    .await
    .unwrap();
    let dir = spool_dir();
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT, Geo::disabled())
        .await
        .unwrap();
    let app = router(t.state.clone());

    let body = r#"{"code":"abc","vid":"v\u0000x","url":"https://a.com/p\u0000q","ref":"https://h\u0000.io"}"#;
    assert_eq!(
        app.clone().oneshot(beacon(body)).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.clone().oneshot(click_req(0)).await.unwrap().status(),
        StatusCode::FOUND
    );
    wait_clicks(&pool, 2).await;
    let nolanding = Request::get("/r/nolanding").body(Body::empty()).unwrap();
    assert_eq!(
        app.oneshot(nolanding).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn unknown_code_is_404(pool: PgPool) {
    seed(&pool, "abc").await;
    let dir = spool_dir();
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT, Geo::disabled())
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
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT, Geo::disabled())
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
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT, Geo::disabled())
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
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT, Geo::disabled())
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

#[sqlx::test(migrations = "../db/migrations")]
async fn metrics_after_click_has_spool_lag(pool: PgPool) {
    seed(&pool, "abc").await;
    let dir = spool_dir();
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT, Geo::disabled())
        .await
        .unwrap();
    let app = router(t.state.clone());

    fire(&app, 1).await;
    wait_clicks(&pool, 1).await;

    let out = tracker::metrics::render();
    for name in [
        "bf_spool_lag",
        "bf_spool_bytes",
        "bf_batch_size_count",
        "bf_shed_total",
    ] {
        assert!(out.contains(name), "missing {name}");
    }
    // The click went through the spool: latency was observed and bytes were appended.
    assert!(tracker::metrics::INGEST.count() >= 1);
    assert!(tracker::metrics::SPOOL_BYTES.get() > 0);
    std::fs::remove_dir_all(&dir).ok();
}

fn click_from(ip: &str, ua: &str) -> Request<Body> {
    Request::get("/r/abc")
        .header("user-agent", ua)
        .header("cf-connecting-ip", ip)
        .body(Body::empty())
        .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn fraud_flagged_clicks_are_stored_with_reason(pool: PgPool) {
    const BOT: &str = "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)";
    seed(&pool, "abc").await;
    let dir = spool_dir();
    let mut t = start(pool.clone(), SpoolConfig::new(&dir), SALT, Geo::disabled())
        .await
        .unwrap();
    t.state.trust = Arc::new(Trust {
        cf: true,
        proxies: vec![],
    });
    let app = router(t.state.clone());

    // Flagged clicks redirect like any other: nothing is dropped.
    for (ip, ua) in [
        ("198.51.100.1", BOT),
        ("198.51.100.2", CHROME),
        ("198.51.100.2", CHROME),
        ("198.51.100.3", CHROME),
    ] {
        let res = app.clone().oneshot(click_from(ip, ua)).await.unwrap();
        assert_eq!(res.status(), StatusCode::FOUND, "{ip}");
    }
    wait_clicks(&pool, 4).await;

    let reason = |ip: &'static str| {
        let pool = pool.clone();
        async move {
            let rows: Vec<Option<String>> = sqlx::query_scalar(
                "SELECT fraud_reason FROM clicks WHERE ip_hash = $1 ORDER BY id",
            )
            .bind(tracker::event::ip_hash(SALT, ip))
            .fetch_all(&pool)
            .await
            .unwrap();
            rows
        }
    };
    assert_eq!(reason("198.51.100.1").await, [Some("bot_ua".to_owned())]);
    assert_eq!(
        reason("198.51.100.2").await,
        [None, Some("duplicate".to_owned())]
    );
    assert_eq!(reason("198.51.100.3").await, [None]);
    std::fs::remove_dir_all(&dir).ok();
}

fn spoofed_click(i: usize) -> Request<Body> {
    Request::get("/r/abc")
        .header("user-agent", CHROME)
        .header(
            "cf-connecting-ip",
            format!("203.0.{}.{}", i / 250, i % 250 + 1),
        )
        .header("cf-ipcountry", "XX")
        .header("x-forwarded-for", format!("198.51.100.{}", i + 1))
        .body(Body::empty())
        .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn client_supplied_ip_headers_are_ignored_by_default(pool: PgPool) {
    seed(&pool, "abc").await;
    let dir = spool_dir();
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT, Geo::disabled())
        .await
        .unwrap();
    let app = router(t.state.clone());

    // A fresh forged address on every click would sidestep the IP rules if it were believed.
    for i in 0..6 {
        let mut req = spoofed_click(i);
        req.extensions_mut().insert(axum::extract::ConnectInfo(
            "192.0.2.50:4000".parse::<std::net::SocketAddr>().unwrap(),
        ));
        assert_eq!(
            app.clone().oneshot(req).await.unwrap().status(),
            StatusCode::FOUND
        );
    }
    wait_clicks(&pool, 6).await;
    let (hashes, countries): (i64, i64) =
        sqlx::query_as("SELECT count(DISTINCT ip_hash), count(country) FROM clicks")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(hashes, 1, "every click is attributed to the real peer");
    assert_eq!(countries, 0, "CF-IPCountry is not believed either");
    let reasons: Vec<Option<String>> =
        sqlx::query_scalar("SELECT fraud_reason FROM clicks ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(
        reasons.iter().any(Option::is_some),
        "repeat clicks from one peer are flagged"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn forwarded_for_is_believed_only_from_a_trusted_proxy(pool: PgPool) {
    seed(&pool, "abc").await;
    let dir = spool_dir();
    let mut t = start(pool.clone(), SpoolConfig::new(&dir), SALT, Geo::disabled())
        .await
        .unwrap();
    t.state.trust = Arc::new(
        Trust::from_lookup(|k| (k == "BF_TRUSTED_PROXIES").then(|| "172.16.0.0/12".to_string()))
            .unwrap(),
    );
    let app = router(t.state.clone());
    let from = |peer: &str| {
        let mut req = Request::get("/r/abc")
            .header("user-agent", CHROME)
            .header("x-forwarded-for", "6.6.6.6, 203.0.113.77")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(axum::extract::ConnectInfo(
            format!("{peer}:4000")
                .parse::<std::net::SocketAddr>()
                .unwrap(),
        ));
        req
    };
    // Via the proxy: the address the proxy appended. Direct: the peer itself.
    app.clone().oneshot(from("172.18.0.2")).await.unwrap();
    app.clone().oneshot(from("198.51.100.9")).await.unwrap();
    wait_clicks(&pool, 2).await;
    let hashes: Vec<Vec<u8>> = sqlx::query_scalar("SELECT ip_hash FROM clicks ORDER BY id")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(hashes[0], tracker::event::ip_hash(SALT, "203.0.113.77"));
    assert_eq!(hashes[1], tracker::event::ip_hash(SALT, "198.51.100.9"));
    std::fs::remove_dir_all(&dir).ok();
}

fn signed_event(i: usize) -> Request<Body> {
    // Signed with the webhook secret `seed` gives the program ('s').
    let body = format!(r#"{{"customer":"c{i}","amount_cents":100}}"#);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string();
    let key = format!("k{i}");
    let mut mac = Hmac::<Sha256>::new_from_slice(b"s").unwrap();
    for part in [ts.as_bytes(), b".", key.as_bytes(), b".", body.as_bytes()] {
        mac.update(part);
    }
    let sig: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Request::post("/v1/events/pk")
        .header("content-type", "application/json")
        .header("idempotency-key", key)
        .header("x-bf-timestamp", ts)
        .header("x-bf-signature", sig)
        .body(Body::from(body))
        .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn click_flood_cannot_take_the_reserved_event_slots(pool: PgPool) {
    seed(&pool, "abc").await;
    let dir = spool_dir();
    let (state, _rx) = undrained(&pool, &dir, 20).await; // 2 of 20 slots are reserved
    let app = router(state);

    let mut accepted = 0;
    for _ in 0..30 {
        if app.clone().oneshot(click_req(0)).await.unwrap().status() == StatusCode::FOUND {
            accepted += 1;
        }
    }
    assert_eq!(accepted, 18, "clicks stop at the reserve");
    for i in 0..2 {
        let res = app.clone().oneshot(signed_event(i)).await.unwrap();
        assert_eq!(
            res.status(),
            StatusCode::ACCEPTED,
            "signed event {i} still gets in"
        );
    }
    let third = app.clone().oneshot(signed_event(2)).await.unwrap();
    assert_eq!(
        third.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "the queue is now full"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// A click for a program that does not exist: Postgres rejects it for good (foreign key).
fn orphan_click() -> tracker::writer::Record {
    tracker::writer::Record::Click(bf_db::tracker::Click {
        id: Uuid::now_v7(),
        program_id: Uuid::now_v7(),
        affiliate_id: Uuid::now_v7(),
        code: "ghost".into(),
        visitor_id: "v".into(),
        ts_ms: 1_700_000_000_000,
        ip_hash: None,
        country: None,
        device: None,
        os: None,
        browser: None,
        referrer_host: None,
        landing_path: None,
        fraud_reason: None,
    })
}

async fn spool_record(spool: &SpoolWriter, record: tracker::writer::Record) -> Queued {
    let payload = serde_json::to_vec(&record).unwrap();
    let end = spool.write(&payload).await.unwrap();
    Queued {
        start: end - bf_spool::HEADER_LEN as u64 - payload.len() as u64,
        end,
        record,
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn a_record_postgres_rejects_is_dead_lettered_not_retried_forever(pool: PgPool) {
    seed(&pool, "abc").await;
    let dir = spool_dir();
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT, Geo::disabled())
        .await
        .unwrap();
    let before = tracker::metrics::DEAD_LETTERED.get();

    // The orphan and a good click land in one batch; the orphan must not take the click down.
    let bad = spool_record(&t.state.spool, orphan_click()).await;
    t.state.queue.send(bad).await.unwrap();
    let app = router(t.state.clone());
    assert_eq!(
        app.oneshot(click_req(0)).await.unwrap().status(),
        StatusCode::FOUND
    );
    wait_clicks(&pool, 1).await;

    // The checkpoint moves past both, so nothing is replayed (and rejected again) at restart.
    let end = spool_end(&dir);
    for _ in 0..100 {
        if t.state.spool.read_checkpoint().unwrap() == end {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(t.state.spool.read_checkpoint().unwrap(), end);
    assert!(tracker::metrics::DEAD_LETTERED.get() > before);
    std::fs::remove_dir_all(&dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn startup_replay_skips_a_record_postgres_rejects(pool: PgPool) {
    seed(&pool, "abc").await;
    let dir = spool_dir();
    {
        // A previous run spooled an orphan click and then a good one, and died before flushing.
        let spool = SpoolWriter::open(SpoolConfig::new(&dir)).await.unwrap();
        spool_record(&spool, orphan_click()).await;
        let snap = Snapshot::load(&pool).await.unwrap();
        let entry = &snap.codes["abc"];
        let good = tracker::writer::Record::Click(bf_db::tracker::Click {
            program_id: entry.program_id,
            affiliate_id: entry.affiliate_id,
            code: "abc".into(),
            ..match orphan_click() {
                tracker::writer::Record::Click(c) => c,
                _ => unreachable!(),
            }
        });
        spool_record(&spool, good).await;
    }
    // Before the fix this returned Err and the tracker crash-looped on the same record.
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT, Geo::disabled())
        .await
        .expect("replay survives the bad record");
    assert_eq!(clicks(&pool).await, 1);
    assert_eq!(t.state.spool.read_checkpoint().unwrap(), spool_end(&dir));
    std::fs::remove_dir_all(&dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn a_burst_of_config_changes_converges_on_the_new_snapshot(pool: PgPool) {
    seed(&pool, "abc").await;
    let dir = spool_dir();
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT, Geo::disabled())
        .await
        .unwrap();
    let first = t.state.snapshot.load_full();
    add_code(&pool, "burst").await;
    for _ in 0..50 {
        sqlx::query("SELECT pg_notify('bf_config', '')")
            .execute(&pool)
            .await
            .unwrap();
    }
    for _ in 0..100 {
        if t.state.snapshot.load().codes.contains_key("burst") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(t.state.snapshot.load().codes.contains_key("burst"));
    // 50 notifications inside the debounce window swap the snapshot a handful of times at
    // most (it was a reload each before), and the final one holds the new code.
    let swapped_once = !Arc::ptr_eq(&first, &t.state.snapshot.load_full());
    assert!(swapped_once);
    std::fs::remove_dir_all(&dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn a_lone_config_change_is_live_without_waiting(pool: PgPool) {
    seed(&pool, "abc").await;
    let dir = spool_dir();
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT, Geo::disabled())
        .await
        .unwrap();
    // Let the quiet period pass so the change is the first of a new burst.
    tokio::time::sleep(Duration::from_millis(400)).await;
    add_code(&pool, "instant").await;
    sqlx::query("SELECT pg_notify('bf_config', '')")
        .execute(&pool)
        .await
        .unwrap();
    let began = std::time::Instant::now();
    while !t.state.snapshot.load().codes.contains_key("instant") {
        assert!(began.elapsed() < Duration::from_secs(2), "never reloaded");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        began.elapsed() < Duration::from_millis(200),
        "a code made in the UI must work at once, took {:?}",
        began.elapsed()
    );
    std::fs::remove_dir_all(&dir).ok();
}
