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

//! Trash index for soft-deleted tables.
//!
//! A trash entry records that a table has been soft-deleted and tracks the
//! information needed to either restore it (via UNDROP) or hard-drop it once
//! the retention window has elapsed (via the trash purger).
//!
//! Key layout: `__trash/{dropped_at_millis:020}/{table_id}`
//!
//! `dropped_at_millis` is zero-padded to 20 digits so that a plain
//! lexicographic range scan over `__trash/` yields entries in chronological
//! order. Expiration selection is a single left-bounded range scan up to
//! `__trash/{now - retention:020}/`.
//!
//! `table_id` is globally unique and never reused, so the pair
//! `(dropped_at_millis, table_id)` always identifies a distinct soft-drop
//! event.
//!
//! This module is a pure metadata primitive. It does not itself trigger any
//! tombstone, close-region, or GC activity. Later PRs wire it into the
//! DROP / UNDROP / purge flows.

use std::fmt::Display;

use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use snafu::{OptionExt, ResultExt};
use table::metadata::TableId;

use crate::error::{InvalidMetadataSnafu, Result, SerdeJsonSnafu};
use crate::key::{MetadataKey, MetadataValue, TRASH_KEY_PREFIX};
use crate::kv_backend::KvBackendRef;
use crate::kv_backend::txn::Txn;
use crate::range_stream::{DEFAULT_PAGE_SIZE, PaginationStream};
use crate::rpc::KeyValue;
use crate::rpc::store::{BatchDeleteRequest, RangeRequest};

/// Width of the zero-padded `dropped_at_millis` segment. i64 max is 19
/// digits; we pad to 20 so any non-negative timestamp sorts lexicographically
/// in the same order as numerically.
pub const DROPPED_AT_WIDTH: usize = 20;

/// Identifier of a soft-deleted table within the trash index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TrashEntryKey {
    pub dropped_at_millis: i64,
    pub table_id: TableId,
}

impl TrashEntryKey {
    pub fn new(dropped_at_millis: i64, table_id: TableId) -> Self {
        Self {
            dropped_at_millis,
            table_id,
        }
    }

    /// Returns the range prefix for scanning the whole trash index.
    pub fn range_prefix() -> Vec<u8> {
        format!("{TRASH_KEY_PREFIX}/").into_bytes()
    }

    /// Returns the exclusive upper bound for a scan of all entries whose
    /// `dropped_at_millis` is strictly less than `cutoff_millis`.
    ///
    /// Entries with `dropped_at_millis == cutoff_millis` are excluded. Pass
    /// `now - retention` here to select entries whose retention window has
    /// elapsed.
    pub fn upper_bound_for_cutoff(cutoff_millis: i64) -> Vec<u8> {
        format!(
            "{TRASH_KEY_PREFIX}/{:0>width$}/",
            cutoff_millis.max(0),
            width = DROPPED_AT_WIDTH
        )
        .into_bytes()
    }
}

fn format_dropped_at(dropped_at_millis: i64) -> String {
    format!(
        "{:0>width$}",
        dropped_at_millis.max(0),
        width = DROPPED_AT_WIDTH
    )
}

impl Display for TrashEntryKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{TRASH_KEY_PREFIX}/{}/{}",
            format_dropped_at(self.dropped_at_millis),
            self.table_id,
        )
    }
}

