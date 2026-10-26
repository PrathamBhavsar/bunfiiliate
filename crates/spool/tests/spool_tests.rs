use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::Barrier;

use bf_spool::{
    Fault, FsyncMode, SpoolConfig, SpoolWriter, delete_segments_below, list_segments,
    read_checkpoint, replay, replay_from_checkpoint, write_checkpoint,
};

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(name: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("bf-spool-test-{}-{}-{}", name, pid, id));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("failed to create test dir");
        Self { path }
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[tokio::test]
async fn test_roundtrip() {
    let test_dir = TestDir::new("roundtrip");
    let config = SpoolConfig::new(&test_dir.path).with_fsync_mode(FsyncMode::Always);
    let writer = SpoolWriter::open(config).await.expect("open writer");

    let mut written_offsets = Vec::new();
    for i in 0..10 {
        let payload = format!("roundtrip-payload-{:03}", i);
        let off = writer
            .write(payload.as_bytes())
            .await
            .expect("write record");
        written_offsets.push(off);
    }

    // Read back via replay from 0
    let records: Vec<_> = replay(&test_dir.path, 0).expect("replay from 0").collect();
    assert_eq!(records.len(), 10);

    for (i, rec) in records.iter().enumerate() {
        let expected_payload = format!("roundtrip-payload-{:03}", i);
        assert_eq!(rec.payload(), expected_payload.as_bytes());
        assert_eq!(rec.end_offset(), written_offsets[i]);
    }

    // Checkpoint at record 5's end offset
    let cp_offset = records[4].end_offset();
    write_checkpoint(&test_dir.path, cp_offset).expect("write checkpoint");
    assert_eq!(read_checkpoint(&test_dir.path).expect("read cp"), cp_offset);

    // Replay from checkpoint should yield remaining 5 records (5..10)
    let remaining: Vec<_> = replay_from_checkpoint(&test_dir.path)
        .expect("replay cp")
        .collect();
    assert_eq!(remaining.len(), 5);
    assert_eq!(remaining[0].payload(), b"roundtrip-payload-005");
    assert_eq!(remaining[4].payload(), b"roundtrip-payload-009");
}

#[tokio::test]
async fn test_torn_tail_replay() {
    let test_dir = TestDir::new("torn-tail");
    let config = SpoolConfig::new(&test_dir.path);
    let writer = SpoolWriter::open(config).await.expect("open writer");

    for i in 0..5 {
        writer
            .write(format!("record-payload-{}", i).as_bytes())
            .await
            .expect("write");
    }
    writer.flush().await.expect("flush");

    let segments = list_segments(&test_dir.path).expect("list segments");
    assert_eq!(segments.len(), 1);
    let seg_path = &segments[0].path;
    let original_len = segments[0].file_len;

    // Truncate file mid-record (truncate 5 bytes from the tail of record 4)
    {
        let file = OpenOptions::new()
            .write(true)
            .open(seg_path)
            .expect("open segment");
        file.set_len(original_len - 5).expect("truncate mid-record");
    }

    // Replay must stop cleanly at the torn tail and return first 4 records
    let records: Vec<_> = replay(&test_dir.path, 0)
        .expect("replay torn tail")
        .collect();
    assert_eq!(records.len(), 4);
    for (i, rec) in records.iter().enumerate() {
        assert_eq!(rec.payload(), format!("record-payload-{}", i).as_bytes());
    }

    // Now truncate into the middle of record 4's header (e.g. 3 bytes of header)
    let rec3_end = records[2].end_offset();
    {
        let file = OpenOptions::new()
            .write(true)
            .open(seg_path)
            .expect("open segment");
        file.set_len(rec3_end + 3).expect("truncate into header");
    }

    let records2: Vec<_> = replay(&test_dir.path, 0)
        .expect("replay torn header")
        .collect();
    assert_eq!(records2.len(), 3);
}

