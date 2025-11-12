use std::{collections::binary_heap::Iter, sync::Mutex};

use crate::core::skiplist::SkipList;

#[derive(Clone)]
pub struct DBConfig {
    memtable_probability: f64,
    memtable_expected_num_keys: u32,
    memtable_allowed_max_levels: u32,
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

pub enum DBError {}

pub struct DB {
    active_memtable: Mutex<SkipList>,
}

impl DB {
    pub fn new(config: DBConfig) -> DB {
        DB {
            active_memtable: Mutex::new(SkipList::new(
                config.memtable_probability,
                config.memtable_expected_num_keys,
                config.memtable_allowed_max_levels,
            )),
        }
    }

    pub fn get(&self, key: &Vec<u8>) -> Option<Vec<u8>> {
        None
    }

    pub fn set(&self, key: Vec<u8>, val: Vec<u8>) -> Result<(), DBError> {
        Ok(())
    }

    pub fn delete(&self, key: &Vec<u8>) -> Result<(), DBError> {
        Ok(())
    }

    pub fn iterator(&self, opts: IteratorOptions) -> Result<Iterator, DBError> {
        Ok(Iterator {})
    }
}

pub struct IteratorOptions {
    lower_bound: Vec<u8>,
    upper_bound: Vec<u8>,
}

pub struct Iterator {}
