// SPDX-License-Identifier: EUPL-1.2
//! The model cache (T-3441 7.3, surfaces-threat-model O16, O21): a view's
//! model is built once per key on a pod, rebuilt when what it is a function
//! of changes, and never served for another view. Every test here sets a
//! subscriber: the build event's callsite is shared by the binary.

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::json;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const SCHEMA: &str = r#"{
  "id": "https://example.org/city", "name": "city",
  "prefixes": {"city": {"prefix_prefix": "city", "prefix_reference": "https://example.org/city/"}},
  "default_prefix": "city",
  "classes": {"Car": {"slots": ["speed", "secret", "colour"]}},
  "slots": {"speed": {"range": "float"}, "secret": {"range": "integer"}, "colour": {}}
}"#;

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

impl Lines {
    fn builds(&self, view: &str) -> usize {
        let log = self
            .0
            .lock()
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .unwrap_or_default();
        log.lines()
            .filter(|l| {
                l.contains("OData model was built") && l.contains(&format!("view=\"{view}\""))
            })
            .count()
    }
}

struct Answer {
    status: StatusCode,
    etag: String,
    body: String,
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<&str>,
    headers: &[(&str, &str)],
) -> Answer {
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
    let etag = resp
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    Answer {
        status,
        etag,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

async fn put_view(app: &Router, id: &str, attrs: &[&str]) -> Result<(), String> {
    let view = json!({
        "id": id, "tenant": "city", "@context": {"city": "https://example.org/city/"},
        "selector": ["city:Car"],
        "rules": [{"types": ["city:Car"], "attributes": attrs.iter().map(|a| format!("city:{a}")).collect::<Vec<_>>()}],
        "operations": ["query_entities", "retrieve_entity", "retrieve_attribute_value"]
    })
    .to_string();
    let put = send(
        app,
        Method::PUT,
        &format!("/q/views/{id}"),
        Some(&view),
        &[],
    )
    .await;
    match put.status {
        StatusCode::CREATED | StatusCode::NO_CONTENT | StatusCode::OK => Ok(()),
        other => Err(format!("{id}: {other} {}", put.body)),
    }
}

async fn broker(views: &[(&str, &[&str])]) -> Result<(TestBroker, Router), String> {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    let b = TestBroker::new().await?;
    let app = b.serve(antares_odata::OData)?;
    for (id, attrs) in views {
        put_view(&app, id, attrs).await?;
    }
    let put = send(&app, Method::PUT, "/q/schemas/city", Some(SCHEMA), &[]).await;
    if put.status != StatusCode::CREATED {
        return Err(format!("schema: {} {}", put.status, put.body));
    }
    Ok((b, app))
}

fn subscriber(lines: &Lines) -> tracing::subscriber::DefaultGuard {
    let writer = lines.clone();
    tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::DEBUG)
            .finish(),
    )
}

#[tokio::test]
async fn odata_metadata_is_built_once_per_view_revision() {
    let lines = Lines::default();
    let _guard = subscriber(&lines);
    let (b, app) = broker(&[("built-once", &["speed"])]).await.expect("broker");
    let root = "/x/views/built-once/odata/v4";
    let first = send(&app, Method::GET, &format!("{root}/$metadata"), None, &[]).await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.body);
    for uri in [
        format!("{root}/$metadata"),
        format!("{root}/"),
        format!("{root}/Car"),
    ] {
        let again = send(&app, Method::GET, &uri, None, &[]).await;
        assert_eq!(again.status, StatusCode::OK, "{uri}: {}", again.body);
    }
    assert_eq!(
        lines.builds("built-once"),
        1,
        "one model for every request of one key"
    );
    let v401 = send(
        &app,
        Method::GET,
        &format!("{root}/$metadata"),
        None,
        &[("OData-MaxVersion", "4.01")],
    )
    .await;
    assert_eq!(v401.status, StatusCode::OK);
    assert_eq!(lines.builds("built-once"), 2, "4.0 and 4.01 models differ");

    put_view(&app, "built-once", &["speed", "colour"])
        .await
        .expect("view");
    b.reload_now().await.expect("reload");
    let changed = send(&app, Method::GET, &format!("{root}/$metadata"), None, &[]).await;
    assert!(
        changed.body.contains(r#"Name="colour""#),
        "{}",
        changed.body
    );
    assert_eq!(lines.builds("built-once"), 3, "a changed view is a new key");
    assert_ne!(changed.etag, first.etag, "the tag follows the document");
}

