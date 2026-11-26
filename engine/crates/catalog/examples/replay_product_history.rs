//! Offline operator job for replaying a digest-pinned product history from a verified restore.
//! Uses workload identity, the native transaction publisher, and the normal revision constraints.
//! No network listener. Plan mode performs the same source and graph checks without publishing.
use kaveon_catalog::{
    product_commit::{CommitOutcome, ProductCatalogCommit, ProductDocuments},
    product_manifest::{
        CatalogChange, CatalogSnapshot, PrepareChange, ProductRecordRef, SnapshotRef,
    },
    product_metrics::TransactionMetrics,
};
use kaveon_storage::workload_identity_adls_commit;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, env, error::Error, sync::Arc};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    source_head: SnapshotRef,
    batches: Vec<Vec<ProductRecordRef>>,
}
fn setting(key: &str) -> Result<String> {
    Ok(env::var(key)?.trim().to_owned())
}
fn catalog(account: &str, container: &str, prefix: &str) -> Result<ProductCatalogCommit> {
    Ok(ProductCatalogCommit::new(
        workload_identity_adls_commit(account, container)?,
        prefix,
        Arc::new(TransactionMetrics::default()),
    )?)
}
fn same_logical_state(a: &CatalogSnapshot, b: &CatalogSnapshot) -> bool {
    a.product_records == b.product_records
        && a.typed_rows == b.typed_rows
        && a.typed_schemas == b.typed_schemas
        && a.control_records == b.control_records
        && a.tables == b.tables
        && a.runtime_table_sources == b.runtime_table_sources
        && a.table_statistics == b.table_statistics
        && a.reference_counts == b.reference_counts
}
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mode = env::args().nth(1).unwrap_or_else(|| "plan".to_owned());
    if !matches!(mode.as_str(), "plan" | "apply") {
        return Err("expected plan or apply".into());
    }
    let account = setting("CUTOVER_ACCOUNT")?;
    let source_container = setting("CUTOVER_SOURCE_CONTAINER")?;
    let source_prefix = setting("CUTOVER_SOURCE_PREFIX")?;
    let target_container = setting("CUTOVER_TARGET_CONTAINER")?;
    let target_prefix = setting("CUTOVER_TARGET_PREFIX")?;
    let base_id = setting("CUTOVER_TARGET_SNAPSHOT")?;
    let digest = setting("CUTOVER_PLAN_SHA256")?;
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return Err("invalid plan SHA-256".into());
    }
    if source_container == target_container && source_prefix == target_prefix {
        return Err("source and destination must differ".into());
    }
    let store = workload_identity_adls_commit(&account, &source_container)?;
    let input = store
        .read_bounded(&format!("work-cutover/merge-plans/{digest}.json"), 8 << 20)
        .await
        .map_err(|e| format!("read plan: {:?}", e.kind))?;
    if format!("{:x}", Sha256::digest(&input.bytes)) != digest {
        return Err("plan digest mismatch".into());
    }
    let plan: Plan = serde_json::from_slice(&input.bytes)?;
    let src = catalog(&account, &source_container, &source_prefix)?;
    let donor = src
        .read_current_shared()
        .await
        .map_err(|e| format!("read source: {e:?}"))?;
    if donor.reference() != plan.source_head {
        return Err("source head changed".into());
    }
    if plan.batches.is_empty() || plan.batches.len() > 10_000 {
        return Err("invalid batch count".into());
    }
    let dst = catalog(&account, &target_container, &target_prefix)?;
    let initial = dst
        .read_current_shared()
        .await
        .map_err(|e| format!("read destination: {e:?}"))?;
    let scope = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&[
            &account,
            &target_container,
            &target_prefix,
            &base_id,
            &digest
        ])?)
    );
    let id = |index: usize| format!("import-{}-{index}", scope);
    let start = if initial.snapshot_id == base_id {
        0
    } else {
        let found = (0..plan.batches.len())
            .find(|i| initial.snapshot_id == id(*i) && initial.operation_id == id(*i));
        found.ok_or("destination differs from the pinned base and from this import")? + 1
    };
    // Replay the authenticated donor history first. The resulting records must equal the donor
    // head, including document digests, revisions, uniqueness indexes and foreign references.
    let mut shadow = CatalogSnapshot::empty("source-replay")?;
    let mut batches = Vec::new();
    let mut source_at_resume = BTreeMap::new();
    let mut total_bytes = 0usize;
    for (i, batch) in plan.batches.iter().enumerate() {
        let mut changes = Vec::new();
        let mut documents = ProductDocuments::new();
        for record in batch {
            if !documents.contains_key(&record.document.path) {
                let bytes = src
                    .fetch_immutable_file(&record.document)
                    .await
                    .map_err(|e| format!("source object: {e:?}"))?;
                total_bytes = total_bytes
                    .checked_add(bytes.len())
                    .ok_or("import size overflow")?;
                if total_bytes > 128 << 20 {
                    return Err("import exceeds in-memory document limit".into());
                }
                documents.insert(record.document.path.clone(), bytes);
            }
            changes.push(if record.revision == 1 {
                CatalogChange::CreateProduct {
                    record: record.clone(),
                }
            } else {
                CatalogChange::UpdateProduct {
                    expected_revision: record.revision.checked_sub(1).ok_or("zero revision")?,
                    record: record.clone(),
                }
            });
        }
        let request = PrepareChange {
            base: shadow.reference(),
            snapshot_id: id(i),
            operation_id: id(i),
            request_digest: format!("{:x}", Sha256::digest(serde_json::to_vec(batch)?)),
            changes,
        };
        shadow = shadow.prepare(request.clone())?;
        if i + 1 == start {
            source_at_resume = shadow.product_records.clone();
        }
        batches.push((request, documents));
    }
    if shadow.product_records != donor.product_records {
        return Err("donor history does not reproduce its head".into());
    }
    for k in donor.product_records.keys() {
        if initial.product_records.get(k) != source_at_resume.get(k) {
            return Err("destination record collision or diverged import".into());
        }
    }
    let mut expected = (*initial).clone();
    let mut pending = Vec::new();
    for (mut req, docs) in batches.into_iter().skip(start) {
        req.base = expected.reference();
        expected = expected.prepare(req.clone())?;
        pending.push((req, docs));
    }
    println!(
        "Qualified {} donor records, {} transactions, {} target records; resume offset {}",
        donor.product_records.len(),
        plan.batches.len(),
        expected.product_records.len(),
        start
    );
    if mode == "plan" {
        return Ok(());
    }
    for (req, docs) in pending {
        let next = req.snapshot_id.clone();
        if dst
            .read_current_shared()
            .await
            .map_err(|e| format!("destination head: {e:?}"))?
            .reference()
            != req.base
        {
            return Err("concurrent destination write; stop and reconcile".into());
        }
        match dst.commit_with_documents(req, docs).await {
            CommitOutcome::Committed(s) | CommitOutcome::Replayed(s) if s.snapshot_id == next => (),
            _ => {
                return Err(
                    "transaction did not publish at the pinned head; inspect before retry".into(),
                );
            }
        }
    }
    let final_state = dst
        .read_current_shared()
        .await
        .map_err(|e| format!("verify destination: {e:?}"))?;
    if final_state.reference() != expected.reference()
        || !same_logical_state(&final_state, &expected)
    {
        return Err("post-import logical state mismatch".into());
    }
    for record in donor.product_records.values() {
        if dst
            .fetch_product_document_at(&final_state, record.kind, &record.id)
            .await
            .map_err(|e| format!("readback: {e:?}"))?
            .is_none()
        {
            return Err("missing imported document".into());
        }
    }
    println!(
        "Verified published snapshot {} at generation {}",
        final_state.snapshot_id, final_state.generation
    );
    Ok(())
}
