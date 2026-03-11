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

use api::v1::region::{
    BulkInsertRequest, RegionRequest, RegionRequestHeader, bulk_insert_request, region_request,
};
use api::v1::value::ValueData;
use api::v1::{ArrowIpc, RowInsertRequests, Rows};
use arrow::array::{
    ArrayRef, Float64Builder, StringBuilder, TimestampMicrosecondBuilder,
    TimestampMillisecondBuilder, TimestampNanosecondBuilder, TimestampSecondBuilder,
};
use arrow::compute::filter_record_batch;
use arrow::datatypes::Schema as ArrowSchema;
use arrow::record_batch::RecordBatch;
use arrow_schema::TimeUnit;
use bytes::Bytes;
use catalog::CatalogManagerRef;
use common_grpc::flight::{FlightEncoder, FlightMessage};
use common_meta::node_manager::NodeManagerRef;
use common_query::prelude::GREPTIME_PHYSICAL_TABLE;
use common_telemetry::tracing_context::TracingContext;
use common_telemetry::{error, info, warn};
use dashmap::DashMap;
use dashmap::mapref::entry::Entry as DashMapEntry;
use partition::manager::PartitionRuleManagerRef;
use session::context::QueryContextRef;
use snafu::{ResultExt, ensure};
use store_api::storage::RegionId;
use table::metadata::TableInfoRef;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, broadcast, mpsc, oneshot};

use crate::error;
use crate::error::{Error, Result};
use crate::metrics::{
    FLUSH_DROPPED_ROWS, FLUSH_ELAPSED, FLUSH_FAILURES, FLUSH_ROWS, FLUSH_TOTAL, PENDING_BATCHES,
    PENDING_ROWS, PENDING_ROWS_BATCH_INGEST_STAGE_ELAPSED,
};

const PHYSICAL_TABLE_KEY: &str = "physical_table";

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
struct BatchKey {
    catalog: String,
    schema: String,
    physical_table: String,
}

#[derive(Debug)]
struct TableBatch {
    table_info: TableInfoRef,
    record_batch: RecordBatch,
    row_count: usize,
}

enum ColumnBuilder {
    Float64(Float64Builder),
    Utf8(StringBuilder),
    TimestampSecond(TimestampSecondBuilder),
    TimestampMillisecond(TimestampMillisecondBuilder),
    TimestampMicrosecond(TimestampMicrosecondBuilder),
    TimestampNanosecond(TimestampNanosecondBuilder),
}

impl ColumnBuilder {
    fn new(data_type: &arrow::datatypes::DataType) -> Result<Self> {
        match data_type {
            arrow::datatypes::DataType::Float64 => Ok(Self::Float64(Float64Builder::new())),
            arrow::datatypes::DataType::Utf8 => Ok(Self::Utf8(StringBuilder::new())),
            arrow::datatypes::DataType::Timestamp(unit, _) => match unit {
                TimeUnit::Second => Ok(Self::TimestampSecond(TimestampSecondBuilder::new())),
                TimeUnit::Millisecond => Ok(Self::TimestampMillisecond(
                    TimestampMillisecondBuilder::new(),
                )),
                TimeUnit::Microsecond => Ok(Self::TimestampMicrosecond(
                    TimestampMicrosecondBuilder::new(),
                )),
                TimeUnit::Nanosecond => {
                    Ok(Self::TimestampNanosecond(TimestampNanosecondBuilder::new()))
                }
            },
            ty => error::InvalidPromRemoteRequestSnafu {
                msg: format!("Unsupported column type in pending rows builder: {:?}", ty),
            }
            .fail(),
        }
    }

