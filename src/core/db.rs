use std::error::Error;
use std::fmt::Display;
use std::sync::{Arc, Mutex, atomic};

use super::db_context::DBContext;
use super::entry::{Entry, LogEntry};
use super::memory_manager::MemoryManager;
use super::memtable::{ImmutableMemtable, ImmutableMemtableError, Memtable, MemtableError};
use super::wal::WAL;
use crate::core::db_config::DBConfig;

#[derive(Debug)]
pub enum DBError {
    MemtableError(MemtableError),
    ImmutableMemtableError(ImmutableMemtableError),
    MutexLockFailed(String),
}

impl Display for DBError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DBError::MemtableError(e) => write!(f, "MemtableError: {}", e),
            DBError::ImmutableMemtableError(e) => write!(f, "ImmutableMemtableError: {}", e),
            DBError::MutexLockFailed(v) => write!(f, "MutexLockFailed: {}", v),
        }
    }
}

impl Error for DBError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            DBError::MemtableError(e) => Some(e),
            DBError::ImmutableMemtableError(e) => Some(e),
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

////////////////////////////////////////////

pub struct ReadState {
    memtables: Vec<Arc<ImmutableMemtable>>,
    // sstable_version: SSTableVersion,
}

pub struct DB {
    db_inner: Mutex<DBInner>,
    memory: Arc<MemoryManager>,
    wal: WAL,
    db_context: Arc<DBContext>,
}

impl DB {
    pub fn new(config: DBConfig) -> DB {
        let db_context = Arc::new(DBContext::new(config));
        let memory = Arc::new(MemoryManager::new(db_context.clone()));
        DB {
            db_inner: Mutex::new(DBInner {
                active_memtable: Arc::new(Memtable::new(db_context.clone(), memory.clone())),
                immutable_memtables: Vec::new(),
                immutable_memtable_idx: atomic::AtomicUsize::new(0),
                memory: memory.clone(),
                db_context: db_context.clone(),
            }),
            memory,
            wal: WAL::new(),
            db_context,
        }
    }

    pub fn get(&self, key: &Vec<u8>) -> Result<Option<Vec<u8>>, DBError> {
        let val = self
            .db_inner
            .lock()?
            .get(key, self.wal.incr_log_sequence_num())?;
        match val {
            Some(val) => match &val.entry {
                Entry::Put { val, .. } => Ok(Some(val.clone())),
                Entry::Del { .. } => Ok(None),
                Entry::Empty => Ok(None),
            },
            None => Ok(None),
        }
    }

    pub fn set(&self, key: Vec<u8>, val: Vec<u8>) -> Result<(), DBError> {
        self.db_inner.lock()?.insert(Arc::new(LogEntry::new(
            Entry::Put { key, val },
            0, //self.wal.incr_log_sequence_num(),
        )))?;
        Ok(())
    }

    pub fn delete(&self, key: Vec<u8>) -> Result<(), DBError> {
        self.db_inner.lock()?.insert(Arc::new(LogEntry::new(
            Entry::Del { key },
            0, //self.wal.incr_log_sequence_num(),
        )))?;
        Ok(())
    }

    pub fn iterator(&self, opts: IteratorOptions) -> Result<Iterator, DBError> {
        Ok(Iterator {})
    }
}

//////////////////////////////////////////

pub struct DBInner {
    active_memtable: Arc<Memtable>,
    immutable_memtables: Vec<Arc<ImmutableMemtable>>,
    immutable_memtable_idx: atomic::AtomicUsize,

    memory: Arc<MemoryManager>,
    db_context: Arc<DBContext>,
}

impl DBInner {
    pub fn insert(&mut self, log_entry: Arc<LogEntry>) -> Result<(), DBError> {
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

    pub fn get(&self, key: &Vec<u8>, log_seq_num: u64) -> Result<Option<Arc<LogEntry>>, DBError> {
        let val = self.active_memtable.get(key, log_seq_num)?;
        if val.is_some() {
            return Ok(val);
        }

        let immutable_memtables = self.immutable_memtables.clone();
        for memtable in immutable_memtables.iter().rev() {
            let val = memtable.get(key, log_seq_num);
            if val.is_some() {
                return Ok(val);
            }
        }

        Ok(None)
    }

    pub fn get_immutable_memtables(&self) -> Result<Vec<Arc<ImmutableMemtable>>, DBError> {
        Ok(self.immutable_memtables.clone())
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
        self.immutable_memtables.push(immutable_memtable);

        Ok(())
    }
}

/////////////////////////////////////////

pub struct IteratorOptions {
    lower_bound: Vec<u8>,
    upper_bound: Vec<u8>,
}

pub struct Iterator {}
