use std::error::Error;

use super::{db::DB, db_config::DBConfigBuilder};

#[test]
fn test_simple_case_no_immutable_memtable_created() -> Result<(), Box<dyn Error>> {
    let config = DBConfigBuilder::new()
        .memory_manager_max_memtable_memory_usage(100_000)
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
    let config = DBConfigBuilder::new()
        .memory_manager_max_memtable_memory_usage(1_000)
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
