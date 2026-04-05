use std::{collections::BTreeMap, error::Error, sync::Arc, thread, time::Duration};

use crate::core::{
    block_cache::BlockCache,
    db::{DBBackend, ReadState},
    db_config::DBConfigBuilder,
    file_utils,
    iterator::SourceIterator,
    manifest::{SSTableLevel, SSTableMeta, SSTableVersion},
    memtable::{ImmutableMemtable, Memtable},
    merge_sort_iterator::MergeSortIterator,
    sstable_builder::SSTableBuilder,
    sstable_writer::SSTableWriter,
    test_utils::{SampleMemtableBuilder, TestContext},
};

use super::db::DB;

/// Test that the level > 0 path in MergeSortIterator correctly merges two non-overlapping
/// SSTable files at Level 1 and returns all keys in sorted order.
#[test]
fn test_merge_sort_iterator_level1() -> Result<(), Box<dyn Error>> {
    let temp_dir = TestContext::temp_dir()?;
    let config = DBConfigBuilder::new()
        .data_dir(temp_dir.dir())
        .logging_enabled(true)
        .build();
    let tc = TestContext::new_from_config(config.clone());
    let memory = tc.memory_manager.clone();

    // Write SSTable 1: keys 0..13 (big-endian u64 byte keys)
    let mt1 = SampleMemtableBuilder::IncreasingPuts {
        size: 13,
        starting_value: 0,
        starting_log_sequence_num: 0,
    }
    .build(&tc)?;
    let im1 = Arc::new(ImmutableMemtable::new(0, tc.db_context.clone(), mt1)?);
    let file1_path = file_utils::sstable_path(&config, 1);
    let mut w1 = SSTableWriter::new(
        tc.db_context.clone(),
        SSTableBuilder::build_from_immutable_memtable(tc.db_context.clone(), im1),
        file1_path,
    )?;
    loop {
        if w1.next_data_block()?.is_none() {
            break;
        }
    }
    w1.index_block()?;
    w1.meta_data_block()?;
    w1.footer_block()?;
    let meta1 = SSTableMeta {
        min_key: w1.first_key().unwrap_or_default(),
        max_key: w1.last_key().unwrap_or_default(),
        file_size: w1.total_bytes_written(),
    };

    // Write SSTable 2: keys 13..26 — non-overlapping with file 1
    let mt2 = SampleMemtableBuilder::IncreasingPuts {
        size: 13,
        starting_value: 13,
        starting_log_sequence_num: 100,
    }
    .build(&tc)?;
    let im2 = Arc::new(ImmutableMemtable::new(1, tc.db_context.clone(), mt2)?);
    let file2_path = file_utils::sstable_path(&config, 2);
    let mut w2 = SSTableWriter::new(
        tc.db_context.clone(),
        SSTableBuilder::build_from_immutable_memtable(tc.db_context.clone(), im2),
        file2_path,
    )?;
    loop {
        if w2.next_data_block()?.is_none() {
            break;
        }
    }
    w2.index_block()?;
    w2.meta_data_block()?;
    w2.footer_block()?;
    let meta2 = SSTableMeta {
        min_key: w2.first_key().unwrap_or_default(),
        max_key: w2.last_key().unwrap_or_default(),
        file_size: w2.total_bytes_written(),
    };

    // Build a SSTableVersion: Level 0 empty, Level 1 has both files
    let mut l1_sstables = BTreeMap::new();
    l1_sstables.insert(1u64, meta1);
    l1_sstables.insert(2u64, meta2);
    let sv = Arc::new(SSTableVersion {
        sstable_levels: vec![
            SSTableLevel {
                sstables: BTreeMap::new(),
            },
            SSTableLevel {
                sstables: l1_sstables,
            },
        ],
    });

    let block_cache = Arc::new(BlockCache::new(tc.db_context.clone(), memory.clone()));
    let empty_mt = Arc::new(Memtable::new(tc.db_context.clone(), memory.clone(), 1_000_000));
    let read_state = Arc::new(ReadState {
        memtables: Vec::new(),
        sstable_version: sv,
    });

    let mut iter = MergeSortIterator::new(
        tc.db_context.clone(),
        empty_mt,
        read_state,
        block_cache,
        u64::MAX,
        None,
        None,
    );

    let mut keys: Vec<Vec<u8>> = Vec::new();
    loop {
        match SourceIterator::next(&mut iter)? {
            Some(entry) => keys.push(entry.entry.key()),
            None => break,
        }
    }

    assert_eq!(keys.len(), 26, "expected 26 keys, got {}", keys.len());
    // Verify keys are in sorted order with no duplicates
    for i in 0..keys.len() - 1 {
        assert!(
            keys[i] < keys[i + 1],
            "keys not sorted at index {}: {:?} >= {:?}",
            i,
            keys[i],
            keys[i + 1]
        );
    }

    Ok(())
}

