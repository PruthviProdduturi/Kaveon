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

/// What one write costs, and how evenly the shards fill.
///
/// Before sharding a kind was one object, so recording one query republished
/// every query: at the ceiling, 64 MiB to store a few hundred bytes. The
/// shard a record lands in is chosen by the digest of its key, so the cost of
/// a write is one shard and the question is how big that is -- and whether
/// the digest spreads records evenly enough that the answer holds.
#[test]
fn a_write_rewrites_one_shard_and_the_shards_fill_evenly() {
    use crate::product_manifest::{PRODUCT_RECORD_SHARDS, partition_product_records};

    // A kind at a realistic size: 200 owners of chat at their cap.
    const OWNERS: usize = 200;
    const MESSAGES_PER_OWNER: usize = 900;
    let mut records = Vec::new();
    for owner in 0..OWNERS {
        for n in 0..MESSAGES_PER_OWNER {
            records.push(record(ProductRecordKind::ChatMessage, owner * 100_000 + n));
        }
    }
    let snapshot = snapshot_with(records.into_iter());
    let (_, evicted) = partition_product_records(&snapshot.product_records);

    let total: usize = snapshot.product_records.len();
    let sizes: Vec<usize> = evicted.values().map(BTreeMap::len).collect();
    let largest = *sizes.iter().max().expect("shards exist");
    let smallest = *sizes.iter().min().expect("shards exist");
    let unsharded_bytes = encoded_len(&snapshot);
    let largest_shard_bytes = evicted
        .values()
        .map(|shard| serde_json::to_vec(shard).expect("encodes").len())
        .max()
        .expect("shards exist");

    println!();
    println!("{total} records of one kind across {} shards", sizes.len());
    println!("  smallest shard        {smallest} records");
    println!("  largest shard         {largest} records");
    println!("  a write rewrites      {largest_shard_bytes} bytes (worst shard)");
    println!("  before sharding       {unsharded_bytes} bytes (the whole kind)");
    println!(
        "  reduction             {:.0}x",
        unsharded_bytes as f64 / largest_shard_bytes as f64
    );
    println!();

    // Every shard in use, or the digest is not spreading records at all.
    assert_eq!(
        sizes.len(),
        PRODUCT_RECORD_SHARDS as usize,
        "records should reach every shard at this volume"
    );
    // Within a factor of two of even. A digest will not divide exactly, but a
    // shard carrying many times its share would mean the worst-case write is
    // far worse than the average and the measurement above is misleading.
    assert!(
        largest * 2 < smallest * 3,
        "shards are uneven: largest {largest}, smallest {smallest}; the cost of a write is \
then set by the unluckiest key rather than by the shard count"
    );
    // The point of the exercise.
    assert!(
        largest_shard_bytes * 20 < unsharded_bytes,
        "sharding must make a write cost a small fraction of the kind, not merely less"
    );
}

/// The shard of a key never moves.
///
/// A shard keeps its object when its contents are unchanged, which is what
/// stops a commit republishing the rest of the kind. That reuse is only sound
/// if a record always lands in the same shard, so this pins the assignment
/// rather than trusting it.
#[test]
fn a_records_shard_is_stable() {
    use crate::product_manifest::product_record_shard;

    for key in [
        "query_history/00000000-1f4b-4c2a-9e7d-000000000000",
        "chat_message/ffffffff-1f4b-4c2a-9e7d-ffffffffffff",
        "activity/1",
        "",
    ] {
        let first = product_record_shard(key);
        assert_eq!(first, product_record_shard(key), "'{key}' moved shards");
        assert!(first < crate::product_manifest::PRODUCT_RECORD_SHARDS);
    }
}

