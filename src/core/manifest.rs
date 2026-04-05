use std::{
    collections::{BTreeMap, HashSet},
    error::Error,
    fmt::Display,
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    path::PathBuf,
    sync::Arc,
};

use crc::{CRC_32_CKSUM, Crc};

use super::db_context::DBContext;

const CRC32: Crc<u32> = Crc::<u32>::new(&CRC_32_CKSUM);

const RECORD_FLUSH_MEMTABLE: u8 = 1;
const RECORD_COMPACT_FILES: u8 = 2;

/////////////////////////////////////////

#[derive(Clone, Debug)]
pub struct SSTableMeta {
    pub(crate) min_key: Vec<u8>,
    pub(crate) max_key: Vec<u8>,
    pub(crate) file_size: u64,
}

impl SSTableMeta {
    fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.file_size.to_le_bytes());
        buf.extend_from_slice(&(self.min_key.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.min_key);
        buf.extend_from_slice(&(self.max_key.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.max_key);
        buf
    }

    fn decode(data: &[u8], pos: &mut usize) -> Result<SSTableMeta, ManifestError> {
        let file_size = read_u64(data, pos)?;
        let min_key = read_bytes(data, pos)?;
        let max_key = read_bytes(data, pos)?;
        Ok(SSTableMeta {
            min_key,
            max_key,
            file_size,
        })
    }
}

/////////////////////////////////////////

#[derive(Clone, Debug)]
pub struct ManifestLogFlushMemtable {
    pub(crate) level: usize,
    pub(crate) file_num: u64,
    pub(crate) min_key: Vec<u8>,
    pub(crate) max_key: Vec<u8>,
    pub(crate) file_size: u64,
}

impl ManifestLogFlushMemtable {
    fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&(self.level as u64).to_le_bytes());
        buf.extend_from_slice(&self.file_num.to_le_bytes());
        let meta = SSTableMeta {
            min_key: self.min_key.clone(),
            max_key: self.max_key.clone(),
            file_size: self.file_size,
        };
        buf.extend_from_slice(&meta.encode());
        buf
    }

    fn decode(data: &[u8]) -> Result<ManifestLogFlushMemtable, ManifestError> {
        let mut pos = 0;
        let level = read_u64(data, &mut pos)? as usize;
        let file_num = read_u64(data, &mut pos)?;
        let meta = SSTableMeta::decode(data, &mut pos)?;
        Ok(ManifestLogFlushMemtable {
            level,
            file_num,
            min_key: meta.min_key,
            max_key: meta.max_key,
            file_size: meta.file_size,
        })
    }
}

#[derive(Clone, Debug)]
pub struct ManifestLogCompactFiles {
    pub(crate) from_level: usize,
    pub(crate) from_level_file_nums: Vec<u64>,
    pub(crate) to_level: usize,
    pub(crate) to_level_file_nums: Vec<u64>,
    pub(crate) output_file_nums: Vec<u64>,
    pub(crate) output_file_metas: Vec<SSTableMeta>,
}

impl ManifestLogCompactFiles {
    fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&(self.from_level as u64).to_le_bytes());
        buf.extend_from_slice(&(self.from_level_file_nums.len() as u64).to_le_bytes());
        for &n in &self.from_level_file_nums {
            buf.extend_from_slice(&n.to_le_bytes());
        }
        buf.extend_from_slice(&(self.to_level as u64).to_le_bytes());
        buf.extend_from_slice(&(self.to_level_file_nums.len() as u64).to_le_bytes());
        for &n in &self.to_level_file_nums {
            buf.extend_from_slice(&n.to_le_bytes());
        }
        buf.extend_from_slice(&(self.output_file_nums.len() as u64).to_le_bytes());
        for (file_num, meta) in self.output_file_nums.iter().zip(self.output_file_metas.iter()) {
            buf.extend_from_slice(&file_num.to_le_bytes());
            buf.extend_from_slice(&meta.encode());
        }
        buf
    }

    fn decode(data: &[u8]) -> Result<ManifestLogCompactFiles, ManifestError> {
        let mut pos = 0;
        let from_level = read_u64(data, &mut pos)? as usize;
        let from_count = read_u64(data, &mut pos)? as usize;
        let mut from_level_file_nums = Vec::with_capacity(from_count);
        for _ in 0..from_count {
            from_level_file_nums.push(read_u64(data, &mut pos)?);
        }
        let to_level = read_u64(data, &mut pos)? as usize;
        let to_removed_count = read_u64(data, &mut pos)? as usize;
        let mut to_level_file_nums = Vec::with_capacity(to_removed_count);
        for _ in 0..to_removed_count {
            to_level_file_nums.push(read_u64(data, &mut pos)?);
        }
        let output_count = read_u64(data, &mut pos)? as usize;
        let mut output_file_nums = Vec::with_capacity(output_count);
        let mut output_file_metas = Vec::with_capacity(output_count);
        for _ in 0..output_count {
            let file_num = read_u64(data, &mut pos)?;
            let meta = SSTableMeta::decode(data, &mut pos)?;
            output_file_nums.push(file_num);
            output_file_metas.push(meta);
        }
        Ok(ManifestLogCompactFiles {
            from_level,
            from_level_file_nums,
            to_level,
            to_level_file_nums,
            output_file_nums,
            output_file_metas,
        })
    }
}

