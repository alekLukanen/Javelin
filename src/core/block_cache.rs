use std::{
    collections::{BTreeMap, HashMap, LinkedList},
    error::Error,
    fmt::Display,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use crate::core::{
    file_utils,
    sstable_reader::{SSTableReader, SSTableReaderError},
};

use super::{
    db_context::DBContext,
    memory_manager::{MemoryManager, MemoryRecord, MemoryRecordError},
};

/////////////////////////////////////////////////////

pub struct SSTable {
    id: u64,
    footer: Arc<Vec<u8>>,
    index: Arc<Vec<u8>>,
    meta: Arc<Vec<u8>>,
}

pub struct FileBlockData {
    file_id: u64,
    block_id: u32,
    block: Vec<u8>,

    record: MemoryRecord,
}

///////////////////////////////////////////////////
// Handles

pub struct DataBlockHandle {
    entry: Arc<CachedDataBlock>,
}

impl DataBlockHandle {
    pub(crate) fn data_block_ref(&self) -> &Vec<u8> {
        &self.entry.data.block
    }
}

impl Clone for DataBlockHandle {
    fn clone(&self) -> Self {
        self.entry.refs.fetch_add(1, Ordering::SeqCst);
        Self {
            entry: self.entry.clone(),
        }
    }
}

impl Drop for DataBlockHandle {
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
    db_context: Arc<DBContext>,

    data_blocks: BTreeMap<(u64, u32), Arc<CachedDataBlock>>,
    lru_list: LinkedList<(u64, u32)>,
}

impl BlockCacheShard {
    fn new(db_context: Arc<DBContext>) -> BlockCacheShard {
        BlockCacheShard {
            db_context,
            data_blocks: BTreeMap::new(),
            lru_list: LinkedList::new(),
        }
    }

    fn add_data_block(
        &mut self,
        file_id: u64,
        block_id: u32,
        block: Vec<u8>,
        record: MemoryRecord,
    ) -> (Option<DataBlockHandle>, bool) {
        let key = (file_id.clone(), block_id.clone());

        if let Some(block) = self.data_blocks.get(&key) {
            return (
                Some(DataBlockHandle {
                    entry: block.clone(),
                }),
                false,
            );
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
            Some(DataBlockHandle {
                entry: cached_data_block,
            }),
            true,
        )
    }

    fn get_data_block(&mut self, file_id: &u64, block_id: &u32) -> Option<DataBlockHandle> {
        let block = self.data_blocks.get(&(*file_id, *block_id));
        match block {
            Some(block) => Some(DataBlockHandle {
                entry: block.clone(),
            }),
            None => None,
        }
    }

    fn evict_file(&mut self, file_id: u64) {
        // Remove all data blocks for this file from the shard.
        let keys_to_remove: Vec<(u64, u32)> = self
            .data_blocks
            .keys()
            .filter(|(fid, _)| *fid == file_id)
            .copied()
            .collect();
        for key in keys_to_remove {
            self.data_blocks.remove(&key);
        }
        // Rebuild lru_list excluding entries for this file.
        let old_lru = std::mem::take(&mut self.lru_list);
        self.lru_list = old_lru
            .into_iter()
            .filter(|(fid, _)| *fid != file_id)
            .collect();
    }
}

pub struct BlockCache {
    db_context: Arc<DBContext>,

    inner: Arc<BlockCacheInner>,
    handle: std::thread::JoinHandle<()>,
}

impl BlockCache {
    pub fn new(db_context: Arc<DBContext>, memory_manager: Arc<MemoryManager>) -> BlockCache {
        let mut shards = Vec::new();
        let num_shards = db_context.config().block_cache_num_shards();
        for _ in 0..num_shards {
            shards.push(Mutex::new(BlockCacheShard::new(db_context.clone())));
        }
        let inner = Arc::new(BlockCacheInner {
            memory_manager: memory_manager.clone(),
            db_context: db_context.clone(),
            close_called: AtomicBool::new(false),
            sstables: Mutex::new(HashMap::new()),
            shards,
            num_shards,
            maintain_idx: AtomicUsize::new(0),
        });
        let inner_clone = inner.clone();
        let db_context_clone = db_context.clone();
        let handle = std::thread::spawn(move || {
            let res = inner_clone.maintain();
            if let Err(err) = res {
                db_context_clone.log_error(format!("[BlockCache] {:?}", err));
            }
        });
        BlockCache {
            db_context,
            inner,
            handle,
        }
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

    pub fn add_sstable(
        &self,
        file_id: u64,
        footer: Vec<u8>,
        index: Vec<u8>,
        meta: Vec<u8>,
    ) -> Result<(), BlockCacheError> {
        self.inner.sstables.lock()?.insert(
            file_id.clone(),
            SSTable {
                id: file_id,
                footer: Arc::new(footer),
                index: Arc::new(index),
                meta: Arc::new(meta),
            },
        );
        Ok(())
    }

    /// Evict all cached data for `file_id` (footer, index, meta, and all data blocks).
    /// Called after compaction removes files from the SSTableVersion.
    pub fn remove_sstable(&self, file_id: u64) -> Result<(), BlockCacheError> {
        self.inner.sstables.lock()?.remove(&file_id);
        let shard_idx = (file_id as usize) % self.inner.num_shards;
        self.inner
            .shards
            .get(shard_idx)
            .expect("expected shard")
            .lock()?
            .evict_file(file_id);
        Ok(())
    }

    pub fn add_data_block(
        &self,
        file_id: u64,
        block_id: u32,
        block: Vec<u8>,
    ) -> Result<(Option<DataBlockHandle>, bool), BlockCacheError> {
        let record = self.new_pre_allocated_record(block.len())?;
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

    pub fn get_data_block(
        &self,
        file_id: &u64,
        block_id: &u32,
    ) -> Result<Option<DataBlockHandle>, BlockCacheError> {
        self.load_sstable_if_not_found(file_id)?;

        let shard_idx = (*file_id as usize) % self.inner.num_shards;
        let handle = self
            .inner
            .shards
            .get(shard_idx)
            .expect("expected shard")
            .lock()?
            .get_data_block(file_id, block_id);
        match handle {
            Some(_) => Ok(handle),
            None => {
                let Some(index_block) = self.get_index_block(file_id)? else {
                    return Err(BlockCacheError::IndexBlockNotFound(*file_id, *block_id));
                };

                let file_path = file_utils::sstable_path(self.db_context.config(), *file_id);
                let data_block = SSTableReader::new(self.db_context.clone(), file_path)?
                    .get_block(block_id, index_block.as_ref())?;

                let (handle, _) =
                    self.add_data_block(file_id.clone(), block_id.clone(), data_block)?;
                Ok(handle)
            }
        }
    }

    pub fn get_index_block(&self, file_id: &u64) -> Result<Option<Arc<Vec<u8>>>, BlockCacheError> {
        self.load_sstable_if_not_found(file_id)?;

        match self.inner.sstables.lock()?.get(file_id) {
            Some(sstable) => Ok(Some(sstable.index.clone())),
            None => Ok(None),
        }
    }

    pub fn get_meta_block(&self, file_id: &u64) -> Result<Option<Arc<Vec<u8>>>, BlockCacheError> {
        self.load_sstable_if_not_found(file_id)?;

        match self.inner.sstables.lock()?.get(file_id) {
            Some(sstable) => Ok(Some(sstable.meta.clone())),
            None => Ok(None),
        }
    }

    fn load_sstable_if_not_found(&self, file_id: &u64) -> Result<(), BlockCacheError> {
        // TODO: improve this method so that all callers wait on the same result

        let has_sstable = self.inner.sstables.lock()?.get(file_id).is_some();

        if has_sstable {
            return Ok(());
        }

        let file_path = file_utils::sstable_path(self.db_context.config(), *file_id);

        let mut sstable_reader = match SSTableReader::new(self.db_context.clone(), file_path) {
            Ok(val) => val,
            Err(err) => return Err(BlockCacheError::FailedCreatingSSTableReader(err)),
        };
        let footer = sstable_reader.footer_block()?;
        let index = sstable_reader.index_block()?;
        let meta = sstable_reader.meta_block()?;
        let sstable = SSTable {
            id: file_id.clone(),
            footer: Arc::new(footer),
            index: Arc::new(index),
            meta: Arc::new(meta),
        };

        self.inner.sstables.lock()?.insert(*file_id, sstable);
        Ok(())
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
    sstables: Mutex<HashMap<u64, SSTable>>,
    shards: Vec<Mutex<BlockCacheShard>>,
    num_shards: usize,
    maintain_idx: AtomicUsize,
}

impl BlockCacheInner {
    fn maintain(&self) -> Result<(), BlockCacheError> {
        loop {
            std::thread::sleep(std::time::Duration::from_millis(50));

            if self.close_called.load(Ordering::Relaxed) {
                self.db_context
                    .log_info(format!("[BlockCache] Exiting the maintain loop"));
                break;
            }
            let idx = self.maintain_idx.fetch_add(1, Ordering::Relaxed) % self.num_shards;

            // wait for the usage to reach 80% before freeing blocks
            if self.memory_manager.usage_ratio() < 0.8 {
                continue;
            }

            self.db_context
                .log_debug(format!("[BlockCache] trying to free blocks"));

            let mut shard = self.shards.get(idx).expect("expected shard").lock()?;
            let keys_to_delete: Vec<(u64, u32)> = shard
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
    SSTableReaderError(SSTableReaderError),
    IndexBlockNotFound(u64, u32),
    FailedCreatingSSTableReader(SSTableReaderError),
}

impl Display for BlockCacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnableToAllocateMemory => write!(f, "UnableToAllocateMemory"),
            Self::MutexLockFailed(e) => write!(f, "MutexLockFailed: {}", e),
            Self::MemoryRecordError(e) => write!(f, "MemoryRecordError: {}", e),
            Self::SSTableReaderError(e) => write!(f, "SSTableReaderError: {}", e),
            Self::IndexBlockNotFound(file_id, block_id) => write!(
                f,
                "IndexBlockNotFound: file_id={}, block_id={}",
                file_id, block_id
            ),
            Self::FailedCreatingSSTableReader(e) => write!(f, "FailedCreatingSSTableReader: {}", e),
        }
    }
}

impl Error for BlockCacheError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnableToAllocateMemory => None,
            Self::MutexLockFailed(_) => None,
            Self::MemoryRecordError(e) => Some(e),
            Self::SSTableReaderError(e) => Some(e),
            Self::IndexBlockNotFound(_, _) => None,
            Self::FailedCreatingSSTableReader(e) => Some(e),
        }
    }
}

impl<T> From<std::sync::PoisonError<std::sync::MutexGuard<'_, T>>> for BlockCacheError {
    fn from(value: std::sync::PoisonError<std::sync::MutexGuard<'_, T>>) -> Self {
        Self::MutexLockFailed(value.to_string())
    }
}

impl From<MemoryRecordError> for BlockCacheError {
    fn from(value: MemoryRecordError) -> Self {
        Self::MemoryRecordError(value)
    }
}

impl From<SSTableReaderError> for BlockCacheError {
    fn from(value: SSTableReaderError) -> Self {
        Self::SSTableReaderError(value)
    }
}
