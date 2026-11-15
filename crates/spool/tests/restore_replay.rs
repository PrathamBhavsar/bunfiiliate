//! A restored spool replays from a checkpoint copied BEFORE the segments, so replay may
//! re-deliver records already in the DB. Delivery must be idempotent (click id PK,
//! `ON CONFLICT DO NOTHING`); the DB side is modelled as a keyed set here.
use std::collections::HashMap;

use bf_spool::{SpoolConfig, SpoolWriter, replay_from_checkpoint};

#[tokio::test]
async fn spool_restore_replay_is_idempotent() {
    let base = std::env::temp_dir().join(format!("bf-restore-{}", std::process::id()));
    let (src, dst) = (base.join("src"), base.join("dst"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&dst).unwrap();

    let w = SpoolWriter::open(SpoolConfig::new(&src)).await.unwrap();
    let mut ends = vec![];
    for i in 0..5u8 {
        ends.push(w.write([i; 4]).await.unwrap());
    }
    w.checkpoint(ends[1]).unwrap();
    // Backup order: checkpoint first, then segments.
    std::fs::copy(src.join("checkpoint"), dst.join("checkpoint")).unwrap();
    for e in std::fs::read_dir(&src).unwrap() {
        let p = e.unwrap().path();
        if p.file_name().unwrap() != "checkpoint" {
            std::fs::copy(&p, dst.join(p.file_name().unwrap())).unwrap();
        }
    }

    let mut db: HashMap<u8, u32> = HashMap::new();
    // The DB already holds the records the checkpoint covers (a dump taken later may hold more).
    db.insert(0, 1);
    db.insert(1, 1);
    for pass in 0..2 {
        let mut n = 0;
        for r in replay_from_checkpoint(&dst).unwrap() {
            db.entry(r.payload()[0]).or_insert(1); // ON CONFLICT DO NOTHING
            n += 1;
        }
        assert_eq!(n, 3, "pass {pass}: records after the checkpoint");
        assert_eq!(db.len(), 5, "pass {pass}: no duplicates");
    }
    let _ = std::fs::remove_dir_all(&base);
}
