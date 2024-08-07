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

use std::time::{Duration, Instant};

use api::v1::column::Values;
use api::v1::{Column, ColumnDataType, InsertRequest, InsertRequests, SemanticType};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use operator::req_convert::insert::ColumnToRow;

fn gen_request(rows: usize) -> InsertRequest {
    let string_values = (0..rows)
        .map(|_| "testtest".to_string())
        .collect::<Vec<_>>();
    let f64_values = (0..rows).map(|_| 1.0f64).collect::<Vec<_>>();
    let timestamp_millisecond_values = (0..rows).map(|r| r as i64).collect::<Vec<_>>();

    InsertRequest {
        table_name: "test".to_string(),
        columns: vec![
            Column {
                column_name: "h".to_string(),
                values: Some(Values {
                    string_values,
                    ..Default::default()
                }),
                semantic_type: SemanticType::Tag as i32,
                datatype: ColumnDataType::String as i32,
                ..Default::default()
            },
            Column {
                column_name: "a".to_string(),
                values: Some(Values {
                    f64_values,
                    ..Default::default()
                }),
                semantic_type: SemanticType::Field as i32,
                datatype: ColumnDataType::Float64 as i32,
                ..Default::default()
            },
            Column {
                column_name: "ts".to_string(),
                values: Some(Values {
                    timestamp_millisecond_values,
                    ..Default::default()
                }),
                semantic_type: SemanticType::Timestamp as i32,
                datatype: ColumnDataType::TimestampMillisecond as i32,
                ..Default::default()
            },
        ],
        row_count: rows as u32,
    }
}
fn bench_column_converter(c: &mut Criterion) {
    let insert = gen_request(1024);

    c.bench_function("ColumnToRow::convert", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::from_nanos(0);
            for _i in 0..iters {
                let request = insert.clone();
                let start = Instant::now();
                black_box(ColumnToRow::convert(InsertRequests {
                    inserts: vec![request],
                }))
                .unwrap();
                total += start.elapsed();
            }
            total
        })
    });
}

criterion_group!(benches, bench_column_converter);
criterion_main!(benches);