/// Test that pick_compaction correctly selects all L0 files and overlapping L1 files
/// when the L0 file count exceeds the trigger.
#[test]
fn test_compaction_job_selection_level0() -> Result<(), Box<dyn Error>> {
    let config = DBConfigBuilder::new()
        .compaction_level0_file_count_trigger(4) // trigger at > 4 files
        .manifest_num_levels(3)
        .build();

    // Build a version with 5 files at L0 (each with a known key range) and
    // one overlapping file at L1.
    let mut l0_sstables = BTreeMap::new();
    for i in 0u64..5 {
        l0_sstables.insert(
            i,
            SSTableMeta {
                min_key: format!("key{:02}", i * 10).into_bytes(),
                max_key: format!("key{:02}", i * 10 + 9).into_bytes(),
                file_size: 1000,
            },
        );
    }
    // One overlapping L1 file (spans the full range of all L0 files)
    let mut l1_sstables = BTreeMap::new();
    l1_sstables.insert(
        100u64,
        SSTableMeta {
            min_key: b"key00".to_vec(),
            max_key: b"key49".to_vec(),
            file_size: 5000,
        },
    );

    let sv = SSTableVersion {
        sstable_levels: vec![
            SSTableLevel {
                sstables: l0_sstables,
            },
            SSTableLevel {
                sstables: l1_sstables,
            },
            SSTableLevel {
                sstables: BTreeMap::new(),
            },
        ],
    };

    let job = DBBackend::pick_compaction(&sv, &config).expect("expected a compaction job");

    assert_eq!(job.from_level, 0);
    assert_eq!(job.from_files.len(), 5, "all 5 L0 files should be included");
    assert_eq!(job.to_level, 1);
    assert_eq!(
        job.to_files.len(),
        1,
        "the overlapping L1 file should be included"
    );
    assert_eq!(job.to_files[0].0, 100, "expected file_num 100 from L1");

    Ok(())
}

/// Test that run_compaction merges L0 files into L1 and all originally inserted keys
/// are still accessible via get() after compaction.
#[test]
fn test_compaction_merges_files() -> Result<(), Box<dyn Error>> {
    let temp_dir = TestContext::temp_dir()?;

    // Very small memtable so every few inserts triggers a flush
    let config = DBConfigBuilder::new()
        .data_dir(temp_dir.dir())
        .memory_manager_max_memtable_memory_usage(500)
        .memory_manager_max_memory_usage(50 * 1024 * 1024)
        .sstable_max_block_size(256)
        .compaction_level0_file_count_trigger(100) // disable auto-compaction
        .manifest_num_levels(3)
        .logging_enabled(true)
        .build();

    let dbase = DB::new(config)?;

    // Insert enough entries to create several SSTables at L0
    let num_keys: u64 = 50;
    for i in 0..num_keys {
        let key = i.to_be_bytes().to_vec();
        dbase.set(key.clone(), key.clone())?;
    }

    // Wait for background flushes to settle
    thread::sleep(Duration::from_millis(500));

    let backend = dbase.backend();

    // Snapshot current L0 files
    let l0_files: Vec<(u64, SSTableMeta)> = {
        let guard = backend.db_inner.lock().unwrap();
        guard.manifest.sstable_version().sstable_levels[0]
            .sstables
            .iter()
            .map(|(k, v): (&u64, &SSTableMeta)| (*k, v.clone()))
            .collect()
    };

    if l0_files.len() < 2 {
        // Not enough flushes happened — skip this test gracefully
        println!("skipping: only {} L0 files flushed", l0_files.len());
        return Ok(());
    }

    // Manually run compaction of all L0 files into L1
    let job = super::db::CompactionJob {
        from_level: 0,
        from_files: l0_files.clone(),
        to_level: 1,
        to_files: Vec::new(),
    };
    DBBackend::run_compaction(&backend, job)?;

    // L0 should now be empty
    let sv_after = {
        let guard = backend.db_inner.lock().unwrap();
        guard.manifest.sstable_version()
    };
    assert_eq!(
        sv_after.sstable_levels[0].sstables.len(),
        0,
        "L0 should be empty after compaction"
    );
    assert!(
        !sv_after.sstable_levels[1].sstables.is_empty(),
        "L1 should have at least one file after compaction"
    );

    // All keys must still be readable
    for i in 0..num_keys {
        let key = i.to_be_bytes().to_vec();
        let val = dbase.get(&key)?;
        assert_eq!(val, Some(key.clone()), "key {} missing after compaction", i);
    }

    Ok(())
}

