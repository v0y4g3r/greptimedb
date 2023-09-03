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

use common_query::Output;
use common_telemetry::error;
use store_api::logstore::LogStore;
use store_api::storage::RegionId;
use tokio::sync::oneshot;

use crate::compaction::CompactionRequest;
use crate::error::{RegionNotFoundSnafu, Result};
use crate::manifest::action::{RegionEdit, RegionMetaAction, RegionMetaActionList};
use crate::region::MitoRegionRef;
use crate::request::{CompactionFailed, CompactionFinished};
use crate::worker::RegionWorkerLoop;

impl<S: LogStore> RegionWorkerLoop<S> {
    /// Handles compaction finished, update region version and manifest, deleted compacted files.
    pub(crate) async fn handle_compaction_finished(
        &mut self,
        region_id: RegionId,
        mut request: CompactionFinished,
    ) {
        let Some(region) = self.regions.get_region(region_id) else {
            request.on_failure(RegionNotFoundSnafu { region_id }.build());
            return;
        };

        // Write region edit to manifest.
        let edit = RegionEdit {
            files_to_add: std::mem::take(&mut request.compaction_outputs),
            files_to_remove: std::mem::take(&mut request.compacted_files),
            compaction_time_window: None, // TODO(hl): update window maybe
            flushed_entry_id: None,
        };
        let action_list = RegionMetaActionList::with_action(RegionMetaAction::Edit(edit.clone()));
        if let Err(e) = region.manifest_manager.update(action_list).await {
            error!(e; "Failed to write manifest, region: {}", region_id);
            request.on_failure(e);
            return;
        }

        // Apply edit to region's version.
        region
            .version_control
            .apply_edit(edit, region.file_purger.clone());
    }

    pub(crate) async fn handle_compaction_failure(&mut self, _req: CompactionFailed) {
        todo!()
    }

    /// Creates a new compaction request.
    fn new_compaction_request(
        &self,
        region: &MitoRegionRef,
        waiters: Vec<oneshot::Sender<Result<Output>>>,
    ) -> CompactionRequest {
        let region_id = region.region_id;
        let region_metadata = region.metadata();
        let current_version = region.version_control.current().version;
        let access_layer = region.access_layer.clone();
        let file_purger = region.file_purger.clone();

        CompactionRequest {
            region_id,
            region_metadata,
            current_version,
            access_layer,
            ttl: None,                    // TODO(hl): get TTL info from region metadata
            compaction_time_window: None, // TODO(hl): get persisted region compaction time window
            request_sender: self.sender.clone(),
            waiters,
            file_purger,
        }
    }
}
