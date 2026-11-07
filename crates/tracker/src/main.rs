use std::net::SocketAddr;

use bf_rt::Config;
use bf_spool::{FsyncMode, SpoolConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if std::env::args().nth(1).as_deref() == Some("openapi") {
        println!("{}", tracker::openapi::json());
        return Ok(());
    }
    bf_rt::init_tracing();
    let cfg = Config::from_env()?;
    let fsync = match std::env::var("BF_FSYNC").as_deref() {
        Ok("off") => FsyncMode::Off,
        _ => FsyncMode::Always,
    };
    let pool = bf_db::connect(&cfg.database_url).await?;
    let spool = SpoolConfig::new(&cfg.spool_dir).with_fsync_mode(fsync);
    // Optional DB-IP lite country database (`scripts/fetch-dbip.sh`); unset = header only.
    let geo = match std::env::var_os("BF_GEOIP_DB") {
        Some(path) => tracker::geo::Geo::open(path.as_ref())?,
        None => tracker::geo::Geo::disabled(),
    };
    // The visitor-IP hash salt is its own secret: sharing the JWT secret would tie the two together.
    let ip_salt = std::env::var("BF_IP_SALT").unwrap_or_default();
    if ip_salt.len() < 16 {
        return Err(
            "BF_IP_SALT must be set to a random string of at least 16 bytes (openssl rand -hex 32)"
                .into(),
        );
    }
    let trust = tracker::proxy::Trust::from_lookup(|k| std::env::var(k).ok())?;
    let vault = std::sync::Arc::new(load_vault()?);
    let mut tracker = tracker::start(pool, spool, &ip_salt, geo, vault).await?;
    tracker.state.trust = std::sync::Arc::new(trust);
    let _space = bf_spool::space::spawn_sampler(
        tracker.state.spool.clone(),
        bf_spool::space::SpaceConfig::from_lookup(|k| std::env::var(k).ok()),
        tracker::metrics::spool_report,
    );

    let metrics = tokio::spawn(bf_rt::metrics::serve(
        "127.0.0.1:9100",
        tracker::metrics::render,
    ));
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", cfg.tracker_port)).await?;
    tracing::info!(port = cfg.tracker_port, "tracker started");
    let app = tracker::router(tracker.state).into_make_service_with_connect_info::<SocketAddr>();
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    // The router is gone, so the queue is closed: let the writer drain it.
    tracker.writer.await?;
    tracker.watcher.abort();
    metrics.abort();
    Ok(())
}

/// Master keys from `BF_MASTER_KEYS` / `BF_MASTER_KEY_FILE`; plaintext only with
/// `BF_ALLOW_PLAINTEXT_SECRETS=1`.
fn load_vault() -> Result<bf_core::secrets::Vault, bf_core::secrets::SecretsError> {
    let allow = matches!(
        std::env::var("BF_ALLOW_PLAINTEXT_SECRETS").as_deref(),
        Ok("1" | "true")
    );
    bf_core::secrets::Vault::from_env(allow)
}
