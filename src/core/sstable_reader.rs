use std::io::prelude::*;
use std::io::{self, Cursor, Read, SeekFrom};
use std::{error::Error, fmt::Display, fs::File, path::PathBuf, sync::Arc};

use crc::{CRC_32_CKSUM, Crc};

use crate::core::buf_utils::{self, BufUtilsError};
use crate::core::db_context::DBContext;
use crate::core::sstable_builder::{
    BlockHandle, DataBlock, FooterBlock, MetaDataBlock, PrefixCompressedEntry,
};

const CRC32: Crc<u32> = Crc::<u32>::new(&CRC_32_CKSUM);

#[derive(Debug)]
pub enum SSTableReaderError {
    IOError(io::Error),
    FileTooSmallForFooter(u64),
    FileTooSmallForDataBlock(u64),
    InvalidMagic(u64, u64),
    InvalidCRC32,
    EntryMalformed(String),
    BufUtilsError(BufUtilsError),
    BlockNotFoundById(u32),
    BlockIdOutOfBounds(u32, u32),
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
            Self::BufUtilsError(err) => {
                write!(f, "BufUtilsError: {}", err)
            }
            Self::BlockNotFoundById(block_id) => {
                write!(f, "BlockNotFoundById: block_id={}", block_id)
            }
            Self::BlockIdOutOfBounds(block_id, num_blocks) => {
                write!(
                    f,
                    "BlockIdOutOfBounds: block_id={}, num_blocks: {}",
                    block_id, num_blocks
                )
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
            Self::BufUtilsError(err) => Some(err),
            Self::BlockNotFoundById(_) => None,
            Self::BlockIdOutOfBounds(_, _) => None,
        }
    }
}

impl From<io::Error> for SSTableReaderError {
    fn from(value: io::Error) -> Self {
        Self::IOError(value)
    }
}

impl From<BufUtilsError> for SSTableReaderError {
    fn from(value: BufUtilsError) -> Self {
        Self::BufUtilsError(value)
    }
}

///////////////////////////////////////////////////

pub struct SSTableReader {
    db_context: Arc<DBContext>,

