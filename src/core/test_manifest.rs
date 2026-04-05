use std::{error::Error, path::PathBuf, sync::Arc};

use crate::core::{
    db_config::DBConfigBuilder,
    manifest::{Manifest, ManifestLog, ManifestLogCompactFiles, ManifestLogFlushMemtable, SSTableMeta},
    memtable::ImmutableMemtable,
    sstable_builder::SSTableBuilder,
    sstable_writer::SSTableWriter,
    test_utils::{SampleMemtableBuilder, TestContext},
};

/// Test that a FlushMemtable log record survives a write → read roundtrip.
#[test]
fn test_manifest_log_flush_roundtrip() -> Result<(), Box<dyn Error>> {
    let log = ManifestLog::FlushMemtable(ManifestLogFlushMemtable {
        level: 0,
        file_num: 42,
        min_key: b"aaa".to_vec(),
        max_key: b"zzz".to_vec(),
        file_size: 12345,
    });

    // Write to a buffer by using a temp manifest file
    let temp_dir = TestContext::temp_dir()?;
    let config = DBConfigBuilder::new()
        .data_dir(temp_dir.dir())
        .build();
    let db_context = Arc::new(crate::core::db_context::DBContext::new(config));

    let mut manifest = Manifest::new(db_context.clone())?;
    manifest.finalize_flushing_memtable(
        0,
        42,
        SSTableMeta {
            min_key: b"aaa".to_vec(),
            max_key: b"zzz".to_vec(),
            file_size: 12345,
        },
    )?;

    // Re-open and recover
    let manifest2 = Manifest::new(db_context.clone())?;
    let sv = manifest2.sstable_version();
    let level0 = sv.sstable_levels.get(0).unwrap();
    let meta = level0.sstables.get(&42).expect("file_num 42 should be recovered");
    assert_eq!(meta.min_key, b"aaa");
    assert_eq!(meta.max_key, b"zzz");
    assert_eq!(meta.file_size, 12345);

    Ok(())
}

/// Test that a CompactFiles log record survives a write → read roundtrip.
#[test]
fn test_manifest_log_compact_roundtrip() -> Result<(), Box<dyn Error>> {
    let log = ManifestLogCompactFiles {
        from_level: 0,
        from_level_file_nums: vec![1, 2, 3],
        to_level: 1,
        to_level_file_nums: vec![10],
        output_file_nums: vec![20, 21],
        output_file_metas: vec![
            SSTableMeta {
                min_key: b"a".to_vec(),
                max_key: b"m".to_vec(),
                file_size: 1000,
            },
            SSTableMeta {
                min_key: b"n".to_vec(),
                max_key: b"z".to_vec(),
                file_size: 2000,
            },
        ],
    };

    // Encode then decode
    let encoded = ManifestLog::CompactFiles(log.clone());
    let temp_dir = TestContext::temp_dir()?;
    let config = DBConfigBuilder::new().data_dir(temp_dir.dir()).build();
    let db_context = Arc::new(crate::core::db_context::DBContext::new(config));

    let mut manifest = Manifest::new(db_context.clone())?;

    // First add the to_level file so compaction can remove it
    manifest.finalize_flushing_memtable(
        0,
        10,
        SSTableMeta { min_key: b"a".to_vec(), max_key: b"z".to_vec(), file_size: 500 },
    )?;
    // Add the from_level files
    for &fn_ in &[1u64, 2, 3] {
        manifest.finalize_flushing_memtable(
            0,
            fn_,
            SSTableMeta { min_key: b"a".to_vec(), max_key: b"z".to_vec(), file_size: 100 },
        )?;
    }

    manifest.apply_compaction(
        log.from_level,
        log.from_level_file_nums.clone(),
        log.to_level,
        log.to_level_file_nums.clone(),
        log.output_file_nums.clone(),
        log.output_file_metas.clone(),
    )?;

    // Re-open and verify the compaction was recovered
    let manifest2 = Manifest::new(db_context.clone())?;
    let sv = manifest2.sstable_version();

    // from_level files 1, 2, 3 should be gone
    let level0 = &sv.sstable_levels[0];
    assert!(!level0.sstables.contains_key(&1));
    assert!(!level0.sstables.contains_key(&2));
    assert!(!level0.sstables.contains_key(&3));

    // to_level removed file 10, output files 20 and 21 should be there
    let level1 = &sv.sstable_levels[1];
    assert!(!level1.sstables.contains_key(&10), "file 10 should be removed by compaction");
    let meta20 = level1.sstables.get(&20).expect("file 20 should exist");
    assert_eq!(meta20.min_key, b"a");
    assert_eq!(meta20.max_key, b"m");
    assert_eq!(meta20.file_size, 1000);
    let meta21 = level1.sstables.get(&21).expect("file 21 should exist");
    assert_eq!(meta21.min_key, b"n");
    assert_eq!(meta21.max_key, b"z");
    assert_eq!(meta21.file_size, 2000);

    Ok(())
}

