use std::collections::VecDeque;
use std::error::Error;
use std::fmt::Display;
use std::sync::{Arc, Condvar, Mutex, atomic};

use super::db_context::DBContext;
use super::memtable::Memtable;
use crate::core::db_config::DBConfig;

pub enum DBError {}

pub struct DB {
    memtable: Memtable,
    memory: MemoryManager,
    db_context: Arc<DBContext>,
}

impl DB {
    pub fn new(config: DBConfig) -> DB {
        let db_context = Arc::new(DBContext::new(config));
        DB {
            memtable: Memtable::new(db_context.clone()),
            memory: MemoryManager::new(db_context.clone()),
            db_context,
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

/////////////////////////////////////////

pub struct IteratorOptions {
    lower_bound: Vec<u8>,
    upper_bound: Vec<u8>,
}

pub struct Iterator {}

////////////////////////////////////////

#[derive(Debug)]
pub enum MemoryManagerError {
    MutexLockFailed(String),
}

impl Display for MemoryManagerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryManagerError::MutexLockFailed(e) => write!(f, "MutexLockFailed: {}", e),
        }
    }
}

impl Error for MemoryManagerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            MemoryManagerError::MutexLockFailed(_) => None,
        }
    }
}

impl<T> From<std::sync::PoisonError<std::sync::MutexGuard<'_, T>>> for MemoryManagerError {
    fn from(value: std::sync::PoisonError<std::sync::MutexGuard<'_, T>>) -> Self {
        MemoryManagerError::MutexLockFailed(value.to_string())
    }
}

pub struct MemoryManager {
    usage: atomic::AtomicUsize,
    max_usage: usize,
    waiters: Mutex<VecDeque<Waiter>>,

    db_context: Arc<DBContext>,
}

struct Waiter {
    amount: usize,
    condvar: Arc<Condvar>,
    woken: bool,
}

impl MemoryManager {
    pub fn new(db_context: Arc<DBContext>) -> Self {
        Self {
            usage: atomic::AtomicUsize::new(0),
            max_usage: db_context.config().memory_manager_max_memory_usage(),
            waiters: Mutex::new(VecDeque::new()),
            db_context,
        }
    }

    pub fn allocate(&self, amount: usize) -> Result<(), MemoryManagerError> {
        loop {
            let current = self.usage.load(atomic::Ordering::Relaxed);
            if current + amount <= self.max_usage {
                let old = self.usage.fetch_add(amount, atomic::Ordering::Relaxed);
                if old + amount <= self.max_usage {
                    return Ok(());
                }
                self.usage.fetch_sub(amount, atomic::Ordering::Relaxed);
            }

            let condvar = Arc::new(Condvar::new());
            let waiter = Waiter {
                amount,
                condvar: condvar.clone(),
                woken: false,
            };

            let mut queue = self.waiters.lock()?;
            queue.push_back(waiter);

            loop {
                let mut_queue = &mut queue;

                if let Some(front) = mut_queue.front() {
                    if Arc::ptr_eq(&front.condvar, &condvar) && front.woken {
                        // It's our turn, continue to allocation attempt
                        break;
                    }
                }

                queue = condvar.wait(queue).unwrap();
            }
        }
    }

    pub fn deallocate(&self, amount: usize) -> Result<(), MemoryManagerError> {
        self.usage.fetch_sub(amount, atomic::Ordering::Relaxed);

        let mut queue = self.waiters.lock()?;

        while let Some(waiter) = queue.front_mut() {
            let current = self.usage.load(atomic::Ordering::Relaxed);

            if current + waiter.amount > self.max_usage {
                break;
            }

            self.usage
                .fetch_add(waiter.amount, atomic::Ordering::Relaxed);

            waiter.woken = true;
            waiter.condvar.notify_one();

            queue.pop_front();
        }

        Ok(())
    }
}

/////////////////////////////////////////

#[derive(Debug)]
pub enum MemoryRecordError {
    MemoryManagerError(MemoryManagerError),
    RecordOutOfMemory,
}

impl Display for MemoryRecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryRecordError::MemoryManagerError(e) => write!(f, "MemoryManagerError: {}", e),
            MemoryRecordError::RecordOutOfMemory => write!(f, "RecordOutOfMemory"),
        }
    }
}

impl Error for MemoryRecordError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            MemoryRecordError::MemoryManagerError(e) => Some(e),
            MemoryRecordError::RecordOutOfMemory => None,
        }
    }
}

impl From<MemoryManagerError> for MemoryRecordError {
    fn from(value: MemoryManagerError) -> Self {
        MemoryRecordError::MemoryManagerError(value)
    }
}

pub struct MemoryRecord {
    primary_manager: Arc<MemoryManager>,
    usage: atomic::AtomicUsize,
    max_usage: usize,

    db_context: Arc<DBContext>,
}

impl MemoryRecord {
    pub fn new(
        db_context: Arc<DBContext>,
        primary_manager: Arc<MemoryManager>,
        max_usage: usize,
    ) -> MemoryRecord {
        MemoryRecord {
            primary_manager,
            usage: atomic::AtomicUsize::new(0),
            max_usage,
            db_context,
        }
    }

    pub fn allocate(&self, amount: usize) -> Result<(), MemoryRecordError> {
        let old = self.usage.fetch_add(amount, atomic::Ordering::Relaxed);
        if old + amount > self.max_usage {
            return Err(MemoryRecordError::RecordOutOfMemory);
        }
        self.primary_manager.allocate(amount)?;
        Ok(())
    }

    pub fn deallocate(&self, amount: usize) -> Result<(), MemoryRecordError> {
        self.usage.fetch_sub(amount, atomic::Ordering::Relaxed);
        self.primary_manager.deallocate(amount)?;
        Ok(())
    }

    pub fn release(&mut self) -> Result<(), MemoryRecordError> {
        let amount = self.usage.load(atomic::Ordering::Relaxed);
        self.primary_manager.deallocate(amount)?;
        Ok(())
    }
}

impl Drop for MemoryRecord {
    fn drop(&mut self) {
        let err = self.release();
        if let Err(err) = err {
            self.db_context.error(&err);
        }
    }
}

/////////////////////////////////////////
