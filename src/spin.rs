use core::sync::atomic::{Ordering, AtomicBool};

pub struct Lock {
    lock: AtomicBool,
}

pub struct LockGuard<'a> {
    lock: &'a AtomicBool,
}

impl Lock {
    pub const fn new() -> Self {
        Self {
            lock: AtomicBool::new(false),
        }
    }
}

impl Lock {
    fn obtain_lock(&self) {
        while self.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Acquire).is_err() {
            // Wait until the lock looks unlocked before retrying
            while self.lock.load(Ordering::Relaxed) {
                std::thread::yield_now();
            }
        }
    }

    pub fn lock(&self) -> LockGuard {
        self.obtain_lock();
        LockGuard {
            lock: &self.lock,
        }
    }
}

impl<'a> Drop for LockGuard<'a> {
    fn drop(&mut self) {
        self.lock.store(false, Ordering::Release);
    }
}
