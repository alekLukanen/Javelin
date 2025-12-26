use std::{cmp::Ordering, error::Error, fmt::Display, sync::Arc};

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
    read_state: Arc<ReadState>,
    block_cache: Arc<BlockCache>,

    log_sequence_num: u64,

    memtable_iters: Vec<Box<dyn SourceIterator>>,
    sstable_level_iters: Vec<Box<dyn SourceIterator>>,

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
        memtable_iters.extend(immutable_memtable_iters);

        MergeSortIterator {
            read_state,
            block_cache,
            memtable_iters: memtable_iters,
            sstable_level_iters: Vec::new(),
            current_entry: None,
            log_sequence_num,
        }
    }

    fn next_iter_entry(&mut self) -> Result<Option<Arc<LogEntry>>, MergeSortIteratorError> {
        let mut primary_entry: Option<Arc<LogEntry>> = None;

        /////////////////////////////////////////////////
        // scan through the memtables

        let mut memtable_iters_to_delete: Vec<usize> = Vec::with_capacity(4);
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
                        // is the entry from a newer log sequence
                        if entry.log_seq_num > self.log_sequence_num {
                            iter.next()?;
                            continue;
                        }

                        // ignore any entries that exist for the same key
                        match &self.current_entry {
                            Some(current_entry) => {
                                match entry.entry.key_ref().cmp(current_entry.entry.key_ref()) {
                                    Ordering::Equal => {
                                        iter.next()?;
                                        continue;
                                    }
                                    Ordering::Less => {
                                        panic!(
                                            "new entry is less than current entry; this should never happen"
                                        );
                                    }
                                    Ordering::Greater => {}
                                }
                            }
                            None => {}
                        }

                        // set the primary entry if the new entry is less than the
                        // current primary entry value
                        match &primary_entry {
                            Some(primary_entry_val) => {
                                if *primary_entry_val > entry {
                                    primary_entry = Some(entry)
                                }
                            }
                            None => {
                                primary_entry = Some(entry);
                            }
                        }
                        break;
                    }
                    None => {
                        memtable_iters_to_delete.push(idx);
                        break;
                    }
                }
            }
        }

        // delete all iterators that are no longer needing to be used
        // this will free up memory if the memtables have been flushed
        for idx in memtable_iters_to_delete.iter().rev() {
            self.memtable_iters.remove(*idx);
        }

        /////////////////////////////////////////////////
        // scan through the sstable levels

        self.current_entry = primary_entry.clone();
        Ok(primary_entry)
    }
}

impl SourceIterator for MergeSortIterator {
    fn next(&mut self) -> Result<Option<Arc<LogEntry>>, IteratorError> {
        // get the current iter values
        match self.next_iter_entry() {
            Ok(val) => Ok(val),
            Err(err) => Err(IteratorError::SourceError(Box::new(err))),
        }
    }

    fn current(&self) -> Option<Arc<LogEntry>> {
        self.current_entry.clone()
    }
}
