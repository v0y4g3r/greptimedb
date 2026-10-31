// Copyright 2023-2026 GrepTime Inc.
//
// This file is part of the GreptimeDB Enterprise Edition and is licensed under
// the GreptimeDB Enterprise License. You may not use this file except in
// compliance with that license. A copy of the license is available at the root
// of this repository in the file LICENSE-ENTERPRISE.
//
// Unless required by applicable law or agreed to in writing, this software is
// distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND,
// either express or implied.

use std::sync::Arc;

use datafusion::execution::TaskContext;
use datafusion::execution::config::SessionConfig;
use datafusion::execution::memory_pool::GreedyMemoryPool;
use datafusion::execution::runtime_env::RuntimeEnvBuilder;
use datafusion::physical_expr::expressions::Column;
use datafusion::physical_expr::{LexOrdering, PhysicalSortExpr};
use datafusion::physical_plan::sorts::sort::SortExec;
use datafusion::physical_plan::streaming::{PartitionStream, StreamingTableExec};
use datafusion::physical_plan::{ExecutionPlan, SendableRecordBatchStream};
use datatypes::arrow::array::{Array, Int64Array, UInt64Array};
use datatypes::arrow::compute::SortOptions;
use datatypes::arrow::datatypes::SchemaRef;
use datatypes::arrow::record_batch::RecordBatch;
use futures::TryStreamExt;
use snafu::{OptionExt, ResultExt};
use store_api::storage::RegionId;
use tokio::sync::Mutex;

use crate::compaction::multi_region::source::{SharedPartition, SharedSource};
use crate::compaction::multi_region::{MultiRegionCompactionContext, MultiRegionCompactionPlan};
use crate::error::{InvalidMultiRegionCompactionSnafu, MultiRegionCompactionSortSnafu, Result};
use crate::read::FlatSource;
use crate::sst::parquet::flat_format::{
    primary_key_column_index, sequence_column_index, time_index_column_index,
};

pub(crate) fn sorted_stream(
    source: Arc<SharedSource>,
    plan: &MultiRegionCompactionPlan,
    context: &MultiRegionCompactionContext,
) -> Result<(SendableRecordBatchStream, Arc<SortExec>)> {
    let source = Arc::new(SharedPartition(source));
    let schema = source.schema().clone();
    let columns = schema.fields().len() - 3;
    // Engine order is key / timestamp / descending sequence. The final ordinal
    // preserves physical source ordering when normalized sequences tie.
    let ordering = [
        (columns, false),
        (columns + 1, false),
        (primary_key_column_index(columns), false),
        (time_index_column_index(columns), false),
        (sequence_column_index(columns), true),
        (columns + 2, false),
    ]
    .into_iter()
    .map(|(index, descending)| PhysicalSortExpr {
        expr: Arc::new(Column::new(schema.field(index).name(), index)),
        options: SortOptions {
            descending,
            nulls_first: true,
        },
    })
    .collect::<Vec<_>>();
    let ordering = LexOrdering::new(ordering).context(InvalidMultiRegionCompactionSnafu {
        reason: "Empty table sort ordering",
    })?;
    let input = StreamingTableExec::try_new(schema, vec![source], None, vec![], false, None)
        .context(MultiRegionCompactionSortSnafu)?;
    let sorter = Arc::new(SortExec::new(ordering, Arc::new(input)));
    let runtime = RuntimeEnvBuilder::new()
        .with_memory_pool(Arc::new(GreedyMemoryPool::new(plan.sort_memory_limit / 2)))
        .with_temp_file_path(&context.spill_directory)
        .with_max_temp_directory_size(plan.spill_limit)
        .with_max_spill_merge_fan_in(2)
        .build_arc()
        .context(MultiRegionCompactionSortSnafu)?;
    let config = SessionConfig::new()
        .with_batch_size(1024)
        .with_sort_spill_reservation_bytes(plan.sort_memory_limit / 4)
        .with_sort_in_place_threshold_bytes(plan.sort_memory_limit / 8);
    let task = Arc::new(
        TaskContext::default()
            .with_session_config(config)
            .with_runtime(runtime),
    );
    let stream = sorter
        .execute(0, task)
        .context(MultiRegionCompactionSortSnafu)?;
    Ok((stream, sorter))
}

pub(crate) fn spilled_bytes(sort: &SortExec) -> u64 {
    sort.metrics()
        .and_then(|metrics| metrics.spilled_bytes())
        .unwrap_or(0) as u64
}

struct Cursor {
    stream: SendableRecordBatchStream,
    pending: Option<RecordBatch>,
    schema: SchemaRef,
}

pub(crate) struct SortedGroups {
    cursor: Arc<Mutex<Cursor>>,
}

impl SortedGroups {
    pub(crate) fn new(stream: SendableRecordBatchStream) -> Self {
        let schema = stream.schema();
        Self {
            cursor: Arc::new(Mutex::new(Cursor {
                stream,
                pending: None,
                schema,
            })),
        }
    }

    pub(crate) async fn next_group(&mut self) -> Result<Option<(RegionId, FlatSource)>> {
        let mut cursor = self.cursor.lock().await;
        if cursor.pending.is_none() {
            cursor.pending = cursor
                .stream
                .try_next()
                .await
                .context(MultiRegionCompactionSortSnafu)?;
        }
        let Some(batch) = &cursor.pending else {
            return Ok(None);
        };
        let key = group_key(batch, 0)?;
        let projection: Vec<_> = (0..cursor.schema.fields().len() - 3).collect();
        let schema = Arc::new(
            cursor
                .schema
                .project(&projection)
                .context(crate::error::ComputeArrowSnafu)?,
        );
        let shared = self.cursor.clone();
        drop(cursor);
        let stream = async_stream::try_stream! {
            loop {
                let mut cursor = shared.lock().await;
                let batch = match cursor.pending.take() {
                    Some(batch) => Some(batch),
                    None => cursor.stream.try_next().await.context(MultiRegionCompactionSortSnafu)?,
                };
                let Some(batch) = batch else { break; };
                if group_key(&batch, 0)? != key { cursor.pending = Some(batch); break; }
                let mut end = 1;
                while end < batch.num_rows() && group_key(&batch, end)? == key { end += 1; }
                if end < batch.num_rows() { cursor.pending = Some(batch.slice(end, batch.num_rows() - end)); }
                let projected = batch.slice(0, end).project(&projection).context(crate::error::ComputeArrowSnafu)?;
                drop(cursor);
                yield projected;
            }
        };
        Ok(Some((
            RegionId::from_u64(key.0),
            FlatSource::new_stream(schema, Box::pin(stream)),
        )))
    }
}

fn group_key(batch: &RecordBatch, row: usize) -> Result<(u64, i64)> {
    let columns = batch.num_columns();
    let targets = batch
        .column(columns - 3)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .context(InvalidMultiRegionCompactionSnafu {
            reason: "Missing target sort column",
        })?;
    let windows = batch
        .column(columns - 2)
        .as_any()
        .downcast_ref::<Int64Array>()
        .context(InvalidMultiRegionCompactionSnafu {
            reason: "Missing window sort column",
        })?;
    if row >= targets.len() {
        return InvalidMultiRegionCompactionSnafu {
            reason: "Empty sorted batch",
        }
        .fail();
    }
    Ok((targets.value(row), windows.value(row)))
}
