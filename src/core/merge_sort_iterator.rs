use std::{error::Error, fmt::Display, sync::Arc};

use crate::core::{
    block_cache::BlockCache,
    db::ReadState,
    entry::LogEntry,
    iterator::{IteratorError, SourceIterator},
    memtable::{ImmutableMemtableIterator, Memtable, MemtableIterator},
};

#[derive(Debug)]
pub enum MergeSortIteratorError {
    SourceIteratorError(IteratorError),
}

impl Display for MergeSortIteratorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SourceIteratorError(err) => write!(f, "SourceIteratorError: {}", err),
        }
    }
}

impl Error for MergeSortIteratorError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::SourceIteratorError(err) => Some(err),
        }
    }
}

impl From<IteratorError> for MergeSortIteratorError {
    fn from(value: IteratorError) -> Self {
        Self::SourceIteratorError(value)
    }
}

pub struct MergeSortIterator {
    active_memtable: Arc<Memtable>,
    read_state: Arc<ReadState>,
    block_cache: Arc<BlockCache>,

    log_sequence_num: u64,

    memtable_iters: Vec<Box<dyn SourceIterator>>,
    current_entry: Option<Arc<LogEntry>>,
}

impl MergeSortIterator {
    pub fn new(
        active_memtable: Arc<Memtable>,
        read_state: Arc<ReadState>,
        block_cache: Arc<BlockCache>,
        log_sequence_num: u64,
    ) -> MergeSortIterator {
        let active_memtable_iter =
            Box::new(MemtableIterator::new(active_memtable.clone())) as Box<dyn SourceIterator>;
        let immutable_memtable_iters = read_state
            .memtables
            .iter()
            .map(|item| {
                Box::new(ImmutableMemtableIterator::new(item.clone())) as Box<dyn SourceIterator>
            })
            .collect::<Vec<Box<dyn SourceIterator>>>();

        let mut memtable_iters = Vec::with_capacity(1 + immutable_memtable_iters.len());
        memtable_iters.push(active_memtable_iter);

        MergeSortIterator {
            active_memtable,
            read_state,
            block_cache,
            memtable_iters: memtable_iters,
            current_entry: None,
            log_sequence_num,
        }
    }

    fn next_iter_entry(&mut self) -> Result<Option<Arc<LogEntry>>, MergeSortIteratorError> {
        let mut memtable_iters_to_delete: Vec<usize> = Vec::with_capacity(4);
        let mut primary_entry: Option<Arc<LogEntry>> = None;
        for (idx, iter) in self.memtable_iters.iter_mut().enumerate() {
            // get the iterators current/next entry
            loop {
                let entry = match iter.current() {
                    Some(entry) => Some(entry),
                    None => match iter.next()? {
                        Some(entry) => Some(entry),
                        None => None,
                    },
                };
                match entry {
                    Some(entry) => {
                        if entry.log_seq_num > self.log_sequence_num {
                            continue;
                        }
                        match &primary_entry {
                            Some(primary_entry_val) => {
                                if *primary_entry_val < entry {
                                    primary_entry = Some(entry)
                                }
                            }
                            None => {
                                primary_entry = Some(entry);
                            }
                        }
                    }
                    None => {
                        memtable_iters_to_delete.push(idx);
                    }
                }
            }
        }

        // delete all iterators that are no longer needing to be used
        // this will free up memory if the memtables have been flushed
        for idx in memtable_iters_to_delete.iter().rev() {
            self.memtable_iters.remove(*idx);
        }

        Ok(primary_entry)
    }
}

impl SourceIterator for MergeSortIterator {
    fn next(&mut self) -> Result<Option<Arc<LogEntry>>, IteratorError> {
        // get the current iter values
        Ok(None)
    }

    fn current(&self) -> Option<Arc<LogEntry>> {
        self.current_entry.clone()
    }
}
