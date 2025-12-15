use std::{error::Error, sync::Arc};

use crate::core::{entry, memtable};

use super::{
    memtable::ImmutableMemtable,
    sstable_builder::{Block, SSTableBuilder},
    test_utils::TestContext,
};

#[test]
fn test_build_from_immutable_memtable() -> Result<(), Box<dyn Error>> {
    let tc = TestContext::new();

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
    let immuitable_memtable = ImmutableMemtable::new(0, tc.db_context.clone(), table)?;

    // create the table writer
    let sstable_bldr = SSTableBuilder::build_from_immutable_memtable(
        tc.db_context.clone(),
        immuitable_memtable,
        10_000,
    );

    let blocks: Vec<Block> = sstable_bldr.collect();
    assert_eq!(1, blocks.len());

    // validate the values
    let (data_block_keys, data_block_keys_len, data_block_restarts) =
        match blocks.get(0).expect("expected block") {
            Block::DataBlock {
                keys,
                keys_len,
                restarts,
            } => (keys, keys_len, restarts),
            _ => panic!("expected first block to be a data block"),
        };

    let expected_keys_len: u64 = data_block_keys
        .iter()
        .map(|item| item.size())
        .sum::<usize>() as u64;
    assert_eq!(expected_keys_len, *data_block_keys_len);

    for (idx, pc_entry) in data_block_keys.iter().enumerate() {
        assert_eq!(
            key_values.get(idx).expect("expected value at index").1,
            pc_entry.value.clone(),
        );
        assert_eq!(8, pc_entry.value_len);
    }

    // validate the keys
    let mut reconstructed_keys: Vec<Vec<u8>> = Vec::new();
    let mut previous_key: Vec<u8> = Vec::new();
    let restart_idx: usize = 1;
    for (idx, pc_entry) in data_block_keys.iter().enumerate() {
        println!("pc_entry: {:?}", pc_entry);

        let key_suffix = pc_entry.key_suffix.clone();
        let restart_entry = data_block_restarts.get(restart_idx as usize);
        if idx == 0 {
            previous_key = key_suffix.clone();
            reconstructed_keys.push(previous_key.clone());
            continue;
        } else if let Some(restart_entry) = restart_entry {
            if *restart_entry as usize == idx {
                previous_key = key_suffix.clone();
                reconstructed_keys.push(previous_key.clone());
                continue;
            }
        }
        let mut full_key = Vec::new();
        full_key.extend_from_slice(&previous_key[0..(pc_entry.shared_len as usize)]);
        full_key.extend_from_slice(&pc_entry.key_suffix);
        reconstructed_keys.push(full_key);
    }

    // validate the keys
    for (idx, key) in reconstructed_keys.iter().enumerate() {
        assert_eq!(
            key_values.get(idx).expect("expected value at index").1,
            key[0..key.len() - 9],
        );
    }

    Ok(())
}
