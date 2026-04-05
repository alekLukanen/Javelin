use std::{error::Error, path::PathBuf, sync::Arc, time::Duration};

use crate::core::{
    db::DB,
    db_config::DBConfigBuilder,
    entry::{Entry, LogEntry},
    test_utils::TestContext,
    wal::WAL,
};

/// Helpers

fn make_put(key: u64, val: u64, lsn: u64) -> Arc<LogEntry> {
    Arc::new(LogEntry::new(
        Entry::Put {
            key: key.to_be_bytes().to_vec(),
            val: val.to_be_bytes().to_vec(),
        },
        lsn,
    ))
}

fn make_del(key: u64, lsn: u64) -> Arc<LogEntry> {
    Arc::new(LogEntry::new(
        Entry::Del {
            key: key.to_be_bytes().to_vec(),
        },
        lsn,
    ))
}

/// Test 5: append entries, sync, then recover and verify round-trip.
#[test]
fn test_wal_append_and_read_back() -> Result<(), Box<dyn Error>> {
    let temp_dir = TestContext::temp_dir()?;
    let config = DBConfigBuilder::new().data_dir(temp_dir.dir()).build();

    let wal_id: u64 = 42;
    let put_entry = make_put(1, 100, 10);
    let del_entry = make_del(2, 11);

    // Write two entries to a WAL file and sync
    {
        let mut wal_file = WAL::rotate(wal_id, &config)?;
        wal_file.append(&put_entry)?;
        wal_file.append(&del_entry)?;
        wal_file.sync()?;
    }

    // Recover and verify
    let recovered = WAL::recover(&config)?;
    assert_eq!(recovered.len(), 1);
    let (recovered_id, entries) = &recovered[0];
    assert_eq!(*recovered_id, wal_id);
    assert_eq!(entries.len(), 2);

    let e0 = &entries[0];
    assert_eq!(e0.log_seq_num, 10);
    match &e0.entry {
        Entry::Put { key, val } => {
            assert_eq!(key, &1u64.to_be_bytes().to_vec());
            assert_eq!(val, &100u64.to_be_bytes().to_vec());
        }
        _ => panic!("expected Put"),
    }

    let e1 = &entries[1];
    assert_eq!(e1.log_seq_num, 11);
    match &e1.entry {
        Entry::Del { key } => {
            assert_eq!(key, &2u64.to_be_bytes().to_vec());
        }
        _ => panic!("expected Del"),
    }

    Ok(())
}

/// Test 6: partial write at tail is silently discarded.
#[test]
fn test_wal_partial_write_recovery() -> Result<(), Box<dyn Error>> {
    let temp_dir = TestContext::temp_dir()?;
    let config = DBConfigBuilder::new().data_dir(temp_dir.dir()).build();

    let wal_id: u64 = 0;

    // Write two valid records, then append random bytes simulating a partial write
    {
        let mut wal_file = WAL::rotate(wal_id, &config)?;
        wal_file.append(&make_put(1, 1, 0))?;
        wal_file.append(&make_put(2, 2, 1))?;
        wal_file.sync()?;
    }

    // Append partial/garbage bytes directly to the file
    {
        use std::fs::OpenOptions;
        use std::io::Write;
        let path = temp_dir.dir().join("0.wal");
        let mut file = OpenOptions::new().append(true).open(path)?;
        file.write_all(&[0xDE, 0xAD, 0xBE, 0xEF, 0x11])?;
    }

    let recovered = WAL::recover(&config)?;
    assert_eq!(recovered.len(), 1);
    let (_, entries) = &recovered[0];
    // Only the two valid records should be returned; the partial tail is dropped
    assert_eq!(entries.len(), 2, "partial tail should be silently discarded");

    Ok(())
}

/// Test 7: a record with a corrupted CRC is skipped.
#[test]
fn test_wal_crc_recovery() -> Result<(), Box<dyn Error>> {
    let temp_dir = TestContext::temp_dir()?;
    let config = DBConfigBuilder::new().data_dir(temp_dir.dir()).build();

    let wal_id: u64 = 0;
    {
        let mut wal_file = WAL::rotate(wal_id, &config)?;
        wal_file.append(&make_put(1, 1, 0))?;
        wal_file.sync()?;
    }

    // Corrupt the last byte of the file (part of the CRC32)
    {
        use std::fs::OpenOptions;
        use std::io::{Seek, SeekFrom, Write};
        let path = temp_dir.dir().join("0.wal");
        let mut file = OpenOptions::new().read(true).write(true).open(&path)?;
        let len = file.seek(SeekFrom::End(0))?;
        file.seek(SeekFrom::Start(len - 1))?;
        file.write_all(&[0xFF])?;
    }

    let recovered = WAL::recover(&config)?;
    // The corrupted record should be skipped; no panic should occur
    assert_eq!(recovered.len(), 1);
    let (_, entries) = &recovered[0];
    assert_eq!(entries.len(), 0, "corrupted record should be skipped");

    Ok(())
}

