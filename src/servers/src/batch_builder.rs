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
use std::time::{Duration, Instant};

use api::v1::value::ValueData;
use api::v1::{ColumnDataType, ColumnSchema, OpType, SemanticType};
use arrow::array::{
    ArrayDataBuilder, ArrayRef, BufferBuilder, Float64Array, GenericByteArray, RecordBatch,
    TimestampMillisecondArray, UInt64Array, UInt8Array, UInt8BufferBuilder,
};
use arrow::compute;
use arrow_schema::Field;
use common_catalog::consts::{DEFAULT_CATALOG_NAME, DEFAULT_SCHEMA_NAME};
use common_meta::node_manager::NodeManagerRef;
use common_query::prelude::{GREPTIME_TIMESTAMP, GREPTIME_VALUE};
use common_telemetry::info;
use datatypes::arrow_array::BinaryArray;
use itertools::Itertools;
use metric_engine::row_modifier::{RowModifier, RowsIter};
use mito_codec::row_converter::SparsePrimaryKeyCodec;
use operator::schema_helper::{
    ensure_logical_tables_for_metrics, metadatas_for_region_ids, LogicalSchema, LogicalSchemas,
    SchemaHelper,
};
use partition::manager::PartitionRuleManagerRef;
use session::context::QueryContextRef;
use snafu::{OptionExt, ResultExt};
use store_api::metadata::RegionMetadataRef;
use store_api::storage::consts::{
    ReservedColumnId, OP_TYPE_COLUMN_NAME, PRIMARY_KEY_COLUMN_NAME, SEQUENCE_COLUMN_NAME,
};
use store_api::storage::{ColumnId, RegionId};
use table::metadata::TableId;

use crate::error;
use crate::metrics::METRIC_BULK_ALTER_TABLE;
use crate::prom_row_builder::{PromCtx, TableBuilder};

pub struct MetricsBatchBuilder {
    schema_helper: SchemaHelper,
    builders:
        HashMap<String /*schema*/, HashMap<RegionId /*physical table name*/, BatchEncoder>>,
    partition_manager: PartitionRuleManagerRef,
    node_manager: NodeManagerRef,
}

#[derive(Debug, Default)]
pub struct AppendMetrics {
    // append_rows_total
    //  |- append_inner
    //      |- create_row_iter
    //      |- encode_pk_inner
    //      |- encode_pk_tags
    //      |- push_row
    //          |- push_row_inner
    //              |- push_pk
    //              |- push_value
    //              |- push_timestamp
    pub append_rows_total: Duration,
    pub append_inner: Duration,
    pub encode_pk_inner: Duration,
    pub encode_pk_tags: Duration,
    pub create_row_iter: Duration,
    pub push_row: Duration,
    push_row_inner: Duration,
    push_pk: Duration,
    push_value: Duration,
    push_timestamp: Duration,

    pub physical_region_meta: Duration,
}

impl Drop for AppendMetrics {
    fn drop(&mut self) {
        METRIC_BULK_ALTER_TABLE
            .with_label_values(&["append_inner"])
            .observe(self.append_inner.as_secs_f64());
        METRIC_BULK_ALTER_TABLE
            .with_label_values(&["encode_pk_inner"])
            .observe(self.encode_pk_inner.as_secs_f64());
        METRIC_BULK_ALTER_TABLE
            .with_label_values(&["encode_pk_tags"])
            .observe(self.encode_pk_tags.as_secs_f64());
        METRIC_BULK_ALTER_TABLE
            .with_label_values(&["push_row"])
            .observe(self.push_row.as_secs_f64());
        METRIC_BULK_ALTER_TABLE
            .with_label_values(&["create_row_iter"])
            .observe(self.create_row_iter.as_secs_f64());
        METRIC_BULK_ALTER_TABLE
            .with_label_values(&["push_row_inner"])
            .observe(self.push_row_inner.as_secs_f64());
        METRIC_BULK_ALTER_TABLE
            .with_label_values(&["push_pk"])
            .observe(self.push_pk.as_secs_f64());
        METRIC_BULK_ALTER_TABLE
            .with_label_values(&["push_timestamp"])
            .observe(self.push_timestamp.as_secs_f64());
        METRIC_BULK_ALTER_TABLE
            .with_label_values(&["push_value"])
            .observe(self.push_value.as_secs_f64());
        METRIC_BULK_ALTER_TABLE
            .with_label_values(&["physical_region_meta"])
            .observe(self.physical_region_meta.as_secs_f64());
        METRIC_BULK_ALTER_TABLE
            .with_label_values(&["append_rows_total"])
            .observe(self.append_rows_total.as_secs_f64());
    }
}