#[tokio::test]
async fn odata_metadata_is_cached_per_view_and_subject_and_never_served_across_them() {
    let lines = Lines::default();
    let _guard = subscriber(&lines);
    let (_b, app) = broker(&[("view-a", &["speed"]), ("view-b", &["secret"])])
        .await
        .expect("broker");
    for _ in 0..2 {
        for subject in ["alice", "bob"] {
            let headers = [("Authorization", subject)];
            let a = send(
                &app,
                Method::GET,
                "/x/views/view-a/odata/v4/$metadata",
                None,
                &headers,
            )
            .await;
            let b = send(
                &app,
                Method::GET,
                "/x/views/view-b/odata/v4/$metadata",
                None,
                &headers,
            )
            .await;
            assert!(
                a.body.contains("speed") && !a.body.contains("secret"),
                "{subject}: {}",
                a.body
            );
            assert!(
                b.body.contains("secret") && !b.body.contains("speed"),
                "{subject}: {}",
                b.body
            );
            assert_ne!(a.etag, b.etag);
        }
    }
    assert_eq!(
        (lines.builds("view-a"), lines.builds("view-b")),
        (1, 1),
        "one model each, cached apart"
    );
}

#[tokio::test]
async fn metadata_etag_changes_when_the_view_changes() {
    let lines = Lines::default();
    let _guard = subscriber(&lines);
    let (b, app) = broker(&[("etag-view", &["speed"])]).await.expect("broker");
    let uri = "/x/views/etag-view/odata/v4/$metadata";
    let before = send(&app, Method::GET, uri, None, &[]).await;
    let tag = before.etag.clone();
    let unchanged = send(
        &app,
        Method::GET,
        uri,
        None,
        &[("If-None-Match", tag.as_str())],
    )
    .await;
    assert_eq!(
        unchanged.status,
        StatusCode::NOT_MODIFIED,
        "{}",
        unchanged.body
    );
    put_view(&app, "etag-view", &["speed", "colour"])
        .await
        .expect("view");
    b.reload_now().await.expect("reload");
    let after = send(
        &app,
        Method::GET,
        uri,
        None,
        &[("If-None-Match", tag.as_str())],
    )
    .await;
    assert_eq!(
        after.status,
        StatusCode::OK,
        "the old tag no longer matches"
    );
    assert_ne!(after.etag, tag);
}

#[tokio::test]
async fn every_json_answer_names_the_metadata_etag() {
    let (_b, app) = broker(&[("tagged", &["speed"])]).await.expect("broker");
    let car = json!({"id": "urn:ngsi-ld:Car:1", "type": "https://example.org/city/Car",
        "https://example.org/city/speed": {"type": "Property", "value": 5}})
    .to_string();
    let seeded = send(
        &app,
        Method::POST,
        "/ngsi-ld/v1/entities",
        Some(&car),
        &[("NGSILD-Tenant", "city")],
    )
    .await;
    assert_eq!(seeded.status, StatusCode::CREATED, "{}", seeded.body);
    let root = "/x/views/tagged/odata/v4";
    let xml = send(&app, Method::GET, &format!("{root}/$metadata"), None, &[]).await;
    assert!(xml.etag.starts_with('"'), "{}", xml.etag);
    let json_doc = send(
        &app,
        Method::GET,
        &format!("{root}/$metadata?$format=json"),
        None,
        &[],
    )
    .await;
    for (path, member) in [
        ("/", "@odata.metadataEtag"),
        ("/Car", "@odata.metadataEtag"),
        ("/Car('urn:ngsi-ld:Car:1')", "@odata.metadataEtag"),
        ("/Car('urn:ngsi-ld:Car:1')/speed", "@odata.metadataEtag"),
        (
            "/Car?$top=1&$format=application/json;odata.metadata=full",
            "@odata.metadataEtag",
        ),
    ] {
        let answer = send(&app, Method::GET, &format!("{root}{path}"), None, &[]).await;
        assert_eq!(answer.status, StatusCode::OK, "{path}: {}", answer.body);
        let body: serde_json::Value = serde_json::from_str(&answer.body).expect("json");
        assert_eq!(
            body[member].as_str(),
            Some(xml.etag.as_str()),
            "{path}: the default metadata document's tag: {}",
            answer.body
        );
        let first = answer.body.find("\",").map_or(0, |at| at + 2);
        assert!(
            answer.body[first..].starts_with(&format!("\"{member}\"")),
            "{path}: right after the context URL: {}",
            answer.body
        );
    }
    let v401 = send(
        &app,
        Method::GET,
        &format!("{root}/Car"),
        None,
        &[("OData-MaxVersion", "4.01")],
    )
    .await;
    let xml401 = send(
        &app,
        Method::GET,
        &format!("{root}/$metadata"),
        None,
        &[("OData-MaxVersion", "4.01")],
    )
    .await;
    assert_ne!(xml401.etag, xml.etag, "a 4.01 document of its own");
    let v401: serde_json::Value = serde_json::from_str(&v401.body).expect("json");
    assert_eq!(
        v401["@metadataEtag"].as_str(),
        Some(xml401.etag.as_str()),
        "the tag of the document this version reads: {v401}"
    );
    let none = send(
        &app,
        Method::GET,
        &format!("{root}/Car?$format=application/json;odata.metadata=none"),
        None,
        &[],
    )
    .await;
    assert!(!none.body.contains("etadataEtag"), "{}", none.body);
    assert_ne!(
        json_doc.etag, xml.etag,
        "each representation its own strong tag"
    );
}
