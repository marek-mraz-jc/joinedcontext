// SPDX-License-Identifier: EUPL-1.2
//! File encodings of an answer read page by page (T-4465, T-4477, EP-119, EP-08, EP-41). Each
//! encoding sits behind its own feature and none depends on a surface: a surface hands over
//! `GeoJSON` Features one page at a time with the columns its schema gives, the encoder writes
//! as it goes, and it refuses to grow past the byte ceiling it was given, so a file is whole or
//! not written. One file holds one collection, one entity type: an `id` column, a `geometry`
//! column when the collection has one, then a column per property.

use serde_json::Value;

#[cfg(feature = "zip")]
pub mod bundle;
#[cfg(feature = "csv")]
pub mod csv;
#[cfg(feature = "parquet")]
pub mod parquet;
#[cfg(feature = "parquet")]
mod wkb;

/// The column holding each Feature's id.
pub const ID: &str = "id";
/// The column holding each Feature's geometry.
pub const GEOMETRY: &str = "geometry";

/// What one member of a ZIP container costs beside its bytes: its local header, its entry in
/// the central directory and a data descriptor, with room for a path of up to 60 bytes.
#[cfg(feature = "zip")]
pub(crate) const PART_OVERHEAD: usize = 256;

/// Why no file was written.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// The file grew past the ceiling it was given.
    TooLarge,
    /// The writer refused the schema or a value.
    Write(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge => f.write_str("the file is larger than the answer may be"),
            Self::Write(why) => write!(f, "the file does not serialize: {why}"),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(feature = "zip")]
impl From<::zip::result::ZipError> for Error {
    fn from(e: ::zip::result::ZipError) -> Self {
        Self::Write(e.to_string())
    }
}

#[cfg(feature = "zip")]
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Write(e.to_string())
    }
}

/// A file being written one page at a time.
pub trait Sink: Send {
    /// One page of `GeoJSON` Features.
    ///
    /// # Errors
    /// [`Error::TooLarge`] past the ceiling, [`Error::Write`] when the writer refuses a value.
    fn page(&mut self, features: &[Value]) -> Result<(), Error>;

    /// The whole file; `provenance` (what was asked, by whom it was served, when) goes where
    /// the format keeps such pairs, or nowhere when it keeps none.
    ///
    /// # Errors
    /// [`Error::TooLarge`] when the end of the file does not fit under the ceiling.
    fn finish(self: Box<Self>, provenance: &[(String, String)]) -> Result<Vec<u8>, Error>;
}

/// What one column holds, read from the JSON Schema of its property.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A string.
    Text,
    /// A whole number.
    Integer,
    /// Any other number.
    Double,
    /// `true` or `false`.
    Boolean,
    /// Anything structured: an object or an array, written as JSON text.
    Json,
}

impl Kind {
    /// The kind of a property whose value `schema` describes; a nullable type (`["number",
    /// "null"]`) is its other member, and anything that is no scalar is [`Kind::Json`].
    #[must_use]
    pub fn of_schema(schema: &Value) -> Self {
        let named = match &schema["type"] {
            Value::String(name) => Some(name.as_str()),
            Value::Array(names) => names
                .iter()
                .filter_map(Value::as_str)
                .find(|n| *n != "null"),
            _ => None,
        };
        match named {
            Some("string") => Self::Text,
            Some("integer") => Self::Integer,
            Some("number") => Self::Double,
            Some("boolean") => Self::Boolean,
            _ => Self::Json,
        }
    }
}

/// One column of the file: a Feature property's name and what it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    /// The property's name in the Features' `properties`.
    pub name: String,
    /// What it holds.
    pub kind: Kind,
}

/// The columns a file holds: a column named like the id, the geometry or an earlier column is
/// left out, because the Feature's own member holds that name.
#[cfg(any(feature = "parquet", feature = "csv"))]
pub(crate) fn distinct(columns: Vec<Column>) -> Vec<Column> {
    let mut named = std::collections::BTreeSet::from([ID.to_owned(), GEOMETRY.to_owned()]);
    columns
        .into_iter()
        .filter(|c| named.insert(c.name.clone()))
        .collect()
}

/// The header of a table: `id`, `geometry` when there is one, then the columns.
#[cfg(feature = "csv")]
pub(crate) fn header(columns: &[Column], geometry: bool) -> Vec<Value> {
    let mut header = vec![Value::from(ID)];
    if geometry {
        header.push(Value::from(GEOMETRY));
    }
    header.extend(columns.iter().map(|c| Value::from(c.name.as_str())));
    header
}

