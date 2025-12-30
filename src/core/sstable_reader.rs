use std::io::prelude::*;
use std::io::{self, Cursor, Read, SeekFrom};
use std::{error::Error, fmt::Display, fs::File, path::PathBuf, sync::Arc};

use crc::{CRC_32_CKSUM, Crc};

use crate::core::buf_utils;
use crate::core::db_context::DBContext;
use crate::core::sstable_builder::{BlockHandle, DataBlock, FooterBlock, PrefixCompressedEntry};

const CRC32: Crc<u32> = Crc::<u32>::new(&CRC_32_CKSUM);

#[derive(Debug)]
pub enum SSTableReaderError {
    IOError(io::Error),
    FileTooSmallForFooter(u64),
    FileTooSmallForDataBlock(u64),
    InvalidMagic(u64, u64),
    InvalidCRC32,
    EntryMalformed(String),
}

impl Display for SSTableReaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IOError(e) => write!(f, "IOError: {}", e),
            Self::FileTooSmallForFooter(size) => {
                write!(f, "FileTooSmallForFooter: file size={}", size)
            }
            Self::FileTooSmallForDataBlock(size) => {
                write!(f, "FileTooSmallForDatablock: file size={}", size)
            }
            Self::InvalidMagic(valid, magic) => {
                write!(f, "InvalidMagic: valid={}, actual={}", valid, magic)
            }
            Self::InvalidCRC32 => {
                write!(f, "InvalidCRC32")
            }
            Self::EntryMalformed(reason) => {
                write!(f, "EntryMalformed: reason={}", reason)
            }
        }
    }
}

impl Error for SSTableReaderError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::IOError(e) => Some(e),
            Self::FileTooSmallForFooter(_) => None,
            Self::FileTooSmallForDataBlock(_) => None,
            Self::InvalidMagic(_, _) => None,
            Self::InvalidCRC32 => None,
            Self::EntryMalformed(_) => None,
        }
    }
}

impl From<io::Error> for SSTableReaderError {
    fn from(value: io::Error) -> Self {
        Self::IOError(value)
    }
}

///////////////////////////////////////////////////

struct BlockContents {
    keys: Vec<PrefixCompressedEntry>,
    restarts: Vec<u32>,
}

pub struct SSTableReader {
    db_context: Arc<DBContext>,

    file: File,

    footer: Option<FooterBlock>,
}

impl SSTableReader {
    pub fn new(
        db_context: Arc<DBContext>,
        file_path: PathBuf,
    ) -> Result<SSTableReader, SSTableReaderError> {
        let file = File::open(file_path)?;
        Ok(SSTableReader {
            db_context,
            file,
            footer: None,
        })
    }

    pub fn index_block(&mut self) -> Result<DataBlock, SSTableReaderError> {
        let footer = self.footer_block()?;

        self.data_block(&footer.index_block_handle)
    }

    pub fn data_block(
        &mut self,
        block_handle: &BlockHandle,
    ) -> Result<DataBlock, SSTableReaderError> {
        let file_len = self.file.metadata()?.len();
        let start_pos = block_handle.offset;
        if start_pos + block_handle.size > file_len {
            return Err(SSTableReaderError::FileTooSmallForDataBlock(file_len));
        }
        self.file.seek(SeekFrom::Start(start_pos))?;

        self.db_context
            .log_debug(format!("file_len={}, start_pos={}", file_len, start_pos));

        self.db_context.log_debug(format!(
            "block_handle.offset={}, block_handle.size={}",
            block_handle.offset, block_handle.size
        ));

        // load the entire block into the buffer
        let buf = buf_utils::file_read_n(&mut self.file, block_handle.size as usize)?;
        let mut cursor = Cursor::new(&buf[..]);

        let keys_len = buf_utils::read_u64(&mut cursor)?;
        let block_size = block_handle.size;

        // keys + keys len size
        let max_keys_pos = keys_len + 8;

        // block size - crc32 and compression size
        let max_restarts_pos = block_size - 5;

        // parse the crc32 and compression
        if !buf_utils::valid_block_crc32(&buf)? {
            return Err(SSTableReaderError::InvalidCRC32);
        }

        // parse the entries
        let mut entries: Vec<PrefixCompressedEntry> = Vec::new();
        while cursor.position() < max_keys_pos {
            let shared_len = buf_utils::read_u32(&mut cursor)?;
            let unshared_len = buf_utils::read_u32(&mut cursor)?;
            let value_len = buf_utils::read_u32(&mut cursor)?;

            self.db_context.log_debug(format!(
                "shared_len={}, unshared_len={}, value_len={}, cursor.position()={}, keys_len={}, max_keys_pos={}",
                shared_len,
                unshared_len,
                value_len,
                cursor.position(),
                keys_len,
                max_keys_pos,
            ));

            // validate the lengths
            if (unshared_len + 9 + value_len) as u64 + cursor.position() > max_keys_pos as u64 {
                return Err(SSTableReaderError::EntryMalformed(
                    "prefix compressed entry length longer than cursor".to_string(),
                ));
            }

            let key_suffix = buf_utils::read_n(&mut cursor, unshared_len as usize + 9)?;
            let value = buf_utils::read_n(&mut cursor, value_len as usize)?;
            let entry = PrefixCompressedEntry {
                shared_len,
                unshared_len,
                value_len,
                key_suffix,
                value,
            };

            self.db_context.log_debug(format!("entry: {:?}", entry));
            entries.push(entry)
        }

        self.db_context.log_debug("read all entries".to_string());
        self.db_context.log_debug(format!(
            "cursor.position()={}, max_restarts_pos={}",
            cursor.position(),
            max_restarts_pos,
        ));

        // parse the restarts
        let mut restarts: Vec<u32> = Vec::new();
        while cursor.position() < max_restarts_pos {
            let restart = buf_utils::read_u32(&mut cursor)?;
            restarts.push(restart);
        }

        assert_eq!(max_restarts_pos as u64, cursor.position());

        Ok(DataBlock {
            keys: entries,
            restarts: restarts,
        })
    }

    pub fn footer_block(&mut self) -> Result<FooterBlock, SSTableReaderError> {
        if let Some(footer_block) = &self.footer {
            return Ok(footer_block.clone());
        }

        let header_size: u64 = 8 + 16 + 16 + 1 + 4;

        let file_len = self.file.metadata()?.len();
        let start_pos = file_len.saturating_sub(header_size);
        if file_len < header_size {
            return Err(SSTableReaderError::FileTooSmallForFooter(file_len));
        }
        self.file.seek(SeekFrom::Start(start_pos))?;

        let mut buf: Vec<u8> = Vec::with_capacity(header_size as usize);
        self.file.read_to_end(&mut buf)?;

        let mut cursor = Cursor::new(&buf[..]);

        // decode the header data into the header block
        let magic: u64 = buf_utils::read_u64(&mut cursor)?;
        let data_block_handle = buf_utils::read_handle(&mut cursor)?;
        let index_block_handle = buf_utils::read_handle(&mut cursor)?;

        if !buf_utils::valid_block_crc32(&buf)? {
            return Err(SSTableReaderError::InvalidCRC32);
        }

        if magic != 69 {
            return Err(SSTableReaderError::InvalidMagic(69, magic));
        }

        let footer_block = FooterBlock {
            magic,
            data_block_handle,
            index_block_handle,
        };
        self.footer = Some(footer_block.clone());
        Ok(footer_block.clone())
    }
}
