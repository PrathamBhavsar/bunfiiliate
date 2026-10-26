use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::AsyncWriteExt;

pub const HEADER_LEN: usize = 8;
pub const MAX_RECORD_PAYLOAD_SIZE: usize = 64 * 1024 * 1024; // 64 MB

#[derive(Debug)]
pub enum SpoolError {
    Io(io::Error),
    Corrupted(String),
    ChannelClosed,
}

impl std::fmt::Display for SpoolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "spool I/O error: {e}"),
            Self::Corrupted(msg) => write!(f, "spool corrupted: {msg}"),
            Self::ChannelClosed => write!(f, "spool channel closed"),
        }
    }
}

impl std::error::Error for SpoolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for SpoolError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<SpoolError> for io::Error {
    fn from(e: SpoolError) -> Self {
        match e {
            SpoolError::Io(io_err) => io_err,
            SpoolError::Corrupted(msg) => io::Error::new(io::ErrorKind::InvalidData, msg),
            SpoolError::ChannelClosed => {
                io::Error::new(io::ErrorKind::BrokenPipe, "channel closed")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FsyncMode {
    #[default]
    Always,
    Off,
}

#[derive(Debug, Clone)]
pub struct SpoolConfig {
    pub dir: PathBuf,
    pub max_segment_size: u64,
    pub fsync_mode: FsyncMode,
    pub channel_capacity: usize,
}

impl Default for SpoolConfig {
    fn default() -> Self {
        Self {
            dir: PathBuf::from("./var/spool"),
            max_segment_size: 16 * 1024 * 1024,
            fsync_mode: FsyncMode::Always,
            channel_capacity: 10_000,
        }
    }
}

impl SpoolConfig {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            ..Default::default()
        }
    }

    pub fn with_max_segment_size(mut self, size: u64) -> Self {
        self.max_segment_size = size;
        self
    }

    pub fn with_fsync_mode(mut self, mode: FsyncMode) -> Self {
        self.fsync_mode = mode;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpoolRecord {
    pub offset: u64,
    pub payload: Vec<u8>,
}

impl SpoolRecord {
    pub fn new(offset: u64, payload: Vec<u8>) -> Self {
        Self { offset, payload }
    }

    pub fn offset(&self) -> u64 {
        self.offset
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub fn end_offset(&self) -> u64 {
        self.offset + HEADER_LEN as u64 + self.payload.len() as u64
    }
}

pub fn encode_record(payload: &[u8]) -> Vec<u8> {
    let len = payload.len() as u32;
    let crc = crc32fast::hash(payload);
    let mut buf = Vec::with_capacity(HEADER_LEN + payload.len());
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(&crc.to_le_bytes());
    buf.extend_from_slice(payload);
    buf
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentMeta {
    pub start_offset: u64,
    pub file_len: u64,
    pub path: PathBuf,
}

impl SegmentMeta {
    pub fn end_offset(&self) -> u64 {
        self.start_offset + self.file_len
    }
}

pub fn segment_filename(start_offset: u64) -> String {
    format!("{:020}.wal", start_offset)
}

pub fn parse_segment_filename(name: &str) -> Option<u64> {
    if name.len() == 24 && name.ends_with(".wal") {
        name[..20].parse::<u64>().ok()
    } else {
        None
    }
}

pub fn list_segments(dir: &Path) -> io::Result<Vec<SegmentMeta>> {
    let mut segments = Vec::new();
    if !dir.exists() {
        return Ok(segments);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let name_str = file_name.to_string_lossy();
        if let Some(start_offset) = parse_segment_filename(&name_str) {
            let meta = entry.metadata()?;
            if meta.is_file() {
                segments.push(SegmentMeta {
                    start_offset,
                    file_len: meta.len(),
                    path: entry.path(),
                });
            }
        }
    }
    segments.sort_by_key(|s| s.start_offset);
    Ok(segments)
}

pub fn write_checkpoint(dir: impl AsRef<Path>, offset: u64) -> Result<(), SpoolError> {
    let dir = dir.as_ref();
    std::fs::create_dir_all(dir)?;
    let target = dir.join("checkpoint");
    let tmp = dir.join(format!(".checkpoint.tmp.{}.{}", std::process::id(), offset));
    let content = format!("{}\n", offset);
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, &target)?;
    Ok(())
}

pub fn read_checkpoint(dir: impl AsRef<Path>) -> Result<u64, SpoolError> {
    let dir = dir.as_ref();
    let target = dir.join("checkpoint");
    if !target.exists() {
        return Ok(0);
    }
    let bytes = std::fs::read(&target)?;
    if bytes.is_empty() {
        return Ok(0);
    }
    if let Some(val) = std::str::from_utf8(&bytes)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
    {
        return Ok(val);
    }
    if bytes.len() == 8 {
        return Ok(u64::from_le_bytes(bytes.try_into().unwrap()));
    }
    Err(SpoolError::Corrupted("invalid checkpoint file".into()))
}

pub fn delete_segments_below(dir: impl AsRef<Path>, checkpoint: u64) -> Result<usize, SpoolError> {
    let dir = dir.as_ref();
    let segments = list_segments(dir)?;
    if segments.is_empty() {
        return Ok(0);
    }
    let active_start = segments.last().map(|s| s.start_offset);
    let mut deleted = 0;
    for seg in &segments {
        if Some(seg.start_offset) == active_start {
            continue;
        }
        if seg.end_offset() <= checkpoint {
            std::fs::remove_file(&seg.path)?;
            deleted += 1;
        }
    }
    Ok(deleted)
}

pub struct ReplayIter {
    segments: Vec<SegmentMeta>,
    current_seg_index: usize,
    current_reader: Option<std::io::BufReader<std::fs::File>>,
    current_global_offset: u64,
    from_offset: u64,
    stopped: bool,
}

impl ReplayIter {
    pub fn open(dir: &Path, from_offset: u64) -> io::Result<Self> {
        let segments = list_segments(dir)?;
        Ok(Self {
            segments,
            current_seg_index: 0,
            current_reader: None,
            current_global_offset: 0,
            from_offset,
            stopped: false,
        })
    }
}

impl Iterator for ReplayIter {
    type Item = SpoolRecord;

    fn next(&mut self) -> Option<Self::Item> {
        if self.stopped {
            return None;
        }

        loop {
            if self.current_reader.is_none() {
                if self.current_seg_index >= self.segments.len() {
                    self.stopped = true;
                    return None;
                }

                let seg = &self.segments[self.current_seg_index];
                self.current_seg_index += 1;

                if seg.end_offset() <= self.from_offset {
                    continue;
                }

                let file = match std::fs::File::open(&seg.path) {
                    Ok(f) => f,
                    Err(_) => {
                        self.stopped = true;
                        return None;
                    }
                };

                self.current_reader = Some(std::io::BufReader::new(file));
                self.current_global_offset = seg.start_offset;
            }

            let reader = self.current_reader.as_mut().unwrap();

            let mut header = [0u8; 8];
            let mut header_read = 0;
            let mut torn_header = false;

            while header_read < 8 {
                match reader.read(&mut header[header_read..]) {
                    Ok(0) => {
                        if header_read == 0 {
                            break;
                        } else {
                            torn_header = true;
                            break;
                        }
                    }
                    Ok(n) => header_read += n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        self.stopped = true;
                        self.current_reader = None;
                        return None;
                    }
                }
            }

            if torn_header {
                self.stopped = true;
                self.current_reader = None;
                return None;
            }

            if header_read == 0 {
                self.current_reader = None;
                continue;
            }

            let len = u32::from_le_bytes(header[0..4].try_into().unwrap()) as usize;
            let expected_crc = u32::from_le_bytes(header[4..8].try_into().unwrap());

            if len > MAX_RECORD_PAYLOAD_SIZE {
                self.stopped = true;
                self.current_reader = None;
                return None;
            }

            let mut payload = vec![0u8; len];
            let mut payload_read = 0;
            let mut torn_payload = false;

            while payload_read < len {
                match reader.read(&mut payload[payload_read..]) {
                    Ok(0) => {
                        torn_payload = true;
                        break;
                    }
                    Ok(n) => payload_read += n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        self.stopped = true;
                        self.current_reader = None;
                        return None;
                    }
                }
            }

            if torn_payload {
                self.stopped = true;
                self.current_reader = None;
                return None;
            }

            let actual_crc = crc32fast::hash(&payload);
            if actual_crc != expected_crc {
                self.stopped = true;
                self.current_reader = None;
                return None;
            }

            let record_start = self.current_global_offset;
            self.current_global_offset += HEADER_LEN as u64 + len as u64;

            if record_start >= self.from_offset {
                return Some(SpoolRecord {
                    offset: record_start,
                    payload,
                });
            }
        }
    }
}

pub fn replay(dir: impl AsRef<Path>, from_offset: u64) -> Result<ReplayIter, SpoolError> {
    ReplayIter::open(dir.as_ref(), from_offset).map_err(SpoolError::Io)
}

pub fn replay_from_checkpoint(dir: impl AsRef<Path>) -> Result<ReplayIter, SpoolError> {
    let dir = dir.as_ref();
    let cp = read_checkpoint(dir)?;
    replay(dir, cp)
}

enum Command {
    Write {
        payload: Vec<u8>,
        ack: tokio::sync::oneshot::Sender<Result<u64, SpoolError>>,
    },
    Flush {
        ack: tokio::sync::oneshot::Sender<Result<(), SpoolError>>,
    },
}

#[derive(Clone)]
pub struct SpoolWriter {
    tx: tokio::sync::mpsc::Sender<Command>,
    fsync_count: Arc<AtomicU64>,
    dir: PathBuf,
}

impl SpoolWriter {
    pub async fn open(config: SpoolConfig) -> Result<Self, SpoolError> {
        tokio::fs::create_dir_all(&config.dir).await?;
        let segments = list_segments(&config.dir)?;

        let (mut current_global_offset, mut current_size, mut active_file) =
            if let Some(last_seg) = segments.last() {
                let mut valid_end = last_seg.start_offset;
                if let Ok(iter) = ReplayIter::open(&config.dir, last_seg.start_offset) {
                    for rec in iter {
                        valid_end = rec.end_offset();
                    }
                }

                let valid_len = valid_end - last_seg.start_offset;
                if valid_len < last_seg.file_len {
                    let f = std::fs::OpenOptions::new()
                        .write(true)
                        .open(&last_seg.path)?;
                    f.set_len(valid_len)?;
                    f.sync_all()?;
                }

                if valid_len > 0 && valid_len >= config.max_segment_size {
                    let new_path = config.dir.join(segment_filename(valid_end));
                    let f = tokio::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&new_path)
                        .await?;
                    (valid_end, 0, f)
                } else {
                    let f = tokio::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&last_seg.path)
                        .await?;
                    (valid_end, valid_len, f)
                }
            } else {
                let new_path = config.dir.join(segment_filename(0));
                let f = tokio::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&new_path)
                    .await?;
                (0, 0, f)
            };

        let fsync_count = Arc::new(AtomicU64::new(0));
        let fsync_count_worker = Arc::clone(&fsync_count);
        let dir = config.dir.clone();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Command>(config.channel_capacity);

        tokio::spawn(async move {
            let mut batch = Vec::new();
            loop {
                batch.clear();
                let first = match rx.recv().await {
                    Some(cmd) => cmd,
                    None => break,
                };
                batch.push(first);
                while let Ok(cmd) = rx.try_recv() {
                    batch.push(cmd);
                }

                let mut write_err: Option<io::Error> = None;
                let mut pending_acks = Vec::with_capacity(batch.len());
                let mut pending_flushes = Vec::new();

                for cmd in batch.drain(..) {
                    match cmd {
                        Command::Write { payload, ack } => {
                            if let Some(ref e) = write_err {
                                let _ = ack.send(Err(SpoolError::Io(io::Error::new(
                                    e.kind(),
                                    e.to_string(),
                                ))));
                                continue;
                            }

                            let encoded = encode_record(&payload);
                            let record_len = encoded.len() as u64;

                            if current_size > 0
                                && current_size + record_len > config.max_segment_size
                            {
                                if config.fsync_mode == FsyncMode::Always {
                                    if let Err(e) = active_file.sync_all().await {
                                        let err_copy = io::Error::new(e.kind(), e.to_string());
                                        write_err = Some(e);
                                        let _ = ack.send(Err(SpoolError::Io(err_copy)));
                                        continue;
                                    }
                                    fsync_count_worker.fetch_add(1, Ordering::SeqCst);
                                }

                                let new_offset = current_global_offset;
                                let new_path = config.dir.join(segment_filename(new_offset));
                                match tokio::fs::OpenOptions::new()
                                    .create(true)
                                    .append(true)
                                    .open(&new_path)
                                    .await
                                {
                                    Ok(f) => {
                                        active_file = f;
                                        current_size = 0;
                                    }
                                    Err(e) => {
                                        let err_copy = io::Error::new(e.kind(), e.to_string());
                                        write_err = Some(e);
                                        let _ = ack.send(Err(SpoolError::Io(err_copy)));
                                        continue;
                                    }
                                }
                            }

                            match active_file.write_all(&encoded).await {
                                Ok(()) => {
                                    current_size += record_len;
                                    current_global_offset += record_len;
                                    pending_acks.push((ack, current_global_offset));
                                }
                                Err(e) => {
                                    let err_copy = io::Error::new(e.kind(), e.to_string());
                                    write_err = Some(e);
                                    let _ = ack.send(Err(SpoolError::Io(err_copy)));
                                }
                            }
                        }
                        Command::Flush { ack } => {
                            pending_flushes.push(ack);
                        }
                    }
                }

                if write_err.is_none()
                    && (!pending_acks.is_empty() || !pending_flushes.is_empty())
                    && config.fsync_mode == FsyncMode::Always
                {
                    if let Err(e) = active_file.sync_all().await {
                        let err_msg = e.to_string();
                        let kind = e.kind();
                        for (ack, _) in pending_acks {
                            let _ = ack
                                .send(Err(SpoolError::Io(io::Error::new(kind, err_msg.clone()))));
                        }
                        for ack in pending_flushes {
                            let _ = ack
                                .send(Err(SpoolError::Io(io::Error::new(kind, err_msg.clone()))));
                        }
                        continue;
                    }
                    fsync_count_worker.fetch_add(1, Ordering::SeqCst);
                }

                for (ack, offset) in pending_acks {
                    let _ = ack.send(Ok(offset));
                }
                for ack in pending_flushes {
                    let _ = ack.send(Ok(()));
                }
            }

            if config.fsync_mode == FsyncMode::Always {
                let _ = active_file.sync_all().await;
            }
        });

        Ok(Self {
            tx,
            fsync_count,
            dir,
        })
    }

