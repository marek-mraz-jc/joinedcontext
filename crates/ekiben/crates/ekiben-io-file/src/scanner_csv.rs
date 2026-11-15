//! `csv` scanner and the `csv` input's reader (`internal/impl/pure/scanner_csv.go`,
//! `internal/impl/io/input_csv.go`): one message per row, read row by row. The field count
//! follows Go's `csv.Reader`: `expected_number_of_fields`, or the first row's count when unset.

use std::collections::BTreeMap;
use std::io::Read;

use ekiben_bloblang::Value;
use ekiben_core::{CoreError, Part};
use serde_json::Value as JsonValue;

#[derive(Clone, Debug)]
pub struct CsvScanner {
    pub parse_header_row: bool,
    pub delimiter: u8,
    pub lazy_quotes: bool,
    pub continue_on_error: bool,
    pub expected_headers: Option<Vec<String>>,
    pub expected_number_of_fields: usize,
}

impl Default for CsvScanner {
    fn default() -> Self {
        Self {
            parse_header_row: true,
            delimiter: b',',
            lazy_quotes: false,
            continue_on_error: false,
            expected_headers: None,
            expected_number_of_fields: 0,
        }
    }
}

impl CsvScanner {
    /// The scanner's fields, or the `csv` input's when `delimiter_field` is `delimiter`.
    pub fn from_config(
        obj: &serde_json::Map<String, JsonValue>,
        delimiter_field: &str,
    ) -> Result<Self, CoreError> {
        let flag = |name: &str, default: bool| match obj.get(name) {
            None => Ok(default),
            Some(v) => v
                .as_bool()
                .ok_or_else(|| CoreError::config(format!("field {name} must be true or false"))),
        };
        let delimiter = match obj.get(delimiter_field).and_then(JsonValue::as_str) {
            None | Some("") => b',',
            Some(s) if s.len() == 1 => s.as_bytes()[0],
            Some(s) => {
                return Err(CoreError::config(format!(
                    "field {delimiter_field} must be one ASCII character, for example \";\" (got {s:?})"
                )))
            }
        };
        let expected_headers = match obj.get("expected_headers") {
            None => None,
            Some(JsonValue::Array(items)) => Some(
                items
                    .iter()
                    .map(|v| v.as_str().map(String::from))
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| {
                        CoreError::config(
                            "field expected_headers must be a list of strings, for example [id, name]",
                        )
                    })?,
            ),
            Some(_) => {
                return Err(CoreError::config(
                    "field expected_headers must be a list of strings, for example [id, name]",
                ))
            }
        };
        let expected_number_of_fields = match obj.get("expected_number_of_fields") {
            None => 0,
            Some(v) => v
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| {
                    CoreError::config(
                        "field expected_number_of_fields must be a whole number, for example 5",
                    )
                })?,
        };
        let scanner = Self {
            parse_header_row: flag("parse_header_row", true)?,
            delimiter,
            lazy_quotes: flag("lazy_quotes", false)?,
            continue_on_error: flag("continue_on_error", false)?,
            expected_headers,
            expected_number_of_fields,
        };
        if scanner.expected_headers.is_some() && !scanner.parse_header_row {
            return Err(CoreError::config(
                "expected_headers is set but parse_header_row is false",
            ));
        }
        Ok(scanner)
    }

    /// Calls `emit` with one message per row of `r`, holding one row at a time.
    pub fn scan(
        &self,
        r: &mut dyn Read,
        emit: &mut dyn FnMut(Part) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        let mut rdr = csv::ReaderBuilder::new()
            .has_headers(false)
            .delimiter(self.delimiter)
            .flexible(true)
            .from_reader(r);
        let mut fields = self.expected_number_of_fields;
        let mut headers: Option<Vec<String>> = None;
        let mut rec = csv::StringRecord::new();
        loop {
            let more = match rdr.read_record(&mut rec) {
                Ok(more) => more,
                Err(e) if self.continue_on_error => {
                    let mut p = Part::default();
                    p.set_error_msg(e.to_string());
                    emit(p)?;
                    continue;
                }
                Err(e) => return Err(CoreError::other(e.to_string())),
            };
            if !more {
                return Ok(());
            }
            let line = rec.position().map_or(0, csv::Position::line);
            if fields == 0 {
                fields = rec.len();
            }
            let count_err = (rec.len() != fields)
                .then(|| format!("record on line {line}: wrong number of fields"));
            if let Some(err) = &count_err {
                if !self.continue_on_error {
                    return Err(CoreError::other(err.clone()));
                }
            }
            if self.parse_header_row && headers.is_none() {
                let hdrs: Vec<String> = rec.iter().map(String::from).collect();
                if self
                    .expected_headers
                    .as_ref()
                    .is_some_and(|want| *want != hdrs)
                {
                    return Err(CoreError::other(
                        "expected_headers don't match file contents",
                    ));
                }
                headers = Some(hdrs);
                continue;
            }
            let mut p = match &headers {
                Some(hdrs) => {
                    let obj: BTreeMap<String, Value> = hdrs
                        .iter()
                        .zip(rec.iter())
                        .map(|(h, v)| (h.clone(), Value::String(v.to_string())))
                        .collect();
                    Part::from_structured(Value::Object(obj.into()))
                }
                None => Part::from_structured(Value::Array(
                    rec.iter()
                        .map(|v| Value::String(v.to_string()))
                        .collect::<Vec<_>>()
                        .into(),
                )),
            };
            if let Some(err) = count_err {
                p.set_error_msg(err);
            }
            emit(p)?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(conf: serde_json::Value, input: &str) -> Result<Vec<String>, CoreError> {
        let s = CsvScanner::from_config(conf.as_object().expect("object"), "custom_delimiter")?;
        let mut out = Vec::new();
        s.scan(&mut input.as_bytes(), &mut |p| {
            let body = String::from_utf8_lossy(&p.as_bytes_ref()).into_owned();
            out.push(match p.error_get() {
                Some(e) => format!("{body} !{e}"),
                None => body,
            });
            Ok(())
        })?;
        Ok(out)
    }

    const RAGGED: &str = "a,b,c\n1,2,3\n4,5\n";

    // Outputs and errors recorded from Bento 1.22 on the same file.
    #[test]
    fn field_counts_follow_go_csv() {
        let err = rows(serde_json::json!({}), RAGGED).expect_err("ragged");
        assert_eq!(err.to_string(), "record on line 3: wrong number of fields");
        let err = rows(serde_json::json!({"expected_number_of_fields": 2}), RAGGED)
            .expect_err("header has 3");
        assert_eq!(err.to_string(), "record on line 1: wrong number of fields");
        let got = rows(serde_json::json!({"continue_on_error": true}), RAGGED).expect("rows");
        assert_eq!(
            got,
            [
                r#"{"a":"1","b":"2","c":"3"}"#,
                r#"{"a":"4","b":"5"} !record on line 3: wrong number of fields"#
            ]
        );
    }

    #[test]
    fn expected_headers_must_match_the_header_row() {
        for want in [
            serde_json::json!(["a", "x", "c"]),
            serde_json::json!(["a", "b"]),
        ] {
            let err = rows(
                serde_json::json!({"expected_headers": want}),
                "a,b,c\n1,2,3\n",
            )
            .expect_err("mismatch");
            assert_eq!(
                err.to_string(),
                "expected_headers don't match file contents"
            );
        }
        let got = rows(
            serde_json::json!({"expected_headers": ["a", "b", "c"]}),
            "a,b,c\n1,2,3\n",
        )
        .expect("match");
        assert_eq!(got, [r#"{"a":"1","b":"2","c":"3"}"#]);
        let err = CsvScanner::from_config(
            serde_json::json!({"parse_header_row": false, "expected_headers": ["a"]})
                .as_object()
                .expect("object"),
            "custom_delimiter",
        )
        .expect_err("lint");
        assert!(err
            .to_string()
            .contains("expected_headers is set but parse_header_row is false"));
    }
}
