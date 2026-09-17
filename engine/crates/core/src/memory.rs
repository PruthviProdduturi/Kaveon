use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
};
use std::{any::Any, collections::HashMap};

use crate::process_memory::ProcessMemory;
use crate::{KaveonError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemorySnapshot {
    pub current_bytes: u64,
    pub peak_bytes: u64,
    pub limit_bytes: u64,
    pub reservation_calls: u64,
    pub reservation_bytes: u64,
}

#[derive(Debug)]
struct QueryMemoryInner {
    query_id: Arc<str>,
    limit_bytes: u64,
    current_bytes: AtomicU64,
    peak_bytes: AtomicU64,
    reservation_calls: AtomicU64,
    reservation_bytes: AtomicU64,
    _admission: Option<AdmissionLease>,
    /// The process guard, when the node runs under a memory limit: a
    /// reservation the process could not honour fails here, before the
    /// allocation, whatever the query budget still allows.
    process: Option<ProcessMemory>,
    resources: QueryResources,
    cancellation: CancellationProbe,
}

#[derive(Default)]
struct CancellationProbe(OnceLock<Arc<dyn Fn() -> bool + Send + Sync>>);

impl std::fmt::Debug for CancellationProbe {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CancellationProbe")
            .field("installed", &self.0.get().is_some())
            .finish()
    }
}

#[derive(Default)]
struct QueryResources(Mutex<HashMap<&'static str, Arc<dyn Any + Send + Sync>>>);

impl std::fmt::Debug for QueryResources {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QueryResources")
            .finish_non_exhaustive()
    }
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
    process: Option<ProcessMemory>,
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
            process: None,
        })
    }

    /// Every admitted query's reservations also answer to the process
    /// guard: what the process really holds against the limit it really
    /// has.
    #[must_use]
    pub fn with_process_memory(mut self, process: ProcessMemory) -> Self {
        self.process = Some(process);
        self
    }

    pub fn process_memory(&self) -> Option<&ProcessMemory> {
        self.process.as_ref()
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
        let mut pool = QueryMemoryPool::new(query_id, query_limit_bytes)?;
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
                    let inner =
                        Arc::get_mut(&mut pool.inner).expect("new query pool is uniquely owned");
                    inner._admission = Some(AdmissionLease {
                        controller: self.clone(),
                        admitted_bytes: query_limit_bytes,
                    });
                    inner.process = self.process.clone();
                    return Ok(AdmittedQueryMemory { pool });
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
            reservation_calls: 0,
            reservation_bytes: 0,
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
    pool: QueryMemoryPool,
}

// Held by the shared pool, including operator accounts and live reservations.
// Canceling/dropping a request must not release admission while its worker still
// owns query memory and has not observed cancellation.
#[derive(Debug)]
struct AdmissionLease {
    controller: MemoryAdmissionController,
    admitted_bytes: u64,
}

impl AdmittedQueryMemory {
    #[must_use]
    pub fn pool(&self) -> &QueryMemoryPool {
        &self.pool
    }
}

