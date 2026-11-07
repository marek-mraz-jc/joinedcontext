// SPDX-License-Identifier: EUPL-1.2
//! Apache Parquet, and GeoParquet 1.1 when the Features carry a geometry (T-4465, EP-119).
//!
//! The file is written while the answer is paged: each page becomes one row group, zstd
//! compressed, so the writer holds one page of rows and the bytes written so far. Those bytes
//! never grow past the ceiling the writer was given: past it the file stops and the caller
//! answers an error, never a truncated file. The footer is written last, with the GeoParquet
//! `geo` metadata (the geometry types and the bounding box of every row) when there is a
//! geometry column.

use std::collections::BTreeSet;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parquet::basic::{Compression, LogicalType, Repetition, Type as Physical, ZstdLevel};
use parquet::data_type::{BoolType, ByteArray, ByteArrayType, DoubleType, Int64Type};
use parquet::errors::ParquetError;
use parquet::file::metadata::KeyValue;
use parquet::file::properties::WriterProperties;
use parquet::file::writer::{
    SerializedColumnWriter, SerializedFileWriter, SerializedRowGroupWriter,
};
use parquet::schema::types::Type;
use serde_json::{json, Value};

use crate::{distinct, text, wkb, Column, Error, Kind, Sink, GEOMETRY, ID};

/// The media type of a Parquet file.
pub const MEDIA_TYPE: &str = "application/vnd.apache.parquet";

impl From<ParquetError> for Error {
    fn from(e: ParquetError) -> Self {
        Self::Write(e.to_string())
    }
}

/// A row group asked for a column its schema does not have: the writer and its schema disagree.
const MISMATCH: &str = "a column the schema does not hold";

/// A Parquet file being written one page at a time.
pub struct Writer {
    file: SerializedFileWriter<Bounded>,
    columns: Vec<Column>,
    /// Whether the file has a geometry column, and what it has seen in it.
    geometry: Option<Seen>,
    over: Arc<AtomicBool>,
}

/// The geometry types and the bounding box of the rows written so far.
#[derive(Default)]
struct Seen {
    types: BTreeSet<String>,
    bbox: Option<[f64; 4]>,
}

impl Writer {
    /// A file of an `id` column, a `geometry` column when `geometry`, and `columns`, refusing to
    /// grow past `max_bytes`. A column named like the id or the geometry, or like an earlier
    /// column, is left out: the Feature's own member holds that name.
    ///
    /// # Errors
    /// [`Error::Write`] when the schema does not build.
    pub fn new(columns: Vec<Column>, geometry: bool, max_bytes: usize) -> Result<Self, Error> {
        let columns = distinct(columns);
        let mut fields = vec![field(
            ID,
            Physical::BYTE_ARRAY,
            Some(LogicalType::String),
            false,
        )?];
        if geometry {
            fields.push(field(GEOMETRY, Physical::BYTE_ARRAY, None, true)?);
        }
        for c in &columns {
            let (physical, logical) = match c.kind {
                Kind::Integer => (Physical::INT64, None),
                Kind::Double => (Physical::DOUBLE, None),
                Kind::Boolean => (Physical::BOOLEAN, None),
                Kind::Text => (Physical::BYTE_ARRAY, Some(LogicalType::String)),
                Kind::Json => (Physical::BYTE_ARRAY, Some(LogicalType::Json)),
            };
            fields.push(field(&c.name, physical, logical, true)?);
        }
        let schema = Type::group_type_builder("schema")
            .with_fields(fields)
            .build()?;
        let properties = WriterProperties::builder()
            .set_compression(Compression::ZSTD(ZstdLevel::default()))
            .set_max_row_group_row_count(None)
            .build();
        let over = Arc::new(AtomicBool::new(false));
        let sink = Bounded {
            bytes: Vec::new(),
            max: max_bytes,
            over: Arc::clone(&over),
        };
        // The writer buffers the magic it opens with, so the sink is not written to here.
        let file = SerializedFileWriter::new(sink, Arc::new(schema), Arc::new(properties))?;
        Ok(Self {
            file,
            columns,
            geometry: geometry.then(Seen::default),
            over,
        })
    }

