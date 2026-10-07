//! The one store a deployment chooses to keep Kaveon's own metadata in.
//!
//! Every catalog definition, schema, table, statistic, cube and product record
//! is written here. Table *data* may live anywhere a catalog definition points —
//! another account, another cloud, a local directory — but the record of what
//! exists and what has been measured about it belongs to the store named once,
//! at deployment, by `KAVEON_SYSTEM_STORAGE`.
//!
//! The address is a URL so one value carries the scheme, the location and the
//! prefix together, and so adding a provider does not add three more
//! environment variables:
//!
//! ```text
//! adls://<account>/<container>[/<prefix>]
//! s3://<bucket>[/<prefix>]
//! file:///<absolute path>
//! ```
//!
//! Credentials never appear in the URL. ADLS uses workload identity, falling
//! back to managed identity; S3 uses the provider's standard chain; `file://`
//! needs none.

use std::path::PathBuf;
use std::sync::Arc;

use object_store::{ObjectStore, aws::AmazonS3Builder};

use crate::adls_commit::{AdlsConditionalCommit, local_file_commit, workload_identity_adls_commit};

/// Where a deployment keeps its own metadata, parsed from one configured URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SystemStorageLocation {
    Adls {
        account: String,
        container: String,
        prefix: String,
    },
    S3 {
        bucket: String,
        prefix: String,
    },
    File {
        root: PathBuf,
    },
}

impl SystemStorageLocation {
    /// Parses `KAVEON_SYSTEM_STORAGE`. Errors name the expected shape rather
    /// than echoing the value, which may carry an account name.
    pub fn parse(value: &str) -> Result<Self, String> {
        let value = value.trim();
        if let Some(rest) = value.strip_prefix("adls://") {
            let (account, rest) = split_once_trimmed(rest)
                .ok_or_else(|| "adls:// needs <account>/<container>[/<prefix>]".to_owned())?;
            let (container, prefix) = match split_once_trimmed(rest) {
                Some((container, prefix)) => (container, prefix),
                None => (rest, ""),
            };
            if account.is_empty() || container.is_empty() {
                return Err("adls:// needs <account>/<container>[/<prefix>]".to_owned());
            }
            return Ok(Self::Adls {
                account: account.to_owned(),
                container: container.to_owned(),
                prefix: normalize_prefix(prefix)?,
            });
        }
        if let Some(rest) = value.strip_prefix("s3://") {
            let (bucket, prefix) = match split_once_trimmed(rest) {
                Some((bucket, prefix)) => (bucket, prefix),
                None => (rest, ""),
            };
            if bucket.is_empty() {
                return Err("s3:// needs <bucket>[/<prefix>]".to_owned());
            }
            return Ok(Self::S3 {
                bucket: bucket.to_owned(),
                prefix: normalize_prefix(prefix)?,
            });
        }
        if let Some(rest) = value.strip_prefix("file://") {
            let path = rest.strip_prefix('/').unwrap_or(rest);
            if path.is_empty() {
                return Err("file:// needs an absolute path".to_owned());
            }
            // `file:///data/x` on POSIX and `file:///C:/data/x` on Windows both
            // arrive here with the leading slash already removed; a Windows
            // drive letter is its own root, a POSIX path needs its slash back.
            let looks_like_windows_root = path.as_bytes().get(1).is_some_and(|byte| *byte == b':');
            let root = if looks_like_windows_root {
                PathBuf::from(path)
            } else {
                PathBuf::from(format!("/{path}"))
            };
            return Ok(Self::File { root });
        }
        Err("system storage must be adls://, s3:// or file://".to_owned())
    }

    /// The prefix every object written by this deployment sits under. Empty for
    /// `file://`, where the root path plays that part.
    #[must_use]
    pub fn prefix(&self) -> &str {
        match self {
            Self::Adls { prefix, .. } | Self::S3 { prefix, .. } => prefix,
            Self::File { .. } => "",
        }
    }

