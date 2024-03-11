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

use std::ops::Deref;
use std::slice;

use api::prom_store::remote::Sample;
use api::v1::RowInsertRequests;
use bytes::{Buf, Bytes};
use prost::encoding::message::merge;
use prost::encoding::{decode_key, decode_varint, WireType};
use prost::DecodeError;

use crate::prom_row_builder::TablesBuilder;
use crate::prom_store::METRIC_NAME_LABEL_BYTES;
use crate::repeated_field::{Clear, RepeatedField};

impl Clear for Sample {
    fn clear(&mut self) {}
}

#[derive(Default, Clone)]
pub struct PromLabel {
    pub name: Bytes,
    pub value: Bytes,
}

impl Clear for PromLabel {
    fn clear(&mut self) {}
}

impl PromLabel {
    pub fn merge_field(
        &mut self,
        tag: u32,
        wire_type: WireType,
        buf: &mut Bytes,
    ) -> Result<(), DecodeError> {
        const STRUCT_NAME: &str = "PromLabel";
        match tag {
            1u32 => {
                // decode label name
                let value = &mut self.name;
                merge_bytes(value, buf).map_err(|mut error| {
                    error.push(STRUCT_NAME, "name");
                    error
                })
            }
            2u32 => {
                // decode label value
                let value = &mut self.value;
                merge_bytes(value, buf).map_err(|mut error| {
                    error.push(STRUCT_NAME, "value");
                    error
                })
            }
            _ => prost::encoding::skip_field(wire_type, tag, buf, Default::default()),
        }
    }
}

#[inline(always)]
fn copy_to_bytes(data: &mut Bytes, len: usize) -> Bytes {
    if len == data.remaining() {
        core::mem::replace(data, Bytes::new())
    } else {
        let ret = split_to(data, len);
        data.advance(len);
        ret
    }
}

/// Similar to `Bytes::split_to`, but directly operates on underlying memory region.
/// # Safety
/// This function is safe as long as `data` is backed by a consecutive region of memory,
/// for example `Vec<u8>` or `&[u8]`, and caller must ensure that `buf` outlives
/// the `Bytes` returned.
#[inline(always)]
fn split_to(buf: &mut Bytes, end: usize) -> Bytes {
    let len = buf.len();
    assert!(
        end <= len,
        "range end out of bounds: {:?} <= {:?}",
        end,
        len,
    );

    if end == 0 {
        return Bytes::new();
    }

    let ptr = buf.as_ptr();
    let x = unsafe { slice::from_raw_parts(ptr, end) };
    // `Bytes::drop` does nothing when it's built via `from_static`.
    Bytes::from_static(x)
}

/// Reads a variable-length encoded bytes field from `buf` and assign it to `value`.
/// # Safety
/// Callers must ensure `buf` outlives `value`.
#[inline(always)]
fn merge_bytes(value: &mut Bytes, buf: &mut Bytes) -> Result<(), DecodeError> {
    let len = decode_varint(buf)?;
    if len > buf.remaining() as u64 {
        return Err(DecodeError::new("buffer underflow"));
    }
    *value = copy_to_bytes(buf, len as usize);
    Ok(())
}

#[derive(Default)]
pub struct PromTimeSeries {
    pub table_name: String,
    pub labels: RepeatedField<PromLabel>,
    pub samples: RepeatedField<Sample>,
}

impl Clear for PromTimeSeries {
    fn clear(&mut self) {
        self.table_name.clear();
        self.labels.clear();
        self.samples.clear();
    }
}

impl PromTimeSeries {
    pub fn merge_field(
        &mut self,
        tag: u32,
        wire_type: WireType,
        buf: &mut Bytes,
    ) -> Result<(), DecodeError> {
        const STRUCT_NAME: &str = "PromTimeSeries";
        match tag {
            1u32 => {
                // decode labels
                let label = self.labels.push_default();

                let len = decode_varint(buf).map_err(|mut error| {
                    error.push(STRUCT_NAME, "labels");
                    error
                })?;
                let remaining = buf.remaining();
                if len > remaining as u64 {
                    return Err(DecodeError::new("buffer underflow"));
                }

                let limit = remaining - len as usize;
                while buf.remaining() > limit {
                    let (tag, wire_type) = decode_key(buf)?;
                    label.merge_field(tag, wire_type, buf)?;
                }
                if buf.remaining() != limit {
                    return Err(DecodeError::new("delimited length exceeded"));
                }
                if label.name.deref() == METRIC_NAME_LABEL_BYTES {
                    // safety: we expect all labels are UTF-8 encoded strings.
                    let table_name = unsafe { String::from_utf8_unchecked(label.value.to_vec()) };
                    self.table_name = table_name;
                    self.labels.truncate(self.labels.len() - 1); // remove last label
                }
                Ok(())
            }
            2u32 => {
                let sample = self.samples.push_default();
                merge(WireType::LengthDelimited, sample, buf, Default::default()).map_err(
                    |mut error| {
                        error.push(STRUCT_NAME, "samples");
                        error
                    },
                )?;
                Ok(())
            }
            // skip exemplars
            3u32 => prost::encoding::skip_field(wire_type, tag, buf, Default::default()),
            _ => prost::encoding::skip_field(wire_type, tag, buf, Default::default()),
        }
    }

