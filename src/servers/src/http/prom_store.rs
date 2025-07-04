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

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use api::prom_store::remote::ReadRequest;
use arrow::array::RecordBatch;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::Extension;
use axum_extra::TypedHeader;
use common_catalog::consts::DEFAULT_SCHEMA_NAME;
use common_meta::node_manager::NodeManagerRef;
use common_query::prelude::GREPTIME_PHYSICAL_TABLE;
use common_telemetry::{info, tracing};
use hyper::HeaderMap;
use lazy_static::lazy_static;
use mito2::sst::file::FileMeta;
use object_pool::Pool;
use operator::metrics::DIST_INGEST_ROW_COUNT;
use operator::schema_helper::SchemaHelper;
use partition::manager::PartitionRuleManagerRef;
use pipeline::util::to_pipeline_version;
use pipeline::{ContextReq, PipelineDefinition};
use prost::Message;
use serde::{Deserialize, Serialize};
use session::context::{Channel, QueryContext, QueryContextRef};
use snafu::prelude::*;
use store_api::metadata::RegionMetadataRef;
use store_api::storage::{ColumnId, RegionId};
use table::metadata::TableId;
use tokio::sync::mpsc::Sender;

use crate::access_layer::AccessLayerFactory;
use crate::batch_builder::{AppendMetrics, MetricsBatchBuilder};
use crate::error::{self, InternalSnafu, PipelineSnafu, Result};
use crate::http::extractor::PipelineInfo;
use crate::http::header::{write_cost_header_map, GREPTIME_DB_HEADER_METRICS};
use crate::http::PromValidationMode;
use crate::metrics::METRIC_BULK_ALTER_TABLE;
use crate::prom_row_builder::{PromCtx, TableBuilder, TablesBuilder};
use crate::prom_store::{snappy_decompress, zstd_decompress};
use crate::proto::{PromSeriesProcessor, PromWriteRequest};
use crate::query_handler::{PipelineHandlerRef, PromStoreProtocolHandlerRef, PromStoreResponse};

pub const PHYSICAL_TABLE_PARAM: &str = "physical_table";
lazy_static! {
    static ref PROM_WRITE_REQUEST_POOL: Pool<PromWriteRequest> =
        Pool::new(256, PromWriteRequest::default);
}

pub const DEFAULT_ENCODING: &str = "snappy";
pub const VM_ENCODING: &str = "zstd";
pub const VM_PROTO_VERSION: &str = "1";

/// Additional states for bulk write requests.
#[derive(Clone)]
pub struct PromBulkState {
    pub schema_helper: SchemaHelper,
    pub partition_manager: PartitionRuleManagerRef,
    pub node_manager: NodeManagerRef,
    pub access_layer_factory: AccessLayerFactory,
    pub tx: Option<
        Sender<(
            QueryContextRef,
            HashMap<PromCtx, HashMap<String, TableBuilder>>,
        )>,
    >,
}

#[derive(Clone)]
pub struct PromStoreState {
    pub prom_store_handler: PromStoreProtocolHandlerRef,
    pub pipeline_handler: Option<PipelineHandlerRef>,
    pub prom_store_with_metric_engine: bool,
    pub prom_validation_mode: PromValidationMode,
    pub bulk_state: Option<PromBulkState>,
}

