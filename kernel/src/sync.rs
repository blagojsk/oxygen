//! Kernel synchronisation primitives.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// A spin lock that masks interrupts for as long as it is held.
///
/// The masking is the entire point, and it was not always here. A plain spin lock taken by both a
/// thread and the interrupt that preempts it deadlocks outright: the handler spins for a lock the
/// code it interrupted is holding and will never get to release. That became reachable the moment
/// the console started delivering input from an interrupt and waking whoever was waiting for it,
/// because waking touches the scheduler's lock — the same one a running thread holds routinely.
///
/// On one core, masking is a complete fix rather than a mitigation: an interrupt that cannot
/// arrive cannot contend. On more than one it stops being sufficient, because another core can
/// hold the lock while this one masks — which is recorded in SPECS.md as part of what multi-core
/// support has to solve.
///
/// The previous interrupt state is saved and restored rather than unconditionally re-enabled, so
/// nesting works: an inner lock releasing does not unmask interrupts an outer one is relying on
/// staying masked.
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
        // Mask before acquiring, not after: taking the lock and then being interrupted leaves the
        // exact window this is meant to close.
        let interrupts = crate::arch::target::save_and_mask_irqs();
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
        SpinLockGuard {
            lock: self,
            interrupts,
        }
    }
}

pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
    /// DAIF as it was before the lock masked interrupts.
    interrupts: u64,
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
        // Unmask last. Restoring first would let an interrupt arrive while this still looks
        // locked, and the handler would spin waiting for a lock nobody is going to release.
        // SAFETY: the value came from this guard's own `save_and_mask_irqs`, and the critical
        // section is over.
        unsafe { crate::arch::target::restore_irqs(self.interrupts) };
    }
}
