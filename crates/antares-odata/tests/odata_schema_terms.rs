// SPDX-License-Identifier: EUPL-1.2
//! What the LinkML schema says beyond ranges (odata-linkml-mapping 3.4 and
//! row C10a, T-3722): an `edm_type` narrows a member's Edm type and a write
//! is checked against it; `description` is `Core.Description`;
//! `odata_computed` and `odata_immutable` are `Core.Computed` and
//! `Core.Immutable` and keep a body from writing the member; a class's
//! `odata_insertable`, `odata_updatable`, `odata_deletable` narrow what the
//! view offers its set, and a write the set refuses is 405.

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const STORE: &str = "https://example.org/store/";
const ROOT: &str = "/x/views/store/odata/v4";
const ITEM: &str = "Item('urn:ngsi-ld:Item:1')";
const LOG: &str = "Log('urn:ngsi-ld:Log:1')";

const SCHEMA: &str = r#"{
  "id": "https://example.org/store", "name": "store",
  "prefixes": {"store": {"prefix_prefix": "store", "prefix_reference": "https://example.org/store/"}},
  "default_prefix": "store",
  "classes": {
    "Item": {"description": "A stocked item", "slots": ["name", "qty", "weight", "code", "took", "blob", "made", "serial"],
             "annotations": {"odata_deletable": {"tag": "odata_deletable", "value": "false"}}},
    "Log": {"slots": ["name"],
            "annotations": {"odata_insertable": {"tag": "odata_insertable", "value": "false"},
                            "odata_updatable": {"tag": "odata_updatable", "value": "false"},
                            "odata_deletable": {"tag": "odata_deletable", "value": "true"}}}
  },
  "slots": {
    "name": {"description": "What it is called"},
    "qty": {"range": "integer", "annotations": {"edm_type": {"tag": "edm_type", "value": "Edm.Int16"}}},
    "weight": {"range": "float", "annotations": {"edm_type": {"tag": "edm_type", "value": "Edm.Single"}}},
    "code": {"annotations": {"edm_type": {"tag": "edm_type", "value": "Edm.Guid"}}},
    "took": {"annotations": {"edm_type": {"tag": "edm_type", "value": "Edm.Duration"}}},
    "blob": {"annotations": {"edm_type": {"tag": "edm_type", "value": "Edm.Binary"}}},
    "made": {"range": "datetime", "annotations": {"odata_computed": {"tag": "odata_computed", "value": "true"}}},
    "serial": {"annotations": {"odata_immutable": {"tag": "odata_immutable", "value": "true"}}}
  }
}"#;

async fn send(app: &Router, method: Method, uri: &str, body: Option<&str>) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("NGSILD-Tenant", "store")
        // CSDL JSON is a 4.01 document.
        .header("OData-MaxVersion", "4.01");
    if let Some(b) = body {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", b.len());
    }
    let Ok(resp) = app
        .clone()
        .oneshot(
            req.body(Body::from(body.unwrap_or_default().to_owned()))
                .unwrap_or_default(),
        )
        .await;
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

async fn odata(
    app: &Router,
    method: Method,
    path: &str,
    body: Option<&str>,
) -> (StatusCode, Value) {
    send(app, method, &format!("{ROOT}/{path}"), body).await
}

/// Tenant `store` with Item 1 and Log 1, and view `store` offering every
/// entity write.
async fn store() -> Result<(TestBroker, Router), String> {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    let b = TestBroker::new().await?;
    let app = b.serve(antares_odata::OData)?;
    let attrs = [
        "name", "qty", "weight", "code", "took", "blob", "made", "serial",
    ]
    .map(|a| format!("store:{a}"));
    let view = json!({
        "id": "store", "tenant": "store", "@context": {"store": STORE},
        "selector": ["store:Item", "store:Log"],
        "rules": [{"types": ["store:Item"], "attributes": attrs},
                  {"types": ["store:Log"], "attributes": ["store:name"]}],
        "operations": ["query_entities", "retrieve_entity", "create_entity", "merge_entity",
                       "replace_entity", "delete_entity", "replace_attribute",
                       "partial_attribute_update", "delete_attribute"]
    });
    for (uri, body) in [
        ("/q/views/store", view.to_string()),
        ("/q/schemas/store", SCHEMA.to_owned()),
    ] {
        let (status, answer) = send(&app, Method::PUT, uri, Some(&body)).await;
        if status != StatusCode::CREATED {
            return Err(format!("{uri}: {status} {answer}"));
        }
    }
    let value = |v: Value| json!({"type": "Property", "value": v});
    for entity in [
        json!({"id": "urn:ngsi-ld:Item:1", "type": format!("{STORE}Item"),
               format!("{STORE}name"): value(json!("bolt")), format!("{STORE}qty"): value(json!(7)),
               format!("{STORE}serial"): value(json!("s-1"))}),
        json!({"id": "urn:ngsi-ld:Log:1", "type": format!("{STORE}Log"),
               format!("{STORE}name"): value(json!("first"))}),
    ] {
        let (status, answer) = send(
            &app,
            Method::POST,
            "/ngsi-ld/v1/entities",
            Some(&entity.to_string()),
        )
        .await;
        if status != StatusCode::CREATED {
            return Err(format!("{:?}: {status} {answer}", entity.get("id")));
        }
    }
    Ok((b, app))
}

fn restriction<'v>(set: &'v Value, term: &str) -> &'v Value {
    &set[format!("@Capabilities.{term}")]
}