#[tokio::test]
async fn test_corrupt_crc_stops_replay() {
    let test_dir = TestDir::new("corrupt-crc");
    let config = SpoolConfig::new(&test_dir.path);
    let writer = SpoolWriter::open(config).await.expect("open writer");

    for i in 0..5 {
        writer
            .write(format!("record-payload-{}", i).as_bytes())
            .await
            .expect("write");
    }
    writer.flush().await.expect("flush");

    let records_before: Vec<_> = replay(&test_dir.path, 0).expect("replay").collect();
    assert_eq!(records_before.len(), 5);

    // Corrupt record 3's payload on disk
    let segments = list_segments(&test_dir.path).expect("list segments");
    let seg_path = &segments[0].path;

    // Record 3 starts at records_before[3].offset
    // Header is 8 bytes [4 bytes len | 4 bytes crc]
    // Payload byte is at offset + 8
    let corrupt_pos = records_before[3].offset() + 9;
    {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(seg_path)
            .expect("open segment");
        file.seek(SeekFrom::Start(corrupt_pos))
            .expect("seek to payload");
        let mut b = [0u8; 1];
        file.read_exact(&mut b).expect("read byte");
        b[0] ^= 0xFF; // flip bits
        file.seek(SeekFrom::Start(corrupt_pos)).expect("seek back");
        file.write_all(&b).expect("write corrupt byte");
        file.sync_all().expect("sync");
    }

    // Replay should cleanly yield records 0, 1, 2, and stop at record 3
    let records_after: Vec<_> = replay(&test_dir.path, 0)
        .expect("replay after corrupt")
        .collect();
    assert_eq!(records_after.len(), 3);
    assert_eq!(records_after[0].payload(), b"record-payload-0");
    assert_eq!(records_after[1].payload(), b"record-payload-1");
    assert_eq!(records_after[2].payload(), b"record-payload-2");
}

#[tokio::test]
async fn test_checkpoint_and_segment_gc() {
    let test_dir = TestDir::new("segment-gc");
    // Use small max segment size (120 bytes) to force rotation
    // Each record: 8 header + 40 payload = 48 bytes
    // Segment fits 2 records (96 bytes). 3rd record (144 bytes > 120) forces rotation.
    let config = SpoolConfig::new(&test_dir.path).with_max_segment_size(120);
    let writer = SpoolWriter::open(config).await.expect("open writer");

    let mut written_ends = Vec::new();
    for i in 0..6 {
        let payload = format!("fixed-length-payload-number-{:012}", i);
        assert_eq!(payload.len(), 40);
        let end_off = writer.write(payload.as_bytes()).await.expect("write");
        written_ends.push(end_off);
    }
    writer.flush().await.expect("flush");

    let segments_initial = list_segments(&test_dir.path).expect("list");
    // With 6 records of 48 bytes each across 120-byte max segment:
    // Seg 0: rec 0 (0..48), rec 1 (48..96). Size 96.
    // Seg 1: rec 2 (96..144), rec 3 (144..192). Size 96.
    // Seg 2: rec 4 (192..240), rec 5 (240..288). Size 96.
    assert!(
        segments_initial.len() >= 3,
        "expected at least 3 segments, got {}",
        segments_initial.len()
    );

    // Advance checkpoint past segment 0 (checkpoint at offset 96)
    let cp1 = written_ends[1];
    assert_eq!(cp1, 96);
    write_checkpoint(&test_dir.path, cp1).expect("write cp");

    // Run GC: delete segments fully below checkpoint 96
    let deleted1 = delete_segments_below(&test_dir.path, cp1).expect("gc 1");
    assert_eq!(deleted1, 1, "expected segment 0 to be deleted");

    let segments_after_gc1 = list_segments(&test_dir.path).expect("list after gc1");
    assert_eq!(segments_after_gc1.len(), segments_initial.len() - 1);
    assert_eq!(segments_after_gc1[0].start_offset, 96);

    // Replay from checkpoint should yield records 2, 3, 4, 5
    let records: Vec<_> = replay_from_checkpoint(&test_dir.path)
        .expect("replay cp")
        .collect();
    assert_eq!(records.len(), 4);
    assert_eq!(records[0].offset(), 96);
    assert_eq!(
        records[0].payload(),
        b"fixed-length-payload-number-000000000002"
    );

    // Advance checkpoint past segment 1 (offset 192)
    let cp2 = written_ends[3];
    assert_eq!(cp2, 192);
    write_checkpoint(&test_dir.path, cp2).expect("write cp2");
    let deleted2 = delete_segments_below(&test_dir.path, cp2).expect("gc 2");
    assert_eq!(deleted2, 1, "expected segment 1 to be deleted");

    let records2: Vec<_> = replay_from_checkpoint(&test_dir.path)
        .expect("replay cp2")
        .collect();
    assert_eq!(records2.len(), 2);
    assert_eq!(records2[0].offset(), 192);
    assert_eq!(
        records2[0].payload(),
        b"fixed-length-payload-number-000000000004"
    );
}

