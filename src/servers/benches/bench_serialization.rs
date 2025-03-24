use arrow_ipc::writer::IpcWriteOptions;
// bench serialization size of insert request
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use datafusion::parquet::basic::ZstdLevel;
use datafusion::parquet::file::properties::WriterProperties;
use prost::Message;
use servers::grpc::test_serialization_size::{
    build_insert_request, gzip_compress, insert_request_to_arrow_ipc, insert_request_to_parquet,
    insert_request_to_record_batch,
};

fn bench_insert_request_serialization(c: &mut Criterion) {
    let (_, insert_request) = build_insert_request(1000);
    let rb = insert_request_to_record_batch(&insert_request);

    c.bench_function("insert_request_serialization_protobuf", |b| {
        b.iter(|| {
            black_box(insert_request.encode_to_vec());
        })
    })
    .bench_function("insert_request_serialization_protobuf_gzip", |b| {
        b.iter(|| {
            let dummy = insert_request.encode_to_vec();
            black_box(gzip_compress(&dummy));
        })
    })
    .bench_function("insert_request_serialization_arrow_ipc_default", |b| {
        b.iter(|| {
            black_box(insert_request_to_arrow_ipc(&rb, IpcWriteOptions::default()));
        })
    })
    .bench_function("insert_request_serialization_arrow_ipc_zstd", |b| {
        let options = IpcWriteOptions::default()
            .try_with_compression(Some(arrow_ipc::CompressionType::ZSTD))
            .unwrap();
        b.iter(|| {
            black_box(insert_request_to_arrow_ipc(&rb, options.clone()));
        })
    })
    .bench_function("insert_request_serialization_parquet_default", |b| {
        b.iter(|| {
            black_box(insert_request_to_parquet(&rb, None));
        })
    })
    .bench_function("insert_request_serialization_parquet_snappy", |b| {
        let options = WriterProperties::builder()
            .set_compression(datafusion::parquet::basic::Compression::SNAPPY)
            .build();
        b.iter(|| {
            black_box(insert_request_to_parquet(&rb, Some(options.clone())));
        })
    })
    .bench_function("insert_request_serialization_parquet_zstd", |b| {
        let options = WriterProperties::builder()
            .set_compression(datafusion::parquet::basic::Compression::ZSTD(
                ZstdLevel::default(),
            ))
            .build();
        b.iter(|| {
            black_box(insert_request_to_parquet(&rb, Some(options.clone())));
        })
    });
}

criterion_group!(benches, bench_insert_request_serialization);
criterion_main!(benches);
