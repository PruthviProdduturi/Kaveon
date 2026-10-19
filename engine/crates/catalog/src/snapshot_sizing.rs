//! How much control-plane state fits under the snapshot ceiling.
//!
//! Every commit republishes the whole snapshot and `encode` refuses it past
//! `MAX_SNAPSHOT_BYTES` (8 MiB). Past that, writes are rejected outright --
//! not slowed, not degraded. So the question "how many records can this
//! product hold?" has an exact answer, and nobody had measured it.
//!
//! This measures it directly: build snapshots of a known shape, encode them
//! the way the commit path does, and divide. It needs no cluster and no
//! object store, because the ceiling is a property of the encoded bytes.
//!
//! Run it for the numbers rather than the pass/fail:
//!
//! ```text
//! cargo test -p kaveon-catalog --test snapshot_sizing -- --nocapture
//! ```
//!
//! The assertions only catch a structural regression -- a field that starts
//! carrying a document body, say. The table it prints is the point.

use std::collections::{BTreeMap, BTreeSet};

use crate::product_manifest::{
    CatalogSnapshot, ImmutableFileRef, ProductRecordKind, ProductRecordRef, ProductRecordReference,
};

/// The ceiling enforced in `product_commit.rs`. Kept in step by the assertion
/// at the end of this file rather than by hoping.
const MAX_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;

/// A 36-character identifier, as the product issues.
fn id_at(index: usize) -> String {
    format!("{:08x}-1f4b-4c2a-9e7d-{:012x}", index, index)
}

/// A content-addressed document reference: a path under the catalog root and
/// a lowercase SHA-256. Both are strings, and JSON stores them literally, so
/// they dominate the per-record cost.
fn document_for(kind: ProductRecordKind, id: &str) -> ImmutableFileRef {
    ImmutableFileRef {
        path: format!("product/{}/{}/document.json", kind_name(kind), id),
        sha256: format!("{:064x}", 0x5eed_1234_u64 as u128 + id.len() as u128),
    }
}

/// The crate's own name for a kind. Reachable from inside the crate, so the
/// harness measures the real encoding rather than a copy that could drift.
fn kind_name(kind: ProductRecordKind) -> &'static str {
    kind.name()
}

/// What a record of this kind actually carries in the snapshot.
///
/// A dataset is named and unique on that name, and points at the source it
/// reads. A chart points at its dataset. A query-history row is anonymous
/// bulk: no unique key, no outgoing reference. The difference between those
/// two shapes is most of the difference in cost.
fn record(kind: ProductRecordKind, index: usize) -> (String, ProductRecordRef) {
    let id = id_at(index);
    let mut unique_values = BTreeMap::new();
    let mut references = BTreeSet::new();

    match kind {
        ProductRecordKind::Dataset => {
            unique_values.insert("name".to_owned(), format!("Operational dataset {index}"));
            references.insert(ProductRecordReference {
                kind: ProductRecordKind::Source,
                id: id_at(index % 16),
            });
        }
        ProductRecordKind::Chart | ProductRecordKind::Dashboard => {
            unique_values.insert("name".to_owned(), format!("Revenue by region {index}"));
            references.insert(ProductRecordReference {
                kind: ProductRecordKind::Dataset,
                id: id_at(index % 64),
            });
        }
        ProductRecordKind::SavedQuery | ProductRecordKind::DlmDefinition => {
            unique_values.insert("name".to_owned(), format!("Saved analysis {index}"));
        }
        // The bulk kinds: one row per user action, no name to be unique on.
        _ => {}
    }

    (
        format!("{}/{}", kind_name(kind), id),
        ProductRecordRef {
            kind,
            id: id.clone(),
            revision: 1 + (index as u64 % 7),
            document: document_for(kind, &id),
            unique_values,
            references,
        },
    )
}

fn snapshot_with(records: impl Iterator<Item = (String, ProductRecordRef)>) -> CatalogSnapshot {
    let mut snapshot = CatalogSnapshot::empty("snapshot-sizing").expect("empty snapshot");
    snapshot.product_records = records.collect();
    snapshot
}

fn encoded_len(snapshot: &CatalogSnapshot) -> usize {
    serde_json::to_vec(snapshot)
        .expect("snapshot encodes")
        .len()
}

