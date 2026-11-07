use std::sync::Arc;
use std::sync::atomic::Ordering;

use bf_spool::space::{SpaceConfig, SpaceGuard};
use bf_spool::{Fault, SpoolConfig, SpoolError, SpoolWriter, replay};

fn tmp(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("bf-diskfull-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn cfg() -> SpaceConfig {
    SpaceConfig {
        high_pct: 85,
        low_pct: 70,
        min_free_bytes: 0,
        max_bytes: None,
    }
}

#[test]
fn space_guard_hysteresis() {
    let g = SpaceGuard::new(cfg());
    assert!(!g.shedding());
    g.observe(10, 100, 0); // 90% used
    assert!(g.shedding());
    g.observe(20, 100, 0); // 80%: stays
    assert!(g.shedding());
    g.observe(31, 100, 0); // 69%: recovers
    assert!(!g.shedding());
    g.observe(20, 100, 0); // 80% from ok: stays ok
    assert!(!g.shedding());
}

#[test]
fn space_guard_min_free_and_max_bytes() {
    let g = SpaceGuard::new(SpaceConfig {
        min_free_bytes: 1000,
        max_bytes: Some(500),
        ..cfg()
    });
    g.observe(900, 10_000, 0);
    assert!(g.shedding(), "below min free");
    g.observe(9000, 10_000, 0);
    assert!(!g.shedding());
    g.observe(9000, 10_000, 501);
    assert!(g.shedding(), "over max bytes");
}

#[test]
fn space_config_from_lookup() {
    let c = SpaceConfig::from_lookup(|k| match k {
        "BF_SPOOL_HIGH_WATER_PCT" => Some("90".into()),
        "BF_SPOOL_MAX_BYTES" => Some("123".into()),
        _ => None,
    });
    assert_eq!((c.high_pct, c.low_pct), (90, 70));
    assert_eq!(c.min_free_bytes, 256 * 1024 * 1024);
    assert_eq!(c.max_bytes, Some(123));
}

#[test]
fn statvfs_reports_free_space() {
    let d = tmp("statvfs");
    let (free, total) = bf_spool::space::free_space(&d).unwrap();
    assert!(total > 0 && free <= total);
}

#[tokio::test]
async fn write_returns_full_on_enospc() {
    let d = tmp("enospc");
    let fault = Arc::new(Fault::default());
    let w = SpoolWriter::open(SpoolConfig::new(&d).with_fault(fault.clone()))
        .await
        .unwrap();
    w.write(b"one").await.unwrap();
    fault.fail_enospc.store(1, Ordering::SeqCst);
    let err = w.write(b"two").await.unwrap_err();
    assert!(matches!(err, SpoolError::Full), "{err:?}");
    assert!(w.is_shedding());
    // Rolled back: later writes work and no torn record is left.
    w.write(b"three").await.unwrap();
    drop(w);
    let got: Vec<Vec<u8>> = replay(&d, 0)
        .unwrap()
        .map(|r| r.payload().to_vec())
        .collect();
    assert_eq!(got, vec![b"one".to_vec(), b"three".to_vec()]);
    let w = SpoolWriter::open(SpoolConfig::new(&d)).await.unwrap();
    assert_eq!(w.replay(0).unwrap().count(), 2);
}

#[tokio::test]
async fn gc_honours_pause_flag() {
    let d = tmp("pause");
    let w = SpoolWriter::open(SpoolConfig::new(&d).with_max_segment_size(32))
        .await
        .unwrap();
    let mut end = 0;
    for _ in 0..6 {
        end = w.write(b"0123456789").await.unwrap();
    }
    bf_spool::pause_gc(&d).unwrap();
    assert!(bf_spool::gc_paused(&d));
    assert_eq!(w.gc(end).unwrap(), 0);
    bf_spool::resume_gc(&d).unwrap();
    assert!(!bf_spool::gc_paused(&d));
    assert!(w.gc(end).unwrap() > 0);
}

/// Linux only; run via `scripts/tmpfs-test.sh` (mounts a 2 MiB tmpfs in a private namespace
/// and sets `BF_TMPFS_DIR`). Fill -> ENOSPC -> checkpoint + gc frees space -> writes work
/// again, and everything acked is still in the spool.
#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "needs BF_TMPFS_DIR on a tiny tmpfs (scripts/tmpfs-test.sh)"]
async fn tiny_tmpfs_fills_and_recovers() {
    let dir = std::path::PathBuf::from(std::env::var("BF_TMPFS_DIR").expect("BF_TMPFS_DIR"));
    let w = SpoolWriter::open(SpoolConfig::new(&dir).with_max_segment_size(256 * 1024))
        .await
        .unwrap();
    let payload = vec![7u8; 4096];
    let (mut acked, mut end) = (0u64, 0u64);
    loop {
        match w.write(&payload).await {
            Ok(e) => {
                acked += 1;
                end = e;
            }
            Err(SpoolError::Full) => break,
            Err(e) => panic!("{e:?}"),
        }
        assert!(acked < 10_000, "tmpfs never filled");
    }
    assert!(w.is_shedding());
    w.checkpoint(end).unwrap();
    assert!(w.gc(end).unwrap() > 0);
    w.set_shedding(false);
    w.write(&payload).await.unwrap();
    let n = w.replay(0).unwrap().count() as u64;
    assert!(n >= 1 && n <= acked + 1);
}
