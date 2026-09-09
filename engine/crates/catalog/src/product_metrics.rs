//! Bounded, payload-free transaction telemetry. Unknown commit outcomes are
//! deliberately distinct from rollbacks: a timed-out CAS may have succeeded.
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

const BUCKETS_US: [u64; 7] = [1_000, 5_000, 10_000, 50_000, 100_000, 1_000_000, u64::MAX];

#[derive(Clone, Copy, Debug)]
pub enum TransactionOutcome {
    Committed,
    Replayed,
    Conflict,
    Rejected,
    Indeterminate,
    Abandoned,
}

#[derive(Default)]
pub struct TransactionMetrics {
    attempts: AtomicU64,
    in_flight: AtomicU64,
    outcomes: [AtomicU64; 6],
    latency_us: AtomicU64,
    latency_buckets: [AtomicU64; 7],
}

#[derive(Debug, Serialize)]
pub struct TransactionMetricsSnapshot {
    pub attempts: u64,
    pub in_flight: u64,
    pub committed: u64,
    pub replayed: u64,
    pub conflicts: u64,
    pub rejected: u64,
    pub indeterminate: u64,
    pub abandoned: u64,
    pub latency_us_total: u64,
    /// Noncumulative buckets with the corresponding inclusive upper bounds.
    pub latency_bucket_counts: [u64; 7],
    pub latency_bucket_upper_bounds_us: [u64; 7],
}

impl TransactionMetrics {
    pub fn begin(&self) -> TransactionAttempt<'_> {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        self.in_flight.fetch_add(1, Ordering::Relaxed);
        TransactionAttempt {
            metrics: self,
            started: Instant::now(),
            finished: false,
        }
    }

    /// A concurrent scrape is approximate; individual counters are atomic.
    pub fn snapshot(&self) -> TransactionMetricsSnapshot {
        let outcomes = self.outcomes.each_ref().map(|v| v.load(Ordering::Relaxed));
        TransactionMetricsSnapshot {
            attempts: self.attempts.load(Ordering::Relaxed),
            in_flight: self.in_flight.load(Ordering::Relaxed),
            committed: outcomes[0],
            replayed: outcomes[1],
            conflicts: outcomes[2],
            rejected: outcomes[3],
            indeterminate: outcomes[4],
            abandoned: outcomes[5],
            latency_us_total: self.latency_us.load(Ordering::Relaxed),
            latency_bucket_counts: self
                .latency_buckets
                .each_ref()
                .map(|v| v.load(Ordering::Relaxed)),
            latency_bucket_upper_bounds_us: BUCKETS_US,
        }
    }

    fn finish(&self, outcome: TransactionOutcome, started: Instant) {
        self.outcomes[outcome as usize].fetch_add(1, Ordering::Relaxed);
        let elapsed = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        self.latency_us.fetch_add(elapsed, Ordering::Relaxed);
        let bucket = BUCKETS_US
            .iter()
            .position(|limit| elapsed <= *limit)
            .unwrap_or(6);
        self.latency_buckets[bucket].fetch_add(1, Ordering::Relaxed);
        self.in_flight.fetch_sub(1, Ordering::Relaxed);
    }
}

#[must_use = "an attempt should be finished with its observed outcome"]
pub struct TransactionAttempt<'a> {
    metrics: &'a TransactionMetrics,
    started: Instant,
    finished: bool,
}

impl TransactionAttempt<'_> {
    pub fn finish(mut self, outcome: TransactionOutcome) {
        self.metrics.finish(outcome, self.started);
        self.finished = true;
    }
}

impl Drop for TransactionAttempt<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.metrics
                .finish(TransactionOutcome::Abandoned, self.started);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ambiguous_and_cancelled_attempts_are_not_reported_as_rollback_or_commit() {
        let metrics = TransactionMetrics::default();
        metrics.begin().finish(TransactionOutcome::Indeterminate);
        drop(metrics.begin());
        metrics.begin().finish(TransactionOutcome::Committed);
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.attempts, 3);
        assert_eq!(snapshot.in_flight, 0);
        assert_eq!(snapshot.committed, 1);
        assert_eq!(snapshot.indeterminate, 1);
        assert_eq!(snapshot.abandoned, 1);
        assert_eq!(snapshot.latency_bucket_counts.iter().sum::<u64>(), 3);
    }

    #[test]
    fn concurrent_outcomes_preserve_counts() {
        let metrics = TransactionMetrics::default();
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let metrics = &metrics;
                scope.spawn(move || {
                    for _ in 0..100 {
                        metrics.begin().finish(TransactionOutcome::Conflict);
                    }
                });
            }
        });
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.attempts, 800);
        assert_eq!(snapshot.conflicts, 800);
        assert_eq!(snapshot.in_flight, 0);
    }
}
