#![forbid(unsafe_code)]
//! The NGSI-LD driver against the ETSI NGSI-LD API (CIM 009) and the NGP Grafana plugin's target
//! model: the request it builds, the frames it reads from entities and temporal entities, and
//! that a caller's token leaves only after an exchange for the Endpoint's audience.

mod common;

use common::{Reply, closed_port, ctx, loopback_guard, mock, q};
use daiya_model::{FieldType, Subject};
use daiya_query::ngsild::{
    build_ngsild_request, extract_ngsi_value, grafana_unit, normalize_query_type, with_variables,
};
use daiya_query::{
    Datasource, DenyTokenExchanger, DsError, HealthState, NgsiLdDatasource,
    PassthroughAudienceExchanger, Query, ResourceReq,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

fn driver(url: &str) -> NgsiLdDatasource {
    NgsiLdDatasource::new(
        "air",
        url,
        false,
        loopback_guard(),
        Arc::new(PassthroughAudienceExchanger),
    )
}

fn params(model: &Value) -> (String, BTreeMap<String, String>) {
    let (path, p) = build_ngsild_request(model, 1_700_000_000_000, 1_700_003_600_000, 60_000, 500);
    (path, p.into_iter().collect())
}

#[test]
fn query_types_units_and_values_follow_the_plugin() {
    for (raw, t) in [
        (None, "temporal"),
        (Some("Entity"), "entity"),
        (Some("entities"), "entity"),
        (Some("timseries"), "temporal"),
        (Some("timeseries"), "temporal"),
        (Some("geo"), "geo"),
        (Some("node_graph"), "nodegraph"),
        (Some("graph"), "nodegraph"),
        (Some("other"), "temporal"),
    ] {
        assert_eq!(normalize_query_type(raw), t, "{raw:?}");
    }
    assert_eq!(grafana_unit(Some(" CEL ")).as_deref(), Some("celsius"));
    assert_eq!(grafana_unit(Some("KTM")).as_deref(), Some("lengthkm"));
    for (code, unit) in [
        ("FAH", "fahrenheit"),
        ("KEL", "kelvin"),
        ("P1", "percent"),
        ("59", "ppm"),
        ("61", "conppb"),
        ("GP", "conmgm3"),
        ("MMT", "lengthmm"),
        ("MTR", "lengthm"),
        ("KMT", "lengthkm"),
        ("DD", "degree"),
        ("C26", "ms"),
        ("SEC", "s"),
        ("MIN", "m"),
        ("HUR", "h"),
        ("DAY", "d"),
        ("MTS", "velocityms"),
        ("KMH", "velocitykmh"),
        ("KGM", "masskg"),
        ("GRM", "massg"),
        ("TNE", "masst"),
        ("PAL", "pressurepa"),
        ("A97", "pressurehpa"),
        ("KPA", "pressurekpa"),
        ("MBR", "pressurembar"),
        ("BAR", "pressurebar"),
        ("JOU", "joule"),
        ("WHR", "watth"),
        ("KWH", "kwatth"),
        ("WTT", "watt"),
        ("KWT", "kwatt"),
        ("AMP", "amp"),
        ("VLT", "volt"),
        ("HTZ", "hertz"),
        ("MTK", "areaM2"),
        ("MTQ", "m3"),
        ("LTR", "litre"),
    ] {
        assert_eq!(grafana_unit(Some(code)).as_deref(), Some(unit), "{code}");
    }
    assert_eq!(grafana_unit(Some("XYZ")).as_deref(), Some("suffix: XYZ"));
    assert_eq!(grafana_unit(Some("  ")), None);
    assert_eq!(grafana_unit(None), None);

    for (attr, want) in [
        (json!({ "type": "Property", "value": 3 }), json!(3)),
        (
            json!({ "value": { "@value": "2024-01-01", "@type": "Date" } }),
            json!("2024-01-01"),
        ),
        (
            json!({ "type": "Relationship", "object": "urn:x" }),
            json!("urn:x"),
        ),
        (
            json!({ "languageMap": { "sk": "Ahoj", "en": "Hi" } }),
            json!("Hi"),
        ),
        (
            json!({ "languageMap": { "@none": "N", "en": "Hi" } }),
            json!("N"),
        ),
        (json!({ "languageMap": { "de": "Hallo" } }), json!("Hallo")),
        (json!({ "vocab": "Celsius" }), json!("Celsius")),
        (json!({ "json": { "a": 1 } }), json!({ "a": 1 })),
        (json!({ "valueList": [1, 2] }), json!([1, 2])),
        (json!({ "languageMap": {} }), json!({ "languageMap": {} })),
        (json!({ "other": 1 }), json!({ "other": 1 })),
        (json!(7), json!(7)),
    ] {
        assert_eq!(extract_ngsi_value(&attr, None), want, "{attr}");
    }
    assert_eq!(
        extract_ngsi_value(
            &json!({ "languageMap": { "sk": "Ahoj", "en": "Hi" } }),
            Some("sk")
        ),
        json!("Ahoj")
    );
}

#[test]
fn an_entity_query_selects_and_projects() {
    let (path, p) = params(&json!({
        "queryType": "entity", "entityType": " AirQualityObserved ", "entityId": "urn:a",
        "idPattern": "urn:.*", "attributes": ["pm10", " ", "pm25"], "entityName": "name",
        "customQuery": "pm10>40", "scopeQuery": "/city/#", "lang": "sk", "limit": 5000,
        "georel": "near;maxDistance==500", "coordinates": "[17.1, 48.1]", "geoproperty": "loc"
    }));
    assert_eq!(path, "/entities");
    assert_eq!(p["type"], "AirQualityObserved");
    assert_eq!(p["id"], "urn:a");
    assert_eq!(p["idPattern"], "urn:.*");
    // The entity name attribute joins the projection so a label can be read.
    assert_eq!(p["attrs"], "pm10,pm25,name");
    assert_eq!(p["q"], "pm10>40");
    assert_eq!(p["scopeQ"], "/city/#");
    assert_eq!(p["georel"], "near;maxDistance==500");
    assert_eq!(p["geometry"], "Point");
    assert_eq!(p["coordinates"], "[17.1,48.1]");
    assert_eq!(p["geoproperty"], "loc");
    assert_eq!(p["lang"], "sk");
    assert_eq!(p["limit"], "500", "the page size caps the limit");
    assert!(!p.contains_key("local"));
    assert!(!p.contains_key("timerel"));

    // A geo query adds its geoproperty to the projection; an id-only entity query is local.
    let (_, p) = params(&json!({ "queryType": "geo", "attributes": ["pm10"] }));
    assert_eq!(p["attrs"], "pm10,location");
    let (_, p) = params(&json!({ "queryType": "entity", "entityId": "urn:a", "limit": 10 }));
    assert_eq!(p["local"], "true");
    assert_eq!(p["limit"], "10");
    // Georel without coordinates is no geo-query.
    let (_, p) = params(&json!({ "queryType": "entity", "georel": "near" }));
    assert!(!p.contains_key("georel"));
}

#[test]
fn a_temporal_query_asks_between_the_range_ends() {
    let (path, p) = params(&json!({
        "queryType": "timseries", "entityType": "T", "attributes": ["pm10"],
        "timeProperty": "modifiedAt", "lastN": 10
    }));
    assert_eq!(path, "/temporal/entities");
    assert_eq!(p["format"], "temporalValues");
    assert_eq!(p["timerel"], "between");
    assert_eq!(p["timeAt"], "2023-11-14T22:13:20.000Z");
    assert_eq!(p["endTimeAt"], "2023-11-14T23:13:20.000Z");
    assert_eq!(p["timeproperty"], "modifiedAt");
    assert_eq!(p["lastN"], "10");

    let (_, p) = params(&json!({
        "entityType": "T", "timeProperty": "observedAt", "aggrMethod": "avg,max",
        "aggrPeriodDuration": "auto", "lastN": 0
    }));
    assert_eq!(p["format"], "aggregatedValues");
    assert_eq!(p["aggrMethods"], "avg,max");
    assert_eq!(p["aggrPeriodDuration"], "PT60S");
    assert!(!p.contains_key("timeproperty"));
    assert!(!p.contains_key("lastN"));
    let (_, p) =
        params(&json!({ "entityType": "T", "aggrMethod": "sum", "aggrPeriodDuration": "P1D" }));
    assert_eq!(p["aggrPeriodDuration"], "P1D");
    // A temporal query with only a q filter is local; q alone is no temporal selector.
    let (_, p) = params(&json!({ "customQuery": "a>1" }));
    assert_eq!(p["local"], "true");
}

#[tokio::test]
async fn entities_become_one_table_with_units_and_the_newest_observation() {
    let m = mock(vec![(
        "/api/endpoint/air/ngsi-ld/v1/entities",
        Reply::json(
            200,
            &json!([
                { "id": "urn:ngsi-ld:A:1", "type": "A", "@context": "x",
                  "pm10": { "type": "Property", "value": 12.5, "unitCode": "GQ", "observedAt": "2024-01-01T00:00:00Z" },
                  "name": { "type": "Property", "value": "Station 1" },
                  "on": [{ "type": "Property", "value": true, "observedAt": "2024-01-01T01:00:00Z" }, { "value": false }] },
                { "id": "urn:ngsi-ld:A:2", "type": ["A", "B"],
                  "pm10": { "type": "Property", "value": null },
                  "extra": { "type": "Property", "value": { "k": 1 } } },
                "not an entity"
            ]),
        ),
    )])
    .await;
    let d = driver(&format!("{}/api/endpoint/air/", m.base));
    assert_eq!(d.endpoint_slug.as_deref(), Some("air"));
    let sets = d
        .query(
            &ctx(Subject::anonymous()),
            &q(json!({ "queryType": "entity", "entityType": "A" })),
        )
        .await
        .unwrap();
    let frame = &sets[0].frames[0];
    let names: Vec<&str> = frame.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["id", "type", "observedAt", "pm10", "name", "on", "extra"]
    );
    assert_eq!(frame.fields[1].values, vec![json!("A"), json!("A,B")]);
    assert_eq!(frame.fields[2].field_type, FieldType::Time);
    assert_eq!(
        frame.fields[2].values,
        vec![json!(1_704_070_800_000_i64), Value::Null]
    );
    assert_eq!(frame.fields[3].unit.as_deref(), Some("conμgm3"));
    assert_eq!(frame.fields[3].field_type, FieldType::Number);
    assert_eq!(frame.fields[3].values, vec![json!(12.5), Value::Null]);
    assert_eq!(frame.fields[4].field_type, FieldType::String);
    assert_eq!(frame.fields[5].field_type, FieldType::Bool);
    assert_eq!(frame.fields[5].values, vec![json!(true), Value::Null]);
    assert_eq!(frame.fields[6].field_type, FieldType::Json);
    assert_eq!(
        frame.meta.preferred_visualisation_type.as_deref(),
        Some("table")
    );

    // An explicit attribute list is a projection, even when the broker answers more; a
    // column with no value at all is numeric.
    let sets = d
        .query(
            &ctx(Subject::anonymous()),
            &q(json!({ "queryType": "geo", "attributes": ["absent", "name"] })),
        )
        .await
        .unwrap();
    let names: Vec<&str> = sets[0].frames[0]
        .fields
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(names, vec!["id", "type", "absent", "name", "location"]);
    assert_eq!(sets[0].frames[0].fields[2].field_type, FieldType::Number);

    // format: table turns a nodegraph target into a table too; a single object answer is one row.
    let one = mock(vec![(
        "/ngsi-ld/v1/entities",
        Reply::json(
            200,
            &json!({ "id": "urn:x", "type": "T", "a": { "value": 1 } }),
        ),
    )])
    .await;
    let sets = driver(&format!("{}/ngsi-ld/v1", one.base))
        .query(
            &ctx(Subject::anonymous()),
            &q(json!({ "queryType": "nodegraph", "format": "table", "entityType": "T" })),
        )
        .await
        .unwrap();
    assert_eq!(sets[0].frames[0].fields[0].values, vec![json!("urn:x")]);
    assert!(
        one.target(0).starts_with("/ngsi-ld/v1/entities?"),
        "{}",
        one.target(0)
    );
}