    /// The row group of one page, its columns in the schema's order: the id, the geometry when
    /// there is one, then the columns.
    fn group(&mut self, features: &[Value]) -> Result<(), Error> {
        let mut group = self.file.next_row_group()?;
        let ids: Vec<ByteArray> = features
            .iter()
            .map(|f| text(&f["id"]).into_bytes().into())
            .collect();
        let mut column = next(&mut group)?;
        column
            .typed::<ByteArrayType>()
            .write_batch(&ids, None, None)?;
        column.close()?;
        if let Some(seen) = &mut self.geometry {
            let encoded: Vec<Option<Vec<u8>>> = features
                .iter()
                .map(|f| {
                    let (bytes, name, bbox) = wkb::encode(&f["geometry"])?;
                    seen.types.insert(name);
                    seen.bbox = merge(seen.bbox, bbox);
                    Some(bytes)
                })
                .collect();
            let levels: Vec<i16> = encoded.iter().map(|g| i16::from(g.is_some())).collect();
            let values: Vec<ByteArray> =
                encoded.into_iter().flatten().map(ByteArray::from).collect();
            let mut column = next(&mut group)?;
            column
                .typed::<ByteArrayType>()
                .write_batch(&values, Some(&levels), None)?;
            column.close()?;
        }
        for c in &self.columns {
            let cells: Vec<&Value> = features
                .iter()
                .map(|f| {
                    f.get("properties")
                        .and_then(|p| p.get(c.name.as_str()))
                        .unwrap_or(&Value::Null)
                })
                .collect();
            let mut column = next(&mut group)?;
            write_cells(&mut column, c.kind, &cells)?;
            column.close()?;
        }
        group.close()?;
        Ok(())
    }
}

impl Sink for Writer {
    /// One page as one row group; an empty page writes nothing.
    fn page(&mut self, features: &[Value]) -> Result<(), Error> {
        if features.is_empty() {
            return Ok(());
        }
        let written = self.group(features);
        written.map_err(|e| refused(&self.over, e))
    }

    /// The footer: the `geo` metadata when there is a geometry, and each provenance pair as
    /// a key-value of the file.
    fn finish(mut self: Box<Self>, provenance: &[(String, String)]) -> Result<Vec<u8>, Error> {
        if let Some(seen) = &self.geometry {
            self.file
                .append_key_value_metadata(KeyValue::new("geo".to_owned(), geo(seen).to_string()));
        }
        for (key, value) in provenance {
            self.file
                .append_key_value_metadata(KeyValue::new(key.clone(), value.clone()));
        }
        let over = Arc::clone(&self.over);
        self.file
            .into_inner()
            .map(|sink| sink.bytes)
            .map_err(|e| refused(&over, e.into()))
    }
}

/// The next column of a row group.
fn next<'g>(
    group: &'g mut SerializedRowGroupWriter<'_, Bounded>,
) -> Result<SerializedColumnWriter<'g>, Error> {
    group
        .next_column()?
        .ok_or(Error::Write(MISMATCH.to_owned()))
}

/// The GeoParquet 1.1 file metadata of the geometry column: WKB in OGC:CRS84 (the default
/// `crs`, so it is not written), its geometry types and the box of every row.
fn geo(seen: &Seen) -> Value {
    let mut column = json!({
        "encoding": "WKB",
        "geometry_types": seen.types.iter().collect::<Vec<_>>(),
    });
    if let (Some(bbox), Some(members)) = (seen.bbox, column.as_object_mut()) {
        members.insert("bbox".into(), json!(bbox));
    }
    json!({"version": "1.1.0", "primary_column": GEOMETRY, "columns": {GEOMETRY: column}})
}

fn merge(a: Option<[f64; 4]>, b: Option<[f64; 4]>) -> Option<[f64; 4]> {
    match (a, b) {
        (Some([a0, a1, a2, a3]), Some([b0, b1, b2, b3])) => {
            Some([a0.min(b0), a1.min(b1), a2.max(b2), a3.max(b3)])
        }
        (a, b) => a.or(b),
    }
}

