use std::fs::File;
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
pub(crate) fn read_u8(cursor: &mut Cursor<&[u8]>) -> Result<u8, io::Error> {
    let mut buf = [0u8; 1];
    cursor.read_exact(&mut buf)?;
    Ok(u8::from_le_bytes(buf))
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

#[inline]
pub(crate) fn read_n(cursor: &mut Cursor<&[u8]>, n: usize) -> Result<Vec<u8>, io::Error> {
    let mut buf: Vec<u8> = Vec::with_capacity(n);
    buf.resize(n, 0);
    cursor.read_exact(&mut buf)?;
    Ok(buf)
}

#[inline]
pub(crate) fn file_read_n(file: &mut File, n: usize) -> Result<Vec<u8>, io::Error> {
    let mut buf: Vec<u8> = Vec::with_capacity(n);
    buf.resize(n, 0);
    file.read_exact(&mut buf)?;
    Ok(buf)
}