    fn append_value_data(&mut self, value: Option<&ValueData>) -> Result<()> {
        match self {
            Self::Float64(builder) => match value {
                Some(ValueData::F64Value(v)) => builder.append_value(*v),
                Some(v) => {
                    return error::InvalidPromRemoteRequestSnafu {
                        msg: format!("Unexpected value: {:?}", v),
                    }
                    .fail();
                }
                None => builder.append_null(),
            },
            Self::Utf8(builder) => match value {
                Some(ValueData::StringValue(v)) => builder.append_value(v),
                Some(v) => {
                    return error::InvalidPromRemoteRequestSnafu {
                        msg: format!("Unexpected value: {:?}", v),
                    }
                    .fail();
                }
                None => builder.append_null(),
            },
            Self::TimestampSecond(builder) => match value {
                Some(ValueData::TimestampSecondValue(v)) => builder.append_value(*v),
                Some(v) => {
                    return error::InvalidPromRemoteRequestSnafu {
                        msg: format!("Unexpected value: {:?}", v),
                    }
                    .fail();
                }
                None => builder.append_null(),
            },
            Self::TimestampMillisecond(builder) => match value {
                Some(ValueData::TimestampMillisecondValue(v)) => builder.append_value(*v),
                Some(v) => {
                    return error::InvalidPromRemoteRequestSnafu {
                        msg: format!("Unexpected value: {:?}", v),
                    }
                    .fail();
                }
                None => builder.append_null(),
            },
            Self::TimestampMicrosecond(builder) => match value {
                Some(ValueData::DatetimeValue(v) | ValueData::TimestampMicrosecondValue(v)) => {
                    builder.append_value(*v)
                }
                Some(v) => {
                    return error::InvalidPromRemoteRequestSnafu {
                        msg: format!("Unexpected value: {:?}", v),
                    }
                    .fail();
                }
                None => builder.append_null(),
            },
            Self::TimestampNanosecond(builder) => match value {
                Some(ValueData::TimestampNanosecondValue(v)) => builder.append_value(*v),
                Some(v) => {
                    return error::InvalidPromRemoteRequestSnafu {
                        msg: format!("Unexpected value: {:?}", v),
                    }
                    .fail();
                }
                None => builder.append_null(),
            },
        }
        Ok(())
    }

    fn append_null(&mut self) {
        match self {
            Self::Float64(builder) => builder.append_null(),
            Self::Utf8(builder) => builder.append_null(),
            Self::TimestampSecond(builder) => builder.append_null(),
            Self::TimestampMillisecond(builder) => builder.append_null(),
            Self::TimestampMicrosecond(builder) => builder.append_null(),
            Self::TimestampNanosecond(builder) => builder.append_null(),
        }
    }

    fn finish(self) -> ArrayRef {
        match self {
            Self::Float64(mut builder) => Arc::new(builder.finish()),
            Self::Utf8(mut builder) => Arc::new(builder.finish()),
            Self::TimestampSecond(mut builder) => Arc::new(builder.finish()),
            Self::TimestampMillisecond(mut builder) => Arc::new(builder.finish()),
            Self::TimestampMicrosecond(mut builder) => Arc::new(builder.finish()),
            Self::TimestampNanosecond(mut builder) => Arc::new(builder.finish()),
        }
    }
}

struct TableBuilders {
    table_name: String,
    table_info: Option<TableInfoRef>,
    schema: Arc<ArrowSchema>,
    builders: Vec<ColumnBuilder>,
    row_count: usize,
}

