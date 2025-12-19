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
    sstable_builder::Block,
};

/////////////////////////////////////////////////////

pub struct SSTable {
    id: u64,
    index: Block,
}

pub struct FileBlockData {
    file_id: u64,
    block_id: u16,
    block: Block,

    record: MemoryRecord,
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

    fn add_data_block(
        &mut self,
        file_id: u64,
        block_id: u16,
        block: Block,
        record: MemoryRecord,
    ) -> (Option<BlockDataHandle>, bool) {
        match block {
            Block::DataBlock { .. } => {
                let key = (file_id.clone(), block_id.clone());
                if self.data_blocks.contains_key(&key) {
                    return (None, false);
                }

                let cached_data_block = Arc::new(CachedDataBlock {
                    refs: AtomicUsize::new(1),
                    evicted: AtomicBool::new(false),
                    data: Arc::new(FileBlockData {
                        file_id,
                        block_id,
                        block,
                        record,
                    }),
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
            Block::IndexBlock { .. } => (None, false),
            Block::FooterBlock { .. } => (None, false),
        }
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
    pub fn new(db_context: Arc<DBContext>, memory_manager: Arc<MemoryManager>) -> BlockCache {
        let mut shards = Vec::new();
        let num_shards = db_context.config().block_cache_num_shards();
        for _ in 0..num_shards {
            shards.push(Mutex::new(BlockCacheShard::new()));
        }
        let inner = Arc::new(BlockCacheInner {
            memory_manager: memory_manager.clone(),
            db_context: db_context.clone(),
            close_called: AtomicBool::new(false),
            shards,
            num_shards,
            maintain_idx: AtomicUsize::new(0),
        });
        let inner_clone = inner.clone();
        let handle = std::thread::spawn(move || {
            let res = inner_clone.maintain();
            if let Err(err) = res {
                db_context.log_error(format!("[BlockCache] {:?}", err));
            }
        });
        BlockCache { inner, handle }
    }

    pub fn close(&self) {
        self.inner.close_called.store(true, Ordering::Relaxed);
        let mut waits: usize = 0;
        loop {
            if self.handle.is_finished() {
                break;
            }

            if waits % 1000 == 0 {
                self.inner
                    .db_context
                    .log_info("[BlockCache] Waiting for the maintain thread to exit".to_string());
            }

            std::thread::sleep(std::time::Duration::from_millis(1));
            waits += 1;
        }
    }

    pub fn add_data_block(
        &self,
        file_id: u64,
        block_id: u16,
        block: Block,
    ) -> Result<(Option<BlockDataHandle>, bool), BlockCacheError> {
        match block {
            Block::DataBlock { .. } => {
                let record = self.new_pre_allocated_record(block.size())?;
                let shard_idx = (file_id as usize) % self.inner.num_shards;
                let handle = self
                    .inner
                    .shards
                    .get(shard_idx)
                    .expect("expected shard")
                    .lock()?
                    .add_data_block(file_id, block_id, block, record);
                Ok(handle)
            }
            Block::IndexBlock { .. } => Ok((None, false)),
            Block::FooterBlock { .. } => Ok((None, false)),
        }
    }

    pub fn get_data_block(
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

    fn new_pre_allocated_record(&self, size: usize) -> Result<MemoryRecord, BlockCacheError> {
        let rec = self.inner.memory_manager.new_record(size, true);
        if rec.allocate(size)? {
            Ok(rec)
        } else {
            Err(BlockCacheError::UnableToAllocateMemory)
        }
    }
}

pub struct BlockCacheInner {
    memory_manager: Arc<MemoryManager>,
    db_context: Arc<DBContext>,

    close_called: AtomicBool,
    shards: Vec<Mutex<BlockCacheShard>>,
    num_shards: usize,
    maintain_idx: AtomicUsize,
}

impl BlockCacheInner {
    fn maintain(&self) -> Result<(), BlockCacheError> {
        loop {
            if self.close_called.load(Ordering::Relaxed) {
                self.db_context
                    .log_info(format!("[BlockCache] Exiting the maintain loop"));
                break;
            }
            let idx = self.maintain_idx.fetch_add(1, Ordering::Relaxed) % self.num_shards;
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
        Ok(())
    }
}

///////////////////////////////////////////////////
// Errors

#[derive(Debug)]
pub enum BlockCacheError {
    UnableToAllocateMemory,
    MutexLockFailed(String),
    MemoryRecordError(MemoryRecordError),
}

impl Display for BlockCacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlockCacheError::UnableToAllocateMemory => write!(f, "UnableToAllocateMemory"),
            BlockCacheError::MutexLockFailed(e) => write!(f, "MutexLockFailed: {}", e),
            BlockCacheError::MemoryRecordError(e) => write!(f, "MemoryRecordError: {}", e),
        }
    }
}

impl Error for BlockCacheError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            BlockCacheError::UnableToAllocateMemory => None,
            BlockCacheError::MutexLockFailed(_) => None,
            BlockCacheError::MemoryRecordError(e) => Some(e),
        }
    }
}

impl<T> From<std::sync::PoisonError<std::sync::MutexGuard<'_, T>>> for BlockCacheError {
    fn from(value: std::sync::PoisonError<std::sync::MutexGuard<'_, T>>) -> Self {
        BlockCacheError::MutexLockFailed(value.to_string())
    }
}

impl From<MemoryRecordError> for BlockCacheError {
    fn from(value: MemoryRecordError) -> Self {
        BlockCacheError::MemoryRecordError(value)
    }
}