#[tokio::test]
async fn test_group_commit_concurrency() {
    let test_dir = TestDir::new("group-commit");
    let config = SpoolConfig::new(&test_dir.path).with_fsync_mode(FsyncMode::Always);
    let writer = SpoolWriter::open(config).await.expect("open writer");

    let n = 50;
    let barrier = Arc::new(Barrier::new(n));
    let mut set = tokio::task::JoinSet::new();

    for i in 0..n {
        let w = writer.clone();
        let b = Arc::clone(&barrier);
        set.spawn(async move {
            b.wait().await;
            let payload = format!("concurrent-record-{:04}", i);
            w.write(payload.as_bytes()).await
        });
    }

    let mut acks = Vec::new();
    while let Some(res) = set.join_next().await {
        let ack_offset = res.expect("join").expect("write ack");
        acks.push(ack_offset);
    }
    assert_eq!(acks.len(), n);

    let fsyncs = writer.fsync_count();
    assert!(
        fsyncs < n as u64,
        "fsyncs ({}) must be strictly fewer than records ({})",
        fsyncs,
        n
    );
    assert!(fsyncs > 0, "fsyncs ({}) must be greater than 0", fsyncs);

    // Verify all N records replay successfully
    let records: Vec<_> = replay(&test_dir.path, 0).expect("replay").collect();
    assert_eq!(records.len(), n);
}

#[tokio::test]
async fn test_open_recovers_torn_tail() {
    let test_dir = TestDir::new("recovery");
    let config = SpoolConfig::new(&test_dir.path);
    {
        let writer = SpoolWriter::open(config.clone()).await.expect("open");
        for i in 0..3 {
            writer
                .write(format!("recovery-record-{}", i).as_bytes())
                .await
                .expect("write");
        }
        writer.flush().await.expect("flush");
    }

    // Simulate crash with partial bytes appended
    let segments = list_segments(&test_dir.path).expect("list");
    let seg_path = &segments[0].path;
    {
        let mut file = OpenOptions::new()
            .append(true)
            .open(seg_path)
            .expect("open");
        file.write_all(b"incomplete partial tail bytes")
            .expect("append junk");
        file.sync_all().expect("sync");
    }

    // Reopen SpoolWriter: it must detect the torn tail, truncate it, and allow new writes
    let writer2 = SpoolWriter::open(config).await.expect("reopen");
    writer2
        .write(b"new-clean-record-after-crash")
        .await
        .expect("write after recovery");
    writer2.flush().await.expect("flush");

    let records: Vec<_> = replay(&test_dir.path, 0).expect("replay").collect();
    assert_eq!(records.len(), 4);
    assert_eq!(records[0].payload(), b"recovery-record-0");
    assert_eq!(records[1].payload(), b"recovery-record-1");
    assert_eq!(records[2].payload(), b"recovery-record-2");
    assert_eq!(records[3].payload(), b"new-clean-record-after-crash");
}

// ---- write and fsync failures

fn payloads(dir: &PathBuf) -> Vec<(u64, Vec<u8>)> {
    replay(dir, 0)
        .expect("replay")
        .map(|r| (r.end_offset(), r.payload().to_vec()))
        .collect()
}