impl TableBuilders {
    fn new(table_name: String, schema: Arc<ArrowSchema>) -> Result<Self> {
        let builders = schema
            .fields()
            .iter()
            .map(|field| ColumnBuilder::new(field.data_type()))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            table_name,
            table_info: None,
            schema,
            builders,
            row_count: 0,
        })
    }

    fn new_with_table_info(table_info: TableInfoRef, schema: Arc<ArrowSchema>) -> Result<Self> {
        let table_name = table_info.name.clone();
        let mut builders = Self::new(table_name, schema)?;
        builders.table_info = Some(table_info);
        Ok(builders)
    }

    fn append_rows(&mut self, rows: &Rows) -> Result<()> {
        if rows.rows.is_empty() {
            return Ok(());
        }

        let source_indices = rows
            .schema
            .iter()
            .enumerate()
            .map(|(idx, col)| (col.column_name.as_str(), idx))
            .collect::<HashMap<_, _>>();

        let target_mappings = self
            .schema
            .fields()
            .iter()
            .map(|field| source_indices.get(field.name().as_str()).copied())
            .collect::<Vec<_>>();

        for (idx, row) in rows.rows.iter().enumerate() {
            ensure!(
                row.values.len() == rows.schema.len(),
                error::InternalSnafu {
                    err_msg: format!(
                        "Column count mismatch in row {}, expected {}, got {}",
                        idx,
                        rows.schema.len(),
                        row.values.len()
                    )
                }
            );
        }

        for row in &rows.rows {
            for (target_idx, source_idx) in target_mappings.iter().enumerate() {
                if let Some(source_idx) = source_idx {
                    self.builders[target_idx]
                        .append_value_data(row.values[*source_idx].value_data.as_ref())?;
                } else {
                    self.builders[target_idx].append_null();
                }
            }
        }

        self.row_count += rows.rows.len();
        Ok(())
    }

    #[cfg(test)]
    fn finish(self) -> Result<(String, RecordBatch, usize)> {
        let Self {
            table_name,
            schema,
            builders,
            row_count,
            ..
        } = self;
        let columns = builders
            .into_iter()
            .map(ColumnBuilder::finish)
            .collect::<Vec<_>>();
        let record_batch = RecordBatch::try_new(schema, columns).context(error::ArrowSnafu)?;
        Ok((table_name, record_batch, row_count))
    }

    fn finish_with_table_info(self) -> Result<(TableInfoRef, RecordBatch, usize)> {
        let Self {
            table_name,
            table_info,
            schema,
            builders,
            row_count,
        } = self;
        let table_info = table_info.ok_or_else(|| Error::Internal {
            err_msg: format!(
                "Pending table builders missing table info for table {}",
                table_name
            ),
        })?;
        let columns = builders
            .into_iter()
            .map(ColumnBuilder::finish)
            .collect::<Vec<_>>();
        let record_batch = RecordBatch::try_new(schema, columns).context(error::ArrowSnafu)?;
        Ok((table_info, record_batch, row_count))
    }
}

struct PendingBatch {
    tables: HashMap<String, TableBuilders>,
    created_at: Option<Instant>,
    total_row_count: usize,
    waiters: Vec<FlushWaiter>,
}

struct FlushWaiter {
    response_tx: oneshot::Sender<Result<()>>,
    _permit: OwnedSemaphorePermit,
}

struct FlushBatch {
    table_batches: Vec<TableBatch>,
    total_row_count: usize,
    waiters: Vec<FlushWaiter>,
}

#[derive(Clone)]
struct PendingWorker {
    tx: mpsc::Sender<WorkerCommand>,
}

enum WorkerCommand {
    Submit {
        table_rows: Vec<(TableInfoRef, Arc<ArrowSchema>, Rows)>,
        total_rows: usize,
        response_tx: oneshot::Sender<Result<()>>,
        _permit: OwnedSemaphorePermit,
    },
}

// Batch key is derived from QueryContext; it assumes catalog/schema/physical_table fully
// define the write target and must remain consistent across the batch.
fn batch_key_from_ctx(ctx: &QueryContextRef) -> BatchKey {
    let physical_table = ctx
        .extension(PHYSICAL_TABLE_KEY)
        .unwrap_or(GREPTIME_PHYSICAL_TABLE)
        .to_string();
    BatchKey {
        catalog: ctx.current_catalog().to_string(),
        schema: ctx.current_schema(),
        physical_table,
    }
}

/// Prometheus remote write pending rows batcher.
pub struct PendingRowsBatcher {
    workers: DashMap<BatchKey, PendingWorker>,
    flush_interval: Duration,
    max_batch_rows: usize,
    partition_manager: PartitionRuleManagerRef,
    node_manager: NodeManagerRef,
    catalog_manager: CatalogManagerRef,
    flush_semaphore: Arc<Semaphore>,
    inflight_semaphore: Arc<Semaphore>,
    worker_channel_capacity: usize,
    shutdown: broadcast::Sender<()>,
}

