#[derive(Clone)]
pub struct DBConfig {
    memtable_probability: f64,
    memtable_expected_num_keys: u32,
    memtable_allowed_max_levels: u32,

    memory_manager_max_memory_usage: usize,

    logging_enabled: bool,
}

impl DBConfig {
    pub fn memtable_probability(&self) -> f64 {
        self.memtable_probability.clone()
    }

    pub fn memtable_expected_num_keys(&self) -> u32 {
        self.memtable_expected_num_keys.clone()
    }

    pub fn memtable_allowed_max_levels(&self) -> u32 {
        self.memtable_allowed_max_levels.clone()
    }

    pub fn memory_manager_max_memory_usage(&self) -> usize {
        self.memory_manager_max_memory_usage.clone()
    }

    pub fn logging_enabled(&self) -> bool {
        self.logging_enabled
    }
}

pub struct DBConfigBuilder {
    config: DBConfig,
}

impl DBConfigBuilder {
    pub fn new() -> DBConfigBuilder {
        DBConfigBuilder {
            config: DBConfig {
                memtable_probability: 0.5,
                memtable_expected_num_keys: 10_000,
                memtable_allowed_max_levels: 32,
                memory_manager_max_memory_usage: 100 * (1 << 20),
                logging_enabled: false,
            },
        }
    }

    pub fn build(self) -> DBConfig {
        self.config.clone()
    }

    pub fn memtable_probability(mut self, val: f64) -> DBConfigBuilder {
        self.config.memtable_probability = val;
        self
    }

    pub fn memtable_expected_num_keys(mut self, val: u32) -> DBConfigBuilder {
        self.config.memtable_expected_num_keys = val;
        self
    }

    pub fn memtable_allowed_max_levels(mut self, val: u32) -> DBConfigBuilder {
        self.config.memtable_allowed_max_levels = val;
        self
    }

    pub fn memory_manager_max_memory_usage(mut self, val: usize) -> DBConfigBuilder {
        self.config.memory_manager_max_memory_usage = val;
        self
    }

    pub fn logging_enabled(mut self, val: bool) -> DBConfigBuilder {
        self.config.logging_enabled = val;
        self
    }
}
