//! DB batch writer: spool-acked clicks and inbound events -> Postgres, then checkpoint + GC.

use std::collections::BTreeMap;
use std::time::Duration;

use bf_db::tracker::{Click, InboundEvent, insert_clicks, insert_events};
use bf_spool::SpoolWriter;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::metrics;

pub type Error = Box<dyn std::error::Error + Send + Sync>;

const MAX_BATCH: usize = 1000;

/// What the spool holds. Untagged: a click keeps its original JSON shape, an event
/// is told apart by its own fields.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum Record {
    Click(Click),
    Event(InboundEvent),
}

/// A record already durable in the spool at byte range `start..end`.
pub struct Queued {
    pub start: u64,
    pub end: u64,
    pub record: Record,
}

#[derive(Default)]
struct Batch {
    clicks: Vec<Click>,
    events: Vec<InboundEvent>,
}

impl Batch {
    fn push(&mut self, record: Record) {
        match record {
            Record::Click(c) => self.clicks.push(c),
            Record::Event(e) => self.events.push(e),
        }
    }

    fn len(&self) -> usize {
        self.clicks.len() + self.events.len()
    }

    /// Idempotent: unique keys make a retry or replay a no-op.
    async fn insert(&self, pool: &PgPool) -> Result<(), sqlx::Error> {
        if !self.clicks.is_empty() {
            insert_clicks(pool, &self.clicks).await?;
        }
        if !self.events.is_empty() {
            insert_events(pool, &self.events).await?;
        }
        Ok(())
    }
}

/// Insert everything past the checkpoint (idempotent), advance it, return it.
pub async fn replay(pool: &PgPool, spool: &SpoolWriter) -> Result<u64, Error> {
    let from = spool.read_checkpoint()?;
    let mut end = from;
    let mut batch = Batch::default();
    for rec in spool.replay(from)? {
        end = rec.end_offset();
        match serde_json::from_slice::<Record>(rec.payload()) {
            Ok(record) => batch.push(record),
            Err(e) => {
                tracing::error!(offset = rec.offset(), error = %e, "skipping bad spool record")
            }
        }
        if batch.len() >= MAX_BATCH {
            batch.insert(pool).await?;
            batch = Batch::default();
        }
    }
    batch.insert(pool).await?;
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
            let mut batch = Batch::default();
            for q in buf.drain(..) {
                flushed.insert(q.start, q.end);
                batch.push(q.record);
            }
            metrics::BATCH.observe(batch.len() as u64);
            while let Err(e) = batch.insert(&pool).await {
                tracing::error!(error = %e, "batch insert failed, retrying");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            let before = checkpoint;
            while let Some(end) = flushed.remove(&checkpoint) {
                checkpoint = end;
            }
            if checkpoint != before {
                match spool
                    .checkpoint(checkpoint)
                    .and_then(|_| spool.gc(checkpoint))
                {
                    Ok(_) => metrics::SPOOL_CHECKPOINT.set(checkpoint),
                    Err(e) => tracing::error!(error = %e, "spool checkpoint failed"),
                }
            }
        }
    })
}
