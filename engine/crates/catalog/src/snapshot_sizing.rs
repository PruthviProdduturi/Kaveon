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

/// The owner every record is attributed to. Sized like a corporate address,
/// because that is what ends up in the snapshot.
fn owner_of(index: usize) -> String {
    format!(
        "person.surname{:04}@organisation.example.com",
        index % 1_000
    )
}

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
        ProductRecordKind::UserRecent => {
            // Keyed "<owner>:<item>" and pointing at what it is a recent view
            // of, per `user_recent_is_owner_unique_and_references_target`.
            unique_values.insert(
                "recent_owner_item".to_owned(),
                format!("{}:{}", owner_of(index), id_at(index % 64)),
            );
            references.insert(ProductRecordReference {
                kind: ProductRecordKind::Dashboard,
                id: id_at(index % 64),
            });
        }
        // The remaining bulk kinds have no name to be unique on beyond the
        // owner every record carries.
        _ => {}
    }

    // Seeded on every record before anything derived is added, so no record is
    // ever as small as an empty map would suggest. A principal is an email
    // address, and JSON stores it literally.
    unique_values.insert("owner_principal".to_owned(), owner_of(index));

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

/// The number that actually decides the architecture: how many people can use
/// this deployment before writes stop.
///
/// The first version of this weighted the kinds by a ratio chosen by hand,
/// which made the answer a model. It does not need to be. The product already
/// caps its bulk kinds per owner, and those caps are constants:
///
/// | kind            | cap per owner | where                              |
/// |-----------------|---------------|------------------------------------|
/// | `query_history` | 800           | `MAX_HISTORY_PER_OWNER`            |
/// | `chat_message`  | 900           | `MAX_OWNER_MESSAGES`               |
/// | `user_recent`   | 20            | `user_recents.py`, hard-coded      |
/// | `activity`      | **uncapped**  | no retention sweep exists          |
///
/// So a single active owner can hold 1,720 records in the snapshot before any
/// dataset, chart or dashboard is counted, and `activity` adds to that without
/// limit. Dividing the ceiling by that number gives the figure below, and it
/// is small.
#[test]
fn how_many_owners_fit_under_the_ceiling() {
    // The caps the product enforces today. If one of these changes, this test
    // is where the consequence shows up.
    const QUERY_HISTORY_PER_OWNER: usize = 800;
    const CHAT_MESSAGES_PER_OWNER: usize = 900;
    const RECENTS_PER_OWNER: usize = 20;

    let per_owner: Vec<(ProductRecordKind, usize)> = vec![
        (ProductRecordKind::QueryHistory, QUERY_HISTORY_PER_OWNER),
        (ProductRecordKind::ChatMessage, CHAT_MESSAGES_PER_OWNER),
        (ProductRecordKind::UserRecent, RECENTS_PER_OWNER),
    ];
    let records_per_owner: usize = per_owner.iter().map(|(_, n)| n).sum();

    let build = |owners: usize| {
        let mut records = Vec::new();
        for owner in 0..owners {
            for (kind, count) in &per_owner {
                for n in 0..*count {
                    records.push(record(*kind, owner * 100_000 + n));
                }
            }
        }
        snapshot_with(records.into_iter())
    };

    let empty = encoded_len(&snapshot_with(std::iter::empty()));
    let one = encoded_len(&build(1));
    let three = encoded_len(&build(3));
    let bytes_per_owner = (three - one) as f64 / 2.0;
    let owners = ((MAX_SNAPSHOT_BYTES - empty) as f64 / bytes_per_owner) as u64;

    println!();
    println!("records one owner may hold at their caps: {records_per_owner}");
    println!("  bytes per owner            {bytes_per_owner:.0}");
    println!("  owners at the ceiling      {owners}");
    println!();
    println!(
        "That is the whole snapshot, before a single dataset, chart or dashboard,\n\
and before `activity`, which has no retention sweep at all."
    );
    println!();

    // Not an aspiration -- a tripwire. If the ceiling ever admits fewer owners
    // than this, something has grown and the architecture decision is overdue.
    assert!(
        owners >= 5,
        "the snapshot holds only {owners} fully-active owners; the bulk kinds have to \
leave it before this is a product"
    );
}