#[tokio::test]
async fn failed_write_is_rolled_back_and_offsets_stay_contiguous() {
    let dir = TestDir::new("fail-write");
    let fault = Arc::new(Fault::default());
    let config = SpoolConfig::new(&dir.path).with_fault(fault.clone());
    let writer = SpoolWriter::open(config).await.unwrap();

    let a = writer.write(b"aaaa").await.unwrap();
    fault.fail_writes.store(1, Ordering::SeqCst);
    assert!(
        writer.write(b"bbbb").await.is_err(),
        "the torn write is reported"
    );
    let c = writer.write(b"cccc").await.unwrap();

    // C sits right after A: nothing was acked at an offset the file does not match.
    assert_eq!(c, a + (bf_spool::HEADER_LEN + 4) as u64);
    assert_eq!(
        payloads(&dir.path),
        [(a, b"aaaa".to_vec()), (c, b"cccc".to_vec())]
    );
    drop(writer);

    // And a restart sees the same thing (no torn bytes in the middle).
    let writer = SpoolWriter::open(SpoolConfig::new(&dir.path))
        .await
        .unwrap();
    let d = writer.write(b"dddd").await.unwrap();
    assert_eq!(d, c + (bf_spool::HEADER_LEN + 4) as u64);
    assert_eq!(payloads(&dir.path).len(), 3);
}

#[tokio::test]
async fn failed_fsync_drops_the_whole_group_and_later_writes_work() {
    let dir = TestDir::new("fail-fsync");
    let fault = Arc::new(Fault::default());
    let config = SpoolConfig::new(&dir.path).with_fault(fault.clone());
    let writer = SpoolWriter::open(config).await.unwrap();

    let a = writer.write(b"aaaa").await.unwrap();
    fault.fail_syncs.store(1, Ordering::SeqCst);
    assert!(writer.write(b"bbbb").await.is_err());
    let c = writer.write(b"cccc").await.unwrap();

    assert_eq!(c, a + (bf_spool::HEADER_LEN + 4) as u64);
    assert_eq!(
        payloads(&dir.path),
        [(a, b"aaaa".to_vec()), (c, b"cccc".to_vec())]
    );
}

#[tokio::test]
async fn writer_is_poisoned_when_the_rollback_fails() {
    let dir = TestDir::new("poison");
    let fault = Arc::new(Fault::default());
    let config = SpoolConfig::new(&dir.path).with_fault(fault.clone());
    let writer = SpoolWriter::open(config).await.unwrap();

    let a = writer.write(b"aaaa").await.unwrap();
    fault.fail_writes.store(1, Ordering::SeqCst);
    fault.fail_truncates.store(1, Ordering::SeqCst);
    assert!(writer.write(b"bbbb").await.is_err());
    // The file may hold torn bytes the writer could not remove: nothing more is acked.
    assert!(writer.write(b"cccc").await.is_err());
    assert!(writer.flush().await.is_err());
    drop(writer);

    // After a restart the torn tail is truncated away and writing resumes after A.
    let writer = SpoolWriter::open(SpoolConfig::new(&dir.path))
        .await
        .unwrap();
    let c = writer.write(b"cccc").await.unwrap();
    assert_eq!(c, a + (bf_spool::HEADER_LEN + 4) as u64);
    assert_eq!(
        payloads(&dir.path),
        [(a, b"aaaa".to_vec()), (c, b"cccc".to_vec())]
    );
}

#[tokio::test]
async fn failure_in_a_group_fails_every_record_in_it() {
    let dir = TestDir::new("fail-group");
    let fault = Arc::new(Fault::default());
    let config = SpoolConfig::new(&dir.path).with_fault(fault.clone());
    let writer = SpoolWriter::open(config).await.unwrap();
    let a = writer.write(b"aaaa").await.unwrap();

    // Several writers race into one group; the fsync fails, so none of them is acked.
    fault.fail_syncs.store(1, Ordering::SeqCst);
    let barrier = Arc::new(Barrier::new(8));
    let mut tasks = Vec::new();
    for i in 0..8 {
        let (w, b) = (writer.clone(), barrier.clone());
        tasks.push(tokio::spawn(async move {
            b.wait().await;
            w.write(format!("g{i}").as_bytes()).await
        }));
    }
    let mut acked = Vec::new();
    for t in tasks {
        if let Ok(off) = t.await.unwrap() {
            acked.push(off);
        }
    }
    // Whatever was acked is exactly what the file holds, back to back after A.
    let on_disk = payloads(&dir.path);
    assert_eq!(on_disk[0].0, a);
    let ends: Vec<u64> = on_disk[1..].iter().map(|(e, _)| *e).collect();
    let mut sorted = acked.clone();
    sorted.sort_unstable();
    assert_eq!(ends, sorted);
}
