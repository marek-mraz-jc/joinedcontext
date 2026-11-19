//! `unarchive` formats `xlsx` and `ods` (an ekiben extension, T-7058 G-10): one message per row
//! of a sheet, keyed by its header row, the way `csv` splits a CSV body. calamine reads xlsx,
//! xlsm, xlsb, xls and ods alike, and evaluates no macro, formula or external link: a formula
//! cell gives the value the file saved for it.
//!
//! - The sheet is the first one, or `xlsx:<sheet name>`.
//! - Leading empty rows are skipped; the first row left is the header. An empty header cell (a
//!   merged header keeps its text in the first cell only) is named by its column letter.
//! - Every value is text, as the CSV export of the same sheet gives it: numbers in their shortest
//!   form (`3`, `2.5`), booleans as `TRUE`/`FALSE`, dates and times as RFC 3339 (the file holds
//!   no zone, so they read as UTC), an error cell as its code (`#DIV/0!`).
//!
//! Caps: the zip members of an xlsx or ods unpack to at most `MAX_ARCHIVE_UNPACKED_BYTES` by their
//! declared sizes (checked before calamine reads them), at most `max_rows` rows, and at most
//! `MAX_CELLS` cells.

use std::collections::BTreeMap;
use std::io::Cursor;

use calamine::{open_workbook_auto_from_rs, Data, ExcelDateTime, ExcelDateTimeType, Reader};
use ekiben_bloblang::Value;
use jiff::civil;
use jiff::Span;

use crate::archive::MAX_ARCHIVE_UNPACKED_BYTES;

/// Cells one sheet may hold.
pub const MAX_CELLS: usize = 2_000_000;

/// Refuses a zip (xlsx, ods) whose members declare more than the unpack cap.
fn check_unpacked(bytes: &[u8]) -> Result<(), String> {
    if !bytes.starts_with(b"PK") {
        return Ok(());
    }
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| format!("failed to read spreadsheet: {e}"))?;
    let mut total: u64 = 0;
    for i in 0..zip.len() {
        let size = zip
            .by_index_raw(i)
            .map_err(|e| format!("failed to read spreadsheet: {e}"))?
            .size();
        total = total.saturating_add(size);
        if total > MAX_ARCHIVE_UNPACKED_BYTES {
            return Err(format!(
                "spreadsheet unpacks to more than {MAX_ARCHIVE_UNPACKED_BYTES} bytes"
            ));
        }
    }
    Ok(())
}

/// `A`, `B`, ..., `Z`, `AA`, ...
fn column_letter(mut i: usize) -> String {
    let mut out = Vec::new();
    loop {
        out.push(b'A' + (i % 26) as u8);
        if i < 26 {
            break;
        }
        i = i / 26 - 1;
    }
    out.reverse();
    String::from_utf8_lossy(&out).into_owned()
}

fn text(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(s) | Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) => f.to_string(),
        Data::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Data::DateTime(dt) => match excel_datetime(dt) {
            Some(t) => t.strftime("%Y-%m-%dT%H:%M:%SZ").to_string(),
            None => dt.as_f64().to_string(),
        },
        Data::Error(e) => e.to_string(),
    }
}

/// calamine's `as_datetime` (its chrono `dates` feature) on jiff: days since 1899-12-30, a 1904
/// workbook's 1462 days later, Excel's phantom 1900-02-29 skipped, rounded to the millisecond.
/// ponytail: jiff ends at 9999-12-31, Excel's own last day; a later serial prints as its number.
fn excel_datetime(dt: &ExcelDateTime) -> Option<civil::DateTime> {
    const EPOCH: civil::Date = civil::date(1899, 12, 30);
    let ty = if dt.is_duration() {
        ExcelDateTimeType::TimeDelta
    } else {
        ExcelDateTimeType::DateTime
    };
    // calamine keeps the 1904 flag private: a cell differs from its 1900 twin only when it is set
    let is_1904 = *dt != ExcelDateTime::new(dt.as_f64(), ty, false);
    let f = dt.as_f64() + if is_1904 { 1462.0 } else { 0.0 };
    let f = if f >= 60.0 { f } else { f + 1.0 };
    let ms = (f * 86_400_000.0).round() as i64;
    // whole days through a Span: jiff refuses a SignedDuration of more days than it has years for,
    // even when the sum is in range (9999-12-31 is 2958465 days on)
    let (days, rem) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    let date = EPOCH.checked_add(Span::new().try_days(days).ok()?).ok()?;
    let time = civil::Time::new(
        (rem / 3_600_000) as i8,
        (rem / 60_000 % 60) as i8,
        (rem / 1000 % 60) as i8,
        (rem % 1000 * 1_000_000) as i32,
    )
    .ok()?;
    Some(date.to_datetime(time))
}

