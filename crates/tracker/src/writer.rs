//! DB batch writer: spool-acked clicks -> `clicks`, then checkpoint + GC.

use std::collections::BTreeMap;
use std::time::Duration;

use bf_db::tracker::{Click, insert_clicks};
use bf_spool::SpoolWriter;
use sqlx::PgPool;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub type Error = Box<dyn std::error::Error + Send + Sync>;

const MAX_BATCH: usize = 1000;

/// A click already durable in the spool at byte range `start..end`.
pub struct Queued {
    pub start: u64,
    pub end: u64,
    pub click: Click,
}

/// Insert everything past the checkpoint (idempotent), advance it, return it.
pub async fn replay(pool: &PgPool, spool: &SpoolWriter) -> Result<u64, Error> {
    let from = spool.read_checkpoint()?;
    let mut end = from;
    let mut batch = Vec::new();
    for rec in spool.replay(from)? {
        end = rec.end_offset();
        match serde_json::from_slice::<Click>(rec.payload()) {
            Ok(click) => batch.push(click),
            Err(e) => {
                tracing::error!(offset = rec.offset(), error = %e, "skipping bad spool record")
            }
        }
        if batch.len() >= MAX_BATCH {
            insert_clicks(pool, &batch).await?;
            batch.clear();
        }
    }
    if !batch.is_empty() {
        insert_clicks(pool, &batch).await?;
    }
    if end > from {
        spool.checkpoint(end)?;
        spool.gc(end)?;
    }
    Ok(end)
}

/// Runs until every sender is dropped and the queue is drained.
///
/// Handlers finish spool writes in any order, so the checkpoint only moves over a
/// contiguous prefix of flushed records: a record still in flight keeps it back.
pub fn spawn(
    pool: PgPool,
    spool: SpoolWriter,
    mut rx: mpsc::Receiver<Queued>,
    mut checkpoint: u64,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut flushed: BTreeMap<u64, u64> = BTreeMap::new();
        let mut buf = Vec::new();
        while rx.recv_many(&mut buf, MAX_BATCH).await > 0 {
            let mut clicks = Vec::with_capacity(buf.len());
            for q in buf.drain(..) {
                flushed.insert(q.start, q.end);
                clicks.push(q.click);
            }
            while let Err(e) = insert_clicks(&pool, &clicks).await {
                tracing::error!(error = %e, "click batch insert failed, retrying");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            let before = checkpoint;
            while let Some(end) = flushed.remove(&checkpoint) {
                checkpoint = end;
            }
            if checkpoint != before
                && let Err(e) = spool
                    .checkpoint(checkpoint)
                    .and_then(|_| spool.gc(checkpoint))
            {
                tracing::error!(error = %e, "spool checkpoint failed");
            }
        }
    })
}
