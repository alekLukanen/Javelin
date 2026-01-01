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
};

#[derive(Debug)]
pub enum FileBlockIteratorError {
    BlockCacheError(BlockCacheError),
    FileIndexNotFound(u64),
    FileMetaDataNotFound(u64),
    DataBlockNotFound(u64, u32),
    InvalidCRC32,
    IOError(io::Error),
    RestartLenNotDivisibleByFour,
    BufUtilsError(BufUtilsError),
}

impl Display for FileBlockIteratorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BlockCacheError(err) => {
                write!(f, "FileBlockIteratorError::BlockCacheError: {}", err)
            }
            Self::FileIndexNotFound(file_id) => {
                write!(f, "FileBlockIteratorError::FileIndexNotFound: {}", file_id)
            }
            Self::FileMetaDataNotFound(file_id) => write!(
                f,
                "FileBlockIteratorError::FileMetaDataNotFound: {}",
                file_id
            ),
            Self::DataBlockNotFound(file_id, block_id) => {
                write!(
                    f,
                    "FileBlockIteratorError::DatablockNotFound: {}, {}",
                    file_id, block_id
                )
            }
            Self::InvalidCRC32 => write!(f, "FileBlockIteratorError::InvalidCRC32"),
            Self::IOError(err) => write!(f, "FileBlockIteratorError::IOError: {}", err),
            Self::RestartLenNotDivisibleByFour => {
                write!(f, "FileBlockIteratorError::RestartLenNotDivisibleByFour")
            }
            Self::BufUtilsError(err) => write!(f, "FileBlockIteratorError::BufUtilsError: {}", err),
        }
    }
}

impl Error for FileBlockIteratorError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::BlockCacheError(err) => Some(err),
            Self::FileIndexNotFound(_) => None,
            Self::FileMetaDataNotFound(_) => None,
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

struct CurrentUserKey {
    start_pos: u64,
    len: u64,
}

struct CurrentBlock {
    idx: u32,
    data: DataBlockHandle,

    keys_len: u64,
    key_offset: u64,

    current_user_key: Option<Vec<u8>>,
}

impl CurrentBlock {
    fn key_cursor(&self) -> Cursor<&[u8]> {
        let mut cursor = Cursor::new(&self.data.data_block_ref()[8..]);
        cursor.set_position(self.key_offset);
        cursor
    }
    fn key_data(&self) -> Option<&[u8]> {
        Some(&self.data.data_block_ref()[8..])
    }
}

