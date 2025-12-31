use std::path::PathBuf;

#[derive(Clone)]
pub struct DBConfig {
    memtable_probability: f64,
    memtable_expected_num_keys: u32,
    memtable_allowed_max_levels: u32,

    memory_manager_max_memory_usage: usize,
    memory_manager_max_memtable_memory_usage: usize,

    block_cache_num_shards: usize,
    data_dir: Option<PathBuf>,
    sstable_max_block_size: usize,
    sstable_restart_interval: usize,

    manifest_num_levels: usize,

    logging_enabled: bool,
    debug_logging_enabled: bool,
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

    pub fn memory_manager_max_memtable_memory_usage(&self) -> usize {
        self.memory_manager_max_memtable_memory_usage.clone()
    }

    pub fn block_cache_num_shards(&self) -> usize {
        self.block_cache_num_shards.clone()
    }

    pub fn data_dir(&self) -> PathBuf {
        match &self.data_dir {
            Some(data_dir) => data_dir.clone(),
            None => panic!("missing data_dir config value"),
        }
    }

    pub fn sstable_max_block_size(&self) -> usize {
        self.sstable_max_block_size.clone()
    }

    pub fn sstable_restart_interval(&self) -> usize {
        self.sstable_restart_interval.clone()
    }

    pub fn manifest_num_levels(&self) -> usize {
        self.manifest_num_levels.clone()
    }

    pub fn logging_enabled(&self) -> bool {
        self.logging_enabled
    }

    pub fn debug_logging_enabled(&self) -> bool {
        self.debug_logging_enabled
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
                memtable_expected_num_keys: 50_000,
                memtable_allowed_max_levels: 32,
                memory_manager_max_memory_usage: 100 * (1 << 20),
                memory_manager_max_memtable_memory_usage: 10 * (1 << 20),
                block_cache_num_shards: 3,
                logging_enabled: false,
                debug_logging_enabled: false,
                data_dir: None,
                manifest_num_levels: 3,
                sstable_max_block_size: 2 << 14,
                sstable_restart_interval: 4,
            },
        }
    }
    pub fn new_from_config(config: &DBConfig) -> DBConfigBuilder {
        DBConfigBuilder {
            config: config.clone(),
        }
    }

    pub fn build(self) -> DBConfig {
        if self.config.memory_manager_max_memtable_memory_usage
            > self.config.memory_manager_max_memory_usage
        {
            panic!("memtable memory usage greater than max memory manager memory usage");
        }
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

    pub fn memory_manager_max_memtable_memory_usage(mut self, val: usize) -> DBConfigBuilder {
        self.config.memory_manager_max_memtable_memory_usage = val;
        self
    }

    pub fn block_cache_num_shards(mut self, val: usize) -> DBConfigBuilder {
        self.config.block_cache_num_shards = val;
        self
    }

    pub fn data_dir(mut self, data_dir: PathBuf) -> DBConfigBuilder {
        self.config.data_dir = Some(data_dir);
        self
    }

    pub fn sstable_max_block_size(mut self, val: usize) -> DBConfigBuilder {
        self.config.sstable_max_block_size = val;
        self
    }

    pub fn sstable_restart_interval(mut self, val: usize) -> DBConfigBuilder {
        self.config.sstable_restart_interval = val;
        self
    }

    pub fn manifest_num_levels(mut self, val: usize) -> DBConfigBuilder {
        self.config.manifest_num_levels = val;
        self
    }

    pub fn logging_enabled(mut self, val: bool) -> DBConfigBuilder {
        self.config.logging_enabled = val;
        self
    }

    pub fn debug_logging_eanbled(mut self, val: bool) -> DBConfigBuilder {
        self.config.debug_logging_enabled = val;
        self
    }
}