    file: File,
    footer: Option<FooterBlock>,
    meta: Option<MetaDataBlock>,
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
            meta: None,
        })
    }

    pub fn index_block(&mut self) -> Result<Vec<u8>, SSTableReaderError> {
        let footer = self.decoded_footer_block()?;

        self.data_block(&footer.index_block_handle)
    }

    pub fn get_block(
        &mut self,
        block_id: &u32,
        index_block: &Vec<u8>,
    ) -> Result<Vec<u8>, SSTableReaderError> {
        self.db_context
            .log_debug(format!("[SSTableReader] get block {}", block_id));

        let meta_block = self.decoded_meta_block()?;
        if *block_id > meta_block.num_blocks - 1 {
            return Err(SSTableReaderError::BlockIdOutOfBounds(
                *block_id,
                meta_block.num_blocks,
            ));
        }

        let (mut entry_cursor, mut restart_cursor) =
            buf_utils::entry_and_restart_cursors(&index_block)?;

        let restart_interval = self.db_context.config().sstable_restart_interval() as u64;

        // get the restart interval closet to the block handle
        let restart_idx = *block_id as u64 / restart_interval;
        let restart_entry_idx = restart_idx * restart_interval;
        let restart_pos = restart_idx * 4;

        restart_cursor.set_position(restart_pos);
        let index_entry_offset = buf_utils::read_u32(&mut restart_cursor)?;

        // set the entry cursor to the restart offset
        entry_cursor.set_position(index_entry_offset as u64);

        // with the restart position get the index entry by cycling forward
        // until the block entry of interest is found

        // skip these entries
        for idx in restart_entry_idx..(*block_id as u64) {
            self.db_context.log_debug(format!(
                "[SSTableReader] skipping entry {} | position={}, len={}",
                idx,
                entry_cursor.position(),
                entry_cursor.get_ref().len()
            ));

            let entry = buf_utils::read_entry(&mut entry_cursor)?;
            self.db_context
                .log_debug(format!("[SSTableReader] entry={:?}", entry));

            if entry_cursor.position() == entry_cursor.get_ref().len() as u64 {
                return Err(SSTableReaderError::BlockNotFoundById(*block_id));
            }
        }

        self.db_context.log_debug(format!(
            "outside of loop | position={}, len={}",
            entry_cursor.position(),
            entry_cursor.get_ref().len()
        ));

        let entry = buf_utils::read_entry(&mut entry_cursor)?;
        let block_handle = buf_utils::read_handle(&mut Cursor::new(&entry.value))?;

        // read the data block
        let data_block = self.data_block(&block_handle)?;

        Ok(data_block)
    }

    pub fn data_block(
        &mut self,
        block_handle: &BlockHandle,
    ) -> Result<Vec<u8>, SSTableReaderError> {
        let file_len = self.file.metadata()?.len();
        let start_pos = block_handle.offset;
        if start_pos + block_handle.size > file_len {
            return Err(SSTableReaderError::FileTooSmallForDataBlock(file_len));
        }
        self.file.seek(SeekFrom::Start(start_pos))?;

        self.db_context.log_debug(format!(
            "[SSTableReader] block_handle.offset={}, block_handle.size={}",
            block_handle.offset, block_handle.size
        ));

        // load the entire block into the buffer
        let buf = buf_utils::file_read_n(&mut self.file, block_handle.size as usize)?;

        // parse the crc32 and compression
        if !buf_utils::valid_block_crc32(&buf)? {
            return Err(SSTableReaderError::InvalidCRC32);
        }

        Ok(buf)
    }

    pub fn decoded_meta_block(&mut self) -> Result<MetaDataBlock, SSTableReaderError> {
        if let Some(meta_block) = &self.meta {
            return Ok(meta_block.clone());
        }

        let meta_block_data = self.meta_block()?;
        let meta = buf_utils::decode_meta_data(&meta_block_data)?;
        self.meta = Some(meta.clone());

        Ok(meta)
    }

    pub fn meta_block(&mut self) -> Result<Vec<u8>, SSTableReaderError> {
        let footer = self.decoded_footer_block()?;

        let meta_size: u64 = 4 + 1 + 4;

        self.file
            .seek(SeekFrom::Start(footer.meta_block_handle.offset))?;

        let buf = buf_utils::file_read_n(&mut self.file, footer.meta_block_handle.size as usize)?;

        assert_eq!(meta_size, buf.len() as u64);

        Ok(buf)
    }

    pub fn footer_block(&mut self) -> Result<Vec<u8>, SSTableReaderError> {
        let header_size: u64 = 8 + 16 + 16 + 16 + 1 + 4;

        let file_len = self.file.metadata()?.len();
        let start_pos = file_len.saturating_sub(header_size);
        if file_len < header_size {
            return Err(SSTableReaderError::FileTooSmallForFooter(file_len));
        }
        self.file.seek(SeekFrom::Start(start_pos))?;

        let mut buf: Vec<u8> = Vec::with_capacity(header_size as usize);
        self.file.read_to_end(&mut buf)?;

        Ok(buf)
    }

    pub fn decoded_footer_block(&mut self) -> Result<FooterBlock, SSTableReaderError> {
        if let Some(footer_block) = &self.footer {
            return Ok(footer_block.clone());
        }

        let buf = self.footer_block()?;
        let mut cursor = Cursor::new(&buf[..]);

        // decode the header data into the header block
        let magic: u64 = buf_utils::read_u64(&mut cursor)?;
        let data_block_handle = buf_utils::read_handle(&mut cursor)?;
        let index_block_handle = buf_utils::read_handle(&mut cursor)?;
        let meta_block_handle = buf_utils::read_handle(&mut cursor)?;

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
            meta_block_handle,
        };
        self.footer = Some(footer_block.clone());
        Ok(footer_block.clone())
    }
}
