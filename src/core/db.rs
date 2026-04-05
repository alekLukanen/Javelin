use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::fmt::Display;
use std::fs;
use std::sync::{Arc, Mutex, atomic};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use super::block_cache::{BlockCache, BlockCacheError};
use super::db_context::DBContext;
use super::entry::{Entry, LogEntry};
use super::manifest::{Manifest, SSTableLevel, SSTableMeta, SSTableVersion};
use super::memory_manager::MemoryManager;
use super::memtable::{ImmutableMemtable, ImmutableMemtableError, Memtable, MemtableError};
use super::sstable_builder::SSTableBuilder;
use super::sstable_writer::{SSTableWriter, SSTableWriterError};
use super::wal::{WAL, WALError, WALFile};
use crate::core::db_config::DBConfig;
use crate::core::file_utils;
use crate::core::iterator::{IteratorError, SourceIterator};
use crate::core::manifest::ManifestError;
use crate::core::merge_sort_iterator::MergeSortIterator;

#[derive(Debug)]
pub enum DBError {
    MemtableError(MemtableError),
    ImmutableMemtableError(ImmutableMemtableError),
    SSTableWriterError(SSTableWriterError),
    BlockCacheError(BlockCacheError),
    MutexLockFailed(String),
    IteratorError(IteratorError),
    ManifestError(ManifestError),
    WALError(WALError),
}

impl Display for DBError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DBError::MemtableError(e) => write!(f, "MemtableError: {}", e),
            DBError::ImmutableMemtableError(e) => write!(f, "ImmutableMemtableError: {}", e),
            DBError::SSTableWriterError(e) => write!(f, "SSTableWriterError: {}", e),
            DBError::BlockCacheError(e) => write!(f, "BlockCacheError: {}", e),
            DBError::MutexLockFailed(v) => write!(f, "MutexLockFailed: {}", v),
            DBError::IteratorError(e) => write!(f, "IteratorError: {}", e),
            DBError::ManifestError(e) => write!(f, "ManifestError: {}", e),
            DBError::WALError(e) => write!(f, "WALError: {}", e),
        }
    }
}

impl Error for DBError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            DBError::MemtableError(e) => Some(e),
            DBError::ImmutableMemtableError(e) => Some(e),
            DBError::SSTableWriterError(e) => Some(e),
            DBError::BlockCacheError(e) => Some(e),
            DBError::MutexLockFailed(_) => None,
            DBError::IteratorError(e) => Some(e),
            DBError::ManifestError(e) => Some(e),
            DBError::WALError(e) => Some(e),
        }
    }
}

impl<T> From<std::sync::PoisonError<std::sync::MutexGuard<'_, T>>> for DBError {
    fn from(value: std::sync::PoisonError<std::sync::MutexGuard<'_, T>>) -> Self {
        DBError::MutexLockFailed(value.to_string())
    }
}

impl From<MemtableError> for DBError {
    fn from(value: MemtableError) -> Self {
        DBError::MemtableError(value)
    }
}

impl From<ImmutableMemtableError> for DBError {
    fn from(value: ImmutableMemtableError) -> Self {
        DBError::ImmutableMemtableError(value)
    }
}

impl From<SSTableWriterError> for DBError {
    fn from(value: SSTableWriterError) -> Self {
        DBError::SSTableWriterError(value)
    }
}

impl From<BlockCacheError> for DBError {
    fn from(value: BlockCacheError) -> Self {
        DBError::BlockCacheError(value)
    }
}

impl From<IteratorError> for DBError {
    fn from(value: IteratorError) -> Self {
        DBError::IteratorError(value)
    }
}

impl From<ManifestError> for DBError {
    fn from(value: ManifestError) -> Self {
        DBError::ManifestError(value)
    }
}

impl From<WALError> for DBError {
    fn from(value: WALError) -> Self {
        DBError::WALError(value)
    }
}

////////////////////////////////////////////

#[derive(Clone)]
pub struct ReadState {
    pub(crate) memtables: Vec<Arc<ImmutableMemtable>>,
    pub(crate) sstable_version: Arc<SSTableVersion>,
}

