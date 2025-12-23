use std::{fs, io, path::PathBuf, sync::Arc};

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