#[tokio::test]
async fn temporal_entities_become_one_series_per_attribute_and_method() {
    let body = json!([
        { "id": "urn:ngsi-ld:Sensor:s1", "type": "Sensor",
          "name": { "type": "Property", "value": "North" },
          "temp": { "type": "Property", "unitCode": "CEL",
                    "values": [[21.5, "2024-01-01T00:10:00Z"], [20.0, "2024-01-01T00:00:00Z"], ["x", "bad time"], [1]] },
          "load": [
            { "type": "Property", "avg": [[1.5, "2024-01-01T00:00:00Z", "2024-01-01T01:00:00Z"]],
              "max": [[3, "2024-01-01T00:00:00Z"], ["n"]] },
            "not an instance"
          ] },
        { "id": "s2", "type": "Sensor", "temp": { "values": [[{ "@value": 1 }, "2024-01-01T00:00:00Z"]] } },
        42
    ]);
    let m = mock(vec![(
        "/ngsi-ld/v1/temporal/entities",
        Reply::json(200, &body),
    )])
    .await;
    let d = driver(&m.base);
    let run = |model: Value| {
        let d = &d;
        async move {
            d.query(&ctx(Subject::anonymous()), &q(model))
                .await
                .unwrap()
                .remove(0)
                .frames
        }
    };
    let frames = run(json!({ "entityType": "Sensor", "entityName": "name" })).await;
    let displays: Vec<String> = frames
        .iter()
        .map(|f| f.fields[1].display_name.clone().unwrap())
        .collect();
    assert_eq!(
        displays,
        vec![
            "North:temp".to_owned(),
            "North:load (avg)".to_owned(),
            "North:load (max)".to_owned(),
            "s2:temp".to_owned()
        ]
    );
    let temp = &frames[0];
    assert_eq!(temp.name, "North");
    // Points sorted by time; an unreadable time counts as 0, a text value is no number.
    assert_eq!(
        temp.fields[0].values,
        vec![
            json!(0),
            json!(1_704_067_200_000_i64),
            json!(1_704_067_800_000_i64)
        ]
    );
    assert_eq!(temp.fields[1].f64_at(1), Some(20.0));
    assert_eq!(temp.fields[1].f64_at(0), None);
    assert_eq!(temp.fields[1].unit.as_deref(), Some("celsius"));
    assert_eq!(temp.fields[1].labels["entity"], "urn:ngsi-ld:Sensor:s1");
    assert_eq!(temp.fields[1].labels["type"], "Sensor");
    assert_eq!(frames[1].fields[1].labels["aggregation"], "avg");
    assert_eq!(
        frames[2].fields[0].values.len(),
        1,
        "a short bucket is skipped"
    );
    assert_eq!(frames[3].fields[1].f64_at(0), Some(1.0), "@value is read");

    let frames = run(json!({ "entityName": "name", "namePattern": "entity_name" })).await;
    assert_eq!(frames[0].fields[1].display_name.as_deref(), Some("North"));
    // namePattern attribute names a series by its attribute only.
    let frames = run(json!({ "entityName": "name", "namePattern": "attribute" })).await;
    assert_eq!(frames[0].fields[1].display_name.as_deref(), Some("temp"));
    assert_eq!(
        frames[1].fields[1].display_name.as_deref(),
        Some("load (avg)")
    );
    let frames = run(json!({ "entityName": "id" })).await;
    assert_eq!(frames[0].name, "urn:ngsi-ld:Sensor:s1");
    let frames = run(json!({ "useLongEntityName": true })).await;
    assert_eq!(frames[0].name, "urn:ngsi-ld:Sensor:s1");
    let frames = run(json!({})).await;
    assert_eq!(frames[0].name, "s1", "the short id is the last segment");
}

