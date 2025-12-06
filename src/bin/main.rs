use std::sync::Arc;

use Javelin::core::{
    db, db_config,
    entry::{Entry, LogEntry},
    skiplist::SkipList,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // try_skiplist()?;

    // try_db()?;

    try_data_formatting();

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
