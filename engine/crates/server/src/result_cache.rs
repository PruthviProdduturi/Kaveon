//! The coordinator's result cache: complete, exact results of finished
//! statements, served again without worker work while nothing they depend
//! on has changed.
//!
//! A result is keyed by everything that decides it: the statement text
//! (normalised outside string literals and quoted identifiers), the catalog
//! and schema it resolved names in, the authenticated authorization scope,
//! the published catalog snapshot, every source identity the planner pinned,
//! and the request settings that shape rows (the time zone). A catalog publish
//! changes the snapshot identity and clears the cache outright; a source
//! identity changes the key for Delta, Iceberg, directory Parquet, and
//! single-file Parquet alike.
//!
//! Entries are evicted least-recently-used by bytes, expire after the TTL,
//! and an entry larger than an eighth of the budget is never kept. A budget
//! of zero disables the cache.

use crate::api::ColumnInfo;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
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
    /// The authenticated authorization view.  Results cannot be shared
    /// between principals (or between role changes for one principal), even
    /// when the SQL and lake snapshot are identical.
    pub authorization_scope: String,
    /// A digest of positional and named bound parameters. Parameters are
    /// part of the result even when the SQL text is unchanged; keeping only
    /// the statement here would let `WHERE id = $1` reuse another value's
    /// rows.
    pub parameter_digest: String,
    /// The analyzed source identity for every resolved source. This is
    /// deliberately broader than `delta_versions`: Parquet file and listing
    /// changes must invalidate a cached result too.
    pub source_versions: BTreeMap<String, String>,
    /// Source location to Delta version, as the planner pinned them.
    pub delta_versions: BTreeMap<String, u64>,
    pub time_zone: Option<String>,
}

impl ResultCacheKey {
    // The key intentionally spells out each independent result determinant;
    // grouping them into an opaque options struct would make omissions easier.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sql: &str,
        catalog: &str,
        schema: &str,
        catalog_snapshot_id: &str,
        authorization_scope: &str,
        parameter_digest: &str,
        source_versions: &BTreeMap<String, String>,
        delta_versions: &BTreeMap<String, u64>,
        time_zone: Option<&str>,
    ) -> Self {
        Self {
            sql: normalize_sql(sql),
            catalog: catalog.to_owned(),
            schema: schema.to_owned(),
            catalog_snapshot_id: catalog_snapshot_id.to_owned(),
            authorization_scope: authorization_scope.to_owned(),
            parameter_digest: parameter_digest.to_owned(),
            source_versions: source_versions.clone(),
            delta_versions: delta_versions.clone(),
            time_zone: time_zone.map(str::to_owned),
        }
    }
}

