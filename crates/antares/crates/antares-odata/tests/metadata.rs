// SPDX-License-Identifier: EUPL-1.2
//! The service and metadata documents of a view (odata-linkml-mapping.md
//! sections 3, 7 and S4): the Tenant's schema intersected with the view, no
//! endpoint without a schema, and nothing hidden named.

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const SCHEMA: &str = r#"{
  "id": "https://example.org/city", "name": "city",
  "prefixes": {"city": {"prefix_prefix": "city", "prefix_reference": "https://example.org/city/"}},
  "default_prefix": "city",
  "classes": {"Car": {"slots": ["speed", "secret", "owner"]}, "Person": {"slots": ["name"]}},
  "slots": {"speed": {"range": "float"}, "secret": {"range": "integer"},
            "owner": {"range": "Person"}, "name": {}}
}"#;

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<&str>,
    headers: &[(&str, &str)],
) -> (StatusCode, String) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(b) = body {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", b.len());
    }
    for (k, v) in headers {
        req = req.header(*k, *v);
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
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn view(id: &str, tenant: &str, attrs: &[&str]) -> String {
    json!({
        "id": id, "tenant": tenant,
        "@context": {"city": "https://example.org/city/"},
        "selector": ["city:Car"],
        "rules": [{"types": ["city:Car"], "attributes": attrs.iter().map(|a| format!("city:{a}")).collect::<Vec<_>>()}],
        "operations": ["query_entities", "retrieve_entity"]
    })
    .to_string()
}

async fn broker() -> Result<(TestBroker, Router), String> {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    let b = TestBroker::new().await?;
    let app = b.serve(antares_odata::OData)?;
    for (id, tenant) in [("cars", "city"), ("port-cars", "port")] {
        let (s, body) = send(
            &app,
            Method::PUT,
            &format!("/q/views/{id}"),
            Some(&view(id, tenant, &["speed", "owner"])),
            &[],
        )
        .await;
        if s != StatusCode::CREATED {
            return Err(format!("{id}: {s} {body}"));
        }
    }
    let (s, body) = send(&app, Method::PUT, "/q/schemas/city", Some(SCHEMA), &[]).await;
    if s != StatusCode::CREATED {
        return Err(format!("schema: {s} {body}"));
    }
    Ok((b, app))
}

#[tokio::test(flavor = "multi_thread")]
async fn metadata_never_names_a_hidden_type_or_attribute() {
    let (_b, app) = broker().await.expect("broker");
    let (s, xml) = send(
        &app,
        Method::GET,
        "/x/views/cars/odata/v4/$metadata",
        None,
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{xml}");
    assert!(
        xml.contains(r#"<EntityType Name="Car" BaseType="NgsiLd.Entity">"#),
        "{xml}"
    );
    assert!(
        xml.contains(r#"<Property Name="speed" Type="Edm.Double">"#),
        "{xml}"
    );
    let (s, json_doc) = send(
        &app,
        Method::GET,
        "/x/views/cars/odata/v4/$metadata?$format=json",
        None,
        &[("OData-MaxVersion", "4.01")],
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{json_doc}");
    let (s, service) = send(&app, Method::GET, "/x/views/cars/odata/v4/", None, &[]).await;
    assert_eq!(s, StatusCode::OK, "{service}");
    let service: Value = serde_json::from_str(&service).unwrap_or(Value::Null);
    assert_eq!(service["value"][0]["name"], "Car", "{service}");
    for doc in [&xml, &json_doc, &service.to_string()] {
        for hidden in ["secret", "Person", "city/name"] {
            assert!(!doc.contains(hidden), "{hidden} in {doc}");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn metadata_relationship_to_hidden_type_is_a_url_string() {
    let (_b, app) = broker().await.expect("broker");
    let (_, xml) = send(
        &app,
        Method::GET,
        "/x/views/cars/odata/v4/$metadata",
        None,
        &[],
    )
    .await;
    assert!(
        xml.contains(r#"<Property Name="owner" Type="Edm.String">"#),
        "R5: {xml}"
    );
    assert!(
        xml.contains(r#"<Annotation Term="Core.IsURL" Bool="true"/>"#),
        "{xml}"
    );
    assert!(
        !xml.contains("NavigationProperty"),
        "no navigation to a hidden type: {xml}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn metadata_xml_and_json_describe_one_model() {
    let (_b, app) = broker().await.expect("broker");
    let v401 = [("OData-MaxVersion", "4.01")];
    let (_, xml) = send(
        &app,
        Method::GET,
        "/x/views/cars/odata/v4/$metadata",
        None,
        &v401,
    )
    .await;
    let (_, doc) = send(
        &app,
        Method::GET,
        "/x/views/cars/odata/v4/$metadata?$format=json",
        None,
        &v401,
    )
    .await;
    let doc: Value = serde_json::from_str(&doc).unwrap_or(Value::Null);
    let mut members = 0;
    for (namespace, schema) in doc.as_object().into_iter().flatten() {
        for (name, element) in schema.as_object().into_iter().flatten() {
            if element["$Kind"] != "EntityType" {
                continue;
            }
            assert!(
                xml.contains(&format!(r#"<EntityType Name="{name}""#)),
                "{namespace}.{name}"
            );
            for (member, _) in element
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(k, _)| !k.starts_with('$') && !k.starts_with('@'))
            {
                members += 1;
                assert!(
                    xml.contains(&format!(r#"Name="{member}""#)),
                    "{name}/{member} in {xml}"
                );
            }
        }
    }
    let in_xml = xml.matches("<Property ").count() + xml.matches("<NavigationProperty ").count();
    assert_eq!(members, in_xml, "{xml}");
    assert!(members > 5, "the base and Car: {members}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_view_without_a_schema_or_an_unknown_view_has_no_endpoint() {
    let (_b, app) = broker().await.expect("broker");
    for uri in [
        "/x/views/port-cars/odata/v4/$metadata",
        "/x/views/nope/odata/v4/$metadata",
        "/x/views/nope/odata/v4",
        "/x/views/ca%72s/odata/v4/$metadata",
    ] {
        let (s, body) = send(&app, Method::GET, uri, None, &[]).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{uri}: {body}");
        let body: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        assert_eq!(body["error"]["code"], "NotFound", "{uri}: {body}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_document_is_written_in_the_version_the_client_reads() {
    let (_b, app) = broker().await.expect("broker");
    for (headers, version) in [
        (&[][..], "4.0"),
        (&[("OData-MaxVersion", "4.0")][..], "4.0"),
        (&[("OData-MaxVersion", "4.01")][..], "4.01"),
    ] {
        let (s, xml) = send(
            &app,
            Method::GET,
            "/x/views/cars/odata/v4/$metadata",
            None,
            headers,
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{xml}");
        assert!(
            xml.contains(&format!(r#"Version="{version}""#)),
            "{headers:?}: {xml}"
        );
    }
    let (s, _) = send(&app, Method::GET, "/x/views/cars/odata/v4", None, &[]).await;
    assert_eq!(s, StatusCode::OK, "the service root without its slash");
    let (s, body) = send(
        &app,
        Method::GET,
        "/x/views/cars/odata/v4/$metadata",
        None,
        &[("OData-MaxVersion", "5.0x")],
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
}

#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Lines {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Ok(mut v) = self.0.lock() {
            v.extend_from_slice(buf);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A schema the surface cannot read closes it for the Tenant's views, and
/// the operator learns why. The only test that reaches that log line: a
/// callsite another test hits first without a subscriber stays disabled.
#[tokio::test]
async fn an_unreadable_schema_is_logged_once_with_its_reason() {
    let lines = Lines::default();
    let writer = lines.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let (_b, app) = broker().await.expect("broker");
    let (s, _) = send(
        &app,
        Method::PUT,
        "/q/schemas/port",
        Some(r#"{"id": "x", "imports": []}"#),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    for _ in 0..2 {
        let (s, _) = send(
            &app,
            Method::GET,
            "/x/views/port-cars/odata/v4/$metadata",
            None,
            &[],
        )
        .await;
        assert_eq!(s, StatusCode::NOT_FOUND);
    }
    let log = String::from_utf8(lines.0.lock().map(|v| v.clone()).unwrap_or_default())
        .unwrap_or_default();
    assert_eq!(log.matches("cannot be read").count(), 1, "{log}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_view_of_every_name_describes_the_whole_schema() {
    let (_b, app) = broker().await.expect("broker");
    let all = json!({
        "id": "all", "tenant": "city", "selector": ["*"],
        "rules": [{"types": ["*"], "attributes": ["*"]}],
        "operations": ["query_entities", "retrieve_entity"]
    })
    .to_string();
    let (s, body) = send(&app, Method::PUT, "/q/views/all", Some(&all), &[]).await;
    assert_eq!(s, StatusCode::CREATED, "{body}");
    let (s, xml) = send(
        &app,
        Method::GET,
        "/x/views/all/odata/v4/$metadata",
        None,
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{xml}");
    for shown in [
        r#"<EntityType Name="Car""#,
        r#"<EntityType Name="Person""#,
        r#"<Property Name="secret""#,
        "NavigationProperty",
    ] {
        assert!(xml.contains(shown), "ADR-0023 rule 12, {shown}: {xml}");
    }
}