#[tokio::test]
async fn the_caller_token_leaves_only_exchanged_and_headers_carry_context_and_tenant() {
    let m = mock(vec![("/", Reply::json(200, &json!([])))]).await;
    let mut d = driver(&m.base);
    d.context_url = Some(" https://ctx.example/c.jsonld ".into());
    d.tenant = Some(" city ".into());
    let caller =
        Subject::authenticated("alice", vec![common::ORG], vec![]).with_bearer_token("jwt-a");
    d.query(&ctx(caller), &q(json!({ "entityType": "T" })))
        .await
        .unwrap();
    let head = m.head(0);
    let lower = head.to_ascii_lowercase();
    // No slug in the URL: the audience is the datasource uid.
    assert!(
        lower.contains("authorization: bearer exchanged:air:jwt-a"),
        "{head}"
    );
    assert!(!lower.contains("bearer jwt-a"), "{head}");
    assert!(lower.contains("ngsild-tenant: city"), "{head}");
    assert!(
        head.contains(
            "<https://ctx.example/c.jsonld>; rel=\"http://www.w3.org/ns/json-ld#context\""
        ),
        "{head}"
    );

    // No token and no pass-through: no Authorization header at all; empty settings add nothing.
    d.context_url = Some(" ".into());
    d.tenant = Some("".into());
    d.query(&ctx(Subject::anonymous()), &q(json!({ "entityType": "T" })))
        .await
        .unwrap();
    let lower = m.head(1).to_ascii_lowercase();
    assert!(!lower.contains("authorization"), "{lower}");
    assert!(
        !lower.contains("ngsild-tenant") && !lower.contains("link:"),
        "{lower}"
    );

    // oauthPassThru without a caller token is refused before anything leaves.
    let pass = NgsiLdDatasource::new(
        "air",
        format!("{}/api/endpoint/slug-x", m.base),
        true,
        loopback_guard(),
        Arc::new(PassthroughAudienceExchanger),
    );
    let err = pass
        .query(&ctx(Subject::anonymous()), &q(json!({ "entityType": "T" })))
        .await
        .unwrap_err();
    assert!(
        matches!(err, DsError::TokenExchange { ref audience, .. } if audience == "slug-x"),
        "{err}"
    );
    let h = pass.health(&ctx(Subject::anonymous())).await;
    assert_eq!(h.status, HealthState::Error);
    assert!(h.message.contains("token exchange"), "{}", h.message);
    let caller =
        Subject::authenticated("bob", vec![common::ORG], vec![]).with_bearer_token("jwt-b");
    pass.query(&ctx(caller), &q(json!({ "entityType": "T" })))
        .await
        .unwrap();
    assert!(
        m.head(2)
            .to_ascii_lowercase()
            .contains("authorization: bearer exchanged:slug-x:jwt-b")
    );

    // With the deny exchanger a caller's token is refused, not forwarded.
    let deny = NgsiLdDatasource::new(
        "air",
        m.base.clone(),
        false,
        loopback_guard(),
        Arc::new(DenyTokenExchanger),
    );
    let caller = Subject::authenticated("c", vec![common::ORG], vec![]).with_bearer_token("jwt-c");
    let err = deny
        .resource(
            &ctx(caller),
            ResourceReq {
                path: "types".into(),
                params: BTreeMap::new(),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, DsError::TokenExchange { .. }), "{err}");
    assert_eq!(m.count(), 3);
}

