use criterion::{criterion_group, criterion_main, Criterion};
use datafusion::arrow::array::StringArray;
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::physical_plan::memory::MemoryExec;
use datafusion::prelude::SessionContext;
use std::sync::Arc;

use promql::extension_plan::series_divide::SeriesDivideExec;

fn generate_test_data(size: usize, series_count: usize) -> MemoryExec {
    let schema = Arc::new(Schema::new(vec![
        Field::new("host", DataType::Utf8, true),
        Field::new("path", DataType::Utf8, true),
    ]));

    let mut host_values = Vec::with_capacity(size);
    let mut path_values = Vec::with_capacity(size);
    
    let rows_per_series = size / series_count;
    for series_idx in 0..series_count {
        let host = format!("host_{:03}", series_idx);
        let path = format!("path_{:03}", series_idx);
        for _ in 0..rows_per_series {
            host_values.push(host.clone());
            path_values.push(path.clone());
        }
    }

    let host_array = Arc::new(StringArray::from(host_values)) as _;
    let path_array = Arc::new(StringArray::from(path_values)) as _;
    
    let batch = RecordBatch::try_new(schema.clone(), vec![host_array, path_array]).unwrap();
    MemoryExec::try_new(&[vec![batch]], schema, None).unwrap()
}

fn bench_series_divide(c: &mut Criterion) {
    let mut group = c.benchmark_group("series_divide");
    
    // Test different data sizes
    for size in [1000, 10000, 100000].iter() {
        // Test different series counts
        for series_ratio in [10, 100, 1000].iter() {
            let series_count = size / series_ratio;
            group.bench_function(
                format!("size_{}_series_{}", size, series_count),
                |b| {
                    b.iter_with_setup(
                        || {
                            let memory_exec = Arc::new(generate_test_data(*size, series_count));
                            let divide_exec = Arc::new(SeriesDivideExec {
                                tag_columns: vec!["host".to_string(), "path".to_string()],
                                input: memory_exec,
                                metric: datafusion::physical_plan::metrics::ExecutionPlanMetricsSet::new(),
                            });
                            let session_context = SessionContext::default();
                            (divide_exec, session_context)
                        },
                        |(divide_exec, session_context)| {
                            futures::executor::block_on(async {
                                let _ = datafusion::physical_plan::collect(
                                    divide_exec,
                                    session_context.task_ctx(),
                                )
                                .await
                                .unwrap();
                            });
                        },
                    )
                },
            );
        }
    }
    group.finish();
}

criterion_group!(benches, bench_series_divide);
criterion_main!(benches);
