use std::{error::Error, path::PathBuf, sync::Arc};

use crate::core::{
    block_cache::BlockCache,
    db_config::DBConfigBuilder,
    file_block_iterator::FileBlockIterator,
    iterator::SourceIterator,
    memtable::ImmutableMemtable,
    sstable_builder::SSTableBuilder,
    sstable_writer::SSTableWriter,
    test_utils::{SampleMemtableBuilder, TestContext},
};

#[test]
fn test_simple_case_file_block_iterator_without_lower_and_upper_bounds()
-> Result<(), Box<dyn Error>> {
    let temp_dir = TestContext::temp_dir()?;

    let config = DBConfigBuilder::new()
        .sstable_max_block_size(500)
        .data_dir(temp_dir.dir())
        .logging_enabled(true)
        .debug_logging_eanbled(true)
        .build();
    let tc = TestContext::new_from_config(config);

    let immuitable_memtable = Arc::new(ImmutableMemtable::new(
        0,
        tc.db_context.clone(),
        SampleMemtableBuilder::IncreasingPuts {
            size: 25,
            starting_value: 0,
            starting_log_sequence_num: 100,
        }
        .build(&tc)?,
    )?);

    let block_cache = Arc::new(BlockCache::new(
        tc.db_context.clone(),
        tc.memory_manager.clone(),
    ));

    // file attributes
    let file_id = 1;
    let level = 0;

    // create the table writer
    let mut sstable_path = PathBuf::from(temp_dir.dir());
    sstable_path.push(format!("{}.dat", file_id));
    let mut sstable_writer = SSTableWriter::new(
        tc.db_context.clone(),
        SSTableBuilder::build_from_immutable_memtable(tc.db_context.clone(), immuitable_memtable),
        sstable_path.clone(),
    )?;

    let mut block_idx = 0;
    loop {
        let Some(data_block) = sstable_writer.next_data_block()? else {
            break;
        };

        println!("adding data block to cache");
        block_cache.add_data_block(file_id, block_idx, data_block)?;
        block_idx += 1;
    }

    let index_block = sstable_writer.index_block()?;
    let footer_block = sstable_writer.footer_block()?;
    block_cache.add_sstable(file_id, footer_block, index_block)?;

    // drop the writer to close the file
    drop(sstable_writer);

    let log_sequence_num: u64 = 200;
    let mut iter = FileBlockIterator::new(
        tc.db_context.clone(),
        block_cache.clone(),
        level,
        file_id,
        log_sequence_num,
        None,
        None,
    );

    loop {
        let Some(entry) = iter.next()? else {
            break;
        };
        println!("entry: {:?}", entry);
    }

    Ok(())
}

#[test]
fn test_simple_case_file_block_iterator_with_lower_but_no_upper_bound() -> Result<(), Box<dyn Error>>
{
    let temp_dir = TestContext::temp_dir()?;

    let config = DBConfigBuilder::new()
        .sstable_max_block_size(500)
        .data_dir(temp_dir.dir())
        .logging_enabled(true)
        .debug_logging_eanbled(true)
        .build();
    let tc = TestContext::new_from_config(config);

    let immuitable_memtable = Arc::new(ImmutableMemtable::new(
        0,
        tc.db_context.clone(),
        SampleMemtableBuilder::IncreasingPuts {
            size: 25,
            starting_value: 0,
            starting_log_sequence_num: 100,
        }
        .build(&tc)?,
    )?);

    let block_cache = Arc::new(BlockCache::new(
        tc.db_context.clone(),
        tc.memory_manager.clone(),
    ));

    // file attributes
    let file_id = 1;
    let level = 0;

    // create the table writer
    let mut sstable_path = PathBuf::from(temp_dir.dir());
    sstable_path.push(format!("{}.dat", file_id));
    let mut sstable_writer = SSTableWriter::new(
        tc.db_context.clone(),
        SSTableBuilder::build_from_immutable_memtable(tc.db_context.clone(), immuitable_memtable),
        sstable_path.clone(),
    )?;

    let mut block_idx = 0;
    loop {
        let Some(data_block) = sstable_writer.next_data_block()? else {
            break;
        };

        println!("adding data block to cache");
        block_cache.add_data_block(file_id, block_idx, data_block)?;
        block_idx += 1;
    }

    let index_block = sstable_writer.index_block()?;
    let footer_block = sstable_writer.footer_block()?;
    block_cache.add_sstable(file_id, footer_block, index_block)?;

    // drop the writer to close the file
    drop(sstable_writer);

    let log_sequence_num: u64 = 200;
    let lower_bound = 15u64.to_be_bytes().to_vec();
    let mut iter = FileBlockIterator::new(
        tc.db_context.clone(),
        block_cache.clone(),
        level,
        file_id,
        log_sequence_num,
        Some(lower_bound),
        None,
    );

    loop {
        let Some(entry) = iter.next()? else {
            println!("iterator empty");
            break;
        };
        println!("entry: {:?}", entry);
    }

    Ok(())
}