fn field(
    name: &str,
    physical: Physical,
    logical: Option<LogicalType>,
    optional: bool,
) -> Result<Arc<Type>, Error> {
    Type::primitive_type_builder(name, physical)
        .with_repetition(if optional {
            Repetition::OPTIONAL
        } else {
            Repetition::REQUIRED
        })
        .with_logical_type(logical)
        .build()
        .map(Arc::new)
        .map_err(Error::from)
}

/// One column of a page: definition level 1 a value, 0 a null. A value of another type than
/// the column's (a text in a number column) is a null, as an absent one is.
fn write_cells(
    column: &mut SerializedColumnWriter<'_>,
    kind: Kind,
    cells: &[&Value],
) -> Result<(), ParquetError> {
    match kind {
        Kind::Integer => {
            let values: Vec<Option<i64>> = cells.iter().map(|v| v.as_i64()).collect();
            let (levels, values) = split(values);
            column
                .typed::<Int64Type>()
                .write_batch(&values, Some(&levels), None)?;
        }
        Kind::Double => {
            let values: Vec<Option<f64>> = cells.iter().map(|v| v.as_f64()).collect();
            let (levels, values) = split(values);
            column
                .typed::<DoubleType>()
                .write_batch(&values, Some(&levels), None)?;
        }
        Kind::Boolean => {
            let values: Vec<Option<bool>> = cells.iter().map(|v| v.as_bool()).collect();
            let (levels, values) = split(values);
            column
                .typed::<BoolType>()
                .write_batch(&values, Some(&levels), None)?;
        }
        Kind::Text | Kind::Json => {
            let values: Vec<Option<ByteArray>> = cells
                .iter()
                .map(|v| (!v.is_null()).then(|| text(v).into_bytes().into()))
                .collect();
            let (levels, values) = split(values);
            column
                .typed::<ByteArrayType>()
                .write_batch(&values, Some(&levels), None)?;
        }
    }
    Ok(())
}

fn split<T>(values: Vec<Option<T>>) -> (Vec<i16>, Vec<T>) {
    let levels = values.iter().map(|v| i16::from(v.is_some())).collect();
    (levels, values.into_iter().flatten().collect())
}

/// The ceiling's refusal reaches the writer as an I/O error of its sink; the flag the sink
/// raised tells it apart from any other.
fn refused(over: &AtomicBool, e: Error) -> Error {
    if over.load(Ordering::Relaxed) {
        Error::TooLarge
    } else {
        e
    }
}

/// The file's bytes, refusing to grow past `max`.
struct Bounded {
    bytes: Vec<u8>,
    max: usize,
    over: Arc<AtomicBool>,
}

impl Write for Bounded {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.bytes.len().saturating_add(buf.len()) > self.max {
            self.over.store(true, Ordering::Relaxed);
            return Err(io::Error::other(
                "the file is larger than the answer may be",
            ));
        }
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parquet::file::reader::{FileReader, SerializedFileReader};
    use parquet::record::{Field, Row, RowAccessor};

    type Outcome = Result<(), Box<dyn std::error::Error>>;

    fn features(n: u32) -> Vec<Value> {
        (0..n)
            .map(|i| {
                json!({
                    "type": "Feature",
                    "id": format!("urn:ngsi-ld:Car:{i}"),
                    "geometry": {"type": "Point", "coordinates": [f64::from(i), 48.0]},
                    "properties": {"speed": i, "name": format!("car {i}"), "fast": i % 2 == 0,
                                   "tags": ["a", i], "missing": null}
                })
            })
            .collect()
    }

    fn column(name: &str, kind: Kind) -> Column {
        Column {
            name: name.into(),
            kind,
        }
    }

    fn columns() -> Vec<Column> {
        vec![
            column("speed", Kind::Integer),
            column("name", Kind::Text),
            column("fast", Kind::Boolean),
            column("tags", Kind::Json),
            column("missing", Kind::Double),
            column("id", Kind::Text),
        ]
    }

