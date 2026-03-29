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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use arrow::array::{Float64Array, StringArray, TimestampMillisecondArray};
use arrow::datatypes::{DataType as ArrowDataType, Field, Schema as ArrowSchema, TimeUnit};
use arrow::record_batch::RecordBatch;
use criterion::{BenchmarkId, Criterion, black_box};
use metric_engine::batch_modifier::modify_batch_sparse;
use servers::pending_rows_batcher::{TableBatch, columns_taxonomy};

/// Build a RecordBatch with the given tag names, each filled with `num_rows` rows.
fn build_batch(num_rows: usize, tag_names: &[String]) -> RecordBatch {
    let mut fields = vec![
        Field::new(
            "greptime_timestamp",
            ArrowDataType::Timestamp(TimeUnit::Millisecond, None),
            false,
        ),
        Field::new("greptime_value", ArrowDataType::Float64, true),
    ];
    for name in tag_names {
        fields.push(Field::new(name.as_str(), ArrowDataType::Utf8, true));
    }

    let schema = Arc::new(ArrowSchema::new(fields));

    let mut columns: Vec<Arc<dyn arrow::array::Array>> = vec![
        Arc::new(TimestampMillisecondArray::from(vec![1000i64; num_rows])),
        Arc::new(Float64Array::from(vec![1.0; num_rows])),
    ];
    for _ in tag_names {
        columns.push(Arc::new(StringArray::from(vec!["val"; num_rows])));
    }

    RecordBatch::try_new(schema, columns).unwrap()
}

/// Generate tag names: tag_0, tag_1, ..., tag_{n-1}.
fn tag_names(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("tag_{}", i)).collect()
}

/// Build a name_to_ids map for the given tag names.
fn build_name_to_ids(names: &[String]) -> HashMap<String, u32> {
    names
        .iter()
        .enumerate()
        .map(|(i, name)| (name.clone(), i as u32 + 10))
        .collect()
}