impl ReadState {
    fn duplicate(&self) -> ReadState {
        ReadState {
            memtables: self.memtables.clone(),
            sstable_version: self.sstable_version.clone(),
        }
    }
}

pub struct DB {
    db_backend: Arc<DBBackend>,
    maintainer_handle: Mutex<Option<JoinHandle<()>>>,

    memory: Arc<MemoryManager>,
    db_context: Arc<DBContext>,
}

impl Drop for DB {
    fn drop(&mut self) {
        self.db_backend.shutdown.store(true, Ordering::Relaxed);
        if let Ok(mut guard) = self.maintainer_handle.lock() {
            if let Some(handle) = guard.take() {
                handle.join().ok();
            }
        }
    }
}

impl DB {
    pub fn new(config: DBConfig) -> Result<DB, DBError> {
        let db_context = Arc::new(DBContext::new(config));
        let memory = Arc::new(MemoryManager::new(db_context.clone()));

        // Manifest::new already replays the on-disk log for recovery.
        let manifest = Manifest::new(db_context.clone())?;
        // Shared counter between WAL and DBInner so WAL rotation can happen inside the lock.
        let shared_file_seq = Arc::new(atomic::AtomicU64::new(0));
        let wal = WAL::new(shared_file_seq.clone());

        // --- WAL Recovery ---
        // Replay any surviving WAL files (data not yet flushed to SSTables).
        let recovered_wals = WAL::recover(db_context.config())?;

        // Determine the maximum file_num already in use (manifest + WAL files)
        // so we can start the file_sequence_num counter safely above all of them.
        let max_manifest_file_num = manifest
            .sstable_version()
            .sstable_levels
            .iter()
            .flat_map(|l| l.sstables.keys().copied())
            .max()
            .unwrap_or(0);

        let max_wal_id = recovered_wals.iter().map(|(id, _)| *id).max().unwrap_or(0);

        let max_log_seq_num = recovered_wals
            .iter()
            .flat_map(|(_, entries)| entries.iter().map(|e| e.log_seq_num))
            .max()
            .unwrap_or(0);

        // Restore counters so new sequences don't collide with recovered data.
        let next_file_num = max_manifest_file_num.max(max_wal_id) + 1;
        shared_file_seq.store(next_file_num, atomic::Ordering::SeqCst);
        if max_log_seq_num > 0 {
            wal.log_sequence_num
                .store(max_log_seq_num + 1, atomic::Ordering::SeqCst);
        }

        // Build recovered ImmutableMemtables from each WAL file.
        // Each WAL file's entries are replayed in order into a Memtable,
        // then frozen to an ImmutableMemtable.
        let mut recovered_immutable_memtables: Vec<Arc<ImmutableMemtable>> = Vec::new();
        let mut memtable_wal_ids: HashMap<usize, u64> = HashMap::new();
        let mut im_id_counter: usize = 0;

        for (wal_id, entries) in &recovered_wals {
            if entries.is_empty() {
                // Empty WAL (e.g. process crashed right after rotation) — clean it up.
                let _ = WAL::delete_wal(db_context.config(), *wal_id);
                continue;
            }
            let recovery_mt = Arc::new(Memtable::new(
                db_context.clone(),
                memory.clone(),
                db_context.config().memory_manager_max_memtable_memory_usage(),
            ));
            for entry in entries {
                recovery_mt.insert(entry.clone())?;
            }
            let im_id = im_id_counter;
            im_id_counter += 1;
            let im = Arc::new(ImmutableMemtable::new(im_id, db_context.clone(), recovery_mt)?);
            memtable_wal_ids.insert(im_id, *wal_id);
            recovered_immutable_memtables.push(im);
        }

        // Create the initial WAL file for the new active memtable.
        let initial_wal_id = wal.incr_file_sequence_num();
        let initial_wal = WAL::rotate(initial_wal_id, db_context.config())?;

        let sstable_version = manifest.sstable_version();

        let shutdown = Arc::new(AtomicBool::new(false));
        let db_backend = Arc::new(DBBackend {
            db_inner: Mutex::new(DBInner {
                active_memtable: Arc::new(Memtable::new(
                    db_context.clone(),
                    memory.clone(),
                    db_context
                        .config()
                        .memory_manager_max_memtable_memory_usage(),
                )),
                active_wal: Some(initial_wal),
                memtable_wal_ids,
                file_sequence_num: shared_file_seq,
                read_state: Arc::new(ReadState {
                    memtables: recovered_immutable_memtables,
                    sstable_version,
                }),
                immutable_memtable_idx: atomic::AtomicUsize::new(im_id_counter),
                block_cache: Arc::new(BlockCache::new(db_context.clone(), memory.clone())),
                manifest,
                memory: memory.clone(),
                db_context: db_context.clone(),
            }),
            wal,
            db_context: db_context.clone(),
            shutdown: shutdown.clone(),
        });
        let db_backend_ref = db_backend.clone();
        let handle = std::thread::spawn(move || {
            let resp = DBBackend::maintainer(db_backend_ref.clone());
            if let Err(err) = resp {
                db_backend_ref
                    .db_context
                    .log_error(format!("[DB] {:?}", err));
            }
        });

        Ok(DB {
            db_backend,
            maintainer_handle: Mutex::new(Some(handle)),
            memory,
            db_context,
        })
    }

