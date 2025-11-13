use std::{
    error::Error,
    fmt::Display,
    sync::{Arc, Mutex},
};

use super::{db_config::DBConfig, entry::LogEntry, skiplist::SkipList};

///////////////////////////////////////

#[derive(Debug)]
pub enum MemtableHandlerError {
    MutexLockFailed(String),
    MemtableError(MemtableError),
}

impl Display for MemtableHandlerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemtableHandlerError::MutexLockFailed(e) => write!(f, "MutexLockFailed: {}", e),
            MemtableHandlerError::MemtableError(e) => write!(f, "MemtableError: {}", e),
        }
    }
}

impl Error for MemtableHandlerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            MemtableHandlerError::MutexLockFailed(_) => None,
            MemtableHandlerError::MemtableError(e) => Some(e),
        }
    }
}

impl<T> From<std::sync::PoisonError<std::sync::MutexGuard<'_, T>>> for MemtableHandlerError {
    fn from(value: std::sync::PoisonError<std::sync::MutexGuard<'_, T>>) -> Self {
        MemtableHandlerError::MutexLockFailed(value.to_string())
    }
}

impl From<MemtableError> for MemtableHandlerError {
    fn from(value: MemtableError) -> Self {
        MemtableHandlerError::MemtableError(value)
    }
}

pub struct MemtableHandler {
    active_memtable: Memtable,
    immutable_memtables: Mutex<Vec<Arc<ImmutableMemtable>>>,
}

impl MemtableHandler {
    pub fn new(config: DBConfig) -> MemtableHandler {
        MemtableHandler {
            active_memtable: Memtable::new(config),
            immutable_memtables: Mutex::new(Vec::new()),
        }
    }

    pub fn insert(&self, log_entry: LogEntry) -> Result<(), MemtableHandlerError> {
        Ok(self.active_memtable.insert(log_entry)?)
    }

    pub fn get(
        &self,
        key: &Vec<u8>,
        log_seq_num: u64,
    ) -> Result<Option<LogEntry>, MemtableHandlerError> {
        let val = self.active_memtable.get(key, log_seq_num)?;
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
}

///////////////////////////////////////

#[derive(Debug)]
pub enum MemtableError {
    MutexLockFailed(String),
}

impl Display for MemtableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemtableError::MutexLockFailed(e) => write!(f, "MutexLockFailed: {}", e),
        }
    }
}

impl Error for MemtableError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            MemtableError::MutexLockFailed(_) => None,
        }
    }
}

impl<T> From<std::sync::PoisonError<std::sync::MutexGuard<'_, T>>> for MemtableError {
    fn from(value: std::sync::PoisonError<std::sync::MutexGuard<'_, T>>) -> Self {
        MemtableError::MutexLockFailed(value.to_string())
    }
}

pub struct Memtable {
    skip_list: Mutex<SkipList>,
}

impl Memtable {
    pub fn new(config: DBConfig) -> Memtable {
        Memtable {
            skip_list: Mutex::new(SkipList::new(
                config.memtable_probability(),
                config.memtable_expected_num_keys(),
                config.memtable_allowed_max_levels(),
            )),
        }
    }

    pub fn insert(&self, log_entry: LogEntry) -> Result<(), MemtableError> {
        let guard = self.skip_list.lock()?;
        guard.insert(log_entry);
        Ok(())
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
}

impl ImmutableMemtable {
    pub fn new(memtable: Memtable) -> Result<ImmutableMemtable, ImmutableMemtableError> {
        let skip_list = memtable.skip_list.into_inner()?;
        Ok(ImmutableMemtable { skip_list })
    }

    pub fn get(&self, key: &Vec<u8>, log_seq_num: u64) -> Option<LogEntry> {
        self.skip_list.get(key, log_seq_num)
    }
}