impl PromBulkState {
    pub fn start_background_task(&mut self) {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(
            QueryContextRef,
            HashMap<PromCtx, HashMap<String, TableBuilder>>,
        )>(16);

        self.tx = Some(tx);
        let schema_helper = self.schema_helper.clone();
        let partition_manager = self.partition_manager.clone();
        let node_manager = self.node_manager.clone();
        let access_layer_factory = self.access_layer_factory.clone();

        let max_batch_num = std::env::var("MAX_BATCH_NUM")
            .ok()
            .and_then(|v| usize::from_str(&v).ok())
            .unwrap_or(10);
        let max_batch_interval_secs = std::env::var("MAX_BATCH_INTERVAL_SECS")
            .ok()
            .and_then(|v| u64::from_str(&v).ok())
            .unwrap_or(5);

        info!(
            "max_batch_num: {}, max_batch_interval_secs: {}",
            max_batch_num, max_batch_interval_secs
        );

        let _handle = tokio::spawn(async move {
            let mut last_process_time = Instant::now();
            loop {
                let mut batch_builder = MetricsBatchBuilder::new(
                    schema_helper.clone(),
                    partition_manager.clone(),
                    node_manager.clone(),
                );
                let mut physical_region_metadata_total = HashMap::new();
                let mut num_batches = 0;
                let mut append_metrics = AppendMetrics::default();

                while let Some((query_context, mut tables)) = rx.recv().await {
                    // let timer = METRIC_BULK_ALTER_TABLE
                    //     .with_label_values(&["alter_table"])
                    //     .start_timer();
                    //
                    // create_or_alter_physical_tables(&schema_helper, &tables, &query_context)
                    //     .await
                    //     .unwrap();
                    // timer.observe_duration();

                    // Extract logical table names from tables for metadata collection
                    let current_schema = query_context.current_schema();
                    let logical_tables: Vec<(
                        String, /*schema name*/
                        String, /*logical table name*/
                    )> = tables
                        .iter()
                        .flat_map(|(ctx, table_map)| {
                            let schema = ctx.schema.as_deref().unwrap_or(&current_schema);
                            table_map
                                .keys()
                                .map(|table_name| (schema.to_string(), table_name.clone()))
                        })
                        .collect();

                    // Gather all region metadata for region 0 of physical tables.
                    let start = Instant::now();
                    let physical_region_metadata = batch_builder
                        .collect_physical_region_metadata(&logical_tables, &query_context)
                        .await
                        .unwrap();
                    append_metrics.physical_region_meta += start.elapsed();
                    physical_region_metadata_total.extend(physical_region_metadata);

                    let start = Instant::now();
                    batch_builder
                        .append_rows_to_batch(
                            None,
                            None,
                            &mut tables,
                            &physical_region_metadata_total,
                            &mut append_metrics,
                        )
                        .expect("send error back");
                    append_metrics.append_rows_total += start.elapsed();
                    num_batches += 1;

                    let last_process_time_elapsed = last_process_time.elapsed();
                    if num_batches >= max_batch_num
                        || last_process_time_elapsed >= Duration::from_secs(max_batch_interval_secs)
                    {
                        info!(
                            "num batches: {}, last_process_time_elapsed: {:?}",
                            num_batches, last_process_time_elapsed
                        );
                        break;
                    }
                }

                let access_layer_factory = access_layer_factory.clone();
                last_process_time = Instant::now();
                tokio::spawn(async move {
                    let start = Instant::now();
                    let timer = METRIC_BULK_ALTER_TABLE
                        .with_label_values(&["finish_encoder"])
                        .start_timer();
                    let record_batches = batch_builder.finish().unwrap();
                    timer.observe_duration();

                    let file_metas = process_record_batches(
                        access_layer_factory,
                        physical_region_metadata_total,
                        record_batches,
                    )
                    .await;

                    let total_rows: u64 = file_metas.iter().map(|f| f.num_rows).sum();

                    DIST_INGEST_ROW_COUNT.inc_by(total_rows);
                    info!(
                        "Upload sst files, elapsed time: {}ms, total rows: {}, file_metas: {:?}, ",
                        start.elapsed().as_millis(),
                        total_rows,
                        file_metas
                    );
                });
            }
        });
    }
}

