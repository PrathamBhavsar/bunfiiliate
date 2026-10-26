use std::net::SocketAddr;

use bf_rt::Config;
use bf_spool::{FsyncMode, SpoolConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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
    let tracker = tracker::start(pool, spool, &cfg.jwt_secret, geo).await?;

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
    Ok(())
}
