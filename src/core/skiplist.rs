use std::{cell::RefCell, cmp::Ordering, error::Error, fmt::Display, rc::Rc, sync::Mutex};

use super::entry::{Entry, LogEntry};

#[derive(Debug)]
pub enum SkipListError {
    MutexLockFailed(String),
}

impl Display for SkipListError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkipListError::MutexLockFailed(e) => write!(f, "MutexLockFailed: {}", e),
        }
    }
}

impl Error for SkipListError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            SkipListError::MutexLockFailed(_) => None,
        }
    }
}

impl<T> From<std::sync::PoisonError<std::sync::MutexGuard<'_, T>>> for SkipListError {
    fn from(value: std::sync::PoisonError<std::sync::MutexGuard<'_, T>>) -> Self {
        SkipListError::MutexLockFailed(value.to_string())
    }
}

////////////////////////////

type NodeLink = Option<Rc<RefCell<Node>>>;

#[derive(Clone)]
struct Node {
    log_entry: LogEntry,
    levels: Vec<NodeLink>,
}

pub struct SkipList {
    head: Rc<RefCell<Node>>,
    probability: f64,
    num_levels: usize,
}

impl SkipList {
    pub fn new(probability: f64, expected_num_keys: i32, allowed_max_level: usize) -> Self {
        let base = 1.0 / probability;
        let mut num_levels = (expected_num_keys as f64).log(base).ceil() as usize;
        num_levels = allowed_max_level.min(num_levels);

        let head = Rc::new(RefCell::new(Node {
            log_entry: LogEntry {
                entry: Entry::Empty,
                log_seq_num: 0,
            },
            levels: vec![None; num_levels],
        }));

        Self {
            head,
            probability,
            num_levels,
        }
    }

    fn random_level(&self) -> usize {
        let mut lvl = 1;
        while lvl < self.num_levels && rand_bool(self.probability) {
            lvl += 1;
        }
        lvl
    }

    pub fn insert(&self, log_entry: LogEntry) {
        let key = match &log_entry.entry {
            Entry::Put { key, .. } => key,
            Entry::Del { key } => key,
            Entry::Empty => return,
        };

        let level = self.random_level();

        // Track nodes that need to be updated at each level
        let mut update: Vec<Rc<RefCell<Node>>> = vec![self.head.clone(); level];

        let mut current = self.head.clone();

        for i in (0..level).rev() {
            loop {
                let next_opt = current.borrow().levels[i].clone();
                match next_opt {
                    Some(ref next) => {
                        let next_borrow = next.borrow(); // extend borrow
                        let next_key = match &next_borrow.log_entry.entry {
                            Entry::Put { key, .. } => key,
                            Entry::Del { key } => key,
                            Entry::Empty => break,
                        };
                        match next_key.as_slice().cmp(&*key) {
                            Ordering::Less => {
                                drop(next_borrow);
                                current = next.clone();
                            }
                            Ordering::Equal => {
                                if log_entry.log_seq_num > next_borrow.log_entry.log_seq_num {
                                    break;
                                } else if log_entry.log_seq_num == next_borrow.log_entry.log_seq_num
                                {
                                    panic!(
                                        "trying to update a value in a previous log sequence number",
                                    );
                                } else {
                                    drop(next_borrow);
                                    current = next.clone();
                                }
                            }
                            Ordering::Greater => break,
                        }
                    }
                    None => break,
                }
            }
            update[i] = current.clone();
        }

        // Create new node
        let new_node = Rc::new(RefCell::new(Node {
            log_entry,
            levels: vec![None; level],
        }));

        // Insert node at each level
        for i in 0..level {
            let mut prev = update[i].borrow_mut();
            new_node.borrow_mut().levels[i] = prev.levels[i].take();
            prev.levels[i] = Some(new_node.clone());
        }
    }

    pub fn get(&self, key: &Vec<u8>, log_seq_num: u64) -> Option<LogEntry> {
        let mut current = self.head.clone();

        for i in (0..self.num_levels).rev() {
            loop {
                let next_opt = current.borrow().levels[i].clone();
                let next = match next_opt {
                    Some(n) => n,
                    None => break,
                };

                // Keep the borrow alive in a variable
                let next_borrow = next.borrow();

                let next_key = match &next_borrow.log_entry.entry {
                    Entry::Put { key, .. } => key,
                    Entry::Del { key } => key,
                    Entry::Empty => break,
                };

                match next_key.as_slice().cmp(&*key) {
                    std::cmp::Ordering::Less => {
                        drop(next_borrow); // release before moving
                        current = next.clone();
                    }
                    std::cmp::Ordering::Equal => {
                        if next_borrow.log_entry.log_seq_num <= log_seq_num {
                            return Some(next_borrow.log_entry.clone());
                        }
                        drop(next_borrow);
                        current = next.clone();
                    }
                    std::cmp::Ordering::Greater => break,
                }
            }
        }

        None
    }
}

// Helper to avoid rand crate
fn rand_bool(prob: f64) -> bool {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    let x = (nanos % 1000) as f64 / 1000.0;
    x < prob
}
