use super::memtable::Memtable;
use crate::core::db_config::DBConfig;

pub enum DBError {}

pub struct DB {
    memtable: Memtable,
}

impl DB {
    pub fn new(config: DBConfig) -> DB {
        DB {
            memtable: Memtable::new(config),
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
