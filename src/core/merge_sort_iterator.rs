use std::sync::Arc;

use crate::core::{
    block_cache::BlockCache,
    db::ReadState,
    memtable::{ImmutableMemtableIterator, Memtable, MemtableIterator},
};

pub struct MergeSortIterator {
    active_memtable: Arc<Memtable>,
    read_state: Arc<ReadState>,
    block_cache: Arc<BlockCache>,

    active_memtable_iter: Option<MemtableIterator>,
    immutable_memtable_iters: Vec<ImmutableMemtableIterator>,
}

impl MergeSortIterator {
    pub fn new(
        active_memtable: Arc<Memtable>,
        read_state: Arc<ReadState>,
        block_cache: Arc<BlockCache>,
    ) -> MergeSortIterator {
        let active_memtable_iter = MemtableIterator::new(active_memtable.clone());
        let immutable_memtable_iters = read_state
            .memtables
            .iter()
            .map(|item| ImmutableMemtableIterator::new(item.clone()))
            .collect::<Vec<_>>();
        MergeSortIterator {
            active_memtable,
            read_state,
            block_cache,
            active_memtable_iter: Some(active_memtable_iter),
            immutable_memtable_iters: immutable_memtable_iters,
        }
    }
}
