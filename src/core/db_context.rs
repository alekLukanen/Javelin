use std::error::Error;

use super::db_config::DBConfig;

pub struct DBContext {
    config: DBConfig,
}

impl DBContext {
    pub fn new(config: DBConfig) -> DBContext {
        DBContext { config }
    }

    pub fn config(&self) -> &DBConfig {
        &self.config
    }

    pub fn log_info(&self, msg: String) {
        if self.config.logging_enabled() {
            return;
        }
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        println!("[INFO  {}] {}", ts, msg);
    }

    pub fn log_error(&self, msg: String) {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        println!("[ERROR {}] {}", ts, msg);
    }

    pub fn error(&self, err: &(dyn Error + 'static)) {
        self.log_error(err.to_string());
    }
}
