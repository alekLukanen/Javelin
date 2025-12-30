use std::{error::Error, io::Cursor, path::PathBuf, sync::Arc};

use crate::core::{
    buf_utils,
    db_config::DBConfigBuilder,
    entry,
    memtable::{self, ImmutableMemtable},
    sstable_builder::{Block, SSTableBuilder},
    sstable_reader::SSTableReader,
    sstable_writer::SSTableWriter,
    test_utils::TestContext,
};

/*
#[test]
fn test_simple_case_sstable_writer_and_reader() -> Result<(), Box<dyn Error>> {
    let temp_dir = TestContext::temp_dir()?;

    let config = DBConfigBuilder::new()
        .sstable_max_block_size(10_000)
        .data_dir(temp_dir.dir())
        .logging_enabled(true)
        .debug_logging_eanbled(true)
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
        sstable_path.clone(),
    )?;

    let mut blocks: Vec<Block> = Vec::new();
    loop {
        let Some(block) = sstable_writer.next_block()? else {
            break;
        };
        blocks.push(block);
    }

    // drop the writer to close the file
    drop(sstable_writer);

    // blocks
    // - data
    // - index
    // - footer
    assert_eq!(3, blocks.len());

    let expected_footer_block_data = match blocks
        .iter()
        .find(|item| matches!(item, Block::FooterBlock(_)))
        .expect("footer block")
    {
        Block::FooterBlock(fb) => fb.clone(),
        _ => panic!("not footer block"),
    };
    let expected_index_block_data = match blocks
        .iter()
        .find(|item| matches!(item, Block::IndexBlock(_)))
        .expect("index block")
    {
        Block::IndexBlock(fb) => fb.clone(),
        _ => panic!("not index block"),
    };
    let expected_data_block_data = match blocks
        .iter()
        .find(|item| matches!(item, Block::DataBlock(_)))
        .expect("data block")
    {
        Block::DataBlock(fb) => fb.clone(),
        _ => panic!("not data block"),
    };

    // create the table reader
    let mut sstable_reader = SSTableReader::new(tc.db_context.clone(), sstable_path.clone())?;

    // read the footer block
    let footer = sstable_reader.footer_block()?;
    assert_eq!(expected_footer_block_data, footer);

    // read the index block
    let index = sstable_reader.index_block()?;
    assert_eq!(expected_index_block_data, index);

    // read the data block
    let data_handle = buf_utils::read_handle(&mut Cursor::new(&index.keys.get(0).unwrap().value))?;
    let data = sstable_reader.data_block(&data_handle)?;
    assert_eq!(expected_data_block_data, data);

    Ok(())
}
*/