/// Test that tombstones are cleaned up when compacting to the bottom level.
#[test]
fn test_compaction_tombstone_cleanup() -> Result<(), Box<dyn Error>> {
    let temp_dir = TestContext::temp_dir()?;

    let config = DBConfigBuilder::new()
        .data_dir(temp_dir.dir())
        .memory_manager_max_memtable_memory_usage(500)
        .memory_manager_max_memory_usage(50 * 1024 * 1024)
        .sstable_max_block_size(256)
        .compaction_level0_file_count_trigger(100) // disable auto-compaction
        .manifest_num_levels(3) // bottom level = 2
        .logging_enabled(true)
        .build();

    let dbase = DB::new(config)?;

    // Insert then delete the key
    dbase.set(b"foo".to_vec(), b"bar".to_vec())?;
    dbase.delete(b"foo".to_vec())?;

    // Wait for the writes to be flushed to L0
    thread::sleep(Duration::from_millis(500));

    // Collect all L0 files
    let backend = dbase.backend();
    let l0_files: Vec<(u64, SSTableMeta)> = {
        let guard = backend.db_inner.lock().unwrap();
        guard.manifest.sstable_version().sstable_levels[0]
            .sstables
            .iter()
            .map(|(k, v): (&u64, &SSTableMeta)| (*k, v.clone()))
            .collect()
    };

    if l0_files.is_empty() {
        println!("skipping: no L0 files flushed");
        return Ok(());
    }

    // Compact L0 → L1 (not bottom level; keep tombstones)
    let l1_job = super::db::CompactionJob {
        from_level: 0,
        from_files: l0_files,
        to_level: 1,
        to_files: Vec::new(),
    };
    DBBackend::run_compaction(&backend, l1_job)?;

    // Compact L1 → L2 (bottom level; tombstones must be dropped)
    let l1_files: Vec<(u64, SSTableMeta)> = {
        let guard = backend.db_inner.lock().unwrap();
        guard.manifest.sstable_version().sstable_levels[1]
            .sstables
            .iter()
            .map(|(k, v): (&u64, &SSTableMeta)| (*k, v.clone()))
            .collect()
    };

    if l1_files.is_empty() {
        println!("skipping: no L1 files after first compaction");
        return Ok(());
    }

    let l2_job = super::db::CompactionJob {
        from_level: 1,
        from_files: l1_files,
        to_level: 2,
        to_files: Vec::new(),
    };
    DBBackend::run_compaction(&backend, l2_job)?;

    // After compacting to the bottom level, get() must return None
    let result = dbase.get(&b"foo".to_vec())?;
    assert_eq!(
        result, None,
        "deleted key should return None after tombstone cleanup"
    );

    Ok(())
}

/// Test that no data is lost after multiple flushes and background compaction.
#[test]
fn test_compaction_no_data_loss() -> Result<(), Box<dyn Error>> {
    let temp_dir = TestContext::temp_dir()?;

    let config = DBConfigBuilder::new()
        .data_dir(temp_dir.dir())
        .memory_manager_max_memtable_memory_usage(2_000)
        .memory_manager_max_memory_usage(50 * 1024 * 1024)
        .sstable_max_block_size(512)
        .compaction_level0_file_count_trigger(4) // allow background compaction
        .manifest_num_levels(3)
        .build();

    let dbase = DB::new(config)?;

    let num_keys: u64 = 500;

    // Insert 500 distinct keys
    for i in 0..num_keys {
        let key = i.to_be_bytes().to_vec();
        dbase.set(key.clone(), key.clone())?;
    }

    // Give background threads time to flush and compact
    thread::sleep(Duration::from_millis(1000));

    // All keys must still be readable
    let mut missing = 0;
    for i in 0..num_keys {
        let key = i.to_be_bytes().to_vec();
        if dbase.get(&key)? != Some(key.clone()) {
            println!("missing key: {}", i);
            missing += 1;
        }
    }

    assert_eq!(missing, 0, "{} keys lost after compaction", missing);

    Ok(())
}