    fn rows(file: &SerializedFileReader<bytes::Bytes>) -> Result<Vec<Row>, ParquetError> {
        file.get_row_iter(None)?.collect()
    }

    fn geo_of(
        file: &SerializedFileReader<bytes::Bytes>,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let kv = file
            .metadata()
            .file_metadata()
            .key_value_metadata()
            .ok_or("no metadata")?;
        let geo = kv.iter().find(|k| k.key == "geo").ok_or("no geo")?;
        let text = geo.value.as_deref().ok_or("an empty geo")?;
        Ok(serde_json::from_str(text)?)
    }

    #[test]
    fn each_page_is_a_row_group_and_every_value_reads_back() -> Outcome {
        let mut writer = Writer::new(columns(), true, 1 << 20)?;
        let all = features(5);
        writer.page(all.get(..3).ok_or("three")?)?;
        writer.page(&[])?;
        writer.page(all.get(3..).ok_or("two")?)?;
        let file = SerializedFileReader::new(bytes::Bytes::from(Box::new(writer).finish(&[])?))?;
        let meta = file.metadata();
        assert_eq!(meta.num_row_groups(), 2, "an empty page writes no group");
        assert_eq!(meta.file_metadata().num_rows(), 5);
        let names: Vec<&str> = meta
            .file_metadata()
            .schema_descr()
            .columns()
            .iter()
            .map(|c| c.name())
            .collect();
        assert_eq!(
            names,
            ["id", "geometry", "speed", "name", "fast", "tags", "missing"],
            "a property named id is the Feature's id"
        );
        let rows = rows(&file)?;
        let first = rows.first().ok_or("a row")?;
        assert_eq!(first.get_string(0)?, "urn:ngsi-ld:Car:0");
        assert_eq!(
            wkb::decode(first.get_bytes(1)?.data()),
            Some(json!([0.0, 48.0]))
        );
        assert_eq!(first.get_long(2)?, 0);
        assert_eq!(first.get_string(3)?, "car 0");
        assert!(first.get_bool(4)?);
        assert_eq!(first.get_string(5)?, "[\"a\",0]");
        let missing = first.get_column_iter().nth(6).ok_or("a seventh column")?;
        assert_eq!(missing.1, &Field::Null);
        let compressed = meta.row_group(0).column(0).compression();
        assert!(matches!(compressed, Compression::ZSTD(_)), "{compressed:?}");
        Ok(())
    }

    #[test]
    fn a_value_of_another_type_than_its_column_is_a_null() -> Outcome {
        let mut writer = Writer::new(vec![column("speed", Kind::Double)], false, 1 << 20)?;
        writer.page(&[json!({"id": 7, "properties": {"speed": "fast"}})])?;
        let file = SerializedFileReader::new(bytes::Bytes::from(Box::new(writer).finish(&[])?))?;
        let rows = rows(&file)?;
        let row = rows.first().ok_or("a row")?;
        assert_eq!(
            row.get_string(0)?,
            "7",
            "an id that is no string is its JSON"
        );
        let speed = row.get_column_iter().nth(1).ok_or("a speed column")?;
        assert_eq!(speed.1, &Field::Null);
        Ok(())
    }