/// Test that manifest recovery correctly restores multiple flushed files after re-open.
#[test]
fn test_manifest_recovery_after_flush() -> Result<(), Box<dyn Error>> {
    let temp_dir = TestContext::temp_dir()?;
    let config = DBConfigBuilder::new().data_dir(temp_dir.dir()).build();
    let db_context = Arc::new(crate::core::db_context::DBContext::new(config));

    {
        let mut manifest = Manifest::new(db_context.clone())?;
        manifest.finalize_flushing_memtable(
            0,
            7,
            SSTableMeta {
                min_key: b"alpha".to_vec(),
                max_key: b"beta".to_vec(),
                file_size: 500,
            },
        )?;
        manifest.finalize_flushing_memtable(
            0,
            8,
            SSTableMeta {
                min_key: b"gamma".to_vec(),
                max_key: b"zeta".to_vec(),
                file_size: 750,
            },
        )?;
        // manifest dropped here — simulates process restart
    }

    // Re-open and recover
    let manifest2 = Manifest::new(db_context.clone())?;
    let sv = manifest2.sstable_version();
    let level0 = sv.sstable_levels.get(0).unwrap();

    assert_eq!(level0.sstables.len(), 2);

    let meta7 = level0.sstables.get(&7).expect("file 7 should be recovered");
    assert_eq!(meta7.min_key, b"alpha");
    assert_eq!(meta7.max_key, b"beta");
    assert_eq!(meta7.file_size, 500);

    let meta8 = level0.sstables.get(&8).expect("file 8 should be recovered");
    assert_eq!(meta8.min_key, b"gamma");
    assert_eq!(meta8.max_key, b"zeta");
    assert_eq!(meta8.file_size, 750);

    Ok(())
}

/// Test that SSTableWriter correctly populates first_key, last_key, and total_bytes_written.
#[test]
fn test_manifest_sstable_meta_populated() -> Result<(), Box<dyn Error>> {
    let temp_dir = TestContext::temp_dir()?;
    let config = DBConfigBuilder::new()
        .data_dir(temp_dir.dir())
        .sstable_max_block_size(10_000)
        .build();
    let tc = TestContext::new_from_config(config);

    // Build a memtable with 10 increasing keys (key = i.to_be_bytes())
    let sample = SampleMemtableBuilder::IncreasingPuts {
        size: 10,
        starting_value: 0,
        starting_log_sequence_num: 0,
    };
    let memtable = sample.build(&tc)?;
    let immutable = Arc::new(ImmutableMemtable::new(0, tc.db_context.clone(), memtable)?);

    let mut sstable_path = PathBuf::from(temp_dir.dir());
    sstable_path.push("meta_test.dat");

    let mut writer = SSTableWriter::new(
        tc.db_context.clone(),
        SSTableBuilder::build_from_immutable_memtable(tc.db_context.clone(), immutable),
        sstable_path,
    )?;

    // Drain all data blocks
    loop {
        if writer.next_data_block()?.is_none() {
            break;
        }
    }
    writer.index_block()?;
    writer.meta_data_block()?;
    writer.footer_block()?;

    let first_key = writer.first_key().expect("first_key should be set");
    let last_key = writer.last_key().expect("last_key should be set");
    let total_bytes = writer.total_bytes_written();

    // keys are i.to_be_bytes() for i in 0..10, so min = 0u64 and max = 9u64 in big-endian
    let expected_min = 0u64.to_be_bytes().to_vec();
    let expected_max = 9u64.to_be_bytes().to_vec();

    assert_eq!(first_key, expected_min, "first_key should be the smallest inserted key");
    assert_eq!(last_key, expected_max, "last_key should be the largest inserted key");
    assert!(total_bytes > 0, "total_bytes_written should be > 0");

    Ok(())
}