pub struct FileBlockIterator {
    db_context: Arc<DBContext>,
    block_cache: Arc<BlockCache>,

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
        file_id: u64,
        log_sequence_num: u64,
        lower_bound: Option<Vec<u8>>,
        upper_bound: Option<Vec<u8>>,
    ) -> FileBlockIterator {
        FileBlockIterator {
            db_context,
            block_cache,
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
                let Some(meta_block_data) = self.block_cache.get_meta_block(&self.file_id)? else {
                    return Err(FileBlockIteratorError::FileMetaDataNotFound(self.file_id));
                };
                let meta_data = buf_utils::decode_meta_data(meta_block_data.as_ref())?;

                let next_block_idx = current_block.idx + 1;
                if next_block_idx >= meta_data.num_blocks {
                    return Ok(false);
                }

                match self.set_block(next_block_idx) {
                    Ok(_) => Ok(true),
                    Err(FileBlockIteratorError::DataBlockNotFound(_, _)) => Ok(false),
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

                    let (mut entry_cursor, mut restart_cursor) =
                        buf_utils::entry_and_restart_cursors(&index_block)?;

                    let mut left: u64 = 0;
                    let mut right: u64 = (restart_cursor.get_ref().len() as u64) / 4 - 1;

                    if right > 500_000 {
                        self.db_context.log_info(format!("right={}", right));
                    }

                    // binary search the keys in the indexes to find the first block
                    let mut lowest_restart_idx = right * 4;
                    while left <= right {
                        let middle = left + (right - left) / 2;
                        let middle_offset = middle * 4;

                        restart_cursor.set_position(middle_offset);
                        let index_key_offset = buf_utils::read_u32(&mut restart_cursor)
                            .expect(format!("issue with index_key_offset | middle={} left={} right={} middle_offset={}", middle, left, right, middle_offset).as_str());

                        entry_cursor.set_position(index_key_offset as u64);
                        let index_key_entry = buf_utils::read_entry(&mut entry_cursor)
                            .expect(format!("issue with index_key_entry | middle={} left={} right={} middle_offset={}", middle, left, right, middle_offset).as_str());

                        let key = index_key_entry.user_key_suffix();

                        // compare the key and log_seq_num
                        match &lower_bound[..].cmp(key) {
                            Ordering::Less => {
                                if left == 0 && right == 0 {
                                    if middle < lowest_restart_idx {
                                        lowest_restart_idx = middle;
                                    }
                                    break;
                                } else {
                                    if middle == 0 {
                                        return Ok(false);
                                    }
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

                    self.set_block(data_block_id.clone() as u32)?;
                    let _ = self.seek_block_lower_bound()?;

                    Ok(true)
                }
                None => {
                    self.set_block(0)?;
                    Ok(true)
                }
            },
        }
    }

    fn set_block(&mut self, block_id: u32) -> Result<(), FileBlockIteratorError> {
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

        // block size - crc32 and compression size
        let max_restarts_pos: u64 = block_size - 5;

        let (_, restart_pos) = buf_utils::restart_start_and_end_offsets(data)?;

        if (max_restarts_pos - restart_pos)
            % self.db_context.config().sstable_restart_interval() as u64
            != 0
        {
            return Err(FileBlockIteratorError::RestartLenNotDivisibleByFour);
        }

        self.current_block = Some(CurrentBlock {
            idx: block_id,
            data: data_block.clone(),
            keys_len: keys_len,
            key_offset: 0,
            current_user_key: None,
        });

        Ok(())
    }

    fn seek_block_lower_bound(&mut self) -> Result<bool, FileBlockIteratorError> {
        match (&self.lower_bound, &mut self.current_block) {
            (Some(lower_bound), Some(current_block)) => {
                let data_block = current_block.data.data_block_ref();

                let (mut entry_cursor, mut restart_cursor) =
                    buf_utils::entry_and_restart_cursors(&data_block)?;

                let mut left: u64 = 0;
                let mut right: u64 = (restart_cursor.get_ref().len() as u64) / 4 - 1;

                let mut lowest_entry_offset: Option<u64> = None;
                while left <= right {
                    let middle = left + (right - left) / 2;
                    let middle_offset = middle * 4;

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
                                if Some(entry_offset as u64) < lowest_entry_offset {
                                    lowest_entry_offset = Some(entry_offset as u64);
                                }
                            }
                            Ordering::Equal | Ordering::Greater => {
                                if left == 0 && right == 0 {
                                    lowest_entry_offset = Some(entry_offset as u64);
                                    break;
                                } else {
                                    right = middle - 1;
                                }
                            }
                        },
                        Ordering::Less => {
                            if left == 0 && right == 0 {
                                lowest_entry_offset = Some(entry_offset as u64);
                                break;
                            } else {
                                right = middle - 1;
                            }
                        }
                        Ordering::Greater => {
                            left = middle + 1;
                            if Some(entry_offset as u64) < lowest_entry_offset {
                                lowest_entry_offset = Some(entry_offset as u64);
                            }
                        }
                    }
                }

                if let Some(lowest_entry_offset) = lowest_entry_offset {
                    current_block.key_offset = lowest_entry_offset;
                } else {
                    current_block.key_offset = 0;
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn build_next_entry(&mut self) -> Result<Option<Arc<LogEntry>>, FileBlockIteratorError> {
        // rebuild the entry from the prefix compressed entries
        // TODO: need to also check the log sequence number

        let Some(current_block) = &mut self.current_block else {
            return Ok(None);
        };

        let lower_bound = &self.lower_bound;
        let upper_bound = &self.upper_bound;

        while current_block.key_offset < current_block.keys_len {
            match &current_block.current_user_key {
                Some(current_user_key) => {
                    let key_data = current_block.key_data().unwrap();
                    let entry_buf_ref =
                        buf_utils::read_entry_buf_ref(key_data, current_block.key_offset as usize)?;

                    // rebuild the user key
                    let shared_user_key = &current_user_key[0..entry_buf_ref.shared_len as usize];
                    let user_key_suffix = entry_buf_ref.key_suffix;
                    let mut temp_user_key = Vec::with_capacity(
                        entry_buf_ref.shared_len as usize + entry_buf_ref.unshared_len as usize,
                    );
                    temp_user_key.extend_from_slice(shared_user_key);
                    temp_user_key.extend_from_slice(user_key_suffix);

                    if !Self::within_lower_bound(lower_bound, &temp_user_key) {
                        current_block.key_offset = entry_buf_ref.entry_end_pos as u64;
                        current_block.current_user_key = Some(temp_user_key.clone());

                        continue;
                    }
                    if !Self::within_upper_bound(upper_bound, &temp_user_key) {
                        current_block.key_offset = entry_buf_ref.entry_end_pos as u64;
                        current_block.current_user_key = Some(temp_user_key.clone());

                        break;
                    }

                    let log_seq_num = entry_buf_ref.log_seq_num;
                    let entry_type = entry_buf_ref.entry_type;
                    let val = entry_buf_ref.value.to_vec();

                    current_block.key_offset = entry_buf_ref.entry_end_pos as u64;
                    current_block.current_user_key = Some(temp_user_key.clone());

                    return Ok(Some(Self::rebuild_entry(
                        temp_user_key,
                        val,
                        log_seq_num,
                        entry_type,
                    )));
                }
                None => {
                    let key_data = current_block.key_data().unwrap();
                    let entry_buf_ref =
                        buf_utils::read_entry_buf_ref(key_data, current_block.key_offset as usize)?;

                    let user_key = entry_buf_ref.key_suffix;
                    let new_key_offset = entry_buf_ref.entry_end_pos;

                    if !Self::within_lower_bound(lower_bound, user_key) {
                        current_block.current_user_key = Some(user_key.to_vec());
                        current_block.key_offset = new_key_offset as u64;

                        continue;
                    }
                    if !Self::within_upper_bound(upper_bound, user_key) {
                        current_block.current_user_key = Some(user_key.to_vec());
                        current_block.key_offset = new_key_offset as u64;

                        break;
                    }

                    let val = entry_buf_ref.value.to_vec();
                    let log_seq_num = entry_buf_ref.log_seq_num;
                    let entry_type = entry_buf_ref.entry_type;
                    let user_key = entry_buf_ref.key_suffix.to_vec();

                    current_block.current_user_key = Some(user_key.to_vec());
                    current_block.key_offset = new_key_offset as u64;

                    return Ok(Some(Self::rebuild_entry(
                        user_key.to_vec(),
                        val,
                        log_seq_num,
                        entry_type,
                    )));
                }
            }
        }

        Ok(None)
    }

    #[inline]
    fn within_lower_bound(lower_bound: &Option<Vec<u8>>, key: &[u8]) -> bool {
        match lower_bound {
            Some(lower_bound) => match &lower_bound[..].cmp(key) {
                Ordering::Greater => return false,
                _ => true,
            },
            None => true,
        }
    }

    #[inline]
    fn within_upper_bound(upper_bound: &Option<Vec<u8>>, key: &[u8]) -> bool {
        match upper_bound {
            Some(upper_bound) => match &upper_bound[..].cmp(key) {
                Ordering::Less => false,
                _ => true,
            },
            None => true,
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
        loop {
            if !self.find_block()? {
                self.current_entry = None;
                return Ok(None);
            }

            match &self.current_block {
                Some(_) => match self.build_next_entry()? {
                    Some(entry) => {
                        self.current_entry = Some(entry.clone());
                        return Ok(Some(entry));
                    }
                    None => continue,
                },
                None => return Ok(None),
            }
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
