use std::{collections::HashSet, sync::Arc};

use super::db_context::DBContext;

#[derive(Clone)]
pub struct SSTableVersion {
    sstable_levels: Vec<SSTableLevel>,
}

impl SSTableVersion {
    fn duplicate(&self) -> Self {
        Self {
            sstable_levels: self.sstable_levels.clone(),
        }
    }
}

impl Default for SSTableVersion {
    fn default() -> Self {
        SSTableVersion {
            sstable_levels: Vec::new(),
        }
    }
}

/////////////////////////////////////////

#[derive(Clone)]
struct SSTableLevel {
    sstables: HashSet<u64>,
}

pub struct Manifest {
    flushing_memtables: HashSet<usize>,

    sstable_version: Arc<SSTableVersion>,
}

impl Manifest {
    pub fn new(db_context: Arc<DBContext>) -> Manifest {
        // construct the sstable levels
        let config_num_levels = db_context.config().manifest_num_levels();
        let mut sstable_levels = Vec::with_capacity(config_num_levels);
        for _ in 0..config_num_levels {
            sstable_levels.push(SSTableLevel {
                sstables: HashSet::new(),
            })
        }

        Manifest {
            flushing_memtables: HashSet::new(),
            sstable_version: Arc::new(SSTableVersion { sstable_levels }),
        }
    }

    pub fn sstable_version(&self) -> Arc<SSTableVersion> {
        self.sstable_version.clone()
    }

    pub fn add_flushing_memtable(&mut self, idx: usize) {
        self.flushing_memtables.insert(idx);
    }

    pub fn finalize_flushing_memtable(&mut self, memtable_id: usize, file_num: u64) {
        let mut sv = self.sstable_version.duplicate();
        sv.sstable_levels
            .get_mut(0)
            .expect("expected level 0")
            .sstables
            .insert(file_num);

        self.sstable_version = Arc::new(sv);
        self.flushing_memtables.remove(&memtable_id);
    }
}
