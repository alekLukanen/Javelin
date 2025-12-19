use std::collections::HashSet;
use std::error::Error;
use std::fmt::Display;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, atomic};
use std::thread::JoinHandle;

use super::block_cache::{BlockCache, BlockCacheError, FileBlockData};
use super::db_context::DBContext;
use super::entry::{Entry, LogEntry};
use super::manifest::{Manifest, SSTableVersion};
use super::memory_manager::MemoryManager;
use super::memtable::{ImmutableMemtable, ImmutableMemtableError, Memtable, MemtableError};
use super::sstable_builder::SSTableBuilder;
use super::sstable_writer::{SSTableWriter, SSTableWriterError};
use super::wal::WAL;
use crate::core::db_config::DBConfig;

#[derive(Debug)]
pub enum DBError {
    MemtableError(MemtableError),
    ImmutableMemtableError(ImmutableMemtableError),
    SSTableWriterError(SSTableWriterError),
    BlockCacheError(BlockCacheError),
    MutexLockFailed(String),
}

impl Display for DBError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DBError::MemtableError(e) => write!(f, "MemtableError: {}", e),
            DBError::ImmutableMemtableError(e) => write!(f, "ImmutableMemtableError: {}", e),
            DBError::SSTableWriterError(e) => write!(f, "SSTableWriterError: {}", e),
            DBError::BlockCacheError(e) => write!(f, "BlockCacheError: {}", e),
            DBError::MutexLockFailed(v) => write!(f, "MutexLockFailed: {}", v),
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

////////////////////////////////////////////

#[derive(Clone)]
struct ReadState {
    memtables: Vec<Arc<ImmutableMemtable>>,
    sstable_version: Arc<SSTableVersion>,
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
    maintainer_handle: JoinHandle<()>,

    memory: Arc<MemoryManager>,
    db_context: Arc<DBContext>,
}

