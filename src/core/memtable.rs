use std::{
    error::Error,
    fmt::Display,
    sync::{Arc, Mutex},
};

use super::{
    db_context::DBContext,
    entry::LogEntry,
    memory_manager::{MemoryManager, MemoryRecord, MemoryRecordError},
    skiplist::SkipList,
};

///////////////////////////////////////

#[derive(Debug)]
pub enum MemtableManagerError {
    MutexLockFailed(String),
    MemtableError(MemtableError),
    ImmutableMemtableError(ImmutableMemtableError),
}

impl Display for MemtableManagerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemtableManagerError::MutexLockFailed(e) => write!(f, "MutexLockFailed: {}", e),
            MemtableManagerError::MemtableError(e) => write!(f, "MemtableError: {}", e),
            MemtableManagerError::ImmutableMemtableError(e) => {
                write!(f, "ImmutableMemtableError: {}", e)
            }
        }
    }
}

impl Error for MemtableManagerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            MemtableManagerError::MutexLockFailed(_) => None,
            MemtableManagerError::MemtableError(e) => Some(e),
            MemtableManagerError::ImmutableMemtableError(e) => Some(e),
        }
    }
}

impl<T> From<std::sync::PoisonError<std::sync::MutexGuard<'_, T>>> for MemtableManagerError {
    fn from(value: std::sync::PoisonError<std::sync::MutexGuard<'_, T>>) -> Self {
        MemtableManagerError::MutexLockFailed(value.to_string())
    }
}

impl From<MemtableError> for MemtableManagerError {
    fn from(value: MemtableError) -> Self {
        MemtableManagerError::MemtableError(value)
    }
}

impl From<ImmutableMemtableError> for MemtableManagerError {
    fn from(value: ImmutableMemtableError) -> Self {
        MemtableManagerError::ImmutableMemtableError(value)
    }
}

pub struct MemtableManager {
    active_memtable: Mutex<Memtable>,
    immutable_memtables: Mutex<Vec<Arc<ImmutableMemtable>>>,

    memory: Arc<MemoryManager>,
    db_context: Arc<DBContext>,
}

impl MemtableManager {
    pub fn new(db_context: Arc<DBContext>, memory: Arc<MemoryManager>) -> MemtableManager {
        MemtableManager {
            active_memtable: Mutex::new(Memtable::new(
                db_context.clone(),
                memory.clone(),
                db_context
                    .config()
                    .memory_manager_max_memtable_memory_usage(),
            )),
            immutable_memtables: Mutex::new(Vec::new()),
            memory,
            db_context,
        }
    }

    pub fn insert(&self, log_entry: LogEntry) -> Result<(), MemtableManagerError> {
        let inserted = self.active_memtable.lock()?.insert(log_entry)?;
        if inserted {
            return Ok(());
        }

        // the active memtable is full. The active memtable needs to be made into a
        // immutable memtable and a new active memtable created.
        self.create_new_active_memtable()?;

        Ok(())
    }

    pub fn get(
        &self,
        key: &Vec<u8>,
        log_seq_num: u64,
    ) -> Result<Option<LogEntry>, MemtableManagerError> {
        let val = self.active_memtable.lock()?.get(key, log_seq_num)?;
        if val.is_some() {
            return Ok(val);
        }

        let immutable_memtables = self.immutable_memtables.lock()?.clone();
        for memtable in immutable_memtables {
            let val = memtable.get(key, log_seq_num);
            if val.is_some() {
                return Ok(val);
            }
        }

        Ok(None)
    }

    fn create_new_active_memtable(&self) -> Result<(), MemtableManagerError> {
        let mut active_memtable_guard = self.active_memtable.lock()?;
        let old = std::mem::replace(
            &mut *active_memtable_guard,
            Memtable::new(
                self.db_context.clone(),
                self.memory.clone(),
                self.db_context
                    .config()
                    .memory_manager_max_memtable_memory_usage(),
            ),
        );

        let immutable_memtable = Arc::new(ImmutableMemtable::new(self.db_context.clone(), old)?);
        self.immutable_memtables.lock()?.push(immutable_memtable);

        Ok(())
    }
}

///////////////////////////////////////

#[derive(Debug)]
pub enum MemtableError {
    MutexLockFailed(String),
    MemoryRecordError(MemoryRecordError),
}

impl Display for MemtableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemtableError::MutexLockFailed(e) => write!(f, "MutexLockFailed: {}", e),
            MemtableError::MemoryRecordError(e) => write!(f, "MemoryRecordError: {}", e),
        }
    }
}

impl Error for MemtableError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            MemtableError::MutexLockFailed(_) => None,
            MemtableError::MemoryRecordError(e) => Some(e),
        }
    }
}

impl<T> From<std::sync::PoisonError<std::sync::MutexGuard<'_, T>>> for MemtableError {
    fn from(value: std::sync::PoisonError<std::sync::MutexGuard<'_, T>>) -> Self {
        MemtableError::MutexLockFailed(value.to_string())
    }
}

impl From<MemoryRecordError> for MemtableError {
    fn from(value: MemoryRecordError) -> Self {
        MemtableError::MemoryRecordError(value)
    }
}

pub struct Memtable {
    skip_list: Mutex<SkipList>,
    memory: Arc<MemoryManager>,
    db_context: Arc<DBContext>,

    memory_record: MemoryRecord,
}

impl Memtable {
    pub fn new(
        db_context: Arc<DBContext>,
        memory: Arc<MemoryManager>,
        max_size: usize,
    ) -> Memtable {
        let record = memory.new_record(max_size, true);
        Memtable {
            skip_list: Mutex::new(SkipList::new(
                db_context.config().memtable_probability(),
                db_context.config().memtable_expected_num_keys(),
                db_context.config().memtable_allowed_max_levels(),
            )),
            memory,
            db_context,
            memory_record: record,
        }
    }

    pub fn insert(&self, log_entry: LogEntry) -> Result<bool, MemtableError> {
        let allocated = self.memory_record.allocate(log_entry.size())?;
        if allocated {
            let guard = self.skip_list.lock()?;
            guard.insert(log_entry);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn get(&self, key: &Vec<u8>, log_seq_num: u64) -> Result<Option<LogEntry>, MemtableError> {
        let guard = self.skip_list.lock()?;
        Ok(guard.get(key, log_seq_num))
    }
}

///////////////////////////////////////

#[derive(Debug)]
pub enum ImmutableMemtableError {
    MutexLockFailed(String),
}

impl Display for ImmutableMemtableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImmutableMemtableError::MutexLockFailed(e) => write!(f, "MutexLockFailed: {}", e),
        }
    }
}

impl Error for ImmutableMemtableError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ImmutableMemtableError::MutexLockFailed(_) => None,
        }
    }
}

impl<T> From<std::sync::PoisonError<T>> for ImmutableMemtableError {
    fn from(value: std::sync::PoisonError<T>) -> Self {
        ImmutableMemtableError::MutexLockFailed(value.to_string())
    }
}

pub struct ImmutableMemtable {
    skip_list: SkipList,
    db_context: Arc<DBContext>,
}

impl ImmutableMemtable {
    pub fn new(
        db_context: Arc<DBContext>,
        memtable: Memtable,
    ) -> Result<ImmutableMemtable, ImmutableMemtableError> {
        let skip_list = memtable.skip_list.into_inner()?;
        Ok(ImmutableMemtable {
            db_context,
            skip_list,
        })
    }

    pub fn get(&self, key: &Vec<u8>, log_seq_num: u64) -> Option<LogEntry> {
        self.skip_list.get(key, log_seq_num)
    }
}