impl Drop for AdmissionLease {
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
                reservation_calls: AtomicU64::new(0),
                reservation_bytes: AtomicU64::new(0),
                _admission: None,
                process: None,
                resources: QueryResources::default(),
                cancellation: CancellationProbe::default(),
            }),
        })
    }

    /// A pool whose reservations also answer to `process`.
    #[must_use]
    pub fn with_process_memory(mut self, process: ProcessMemory) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.process = Some(process);
        }
        self
    }

    #[must_use]
    pub fn query_id(&self) -> &str {
        &self.inner.query_id
    }

    /// Connects synchronous operator loops to an external cancellation token.
    /// The callback must be fast, nonblocking, and must not retain this pool.
    pub fn set_cancellation_probe(
        &self,
        probe: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<()> {
        self.inner.cancellation.0.set(Arc::new(probe)).map_err(|_| {
            KaveonError::Execution("query cancellation probe already installed".into())
        })
    }

    pub fn check_cancelled(&self) -> Result<()> {
        if self.inner.cancellation.0.get().is_some_and(|probe| probe()) {
            return Err(KaveonError::Execution("query canceled".into()));
        }
        Ok(())
    }

    #[must_use]
    pub fn snapshot(&self) -> MemorySnapshot {
        MemorySnapshot {
            current_bytes: self.inner.current_bytes.load(Ordering::Acquire),
            peak_bytes: self.inner.peak_bytes.load(Ordering::Acquire),
            limit_bytes: self.inner.limit_bytes,
            reservation_calls: self.inner.reservation_calls.load(Ordering::Acquire),
            reservation_bytes: self.inner.reservation_bytes.load(Ordering::Acquire),
        }
    }

    /// Shares a typed, query-lifetime resource (for example a spill quota) among
    /// operators. Resource initializers must not recursively access this pool.
    /// Resources must not retain the pool itself, which would create a cycle.
    pub fn shared_resource<T: Any + Send + Sync>(
        &self,
        key: &'static str,
        initialize: impl FnOnce() -> Result<T>,
    ) -> Result<Arc<T>> {
        let mut resources =
            self.inner.resources.0.lock().map_err(|_| {
                KaveonError::Execution("query resource registry lock poisoned".into())
            })?;
        if let Some(resource) = resources.get(key) {
            return Arc::clone(resource).downcast::<T>().map_err(|_| {
                KaveonError::Execution(format!("query resource '{key}' has an incompatible type"))
            });
        }
        let resource = Arc::new(initialize()?);
        resources.insert(key, resource.clone());
        Ok(resource)
    }

    /// A shared resource already registered under `key`, if any.
    pub fn shared_resource_if_present<T: Any + Send + Sync>(
        &self,
        key: &'static str,
    ) -> Result<Option<Arc<T>>> {
        let resources =
            self.inner.resources.0.lock().map_err(|_| {
                KaveonError::Execution("query resource registry lock poisoned".into())
            })?;
        resources
            .get(key)
            .map(|resource| {
                Arc::clone(resource).downcast::<T>().map_err(|_| {
                    KaveonError::Execution(format!(
                        "query resource '{key}' has an incompatible type"
                    ))
                })
            })
            .transpose()
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
        self.check_cancelled()?;
        if let Some(process) = &self.inner.process
            && !process.can_reserve(bytes)
        {
            return Err(KaveonError::MemoryLimit(format!(
                "query '{}' operator '{}' cannot reserve {bytes} bytes: the process holds {} of {} bytes with {} bytes kept free",
                self.query_id(),
                operator_id,
                process.allocated_bytes(),
                process.limit_bytes(),
                process.headroom_bytes(),
            )));
        }
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
        KaveonError::MemoryLimit(format!(
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
    pub fn check_cancelled(&self) -> Result<()> {
        self.query.check_cancelled()
    }
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
            reservation_calls: self.query.inner.reservation_calls.load(Ordering::Acquire),
            reservation_bytes: self.query.inner.reservation_bytes.load(Ordering::Acquire),
        }
    }

    pub fn reserve(&self, bytes: u64) -> Result<MemoryReservation> {
        self.query.try_reserve(bytes, self.operator_id())?;
        self.query
            .inner
            .reservation_calls
            .fetch_add(1, Ordering::Relaxed);
        self.query
            .inner
            .reservation_bytes
            .fetch_add(bytes, Ordering::Relaxed);
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

/// Reservations in slabs: one pool round trip per 64 KiB of admitted
/// entries, not one per entry. Operators that admit millions of small
/// items (a distinct key, a build-side key) reserve through this; one
/// `MemoryReservation` per item — an Arc clone and an atomic on the shared
/// pool each — cost more than the item and held tens of millions of guards.
#[derive(Debug, Default)]
pub struct ReservationSlab {
    guards: Vec<MemoryReservation>,
    available: u64,
}

const RESERVATION_SLAB_BYTES: u64 = 64 * 1024;

impl ReservationSlab {
    /// Charge `bytes`, taking a new slab from `memory` when the current one
    /// cannot cover it; a slab that the budget refuses falls back to the
    /// exact amount before failing.
    pub fn reserve(&mut self, memory: &OperatorMemoryAccount, bytes: u64) -> Result<()> {
        if bytes > self.available {
            let slab_bytes = RESERVATION_SLAB_BYTES.max(bytes);
            let guard = match memory.reserve(slab_bytes) {
                Ok(guard) => guard,
                Err(KaveonError::MemoryLimit(_)) if slab_bytes != bytes => memory.reserve(bytes)?,
                Err(error) => return Err(error),
            };
            self.available = self.available.saturating_add(guard.bytes());
            self.guards.push(guard);
        }
        self.available -= bytes;
        Ok(())
    }

    /// Bytes charged through this slab that are still held.
    pub fn charged_bytes(&self) -> u64 {
        self.guards
            .iter()
            .map(MemoryReservation::bytes)
            .sum::<u64>()
            .saturating_sub(self.available)
    }

    /// Release every slab.
    pub fn clear(&mut self) {
        self.guards.clear();
        self.available = 0;
    }

    pub fn into_guards(self) -> Vec<MemoryReservation> {
        self.guards
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
    fn admission_lives_until_last_worker_reservation_is_released() {
        let admission = MemoryAdmissionController::new(1024).unwrap();
        let admitted = admission.admit("canceling", 1024).unwrap();
        let account = admitted.pool().operator("worker").unwrap();
        let reservation = account.reserve(64).unwrap();
        drop(admitted);
        drop(account);
        assert_eq!(admission.snapshot().current_bytes, 1024);
        assert!(admission.admit("premature", 1024).is_err());
        drop(reservation);
        assert_eq!(admission.snapshot().current_bytes, 0);
        assert!(admission.admit("next", 1024).is_ok());
    }

    #[test]
    fn query_resources_initialize_once_and_reject_type_collisions() {
        let pool = QueryMemoryPool::new("resources", 1024).unwrap();
        let first = pool
            .shared_resource("disk", || Ok(AtomicU64::new(7)))
            .unwrap();
        let clone = pool.clone();
        let second = clone
            .shared_resource::<AtomicU64>("disk", || panic!("already initialized"))
            .unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(second.load(Ordering::Relaxed), 7);
        assert!(pool.shared_resource("disk", || Ok(String::new())).is_err());
        assert!(
            pool.shared_resource::<u64>("retry", || Err(KaveonError::Execution("failed".into())))
                .is_err()
        );
        assert_eq!(*pool.shared_resource("retry", || Ok(9_u64)).unwrap(), 9);
    }

    #[test]
    fn cancellation_probe_stops_new_reservations_without_leaking_existing_ones() {
        let pool = QueryMemoryPool::new("cancel", 1024).unwrap();
        let canceled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let signal = canceled.clone();
        pool.set_cancellation_probe(move || signal.load(Ordering::Acquire))
            .unwrap();
        let account = pool.operator("operator").unwrap();
        let reservation = account.reserve(64).unwrap();
        canceled.store(true, Ordering::Release);
        assert!(account.check_cancelled().is_err());
        assert!(account.reserve(0).is_err());
        assert_eq!(pool.snapshot().current_bytes, 64);
        drop(reservation);
        assert_eq!(pool.snapshot().current_bytes, 0);
        assert!(pool.set_cancellation_probe(|| false).is_err());
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
            assert_eq!(pool.snapshot().reservation_calls, 2);
            assert_eq!(pool.snapshot().reservation_bytes, 384);
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

        let error = sort.reserve(512).unwrap_err();
        assert!(matches!(&error, KaveonError::MemoryLimit(_)));
        assert!(error.to_string().contains("query 'query' operator 'sort'"));
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
    fn reservations_answer_to_the_process_guard_before_the_query_budget() {
        // The query budget would allow it; the process cannot hold it.
        let live = Arc::new(AtomicU64::new(0));
        let probe = live.clone();
        let process = ProcessMemory::with_probe(1_000, 100, move || probe.load(Ordering::Relaxed));
        let admission = MemoryAdmissionController::new(4_096)
            .unwrap()
            .with_process_memory(process);
        let admitted = admission.admit("guarded", 4_096).unwrap();
        let account = admitted.pool().operator("scan").unwrap();
        let held = account.reserve(900).unwrap();
        live.store(950, Ordering::Relaxed);
        let error = account.reserve(1).unwrap_err().to_string();
        assert!(error.contains("the process holds 950 of 1000 bytes"));
        assert!(error.contains("100 bytes kept free"));
        live.store(0, Ordering::Relaxed);
        let more = account.reserve(1).unwrap();
        assert_eq!(admitted.pool().snapshot().current_bytes, 901);
        drop(more);
        drop(held);
        assert_eq!(admitted.pool().snapshot().current_bytes, 0);
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