    pub fn close(&self) -> Result<(), DBError> {
        self.db_backend.db_inner.lock()?.block_cache.close();
        self.db_backend.shutdown.store(true, Ordering::Relaxed);
        if let Ok(mut guard) = self.maintainer_handle.lock() {
            if let Some(handle) = guard.take() {
                handle.join().ok();
            }
        }
        Ok(())
    }

    /// Expose the backend for tests that need to call compaction directly.
    pub(crate) fn backend(&self) -> Arc<DBBackend> {
        self.db_backend.clone()
    }

    /// Peek at the current log sequence number (useful for tests verifying counter recovery).
    pub(crate) fn current_log_seq_num(&self) -> u64 {
        self.db_backend
            .wal
            .log_sequence_num
            .load(atomic::Ordering::SeqCst)
    }

    pub fn get(&self, key: &Vec<u8>) -> Result<Option<Vec<u8>>, DBError> {
        let log_seq_num = self.db_backend.wal.incr_log_sequence_num();

        let inner_guard = self.db_backend.db_inner.lock()?;

        let active_memtable = inner_guard.active_memtable.clone();
        let read_state = inner_guard.get_read_state();
        let block_cache = inner_guard.block_cache.clone();

        drop(inner_guard);

        let mut iter = MergeSortIterator::new(
            self.db_context.clone(),
            active_memtable,
            read_state,
            block_cache,
            log_seq_num,
            Some(key.clone()),
            Some(key.clone()),
        );

        match SourceIterator::next(&mut iter)? {
            Some(entry) => match &entry.entry {
                Entry::Put { val, .. } => Ok(Some(val.clone())),
                Entry::Del { .. } => Ok(None),
                Entry::Empty => Ok(None),
            },
            None => Ok(None),
        }
    }

    pub fn set(&self, key: Vec<u8>, val: Vec<u8>) -> Result<(), DBError> {
        self.db_backend
            .db_inner
            .lock()?
            .insert(Arc::new(LogEntry::new(
                Entry::Put { key, val },
                self.db_backend.wal.incr_log_sequence_num(),
            )))?;
        Ok(())
    }

    pub fn delete(&self, key: Vec<u8>) -> Result<(), DBError> {
        self.db_backend
            .db_inner
            .lock()?
            .insert(Arc::new(LogEntry::new(
                Entry::Del { key },
                self.db_backend.wal.incr_log_sequence_num(),
            )))?;
        Ok(())
    }

    pub fn iterate(
        &self,
        lower_bound: Option<Vec<u8>>,
        upper_bound: Option<Vec<u8>>,
    ) -> Result<MergeSortIterator, DBError> {
        let log_seq_num = self.db_backend.wal.incr_log_sequence_num();

        let inner_guard = self.db_backend.db_inner.lock()?;

        let active_memtable = inner_guard.active_memtable.clone();
        let read_state = inner_guard.get_read_state();
        let block_cache = inner_guard.block_cache.clone();

        drop(inner_guard);

        let iter = MergeSortIterator::new(
            self.db_context.clone(),
            active_memtable,
            read_state,
            block_cache,
            log_seq_num,
            lower_bound,
            upper_bound,
        );

        Ok(iter)
    }
}