impl MetricsBatchBuilder {
    pub fn new(
        schema_helper: SchemaHelper,
        partition_manager: PartitionRuleManagerRef,
        node_manager: NodeManagerRef,
    ) -> Self {
        MetricsBatchBuilder {
            schema_helper,
            builders: Default::default(),
            partition_manager,
            node_manager,
        }
    }

    /// Retrieves physical region metadata of given logical table names.
    ///
    /// The `logical_tables` is a list of table names, each entry contains the schema name and the table name.
    /// Returns the following mapping: `schema => logical table => (logical table id, region 0 metadata of the physical table)`.
    pub(crate) async fn collect_physical_region_metadata(
        &self,
        logical_tables: &[(String, String)],
        query_ctx: &QueryContextRef,
    ) -> error::Result<
        HashMap<
            String,
            HashMap<String, (TableId, RegionMetadataRef, Arc<HashMap<String, ColumnId>>)>,
        >,
    > {
        let catalog = query_ctx.current_catalog();
        // Logical and physical table ids.
        let mut table_ids = Vec::with_capacity(logical_tables.len());
        let mut physical_region_ids = HashSet::new();
        for (schema, table_name) in logical_tables {
            let logical_table = self
                .schema_helper
                .get_table(catalog, schema, table_name)
                .await
                .context(error::OperatorSnafu)?
                .context(error::TableNotFoundSnafu {
                    catalog,
                    schema,
                    table: table_name,
                })?;
            let logical_table_id = logical_table.table_info().table_id();
            let physical_table_id = self
                .schema_helper
                .table_route_manager()
                .get_physical_table_id(logical_table_id)
                .await
                .context(error::CommonMetaSnafu)?;
            table_ids.push((logical_table_id, physical_table_id));
            // We only get metadata from region 0.
            physical_region_ids.insert(RegionId::new(physical_table_id, 0));
        }

        // Batch get physical metadata.
        let physical_region_ids = physical_region_ids.into_iter().collect_vec();
        let region_metadatas = metadatas_for_region_ids(
            &self.partition_manager,
            &self.node_manager,
            &physical_region_ids,
            query_ctx,
        )
        .await
        .context(error::OperatorSnafu)?;
        let mut result_map: HashMap<_, HashMap<_, _>> = HashMap::new();
        let region_metadatas: HashMap<_, _> = region_metadatas
            .into_iter()
            .flatten()
            .map(|meta| {
                let name_to_id: HashMap<_, _> = meta
                    .column_metadatas
                    .iter()
                    .map(|c| (c.column_schema.name.clone(), c.column_id))
                    .collect();
                (meta.region_id, (Arc::new(meta), Arc::new(name_to_id)))
            })
            .collect();

        for (i, (schema, table_name)) in logical_tables.iter().enumerate() {
            let physical_table_id = table_ids[i].1;
            let physical_region_id = RegionId::new(physical_table_id, 0);
            let (physical_metadata, name_to_id) = region_metadatas
                .get(&physical_region_id)
                .with_context(|| error::UnexpectedResultSnafu {
                    reason: format!(
                        "Physical region metadata {} for table {} not found",
                        physical_region_id, table_name
                    ),
                })?;

            match result_map.get_mut(schema) {
                Some(table_map) => {
                    table_map.insert(
                        table_name.clone(),
                        (
                            table_ids[i].0,
                            physical_metadata.clone(),
                            name_to_id.clone(),
                        ),
                    );
                }
                None => {
                    let mut table_map = HashMap::new();
                    table_map.insert(
                        table_name.clone(),
                        (
                            table_ids[i].0,
                            physical_metadata.clone(),
                            name_to_id.clone(),
                        ),
                    );
                    result_map.insert(schema.to_string(), table_map);
                }
            }
        }

        Ok(result_map)
    }

