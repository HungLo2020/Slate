//! Atomic byte reservations shared by bounded process input queues.
use std::sync::atomic::{AtomicUsize, Ordering};

pub(crate) fn reserve(counter: &AtomicUsize, bytes: usize, limit: usize) -> bool {
    let mut current = counter.load(Ordering::Acquire);
    loop {
        let Some(next) = current.checked_add(bytes).filter(|n| *n <= limit) else {
            return false;
        };
        match counter.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(actual) => current = actual,
        }
    }
}
