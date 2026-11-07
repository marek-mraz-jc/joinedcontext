// SPDX-License-Identifier: EUPL-1.2
//! ADR-0023 rule 12: a view that selects every type and shows every
//! Attribute answers what the broker answers without a view, in the view's
//! Tenant (T-3666; the view T-3501 runs the ETSI suite through).

use crate::common::{fixture, put_view, send, Answer, READS, SUBSCRIPTIONS, WRITES};
use axum::http::{Method, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const ALL: &str = "/x/views/all/ngsi-ld/v1";

fn all_view() -> Value {
    json!({
        "id": "all", "tenant": "city", "selector": ["*"],
        "rules": [{"types": ["*"], "attributes": ["*"]}],
        "operations": READS.iter().chain(&WRITES).chain(&SUBSCRIPTIONS).collect::<Vec<_>>()
    })
}

/// `doc` without the id a discovery list mints on every answer
/// (`urn:ngsi-ld:EntityTypeList:<uuid>` and the like).
fn minted(mut doc: Value) -> Value {
    let list = doc
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|t| t.ends_with("List"));
    if list {
        if let Some(o) = doc.as_object_mut() {
            o.remove("id");
        }
    }
    doc
}

async fn direct(app: &Router, path: &str) -> Answer {
    send(
        app,
        Method::GET,
        &format!("/ngsi-ld/v1{path}"),
        &[("NGSILD-Tenant", "city")],
        None,
    )
    .await
}

async fn through(app: &Router, path: &str) -> Answer {
    send(app, Method::GET, &format!("{ALL}{path}"), &[], None).await
}

#[tokio::test(flavor = "multi_thread")]
async fn views_all_answers_what_the_broker_answers() -> Result<(), String> {
    let f = fixture().await?;
    put_view(&f.app, &all_view()).await?;
    for path in [
        "/entities?type=Car",
        "/entities?type=Car,Person,Organization&count=true",
        "/entities?type=Car&q=secret==1",
        "/entities?type=Car&attrs=secret",
        "/entities/urn:ngsi-ld:Car:3",
        "/entities/urn:ngsi-ld:Org:o",
        "/entities/urn:ngsi-ld:Car:1/attrs/secret",
        "/entities/urn:ngsi-ld:Car:1?join=inline&joinLevel=1",
        "/types",
        "/types?details=true",
        "/types/Car",
        "/attributes",
        "/attributes/secret",
        "/temporal/entities?type=Car&timerel=after&timeAt=1970-01-01T00:00:00Z",
        "/temporal/entities/urn:ngsi-ld:Car:3",
    ] {
        let (d, v) = (direct(&f.app, path).await, through(&f.app, path).await);
        assert_eq!(d.status, StatusCode::OK, "{path}: {}", d.text);
        assert_eq!(v.status, d.status, "{path}: {}", v.text);
        assert_eq!(minted(v.json.clone()), minted(d.json.clone()), "{path}");
        assert_eq!(
            v.header("NGSILD-Results-Count"),
            d.header("NGSILD-Results-Count"),
            "{path}"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn views_all_writes_and_watches_every_attribute() -> Result<(), String> {
    let f = fixture().await?;
    put_view(&f.app, &all_view()).await?;
    let patch = send(
        &f.app,
        Method::PATCH,
        &format!("{ALL}/entities/urn:ngsi-ld:Car:3/attrs"),
        &[],
        Some(&json!({"secret": {"type": "Property", "value": 7}})),
    )
    .await;
    assert_eq!(patch.status, StatusCode::NO_CONTENT, "{}", patch.text);
    let sub = send(
        &f.app,
        Method::POST,
        &format!("{ALL}/subscriptions"),
        &[],
        Some(&json!({
            "type": "Subscription", "entities": [{"type": "Car"}],
            "watchedAttributes": ["secret"],
            "notification": {"endpoint": {"uri": "http://localhost:1/n"}}
        })),
    )
    .await;
    assert_eq!(sub.status, StatusCode::CREATED, "{}", sub.text);
    Ok(())
}

/// A Relationship to an Entity that does not exist, written through `all`,
/// is written as at the root (T-3786, ETSI `454_01`): a view that selects
/// every Entity hides none, so the absent target tells nothing. A view of
/// every type narrowed by a `q` still answers absent as hidden (D7).
#[tokio::test(flavor = "multi_thread")]
async fn views_all_writes_a_relationship_to_an_absent_entity_as_the_root_does() -> Result<(), String>
{
    let f = fixture().await?;
    put_view(&f.app, &all_view()).await?;
    let absent = "urn:ngsi-ld:Person:nobody";
    let car3 = "/entities/urn:ngsi-ld:Car:3";
    let list = json!({"type": "ListRelationship", "objectList": [absent]});
    for (method, path, body, want) in [
        (
            Method::POST,
            "/entities".to_owned(),
            json!({"id": "urn:ngsi-ld:Car:z1", "type": "Car", "r": list}),
            StatusCode::CREATED,
        ),
        (
            Method::PATCH,
            format!("{car3}/attrs"),
            json!({"owner": {"type": "Relationship", "object": absent}}),
            StatusCode::NO_CONTENT,
        ),
        (
            Method::POST,
            format!("{car3}/attrs"),
            json!({"driver": {"type": "Relationship", "object": absent}}),
            StatusCode::NO_CONTENT,
        ),
        (
            Method::PATCH,
            car3.to_owned(),
            json!({"owner": {"type": "Relationship", "object": absent}}),
            StatusCode::NO_CONTENT,
        ),
    ] {
        let a = send(
            &f.app,
            method.clone(),
            &format!("{ALL}{path}"),
            &[],
            Some(&body),
        )
        .await;
        assert_eq!(a.status, want, "{method} {path}: {}", a.text);
    }
    let made = direct(&f.app, "/entities/urn:ngsi-ld:Car:z1").await;
    assert_eq!(
        made.json.pointer("/r/objectList"),
        Some(&json!([{"object": absent}])),
        "{}",
        made.text
    );

    let mut narrowed = all_view();
    if let Some(o) = narrowed.as_object_mut() {
        o.insert("id".into(), json!("all-slow"));
        o.insert("q".into(), json!("speed<80"));
    }
    put_view(&f.app, &narrowed).await?;
    let a = send(
        &f.app,
        Method::POST,
        "/x/views/all-slow/ngsi-ld/v1/entities",
        &[],
        Some(&json!({"id": "urn:ngsi-ld:Car:z2", "type": "Car", "speed": {"type": "Property", "value": 1}, "r": list})),
    )
    .await;
    assert_eq!(a.status, StatusCode::FORBIDDEN, "{}", a.text);
    assert_eq!(
        a.json.pointer("/detail"),
        Some(&json!(
            "a Relationship written through this view must point at an Entity it shows"
        ))
    );
    Ok(())
}

/// A Context Source on loopback that answers every request `201` and
/// records its request line.
async fn context_source() -> Result<(String, Arc<Mutex<Vec<String>>>), String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    antares_ops::test_kit::allow_private_egress();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| e.to_string())?;
    let url = format!(
        "http://{}",
        listener.local_addr().map_err(|e| e.to_string())?
    );
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let log = Arc::clone(&log);
            tokio::spawn(async move {
                let mut buf = vec![0_u8; 64 * 1024];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(buf.get(..n).unwrap_or_default());
                if let Some(line) = head.lines().next() {
                    log.lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(line.to_owned());
                }
                let _ = sock
                    .write_all(
                        b"HTTP/1.1 201 Created\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
            });
        }
    });
    Ok((url, seen))
}