impl PendingRowsBatcher {
    pub fn try_new(
        partition_manager: PartitionRuleManagerRef,
        node_manager: NodeManagerRef,
        catalog_manager: CatalogManagerRef,
        flush_interval: Duration,
        max_batch_rows: usize,
        max_concurrent_flushes: usize,
        worker_channel_capacity: usize,
        max_inflight_requests: usize,
    ) -> Option<Arc<Self>> {
        if flush_interval.is_zero() {
            return None;
        }

        let (shutdown, _) = broadcast::channel(1);
        Some(Arc::new(Self {
            workers: DashMap::new(),
            flush_interval,
            max_batch_rows,
            partition_manager,
            node_manager,
            catalog_manager,
            flush_semaphore: Arc::new(Semaphore::new(max_concurrent_flushes)),
            inflight_semaphore: Arc::new(Semaphore::new(max_inflight_requests)),
            worker_channel_capacity,
            shutdown,
        }))
    }

    pub async fn submit(&self, requests: RowInsertRequests, ctx: QueryContextRef) -> Result<u64> {
        let (table_rows, total_rows) = {
            let _timer = PENDING_ROWS_BATCH_INGEST_STAGE_ELAPSED
                .with_label_values(&["submit_build_table_batches"])
                .start_timer();
            let mut table_rows = Vec::with_capacity(requests.inserts.len());
            let mut total_rows = 0;
            for request in requests.inserts {
                let Some(rows) = request.rows else {
                    continue;
                };
                if rows.rows.is_empty() {
                    continue;
                }
                total_rows += rows.rows.len();
                table_rows.push((request.table_name, rows));
            }
            (table_rows, total_rows)
        };
        if total_rows == 0 {
            return Ok(0);
        }
        let table_rows = {
            let _timer = PENDING_ROWS_BATCH_INGEST_STAGE_ELAPSED
                .with_label_values(&["submit_align_region_schema"])
                .start_timer();
            self.resolve_region_schemas(table_rows, &ctx).await?
        };

        let permit = {
            let _timer = PENDING_ROWS_BATCH_INGEST_STAGE_ELAPSED
                .with_label_values(&["submit_acquire_inflight_permit"])
                .start_timer();
            self.inflight_semaphore
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| Error::BatcherChannelClosed)?
        };

        let (response_tx, response_rx) = oneshot::channel();

        let worker = self.get_or_spawn_worker(batch_key_from_ctx(&ctx));
        let cmd = WorkerCommand::Submit {
            table_rows,
            total_rows,
            response_tx,
            _permit: permit,
        };

        {
            let _timer = PENDING_ROWS_BATCH_INGEST_STAGE_ELAPSED
                .with_label_values(&["submit_send_to_worker"])
                .start_timer();
            worker
                .tx
                .send(cmd)
                .await
                .map_err(|_| Error::BatcherChannelClosed)?;
        }

