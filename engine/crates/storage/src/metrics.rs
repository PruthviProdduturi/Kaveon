use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanMetricsSnapshot {
    pub files_considered: u64,
    pub files_opened: u64,
    pub row_groups_considered: u64,
    pub row_groups_selected: u64,
    pub rows_selected: u64,
    pub rows_emitted: u64,
    pub compressed_bytes_selected: u64,
    pub batches_emitted: u64,
    pub snapshot_elapsed: Duration,
    pub footer_elapsed: Duration,
    pub read_elapsed: Duration,
}

impl ScanMetricsSnapshot {
    pub fn row_groups_pruned(&self) -> u64 {
        self.row_groups_considered
            .saturating_sub(self.row_groups_selected)
    }

    pub fn rows_per_second(&self) -> f64 {
        rate(self.rows_emitted, self.read_elapsed)
    }

    pub fn compressed_bytes_per_second(&self) -> f64 {
        rate(self.compressed_bytes_selected, self.read_elapsed)
    }
}

fn rate(value: u64, elapsed: Duration) -> f64 {
    let seconds = elapsed.as_secs_f64();
    if seconds == 0.0 {
        0.0
    } else {
        value as f64 / seconds
    }
}

#[derive(Clone, Debug, Default)]
pub struct ScanMetrics(Arc<ScanMetricsInner>);

#[derive(Debug, Default)]
struct ScanMetricsInner {
    files_considered: AtomicU64,
    files_opened: AtomicU64,
    row_groups_considered: AtomicU64,
    row_groups_selected: AtomicU64,
    rows_selected: AtomicU64,
    rows_emitted: AtomicU64,
    compressed_bytes_selected: AtomicU64,
    batches_emitted: AtomicU64,
    snapshot_nanos: AtomicU64,
    footer_nanos: AtomicU64,
    read_nanos: AtomicU64,
}

impl ScanMetrics {
    pub fn snapshot(&self) -> ScanMetricsSnapshot {
        ScanMetricsSnapshot {
            files_considered: self.load(&self.0.files_considered),
            files_opened: self.load(&self.0.files_opened),
            row_groups_considered: self.load(&self.0.row_groups_considered),
            row_groups_selected: self.load(&self.0.row_groups_selected),
            rows_selected: self.load(&self.0.rows_selected),
            rows_emitted: self.load(&self.0.rows_emitted),
            compressed_bytes_selected: self.load(&self.0.compressed_bytes_selected),
            batches_emitted: self.load(&self.0.batches_emitted),
            snapshot_elapsed: Duration::from_nanos(self.load(&self.0.snapshot_nanos)),
            footer_elapsed: Duration::from_nanos(self.load(&self.0.footer_nanos)),
            read_elapsed: Duration::from_nanos(self.load(&self.0.read_nanos)),
        }
    }

    fn load(&self, value: &AtomicU64) -> u64 {
        value.load(Ordering::Relaxed)
    }
    pub(crate) fn files_considered(&self, value: u64) {
        self.0.files_considered.fetch_add(value, Ordering::Relaxed);
    }
    pub(crate) fn file_opened(&self) {
        self.0.files_opened.fetch_add(1, Ordering::Relaxed);
    }
    pub(crate) fn row_groups(&self, considered: u64, selected: u64) {
        self.0
            .row_groups_considered
            .fetch_add(considered, Ordering::Relaxed);
        self.0
            .row_groups_selected
            .fetch_add(selected, Ordering::Relaxed);
    }
    pub(crate) fn selected(&self, rows: u64, bytes: u64) {
        self.0.rows_selected.fetch_add(rows, Ordering::Relaxed);
        self.0
            .compressed_bytes_selected
            .fetch_add(bytes, Ordering::Relaxed);
    }
    pub(crate) fn emitted(&self, rows: usize) {
        self.0
            .rows_emitted
            .fetch_add(rows as u64, Ordering::Relaxed);
        self.0.batches_emitted.fetch_add(1, Ordering::Relaxed);
    }
    pub(crate) fn snapshot_time(&self, value: Duration) {
        add_duration(&self.0.snapshot_nanos, value);
    }
    pub(crate) fn footer_time(&self, value: Duration) {
        add_duration(&self.0.footer_nanos, value);
    }
    pub(crate) fn read_time(&self, value: Duration) {
        add_duration(&self.0.read_nanos, value);
    }
}

fn add_duration(target: &AtomicU64, value: Duration) {
    target.fetch_add(
        value.as_nanos().min(u128::from(u64::MAX)) as u64,
        Ordering::Relaxed,
    );
}
