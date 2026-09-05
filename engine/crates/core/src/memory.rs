use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use crate::{KaveonError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemorySnapshot {
    pub current_bytes: u64,
    pub peak_bytes: u64,
    pub limit_bytes: u64,
}

#[derive(Debug)]
struct QueryMemoryInner {
    query_id: Arc<str>,
    limit_bytes: u64,
    current_bytes: AtomicU64,
    peak_bytes: AtomicU64,
}

/// A thread-safe hard memory limit shared by every operator in one query.
#[derive(Debug, Clone)]
pub struct QueryMemoryPool {
    inner: Arc<QueryMemoryInner>,
}

#[derive(Debug)]
struct AdmissionInner {
    limit_bytes: u64,
    admitted_bytes: AtomicU64,
    peak_admitted_bytes: AtomicU64,
}

/// Reserves query memory budgets before execution begins.
#[derive(Debug, Clone)]
pub struct MemoryAdmissionController {
    inner: Arc<AdmissionInner>,
}

impl MemoryAdmissionController {
    pub fn new(limit_bytes: u64) -> Result<Self> {
        if limit_bytes == 0 {
            return Err(KaveonError::Execution(
                "memory admission limit must be greater than zero".into(),
            ));
        }
        Ok(Self {
            inner: Arc::new(AdmissionInner {
                limit_bytes,
                admitted_bytes: AtomicU64::new(0),
                peak_admitted_bytes: AtomicU64::new(0),
            }),
        })
    }

    pub fn admit(
        &self,
        query_id: impl Into<String>,
        query_limit_bytes: u64,
    ) -> Result<AdmittedQueryMemory> {
        if query_limit_bytes == 0 || query_limit_bytes > self.inner.limit_bytes {
            return Err(KaveonError::Execution(format!(
                "query memory limit {query_limit_bytes} must be between 1 and the admission limit of {} bytes",
                self.inner.limit_bytes
            )));
        }
        let pool = QueryMemoryPool::new(query_id, query_limit_bytes)?;
        let mut current = self.inner.admitted_bytes.load(Ordering::Acquire);
        loop {
            let Some(next) = current.checked_add(query_limit_bytes) else {
                return Err(self.capacity_error(query_limit_bytes, current));
            };
            if next > self.inner.limit_bytes {
                return Err(self.capacity_error(query_limit_bytes, current));
            }
            match self.inner.admitted_bytes.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.inner
                        .peak_admitted_bytes
                        .fetch_max(next, Ordering::AcqRel);
                    return Ok(AdmittedQueryMemory {
                        controller: self.clone(),
                        pool,
                        admitted_bytes: query_limit_bytes,
                    });
                }
                Err(observed) => current = observed,
            }
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> MemorySnapshot {
        MemorySnapshot {
            current_bytes: self.inner.admitted_bytes.load(Ordering::Acquire),
            peak_bytes: self.inner.peak_admitted_bytes.load(Ordering::Acquire),
            limit_bytes: self.inner.limit_bytes,
        }
    }

    fn capacity_error(&self, requested: u64, current: u64) -> KaveonError {
        KaveonError::Execution(format!(
            "memory admission rejected query budget of {requested} bytes: {current} of {} bytes already admitted",
            self.inner.limit_bytes
        ))
    }

    fn release(&self, bytes: u64) {
        let previous = self.inner.admitted_bytes.fetch_sub(bytes, Ordering::AcqRel);
        debug_assert!(previous >= bytes, "memory admission accounting underflow");
    }
}

/// An admitted query budget that releases cluster capacity through RAII.
#[derive(Debug)]
pub struct AdmittedQueryMemory {
    controller: MemoryAdmissionController,
    pool: QueryMemoryPool,
    admitted_bytes: u64,
}

impl AdmittedQueryMemory {
    #[must_use]
    pub fn pool(&self) -> &QueryMemoryPool {
        &self.pool
    }
}

impl Drop for AdmittedQueryMemory {
    fn drop(&mut self) {
        self.controller.release(self.admitted_bytes);
    }
}

impl QueryMemoryPool {
    pub fn new(query_id: impl Into<String>, limit_bytes: u64) -> Result<Self> {
        let query_id = query_id.into();
        if query_id.trim().is_empty() {
            return Err(KaveonError::Execution(
                "query memory pool requires a non-empty query ID".into(),
            ));
        }
        if limit_bytes == 0 {
            return Err(KaveonError::Execution(
                "query memory limit must be greater than zero".into(),
            ));
        }

        Ok(Self {
            inner: Arc::new(QueryMemoryInner {
                query_id: query_id.into(),
                limit_bytes,
                current_bytes: AtomicU64::new(0),
                peak_bytes: AtomicU64::new(0),
            }),
        })
    }

    #[must_use]
    pub fn query_id(&self) -> &str {
        &self.inner.query_id
    }

    #[must_use]
    pub fn snapshot(&self) -> MemorySnapshot {
        MemorySnapshot {
            current_bytes: self.inner.current_bytes.load(Ordering::Acquire),
            peak_bytes: self.inner.peak_bytes.load(Ordering::Acquire),
            limit_bytes: self.inner.limit_bytes,
        }
    }

    pub fn operator(&self, operator_id: impl Into<String>) -> Result<OperatorMemoryAccount> {
        let operator_id = operator_id.into();
        if operator_id.trim().is_empty() {
            return Err(KaveonError::Execution(
                "operator memory account requires a non-empty operator ID".into(),
            ));
        }

        Ok(OperatorMemoryAccount {
            query: self.clone(),
            operator_id: operator_id.into(),
            current_bytes: Arc::new(AtomicU64::new(0)),
            peak_bytes: Arc::new(AtomicU64::new(0)),
        })
    }

    fn try_reserve(&self, bytes: u64, operator_id: &str) -> Result<()> {
        let mut current = self.inner.current_bytes.load(Ordering::Acquire);
        loop {
            let Some(next) = current.checked_add(bytes) else {
                return Err(self.limit_error(operator_id, bytes, current));
            };
            if next > self.inner.limit_bytes {
                return Err(self.limit_error(operator_id, bytes, current));
            }

            match self.inner.current_bytes.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.inner.peak_bytes.fetch_max(next, Ordering::AcqRel);
                    return Ok(());
                }
                Err(observed) => current = observed,
            }
        }
    }

    fn limit_error(&self, operator_id: &str, requested: u64, current: u64) -> KaveonError {
        KaveonError::Execution(format!(
            "query '{}' operator '{}' cannot reserve {requested} bytes: {current} of {} bytes already reserved",
            self.query_id(),
            operator_id,
            self.inner.limit_bytes
        ))
    }

    fn release(&self, bytes: u64) {
        let previous = self.inner.current_bytes.fetch_sub(bytes, Ordering::AcqRel);
        debug_assert!(previous >= bytes, "query memory accounting underflow");
    }
}