async fn process_record_batches(
    access_layer_factory: AccessLayerFactory,
    physical_region_metadata_total: HashMap<
        String,
        HashMap<String, (TableId, RegionMetadataRef, Arc<HashMap<String, ColumnId>>)>,
    >,
    record_batches: HashMap<String, HashMap<RegionId, Vec<(RecordBatch, (i64, i64))>>>,
) -> Vec<FileMeta> {
    let physical_region_id_to_meta = physical_region_metadata_total
        .into_iter()
        .map(|(schema_name, tables)| {
            let region_id_to_meta = tables
                .into_values()
                .map(|(_, physical_region_meta, _)| {
                    (physical_region_meta.region_id, physical_region_meta)
                })
                .collect::<HashMap<_, _>>();
            (schema_name, region_id_to_meta)
        })
        .collect::<HashMap<_, _>>();

    let timer = METRIC_BULK_ALTER_TABLE
        .with_label_values(&["write_sst"])
        .start_timer();
    let mut tasks = vec![];
    for (schema_name, schema_batches) in record_batches {
        let schema_regions = physical_region_id_to_meta
            .get(&schema_name)
            .expect("physical region schema not found");
        for (physical_region_id, record_batches) in schema_batches {
            let physical_region_metadata = schema_regions
                .get(&physical_region_id)
                .expect("physical region metadata not found");
            for (rb, time_range) in record_batches {
                let schema_name_cloned = schema_name.clone();
                let access_layer_factory = access_layer_factory.clone();
                let physical_region_metadata = physical_region_metadata.clone();
                let handle = tokio::spawn(async move {
                    let mut writer = access_layer_factory
                        .create_sst_writer(
                            "greptime", //todo(hl): use the catalog name in query context.
                            &schema_name_cloned,
                            physical_region_metadata,
                        )
                        .await
                        .unwrap();
                    let start = Instant::now();
                    info!("Created writer: {}", writer.file_id());
                    writer
                        .write_record_batch(&rb, Some(time_range))
                        .await
                        .unwrap();
                    let file_meta = writer.finish().await.unwrap();
                    info!(
                        "Finished writer: {}, elapsed time: {}ms",
                        writer.file_id(),
                        start.elapsed().as_millis()
                    );
                    file_meta
                });
                tasks.push(handle);
            }
        }
    }

    let file_metas: Vec<_> = futures::future::try_join_all(tasks).await.unwrap();
    timer.observe_duration();
    file_metas
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RemoteWriteQuery {
    pub db: Option<String>,
    /// Specify which physical table to use for storing metrics.
    /// This only works on remote write requests.
    pub physical_table: Option<String>,
    /// For VictoriaMetrics modified remote write protocol
    pub get_vm_proto_version: Option<String>,
}

impl Default for RemoteWriteQuery {
    fn default() -> RemoteWriteQuery {
        Self {
            db: Some(DEFAULT_SCHEMA_NAME.to_string()),
            physical_table: Some(GREPTIME_PHYSICAL_TABLE.to_string()),
            get_vm_proto_version: None,
        }
    }
}

#[axum_macros::debug_handler]
#[tracing::instrument(
    skip_all,
    fields(protocol = "prometheus", request_type = "remote_write")
)]
pub async fn remote_write(
    State(state): State<PromStoreState>,
    Query(params): Query<RemoteWriteQuery>,
    Extension(mut query_ctx): Extension<QueryContext>,
    pipeline_info: PipelineInfo,
    content_encoding: TypedHeader<headers::ContentEncoding>,
    body: Bytes,
) -> Result<impl IntoResponse> {
    let PromStoreState {
        prom_store_handler,
        pipeline_handler,
        prom_store_with_metric_engine,
        prom_validation_mode,
        bulk_state,
    } = state;

    if let Some(_vm_handshake) = params.get_vm_proto_version {
        return Ok(VM_PROTO_VERSION.into_response());
    }

    let db = params.db.clone().unwrap_or_default();
    query_ctx.set_channel(Channel::Prometheus);
    if let Some(physical_table) = params.physical_table {
        query_ctx.set_extension(PHYSICAL_TABLE_PARAM, physical_table);
    }
    let query_ctx = Arc::new(query_ctx);
    let _timer = crate::metrics::METRIC_HTTP_PROM_STORE_WRITE_ELAPSED
        .with_label_values(&[db.as_str()])
        .start_timer();

    let is_zstd = content_encoding.contains(VM_ENCODING);

    let mut processor = PromSeriesProcessor::default_processor();
    if let Some(pipeline_name) = pipeline_info.pipeline_name {
        let pipeline_def = PipelineDefinition::from_name(
            &pipeline_name,
            to_pipeline_version(pipeline_info.pipeline_version.as_deref())
                .context(PipelineSnafu)?,
            None,
        )
        .context(PipelineSnafu)?;
        let pipeline_handler = pipeline_handler.context(InternalSnafu {
            err_msg: "pipeline handler is not set".to_string(),
        })?;

        processor.set_pipeline(pipeline_handler, query_ctx.clone(), pipeline_def);
    }

    if let Some(state) = bulk_state {
        let builder = decode_remote_write_request_to_batch(
            is_zstd,
            body,
            prom_validation_mode,
            &mut processor,
        )
        .await?;
        state
            .tx
            .as_ref()
            .unwrap()
            .send((query_ctx, builder.tables))
            .await
            .unwrap();
        return Ok((StatusCode::NO_CONTENT, write_cost_header_map(0)).into_response());
    }

    let req =
        decode_remote_write_request(is_zstd, body, prom_validation_mode, &mut processor).await?;

    let mut cost = 0;
    for (temp_ctx, reqs) in req.as_req_iter(query_ctx) {
        let cnt: u64 = reqs
            .inserts
            .iter()
            .filter_map(|s| s.rows.as_ref().map(|r| r.rows.len() as u64))
            .sum();
        let output = prom_store_handler
            .write(reqs, temp_ctx, prom_store_with_metric_engine)
            .await?;
        crate::metrics::PROM_STORE_REMOTE_WRITE_SAMPLES.inc_by(cnt);
        cost += output.meta.cost;
    }

    Ok((StatusCode::NO_CONTENT, write_cost_header_map(cost)).into_response())
}