impl DB {
    pub fn new(config: DBConfig) -> DB {
        panic!("under development");

        let db_context = Arc::new(DBContext::new(config));
        let memory = Arc::new(MemoryManager::new(db_context.clone()));

        let wal = WAL::new();
        let manifest = Manifest::new(db_context.clone());

        let db_backend = Arc::new(DBBackend {
            db_inner: Mutex::new(DBInner {
                active_memtable: Arc::new(Memtable::new(db_context.clone(), memory.clone())),
                read_state: Arc::new(ReadState {
                    memtables: Vec::new(),
                    sstable_version: manifest.sstable_version(),
                }),
                immutable_memtable_idx: atomic::AtomicUsize::new(0),
                block_cache: BlockCache::new(db_context.clone(), memory.clone()),
                manifest: Manifest::new(db_context.clone()),
                memory: memory.clone(),
                db_context: db_context.clone(),
            }),
            wal,
            db_context: db_context.clone(),
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

        DB {
            db_backend,
            maintainer_handle: handle,
            memory,
            db_context,
        }
    }

    pub fn close(&self) -> Result<(), DBError> {
        self.db_backend.db_inner.lock()?.block_cache.close();
        Ok(())
    }

    pub fn get(&self, key: &Vec<u8>) -> Result<Option<Vec<u8>>, DBError> {
        let log_seq_num = self.db_backend.wal.incr_log_sequence_num();

        let inner_guard = self.db_backend.db_inner.lock()?;

        let val = inner_guard.active_memtable.get(key, log_seq_num)?;
        match val {
            Some(val) => match &val.entry {
                Entry::Put { val, .. } => return Ok(Some(val.clone())),
                Entry::Del { .. } => return Ok(None),
                Entry::Empty => return Ok(None),
            },
            None => {}
        }

        let immutable_memtables = &inner_guard.get_read_state().memtables;
        drop(inner_guard);

        for memtable in immutable_memtables.iter().rev() {
            let val = memtable.get(key, log_seq_num);
            return match val {
                Some(val) => match &val.entry {
                    Entry::Put { val, .. } => Ok(Some(val.clone())),
                    Entry::Del { .. } => Ok(None),
                    Entry::Empty => Ok(None),
                },
                None => Ok(None),
            };
        }

        Ok(None)
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
}

//////////////////////////////////////////

struct DBBackend {
    db_inner: Mutex<DBInner>,
    wal: WAL,
    db_context: Arc<DBContext>,
}

impl DBBackend {
    /// Responsible for:
    /// - Flusing immutable memtables
    /// - Compacting SSTables when a layer gets too large
    fn maintainer(db_backend: Arc<DBBackend>) -> Result<(), DBError> {
        loop {
            // check for immutable memtables to flush

            match Self::flush_immutable_memtable(&db_backend) {
                Ok(_) => {}
                Err(err) => {
                    db_backend
                        .db_context
                        .log_error(format!("[DBBackend-maintainer] {:?}", err));
                }
            }

            std::thread::sleep(std::time::Duration::from_millis(1));
        }
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

        // convert the immutable memtable to an sstable
        // and write the blocks to the block cache

        let file_num = db_backend.wal.incr_file_sequence_num();

        let mut file_path = PathBuf::new();
        file_path.push(db_backend.db_context.config().data_dir());
        file_path.push(format!("{}.dat", file_num));

        let sstable_builder = SSTableBuilder::build_from_immutable_memtable(
            db_backend.db_context.clone(),
            mt.clone(),
        );
        let mut sstable_writer =
            SSTableWriter::new(db_backend.db_context.clone(), sstable_builder, file_path)?;

        // write the data to the sstable and the block cache
        let mut block_id: u16 = 0;
        loop {
            let Some(block) = sstable_writer.next_block()? else {
                break;
            };
            db_backend
                .db_inner
                .lock()?
                .block_cache
                .add_data_block(file_num, block_id, block)?;
            block_id += 1;
        }

        // write the file to the manifest and remove the flushing memtable reference
        let mut guard = db_backend.db_inner.lock()?;
        guard.manifest.finalize_flushing_memtable(mt_id, file_num);

        Ok(())
    }
}

struct DBInner {
    active_memtable: Arc<Memtable>,
    read_state: Arc<ReadState>,
    immutable_memtable_idx: atomic::AtomicUsize,
    block_cache: BlockCache,

    // maintainer state
    manifest: Manifest,

    memory: Arc<MemoryManager>,
    db_context: Arc<DBContext>,
}

impl DBInner {
    fn insert(&mut self, log_entry: Arc<LogEntry>) -> Result<(), DBError> {
        let inserted = self.active_memtable.insert(log_entry.clone())?;
        if inserted {
            return Ok(());
        }

        // the active memtable is full. The active memtable needs to be made into a
        // immutable memtable and a new active memtable created.
        self.create_new_active_memtable()?;

        let inserted = self.active_memtable.insert(log_entry)?;
        if inserted {
            return Ok(());
        }

        Ok(())
    }

    fn get_read_state(&self) -> Arc<ReadState> {
        self.read_state.clone()
    }

    fn create_new_active_memtable(&mut self) -> Result<(), DBError> {
        self.db_context
            .log_info("[Memtable] Creating new active memtable".to_string());

        let old = self.active_memtable.clone();
        self.active_memtable =
            Arc::new(Memtable::new(self.db_context.clone(), self.memory.clone()));

        let im_id = self
            .immutable_memtable_idx
            .fetch_add(1, atomic::Ordering::Relaxed);

        let immutable_memtable =
            Arc::new(ImmutableMemtable::new(im_id, self.db_context.clone(), old)?);

        // update the read state
        let mut read_state = self.read_state.duplicate();
        read_state.memtables.push(immutable_memtable);

        self.read_state = Arc::new(read_state);

        Ok(())
    }
}

/////////////////////////////////////////
