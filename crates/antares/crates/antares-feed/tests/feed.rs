// SPDX-License-Identifier: EUPL-1.2
//! `antares-feed` against a broker (T-5202, EP-150, EP-151): the view narrows what a feed
//! lists, the Policy decides each read, an output's filter only narrows, the three formats
//! carry one item list, text stays text, and paging and conditional GETs hold.
//!
//! The Tenant `city` holds the Events `e1` (culture), `e2` (sport, a `javascript:` link) and
//! `e3` (no name: skipped), the Article `a1` with a hidden `owner`, and the hidden `Secret` `s`.
//! The view `city` lists the outputs `vsetky` (Event and Article), `sport` (filtered) and
//! `tajne` (the hidden type); `plain` lists none; `writes` allows Create Entity alone; `void`
//! reads the empty Tenant `void`. With `FEED_FIXTURE_DIR` set, the first test writes the three
//! documents of `vsetky` there for the T-5201 validator (`tests/feed/README.md`).

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use antares_ops::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ops::test_kit::TestBroker;
use antares_ops::Surface as _;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tower::ServiceExt;

const ALL: &str = "/x/views/city/feed/v1/vsetky";
const FILES: [&str; 3] = ["atom.xml", "rss.xml", "feed.json"];

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    body: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, String) {
    let mut req = Request::builder().method(method).uri(uri);
    if uri.starts_with("/ngsi-ld") {
        req = req.header("NGSILD-Tenant", "city");
    }
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    if !body.is_empty() {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len());
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(body.to_owned())).expect("request"))
        .await
        .expect("infallible");
    let (status, headers) = (resp.status(), resp.headers().clone());
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        headers,
        String::from_utf8(bytes.to_vec()).expect("utf-8"),
    )
}

async fn send(app: &Router, method: &str, uri: &str, body: &str) -> (StatusCode, String) {
    let (status, _, body) = call(app, method, uri, body, &[]).await;
    (status, body)
}

/// Denies every Query Entities (clause 5.7) once locked, as a Policy granting no read would.
#[derive(Default)]
struct NoReads(AtomicBool);

impl PolicyEngine for NoReads {
    fn name(&self) -> &'static str {
        "no-reads"
    }

    fn decide<'a>(&'a self, _: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let locked = self.0.load(Ordering::SeqCst);
        Box::pin(std::future::ready(
            if locked && op.clause.starts_with("5.7.") {
                Decision::Deny("no read".into())
            } else {
                Decision::Allow
            },
        ))
    }

    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

fn outputs() -> Value {
    json!([
        {"name": "vsetky", "version": 1, "title": {"sk": "Všetko", "en": "Everything"},
         "description": {"sk": "Podujatia a správy"}, "lang": "sk",
         "sources": [
            {"type": "Event", "preset": "event", "mapping": {"updated": "dateModified"}},
            {"type": "Article", "preset": "article", "mapping": {"author": "owner"}}]},
        {"name": "sport", "title": {"sk": "Šport"},
         "sources": [{"type": "Event", "preset": "event", "mapping": {"updated": "dateModified"}}],
         "filter": {"q": "category==\"šport\""}},
        {"name": "tajne", "sources": [{"type": "Secret", "preset": "generic"}]},
        {"name": "blizko", "sources": [{"type": "Event", "preset": "event"}],
         "filter": {"georel": "near;maxDistance==1000", "geometry": "Point", "coordinates": "[19.14,48.73]"}}
    ])
}

