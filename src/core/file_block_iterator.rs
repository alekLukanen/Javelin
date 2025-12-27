use std::{error::Error, fmt::Display, sync::Arc};

use crate::core::{
    block_cache::{BlockCache, BlockCacheError},
    db::ReadState,
    entry::LogEntry,
    iterator::{IteratorError, SourceIterator},
    sstable_builder::DataBlock,
};

#[derive(Debug)]
pub enum FileBlockIteratorError {
    BlockCacheError(BlockCacheError),
    FileIndexNotFound(u64),
}

impl Display for FileBlockIteratorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BlockCacheError(err) => write!(f, "BlockCacheError: {}", err),
            Self::FileIndexNotFound(file_id) => write!(f, "FileIndexNotFound: {}", file_id),
        }
    }
}

impl Error for FileBlockIteratorError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::BlockCacheError(err) => Some(err),
            Self::FileIndexNotFound(_) => None,
        }
    }
}

impl From<BlockCacheError> for FileBlockIteratorError {
    fn from(value: BlockCacheError) -> Self {
        FileBlockIteratorError::BlockCacheError(value)
    }
}

pub struct FileBlockIterator {
    read_state: Arc<ReadState>,
    block_cache: Arc<BlockCache>,

    level: usize,
    file_id: u64,
    log_sequence_num: u64,
    lower_bound: Option<Vec<u8>>,
    upper_bound: Option<Vec<u8>>,

    current_block_idx: u32,
    current_block: Option<DataBlock>,
    current_block_row_idx: usize,

    current_entry: Option<Arc<LogEntry>>,
}

impl FileBlockIterator {
    pub fn new(
        read_state: Arc<ReadState>,
        block_cache: Arc<BlockCache>,
        level: usize,
        file_id: u64,
        log_sequence_num: u64,
        lower_bound: Option<Vec<u8>>,
        upper_bound: Option<Vec<u8>>,
    ) -> FileBlockIterator {
        FileBlockIterator {
            read_state,
            block_cache,
            level,
            file_id,

            log_sequence_num,
            lower_bound,
            upper_bound,

            current_block_idx: 0,
            current_block: None,
            current_block_row_idx: 0,
            current_entry: None,
        }
    }

    fn get_block(&mut self) -> Result<Option<DataBlock>, FileBlockIteratorError> {
        match &self.current_block {
            Some(current_block) => Ok(None),
            None => {
                // find the first block in the file that might contain
                // the lower_bound
                let Some(index_block) = self.block_cache.get_index_block(&self.file_id)? else {
                    return Err(FileBlockIteratorError::FileIndexNotFound(
                        self.file_id.clone(),
                    ));
                };

                // binary search the keys in the indexes to find the first block

                Ok(None)
            }
        }
    }

    fn next_iter_entry(&mut self) -> Result<Option<Arc<LogEntry>>, FileBlockIteratorError> {
        Ok(None)
    }
}

impl SourceIterator for FileBlockIterator {
    fn next(&mut self) -> Result<Option<Arc<LogEntry>>, IteratorError> {
        Ok(None)
    }
    fn current(&self) -> Option<Arc<LogEntry>> {
        None
    }
}
