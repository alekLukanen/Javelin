use std::io::{self, Read};
use std::{error::Error, fmt::Display, fs::File, path::PathBuf, sync::Arc};

use crc::{CRC_32_CKSUM, Crc};

use crate::core::sstable_builder::{BlockHandle, DataBlock, FooterBlock, IndexBlock};
use crate::core::{db_context::DBContext, sstable_builder::Block};

const CRC32: Crc<u32> = Crc::<u32>::new(&CRC_32_CKSUM);

#[derive(Debug)]
pub enum SSTableReaderError {
    IOError(io::Error),
    FileTooSmallForHeader(u64),
    InvalidCRC32(u32, u32),
}

impl Display for SSTableReaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IOError(e) => write!(f, "IOError: {}", e),
            Self::FileTooSmallForHeader(size) => {
                write!(f, "FileTooSmallForHeader: file size={}", size)
            }
            Self::InvalidCRC32(valid, actual) => {
                write!(f, "InvalidCRC32: valid={}, actual={}", valid, actual)
            }
        }
    }
}

impl Error for SSTableReaderError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::IOError(e) => Some(e),
            Self::FileTooSmallForHeader(_) => None,
            Self::InvalidCRC32(_, _) => None,
        }
    }
}

impl From<io::Error> for SSTableReaderError {
    fn from(value: io::Error) -> Self {
        Self::IOError(value)
    }
}

///////////////////////////////////////////////////

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

    pub fn index_block(&mut self) -> Result<Block, SSTableReaderError> {
        Ok(Block::IndexBlock(IndexBlock {
            keys: Vec::new(),
            keys_len: 0,
            restarts: Vec::new(),
        }))
    }

    pub fn data_block(&mut self) -> Result<Block, SSTableReaderError> {
        Ok(Block::DataBlock(DataBlock {
            keys: Vec::new(),
            keys_len: 0,
            restarts: Vec::new(),
        }))
    }

    fn read_footer(&mut self) -> Result<FooterBlock, SSTableReaderError> {
        if let Some(footer_block) = &self.footer {
            return Ok(footer_block.clone());
        }

        let header_size: u64 = 8 + 16 + 16 + 4 + 1;

        let file_len = self.file.metadata()?.len();
        let start_pos = file_len.saturating_sub(header_size);
        if file_len < header_size {
            return Err(SSTableReaderError::FileTooSmallForHeader(file_len));
        }

        let mut buf: Vec<u8> = Vec::with_capacity(header_size as usize);
        self.file.read_to_end(&mut buf)?;

        // decode the header data into the header block
        let magic: u64 = u64::from_le_bytes(buf[0..8].try_into().expect("incorrect magic length"));
        let data_block_handle = Self::read_handle(&buf, 8);
        let index_block_handle = Self::read_handle(&buf, 24);
        let crc32 = u32::from_le_bytes(buf[40..44].try_into().expect("crc32 length"));
        let _: u8 = buf[45];

        let valid_crc32 = CRC32.checksum(&buf[0..40]);
        if valid_crc32 != crc32 {
            return Err(SSTableReaderError::InvalidCRC32(valid_crc32, crc32));
        }

        let footer_block = FooterBlock {
            magic,
            data_block_handle,
            index_block_handle,
        };
        self.footer = Some(footer_block.clone());
        Ok(footer_block.clone())
    }

    #[inline]
    fn read_handle(buf: &Vec<u8>, start_pos: usize) -> BlockHandle {
        let offset: u64 = u64::from_le_bytes(
            buf[start_pos..start_pos + 8]
                .try_into()
                .expect("incorrect block offset length"),
        );
        let size: u64 = u64::from_le_bytes(
            buf[start_pos + 8..start_pos + 16]
                .try_into()
                .expect("incorrect block size length"),
        );
        BlockHandle { offset, size }
    }
}