    /// Builds [RecordBatch] from rows with primary key encoded.
    /// Potentially we also need to modify the column name of timestamp and value field to
    /// match the schema of physical tables.
    /// Note:
    /// Make sure all logical table and physical table are created when reach here and the mapping
    /// from logical table name to physical table ref is stored in [physical_region_metadata].
    pub(crate) fn append_rows_to_batch(
        &mut self,
        current_catalog: Option<String>,
        current_schema: Option<String>,
        table_data: &mut HashMap<PromCtx, HashMap<String, TableBuilder>>,
        physical_region_metadata: &HashMap<
            String, /*schema name*/
            HashMap<
                String, /*logical table name*/
                (
                    TableId, /*logical table id*/
                    RegionMetadataRef,
                    Arc<HashMap<String, ColumnId>>,
                ),
            >,
        >,
        metrics: &mut AppendMetrics,
    ) -> error::Result<()> {
        for (ctx, tables_in_schema) in table_data {
            // use session catalog.
            let catalog = current_catalog.as_deref().unwrap_or(DEFAULT_CATALOG_NAME);
            // schema in PromCtx precedes session schema.
            let schema = ctx
                .schema
                .as_deref()
                .or(current_schema.as_deref())
                .unwrap_or(DEFAULT_SCHEMA_NAME);
            // Look up physical region metadata by schema and table name
            let schema_metadata =
                physical_region_metadata
                    .get(schema)
                    .context(error::TableNotFoundSnafu {
                        catalog,
                        schema,
                        table: "",
                    })?;

            for (logical_table_name, table) in tables_in_schema {
                let (logical_table_id, physical_table, name_to_id) = schema_metadata
                    .get(logical_table_name)
                    .context(error::TableNotFoundSnafu {
                        catalog,
                        schema,
                        table: logical_table_name,
                    })?;

                let encoder = self
                    .builders
                    .entry(schema.to_string())
                    .or_default()
                    .entry(physical_table.region_id)
                    .or_insert_with(|| Self::create_sparse_encoder(&physical_table));
                let start = Instant::now();
                encoder.append_rows(
                    *logical_table_id,
                    std::mem::take(table),
                    &name_to_id,
                    metrics,
                )?;
                metrics.append_inner += start.elapsed();
            }
        }

        Ok(())
    }

    /// Finishes current record batch builder and returns record batches grouped by physical table id.
    pub(crate) fn finish(
        self,
    ) -> error::Result<
        HashMap<
            String, /*schema name*/
            HashMap<RegionId /*physical region id*/, Vec<(RecordBatch, (i64, i64))>>,
        >,
    > {
        let mut table_batches: HashMap<String, HashMap<RegionId, Vec<(RecordBatch, (i64, i64))>>> =
            HashMap::with_capacity(self.builders.len());

        for (schema_name, schema_tables) in self.builders {
            let schema_batches = table_batches.entry(schema_name).or_default();
            for (physical_region_id, table_data) in schema_tables {
                let rb = table_data.finish()?;
                if !rb.is_empty() {
                    schema_batches
                        .entry(physical_region_id)
                        .or_default()
                        .extend(rb);
                }
            }
        }
        Ok(table_batches)
    }

    /// Creates Encoder that converts Rows into RecordBatch with primary key encoded.
    fn create_sparse_encoder(physical_region_meta: &RegionMetadataRef) -> BatchEncoder {
        let name_to_id: HashMap<_, _> = physical_region_meta
            .column_metadatas
            .iter()
            .map(|c| (c.column_schema.name.clone(), c.column_id))
            .collect();
        BatchEncoder::new(name_to_id)
    }
}

/// Detected the DDL requirements according to the staged table rows.
pub async fn create_or_alter_physical_tables(
    schema_helper: &SchemaHelper,
    tables: &HashMap<PromCtx, HashMap<String, TableBuilder>>,
    query_ctx: &QueryContextRef,
) -> error::Result<()> {
    // Physical table name -> logical tables -> tags in logical table
    let mut tags: HashMap<String, HashMap<String, HashSet<String>>> = HashMap::default();
    let catalog = query_ctx.current_catalog();
    let schema = query_ctx.current_schema();

    for (ctx, tables) in tables {
        for (logical_table_name, table_builder) in tables {
            let physical_table_name = schema_helper
                .determine_physical_table_name(
                    logical_table_name,
                    &ctx.physical_table,
                    catalog,
                    &schema,
                )
                .await
                .context(error::OperatorSnafu)?;
            tags.entry(physical_table_name)
                .or_default()
                .entry(logical_table_name.clone())
                .or_default()
                .extend(table_builder.tags().cloned());
        }
    }
    let logical_schemas = tags_to_logical_schemas(tags);
    ensure_logical_tables_for_metrics(schema_helper, &logical_schemas, query_ctx)
        .await
        .context(error::OperatorSnafu)?;
    Ok(())
}

