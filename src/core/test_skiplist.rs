use std::{error::Error, sync::Arc};

use super::{
    entry::{Entry, LogEntry},
    skiplist::SkipList,
};

fn new_put_int_entry(key: i32, log_seq_num: u64) -> Arc<LogEntry> {
    let key_bytes = key.to_be_bytes().to_vec();
    Arc::new(LogEntry::new(
        Entry::Put {
            key: key_bytes.clone(),
            val: key_bytes.clone(),
        },
        log_seq_num,
    ))
}

fn new_del_int_entry(key: i32, log_seq_num: u64) -> Arc<LogEntry> {
    let key_bytes = key.to_be_bytes().to_vec();
    Arc::new(LogEntry::new(
        Entry::Del {
            key: key_bytes.clone(),
        },
        log_seq_num,
    ))
}

#[test]
fn test_duplicates() -> Result<(), Box<dyn Error>> {
    let skiplist = SkipList::new(0.5, 1_000, 3);

    // insert
    let put_entry_1 = new_put_int_entry(3, 1);
    skiplist.insert(put_entry_1.clone());
    skiplist.insert(put_entry_1.clone());

    // get
    let got_entry_1 = skiplist.get(&(3 as i32).to_be_bytes(), 10);
    assert_eq!(Some(put_entry_1.clone()), got_entry_1);

    // insert
    let put_entry_2 = new_put_int_entry(3, 2);
    skiplist.insert(put_entry_2.clone());

    // get
    let got_entry_2 = skiplist.get(&(3 as i32).to_be_bytes(), 10);
    assert_eq!(Some(put_entry_2.clone()), got_entry_2);

    // insert
    let put_entry_3 = new_put_int_entry(3, 3);
    skiplist.insert(put_entry_3.clone());

    // get
    let got_entry_3 = skiplist.get(&(3 as i32).to_be_bytes(), 10);
    assert_eq!(Some(put_entry_3.clone()), got_entry_3);

    // insert
    let _ = new_put_int_entry(3, 1);
    skiplist.insert(put_entry_3.clone());

    // get most recent sequence 3, not the most recent insert
    let got_entry_3_prime = skiplist.get(&(3 as i32).to_be_bytes(), 10);
    assert_eq!(Some(put_entry_3.clone()), got_entry_3_prime);

    // get most recent sequence 2, not sequence 3
    let got_entry_2_prime = skiplist.get(&(3 as i32).to_be_bytes(), 2);
    assert_eq!(Some(put_entry_2.clone()), got_entry_2_prime);

    // get most recent sequence 1, not sequence 3
    let got_entry_1_prime = skiplist.get(&(3 as i32).to_be_bytes(), 1);
    assert_eq!(Some(put_entry_1.clone()), got_entry_1_prime);

    Ok(())
}

#[test]
fn test_skiplist_insert_and_delete() -> Result<(), Box<dyn Error>> {
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
fn test_skiplist_with_increasing_insert_keys() -> Result<(), Box<dyn Error>> {
    let skiplist = SkipList::new(0.5, 1_000, 3);

    let mut expected_keys: Vec<Vec<u8>> = Vec::new();

    for i in 0..1000 as u64 {
        let key = i.to_be_bytes().to_vec();
        skiplist.insert(Arc::new(LogEntry::new(
            Entry::Put {
                key: key.clone(),
                val: key.clone(),
            },
            i,
        )));
        expected_keys.push(key.clone());
    }

    for (i, key) in expected_keys.iter().enumerate() {
        let entry = skiplist.get(&key, i as u64);
        let entry_val = entry.expect("expected a log entry");
        assert_eq!(
            Arc::new(LogEntry::new(
                Entry::Put {
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

#[test]
fn test_skiplist_with_decreasing_insert_keys() -> Result<(), Box<dyn Error>> {
    let skiplist = SkipList::new(0.5, 1_000, 3);

    let mut expected_keys: Vec<Vec<u8>> = Vec::new();

    for i in (0..1000 as u64).rev() {
        let key = i.to_be_bytes().to_vec();
        skiplist.insert(Arc::new(LogEntry::new(
            Entry::Put {
                key: key.clone(),
                val: key.clone(),
            },
            999 - i,
        )));
        expected_keys.push(key.clone())
    }

    for (i, key) in expected_keys.iter().enumerate() {
        let entry = skiplist.get(&key, i as u64);
        let entry_val = entry.expect("expected a log entry");
        assert_eq!(
            Arc::new(LogEntry::new(
                Entry::Put {
                    key: key.clone(),
                    val: key.clone()
                },
                i as u64
            )),
            entry_val
        );
    }

    Ok(())
}
