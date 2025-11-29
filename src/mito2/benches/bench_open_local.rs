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

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use mito2::config::MitoConfig;
use mito2::test_util::TestEnv;
use store_api::region_engine::RegionEngine;
use store_api::region_request::{PathType, RegionOpenRequest, RegionRequest};
use store_api::storage::{RegionId, ScanRequest};
use tokio_stream::StreamExt;

struct BenchSetup {
    engine: Arc<mito2::engine::MitoEngine>,
    region_id: RegionId,
    max_batches: usize,
}

async fn setup_region() -> BenchSetup {
    common_telemetry::init_default_ut_logging();
    
    // Read data home from environment variable
    let data_home = std::env::var("MITO2_BENCH_DATA_HOME")
        .expect("MITO2_BENCH_DATA_HOME environment variable must be set");
    let data_home_path = PathBuf::from_str(&data_home)
        .unwrap_or_else(|_| panic!("Invalid path in MITO2_BENCH_DATA_HOME: {}", data_home));
    
    // Read table id from environment variable
    let table_id = std::env::var("MITO2_BENCH_TABLE_ID")
        .expect("MITO2_BENCH_TABLE_ID environment variable must be set");
    let table_id = u32::from_str(&table_id)
        .unwrap_or_else(|_| panic!("Invalid table_id in MITO2_BENCH_TABLE_ID: {}", table_id));
    
    let mut env = TestEnv::with_data_home(either::Right(data_home_path)).await;
    let engine = env
        .create_engine(MitoConfig {
            default_experimental_flat_format: false,
            ..Default::default()
        })
        .await;
    let region_id = RegionId::new(table_id, 0);
    let engine = Arc::new(engine);
    let engine_cloned = engine.clone();

    // Construct table_dir from table_id
    let table_dir = format!("greptime/public/{}/", table_id);

    // Open the region once during setup
    engine_cloned
        .handle_request(
            region_id,
            RegionRequest::Open(RegionOpenRequest {
                engine: String::new(),
                table_dir,
                path_type: PathType::Data,
                options: [("physical_metric_table".to_owned(), "true".to_owned())]
                    .into_iter()
                    .collect(),
                skip_wal_replay: true,
                checkpoint: None,
            }),
        )
        .await
        .unwrap();

    // Wait a bit for the region to be fully opened
    tokio::time::sleep(Duration::from_millis(100)).await;

    BenchSetup {
        engine,
        region_id,
        max_batches: 10000,
    }
}

async fn scan_region_batches(setup: &BenchSetup) {
    // Only benchmark the scanning operation
    let request = ScanRequest::default();
    let stream = setup
        .engine
        .scan_to_stream(setup.region_id, request)
        .await
        .unwrap();
    let mut batch_cnt = 0;
    let mut stream = stream;
    while let Some(batch) = stream.next().await {
        if batch_cnt >= setup.max_batches {
            break;
        }
        let batch = batch.unwrap();
        black_box(batch);
        batch_cnt += 1;
    }
}

fn bench_open_local(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Set up the region once before benchmarking
    let setup = rt.block_on(setup_region());
    let setup = Arc::new(setup);

    c.bench_function("scan_region_batches", |b| {
        let setup = setup.clone();
        b.to_async(&rt).iter(|| async {
            scan_region_batches(&setup).await;
        });
    });
}

criterion_group!(benches, bench_open_local);
criterion_main!(benches);
