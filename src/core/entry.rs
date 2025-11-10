#[derive(Clone)]
pub struct LogEntry {
    pub(crate) entry: Entry,
    pub(crate) log_seq_num: u64,
}

impl LogEntry {
    fn new(entry: Entry, log_seq_num: u64) -> LogEntry {
        LogEntry { entry, log_seq_num }
    }
}

impl Default for LogEntry {
    fn default() -> Self {
        LogEntry {
            entry: Entry::Empty,
            log_seq_num: 0,
        }
    }
}

/////////////////////////////////////////////

#[derive(Clone)]
pub enum Entry {
    Put { key: Vec<u8>, val: Vec<u8> },
    Del { key: Vec<u8> },
    Empty,
}
