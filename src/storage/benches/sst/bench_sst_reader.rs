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

use common_time::range::TimestampRange;
use common_time::timestamp::TimeUnit;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use datatypes::prelude::ScalarVector;
use datatypes::vectors::TimestampMillisecondVector;
use object_store::backend::fs::Builder;
use object_store::ObjectStore;
use storage::memtable;
use storage::read::BatchReader;
use storage::schema::{ProjectedSchema, ProjectedSchemaRef};
use storage::sst::parquet::{ParquetReader, ParquetWriter};
use table::predicate::Predicate;

async fn read_no_predicate(
    range: TimestampRange,
    object_store: ObjectStore,
    schema: ProjectedSchemaRef,
    ts_idx: usize,
) -> usize {
    let reader = ParquetReader::new_without_time_range(
        "./test-read.parquet",
        object_store.clone(),
        schema.clone(),
        Predicate::empty(),
    );
    let mut s = reader.chunk_stream().await.unwrap();
    let mut size = 0;
    while let Some(b) = s.next_batch().await.unwrap() {
        let ts_col = b.column(ts_idx);
        let ts_col = ts_col
            .as_any()
            .downcast_ref::<TimestampMillisecondVector>()
            .unwrap();
        let i = ts_col
            .iter_data()
            .filter(|ts| range.contains(&ts.as_ref().unwrap().0))
            .count();
        size += i;
    }
    size
}

fn test_read_no_predicate(c: &mut Criterion) {
    let size: usize = 1024;

    common_telemetry::init_default_ut_logging();
    let schema = memtable::tests::schema_for_test();
    let backend = Builder::default()
        .root("/Users/lei/parquet")
        .build()
        .unwrap();
    let object_store = ObjectStore::new(backend);

    let projected_schema = Arc::new(ProjectedSchema::new(schema, None).unwrap());

    let ts_idx = projected_schema
        .projected_user_schema()
        .timestamp_index()
        .unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_with_input(
        BenchmarkId::new("read_parquet", "no_predicate"),
        &(projected_schema, object_store, ts_idx),
        |b, (projected_schema, object_store, ts_idx)| {
            b.to_async(&rt).iter(|| async move {
                let res = read_no_predicate(
                    TimestampRange::with_unit(1000, 2000, TimeUnit::Millisecond).unwrap(),
                    object_store.clone(),
                    projected_schema.clone(),
                    *ts_idx,
                )
                .await;
                assert_eq!(1000, res);
            });
        },
    );
}

criterion_group!(benches, test_read_no_predicate);
criterion_main!(benches);
