use std::{
    error::Error,
    fmt::Display,
    fs::{self, File},
    io::{self, BufWriter, Write},
    sync::atomic::{self, AtomicU64},
    sync::Arc,
};

use crc::{CRC_32_CKSUM, Crc};

use super::{
    db_config::DBConfig,
    entry::{Entry, LogEntry},
    file_utils,
};

const CRC32: Crc<u32> = Crc::<u32>::new(&CRC_32_CKSUM);

const ENTRY_TYPE_DEL: u8 = 0;
const ENTRY_TYPE_PUT: u8 = 1;

/////////////////////////////////////////

#[derive(Debug)]
pub enum WALError {
    IOError(io::Error),
    TruncatedRecord,
    ChecksumMismatch,
    DecodeError(String),
}

impl Display for WALError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IOError(e) => write!(f, "WALError::IOError: {}", e),
            Self::TruncatedRecord => write!(f, "WALError::TruncatedRecord"),
            Self::ChecksumMismatch => write!(f, "WALError::ChecksumMismatch"),
            Self::DecodeError(msg) => write!(f, "WALError::DecodeError: {}", msg),
        }
    }
}

impl Error for WALError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::IOError(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for WALError {
    fn from(value: io::Error) -> Self {
        Self::IOError(value)
    }
}

/////////////////////////////////////////

/// An open WAL file ready for appending.
pub(crate) struct WALFile {
    pub(crate) id: u64,
    file: BufWriter<File>,
}

impl WALFile {
    /// Append a log entry as a WAL record.
    ///
    /// Record layout (all little-endian):
    /// ```text
    /// [length: u32]       = 8 + 1 + 4 + key.len() + 4 + val.len()
    /// [log_seq_num: u64]
    /// [entry_type: u8]    1=Put, 0=Del
    /// [key_len: u32]
    /// [key bytes]
    /// [val_len: u32]
    /// [val bytes]
    /// [crc32: u32]        over the entire record up to (not including) the crc
    /// ```
    pub(crate) fn append(&mut self, entry: &Arc<LogEntry>) -> Result<(), WALError> {
        let (entry_type, key, val) = match &entry.entry {
            Entry::Put { key, val } => (ENTRY_TYPE_PUT, key.as_slice(), val.as_slice()),
            Entry::Del { key } => (ENTRY_TYPE_DEL, key.as_slice(), &[][..]),
            Entry::Empty => return Ok(()),
        };

        let length: u32 = (8 + 1 + 4 + key.len() + 4 + val.len()) as u32;

        let mut record: Vec<u8> = Vec::with_capacity(4 + length as usize + 4);
        record.extend_from_slice(&length.to_le_bytes());
        record.extend_from_slice(&entry.log_seq_num.to_le_bytes());
        record.push(entry_type);
        record.extend_from_slice(&(key.len() as u32).to_le_bytes());
        record.extend_from_slice(key);
        record.extend_from_slice(&(val.len() as u32).to_le_bytes());
        record.extend_from_slice(val);

        let crc = CRC32.checksum(&record);
        record.extend_from_slice(&crc.to_le_bytes());

        self.file.write_all(&record)?;
        Ok(())
    }

    /// Flush the write buffer and sync to disk for durability.
    pub(crate) fn sync(&mut self) -> Result<(), WALError> {
        self.file.flush()?;
        self.file.get_ref().sync_all()?;
        Ok(())
    }
}

/////////////////////////////////////////

pub struct WAL {
    pub(crate) log_sequence_num: AtomicU64,
    /// Shared with `DBInner` so WAL rotation can happen inside the mutex
    /// without an extra lock or reference to `DBBackend`.
    pub(crate) file_sequence_num: Arc<AtomicU64>,
}

impl WAL {
    pub fn new(file_sequence_num: Arc<AtomicU64>) -> WAL {
        WAL {
            log_sequence_num: AtomicU64::new(0),
            file_sequence_num,
        }
    }

    pub fn incr_log_sequence_num(&self) -> u64 {
        self.log_sequence_num.fetch_add(1, atomic::Ordering::SeqCst)
    }

    pub fn incr_file_sequence_num(&self) -> u64 {
        self.file_sequence_num.fetch_add(1, atomic::Ordering::SeqCst)
    }

    /// Open a new WAL file and return it. Does not affect atomic counters.
    pub(crate) fn rotate(wal_id: u64, config: &DBConfig) -> Result<WALFile, WALError> {
        let path = file_utils::wal_path(config, wal_id);
        let file = File::create_new(path)?;
        Ok(WALFile {
            id: wal_id,
            file: BufWriter::new(file),
        })
    }

    /// Delete the WAL file with the given id from disk.
    pub(crate) fn delete_wal(config: &DBConfig, wal_id: u64) -> Result<(), WALError> {
        let path = file_utils::wal_path(config, wal_id);
        match fs::remove_file(path) {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()), // already gone is fine
            Err(e) => Err(WALError::IOError(e)),
        }
    }

    /// Scan `data_dir` for `*.wal` files, parse each one, and return
    /// `(wal_id, entries)` pairs sorted by `wal_id` ascending.
    ///
    /// A CRC mismatch on the last record of a file is treated as a partial
    /// write (process crashed mid-write); all prior records in that file are
    /// still returned. A CRC mismatch mid-file logs and skips remaining records.
    pub(crate) fn recover(
        config: &DBConfig,
    ) -> Result<Vec<(u64, Vec<Arc<LogEntry>>)>, WALError> {
        let data_dir = config.data_dir();
        let read_dir = match fs::read_dir(&data_dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(WALError::IOError(e)),
        };

        let mut wal_ids: Vec<u64> = Vec::new();
        for entry in read_dir {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("wal") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if let Ok(id) = stem.parse::<u64>() {
                        wal_ids.push(id);
                    }
                }
            }
        }
        wal_ids.sort_unstable();

        let mut result = Vec::new();
        for wal_id in wal_ids {
            let path = file_utils::wal_path(config, wal_id);
            let data = match fs::read(&path) {
                Ok(d) => d,
                Err(_) => continue,
            };

            let entries = Self::parse_wal_file(&data);
            result.push((wal_id, entries));
        }
        Ok(result)
    }

    /// Parse all valid records from raw WAL file bytes.
    /// Stops at the first CRC mismatch or truncation (partial-write tail).
    fn parse_wal_file(data: &[u8]) -> Vec<Arc<LogEntry>> {
        let mut entries = Vec::new();
        let mut pos = 0;

        while pos < data.len() {
            match Self::read_wal_record(data, pos) {
                Ok((entry, new_pos)) => {
                    entries.push(entry);
                    pos = new_pos;
                }
                Err(_) => break, // truncated or bad CRC — stop here
            }
        }
        entries
    }

    /// Attempt to decode one WAL record at `pos`. Returns `(entry, new_pos)` on success.
    fn read_wal_record(data: &[u8], pos: usize) -> Result<(Arc<LogEntry>, usize), WALError> {
        // Need at least: 4 (length) + 4 (crc) = 8 bytes
        if pos + 8 > data.len() {
            return Err(WALError::TruncatedRecord);
        }

        let length = u32::from_le_bytes(
            data[pos..pos + 4]
                .try_into()
                .map_err(|_| WALError::TruncatedRecord)?,
        ) as usize;

        let record_end = pos + 4 + length;
        if record_end + 4 > data.len() {
            return Err(WALError::TruncatedRecord);
        }

        let record_body = &data[pos..record_end]; // includes the length field
        let stored_crc = u32::from_le_bytes(
            data[record_end..record_end + 4]
                .try_into()
                .map_err(|_| WALError::TruncatedRecord)?,
        );

        let computed_crc = CRC32.checksum(record_body);
        if computed_crc != stored_crc {
            return Err(WALError::ChecksumMismatch);
        }

        // Parse the record body (skip the 4-byte length prefix)
        let body = &data[pos + 4..record_end];
        let mut bpos = 0;

        let log_seq_num = Self::read_u64(body, &mut bpos)?;
        let entry_type = Self::read_u8(body, &mut bpos)?;
        let key = Self::read_bytes(body, &mut bpos)?;
        let val = Self::read_bytes(body, &mut bpos)?;

        let entry = match entry_type {
            ENTRY_TYPE_PUT => Entry::Put { key, val },
            ENTRY_TYPE_DEL => Entry::Del { key },
            _ => return Err(WALError::DecodeError(format!("unknown entry type {}", entry_type))),
        };

        Ok((
            Arc::new(LogEntry::new(entry, log_seq_num)),
            record_end + 4,
        ))
    }

    fn read_u64(data: &[u8], pos: &mut usize) -> Result<u64, WALError> {
        if *pos + 8 > data.len() {
            return Err(WALError::TruncatedRecord);
        }
        let val = u64::from_le_bytes(
            data[*pos..*pos + 8]
                .try_into()
                .map_err(|_| WALError::TruncatedRecord)?,
        );
        *pos += 8;
        Ok(val)
    }

    fn read_u8(data: &[u8], pos: &mut usize) -> Result<u8, WALError> {
        if *pos >= data.len() {
            return Err(WALError::TruncatedRecord);
        }
        let val = data[*pos];
        *pos += 1;
        Ok(val)
    }

    fn read_bytes(data: &[u8], pos: &mut usize) -> Result<Vec<u8>, WALError> {
        if *pos + 4 > data.len() {
            return Err(WALError::TruncatedRecord);
        }
        let len = u32::from_le_bytes(
            data[*pos..*pos + 4]
                .try_into()
                .map_err(|_| WALError::TruncatedRecord)?,
        ) as usize;
        *pos += 4;
        if *pos + len > data.len() {
            return Err(WALError::TruncatedRecord);
        }
        let bytes = data[*pos..*pos + len].to_vec();
        *pos += len;
        Ok(bytes)
    }
}
