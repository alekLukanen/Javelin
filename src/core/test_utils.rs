use std::{cmp::Ordering, error::Error, fs, io, path::PathBuf, sync::Arc};

use crate::core::{
    entry::{Entry, LogEntry},
    memtable::Memtable,
};

use super::{
    db_config::{DBConfig, DBConfigBuilder},
    db_context::DBContext,
    memory_manager::MemoryManager,
};

pub struct TestContext {
    pub(crate) db_context: Arc<DBContext>,
    pub(crate) memory_manager: Arc<MemoryManager>,
}

impl TestContext {
    pub fn new() -> TestContext {
        let config = DBConfigBuilder::new()
            .logging_enabled(true)
            .debug_logging_eanbled(true)
            .build();
        Self::new_from_config(config)
    }

    pub fn new_from_config(config: DBConfig) -> TestContext {
        let db_context = Arc::new(DBContext::new(config.clone()));
        let memory_manager = Arc::new(MemoryManager::new(db_context.clone()));
        TestContext {
            db_context,
            memory_manager,
        }
    }

    pub fn temp_dir() -> io::Result<TempDir> {
        let dir = std::env::temp_dir().join(format!("javelin-{}", fastrand::u64(0..std::u64::MAX)));

        fs::create_dir(&dir)?;

        Ok(TempDir { path: dir })
    }
}

///////////////////////////////////////////////////////

pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn dir(&self) -> PathBuf {
        self.path.clone()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        println!("dropping temp dir: {:?}", self.path);
        match fs::remove_dir_all(&self.path) {
            Ok(_) => {}
            Err(err) => {
                println!("error: {:?}", err);
            }
        }
    }
}

///////////////////////////////////////////////////////
// Create sample data

pub(crate) enum SampleMemtableBuilder {
    IncreasingPuts {
        size: u64,
        starting_value: u64,
        starting_log_sequence_num: u64,
    },
    DecreasingPuts {
        size: u64,
        starting_value: u64,
        starting_log_sequence_num: u64,
    },
}

impl SampleMemtableBuilder {
    pub(crate) fn build_log_entries(&self, tc: &TestContext) -> Result<LogEntries, Box<dyn Error>> {
        Ok(LogEntries::new(
            self.build(tc)?.skip_list_iter().collect::<Vec<_>>(),
        ))
    }
    pub(crate) fn build(&self, tc: &TestContext) -> Result<Arc<Memtable>, Box<dyn Error>> {
        let table = Arc::new(Memtable::new(
            tc.db_context.clone(),
            tc.memory_manager.clone(),
            10_000_000,
        ));

        match self {
            Self::IncreasingPuts {
                size,
                starting_value,
                starting_log_sequence_num,
            } => {
                for i in *starting_value..*size as u64 {
                    let key = i.to_be_bytes().to_vec();
                    let inserted = table.insert(Arc::new(LogEntry::new(
                        Entry::Put {
                            key: key.clone(),
                            val: key.clone(),
                        },
                        starting_log_sequence_num + i,
                    )))?;
                    assert!(inserted);
                }
            }
            Self::DecreasingPuts {
                size,
                starting_value,
                starting_log_sequence_num,
            } => {
                for i in (*starting_value..*size as u64).rev() {
                    let key = i.to_be_bytes().to_vec();
                    let inserted = table.insert(Arc::new(LogEntry::new(
                        Entry::Put {
                            key: key.clone(),
                            val: key.clone(),
                        },
                        starting_log_sequence_num + i,
                    )))?;
                    assert!(inserted);
                }
            }
        }

        Ok(table)
    }
}

pub struct LogEntries {
    pub(crate) entries: Vec<Arc<LogEntry>>,
    pub(crate) entries_asserted: Vec<bool>,
}

impl LogEntries {
    fn new(entries: Vec<Arc<LogEntry>>) -> LogEntries {
        LogEntries {
            entries: entries.clone(),
            entries_asserted: vec![false; entries.len()],
        }
    }
    pub(crate) fn assert_contains_entry(&mut self, entry: Arc<LogEntry>) {
        match self
            .entries
            .iter()
            .enumerate()
            .find(|(_, item)| ***item == *entry)
        {
            Some((idx, _)) => {
                self.entries_asserted.insert(idx, true);
            }
            None => {
                panic!("entry not in expected entries: {:?}", entry);
            }
        }
    }
    pub(crate) fn assert_all_entries_found(&self) {
        let mut entries_found = 0;
        for (idx, entry) in self.entries.iter().enumerate() {
            if !self.entries_asserted.get(idx).unwrap() {
                println!("[idx={}] entry not found: {:?}", idx, entry);
            } else {
                entries_found += 1;
            }
        }
        if entries_found != self.entries.len() {
            panic!(
                "not all entries found; found={}, not found={}",
                entries_found,
                self.entries.len() - entries_found
            );
        }
    }
    pub(crate) fn filter_in_bounds(
        &mut self,
        lower_bound: Option<Vec<u8>>,
        upper_bound: Option<Vec<u8>>,
    ) {
        let mut entries = Vec::new();
        for entry in &self.entries {
            match &lower_bound {
                Some(lower_bound) => match &lower_bound[..].cmp(entry.entry.key_ref()) {
                    Ordering::Greater => continue,
                    _ => {}
                },
                None => {}
            }
            match &upper_bound {
                Some(upper_bound) => match &upper_bound[..].cmp(entry.entry.key_ref()) {
                    Ordering::Less => continue,
                    _ => {}
                },
                None => {}
            }
            entries.push(entry.clone());
        }
        self.entries = entries;
    }
}
