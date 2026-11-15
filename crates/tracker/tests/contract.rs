//! Contract tests: the v2 JSON Schemas, the Rust validators, the router and the OpenAPI
//! document must agree. Schema verdicts come from `scripts/schema-verdicts.mjs` (ajv) via
//! `node`; those tests skip with a message when node or `scripts/schemas/node_modules` is
//! missing (`make schemas-check` installs it). The `jsonschema` crate was not used so the
//! dependency tree (and `cargo deny`) stays unchanged.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, Response, StatusCode};
use bf_spool::SpoolConfig;
use hmac::{Hmac, Mac};
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::{Config, TestRunner};
use serde_json::{Value, json};
use sha2::Sha256;
use sqlx::PgPool;
use tower::ServiceExt;
use tracker::geo::Geo;
use tracker::snapshot::Snapshot;
use tracker::{router, start, v2};
use uuid::Uuid;

const SECRET: &str = "generic-secret";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn examples() -> Vec<(String, String)> {
    let mut out: Vec<_> = std::fs::read_dir(root().join("schemas/tracker/v2/examples"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read_to_string(&p).unwrap(),
            )
        })
        .filter(|(n, _)| n != "signature-vector.json")
        .collect();
    out.sort();
    out
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn sig(secret: &str, t: i64, key: &str, body: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    for p in [
        t.to_string().as_bytes(),
        b".",
        key.as_bytes(),
        b".",
        body.as_bytes(),
    ] {
        mac.update(p);
    }
    let hex: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("t={t},v1={hex}")
}

fn event_req(key: &str, body: &str) -> Request<Body> {
    Request::post("/v2/events/pk")
        .header("content-type", "application/json")
        .header("idempotency-key", key)
        .header("bf-signature", sig(SECRET, now(), key, body))
        .body(Body::from(body.to_owned()))
        .unwrap()
}

fn beacon_req(body: &str) -> Request<Body> {
    Request::post("/v2/c")
        .header("content-type", "text/plain")
        .body(Body::from(body.to_owned()))
        .unwrap()
}

async fn app(pool: &PgPool) -> (Router, tracker::AppState, PathBuf) {
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
    q("INSERT INTO users (id, workspace_id, email, password_hash, role) VALUES ($1, $2, 'a@x.io', 'h', 'affiliate')")
        .bind(user).bind(ws).execute(pool).await.unwrap();
    q("INSERT INTO affiliates (id, workspace_id, user_id) VALUES ($1, $2, $3)")
        .bind(aff)
        .bind(ws)
        .bind(user)
        .execute(pool)
        .await
        .unwrap();
    q(
        "INSERT INTO programs (id, workspace_id, name, currency, public_key, webhook_secret) \
       VALUES ($1, $2, 'p', 'USD', 'pk', $3)",
    )
    .bind(prog)
    .bind(ws)
    .bind(SECRET)
    .execute(pool)
    .await
    .unwrap();
    // `AFF123` is the code the beacon examples use.
    q(
        "INSERT INTO referral_codes (code, affiliate_id, program_id, landing_url) \
       VALUES ('AFF123', $1, $2, 'https://example.com/pricing')",
    )
    .bind(aff)
    .bind(prog)
    .execute(pool)
    .await
    .unwrap();
    let dir = std::env::temp_dir().join(format!("bf-contract-{}", Uuid::now_v7()));
    let t = start(
        pool.clone(),
        SpoolConfig::new(&dir),
        "salt",
        Geo::disabled(),
        Arc::new(bf_core::secrets::Vault::plaintext()),
    )
    .await
    .unwrap();
    (router(t.state.clone()), t.state, dir)
}

async fn send(app: &Router, req: Request<Body>) -> Response<Body> {
    app.clone().oneshot(req).await.unwrap()
}

async fn body_of(res: Response<Body>) -> String {
    String::from_utf8(to_bytes(res.into_body(), 1 << 20).await.unwrap().to_vec()).unwrap()
}