struct Columns {
    encoded_primary_key_array_builder: NonNullBinaryBuilder,
    timestamps: Vec<i64>,
    value: Vec<f64>,
    timestamp_range: Option<(i64, i64)>,
}

impl Columns {
    fn with_capacity(cap: usize, avg_pk_len: Option<usize>) -> Self {
        Self {
            encoded_primary_key_array_builder: NonNullBinaryBuilder::with_capacity(
                cap,
                cap * avg_pk_len.unwrap_or(10),
            ),
            timestamps: Vec::with_capacity(cap),
            value: Vec::with_capacity(cap),
            timestamp_range: None,
        }
    }

    fn reserve(&mut self, additional: usize) {
        self.encoded_primary_key_array_builder.reserve(additional);
        self.timestamps.reserve(additional);
        self.value.reserve(additional);
    }

    fn pk_offset(&self) -> usize {
        self.encoded_primary_key_array_builder.len()
    }

    fn estimated_size(&self) -> usize {
        let value_size = self.encoded_primary_key_array_builder.value_builder.len();
        let offset_size = self.encoded_primary_key_array_builder.value_builder.len() * 4;
        let timestamp_size = self.timestamps.len() * 8 + std::mem::size_of::<Vec<i64>>();
        let val_size = self.value.len() * 8 + std::mem::size_of::<Vec<f64>>();
        value_size + offset_size + timestamp_size + val_size + size_of::<Self>()
    }

    fn push(&mut self, pk: &[u8], val: f64, timestamp: i64, metrics: &mut AppendMetrics) {
        let start = Instant::now();
        self.encoded_primary_key_array_builder.append(&pk);
        metrics.push_pk += start.elapsed();

        let start = Instant::now();
        self.value.push(val);
        metrics.push_value += start.elapsed();

        let start = Instant::now();
        self.timestamps.push(timestamp);
        metrics.push_timestamp += start.elapsed();
        if let Some((min, max)) = &mut self.timestamp_range {
            *min = (*min).min(timestamp);
            *max = (*max).max(timestamp);
        } else {
            self.timestamp_range = Some((timestamp, timestamp));
        }
    }
}

struct ColumnsBuilder {
    columns: Vec<Columns>,
}

impl ColumnsBuilder {
    pub fn new(initial_cap: usize) -> Self {
        let columns = Columns::with_capacity(initial_cap, None);
        Self {
            columns: vec![columns],
        }
    }
}

impl ColumnsBuilder {
    fn reserve(&mut self, additional: usize) {
        self.columns.last_mut().unwrap().reserve(additional);
    }

    fn push(
        &mut self,
        pk: &[u8],
        val: f64,
        ts: i64,
        metrics: &mut AppendMetrics,
        remaining_rows: usize,
    ) {
        let mut last_builder = self.columns.last_mut().unwrap();
        if last_builder.pk_offset() + pk.len() >= i32::MAX as usize {
            let avg_pk_size = last_builder.pk_offset() / last_builder.timestamps.len();
            info!(
                "Current builder is full {}, rows: {}, avg pk size: {}",
                last_builder.pk_offset(),
                last_builder.timestamps.len(),
                avg_pk_size
            );
            // Current builder is full, create a new one
            self.columns
                .push(Columns::with_capacity(remaining_rows, Some(avg_pk_size)));
            last_builder = self.columns.last_mut().unwrap()
        };

        let start = Instant::now();
        last_builder.push(pk, val, ts, metrics);
        metrics.push_row_inner += start.elapsed();
    }
}

struct BatchEncoder {
    name_to_id: HashMap<String, ColumnId>,
    pk_codec: SparsePrimaryKeyCodec,
    columns_builder: ColumnsBuilder,
}

impl BatchEncoder {
    fn new(name_to_id: HashMap<String, ColumnId>) -> BatchEncoder {
        Self {
            name_to_id,
            pk_codec: SparsePrimaryKeyCodec::schemaless(),
            columns_builder: ColumnsBuilder::new(16384),
        }
    }

    pub(crate) fn estimated_size(&self) -> usize {
        self.columns_builder
            .columns
            .iter()
            .map(|v| v.estimated_size())
            .sum()
    }

    pub(crate) fn total_rows(&self) -> usize {
        self.columns_builder
            .columns
            .iter()
            .map(|v| v.timestamps.len())
            .sum()
    }

