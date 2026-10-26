//! In-memory config the hot path reads: code -> target, program key -> secrets.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use bf_db::tracker::{CONFIG_CHANNEL, load_codes, load_program_keys};
use sqlx::PgPool;
use sqlx::postgres::PgListener;
use tokio::task::JoinHandle;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct CodeEntry {
    pub program_id: Uuid,
    pub affiliate_id: Uuid,
    pub landing_url: String,
}

#[derive(Debug, Clone)]
pub struct ProgramSecrets {
    pub program_id: Uuid,
    pub webhook_secret: String,
    pub stripe_secret: Option<String>,
    pub paddle_secret: Option<String>,
}

#[derive(Debug, Default)]
pub struct Snapshot {
    pub codes: HashMap<String, CodeEntry>,
    /// Keyed by program `public_key`.
    pub programs: HashMap<String, ProgramSecrets>,
}

impl Snapshot {
    pub async fn load(pool: &PgPool) -> Result<Self, sqlx::Error> {
        let codes = load_codes(pool)
            .await?
            .into_iter()
            .map(|r| {
                let entry = CodeEntry {
                    program_id: r.program_id,
                    affiliate_id: r.affiliate_id,
                    landing_url: r.landing_url,
                };
                (r.code, entry)
            })
            .collect();
        let programs = load_program_keys(pool)
            .await?
            .into_iter()
            .map(|r| {
                let secrets = ProgramSecrets {
                    program_id: r.program_id,
                    webhook_secret: r.webhook_secret,
                    stripe_secret: r.stripe_secret,
                    paddle_secret: r.paddle_secret,
                };
                (r.public_key, secrets)
            })
            .collect();
        Ok(Self { codes, programs })
    }
}

/// Reloads are at least this far apart. The first change after a quiet spell reloads at once
/// (a code created in the UI works immediately); changes inside the window share the next
/// reload, so a burst (bulk import, code-creation flood) costs about four table loads a
/// second, not one per change.
const RELOAD_MIN_INTERVAL: Duration = Duration::from_millis(250);

/// Load the snapshot, then keep it fresh: reload after `bf_config` notifications (coalesced).
/// LISTEN is established before the first load so no change is missed.
pub async fn load_and_watch(
    pool: PgPool,
) -> Result<(Arc<ArcSwap<Snapshot>>, JoinHandle<()>), sqlx::Error> {
    let mut listener = PgListener::connect_with(&pool).await?;
    listener.listen(CONFIG_CHANNEL).await?;
    let cell = Arc::new(ArcSwap::from_pointee(Snapshot::load(&pool).await?));
    let watched = Arc::clone(&cell);
    let task = tokio::spawn(async move {
        let mut last_reload = Instant::now();
        let mut pending = false;
        loop {
            let due = last_reload + RELOAD_MIN_INTERVAL;
            tokio::select! {
                got = listener.recv() => {
                    // After a dropped connection recv() reconnects; the reload covers
                    // notifications missed meanwhile.
                    pending = true;
                    if let Err(e) = got {
                        tracing::warn!(error = %e, "bf_config listener error");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
                _ = tokio::time::sleep_until(due.into()), if pending => {}
            }
            if pending && Instant::now() >= due {
                reload(&pool, &watched).await;
                last_reload = Instant::now();
                pending = false;
            }
        }
    });
    Ok((cell, task))
}

async fn reload(pool: &PgPool, cell: &ArcSwap<Snapshot>) {
    match Snapshot::load(pool).await {
        Ok(s) => cell.store(Arc::new(s)),
        Err(e) => tracing::error!(error = %e, "snapshot reload failed"),
    }
}
