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
use common_telemetry::{error, info};
use tonic::codegen::tokio_stream::StreamExt;

use crate::statement::StatementExecutor;

impl StatementExecutor {
    pub async fn execute_kill(&self, process_id: u64) -> crate::error::Result<Output> {
        let Some(process_manager) = self.process_manager.as_ref() else {
            error!("Process manager is not initialized");
            return Ok(Output::new_with_affected_rows(0));
        };
        let mut stream = Box::pin(process_manager.list_all_processes().unwrap());
        let mut server = None;
        while let Some(process) = stream.next().await.transpose().unwrap() {
            if process.key.id == process_id {
                server = Some(process.server_addr().to_string());
            }
        }
        let Some(server) = server else {
            error!("process with id: {} not found", process_id);
return             Ok(Output::new_with_affected_rows(0));
        };
        process_manager
            .kill(server.clone(), process_id)
            .await
            .unwrap();
        info!(
            "Successfully killed process {} at server: {}",
            process_id, server
        );

        Ok(Output::new_with_affected_rows(0))
    }
}