// ---------------------------------------------------------------------------
// Benchmark: columns_taxonomy with varying tag counts
// ---------------------------------------------------------------------------
pub fn bench_columns_taxonomy(c: &mut Criterion) {
    let mut group = c.benchmark_group("columns_taxonomy");
    let partition_columns: HashSet<&str> = HashSet::new();

    for &num_tags in &[5, 20, 50, 100] {
        let names = tag_names(num_tags);
        let name_to_ids = build_name_to_ids(&names);
        let batch = build_batch(1, &names);
        let batch_schema = batch.schema();

        group.bench_with_input(BenchmarkId::new("tags", num_tags), &num_tags, |b, _| {
            b.iter(|| {
                black_box(
                    columns_taxonomy(
                        black_box(&batch_schema),
                        black_box("test_table"),
                        black_box(&name_to_ids),
                        black_box(&partition_columns),
                    )
                    .unwrap(),
                );
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark: modify_batch_sparse with varying tag counts and row counts
// ---------------------------------------------------------------------------
pub fn bench_modify_batch_sparse(c: &mut Criterion) {
    let mut group = c.benchmark_group("modify_batch_sparse");
    let partition_columns: HashSet<&str> = HashSet::new();

    for &num_tags in &[5, 20, 50] {
        for &num_rows in &[100, 1000] {
            let names = tag_names(num_tags);
            let name_to_ids = build_name_to_ids(&names);
            let batch = build_batch(num_rows, &names);
            let batch_schema = batch.schema();

            let (tag_columns, essential_indices) = columns_taxonomy(
                &batch_schema,
                "test_table",
                &name_to_ids,
                &partition_columns,
            )
            .unwrap();

            group.bench_with_input(
                BenchmarkId::new("rows_tags", format!("R{}_T{}", num_rows, num_tags)),
                &(num_rows, num_tags),
                |b, _| {
                    b.iter(|| {
                        black_box(
                            modify_batch_sparse(
                                black_box(batch.clone()),
                                black_box(42),
                                black_box(&tag_columns),
                                black_box(&essential_indices),
                            )
                            .unwrap(),
                        );
                    });
                },
            );
        }
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark: per-batch taxonomy + transform (the hot loop from flush)
//
// This is the critical path changed by the fix: columns_taxonomy is now
// called once per batch instead of once per table, so we measure the
// combined cost of taxonomy + modify_batch_sparse per batch.
// ---------------------------------------------------------------------------
pub fn bench_per_batch_transform(c: &mut Criterion) {
    let mut group = c.benchmark_group("per_batch_transform");
    let partition_columns: HashSet<&str> = HashSet::new();

    for &num_tags in &[10, 50] {
        for &batches_per_table in &[1, 5, 20] {
            let names = tag_names(num_tags);
            let name_to_ids = build_name_to_ids(&names);
            let rows_per_batch = 100;

            // All batches share the same schema (uniform case).
            let batches: Vec<RecordBatch> = (0..batches_per_table)
                .map(|_| build_batch(rows_per_batch, &names))
                .collect();

            let table_batch = TableBatch {
                table_name: "test_table".to_string(),
                table_id: 1,
                batches,
                row_count: rows_per_batch * batches_per_table,
            };

            group.bench_with_input(
                BenchmarkId::new(
                    "uniform_tags",
                    format!("T{}_B{}", num_tags, batches_per_table),
                ),
                &table_batch,
                |b, table_batch| {
                    b.iter(|| {
                        let mut modified = Vec::with_capacity(table_batch.batches.len());
                        for batch in &table_batch.batches {
                            let schema = batch.schema();
                            let (tag_cols, essential_idx) = columns_taxonomy(
                                &schema,
                                &table_batch.table_name,
                                &name_to_ids,
                                &partition_columns,
                            )
                            .unwrap();
                            modified.push(
                                modify_batch_sparse(
                                    batch.clone(),
                                    table_batch.table_id,
                                    &tag_cols,
                                    &essential_idx,
                                )
                                .unwrap(),
                            );
                        }
                        black_box(&modified);
                    });
                },
            );
        }
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark: diverse tag sets across batches within a single table
//
// This is the scenario the bug fix addresses. Each batch has a different
// subset of tags (simulating schema evolution between submits).
// ---------------------------------------------------------------------------
pub fn bench_diverse_tag_sets(c: &mut Criterion) {
    let mut group = c.benchmark_group("diverse_tag_sets");
    let partition_columns: HashSet<&str> = HashSet::new();
    let rows_per_batch = 100;

    for &num_batches in &[2, 5, 10] {
        // Each batch gets an incrementally larger tag set:
        // batch 0: [tag_0..tag_4]
        // batch 1: [tag_0..tag_5]
        // ...
        // batch N: [tag_0..tag_{4+N}]
        let base_tags = 5;
        let all_names = tag_names(base_tags + num_batches);
        let name_to_ids = build_name_to_ids(&all_names);

        let batches: Vec<RecordBatch> = (0..num_batches)
            .map(|i| {
                let batch_tags = &all_names[..base_tags + i];
                build_batch(rows_per_batch, batch_tags)
            })
            .collect();

        let table_batch = TableBatch {
            table_name: "evolving_table".to_string(),
            table_id: 99,
            batches,
            row_count: rows_per_batch * num_batches,
        };

        group.bench_with_input(
            BenchmarkId::new("batches", num_batches),
            &table_batch,
            |b, table_batch| {
                b.iter(|| {
                    let mut modified = Vec::with_capacity(table_batch.batches.len());
                    for batch in &table_batch.batches {
                        let schema = batch.schema();
                        let (tag_cols, essential_idx) = columns_taxonomy(
                            &schema,
                            &table_batch.table_name,
                            &name_to_ids,
                            &partition_columns,
                        )
                        .unwrap();
                        modified.push(
                            modify_batch_sparse(
                                batch.clone(),
                                table_batch.table_id,
                                &tag_cols,
                                &essential_idx,
                            )
                            .unwrap(),
                        );
                    }
                    black_box(&modified);
                });
            },
        );
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark: multiple tables (simulates a flush with many logical tables)
// ---------------------------------------------------------------------------
pub fn bench_multi_table_transform(c: &mut Criterion) {
    let mut group = c.benchmark_group("multi_table_transform");
    let partition_columns: HashSet<&str> = HashSet::new();
    let num_tags = 10;
    let rows_per_batch = 100;
    let batches_per_table = 3;

    let names = tag_names(num_tags);
    let name_to_ids = build_name_to_ids(&names);

    for &num_tables in &[10, 50, 200] {
        let table_batches: Vec<TableBatch> = (0..num_tables)
            .map(|t| {
                let batches: Vec<RecordBatch> = (0..batches_per_table)
                    .map(|_| build_batch(rows_per_batch, &names))
                    .collect();
                TableBatch {
                    table_name: format!("table_{}", t),
                    table_id: (t + 1) as u32,
                    batches,
                    row_count: rows_per_batch * batches_per_table,
                }
            })
            .collect();

        group.bench_with_input(
            BenchmarkId::new("tables", num_tables),
            &table_batches,
            |b, table_batches| {
                b.iter(|| {
                    let mut modified = Vec::new();
                    for table_batch in table_batches {
                        for batch in &table_batch.batches {
                            let schema = batch.schema();
                            let (tag_cols, essential_idx) = columns_taxonomy(
                                &schema,
                                &table_batch.table_name,
                                &name_to_ids,
                                &partition_columns,
                            )
                            .unwrap();
                            modified.push(
                                modify_batch_sparse(
                                    batch.clone(),
                                    table_batch.table_id,
                                    &tag_cols,
                                    &essential_idx,
                                )
                                .unwrap(),
                            );
                        }
                    }
                    black_box(&modified);
                });
            },
        );
    }
    group.finish();
}

criterion::criterion_group!(
    benches,
    bench_columns_taxonomy,
    bench_modify_batch_sparse,
    bench_per_batch_transform,
    bench_diverse_tag_sets,
    bench_multi_table_transform,
);
