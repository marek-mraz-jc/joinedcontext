// SPDX-License-Identifier: EUPL-1.2
//! RFC 4180 CSV of one collection (T-4477, EP-08, EP-44): a header row, then one row per
//! Feature, CRLF line ends, UTF-8 without a byte order mark (the gateway's CSV has none). The
//! rows go into a buffer that refuses to grow past the ceiling, so a file past it is an error
//! and never a CSV cut short, which would read as a whole one.

use serde_json::Value;

use crate::{cells, distinct, guarded, header, Column, Error, Sink};

/// The media type of the CSV answer, with the parameters RFC 7111 wants.
pub const MEDIA_TYPE: &str = "text/csv; charset=utf-8; header=present";

/// A CSV file being written one page at a time.
pub struct Writer {
    out: Vec<u8>,
    columns: Vec<Column>,
    geometry: bool,
    max: usize,
}

impl Writer {
    /// A file whose header names `id`, `geometry` when `geometry` (its cells `GeoJSON` text),
    /// and `columns`, refusing to grow past `max_bytes`.
    ///
    /// # Errors
    /// [`Error::TooLarge`] when the header alone is past the ceiling.
    pub fn new(columns: Vec<Column>, geometry: bool, max_bytes: usize) -> Result<Self, Error> {
        let mut writer = Self {
            out: Vec::new(),
            columns: distinct(columns),
            geometry,
            max: max_bytes,
        };
        let header = header(&writer.columns, geometry);
        writer.record(&header.iter().collect::<Vec<_>>())?;
        Ok(writer)
    }

    /// One record, checked against the ceiling before it is kept.
    fn record(&mut self, cells: &[&Value]) -> Result<(), Error> {
        let fields: Vec<String> = cells.iter().map(|v| quoted(&guarded(v))).collect();
        let line = format!("{}\r\n", fields.join(","));
        if self.out.len().saturating_add(line.len()) > self.max {
            return Err(Error::TooLarge);
        }
        self.out.extend_from_slice(line.as_bytes());
        Ok(())
    }
}

impl Sink for Writer {
    fn page(&mut self, features: &[Value]) -> Result<(), Error> {
        for feature in features {
            let row = cells(feature, &self.columns, self.geometry);
            self.record(&row)?;
        }
        Ok(())
    }

    /// A CSV keeps no metadata: the provenance goes nowhere.
    fn finish(self: Box<Self>, _provenance: &[(String, String)]) -> Result<Vec<u8>, Error> {
        Ok(self.out)
    }
}

/// A field, quoted when it holds a comma, a quote, a line break or an edge space.
fn quoted(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) || field.starts_with(' ') || field.ends_with(' ') {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Kind;
    use serde_json::json;

    fn written(features: &[Value], geometry: bool) -> Result<String, Error> {
        let columns = vec![
            Column {
                name: "name".into(),
                kind: Kind::Text,
            },
            Column {
                name: "speed".into(),
                kind: Kind::Double,
            },
        ];
        let mut writer = Box::new(Writer::new(columns, geometry, 1 << 20)?);
        writer.page(features)?;
        writer.page(&[])?;
        let bytes = writer.finish(&[("view".into(), "city".into())])?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    #[test]
    fn a_row_per_feature_under_its_header() -> Result<(), Error> {
        let csv = written(
            &[
                json!({"id": "urn:a", "geometry": {"type": "Point", "coordinates": [17.1, 48.1]},
                       "properties": {"name": "Hlavná, 1", "speed": 12.5}}),
                json!({"id": "urn:b", "geometry": null, "properties": {"name": "say \"hi\""}}),
            ],
            true,
        )?;
        assert_eq!(
            csv,
            "id,geometry,name,speed\r\n\
             urn:a,\"{\"\"coordinates\"\":[17.1,48.1],\"\"type\"\":\"\"Point\"\"}\",\"Hlavná, 1\",12.5\r\n\
             urn:b,,\"say \"\"hi\"\"\",\r\n"
        );
        Ok(())
    }

    #[test]
    fn a_formula_cell_is_text_and_edges_are_quoted() -> Result<(), Error> {
        let csv = written(
            &[
                json!({"id": "urn:a", "properties": {"name": "=HYPERLINK(\"x\")", "speed": -3}}),
                json!({"id": "urn:b", "properties": {"name": " padded", "speed": null}}),
                json!({"id": "urn:c", "properties": {"name": "two\nlines"}}),
                json!({"id": "urn:d", "properties": {"name": "trailing "}}),
            ],
            false,
        )?;
        assert_eq!(
            csv,
            "id,name,speed\r\n\
             urn:a,\"'=HYPERLINK(\"\"x\"\")\",-3\r\n\
             urn:b,\" padded\",\r\n\
             urn:c,\"two\nlines\",\r\n\
             urn:d,\"trailing \",\r\n"
        );
        Ok(())
    }

    #[test]
    fn past_the_ceiling_no_row_is_kept() -> Result<(), Error> {
        assert_eq!(
            Writer::new(Vec::new(), false, 3).err(),
            Some(Error::TooLarge)
        );
        let mut writer = Box::new(Writer::new(Vec::new(), false, 8)?);
        let page = writer.page(&[json!({"id": "urn:long-enough"})]);
        assert_eq!(page, Err(Error::TooLarge));
        assert_eq!(
            writer.finish(&[])?,
            b"id\r\n",
            "the row that crossed is not kept"
        );
        Ok(())
    }
}
