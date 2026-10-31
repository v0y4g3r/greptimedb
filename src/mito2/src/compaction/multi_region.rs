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

//! Lossless multi-region SST compaction using a captured table partition rule.
//!
//! The caller selects and pins the inputs (for example, from LEVEL_SHARED), and
//! supplies their effective metadata in every target. This primitive deliberately
//! does not select files, change persisted levels, publish manifests, or fold row
//! versions. Output protection and eventual publication belong to its caller.

mod sort;
mod source;
#[cfg(test)]
mod tests;
mod writer;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use common_base::cancellation::CancellationHandle;
use partition::PartitionRuleRef;
use snafu::{OptionExt, ResultExt, ensure};
use store_api::metadata::RegionMetadataRef;
use store_api::storage::RegionId;

use crate::access_layer::AccessLayerRef;
use crate::cache::CacheManagerRef;
use crate::config::MitoConfig;
use crate::error::{InvalidMultiRegionCompactionSnafu, Result};
use crate::region::options::RegionOptions;
use crate::sst::file::{FileMeta, RegionFileId};
use crate::sst::parquet::SstInfo;
use crate::sst::parquet::writer::SstWriteGuard;

/// A target partition and the source versions currently visible in its sequence domain.
#[derive(Debug, Clone)]
pub struct MultiRegionTarget {
    pub metadata: RegionMetadataRef,
    pub options: RegionOptions,
    /// All selected sources, with the sequence metadata used by this target's reader.
    pub inputs: HashMap<RegionFileId, FileMeta>,
}

/// Immutable inputs, routing snapshot and execution limits for one compaction.
pub struct MultiRegionCompactionPlan {
    /// Captured table routing rule; the executor never fetches mutable routing metadata.
    pub partition_rule: PartitionRuleRef,
    /// Unique sources in stable tie-breaking order. The caller retains their GC protection.
    pub sources: Vec<FileMeta>,
    /// Metadata and routing must belong to the same physical-table snapshot.
    pub targets: BTreeMap<RegionId, MultiRegionTarget>,
    /// Memory budget for source batches and the external sort, not a process RSS cap.
    /// SST/index writer memory is governed separately by the supplied MitoConfig.
    pub sort_memory_limit: usize,
    pub spill_limit: u64,
    pub max_output_file_size: usize,
    pub window_seconds: i64,
}

/// Shared storage services and attempt-owned output/spill resources.
pub struct MultiRegionCompactionContext {
    pub access_layer: AccessLayerRef,
    pub cache: CacheManagerRef,
    pub config: Arc<MitoConfig>,
    pub spill_directory: PathBuf,
    pub protection: Arc<dyn SstWriteGuard>,
    pub cancellation: Arc<CancellationHandle>,
}

/// Source I/O and lossless output accounting, excluding footer and query reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MultiRegionCompactionStats {
    pub source_files: u64,
    pub source_rows: u64,
    /// Row-group page bytes fetched from object storage, excluding footer traffic and retries.
    pub source_store_page_bytes: u64,
    pub output_rows: u64,
    pub output_bytes: u64,
    pub spilled_bytes: u64,
}

/// Complete per-target outputs, including explicit empty partitions.
pub struct MultiRegionCompactionOutput {
    pub files: BTreeMap<RegionId, Vec<FileMeta>>,
    /// Complete footer/index statistics for target hook delivery; not fabricated from FileMeta.
    pub sst_infos: BTreeMap<RegionId, Vec<SstInfo>>,
    pub stats: MultiRegionCompactionStats,
}

#[derive(Default)]
struct SourceCounters {
    files: AtomicU64,
    rows: AtomicU64,
    store_page_bytes: AtomicU64,
}

