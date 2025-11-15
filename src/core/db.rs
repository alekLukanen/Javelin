use std::error::Error;
use std::fmt::Display;
use std::sync::Arc;

use super::db_context::DBContext;
use super::entry::{Entry, LogEntry};
use super::memory_manager::MemoryManager;
use super::memtable::{MemtableManager, MemtableManagerError};
use super::wal::WAL;
use crate::core::db_config::DBConfig;

#[derive(Debug)]
pub enum DBError {
    MemtableManagerError(MemtableManagerError),
}

impl Display for DBError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DBError::MemtableManagerError(e) => write!(f, "MemtableError: {}", e),
        }
    }
}

impl Error for DBError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            DBError::MemtableManagerError(_) => None,
        }
    }
}

impl From<MemtableManagerError> for DBError {
    fn from(value: MemtableManagerError) -> Self {
        DBError::MemtableManagerError(value)
    }
}

////////////////////////////////////////////

pub struct DB {
    memtable: MemtableManager,
    memory: Arc<MemoryManager>,
    wal: WAL,
    db_context: Arc<DBContext>,
}

impl DB {
    pub fn new(config: DBConfig) -> DB {
        let db_context = Arc::new(DBContext::new(config));
        let memory = Arc::new(MemoryManager::new(db_context.clone()));
        DB {
            memtable: MemtableManager::new(db_context.clone(), memory.clone()),
            memory,
            wal: WAL::new(),
            db_context,
        }
    }

    pub fn get(&self, key: &Vec<u8>) -> Option<Vec<u8>> {
        None
    }

    pub fn set(&self, key: Vec<u8>, val: Vec<u8>) -> Result<(), DBError> {
        self.memtable.insert(LogEntry::new(
            Entry::Put { key, val },
            self.wal.incr_log_sequence_num(),
        ))?;
        Ok(())
    }

    pub fn delete(&self, key: Vec<u8>) -> Result<(), DBError> {
        self.memtable.insert(LogEntry::new(
            Entry::Del { key },
            self.wal.incr_log_sequence_num(),
        ))?;
        Ok(())
    }

    pub fn iterator(&self, opts: IteratorOptions) -> Result<Iterator, DBError> {
        Ok(Iterator {})
    }
}

/////////////////////////////////////////

pub struct IteratorOptions {
    lower_bound: Vec<u8>,
    upper_bound: Vec<u8>,
}

pub struct Iterator {}
