// test grpc InsertRequest serialization size

use std::io::{BufRead, Write};
use std::sync::Arc;

use api::v1::column::Values;
use api::v1::{ColumnDataType, InsertRequest, SemanticType};
use arrow_array::{ArrayRef, RecordBatch, StringArray, TimestampMillisecondArray};
use arrow_ipc::writer::IpcWriteOptions;
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use datafusion::parquet::arrow::ArrowWriter;
use datafusion::parquet::file::properties::WriterProperties;
use rand::Rng;

fn random_string(length: usize) -> String {
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..length).map(|_| rng.gen_range(b'0'..=b'z')).collect();
    String::from_utf8(bytes).unwrap()
}

fn random_in_candidates(candidates: &[&str]) -> String {
    let mut rng = rand::thread_rng();
    candidates[rng.gen_range(0..candidates.len())].to_string()
}

pub fn build_insert_request(num_rows: usize) -> (usize, InsertRequest) {
    let mut total_size = 0;

    let file = std::fs::File::open("/home/lei/workspace/grafatheus/assets/demo_logs.log").unwrap();
    let reader = std::io::BufReader::new(file);
    let mut lines = reader.lines();
    let log_messages = (0..num_rows)
        .map(|_| {
            let mut line = String::new();
            loop {
                line.push_str(&lines.next().unwrap().unwrap());

                if line.len() > 1000 {
                    break;
                }
            }
            total_size += line.len();
            line
        })
        .collect::<Vec<_>>();

    let message = api::v1::Column {
        column_name: "message".to_string(),
        semantic_type: SemanticType::Field as i32,
        values: Some(Values {
            string_values: log_messages,
            ..Default::default()
        }),
        datatype: ColumnDataType::String as i32,
        ..Default::default()
    };

    let extract_level = api::v1::Column {
        column_name: "extractLevel".to_string(),
        semantic_type: SemanticType::Field as i32,
        values: Some(Values {
            string_values: (0..num_rows)
                .map(|_| random_in_candidates(&["INFO", "WARN", "ERROR"]))
                .collect(),
            ..Default::default()
        }),
        datatype: ColumnDataType::String as i32,
        ..Default::default()
    };
    total_size += 4 * num_rows;

    let dltag = api::v1::Column {
        column_name: "dltag".to_string(),
        semantic_type: SemanticType::Field as i32,
        values: Some(Values {
            string_values: (0..num_rows).map(|_| random_string(10)).collect(),
            ..Default::default()
        }),
        datatype: ColumnDataType::String as i32,
        ..Default::default()
    };
    total_size += 10 * num_rows;
    let host_name = api::v1::Column {
        column_name: "hostName".to_string(),
        semantic_type: SemanticType::Field as i32,
        values: Some(Values {
            string_values: (0..num_rows).map(|_| random_string(10)).collect(),
            ..Default::default()
        }),
        datatype: ColumnDataType::String as i32,
        ..Default::default()
    };
    total_size += 10 * num_rows;
    let odin_leaf = api::v1::Column {
        column_name: "odinLeaf".to_string(),
        semantic_type: SemanticType::Field as i32,
        values: Some(Values {
            string_values: (0..num_rows).map(|_| random_string(10)).collect(),
            ..Default::default()
        }),
        datatype: ColumnDataType::String as i32,
        ..Default::default()
    };
    total_size += 10 * num_rows;

    let log_name = api::v1::Column {
        column_name: "logName".to_string(),
        semantic_type: SemanticType::Field as i32,
        values: Some(Values {
            string_values: (0..num_rows).map(|_| random_string(10)).collect(),
            ..Default::default()
        }),
        datatype: ColumnDataType::String as i32,
        ..Default::default()
    };
    total_size += 10 * num_rows;
    let current_timestamp = common_time::util::current_time_millis();
    let mut rng = rand::thread_rng();

    let log_time = api::v1::Column {
        column_name: "logTime".to_string(),
        semantic_type: SemanticType::Timestamp as i32,
        values: Some(Values {
            timestamp_millisecond_values: (0..num_rows)
                .map(|_| rng.gen_range(0..1000) + current_timestamp)
                .collect(),
            ..Default::default()
        }),
        datatype: ColumnDataType::TimestampMillisecond as i32,
        ..Default::default()
    };
    total_size += 8 * num_rows;
    let traceid = api::v1::Column {
        column_name: "traceid".to_string(),
        semantic_type: SemanticType::Field as i32,
        values: Some(Values {
            string_values: (0..num_rows).map(|_| random_string(16)).collect(),
            ..Default::default()
        }),
        datatype: ColumnDataType::String as i32,
        ..Default::default()
    };
    total_size += 16 * num_rows;
    let spanid = api::v1::Column {
        column_name: "spanid".to_string(),
        semantic_type: SemanticType::Field as i32,
        values: Some(Values {
            string_values: (0..num_rows).map(|_| random_string(16)).collect(),
            ..Default::default()
        }),
        datatype: ColumnDataType::String as i32,
        ..Default::default()
    };
    total_size += 16 * num_rows;
    let cspanid = api::v1::Column {
        column_name: "cspanid".to_string(),
        semantic_type: SemanticType::Field as i32,
        values: Some(Values {
            string_values: (0..num_rows).map(|_| random_string(16)).collect(),
            ..Default::default()
        }),
        datatype: ColumnDataType::String as i32,
        ..Default::default()
    };
    total_size += 16 * num_rows;
    let uri = api::v1::Column {
        column_name: "uri".to_string(),
        semantic_type: SemanticType::Field as i32,
        values: Some(Values {
            string_values: (0..num_rows).map(|_| random_string(10)).collect(),
            ..Default::default()
        }),
        datatype: ColumnDataType::String as i32,
        ..Default::default()
    };
    total_size += 10 * num_rows;
    let errno = api::v1::Column {
        column_name: "errno".to_string(),
        semantic_type: SemanticType::Field as i32,
        values: Some(Values {
            string_values: (0..num_rows).map(|_| random_string(10)).collect(),
            ..Default::default()
        }),
        datatype: ColumnDataType::String as i32,
        ..Default::default()
    };
    total_size += 10 * num_rows;
    (
        total_size,
        InsertRequest {
            table_name: "test_bamai_table_v5".to_string(),
            columns: vec![
                message,
                extract_level,
                dltag,
                host_name,
                odin_leaf,
                log_name,
                log_time,
                traceid,
                spanid,
                cspanid,
                uri,
                errno,
            ],
            row_count: num_rows as u32,
        },
    )
}

