use std::{cmp::Ordering, error::Error, fmt::Display, sync::Arc};

use crate::core::{
    block_cache::{BlockCache, BlockCacheError, DataBlockHandle},
    db::ReadState,
    db_context::DBContext,
    entry::{Entry, LogEntry},
    iterator::{IteratorError, SourceIterator},
};

#[derive(Debug)]
pub enum FileBlockIteratorError {
    BlockCacheError(BlockCacheError),
    FileIndexNotFound(u64),
    DataBlockNotFound(u64, u32),
}

impl Display for FileBlockIteratorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BlockCacheError(err) => write!(f, "BlockCacheError: {}", err),
            Self::FileIndexNotFound(file_id) => write!(f, "FileIndexNotFound: {}", file_id),
            Self::DataBlockNotFound(file_id, block_id) => {
                write!(f, "DatablockNotFound: {}, {}", file_id, block_id)
            }
        }
    }
}

impl Error for FileBlockIteratorError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::BlockCacheError(err) => Some(err),
            Self::FileIndexNotFound(_) => None,
            Self::DataBlockNotFound(_, _) => None,
        }
    }
}

impl From<BlockCacheError> for FileBlockIteratorError {
    fn from(value: BlockCacheError) -> Self {
        FileBlockIteratorError::BlockCacheError(value)
    }
}

struct CurrentBlock {
    idx: u32,
    data: DataBlockHandle,
    row_idx: usize,
    restart_idx: usize,
    current_user_key: Option<Vec<u8>>,
}

pub struct FileBlockIterator {
    db_context: Arc<DBContext>,
    block_cache: Arc<BlockCache>,

    level: usize,
    file_id: u64,
    log_sequence_num: u64,
    lower_bound: Option<Vec<u8>>,
    upper_bound: Option<Vec<u8>>,

    current_block: Option<CurrentBlock>,
    current_entry: Option<Arc<LogEntry>>,
}

impl FileBlockIterator {
    pub fn new(
        db_context: Arc<DBContext>,
        block_cache: Arc<BlockCache>,
        level: usize,
        file_id: u64,
        log_sequence_num: u64,
        lower_bound: Option<Vec<u8>>,
        upper_bound: Option<Vec<u8>>,
    ) -> FileBlockIterator {
        FileBlockIterator {
            db_context,
            block_cache,
            level,
            file_id,

            log_sequence_num,
            lower_bound,
            upper_bound,

            current_block: None,
            current_entry: None,
        }
    }

    fn find_block(&mut self) -> Result<bool, FileBlockIteratorError> {
        match &self.current_block {
            Some(current_block) => {
                if current_block.row_idx < current_block.data.data_block_ref().keys.len() - 1 {
                    return Ok(true);
                }

                // find the next block if it exists
                let next_block_idx = current_block.idx + 1;
                match self.set_block(next_block_idx) {
                    Ok(_) => {
                        self.db_context
                            .log_debug(format!("set block to: {}", next_block_idx));
                        Ok(true)
                    }
                    Err(FileBlockIteratorError::DataBlockNotFound(_, _)) => {
                        self.db_context.log_debug(format!("at end of sstable"));
                        Ok(false)
                    }
                    Err(err) => Err(err),
                }
            }
            ///////////////////////////////////////////////////////////
            // fetch the initial block depending on the configuration
            None => match &self.lower_bound {
                Some(lower_bound) => {
                    // find the first block in the file that might contain
                    // the lower_bound
                    let Some(index_block) = self.block_cache.get_index_block(&self.file_id)? else {
                        return Err(FileBlockIteratorError::FileIndexNotFound(
                            self.file_id.clone(),
                        ));
                    };

                    self.db_context.log_debug(format!(
                        "restarts.len(): {}, keys.len(): {}",
                        index_block.restarts.len(),
                        index_block.keys.len()
                    ));

                    // binary search the keys in the indexes to find the first block
                    let mut left = 0;
                    let mut right = index_block.restarts.len() - 1;
                    let mut lowest_data_block = right;
                    while left <= right {
                        let middle = left + (right - left) / 2;
                        self.db_context.log_debug(format!(
                            "left: {}, right: {}, middle: {}",
                            left, right, middle
                        ));

                        let sample_entry = &index_block.keys.get(middle).expect("expected entry");
                        let key = sample_entry.user_key_suffix();

                        // compare the key and log_seq_num
                        match &lower_bound[..].cmp(key) {
                            Ordering::Less => {
                                if left == 0 && right == 0 {
                                    break;
                                } else {
                                    right = middle - 1;
                                }
                            }
                            Ordering::Equal | Ordering::Greater => {
                                left = middle + 1;
                                if middle < lowest_data_block {
                                    lowest_data_block = middle;
                                }
                            }
                        }
                    }

                    self.set_block(lowest_data_block as u32)?;
                    self.seek_block_lower_bound();

                    Ok(true)
                }
                None => {
                    self.db_context
                        .log_debug("setting up initial block".to_string());
                    self.set_block(0)?;
                    Ok(true)
                }
            },
        }
    }

    fn set_block(&mut self, block_id: u32) -> Result<(), FileBlockIteratorError> {
        self.db_context
            .log_debug(format!("set_block: block_id={}", block_id));

        let Some(data_block) = self.block_cache.get_data_block(&self.file_id, &block_id)? else {
            return Err(FileBlockIteratorError::DataBlockNotFound(
                self.file_id.clone(),
                block_id.clone(),
            ));
        };

        self.current_block = Some(CurrentBlock {
            idx: block_id,
            data: data_block.clone(),
            row_idx: 0,
            restart_idx: 0,
            current_user_key: None,
        });

        Ok(())
    }