    /// A short, credential-free description for startup logs and errors.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Adls {
                account,
                container,
                prefix,
            } => format!("adls://{account}/{container}/{prefix}"),
            Self::S3 { bucket, prefix } => format!("s3://{bucket}/{prefix}"),
            Self::File { root } => format!("file://{}", root.display()),
        }
    }

    /// Opens the conditional-commit store this location describes.
    ///
    /// The commit protocol is identical across providers: it needs only
    /// create-if-absent and compare-and-swap, which all three support.
    pub fn open(&self) -> Result<AdlsConditionalCommit, String> {
        match self {
            Self::Adls {
                account, container, ..
            } => workload_identity_adls_commit(account, container),
            Self::S3 { bucket, .. } => {
                if bucket.trim() != bucket
                    || !(3..=63).contains(&bucket.len())
                    || !bucket.bytes().all(|byte| {
                        byte.is_ascii_lowercase()
                            || byte.is_ascii_digit()
                            || byte == b'-'
                            || byte == b'.'
                    })
                    || bucket.starts_with('-')
                    || bucket.starts_with('.')
                    || bucket.ends_with('-')
                    || bucket.ends_with('.')
                {
                    return Err("S3 bucket must be a normalized bucket name".to_owned());
                }
                // Credentials come from the provider's own chain — environment,
                // web identity, instance profile — never from configuration we
                // store or log.
                let store = AmazonS3Builder::from_env()
                    .with_bucket_name(bucket)
                    .with_conditional_put(object_store::aws::S3ConditionalPut::ETagMatch)
                    .build()
                    .map_err(|error| format!("S3 system storage is not usable: {error}"))?;
                Ok(AdlsConditionalCommit::new(
                    Arc::new(store) as Arc<dyn ObjectStore>
                ))
            }
            Self::File { root } => local_file_commit(root),
        }
    }
}

fn split_once_trimmed(value: &str) -> Option<(&str, &str)> {
    value.split_once('/')
}

/// Keeps a prefix to the same shape the object paths use: no leading or
/// trailing slash, no empty or relative segment.
fn normalize_prefix(value: &str) -> Result<String, String> {
    let trimmed = value.trim_matches('/');
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if trimmed
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err("system storage prefix must not contain empty or relative segments".to_owned());
    }
    Ok(trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_scheme_with_and_without_a_prefix() {
        assert_eq!(
            SystemStorageLocation::parse("adls://kaveonlake/product/kaveon/system").unwrap(),
            SystemStorageLocation::Adls {
                account: "kaveonlake".into(),
                container: "product".into(),
                prefix: "kaveon/system".into(),
            }
        );
        assert_eq!(
            SystemStorageLocation::parse("adls://kaveonlake/product").unwrap(),
            SystemStorageLocation::Adls {
                account: "kaveonlake".into(),
                container: "product".into(),
                prefix: String::new(),
            }
        );
        assert_eq!(
            SystemStorageLocation::parse("s3://kaveon-system/metadata").unwrap(),
            SystemStorageLocation::S3 {
                bucket: "kaveon-system".into(),
                prefix: "metadata".into(),
            }
        );
        assert_eq!(
            SystemStorageLocation::parse("file:///var/lib/kaveon/system").unwrap(),
            SystemStorageLocation::File {
                root: PathBuf::from("/var/lib/kaveon/system"),
            }
        );
    }

    #[test]
    fn a_prefix_is_normalized_and_never_relative() {
        assert_eq!(
            SystemStorageLocation::parse("adls://a1/c1//kaveon/system/")
                .unwrap()
                .prefix(),
            "kaveon/system"
        );
        // A relative segment would let a prefix escape the deployment's own
        // area of the container.
        assert!(SystemStorageLocation::parse("adls://a1/c1/../other").is_err());
        assert!(SystemStorageLocation::parse("s3://bucket/a/./b").is_err());
    }

    #[test]
    fn an_unusable_address_is_refused_without_echoing_it() {
        for value in [
            "",
            "kaveonlake/product",
            "https://kaveonlake.blob.core.windows.net/product",
            "adls://kaveonlake",
            "s3://",
            "file://",
        ] {
            let error = SystemStorageLocation::parse(value)
                .expect_err(&format!("{value:?} should not parse"));
            assert!(
                !error.contains("kaveonlake"),
                "the error must not echo the address: {error}"
            );
        }
    }

    #[test]
    fn the_description_carries_no_credential() {
        let described = SystemStorageLocation::parse("adls://kaveonlake/product/kaveon")
            .unwrap()
            .describe();
        assert_eq!(described, "adls://kaveonlake/product/kaveon");
    }
}