pub fn insert_request_to_record_batch(insert_request: &InsertRequest) -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![
        Field::new("message", DataType::Utf8, true),
        Field::new("extractLevel", DataType::Utf8, true),
        Field::new("dltag", DataType::Utf8, true),
        Field::new("hostName", DataType::Utf8, true),
        Field::new("odinLeaf", DataType::Utf8, true),
        Field::new("logName", DataType::Utf8, true),
        Field::new(
            "logTime",
            DataType::Timestamp(TimeUnit::Millisecond, None),
            false,
        ),
        Field::new("traceid", DataType::Utf8, true),
        Field::new("spanid", DataType::Utf8, true),
        Field::new("cspanid", DataType::Utf8, true),
        Field::new("uri", DataType::Utf8, true),
        Field::new("errno", DataType::Utf8, true),
    ]));

    let columns = insert_request
        .columns
        .iter()
        .zip(schema.fields().iter())
        .map(|(column, field)| {
            let values = column.values.as_ref().unwrap();
            match field.data_type() {
                arrow::datatypes::DataType::Utf8 => {
                    Arc::new(StringArray::from(values.string_values.clone())) as ArrayRef
                }
                arrow::datatypes::DataType::Timestamp(TimeUnit::Millisecond, None) => Arc::new(
                    TimestampMillisecondArray::from(values.timestamp_millisecond_values.clone()),
                )
                    as ArrayRef,
                _ => {
                    unreachable!()
                }
            }
        })
        .collect::<Vec<_>>();

    arrow::array::RecordBatch::try_new(schema, columns).unwrap()
}

pub fn gzip_compress(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

pub fn insert_request_to_arrow_ipc(rb: &RecordBatch, options: IpcWriteOptions) -> Vec<u8> {
    let mut writer = arrow_ipc::writer::FileWriter::try_new_with_options(
        Vec::with_capacity(1024 * 1024 * 16),
        &rb.schema(),
        options,
    )
    .unwrap();
    writer.write(rb).unwrap();
    writer.into_inner().unwrap()
}

pub fn insert_request_to_parquet(
    rb: &RecordBatch,
    write_properties: Option<WriterProperties>,
) -> Vec<u8> {
    let mut writer = ArrowWriter::try_new(
        Vec::with_capacity(1024 * 1024 * 16),
        rb.schema(),
        write_properties,
    )
    .unwrap();
    writer.write(rb).unwrap();
    writer.finish().unwrap();
    writer.inner_mut().clone()
}

#[test]
fn test_arrow_ipc_size() {
    use prost::Message;
    let (raw_size, insert_request) = build_insert_request(230000);
    let protobuf_encoded = insert_request.encode_to_vec();
    let protobuf_encoded_gzip = gzip_compress(&protobuf_encoded);

    let rb = insert_request_to_record_batch(&insert_request);
    let ipc_default_encoded = insert_request_to_arrow_ipc(&rb, IpcWriteOptions::default());
    let ipc_zstd_encoded = insert_request_to_arrow_ipc(
        &rb,
        IpcWriteOptions::default()
            .try_with_compression(Some(arrow_ipc::CompressionType::ZSTD))
            .unwrap(),
    );
    let parquet_default_encoded = insert_request_to_parquet(&rb, None);
    let parquet_snappy_encoded = insert_request_to_parquet(
        &rb,
        Some(
            WriterProperties::builder()
                .set_compression(datafusion::parquet::basic::Compression::SNAPPY)
                .build(),
        ),
    );
    let parquet_zstd_encoded = insert_request_to_parquet(
        &rb,
        Some(
            WriterProperties::builder()
                .set_compression(datafusion::parquet::basic::Compression::ZSTD(
                    datafusion::parquet::basic::ZstdLevel::default(),
                ))
                .build(),
        ),
    );

    println!("raw size: {}", raw_size / 1024);
    println!("protobuf size: {}", protobuf_encoded.len() / 1024);
    println!("protobuf gzip size: {}", protobuf_encoded_gzip.len() / 1024);
    println!("ipc default size: {}", ipc_default_encoded.len() / 1024);
    println!("ipc zstd size: {}", ipc_zstd_encoded.len() / 1024);
    println!(
        "parquet default size: {}",
        parquet_default_encoded.len() / 1024
    );
    println!(
        "parquet snappy size: {}",
        parquet_snappy_encoded.len() / 1024
    );
    println!("parquet zstd size: {}", parquet_zstd_encoded.len() / 1024);
}
