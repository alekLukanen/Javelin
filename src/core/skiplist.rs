use std::{cell::RefCell, cmp::Ordering, error::Error, fmt::Display, rc::Rc, sync::Arc};

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
    log_entry: Arc<LogEntry>,
    levels: Vec<NodeLink>,
}

#[derive(Clone)]
pub struct SkipList {
    head: Rc<RefCell<Node>>,
    probability: f64,
    num_levels: usize,
}

impl SkipList {
    pub fn new(probability: f64, expected_num_keys: u32, allowed_max_level: u32) -> Self {
        let base = 1.0 / probability;
        let mut num_levels = (expected_num_keys as f64).log(base).ceil() as usize;
        num_levels = (allowed_max_level as usize).min(num_levels);

        let head = Rc::new(RefCell::new(Node {
            log_entry: Arc::new(LogEntry {
                entry: Entry::Empty,
                log_seq_num: u64::MAX,
            }),
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
        while lvl < self.num_levels && fastrand::f64() < self.probability {
            lvl += 1;
        }
        lvl
    }

    #[inline]
    fn user_key(entry: &Entry) -> &[u8] {
        match entry {
            Entry::Put { key, .. } => key,
            Entry::Del { key } => key,
            Entry::Empty => unreachable!(),
        }
    }

    // Used only at level 0
    #[inline]
    fn cmp_full(a: &LogEntry, b: &LogEntry) -> Ordering {
        match Self::user_key(&a.entry).cmp(Self::user_key(&b.entry)) {
            Ordering::Equal => b.log_seq_num.cmp(&a.log_seq_num), // DESC
            other => other,
        }
    }

    pub fn insert(&self, log_entry: Arc<LogEntry>) {
        if matches!(log_entry.entry, Entry::Empty) {
            return;
        }

        let mut update = vec![self.head.clone(); self.num_levels];
        let mut current = self.head.clone();

        // Traverse top-down
        for level in (0..self.num_levels).rev() {
            loop {
                let next = {
                    let cur = current.borrow();
                    if level < cur.levels.len() {
                        cur.levels[level].clone()
                    } else {
                        None
                    }
                };

                let Some(next_rc) = next else { break };
                let next_ref = next_rc.borrow();

                let cmp = if level == 0 {
                    Self::cmp_full(&next_ref.log_entry, &log_entry)
                } else {
                    Self::user_key(&next_ref.log_entry.entry).cmp(Self::user_key(&log_entry.entry))
                };

                if cmp == Ordering::Less {
                    drop(next_ref);
                    current = next_rc;
                } else {
                    break;
                }
            }
            update[level] = current.clone();
        }

        let node_level = self.random_level();
        let new_node = Rc::new(RefCell::new(Node {
            log_entry,
            levels: vec![None; node_level],
        }));

        // Splice
        for level in 0..node_level {
            let mut prev = update[level].borrow_mut();
            let next = prev.levels[level].take();
            new_node.borrow_mut().levels[level] = next;
            prev.levels[level] = Some(new_node.clone());
        }
    }

    pub fn get(&self, key: &[u8], snapshot_seq: u64) -> Option<Arc<LogEntry>> {
        let mut current = self.head.clone();

        // Only user-key traversal above level 0
        for level in (1..self.num_levels).rev() {
            loop {
                let next = {
                    let cur = current.borrow();
                    if level < cur.levels.len() {
                        cur.levels[level].clone()
                    } else {
                        None
                    }
                };

                let Some(next_rc) = next else { break };
                let next_ref = next_rc.borrow();

                match Self::user_key(&next_ref.log_entry.entry).cmp(key) {
                    Ordering::Less => {
                        drop(next_ref);
                        current = next_rc;
                    }
                    _ => break,
                }
            }
        }

        // Level 0: full ordering
        loop {
            let next = {
                let cur = current.borrow();
                cur.levels[0].clone()
            };

            let Some(next_rc) = next else { return None };
            let next_ref = next_rc.borrow();

            match Self::user_key(&next_ref.log_entry.entry).cmp(key) {
                Ordering::Less => {
                    drop(next_ref);
                    current = next_rc;
                }
                Ordering::Equal => {
                    if next_ref.log_entry.log_seq_num <= snapshot_seq {
                        return Some(next_ref.log_entry.clone());
                    }
                    drop(next_ref);
                    current = next_rc;
                }
                Ordering::Greater => return None,
            }
        }
    }
    pub fn iter(&self) -> SkipListIter {
        // First real node = head.levels[0]
        let first = self.head.borrow().levels.get(0).cloned().flatten();

        SkipListIter { current: first }
    }
}

impl Display for SkipList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut res = String::new();

        let mut current = self.head.clone();
        loop {
            let next_opt = current.borrow().levels[0].clone();
            let next = match next_opt {
                Some(n) => n,
                None => break,
            };

            // Keep the borrow alive in a variable
            let next_borrow = next.borrow();
            res = format!("{}: {:?}", res, next_borrow.log_entry.entry);
            drop(next_borrow);
            current = next;
        }

        write!(f, "{}", res)
    }
}

/////////////////////////////////////////////////////

pub struct SkipListIter {
    current: Option<Rc<RefCell<Node>>>,
}

impl Iterator for SkipListIter {
    type Item = Arc<LogEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        // Move to next node
        let curr_rc = self.current.take()?;
        let curr = curr_rc.borrow();

        // Extract current log_entry unless it's the head's dummy entry
        let entry = match &curr.log_entry.entry {
            Entry::Empty => None,
            _ => Some(curr.log_entry.clone()),
        };

        // Advance to next (level 0 always exists)
        self.current = curr.levels.get(0).cloned().flatten();

        entry
    }
}
