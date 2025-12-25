use std::path::PathBuf;

use crate::core::db_config::DBConfig;

pub(crate) fn sstable_path(config: &DBConfig, num: u64) -> PathBuf {
    let mut sstable_path = PathBuf::new();
    sstable_path.push(config.data_dir());
    sstable_path.push(format!("{}.dat", num));
    sstable_path
}
