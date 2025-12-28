use std::collections::VecDeque;
use std::error::Error;
use std::fmt::Display;
use std::sync::{Arc, Condvar, Mutex, atomic};

use super::db_context::DBContext;

pub struct MemoryManager {
    memory_pool: Arc<MemoryPool>,

    db_context: Arc<DBContext>,
}

impl MemoryManager {
    pub fn new(db_context: Arc<DBContext>) -> MemoryManager {
        MemoryManager {
            memory_pool: Arc::new(MemoryPool::new(db_context.clone())),
            db_context,
        }
    }

    pub fn new_record(&self, max_usage: usize, allow_first_allocation: bool) -> MemoryRecord {
        MemoryRecord::new(
            self.db_context.clone(),
            self.memory_pool.clone(),
            max_usage,
            allow_first_allocation,
        )
    }

    pub fn usage_ratio(&self) -> f32 {
        self.memory_pool.usage.load(atomic::Ordering::SeqCst) as f32
            / self.memory_pool.max_usage as f32
    }
}

////////////////////////////////////////

#[derive(Debug)]
pub enum MemoryPoolError {
    MutexLockFailed(String),
    DeallocationUnderflow,
}

impl Display for MemoryPoolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryPoolError::MutexLockFailed(e) => write!(f, "MutexLockFailed: {}", e),
            MemoryPoolError::DeallocationUnderflow => write!(f, "DeallocationUnderflow"),
        }
    }
}

impl Error for MemoryPoolError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            MemoryPoolError::MutexLockFailed(_) => None,
            MemoryPoolError::DeallocationUnderflow => None,
        }
    }
}

impl<T> From<std::sync::PoisonError<std::sync::MutexGuard<'_, T>>> for MemoryPoolError {
    fn from(value: std::sync::PoisonError<std::sync::MutexGuard<'_, T>>) -> Self {
        MemoryPoolError::MutexLockFailed(value.to_string())
    }
}

pub struct MemoryPool {
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

impl MemoryPool {
    pub fn new(db_context: Arc<DBContext>) -> Self {
        Self {
            usage: atomic::AtomicUsize::new(0),
            max_usage: db_context.config().memory_manager_max_memory_usage(),
            waiters: Mutex::new(VecDeque::new()),
            db_context,
        }
    }

    pub fn allocate(&self, amount: usize) -> Result<bool, MemoryPoolError> {
        self.db_context.log_info(format!(
            "[allocate] usage: {}, amount: {}",
            self.usage.load(atomic::Ordering::SeqCst),
            amount
        ));
        loop {
            let current = self.usage.load(atomic::Ordering::SeqCst);
            if current + amount <= self.max_usage {
                let old = self.usage.fetch_add(amount, atomic::Ordering::Relaxed);
                if old + amount <= self.max_usage {
                    return Ok(true);
                }
                self.db_context.log_info("subtracing".to_string());
                self.usage.fetch_sub(amount, atomic::Ordering::SeqCst);
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

    pub fn deallocate(&self, amount: usize) -> Result<(), MemoryPoolError> {
        self.db_context.log_info(format!(
            "[deallocate] usage: {}, amount: {}",
            self.usage.load(atomic::Ordering::SeqCst),
            amount
        ));

        let current = self.usage.load(atomic::Ordering::SeqCst);
        if current < amount {
            return Err(MemoryPoolError::DeallocationUnderflow);
        }
        self.usage.fetch_sub(amount, atomic::Ordering::SeqCst);

        let mut queue = self.waiters.lock()?;
        while let Some(waiter) = queue.front_mut() {
            let current = self.usage.load(atomic::Ordering::SeqCst);

            if current + waiter.amount > self.max_usage {
                break;
            }

            self.usage
                .fetch_add(waiter.amount, atomic::Ordering::SeqCst);

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
    MemoryPoolError(MemoryPoolError),
    RecordOutOfMemory,
}

impl Display for MemoryRecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryRecordError::MemoryPoolError(e) => write!(f, "MemoryPoolError: {}", e),
            MemoryRecordError::RecordOutOfMemory => write!(f, "RecordOutOfMemory"),
        }
    }
}

impl Error for MemoryRecordError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            MemoryRecordError::MemoryPoolError(e) => Some(e),
            MemoryRecordError::RecordOutOfMemory => None,
        }
    }
}

impl From<MemoryPoolError> for MemoryRecordError {
    fn from(value: MemoryPoolError) -> Self {
        MemoryRecordError::MemoryPoolError(value)
    }
}

pub struct MemoryRecord {
    primary_manager: Arc<MemoryPool>,
    usage: atomic::AtomicUsize,
    max_usage: usize,
    allow_first_allocation: bool,

    db_context: Arc<DBContext>,
}

impl MemoryRecord {
    pub fn new(
        db_context: Arc<DBContext>,
        primary_manager: Arc<MemoryPool>,
        max_usage: usize,
        allow_first_allocation: bool,
    ) -> MemoryRecord {
        MemoryRecord {
            primary_manager,
            usage: atomic::AtomicUsize::new(0),
            max_usage,
            allow_first_allocation,
            db_context,
        }
    }

    pub fn duplicate(&self) -> Result<Option<MemoryRecord>, MemoryRecordError> {
        let current = self.usage.load(atomic::Ordering::SeqCst);
        let allocated = self.primary_manager.allocate(current)?;
        if allocated {
            let record = MemoryRecord {
                primary_manager: self.primary_manager.clone(),
                usage: atomic::AtomicUsize::new(current),
                max_usage: self.max_usage,
                allow_first_allocation: self.allow_first_allocation,
                db_context: self.db_context.clone(),
            };
            Ok(Some(record))
        } else {
            Ok(None)
        }
    }

    pub fn allocate(&self, amount: usize) -> Result<bool, MemoryRecordError> {
        let current = self.usage.load(atomic::Ordering::SeqCst);
        if current + amount > self.max_usage && !(current == 0 && self.allow_first_allocation) {
            return Ok(false);
        }

        self.usage.fetch_add(amount, atomic::Ordering::SeqCst);
        let allocated = self.primary_manager.allocate(amount)?;
        Ok(allocated)
    }

    pub fn deallocate(&self, amount: usize) -> Result<(), MemoryRecordError> {
        self.usage.fetch_sub(amount, atomic::Ordering::SeqCst);
        self.primary_manager.deallocate(amount)?;
        Ok(())
    }

    pub fn release(&mut self) -> Result<(), MemoryRecordError> {
        let amount = self.usage.load(atomic::Ordering::SeqCst);
        self.primary_manager.deallocate(amount)?;
        Ok(())
    }

    pub fn size(&self) -> usize {
        self.usage.load(atomic::Ordering::SeqCst)
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
