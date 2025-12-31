use std::{cmp::Ordering, collections::HashMap, error::Error, fmt::Display, sync::Arc};

use crate::core::{
    block_cache::BlockCache,
    db::ReadState,
    db_context::DBContext,
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

struct SSTableIterRefs {
    iters: Vec<Box<dyn SourceIterator>>,
    loaded: bool,
}

pub struct MergeSortIterator {
    db_context: Arc<DBContext>,
    read_state: Arc<ReadState>,
    block_cache: Arc<BlockCache>,

    log_sequence_num: u64,
    lower_bound: Option<Vec<u8>>,
    upper_bound: Option<Vec<u8>>,

    memtable_iters: Vec<Box<dyn SourceIterator>>,

    sstable_level_iters: HashMap<usize, SSTableIterRefs>,

    current_entry: Option<Arc<LogEntry>>,
}

impl MergeSortIterator {
    pub fn new(
        db_context: Arc<DBContext>,
        active_memtable: Arc<Memtable>,
        read_state: Arc<ReadState>,
        block_cache: Arc<BlockCache>,
        log_sequence_num: u64,
        lower_bound: Option<Vec<u8>>,
        upper_bound: Option<Vec<u8>>,
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

        let mut sstable_level_iters =
            HashMap::with_capacity(read_state.sstable_version.sstable_levels.len());
        for idx in read_state.sstable_version.sstable_levels.keys() {
            sstable_level_iters.insert(
                *idx,
                SSTableIterRefs {
                    iters: Vec::new(),
                    loaded: false,
                },
            );
        }

        MergeSortIterator {
            db_context,
            read_state,
            block_cache,
            memtable_iters: memtable_iters,
            sstable_level_iters,
            current_entry: None,
            log_sequence_num,
            lower_bound,
            upper_bound,
        }
    }

    fn next_iter_entry(&mut self) -> Result<Option<Arc<LogEntry>>, MergeSortIteratorError> {
        let mut primary_entry: Option<Arc<LogEntry>> = None;

        /////////////////////////////////////////////////
        // scan through the memtables
        let mut memtable_iters_to_delete: Vec<usize> = Vec::with_capacity(4);
        for idx in 0..self.memtable_iters.len() {
            // get the iterators current/next entry
            let (next_entry, iter_done) = self.find_next_valid_memtable_entry(&idx)?;
            if iter_done {
                memtable_iters_to_delete.push(idx);
            }
            if let Some(_) = next_entry {
                primary_entry = next_entry;
                break;
            }
        }

        self.db_context.log_debug(format!(
            "[MergeSortIterator] deleting memtable iters {:?}",
            memtable_iters_to_delete
        ));

        // exit early if this is a single item get
        if self.upper_bound == self.lower_bound && primary_entry.is_some() {
            // free up resources
            self.memtable_iters = Vec::new();
            self.clear_sstable_iters();

            self.current_entry = primary_entry.clone();
            return Ok(primary_entry);
        } else if self.upper_bound == self.lower_bound
            && self
                .sstable_level_iters
                .values()
                .filter(|item| item.loaded == true)
                .count()
                == self.sstable_level_iters.len()
        {
            self.current_entry = primary_entry.clone();
            return Ok(primary_entry);
        }

        // delete all iterators that are no longer needing to be used
        // this will free up memory if the memtables have been flushed
        for idx in memtable_iters_to_delete.iter().rev() {
            self.memtable_iters.remove(*idx);
        }

        /////////////////////////////////////////////////
        // scan through the sstable levels
        let mut sstable_levels = self
            .read_state
            .sstable_version
            .sstable_levels
            .keys()
            .map(|item| item.clone())
            .collect::<Vec<_>>();
        sstable_levels.sort();

        for level_idx in &sstable_levels {
            let Some(next_entry) = self.get_sstable_current_entry(level_idx)? else {
                continue;
            };
        }

        self.current_entry = primary_entry.clone();
        Ok(primary_entry)
    }

    fn find_next_valid_memtable_entry(
        &mut self,
        idx: &usize,
    ) -> Result<(Option<Arc<LogEntry>>, bool), MergeSortIteratorError> {
        let iter = self
            .memtable_iters
            .get_mut(*idx)
            .expect("MergeSortIterator: expected memtable");

        let mut primary_entry: Option<Arc<LogEntry>> = None;
        let mut iter_done = false;

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

                    // ignore entries that come before the lower bound
                    match &self.lower_bound {
                        Some(lower_bound) => match entry.entry.key_ref().cmp(lower_bound) {
                            Ordering::Equal => {}
                            Ordering::Less => {
                                iter.next()?;
                                continue;
                            }
                            Ordering::Greater => {}
                        },
                        None => {}
                    }

                    // ignore entries that come after the upper bound
                    match &self.upper_bound {
                        Some(upper_bound) => match entry.entry.key_ref().cmp(upper_bound) {
                            Ordering::Equal => {}
                            Ordering::Less => {}
                            Ordering::Greater => {
                                iter.next()?;
                                continue;
                            }
                        },
                        None => {}
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
                    iter_done = true;
                    break;
                }
            }
        }
        Ok((primary_entry, iter_done))
    }

    fn get_sstable_current_entry(
        &mut self,
        level: &usize,
    ) -> Result<Option<Arc<LogEntry>>, MergeSortIteratorError> {
        let level_loaded = match self.sstable_level_iters.get(level) {
            Some(level_iters) => level_iters.loaded,
            None => panic!("MergeSortIterator: requested level that doesn't exist"),
        };

        if !level_loaded {
            self.load_sstable_iters(level)?;
        }

        let level_iters = self
            .sstable_level_iters
            .get_mut(level)
            .expect("MergeSortIterator: requested level tht doesn't exist");

        for iter in &mut level_iters.iters {
            let entry = match iter.current() {
                Some(entry) => Some(entry),
                None => match iter.next()? {
                    Some(entry) => Some(entry),
                    None => None,
                },
            };
            return Ok(entry);
        }

        Ok(None)
    }

    fn load_sstable_iters(&mut self, level: &usize) -> Result<(), MergeSortIteratorError> {
        // TODO: get a list of all level-0 sstables that might contain the key
        let sstables = &self
            .read_state
            .sstable_version
            .sstable_levels
            .get(level)
            .unwrap()
            .sstables;

        if *level == 0 {
            // TODO: improve this so only
            // load all sstables
        } else {
        }
        Ok(())
    }

    fn clear_sstable_iters(&mut self) {
        self.sstable_level_iters.iter_mut().for_each(|(_, item)| {
            item.iters = Vec::new();
            item.loaded = true;
        })
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