//////////////////////////////////////////

pub(crate) struct CompactionJob {
    pub(crate) from_level: usize,
    pub(crate) from_files: Vec<(u64, SSTableMeta)>,
    pub(crate) to_level: usize,
    pub(crate) to_files: Vec<(u64, SSTableMeta)>,
}

pub(crate) struct DBBackend {
    pub(crate) db_inner: Mutex<DBInner>,
    pub(crate) wal: WAL,
    pub(crate) db_context: Arc<DBContext>,
    pub(crate) shutdown: Arc<AtomicBool>,
}

impl DBBackend {
    /// Responsible for:
    /// - Flusing immutable memtables
    /// - Compacting SSTables when a layer gets too large
    fn maintainer(db_backend: Arc<DBBackend>) -> Result<(), DBError> {
        loop {
            if db_backend.shutdown.load(Ordering::Relaxed) {
                break Ok(());
            }

            // check for immutable memtables to flush
            match Self::flush_immutable_memtable(&db_backend) {
                Ok(_) => {}
                Err(err) => {
                    db_backend
                        .db_context
                        .log_error(format!("[DBBackend-maintainer] {:?}", err));
                }
            }

            // compact any over-full levels
            match Self::maybe_compact(&db_backend) {
                Ok(_) => {}
                Err(err) => {
                    db_backend
                        .db_context
                        .log_error(format!("[DBBackend-maintainer] {:?}", err));
                }
            }

            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn maybe_compact(db_backend: &Arc<DBBackend>) -> Result<(), DBError> {
        let sv = {
            let guard = db_backend.db_inner.lock()?;
            guard.manifest.sstable_version()
        };

        let Some(job) = Self::pick_compaction(&sv, db_backend.db_context.config()) else {
            return Ok(());
        };

        db_backend.db_context.log_info(format!(
            "[DBBackend] Compacting: level {} ({} files) -> level {} ({} to_files)",
            job.from_level,
            job.from_files.len(),
            job.to_level,
            job.to_files.len()
        ));

        Self::run_compaction(db_backend, job)
    }

    pub(crate) fn pick_compaction(sv: &SSTableVersion, config: &DBConfig) -> Option<CompactionJob> {
        let num_levels = sv.sstable_levels.len();

        // Level 0: compact when file count exceeds the trigger threshold.
        let l0 = &sv.sstable_levels[0];
        if l0.sstables.len() > config.compaction_level0_file_count_trigger() {
            if num_levels < 2 {
                return None;
            }
            let from_files: Vec<(u64, SSTableMeta)> = l0
                .sstables
                .iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect();

            // Find overlapping files at Level 1.
            let l1 = &sv.sstable_levels[1];
            let l0_min = from_files
                .iter()
                .map(|(_, m)| m.min_key.as_slice())
                .min()
                .unwrap_or(b"");
            let l0_max = from_files
                .iter()
                .map(|(_, m)| m.max_key.as_slice())
                .max()
                .unwrap_or(b"");
            let to_files: Vec<(u64, SSTableMeta)> = l1
                .sstables
                .iter()
                .filter(|(_, m)| {
                    m.min_key.as_slice() <= l0_max && m.max_key.as_slice() >= l0_min
                })
                .map(|(k, v)| (*k, v.clone()))
                .collect();

            return Some(CompactionJob {
                from_level: 0,
                from_files,
                to_level: 1,
                to_files,
            });
        }

        // Level i > 0: compact when total size exceeds the geometric size limit.
        for level_idx in 1..num_levels.saturating_sub(1) {
            let level = &sv.sstable_levels[level_idx];
            let total_size: u64 = level.sstables.values().map(|m| m.file_size).sum();
            let size_limit = config.compaction_level_size_base()
                * config
                    .compaction_level_size_multiplier()
                    .pow(level_idx as u32);

            if total_size > size_limit {
                // Pick the largest file in this level.
                let (largest_num, largest_meta) = level
                    .sstables
                    .iter()
                    .max_by_key(|(_, m)| m.file_size)
                    .map(|(k, v)| (*k, v.clone()))
                    .unwrap();

                // Find overlapping files at the next level.
                let next_level = &sv.sstable_levels[level_idx + 1];
                let to_files: Vec<(u64, SSTableMeta)> = next_level
                    .sstables
                    .iter()
                    .filter(|(_, m)| {
                        m.min_key.as_slice() <= largest_meta.max_key.as_slice()
                            && m.max_key.as_slice() >= largest_meta.min_key.as_slice()
                    })
                    .map(|(k, v)| (*k, v.clone()))
                    .collect();

                return Some(CompactionJob {
                    from_level: level_idx,
                    from_files: vec![(largest_num, largest_meta)],
                    to_level: level_idx + 1,
                    to_files,
                });
            }
        }

        None
    }

    pub(crate) fn run_compaction(db_backend: &Arc<DBBackend>, job: CompactionJob) -> Result<(), DBError> {
        let config = db_backend.db_context.config();

        // Snapshot shared resources without holding the lock during I/O.
        let (block_cache, memory, db_context) = {
            let guard = db_backend.db_inner.lock()?;
            (
                guard.block_cache.clone(),
                guard.memory.clone(),
                guard.db_context.clone(),
            )
        };

        // Build a fake SSTableVersion with all input files placed at level 0.
        // MergeSortIterator's level-0 path loads all files in the level, which
        // gives us a proper k-way merge over all compaction inputs.
        let mut fake_sstables: BTreeMap<u64, SSTableMeta> = BTreeMap::new();
        let from_file_nums: Vec<u64> = job.from_files.iter().map(|(n, _)| *n).collect();
        let to_file_nums: Vec<u64> = job.to_files.iter().map(|(n, _)| *n).collect();
        for (file_num, meta) in job.from_files.iter().chain(job.to_files.iter()) {
            fake_sstables.insert(*file_num, meta.clone());
        }
        let fake_sv = Arc::new(SSTableVersion {
            sstable_levels: vec![SSTableLevel {
                sstables: fake_sstables,
            }],
        });

        // Empty active memtable — compaction only merges SSTables.
        let empty_memtable = Arc::new(Memtable::new(
            db_context.clone(),
            memory.clone(),
            db_context
                .config()
                .memory_manager_max_memtable_memory_usage(),
        ));
        let fake_read_state = Arc::new(ReadState {
            memtables: Vec::new(),
            sstable_version: fake_sv,
        });

        // Drop tombstones when compacting into the bottom level.
        let skip_deletes = job.to_level + 1 == config.manifest_num_levels();

        let merge_iter = MergeSortIterator::new(
            db_context.clone(),
            empty_memtable,
            fake_read_state,
            block_cache.clone(),
            u64::MAX,
            None,
            None,
        );

        // Write the merged output into one SSTable file.
        let file_num = db_backend.wal.incr_file_sequence_num();
        let file_path = file_utils::sstable_path(config, file_num);
        let builder = SSTableBuilder::build_from_source_iterator(
            db_context.clone(),
            Box::new(merge_iter),
            skip_deletes,
        );
        let mut writer = SSTableWriter::new(db_context.clone(), builder, file_path.clone())?;

        let mut block_id: u32 = 0;
        loop {
            let Some(data_block) = writer.next_data_block()? else {
                break;
            };
            db_backend
                .db_inner
                .lock()?
                .block_cache
                .add_data_block(file_num, block_id, data_block)?;
            block_id += 1;
        }

        let mut output_file_nums: Vec<u64> = Vec::new();
        let mut output_metas: Vec<SSTableMeta> = Vec::new();

        if block_id > 0 {
            // Finalize the output file.
            let index_block = writer.index_block()?;
            let meta_block = writer.meta_data_block()?;
            let footer_block = writer.footer_block()?;

            db_backend.db_inner.lock()?.block_cache.add_sstable(
                file_num,
                footer_block,
                index_block,
                meta_block,
            )?;

            let meta = SSTableMeta {
                min_key: writer.first_key().unwrap_or_default(),
                max_key: writer.last_key().unwrap_or_default(),
                file_size: writer.total_bytes_written(),
            };
            output_file_nums.push(file_num);
            output_metas.push(meta);
        } else {
            // No data produced (e.g. all tombstones dropped at bottom level).
            // The file was already created by the writer constructor — remove it.
            let _ = fs::remove_file(&file_path);
        }

        // Update manifest and swap the read state atomically.
        {
            let mut guard = db_backend.db_inner.lock()?;
            guard.manifest.apply_compaction(
                job.from_level,
                from_file_nums.clone(),
                job.to_level,
                to_file_nums.clone(),
                output_file_nums.clone(),
                output_metas.clone(),
            )?;
            let mut read_state = guard.read_state.duplicate();
            read_state.sstable_version = guard.manifest.sstable_version();
            guard.read_state = Arc::new(read_state);
        }

        // Evict all removed input files from the block cache.
        for fn_ in from_file_nums.iter().chain(to_file_nums.iter()) {
            block_cache.remove_sstable(*fn_)?;
        }

        // Delete input SSTable files from disk.
        for fn_ in from_file_nums.iter().chain(to_file_nums.iter()) {
            let path = file_utils::sstable_path(config, *fn_);
            if let Err(e) = fs::remove_file(&path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    db_backend.db_context.log_error(format!(
                        "[run_compaction] failed to delete {:?}: {}",
                        path, e
                    ));
                }
            }
        }

        Ok(())
    }

    fn flush_immutable_memtable(db_backend: &Arc<DBBackend>) -> Result<(), DBError> {
        let mut guard = db_backend.db_inner.lock()?;

        let (mt, mt_id) = {
            let Some(mt) = guard.read_state.memtables.first() else {
                return Ok(());
            };
            (mt.clone(), mt.id.clone())
        };

        guard.manifest.add_flushing_memtable(mt_id);
        drop(guard);

        db_backend
            .db_context
            .log_debug(format!("[DBBackend] flusing memetable {}", mt_id));

        // convert the immutable memtable to an sstable
        // and write the blocks to the block cache
        let file_num = db_backend.wal.incr_file_sequence_num();

        db_backend.db_context.log_debug(format!(
            "flushing immutable memtables to sstable {}",
            file_num
        ));

        let file_path = file_utils::sstable_path(db_backend.db_context.config(), file_num);
        let mut sstable_writer = SSTableWriter::new(
            db_backend.db_context.clone(),
            SSTableBuilder::build_from_immutable_memtable(
                db_backend.db_context.clone(),
                mt.clone(),
            ),
            file_path,
        )?;

        // write the data to the sstable and the block cache
        let mut block_id: u32 = 0;
        loop {
            let Some(data_block) = sstable_writer.next_data_block()? else {
                break;
            };
            db_backend
                .db_inner
                .lock()?
                .block_cache
                .add_data_block(file_num, block_id, data_block)?;
            if block_id == u32::MAX {
                todo!("handle max block id");
            }
            block_id += 1;
        }

        // write the index and footer
        let index_block = sstable_writer.index_block()?;
        let meta_block = sstable_writer.meta_data_block()?;
        let footer_block = sstable_writer.footer_block()?;

        // write the file to the block cache
        db_backend.db_inner.lock()?.block_cache.add_sstable(
            file_num,
            footer_block,
            index_block,
            meta_block,
        )?;

        // collect sstable metadata (min/max key and total file size)
        let sstable_meta = SSTableMeta {
            min_key: sstable_writer.first_key().unwrap_or_default(),
            max_key: sstable_writer.last_key().unwrap_or_default(),
            file_size: sstable_writer.total_bytes_written(),
        };

        // write the file to the manifest and remove the flushing memtable reference
        let mut guard = db_backend.db_inner.lock()?;
        guard
            .manifest
            .finalize_flushing_memtable(mt_id, file_num, sstable_meta)?;

        // update the read state with the new sstables versions and
        // remove the flushed memtable
        let mut read_state = guard.read_state.duplicate();
        read_state.sstable_version = guard.manifest.sstable_version();

        if let Some(pos) = read_state
            .memtables
            .iter()
            .position(|item| item.id == mt_id)
        {
            read_state.memtables.remove(pos);
        } else {
            panic!(
                "REMOVE MEMTABLE ERROR: memtable no long found in memtables vec; this is impossible!"
            );
        }

        guard.read_state = Arc::new(read_state);

        // Delete the WAL file associated with the now-flushed memtable.
        if let Some(wal_id) = guard.memtable_wal_ids.remove(&mt_id) {
            drop(guard);
            if let Err(e) = WAL::delete_wal(db_backend.db_context.config(), wal_id) {
                db_backend
                    .db_context
                    .log_error(format!("[DBBackend] failed to delete WAL {}: {}", wal_id, e));
            }
        }

        Ok(())
    }
}

pub(crate) struct DBInner {
    active_memtable: Arc<Memtable>,
    /// The WAL file currently receiving writes for the active memtable.
    active_wal: Option<WALFile>,
    /// Maps ImmutableMemtable.id → WAL file id so we can delete the WAL after flush.
    memtable_wal_ids: HashMap<usize, u64>,
    /// Shared with WAL so WAL rotation (needs a new file_num) can happen inside the mutex.
    file_sequence_num: Arc<atomic::AtomicU64>,

