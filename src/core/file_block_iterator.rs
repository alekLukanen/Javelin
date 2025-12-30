use std::{
    cmp::Ordering,
    error::Error,
    fmt::Display,
    io::{self, Cursor},
    sync::Arc,
};

use crate::core::{
    block_cache::{BlockCache, BlockCacheError, DataBlockHandle},
    buf_utils::{self, BufUtilsError},
    db_context::DBContext,
    entry::{Entry, LogEntry},
    iterator::{IteratorError, SourceIterator},
    sstable_builder::PrefixCompressedEntry,
};

#[derive(Debug)]
pub enum FileBlockIteratorError {
    BlockCacheError(BlockCacheError),
    FileIndexNotFound(u64),
    DataBlockNotFound(u64, u32),
    InvalidCRC32,
    IOError(io::Error),
    RestartLenNotDivisibleByFour,
    BufUtilsError(BufUtilsError),
}

impl Display for FileBlockIteratorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BlockCacheError(err) => write!(f, "BlockCacheError: {}", err),
            Self::FileIndexNotFound(file_id) => write!(f, "FileIndexNotFound: {}", file_id),
            Self::DataBlockNotFound(file_id, block_id) => {
                write!(f, "DatablockNotFound: {}, {}", file_id, block_id)
            }
            Self::InvalidCRC32 => write!(f, "InvalidCRC32"),
            Self::IOError(err) => write!(f, "IOError: {}", err),
            Self::RestartLenNotDivisibleByFour => write!(f, "RestartLenNotDivisibleByFour"),
            Self::BufUtilsError(err) => write!(f, "BufUtilsError: {}", err),
        }
    }
}

impl Error for FileBlockIteratorError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::BlockCacheError(err) => Some(err),
            Self::FileIndexNotFound(_) => None,
            Self::DataBlockNotFound(_, _) => None,
            Self::InvalidCRC32 => None,
            Self::IOError(err) => Some(err),
            Self::RestartLenNotDivisibleByFour => None,
            Self::BufUtilsError(err) => Some(err),
        }
    }
}

impl From<BlockCacheError> for FileBlockIteratorError {
    fn from(value: BlockCacheError) -> Self {
        Self::BlockCacheError(value)
    }
}

impl From<io::Error> for FileBlockIteratorError {
    fn from(value: io::Error) -> Self {
        Self::IOError(value)
    }
}

impl From<BufUtilsError> for FileBlockIteratorError {
    fn from(value: BufUtilsError) -> Self {
        Self::BufUtilsError(value)
    }
}

struct CurrentBlock {
    idx: u32,
    data: DataBlockHandle,

    keys_len: u64,
    key_offset: u64,

    restart_len: u64,

    current_user_key: Option<Vec<u8>>,
}

