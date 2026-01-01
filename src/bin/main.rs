use std::{error::Error, fs, sync::Arc, thread, time};

use Javelin::core::{
    db::{self, DB},
    db_config::{self, DBConfigBuilder},
    entry::{Entry, LogEntry},
    skiplist::SkipList,
};

use Javelin::core::test_utils::{SampleMemtableBuilder, TestContext};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // try_skiplist()?;

    // try_db()?;

    // try_data_formatting();

    large_db_test()?;

    Ok(())
}

fn large_db_test() -> Result<(), Box<dyn Error>> {
    let temp_dir = TestContext::temp_dir()?;

    let config = DBConfigBuilder::new()
        .sstable_max_block_size(5_000)
        .memory_manager_max_memtable_memory_usage(10_000)
        .data_dir(temp_dir.dir())
        .logging_enabled(true)
        .debug_logging_eanbled(false)
        .build();

    let tc = TestContext::new_from_config(config.clone());
    let dbase = DB::new(config.clone());

    println!("inserting records");

    let sample_config = SampleMemtableBuilder::IncreasingPuts {
        size: 10_000,
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

fn try_data_formatting() {
    let val1: u32 = 256;
    let val1_data_be = val1.to_be_bytes();
    let val1_data_le = val1.to_le_bytes();

    println!("val1_data_be: {:?}", val1_data_be);
    println!("val1_data_le: {:?}", val1_data_le);
}

fn try_db() -> Result<(), Box<dyn std::error::Error>> {
    let db_config = db_config::DBConfigBuilder::new().build();

    let db = db::DB::new(db_config);

    // set 1
    db.set(vec![1u8], vec![1u8, 1u8])?;

    let val = db.get(&vec![1u8])?;
    assert_eq!(val, Some(vec![1u8, 1u8]));

    let val = db.get(&vec![2u8])?;
    assert_eq!(val, None);

    // set 2
    db.set(vec![3u8], vec![3u8, 1u8])?;

    let val = db.get(&vec![3u8])?;
    assert_eq!(val, Some(vec![3u8, 1u8]));

    Ok(())
}

fn try_skiplist() -> Result<(), Box<dyn std::error::Error>> {
    println!("Hello, world!");

    let skiplist = SkipList::new(0.5, 1_000, 3);

    skiplist.insert(Arc::new(LogEntry::new(
        Entry::Put {
            key: vec![1u8],
            val: vec![1u8],
        },
        1,
    )));

    let entry = skiplist.get(&vec![1u8], 1);
    let entry_not_found = skiplist.get(&vec![1u8], 0);

    println!("entry: {:?}, entry_not_found: {:?}", entry, entry_not_found);

    Ok(())
}
