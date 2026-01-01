use crc::{CRC_32_CKSUM, Crc};
use std::error::Error;
use std::fmt::Display;
use std::fs::File;
use std::io::Read;
use std::io::{self, Cursor};

use crate::core::sstable_builder::{BlockHandle, MetaDataBlock, PrefixCompressedEntry};

const CRC32: Crc<u32> = Crc::<u32>::new(&CRC_32_CKSUM);

#[derive(Debug)]
pub enum BufUtilsError {
    RestartLenNotDivisibleByFour,
    IOError(io::Error),
    EntryMalformed(String),
}

impl Display for BufUtilsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RestartLenNotDivisibleByFour => {
                write!(f, "BufUtilsError::RestartLenNotDivisibleByFour")
            }
            Self::IOError(err) => write!(f, "BufUtilsError::IOError: {}", err),
            Self::EntryMalformed(msg) => write!(f, "BufUtilsError::EntryMalformed: {}", msg),
        }
    }
}

impl Error for BufUtilsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::RestartLenNotDivisibleByFour => None,
            Self::IOError(err) => Some(err),
            Self::EntryMalformed(_) => None,
        }
    }
}

impl From<io::Error> for BufUtilsError {
    fn from(value: io::Error) -> Self {
        Self::IOError(value)
    }
}

#[inline]
pub(crate) fn read_handle(cursor: &mut Cursor<&[u8]>) -> Result<BlockHandle, io::Error> {
    let offset: u64 = read_u64(cursor)?;
    let size: u64 = read_u64(cursor)?;
    Ok(BlockHandle { offset, size })
}

#[inline]
pub(crate) fn read_u8(cursor: &mut Cursor<&[u8]>) -> Result<u8, io::Error> {
    let mut buf = [0u8; 1];
    cursor.read_exact(&mut buf)?;
    Ok(u8::from_le_bytes(buf))
}

#[inline]
pub(crate) fn read_u32(cursor: &mut Cursor<&[u8]>) -> Result<u32, io::Error> {
    let start = cursor.position() as usize;
    let end = cursor.position() as usize + 4;
    let data = cursor.get_ref();

    if end > data.len() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "out of bounds",
        ));
    }

    let bytes: [u8; 4] = data[start..end].try_into().unwrap();
    cursor.set_position(end as u64);

    Ok(u32::from_le_bytes(bytes))
}

#[inline]
pub(crate) fn read_u64(cursor: &mut Cursor<&[u8]>) -> Result<u64, io::Error> {
    let start = cursor.position() as usize;
    let end = cursor.position() as usize + 8;
    let data = cursor.get_ref();

    if end > data.len() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "out of bounds",
        ));
    }

    let bytes: [u8; 8] = data[start..end].try_into().unwrap();
    cursor.set_position(end as u64);

    Ok(u64::from_le_bytes(bytes))
}

#[inline]
pub(crate) fn read_n(cursor: &mut Cursor<&[u8]>, n: usize) -> Result<Vec<u8>, io::Error> {
    let mut buf: Vec<u8> = Vec::with_capacity(n);
    buf.resize(n, 0);
    cursor.read_exact(&mut buf)?;
    Ok(buf)
}

#[inline]
pub(crate) fn read_n_ref<'a>(buf: &'a [u8], start: usize, n: usize) -> Result<&'a [u8], io::Error> {
    let end = start + n;

    if end > buf.len() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "out of bounds",
        ));
    }

    Ok(&buf[start..end])
}

#[inline]
pub(crate) fn file_read_n(file: &mut File, n: usize) -> Result<Vec<u8>, io::Error> {
    let mut buf: Vec<u8> = Vec::with_capacity(n);
    buf.resize(n, 0);
    file.read_exact(&mut buf)?;
    Ok(buf)
}

#[inline]
pub(crate) fn restart_start_and_end_offsets(buf: &Vec<u8>) -> Result<(u64, u64), BufUtilsError> {
    let mut cursor = Cursor::new(&buf[..]);

    let keys_len = read_u64(&mut cursor)?;
    let block_size = buf.len() as u64;

    // keys + keys len size
    let max_keys_pos: u64 = keys_len + 8;

    // block size - crc32 and compression size
    let max_restarts_pos: u64 = block_size - 5;

    if (max_restarts_pos - max_restarts_pos) % 4 != 0 {
        return Err(BufUtilsError::RestartLenNotDivisibleByFour);
    }

    Ok((max_keys_pos, max_restarts_pos))
}