    fn append_rows(
        &mut self,
        logical_table_id: TableId,
        mut table_builder: TableBuilder,
        name_to_id: &HashMap<String, ColumnId>,
        metrics: &mut AppendMetrics,
    ) -> error::Result<()> {
        // todo(hl): we can simplified the row iter because schema in TableBuilder is known (ts, val, tags...)
        let row_insert_request = table_builder.as_row_insert_request("don't care".to_string());

        let start = Instant::now();
        let rows = row_insert_request.rows.unwrap();
        let num_rows = rows.rows.len();
        let mut iter = RowsIter::new(rows, name_to_id);
        metrics.create_row_iter += start.elapsed();

        let mut encode_buf = vec![];
        let mut rows_written = 0;

        self.columns_builder.reserve(num_rows);
        for row in iter.iter_mut() {
            encode_buf.clear();
            let start = Instant::now();
            let (table_id, ts_id) = RowModifier::fill_internal_columns(logical_table_id, &row);
            let internal_columns = [
                (
                    ReservedColumnId::table_id(),
                    api::helper::pb_value_to_value_ref(&table_id, &None),
                ),
                (
                    ReservedColumnId::tsid(),
                    api::helper::pb_value_to_value_ref(&ts_id, &None),
                ),
            ];
            self.pk_codec
                .encode_to_vec(internal_columns.into_iter(), &mut encode_buf)
                .context(error::EncodePrimaryKeySnafu)?;
            metrics.encode_pk_inner += start.elapsed();

            let start = Instant::now();
            self.pk_codec
                .encode_to_vec(row.primary_keys(), &mut encode_buf)
                .context(error::EncodePrimaryKeySnafu)?;
            metrics.encode_pk_tags += start.elapsed();

            // safety: field values cannot be null in prom remote write
            let ValueData::F64Value(val) = row.value_at(1).value_data.as_ref().unwrap() else {
                return error::InvalidFieldValueTypeSnafu.fail();
            };
            // process timestamp and field. We already know the position of timestamps and values in [TableBuilder].
            let ValueData::TimestampMillisecondValue(ts) =
                // safety: timestamp values cannot be null
                row.value_at(0).value_data.as_ref().unwrap()
            else {
                return error::InvalidTimestampValueTypeSnafu.fail();
            };

            let start = Instant::now();
            let remaining_rows = num_rows - rows_written;
            self.columns_builder
                .push(&encode_buf, *val, *ts, metrics, remaining_rows);
            metrics.push_row += start.elapsed();
            rows_written += 1;
        }
        Ok(())
    }

    fn finish(self) -> error::Result<Vec<(RecordBatch, (i64, i64))>> {
        if self.columns_builder.columns.is_empty() {
            return Ok(vec![]);
        }

        let mut res = Vec::with_capacity(self.columns_builder.columns.len());

        for mut columns in self.columns_builder.columns {
            let num_rows = columns.timestamps.len();
            let value = Float64Array::from(columns.value);
            let timestamp = TimestampMillisecondArray::from(columns.timestamps);

            let op_type = Arc::new(UInt8Array::from_value(OpType::Put as u8, num_rows)) as ArrayRef;
            // todo: now we set sequence all to 0.
            let sequence = Arc::new(UInt64Array::from_value(0, num_rows)) as ArrayRef;

            let pk = columns.encoded_primary_key_array_builder.build();
            let indices = compute::sort_to_indices(&pk, None, None).context(error::ArrowSnafu)?;

            // Sort arrays
            let value = compute::take(&value, &indices, None).context(error::ArrowSnafu)?;
            let ts = compute::take(&timestamp, &indices, None).context(error::ArrowSnafu)?;
            let pk = compute::take(&pk, &indices, None).context(error::ArrowSnafu)?;
            let rb =
                RecordBatch::try_new(physical_schema(), vec![value, ts, pk, sequence, op_type])
                    .context(error::ArrowSnafu)?;
            res.push((rb, columns.timestamp_range.unwrap()))
        }

        Ok(res)
    }
}

