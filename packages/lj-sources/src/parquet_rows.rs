//! A parquet record as JSON: its non-null text, number and date fields by column name.

use parquet::record::{Field, Row};
use serde_json::{Map, Value};

use crate::error::{Result, SourceError};

pub(crate) fn invalid(e: parquet::errors::ParquetError) -> SourceError {
    SourceError::Invalid(format!("parquet: {e}"))
}

/// A timestamp reads as RFC 3339 in UTC.
pub(crate) fn json_row(row: &Row) -> Result<Map<String, Value>> {
    let mut fields = Map::new();
    for (name, field) in row.get_column_iter() {
        let value = match field {
            Field::Null => continue,
            Field::Str(s) => Value::from(s.as_str()),
            Field::Bool(b) => Value::from(*b),
            Field::Int(i) => Value::from(*i),
            Field::Long(i) => Value::from(*i),
            Field::TimestampMicros(us) => chrono::DateTime::from_timestamp_micros(*us)
                .map(|t| Value::from(t.to_rfc3339()))
                .ok_or_else(|| {
                    SourceError::Invalid(format!("parquet column {name}: timestamp {us}"))
                })?,
            other => {
                return Err(SourceError::Invalid(format!(
                    "parquet column {name}: unexpected {other:?}"
                )))
            }
        };
        fields.insert(name.clone(), value);
    }
    Ok(fields)
}
