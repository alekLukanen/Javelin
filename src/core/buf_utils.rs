use std::io::Read;
use std::io::{self, Cursor};

use crate::core::sstable_builder::BlockHandle;

#[inline]
pub(crate) fn read_handle(cursor: &mut Cursor<&[u8]>) -> Result<BlockHandle, io::Error> {
    let offset: u64 = read_u64(cursor)?;
    let size: u64 = read_u64(cursor)?;
    Ok(BlockHandle { offset, size })
}

#[inline]
pub(crate) fn read_u32(cursor: &mut Cursor<&[u8]>) -> Result<u32, io::Error> {
    let mut buf = [0u8; 4];
    cursor.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

#[inline]
pub(crate) fn read_u64(cursor: &mut Cursor<&[u8]>) -> Result<u64, io::Error> {
    let mut buf = [0u8; 8];
    cursor.read_exact(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}
