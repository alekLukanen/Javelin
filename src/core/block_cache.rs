use std::{
    collections::{BTreeMap, LinkedList},
    error::Error,
    fmt::Display,
    sync::{
        Arc, Mutex,
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
    data: Arc<FileBlockData>,
}

pub struct BlockCacheShard {
    data_blocks: BTreeMap<(u64, u16), Arc<CachedDataBlock>>,
    lru_list: LinkedList<(u64, u16)>,
}

impl BlockCacheShard {
    fn new() -> BlockCacheShard {
        BlockCacheShard {
            data_blocks: BTreeMap::new(),
            lru_list: LinkedList::new(),
        }
    }

    fn add_data_block(&mut self, block: FileBlockData) -> (Option<BlockDataHandle>, bool) {
        let key = (block.file_id.clone(), block.block_id.clone());
        if self.data_blocks.contains_key(&key) {
            return (None, false);
        }

        let cached_data_block = Arc::new(CachedDataBlock {
            refs: AtomicUsize::new(1),
            evicted: AtomicBool::new(false),
            data: Arc::new(block),
        });
        self.data_blocks
            .insert(key.clone(), cached_data_block.clone());
        self.lru_list.push_front(key);
        (
            Some(BlockDataHandle {
                entry: cached_data_block,
            }),
            true,
        )
    }

    fn get_data_block(&mut self, file_id: u64, block_id: u16) -> Option<BlockDataHandle> {
        let block = self.data_blocks.get(&(file_id, block_id));
        match block {
            Some(block) => Some(BlockDataHandle {
                entry: block.clone(),
            }),
            None => None,
        }
    }
}

pub struct BlockCache {
    inner: Arc<BlockCacheInner>,
    handle: std::thread::JoinHandle<()>,
}

impl BlockCache {
    fn new(db_context: Arc<DBContext>, memory_manager: Arc<MemoryManager>) -> BlockCache {
        let mut shards = Vec::new();
        let num_shards = db_context.config().block_cache_num_shards();
        for _ in 0..num_shards {
            shards.push(Mutex::new(BlockCacheShard::new()));
        }
        let inner = Arc::new(BlockCacheInner {
            memory_manager: memory_manager.clone(),
            db_context: db_context.clone(),
            file_manager: Arc::new(FileManager::new(db_context, memory_manager)),
            shards: Vec::new(),
            num_shards,
            maintain_idx: AtomicUsize::new(0),
        });
        let handle = std::thread::spawn(|| {});
        BlockCache { inner, handle }
    }

    fn add_data_block(
        &self,
        block: FileBlockData,
    ) -> Result<(Option<BlockDataHandle>, bool), BlockCacheError> {
        let shard_idx = (block.file_id as usize) % self.inner.num_shards;
        let handle = self
            .inner
            .shards
            .get(shard_idx)
            .expect("expected shard")
            .lock()?
            .add_data_block(block);
        Ok(handle)
    }

    fn get_data_block(
        &self,
        file_id: u64,
        block_id: u16,
    ) -> Result<Option<BlockDataHandle>, BlockCacheError> {
        let shard_idx = (file_id as usize) % self.inner.num_shards;
        let handle = self
            .inner
            .shards
            .get(shard_idx)
            .expect("expected shard")
            .lock()?
            .get_data_block(file_id, block_id);
        Ok(handle)
    }
}

pub struct BlockCacheInner {
    memory_manager: Arc<MemoryManager>,
    db_context: Arc<DBContext>,

    file_manager: Arc<FileManager>,

    shards: Vec<Mutex<BlockCacheShard>>,
    num_shards: usize,
    maintain_idx: AtomicUsize,
}

impl BlockCacheInner {
    fn maintain(&self) -> Result<(), BlockCacheError> {
        loop {
            let idx = self.maintain_idx.fetch_add(1, Ordering::Relaxed) & self.num_shards;
            let mut shard = self.shards.get(idx).expect("expected shard").lock()?;
            let keys_to_delete: Vec<(u64, u16)> = shard
                .data_blocks
                .iter()
                .filter_map(|(key, block)| {
                    if block.refs.load(Ordering::Relaxed) == 0 {
                        Some(key.clone())
                    } else {
                        None
                    }
                })
                .collect();
            for key in keys_to_delete {
                shard.data_blocks.remove(&key);
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}

///////////////////////////////////////////////////
// Errors

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
