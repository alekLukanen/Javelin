use std::sync::Arc;

use super::{
    db_context::DBContext,
    entry::{self, LogEntry},
    memtable::ImmutableMemtable,
    skiplist::SkipListIter,
};

pub struct BlockHandle {
    offset: u64,
    size: u64,
}

pub enum Block {
    DataBlock {
        keys: Vec<PrefixCompressedEntry>,
        restarts: Vec<u32>,
    },
    IndexBlock {
        keys: Vec<PrefixCompressedEntry>,
        restarts: Vec<u32>,
    },
    FooterBlock {
        magic: u64,
        data_block_handle: BlockHandle,
        index_block_handle: BlockHandle,
        padding: u8,
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
    fn size(&self) -> u32 {
        self.shared_len + self.unshared_len + self.value_len
    }
}

pub struct SSTableBuilder {
    db_context: Arc<DBContext>,

    immutable_memtable: Option<SkipListIter>,
    restart_segment_size: usize,
    max_block_size: usize,
}

impl SSTableBuilder {
    pub fn build_from_immutable_memtable(
        db_context: Arc<DBContext>,
        memtable: ImmutableMemtable,
        max_block_size: usize,
    ) -> SSTableBuilder {
        SSTableBuilder {
            db_context,
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

        let mut restart_offsets: Vec<u32> = Vec::new();
        let mut restart_offset = 0;
        let mut restart_idx: u32 = 0;

        let mut prev_key: Option<Vec<u8>> = None;
        let mut block_size = 0;

        for entry in iter {
            if restart_idx % self.restart_segment_size as u32 == 0 {
                let next_compressed_entry = Self::compressed_entry_from_log_entry(&entry, &vec![]);

                self.db_context.log_info(format!(
                    "[SSTable Builder] entry: {:?}",
                    next_compressed_entry
                ));

                // update state
                restart_offsets.push(restart_offset);
                restart_offset += next_compressed_entry.size();
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
                restart_offset += next_compressed_entry.size();
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
            self.db_context
                .log_info(format!("[SSTable Builder] block size: {}", block_size));
            Some(Block::DataBlock {
                keys: compressed_entries,
                restarts: restart_offsets,
            })
        } else {
            None
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