/// Returns a stable, non-reversible identity for bound parameters. Named
/// parameter object order is ignored, while positional and nested array order
/// remains significant. The values themselves never enter cache keys,
/// telemetry, or query records.
pub fn parameter_digest(
    positional: &[Value],
    named: &std::collections::HashMap<String, Value>,
) -> String {
    fn append(digest: &mut Sha256, value: &Value) {
        match value {
            Value::Null => digest.update(b"null"),
            Value::Bool(value) => digest.update(if *value { b"bool:1" } else { b"bool:0" }),
            Value::Number(value) => {
                digest.update(b"number:");
                digest.update(value.to_string().as_bytes());
            }
            Value::String(value) => {
                digest.update(b"string:");
                digest.update((value.len() as u64).to_be_bytes());
                digest.update(value.as_bytes());
            }
            Value::Array(values) => {
                digest.update(b"array:");
                digest.update((values.len() as u64).to_be_bytes());
                for value in values {
                    append(digest, value);
                }
            }
            Value::Object(values) => {
                digest.update(b"object:");
                let mut keys = values.keys().collect::<Vec<_>>();
                keys.sort_unstable();
                digest.update((keys.len() as u64).to_be_bytes());
                for key in keys {
                    digest.update((key.len() as u64).to_be_bytes());
                    digest.update(key.as_bytes());
                    append(digest, &values[key]);
                }
            }
        }
    }

    let mut digest = Sha256::new();
    digest.update(b"kaveon-result-parameters-v1");
    digest.update((positional.len() as u64).to_be_bytes());
    for value in positional {
        append(&mut digest, value);
    }
    let mut names = named.keys().collect::<Vec<_>>();
    names.sort_unstable();
    digest.update((names.len() as u64).to_be_bytes());
    for name in names {
        digest.update((name.len() as u64).to_be_bytes());
        digest.update(name.as_bytes());
        append(&mut digest, &named[name]);
    }
    format!("sha256:{:x}", digest.finalize())
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
    let chars = sql.trim().chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        match state {
            State::Outside => {
                if c == '-' && chars.get(index + 1) == Some(&'-') {
                    // SQL comments are lexical whitespace. Keeping one
                    // pending space prevents `SELECT/**/1` from becoming
                    // the different token `select1` while allowing comments
                    // to share a cache entry with ordinary whitespace.
                    pending_space = true;
                    index += 2;
                    while index < chars.len() && chars[index] != '\n' {
                        index += 1;
                    }
                    continue;
                }
                if c == '/' && chars.get(index + 1) == Some(&'*') {
                    pending_space = true;
                    index += 2;
                    while index + 1 < chars.len()
                        && !(chars[index] == '*' && chars[index + 1] == '/')
                    {
                        index += 1;
                    }
                    if index + 1 < chars.len() {
                        index += 2;
                    }
                    continue;
                }
                if c.is_whitespace() {
                    pending_space = true;
                    index += 1;
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
        index += 1;
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
    /// Coordinator-side lookup latency for hits, excluding query execution.
    pub hit_latency_samples: u64,
    pub hit_latency_ns_total: u64,
    pub hit_latency_ns_max: u64,
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
    hit_latency_samples: AtomicU64,
    hit_latency_ns_total: AtomicU64,
    hit_latency_ns_max: AtomicU64,
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
            hit_latency_samples: AtomicU64::new(0),
            hit_latency_ns_total: AtomicU64::new(0),
            hit_latency_ns_max: AtomicU64::new(0),
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
        // Saturation keeps a corrupted or near-u64::MAX accounting value from
        // wrapping below the budget and admitting an unbounded cache entry.
        // The normal path never approaches this bound, but fail-closed
        // accounting is part of the cache's memory safety contract.
        while inner.bytes > self.budget_bytes.saturating_sub(bytes) {
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
        inner.bytes = inner.bytes.saturating_add(bytes);
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

    /// Removes entries that have exceeded the TTL without requiring a lookup.
    /// The coordinator calls this from periodic housekeeping so cold expired
    /// results do not occupy the byte budget indefinitely.
    pub(crate) fn cleanup_expired(&self) {
        if !self.enabled() {
            return;
        }
        let now = Instant::now();
        let mut inner = self.lock();
        let expired = inner
            .entries
            .iter()
            .filter(|(_, entry)| entry.inserted + self.ttl <= now)
            .map(|(key, _)| Arc::clone(key))
            .collect::<Vec<_>>();
        for key in &expired {
            Self::remove(&mut inner, key);
        }
        self.expirations
            .fetch_add(expired.len() as u64, Ordering::Relaxed);
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
            hit_latency_samples: self.hit_latency_samples.load(Ordering::Relaxed),
            hit_latency_ns_total: self.hit_latency_ns_total.load(Ordering::Relaxed),
            hit_latency_ns_max: self.hit_latency_ns_max.load(Ordering::Relaxed),
        }
    }

    /// Records only a cache hit lookup. The API measures this around `get`
    /// so query execution and serialization are not mixed into the metric.
    pub(crate) fn record_hit_latency(&self, elapsed: Duration) {
        let nanos = elapsed.as_nanos().min(u128::from(u64::MAX)) as u64;
        self.hit_latency_samples.fetch_add(1, Ordering::Relaxed);
        self.hit_latency_ns_total
            .fetch_add(nanos, Ordering::Relaxed);
        self.hit_latency_ns_max.fetch_max(nanos, Ordering::Relaxed);
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
        ResultCacheKey::new(
            sql,
            "lake",
            "sales",
            "snapshot-1",
            "analyst:alice",
            "parameters:none",
            &BTreeMap::new(),
            &BTreeMap::new(),
            None,
        )
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
    fn normalisation_treats_sql_comments_as_whitespace() {
        assert_eq!(
            normalize_sql("SELECT /* projection */ 1 -- source\n FROM t"),
            normalize_sql("select 1 from t")
        );
        assert_eq!(
            normalize_sql("SELECT 'a--b', \"a/*b\" FROM t"),
            "select 'a--b', \"a/*b\" from t"
        );
        assert_eq!(normalize_sql("SELECT/**/1"), "select 1");
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
        let mut other_source = base.clone();
        other_source
            .source_versions
            .insert("file:///t".into(), "source-2".into());
        let mut other_zone = base.clone();
        other_zone.time_zone = Some("UTC".into());
        let mut other_principal = base.clone();
        other_principal.authorization_scope = "analyst:bob".into();
        let mut other_parameters = base.clone();
        other_parameters.parameter_digest = "parameters:other".into();
        for other in [
            other_schema,
            other_snapshot,
            other_version,
            other_source,
            other_zone,
            other_principal,
            other_parameters,
        ] {
            assert_ne!(base, other);
        }
    }

    #[test]
    fn a_hit_serves_the_original_result_and_counts() {
        let cache = ResultCache::new(1 << 20, Duration::from_secs(60));
        assert!(cache.get(&key("SELECT 1")).is_none());
        assert!(cache.insert(key("SELECT 1"), &columns(), &rows(3, 4), 1234, "query-a"));
        let hit = cache.get(&key("select   1")).expect("hit");
        cache.record_hit_latency(Duration::from_nanos(100));
        assert_eq!(hit.query_id, "query-a");
        assert_eq!(hit.elapsed_ms, 1234);
        assert_eq!(hit.rows.len(), 3);
        assert_eq!(hit.columns, columns());
        let stats = cache.stats();
        assert_eq!((stats.hits, stats.misses, stats.entries), (1, 1, 1));
        assert_eq!(stats.bytes, hit.bytes);
        assert_eq!(
            (
                stats.hit_latency_samples,
                stats.hit_latency_ns_total,
                stats.hit_latency_ns_max
            ),
            (1, 100, 100)
        );
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
    fn housekeeping_releases_expired_entries_without_a_lookup() {
        let cache = ResultCache::new(1 << 20, Duration::from_millis(20));
        assert!(cache.insert(key("SELECT 1"), &columns(), &rows(1, 1), 1, "q"));
        std::thread::sleep(Duration::from_millis(40));
        cache.cleanup_expired();
        let stats = cache.stats();
        assert_eq!(stats.entries, 0);
        assert_eq!(stats.bytes, 0);
        assert_eq!(stats.expirations, 1);
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

    #[test]
    fn near_maximum_accounting_evicts_before_byte_counter_can_wrap() {
        let cache = ResultCache::new(u64::MAX, Duration::from_secs(60));
        assert!(cache.insert(key("SELECT 1"), &columns(), &rows(1, 1), 1, "q1"));
        {
            let mut inner = cache.lock();
            inner.bytes = u64::MAX - 1;
        }

        assert!(cache.insert(key("SELECT 2"), &columns(), &rows(1, 1), 1, "q2"));
        let stats = cache.stats();
        assert_eq!(stats.entries, 1);
        assert_eq!(stats.evictions, 1);
        assert!(stats.bytes < 1_024);
        assert!(cache.get(&key("SELECT 1")).is_none());
        assert!(cache.get(&key("SELECT 2")).is_some());
    }

    #[test]
    fn parameter_digest_is_value_sensitive_but_named_order_independent() {
        let positional_a = vec![serde_json::json!(7), serde_json::json!(["x", "y"])];
        let positional_b = vec![serde_json::json!(8), serde_json::json!(["x", "y"])];
        let named_a = std::collections::HashMap::from([
            ("region".to_owned(), serde_json::json!("west")),
            ("active".to_owned(), serde_json::json!(true)),
        ]);
        let named_b = std::collections::HashMap::from([
            ("active".to_owned(), serde_json::json!(true)),
            ("region".to_owned(), serde_json::json!("west")),
        ]);
        assert_eq!(
            parameter_digest(&positional_a, &named_a),
            parameter_digest(&positional_a, &named_b)
        );
        assert_ne!(
            parameter_digest(&positional_a, &named_a),
            parameter_digest(&positional_b, &named_a)
        );
    }

    #[test]
    fn parameterized_statements_never_reuse_another_binding() {
        let cache = ResultCache::new(1 << 20, Duration::from_secs(60));
        let named = std::collections::HashMap::new();
        let first = parameter_digest(&[serde_json::json!(7)], &named);
        let second = parameter_digest(&[serde_json::json!(8)], &named);
        let mut first_key = key("SELECT * FROM events WHERE id = $1");
        first_key.parameter_digest = first;
        let mut second_key = first_key.clone();
        second_key.parameter_digest = second;

        assert!(cache.insert(
            first_key,
            &columns(),
            &[vec![serde_json::json!(7)]],
            1,
            "q1"
        ));
        assert!(cache.get(&second_key).is_none());
        assert_eq!(cache.stats().hits, 0);
        assert_eq!(cache.stats().misses, 1);
    }
}
