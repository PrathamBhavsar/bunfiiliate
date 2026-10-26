//! In-memory config the hot path reads: code -> target, program key -> secrets.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

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

/// Load the snapshot, then keep it fresh: reload on every `bf_config` notification.
/// LISTEN is established before the first load so no change is missed.
pub async fn load_and_watch(
    pool: PgPool,
) -> Result<(Arc<ArcSwap<Snapshot>>, JoinHandle<()>), sqlx::Error> {
    let mut listener = PgListener::connect_with(&pool).await?;
    listener.listen(CONFIG_CHANNEL).await?;
    let cell = Arc::new(ArcSwap::from_pointee(Snapshot::load(&pool).await?));
    let watched = Arc::clone(&cell);
    let task = tokio::spawn(async move {
        loop {
            // After a dropped connection recv() reconnects; reload covers missed notifications.
            let lost = listener.recv().await.err();
            if let Some(e) = &lost {
                tracing::warn!(error = %e, "bf_config listener error");
            }
            match Snapshot::load(&pool).await {
                Ok(s) => watched.store(Arc::new(s)),
                Err(e) => tracing::error!(error = %e, "snapshot reload failed"),
            }
            if lost.is_some() {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    });
    Ok((cell, task))
}