/// Which limit actually binds after the eviction.
///
/// The split removed the snapshot's byte ceiling as the constraint. It did not
/// remove the other two, and both are counts:
///
/// - `MAX_PRODUCT_RECORDS` (100,000) caps the records a snapshot may hold,
///   and `validate_product_records` checks it against the merged map -- the
///   one that still holds every kind, because eviction changed what is
///   published, not what is validated.
/// - a per-kind object has its own byte bound, eight times the snapshot's.
///
/// Reporting the byte figure alone would overstate what the change bought.
#[test]
fn which_limit_binds_after_the_eviction() {
    use crate::product_manifest::{MAX_PRODUCT_RECORDS, partition_product_records};

    const QUERY_HISTORY_PER_OWNER: usize = 800;
    const CHAT_MESSAGES_PER_OWNER: usize = 900;
    const RECENTS_PER_OWNER: usize = 20;
    // What one owner curates, which stays inline.
    const ARTIFACTS_PER_OWNER: usize = 3 + 12 + 3 + 6;

    let owner_records = |owner: usize| {
        let mut records = Vec::new();
        for (kind, count) in [
            (ProductRecordKind::QueryHistory, QUERY_HISTORY_PER_OWNER),
            (ProductRecordKind::ChatMessage, CHAT_MESSAGES_PER_OWNER),
            (ProductRecordKind::UserRecent, RECENTS_PER_OWNER),
            (ProductRecordKind::Dataset, 3),
            (ProductRecordKind::Chart, 12),
            (ProductRecordKind::Dashboard, 3),
            (ProductRecordKind::SavedQuery, 6),
        ] {
            for n in 0..count {
                records.push(record(kind, owner * 100_000 + n));
            }
        }
        records
    };

    let build = |owners: usize| {
        let mut records = Vec::new();
        for owner in 0..owners {
            records.extend(owner_records(owner));
        }
        snapshot_with(records.into_iter())
    };

    let empty = encoded_len(&snapshot_with(std::iter::empty()));
    let inline_bytes = |owners: usize| {
        let snapshot = build(owners);
        let (inline, _) = partition_product_records(&snapshot.product_records);
        encoded_len(&snapshot_with(inline.into_iter()))
    };
    let per_owner_inline = (inline_bytes(3) - inline_bytes(1)) as f64 / 2.0;

    // A per-kind object has its own byte bound, and the kind that fills
    // fastest decides. Chat messages are the largest allowance per owner.
    const MAX_PRODUCT_RECORD_TABLE_BYTES: usize = 64 * 1024 * 1024;
    let chat_bytes_per_owner =
        bytes_per_record(ProductRecordKind::ChatMessage) * CHAT_MESSAGES_PER_OWNER as f64;
    let history_bytes_per_owner =
        bytes_per_record(ProductRecordKind::QueryHistory) * QUERY_HISTORY_PER_OWNER as f64;
    // A kind is spread over PRODUCT_RECORD_SHARDS objects, so its capacity is
    // the object bound times the shard count rather than the bound alone.
    let by_kind_object = ((MAX_PRODUCT_RECORD_TABLE_BYTES
        * crate::product_manifest::PRODUCT_RECORD_SHARDS as usize) as f64
        / chat_bytes_per_owner.max(history_bytes_per_owner)) as u64;

    // Only the records the snapshot still carries count against this now.
    let by_record_count = (MAX_PRODUCT_RECORDS / ARTIFACTS_PER_OWNER) as u64;
    // An owner costs the snapshot nothing once every kind is published
    // beside it, and dividing by nothing is not a ceiling.
    let by_snapshot_bytes = if per_owner_inline < 1.0 {
        None
    } else {
        Some(((MAX_SNAPSHOT_BYTES - empty) as f64 / per_owner_inline) as u64)
    };
    let binding = by_record_count
        .min(by_kind_object)
        .min(by_snapshot_bytes.unwrap_or(u64::MAX));

    println!();
    println!("owners permitted by each limit, after eviction");
    match by_snapshot_bytes {
        Some(owners) => println!("  snapshot bytes        {owners}"),
        None => println!("  snapshot bytes        does not bind"),
    }
    println!("  carried-record count  {by_record_count}");
    println!("  per-kind object       {by_kind_object}");
    println!("  -> the ceiling is     {binding}");
    println!();
    println!(
        "Every kind is published beside the snapshot now, so an owner costs it\n\
{per_owner_inline:.0} bytes and its size stops being a ceiling at all. What constrains a\n\
deployment is the per-kind objects: one kind is spread over its shards, each bounded when\n\
it is encoded, and the kind with the largest per-owner allowance decides."
    );
    println!();

    // Not an assertion about which limit wins. That moves as each one is
    // addressed -- bytes bound, then the record count, then the per-kind
    // object, then bytes again once a kind was sharded -- and pinning it
    // would make this a record of the order the work happened in. What
    // matters is where the ceiling now is.
    assert!(
        binding > 500,
        "the ceiling is {binding} owners; eviction and sharding together were measured to \
carry it well past the eleven an inline snapshot allowed"
    );
}

