//! The coordinator's result cache: complete, exact results of finished
//! statements, served again without worker work while nothing they depend
//! on has changed.
//!
//! A result is keyed by everything that decides it: the statement text
//! (normalised outside string literals and quoted identifiers), the catalog
//! and schema it resolved names in, the published catalog snapshot, the
//! Delta versions the planner pinned, and the request settings that shape
//! rows (the time zone). A catalog publish changes the snapshot identity
//! and clears the cache outright; a Delta version the planner pinned
//! changes the key. Tables the planner did not pin (single-table scans of
//! Delta or Parquet locations) are covered by the catalog snapshot and by
//! the TTL, which is the staleness bound for them.
//!
//! Entries are evicted least-recently-used by bytes, expire after the TTL,
//! and an entry larger than an eighth of the budget is never kept. A budget
//! of zero disables the cache.

use crate::api::ColumnInfo;
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

/// What decides a result. Two statements with equal keys return equal rows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ResultCacheKey {
    /// The statement, normalised: trimmed, whitespace collapsed to one
    /// space and letters lowercased outside `'...'` and `"..."`.
    pub sql: String,
    pub catalog: String,
    pub schema: String,
    pub catalog_snapshot_id: String,
    /// Source location to Delta version, as the planner pinned them.
    pub delta_versions: BTreeMap<String, u64>,
    pub time_zone: Option<String>,
}

impl ResultCacheKey {
    pub fn new(
        sql: &str,
        catalog: &str,
        schema: &str,
        catalog_snapshot_id: &str,
        delta_versions: &BTreeMap<String, u64>,
        time_zone: Option<&str>,
    ) -> Self {
        Self {
            sql: normalize_sql(sql),
            catalog: catalog.to_owned(),
            schema: schema.to_owned(),
            catalog_snapshot_id: catalog_snapshot_id.to_owned(),
            delta_versions: delta_versions.clone(),
            time_zone: time_zone.map(str::to_owned),
        }
    }
}

/// The statement text as the key sees it. Whitespace runs outside quotes
/// become one space and letters outside quotes fold to lowercase; the
/// contents of `'...'` literals and `"..."` identifiers are kept verbatim,
/// quotes included, so `'Chat'` and `'chat'` stay different statements.
pub fn normalize_sql(sql: &str) -> String {
    #[derive(PartialEq)]
    enum State {
        Outside,
        Single,
        Double,
    }
    let mut out = String::with_capacity(sql.len());
    let mut state = State::Outside;
    let mut pending_space = false;
    for c in sql.trim().chars() {
        match state {
            State::Outside => {
                if c.is_whitespace() {
                    pending_space = true;
                    continue;
                }
                if pending_space {
                    out.push(' ');
                    pending_space = false;
                }
                match c {
                    '\'' => {
                        state = State::Single;
                        out.push(c);
                    }
                    '"' => {
                        state = State::Double;
                        out.push(c);
                    }
                    _ => out.extend(c.to_lowercase()),
                }
            }
            State::Single => {
                // A doubled quote is an escaped quote: it toggles out and
                // straight back in, which keeps it verbatim.
                if c == '\'' {
                    state = State::Outside;
                }
                out.push(c);
            }
            State::Double => {
                if c == '"' {
                    state = State::Outside;
                }
                out.push(c);
            }
        }
    }
    out
}

/// A complete result and where it came from.
pub(crate) struct CachedResult {
    pub columns: Vec<ColumnInfo>,
    pub rows: Arc<Vec<Vec<Value>>>,
    /// The original statement's elapsed time.
    pub elapsed_ms: u64,
    /// The original statement's query id.
    pub query_id: String,
    pub bytes: u64,
}

struct Entry {
    result: Arc<CachedResult>,
    inserted: Instant,
    tick: u64,
}

#[derive(Default)]
struct Inner {
    entries: HashMap<Arc<ResultCacheKey>, Entry>,
    /// Least recently used first.
    order: BTreeMap<u64, Arc<ResultCacheKey>>,
    tick: u64,
    bytes: u64,
}

/// Counters for `/v1/node` and `/v1/cluster`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct ResultCacheStats {
    pub enabled: bool,
    pub budget_bytes: u64,
    pub ttl_seconds: u64,
    pub entries: u64,
    pub bytes: u64,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub expirations: u64,
    /// Results refused because they exceed the per-entry ceiling.
    pub refused: u64,
}