        if std::env::var("PENDING_ROWS_BATCH_SYNC")
            .ok()
            .and_then(|v| bool::from_str(&v).ok())
            .unwrap_or(false)
        {
            let result = {
                let _timer = PENDING_ROWS_BATCH_INGEST_STAGE_ELAPSED
                    .with_label_values(&["submit_wait_flush_result"])
                    .start_timer();
                response_rx.await.map_err(|_| Error::BatcherChannelClosed)?
            };
            result.map(|()| total_rows as u64)
        } else {
            Ok(total_rows as u64)
        }
    }

    async fn resolve_region_schemas(
        &self,
        table_rows: Vec<(String, Rows)>,
        ctx: &QueryContextRef,
    ) -> Result<Vec<(TableInfoRef, Arc<ArrowSchema>, Rows)>> {
        let catalog = ctx.current_catalog().to_string();
        let schema = ctx.current_schema();
        let mut table_metadata: HashMap<String, (TableInfoRef, Arc<ArrowSchema>)> = HashMap::new();
        let mut resolved_rows = Vec::with_capacity(table_rows.len());

        for (table_name, rows) in table_rows {
            let (table_info, region_schema) =
                if let Some((table_info, region_schema)) = table_metadata.get(&table_name) {
                    (table_info.clone(), region_schema.clone())
                } else {
                    let table = self
                        .catalog_manager
                        .table(&catalog, &schema, &table_name, Some(ctx.as_ref()))
                        .await
                        .map_err(|err| Error::Internal {
                            err_msg: format!(
                                "Failed to resolve table {} for pending batch alignment: {}",
                                table_name, err
                            ),
                        })?
                        .ok_or_else(|| Error::Internal {
                            err_msg: format!(
                                "Table not found during pending batch alignment: {}",
                                table_name
                            ),
                        })?;
                    let table_info = table.table_info();
                    let region_schema = table_info.meta.schema.arrow_schema().clone();
                    table_metadata.insert(
                        table_name.clone(),
                        (table_info.clone(), region_schema.clone()),
                    );
                    (table_info, region_schema)
                };

            resolved_rows.push((table_info, region_schema, rows));
        }

        Ok(resolved_rows)
    }

    fn get_or_spawn_worker(&self, key: BatchKey) -> PendingWorker {
        if let Some(worker) = self.workers.get(&key) {
            return worker.clone();
        }

        let entry = self.workers.entry(key);
        match entry {
            DashMapEntry::Occupied(worker) => worker.get().clone(),
            DashMapEntry::Vacant(vacant) => {
                let (tx, rx) = mpsc::channel(self.worker_channel_capacity);
                let worker = PendingWorker { tx };

                start_worker(
                    rx,
                    self.shutdown.clone(),
                    self.partition_manager.clone(),
                    self.node_manager.clone(),
                    self.flush_interval,
                    self.max_batch_rows,
                    self.flush_semaphore.clone(),
                );

                vacant.insert(worker.clone());
                worker
            }
        }
    }
}

impl Drop for PendingRowsBatcher {
    fn drop(&mut self) {
        let _ = self.shutdown.send(());
    }
}

impl PendingBatch {
    fn new() -> Self {
        Self {
            tables: HashMap::new(),
            created_at: None,
            total_row_count: 0,
            waiters: Vec::new(),
        }
    }
}

fn start_worker(
    mut rx: mpsc::Receiver<WorkerCommand>,
    shutdown: broadcast::Sender<()>,
    partition_manager: PartitionRuleManagerRef,
    node_manager: NodeManagerRef,
    flush_interval: Duration,
    max_batch_rows: usize,
    flush_semaphore: Arc<Semaphore>,
) {
    tokio::spawn(async move {
        let mut batch = PendingBatch::new();
        let mut interval = tokio::time::interval(flush_interval);
        let mut shutdown_rx = shutdown.subscribe();

        loop {
            tokio::select! {
                cmd = rx.recv() => {
                    match cmd {
                        Some(WorkerCommand::Submit { table_rows, total_rows, response_tx, _permit }) => {
                            if batch.total_row_count == 0 {
                                batch.created_at = Some(Instant::now());
                                PENDING_BATCHES.inc();
                            }

                            batch.waiters.push(FlushWaiter { response_tx, _permit });

                            for (table_info, schema, rows) in table_rows {
                                let entry = match batch.tables.entry(table_info.name.clone()) {
                                    std::collections::hash_map::Entry::Occupied(entry) => {
                                        entry.into_mut()
                                    }
                                    std::collections::hash_map::Entry::Vacant(entry) => {
                                        let table_builders =
                                            match TableBuilders::new_with_table_info(
                                                table_info, schema,
                                            ) {
                                                Ok(table_builders) => table_builders,
                                                Err(err) => {
                                                    flush_with_error(
                                                        &mut batch,
                                                        &format!(
                                                            "Failed to create table builders: {:?}",
                                                            err
                                                        ),
                                                    );
                                                    continue;
                                                }
                                            };
                                        entry.insert(table_builders)
                                    }
                                };
                                if let Err(err) = entry.append_rows(&rows) {
                                    flush_with_error(
                                        &mut batch,
                                        &format!("Failed to append pending rows: {:?}", err),
                                    );
                                    continue;
                                }
                            }

                            batch.total_row_count += total_rows;
                            PENDING_ROWS.add(total_rows as i64);

                            if batch.total_row_count >= max_batch_rows
                                && let Some(flush) = drain_batch(&mut batch) {
                                    spawn_flush(
                                        flush,
                                        partition_manager.clone(),
                                        node_manager.clone(),
                                        flush_semaphore.clone(),
                                    ).await;
                            }
                        }
                        None => {
                            if let Some(flush) = drain_batch(&mut batch) {
                                flush_batch(
                                    flush,
                                    partition_manager.clone(),
                                    node_manager.clone(),
                                ).await;
                            }
                            break;
                        }
                    }
                }
                _ = interval.tick() => {
                    if let Some(created_at) = batch.created_at
                        && batch.total_row_count > 0
                        && created_at.elapsed() >= flush_interval
                        && let Some(flush) = drain_batch(&mut batch) {
                            spawn_flush(
                                flush,
                                partition_manager.clone(),
                                node_manager.clone(),
                                flush_semaphore.clone(),
                            ).await;
                    }
                }
                _ = shutdown_rx.recv() => {
                    if let Some(flush) = drain_batch(&mut batch) {
                        flush_batch(
                            flush,
                            partition_manager.clone(),
                            node_manager.clone(),
                        ).await;
                    }
                    break;
                }
            }
        }
    });
}

