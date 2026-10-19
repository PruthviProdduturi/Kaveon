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
    /// Encoded size of the most recently published snapshot, and the largest
    /// seen. The ceiling is enforced on this number, so publishing it is what
    /// turns a hard stop into something an operator can see coming.
    snapshot_bytes: AtomicU64,
    snapshot_bytes_peak: AtomicU64,
    /// The highest occupancy threshold already warned about, so a deployment
    /// sitting at 80% says so once rather than on every commit.
    snapshot_warned_percent: AtomicU64,
    /// The ceiling most recently applied, so a scrape can report it.
    snapshot_limit: AtomicU64,
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
    /// Encoded bytes of the most recent published snapshot, 0 before the first
    /// commit of this process.
    pub snapshot_bytes: u64,
    /// The largest snapshot this process has published.
    pub snapshot_bytes_peak: u64,
    /// The ceiling a commit is refused past, so a reader does not have to know
    /// the constant to interpret the two numbers above.
    pub snapshot_bytes_limit: u64,
    /// Peak occupancy as a whole percentage of the ceiling. This is the number
    /// worth alerting on; at 100 writes stop.
    pub snapshot_percent_of_limit: u64,
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
        let peak = self.snapshot_bytes_peak.load(Ordering::Relaxed);
        let limit = self.snapshot_limit.load(Ordering::Relaxed);
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
            snapshot_bytes: self.snapshot_bytes.load(Ordering::Relaxed),
            snapshot_bytes_peak: peak,
            snapshot_bytes_limit: limit,
            // A limit of zero means no ceiling has been applied yet, which is
            // reported as zero occupancy rather than as a division.
            snapshot_percent_of_limit: peak.saturating_mul(100).checked_div(limit).unwrap_or(0),
        }
    }

    /// Record the encoded size of a snapshot that was just published, and warn
    /// as it approaches the ceiling.
    ///
    /// `limit` is passed in rather than read from a constant here so the
    /// ceiling stays owned by the module that enforces it; a metric reporting
    /// a different limit from the one actually applied would be worse than no
    /// metric.
    pub fn record_snapshot_bytes(&self, bytes: u64, limit: u64) {
        self.snapshot_bytes.store(bytes, Ordering::Relaxed);
        self.snapshot_bytes_peak.fetch_max(bytes, Ordering::Relaxed);
        self.snapshot_limit.store(limit, Ordering::Relaxed);
        if limit == 0 {
            return;
        }
        let percent = bytes.saturating_mul(100) / limit;
        // Warn once per threshold crossed. A deployment that settles at 80%
        // should say so when it arrives, not on every write thereafter.
        for threshold in [75_u64, 90, 95] {
            if percent >= threshold
                && self
                    .snapshot_warned_percent
                    .fetch_max(threshold, Ordering::Relaxed)
                    < threshold
            {
                eprintln!(
                    "[product-commit] snapshot is at {percent}% of its {limit}-byte ceiling \
({bytes} bytes); past 100% commits are rejected, not slowed"
                );
            }
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
    fn occupancy_follows_the_peak_rather_than_the_latest_write() {
        let metrics = TransactionMetrics::default();
        let limit = 1_000;

        metrics.record_snapshot_bytes(400, limit);
        assert_eq!(metrics.snapshot().snapshot_percent_of_limit, 40);

        metrics.record_snapshot_bytes(800, limit);
        assert_eq!(metrics.snapshot().snapshot_percent_of_limit, 80);

        // Records can be deleted, so a later snapshot can be smaller. The
        // occupancy worth alerting on is still the high-water mark: a quiet
        // week must not make a deployment look further from the ceiling than
        // it has already been.
        metrics.record_snapshot_bytes(100, limit);
        let scrape = metrics.snapshot();
        assert_eq!(scrape.snapshot_bytes, 100, "latest size is reported as-is");
        assert_eq!(scrape.snapshot_bytes_peak, 800);
        assert_eq!(scrape.snapshot_percent_of_limit, 80);
        assert_eq!(scrape.snapshot_bytes_limit, limit);
    }

    #[test]
    fn a_threshold_warns_once_rather_than_on_every_commit() {
        let metrics = TransactionMetrics::default();
        let limit = 1_000;

        // Below the first threshold nothing has been warned about.
        metrics.record_snapshot_bytes(700, limit);
        assert_eq!(metrics.snapshot_warned_percent.load(Ordering::Relaxed), 0);

        // Crossing 75 arms it, and staying there does not re-arm it.
        metrics.record_snapshot_bytes(760, limit);
        assert_eq!(metrics.snapshot_warned_percent.load(Ordering::Relaxed), 75);
        metrics.record_snapshot_bytes(770, limit);
        assert_eq!(
            metrics.snapshot_warned_percent.load(Ordering::Relaxed),
            75,
            "a deployment parked above a threshold must not warn on every write"
        );

        // A higher threshold is its own event.
        metrics.record_snapshot_bytes(960, limit);
        assert_eq!(metrics.snapshot_warned_percent.load(Ordering::Relaxed), 95);
    }

    #[test]
    fn a_zero_limit_is_reported_rather_than_dividing_by_it() {
        let metrics = TransactionMetrics::default();
        metrics.record_snapshot_bytes(500, 0);
        let scrape = metrics.snapshot();
        assert_eq!(scrape.snapshot_percent_of_limit, 0);
        assert_eq!(scrape.snapshot_bytes, 500);
    }

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
