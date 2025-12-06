use std::sync::Arc;

use super::{
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

pub struct PrefixCompressedEntry {
    shared_len: usize,
    unshared_len: usize,
    value_len: usize,
    key_suffix: Vec<u8>,
    value: Vec<u8>,
}

pub struct SSTableBuilder {
    immutable_memtable: Option<SkipListIter>,
    restart_segment_size: usize,
}

impl SSTableBuilder {
    pub fn build_from_immutable_memtable(memtable: ImmutableMemtable) -> SSTableBuilder {
        SSTableBuilder {
            immutable_memtable: Some(memtable.iter()),
            restart_segment_size: 4,
        }
    }

    fn next_memtable_block(&mut self) -> Option<Block> {
        let compressed_entries: Vec<PrefixCompressedEntry> = Vec::new();

        let next_entry = self.immutable_memtable.expect("expected memtable").next();
        match next_entry {
            Some(entry) => {
                compressed_entries.push(Self::compressed_entry_from_log_entry(&entry, &vec![]))
            }
            None => return None,
        }

        let restart_idx: usize = 0;
        for entry in self.immutable_memtable.expect("expected memtable") {
            if restart_idx % self.restart_segment_size {
                compressed_entries.push(Self::compressed_entry_from_log_entry(&entry, &vec![]));
            } else {
                let compressed_entry = compressed_entries.last();
                match prev_key {
                    Some(prev_entry)
                }
            }
        }
        Some(Block::DataBlock {
            keys: compressed_entries,
            restarts: (),
        })
    }

    #[inline]
    fn compressed_entry_from_log_entry(
        entry: &Arc<LogEntry>,
        previous_key: &[u8],
    ) -> PrefixCompressedEntry {
        let key = entry.entry.key();
        let value = entry.entry.value();
        let id = entry.entry.id();
        PrefixCompressedEntry {
            shared_len: 0,
            unshared_len: key.len(),
            value_len: value.len(),
            key_suffix: Vec::new(),
            value: Vec::new(),
        }
    }
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
