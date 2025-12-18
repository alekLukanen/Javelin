use std::sync::Arc;

use super::{
    db_context::DBContext, entry::LogEntry, memtable::ImmutableMemtable, skiplist::SkipListIter,
};

pub enum Compression {
    None,
}

impl Compression {
    pub fn value(&self) -> u8 {
        match self {
            Self::None => 0,
        }
    }
}

pub struct BlockHandle {
    offset: u64,
    size: u64,
}

impl BlockHandle {
    pub fn size(&self) -> usize {
        8 + 8
    }
    pub fn value(&self) -> Vec<u8> {
        let mut val = self.offset.to_le_bytes().to_vec();
        val.extend_from_slice(&self.size.to_le_bytes());
        val
    }
}

pub enum Block {
    DataBlock {
        keys: Vec<PrefixCompressedEntry>,
        keys_len: u64,
        restarts: Vec<u32>,
    },
    IndexBlock {
        keys: Vec<PrefixCompressedEntry>,
        keys_len: u64,
        restarts: Vec<u32>,
    },
    FooterBlock {
        magic: u64,
        data_block_handle: BlockHandle,
        index_block_handle: BlockHandle,
    },
}

#[derive(Debug)]
pub struct PrefixCompressedEntry {
    pub(crate) shared_len: u32,
    pub(crate) unshared_len: u32,
    pub(crate) value_len: u32,
    pub(crate) key_suffix: Vec<u8>,
    pub(crate) value: Vec<u8>,
}

impl PrefixCompressedEntry {
    pub fn size(&self) -> usize {
        // the 9 is from the entry type + sequence number
        4 + 4 + 4 + self.unshared_len as usize + 9 + self.value_len as usize
    }
}

pub struct SSTableBuilder {
    db_context: Arc<DBContext>,

    returned_index_block: bool,

    immutable_memtable: Option<SkipListIter>,
    restart_segment_size: usize,
    max_block_size: usize,
}

impl SSTableBuilder {
    pub fn build_from_immutable_memtable(
        db_context: Arc<DBContext>,
        memtable: Arc<ImmutableMemtable>,
    ) -> SSTableBuilder {
        let max_block_size = db_context.config().sstable_max_block_size();
        SSTableBuilder {
            db_context,
            returned_index_block: false,
            immutable_memtable: Some(memtable.iter()),
            restart_segment_size: 8,
            max_block_size,
        }
    }

    fn next_memtable_block(&mut self) -> Option<Block> {
        let iter = match &mut self.immutable_memtable {
            Some(iter) => iter,
            None => return None,
        };

        let mut compressed_entries: Vec<PrefixCompressedEntry> = Vec::new();
        let mut smallest_entry: Option<Arc<LogEntry>> = None;

        let mut restart_offsets: Vec<u32> = Vec::new();
        let mut restart_offset = 0;
        let mut restart_idx: u32 = 0;

        let mut prev_key: Option<Vec<u8>> = None;
        let mut block_size = 0;

        for entry in iter {
            if restart_idx % self.restart_segment_size as u32 == 0 {
                if smallest_entry.is_none() {
                    smallest_entry = Some(entry.clone());
                }
                let next_compressed_entry = Self::compressed_entry_from_log_entry(&entry, &vec![]);

                // update state
                restart_offsets.push(restart_offset);
                restart_offset += next_compressed_entry.size() as u32;
                block_size += next_compressed_entry.size();
                prev_key = Some(entry.entry.key());
                compressed_entries.push(next_compressed_entry);
            } else {
                // get the previous key
                let prev_key = match &prev_key {
                    Some(prev_key) => prev_key,
                    None => panic!("previous key not found"),
                };
                let next_compressed_entry = Self::compressed_entry_from_log_entry(&entry, prev_key);

                // update state
                restart_offset += next_compressed_entry.size() as u32;
                block_size += next_compressed_entry.size();
                compressed_entries.push(next_compressed_entry);
            }

            // break when the block size is large enough
            if block_size as usize > self.max_block_size {
                break;
            }
            restart_idx += 1;
        }
        if block_size != 0 {
            let keys_len: u64 = compressed_entries
                .iter()
                .map(|item| item.size())
                .sum::<usize>() as u64;
            let data_block = Block::DataBlock {
                keys: compressed_entries,
                keys_len,
                restarts: restart_offsets,
            };
            Some(data_block)
        } else {
            None
        }
    }

