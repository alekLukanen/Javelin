use std::{
    collections::BTreeMap,
    error::Error,
    fmt::Display,
    sync::{
        self, Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

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

pub struct FileFilterBlock {}

pub struct FileIndexBlock {}

pub struct FileData {
    id: u64,
    filter: FileFilterBlock,
    index: FileIndexBlock,
}

pub struct FileBlockData {
    file_id: u64,
    block_id: u16,
    data: Option<Vec<u8>>,

    record: MemoryRecord,
}

pub struct FileManager {
    memory_manager: Arc<MemoryManager>,
    db_context: Arc<DBContext>,
}

impl FileManager {
    pub fn new(db_context: Arc<DBContext>, memory_manager: Arc<MemoryManager>) -> FileManager {
        FileManager {
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

    pub fn new_lvl0_sstable_file(
        &self,
        memtable: ImmutableMemtable,
    ) -> Result<Arc<FileData>, FileManagerError> {
        // create the file data blocks
        //

        Ok(Arc::new(FileData {
            id: 0,
            filter: FileFilterBlock {},
            index: FileIndexBlock {},
        }))
    }
}

/////////////////////////////////////////////////

pub struct TableManager {
    file_manager: Arc<FileManager>,

    cached_file_block_data: BTreeMap<u64, Arc<FileBlockData>>,
}

impl TableManager {
    pub fn new(db_context: Arc<DBContext>, memory_manager: Arc<MemoryManager>) -> TableManager {
        TableManager {
            file_manager: Arc::new(FileManager::new(db_context, memory_manager)),
            cached_file_block_data: BTreeMap::new(),
        }
    }
}

///////////////////////////////////////////////////
// Handles

pub struct BlockDataHandle {
    entry: Arc<CachedDataBlock>,
}

impl Drop for BlockDataHandle {
    fn drop(&mut self) {
        self.entry.refs.fetch_sub(1, Ordering::SeqCst);
    }
}

///////////////////////////////////////////////////

pub struct CachedDataBlock {
    refs: AtomicUsize,
    evicted: AtomicBool,
    data: FileBlockData,
}

pub struct BlockCacheShard {
    data_blocks: BTreeMap<(u64, u16), Arc<CachedDataBlock>>,
}

impl BlockCacheShard {
    fn new() -> BlockCacheShard {
        BlockCacheShard {
            data_blocks: BTreeMap::new(),
        }
    }

    fn add_data_block(&mut self, block: FileBlockData) -> BlockDataHandle {
        let key = (block.file_id.clone(), block.block_id.clone());
        let cached_data_block = Arc::new(CachedDataBlock {
            refs: AtomicUsize::new(1),
            evicted: AtomicBool::new(false),
            data: block,
        });
        self.data_blocks.insert(key, cached_data_block.clone());
        BlockDataHandle {
            entry: cached_data_block,
        }
    }
}

#[derive(Debug)]
pub enum BlockCacheError {
    MutexLockFailed(String),
}

impl Display for BlockCacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlockCacheError::MutexLockFailed(e) => write!(f, "MutexLockFailed: {}", e),
        }
    }
}

impl Error for BlockCacheError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            BlockCacheError::MutexLockFailed(_) => None,
        }
    }
}

impl<T> From<std::sync::PoisonError<std::sync::MutexGuard<'_, T>>> for BlockCacheError {
    fn from(value: std::sync::PoisonError<std::sync::MutexGuard<'_, T>>) -> Self {
        BlockCacheError::MutexLockFailed(value.to_string())
    }
}

pub struct BlockCache {
    memory_manager: Arc<MemoryManager>,
    db_context: Arc<DBContext>,

    shards: Vec<Mutex<BlockCacheShard>>,
    num_shards: usize,
}

impl BlockCache {
    fn new(db_context: Arc<DBContext>, memory_manager: Arc<MemoryManager>) -> BlockCache {
        let mut shards = Vec::new();
        let num_shards = db_context.config().block_cache_num_shards();
        for _ in 0..num_shards {
            shards.push(Mutex::new(BlockCacheShard::new()));
        }
        BlockCache {
            memory_manager,
            db_context,
            shards: Vec::new(),
            num_shards,
        }
    }

    fn add_data_block(&self, block: FileBlockData) -> Result<BlockDataHandle, BlockCacheError> {
        let shard_idx = (block.file_id as usize) % self.num_shards;
        let handle = self
            .shards
            .get(shard_idx)
            .expect("expected shard")
            .lock()?
            .add_data_block(block);
        Ok(handle)
    }
}