/// Per-operator accounting backed by a query-wide hard limit.
#[derive(Debug, Clone)]
pub struct OperatorMemoryAccount {
    query: QueryMemoryPool,
    operator_id: Arc<str>,
    current_bytes: Arc<AtomicU64>,
    peak_bytes: Arc<AtomicU64>,
}

impl OperatorMemoryAccount {
    #[must_use]
    pub fn operator_id(&self) -> &str {
        &self.operator_id
    }

    #[must_use]
    pub fn query(&self) -> &QueryMemoryPool {
        &self.query
    }

    #[must_use]
    pub fn snapshot(&self) -> MemorySnapshot {
        MemorySnapshot {
            current_bytes: self.current_bytes.load(Ordering::Acquire),
            peak_bytes: self.peak_bytes.load(Ordering::Acquire),
            limit_bytes: self.query.inner.limit_bytes,
        }
    }

    pub fn reserve(&self, bytes: u64) -> Result<MemoryReservation> {
        self.query.try_reserve(bytes, self.operator_id())?;
        let current = self.current_bytes.fetch_add(bytes, Ordering::AcqRel) + bytes;
        self.peak_bytes.fetch_max(current, Ordering::AcqRel);

        Ok(MemoryReservation {
            account: self.clone(),
            bytes,
        })
    }

    fn release(&self, bytes: u64) {
        let previous = self.current_bytes.fetch_sub(bytes, Ordering::AcqRel);
        debug_assert!(previous >= bytes, "operator memory accounting underflow");
        self.query.release(bytes);
    }
}

/// An owned reservation that returns its bytes when dropped.
#[derive(Debug)]
#[must_use = "dropping the reservation immediately releases the reserved memory"]
pub struct MemoryReservation {
    account: OperatorMemoryAccount,
    bytes: u64,
}

impl MemoryReservation {
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn release(self) {
        drop(self);
    }
}