#[tokio::test]
async fn upstream_failures_are_errors_with_the_status() {
    let m = mock(vec![
        (
            "/ngsi-ld/v1/entities?type=Forbidden",
            Reply::raw(403, &"no ".repeat(100)),
        ),
        ("/ngsi-ld/v1/entities?type=Html", Reply::raw(200, "<html>")),
        ("/ngsi-ld/v1/entities?type=Number", Reply::raw(200, "42")),
    ])
    .await;
    let d = driver(&m.base);
    let c = ctx(Subject::anonymous());
    let err = d
        .query(
            &c,
            &q(json!({ "queryType": "entity", "entityType": "Forbidden" })),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&err, DsError::Upstream { status: 403, message } if message.len() == 200),
        "{err}"
    );
    let err = d
        .query(
            &c,
            &q(json!({ "queryType": "entity", "entityType": "Html" })),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("valid JSON"), "{err}");
    // A scalar answer holds no entities.
    let sets = d
        .query(
            &c,
            &q(json!({ "queryType": "entity", "entityType": "Number" })),
        )
        .await
        .unwrap();
    assert_eq!(sets[0].frames[0].fields[0].values.len(), 0);
    // A hidden target is not run.
    let hidden = vec![Query {
        ref_id: "H".into(),
        hide: true,
        model: json!({}),
    }];
    assert!(d.query(&c, &hidden).await.unwrap().is_empty());
    assert_eq!(m.count(), 3);
}

