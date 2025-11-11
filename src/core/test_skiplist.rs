use super::{
    entry::{Entry, LogEntry},
    skiplist::SkipList,
};

fn new_put_int_entry(key: i32, log_seq_num: u64) -> LogEntry {
    let key_bytes = key.to_be_bytes().to_vec();
    LogEntry::new(
        Entry::Put {
            key: key_bytes.clone(),
            val: key_bytes.clone(),
        },
        log_seq_num,
    )
}

fn new_del_int_entry(key: i32, log_seq_num: u64) -> LogEntry {
    let key_bytes = key.to_be_bytes().to_vec();
    LogEntry::new(
        Entry::Del {
            key: key_bytes.clone(),
        },
        log_seq_num,
    )
}

#[test]
fn test_skiplist_insert_and_delete() -> Result<(), Box<dyn std::error::Error>> {
    let skiplist = SkipList::new(0.5, 1_000, 3);

    let key_1: i32 = 1;
    let key_1_bytes = key_1.to_be_bytes().to_vec();

    let put_entry_1 = new_put_int_entry(1, 1);
    skiplist.insert(put_entry_1.clone());

    let put_entry_2 = new_put_int_entry(2, 2);
    skiplist.insert(put_entry_2.clone());

    let del_entry_1 = new_del_int_entry(1, 3);
    skiplist.insert(del_entry_1.clone());

    let got_entry_1 = skiplist.get(&key_1_bytes, 2);
    assert_eq!(Some(put_entry_1.clone()), got_entry_1);

    let got_del_entry_1 = skiplist.get(&key_1_bytes, 3);
    assert_eq!(Some(del_entry_1.clone()), got_del_entry_1);

    Ok(())
}

#[test]
fn test_skiplist_with_increasing_insert_keys() -> Result<(), Box<dyn std::error::Error>> {
    let skiplist = SkipList::new(0.5, 1_000, 3);

    let mut expected_keys: Vec<Vec<u8>> = Vec::new();

    for i in 0..1000 as u64 {
        let key = i.to_be_bytes().to_vec();
        skiplist.insert(LogEntry::new(
            Entry::Put {
                key: key.clone(),
                val: key.clone(),
            },
            i,
        ));
        expected_keys.push(key.clone())
    }

    for (i, key) in expected_keys.iter().enumerate() {
        let entry = skiplist.get(&key, i as u64);
        let entry_val = entry.expect("expected a log entry");
        assert_eq!(
            LogEntry::new(
                Entry::Put {
                    key: key.clone(),
                    val: key.clone()
                },
                i as u64,
            ),
            entry_val
        );
    }

    Ok(())
}

#[test]
fn test_skiplist_with_decreasing_insert_keys() -> Result<(), Box<dyn std::error::Error>> {
    let skiplist = SkipList::new(0.5, 1_000, 3);

    let mut expected_keys: Vec<Vec<u8>> = Vec::new();

    for i in (0..1000 as u64).rev() {
        let key = i.to_be_bytes().to_vec();
        skiplist.insert(LogEntry::new(
            Entry::Put {
                key: key.clone(),
                val: key.clone(),
            },
            999 - i,
        ));
        expected_keys.push(key.clone())
    }

    for (i, key) in expected_keys.iter().enumerate() {
        let entry = skiplist.get(&key, i as u64);
        let entry_val = entry.expect("expected a log entry");
        assert_eq!(
            LogEntry::new(
                Entry::Put {
                    key: key.clone(),
                    val: key.clone()
                },
                i as u64
            ),
            entry_val
        );
    }

    Ok(())
}