impl CurrentBlock {
    fn key_cursor(&self) -> Cursor<&[u8]> {
        let mut cursor = Cursor::new(&self.data.data_block_ref()[8..]);
        cursor.set_position(self.key_offset);
        cursor
    }
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
                if current_block.key_offset < current_block.keys_len {
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
                    self.db_context
                        .log_debug("finding block with lower bound".to_string());

                    // find the first block in the file that might contain
                    // the lower_bound
                    let Some(index_block) = self.block_cache.get_index_block(&self.file_id)? else {
                        return Err(FileBlockIteratorError::FileIndexNotFound(
                            self.file_id.clone(),
                        ));
                    };

                    self.db_context
                        .log_debug(format!("index_block.len(): {}", index_block.len(),));

                    let (mut entry_cursor, mut restart_cursor) =
                        buf_utils::entry_and_restart_cursors(&index_block)?;

                    let mut left: u64 = 0;
                    let mut right: u64 = (restart_cursor.get_ref().len() as u64) / 4 - 1;

                    // binary search the keys in the indexes to find the first block
                    let mut lowest_restart_idx = right * 4;
                    while left <= right {
                        let middle = left + (right - left) / 2;
                        let middle_offset = middle * 4;
                        self.db_context.log_debug(format!(
                            "left: {}, right: {}, middle: {}",
                            left, right, middle
                        ));

                        restart_cursor.set_position(middle_offset);
                        let index_key_offset = buf_utils::read_u32(&mut restart_cursor)?;

                        self.db_context
                            .log_debug(format!("index_key_offset: {}", index_key_offset));

                        entry_cursor.set_position(index_key_offset as u64);
                        let index_key_entry = buf_utils::read_entry(&mut entry_cursor)?;

                        self.db_context
                            .log_debug(format!("index_key_entry: {:?}", index_key_entry));

                        let key = index_key_entry.user_key_suffix();

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
                                if middle < lowest_restart_idx {
                                    lowest_restart_idx = middle;
                                }
                            }
                        }
                    }

                    // TODO: improve this so that the iterator doesn't need to scan through
                    // data blocks to find the starting point. Reconstruct the index keys
                    // after finding the lowest restart offset
                    let data_block_id = lowest_restart_idx
                        * self.db_context.config().sstable_restart_interval() as u64;

                    self.set_block(data_block_id as u32)?;
                    self.seek_block_lower_bound()?;

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

        let data = data_block.data_block_ref();
        let mut cursor = Cursor::new(&data[..]);

        let keys_len = buf_utils::read_u64(&mut cursor)?;
        let block_size = data.len() as u64;

        // keys + keys len size
        let max_keys_pos: u64 = keys_len + 8;

        // block size - crc32 and compression size
        let max_restarts_pos: u64 = block_size - 5;

        if (max_restarts_pos - max_restarts_pos)
            % self.db_context.config().sstable_restart_interval() as u64
            != 0
        {
            return Err(FileBlockIteratorError::RestartLenNotDivisibleByFour);
        }

        let restart_len = (max_restarts_pos - max_keys_pos)
            / self.db_context.config().sstable_restart_interval() as u64;

        // parse the crc32 and compression
        if !buf_utils::valid_block_crc32(data)? {
            return Err(FileBlockIteratorError::InvalidCRC32);
        }

        self.current_block = Some(CurrentBlock {
            idx: block_id,
            data: data_block.clone(),
            keys_len: keys_len,
            key_offset: 0,
            restart_len,
            current_user_key: None,
        });

        Ok(())
    }

    fn seek_block_lower_bound(&mut self) -> Result<bool, FileBlockIteratorError> {
        match (&self.lower_bound, &mut self.current_block) {
            (Some(lower_bound), Some(current_block)) => {
                self.db_context
                    .log_debug("seek block lower bound".to_string());

                let data_block = current_block.data.data_block_ref();

                let (mut entry_cursor, mut restart_cursor) =
                    buf_utils::entry_and_restart_cursors(&data_block)?;

                let mut left: u64 = 0;
                let mut right: u64 = (restart_cursor.get_ref().len() as u64) / 4 - 1;

                let mut lowest_entry_offset: u64 = current_block.keys_len;
                while left <= right {
                    let middle = left + (right - left) / 2;
                    let middle_offset = middle * 4;
                    self.db_context.log_debug(format!(
                        "left: {}, right: {}, middle: {}",
                        left, right, middle
                    ));

                    restart_cursor.set_position(middle_offset);
                    let entry_offset = buf_utils::read_u32(&mut restart_cursor)?;

                    entry_cursor.set_position(entry_offset as u64);
                    let key_entry = buf_utils::read_entry(&mut entry_cursor)?;

                    let key = key_entry.user_key_suffix();
                    let log_sequence_num = key_entry.log_seq_num();

                    // compare the key and log_seq_num
                    match &lower_bound[..].cmp(key) {
                        Ordering::Equal => match self.log_sequence_num.cmp(&log_sequence_num) {
                            Ordering::Less => {
                                left = middle + 1;
                                if middle < lowest_entry_offset {
                                    lowest_entry_offset = entry_offset as u64;
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
                            if middle < lowest_entry_offset {
                                lowest_entry_offset = entry_offset as u64;
                            }
                        }
                    }
                }

                self.db_context
                    .log_debug(format!("lowest_entry_offset: {}", lowest_entry_offset));

                current_block.key_offset = lowest_entry_offset;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn build_next_entry(&mut self) -> Result<Option<Arc<LogEntry>>, FileBlockIteratorError> {
        // rebuild the entry from the prefix compressed entries

        let Some(current_block) = &mut self.current_block else {
            return Ok(None);
        };

        if current_block.key_offset >= current_block.keys_len {
            return Ok(None);
        }

        loop {
            let lower_bound = &self.lower_bound;
            let upper_bound = &self.upper_bound;

            match &current_block.current_user_key {
                Some(current_user_key) => {
                    let mut key_cursor = current_block.key_cursor();

                    let entry = buf_utils::read_entry(&mut key_cursor)?;

                    // rebuild the user key
                    let shared_user_key = &current_user_key[0..entry.shared_len as usize];
                    let user_key_suffix = entry.user_key_suffix();
                    let mut temp_user_key =
                        Vec::with_capacity(entry.shared_len as usize + entry.unshared_len as usize);
                    temp_user_key.extend_from_slice(shared_user_key);
                    temp_user_key.extend_from_slice(user_key_suffix);

                    let val = entry.value.clone();
                    let log_seq_num = entry.log_seq_num();
                    let entry_type = entry.entry_type();

                    let new_key_offset = key_cursor.position();

                    current_block.current_user_key = Some(temp_user_key.clone());
                    current_block.key_offset = new_key_offset;

                    if !Self::within_bounds(lower_bound, upper_bound, &temp_user_key) {
                        continue;
                    }

                    return Ok(Some(Self::rebuild_entry(
                        temp_user_key,
                        val,
                        log_seq_num,
                        entry_type,
                    )));
                }
                None => {
                    let mut key_cursor = current_block.key_cursor();

                    let entry = buf_utils::read_entry(&mut key_cursor)?;

                    let user_key = entry.user_key_suffix().to_vec();
                    let val = entry.value.clone();
                    let log_seq_num = entry.log_seq_num();
                    let entry_type = entry.entry_type();
                    let new_key_offset = key_cursor.position();

                    current_block.current_user_key = Some(user_key.clone());
                    current_block.key_offset = new_key_offset;

                    if !Self::within_bounds(lower_bound, upper_bound, &user_key) {
                        continue;
                    }

                    return Ok(Some(Self::rebuild_entry(
                        user_key,
                        val,
                        log_seq_num,
                        entry_type,
                    )));
                }
            }
        }
    }

    #[inline]
    fn within_bounds(
        lower_bound: &Option<Vec<u8>>,
        upper_bound: &Option<Vec<u8>>,
        key: &Vec<u8>,
    ) -> bool {
        match lower_bound {
            Some(lower_bound) => match &lower_bound[..].cmp(key) {
                Ordering::Greater => return false,
                _ => {}
            },
            None => {}
        }
        match upper_bound {
            Some(upper_bound) => match &upper_bound[..].cmp(key) {
                Ordering::Less => return false,
                _ => {}
            },
            None => {}
        }
        true
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

        match &self.current_block {
            Some(_) => {
                let row = self.build_next_entry()?;
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