    fn add_to_table_data(&mut self, table_builders: &mut TablesBuilder) {
        let label_num = self.labels.len();
        let row_num = self.samples.len();
        let table_data = table_builders.get_or_create_table_builder(
            std::mem::take(&mut self.table_name),
            label_num,
            row_num,
        );
        table_data.add_labels_and_samples(self.labels.as_slice(), self.samples.as_slice());
        self.labels.clear();
        self.samples.clear();
    }
}

#[derive(Default)]
pub struct PromWriteRequest {
    table_data: TablesBuilder,
    series: PromTimeSeries,
}

impl Clear for PromWriteRequest {
    fn clear(&mut self) {
        self.table_data.clear();
    }
}

impl PromWriteRequest {
    pub fn as_row_insert_requests(&mut self) -> (RowInsertRequests, usize) {
        self.table_data.as_insert_requests()
    }

    pub fn merge(&mut self, mut buf: Bytes) -> Result<(), DecodeError> {
        const STRUCT_NAME: &str = "PromWriteRequest";
        while buf.has_remaining() {
            let (tag, wire_type) = decode_key(&mut buf)?;
            assert_eq!(WireType::LengthDelimited, wire_type);
            match tag {
                1u32 => {
                    // decode TimeSeries
                    let len = decode_varint(&mut buf).map_err(|mut e| {
                        e.push(STRUCT_NAME, "timeseries");
                        e
                    })?;
                    let remaining = buf.remaining();
                    if len > remaining as u64 {
                        return Err(DecodeError::new("buffer underflow"));
                    }

                    let limit = remaining - len as usize;
                    while buf.remaining() > limit {
                        let (tag, wire_type) = decode_key(&mut buf)?;
                        self.series.merge_field(tag, wire_type, &mut buf)?;
                    }
                    if buf.remaining() != limit {
                        return Err(DecodeError::new("delimited length exceeded"));
                    }
                    self.series.add_to_table_data(&mut self.table_data);
                }
                3u32 => {
                    // we can ignore metadata for now.
                    prost::encoding::skip_field(wire_type, tag, &mut buf, Default::default())?;
                }
                _ => prost::encoding::skip_field(wire_type, tag, &mut buf, Default::default())?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use api::prom_store::remote::WriteRequest;
    use api::v1::RowInsertRequests;
    use bytes::Bytes;
    use prost::Message;

    use crate::prom_store::to_grpc_row_insert_requests;
    use crate::proto::PromWriteRequest;
    use crate::repeated_field::Clear;

    fn check_deserialized(
        prom_write_request: &mut PromWriteRequest,
        data: &Bytes,
        expected_samples: usize,
        expected_rows: &RowInsertRequests,
    ) {
        prom_write_request.clear();
        prom_write_request.merge(data.clone()).unwrap();
        let (prom_rows, samples) = prom_write_request.as_row_insert_requests();

        assert_eq!(expected_samples, samples);
        assert_eq!(expected_rows.inserts.len(), prom_rows.inserts.len());

        let schemas = expected_rows
            .inserts
            .iter()
            .map(|r| {
                (
                    r.table_name.clone(),
                    r.rows
                        .as_ref()
                        .unwrap()
                        .schema
                        .iter()
                        .map(|c| (c.column_name.clone(), c.datatype, c.semantic_type))
                        .collect::<HashSet<_>>(),
                )
            })
            .collect::<HashMap<_, _>>();

        for r in &prom_rows.inserts {
            let expected = schemas.get(&r.table_name).unwrap();
            assert_eq!(
                expected,
                &r.rows
                    .as_ref()
                    .unwrap()
                    .schema
                    .iter()
                    .map(|c| { (c.column_name.clone(), c.datatype, c.semantic_type) })
                    .collect()
            );
        }
    }

    // Ensures `WriteRequest` and `PromWriteRequest` produce the same gRPC request.
    #[test]
    fn test_decode_write_request() {
        let mut d = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("benches");
        d.push("write_request.pb.data");
        let data = Bytes::from(std::fs::read(d).unwrap());

        let (expected_rows, expected_samples) =
            to_grpc_row_insert_requests(&WriteRequest::decode(data.clone()).unwrap()).unwrap();

        let mut prom_write_request = PromWriteRequest::default();
        for _ in 0..3 {
            check_deserialized(
                &mut prom_write_request,
                &data,
                expected_samples,
                &expected_rows,
            );
        }
    }
}