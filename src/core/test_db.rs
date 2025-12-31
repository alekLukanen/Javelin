use std::{error::Error, fs, thread, time};

use crate::core::{
    block_cache::BlockCache, file_utils, sstable_reader::SSTableReader, test_utils::TestContext,
};

use super::{db::DB, db_config::DBConfigBuilder};

#[test]
fn test_simple_case_no_immutable_memtable_created() -> Result<(), Box<dyn Error>> {
    let config = DBConfigBuilder::new()
        .sstable_max_block_size(100_000)
        .logging_enabled(true)
        .debug_logging_eanbled(true)
        .build();

    let dbase = DB::new(config);

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
fn test_large_case_with_immutable_memtable_created() -> Result<(), Box<dyn Error>> {
    let temp_dir = TestContext::temp_dir()?;

    let config = DBConfigBuilder::new()
        .sstable_max_block_size(500)
        .memory_manager_max_memtable_memory_usage(1_000)
        .data_dir(temp_dir.dir())
        .logging_enabled(true)
        .debug_logging_eanbled(true)
        .build();

    let tc = TestContext::new_from_config(config.clone());
    let dbase = DB::new(config.clone());

    println!("inserting records");

    // 8 * 100 -> 1 active + 7 immutable
    for i in 0..100 as u64 {
        let key = i.to_be_bytes().to_vec();
        dbase.set(key.clone(), key.clone())?;
    }

    println!("wait for sstable to be written to disk...");

    thread::sleep(time::Duration::from_millis(100));

    // check if the sstable exists
    let block_cache = BlockCache::new(tc.db_context.clone(), tc.memory_manager.clone());

    let _ = block_cache
        .get_index_block(&0)?
        .expect("expected index block");

    let sstable_0_path = file_utils::sstable_path(&config, 0);

    // validate that another file doesn't exist
    let expected_files = vec![sstable_0_path];
    for entry in fs::read_dir(config.data_dir())? {
        let entry = entry?;
        match expected_files.iter().find(|item| **item == entry.path()) {
            Some(_) => {}
            None => {
                panic!(
                    "found file which shouldn't exist: {}",
                    entry.path().display()
                );
            }
        }
    }

    println!("closing the db");
    dbase.close()?;

    Ok(())
}
