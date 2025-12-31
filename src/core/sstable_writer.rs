use std::io::prelude::*;
use std::{error::Error, fmt::Display, fs::File, io, path::PathBuf, sync::Arc};

use crc::{CRC_32_CKSUM, Crc};

use super::sstable_builder::BlockHandle;
use super::{
    db_context::DBContext,
    sstable_builder::{Block, Compression, PrefixCompressedEntry, SSTableBuilder},
};

const CRC32: Crc<u32> = Crc::<u32>::new(&CRC_32_CKSUM);

#[derive(Debug)]
pub enum SSTableWriterError {
    IOError(io::Error),
    IndexBlockAlreadyWritten,
    FooterBlockAlreadyWritten,
    IndexNotWrittenBeforeFooter,
}

impl Display for SSTableWriterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IOError(e) => write!(f, "IOError: {}", e),
            Self::IndexBlockAlreadyWritten => write!(f, "IndexBlockAlreadyWritten"),
            Self::FooterBlockAlreadyWritten => write!(f, "FooterBlockAlreadyWritten"),
            Self::IndexNotWrittenBeforeFooter => write!(f, "IndexNotWrittenBeforeFooter"),
        }
    }
}

impl Error for SSTableWriterError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::IOError(e) => Some(e),
            Self::IndexBlockAlreadyWritten => None,
            Self::FooterBlockAlreadyWritten => None,
            Self::IndexNotWrittenBeforeFooter => None,
        }
    }
}

impl From<io::Error> for SSTableWriterError {
    fn from(value: io::Error) -> Self {
        Self::IOError(value)
    }
}

//////////////////////////////////////////////////////

pub enum BlockData {
    DataBlock(Vec<u8>),
    IndexBlock(Vec<u8>),
    FooterBlock(Vec<u8>),
}

pub struct SSTableWriter {
    db_context: Arc<DBContext>,

    builder: SSTableBuilder,
    file: File,

    data_size: usize,
    index_size: usize,
    index_block_entries: Vec<(Vec<u8>, usize)>,
    returned_index_block: bool,
    returned_footer_block: bool,
}

impl SSTableWriter {
    pub fn new(
        db_context: Arc<DBContext>,
        builder: SSTableBuilder,
        file_path: PathBuf,
    ) -> Result<SSTableWriter, SSTableWriterError> {
        let file = File::create_new(file_path)?;
        Ok(SSTableWriter {
            db_context,
            builder,
            file,
            data_size: 0,
            index_size: 0,
            index_block_entries: Vec::new(),
            returned_index_block: false,
            returned_footer_block: false,
        })
    }

    pub fn next_data_block(&mut self) -> Result<Option<Vec<u8>>, SSTableWriterError> {
        let next_block = self.builder.next();
        match next_block {
            Some(next_block) => {
                let data = self.write_block(&next_block)?;
                Ok(Some(data))
            }
            None => Ok(None),
        }
    }

    pub fn index_block(&mut self) -> Result<Vec<u8>, SSTableWriterError> {
        if !self.returned_index_block {
            let index_block = self.builder.index_block(&self.index_block_entries);
            let data = self.write_block(&index_block)?;
            self.returned_index_block = true;
            Ok(data)
        } else {
            Err(SSTableWriterError::IndexBlockAlreadyWritten)
        }
    }

    pub fn footer_block(&mut self) -> Result<Vec<u8>, SSTableWriterError> {
        if !self.returned_footer_block && self.returned_index_block {
            let footer_block = self.builder.footer(self.data_size, self.index_size);
            let data = self.write_block(&footer_block)?;
            self.returned_footer_block = true;

            self.file.sync_all()?;

            Ok(data)
        } else if !self.returned_index_block {
            Err(SSTableWriterError::IndexNotWrittenBeforeFooter)
        } else {
            Err(SSTableWriterError::FooterBlockAlreadyWritten)
        }
    }

    fn write_block(&mut self, block: &Block) -> Result<Vec<u8>, SSTableWriterError> {
        match block {
            Block::DataBlock(data_block) => {
                self.db_context.log_debug(format!(
                    "writing data block id={}",
                    self.index_block_entries.len()
                ));
                let data =
                    self.write_data_or_index_block(&data_block.keys, &data_block.restarts)?;
                self.data_size += data.len();
                self.index_block_entries.push((
                    data_block
                        .keys
                        .first()
                        .expect("expected at least one key")
                        .key_suffix
                        .clone(),
                    data.len(),
                ));
                Ok(data)
            }
            Block::IndexBlock(index_block) => {
                self.db_context.log_debug("writing index block".to_string());
                let data =
                    self.write_data_or_index_block(&index_block.keys, &index_block.restarts)?;
                self.index_size = data.len();
                Ok(data)
            }
            Block::FooterBlock(footer_block) => {
                self.db_context
                    .log_debug("writing footer block".to_string());
                let data = self.write_footer_block(
                    &footer_block.magic,
                    &footer_block.data_block_handle,
                    &footer_block.index_block_handle,
                )?;
                Ok(data)
            }
        }
    }

    fn write_footer_block(
        &mut self,
        magic: &u64,
        data_block_handle: &BlockHandle,
        index_block_handle: &BlockHandle,
    ) -> Result<Vec<u8>, SSTableWriterError> {
        let size = 8 + data_block_handle.size() + index_block_handle.size() + 1 + 4;
        let mut data: Vec<u8> = Vec::with_capacity(size);

        data.extend_from_slice(&magic.to_le_bytes());
        data.extend_from_slice(&data_block_handle.value());
        data.extend_from_slice(&index_block_handle.value());

        let compression = Compression::None.value();
        data.push(compression);

        let crc32 = CRC32.checksum(&data);
        data.extend_from_slice(&crc32.to_le_bytes());

        self.file.write_all(&data)?;

        self.db_context.log_debug(format!(
            "footer: data.len()={}, data_block_handle={:?}, index_block_handle={:?}",
            data.len(),
            data_block_handle,
            index_block_handle
        ));

        assert_eq!(size, data.len());

        Ok(data)
    }

    fn write_data_or_index_block(
        &mut self,
        keys: &Vec<PrefixCompressedEntry>,
        restarts: &Vec<u32>,
    ) -> Result<Vec<u8>, SSTableWriterError> {
        let keys_len = keys.iter().map(|key| key.size() as u64).sum::<u64>();
        let size = 8 + keys_len as usize + restarts.len() * 4 + 4 + 1;

        let mut data: Vec<u8> = Vec::with_capacity(size);

        // write the data section
        data.extend_from_slice(&keys_len.to_le_bytes());
        for entry in keys {
            //self.db_context.log_debug(format!("entry: {:?}", entry));
            data.extend_from_slice(&entry.shared_len.to_le_bytes());
            data.extend_from_slice(&entry.unshared_len.to_le_bytes());
            data.extend_from_slice(&entry.value_len.to_le_bytes());
            data.extend_from_slice(&entry.key_suffix);
            data.extend_from_slice(&entry.value);
        }

        // write the restarts section
        for restart in restarts {
            data.extend_from_slice(&restart.to_le_bytes());
        }

        let compression = Compression::None.value();
        data.push(compression);

        let crc32 = CRC32.checksum(&data);
        data.extend_from_slice(&crc32.to_le_bytes());

        self.file.write_all(&data)?;

        self.db_context.log_debug(format!(
            "data.len() = {}, expected size = {}",
            data.len(),
            size,
        ));

        assert_eq!(size, data.len());

        Ok(data)
    }
}