#[tokio::test(flavor = "multi_thread")]
async fn metadata_carries_the_schema_types_and_terms() {
    let (_b, app) = store().await.expect("broker");
    let (status, doc) = odata(&app, Method::GET, "$metadata?$format=json", None).await;
    assert_eq!(status, StatusCode::OK, "{doc}");
    let ns = &doc["store"];
    let item = &ns["Item"];
    for (member, edm) in [
        ("qty", "Edm.Int16"),
        ("weight", "Edm.Single"),
        ("code", "Edm.Guid"),
        ("took", "Edm.Duration"),
        ("blob", "Edm.Binary"),
        ("made", "Edm.DateTimeOffset"),
    ] {
        assert_eq!(item[member]["$Type"], edm, "{member}: {item}");
    }
    assert_eq!(item["@Core.Description"], "A stocked item", "{item}");
    assert_eq!(item["name"]["@Core.Description"], "What it is called");
    assert_eq!(item["made"]["@Core.Computed"], true, "{item}");
    assert_eq!(item["serial"]["@Core.Immutable"], true, "{item}");
    assert_eq!(item["name"]["@Core.Computed"], Value::Null);

    let container = &ns["Container"];
    let items = &container["Item"];
    assert_eq!(restriction(items, "InsertRestrictions")["Insertable"], true);
    assert_eq!(restriction(items, "UpdateRestrictions")["Updatable"], true);
    assert_eq!(
        restriction(items, "DeleteRestrictions")["Deletable"],
        false,
        "{items}"
    );
    let non = |term: &str, list: &str| -> Vec<String> {
        restriction(items, term)[list]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p.as_str().map(str::to_owned))
            .collect()
    };
    let inserts = non("InsertRestrictions", "NonInsertableProperties");
    let updates = non("UpdateRestrictions", "NonUpdatableProperties");
    assert!(
        inserts.contains(&"made".to_owned()) && !inserts.contains(&"serial".to_owned()),
        "{inserts:?}"
    );
    assert!(
        updates.contains(&"made".to_owned()) && updates.contains(&"serial".to_owned()),
        "{updates:?}"
    );
    let logs = &container["Log"];
    assert_eq!(
        restriction(logs, "InsertRestrictions")["Insertable"],
        false,
        "{logs}"
    );
    assert_eq!(
        restriction(logs, "UpdateRestrictions")["Updatable"],
        false,
        "{logs}"
    );
    assert_eq!(
        restriction(logs, "DeleteRestrictions")["Deletable"],
        true,
        "{logs}"
    );

    let (status, xml) = odata(&app, Method::GET, "$metadata", None).await;
    assert_eq!(status, StatusCode::OK, "{xml}");
    assert!(
        xml.as_str()
            .is_some_and(|x| x.contains(r#"Type="Edm.Int16""#)),
        "{xml}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_write_is_checked_against_the_narrower_type_and_the_terms() {
    let (_b, app) = store().await.expect("broker");
    let create = |extra: Value| {
        let mut body = json!({"id": "urn:ngsi-ld:Item:2", "name": "nut"});
        if let (Some(b), Value::Object(e)) = (body.as_object_mut(), extra) {
            b.extend(e);
        }
        body.to_string()
    };
    for (what, extra) in [
        ("an Int16 past its range", json!({"qty": 40000})),
        ("a GUID that is no string", json!({"code": 5})),
        ("a computed member", json!({"made": "2026-01-01T00:00:00Z"})),
    ] {
        let (status, answer) = odata(&app, Method::POST, "Item", Some(&create(extra))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{what}: {answer}");
    }
    let body = create(json!({"qty": -32768, "weight": 1.5, "serial": "s-2", "took": "PT1H"}));
    let (status, answer) = odata(&app, Method::POST, "Item", Some(&body)).await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");
    assert_eq!(answer["qty"], -32768, "{answer}");

    let (status, answer) = odata(&app, Method::PATCH, ITEM, Some(r#"{"serial": "s-9"}"#)).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an immutable member: {answer}"
    );
    let (status, answer) = odata(&app, Method::PATCH, ITEM, Some(r#"{"qty": 8}"#)).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{answer}");
    let (status, answer) = odata(&app, Method::GET, "Item?$filter=qty%20gt%207", None).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(
        answer["value"].as_array().map(Vec::len),
        Some(1),
        "{answer}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_write_the_class_refuses_is_405_before_its_body_is_read() {
    let (_b, app) = store().await.expect("broker");
    for (method, path, body) in [
        (Method::DELETE, ITEM.to_owned(), None),
        (Method::POST, "Log".to_owned(), Some("not json")),
        (Method::PATCH, LOG.to_owned(), Some(r#"{"name": "x"}"#)),
        (Method::PUT, LOG.to_owned(), Some(r#"{"name": "x"}"#)),
        (
            Method::PUT,
            format!("{LOG}/name"),
            Some(r#"{"value": "x"}"#),
        ),
        (Method::DELETE, format!("{LOG}/name"), None),
    ] {
        let (status, answer) = odata(&app, method.clone(), &path, body).await;
        assert_eq!(
            status,
            StatusCode::METHOD_NOT_ALLOWED,
            "{method} {path}: {answer}"
        );
    }
    let (status, item) = odata(&app, Method::GET, ITEM, None).await;
    assert_eq!(status, StatusCode::OK, "nothing was deleted: {item}");
    let (status, answer) = odata(&app, Method::DELETE, LOG, None).await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "the view and the class allow it: {answer}"
    );
}
