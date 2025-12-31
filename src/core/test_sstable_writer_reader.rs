use std::{error::Error, io::Cursor, path::PathBuf, sync::Arc};

use crate::core::{
    buf_utils,
    db_config::DBConfigBuilder,
    entry,
    memtable::{self, ImmutableMemtable},
    sstable_builder::SSTableBuilder,
    sstable_reader::SSTableReader,
    sstable_writer::SSTableWriter,
    test_utils::{SampleMemtableBuilder, TestContext},
};

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

    let mut data_blocks: Vec<Vec<u8>> = Vec::new();
    loop {
        let Some(data_block) = sstable_writer.next_data_block()? else {
            break;
        };
        data_blocks.push(data_block);
    }

    // blocks
    // - data
    // - index
    // - footer
    assert_eq!(1, data_blocks.len());

    let expected_index_block_data = sstable_writer.index_block()?;
    let expected_footer_block_data = sstable_writer.footer_block()?;
    let expected_data_block_data = data_blocks.get(0).expect("expected data block 0");

    // drop the writer to close the file
    drop(sstable_writer);

    // create the table reader
    let mut sstable_reader = SSTableReader::new(tc.db_context.clone(), sstable_path.clone())?;

    // read the footer block
    let footer = sstable_reader.footer_block()?;
    assert_eq!(expected_footer_block_data, footer);

    // read the index block
    let index = sstable_reader.index_block()?;
    assert_eq!(expected_index_block_data, index);

    // read the data block
    let mut sstable_reader = SSTableReader::new(tc.db_context.clone(), sstable_path.clone())?;

    let _ = sstable_reader.footer_block()?;
    let index = sstable_reader.index_block()?;

    let (mut index_cursor, _) = buf_utils::entry_and_restart_cursors(&index)?;
    let data_0_index_entry = buf_utils::read_entry(&mut index_cursor)?;
    let data_0_handle = buf_utils::read_handle(&mut Cursor::new(&data_0_index_entry.value))?;
    let data_0 = sstable_reader.data_block(&data_0_handle)?;

    assert_eq!(*expected_data_block_data, data_0);

    Ok(())
}

#[test]
fn test_large_case_sstable_reader_get_block() -> Result<(), Box<dyn Error>> {
    let temp_dir = TestContext::temp_dir()?;

    let config = DBConfigBuilder::new()
        .sstable_max_block_size(1_000)
        .data_dir(temp_dir.dir())
        .logging_enabled(true)
        .debug_logging_eanbled(true)
        .build();
    let tc = TestContext::new_from_config(config);

    let sample_config = SampleMemtableBuilder::IncreasingPuts {
        size: 5000,
        starting_value: 0,
        starting_log_sequence_num: 100,
    };
    let sample_memtable = sample_config.build(&tc)?;

    // create the memtable with some sample data
    let immuitable_memtable = Arc::new(ImmutableMemtable::new(
        0,
        tc.db_context.clone(),
        sample_memtable,
    )?);

    // create the table writer
    let mut sstable_path = PathBuf::from(temp_dir.dir());
    sstable_path.push("sstable-simple.dat");
    let mut sstable_writer = SSTableWriter::new(
        tc.db_context.clone(),
        SSTableBuilder::build_from_immutable_memtable(tc.db_context.clone(), immuitable_memtable),
        sstable_path.clone(),
    )?;

    let mut data_blocks: Vec<Vec<u8>> = Vec::new();
    loop {
        let Some(data_block) = sstable_writer.next_data_block()? else {
            break;
        };
        data_blocks.push(data_block);
    }

    // blocks
    // - data
    // - index
    // - footer
    assert_eq!(157, data_blocks.len());

    let expected_index_block_data = sstable_writer.index_block()?;
    let expected_footer_block_data = sstable_writer.footer_block()?;

    // drop the writer to close the file
    drop(sstable_writer);

    // create the table reader
    let mut sstable_reader = SSTableReader::new(tc.db_context.clone(), sstable_path.clone())?;

    // read the footer block
    let footer = sstable_reader.footer_block()?;
    assert_eq!(expected_footer_block_data, footer);

    // read the index block
    let index = sstable_reader.index_block()?;
    assert_eq!(expected_index_block_data, index);

    // read the data block
    let mut sstable_reader = SSTableReader::new(tc.db_context.clone(), sstable_path.clone())?;

    // using arc to test reference from Arc functionality
    let index = Arc::new(sstable_reader.index_block()?);

    // check each of the data blocks to make sure they're equal to what was written
    for idx in 0..157 {
        let data_block = sstable_reader.get_block(&idx, index.as_ref())?;

        assert_eq!(
            *data_blocks
                .get(idx as usize)
                .expect(format!("expected data block {}", idx).as_str()),
            data_block
        );
    }

    Ok(())
}