/// Schema verdicts from ajv, or `None` (with a message) when node/ajv is unavailable.
fn verdicts(schema: &str, bodies: &[String]) -> Option<Vec<bool>> {
    if !root().join("scripts/schemas/node_modules/ajv").exists() {
        eprintln!("SKIP: run `npm --prefix scripts/schemas install` (ajv missing)");
        return None;
    }
    let child = Command::new("node")
        .arg(root().join("scripts/schema-verdicts.mjs"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            eprintln!("SKIP: node not available ({e})");
            return None;
        }
    };
    let req = json!({"schema": schema, "bodies": bodies}).to_string();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(req.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "schema-verdicts failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(serde_json::from_slice(&out.stdout).unwrap())
}

// ------------------------------------------------------------------ examples vs server

#[sqlx::test(migrations = "../db/migrations")]
async fn every_valid_example_accepted_by_server(pool: PgPool) {
    let (app, _, dir) = app(&pool).await;
    let mut n = 0;
    for (name, body) in examples()
        .into_iter()
        .filter(|(n, _)| n.ends_with(".valid.json"))
    {
        let res = if name.starts_with("event.") {
            // Unique idempotency key per example; the body carries its own external id.
            let r = send(&app, event_req(&format!("ex-{n}"), &body)).await;
            assert_eq!(r.status(), StatusCode::ACCEPTED, "{name}");
            r
        } else if name.starts_with("click-beacon.") {
            let r = send(&app, beacon_req(&body)).await;
            assert_eq!(r.status(), StatusCode::NO_CONTENT, "{name}");
            r
        } else {
            continue;
        };
        drop(res);
        n += 1;
    }
    assert!(n >= 7, "only {n} valid examples exercised");
    std::fs::remove_dir_all(dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn every_invalid_example_rejected_by_server(pool: PgPool) {
    let (app, _, dir) = app(&pool).await;
    let mut n = 0;
    for (name, body) in examples()
        .into_iter()
        .filter(|(n, _)| n.ends_with(".invalid.json"))
    {
        let req = if name.starts_with("event.") {
            event_req(&format!("bad-{n}"), &body)
        } else if name.starts_with("click-beacon.") {
            beacon_req(&body)
        } else {
            continue;
        };
        let res = send(&app, req).await;
        let status = res.status();
        // 413 is allowed for an oversize body; none of the checked-in examples are that big.
        assert!(
            status == StatusCode::BAD_REQUEST || status == StatusCode::PAYLOAD_TOO_LARGE,
            "{name}: {status}"
        );
        n += 1;
    }
    assert!(n >= 25, "only {n} invalid examples exercised");
    std::fs::remove_dir_all(dir).ok();
}

// ------------------------------------------------------------- schema vs Rust validators

#[test]
fn schema_and_validators_agree_on_examples() {
    for (prefix, schema) in [("event.", "event"), ("click-beacon.", "click-beacon")] {
        let all: Vec<_> = examples()
            .into_iter()
            .filter(|(n, _)| n.starts_with(prefix))
            .collect();
        let bodies: Vec<String> = all.iter().map(|(_, b)| b.clone()).collect();
        let Some(v) = verdicts(schema, &bodies) else {
            return;
        };
        for ((name, body), schema_ok) in all.iter().zip(v) {
            let rust_ok = if schema == "event" {
                v2::parse_event(body.as_bytes()).is_ok()
            } else {
                v2::parse_beacon(body.as_bytes()).is_ok()
            };
            assert_eq!(schema_ok, rust_ok, "{name}");
        }
    }
}

fn event_strategy() -> impl Strategy<Value = String> {
    let ty = prop::sample::select(vec![
        "sale",
        "sale",
        "sale",
        "signup",
        "signup",
        "lead",
        "refund",
        "refund",
        "chargeback",
        "custom:x_1",
        "custom:",
        "Sale",
        "bogus",
    ]);
    let ext = prop::sample::select(vec![
        r#""o1""#,
        r#""o1""#,
        r#""o1""#,
        r#""o1""#,
        r#""o1""#,
        r#""""#,
        r#""a\u0000b""#,
        "7",
        "null",
    ]);
    let ext_long = prop::bool::weighted(0.05);
    let amount = prop::sample::select(vec![
        None,
        Some("4900"),
        Some("4900"),
        Some("4900"),
        Some("4900"),
        Some("0"),
        Some("-1"),
        Some("49.0"),
        Some("4.9e3"),
        Some(r#""4900""#),
    ]);
    let currency = prop::sample::select(vec![
        None,
        Some("USD"),
        Some("USD"),
        Some("USD"),
        Some("USD"),
        Some("usd"),
        Some("US"),
        Some("EURO"),
    ]);
    let at = prop::sample::select(vec![
        "2026-01-01T12:00:00Z",
        "2026-01-01T12:00:00Z",
        "2026-01-01T12:00:00Z",
        "2026-01-01T12:00:00.5+05:30",
        "2026-01-01",
        "2026-01-01T12:00:00",
        "yesterday",
    ]);
    let refx = prop::sample::select(vec![
        None,
        Some(r#""o0""#),
        Some(r#""o0""#),
        Some(r#""o0""#),
        Some(r#""""#),
    ]);
    let extra = prop::bool::weighted(0.1);
    let meta = prop::sample::select(vec![
        None,
        None,
        None,
        Some(r#"{"a":1,"b":"x","c":true}"#),
        Some(r#"{"a":{"b":1}}"#),
        Some(r#"{"a":null}"#),
        Some(r#"{"a":[1]}"#),
    ]);
    let customer = prop::sample::select(vec![
        None,
        None,
        None,
        Some(r#"{"external_id":"c1"}"#),
        Some(&*Box::leak(
            format!(r#"{{"email_sha256":"{}"}}"#, "a".repeat(64)).into_boxed_str(),
        )),
        Some(r#"{"email_sha256":"ABC"}"#),
        Some(r#"{"raw_email":"a@b.c"}"#),
    ]);
    (
        (ty, ext, ext_long, amount, currency),
        (at, refx, extra, meta, customer),
    )
        .prop_map(
            |((ty, ext, long, amount, cur), (at, refx, extra, meta, cust))| {
                let ext = if long {
                    format!("\"{}\"", "x".repeat(129))
                } else {
                    ext.to_owned()
                };
                let mut f = vec![
                    r#""schema":"bf.event.v2""#.to_owned(),
                    format!(r#""type":"{ty}""#),
                    format!(r#""external_id":{ext}"#),
                    format!(r#""occurred_at":"{at}""#),
                ];
                if let Some(a) = amount {
                    f.push(format!(r#""amount_cents":{a}"#));
                }
                if let Some(c) = cur {
                    f.push(format!(r#""currency":"{c}""#));
                }
                if let Some(r) = refx {
                    f.push(format!(r#""ref_external_id":{r}"#));
                }
                if let Some(m) = meta {
                    f.push(format!(r#""meta":{m}"#));
                }
                if let Some(c) = cust {
                    f.push(format!(r#""customer":{c}"#));
                }
                if extra {
                    f.push(r#""surprise":1"#.to_owned());
                }
                format!("{{{}}}", f.join(","))
            },
        )
}

/// Schema-valid iff the server's validator accepts, over generated events.
#[test]
fn server_and_schema_agree_property() {
    let mut runner = TestRunner::new(Config::default());
    let strat = event_strategy();
    let bodies: Vec<String> = (0..600)
        .map(|_| strat.new_tree(&mut runner).unwrap().current())
        .collect();
    let Some(v) = verdicts("event", &bodies) else {
        return;
    };
    let (mut accepted, mut rejected) = (0, 0);
    for (body, schema_ok) in bodies.iter().zip(v) {
        let rust_ok = v2::parse_event(body.as_bytes()).is_ok();
        assert_eq!(schema_ok, rust_ok, "disagreement on {body}");
        if rust_ok {
            accepted += 1;
        } else {
            rejected += 1;
        }
    }
    assert!(accepted > 5 && rejected > 5, "{accepted}/{rejected}");
}

// ------------------------------------------------------------------- OpenAPI vs router

fn spec() -> Value {
    serde_json::from_str(&tracker::openapi::json()).unwrap()
}

fn spec_v2_routes() -> BTreeSet<(String, String)> {
    spec()["paths"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(p, _)| p.starts_with("/v2/"))
        .flat_map(|(p, ops)| {
            ops.as_object()
                .unwrap()
                .keys()
                .map(move |m| (m.clone(), p.clone()))
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn openapi_paths_match_router() {
    let table: BTreeSet<(String, String)> = tracker::openapi::V2_ROUTES
        .iter()
        .map(|(m, p)| (m.to_string(), p.to_string()))
        .collect();
    assert_eq!(spec_v2_routes(), table, "spec vs V2_ROUTES");

    // Every `/v2/...` route literal in lib.rs is in the table, and the other way round.
    let src = std::fs::read_to_string(root().join("crates/tracker/src/lib.rs")).unwrap();
    let in_src: BTreeSet<String> = src
        .split('"')
        .filter(|s| s.starts_with("/v2/") && !s.contains(' '))
        .map(str::to_owned)
        .collect();
    let in_table: BTreeSet<String> = table.iter().map(|(_, p)| p.clone()).collect();
    assert_eq!(in_src, in_table, "route literals in lib.rs vs V2_ROUTES");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn openapi_routes_are_served_with_the_documented_method(pool: PgPool) {
    let (app, _, dir) = app(&pool).await;
    for (m, p) in spec_v2_routes() {
        let path = p.replace("{key}", "nope");
        let req = Request::builder()
            .method(m.to_uppercase().as_str())
            .uri(&path)
            .body(Body::empty())
            .unwrap();
        let res = send(&app, req).await;
        assert_ne!(res.status(), StatusCode::METHOD_NOT_ALLOWED, "{m} {p}");
        if res.status() == StatusCode::NOT_FOUND {
            // A real handler answers 404 with the error schema; a missing route is empty.
            assert!(
                body_of(res).await.contains("\"error\""),
                "{m} {p} not routed"
            );
        }
    }
    // A method the spec does not list is refused.
    let res = send(&app, Request::get("/v2/c").body(Body::empty()).unwrap()).await;
    assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);
    std::fs::remove_dir_all(dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn openapi_endpoint_serves_the_document(pool: PgPool) {
    let (app, _, dir) = app(&pool).await;
    let res = send(
        &app,
        Request::get("/v2/openapi.json")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let served: Value = serde_json::from_str(&body_of(res).await).unwrap();
    assert_eq!(served, spec());
    assert_eq!(served["openapi"], "3.1.0");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn openapi_snapshot_not_drifted() {
    let path = root().join("docs/openapi/tracker-v2.json");
    let snap = std::fs::read_to_string(&path).expect("docs/openapi/tracker-v2.json");
    assert_eq!(
        snap,
        format!("{}\n", tracker::openapi::json()),
        "drift: cargo run -q -p tracker -- openapi > docs/openapi/tracker-v2.json"
    );
}

#[test]
fn openapi_documents_every_error_status() {
    let doc = spec();
    for p in ["/v2/events/{key}", "/v2/c"] {
        let op = doc["paths"][p]["post"]["responses"].as_object().unwrap();
        for s in ["400", "404", "413", "503"] {
            assert!(op.contains_key(s), "{p} missing {s}");
        }
    }
    let ev = doc["paths"]["/v2/events/{key}"]["post"]["responses"]
        .as_object()
        .unwrap();
    assert!(ev.contains_key("401") && ev.contains_key("409"));
}

// ------------------------------------------------------------------- signature, errors

#[test]
fn signature_vector_matches_js() {
    let path = root().join("schemas/tracker/v2/examples/signature-vector.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let got = sig(
        v["secret"].as_str().unwrap(),
        v["t"].as_i64().unwrap(),
        v["idempotency_key"].as_str().unwrap(),
        v["body"].as_str().unwrap(),
    );
    // The JS SDK checks the same file (sdks/js/test/signing.test.ts).
    assert_eq!(got, v["header"].as_str().unwrap());
}

/// 400/401/404/409/413/503 bodies all match `error.schema.json`.
#[sqlx::test(migrations = "../db/migrations")]
async fn error_body_matches_error_schema(pool: PgPool) {
    let (app, state, dir) = app(&pool).await;
    let sale = r#"{"schema":"bf.event.v2","type":"sale","external_id":"o1","occurred_at":"2026-01-01T12:00:00Z","amount_cents":4900,"currency":"USD"}"#;
    let mut got: Vec<(StatusCode, String)> = Vec::new();
    let take = |status: StatusCode, res: Response<Body>| {
        assert_eq!(res.status(), status);
        res
    };
    // 400
    got.push((
        StatusCode::BAD_REQUEST,
        body_of(take(
            StatusCode::BAD_REQUEST,
            send(&app, event_req("k1", "{}")).await,
        ))
        .await,
    ));
    // 401
    let unsigned = Request::post("/v2/events/pk")
        .header("idempotency-key", "k2")
        .header("bf-signature", "t=1,v1=00")
        .body(Body::from(sale))
        .unwrap();
    got.push((
        StatusCode::UNAUTHORIZED,
        body_of(take(StatusCode::UNAUTHORIZED, send(&app, unsigned).await)).await,
    ));
    // 404
    let nokey = Request::post("/v2/events/nope")
        .header("idempotency-key", "k3")
        .body(Body::from(sale))
        .unwrap();
    got.push((
        StatusCode::NOT_FOUND,
        body_of(take(StatusCode::NOT_FOUND, send(&app, nokey).await)).await,
    ));
    // 409: same key, different body
    let first = send(&app, event_req("same", sale)).await;
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    let other = sale.replace("4900", "5000");
    got.push((
        StatusCode::CONFLICT,
        body_of(take(
            StatusCode::CONFLICT,
            send(&app, event_req("same", &other)).await,
        ))
        .await,
    ));
    // 413
    let big = "x".repeat(300 * 1024);
    got.push((
        StatusCode::PAYLOAD_TOO_LARGE,
        body_of(take(
            StatusCode::PAYLOAD_TOO_LARGE,
            send(&app, event_req("k4", &big)).await,
        ))
        .await,
    ));
    // 503: a program whose secrets could not be opened
    let mut snap = Snapshot::default();
    snap.broken.insert("pk".to_owned());
    let swapped: Arc<ArcSwap<Snapshot>> = state.snapshot.clone();
    swapped.store(Arc::new(snap));
    let res = take(
        StatusCode::SERVICE_UNAVAILABLE,
        send(&app, event_req("k5", sale)).await,
    );
    assert_eq!(res.headers().get("retry-after").unwrap(), "1");
    got.push((StatusCode::SERVICE_UNAVAILABLE, body_of(res).await));

    let bodies: Vec<String> = got.iter().map(|(_, b)| b.clone()).collect();
    for (status, body) in &got {
        let v: Value = serde_json::from_str(body).unwrap_or_else(|_| panic!("{status}: not JSON"));
        assert!(v["error"]["code"].is_string(), "{status}");
    }
    if let Some(ok) = verdicts("error", &bodies) {
        for ((status, body), ok) in got.iter().zip(ok) {
            assert!(ok, "{status} body fails error.schema.json: {body}");
        }
    }
    let _ = Path::new("");
    std::fs::remove_dir_all(dir).ok();
}
