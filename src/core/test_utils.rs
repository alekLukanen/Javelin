use std::sync::Arc;

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
        let config = DBConfigBuilder::new().build();
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
}
