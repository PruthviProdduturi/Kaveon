//! Atomic reservation helpers compatible with the minimum Rust toolchain.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

pub fn try_update_usize(
    value: &AtomicUsize,
    success: Ordering,
    failure: Ordering,
    mut update: impl FnMut(usize) -> Option<usize>,
) -> Result<usize, usize> {
    loop {
        let current = value.load(failure);
        let Some(next) = update(current) else {
            return Err(current);
        };
        match value.compare_exchange(current, next, success, failure) {
            Ok(previous) => return Ok(previous),
            Err(_) => continue,
        }
    }
}

pub fn try_update_u64(
    value: &AtomicU64,
    success: Ordering,
    failure: Ordering,
    mut update: impl FnMut(u64) -> Option<u64>,
) -> Result<u64, u64> {
    loop {
        let current = value.load(failure);
        let Some(next) = update(current) else {
            return Err(current);
        };
        match value.compare_exchange(current, next, success, failure) {
            Ok(previous) => return Ok(previous),
            Err(_) => continue,
        }
    }
}
