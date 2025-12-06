#[derive(Clone, Debug, PartialEq)]
pub struct LogEntry {
    pub(crate) entry: Entry,
    pub(crate) log_seq_num: u64,
}

impl LogEntry {
    pub fn new(entry: Entry, log_seq_num: u64) -> LogEntry {
        LogEntry { entry, log_seq_num }
    }

    pub fn size(&self) -> usize {
        self.entry.size()
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

#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Put { key: Vec<u8>, val: Vec<u8> },
    Del { key: Vec<u8> },
    Empty,
}

impl Entry {
    pub fn size(&self) -> usize {
        match self {
            Self::Put { key, val } => key.len() + val.len(),
            Self::Del { key } => key.len(),
            Self::Empty => 0,
        }
    }

    pub fn key_size(&self) -> usize {
        match self {
            Self::Put { key, .. } => key.len(),
            Self::Del { key } => key.len(),
            Self::Empty => 0,
        }
    }

    pub fn key(&self) -> Vec<u8> {
        match self {
            Self::Put { key, .. } => key.clone(),
            Self::Del { key } => key.clone(),
            Self::Empty => Vec::new(),
        }
    }

    pub fn value(&self) -> Vec<u8> {
        match self {
            Self::Put { val, .. } => val.clone(),
            Self::Del { .. } => Vec::new(),
            Self::Empty => Vec::new(),
        }
    }

    pub fn id(&self) -> u8 {
        match self {
            Self::Put { .. } => 1,
            Self::Del { .. } => 0,
            Self::Empty { .. } => 128,
        }
    }
}
