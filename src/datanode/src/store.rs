// Copyright 2023 Greptime Team
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! object storage utilities

mod azblob;
mod fs;
mod oss;
mod s3;

use std::path;
use std::sync::Arc;

use common_base::readable_size::ReadableSize;
use common_telemetry::logging::info;
use object_store::layers::{LoggingLayer, LruCacheLayer, MetricsLayer, RetryLayer, TracingLayer};
use object_store::services::Fs as FsBuilder;
use object_store::{util, ObjectStore, ObjectStoreBuilder};
use snafu::prelude::*;

use crate::datanode::{ObjectStoreConfig, DEFAULT_OBJECT_STORE_CACHE_SIZE};
use crate::error::{self, Result};

pub(crate) async fn new_object_store(store_config: &ObjectStoreConfig) -> Result<ObjectStore> {
    let object_store = match store_config {
        ObjectStoreConfig::File(file_config) => fs::new_fs_object_store(file_config).await,
        ObjectStoreConfig::S3(s3_config) => s3::new_s3_object_store(s3_config).await,
        ObjectStoreConfig::Oss(oss_config) => oss::new_oss_object_store(oss_config).await,
        ObjectStoreConfig::Azblob(azblob_config) => {
            azblob::new_azblob_object_store(azblob_config).await
        }
    }?;

    // Enable retry layer and cache layer for non-fs object storages
    let object_store = if !matches!(store_config, ObjectStoreConfig::File(..)) {
        let object_store = create_object_store_with_cache(object_store, store_config).await?;
        object_store.layer(RetryLayer::new().with_jitter())
    } else {
        object_store
    };

    Ok(object_store
        .layer(MetricsLayer)
        .layer(
            LoggingLayer::default()
                // Print the expected error only in DEBUG level.
                // See https://docs.rs/opendal/latest/opendal/layers/struct.LoggingLayer.html#method.with_error_level
                .with_error_level(Some("debug"))
                .expect("input error level must be valid"),
        )
        .layer(TracingLayer))
}

async fn create_object_store_with_cache(
    object_store: ObjectStore,
    store_config: &ObjectStoreConfig,
) -> Result<ObjectStore> {
    let (cache_path, cache_capacity) = match store_config {
        ObjectStoreConfig::S3(s3_config) => {
            let path = s3_config.cache_path.as_ref();
            let capacity = s3_config
                .cache_capacity
                .unwrap_or(DEFAULT_OBJECT_STORE_CACHE_SIZE);
            (path, capacity)
        }
        ObjectStoreConfig::Oss(oss_config) => {
            let path = oss_config.cache_path.as_ref();
            let capacity = oss_config
                .cache_capacity
                .unwrap_or(DEFAULT_OBJECT_STORE_CACHE_SIZE);
            (path, capacity)
        }
        ObjectStoreConfig::Azblob(azblob_config) => {
            let path = azblob_config.cache_path.as_ref();
            let capacity = azblob_config
                .cache_capacity
                .unwrap_or(DEFAULT_OBJECT_STORE_CACHE_SIZE);
            (path, capacity)
        }
        _ => (None, ReadableSize(0)),
    };

    if let Some(path) = cache_path {
        let path = util::normalize_dir(path);
        let atomic_temp_dir = format!("{path}.tmp/");
        clean_temp_dir(&atomic_temp_dir)?;
        let cache_store = FsBuilder::default()
            .root(&path)
            .atomic_write_dir(&atomic_temp_dir)
            .build()
            .context(error::InitBackendSnafu)?;

        let cache_layer = LruCacheLayer::new(Arc::new(cache_store), cache_capacity.0 as usize)
            .await
            .context(error::InitBackendSnafu)?;
        Ok(object_store.layer(cache_layer))
    } else {
        Ok(object_store)
    }
}

pub(crate) fn clean_temp_dir(dir: &str) -> Result<()> {
    if path::Path::new(&dir).exists() {
        info!("Begin to clean temp storage directory: {}", dir);
        std::fs::remove_dir_all(dir).context(error::RemoveDirSnafu { dir })?;
        info!("Cleaned temp storage directory: {}", dir);
    }

    Ok(())
}
