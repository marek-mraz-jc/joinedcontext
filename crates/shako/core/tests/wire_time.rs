#![forbid(unsafe_code)]
//! Record timestamps on the wire, pinned from chrono 0.4.45 before the move to jiff (T-7237):
//! RFC 3339 in UTC with `Z` and 0, 3, 6 or 9 fraction digits in JSON, `+00:00` in the OData and
//! Gitea answers (chrono's `to_rfc3339()`), the epoch as the default, and `date-time` in the
//! JSON Schema.

use jc_time::{TimestampTz, parse_rfc3339};
use serde_json::json;
use shako_core::model::QuotaUsageRecord;

fn at(s: &str) -> TimestampTz {
    TimestampTz(parse_rfc3339(s).expect(s))
}

#[test]
fn record_timestamps_serialize_as_chrono_did() {
    let rec = QuotaUsageRecord::default();
    assert_eq!(
        serde_json::to_value(&rec).expect("serialize")["updatedAt"],
        json!("1970-01-01T00:00:00Z")
    );
    for (sent, json_wire, odata_wire) in [
        (
            "2025-11-18T16:20:00Z",
            "2025-11-18T16:20:00Z",
            "2025-11-18T16:20:00+00:00",
        ),
        (
            "2025-11-18T18:20:00.5+02:00",
            "2025-11-18T16:20:00.500Z",
            "2025-11-18T16:20:00.500+00:00",
        ),
        (
            "2025-11-18T16:20:00.123456Z",
            "2025-11-18T16:20:00.123456Z",
            "2025-11-18T16:20:00.123456+00:00",
        ),
    ] {
        let rec = QuotaUsageRecord {
            updated_at: at(sent),
            ..QuotaUsageRecord::default()
        };
        let v = serde_json::to_value(&rec).expect("serialize");
        assert_eq!(v["updatedAt"], json!(json_wire), "{sent}");
        assert_eq!(rec.updated_at.to_rfc3339(), odata_wire, "{sent}");
        let back: QuotaUsageRecord = serde_json::from_value(v).expect("deserialize");
        assert_eq!(back, rec);
    }
}

#[test]
fn record_schema_keeps_date_time() {
    let schema = serde_json::to_value(schemars::schema_for!(QuotaUsageRecord)).expect("schema");
    assert_eq!(
        schema["properties"]["updatedAt"],
        json!({ "type": "string", "format": "date-time" })
    );
}
