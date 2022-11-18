// Copyright 2022 Greptime Team
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use api::v1::alter_expr::Kind;
use api::v1::{AlterExpr, DropColumns};
use snafu::OptionExt;
use table::requests::{AddColumnRequest, AlterKind, AlterTableRequest};

use crate::column::create_column_schema;
use crate::error::{MissingFieldSnafu, Result};

/// Convert an [`AlterExpr`] to an optional [`AlterTableRequest`]
pub fn alter_expr_to_request(expr: AlterExpr) -> Result<Option<AlterTableRequest>> {
    match expr.kind {
        Some(Kind::AddColumns(add_columns)) => {
            let mut add_column_requests = vec![];
            for add_column_expr in add_columns.add_columns {
                let column_def = add_column_expr.column_def.context(MissingFieldSnafu {
                    field: "column_def",
                })?;

                let schema = create_column_schema(&column_def)?;
                add_column_requests.push(AddColumnRequest {
                    column_schema: schema,
                    is_key: add_column_expr.is_key,
                })
            }

            Ok(Some(AlterTableRequest {
                catalog_name: expr.catalog_name,
                schema_name: expr.schema_name,
                table_name: expr.table_name,
                alter_kind: AlterKind::AddColumns {
                    columns: add_column_requests,
                },
            }))
        }
        Some(Kind::DropColumns(DropColumns { drop_columns })) => {
            let alter_kind = AlterKind::DropColumns {
                names: drop_columns.into_iter().map(|c| c.name).collect(),
            };

            let request = AlterTableRequest {
                catalog_name: expr.catalog_name,
                schema_name: expr.schema_name,
                table_name: expr.table_name,
                alter_kind,
            };
            Ok(Some(request))
        }
        None => Ok(None),
    }
}
