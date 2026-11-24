//! Any broker page bytes and query parameters: the collector, the column order and the CSV writer
//! never panic, every row is as wide as the header, and the CSV stays under its byte ceiling (T-7794).
#![no_main]

use jc_tabular::{csv, humanize, Collector, Dialect, Limits, Slot, Window};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // First line: the query string (offset, limit, delimiter, bom); the rest: pages split at NUL.
    let (query, body) = match data.iter().position(|b| *b == b'\n') {
        Some(at) => (&data[..at], &data[at + 1..]),
        None => (&data[..0], data),
    };
    let query = String::from_utf8_lossy(query);
    let params: Vec<(String, String)> = query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    let get = |name: &str| {
        params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    let Ok(window) = Window::parse(&params) else {
        return;
    };
    let human = get("humanHeaders") == Some("true");
    let Ok(dialect) = Dialect::of(get("delimiter"), get("bom"), human) else {
        return;
    };
    let limits = Limits {
        max_rows: 1_000,
        max_bytes: 1 << 20,
    };
    let mut collector = Collector::new(limits, window, 1 << 20);
    for page in body.split(|b| *b == 0) {
        if collector.push_page(page).is_err() {
            return;
        }
    }
    let mut table = collector.finish();
    assert!(table.rows.len() <= window.limit);
    let slots: Vec<Slot> = table
        .columns
        .iter()
        .take(4)
        .map(|name| Slot {
            name: name.split('.').next().unwrap_or_default().to_owned(),
            kind: "Property".to_owned(),
            scalar: true,
            unit: name.len() % 2 == 0,
        })
        .collect();
    table.settle(&slots);
    if human {
        humanize(&mut table);
    }
    for row in &table.rows {
        assert_eq!(row.len(), table.columns.len());
    }
    if let Ok(text) = csv(&table, &limits, dialect) {
        assert!(text.len() as u64 <= limits.max_bytes);
    }
});
