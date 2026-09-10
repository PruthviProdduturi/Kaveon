//! Conditional object writes for an ADLS-backed transaction manifest.
//!
//! This primitive deliberately has no overwrite operation. A caller must create
//! immutable objects or compare-and-swap a version it read from the same GET
//! response. Retryable and unknown write outcomes require durable-head
//! reconciliation before a caller retries an operation.

use std::sync::Arc;

use futures::StreamExt;
use object_store::{
    Error as ObjectStoreError, ObjectStore, PutMode, UpdateVersion, azure::MicrosoftAzureBuilder,
    path::Path,
};

use crate::object_reader::relative_path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectVersion {
    e_tag: String,
    version: Option<String>,
}

impl ObjectVersion {
    #[must_use]
    pub fn e_tag(&self) -> &str {
        &self.e_tag
    }

    #[must_use]
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    fn update_version(&self) -> UpdateVersion {
        UpdateVersion {
            e_tag: Some(self.e_tag.clone()),
            version: self.version.clone(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct VersionedObject {
    pub bytes: Vec<u8>,
    pub version: ObjectVersion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitErrorKind {
    Conflict,
    Missing,
    Retryable,
    Authentication,
    Authorization,
    Unsupported,
    Invalid,
    LimitExceeded,
    Other,
}

#[derive(Debug, PartialEq, Eq)]
pub struct CommitError {
    pub kind: CommitErrorKind,
}

pub type CommitResult<T> = std::result::Result<T, CommitError>;

/// An ADLS-compatible conditional object writer.
///
/// It accepts only normalized relative object paths, and never logs payloads,
/// credentials, ETags, or paths.
#[derive(Clone)]
pub struct AdlsConditionalCommit {
    store: Arc<dyn ObjectStore>,
}

impl AdlsConditionalCommit {
    #[must_use]
    pub fn new(store: Arc<dyn ObjectStore>) -> Self {
        Self { store }
    }

    /// Atomically writes a new immutable object and fails if it already exists.
    pub async fn create_immutable(
        &self,
        object_path: &str,
        bytes: Vec<u8>,
    ) -> CommitResult<ObjectVersion> {
        let path = parse_path(object_path)?;
        let result = self
            .store
            .put_opts(&path, bytes.into(), PutMode::Create.into())
            .await
            .map_err(classify_error)?;
        version_from_put(result.e_tag, result.version)
    }

    /// Reads bytes and the exact version returned for those bytes in one GET.
    pub async fn read(&self, object_path: &str) -> CommitResult<VersionedObject> {
        let path = parse_path(object_path)?;
        let result = self.store.get(&path).await.map_err(classify_error)?;
        let version = version_from_put(result.meta.e_tag.clone(), result.meta.version.clone())?;
        let bytes = result.bytes().await.map_err(classify_error)?;
        Ok(VersionedObject {
            bytes: bytes.to_vec(),
            version,
        })
    }

    /// Reads an object only when both its reported and streamed size fit `max_bytes`.
    pub async fn read_bounded(
        &self,
        object_path: &str,
        max_bytes: usize,
    ) -> CommitResult<VersionedObject> {
        if max_bytes == 0 {
            return Err(CommitError {
                kind: CommitErrorKind::Invalid,
            });
        }
        let path = parse_path(object_path)?;
        let result = self.store.get(&path).await.map_err(classify_error)?;
        let reported_size = result.meta.size;
        if reported_size > max_bytes {
            return Err(CommitError {
                kind: CommitErrorKind::LimitExceeded,
            });
        }
        let version = version_from_put(result.meta.e_tag.clone(), result.meta.version.clone())?;
        let mut stream = result.into_stream();
        let mut bytes = Vec::with_capacity(reported_size);
        while let Some(chunk) = stream.next().await {
            append_bounded(&mut bytes, &chunk.map_err(classify_error)?, max_bytes)?;
        }
        Ok(VersionedObject { bytes, version })
    }

    /// Atomically replaces an object only when its ETag is still `expected`.
    pub async fn compare_and_swap(
        &self,
        object_path: &str,
        expected: &ObjectVersion,
        bytes: Vec<u8>,
    ) -> CommitResult<ObjectVersion> {
        let path = parse_path(object_path)?;
        let result = self
            .store
            .put_opts(
                &path,
                bytes.into(),
                PutMode::Update(expected.update_version()).into(),
            )
            .await
            .map_err(classify_error)?;
        version_from_put(result.e_tag, result.version)
    }
}

fn parse_path(value: &str) -> CommitResult<Path> {
    relative_path(value).map_err(|_| CommitError {
        kind: CommitErrorKind::Invalid,
    })
}

fn version_from_put(e_tag: Option<String>, version: Option<String>) -> CommitResult<ObjectVersion> {
    let Some(e_tag) = e_tag.filter(|value| !value.is_empty()) else {
        return Err(CommitError {
            kind: CommitErrorKind::Unsupported,
        });
    };
    Ok(ObjectVersion { e_tag, version })
}

fn append_bounded(target: &mut Vec<u8>, chunk: &[u8], max_bytes: usize) -> CommitResult<()> {
    if target.len().saturating_add(chunk.len()) > max_bytes {
        return Err(CommitError {
            kind: CommitErrorKind::LimitExceeded,
        });
    }
    target.extend_from_slice(chunk);
    Ok(())
}

fn classify_error(error: ObjectStoreError) -> CommitError {
    let kind = match error {
        ObjectStoreError::AlreadyExists { .. }
        | ObjectStoreError::Precondition { .. }
        | ObjectStoreError::NotModified { .. } => CommitErrorKind::Conflict,
        ObjectStoreError::NotFound { .. } => CommitErrorKind::Missing,
        ObjectStoreError::Unauthenticated { .. } => CommitErrorKind::Authentication,
        ObjectStoreError::PermissionDenied { .. } => CommitErrorKind::Authorization,
        ObjectStoreError::NotSupported { .. } | ObjectStoreError::NotImplemented => {
            CommitErrorKind::Unsupported
        }
        ObjectStoreError::InvalidPath { .. } | ObjectStoreError::UnknownConfigurationKey { .. } => {
            CommitErrorKind::Invalid
        }
        // Cloud HTTP and transport failures arrive here after object_store's own
        // retries. The caller must read the durable head before deciding whether
        // the operation was committed; this layer never retries a write.
        ObjectStoreError::Generic { .. } | ObjectStoreError::JoinError { .. } => {
            CommitErrorKind::Retryable
        }
        _ => CommitErrorKind::Other,
    };
    CommitError { kind }
}

/// Builds an ADLS conditional store from AKS workload identity, falling back to
/// Azure managed identity when no workload variables are present. This API does
/// not read account keys, SAS tokens, bearer tokens, or client secrets.
pub fn workload_identity_adls_commit(
    account: &str,
    container: &str,
) -> Result<AdlsConditionalCommit, String> {
    if account.trim() != account
        || !(3..=24).contains(&account.len())
        || !account
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        return Err("ADLS account must contain only lowercase ASCII letters and digits".to_owned());
    }
    if container.trim() != container
        || container.len() < 3
        || container.len() > 63
        || !container
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || container.starts_with('-')
        || container.ends_with('-')
        || container.contains("--")
    {
        return Err("ADLS container must be a normalized Azure container name".to_owned());
    }
    let mut builder = MicrosoftAzureBuilder::new()
        .with_account(account)
        .with_container_name(container);
    let workload = (
        std::env::var("AZURE_CLIENT_ID").ok(),
        std::env::var("AZURE_TENANT_ID").ok(),
        std::env::var("AZURE_FEDERATED_TOKEN_FILE").ok(),
    );
    builder = match workload {
        (Some(client), Some(tenant), Some(token_file)) => builder
            .with_client_id(client)
            .with_tenant_id(tenant)
            .with_federated_token_file(token_file),
        (None, None, None) => builder,
        _ => return Err("Azure workload identity environment is incomplete".to_owned()),
    };
    if let Ok(authority) = std::env::var("AZURE_AUTHORITY_HOST") {
        builder = builder.with_authority_host(authority);
    }
    let store = builder
        .build()
        .map_err(|error| format!("cannot configure ADLS object store: {error}"))?;
    Ok(AdlsConditionalCommit::new(Arc::new(store)))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use object_store::memory::InMemory;

    use super::*;

    fn store() -> AdlsConditionalCommit {
        AdlsConditionalCommit::new(Arc::new(InMemory::new()))
    }

    #[tokio::test]
    async fn immutable_create_rejects_a_second_writer() {
        let store = store();
        store
            .create_immutable("transactions/1.json", b"first".to_vec())
            .await
            .unwrap();
        let error = store
            .create_immutable("transactions/1.json", b"second".to_vec())
            .await
            .unwrap_err();
        assert_eq!(error.kind, CommitErrorKind::Conflict);
        assert_eq!(
            store.read("transactions/1.json").await.unwrap().bytes,
            b"first"
        );
    }

    #[tokio::test]
    async fn compare_and_swap_rejects_a_stale_etag_without_overwriting() {
        let store = store();
        store
            .create_immutable("heads/catalog.json", b"generation-1".to_vec())
            .await
            .unwrap();
        let initial = store.read("heads/catalog.json").await.unwrap();
        store
            .compare_and_swap(
                "heads/catalog.json",
                &initial.version,
                b"generation-2".to_vec(),
            )
            .await
            .unwrap();
        let error = store
            .compare_and_swap(
                "heads/catalog.json",
                &initial.version,
                b"stale-overwrite".to_vec(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.kind, CommitErrorKind::Conflict);
        assert_eq!(
            store.read("heads/catalog.json").await.unwrap().bytes,
            b"generation-2"
        );
    }

    #[tokio::test]
    async fn bounded_read_rejects_oversized_metadata_and_accepts_the_exact_limit() {
        let store = store();
        store
            .create_immutable("snapshots/one.json", b"1234".to_vec())
            .await
            .unwrap();
        assert_eq!(
            store
                .read_bounded("snapshots/one.json", 3)
                .await
                .unwrap_err()
                .kind,
            CommitErrorKind::LimitExceeded
        );
        assert_eq!(
            store
                .read_bounded("snapshots/one.json", 4)
                .await
                .unwrap()
                .bytes,
            b"1234"
        );
        assert_eq!(
            store
                .read_bounded("snapshots/one.json", 0)
                .await
                .unwrap_err()
                .kind,
            CommitErrorKind::Invalid
        );
    }

    #[test]
    fn bounded_body_append_never_exceeds_its_limit() {
        let mut body = b"12".to_vec();
        assert_eq!(
            append_bounded(&mut body, b"34", 3).unwrap_err().kind,
            CommitErrorKind::LimitExceeded
        );
        assert_eq!(body, b"12");
    }

    #[test]
    fn rejects_non_relative_paths_before_touching_the_store() {
        let error = parse_path("../credentials.json").unwrap_err();
        assert_eq!(error.kind, CommitErrorKind::Invalid);
    }

    #[test]
    fn classifies_conflict_auth_and_retryable_failures() {
        let source = || Box::new(std::io::Error::other("test")) as Box<_>;
        assert_eq!(
            classify_error(ObjectStoreError::Precondition {
                path: "head".into(),
                source: source(),
            })
            .kind,
            CommitErrorKind::Conflict
        );
        assert_eq!(
            classify_error(ObjectStoreError::Unauthenticated {
                path: "head".into(),
                source: source(),
            })
            .kind,
            CommitErrorKind::Authentication
        );
        assert_eq!(
            classify_error(ObjectStoreError::PermissionDenied {
                path: "head".into(),
                source: source(),
            })
            .kind,
            CommitErrorKind::Authorization
        );
        assert_eq!(
            classify_error(ObjectStoreError::Generic {
                store: "test",
                source: source(),
            })
            .kind,
            CommitErrorKind::Retryable
        );
    }

    #[test]
    fn workload_identity_store_rejects_unsafe_coordinates() {
        assert!(workload_identity_adls_commit("Unsafe Account", "product").is_err());
        assert!(workload_identity_adls_commit("kaveon", "../product").is_err());
        assert!(workload_identity_adls_commit("ka", "product").is_err());
    }
}