    #[test]
    fn the_geo_metadata_is_geoparquet_1_1() -> Outcome {
        let mut writer = Writer::new(Vec::new(), true, 1 << 20)?;
        let mut page = features(3);
        let line = page.get_mut(2).ok_or("a third Feature")?;
        line["geometry"] =
            json!({"type": "LineString", "coordinates": [[-1.0, 47.0], [3.0, 49.5]]});
        page.get_mut(1).ok_or("a second Feature")?["geometry"] = Value::Null;
        writer.page(&page)?;
        let file = SerializedFileReader::new(bytes::Bytes::from(Box::new(writer).finish(&[])?))?;
        let geo = geo_of(&file)?;
        // GeoParquet 1.1 schema: version, primary_column and columns are required; a column
        // requires encoding and geometry_types; bbox is four numbers in 2D.
        assert_eq!(geo.pointer("/version"), Some(&json!("1.1.0")));
        assert_eq!(geo.pointer("/primary_column"), Some(&json!("geometry")));
        assert_eq!(
            geo.pointer("/columns/geometry/encoding"),
            Some(&json!("WKB"))
        );
        assert_eq!(
            geo.pointer("/columns/geometry/geometry_types"),
            Some(&json!(["LineString", "Point"]))
        );
        assert_eq!(
            geo.pointer("/columns/geometry/bbox"),
            Some(&json!([-1.0, 47.0, 3.0, 49.5]))
        );
        assert_eq!(
            geo.pointer("/columns/geometry/crs"),
            None,
            "absent crs is OGC:CRS84"
        );
        let rows = rows(&file)?;
        let second = rows.get(1).ok_or("a second row")?;
        let geometry = second.get_column_iter().nth(1).ok_or("a geometry column")?;
        assert_eq!(
            geometry.1,
            &Field::Null,
            "a Feature without a geometry is a null"
        );
        Ok(())
    }

    #[test]
    fn rows_without_any_geometry_have_no_box() {
        let seen = Seen {
            types: BTreeSet::new(),
            bbox: None,
        };
        assert_eq!(geo(&seen).pointer("/columns/geometry/bbox"), None);
        assert_eq!(
            merge(None, Some([0.0, 1.0, 2.0, 3.0])),
            Some([0.0, 1.0, 2.0, 3.0])
        );
    }

    #[test]
    fn a_file_without_a_geometry_is_plain_parquet() -> Outcome {
        let mut writer = Writer::new(columns(), false, 1 << 20)?;
        writer.page(&features(2))?;
        let file = SerializedFileReader::new(bytes::Bytes::from(Box::new(writer).finish(&[])?))?;
        let meta = file.metadata().file_metadata();
        assert!(meta.key_value_metadata().is_none(), "no geo metadata");
        let mut writer = Writer::new(Vec::new(), false, 1 << 20)?;
        writer.page(&features(1))?;
        let provenance = [("view".to_owned(), "city".to_owned())];
        let file =
            SerializedFileReader::new(bytes::Bytes::from(Box::new(writer).finish(&provenance)?))?;
        let kv = file
            .metadata()
            .file_metadata()
            .key_value_metadata()
            .ok_or("provenance")?;
        assert_eq!(kv.len(), 1, "the provenance, no geo");
        let view = kv.first().ok_or("a pair")?;
        assert_eq!(
            (view.key.as_str(), view.value.as_deref()),
            ("view", Some("city"))
        );
        let names: Vec<&str> = meta
            .schema_descr()
            .columns()
            .iter()
            .map(|c| c.name())
            .collect();
        assert!(!names.contains(&"geometry"), "{names:?}");
        Ok(())
    }

    #[test]
    fn past_the_ceiling_the_file_is_refused_not_cut() -> Outcome {
        // A page larger than the ceiling is refused as it is written.
        let mut writer = Writer::new(columns(), true, 4096)?;
        let page = writer.page(&features(2000));
        assert!(matches!(page, Err(Error::TooLarge)), "{page:?}");
        // The writer buffers its header, so an empty file past the ceiling is refused at the end.
        let tiny = Box::new(Writer::new(columns(), true, 3)?).finish(&[]);
        assert!(
            matches!(tiny, Err(Error::TooLarge)),
            "even an empty file is past three bytes"
        );
        Ok(())
    }

    #[test]
    fn a_writer_error_below_the_ceiling_stays_what_it_is() {
        let e = refused(
            &AtomicBool::new(false),
            ParquetError::General("bad".into()).into(),
        );
        assert_eq!(
            e.to_string(),
            "the file does not serialize: Parquet error: bad"
        );
        assert_eq!(
            Error::TooLarge.to_string(),
            "the file is larger than the answer may be"
        );
    }
}
