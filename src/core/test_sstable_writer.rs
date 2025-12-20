use std::{error::Error, sync::Arc};

use crate::core::{
    db_config::DBConfigBuilder,
    entry,
    memtable::{self, ImmutableMemtable},
    sstable_builder::{Block, SSTableBuilder},
    sstable_writer::SSTableWriter,
    test_utils::TestContext,
};

#[test]
fn test_simple_case_sstable_writer() -> Result<(), Box<dyn Error>> {
    let temp_dir = TestContext::temp_dir()?;

    println!("dir: {:?}", temp_dir.dir());

    return Ok(());

    let config = DBConfigBuilder::new()
        .sstable_max_block_size(10_000)
        .data_dir(temp_dir.dir())
        .build();
    let tc = TestContext::new_from_config(config);

    // create the memtable with some sample data
    let table = Arc::new(memtable::Memtable::new(
        tc.db_context.clone(),
        tc.memory_manager.clone(),
    ));

    let mut key_values: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    for i in 0..24 as u64 {
        let key = i.to_be_bytes().to_vec();
        table.insert(Arc::new(entry::LogEntry::new(
            entry::Entry::Put {
                key: key.clone(),
                val: key.clone(),
            },
            i,
        )))?;
        key_values.push((key.clone(), key.clone()));
    }
    let immuitable_memtable = Arc::new(ImmutableMemtable::new(0, tc.db_context.clone(), table)?);

    // create the table writer
    let sstable_bldr =
        SSTableBuilder::build_from_immutable_memtable(tc.db_context.clone(), immuitable_memtable);
    let sstable_writer = SSTableWriter::new(tc.db_context.clone(), sstable_bldr, temp_dir.dir())?;

    let blocks: Vec<Block> = sstable_bldr.collect();
    assert_eq!(1, blocks.len());

    Ok(())
}