async fn fixture_with(broker: TestBroker) -> Router {
    let app = broker.serve(antares_feed::Feed).expect("mounted");
    let reads = json!(["query_entities"]);
    let rules = json!([
        {"types": ["Event"], "attributes": ["name", "description", "category", "dateModified", "url", "organizer", "image"]},
        {"types": ["Article"], "attributes": ["headline", "dateModified", "url", "articleBody"]}]);
    let views = [
        json!({"id": "city", "tenant": "city", "selector": ["Event", "Article"], "operations": reads,
               "rules": rules, "representations": {"feed": outputs()}}),
        json!({"id": "plain", "tenant": "city", "selector": ["Event", "Article"], "operations": reads,
               "rules": rules}),
        json!({"id": "writes", "tenant": "city", "selector": ["Event"], "operations": ["create_entity"],
               "rules": [{"types": ["Event"], "attributes": ["name"]}], "representations": {"feed": outputs()}}),
        json!({"id": "every", "tenant": "city", "selector": ["*"], "operations": reads,
               "rules": [{"types": ["*"], "attributes": ["*"]}], "representations": {"feed": outputs()}}),
        json!({"id": "void", "tenant": "void", "selector": ["Event", "Article"], "operations": reads,
               "rules": rules, "representations": {"feed": outputs()}}),
    ];
    for view in views {
        let id = view["id"].as_str().expect("id");
        let (status, body) = send(&app, "PUT", &format!("/q/views/{id}"), &view.to_string()).await;
        assert_eq!(status, StatusCode::CREATED, "view {id}: {body}");
    }
    let p = |v: Value| json!({"type": "Property", "value": v});
    let entities = [
        json!({"id": "urn:ngsi-ld:Event:e1", "type": "Event", "name": p(json!("Koncert & <b>]]>\u{1} \"x\"")),
               "description": p(json!("<script>alert(1)</script>")), "category": p(json!("kultúra")), "organizer": p(json!("Divadlo")),
               "dateModified": p(json!("2026-10-02T10:00:00Z")), "url": p(json!("https://bbsk.sk/koncert?a=1&b=2"))}),
        json!({"id": "urn:ngsi-ld:Event:e2", "type": "Event", "name": p(json!("Futbal")), "category": p(json!("šport")),
               "dateModified": p(json!("2026-10-03T10:00:00Z")), "url": p(json!("javascript:alert(1)")),
               "image": p(json!("https://bbsk.sk/futbal.jpg"))}),
        json!({"id": "urn:ngsi-ld:Event:e3", "type": "Event", "category": p(json!("šport")),
               "dateModified": p(json!("2026-10-04T10:00:00Z"))}),
        json!({"id": "urn:ngsi-ld:Article:a1", "type": "Article", "headline": p(json!("Správa")),
               "dateModified": p(json!("2026-10-01")), "articleBody": p(json!("Celý text")), "owner": p(json!("hidden-owner"))}),
        json!({"id": "urn:ngsi-ld:Secret:s", "type": "Secret", "name": p(json!("hidden-secret"))}),
    ];
    for entity in entities {
        let (status, body) = send(&app, "POST", "/ngsi-ld/v1/entities", &entity.to_string()).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    app
}

async fn fixture() -> Router {
    fixture_with(TestBroker::new().await.expect("broker")).await
}

/// The item ids of a document in their order: Atom `<id>` of entries, RSS `<guid>`, JSON `id`.
fn ids(file: &str, body: &str) -> Vec<String> {
    if file == "feed.json" {
        let feed: Value = serde_json::from_str(body).expect("json");
        return feed["items"]
            .as_array()
            .expect("items")
            .iter()
            .map(|i| i["id"].as_str().expect("id").to_owned())
            .collect();
    }
    let open = if file == "atom.xml" {
        "<entry><id>"
    } else {
        "<guid isPermaLink=\"false\">"
    };
    body.split(open)
        .skip(1)
        .map(|s| s.split('<').next().expect("id").to_owned())
        .collect()
}

#[tokio::test]
async fn three_formats_list_the_same_entries_newest_first_without_what_the_view_hides() {
    let app = fixture().await;
    let want = ["Event:e2", "Event:e1", "Article:a1"]
        .map(|id| format!("tag:localhost,2026:city/urn:ngsi-ld:{id}"));
    let media = [
        "application/atom+xml; charset=utf-8",
        "application/rss+xml; charset=utf-8",
        "application/feed+json; charset=utf-8",
    ];
    for (file, media) in FILES.into_iter().zip(media) {
        let (status, headers, body) = call(&app, "GET", &format!("{ALL}/{file}"), "", &[]).await;
        assert_eq!(status, StatusCode::OK, "{file}: {body}");
        assert_eq!(headers["content-type"], media);
        assert_eq!(headers["antares-skipped-records"], "1", "e3 has no title");
        assert_eq!(headers["cache-control"], "private, max-age=300");
        assert_eq!(headers["last-modified"], "Sat, 03 Oct 2026 10:00:00 GMT");
        assert_eq!(ids(file, &body), want, "{file}: {body}");
        // JSON Feed carries content_text alone, so its text may hold markup as text
        let markup: &[&str] = if file == "feed.json" {
            &[]
        } else {
            &["<script>", "<b>"]
        };
        let hidden = [
            "hidden-owner",
            "hidden-secret",
            "javascript:",
            "\u{1}",
            "\\u0001",
        ];
        for hidden in hidden.iter().chain(markup) {
            assert!(!body.contains(hidden), "{file} leaks {hidden:?}: {body}");
        }
        assert!(body.contains("Všetko") && body.contains("Futbal"), "{body}");
        if let Ok(dir) = std::env::var("FEED_FIXTURE_DIR") {
            std::fs::write(format!("{dir}/vsetky.{file}"), &body).expect("fixture written");
        }
    }
    let (_, atom) = send(&app, "GET", &format!("{ALL}/atom.xml"), "").await;
    for escaped in ["Koncert &amp; &lt;b&gt;]]&gt; &quot;x&quot;", "xml:lang=\"sk\"",
        "href=\"https://bbsk.sk/koncert?a=1&amp;b=2\"", "<updated>2026-10-03T10:00:00Z</updated>",
        "<link rel=\"self\" type=\"application/atom+xml\" href=\"http://localhost:9090/x/views/city/feed/v1/vsetky/atom.xml\"/>"] {
        assert!(atom.contains(escaped), "{escaped}: {atom}");
    }
    let (_, rss) = send(&app, "GET", &format!("{ALL}/rss.xml"), "").await;
    assert!(
        rss.contains("&amp;lt;script&amp;gt;"),
        "RSS description is HTML-escaped twice: {rss}"
    );
    let (_, json) = send(&app, "GET", &format!("{ALL}/feed.json?lang=en"), "").await;
    let feed: Value = serde_json::from_str(&json).expect("json");
    assert_eq!(
        (feed["title"].as_str(), feed["language"].as_str()),
        (Some("Everything"), Some("en"))
    );
    let (status, body) = send(&app, "HEAD", &format!("{ALL}/atom.xml"), "").await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, ""));
    let info = antares_feed::Feed.version_info();
    assert_eq!(
        info["endpoint"],
        "/x/views/{view}/feed/v1/{name}/{atom.xml|rss.xml|feed.json}"
    );
}