impl IntoResponse for PromStoreResponse {
    fn into_response(self) -> axum::response::Response {
        let mut header_map = HeaderMap::new();
        header_map.insert(&header::CONTENT_TYPE, self.content_type);
        header_map.insert(&header::CONTENT_ENCODING, self.content_encoding);

        let metrics = if self.resp_metrics.is_empty() {
            None
        } else {
            serde_json::to_string(&self.resp_metrics).ok()
        };
        if let Some(m) = metrics.and_then(|m| HeaderValue::from_str(&m).ok()) {
            header_map.insert(&GREPTIME_DB_HEADER_METRICS, m);
        }

        (header_map, self.body).into_response()
    }
}

#[axum_macros::debug_handler]
#[tracing::instrument(
    skip_all,
    fields(protocol = "prometheus", request_type = "remote_read")
)]
pub async fn remote_read(
    State(state): State<PromStoreState>,
    Query(params): Query<RemoteWriteQuery>,
    Extension(mut query_ctx): Extension<QueryContext>,
    body: Bytes,
) -> Result<PromStoreResponse> {
    let db = params.db.clone().unwrap_or_default();
    query_ctx.set_channel(Channel::Prometheus);
    let query_ctx = Arc::new(query_ctx);
    let _timer = crate::metrics::METRIC_HTTP_PROM_STORE_READ_ELAPSED
        .with_label_values(&[db.as_str()])
        .start_timer();

    let request = decode_remote_read_request(body).await?;

    state.prom_store_handler.read(request, query_ctx).await
}

fn try_decompress(is_zstd: bool, body: &[u8]) -> Result<Bytes> {
    Ok(Bytes::from(if is_zstd {
        zstd_decompress(body)?
    } else {
        snappy_decompress(body)?
    }))
}

async fn decode_remote_write_request(
    is_zstd: bool,
    body: Bytes,
    prom_validation_mode: PromValidationMode,
    processor: &mut PromSeriesProcessor,
) -> Result<ContextReq> {
    let _timer = crate::metrics::METRIC_HTTP_PROM_STORE_DECODE_ELAPSED.start_timer();

    // due to vmagent's limitation, there is a chance that vmagent is
    // sending content type wrong so we have to apply a fallback with decoding
    // the content in another method.
    //
    // see https://github.com/VictoriaMetrics/VictoriaMetrics/issues/5301
    // see https://github.com/GreptimeTeam/greptimedb/issues/3929
    let buf = if let Ok(buf) = try_decompress(is_zstd, &body[..]) {
        buf
    } else {
        // fallback to the other compression method
        try_decompress(!is_zstd, &body[..])?
    };

    let mut request = PROM_WRITE_REQUEST_POOL.pull(PromWriteRequest::default);

    request
        .merge(buf, prom_validation_mode, processor)
        .context(error::DecodePromRemoteRequestSnafu)?;

    if processor.use_pipeline {
        processor.exec_pipeline().await
    } else {
        Ok(request.as_row_insert_requests())
    }
}

async fn decode_remote_write_request_to_batch(
    is_zstd: bool,
    body: Bytes,
    prom_validation_mode: PromValidationMode,
    processor: &mut PromSeriesProcessor,
) -> Result<TablesBuilder> {
    let _timer = crate::metrics::METRIC_HTTP_PROM_STORE_DECODE_ELAPSED.start_timer();

    // due to vmagent's limitation, there is a chance that vmagent is
    // sending content type wrong so we have to apply a fallback with decoding
    // the content in another method.
    //
    // see https://github.com/VictoriaMetrics/VictoriaMetrics/issues/5301
    // see https://github.com/GreptimeTeam/greptimedb/issues/3929
    let buf = if let Ok(buf) = try_decompress(is_zstd, &body[..]) {
        buf
    } else {
        // fallback to the other compression method
        try_decompress(!is_zstd, &body[..])?
    };

    let mut request = PROM_WRITE_REQUEST_POOL.pull(PromWriteRequest::default);

    processor.use_pipeline = false;
    request
        .merge(buf, prom_validation_mode, processor)
        .context(error::DecodePromRemoteRequestSnafu)?;

    Ok(std::mem::take(&mut request.table_data))
}

async fn decode_remote_read_request(body: Bytes) -> Result<ReadRequest> {
    let buf = snappy_decompress(&body[..])?;

    ReadRequest::decode(&buf[..]).context(error::DecodePromRemoteRequestSnafu)
}
