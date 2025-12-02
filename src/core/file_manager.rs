use std::{collections::BTreeMap, error::Error, fmt::Display, sync::Arc};

use super::{
    db_context::DBContext,
    memory_manager::{MemoryManager, MemoryRecord, MemoryRecordError},
    memtable::ImmutableMemtable,
};

#[derive(Debug)]
pub enum FileManagerError {
    UnableToAllocateMemory,
    MemoryRecordError(MemoryRecordError),
}

impl Display for FileManagerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileManagerError::UnableToAllocateMemory => write!(f, "UnableToAllocateMemory"),
            FileManagerError::MemoryRecordError(e) => write!(f, "MemoryRecordError: {}", e),
        }
    }
}

impl Error for FileManagerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            FileManagerError::UnableToAllocateMemory => None,
            FileManagerError::MemoryRecordError(e) => Some(e),
        }
    }
}

impl From<MemoryRecordError> for FileManagerError {
    fn from(value: MemoryRecordError) -> Self {
        FileManagerError::MemoryRecordError(value)
    }
}

/////////////////////////////////////////////////////

pub struct FileData {
    id: u64,
    file_data: Option<Vec<u8>>,

    record: MemoryRecord,
}

pub struct FileManager {
    cached_file_data: BTreeMap<u64, Arc<FileData>>,

    memory_manager: Arc<MemoryManager>,
    db_context: Arc<DBContext>,
}

impl FileManager {
    pub fn new(db_context: Arc<DBContext>, memory_manager: Arc<MemoryManager>) -> FileManager {
        FileManager {
            cached_file_data: BTreeMap::new(),
            memory_manager,
            db_context,
        }
    }

    fn new_pre_allocated_record(&self, size: usize) -> Result<MemoryRecord, FileManagerError> {
        let rec = self.memory_manager.new_record(size, true);
        if rec.allocate(size)? {
            Ok(rec)
        } else {
            Err(FileManagerError::UnableToAllocateMemory)
        }
    }

    pub fn new_lvl0_sstable(
        &self,
        memtable: ImmutableMemtable,
    ) -> Result<Arc<FileData>, FileManagerError> {
        Ok(Arc::new(FileData {
            id: 0,
            file_data: None,
            record: self.new_pre_allocated_record(memtable.size())?,
        }))
    }
}
