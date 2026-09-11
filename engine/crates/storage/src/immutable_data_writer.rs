//! Bounded create-only writer for immutable Parquet data objects.
//!
//! This is a preparation primitive: it writes one immutable object and returns
//! its verified reference. It does not publish a table head or perform a
//! transaction commit. Callers must publish the returned reference through a
//! catalog snapshot CAS.

use crate::{AdlsConditionalCommit, CommitErrorKind};
use sha2::{Digest, Sha256};

pub const DEFAULT_MAX_PARQUET_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImmutableDataReference {
    pub path: String,
    pub sha256: String,
    pub size_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataWriteError {
    Invalid(String),
    Conflict,
    Storage(CommitErrorKind),
}

#[derive(Clone)]
pub struct ImmutableParquetWriter {
    storage: AdlsConditionalCommit,
    max_bytes: usize,
}

impl ImmutableParquetWriter {
    pub fn new(storage: AdlsConditionalCommit, max_bytes: usize) -> Result<Self, DataWriteError> {
        if max_bytes == 0 {
            return Err(DataWriteError::Invalid(
                "maximum object size must be positive".into(),
            ));
        }
        Ok(Self { storage, max_bytes })
    }

    pub fn with_default_limit(storage: AdlsConditionalCommit) -> Self {
        Self {
            storage,
            max_bytes: DEFAULT_MAX_PARQUET_BYTES,
        }
    }

    /// Creates or verifies one immutable Parquet object. Existing objects are
    /// accepted only when their bytes match exactly; they are never replaced.
    pub async fn write(
        &self,
        path: &str,
        bytes: Vec<u8>,
    ) -> Result<ImmutableDataReference, DataWriteError> {
        if !path.ends_with(".parquet") {
            return Err(DataWriteError::Invalid(
                "data path must end in .parquet".into(),
            ));
        }
        if bytes.len() < 8 || &bytes[..4] != b"PAR1" || &bytes[bytes.len() - 4..] != b"PAR1" {
            return Err(DataWriteError::Invalid(
                "object is not a Parquet byte stream".into(),
            ));
        }
        self.write_immutable(path, bytes).await
    }

    /// Creates or verifies a bounded immutable manifest object. The catalog
    /// validates its JSON/reference shape before publication.
    pub async fn write_manifest(
        &self,
        path: &str,
        bytes: Vec<u8>,
    ) -> Result<ImmutableDataReference, DataWriteError> {
        if !path.ends_with(".json") {
            return Err(DataWriteError::Invalid(
                "manifest path must end in .json".into(),
            ));
        }
        self.write_immutable(path, bytes).await
    }

    async fn write_immutable(
        &self,
        path: &str,
        bytes: Vec<u8>,
    ) -> Result<ImmutableDataReference, DataWriteError> {
        if bytes.len() > self.max_bytes {
            return Err(DataWriteError::Invalid(
                "immutable data exceeds size limit".into(),
            ));
        }
        let sha256 = digest(&bytes);
        match self.storage.create_immutable(path, bytes.clone()).await {
            Ok(_) => Ok(reference(path, sha256, bytes.len())),
            Err(error) if error.kind == CommitErrorKind::Conflict => {
                let existing = self
                    .storage
                    .read_bounded(path, self.max_bytes)
                    .await
                    .map_err(|error| DataWriteError::Storage(error.kind))?;
                if existing.bytes == bytes {
                    Ok(reference(path, sha256, bytes.len()))
                } else {
                    Err(DataWriteError::Conflict)
                }
            }
            Err(error) => Err(DataWriteError::Storage(error.kind)),
        }
    }
}

fn reference(path: &str, sha256: String, size_bytes: usize) -> ImmutableDataReference {
    ImmutableDataReference {
        path: path.to_owned(),
        sha256,
        size_bytes,
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use object_store::memory::InMemory;

    use super::*;

    fn parquet(payload: &[u8]) -> Vec<u8> {
        let mut bytes = b"PAR1".to_vec();
        bytes.extend_from_slice(payload);
        bytes.extend_from_slice(b"PAR1");
        bytes
    }

    fn writer(limit: usize) -> ImmutableParquetWriter {
        ImmutableParquetWriter::new(AdlsConditionalCommit::new(Arc::new(InMemory::new())), limit)
            .unwrap()
    }

    #[tokio::test]
    async fn writes_verified_immutable_object_and_reuses_identical_bytes() {
        let writer = writer(1024);
        let bytes = parquet(b"rows");
        let first = writer
            .write("objects/one.parquet", bytes.clone())
            .await
            .unwrap();
        let second = writer.write("objects/one.parquet", bytes).await.unwrap();
        assert_eq!(first, second);
        assert_eq!(first.size_bytes, 12);
    }

    #[tokio::test]
    async fn rejects_conflicting_replacement_and_invalid_payloads() {
        let writer = writer(1024);
        writer
            .write("objects/one.parquet", parquet(b"first"))
            .await
            .unwrap();
        assert_eq!(
            writer
                .write("objects/one.parquet", parquet(b"second"))
                .await,
            Err(DataWriteError::Conflict)
        );
        assert!(matches!(
            writer.write("objects/one.txt", parquet(b"x")).await,
            Err(DataWriteError::Invalid(_))
        ));
        assert!(matches!(
            writer
                .write("objects/bad.parquet", b"not parquet".to_vec())
                .await,
            Err(DataWriteError::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn size_failure_does_not_publish_an_object() {
        let writer = writer(8);
        let error = writer
            .write("objects/large.parquet", parquet(b"too large"))
            .await;
        assert!(matches!(error, Err(DataWriteError::Invalid(_))));
    }
}
