//! The record time and date formats, pinned from chrono 0.4.45 before the move to jiff (T-7239):
//! JSON as chrono's serde wrote and read it, OData rows as `to_rfc3339()`, the JSON Schema.

use dataroom_core::odata_defs::harvest_run_to_row;
use dataroom_core::{HarvestRun, TemporalPeriod};
use serde_json::{json, Value};

fn run(started: &str) -> HarvestRun {
    serde_json::from_value(json!({
        "orgId": "00000000-0000-0000-0000-000000000001",
        "catalogueId": "00000000-0000-0000-0000-000000000002",
        "id": "r1", "sourceId": "s1", "state": "succeeded",
        "startedAt": started, "finishedAt": "2026-10-25T01:30:00+03:00",
        "createdCount": 0, "updatedCount": 0, "unchangedCount": 0, "withdrawnCount": 0,
        "logs": [{"at": "2026-03-29T01:00:00.000001Z", "level": "info", "fingerprint": "f", "message": "m"}]
    }))
    .expect("run")
}

#[test]
fn record_json_and_odata_rows_keep_chrono_bytes() {
    // Bratislava's spring change (03:30+02:00) and Helsinki's autumn one (01:30+03:00).
    let r = run("2026-03-29T03:30:00.120+02:00");
    let v = serde_json::to_value(&r).expect("json");
    assert_eq!(v["startedAt"], "2026-03-29T01:30:00.120Z");
    assert_eq!(v["finishedAt"], "2026-10-24T22:30:00Z");
    assert_eq!(v["logs"][0]["at"], "2026-03-29T01:00:00.000001Z");
    let row = harvest_run_to_row(&r);
    assert_eq!(row["started_at"], "2026-03-29T01:30:00.120+00:00");
    assert_eq!(row["finished_at"], "2026-10-24T22:30:00+00:00");
    let read = |s: &str| {
        let mut v = v.clone();
        v["startedAt"] = json!(s);
        serde_json::from_value::<HarvestRun>(v)
            .ok()
            .map(|r| serde_json::to_value(r).expect("json")["startedAt"].clone())
    };
    for (input, want) in [
        ("2026-03-29 03:30:00Z", Some("2026-03-29T03:30:00Z")),
        ("2026-03-29T03:30:00+0200", Some("2026-03-29T01:30:00Z")),
        (
            "2026-03-29T03:30:00.123456789123Z",
            Some("2026-03-29T03:30:00.123456789Z"),
        ),
        ("2026-03-29T03:30Z", None),
        ("2026-03-29", None),
    ] {
        assert_eq!(read(input), want.map(Value::from), "{input}");
    }
}

#[test]
fn temporal_dates_keep_chrono_bytes() {
    let read = |s: &str| {
        serde_json::from_value::<TemporalPeriod>(json!({ "start": s }))
            .ok()
            .map(|t| serde_json::to_value(t).expect("json")["start"].clone())
    };
    for (input, want) in [
        ("2024-03-01", Some("2024-03-01")),
        ("2024-3-1", Some("2024-03-01")),
        (" 2024-03-01", Some("2024-03-01")),
        ("2024-03-01 ", Some("2024-03-01")),
        ("+2024-03-01", Some("2024-03-01")),
        ("24-03-01", Some("0024-03-01")),
        ("2024-02-29", Some("2024-02-29")),
        ("2024-02-30", None),
        ("2024-03-01T00:00", None),
    ] {
        assert_eq!(read(input), want.map(Value::from), "{input:?}");
    }
    let schema = serde_json::to_value(schemars::schema_for!(TemporalPeriod)).expect("schema");
    assert_eq!(
        schema["properties"]["start"],
        json!({"format": "date", "type": ["string", "null"]})
    );
    let schema = serde_json::to_value(schemars::schema_for!(HarvestRun)).expect("schema");
    assert_eq!(
        schema["properties"]["startedAt"],
        json!({"format": "date-time", "type": "string"})
    );
}