    pub fn index_block(&self, index_block_entries: &Vec<(Vec<u8>, usize)>) -> Block {
        let mut compressed_entries: Vec<PrefixCompressedEntry> = Vec::new();

        let mut restart_offsets: Vec<u32> = Vec::new();
        let mut restart_offset = 0;
        let mut restart_idx: u32 = 0;

        let mut prev_key: Option<Vec<u8>> = None;
        let mut block_size = 0;

        let mut block_offset: usize = 0;

        for (key, data_block_size) in index_block_entries {
            let block_handle = BlockHandle {
                offset: block_offset as u64,
                size: *data_block_size as u64,
            };
            block_offset += *data_block_size;
            if restart_idx % self.restart_segment_size as u32 == 0 {
                let next_compressed_entry =
                    Self::compressed_entry_from_index_item(key, block_handle, &vec![]);

                // update state
                restart_offsets.push(restart_offset);
                restart_offset += next_compressed_entry.size() as u32;
                block_size += next_compressed_entry.size();
                prev_key = Some(key.clone());
                compressed_entries.push(next_compressed_entry);
            } else {
                // get the previous key
                let prev_key = match &prev_key {
                    Some(prev_key) => prev_key,
                    None => panic!("previous key not found"),
                };
                let next_compressed_entry =
                    Self::compressed_entry_from_index_item(key, block_handle, prev_key);

                // update state
                restart_offset += next_compressed_entry.size() as u32;
                block_size += next_compressed_entry.size();
                compressed_entries.push(next_compressed_entry);
            }

            // break when the block size is large enough
            if block_size as usize > self.max_block_size {
                break;
            }
            restart_idx += 1;
        }
        let keys_len: u64 = compressed_entries
            .iter()
            .map(|item| item.size())
            .sum::<usize>() as u64;
        Block::IndexBlock {
            keys: compressed_entries,
            keys_len,
            restarts: restart_offsets,
        }
    }

    pub fn footer(&self, data_size: usize, index_size: usize) -> Block {
        Block::FooterBlock {
            magic: 69,
            data_block_handle: BlockHandle {
                offset: 0,
                size: data_size as u64,
            },
            index_block_handle: BlockHandle {
                offset: data_size as u64,
                size: index_size as u64,
            },
        }
    }

    #[inline]
    fn compressed_entry_from_log_entry(
        entry: &Arc<LogEntry>,
        previous_key: &[u8],
    ) -> PrefixCompressedEntry {
        let key = entry.entry.key();
        let value = entry.entry.value();
        let id = entry.entry.id();
        let mut trailer = entry.log_seq_num.to_le_bytes().to_vec();
        trailer.push(id.to_le());

        let shared_len = shared_prefix_len(&key, previous_key);
        let suffix = compute_suffix(&key, shared_len, &trailer);

        PrefixCompressedEntry {
            shared_len: shared_len as u32,
            unshared_len: (key.len() - shared_len) as u32,
            value_len: value.len() as u32,
            key_suffix: suffix,
            value,
        }
    }

    #[inline]
    fn compressed_entry_from_index_item(
        key: &Vec<u8>,
        handle: BlockHandle,
        previous_key: &[u8],
    ) -> PrefixCompressedEntry {
        let value = handle.value();

        let shared_len = shared_prefix_len(key, previous_key);
        let suffix = compute_suffix(key, shared_len, &[]);

        PrefixCompressedEntry {
            shared_len: shared_len as u32,
            unshared_len: (key.len() - shared_len) as u32,
            value_len: value.len() as u32,
            key_suffix: suffix,
            value,
        }
    }
}

#[inline]
fn shared_prefix_len(a: &[u8], b: &[u8]) -> usize {
    let mut i = 0;
    let min_len = a.len().min(b.len());
    while i < min_len && a[i] == b[i] {
        i += 1;
    }
    i
}

#[inline]
fn compute_suffix(next: &[u8], shared: usize, trailer: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(next.len() - shared + trailer.len());
    out.extend_from_slice(&next[shared..]);
    out.extend_from_slice(trailer);
    out
}

impl Iterator for SSTableBuilder {
    type Item = Block;

    fn next(&mut self) -> Option<Self::Item> {
        if self.immutable_memtable.is_some() {
            self.next_memtable_block()
        } else {
            None
        }
    }
}