    pub async fn write(&self, payload: impl AsRef<[u8]>) -> Result<u64, SpoolError> {
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(Command::Write {
                payload: payload.as_ref().to_vec(),
                ack: ack_tx,
            })
            .await
            .map_err(|_| SpoolError::ChannelClosed)?;
        ack_rx.await.map_err(|_| SpoolError::ChannelClosed)?
    }

    pub async fn flush(&self) -> Result<(), SpoolError> {
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(Command::Flush { ack: ack_tx })
            .await
            .map_err(|_| SpoolError::ChannelClosed)?;
        ack_rx.await.map_err(|_| SpoolError::ChannelClosed)?
    }

    pub fn fsync_count(&self) -> u64 {
        self.fsync_count.load(Ordering::SeqCst)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn checkpoint(&self, offset: u64) -> Result<(), SpoolError> {
        write_checkpoint(&self.dir, offset)
    }

    pub fn read_checkpoint(&self) -> Result<u64, SpoolError> {
        read_checkpoint(&self.dir)
    }

    pub fn gc(&self, checkpoint: u64) -> Result<usize, SpoolError> {
        delete_segments_below(&self.dir, checkpoint)
    }

    pub fn replay(&self, from_offset: u64) -> Result<ReplayIter, SpoolError> {
        replay(&self.dir, from_offset)
    }

    pub fn replay_from_checkpoint(&self) -> Result<ReplayIter, SpoolError> {
        replay_from_checkpoint(&self.dir)
    }
}
