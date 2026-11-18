#![forbid(unsafe_code)]
//! Record timestamps on the wire, pinned from chrono 0.4.45 before the move to jiff (T-7234):
//! RFC 3339 in UTC with `Z` and 0, 3, 6 or 9 fraction digits, read back from any offset, and
//! the JSON Schema `string` / `date-time`.

use daiya_model::{FolderRecord, InstallationId, InstanceId, OrgId};
use jc_time::{TimestampTz, parse_rfc3339};
use serde_json::{Value, json};

fn at(s: &str) -> TimestampTz {
    TimestampTz(parse_rfc3339(s).expect(s))
}

fn folder(created_at: TimestampTz, updated_at: TimestampTz) -> FolderRecord {
    FolderRecord {
        installation_id: InstallationId::from_u128(1),
        org_id: OrgId::from_u128(2),
        instance_id: InstanceId::from_u128(3),
        project_id: None,
        uid: "f".into(),
        parent_uid: None,
        title: "F".into(),
        version: 1,
        managed_by: None,
        created_by: "u".into(),
        updated_by: "u".into(),
        created_at,
        updated_at,
    }
}

#[test]
fn record_timestamps_serialize_as_chrono_did() {
    for (sent, wire) in [
        ("2025-11-18T16:20:00Z", "2025-11-18T16:20:00Z"),
        ("2025-11-18T16:20:00.12Z", "2025-11-18T16:20:00.120Z"),
        ("2025-11-18T16:20:00.123456Z", "2025-11-18T16:20:00.123456Z"),
        (
            "2025-11-18T16:20:00.123456789Z",
            "2025-11-18T16:20:00.123456789Z",
        ),
        (
            "2025-11-18T17:20:00.000001+01:00",
            "2025-11-18T16:20:00.000001Z",
        ),
    ] {
        let rec = folder(at(sent), at(sent));
        let v = serde_json::to_value(&rec).expect("serialize");
        assert_eq!(v["createdAt"], json!(wire), "{sent}");
        assert_eq!(v["updatedAt"], json!(wire), "{sent}");
        let back: FolderRecord = serde_json::from_value(v).expect("deserialize");
        assert_eq!(back, rec);
    }
}

#[test]
fn record_timestamps_read_any_offset_and_refuse_a_date() {
    let mut v = serde_json::to_value(folder(TimestampTz::UNIX_EPOCH, TimestampTz::UNIX_EPOCH))
        .expect("serialize");
    v["createdAt"] = json!("2026-03-29T03:00:00+02:00");
    v["updatedAt"] = json!("2026-10-25T03:30:00+03:00");
    let rec: FolderRecord = serde_json::from_value(v.clone()).expect("deserialize");
    assert_eq!(rec.created_at, at("2026-03-29T01:00:00Z"));
    assert_eq!(rec.updated_at, at("2026-10-25T00:30:00Z"));
    v["createdAt"] = json!("2026-03-29");
    assert!(serde_json::from_value::<FolderRecord>(v).is_err());
}

#[test]
fn record_schema_keeps_date_time() {
    let schema = serde_json::to_value(schemars::schema_for!(FolderRecord)).expect("schema");
    let props = &schema["properties"];
    for field in ["createdAt", "updatedAt"] {
        assert_eq!(
            props[field],
            json!({ "type": "string", "format": "date-time" }),
            "{field}: {}",
            Value::to_string(props)
        );
    }
}