/// The rows of `sheet` (the first one when `None`), each its header names to its values.
pub fn rows(bytes: &[u8], sheet: Option<&str>, max_rows: usize) -> Result<Vec<Value>, String> {
    check_unpacked(bytes)?;
    let mut book = open_workbook_auto_from_rs(Cursor::new(bytes))
        .map_err(|e| format!("failed to read spreadsheet: {e}"))?;
    let name = match sheet {
        Some(s) => s.to_string(),
        None => book
            .sheet_names()
            .first()
            .cloned()
            .ok_or("the spreadsheet holds no sheet")?,
    };
    let range = book
        .worksheet_range(&name)
        .map_err(|e| format!("failed to read sheet '{name}': {e}"))?;
    let (height, width) = range.get_size();
    if height.saturating_mul(width) > MAX_CELLS {
        return Err(format!(
            "sheet '{name}' holds {height} x {width} cells, more than {MAX_CELLS}"
        ));
    }
    let mut lines = range
        .rows()
        .skip_while(|r| r.iter().all(|c| matches!(c, Data::Empty)));
    let Some(header) = lines.next() else {
        return Ok(Vec::new());
    };
    let names: Vec<String> = header
        .iter()
        .enumerate()
        .map(|(i, c)| match text(c) {
            t if t.trim().is_empty() => column_letter(i),
            t => t,
        })
        .collect();
    let mut out = Vec::new();
    for row in lines {
        if row.iter().all(|c| matches!(c, Data::Empty)) {
            continue;
        }
        if out.len() >= max_rows {
            return Err(format!("sheet '{name}' has more than {max_rows} rows"));
        }
        let obj: BTreeMap<String, Value> = names
            .iter()
            .zip(row.iter())
            .map(|(k, c)| (k.clone(), Value::String(text(c))))
            .collect();
        out.push(Value::Object(obj.into()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excel_times_print_as_the_chrono_build_did() {
        use calamine::{ExcelDateTime, ExcelDateTimeType};
        let cell = |v: f64, ty: ExcelDateTimeType, is_1904: bool| {
            text(&Data::DateTime(ExcelDateTime::new(v, ty, is_1904)))
        };
        let mut got = Vec::new();
        for v in [
            45943.541,
            45_943.999_999_99,
            0.0,
            1.0,
            59.0,
            60.0,
            61.0,
            -1.0,
            0.25,
            2_958_465.9,
            3.0e6,
            1.0e300,
            f64::NAN,
        ] {
            got.push(format!(
                "{v} {}",
                cell(v, ExcelDateTimeType::DateTime, false)
            ));
            got.push(format!(
                "{v} 1904 {}",
                cell(v, ExcelDateTimeType::DateTime, true)
            ));
            got.push(format!(
                "{v} delta {}",
                cell(v, ExcelDateTimeType::TimeDelta, false)
            ));
        }
        let got = got.join("\n");
        assert_eq!(got, EXCEL_GOLDEN.trim(), "\n{got}");
    }

    // Decided (worker-t7243, T-7245): a serial past 9999-12-31 prints as its number; chrono
    // printed a signed five-digit year (`+10113-09-19T00:00:00Z`) for the four such cells below.
    const EXCEL_GOLDEN: &str = r#"
45943.541 2025-10-13T12:59:02Z
45943.541 1904 2029-10-14T12:59:02Z
45943.541 delta 2025-10-13T12:59:02Z
45943.99999999 2025-10-13T23:59:59Z
45943.99999999 1904 2029-10-14T23:59:59Z
45943.99999999 delta 2025-10-13T23:59:59Z
0 1899-12-31T00:00:00Z
0 1904 1904-01-01T00:00:00Z
0 delta 1899-12-31T00:00:00Z
1 1900-01-01T00:00:00Z
1 1904 1904-01-02T00:00:00Z
1 delta 1900-01-01T00:00:00Z
59 1900-02-28T00:00:00Z
59 1904 1904-02-29T00:00:00Z
59 delta 1900-02-28T00:00:00Z
60 1900-02-28T00:00:00Z
60 1904 1904-03-01T00:00:00Z
60 delta 1900-02-28T00:00:00Z
61 1900-03-01T00:00:00Z
61 1904 1904-03-02T00:00:00Z
61 delta 1900-03-01T00:00:00Z
-1 1899-12-30T00:00:00Z
-1 1904 1903-12-31T00:00:00Z
-1 delta 1899-12-30T00:00:00Z
0.25 1899-12-31T06:00:00Z
0.25 1904 1904-01-01T06:00:00Z
0.25 delta 1899-12-31T06:00:00Z
2958465.9 9999-12-31T21:36:00Z
2958465.9 1904 2958465.9
2958465.9 delta 9999-12-31T21:36:00Z
3000000 3000000
3000000 1904 3000000
3000000 delta 3000000
1000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000 1000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000
1000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000 1904 1000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000
1000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000 delta 1000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000
NaN 1899-12-30T00:00:00Z
NaN 1904 1899-12-30T00:00:00Z
NaN delta 1899-12-30T00:00:00Z
"#;

    #[test]
    fn column_letters() {
        assert_eq!(column_letter(0), "A");
        assert_eq!(column_letter(25), "Z");
        assert_eq!(column_letter(26), "AA");
        assert_eq!(column_letter(701), "ZZ");
        assert_eq!(column_letter(702), "AAA");
    }

    #[test]
    fn values_read_as_the_csv_export_writes_them() {
        assert_eq!(text(&Data::Float(3.0)), "3");
        assert_eq!(text(&Data::Float(2.5)), "2.5");
        assert_eq!(text(&Data::Bool(true)), "TRUE");
        assert_eq!(text(&Data::Empty), "");
    }

    /// A minimal xlsx: a merged header (B1:C1), a number, a date (style numFmt 22), an empty
    /// row, a boolean, and a second sheet.
    fn workbook() -> Vec<u8> {
        use std::io::Write;
        let files = [
            (
                "[Content_Types].xml",
                r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/></Types>"#,
            ),
            (
                "_rels/.rels",
                r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
            ),
            (
                "xl/workbook.xml",
                r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Counts" sheetId="1" r:id="rId1"/><sheet name="Other" sheetId="2" r:id="rId2"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#,
            ),
            (
                "xl/styles.xml",
                r#"<?xml version="1.0" encoding="UTF-8"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><cellXfs count="2"><xf numFmtId="0"/><xf numFmtId="22" applyNumberFormat="1"/></cellXfs></styleSheet>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>id</t></is></c><c r="B1" t="inlineStr"><is><t>measured</t></is></c><c r="D1" t="inlineStr"><is><t>when</t></is></c></row><row r="2"><c r="A2"><v>1</v></c><c r="B2"><v>2.5</v></c><c r="C2" t="inlineStr"><is><t>x</t></is></c><c r="D2" s="1"><v>45992.5</v></c></row><row r="4"><c r="A4" t="b"><v>1</v></c></row></sheetData><mergeCells count="1"><mergeCell ref="B1:C1"/></mergeCells></worksheet>"#,
            ),
            (
                "xl/worksheets/sheet2.xml",
                r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="2"><c r="A2" t="inlineStr"><is><t>k</t></is></c></row><row r="3"><c r="A3" t="inlineStr"><is><t>v</t></is></c></row></sheetData></worksheet>"#,
            ),
        ];
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, body) in files {
            zip.start_file(name, opts).expect("member");
            zip.write_all(body.as_bytes()).expect("write");
        }
        zip.finish().expect("zip").into_inner()
    }

    fn json(rows: Vec<Value>) -> Vec<serde_json::Value> {
        rows.into_iter().map(|r| r.to_json_value()).collect()
    }

    #[test]
    fn a_sheet_splits_into_rows_keyed_by_its_header() {
        let book = workbook();
        assert_eq!(
            json(rows(&book, None, 10).expect("reads")),
            vec![
                serde_json::json!({"id": "1", "measured": "2.5", "C": "x", "when": "2025-12-01T12:00:00Z"}),
                serde_json::json!({"id": "TRUE", "measured": "", "C": "", "when": ""}),
            ]
        );
        assert_eq!(
            json(rows(&book, Some("Other"), 10).expect("reads")),
            vec![serde_json::json!({"k": "v"})]
        );
        assert!(rows(&book, Some("Missing"), 10).is_err());
        let capped = rows(&book, None, 1).expect_err("over the row cap");
        assert!(capped.contains("more than 1 rows"), "{capped}");
    }

    #[test]
    fn unarchive_names_the_sheet_after_the_colon() {
        use crate::archive::UnarchiveProcessor;
        use ekiben_core::{ComponentConfig, Part};
        let build = |format: &str| {
            UnarchiveProcessor::from_config(&ComponentConfig {
                label: String::new(),
                component_type: "unarchive".into(),
                config: serde_json::json!({ "format": format }),
                processors: Vec::new(),
            })
        };
        assert!(build("xlsx:").is_err());
        let mut part = Part::new(workbook());
        part.meta_set_str("source", "upload");
        let rows = build("ods:Other")
            .expect("builds")
            .unarchive_part(&mut part)
            .expect("splits");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].meta_get_str("source"), "upload");
        assert_eq!(
            rows[0].as_structured_ref().expect("a row").to_json_value(),
            serde_json::json!({"k": "v"})
        );
    }

    #[test]
    fn something_that_is_no_spreadsheet_is_refused() {
        assert!(rows(b"not a workbook", None, 10).is_err());
        assert!(rows(b"PK\x03\x04broken", None, 10).is_err());
    }
}
