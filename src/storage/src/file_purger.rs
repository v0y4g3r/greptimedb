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

use std::sync::Arc;

use store_api::storage::RegionId;
use tokio::sync::Notify;

use crate::scheduler::rate_limit::{BoxedRateLimitToken, RateLimitToken};
use crate::scheduler::{Handler, LocalScheduler, Request};
use crate::sst::AccessLayerRef;

pub struct FilePurgeRequest {
    pub region_id: RegionId,
    pub file_name: String,
    pub sst_layer: AccessLayerRef,
}

impl Request for FilePurgeRequest {
    type Key = String;

    fn key(&self) -> Self::Key {
        format!("{}/{}", self.region_id, self.file_name)
    }
}

pub struct FilePurgeHandler;

#[async_trait::async_trait]
impl Handler for FilePurgeHandler {
    type Request = FilePurgeRequest;

    async fn handle_request(
        &self,
        req: Self::Request,
        token: BoxedRateLimitToken,
        finish_notifier: Arc<Notify>,
    ) -> crate::error::Result<()> {
        req.sst_layer.delete_sst(&req.file_name).await?;
        token.try_release();
        finish_notifier.notify_one();
        Ok(())
    }
}

pub type FilePurgerRef = Arc<LocalScheduler<FilePurgeRequest>>;

#[cfg(test)]
pub mod noop {
    use std::sync::Arc;

    use tokio::sync::Notify;

    use crate::file_purger::{FilePurgeRequest, FilePurgerRef};
    use crate::scheduler::rate_limit::{BoxedRateLimitToken, RateLimitToken};
    use crate::scheduler::{Handler, LocalScheduler, SchedulerConfig};

    pub fn new_noop_file_purger() -> FilePurgerRef {
        Arc::new(LocalScheduler::new(
            SchedulerConfig::default(),
            NoopFilePurgeHandler,
        ))
    }

    #[derive(Debug)]
    pub struct NoopFilePurgeHandler;

    #[async_trait::async_trait]
    impl Handler for NoopFilePurgeHandler {
        type Request = FilePurgeRequest;

        async fn handle_request(
            &self,
            _req: Self::Request,
            token: BoxedRateLimitToken,
            finish_notifier: Arc<Notify>,
        ) -> crate::error::Result<()> {
            token.try_release();
            finish_notifier.notify_one();
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use object_store::backend::fs::Builder;
    use object_store::ObjectStore;
    use store_api::storage::OpType;
    use tempdir::TempDir;

    use super::*;
    use crate::file_purger::noop::NoopFilePurgeHandler;
    use crate::memtable::tests::{schema_for_test, write_kvs};
    use crate::memtable::{DefaultMemtableBuilder, IterContext, MemtableBuilder};
    use crate::scheduler::SchedulerConfig;
    use crate::sst::{AccessLayer, FileHandle, FileMeta, FsAccessLayer, Source, WriteOptions};

    struct MockRateLimitToken;

    impl RateLimitToken for MockRateLimitToken {
        fn try_release(&self) {}
    }

    async fn create_sst_file(
        os: ObjectStore,
        sst_file_name: &str,
    ) -> (FileHandle, String, AccessLayerRef) {
        let schema = schema_for_test();
        let memtable = DefaultMemtableBuilder::default().build(schema.clone());

        write_kvs(
            &*memtable,
            10,
            OpType::Put,
            &[(1, 1), (2, 2)],
            &[(Some(1), Some(1)), (Some(2), Some(2))],
        );

        let iter = memtable.iter(&IterContext::default()).unwrap();
        let sst_path = "table1";
        let layer = Arc::new(FsAccessLayer::new(sst_path, os.clone()));
        let _sst_info = layer
            .write_sst(sst_file_name, Source::Iter(iter), &WriteOptions {})
            .await
            .unwrap();

        (
            FileHandle::new(
                FileMeta {
                    region_id: 0,
                    file_name: sst_file_name.to_string(),
                    time_range: None,
                    level: 0,
                },
                layer.clone(),
                Arc::new(LocalScheduler::new(
                    SchedulerConfig::default(),
                    NoopFilePurgeHandler,
                )),
            ),
            sst_path.to_string(),
            layer as _,
        )
    }

    #[tokio::test]
    async fn test_file_purger_handler() {
        let dir = TempDir::new("file-purge").unwrap();
        let object_store = ObjectStore::new(
            Builder::default()
                .root(dir.path().to_str().unwrap())
                .build()
                .unwrap(),
        );
        let sst_file_name = "test-file-purge-handler.parquet";

        let (file, path, layer) = create_sst_file(object_store.clone(), sst_file_name).await;
        let request = FilePurgeRequest {
            region_id: 0,
            file_name: file.file_name().to_string(),
            sst_layer: layer,
        };

        let handler = FilePurgeHandler;
        let notify = Arc::new(Notify::new());
        handler
            .handle_request(request, Box::new(MockRateLimitToken {}), notify.clone())
            .await
            .unwrap();

        notify.notified().await;

        let object = object_store.object(&format!("{}/{}", path, sst_file_name));
        assert!(!object.is_exist().await.unwrap());
    }

    #[tokio::test]
    async fn test_file_purge_loop() {
        let scheduler = LocalScheduler::new(SchedulerConfig::default(), FilePurgeHandler);
        // scheduler.schedule();
    }
}