    pub(crate) read_state: Arc<ReadState>,
    immutable_memtable_idx: atomic::AtomicUsize,
    pub(crate) block_cache: Arc<BlockCache>,

    // maintainer state
    pub(crate) manifest: Manifest,

    memory: Arc<MemoryManager>,
    db_context: Arc<DBContext>,
}

impl DBInner {
    fn insert(&mut self, log_entry: Arc<LogEntry>) -> Result<(), DBError> {
        // Write to WAL before inserting into the memtable for durability.
        self.append_to_wal(&log_entry)?;

        let inserted = self.active_memtable.insert(log_entry.clone())?;
        if inserted {
            return Ok(());
        }

        // the active memtable is full. The active memtable needs to be made into a
        // immutable memtable and a new active memtable created.
        self.create_new_active_memtable()?;

        // The entry was already written to the old WAL above; it will be replayed
        // from that WAL during recovery. We do NOT re-write it to the new WAL —
        // doing so would double the entry in the new WAL.

        let inserted = self.active_memtable.insert(log_entry)?;
        if inserted {
            return Ok(());
        }

        Ok(())
    }

    fn append_to_wal(&mut self, entry: &Arc<LogEntry>) -> Result<(), DBError> {
        if let Some(wal_file) = &mut self.active_wal {
            wal_file.append(entry)?;
            wal_file.sync()?;
        }
        Ok(())
    }