fn tags_to_logical_schemas(
    tags: HashMap<String, HashMap<String, HashSet<String>>>,
) -> LogicalSchemas {
    let schemas: HashMap<String, Vec<LogicalSchema>> = tags
        .into_iter()
        .map(|(physical, logical_tables)| {
            let schemas: Vec<_> = logical_tables
                .into_iter()
                .map(|(logical, tags)| {
                    let mut columns: Vec<_> = tags
                        .into_iter()
                        .map(|tag_name| ColumnSchema {
                            column_name: tag_name,
                            datatype: ColumnDataType::String as i32,
                            semantic_type: SemanticType::Tag as i32,
                            ..Default::default()
                        })
                        .collect();
                    columns.push(ColumnSchema {
                        column_name: GREPTIME_TIMESTAMP.to_string(),
                        datatype: ColumnDataType::TimestampMillisecond as i32,
                        semantic_type: SemanticType::Timestamp as i32,
                        ..Default::default()
                    });
                    columns.push(ColumnSchema {
                        column_name: GREPTIME_VALUE.to_string(),
                        datatype: ColumnDataType::Float64 as i32,
                        semantic_type: SemanticType::Field as i32,
                        ..Default::default()
                    });
                    LogicalSchema {
                        name: logical,
                        columns,
                    }
                })
                .collect();
            (physical, schemas)
        })
        .collect();

    LogicalSchemas { schemas }
}

/// Creates the schema of output record batch.
pub fn physical_schema() -> arrow::datatypes::SchemaRef {
    Arc::new(arrow::datatypes::Schema::new(vec![
        Field::new(GREPTIME_VALUE, arrow::datatypes::DataType::Float64, false),
        Field::new(
            GREPTIME_TIMESTAMP,
            arrow::datatypes::DataType::Timestamp(arrow::datatypes::TimeUnit::Millisecond, None),
            false,
        ),
        Field::new(
            PRIMARY_KEY_COLUMN_NAME,
            arrow::datatypes::DataType::Binary,
            false,
        ),
        Field::new(
            SEQUENCE_COLUMN_NAME,
            arrow::datatypes::DataType::UInt64,
            false,
        ),
        Field::new(
            OP_TYPE_COLUMN_NAME,
            arrow::datatypes::DataType::UInt8,
            false,
        ),
    ]))
}

pub(crate) struct NonNullBinaryBuilder {
    value_builder: UInt8BufferBuilder,
    offsets_builder: BufferBuilder<i32>,
}

impl Default for NonNullBinaryBuilder {
    fn default() -> Self {
        Self::with_capacity(16, 256)
    }
}

impl NonNullBinaryBuilder {
    /// Creates a new [`GenericByteBuilder`].
    ///
    /// - `item_capacity` is the number of items to pre-allocate.
    ///   The size of the preallocated buffer of offsets is the number of items plus one.
    /// - `data_capacity` is the total number of bytes of data to pre-allocate
    ///   (for all items, not per item).
    pub fn with_capacity(item_capacity: usize, data_capacity: usize) -> Self {
        let mut offsets_builder = BufferBuilder::<i32>::new(item_capacity + 1);
        offsets_builder.append(0);
        Self {
            value_builder: UInt8BufferBuilder::new(data_capacity),
            offsets_builder,
        }
    }

    pub fn append(&mut self, data: &[u8]) {
        self.value_builder.append_slice(data);
        self.offsets_builder.append(self.next_offset());
    }

    #[inline]
    fn next_offset(&self) -> i32 {
        i32::try_from(self.value_builder.len()).expect("byte array offset overflow")
    }

    pub fn len(&self) -> usize {
        self.offsets_builder.len() - 1
    }

    pub fn reserve(&mut self, additional: usize) {
        self.offsets_builder.reserve(additional);
        let avg_item_size = if self.len() == 0 {
            1
        } else {
            self.value_builder.len() / self.len()
        };
        self.value_builder.reserve(avg_item_size * additional);
    }

    pub fn build(&mut self) -> BinaryArray {
        let array_builder = ArrayDataBuilder::new(arrow::datatypes::DataType::Binary)
            .len(self.len())
            .add_buffer(self.offsets_builder.finish())
            .add_buffer(self.value_builder.finish());

        self.offsets_builder.append(self.next_offset());
        let array_data = unsafe { array_builder.build_unchecked() };
        GenericByteArray::from(array_data)
    }
}

#[cfg(test)]
mod tests {
    use crate::batch_builder::NonNullBinaryBuilder;

    #[test]
    fn test_binary_builder() {
        let mut builder = NonNullBinaryBuilder::with_capacity(1, 10);
        builder.append("a".as_bytes());
        builder.append("b".as_bytes());
        builder.append("cdefg".as_bytes());
        let array = builder.build();
        assert_eq!(
            array
                .iter()
                .map(|v| String::from_utf8_lossy(v.unwrap()))
                .collect::<Vec<_>>(),
            vec!["a", "b", "cdefg"]
        );
    }
}
