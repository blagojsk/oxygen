//! Kernel synchronisation primitives.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// A spin lock.
///
/// Spinning rather than blocking because there is nothing to block on yet — no scheduler, no way
/// to yield. That is acceptable while the kernel is single-core and the critical sections are a
/// handful of instructions, and it stops being acceptable the moment either changes: a spin lock
/// held across anything slow wastes a whole core, and one taken in both a thread and the interrupt
/// that preempts it deadlocks outright. Neither situation exists yet; both are coming.
pub struct SpinLock<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: the lock serialises access, so a `T` that can move between threads can be shared.
unsafe impl<T: Send> Sync for SpinLock<T> {}
// SAFETY: ownership of the inner value moves with the lock.
unsafe impl<T: Send> Send for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> Self {
        SpinLock {
            locked: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            // Re-read without writing while waiting: hammering the cache line with atomic
            // read-modify-writes starves the holder trying to release it.
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
        SpinLockGuard { lock: self }
    }
}

pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
}

impl<T> Deref for SpinLockGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: holding the lock grants exclusive access for the guard's lifetime.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as above, and `&mut self` proves this guard is not aliased.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        // Release ordering publishes everything written under the lock to the next holder.
        self.lock.locked.store(false, Ordering::Release);
    }
}