/// What the Context Source received whose request line starts with `start`,
/// after giving a forward made in the background `wait` to arrive.
/// 10 under a sanitizer (`ANTARES_TEST_SANITIZER`, set by the strict workflow), 1 otherwise: the
/// stretch `antares_jsonld::slow_factor` gives every outbound deadline. A `ThreadSanitizer` run held the
/// root's forwarded Subscription past a flat 5 s (strict 39095897849, T-4564).
fn slow_factor() -> u64 {
    if std::env::var_os("ANTARES_TEST_SANITIZER").is_some() {
        10
    } else {
        1
    }
}

async fn received(seen: &Mutex<Vec<String>>, start: &str, wait: Duration) -> usize {
    let since = tokio::time::Instant::now();
    loop {
        let n = seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|l| l.starts_with(start))
            .count();
        if n > 0 || since.elapsed() >= wait {
            return n;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A write and a Subscription through `all` stay local (ADR-0023 rules 7
/// and 9, T-3945): the root forwards both to a matching Context Source, the
/// view sends it nothing, since the source would hold what the view never
/// checked. The ETSI `DistributedOperations` TPs are owed this refusal.
#[tokio::test(flavor = "multi_thread")]
async fn views_all_keeps_writes_and_subscriptions_off_context_sources() -> Result<(), String> {
    let f = fixture().await?;
    put_view(&f.app, &all_view()).await?;
    let (endpoint, seen) = context_source().await?;
    let reg = send(
        &f.app,
        Method::POST,
        "/ngsi-ld/v1/csourceRegistrations",
        &[("NGSILD-Tenant", "city")],
        Some(&json!({
            "id": "urn:ngsi-ld:ContextSourceRegistration:v1", "type": "ContextSourceRegistration",
            "information": [{"entities": [{"type": "Vehicle"}]}],
            "operations": ["redirectionOps", "federationOps"], "endpoint": endpoint
        })),
    )
    .await;
    assert_eq!(reg.status, StatusCode::CREATED, "{}", reg.text);
    let vehicle =
        |id: &str| json!({"id": id, "type": "Vehicle", "speed": {"type": "Property", "value": 1}});
    let subscription = json!({
        "type": "Subscription", "entities": [{"type": "Vehicle"}],
        "notification": {"endpoint": {"uri": "http://localhost:1/n"}}
    });

    let at_view = send(
        &f.app,
        Method::POST,
        &format!("{ALL}/entities"),
        &[],
        Some(&vehicle("urn:ngsi-ld:Vehicle:v")),
    )
    .await;
    assert_eq!(at_view.status, StatusCode::CREATED, "{}", at_view.text);
    let sub = send(
        &f.app,
        Method::POST,
        &format!("{ALL}/subscriptions"),
        &[],
        Some(&subscription),
    )
    .await;
    assert_eq!(sub.status, StatusCode::CREATED, "{}", sub.text);
    let short = Duration::from_millis(500);
    assert_eq!(
        received(&seen, "POST /ngsi-ld/v1/entities", short).await,
        0,
        "a write through the view stays local"
    );
    assert_eq!(
        received(&seen, "POST /ngsi-ld/v1/subscriptions", short).await,
        0,
        "a Subscription through the view stays local"
    );
    let stored = direct(&f.app, "/entities/urn:ngsi-ld:Vehicle:v?local=true").await;
    assert_eq!(
        stored.status,
        StatusCode::OK,
        "the local store holds it: {}",
        stored.text
    );

    // the root, the same requests: both reach the Context Source
    let city = [("NGSILD-Tenant", "city")];
    let at_root = send(
        &f.app,
        Method::POST,
        "/ngsi-ld/v1/entities",
        &city,
        Some(&vehicle("urn:ngsi-ld:Vehicle:r")),
    )
    .await;
    assert!(at_root.status.is_success(), "{}", at_root.text);
    let sub = send(
        &f.app,
        Method::POST,
        "/ngsi-ld/v1/subscriptions",
        &city,
        Some(&subscription),
    )
    .await;
    assert_eq!(sub.status, StatusCode::CREATED, "{}", sub.text);
    let long = Duration::from_secs(5 * slow_factor());
    assert!(
        received(&seen, "POST /ngsi-ld/v1/entities", long).await > 0,
        "the root forwards the write: {seen:?}"
    );
    assert!(
        received(&seen, "POST /ngsi-ld/v1/subscriptions", long).await > 0,
        "the root forwards the Subscription: {seen:?}"
    );
    Ok(())
}

/// List @contexts through `all` lists the Tenant's stored @contexts as the
/// root does, every `kind` and `details` alike; a narrowing view still lists
/// the core @context alone (`views_read`, ADR-0023 rule 5) (T-3946).
#[tokio::test(flavor = "multi_thread")]
async fn views_all_lists_the_stored_contexts_as_the_root_does() -> Result<(), String> {
    let f = fixture().await?;
    put_view(&f.app, &all_view()).await?;
    let ctx = json!({"@context": {"secret": "https://example.org/secret"}});
    let stored = send(
        &f.app,
        Method::POST,
        "/ngsi-ld/v1/jsonldContexts",
        &[("NGSILD-Tenant", "city")],
        Some(&ctx),
    )
    .await;
    assert_eq!(stored.status, StatusCode::CREATED, "{}", stored.text);
    for query in [
        "",
        "?kind=Hosted",
        "?details=false",
        "?details=true",
        "?details=true&kind=Hosted",
        "?kind=Cached",
    ] {
        let path = format!("/jsonldContexts{query}");
        let (d, v) = (direct(&f.app, &path).await, through(&f.app, &path).await);
        assert_eq!(d.status, StatusCode::OK, "{path}: {}", d.text);
        assert_eq!(v.status, d.status, "{path}: {}", v.text);
        assert_eq!(v.json, d.json, "{path}");
    }
    let hosted = through(&f.app, "/jsonldContexts?kind=Hosted").await;
    assert_eq!(
        hosted.json.as_array().map(Vec::len),
        Some(1),
        "the stored @context: {}",
        hosted.text
    );
    Ok(())
}

/// A write whose Entity id segment is empty is a malformed request through
/// `all` as at the root (T-3948, ETSI `010_02_01`, `011_02_01`, `012_02_01`,
/// `013_02_01`): 400 `BadRequestData`, not the 404 of a path no operation binds.
#[tokio::test(flavor = "multi_thread")]
async fn views_all_answers_an_empty_entity_id_as_the_root_does() -> Result<(), String> {
    let f = fixture().await?;
    put_view(&f.app, &all_view()).await?;
    let attr = json!({"speed": {"type": "Property", "value": 1}});
    let value = json!({"type": "Property", "value": 1});
    for (method, path, body) in [
        (Method::POST, "/entities//attrs/", Some(&attr)),
        (Method::PATCH, "/entities//attrs/", Some(&attr)),
        (Method::PATCH, "/entities//attrs/speed", Some(&value)),
        (
            Method::DELETE,
            "/entities//attrs/speed?deleteAll=false",
            None,
        ),
    ] {
        let d = send(
            &f.app,
            method.clone(),
            &format!("/ngsi-ld/v1{path}"),
            &[("NGSILD-Tenant", "city")],
            body,
        )
        .await;
        let v = send(&f.app, method.clone(), &format!("{ALL}{path}"), &[], body).await;
        assert_eq!(
            d.status,
            StatusCode::BAD_REQUEST,
            "{method} {path}: {}",
            d.text
        );
        assert_eq!(v.status, d.status, "{method} {path}: {}", v.text);
        assert_eq!(
            v.json.pointer("/type"),
            d.json.pointer("/type"),
            "{method} {path}"
        );
    }
    Ok(())
}
