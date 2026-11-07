// SPDX-License-Identifier: EUPL-1.2
//! `versions=all` on a temporal retrieve (T-4665, platform ADR-N-054 2.4):
//! the chain of each measurement, oldest first, each version with its
//! provenance and a superseded one with `supersededAt`, for a caller the
//! gateway says the space's Policy granted `readVersions`. Versions live in
//! the Postgres temporal store only, so this file skips loudly without
//! `ANTARES_TEST_DATABASE_URL` (container recipe in `antares-sql/tests/pg.rs`).
#![cfg(all(feature = "postgres", feature = "jc"))]

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

const GATEWAY: &str = "context-gateway.dev.serviceaccount.identity.linkerd.cluster.local";

async fn pg_state() -> Option<(AppState, String)> {
    pg_state_as(None).await
}

/// [`pg_state`] recording as `record` says; set before `wire`, which hands
/// the write path the state it records through.
async fn pg_state_as(record: Option<antares_api::TemporalRecord>) -> Option<(AppState, String)> {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = antares_sql::store::pg::connect(&url, 5)
        .await
        .expect("connect");
    let store =
        antares_sql::store::any::AnyStore::Pg(antares_sql::store::any::PgBackend::new(pool));
    let mut st = AppState::with_store("me".into(), std::sync::Arc::new(store), "postgres");
    if let Some(record) = record {
        st.temporal_record = record;
    }
    antares_api::wire(&mut st).await;
    st.provenance_from = Some(GATEWAY.into());
    let tenant = format!("ver{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    Some((st, tenant))
}

async fn send(
    st: &AppState,
    tenant: &str,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header("NGSILD-Tenant", tenant);
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let body = match body {
        Some(v) => {
            let s = v.to_string();
            b = b
                .header("Content-Type", "application/json")
                .header("Content-Length", s.len());
            Body::from(s)
        }
        None => Body::empty(),
    };
    let res = antares_api::router(st.clone())
        .oneshot(b.body(body).expect("request"))
        .await
        .expect("response");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn speed(value: f64, at: &str) -> Value {
    json!({"speed": {"type": "Property", "value": value, "observedAt": at}})
}

#[tokio::test(flavor = "multi_thread")]
async fn a_correction_shows_its_chain_only_to_a_caller_granted_read_versions() {
    let Some((st, t)) = pg_state().await else {
        return;
    };
    let id = "urn:ngsi-ld:Vehicle:ver1";
    let (early, late) = ("2026-10-18T07:00:00Z", "2026-10-18T08:00:00Z");
    let mut doc = speed(80.0, early);
    doc["id"] = json!(id);
    doc["type"] = json!("Vehicle");
    let (s, b) = send(&st, &t, "POST", "/ngsi-ld/v1/entities", &[], Some(doc)).await;
    assert_eq!(s, 201, "{b}");
    let attrs = format!("/ngsi-ld/v1/entities/{id}/attrs");
    let from_ui = [
        ("l5d-client-id", GATEWAY),
        ("jc-provenance-channel", "ui"),
        ("jc-provenance-actor", "steward-1"),
        ("jc-reason", "Sensor%20stuck"),
    ];
    for (value, at, headers) in [(60.0, early, &from_ui[..]), (50.0, late, &[][..])] {
        let (s, b) = send(&st, &t, "PATCH", &attrs, headers, Some(speed(value, at))).await;
        assert_eq!(s, 204, "{b}");
    }
    let temporal = |q: &str| format!("/ngsi-ld/v1/temporal/entities/{id}?attrs=speed{q}");
    let granted = [("l5d-client-id", GATEWAY), ("jc-read-versions", "true")];

    // Without the grant: the current version only, or a refusal when asked.
    let (s, plain) = send(&st, &t, "GET", &temporal(""), &[], None).await;
    assert_eq!(s, 200, "{plain}");
    assert_eq!(plain["speed"].as_array().map(Vec::len), Some(2), "{plain}");
    let (s, b) = send(
        &st,
        &t,
        "GET",
        &temporal("&versions=all"),
        &granted[..1],
        None,
    )
    .await;
    assert_eq!(s, 403, "{b}");
    let (s, b) = send(&st, &t, "GET", &temporal("&versions=some"), &granted, None).await;
    assert_eq!(s, 400, "{b}");

    // With it: the 07:00 measurement as v1 then v2, the 08:00 one as v1.
    let (s, all) = send(&st, &t, "GET", &temporal("&versions=all"), &granted, None).await;
    assert_eq!(s, 200, "{all}");
    let rows = all["speed"].as_array().expect("rows").clone();
    let seen: Vec<(Value, Value)> = rows
        .iter()
        .map(|r| (r["value"].clone(), r["version"].clone()))
        .collect();
    assert_eq!(
        seen,
        [
            (json!(80.0), json!(1)),
            (json!(60.0), json!(2)),
            (json!(50.0), json!(1))
        ],
        "{all}"
    );
    assert!(rows[0]["supersededAt"].is_string(), "{all}");
    assert!(rows[1].get("supersededAt").is_none(), "{all}");
    let fix = &rows[1]["provenance"];
    assert_eq!(
        (
            &fix["channel"],
            &fix["actor"],
            &fix["operation"],
            &fix["reason"]
        ),
        (
            &json!("ui"),
            &json!("steward-1"),
            &json!("correct"),
            &json!("Sensor stuck")
        ),
        "{all}"
    );
    assert_eq!(rows[0]["provenance"]["channel"], "api", "{all}");
    // The broker stamps when it stored each version (ADR-N-054 2.3, T-5361).
    for row in &rows {
        let at = row["provenance"]["writtenAt"].as_str().unwrap_or_default();
        assert!(
            chrono::DateTime::parse_from_rfc3339(at).is_ok(),
            "writtenAt {at:?}: {all}"
        );
    }

    // lastN counts measurements: the latest one, with every version it has.
    let (s, last) = send(
        &st,
        &t,
        "GET",
        &temporal("&versions=all&lastN=1"),
        &granted,
        None,
    )
    .await;
    assert_eq!(s, 200, "{last}");
    let last = match &last["speed"] {
        Value::Array(a) => a.clone(),
        v => vec![v.clone()],
    };
    assert_eq!(last.len(), 1, "{last:?}");
    assert_eq!(last[0]["value"], 50.0);
}

/// ADR-N-066 2.3, DM-149: a grant listing `speed` reads the versions of
/// `speed` alone; a read selecting `colour` is refused naming it, and a read
/// selecting nothing gets `colour`'s history without its versions.
#[tokio::test(flavor = "multi_thread")]
async fn versions_are_answered_only_for_the_attributes_the_grant_lists() {
    let Some((st, t)) = pg_state().await else {
        return;
    };
    let id = "urn:ngsi-ld:Vehicle:ver-listed";
    let at = "2026-10-18T07:00:00Z";
    let both = |speed: f64, colour: &str| {
        json!({"speed": {"type": "Property", "value": speed, "observedAt": at},
               "colour": {"type": "Property", "value": colour, "observedAt": at}})
    };
    let mut doc = both(80.0, "red");
    doc["id"] = json!(id);
    doc["type"] = json!("Vehicle");
    let (s, b) = send(&st, &t, "POST", "/ngsi-ld/v1/entities", &[], Some(doc)).await;
    assert_eq!(s, 201, "{b}");
    let attrs = format!("/ngsi-ld/v1/entities/{id}/attrs");
    let (s, b) = send(&st, &t, "PATCH", &attrs, &[], Some(both(60.0, "blue"))).await;
    assert_eq!(s, 204, "{b}");
    let speed_only = [("l5d-client-id", GATEWAY), ("jc-read-versions", "speed")];
    let temporal = |q: &str| format!("/ngsi-ld/v1/temporal/entities/{id}?versions=all{q}");

    let (s, b) = send(
        &st,
        &t,
        "GET",
        &temporal("&attrs=colour"),
        &speed_only,
        None,
    )
    .await;
    assert_eq!(s, 403, "{b}");
    assert!(b.to_string().contains("colour"), "{b}");
    let (s, b) = send(
        &st,
        &t,
        "GET",
        &temporal("&attrs=speed,colour"),
        &speed_only,
        None,
    )
    .await;
    assert_eq!(s, 403, "{b}");

    let (s, all) = send(&st, &t, "GET", &temporal(""), &speed_only, None).await;
    assert_eq!(s, 200, "{all}");
    let rows = |a: &str| match &all[a] {
        Value::Array(rows) => rows.clone(),
        v => vec![v.clone()],
    };
    assert_eq!(rows("speed").len(), 2, "{all}");
    assert!(
        rows("speed").iter().all(|r| r["version"].is_number()),
        "{all}"
    );
    assert!(
        rows("colour").iter().all(|r| r.get("version").is_none()),
        "{all}"
    );
    assert!(
        rows("colour").iter().all(|r| r.get("provenance").is_none()),
        "{all}"
    );
}

/// One write per channel (Done when of T-5361): what each row stores is
/// what the gateway said when the gateway's mesh identity sent it, and
/// `channel: api` with no actor for a write that reached the broker's port
/// another way, whatever headers it carried.
#[tokio::test(flavor = "multi_thread")]
async fn each_channel_is_stored_as_the_gateway_named_it() {
    let Some((st, t)) = pg_state().await else {
        return;
    };
    let id = "urn:ngsi-ld:Vehicle:ver2";
    let mut doc = speed(1.0, "2026-10-18T06:00:00Z");
    doc["id"] = json!(id);
    doc["type"] = json!("Vehicle");
    let (s, b) = send(&st, &t, "POST", "/ngsi-ld/v1/entities", &[], Some(doc)).await;
    assert_eq!(s, 201, "{b}");
    let request_id = "6f1c2b9e-0d4a-4c3e-9a07-5b1d2e3f4a5b";
    let writes = [
        (
            2.0,
            "2026-10-18T07:00:00Z",
            vec![
                ("l5d-client-id", GATEWAY),
                ("jc-provenance-channel", "pipeline"),
                ("jc-provenance-actor", "serviceAccount%3Apl-parking"),
                ("x-request-id", request_id),
            ],
        ),
        (
            3.0,
            "2026-10-18T08:00:00Z",
            vec![
                ("l5d-client-id", GATEWAY),
                ("jc-provenance-channel", "ui"),
                ("jc-provenance-actor", "3b0f5c1e-steward"),
                ("jc-reason", "Counted%20on%20site"),
            ],
        ),
        (
            4.0,
            "2026-10-18T09:00:00Z",
            vec![
                ("jc-provenance-channel", "pipeline"),
                ("jc-provenance-actor", "serviceAccount%3Apl-forged"),
            ],
        ),
        (
            5.0,
            "2026-10-18T10:00:00Z",
            vec![
                (
                    "l5d-client-id",
                    "intruder.dev.serviceaccount.identity.linkerd.cluster.local",
                ),
                ("jc-provenance-channel", "ui"),
                ("jc-provenance-actor", "admin"),
            ],
        ),
    ];
    let attrs = format!("/ngsi-ld/v1/entities/{id}/attrs");
    for (value, at, headers) in &writes {
        let (s, b) = send(&st, &t, "PATCH", &attrs, headers, Some(speed(*value, at))).await;
        assert_eq!(s, 204, "{b}");
    }
    let granted = [("l5d-client-id", GATEWAY), ("jc-read-versions", "true")];
    let uri = format!("/ngsi-ld/v1/temporal/entities/{id}?attrs=speed&versions=all");
    let (s, all) = send(&st, &t, "GET", &uri, &granted, None).await;
    assert_eq!(s, 200, "{all}");
    let rows = all["speed"].as_array().expect("rows").clone();
    let stored: Vec<(Value, Value, Value, Value, Value)> = rows
        .iter()
        .map(|r| {
            let p = &r["provenance"];
            (
                r["value"].clone(),
                p["channel"].clone(),
                p["actor"].clone(),
                p["requestId"].clone(),
                p["reason"].clone(),
            )
        })
        .collect();
    let null = Value::Null;
    assert_eq!(
        stored,
        [
            (
                json!(1.0),
                json!("api"),
                null.clone(),
                null.clone(),
                null.clone()
            ),
            (
                json!(2.0),
                json!("pipeline"),
                json!("serviceAccount:pl-parking"),
                json!(request_id),
                null.clone()
            ),
            (
                json!(3.0),
                json!("ui"),
                json!("3b0f5c1e-steward"),
                null.clone(),
                json!("Counted on site")
            ),
            (
                json!(4.0),
                json!("api"),
                null.clone(),
                null.clone(),
                null.clone()
            ),
            (json!(5.0), json!("api"), null.clone(), null.clone(), null),
        ],
        "{all}"
    );
    for row in &rows {
        let at = row["provenance"]["writtenAt"].as_str().unwrap_or_default();
        assert!(
            chrono::DateTime::parse_from_rfc3339(at).is_ok(),
            "writtenAt {at:?}: {all}"
        );
    }
}

/// Audit mode (platform ADR-N-054 2.8, DM-136): where the type keeps no
/// history (`ruled` with no rule), a write through an audited Endpoint is
/// kept as a version with the writer's Organization and kind, an exact
/// repeat adds nothing, and the same writes without the gateway's audit mark,
/// or with a forged one, add nothing.
#[tokio::test(flavor = "multi_thread")]
async fn an_audited_endpoint_keeps_every_change_where_history_is_off() {
    let Some((st, t)) = pg_state_as(Some(antares_api::TemporalRecord::Ruled)).await else {
        return;
    };
    let audited = [
        ("l5d-client-id", GATEWAY),
        ("jc-provenance-channel", "api"),
        ("jc-provenance-actor", "serviceAccount%3Ahel-traffic"),
        ("jc-provenance-organization", "hel"),
        ("jc-provenance-actor-kind", "serviceAccount"),
        ("jc-provenance-audit", "changes"),
    ];
    let plain = &audited[..5];
    let forged = [("jc-provenance-audit", "changes")];
    let granted = [("l5d-client-id", GATEWAY), ("jc-read-versions", "true")];
    let mut values = Vec::new();
    for (id, headers) in [
        ("urn:ngsi-ld:Vehicle:audit-on", &audited[..]),
        ("urn:ngsi-ld:Vehicle:audit-off", plain),
        ("urn:ngsi-ld:Vehicle:audit-forged", &forged[..]),
    ] {
        let mut doc = speed(1.0, "2026-10-18T06:00:00Z");
        doc["id"] = json!(id);
        doc["type"] = json!("Vehicle");
        let (s, b) = send(&st, &t, "POST", "/ngsi-ld/v1/entities", headers, Some(doc)).await;
        assert_eq!(s, 201, "{b}");
        let attrs = format!("/ngsi-ld/v1/entities/{id}/attrs");
        for (value, at) in [
            (2.0, "2026-10-18T07:00:00Z"),
            (2.0, "2026-10-18T07:00:00Z"),
            (3.0, "2026-10-18T08:00:00Z"),
        ] {
            let (s, b) = send(&st, &t, "PATCH", &attrs, headers, Some(speed(value, at))).await;
            assert_eq!(s, 204, "{b}");
        }
        let uri = format!("/ngsi-ld/v1/temporal/entities/{id}?attrs=speed&versions=all");
        let (_, all) = send(&st, &t, "GET", &uri, &granted, None).await;
        let rows = all["speed"].as_array().cloned().unwrap_or_default();
        values.push(rows.iter().map(|r| r["value"].clone()).collect::<Vec<_>>());
        if id.ends_with("audit-on") {
            for row in &rows {
                let p = &row["provenance"];
                assert_eq!(p["organization"], json!("hel"), "{row}");
                assert_eq!(p["actorKind"], json!("serviceAccount"), "{row}");
                assert_eq!(p["audit"], json!(true), "{row}");
            }
        }
    }
    assert_eq!(
        values,
        [vec![json!(1.0), json!(2.0), json!(3.0)], vec![], vec![]]
    );
}

/// Versions on request (platform ADR-N-054 2.9, DM-145, DM-146): where the
/// type keeps no history, a rename sent with the gateway's audit mark (the
/// gateway's translation of `JC-Record: version`) keeps the replaced name
/// first, attributed to whoever the current state says wrote it, then each
/// new name with its own writer and reason; an exact repeat keeps nothing,
/// the same renames without the mark keep nothing, and a temporal read
/// without `versions=all` shows none of it.
#[tokio::test(flavor = "multi_thread")]
async fn a_rename_asked_as_a_version_keeps_the_old_name_and_the_new_where_history_is_off() {
    let Some((st, t)) = pg_state_as(Some(antares_api::TemporalRecord::Ruled)).await else {
        return;
    };
    let creator = [
        ("l5d-client-id", GATEWAY),
        ("jc-provenance-channel", "pipeline"),
        ("jc-provenance-actor", "serviceAccount%3Apl-stations"),
        ("jc-provenance-organization", "hel"),
        ("jc-provenance-actor-kind", "pipeline"),
    ];
    let versioned = [
        ("l5d-client-id", GATEWAY),
        ("jc-provenance-channel", "ui"),
        ("jc-provenance-actor", "user%3Ajana"),
        ("jc-provenance-organization", "bb"),
        ("jc-provenance-actor-kind", "person"),
        ("jc-provenance-audit", "changes"),
        ("jc-reason", "Renamed%20by%20the%20council"),
    ];
    let plain = &versioned[..5];
    let granted = [("l5d-client-id", GATEWAY), ("jc-read-versions", "true")];
    let name = |v: &str| json!({"name": {"type": "Property", "value": v}});
    let mut chains = Vec::new();
    for (id, renamer) in [
        ("urn:ngsi-ld:Station:renamed", &versioned[..]),
        ("urn:ngsi-ld:Station:plain", plain),
    ] {
        let mut doc = name("Hlavná stanica");
        doc["id"] = json!(id);
        doc["type"] = json!("Station");
        let (s, b) = send(&st, &t, "POST", "/ngsi-ld/v1/entities", &creator, Some(doc)).await;
        assert_eq!(s, 201, "{b}");
        let attrs = format!("/ngsi-ld/v1/entities/{id}/attrs");
        for value in ["Stanica Centrum", "Stanica Centrum", "Stanica Námestie"] {
            let (s, b) = send(&st, &t, "PATCH", &attrs, renamer, Some(name(value))).await;
            assert_eq!(s, 204, "{b}");
        }
        let uri = format!("/ngsi-ld/v1/temporal/entities/{id}?attrs=name&versions=all");
        let (s, all) = send(&st, &t, "GET", &uri, &granted, None).await;
        // nothing kept of `name` is the 404 of an Attribute the Entity never had
        let expected = if id.ends_with("renamed") { 200 } else { 404 };
        assert_eq!(s, expected, "{all}");
        let rows = all["name"].as_array().cloned().unwrap_or_default();
        if id.ends_with("renamed") {
            let p = |i: usize| rows.get(i).map(|r| r["provenance"].clone());
            let p0 = p(0).unwrap_or_default();
            assert_eq!(p0["operation"], json!("create"), "{all}");
            assert_eq!(p0["organization"], json!("hel"), "{all}");
            assert_eq!(p0["audit"], json!(true), "{all}");
            for i in [1, 2] {
                let pi = p(i).unwrap_or_default();
                assert_eq!(pi["organization"], json!("bb"), "{all}");
                assert_eq!(pi["actorKind"], json!("person"), "{all}");
                assert_eq!(pi["reason"], json!("Renamed by the council"), "{all}");
                assert!(pi["writtenAt"].is_string(), "{all}");
            }
            let (_, flat) = send(
                &st,
                &t,
                "GET",
                &format!("/ngsi-ld/v1/temporal/entities/{id}?attrs=name"),
                &granted,
                None,
            )
            .await;
            assert!(flat.get("name").is_none(), "{flat}");
        }
        chains.push(rows.iter().map(|r| r["value"].clone()).collect::<Vec<_>>());
    }
    assert_eq!(
        chains,
        [
            vec![
                json!("Hlavná stanica"),
                json!("Stanica Centrum"),
                json!("Stanica Námestie")
            ],
            vec![]
        ]
    );
}
