use std::{error::Error, fs, io, path::PathBuf, sync::Arc};

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
        Ok(LogEntries {
            entries: self.build(tc)?.skip_list_iter().collect::<Vec<_>>(),
        })
    }
    pub(crate) fn build(&self, tc: &TestContext) -> Result<Arc<Memtable>, Box<dyn Error>> {
        let table = Arc::new(Memtable::new(
            tc.db_context.clone(),
            tc.memory_manager.clone(),
        ));

        match self {
            Self::IncreasingPuts {
                size,
                starting_value,
                starting_log_sequence_num,
            } => {
                for i in *starting_value..*size as u64 {
                    let key = i.to_be_bytes().to_vec();
                    table.insert(Arc::new(LogEntry::new(
                        Entry::Put {
                            key: key.clone(),
                            val: key.clone(),
                        },
                        starting_log_sequence_num + i,
                    )))?;
                }
            }
            Self::DecreasingPuts {
                size,
                starting_value,
                starting_log_sequence_num,
            } => {
                for i in (*starting_value..*size as u64).rev() {
                    let key = i.to_be_bytes().to_vec();
                    table.insert(Arc::new(LogEntry::new(
                        Entry::Put {
                            key: key.clone(),
                            val: key.clone(),
                        },
                        starting_log_sequence_num + i,
                    )))?;
                }
            }
        }

        Ok(table)
    }
}

pub struct LogEntries {
    entries: Vec<Arc<LogEntry>>,
}

impl LogEntries {
    pub(crate) fn contains_entry(&self, entry: Arc<LogEntry>) -> bool {
        self.entries.iter().find(|item| ***item == *entry).is_some()
    }
}