fn drain_batch(batch: &mut PendingBatch) -> Option<FlushBatch> {
    if batch.total_row_count == 0 {
        return None;
    }

    let total_row_count = batch.total_row_count;
    let table_batches = {
        let mut table_batches = Vec::with_capacity(batch.tables.len());
        for table_builders in std::mem::take(&mut batch.tables).into_values() {
            let (table_info, record_batch, row_count) =
                match table_builders.finish_with_table_info() {
                    Ok(values) => values,
                    Err(err) => {
                        flush_with_error(
                            batch,
                            &format!("Failed to finalize pending table builders: {:?}", err),
                        );
                        return None;
                    }
                };
            table_batches.push(TableBatch {
                table_info,
                record_batch,
                row_count,
            });
        }
        table_batches
    };
    let waiters = std::mem::take(&mut batch.waiters);
    batch.total_row_count = 0;
    batch.created_at = None;

    PENDING_ROWS.sub(total_row_count as i64);
    PENDING_BATCHES.dec();

    Some(FlushBatch {
        table_batches,
        total_row_count,
        waiters,
    })
}

async fn spawn_flush(
    flush: FlushBatch,
    partition_manager: PartitionRuleManagerRef,
    node_manager: NodeManagerRef,
    semaphore: Arc<Semaphore>,
) {
    match semaphore.acquire_owned().await {
        Ok(permit) => {
            tokio::spawn(async move {
                let _permit = permit;
                flush_batch(flush, partition_manager, node_manager).await;
            });
        }
        Err(err) => {
            warn!(err; "Flush semaphore closed, flushing inline");
            flush_batch(flush, partition_manager, node_manager).await;
        }
    }
}