#[derive(Clone, Debug)]
pub enum ManifestLog {
    FlushMemtable(ManifestLogFlushMemtable),
    CompactFiles(ManifestLogCompactFiles),
}

/////////////////////////////////////////

#[derive(Debug)]
pub enum ManifestError {
    IOError(io::Error),
    TruncatedRecord,
    ChecksumMismatch,
    UnknownRecordType(u8),
    DecodeError(String),
}

impl Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IOError(e) => write!(f, "IOError: {}", e),
            Self::TruncatedRecord => write!(f, "TruncatedRecord"),
            Self::ChecksumMismatch => write!(f, "ChecksumMismatch"),
            Self::UnknownRecordType(t) => write!(f, "UnknownRecordType: {}", t),
            Self::DecodeError(msg) => write!(f, "DecodeError: {}", msg),
        }
    }
}

impl Error for ManifestError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::IOError(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for ManifestError {
    fn from(value: io::Error) -> Self {
        Self::IOError(value)
    }
}

/////////////////////////////////////////

#[derive(Clone)]
pub struct SSTableLevel {
    pub(crate) sstables: BTreeMap<u64, SSTableMeta>,
}

/**
 * The manifest keeps track of the data pages on disk. Whenever
 * a new file is added by flushing the change will be recorded
 * to the manifest as a change. When a compaction happens we
 * remove files from one level and add them to an upper level.
 * Each operation happens transactionally so the SSTableVersion
 * reference is updated in place with the new SSTableVersion.
 * Current holders of the SSTableVersion don't see the change.
 *
 * In summary the manifest will record the "changes" that
 * lead to SSTableVersion's current state.
 */
pub struct Manifest {
    file: File,
    path: PathBuf,

    flushing_memtables: HashSet<usize>,
    sstable_version: Arc<SSTableVersion>,
}

impl Manifest {
    pub fn new(db_context: Arc<DBContext>) -> Result<Manifest, ManifestError> {
        let config_num_levels = db_context.config().manifest_num_levels();
        let mut sstable_levels: Vec<SSTableLevel> = (0..config_num_levels)
            .map(|_| SSTableLevel {
                sstables: BTreeMap::new(),
            })
            .collect();

        let mut manifest_path = db_context.config().data_dir();
        manifest_path.push("manifest.dat");

        // Open in append mode (creates if not exists). Replays existing records on re-open.
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&manifest_path)?;

        // Replay existing log to recover sstable_version
        if let Ok(mut read_file) = File::open(&manifest_path) {
            let mut data = Vec::new();
            read_file.read_to_end(&mut data).unwrap_or(0);

            let mut pos = 0;
            while pos < data.len() {
                match Self::read_log_record(&data, &mut pos) {
                    Ok(Some(log)) => Self::apply_log(&mut sstable_levels, &log),
                    Ok(None) => break,
                    Err(_) => break, // truncated or corrupt tail — stop
                }
            }
        }

        Ok(Manifest {
            file,
            path: manifest_path,
            flushing_memtables: HashSet::new(),
            sstable_version: Arc::new(SSTableVersion { sstable_levels }),
        })
    }

    pub fn sstable_version(&self) -> Arc<SSTableVersion> {
        self.sstable_version.clone()
    }

    pub fn add_flushing_memtable(&mut self, idx: usize) {
        self.flushing_memtables.insert(idx);
    }

    pub fn finalize_flushing_memtable(
        &mut self,
        memtable_id: usize,
        file_num: u64,
        meta: SSTableMeta,
    ) -> Result<(), ManifestError> {
        let log = ManifestLog::FlushMemtable(ManifestLogFlushMemtable {
            level: 0,
            file_num,
            min_key: meta.min_key.clone(),
            max_key: meta.max_key.clone(),
            file_size: meta.file_size,
        });
        self.write_log_record(&log)?;

        let mut sv = self.sstable_version.duplicate();
        sv.sstable_levels
            .get_mut(0)
            .expect("expected level 0")
            .sstables
            .insert(file_num, meta);

        self.sstable_version = Arc::new(sv);
        self.flushing_memtables.remove(&memtable_id);
        Ok(())
    }

    pub fn apply_compaction(
        &mut self,
        from_level: usize,
        from_file_nums: Vec<u64>,
        to_level: usize,
        to_file_nums: Vec<u64>,
        output_file_nums: Vec<u64>,
        output_metas: Vec<SSTableMeta>,
    ) -> Result<(), ManifestError> {
        let log = ManifestLog::CompactFiles(ManifestLogCompactFiles {
            from_level,
            from_level_file_nums: from_file_nums.clone(),
            to_level,
            to_level_file_nums: to_file_nums.clone(),
            output_file_nums: output_file_nums.clone(),
            output_file_metas: output_metas.clone(),
        });
        self.write_log_record(&log)?;

        let mut sv = self.sstable_version.duplicate();
        for file_num in &from_file_nums {
            sv.sstable_levels[from_level].sstables.remove(file_num);
        }
        for file_num in &to_file_nums {
            sv.sstable_levels[to_level].sstables.remove(file_num);
        }
        for (file_num, meta) in output_file_nums.iter().zip(output_metas.into_iter()) {
            sv.sstable_levels[to_level].sstables.insert(*file_num, meta);
        }

        self.sstable_version = Arc::new(sv);
        Ok(())
    }

    fn write_log_record(&mut self, log: &ManifestLog) -> Result<(), ManifestError> {
        let (record_type, payload) = match log {
            ManifestLog::FlushMemtable(r) => (RECORD_FLUSH_MEMTABLE, r.encode()),
            ManifestLog::CompactFiles(r) => (RECORD_COMPACT_FILES, r.encode()),
        };

        let payload_len = payload.len() as u32;
        let mut to_checksum = vec![record_type];
        to_checksum.extend_from_slice(&payload_len.to_le_bytes());
        to_checksum.extend_from_slice(&payload);
        let crc = CRC32.checksum(&to_checksum);

        self.file.write_all(&[record_type])?;
        self.file.write_all(&payload_len.to_le_bytes())?;
        self.file.write_all(&payload)?;
        self.file.write_all(&crc.to_le_bytes())?;
        self.file.sync_all()?;

        Ok(())
    }

    /// Deserialise a single log record starting at `pos`. Advances `pos` past the record.
    /// Returns `None` at EOF. Returns `Err` on truncation or checksum mismatch.
    pub(crate) fn read_log_record(
        data: &[u8],
        pos: &mut usize,
    ) -> Result<Option<ManifestLog>, ManifestError> {
        if *pos >= data.len() {
            return Ok(None);
        }
        // Minimum record: 1 (type) + 4 (payload_len) + 4 (crc) = 9 bytes
        if *pos + 9 > data.len() {
            return Err(ManifestError::TruncatedRecord);
        }

        let record_type = data[*pos];
        let payload_len = u32::from_le_bytes(
            data[*pos + 1..*pos + 5]
                .try_into()
                .map_err(|_| ManifestError::TruncatedRecord)?,
        ) as usize;

        if *pos + 5 + payload_len + 4 > data.len() {
            return Err(ManifestError::TruncatedRecord);
        }

        let payload = &data[*pos + 5..*pos + 5 + payload_len];
        let stored_crc = u32::from_le_bytes(
            data[*pos + 5 + payload_len..*pos + 5 + payload_len + 4]
                .try_into()
                .map_err(|_| ManifestError::TruncatedRecord)?,
        );

        // Verify CRC over: record_type + payload_len_bytes + payload
        let mut to_checksum = vec![record_type];
        to_checksum.extend_from_slice(&(payload_len as u32).to_le_bytes());
        to_checksum.extend_from_slice(payload);
        let computed_crc = CRC32.checksum(&to_checksum);

        if computed_crc != stored_crc {
            return Err(ManifestError::ChecksumMismatch);
        }

        *pos += 5 + payload_len + 4;

        let log = match record_type {
            RECORD_FLUSH_MEMTABLE => {
                ManifestLog::FlushMemtable(ManifestLogFlushMemtable::decode(payload)?)
            }
            RECORD_COMPACT_FILES => {
                ManifestLog::CompactFiles(ManifestLogCompactFiles::decode(payload)?)
            }
            _ => return Err(ManifestError::UnknownRecordType(record_type)),
        };

        Ok(Some(log))
    }

    fn apply_log(levels: &mut Vec<SSTableLevel>, log: &ManifestLog) {
        match log {
            ManifestLog::FlushMemtable(r) => {
                if let Some(level) = levels.get_mut(r.level) {
                    level.sstables.insert(
                        r.file_num,
                        SSTableMeta {
                            min_key: r.min_key.clone(),
                            max_key: r.max_key.clone(),
                            file_size: r.file_size,
                        },
                    );
                }
            }
            ManifestLog::CompactFiles(r) => {
                if let Some(from_level) = levels.get_mut(r.from_level) {
                    for file_num in &r.from_level_file_nums {
                        from_level.sstables.remove(file_num);
                    }
                }
                if let Some(to_level) = levels.get_mut(r.to_level) {
                    for file_num in &r.to_level_file_nums {
                        to_level.sstables.remove(file_num);
                    }
                    for (file_num, meta) in r
                        .output_file_nums
                        .iter()
                        .zip(r.output_file_metas.iter())
                    {
                        to_level.sstables.insert(*file_num, meta.clone());
                    }
                }
            }
        }
    }
}

