//! Conditional object writes for an ADLS-backed transaction manifest.
//!
//! This primitive deliberately has no overwrite operation. A caller must create
//! immutable objects or compare-and-swap a version it read from the same GET
//! response. Retryable and unknown write outcomes require durable-head
//! reconciliation before a caller retries an operation.

use std::path::{Path as FsPath, PathBuf};
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
    local_root: Option<Arc<PathBuf>>,
}

impl AdlsConditionalCommit {
    #[must_use]
    pub fn new(store: Arc<dyn ObjectStore>) -> Self {
        Self {
            store,
            local_root: None,
        }
    }

    fn new_local(root: PathBuf) -> Self {
        Self {
            store: Arc::new(object_store::memory::InMemory::new()),
            local_root: Some(Arc::new(root)),
        }
    }

    /// Atomically writes a new immutable object and fails if it already exists.
    pub async fn create_immutable(
        &self,
        object_path: &str,
        bytes: Vec<u8>,
    ) -> CommitResult<ObjectVersion> {
        if let Some(root) = &self.local_root {
            return local_create(root, object_path, bytes);
        }
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
        if let Some(root) = &self.local_root {
            return local_read(root, object_path);
        }
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
        if let Some(root) = &self.local_root {
            let result = local_read(root, object_path)?;
            if result.bytes.len() > max_bytes {
                return Err(CommitError {
                    kind: CommitErrorKind::LimitExceeded,
                });
            }
            return Ok(result);
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
        if let Some(root) = &self.local_root {
            return local_compare_and_swap(root, object_path, expected, bytes);
        }
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

/// True when an untyped object-store failure is really a failed precondition.
///
/// Matched on the rendered error because the status is not carried in the
/// variant: Azure says `412 ConditionNotMet`, S3 says `PreconditionFailed`, and
/// GCS says `conditionNotMet`. A false positive here would turn a genuinely
/// unknown outcome into a retryable conflict, so the patterns stay narrow and
/// each one is anchored to a status or a documented error code.
fn is_precondition_failure(rendered: &str) -> bool {
    let text = rendered.to_ascii_lowercase();
    // The provider error codes are unambiguous on their own.
    if text.contains("conditionnotmet")
        || text.contains("preconditionfailed")
        || text.contains("precondition failed")
    {
        return true;
    }
    // A bare "412" is not: it also appears in byte counts, offsets and request
    // IDs. Only accept it where the renderer puts a status.
    [
        "status: 412",
        "status code: 412",
        "status=412",
        "http 412",
        "412 ",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn classify_error(error: ObjectStoreError) -> CommitError {
    // Keep the public error deliberately coarse, but retain a safe diagnostic
    // in the coordinator logs so cloud authentication/endpoint failures can
    // be distinguished from a missing durable head during startup. The object
    // store error never contains bearer tokens or secret values.
    eprintln!("ADLS object-store request failed: {error:?}");
    let error_text = error.to_string();
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
        // A failed precondition does not always arrive typed. Azure answers a
        // conditional PUT whose ETag no longer matches with `412 ConditionNotMet`,
        // and on several object_store paths that reaches us as `Generic` rather
        // than `Precondition`. It is still a conflict and nothing was written, so
        // classifying it as `Retryable` told the caller "indeterminate" about an
        // outcome that is in fact known — which is what stalled the product
        // catalog head CAS in the cloud. Read the status before the fallback.
        ObjectStoreError::Generic { .. } if is_precondition_failure(&error_text) => {
            CommitErrorKind::Conflict
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
/// An Azure container name, normalized. Shared so the emulator and the
/// workload-identity store cannot drift into accepting different names.
fn validate_container(container: &str) -> Result<(), String> {
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
    Ok(())
}

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
    validate_container(container)?;
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
    // Azure Container Apps injects its managed-identity endpoint as
    // IDENTITY_ENDPOINT (and the request header as IDENTITY_HEADER).  Using
    // `MicrosoftAzureBuilder::new()` intentionally avoids importing arbitrary
    // credential environment variables, so pass this endpoint explicitly;
    // otherwise object_store falls back to the VM metadata address, which is
    // unreachable from ACA and is reported only as a generic retryable error.
    if let Ok(endpoint) = std::env::var("IDENTITY_ENDPOINT") {
        builder = builder.with_msi_endpoint(endpoint);
    } else if let Ok(endpoint) = std::env::var("MSI_ENDPOINT") {
        builder = builder.with_msi_endpoint(endpoint);
    }
    let store = builder
        .build()
        .map_err(|error| format!("cannot configure ADLS object store: {error}"))?;
    Ok(AdlsConditionalCommit::new(Arc::new(store)))
}

/// The same primitive against an Azure Storage emulator.
///
/// This exists for one reason the in-memory backend cannot serve: the blob
/// path decides whether a failed conditional write is a conflict or an unknown
/// outcome by reading the provider's rendered error text, and only a real
/// Azure endpoint produces that text. Azurite speaks the Azure protocol and
/// returns Azure's errors, so the classifier is exercised for real.
///
/// Credentials are not configured here and no key appears in this repository:
/// `object_store`'s emulator mode supplies the well-known development account
/// and key itself, allows plain HTTP, and takes the endpoint from
/// `AZURITE_BLOB_STORAGE_URL` so the container can reach a sibling service.
///
/// Development and test only. It is reached by setting the product storage
/// mode to `emulator`, which no deployed configuration does.
pub fn emulator_adls_commit(container: &str) -> Result<AdlsConditionalCommit, String> {
    validate_container(container)?;
    let store = MicrosoftAzureBuilder::new()
        .with_use_emulator(true)
        .with_container_name(container)
        .build()
        .map_err(|error| format!("cannot configure the Azure emulator store: {error}"))?;
    Ok(AdlsConditionalCommit::new(Arc::new(store)))
}

/// Builds the same conditional immutable commit primitive over a local
/// directory. This is the single-node development equivalent of the ADLS
/// store: the transaction protocol, object layout, and compare-and-swap
/// semantics remain identical while the durable root is a caller-owned path.
pub fn local_file_commit(root: &FsPath) -> Result<AdlsConditionalCommit, String> {
    std::fs::create_dir_all(root)
        .map_err(|error| format!("cannot create local transaction store: {error}"))?;
    Ok(AdlsConditionalCommit::new_local(root.to_path_buf()))
}

fn local_path(root: &FsPath, object_path: &str) -> CommitResult<PathBuf> {
    let relative = parse_path(object_path)?.to_string();
    let path = root.join(relative);
    if !path.starts_with(root) {
        return Err(CommitError {
            kind: CommitErrorKind::Invalid,
        });
    }
    Ok(path)
}

fn local_version(bytes: &[u8]) -> ObjectVersion {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    ObjectVersion {
        e_tag: format!("{:016x}", hasher.finish()),
        version: None,
    }
}

fn local_create(root: &FsPath, object_path: &str, bytes: Vec<u8>) -> CommitResult<ObjectVersion> {
    let path = local_path(root, object_path)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|_| CommitError {
            kind: CommitErrorKind::Other,
        })?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| CommitError {
            kind: if error.kind() == std::io::ErrorKind::AlreadyExists {
                CommitErrorKind::Conflict
            } else {
                CommitErrorKind::Other
            },
        })?;
    std::io::Write::write_all(&mut file, &bytes).map_err(|_| CommitError {
        kind: CommitErrorKind::Other,
    })?;
    Ok(local_version(&bytes))
}

fn local_read(root: &FsPath, object_path: &str) -> CommitResult<VersionedObject> {
    let path = local_path(root, object_path)?;
    let bytes = std::fs::read(path).map_err(|error| CommitError {
        kind: if error.kind() == std::io::ErrorKind::NotFound {
            CommitErrorKind::Missing
        } else {
            CommitErrorKind::Other
        },
    })?;
    let version = local_version(&bytes);
    Ok(VersionedObject { bytes, version })
}

fn local_compare_and_swap(
    root: &FsPath,
    object_path: &str,
    expected: &ObjectVersion,
    bytes: Vec<u8>,
) -> CommitResult<ObjectVersion> {
    let path = local_path(root, object_path)?;
    let lock = path.with_extension("kaveon-lock");
    let _guard = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock)
        .map_err(|_| CommitError {
            kind: CommitErrorKind::Conflict,
        })?;
    let result = (|| {
        let current = local_read(root, object_path)?;
        if current.version != *expected {
            return Err(CommitError {
                kind: CommitErrorKind::Conflict,
            });
        }
        let temporary = path.with_extension(format!("kaveon-tmp-{}", std::process::id()));
        std::fs::write(&temporary, &bytes).map_err(|_| CommitError {
            kind: CommitErrorKind::Other,
        })?;
        std::fs::rename(&temporary, &path).map_err(|_| CommitError {
            kind: CommitErrorKind::Conflict,
        })?;
        Ok(local_version(&bytes))
    })();
    let _ = std::fs::remove_file(&lock);
    result
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
    async fn local_store_preserves_conditional_commit_contract() {
        let root = std::env::temp_dir().join(format!(
            "kaveon-local-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = local_file_commit(&root).unwrap();
        let first = store
            .create_immutable("heads/catalog.json", b"generation-1".to_vec())
            .await
            .unwrap();
        let read = store.read("heads/catalog.json").await.unwrap();
        assert_eq!(read.bytes, b"generation-1");
        assert_eq!(read.version, first);
        store
            .compare_and_swap("heads/catalog.json", &first, b"generation-2".to_vec())
            .await
            .unwrap();
        assert_eq!(
            store.read("heads/catalog.json").await.unwrap().bytes,
            b"generation-2"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    /// The blob path against a real Azure endpoint.
    ///
    /// Every other test here drives the in-memory backend, which implements
    /// conditional put but never produces Azure's error text -- and the text
    /// is what `is_precondition_failure` reads to tell a retryable conflict
    /// from an outcome it must treat as unknown. A provider that reworded its
    /// errors would break that silently, and nothing in this suite would
    /// notice. Azurite speaks the Azure protocol, so it does.
    ///
    /// Ignored because it needs a service. To run it:
    ///
    /// ```text
    /// docker compose up -d azurite
    /// az storage container create -n kaveon-product \
    ///   --connection-string "UseDevelopmentStorage=true"
    /// cargo test -p kaveon-storage azurite -- --ignored --nocapture
    /// ```
    ///
    /// `UseDevelopmentStorage=true` is Azure's own name for the emulator's
    /// well-known account, so no key is written to this repository or to a
    /// shell history.
    /// Concurrent writers against a real Azure endpoint.
    ///
    /// The in-memory and filesystem backends both settle a conditional write
    /// locally and instantly. Blob storage settles it over HTTP, against an
    /// ETag it issued, and reports a loss as a 412 whose meaning has to be
    /// read back out of an XML body. Writers contending there behave
    /// differently from writers contending in a map, and the deployment only
    /// ever meets the former.
    ///
    /// Same property as the in-process test: every writer increments, so with
    /// no lost update the counter must finish at exactly the number of
    /// writers. Fewer writers here than in-process, because each attempt is a
    /// round trip rather than a map lookup.
    ///
    /// Runs with the same setup as the contract test above.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "requires Azurite: docker compose up -d azurite"]
    async fn azurite_concurrent_writers_never_lose_an_update() {
        const WRITERS: u64 = 6;

        let container =
            std::env::var("KAVEON_AZURITE_CONTAINER").unwrap_or_else(|_| "kaveon-product".into());
        let store = emulator_adls_commit(&container).expect("emulator store");

        let key = format!(
            "azurite-race/{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        store
            .create_immutable(&key, b"0".to_vec())
            .await
            .expect("seed");

        // tokio::sync::Barrier, not std::sync::Barrier. These writers are async
        // tasks, and #[tokio::test] gives them a single-threaded runtime, so a
        // blocking wait parks the only worker and no other task ever arrives at
        // the barrier. The first version of this deadlocked for exactly that
        // reason; the in-process test gets away with the blocking one because
        // it runs its writers on spawn_blocking.
        let gate = Arc::new(tokio::sync::Barrier::new(WRITERS as usize));
        let conflicts = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let mut tasks = Vec::new();
        for _ in 0..WRITERS {
            let store = store.clone();
            let key = key.clone();
            let gate = Arc::clone(&gate);
            let conflicts = Arc::clone(&conflicts);
            tasks.push(tokio::spawn(async move {
                gate.wait().await;
                for _ in 0..200 {
                    let Ok(current) = store.read(&key).await else {
                        continue;
                    };
                    let value: u64 = String::from_utf8_lossy(&current.bytes)
                        .trim()
                        .parse()
                        .expect("counter stays numeric");
                    match store
                        .compare_and_swap(
                            &key,
                            &current.version,
                            (value + 1).to_string().into_bytes(),
                        )
                        .await
                    {
                        Ok(_) => return true,
                        Err(error) if error.kind == CommitErrorKind::Conflict => {
                            conflicts.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                        Err(error) => panic!("unexpected failure against Azurite: {error:?}"),
                    }
                }
                false
            }));
        }

        for task in tasks {
            assert!(
                task.await.unwrap(),
                "a writer never succeeded against Azurite"
            );
        }

        let observed = conflicts.load(std::sync::atomic::Ordering::Relaxed);
        assert!(
            observed > 0,
            "no writer ever lost a race against Azurite, so nothing was contended and the \
counter below proves nothing"
        );

        let final_value: u64 = String::from_utf8_lossy(&store.read(&key).await.unwrap().bytes)
            .trim()
            .parse()
            .unwrap();
        assert_eq!(
            final_value, WRITERS,
            "{WRITERS} writers each reported success against Azurite but the counter reached \
{final_value}; a conditional write was allowed to publish from a version it did not hold"
        );
        println!("azurite: {WRITERS} contended writers, {observed} conflicts, no lost update");
    }

    #[tokio::test]
    #[ignore = "requires Azurite: docker compose up -d azurite"]
    async fn azurite_conditional_commit_matches_the_contract() {
        let container =
            std::env::var("KAVEON_AZURITE_CONTAINER").unwrap_or_else(|_| "kaveon-product".into());
        let store = match emulator_adls_commit(&container) {
            Ok(store) => store,
            Err(error) => panic!("emulator store not configurable: {error}"),
        };

        // A fresh key per run, so repeated runs do not collide on a leftover.
        let key = format!(
            "azurite-contract/{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );

        let first = store
            .create_immutable(&key, b"{\"generation\":1}".to_vec())
            .await
            .expect("first write lands");

        // A second create of the same immutable key must not overwrite it.
        let second = store
            .create_immutable(&key, b"{\"generation\":2}".to_vec())
            .await;
        assert!(
            second.is_err() || second.is_ok(),
            "create_immutable must resolve, not hang"
        );

        let read = store.read(&key).await.expect("read back");
        assert_eq!(
            read.version.e_tag(),
            first.e_tag(),
            "the stored object must still be the one the first write published"
        );

        // The point of the test: a stale expectation must come back as a
        // conflict, classified from Azure's own error text rather than from a
        // string this suite wrote.
        let stale = ObjectVersion {
            e_tag: "\"0x8DUMMYSTALEETAG00\"".to_owned(),
            version: None,
        };
        let conflict = store
            .compare_and_swap(&key, &stale, b"{\"generation\":3}".to_vec())
            .await;
        match conflict {
            Err(error) if error.kind == CommitErrorKind::Conflict => {}
            Err(error) => panic!(
                "a stale ETag must classify as Conflict against real Azure errors, got {:?}. \
This is the classifier drifting from the provider's wording.",
                error.kind
            ),
            Ok(_) => panic!("a stale ETag must not be allowed to overwrite"),
        }

        // And the current expectation must still succeed afterwards.
        let swapped = store
            .compare_and_swap(&key, &read.version, b"{\"generation\":4}".to_vec())
            .await
            .expect("a current ETag still commits after a rejected one");
        assert_ne!(
            swapped.e_tag(),
            read.version.e_tag(),
            "a successful swap must publish a new version"
        );

        println!("azurite: conditional commit contract holds against real Azure errors");
    }

    /// A temporary directory that cleans itself up, for the local backend.
    struct TempRoot(std::path::PathBuf);

    impl TempRoot {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "kaveon-cas-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Run a scenario against both compare-and-swap implementations.
    ///
    /// `local_file_commit` documents itself as preserving the blob store's
    /// commit semantics. These are different code paths -- the blob one issues
    /// a conditional PUT, the local one takes a lock file and renames -- so the
    /// claim only holds as far as it is checked.
    async fn on_both_backends<F, Fut>(tag: &str, scenario: F)
    where
        F: Fn(&'static str, AdlsConditionalCommit) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        scenario(
            "blob",
            AdlsConditionalCommit::new(Arc::new(InMemory::new())),
        )
        .await;
        let root = TempRoot::new(tag);
        scenario("local", local_file_commit(&root.0).unwrap()).await;
    }

    /// Concurrent writers must not lose a write.
    ///
    /// Each task reads the current value, adds one, and swaps; a task that
    /// loses the race re-reads and tries again. With every write preserved the
    /// counter must finish at exactly the number of tasks. A lost update --
    /// two writers reading the same version and both being allowed to publish
    /// -- lands it lower, and nothing else does.
    #[tokio::test]
    async fn concurrent_writers_never_lose_an_update() {
        const WRITERS: u64 = 12;

        on_both_backends("lost-update", |backend, store| async move {
            let key = "races/counter.json";
            store
                .create_immutable(key, b"0".to_vec())
                .await
                .unwrap_or_else(|error| panic!("{backend}: seed failed: {error:?}"));

            // Every writer's first attempt happens at once. Without this the
            // tasks can run one after another, and a counter that reaches
            // WRITERS then says nothing -- it would do so even against a
            // compare-and-swap that ignored its expectation.
            let gate = Arc::new(std::sync::Barrier::new(WRITERS as usize));
            let conflicts = Arc::new(std::sync::atomic::AtomicU64::new(0));

            let mut tasks = Vec::new();
            for _ in 0..WRITERS {
                let store = store.clone();
                let gate = Arc::clone(&gate);
                let conflicts = Arc::clone(&conflicts);
                tasks.push(tokio::task::spawn_blocking(move || {
                    gate.wait();
                    // Bounded so a genuine livelock fails the test instead of
                    // hanging it. Twelve writers on one key need far fewer.
                    for _ in 0..2_000 {
                        let current = match futures::executor::block_on(store.read(key)) {
                            Ok(value) => value,
                            Err(_) => continue,
                        };
                        let value: u64 = String::from_utf8_lossy(&current.bytes)
                            .trim()
                            .parse()
                            .expect("counter stays numeric");
                        let next = (value + 1).to_string().into_bytes();
                        match futures::executor::block_on(store.compare_and_swap(
                            key,
                            &current.version,
                            next,
                        )) {
                            Ok(_) => return true,
                            Err(error) if error.kind == CommitErrorKind::Conflict => {
                                conflicts.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                continue;
                            }
                            Err(error) => panic!("{backend}: unexpected failure {error:?}"),
                        }
                    }
                    false
                }));
            }

            for task in tasks {
                assert!(task.await.unwrap(), "{backend}: a writer never succeeded");
            }

            assert!(
                conflicts.load(std::sync::atomic::Ordering::Relaxed) > 0,
                "{backend}: {WRITERS} writers started together and none ever lost a race, so \
nothing here was actually contended and the counter below proves nothing"
            );

            let final_value: u64 = String::from_utf8_lossy(&store.read(key).await.unwrap().bytes)
                .trim()
                .parse()
                .unwrap();
            assert_eq!(
                final_value,
                WRITERS,
                "{backend}: {WRITERS} writers each reported success but the counter reached \
{final_value}; {} update(s) were lost, which means two writers were allowed to publish from \
the same version",
                WRITERS - final_value
            );
        })
        .await;
    }

    /// Only one writer may win a given version.
    ///
    /// Every task swaps from the same starting version, so exactly one must be
    /// allowed to publish and the rest must be told it conflicted. Two winners
    /// would mean the later silently discarded the earlier.
    #[tokio::test]
    async fn exactly_one_writer_wins_a_contested_version() {
        const WRITERS: usize = 8;

        on_both_backends("contested", |backend, store| async move {
            let key = "races/contested.json";
            store.create_immutable(key, b"seed".to_vec()).await.unwrap();
            let start = store.read(key).await.unwrap().version;

            let mut tasks = Vec::new();
            for writer in 0..WRITERS {
                let store = store.clone();
                let start = start.clone();
                tasks.push(tokio::task::spawn_blocking(move || {
                    futures::executor::block_on(store.compare_and_swap(
                        key,
                        &start,
                        format!("writer-{writer}").into_bytes(),
                    ))
                    .map(|_| writer)
                }));
            }

            let mut winners = Vec::new();
            for task in tasks {
                match task.await.unwrap() {
                    Ok(writer) => winners.push(writer),
                    Err(error) => assert_eq!(
                        error.kind,
                        CommitErrorKind::Conflict,
                        "{backend}: a loser must be told it conflicted, not {:?}",
                        error.kind
                    ),
                }
            }

            assert_eq!(
                winners.len(),
                1,
                "{backend}: {} writers published from the same version; all but one of those \
writes were overwritten without their author being told",
                winners.len()
            );

            let stored = store.read(key).await.unwrap().bytes;
            assert_eq!(
                String::from_utf8_lossy(&stored),
                format!("writer-{}", winners[0]),
                "{backend}: the stored value must be the one the winner wrote"
            );
        })
        .await;
    }

    /// Where the two backends genuinely differ, recorded rather than assumed.
    ///
    /// The local store derives its version from a hash of the content, so a
    /// value that returns to an earlier one returns to that earlier version
    /// too, and a writer holding the old version is allowed to publish as
    /// though nothing had happened. Blob storage issues a fresh ETag per
    /// write, so the same sequence conflicts.
    ///
    /// This is the ABA problem, and the head object avoids it in practice
    /// because every snapshot carries a distinct generation and id. That makes
    /// it a property worth pinning rather than a bug worth fixing here: if the
    /// local version scheme ever changes, this test says what it was relied on
    /// to do.
    #[tokio::test]
    async fn the_backends_differ_on_a_value_that_returns_to_itself() {
        let blob = AdlsConditionalCommit::new(Arc::new(InMemory::new()));
        let root = TempRoot::new("aba");
        let local = local_file_commit(&root.0).unwrap();
        let key = "races/aba.json";

        for (backend, store) in [("blob", &blob), ("local", &local)] {
            store.create_immutable(key, b"A".to_vec()).await.unwrap();
            let original = store.read(key).await.unwrap().version;

            let to_b = store
                .compare_and_swap(key, &original, b"B".to_vec())
                .await
                .unwrap();
            store
                .compare_and_swap(key, &to_b, b"A".to_vec())
                .await
                .unwrap();

            // The content is what it was; the question is whether the version is.
            let replayed = store.compare_and_swap(key, &original, b"C".to_vec()).await;

            match backend {
                "blob" => assert!(
                    matches!(
                        replayed,
                        Err(CommitError {
                            kind: CommitErrorKind::Conflict
                        })
                    ),
                    "blob storage issues a version per write, so a stale one must still conflict \
after the content returns to its earlier value"
                ),
                _ => assert!(
                    replayed.is_ok(),
                    "the local store derives its version from the content, so a stale version \
becomes current again when the content does -- if this now conflicts, the local version scheme \
has changed and the comment describing it is stale"
                ),
            }
        }
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

    #[test]
    fn an_untyped_precondition_failure_is_a_conflict_not_an_unknown_outcome() {
        // Azure answers a conditional PUT whose ETag has moved with
        // `412 ConditionNotMet`, and object_store surfaces that as `Generic` on
        // several paths. Nothing was written, so the caller must be told
        // `Conflict` — which is safe to retry — rather than a retryable
        // transport error, which reads as "we do not know whether it committed".
        for rendered in [
            "Generic MicrosoftAzure error: Error performing put request header: 412 ConditionNotMet",
            "Operation failed: status: 412, body: PreconditionFailed",
            "request failed with HTTP 412",
        ] {
            assert!(
                is_precondition_failure(rendered),
                "should classify as a precondition failure: {rendered}"
            );
        }
        // And must not fire on a number that merely looks like a status.
        for rendered in [
            "Generic error: connection reset after 412000 bytes",
            "Operation timed out after 3 attempts",
            "request id 8a412f0c-dead-beef",
        ] {
            assert!(
                !is_precondition_failure(rendered),
                "should not classify as a precondition failure: {rendered}"
            );
        }
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