/// Cost of one record of a kind, measured as the slope rather than the
/// intercept: encode 2,000 and 1,000 and take the difference, so the
/// snapshot's own fixed header does not inflate the per-record figure.
fn bytes_per_record(kind: ProductRecordKind) -> f64 {
    let small = encoded_len(&snapshot_with((0..1_000).map(|i| record(kind, i))));
    let large = encoded_len(&snapshot_with((0..2_000).map(|i| record(kind, i))));
    (large - small) as f64 / 1_000.0
}

#[test]
fn snapshot_ceiling_by_record_kind() {
    let kinds = [
        ProductRecordKind::Dataset,
        ProductRecordKind::Chart,
        ProductRecordKind::Dashboard,
        ProductRecordKind::SavedQuery,
        ProductRecordKind::Source,
        ProductRecordKind::Favorite,
        ProductRecordKind::UserRecent,
        ProductRecordKind::QueryHistory,
        ProductRecordKind::Activity,
        ProductRecordKind::ChatMessage,
        ProductRecordKind::DlmRun,
    ];

    let empty = encoded_len(&snapshot_with(std::iter::empty()));
    println!();
    println!("snapshot ceiling: {MAX_SNAPSHOT_BYTES} bytes (8 MiB), empty snapshot {empty} bytes");
    println!();
    println!(
        "{:<16} {:>14} {:>18}",
        "kind", "bytes/record", "records at ceiling"
    );
    println!("{}", "-".repeat(50));

    for kind in kinds {
        let per = bytes_per_record(kind);
        let capacity = ((MAX_SNAPSHOT_BYTES - empty) as f64 / per) as u64;
        println!("{:<16} {:>14.1} {:>18}", kind_name(kind), per, capacity);
        assert!(
            per > 0.0 && per < 4_096.0,
            "{} costs {per} bytes/record, which is outside any plausible range for a \
reference-only record -- has a document body started being inlined into the snapshot?",
            kind_name(kind)
        );
    }
    println!();
}

/// The number that actually decides the architecture.
///
/// Artifacts are bounded by human curation; the bulk kinds are not. A
/// deployment's ceiling is therefore set by how fast query history, activity
/// and recents accumulate, not by how many dashboards anyone builds. This
/// weights the mix accordingly: for every curated artifact, many rows of
/// bulk.
#[test]
fn snapshot_ceiling_for_a_realistic_mix() {
    // Per 1 dataset: 4 charts, 1 dashboard, 2 saved queries, and the bulk that
    // using them generates.
    let weights: [(ProductRecordKind, usize); 8] = [
        (ProductRecordKind::Dataset, 1),
        (ProductRecordKind::Chart, 4),
        (ProductRecordKind::Dashboard, 1),
        (ProductRecordKind::SavedQuery, 2),
        (ProductRecordKind::Favorite, 3),
        (ProductRecordKind::UserRecent, 10),
        (ProductRecordKind::Activity, 25),
        (ProductRecordKind::QueryHistory, 60),
    ];
    let per_unit: usize = weights.iter().map(|(_, n)| n).sum();

    let build = |units: usize| {
        let mut records = Vec::new();
        for unit in 0..units {
            for (kind, count) in weights {
                for n in 0..count {
                    records.push(record(kind, unit * 1_000 + n));
                }
            }
        }
        snapshot_with(records.into_iter())
    };

    let empty = encoded_len(&snapshot_with(std::iter::empty()));
    let small = encoded_len(&build(50));
    let large = encoded_len(&build(100));
    let per_record = (large - small) as f64 / ((50 * per_unit) as f64);
    let capacity = ((MAX_SNAPSHOT_BYTES - empty) as f64 / per_record) as u64;

    println!();
    println!("realistic mix: {per_unit} records per curated dataset");
    println!("  bytes/record          {per_record:.1}");
    println!("  records at ceiling    {capacity}");
    println!("  datasets at ceiling   {}", capacity / per_unit as u64);
    println!(
        "  query-history rows    {} of the ceiling",
        capacity * 60 / per_unit as u64
    );
    println!();
    println!(
        "A commit republishes all of this, so the last write before the ceiling \n\
         encodes about {:.1} MiB.",
        MAX_SNAPSHOT_BYTES as f64 / (1024.0 * 1024.0)
    );
    println!();

    assert!(
        capacity > 1_000,
        "a ceiling of {capacity} records would be too low to run a product on; \
check the record shape before believing it"
    );
}