impl MetadataKey<'_, TrashEntryKey> for TrashEntryKey {
    fn to_bytes(&self) -> Vec<u8> {
        self.to_string().into_bytes()
    }

    fn from_bytes(bytes: &[u8]) -> Result<TrashEntryKey> {
        let key = std::str::from_utf8(bytes).map_err(|e| {
            InvalidMetadataSnafu {
                err_msg: format!(
                    "TrashEntryKey '{}' is not a valid UTF8 string: {e}",
                    String::from_utf8_lossy(bytes)
                ),
            }
            .build()
        })?;

        let rest = key
            .strip_prefix(TRASH_KEY_PREFIX)
            .and_then(|s| s.strip_prefix('/'))
            .with_context(|| InvalidMetadataSnafu {
                err_msg: format!("TrashEntryKey '{key}' missing prefix '{TRASH_KEY_PREFIX}/'"),
            })?;

        let (dropped_at_str, table_id_str) =
            rest.split_once('/').with_context(|| InvalidMetadataSnafu {
                err_msg: format!("TrashEntryKey '{key}' missing table_id segment"),
            })?;

        if table_id_str.is_empty() || table_id_str.contains('/') {
            return InvalidMetadataSnafu {
                err_msg: format!("TrashEntryKey '{key}' has invalid table_id segment"),
            }
            .fail();
        }

        let dropped_at_millis: i64 =
            dropped_at_str
                .parse()
                .ok()
                .with_context(|| InvalidMetadataSnafu {
                    err_msg: format!(
                        "TrashEntryKey '{key}' has non-numeric dropped_at '{dropped_at_str}'"
                    ),
                })?;
        let table_id: TableId =
            table_id_str
                .parse()
                .ok()
                .with_context(|| InvalidMetadataSnafu {
                    err_msg: format!(
                        "TrashEntryKey '{key}' has non-numeric table_id '{table_id_str}'"
                    ),
                })?;

        Ok(TrashEntryKey {
            dropped_at_millis,
            table_id,
        })
    }
}

/// Value stored under a [`TrashEntryKey`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrashEntry {
    pub table_id: TableId,
    pub original_catalog: String,
    pub original_schema: String,
    pub original_name: String,
    pub dropped_at_millis: i64,
    pub retention_millis: i64,
}

impl TrashEntry {
    /// The absolute time (in epoch millis) at which this entry becomes
    /// eligible for hard-drop by the purger.
    pub fn expires_at_millis(&self) -> i64 {
        self.dropped_at_millis.saturating_add(self.retention_millis)
    }
}

impl MetadataValue for TrashEntry {
    fn try_from_raw_value(raw_value: &[u8]) -> Result<Self> {
        serde_json::from_slice::<TrashEntry>(raw_value).context(SerdeJsonSnafu)
    }

    fn try_as_raw_value(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self).context(SerdeJsonSnafu)
    }
}

fn trash_decoder(kv: KeyValue) -> Result<(TrashEntryKey, TrashEntry)> {
    let key = TrashEntryKey::from_bytes(&kv.key)?;
    let value = TrashEntry::try_from_raw_value(&kv.value)?;
    Ok((key, value))
}

/// Manages trash entries in the metasrv KV backend.
///
/// The manager owns only the `__trash/` key space. The soft-drop helper
/// introduced in a subsequent PR is responsible for ordering the trash insert
/// with respect to the tombstone move so that the index and the tombstoned
/// metadata are always consistent on recovery.
pub struct TrashManager {
    kv_backend: KvBackendRef,
}

pub type TrashManagerRef = std::sync::Arc<TrashManager>;

impl TrashManager {
    pub fn new(kv_backend: KvBackendRef) -> Self {
        Self { kv_backend }
    }

    /// Inserts a new trash entry. Fails if the key is already occupied.
    pub async fn insert(&self, key: &TrashEntryKey, value: &TrashEntry) -> Result<()> {
        let raw_key = key.to_bytes();
        let raw_value = value.try_as_raw_value()?;
        let txn = Txn::put_if_not_exists(raw_key.clone(), raw_value);
        let resp = self.kv_backend.txn(txn).await?;
        if !resp.succeeded {
            return InvalidMetadataSnafu {
                err_msg: format!(
                    "Trash entry already exists for key '{}'",
                    String::from_utf8_lossy(&raw_key)
                ),
            }
            .fail();
        }
        Ok(())
    }

    pub async fn get(&self, key: &TrashEntryKey) -> Result<Option<TrashEntry>> {
        self.kv_backend
            .get(&key.to_bytes())
            .await?
            .map(|kv| TrashEntry::try_from_raw_value(&kv.value))
            .transpose()
    }

    /// Removes a trash entry. Deleting a non-existent key is a no-op.
    pub async fn delete(&self, key: &TrashEntryKey) -> Result<()> {
        let _ = self
            .kv_backend
            .batch_delete(BatchDeleteRequest::new().with_keys(vec![key.to_bytes()]))
            .await?;
        Ok(())
    }

    /// Lists all trash entries in chronological (ascending) order.
    pub async fn list_all(&self) -> Result<Vec<(TrashEntryKey, TrashEntry)>> {
        self.scan_range(TrashEntryKey::range_prefix(), None).await
    }