async fn flush_batch(
    flush: FlushBatch,
    partition_manager: PartitionRuleManagerRef,
    node_manager: NodeManagerRef,
) {
    let FlushBatch {
        table_batches,
        total_row_count,
        waiters,
    } = flush;
    let start = Instant::now();
    let mut first_error: Option<String> = None;

    macro_rules! record_failure {
        ($row_count:expr, $msg:expr) => {{
            let msg = $msg;
            if first_error.is_none() {
                first_error = Some(msg.clone());
            }
            mark_flush_failure($row_count, &msg);
        }};
    }

    for table_batch in table_batches {
        if table_batch.row_count == 0 {
            continue;
        }
        let table_info = table_batch.table_info;
        let record_batch = table_batch.record_batch;

        let partition_rule = {
            let _timer = PENDING_ROWS_BATCH_INGEST_STAGE_ELAPSED
                .with_label_values(&["flush_fetch_partition_rule"])
                .start_timer();
            match partition_manager
                .find_table_partition_rule(&table_info)
                .await
            {
                Ok(rule) => rule,
                Err(err) => {
                    record_failure!(
                        table_batch.row_count,
                        format!(
                            "Failed to fetch partition rule for table {}: {:?}",
                            table_info.name, err
                        )
                    );
                    continue;
                }
            }
        };

        let region_masks = {
            let _timer = PENDING_ROWS_BATCH_INGEST_STAGE_ELAPSED
                .with_label_values(&["flush_split_record_batch"])
                .start_timer();
            match partition_rule.split_record_batch(&record_batch) {
                Ok(masks) => masks,
                Err(err) => {
                    record_failure!(
                        table_batch.row_count,
                        format!(
                            "Failed to split record batch for table {}: {:?}",
                            table_info.name, err
                        )
                    );
                    continue;
                }
            }
        };

        for (region_number, mask) in region_masks {
            if mask.select_none() {
                continue;
            }

            let region_batch = if mask.select_all() {
                record_batch.clone()
            } else {
                let _timer = PENDING_ROWS_BATCH_INGEST_STAGE_ELAPSED
                    .with_label_values(&["flush_filter_record_batch"])
                    .start_timer();
                match filter_record_batch(&record_batch, mask.array()) {
                    Ok(batch) => batch,
                    Err(err) => {
                        record_failure!(
                            table_batch.row_count,
                            format!(
                                "Failed to filter record batch for table {}: {:?}",
                                table_info.name, err
                            )
                        );
                        continue;
                    }
                }
            };

            let row_count = region_batch.num_rows();
            if row_count == 0 {
                continue;
            }

            let region_id = RegionId::new(table_info.table_id(), region_number);
            let datanode = {
                let _timer = PENDING_ROWS_BATCH_INGEST_STAGE_ELAPSED
                    .with_label_values(&["flush_resolve_region_leader"])
                    .start_timer();
                match partition_manager.find_region_leader(region_id).await {
                    Ok(peer) => peer,
                    Err(err) => {
                        record_failure!(
                            row_count,
                            format!("Failed to resolve region leader {}: {:?}", region_id, err)
                        );
                        continue;
                    }
                }
            };

            let (schema_bytes, data_header, payload) = {
                let _timer = PENDING_ROWS_BATCH_INGEST_STAGE_ELAPSED
                    .with_label_values(&["flush_encode_ipc"])
                    .start_timer();
                match record_batch_to_ipc(region_batch) {
                    Ok(encoded) => encoded,
                    Err(err) => {
                        record_failure!(
                            row_count,
                            format!(
                                "Failed to encode Arrow IPC for region {}: {:?}",
                                region_id, err
                            )
                        );
                        continue;
                    }
                }
            };

            let request = RegionRequest {
                header: Some(RegionRequestHeader {
                    tracing_context: TracingContext::from_current_span().to_w3c(),
                    ..Default::default()
                }),
                body: Some(region_request::Body::BulkInsert(BulkInsertRequest {
                    region_id: region_id.as_u64(),
                    body: Some(bulk_insert_request::Body::ArrowIpc(ArrowIpc {
                        schema: schema_bytes,
                        data_header,
                        payload,
                    })),
                })),
            };

            let datanode = node_manager.datanode(&datanode).await;
            let _timer = PENDING_ROWS_BATCH_INGEST_STAGE_ELAPSED
                .with_label_values(&["flush_write_region"])
                .start_timer();
            match datanode.handle(request).await {
                Ok(_) => {
                    FLUSH_TOTAL.inc();
                    FLUSH_ROWS.observe(row_count as f64);
                }
                Err(err) => {
                    record_failure!(
                        row_count,
                        format!(
                            "Bulk insert flush failed for region {}: {:?}",
                            region_id, err
                        )
                    );
                }
            }
        }
    }

    let elapsed = start.elapsed().as_secs_f64();
    FLUSH_ELAPSED.observe(elapsed);
    info!(
        "Pending rows batch flushed, total rows: {}, elapsed time: {}s",
        total_row_count, elapsed
    );

    notify_waiters(waiters, &first_error);
}

fn notify_waiters(waiters: Vec<FlushWaiter>, first_error: &Option<String>) {
    for waiter in waiters {
        let result = match first_error {
            Some(err_msg) => Err(Error::Internal {
                err_msg: err_msg.clone(),
            }),
            None => Ok(()),
        };
        let _ = waiter.response_tx.send(result);
        // waiter._permit is dropped here, releasing the inflight semaphore slot
    }
}