/// What the eviction buys, measured the same way the ceiling was.
///
/// The bulk kinds move to one content-addressed object each and the snapshot
/// keeps only what it still has to resolve: the artifacts, and the references
/// that make deletion restrict. If the snapshot does not get dramatically
/// smaller here, the change is not worth making.
#[test]
fn evicting_the_bulk_kinds_moves_the_ceiling() {
    use crate::product_manifest::{merge_product_records, partition_product_records};

    const QUERY_HISTORY_PER_OWNER: usize = 800;
    const CHAT_MESSAGES_PER_OWNER: usize = 900;
    const RECENTS_PER_OWNER: usize = 20;

    let owner_records = |owner: usize| {
        let mut records = Vec::new();
        for (kind, count) in [
            (ProductRecordKind::QueryHistory, QUERY_HISTORY_PER_OWNER),
            (ProductRecordKind::ChatMessage, CHAT_MESSAGES_PER_OWNER),
            (ProductRecordKind::UserRecent, RECENTS_PER_OWNER),
        ] {
            for n in 0..count {
                records.push(record(kind, owner * 100_000 + n));
            }
        }
        // The artifacts an owner actually curates, which stay inline.
        for (kind, count) in [
            (ProductRecordKind::Dataset, 3),
            (ProductRecordKind::Chart, 12),
            (ProductRecordKind::Dashboard, 3),
            (ProductRecordKind::SavedQuery, 6),
        ] {
            for n in 0..count {
                records.push(record(kind, owner * 100_000 + 90_000 + n));
            }
        }
        records
    };

    let build = |owners: usize| {
        let mut records = Vec::new();
        for owner in 0..owners {
            records.extend(owner_records(owner));
        }
        snapshot_with(records.into_iter())
    };

    let whole = |owners: usize| encoded_len(&build(owners));
    let split = |owners: usize| {
        let snapshot = build(owners);
        let (inline, _evicted) = partition_product_records(&snapshot.product_records);
        encoded_len(&snapshot_with(inline.into_iter()))
    };

    let empty = encoded_len(&snapshot_with(std::iter::empty()));
    let before_per_owner = (whole(3) - whole(1)) as f64 / 2.0;
    let after_per_owner = (split(3) - split(1)) as f64 / 2.0;

    let owners_before = ((MAX_SNAPSHOT_BYTES - empty) as f64 / before_per_owner) as u64;
    let owners_after = ((MAX_SNAPSHOT_BYTES - empty) as f64 / after_per_owner) as u64;

    println!();
    println!("bytes an owner costs the snapshot");
    println!("  inline today      {before_per_owner:.0}");
    println!("  after eviction    {after_per_owner:.0}");
    println!("owners at the ceiling");
    println!("  inline today      {owners_before}");
    println!("  after eviction    {owners_after}");
    println!(
        "  improvement       {:.0}x",
        owners_after as f64 / owners_before as f64
    );
    println!();

    // The split has to be lossless, not merely smaller.
    let snapshot = build(2);
    let (inline, evicted) = partition_product_records(&snapshot.product_records);
    let merged = merge_product_records(inline, evicted).expect("the split rejoins");
    assert_eq!(
        merged, snapshot.product_records,
        "a snapshot split and rejoined must hold exactly what it held before"
    );

    assert!(
        owners_after > owners_before * 10,
        "eviction moved the ceiling from {owners_before} owners to {owners_after}, which is \
not enough to justify changing the published format"
    );
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
