use std::{
    collections::{BTreeMap, HashMap, HashSet},
    error::Error,
    fmt::Display,
    fs::File,
    io,
    sync::Arc,
};

use super::db_context::DBContext;

#[derive(Clone)]
pub struct SSTableVersion {
    pub(crate) sstable_levels: Vec<SSTableLevel>,
}

impl SSTableVersion {
    fn duplicate(&self) -> Self {
        Self {
            sstable_levels: self.sstable_levels.clone(),
        }
    }
    pub fn levels(&self) -> Vec<usize> {
        self.sstable_levels
            .iter()
            .enumerate()
            .map(|(idx, _)| idx)
            .collect()
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

#[derive(Clone, Debug)]
pub struct ManifestLogFlushMemtable {
    level: usize,
    file_num: u64,
}

impl ManifestLogFlushMemtable {
    fn size(&self) -> usize {
        8 + 8
    }
}

#[derive(Clone, Debug)]
pub struct ManifestLogCompactFiles {
    // from_level
    from_level: usize,
    from_level_file_nums: Vec<u64>,
    // to level
    to_level: usize,
    to_level_file_nums: Vec<u64>,
}

impl ManifestLogCompactFiles {
    fn size(&self) -> usize {
        8 +                                        // from_level
        8 + self.from_level_file_nums.len() * 8 +  // from_level_file_nums
        8 +                                        // to_level
        8 + self.to_level_file_nums.len() * 8 // to_level_file_nums
    }
}

#[derive(Clone, Debug)]
pub enum ManifestLog {
    FlushMemtable(ManifestLogFlushMemtable),
    CompactFiles(ManifestLogCompactFiles),
}

impl ManifestLog {
    fn size(&self) -> usize {
        match self {
            Self::FlushMemtable(log) => log.size(),
            Self::CompactFiles(log) => log.size(),
        }
    }
}

/////////////////////////////////////////

#[derive(Debug)]
pub enum ManifestError {
    IOError(io::Error),
}

impl Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IOError(e) => write!(f, "IOError: {}", e),
        }
    }
}

impl Error for ManifestError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::IOError(e) => Some(e),
        }
    }
}

impl From<io::Error> for ManifestError {
    fn from(value: io::Error) -> Self {
        Self::IOError(value)
    }
}

/////////////////////////////////////////

#[derive(Clone)]
pub struct SSTableLevel {
    pub(crate) sstables: BTreeMap<u64, ()>,
}

pub struct Manifest {
    file: File,

    flushing_memtables: HashSet<usize>,
    sstable_version: Arc<SSTableVersion>,
}

impl Manifest {
    pub fn new(db_context: Arc<DBContext>) -> Result<Manifest, ManifestError> {
        // construct the sstable levels
        let config_num_levels = db_context.config().manifest_num_levels();
        let mut sstable_levels = Vec::with_capacity(config_num_levels);
        for idx in 0..config_num_levels {
            sstable_levels.insert(
                idx,
                SSTableLevel {
                    sstables: BTreeMap::new(),
                },
            );
        }

        let mut manifest_path = db_context.config().data_dir();
        manifest_path.push("manifest.dat");

        let file = File::create(manifest_path)?;

        Ok(Manifest {
            file,
            flushing_memtables: HashSet::new(),
            sstable_version: Arc::new(SSTableVersion { sstable_levels }),
        })
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
            .insert(file_num, ());

        self.sstable_version = Arc::new(sv);
        self.flushing_memtables.remove(&memtable_id);
    }

    fn record_flushed_memtable(
        &mut self,
        level: usize,
        file_num: u64,
    ) -> Result<(), ManifestError> {
        Ok(())
    }
}