#[tokio::test]
async fn only_a_listed_output_of_a_readable_view_has_a_feed() {
    let app = fixture().await;
    for uri in [
        "/x/views/city/feed/v1/nic/atom.xml",
        "/x/views/city/feed/v1/vsetky/feed.xml",
        "/x/views/plain/feed/v1/vsetky/atom.xml",
        "/x/views/writes/feed/v1/vsetky/atom.xml",
        "/x/views/nowhere/feed/v1/vsetky/atom.xml",
        "/x/views/city/feed/v1/tajne/rss.xml",
    ] {
        let (status, body) = send(&app, "GET", uri, "").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
        assert!(!body.contains("hidden-secret"), "{body}");
    }
}

#[tokio::test]
async fn a_filter_only_narrows_and_a_request_cannot_widen_it() {
    let app = fixture().await;
    let sport = "/x/views/city/feed/v1/sport/feed.json";
    let (status, body) = send(&app, "GET", sport, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        ids("feed.json", &body),
        ["tag:localhost,2026:city/urn:ngsi-ld:Event:e2"]
    );
    let widen = format!("{sport}?q=name==%22x%22)%7C(category==%22kult%C3%BAra%22");
    let (status, body) = send(&app, "GET", &widen, "").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, body) = send(&app, "GET", &format!("{sport}?q=name==%22Koncert%22"), "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        ids("feed.json", &body).is_empty(),
        "the filter still holds: {body}"
    );
    let every = "/x/views/every/feed/v1/vsetky/feed.json";
    let (status, body) = send(&app, "GET", every, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let all = ["Event:e2", "Event:e1", "Article:a1"]
        .map(|id| format!("tag:localhost,2026:every/urn:ngsi-ld:{id}"));
    assert_eq!(
        ids("feed.json", &body),
        all,
        "a view of every type lists only the output's types"
    );
    let (_, body) = send(&app, "GET", &format!("{every}?attrs=name,dateModified"), "").await;
    assert_eq!(
        ids("feed.json", &body).len(),
        2,
        "attrs selects; an Article without them has no title: {body}"
    );
    let (status, body) = send(&app, "GET", "/x/views/city/feed/v1/sport/rss.xml", "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        !body.contains("<language>"),
        "an output without lang names none: {body}"
    );
    let near = "/x/views/city/feed/v1/blizko/atom.xml";
    let (status, body) = send(&app, "GET", near, "").await;
    assert_eq!(status, StatusCode::OK, "no Event has a location: {body}");
    let (status, body) = send(
        &app,
        "GET",
        &format!("{near}?georel=near%3BmaxDistance==9"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, body) = send(&app, "GET", &format!("{ALL}/feed.json?type=Article"), "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        ids("feed.json", &body),
        ["tag:localhost,2026:city/urn:ngsi-ld:Article:a1"]
    );
}

#[tokio::test]
async fn pages_link_each_other_and_bad_parameters_are_400() {
    let app = fixture().await;
    let (_, first) = send(&app, "GET", &format!("{ALL}/atom.xml?limit=2"), "").await;
    assert_eq!(ids("atom.xml", &first).len(), 2);
    let next = "http://localhost:9090/x/views/city/feed/v1/vsetky/atom.xml?limit=2&amp;offset=2";
    assert!(
        first.contains(&format!("rel=\"next\" href=\"{next}\"")),
        "{first}"
    );
    assert!(!first.contains("rel=\"previous\""), "{first}");
    let (_, second) = send(
        &app,
        "GET",
        &format!("{ALL}/feed.json?offset=2&limit=2"),
        "",
    )
    .await;
    assert_eq!(
        ids("feed.json", &second),
        ["tag:localhost,2026:city/urn:ngsi-ld:Article:a1"]
    );
    let feed: Value = serde_json::from_str(&second).expect("json");
    assert!(feed.get("next_url").is_none(), "{second}");
    for query in [
        "limit=0",
        "limit=201",
        "limit=x",
        "offset=-1",
        "limit=1&limit=2",
        "from=2026",
        "type=Secret",
        "georel=near%3BmaxDistance==1",
    ] {
        let uri = if query.starts_with("georel") {
            format!("/x/views/city/feed/v1/sport/atom.xml?{query}")
        } else {
            format!("{ALL}/atom.xml?{query}")
        };
        let (status, body) = send(&app, "GET", &uri, "").await;
        if query.starts_with("georel") {
            assert_ne!(status, StatusCode::OK, "{query}: {body}");
        } else {
            assert_eq!(status, StatusCode::BAD_REQUEST, "{query}: {body}");
        }
    }
    let (status, body) = send(&app, "GET", "/x/views/void/feed/v1/vsetky/atom.xml", "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains("<updated>1970-01-01T00:00:00Z</updated>") && !body.contains("<entry>"),
        "{body}"
    );
}

#[tokio::test]
async fn a_conditional_get_of_an_unchanged_feed_is_304() {
    let app = fixture().await;
    let uri = format!("{ALL}/rss.xml");
    let (_, headers, _) = call(&app, "GET", &uri, "", &[]).await;
    let etag = headers["etag"].to_str().expect("etag").to_owned();
    let modified = headers["last-modified"].to_str().expect("date").to_owned();
    for asked in [
        ("If-None-Match", etag.as_str()),
        ("If-Modified-Since", modified.as_str()),
    ] {
        let (status, headers, body) = call(&app, "GET", &uri, "", &[asked]).await;
        assert_eq!(
            (status, body.as_str()),
            (StatusCode::NOT_MODIFIED, ""),
            "{asked:?}"
        );
        assert_eq!(headers["etag"], etag.as_str());
    }
    for stale in [
        ("If-None-Match", "\"other\""),
        ("If-Modified-Since", "Fri, 02 Oct 2026 10:00:00 GMT"),
    ] {
        let (status, _, _) = call(&app, "GET", &uri, "", &[stale]).await;
        assert_eq!(status, StatusCode::OK, "{stale:?}");
    }
}

#[tokio::test]
async fn the_policy_decides_every_read() {
    let engine = Arc::new(NoReads::default());
    let broker = TestBroker::new()
        .await
        .expect("broker")
        .with_policy(engine.clone());
    let app = fixture_with(broker).await;
    engine.0.store(true, Ordering::SeqCst);
    let (status, body) = send(&app, "GET", &format!("{ALL}/atom.xml"), "").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        !body.contains("urn:ngsi-ld:Event"),
        "a denied read served data: {body}"
    );
}