impl Drop for MemoryReservation {
    fn drop(&mut self) {
        self.account.release(self.bytes);
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, thread};

    use super::*;

    const QUERY_LIMIT: u64 = 1_024;

    #[test]
    fn validates_pool_and_operator_identity() {
        assert!(QueryMemoryPool::new("", QUERY_LIMIT).is_err());
        assert!(QueryMemoryPool::new("query", 0).is_err());

        let pool = QueryMemoryPool::new("query", QUERY_LIMIT).unwrap();
        assert!(pool.operator(" ").is_err());
    }

    #[test]
    fn reservations_track_current_and_peak_then_release_on_drop() {
        let pool = QueryMemoryPool::new("query", QUERY_LIMIT).unwrap();
        let account = pool.operator("hash-aggregate").unwrap();

        let first = account.reserve(128).unwrap();
        {
            let _second = account.reserve(256).unwrap();
            assert_eq!(account.snapshot().current_bytes, 384);
            assert_eq!(pool.snapshot().peak_bytes, 384);
        }
        assert_eq!(account.snapshot().current_bytes, 128);
        first.release();

        assert_eq!(account.snapshot().current_bytes, 0);
        assert_eq!(account.snapshot().peak_bytes, 384);
        assert_eq!(pool.snapshot().current_bytes, 0);
        assert_eq!(pool.snapshot().peak_bytes, 384);
    }

    #[test]
    fn query_limit_is_shared_across_operators() {
        let pool = QueryMemoryPool::new("query", QUERY_LIMIT).unwrap();
        let join = pool.operator("join").unwrap();
        let sort = pool.operator("sort").unwrap();
        let _join_memory = join.reserve(768).unwrap();

        let error = sort.reserve(512).unwrap_err().to_string();
        assert!(error.contains("query 'query' operator 'sort'"));
        assert_eq!(pool.snapshot().current_bytes, 768);
        assert_eq!(sort.snapshot().current_bytes, 0);
    }

    #[test]
    fn concurrent_reservations_never_exceed_the_hard_limit() {
        const THREAD_COUNT: usize = 16;
        const RESERVATION_BYTES: u64 = 128;
        const CONCURRENT_LIMIT: u64 = 512;

        let pool = Arc::new(QueryMemoryPool::new("parallel", CONCURRENT_LIMIT).unwrap());
        let handles = (0..THREAD_COUNT)
            .map(|index| {
                let pool = Arc::clone(&pool);
                thread::spawn(move || {
                    let account = pool.operator(format!("operator-{index}")).unwrap();
                    account.reserve(RESERVATION_BYTES).ok()
                })
            })
            .collect::<Vec<_>>();

        let reservations = handles
            .into_iter()
            .filter_map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(reservations.len(), 4);
        assert_eq!(pool.snapshot().current_bytes, CONCURRENT_LIMIT);
        assert_eq!(pool.snapshot().peak_bytes, CONCURRENT_LIMIT);
        drop(reservations);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn zero_byte_reservation_is_balanced() {
        let pool = QueryMemoryPool::new("query", QUERY_LIMIT).unwrap();
        let account = pool.operator("projection").unwrap();
        let reservation = account.reserve(0).unwrap();

        assert_eq!(reservation.bytes(), 0);
        drop(reservation);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn admission_rejects_oversubscription_and_releases_capacity() {
        let admission = MemoryAdmissionController::new(1_024).unwrap();
        let first = admission.admit("first", 768).unwrap();
        let error = admission.admit("second", 512).unwrap_err().to_string();
        assert!(error.contains("memory admission rejected"));
        assert_eq!(admission.snapshot().current_bytes, 768);

        drop(first);
        let second = admission.admit("second", 512).unwrap();
        assert_eq!(second.pool().snapshot().limit_bytes, 512);
        assert_eq!(admission.snapshot().peak_bytes, 768);
        drop(second);
        assert_eq!(admission.snapshot().current_bytes, 0);
    }

    #[test]
    fn admission_validates_global_and_per_query_limits() {
        assert!(MemoryAdmissionController::new(0).is_err());
        let admission = MemoryAdmissionController::new(1_024).unwrap();
        assert!(admission.admit("query", 0).is_err());
        assert!(admission.admit("query", 2_048).is_err());
        assert!(admission.admit(" ", 128).is_err());
        assert_eq!(admission.snapshot().current_bytes, 0);
    }
}
