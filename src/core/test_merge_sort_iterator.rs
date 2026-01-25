use std::{collections::HashMap, error::Error, sync::Arc};

use crate::core::{
    block_cache::BlockCache,
    db::ReadState,
    db_config::DBConfigBuilder,
    entry::{Entry, LogEntry},
    iterator::SourceIterator,
    manifest::SSTableVersion,
    memtable::{ImmutableMemtable, MemtableIterator},
    merge_sort_iterator::MergeSortIterator,
    test_utils::{SampleMemtableBuilder, TestContext},
};

#[test]
fn test_simple_case_memtables_only() -> Result<(), Box<dyn Error>> {
    let config = DBConfigBuilder::new().logging_enabled(true).build();
    let tc = TestContext::new_from_config(config);

    let active_memtable = SampleMemtableBuilder::IncreasingPuts {
        size: 5,
        starting_value: 5,
        starting_log_sequence_num: 100,
    }
    .build(&tc)?;
    let immuitable_memtable = Arc::new(ImmutableMemtable::new(
        0,
        tc.db_context.clone(),
        SampleMemtableBuilder::IncreasingPuts {
            size: 10,
            starting_value: 0,
            starting_log_sequence_num: 0,
        }
        .build(&tc)?,
    )?);

    let read_state = Arc::new(ReadState {
        memtables: vec![immuitable_memtable],
        sstable_version: Arc::new(SSTableVersion {
            sstable_levels: HashMap::new(),
        }),
    });

    let block_cache = Arc::new(BlockCache::new(
        tc.db_context.clone(),
        tc.memory_manager.clone(),
    ));

    let log_sequence_num: u64 = 200;

    let mut iter = MergeSortIterator::new(
        tc.db_context.clone(),
        active_memtable.clone(),
        read_state.clone(),
        block_cache.clone(),
        log_sequence_num.clone(),
        None,
        None,
    );

    let mut expected_entries = Vec::new();
    for i in 0..5u64 {
        expected_entries.push(LogEntry::new(
            Entry::Put {
                key: i.to_be_bytes().to_vec(),
                val: i.to_be_bytes().to_vec(),
            },
            i,
        ));
    }
    for (idx, i) in (5..10u64).enumerate() {
        expected_entries.push(LogEntry::new(
            Entry::Put {
                key: i.to_be_bytes().to_vec(),
                val: i.to_be_bytes().to_vec(),
            },
            100 + idx as u64,
        ));
    }

    let mut idx: usize = 0;
    loop {
        let Some(entry) = iter.next()? else {
            break;
        };
        println!("[idx={}] entry: {:?}", idx, entry);
        assert_eq!(*expected_entries.get(idx).unwrap(), *entry);
        idx += 1;
    }

    Ok(())
}

#[test]
fn test_simple_case_memtables_only_with_bounds() -> Result<(), Box<dyn Error>> {
    let config = DBConfigBuilder::new().build();
    let tc = TestContext::new_from_config(config);

    let active_memtable = SampleMemtableBuilder::IncreasingPuts {
        size: 5,
        starting_value: 0,
        starting_log_sequence_num: 100,
    }
    .build(&tc)?;
    let immuitable_memtable = Arc::new(ImmutableMemtable::new(
        0,
        tc.db_context.clone(),
        SampleMemtableBuilder::IncreasingPuts {
            size: 10,
            starting_value: 5,
            starting_log_sequence_num: 0,
        }
        .build(&tc)?,
    )?);

    let read_state = Arc::new(ReadState {
        memtables: vec![immuitable_memtable],
        sstable_version: Arc::new(SSTableVersion {
            sstable_levels: HashMap::new(),
        }),
    });

    let block_cache = Arc::new(BlockCache::new(
        tc.db_context.clone(),
        tc.memory_manager.clone(),
    ));

    let log_sequence_num: u64 = 200;

    let mut iter = MergeSortIterator::new(
        tc.db_context.clone(),
        active_memtable.clone(),
        read_state.clone(),
        block_cache.clone(),
        log_sequence_num.clone(),
        Some(2u64.to_be_bytes().to_vec()),
        Some(7u64.to_be_bytes().to_vec()),
    );

    let mut expected_entries = Vec::new();
    for i in 2..5u64 {
        expected_entries.push(LogEntry::new(
            Entry::Put {
                key: i.to_be_bytes().to_vec(),
                val: i.to_be_bytes().to_vec(),
            },
            100 + i,
        ));
    }
    for (idx, i) in (5..8u64).enumerate() {
        expected_entries.push(LogEntry::new(
            Entry::Put {
                key: i.to_be_bytes().to_vec(),
                val: i.to_be_bytes().to_vec(),
            },
            idx as u64,
        ));
    }

    let mut idx: usize = 0;
    loop {
        let Some(entry) = iter.next()? else {
            break;
        };
        println!("[idx={}] entry: {:?}", idx, entry);
        assert_eq!(*expected_entries.get(idx).unwrap(), *entry);
        idx += 1;
    }

    Ok(())
}

#[test]
fn test_simple_case_memtables_only_with_equal_bounds() -> Result<(), Box<dyn Error>> {
    let config = DBConfigBuilder::new().build();
    let tc = TestContext::new_from_config(config);

    let active_memtable = SampleMemtableBuilder::IncreasingPuts {
        size: 10,
        starting_value: 0,
        starting_log_sequence_num: 100,
    }
    .build(&tc)?;
    let immuitable_memtable = Arc::new(ImmutableMemtable::new(
        0,
        tc.db_context.clone(),
        SampleMemtableBuilder::IncreasingPuts {
            size: 10,
            starting_value: 5,
            starting_log_sequence_num: 0,
        }
        .build(&tc)?,
    )?);

    let read_state = Arc::new(ReadState {
        memtables: vec![immuitable_memtable],
        sstable_version: Arc::new(SSTableVersion {
            sstable_levels: HashMap::new(),
        }),
    });

    let block_cache = Arc::new(BlockCache::new(
        tc.db_context.clone(),
        tc.memory_manager.clone(),
    ));

    let log_sequence_num: u64 = 200;

    let mut iter = MergeSortIterator::new(
        tc.db_context.clone(),
        active_memtable.clone(),
        read_state.clone(),
        block_cache.clone(),
        log_sequence_num.clone(),
        Some(7u64.to_be_bytes().to_vec()),
        Some(7u64.to_be_bytes().to_vec()),
    );

    let mut expected_entries = Vec::new();
    for (idx, i) in (7..8u64).enumerate() {
        expected_entries.push(LogEntry::new(
            Entry::Put {
                key: i.to_be_bytes().to_vec(),
                val: i.to_be_bytes().to_vec(),
            },
            107 + idx as u64,
        ));
    }

    let mut idx: usize = 0;
    loop {
        let Some(entry) = iter.next()? else {
            break;
        };
        println!("[idx={}] entry: {:?}", idx, entry);
        assert_eq!(*expected_entries.get(idx).unwrap(), *entry);
        idx += 1;
    }

    assert_eq!(1, idx);

    Ok(())
}
