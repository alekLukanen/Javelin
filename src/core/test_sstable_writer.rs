use std::{error::Error, path::PathBuf, sync::Arc};

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
    let mut sstable_path = PathBuf::from(temp_dir.dir());
    sstable_path.push("sstable-simple.dat");
    let mut sstable_writer = SSTableWriter::new(
        tc.db_context.clone(),
        SSTableBuilder::build_from_immutable_memtable(tc.db_context.clone(), immuitable_memtable),
        sstable_path,
    )?;

    let mut blocks: Vec<Block> = Vec::new();
    loop {
        let Some(block) = sstable_writer.next_block()? else {
            break;
        };
        blocks.push(block);
    }

    // blocks
    // - data
    // - index
    // - footer
    assert_eq!(3, blocks.len());

    Ok(())
}