    /// Lists entries whose `dropped_at_millis` is strictly less than
    /// `cutoff_millis`. Entries at the cutoff are excluded.
    ///
    /// Callers typically pass `now - retention`. Results are bounded by
    /// `limit`; pass `0` for unlimited.
    pub async fn list_before(
        &self,
        cutoff_millis: i64,
        limit: usize,
    ) -> Result<Vec<(TrashEntryKey, TrashEntry)>> {
        let start = TrashEntryKey::range_prefix();
        let end = TrashEntryKey::upper_bound_for_cutoff(cutoff_millis);
        let limit = if limit == 0 { None } else { Some(limit) };
        self.scan_range_with_end(start, end, limit).await
    }

    async fn scan_range(
        &self,
        prefix: Vec<u8>,
        limit: Option<usize>,
    ) -> Result<Vec<(TrashEntryKey, TrashEntry)>> {
        let req = RangeRequest::new().with_prefix(prefix);
        self.collect(req, limit).await
    }

    async fn scan_range_with_end(
        &self,
        start: Vec<u8>,
        end: Vec<u8>,
        limit: Option<usize>,
    ) -> Result<Vec<(TrashEntryKey, TrashEntry)>> {
        let req = RangeRequest::new().with_range(start, end);
        self.collect(req, limit).await
    }

    async fn collect(
        &self,
        req: RangeRequest,
        limit: Option<usize>,
    ) -> Result<Vec<(TrashEntryKey, TrashEntry)>> {
        let stream = PaginationStream::new(
            self.kv_backend.clone(),
            req,
            DEFAULT_PAGE_SIZE,
            trash_decoder,
        )
        .into_stream();

        let mut entries = stream.try_collect::<Vec<_>>().await?;
        if let Some(limit) = limit {
            entries.truncate(limit);
        }
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::kv_backend::memory::MemoryKvBackend;

    fn sample_entry(dropped_at: i64, retention: i64, table_id: TableId) -> TrashEntry {
        TrashEntry {
            table_id,
            original_catalog: "greptime".to_string(),
            original_schema: "public".to_string(),
            original_name: "foo".to_string(),
            dropped_at_millis: dropped_at,
            retention_millis: retention,
        }
    }

    #[test]
    fn key_round_trip() {
        let key = TrashEntryKey::new(1_714_118_400_000, 7);
        let bytes = key.to_bytes();
        let encoded = std::str::from_utf8(&bytes).unwrap();
        assert_eq!(
            encoded, "__trash/00000001714118400000/7",
            "dropped_at segment must be zero-padded to {DROPPED_AT_WIDTH} digits"
        );
        let decoded = TrashEntryKey::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, key);
    }

    #[test]
    fn key_negative_dropped_at_clamps_in_display() {
        // Display clamps to 0 — callers are expected to pass non-negative
        // timestamps.
        let key = TrashEntryKey::new(-1, 1);
        let encoded = String::from_utf8(key.to_bytes()).unwrap();
        assert!(
            encoded.starts_with("__trash/00000000000000000000/"),
            "negative dropped_at should clamp to 0 in the key: {encoded}"
        );
    }

    #[test]
    fn key_rejects_invalid_bytes() {
        assert!(TrashEntryKey::from_bytes(b"not_a_trash_key").is_err());
        assert!(TrashEntryKey::from_bytes(b"__trash/abc/1").is_err());
        assert!(TrashEntryKey::from_bytes(b"__trash/000/notanum").is_err());
        assert!(TrashEntryKey::from_bytes(b"__trash/000/").is_err());
        assert!(TrashEntryKey::from_bytes(b"__trash/000").is_err());
    }

    #[test]
    fn lex_order_matches_chronological_order() {
        let mut keys = [
            TrashEntryKey::new(3_000, 1),
            TrashEntryKey::new(100, 2),
            TrashEntryKey::new(1_500, 3),
            TrashEntryKey::new(20, 4),
        ];
        keys.sort_by_key(|k| k.to_bytes());
        let stamps: Vec<_> = keys.iter().map(|k| k.dropped_at_millis).collect();
        assert_eq!(stamps, vec![20, 100, 1_500, 3_000]);
    }