#[tokio::test]
async fn the_surface_writes_nothing() {
    let app = fixture().await;
    let uri = format!("{ALL}/atom.xml");
    let (status, _) = send(
        &app,
        "POST",
        &uri,
        r#"{"id":"urn:ngsi-ld:Event:x","type":"Event"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    let (status, _) = send(&app, "OPTIONS", &uri, "").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(&app, "GET", "/ngsi-ld/v1/entities/urn:ngsi-ld:Event:x", "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_request_naming_another_tenant_is_refused() {
    let app = fixture().await;
    let (status, _, _) = call(
        &app,
        "GET",
        &format!("{ALL}/atom.xml"),
        "",
        &[("NGSILD-Tenant", "void")],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_list_of_more_than_twenty_outputs_or_a_repeated_name_serves_nobody() {
    let app = fixture().await;
    let one = json!({"name": "x", "sources": [{"type": "Event", "preset": "event"}]});
    let many: Vec<Value> = (0..21)
        .map(
            |i| json!({"name": format!("o{i}"), "sources": [{"type": "Event", "preset": "event"}]}),
        )
        .collect();
    let broken = json!([{"name": "zle", "sources": [{"type": "Event", "unknown": 1}]}, one]);
    for (id, feed) in [
        ("many", json!(many)),
        ("twice", json!([one, one])),
        ("broken", broken),
    ] {
        let view = json!({"id": id, "tenant": "city", "selector": ["Event"], "operations": ["query_entities"],
            "rules": [{"types": ["Event"], "attributes": ["name"]}], "representations": {"feed": feed}});
        let (status, body) = send(&app, "PUT", &format!("/q/views/{id}"), &view.to_string()).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    for uri in [
        "/x/views/many/feed/v1/o0/atom.xml",
        "/x/views/twice/feed/v1/x/atom.xml",
        "/x/views/broken/feed/v1/zle/atom.xml",
    ] {
        let (status, body) = send(&app, "GET", uri, "").await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{uri}: {body}");
        assert!(
            !body.contains("unknown"),
            "the reason is the gateway's to tell: {body}"
        );
    }
    let (status, body) = send(&app, "GET", "/x/views/broken/feed/v1/x/atom.xml", "").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a broken sibling does not stop this output: {body}"
    );
}

/// F-ANT5-010: a view of every type pushes the output's types, so a Tenant
/// holding more Entities of another type than one read may scan still
/// serves its Events.
#[tokio::test]
async fn a_view_of_every_type_reads_only_the_outputs_types() {
    let app = fixture().await;
    let n = antares_ops::RESIDUAL_SCAN.saturating_add(1);
    let noise: Vec<Value> = (0..n)
        .map(|i| json!({"id": format!("urn:ngsi-ld:A:n{i:06}"), "type": "Noise"}))
        .collect();
    for chunk in noise.chunks(1000) {
        let body = Value::Array(chunk.to_vec()).to_string();
        let (status, b) = send(&app, "POST", "/ngsi-ld/v1/entityOperations/create", &body).await;
        assert_eq!(status, StatusCode::CREATED, "{b}");
    }
    let (status, body) = send(&app, "GET", "/x/views/every/feed/v1/vsetky/feed.json", "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let got = ids("feed.json", &body);
    assert!(
        got.iter().any(|id| id.ends_with("urn:ngsi-ld:Event:e2")),
        "{got:?}"
    );
}