    fn get_read_state(&self) -> Arc<ReadState> {
        self.read_state.clone()
    }

    fn create_new_active_memtable(&mut self) -> Result<(), DBError> {
        self.db_context
            .log_info("[DBInner] Creating new active memtable".to_string());

        let old = self.active_memtable.clone();
        self.active_memtable = Arc::new(Memtable::new(
            self.db_context.clone(),
            self.memory.clone(),
            self.db_context
                .config()
                .memory_manager_max_memtable_memory_usage(),
        ));

        let im_id = self
            .immutable_memtable_idx
            .fetch_add(1, atomic::Ordering::Relaxed);

        let immutable_memtable =
            Arc::new(ImmutableMemtable::new(im_id, self.db_context.clone(), old)?);

        // Record which WAL file belongs to the frozen memtable so we can delete
        // it once that memtable is flushed to an SSTable.
        if let Some(current_wal) = &self.active_wal {
            self.memtable_wal_ids.insert(im_id, current_wal.id);
        }

        // Rotate to a new WAL file for the new active memtable.
        // We use the shared file_sequence_num counter to avoid needing a reference
        // to DBBackend.wal from inside the mutex.
        let new_wal_id = self
            .file_sequence_num
            .fetch_add(1, atomic::Ordering::SeqCst);
        self.active_wal = Some(WAL::rotate(new_wal_id, self.db_context.config())?);

        // update the read state
        let mut read_state = self.read_state.duplicate();
        read_state.memtables.push(immutable_memtable);

        self.read_state = Arc::new(read_state);

        Ok(())
    }
}

/////////////////////////////////////////