#[inline]
pub(crate) fn entry_and_restart_cursors(
    buf: &Vec<u8>,
) -> Result<(Cursor<&[u8]>, Cursor<&[u8]>), BufUtilsError> {
    let (max_keys_pos, max_restarts_pos) = restart_start_and_end_offsets(buf)?;

    let entry_cursor = Cursor::new(&buf[8..max_keys_pos as usize]);
    let restart_cursor = Cursor::new(&buf[max_keys_pos as usize..max_restarts_pos as usize]);

    Ok((entry_cursor, restart_cursor))
}

#[inline]
pub(crate) fn read_entry(
    cursor: &mut Cursor<&[u8]>,
) -> Result<PrefixCompressedEntry, BufUtilsError> {
    let shared_len = read_u32(cursor)?;
    let unshared_len = read_u32(cursor)?;
    let value_len = read_u32(cursor)?;

    // validate the lengths
    if (unshared_len + 9 + value_len) as u64 + cursor.position() > cursor.get_ref().len() as u64 {
        return Err(BufUtilsError::EntryMalformed(
            "prefix compressed entry length longer than cursor".to_string(),
        ));
    }

    let key_suffix = read_n(cursor, unshared_len as usize + 9)?;
    let value = read_n(cursor, value_len as usize)?;
    let entry = PrefixCompressedEntry {
        shared_len,
        unshared_len,
        value_len,
        key_suffix,
        value,
    };

    Ok(entry)
}

pub struct EntryBufRef<'a> {
    pub key_suffix: &'a [u8],
    pub value: &'a [u8],
    pub log_seq_num: u64,
    pub entry_type: u8,
    pub entry_end_pos: usize,

    pub shared_len: u32,
    pub unshared_len: u32,
}

#[inline]
pub(crate) fn read_entry_buf_ref<'a>(
    buf: &'a [u8],
    start_pos: usize,
) -> Result<EntryBufRef<'a>, BufUtilsError> {
    let shared_len_bytes: [u8; 4] = buf[start_pos..start_pos + 4].try_into().unwrap();
    let shared_len = u32::from_le_bytes(shared_len_bytes);

    let unshared_len_bytes: [u8; 4] = buf[start_pos + 4..start_pos + 8].try_into().unwrap();
    let unshared_len = u32::from_le_bytes(unshared_len_bytes);

    let value_len_bytes: [u8; 4] = buf[start_pos + 8..start_pos + 12].try_into().unwrap();
    let value_len = u32::from_le_bytes(value_len_bytes);

    let after_key_pos = start_pos + 12 + unshared_len as usize;
    let entry_end_pos = after_key_pos + 9 + value_len as usize;

    if after_key_pos + 9 + value_len as usize > buf.len() {
        return Err(BufUtilsError::EntryMalformed(
            "prefix compressed entry length longer than cursor".to_string(),
        ));
    }

    // Now it is safe to borrow immutably
    let key_suffix = read_n_ref(buf, start_pos + 12, unshared_len as usize)?;

    let log_seq_num_bytes: [u8; 8] = buf[after_key_pos..after_key_pos + 8].try_into().unwrap();
    let log_seq_num = u64::from_le_bytes(log_seq_num_bytes);

    let value = read_n_ref(buf, after_key_pos + 9, value_len as usize)?;

    let entry_type = buf[after_key_pos + 8];

    Ok(EntryBufRef {
        key_suffix,
        value,
        log_seq_num,
        entry_type,
        entry_end_pos,
        shared_len,
        unshared_len,
    })
}

#[inline]
pub(crate) fn decode_meta_data(buf: &Vec<u8>) -> Result<MetaDataBlock, BufUtilsError> {
    let mut cursor = Cursor::new(&buf[..]);
    let meta = MetaDataBlock {
        num_blocks: read_u32(&mut cursor)?,
    };
    Ok(meta)
}

#[inline]
pub(crate) fn valid_block_crc32(buf: &Vec<u8>) -> Result<bool, io::Error> {
    let mut cursor = Cursor::new(&buf[..]);
    cursor.set_position(buf.len() as u64 - 4);

    let crc32 = read_u32(&mut cursor)?;

    // validate data
    let valid_crc32 = CRC32.checksum(&buf[0..(buf.len() - 4) as usize]);
    if valid_crc32 != crc32 {
        return Ok(false);
    }

    Ok(true)
}