/// One Feature's cells in the header's order; a property the Feature lacks is a null.
#[cfg(feature = "csv")]
pub(crate) fn cells<'f>(feature: &'f Value, columns: &[Column], geometry: bool) -> Vec<&'f Value> {
    static NULL: Value = Value::Null;
    let member = |name: &str| feature.get(name).unwrap_or(&NULL);
    let mut cells = vec![member(ID)];
    if geometry {
        cells.push(member(GEOMETRY));
    }
    let properties = member("properties");
    cells.extend(
        columns
            .iter()
            .map(|c| properties.get(c.name.as_str()).unwrap_or(&NULL)),
    );
    cells
}

/// A cell as text: a string as it stands, a null empty, anything else as its JSON.
#[cfg(any(feature = "parquet", feature = "csv"))]
pub(crate) fn text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// A text cell a spreadsheet would read as a formula (`=`, `+`, `-`, `@`, a tab or a carriage
/// return first) gets a leading `'`, so it stays text (OWASP, CSV injection). A number is no
/// text cell and stays as it is.
#[cfg(feature = "csv")]
pub(crate) fn guarded(value: &Value) -> String {
    match value {
        Value::String(s) if s.starts_with(['=', '+', '-', '@', '\t', '\r']) => format!("'{s}"),
        other => text(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_column_is_typed_from_its_schema() {
        for (schema, kind) in [
            (json!({"type": "string"}), Kind::Text),
            (json!({"type": "string", "format": "date-time"}), Kind::Text),
            (json!({"type": "integer"}), Kind::Integer),
            (json!({"type": "number"}), Kind::Double),
            (json!({"type": ["null", "number"]}), Kind::Double),
            (json!({"type": "boolean"}), Kind::Boolean),
            (
                json!({"type": "array", "items": {"type": "number"}}),
                Kind::Json,
            ),
            (json!({"type": "object"}), Kind::Json),
            (json!({}), Kind::Json),
        ] {
            assert_eq!(Kind::of_schema(&schema), kind, "{schema}");
        }
    }

    #[test]
    fn an_error_says_what_stopped_the_file() {
        assert_eq!(
            Error::TooLarge.to_string(),
            "the file is larger than the answer may be"
        );
        assert_eq!(
            Error::Write("bad".into()).to_string(),
            "the file does not serialize: bad"
        );
    }

    #[cfg(feature = "zip")]
    #[test]
    fn an_archive_error_is_a_write_error() {
        let zip = Error::from(::zip::result::ZipError::FileNotFound);
        assert!(matches!(zip, Error::Write(_)), "{zip:?}");
        let io = Error::from(std::io::Error::other("disk"));
        assert_eq!(io, Error::Write("disk".into()));
    }

    #[cfg(any(feature = "parquet", feature = "csv"))]
    #[test]
    fn a_cell_is_its_text() {
        assert_eq!(text(&Value::Null), "");
        assert_eq!(text(&json!("a,b")), "a,b");
        assert_eq!(text(&json!(1.5)), "1.5");
        assert_eq!(text(&json!([1, "x"])), "[1,\"x\"]");
    }

    #[cfg(feature = "csv")]
    #[test]
    fn a_formula_stays_text_and_a_negative_number_a_number() {
        for (cell, written) in [
            (json!("=1+2"), "'=1+2"),
            (json!("+421 2 1234"), "'+421 2 1234"),
            (json!("-x"), "'-x"),
            (json!("@SUM(A1)"), "'@SUM(A1)"),
            (json!("\tx"), "'\tx"),
            (json!("\rx"), "'\rx"),
            (json!("a=b"), "a=b"),
            (json!(-5), "-5"),
            (Value::Null, ""),
        ] {
            assert_eq!(guarded(&cell), written, "{cell}");
        }
    }

    #[cfg(feature = "csv")]
    #[test]
    fn a_row_follows_its_header() {
        let columns = distinct(vec![
            Column {
                name: "speed".into(),
                kind: Kind::Double,
            },
            Column {
                name: "id".into(),
                kind: Kind::Text,
            },
            Column {
                name: "speed".into(),
                kind: Kind::Text,
            },
        ]);
        assert_eq!(
            header(&columns, true),
            [json!("id"), json!("geometry"), json!("speed")]
        );
        assert_eq!(header(&columns, false), [json!("id"), json!("speed")]);
        let feature = json!({"id": "urn:a", "geometry": null, "properties": {"speed": 3}});
        assert_eq!(
            cells(&feature, &columns, true),
            [&json!("urn:a"), &Value::Null, &json!(3)]
        );
        let bare = json!({"id": "urn:b"});
        assert_eq!(
            cells(&bare, &columns, false),
            [&json!("urn:b"), &Value::Null]
        );
    }
}