    #[test]
    fn upper_bound_excludes_cutoff() {
        let at_cutoff = TrashEntryKey::new(5_000, 1).to_bytes();
        let before = TrashEntryKey::new(4_999, 1).to_bytes();
        let bound = TrashEntryKey::upper_bound_for_cutoff(5_000);
        assert!(before < bound, "entries before cutoff are < bound");
        assert!(at_cutoff >= bound, "entries at cutoff are >= bound");
    }

    #[test]
    fn entry_value_round_trip() {
        let entry = sample_entry(1_000, 30 * 86_400 * 1_000, 42);
        let raw = entry.try_as_raw_value().unwrap();
        let back = TrashEntry::try_from_raw_value(&raw).unwrap();
        assert_eq!(back, entry);
    }

    #[test]
    fn expires_at_saturates() {
        let entry = TrashEntry {
            retention_millis: i64::MAX,
            dropped_at_millis: 100,
            ..sample_entry(100, 0, 42)
        };
        assert_eq!(entry.expires_at_millis(), i64::MAX);
    }

    fn new_manager() -> TrashManager {
        TrashManager::new(Arc::new(MemoryKvBackend::default()))
    }

    #[tokio::test]
    async fn insert_get_delete_round_trip() {
        let mgr = new_manager();
        let key = TrashEntryKey::new(1_000, 7);
        let entry = sample_entry(1_000, 1, 7);

        assert!(mgr.get(&key).await.unwrap().is_none());
        mgr.insert(&key, &entry).await.unwrap();
        assert_eq!(mgr.get(&key).await.unwrap().as_ref(), Some(&entry));

        mgr.delete(&key).await.unwrap();
        assert!(mgr.get(&key).await.unwrap().is_none());
        // Delete is idempotent.
        mgr.delete(&key).await.unwrap();
    }

    #[tokio::test]
    async fn insert_rejects_duplicate_key() {
        let mgr = new_manager();
        let key = TrashEntryKey::new(1_000, 7);
        let entry = sample_entry(1_000, 1, 7);
        mgr.insert(&key, &entry).await.unwrap();
        let err = mgr.insert(&key, &entry).await.unwrap_err();
        assert!(
            format!("{err}").contains("already exists"),
            "expected duplicate-key error, got: {err}"
        );
    }

    #[tokio::test]
    async fn list_before_bounds_and_order() {
        let mgr = new_manager();
        let entries = [
            (TrashEntryKey::new(100, 1), sample_entry(100, 10, 1)),
            (TrashEntryKey::new(200, 2), sample_entry(200, 10, 2)),
            (TrashEntryKey::new(300, 3), sample_entry(300, 10, 3)),
            (TrashEntryKey::new(400, 4), sample_entry(400, 10, 4)),
        ];
        for (k, v) in &entries {
            mgr.insert(k, v).await.unwrap();
        }

        // Cutoff at 300 excludes the entry at exactly 300.
        let before = mgr.list_before(300, 0).await.unwrap();
        let stamps: Vec<_> = before.iter().map(|(k, _)| k.dropped_at_millis).collect();
        assert_eq!(stamps, vec![100, 200]);

        // Limit is respected.
        let limited = mgr.list_before(1_000, 2).await.unwrap();
        assert_eq!(limited.len(), 2);
        assert_eq!(limited[0].0.dropped_at_millis, 100);
        assert_eq!(limited[1].0.dropped_at_millis, 200);
    }

    #[tokio::test]
    async fn list_all_returns_chronological_order() {
        let mgr = new_manager();
        mgr.insert(&TrashEntryKey::new(500, 1), &sample_entry(500, 10, 1))
            .await
            .unwrap();
        mgr.insert(&TrashEntryKey::new(100, 2), &sample_entry(100, 10, 2))
            .await
            .unwrap();
        mgr.insert(&TrashEntryKey::new(300, 3), &sample_entry(300, 10, 3))
            .await
            .unwrap();

        let all = mgr.list_all().await.unwrap();
        let stamps: Vec<_> = all.iter().map(|(k, _)| k.dropped_at_millis).collect();
        assert_eq!(stamps, vec![100, 300, 500]);
    }
}
