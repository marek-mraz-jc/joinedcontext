//! Time wire formats pinned from the chrono build before the move to jiff (T-7246, T-7233 plan):
//! every expected string below is what the chrono code printed. A change here is a format change
//! on a pipeline's output.

use ekiben_bloblang::{Environment, Value};

fn run(mapping: &str) -> String {
    let exec = Environment::new()
        .compile_mapping(mapping)
        .unwrap_or_else(|e| panic!("{mapping}: {e}"));
    match exec.execute_bytes(b"{}") {
        Ok(Some(Value::String(s))) => s,
        Ok(Some(v)) => serde_json::to_string(&v.to_json_value()).expect("json"),
        Ok(None) => "<none>".to_string(),
        Err(e) => format!("error: {e}"),
    }
}

const CASES: &[(&str, &str)] = &[
    // A timestamp value in JSON: Go's RFC3339Nano, fraction trimmed, the parsed offset kept.
    (r#"root = "2026-03-29T01:30:00.120Z".ts_parse("2006-01-02T15:04:05Z07:00")"#, "\"2026-03-29T01:30:00.12Z\""),
    (r#"root = "2026-03-29T03:30:00.000000001+02:00".ts_parse("2006-01-02T15:04:05Z07:00")"#, "\"2026-03-29T03:30:00.000000001+02:00\""),
    (r#"root = "2026-03-29T03:30:00-05:30".ts_parse("2006-01-02T15:04:05Z07:00")"#, "\"2026-03-29T03:30:00-05:30\""),
    // RFC 3339 read: chrono's grammar (lowercase t and z, a space, a leap second, 10 digits).
    // Decided (worker-t7243, T-7246): two changes from the chrono build. jiff has no leap second,
    // so second 60 reads as the next second (chrono printed 1483228799 here, its millisecond and
    // nanosecond values already were the next second's); a refused text ends in "not RFC 3339"
    // where chrono named its parser's state ("premature end of input").
    (r#"root = "2026-03-29t01:30:00z".ts_unix()"#, "1774747800"),
    (r#"root = "2026-03-29 01:30:00+01:00".ts_unix()"#, "1774744200"),
    (r#"root = "2016-12-31T23:59:60Z".ts_unix()"#, "1483228800"),
    (r#"root = "2026-03-29T01:30:00.1234567891Z".ts_unix_nano()"#, "1774747800123456789"),
    (r#"root = "2026-03-29T01:30Z".ts_unix()"#, "error: failed assignment (line 1): string literal: failed to parse RFC3339 timestamp '2026-03-29T01:30Z': not RFC 3339"),
    (r#"root = "2026-03-29T01:30:00".ts_unix()"#, "error: failed assignment (line 1): string literal: failed to parse RFC3339 timestamp '2026-03-29T01:30:00': not RFC 3339"),
    (r#"root = "20260329T013000Z".ts_unix()"#, "error: failed assignment (line 1): string literal: failed to parse RFC3339 timestamp '20260329T013000Z': not RFC 3339"),
    // Numbers as unix time.
    (r#"root = 1771628400.ts_format("2006-01-02T15:04:05.000Z07:00")"#, "2026-02-20T23:00:00.000Z"),
    (r#"root = 1771628400.5.ts_format()"#, "2026-02-20T23:00:00.5Z"),
    (r#"root = -1.25.ts_format()"#, "1969-12-31T23:59:58.75Z"),
    // Before 1970 a unix count rounds down, as chrono's timestamp*() did (jiff truncates).
    (r#"root = -1.2505.ts_unix()"#, "-2"),
    (r#"root = -1.2505.ts_unix_milli()"#, "-1251"),
    (r#"root = -1.2505.ts_unix_micro()"#, "-1250500"),
    // Go layouts, zones on both sides of the 2026 DST changes.
    (r#"root = "2026-03-29T00:59:59Z".ts_format("Mon Jan _2 15:04:05 MST 2006 -0700 Z0700", "Europe/Bratislava")"#, "Sun Mar 29 01:59:59 +01:00 2026 +0100 +0100"),
    (r#"root = "2026-03-29T01:00:00Z".ts_format("Monday January 2 3:04PM -07:00 Z07:00", "Europe/Bratislava")"#, "Sunday March 29 3:00AM +02:00 +02:00"),
    (r#"root = "2026-10-25T00:59:59Z".ts_format("2006-01-02 15:04:05.000000 -07 Z07 pm", "Europe/Helsinki")"#, "2026-10-25 03:59:59.000000 +03 +03 am"),
    (r#"root = "2026-10-25T01:00:00Z".ts_format("06/1/2 03:04:05.999 MST", "Europe/Helsinki")"#, "26/10/25 03:00:00 +02:00"),
    (r#"root = "2026-10-25T01:00:00.5Z".ts_tz("Europe/Helsinki")"#, "\"2026-10-25T03:00:00.5+02:00\""),
    (r#"root = "2026-10-25T01:00:00Z".ts_format("2006", "Nowhere/Town")"#, "error: failed assignment (line 1): string literal: failed to parse timezone location name: unknown time zone Nowhere/Town"),
    (r#"root = "Sun Mar 29 02:30:00 +0100 CET 2026".ts_parse("Mon Jan 2 15:04:05 -0700 MST 2006")"#, "\"2026-03-29T02:30:00+01:00\""),
    (r#"root = "29.3.2026 3:04PM".ts_parse("2.1.2006 3:04PM")"#, "\"2026-03-29T12:04:00Z\""),
    (r#"root = "2026-02-30".ts_parse("2006-01-02")"#, "error: failed assignment (line 1): string literal: invalid date 2026-02-30"),
    // strftime both ways.
    (r#"root = "2026-03-29T01:00:00.123456789Z".ts_strftime("%a %A %b %B %d %e %H %I %M %S %f %p %P %z %Z %s %F %T %y %%", "Europe/Bratislava")"#, "Sun Sunday Mar March 29 29 03 03 00 00 123456 AM am +0200 +02:00 1774746000 2026-03-29 03:00:00 26 %"),
    (r#"root = "31.02.2026 11:30 pm +0530".ts_strptime("%d.%m.%Y %I:%M %p %z")"#, "\"2026-03-03T23:30:00+05:30\""),
    // Unix outputs, differences, rounding and periods.
    (r#"root = "2026-03-29T01:30:00.123456789+02:00".ts_unix_milli()"#, "1774740600123"),
    (r#"root = "2026-03-29T01:30:00.123456789+02:00".ts_unix_micro()"#, "1774740600123456"),
    (r#"root = "2026-03-29T01:30:00.123456789+02:00".ts_sub("2026-03-28T01:30:00Z".ts_parse("2006-01-02T15:04:05Z07:00"))"#, "79200123456789"),
    (r#"root = "2026-03-29T01:30:31.5+02:00".ts_round(60000000000)"#, "\"2026-03-29T01:31:00+02:00\""),
    (r#"root = "2026-01-31T12:00:00+01:00".ts_add_iso8601("P1M1DT1H")"#, "\"2026-03-01T13:00:00+01:00\""),
    (r#"root = "2024-02-29T12:00:00Z".ts_sub_iso8601("P1Y")"#, "\"2023-02-28T12:00:00Z\""),
    // Go's %v of a time.Time, and the string a timestamp compares as.
    (r#"root = "%v|%v".format("2026-03-29T01:30:00.12Z".ts_parse("2006-01-02T15:04:05Z07:00"), "2026-03-29T03:30:00+02:00".ts_parse("2006-01-02T15:04:05Z07:00"))"#, "2026-03-29 01:30:00.120 +0000 UTC|2026-03-29 03:30:00 +0200 +0200"),
    (r#"root = "2026-03-29T01:30:00.12Z".ts_parse("2006-01-02T15:04:05Z07:00") == "2026-03-29T01:30:00.120Z""#, "true"),
    (r#"root = "2026-03-29T01:30:00.12Z".ts_parse("2006-01-02T15:04:05Z07:00") == "2026-03-29T01:30:00.12Z""#, "false"),
    // NGSI-LD observedAt from a timestamp value: UTC, AutoSi digits.
    (r#"root = 1.ngsild_property(observed_at: "2026-01-01T01:00:00.5+01:00".ts_parse("2006-01-02T15:04:05Z07:00"))"#, "{\"observedAt\":\"2026-01-01T00:00:00.500Z\",\"type\":\"Property\",\"value\":1}"),
    (r#"root = 1.ngsild_property(observed_at: "2026-01-01T01:00:00+01")"#, "error: failed assignment (line 1): number literal: parameter observed_at: \"2026-01-01T01:00:00+01\" is not an RFC 3339 date-time"),
];

#[test]
fn time_wire_formats_match_the_chrono_build() {
    let mut diffs = Vec::new();
    for (mapping, want) in CASES {
        let got = run(mapping);
        if got != *want {
            diffs.push(format!("    (r#\"{mapping}\"#, {got:?}),"));
        }
    }
    assert!(diffs.is_empty(), "changed:\n{}", diffs.join("\n"));
}