pub struct ResultCache {
    budget_bytes: u64,
    ttl: Duration,
    inner: Mutex<Inner>,
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,
    expirations: AtomicU64,
    refused: AtomicU64,
}

impl ResultCache {
    pub fn new(budget_bytes: u64, ttl: Duration) -> Self {
        Self {
            budget_bytes,
            ttl,
            inner: Mutex::new(Inner::default()),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
            expirations: AtomicU64::new(0),
            refused: AtomicU64::new(0),
        }
    }

    /// A cache that keeps nothing.
    pub fn disabled() -> Self {
        Self::new(0, Duration::ZERO)
    }

    pub fn enabled(&self) -> bool {
        self.budget_bytes > 0
    }

    /// The largest result kept: an eighth of the budget.
    pub fn entry_ceiling_bytes(&self) -> u64 {
        self.budget_bytes / 8
    }

    /// The result for `key`, if it is present and not expired. Counts a hit
    /// or a miss; a disabled cache counts nothing.
    pub(crate) fn get(&self, key: &ResultCacheKey) -> Option<Arc<CachedResult>> {
        if !self.enabled() {
            return None;
        }
        let mut inner = self.lock();
        let Some(entry) = inner.entries.get(key) else {
            self.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        if entry.inserted.elapsed() >= self.ttl {
            let key = Arc::clone(
                inner
                    .entries
                    .get_key_value(key)
                    .map(|(key, _)| key)
                    .expect("entry present"),
            );
            Self::remove(&mut inner, &key);
            self.expirations.fetch_add(1, Ordering::Relaxed);
            self.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        let result = Arc::clone(&entry.result);
        let old_tick = entry.tick;
        inner.tick += 1;
        let tick = inner.tick;
        let owned = inner.order.remove(&old_tick).expect("ordered entry");
        inner.order.insert(tick, owned);
        inner.entries.get_mut(key).expect("entry present").tick = tick;
        self.hits.fetch_add(1, Ordering::Relaxed);
        Some(result)
    }

    /// Keeps a complete result. Returns whether it was kept: a disabled
    /// cache and a result above the per-entry ceiling keep nothing. Older
    /// entries are evicted least-recently-used until the result fits.
    pub(crate) fn insert(
        &self,
        key: ResultCacheKey,
        columns: &[ColumnInfo],
        rows: &[Vec<Value>],
        elapsed_ms: u64,
        query_id: &str,
    ) -> bool {
        if !self.enabled() {
            return false;
        }
        let bytes = result_bytes(columns, rows);
        if bytes > self.entry_ceiling_bytes() {
            self.refused.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        let result = Arc::new(CachedResult {
            columns: columns.to_vec(),
            rows: Arc::new(rows.to_vec()),
            elapsed_ms,
            query_id: query_id.to_owned(),
            bytes,
        });
        let mut inner = self.lock();
        let key = Arc::new(key);
        if inner.entries.contains_key(&key) {
            Self::remove(&mut inner, &key);
        }
        while inner.bytes + bytes > self.budget_bytes {
            let Some((_, oldest)) = inner.order.iter().next() else {
                break;
            };
            let oldest = Arc::clone(oldest);
            Self::remove(&mut inner, &oldest);
            self.evictions.fetch_add(1, Ordering::Relaxed);
        }
        inner.tick += 1;
        let tick = inner.tick;
        inner.order.insert(tick, Arc::clone(&key));
        inner.bytes += bytes;
        inner.entries.insert(
            key,
            Entry {
                result,
                inserted: Instant::now(),
                tick,
            },
        );
        true
    }

    /// Drops every entry: a catalog publish, a product commit, or an
    /// administrator asked for it. Returns what was dropped.
    pub fn clear(&self) -> (u64, u64) {
        let mut inner = self.lock();
        let dropped = (inner.entries.len() as u64, inner.bytes);
        inner.entries.clear();
        inner.order.clear();
        inner.bytes = 0;
        dropped
    }

    pub fn stats(&self) -> ResultCacheStats {
        let inner = self.lock();
        ResultCacheStats {
            enabled: self.enabled(),
            budget_bytes: self.budget_bytes,
            ttl_seconds: self.ttl.as_secs(),
            entries: inner.entries.len() as u64,
            bytes: inner.bytes,
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
            expirations: self.expirations.load(Ordering::Relaxed),
            refused: self.refused.load(Ordering::Relaxed),
        }
    }

    fn remove(inner: &mut Inner, key: &Arc<ResultCacheKey>) {
        if let Some(entry) = inner.entries.remove(key) {
            inner.order.remove(&entry.tick);
            inner.bytes = inner.bytes.saturating_sub(entry.result.bytes);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A poisoned lock holds a consistent map: every mutation completes
        // without a panic point between the two structures' updates that
        // could leave them disagreeing on an entry.
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// An estimate of a result's footprint in the cache, from the JSON values
/// it holds: strings by length, numbers and scalars by a fixed cost,
/// nested values by their encoding.
pub(crate) fn result_bytes(columns: &[ColumnInfo], rows: &[Vec<Value>]) -> u64 {
    const VALUE_OVERHEAD: u64 = 32;
    const ROW_OVERHEAD: u64 = 24;
    fn value_bytes(value: &Value) -> u64 {
        match value {
            Value::Null | Value::Bool(_) | Value::Number(_) => VALUE_OVERHEAD,
            Value::String(text) => VALUE_OVERHEAD + text.len() as u64,
            nested => VALUE_OVERHEAD + nested.to_string().len() as u64,
        }
    }
    let columns = columns
        .iter()
        .map(|column| (column.name.len() + column.data_type.len()) as u64 + VALUE_OVERHEAD)
        .sum::<u64>();
    let rows = rows
        .iter()
        .map(|row| ROW_OVERHEAD + row.iter().map(value_bytes).sum::<u64>())
        .sum::<u64>();
    columns + rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn key(sql: &str) -> ResultCacheKey {
        ResultCacheKey::new(sql, "lake", "sales", "snapshot-1", &BTreeMap::new(), None)
    }

    fn columns() -> Vec<ColumnInfo> {
        vec![ColumnInfo {
            name: "n".into(),
            data_type: "bigint".into(),
        }]
    }

    fn rows(count: usize, width: usize) -> Vec<Vec<Value>> {
        (0..count)
            .map(|index| vec![json!(index), Value::String("x".repeat(width))])
            .collect()
    }

    #[test]
    fn normalisation_folds_outside_quotes_only() {
        assert_eq!(
            normalize_sql("  SELECT   Country,\n\tSUM(actions) FROM   T WHERE surface = 'Chat'  "),
            "select country, sum(actions) from t where surface = 'Chat'"
        );
        assert_eq!(
            normalize_sql("select \"MyCol\" from t where s = 'a  b' and o = 'it''s'"),
            "select \"MyCol\" from t where s = 'a  b' and o = 'it''s'"
        );
        assert_eq!(
            key("SELECT 1 FROM t"),
            key("select 1\nfrom t"),
            "case and whitespace outside quotes do not change the key"
        );
        assert_ne!(key("SELECT 'A' FROM t"), key("SELECT 'a' FROM t"));
    }

    #[test]
    fn the_key_carries_everything_that_decides_a_result() {
        let base = key("SELECT 1");
        let mut other_schema = base.clone();
        other_schema.schema = "other".into();
        let mut other_snapshot = base.clone();
        other_snapshot.catalog_snapshot_id = "snapshot-2".into();
        let mut other_version = base.clone();
        other_version.delta_versions.insert("file:///t".into(), 3);
        let mut other_zone = base.clone();
        other_zone.time_zone = Some("UTC".into());
        for other in [other_schema, other_snapshot, other_version, other_zone] {
            assert_ne!(base, other);
        }
    }

    #[test]
    fn a_hit_serves_the_original_result_and_counts() {
        let cache = ResultCache::new(1 << 20, Duration::from_secs(60));
        assert!(cache.get(&key("SELECT 1")).is_none());
        assert!(cache.insert(key("SELECT 1"), &columns(), &rows(3, 4), 1234, "query-a"));
        let hit = cache.get(&key("select   1")).expect("hit");
        assert_eq!(hit.query_id, "query-a");
        assert_eq!(hit.elapsed_ms, 1234);
        assert_eq!(hit.rows.len(), 3);
        assert_eq!(hit.columns, columns());
        let stats = cache.stats();
        assert_eq!((stats.hits, stats.misses, stats.entries), (1, 1, 1));
        assert_eq!(stats.bytes, hit.bytes);
    }

    #[test]
    fn entries_expire_after_the_ttl() {
        let cache = ResultCache::new(1 << 20, Duration::from_millis(30));
        assert!(cache.insert(key("SELECT 1"), &columns(), &rows(1, 1), 1, "q"));
        assert!(cache.get(&key("SELECT 1")).is_some());
        std::thread::sleep(Duration::from_millis(60));
        assert!(cache.get(&key("SELECT 1")).is_none());
        let stats = cache.stats();
        assert_eq!(stats.expirations, 1);
        assert_eq!(stats.entries, 0);
        assert_eq!(stats.bytes, 0);
    }

    #[test]
    fn eviction_is_least_recently_used_by_bytes() {
        // Eight entries of one size fill the budget exactly, and each is
        // exactly the per-entry ceiling.
        let one = result_bytes(&columns(), &rows(10, 100));
        let cache = ResultCache::new(one * 8, Duration::from_secs(60));
        for index in 1..=8 {
            assert!(cache.insert(
                key(&format!("SELECT {index}")),
                &columns(),
                &rows(10, 100),
                1,
                "q"
            ));
        }
        assert_eq!(cache.stats().bytes, one * 8);
        // Touch 1 so 2 is the least recently used, then overflow by one.
        assert!(cache.get(&key("SELECT 1")).is_some());
        assert!(cache.insert(key("SELECT 9"), &columns(), &rows(10, 100), 1, "q9"));
        assert!(cache.get(&key("SELECT 2")).is_none(), "2 was evicted");
        for present in [1, 3, 4, 5, 6, 7, 8, 9] {
            assert!(
                cache.get(&key(&format!("SELECT {present}"))).is_some(),
                "{present}"
            );
        }
        let stats = cache.stats();
        assert_eq!(stats.evictions, 1);
        assert_eq!(stats.entries, 8);
        assert!(stats.bytes <= cache.budget_bytes);
    }

    #[test]
    fn a_result_above_the_per_entry_ceiling_is_not_kept() {
        let cache = ResultCache::new(8 * 1024, Duration::from_secs(60));
        assert_eq!(cache.entry_ceiling_bytes(), 1024);
        let large = rows(20, 100);
        assert!(result_bytes(&columns(), &large) > 1024);
        assert!(!cache.insert(key("SELECT big"), &columns(), &large, 1, "q"));
        assert!(cache.get(&key("SELECT big")).is_none());
        assert_eq!(cache.stats().refused, 1);
        assert!(cache.insert(key("SELECT small"), &columns(), &rows(2, 10), 1, "q"));
    }

    #[test]
    fn a_disabled_cache_keeps_and_counts_nothing() {
        let cache = ResultCache::disabled();
        assert!(!cache.enabled());
        assert!(!cache.insert(key("SELECT 1"), &columns(), &rows(1, 1), 1, "q"));
        assert!(cache.get(&key("SELECT 1")).is_none());
        assert_eq!(cache.stats(), ResultCacheStats::default());
    }

    #[test]
    fn a_snapshot_change_misses_and_a_publish_clears() {
        let cache = ResultCache::new(1 << 20, Duration::from_secs(60));
        assert!(cache.insert(key("SELECT 1"), &columns(), &rows(1, 1), 1, "q"));
        let mut republished = key("SELECT 1");
        republished.catalog_snapshot_id = "snapshot-2".into();
        assert!(cache.get(&republished).is_none());
        assert!(cache.get(&key("SELECT 1")).is_some());
        assert_eq!(cache.clear(), (1, result_bytes(&columns(), &rows(1, 1))));
        assert!(cache.get(&key("SELECT 1")).is_none());
        assert_eq!(cache.stats().entries, 0);
    }

    #[test]
    fn reinserting_a_key_replaces_its_entry_and_its_bytes() {
        let cache = ResultCache::new(1 << 20, Duration::from_secs(60));
        assert!(cache.insert(key("SELECT 1"), &columns(), &rows(10, 10), 1, "q1"));
        assert!(cache.insert(key("SELECT 1"), &columns(), &rows(1, 1), 2, "q2"));
        let stats = cache.stats();
        assert_eq!(stats.entries, 1);
        assert_eq!(stats.bytes, result_bytes(&columns(), &rows(1, 1)));
        assert_eq!(cache.get(&key("SELECT 1")).unwrap().query_id, "q2");
    }
}