    fn seek_block_lower_bound(&mut self) -> bool {
        match (&self.lower_bound, &mut self.current_block) {
            (Some(lower_bound), Some(current_block)) => {
                let data_block = current_block.data.data_block_ref();

                let mut left = 0;
                let mut right = data_block.restarts.len() - 1;
                let mut lowest_entry = data_block.restarts.len();
                while left <= right {
                    let middle = left + (right - left) / 2;
                    self.db_context.log_debug(format!(
                        "left: {}, right: {}, middle: {}",
                        left, right, middle
                    ));

                    let restart_row_idx =
                        *data_block.restarts.get(middle).expect("expected restart") as usize;

                    self.db_context
                        .log_debug(format!("restart_row_idx: {}", restart_row_idx));

                    let sample_entry = data_block
                        .keys
                        .get(restart_row_idx)
                        .expect("expected entry");
                    let key = sample_entry.user_key_suffix();
                    let log_sequence_num = sample_entry.log_seq_num();

                    // compare the key and log_seq_num
                    match &lower_bound[..].cmp(key) {
                        Ordering::Equal => match self.log_sequence_num.cmp(&log_sequence_num) {
                            Ordering::Less => {
                                left = middle + 1;
                                if middle < lowest_entry {
                                    lowest_entry = middle;
                                }
                            }
                            Ordering::Equal | Ordering::Greater => {
                                if left == 0 && right == 0 {
                                    break;
                                } else {
                                    right = middle - 1;
                                }
                            }
                        },
                        Ordering::Less => {
                            if left == 0 && right == 0 {
                                break;
                            } else {
                                right = middle - 1;
                            }
                        }
                        Ordering::Greater => {
                            left = middle + 1;
                            if middle < lowest_entry {
                                lowest_entry = middle;
                            }
                        }
                    }
                }

                if lowest_entry != data_block.keys.len() {
                    current_block.row_idx = lowest_entry;
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn build_next_entry(current_block: &mut CurrentBlock) -> Option<Arc<LogEntry>> {
        // rebuild the entry from the prefix compressed entries
        let data_block = current_block.data.data_block_ref();

        if current_block.row_idx >= data_block.keys.len() {
            return None;
        }

        match &mut current_block.current_user_key {
            Some(current_user_key) => {
                current_block.row_idx += 1;

                let restart_row_idx = data_block
                    .restarts
                    .get(current_block.restart_idx)
                    .expect("expected restart row");

                // rebuild the user key
                let row = data_block.keys.get(current_block.row_idx).unwrap();
                let log_seq_num = row.log_seq_num();
                let entry_type = row.entry_type();

                if *restart_row_idx as usize != current_block.row_idx {
                    let shared_user_key = &current_user_key[0..row.shared_len as usize];
                    let user_key_suffix = row.user_key_suffix();
                    let mut temp_user_key =
                        Vec::with_capacity(row.shared_len as usize + row.unshared_len as usize);
                    temp_user_key.extend_from_slice(shared_user_key);
                    temp_user_key.extend_from_slice(user_key_suffix);

                    current_block.current_user_key = Some(temp_user_key.clone());

                    Some(Self::rebuild_entry(
                        temp_user_key,
                        row.value.clone(),
                        log_seq_num,
                        entry_type,
                    ))
                } else {
                    let user_key = row.user_key_suffix().to_vec();

                    current_block.current_user_key = Some(user_key.clone());

                    Some(Self::rebuild_entry(
                        user_key,
                        row.value.clone(),
                        log_seq_num,
                        entry_type,
                    ))
                }
            }
            None => {
                let Some(first_row) = data_block.keys.get(current_block.row_idx) else {
                    return None;
                };
                let user_key = first_row.user_key_suffix().to_vec();
                let val = first_row.value.clone();
                let log_seq_num = first_row.log_seq_num();
                let entry_type = first_row.entry_type();

                current_block.current_user_key = Some(user_key.clone());

                Some(Self::rebuild_entry(user_key, val, log_seq_num, entry_type))
            }
        }
    }

    #[inline]
    fn rebuild_entry(
        user_key: Vec<u8>,
        val: Vec<u8>,
        log_seq_num: u64,
        entry_type: u8,
    ) -> Arc<LogEntry> {
        match entry_type {
            0 => Arc::new(LogEntry::new(Entry::Del { key: user_key }, log_seq_num)),
            1 => Arc::new(LogEntry::new(
                Entry::Put { key: user_key, val },
                log_seq_num,
            )),
            _ => {
                panic!("unknown entry_type {}", entry_type);
            }
        }
    }

    fn next_iter_entry(&mut self) -> Result<Option<Arc<LogEntry>>, FileBlockIteratorError> {
        if !self.find_block()? {
            self.current_entry = None;
            return Ok(None);
        }

        match &mut self.current_block {
            Some(current_block) => {
                let row = Self::build_next_entry(current_block);
                Ok(row)
            }
            None => Ok(None),
        }
    }
}

impl SourceIterator for FileBlockIterator {
    fn next(&mut self) -> Result<Option<Arc<LogEntry>>, IteratorError> {
        match self.next_iter_entry() {
            Ok(val) => Ok(val),
            Err(err) => Err(IteratorError::SourceError(Box::new(err))),
        }
    }
    fn current(&self) -> Option<Arc<LogEntry>> {
        self.current_entry.clone()
    }
}
