#[derive(Clone)]
pub struct DBConfig {
    memtable_probability: f64,
    memtable_expected_num_keys: u32,
    memtable_allowed_max_levels: u32,
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
}
