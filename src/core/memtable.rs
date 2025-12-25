use std::{error::Error, fmt::Display, sync::Arc};

use crate::core::iterator::{IteratorError, SourceIterator};

use super::{
    db_context::DBContext,
    entry::LogEntry,
    memory_manager::{MemoryManager, MemoryRecord, MemoryRecordError},
    skiplist::{SkipList, SkipListIter},
};

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
    skip_list: SkipList,

    memory_record: MemoryRecord,
}

impl Memtable {
    pub fn new(db_context: Arc<DBContext>, memory: Arc<MemoryManager>) -> Memtable {
        let record = memory.new_record(
            db_context
                .config()
                .memory_manager_max_memtable_memory_usage(),
            true,
        );
        Memtable {
            skip_list: SkipList::new(
                db_context.config().memtable_probability(),
                db_context.config().memtable_expected_num_keys(),
                db_context.config().memtable_allowed_max_levels(),
            ),
            memory_record: record,
        }
    }

    pub fn copy_skip_list(&self) -> Result<SkipList, MemtableError> {
        Ok(self.skip_list.clone())
    }

    pub fn skip_list_iter(&self) -> SkipListIter {
        self.skip_list.iter()
    }

    pub fn insert(&self, log_entry: Arc<LogEntry>) -> Result<bool, MemtableError> {
        let allocated = self.memory_record.allocate(log_entry.size())?;
        if allocated {
            self.skip_list.insert(log_entry);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn get(
        &self,
        key: &Vec<u8>,
        log_seq_num: u64,
    ) -> Result<Option<Arc<LogEntry>>, MemtableError> {
        Ok(self.skip_list.get(key, log_seq_num))
    }
}

pub struct MemtableIterator {
    memtable: Arc<Memtable>,
    skip_list_iter: SkipListIter,
}

impl MemtableIterator {
    pub fn new(memtable: Arc<Memtable>) -> MemtableIterator {
        MemtableIterator {
            memtable: memtable.clone(),
            skip_list_iter: memtable.skip_list_iter(),
        }
    }
}

impl SourceIterator for MemtableIterator {
    fn next(&mut self) -> Result<Option<Arc<LogEntry>>, IteratorError> {
        let log_entry = self.skip_list_iter.next();
        Ok(log_entry)
    }
}

///////////////////////////////////////

#[derive(Debug)]
pub enum ImmutableMemtableError {
    MutexLockFailed(String),
    MemtableError(MemtableError),
    MemoryRecordError(MemoryRecordError),
    UnableToAllocateMemory,
}

impl Display for ImmutableMemtableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImmutableMemtableError::MutexLockFailed(e) => write!(f, "MutexLockFailed: {}", e),
            ImmutableMemtableError::MemtableError(e) => write!(f, "MemtableError: {}", e),
            ImmutableMemtableError::MemoryRecordError(e) => write!(f, "MemoryRecordError: {}", e),
            ImmutableMemtableError::UnableToAllocateMemory => write!(f, "UnableToAllocateMemory"),
        }
    }
}

impl Error for ImmutableMemtableError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ImmutableMemtableError::MutexLockFailed(_) => None,
            ImmutableMemtableError::MemtableError(e) => Some(e),
            ImmutableMemtableError::MemoryRecordError(e) => Some(e),
            ImmutableMemtableError::UnableToAllocateMemory => None,
        }
    }
}

impl<T> From<std::sync::PoisonError<T>> for ImmutableMemtableError {
    fn from(value: std::sync::PoisonError<T>) -> Self {
        ImmutableMemtableError::MutexLockFailed(value.to_string())
    }
}

impl From<MemtableError> for ImmutableMemtableError {
    fn from(value: MemtableError) -> Self {
        ImmutableMemtableError::MemtableError(value)
    }
}

impl From<MemoryRecordError> for ImmutableMemtableError {
    fn from(value: MemoryRecordError) -> Self {
        ImmutableMemtableError::MemoryRecordError(value)
    }
}

pub struct ImmutableMemtable {
    pub(crate) id: usize,

    skip_list: SkipList,
    db_context: Arc<DBContext>,

    memory_record: MemoryRecord,
}

impl ImmutableMemtable {
    pub fn new(
        id: usize,
        db_context: Arc<DBContext>,
        memtable: Arc<Memtable>,
    ) -> Result<ImmutableMemtable, ImmutableMemtableError> {
        let memory_record = memtable.memory_record.duplicate()?;
        match memory_record {
            Some(memory_record) => {
                let skip_list = memtable.copy_skip_list()?;
                Ok(ImmutableMemtable {
                    id,
                    db_context,
                    skip_list,
                    memory_record,
                })
            }
            None => Err(ImmutableMemtableError::UnableToAllocateMemory),
        }
    }

    pub fn get(&self, key: &Vec<u8>, log_seq_num: u64) -> Option<Arc<LogEntry>> {
        self.skip_list.get(key, log_seq_num)
    }

    pub fn size(&self) -> usize {
        self.memory_record.size()
    }

    pub fn iter(&self) -> SkipListIter {
        self.skip_list.iter()
    }
}