/// Computes a complete result or an error. Any already-written outputs remain
/// owned by the attempt's durable protection; this function never purges sources.
pub async fn compact_regions(
    plan: MultiRegionCompactionPlan,
    context: MultiRegionCompactionContext,
) -> Result<MultiRegionCompactionOutput> {
    let metadata = validate_plan(&plan)?;
    if context.cancellation.is_cancelled() {
        return crate::error::CompactionCancelledSnafu.fail();
    }
    tokio::fs::create_dir_all(&context.spill_directory)
        .await
        .context(crate::error::CreateDirSnafu {
            dir: context.spill_directory.display().to_string(),
        })?;
    let counters = Arc::new(SourceCounters::default());
    let source = source::SharedSource::build(&plan, &context, metadata, counters.clone()).await?;
    let (stream, sort_plan) = sort::sorted_stream(source, &plan, &context)?;
    let mut groups = sort::SortedGroups::new(stream);
    let mut output = MultiRegionCompactionOutput {
        files: plan.targets.keys().map(|id| (*id, vec![])).collect(),
        sst_infos: plan.targets.keys().map(|id| (*id, vec![])).collect(),
        stats: MultiRegionCompactionStats::default(),
    };
    while let Some((region_id, source)) = groups.next_group().await? {
        let target = plan
            .targets
            .get(&region_id)
            .context(InvalidMultiRegionCompactionSnafu {
                reason: "Sorted output names an unknown target",
            })?;
        let (files, infos) = writer::write_target_group(target, source, &plan, &context).await?;
        output.stats.output_rows += files.iter().map(|file| file.num_rows).sum::<u64>();
        output.stats.output_bytes += files
            .iter()
            .map(|file| file.file_size + file.index_file_size)
            .sum::<u64>();
        output.files.entry(region_id).or_default().extend(files);
        output.sst_infos.entry(region_id).or_default().extend(infos);
    }
    output.stats.source_files = counters.files.load(Ordering::Relaxed);
    output.stats.source_rows = counters.rows.load(Ordering::Relaxed);
    output.stats.source_store_page_bytes = counters.store_page_bytes.load(Ordering::Relaxed);
    output.stats.spilled_bytes = sort::spilled_bytes(&sort_plan);
    ensure!(
        output.stats.source_files == plan.sources.len() as u64
            && output.stats.output_rows == output.stats.source_rows,
        InvalidMultiRegionCompactionSnafu {
            reason: "Multi-region compaction did not account for every source row"
        }
    );
    Ok(output)
}

fn validate_plan(plan: &MultiRegionCompactionPlan) -> Result<RegionMetadataRef> {
    ensure!(
        !plan.sources.is_empty()
            && plan.sort_memory_limit >= 1024 * 1024
            && plan.spill_limit > 0
            && plan.max_output_file_size > 0
            && plan.window_seconds > 0,
        InvalidMultiRegionCompactionSnafu {
            reason: "Invalid sources or execution budgets"
        }
    );
    let first = plan
        .targets
        .values()
        .next()
        .context(InvalidMultiRegionCompactionSnafu {
            reason: "No routing targets",
        })?;
    let mut sources = HashSet::new();
    for file in &plan.sources {
        ensure!(
            file.region_id.table_id() == first.metadata.region_id.table_id()
                && sources.insert(file.file_id()),
            InvalidMultiRegionCompactionSnafu {
                reason: "Expected unique source files from one physical table"
            }
        );
    }
    for (id, target) in &plan.targets {
        ensure!(
            *id == target.metadata.region_id
                && id.table_id() == first.metadata.region_id.table_id()
                && target.metadata.column_metadatas == first.metadata.column_metadatas
                && target.metadata.primary_key == first.metadata.primary_key
                && target.metadata.primary_key_encoding == first.metadata.primary_key_encoding,
            InvalidMultiRegionCompactionSnafu {
                reason: "Target schemas are not a consistent physical-table snapshot"
            }
        );
        ensure!(
            target.inputs.len() == sources.len()
                && target
                    .inputs
                    .iter()
                    .all(|(key, file)| *key == file.file_id() && sources.contains(key)),
            InvalidMultiRegionCompactionSnafu {
                reason: "Every target must describe the effective metadata of every selected source"
            }
        );
    }
    Ok(first.metadata.clone())
}
