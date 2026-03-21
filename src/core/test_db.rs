use std::{error::Error, fs, thread, time};

use crate::core::test_utils::{SampleMemtableBuilder, TestContext};

use super::{db::DB, db_config::DBConfigBuilder};

#[test]
fn test_simple_case_no_immutable_memtable_created() -> Result<(), Box<dyn Error>> {
    let temp_dir = TestContext::temp_dir()?;

    let config = DBConfigBuilder::new()
        .sstable_max_block_size(100_000)
        .data_dir(temp_dir.dir())
        .logging_enabled(true)
        .debug_logging_eanbled(true)
        .build();

    let dbase = DB::new(config)?;

    println!("inserting records");

    // 8 * 100 -> 1 active + 7 immutable
    for i in 0..100 as u64 {
        let key = i.to_be_bytes().to_vec();
        dbase.set(key.clone(), key.clone())?;
    }

    println!("getting records");

    for i in 0..100 as u64 {
        let key = i.to_be_bytes().to_vec();
        println!("i: {}, key: {:?}", i, key);

        let val = dbase.get(&key)?;
        assert_eq!(Some(key), val);
    }

    println!("closing the db");
    dbase.close()?;

    Ok(())
}

#[test]
fn test_simple_case_with_immutable_memtable_created() -> Result<(), Box<dyn Error>> {
    let temp_dir = TestContext::temp_dir()?;

    let config = DBConfigBuilder::new()
        .sstable_max_block_size(500)
        .memory_manager_max_memtable_memory_usage(1_000)
        .data_dir(temp_dir.dir())
        .logging_enabled(true)
        .debug_logging_eanbled(false)
        .build();

    let tc = TestContext::new_from_config(config.clone());
    let dbase = DB::new(config.clone())?;

    println!("inserting records");

    let sample_config = SampleMemtableBuilder::IncreasingPuts {
        size: 100,
        starting_value: 0,
        starting_log_sequence_num: 0,
    };
    let mut sample_entries = sample_config.build_log_entries(&tc)?;

    for entry in &sample_entries.entries {
        dbase.set(entry.entry.key(), entry.entry.value())?;
    }

    println!("finished inserting records");
    println!("wait for sstable to be written to disk...");
    thread::sleep(time::Duration::from_millis(250));

    for entry in fs::read_dir(config.data_dir())? {
        let entry = entry?;
        println!("data dir entry: {:?}", entry.file_name());
    }

    // validate the entries can be retrieved
    for entry in &sample_entries.entries.clone() {
        let Some(_) = dbase.get(&entry.entry.key())? else {
            println!("entry not found: {:?}", entry);
            panic!("entry not found");
        };
        sample_entries.assert_contains_entry(entry.clone());
    }

    println!("closing the db");
    dbase.close()?;

    Ok(())
}

#[test]
fn test_large_case_with_immutable_memtable_created() -> Result<(), Box<dyn Error>> {
    let temp_dir = TestContext::temp_dir()?;

    let config = DBConfigBuilder::new()
        .sstable_max_block_size(5_000)
        .memory_manager_max_memtable_memory_usage(10_000)
        .data_dir(temp_dir.dir())
        .logging_enabled(true)
        .debug_logging_eanbled(false)
        .build();

    let tc = TestContext::new_from_config(config.clone());
    let dbase = DB::new(config.clone())?;

    println!("inserting records");

    let sample_config = SampleMemtableBuilder::IncreasingPuts {
        size: 1_000,
        starting_value: 0,
        starting_log_sequence_num: 0,
    };
    let mut sample_entries = sample_config.build_log_entries(&tc)?;

    for entry in &sample_entries.entries {
        dbase.set(entry.entry.key(), entry.entry.value())?;
    }

    println!("finished inserting records");
    println!("wait for sstable to be written to disk...");
    thread::sleep(time::Duration::from_millis(250));

    for entry in fs::read_dir(config.data_dir())? {
        let entry = entry?;
        println!("data dir entry: {:?}", entry.file_name());
    }

    // validate the entries can be retrieved
    let mut latency_sum = 0;
    let mut latency_samples = 0;
    for entry in &sample_entries.entries.clone() {
        let get_start = std::time::Instant::now();
        let Some(_) = dbase.get(&entry.entry.key())? else {
            println!("entry not found: {:?}", entry);
            panic!("entry not found");
        };
        latency_sum += get_start.elapsed().as_micros();
        latency_samples += 1;
        sample_entries.assert_contains_entry(entry.clone());
    }

    println!(
        "[METRIC] average get latency: {} micro seconds",
        latency_sum / latency_samples
    );

    sample_entries.assert_all_entries_found();

    println!("closing the db");
    dbase.close()?;

    Ok(())
}