fn mark_flush_failure(row_count: usize, message: &str) {
    error!("Pending rows batch flush failed, message: {}", message);
    FLUSH_FAILURES.inc();
    FLUSH_DROPPED_ROWS.inc_by(row_count as u64);
}

fn flush_with_error(batch: &mut PendingBatch, message: &str) {
    if batch.total_row_count == 0 {
        return;
    }

    let row_count = batch.total_row_count;
    let waiters = std::mem::take(&mut batch.waiters);
    batch.tables.clear();
    batch.total_row_count = 0;
    batch.created_at = None;

    PENDING_ROWS.sub(row_count as i64);
    PENDING_BATCHES.dec();

    let err_msg = Some(message.to_string());
    notify_waiters(waiters, &err_msg);
    mark_flush_failure(row_count, message);
}

fn record_batch_to_ipc(record_batch: RecordBatch) -> Result<(Bytes, Bytes, Bytes)> {
    let mut encoder = FlightEncoder::default();
    let schema = encoder.encode_schema(record_batch.schema().as_ref());
    let mut iter = encoder
        .encode(FlightMessage::RecordBatch(record_batch))
        .into_iter();
    let Some(flight_data) = iter.next() else {
        return Err(Error::Internal {
            err_msg: "Failed to encode empty flight data".to_string(),
        });
    };
    if iter.next().is_some() {
        return Err(Error::NotSupported {
            feat: "bulk insert RecordBatch with dictionary arrays".to_string(),
        });
    }

    Ok((
        schema.data_header,
        flight_data.data_header,
        flight_data.data_body,
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use api::v1::value::ValueData;
    use api::v1::{ColumnDataType, ColumnSchema, Row, Rows, SemanticType, Value};
    use arrow::datatypes::{DataType, Field, Schema as ArrowSchema};

    use super::TableBuilders;

    #[test]
    fn test_table_builders_append_and_finish() {
        let schema = Arc::new(ArrowSchema::new(vec![
            Field::new(
                "ts",
                DataType::Timestamp(arrow_schema::TimeUnit::Millisecond, None),
                true,
            ),
            Field::new("value", DataType::Float64, true),
            Field::new("host", DataType::Utf8, true),
        ]));
        let mut builders = TableBuilders::new("demo".to_string(), schema).unwrap();

        let rows = Rows {
            schema: vec![
                ColumnSchema {
                    column_name: "host".to_string(),
                    datatype: ColumnDataType::String as i32,
                    semantic_type: SemanticType::Tag as i32,
                    ..Default::default()
                },
                ColumnSchema {
                    column_name: "ts".to_string(),
                    datatype: ColumnDataType::TimestampMillisecond as i32,
                    semantic_type: SemanticType::Timestamp as i32,
                    ..Default::default()
                },
                ColumnSchema {
                    column_name: "value".to_string(),
                    datatype: ColumnDataType::Float64 as i32,
                    semantic_type: SemanticType::Field as i32,
                    ..Default::default()
                },
            ],
            rows: vec![
                Row {
                    values: vec![
                        Value {
                            value_data: Some(ValueData::StringValue("h1".to_string())),
                        },
                        Value {
                            value_data: Some(ValueData::TimestampMillisecondValue(1000)),
                        },
                        Value {
                            value_data: Some(ValueData::F64Value(42.0)),
                        },
                    ],
                },
                Row {
                    values: vec![
                        Value {
                            value_data: Some(ValueData::StringValue("h2".to_string())),
                        },
                        Value {
                            value_data: Some(ValueData::TimestampMillisecondValue(2000)),
                        },
                        Value { value_data: None },
                    ],
                },
            ],
        };

        builders.append_rows(&rows).unwrap();
        let (table_name, batch, row_count) = builders.finish().unwrap();
        assert_eq!("demo", table_name);
        assert_eq!(2, row_count);
        assert_eq!(2, batch.num_rows());
        assert_eq!(3, batch.num_columns());
    }
}
