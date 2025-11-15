use std::sync::atomic;

pub struct WAL {
    log_sequence_num: atomic::AtomicU64,
}

impl WAL {
    pub fn new() -> WAL {
        WAL {
            log_sequence_num: atomic::AtomicU64::new(0),
        }
    }

    pub fn incr_log_sequence_num(&self) -> u64 {
        self.log_sequence_num
            .fetch_add(1, atomic::Ordering::Relaxed)
    }
}