#[tokio::test]
async fn health_and_editor_resources_read_types_and_attributes() {
    let m = mock(vec![
        (
            "/ngsi-ld/v1/types?details=true",
            Reply::json(200, &json!([{ "id": "T" }])),
        ),
        ("/ngsi-ld/v1/attributes", Reply::raw(500, "broker down")),
    ])
    .await;
    let d = driver(&m.base);
    let c = ctx(Subject::anonymous());
    let h = d.health(&c).await;
    assert_eq!(h.status, HealthState::Ok, "{}", h.message);
    let types = d
        .resource(
            &c,
            ResourceReq {
                path: "/types/".into(),
                params: BTreeMap::new(),
            },
        )
        .await
        .unwrap();
    assert_eq!(types.data, json!([{ "id": "T" }]));
    let err = d
        .resource(
            &c,
            ResourceReq {
                path: "attributes".into(),
                params: BTreeMap::new(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        err,
        DsError::Upstream {
            status: 500,
            message: "broker down".into()
        }
    );
    let err = d
        .resource(
            &c,
            ResourceReq {
                path: "entities".into(),
                params: BTreeMap::new(),
            },
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("unsupported NGSI-LD resource path")
    );

    let bad = mock(vec![("/ngsi-ld/v1/types", Reply::raw(200, "nope"))]).await;
    let err = driver(&bad.base)
        .resource(
            &c,
            ResourceReq {
                path: "types".into(),
                params: BTreeMap::new(),
            },
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("invalid JSON"), "{err}");

    let down = mock(vec![("/", Reply::raw(503, ""))]).await;
    let h = driver(&down.base).health(&c).await;
    assert_eq!(h.status, HealthState::Error);
    assert!(h.message.contains("503"));
    let port = closed_port().await;
    let h = driver(&format!("http://127.0.0.1:{port}")).health(&c).await;
    assert_eq!(h.status, HealthState::Error);
    assert!(h.message.contains("Cannot reach"), "{}", h.message);
}

#[test]
fn import_keeps_every_target_field_and_warns_on_what_it_does_not_know() {
    let d = driver("https://broker.example");
    assert_eq!(d.kind(), "ngsild");
    let target = json!({
        "refId": "B", "hide": true, "queryType": "timseries", "entityType": "T",
        "attributes": ["a"], "graphDepth": 2, "mystery": 1
    });
    let imported = d.import(&target).unwrap();
    assert_eq!(imported.query.ref_id, "B");
    assert!(imported.query.hide);
    assert_eq!(imported.warnings.len(), 2);
    assert!(imported.warnings[0].contains("timseries"));
    assert!(imported.warnings[1].contains("'mystery'"));
    assert_eq!(d.export(&imported.query), target, "export is lossless");
    let plain = d.import(&json!({ "entityType": "T" })).unwrap();
    assert_eq!(plain.query.ref_id, "A");
    assert!(plain.warnings.is_empty());
    assert_eq!(
        d.export(&plain.query),
        json!({ "entityType": "T", "refId": "A" })
    );
    assert!(d.import(&json!("T")).is_err());
    assert_eq!(
        d.export(&Query {
            ref_id: "Z".into(),
            hide: false,
            model: json!(null)
        }),
        json!({ "refId": "Z" })
    );
    let editor = d.editor();
    assert_eq!(editor.kind, "ngsild");
    assert!(
        editor
            .field_hints
            .iter()
            .any(|h| h.resource_path.as_deref() == Some("types"))
    );
}

#[test]
fn a_dashboard_variable_reaches_the_q_as_grafana_replaces_it() {
    let model = json!({ "queryType": "entity", "entityType": "AirQualityObserved",
        "customQuery": "station~=\"^${station:regex}$\";pm25>$floor;x==\"$unknown\";y==\"${open\"" });
    let vars = BTreeMap::from([
        ("station".to_owned(), "(Kallio|Vallila\\.2)".to_owned()),
        ("floor".to_owned(), "5".to_owned()),
    ]);
    let (_, params) = params(&with_variables(&model, &vars));
    assert_eq!(
        params["q"],
        "station~=\"^(Kallio|Vallila\\.2)$\";pm25>5;x==\"$unknown\";y==\"${open\""
    );
    assert_eq!(
        with_variables(&model, &BTreeMap::new()),
        model,
        "no variables, no change"
    );
}
