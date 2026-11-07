//! The bucket: the files no query reads, kept in S3-compatible object storage.

use std::sync::Arc;

use bytes::Bytes;
use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::path::Path;
use object_store::{ObjectStoreExt, PutPayload};

pub use object_store::{Error, Result};

pub mod key;

/// Where the bucket is, and the key that opens it.
#[derive(Debug, Clone)]
pub struct Config {
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    pub access_key_id: String,
    pub secret_access_key: String,
}

#[derive(Clone)]
pub struct Bucket {
    store: Arc<AmazonS3>,
}

impl Bucket {
    pub fn new(config: &Config) -> Result<Self> {
        let store = AmazonS3Builder::new()
            .with_endpoint(&config.endpoint)
            .with_region(&config.region)
            .with_bucket_name(&config.bucket)
            .with_access_key_id(&config.access_key_id)
            .with_secret_access_key(&config.secret_access_key)
            .build()?;
        Ok(Self {
            store: Arc::new(store),
        })
    }

    /// The object at `key`, `None` when there is none.
    pub async fn get(&self, key: &str) -> Result<Option<Bytes>> {
        match self.store.get(&Path::from(key)).await {
            Ok(object) => object.bytes().await.map(Some),
            Err(Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub async fn contains(&self, key: &str) -> Result<bool> {
        match self.store.head(&Path::from(key)).await {
            Ok(_) => Ok(true),
            Err(Error::NotFound { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }

    pub async fn put(&self, key: &str, bytes: impl Into<Bytes>) -> Result<()> {
        self.store
            .put(&Path::from(key), PutPayload::from(bytes.into()))
            .await
            .map(drop)
    }
}