/////////////////////////////////////////

#[derive(Clone)]
pub struct SSTableVersion {
    pub(crate) sstable_levels: Vec<SSTableLevel>,
}

impl SSTableVersion {
    fn duplicate(&self) -> Self {
        Self {
            sstable_levels: self.sstable_levels.clone(),
        }
    }
    pub fn levels(&self) -> Vec<usize> {
        self.sstable_levels
            .iter()
            .enumerate()
            .map(|(idx, _)| idx)
            .collect()
    }
}

impl Default for SSTableVersion {
    fn default() -> Self {
        SSTableVersion {
            sstable_levels: Vec::new(),
        }
    }
}

/////////////////////////////////////////

/// Helper: read a u64 from `data` at `pos`, advancing `pos` by 8.
fn read_u64(data: &[u8], pos: &mut usize) -> Result<u64, ManifestError> {
    if *pos + 8 > data.len() {
        return Err(ManifestError::DecodeError("unexpected EOF reading u64".into()));
    }
    let val = u64::from_le_bytes(
        data[*pos..*pos + 8]
            .try_into()
            .map_err(|_| ManifestError::DecodeError("u64 slice error".into()))?,
    );
    *pos += 8;
    Ok(val)
}

/// Helper: read a length-prefixed byte slice (u32 len then bytes).
fn read_bytes(data: &[u8], pos: &mut usize) -> Result<Vec<u8>, ManifestError> {
    if *pos + 4 > data.len() {
        return Err(ManifestError::DecodeError(
            "unexpected EOF reading bytes length".into(),
        ));
    }
    let len = u32::from_le_bytes(
        data[*pos..*pos + 4]
            .try_into()
            .map_err(|_| ManifestError::DecodeError("u32 slice error".into()))?,
    ) as usize;
    *pos += 4;
    if *pos + len > data.len() {
        return Err(ManifestError::DecodeError(
            "unexpected EOF reading bytes body".into(),
        ));
    }
    let bytes = data[*pos..*pos + len].to_vec();
    *pos += len;
    Ok(bytes)
}
