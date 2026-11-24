//! Edge cases of the table, its column order, its limits and its dialect (T-7794).

mod support;

use jc_core::kinds::FileLimits;
use jc_tabular::{csv, humanize, Dialect, Limits, Slot, Table};
use serde_json::{json, Value};
use support::table;

fn slot(name: &str, kind: &str, scalar: bool, unit: bool) -> Slot {
    Slot {
        name: name.to_owned(),
        kind: kind.to_owned(),
        scalar,
        unit,
    }
}

#[test]
fn limits_without_a_declaration_are_the_gateway_default() {
    assert_eq!(Limits::of(None), Limits::DEFAULT);
    assert_eq!(Limits::default(), Limits::DEFAULT);
}

#[test]
fn limits_take_each_declared_field_and_fall_back_per_field() {
    let rows_only = FileLimits {
        max_file_rows: Some(10),
        max_file_bytes: None,
    };
    assert_eq!(
        Limits::of(Some(&rows_only)),
        Limits {
            max_rows: 10,
            max_bytes: Limits::DEFAULT.max_bytes
        }
    );
    let bytes_only = FileLimits {
        max_file_rows: None,
        max_file_bytes: Some(512),
    };
    assert_eq!(
        Limits::of(Some(&bytes_only)),
        Limits {
            max_rows: Limits::DEFAULT.max_rows,
            max_bytes: 512
        }
    );
    assert_eq!(Limits::of(Some(&FileLimits::default())), Limits::DEFAULT);
}

#[test]
fn a_single_entity_object_is_one_row_and_a_scalar_answer_is_none() {
    let mut table = Table::default();
    table.append(&json!({"id": "urn:ngsi-ld:T:1", "type": "T"}));
    assert_eq!(table.rows.len(), 1);
    assert_eq!(table.ids(), vec!["urn:ngsi-ld:T:1".to_owned()]);
    table.append(&json!("not an entity"));
    table.append(&Value::Null);
    assert_eq!(table.rows.len(), 1);
}

#[test]
fn a_repeated_id_is_kept_once() {
    let mut table = Table::default();
    table.append(
        &json!([{"id": "urn:ngsi-ld:T:1", "type": "T", "a": {"type": "Property", "value": 1}}]),
    );
    table.append(
        &json!([{"id": "urn:ngsi-ld:T:1", "type": "T", "a": {"type": "Property", "value": 2}}]),
    );
    assert_eq!(table.rows.len(), 1);
    let at = table.columns.iter().position(|c| c == "a.value").unwrap();
    assert_eq!(table.rows[0][at], json!(1));
}

#[test]
fn a_vocab_slot_fixes_its_vocab_column_even_with_no_rows() {
    let mut table = Table::default();
    table.settle(&[slot("category", "VocabProperty", true, false)]);
    assert_eq!(table.columns, vec!["id", "type", "category.vocab"]);
}

#[test]
fn a_slot_of_an_object_value_fixes_no_column() {
    let mut table = Table::default();
    table.settle(&[
        slot("address", "Property", false, false),
        slot("blob", "JsonProperty", true, false),
    ]);
    assert_eq!(table.columns, vec!["id", "type"]);
}

#[test]
fn a_typed_literal_is_its_value() {
    let entity = json!({
        "id": "urn:ngsi-ld:T:1",
        "type": "T",
        "seen": {"type": "Property", "value": {"@type": "DateTime", "@value": "2026-01-01T00:00:00Z"}}
    });
    let table = table(&entity, &Limits::DEFAULT).unwrap();
    let at = table
        .columns
        .iter()
        .position(|c| c == "seen.value")
        .unwrap();
    assert_eq!(table.rows[0][at], json!("2026-01-01T00:00:00Z"));
}

#[test]
fn unknown_columns_sort_by_number_and_leading_zeros_do_not_count() {
    let mut table = Table::default();
    table.columns = ["z[10]", "z[2]", "z", "z[02]x", "z[2]a", "y"]
        .map(String::from)
        .to_vec();
    table.settle(&[]);
    assert_eq!(
        table.columns,
        vec!["id", "type", "y", "z", "z[2]", "z[2]a", "z[02]x", "z[10]"]
    );
}

#[test]
fn equal_names_after_their_digits_keep_their_given_order() {
    let mut table = Table::default();
    table.columns = vec!["a[01]".into(), "a[1]".into()];
    table.rows = vec![vec![json!("first"), json!("second")]];
    table.settle(&[]);
    assert_eq!(table.columns, vec!["id", "type", "a[01]", "a[1]"]);
    assert_eq!(
        table.rows[0],
        vec![Value::Null, Value::Null, json!("first"), json!("second")]
    );
}

#[test]
fn bom_false_switches_the_mark_off_even_for_human_headers() {
    assert!(Dialect::of(None, Some("true"), false).unwrap().bom);
    let dialect = Dialect::of(None, Some("false"), true).unwrap();
    assert!(!dialect.bom);
    let mut table = Table::default();
    table.append(&json!({"id": "urn:ngsi-ld:T:1", "type": "T"}));
    humanize(&mut table);
    let text = csv(&table, &Limits::DEFAULT, dialect).unwrap();
    assert!(!text.starts_with('\u{feff}'), "{text:?}");
}

#[test]
fn an_unknown_bom_or_delimiter_names_the_parameter_and_an_example() {
    let bom = Dialect::of(None, Some("yes"), false)
        .unwrap_err()
        .to_string();
    assert_eq!(bom, "bom must be true or false, e.g. bom=true");
    let delimiter = Dialect::of(Some("pipe"), None, false)
        .unwrap_err()
        .to_string();
    assert_eq!(
        delimiter,
        "delimiter must be comma, semicolon or tab, e.g. delimiter=semicolon"
    );
}

#[test]
fn a_digit_sorts_before_a_letter_at_the_same_place() {
    let mut table = Table::default();
    table.columns = ["zb", "z1"].map(String::from).to_vec();
    table.settle(&[]);
    assert_eq!(table.columns, vec!["id", "type", "z1", "zb"]);
}

#[test]
fn a_csv_exactly_at_the_byte_ceiling_is_served_and_one_byte_over_is_refused() {
    let mut table = Table::default();
    table.append(&json!({"id": "urn:ngsi-ld:T:1", "type": "T"}));
    let whole = csv(&table, &Limits::DEFAULT, Dialect::default()).unwrap();
    let exact = Limits {
        max_rows: 10,
        max_bytes: whole.len() as u64,
    };
    assert_eq!(csv(&table, &exact, Dialect::default()).unwrap(), whole);
    let under = Limits {
        max_bytes: whole.len() as u64 - 1,
        ..exact
    };
    assert!(csv(&table, &under, Dialect::default()).is_err());
}

#[test]
fn a_cell_with_a_space_at_either_end_is_quoted_so_a_reader_keeps_it() {
    let mut table = Table::default();
    table.append(&json!([
        {"id": "urn:ngsi-ld:T:1", "type": "T", "a": {"type": "Property", "value": " lead"}},
        {"id": "urn:ngsi-ld:T:2", "type": "T", "a": {"type": "Property", "value": "trail "}}
    ]));
    let text = csv(&table, &Limits::DEFAULT, Dialect::default()).unwrap();
    assert!(text.contains(",\" lead\""), "{text}");
    assert!(text.contains(",\"trail \""), "{text}");
}