/// Test 8: sequence number counter is restored from recovered WAL entries.
#[test]
fn test_wal_sequence_num_restored() -> Result<(), Box<dyn Error>> {
    let temp_dir = TestContext::temp_dir()?;
    let config = DBConfigBuilder::new()
        .data_dir(temp_dir.dir())
        .memory_manager_max_memtable_memory_usage(50_000_000)
        .memory_manager_max_memory_usage(100_000_000)
        .build();

    // Session 1: open DB, insert 5 entries, then drop without waiting for flush
    {
        let db = DB::new(config.clone())?;
        for i in 0u64..5 {
            db.set(i.to_be_bytes().to_vec(), i.to_be_bytes().to_vec())?;
        }
        // Drop without closing — simulates crash / normal drop
    }

    // Session 2: re-open DB; the recovered WAL should have restored counters.
    // The log_sequence_num should be at least 5 (entries 0-4 used LSNs 0-4).
    {
        let db = DB::new(config.clone())?;
        let current_lsn = db.current_log_seq_num();
        assert!(
            current_lsn >= 5,
            "log_sequence_num should be restored from recovered WAL; got {}",
            current_lsn
        );
    }

    Ok(())
}

/// Test 9: WAL file is deleted after its memtable is flushed to an SSTable.
#[test]
fn test_wal_file_deleted_after_flush() -> Result<(), Box<dyn Error>> {
    let temp_dir = TestContext::temp_dir()?;
    let config = DBConfigBuilder::new()
        .data_dir(temp_dir.dir())
        // Very small memtable so it flushes quickly
        .memory_manager_max_memtable_memory_usage(1_000)
        .memory_manager_max_memory_usage(100_000_000)
        .build();

    let db = DB::new(config.clone())?;

    // Insert enough to trigger a freeze and flush
    for i in 0u64..50 {
        let key = i.to_be_bytes().to_vec();
        db.set(key.clone(), key)?;
    }

    // Wait for the background flush to complete
    std::thread::sleep(Duration::from_millis(200));

    // Collect remaining WAL files
    let wal_files: Vec<_> = std::fs::read_dir(temp_dir.dir())?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("wal"))
        .collect();

    // The WAL for the flushed memtable should have been deleted.
    // At least one flush must have occurred, meaning at least one WAL should be gone.
    // We can't know the exact count without knowing how many freezes happened,
    // but there should be at most 1 remaining WAL (for the current active memtable).
    assert!(
        wal_files.len() <= 1,
        "after flush, only the active WAL should remain; found {} WAL files",
        wal_files.len()
    );

    Ok(())
}

/// Test 10: data inserted before a crash (simulated by drop) is readable after restart.
#[test]
fn test_wal_crash_recovery_reads() -> Result<(), Box<dyn Error>> {
    let temp_dir = TestContext::temp_dir()?;
    let config = DBConfigBuilder::new()
        .data_dir(temp_dir.dir())
        // Large enough that no flush happens during the test
        .memory_manager_max_memtable_memory_usage(50_000_000)
        .memory_manager_max_memory_usage(100_000_000)
        .build();

    // Session 1: insert 30 entries without waiting for flush, then drop
    {
        let db = DB::new(config.clone())?;
        for i in 0u64..30 {
            db.set(i.to_be_bytes().to_vec(), i.to_be_bytes().to_vec())?;
        }
        // Drop here without close() — WAL file stays on disk
    }

    // Session 2: re-open and verify all 30 entries are readable
    {
        let db = DB::new(config.clone())?;
        for i in 0u64..30 {
            let key = i.to_be_bytes().to_vec();
            let val = db.get(&key)?;
            assert_eq!(
                val,
                Some(i.to_be_bytes().to_vec()),
                "key {} should be recoverable from WAL",
                i
            );
        }
    }

    Ok(())
}
