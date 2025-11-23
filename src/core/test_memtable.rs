use std::sync::Arc;

use super::{db_config, db_context, entry, memory_manager, memtable};

#[test]
fn test_memtable_immutible_tables() -> Result<(), Box<dyn std::error::Error>> {
    let db_config = db_config::DBConfigBuilder::new()
        .memory_manager_max_memory_usage(10000)
        .memory_manager_max_memtable_memory_usage(100)
        .build();
    let db_context = Arc::new(db_context::DBContext::new(db_config));
    let memory_manager = Arc::new(memory_manager::MemoryManager::new(db_context.clone()));

    let table = memtable::MemtableManager::new(db_context, memory_manager);

    let mut expected_keys: Vec<Vec<u8>> = Vec::new();

    println!("inserting records");

    // 8 * 100 -> 1 active + 7 immutable
    for i in 0..100 as u64 {
        let key = i.to_be_bytes().to_vec();
        table.insert(Arc::new(entry::LogEntry::new(
            entry::Entry::Put {
                key: key.clone(),
                val: key.clone(),
            },
            i,
        )))?;
        expected_keys.push(key.clone());
    }

    println!("getting records");

    for (i, key) in expected_keys.iter().enumerate() {
        println!("i: {}, key: {:?}", i, key);
        let entry = table.get(&key, i as u64)?;
        let entry_val = entry.expect("expected a log entry");
        assert_eq!(
            Arc::new(entry::LogEntry::new(
                entry::Entry::Put {
                    key: key.clone(),
                    val: key.clone()
                },
                i as u64,
            )),
            entry_val
        );
    }

    Ok(())
}
