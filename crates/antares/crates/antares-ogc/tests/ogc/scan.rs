// SPDX-License-Identifier: EUPL-1.2
//! `items` through `ViewOps::scan` (T-3759, ADR-0028 rules 4 to 6): the `ngsild.` options
//! joined to the pushdown, `next` from the upstream cursor, `numberMatched` exact within the budget
//! and omitted beyond it, and a `bbox` the residual evaluates when `ngsild.georel` holds the
//! request's one geo query.

use crate::common::{
    fixture, fixture_with, get, href, ids, local, narrowed, schemas_on, send, Fixture,
};
use antares_ops::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ops::test_kit::TestBroker;
use axum::http::StatusCode;
use serde_json::{json, Value};
use std::sync::Arc;

const CARS: &str = "/x/views/city/ogc/features/collections/Car/items";

/// Every Car with a location lies within this box, so it holds `ngsild.georel` without
/// narrowing by itself.
const WORLD: &str = "ngsild.georel=within&ngsild.geometry=Polygon&ngsild.coordinates=%5B%5B%5B-180,-90%5D,%5B180,-90%5D,%5B180,90%5D,%5B-180,90%5D,%5B-180,-90%5D%5D%5D";

/// The queries every property below runs: plain, `bbox` (two parts), `bbox` beside an
/// `ngsild.georel` (a residual), `ngsild.q`, an order, and the residual with the order.
fn queries() -> Vec<String> {
    vec![
        String::new(),
        "bbox=170,-10,180,10".to_owned(),
        format!("bbox=170,-10,180,10&{WORLD}"),
        format!("bbox=-180,-90,180,90&{WORLD}"),
        "ngsild.q=speed%3E20".to_owned(),
        "sortby=-speed".to_owned(),
        format!("sortby=-speed&bbox=-180,-90,180,90&{WORLD}"),
    ]
}

/// Every page of `uri` followed through its `next` links: the ids in order and each page.
async fn pages(f: &Fixture, uri: &str) -> Result<(Vec<String>, Vec<Value>), String> {
    let (mut seen, mut bodies, mut uri) = (Vec::new(), Vec::new(), uri.to_owned());
    loop {
        let a = get(&f.app, &uri).await;
        if a.status != StatusCode::OK {
            return Err(format!("{uri}: {} {}", a.status, a.body));
        }
        seen.extend(ids(&a.body));
        let next = href(&a.body, "next");
        bodies.push(a.body);
        match next {
            Some(next) => uri = local(&next),
            None => return Ok((seen, bodies)),
        }
        if bodies.len() > 20 {
            return Err(format!("{uri}: the pages do not end"));
        }
    }
}

/// The value at the JSON pointer `path`, `null` where there is none.
fn at<'a>(v: &'a Value, path: &str) -> &'a Value {
    v.pointer(path).unwrap_or(&Value::Null)
}

fn with(query: &str, more: &str) -> String {
    match (query.is_empty(), more.is_empty()) {
        (true, _) => format!("{CARS}?{more}"),
        (false, true) => format!("{CARS}?{query}"),
        (false, false) => format!("{CARS}?{query}&{more}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn pages_concatenated_equal_the_unpaged_answer() -> Result<(), String> {
    let f = fixture().await?;
    for query in queries() {
        let (whole, _) = pages(&f, &with(&query, "limit=1000")).await?;
        for limit in 1..=3 {
            let (paged, bodies) = pages(&f, &with(&query, &format!("limit={limit}"))).await?;
            assert_eq!(paged, whole, "{query} by {limit}: no gap, no repeat");
            for body in &bodies {
                let returned = at(body, "/numberReturned").as_u64().unwrap_or(u64::MAX);
                assert!(returned <= limit, "{query}: {body}");
                assert_eq!(returned, ids(body).len() as u64);
                if let Some(matched) = body.get("numberMatched") {
                    assert_eq!(matched, &json!(whole.len()), "{query}: an exact count");
                }
            }
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bbox_beside_ngsild_georel_is_the_residual_and_stays_exact() -> Result<(), String> {
    let f = fixture().await?;
    let a = get(&f.app, &format!("{CARS}?bbox=170,-10,180,10&{WORLD}")).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    // Car 2 lies in the box; Car 3 has no location, so `ngsild.georel` drops it.
    assert_eq!(ids(&a.body), ["urn:ngsi-ld:Car:2"]);
    assert_eq!(*at(&a.body, "/numberMatched"), json!(1));
    let car = at(&a.body, "/features/0");
    assert_eq!(*at(car, "/properties/speed"), json!(60), "{car}");
    assert_eq!(*at(car, "/geometry/type"), json!("Point"), "{car}");
    assert_eq!(
        *at(car, "/properties/at"),
        json!("urn:ngsi-ld:Stop:1"),
        "a Relationship reads as the plain answer reads it"
    );
    let plain = get(&f.app, &format!("{CARS}?bbox=170,-10,180,10")).await;
    let same = at(&plain.body, "/features")
        .as_array()
        .and_then(|fs| fs.iter().find(|x| at(x, "/id") == "urn:ngsi-ld:Car:2"))
        .ok_or("Car 2 in the plain answer")?;
    assert_eq!(same, car, "the residual's Feature is the plain one");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn ngsild_options_narrow() -> Result<(), String> {
    let f = fixture().await?;
    let options = [
        "ngsild.q=speed%3E50",
        "ngsild.q=colour==%22red%22",
        "ngsild.scopeQ=/nowhere",
        WORLD,
        "ngsild.georel=near;maxDistance==1000&ngsild.geometry=Point&ngsild.coordinates=%5B17.1,48.1%5D",
        "ngsild.pick=id,type,speed",
        "ngsild.omit=speed",
    ];
    for query in queries() {
        let (base, base_pages) = pages(&f, &with(&query, "limit=1000")).await?;
        let features: Vec<&Value> = base_pages
            .iter()
            .flat_map(|b| at(b, "/features").as_array().into_iter().flatten())
            .collect();
        for option in options {
            let name = option.split('=').next().unwrap_or_default();
            if query.contains(name)
                || name.starts_with("ngsild.geo") && query.contains("ngsild.geo")
            {
                continue;
            }
            let (got, got_pages) =
                pages(&f, &with(&query, &format!("{option}&limit=1000"))).await?;
            assert!(
                got.iter().all(|id| base.contains(id)),
                "{query} + {option}: {got:?} is not within {base:?}"
            );
            for feature in got_pages
                .iter()
                .flat_map(|b| at(b, "/features").as_array().into_iter().flatten())
            {
                let plain = features
                    .iter()
                    .find(|p| at(p, "/id") == at(feature, "/id"))
                    .ok_or("the Feature in the plain answer")?;
                for (name, value) in at(feature, "/properties").as_object().into_iter().flatten() {
                    assert_eq!(
                        plain
                            .get("properties")
                            .and_then(|p| p.get(name))
                            .unwrap_or(&Value::Null),
                        value,
                        "{query} + {option}"
                    );
                }
            }
        }
    }
    // Each option does narrow.
    let fast = get(&f.app, &format!("{CARS}?ngsild.q=speed%3E50")).await;
    let mut got = ids(&fast.body);
    got.sort();
    assert_eq!(got, ["urn:ngsi-ld:Car:2", "urn:ngsi-ld:Car:4"]);
    assert_eq!(*at(&fast.body, "/numberMatched"), json!(2));
    let picked = get(&f.app, &format!("{CARS}?ngsild.pick=id,type,speed")).await;
    for feature in at(&picked.body, "/features")
        .as_array()
        .into_iter()
        .flatten()
    {
        let names: Vec<&String> = at(feature, "/properties")
            .as_object()
            .ok_or("properties")?
            .keys()
            .collect();
        assert_eq!(names, ["speed"], "{feature}");
    }
    let near = get(
        &f.app,
        &format!("{CARS}?ngsild.georel=near;maxDistance==1000&ngsild.geometry=Point&ngsild.coordinates=%5B17.1,48.1%5D"),
    )
    .await;
    assert_eq!(ids(&near.body), ["urn:ngsi-ld:Car:1"], "{}", near.body);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hidden_feature_never_appears_and_never_counts() -> Result<(), String> {
    let f = narrowed().await?;
    for query in queries() {
        let (seen, bodies) = pages(&f, &with(&query, "limit=1")).await?;
        assert!(
            seen.iter()
                .all(|id| id == "urn:ngsi-ld:Car:1" || id == "urn:ngsi-ld:Car:3"),
            "{query}: {seen:?}"
        );
        for body in &bodies {
            if let Some(matched) = body.get("numberMatched") {
                assert_eq!(
                    matched,
                    &json!(seen.len()),
                    "{query}: hidden Cars are not counted"
                );
            }
            assert!(!body.to_string().contains("\"s1\""), "a hidden Attribute");
        }
    }
    // A hidden Attribute evaluates as an absent one through ngsild.q.
    let probe = get(&f.app, &format!("{CARS}?ngsild.q=secret==%22s1%22")).await;
    assert_eq!(probe.status, StatusCode::OK);
    assert!(ids(&probe.body).is_empty(), "{}", probe.body);
    assert_eq!(*at(&probe.body, "/numberMatched"), json!(0));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn ngsild_options_are_checked() -> Result<(), String> {
    let f = fixture().await?;
    for (query, status) in [
        ("ngsild.q=speed%3E", StatusCode::BAD_REQUEST),
        ("ngsild.limit=5", StatusCode::BAD_REQUEST),
        (
            "ngsild.q=speed%3E1&ngsild.q=speed%3E2",
            StatusCode::BAD_REQUEST,
        ),
        (
            "properties=speed&ngsild.pick=speed",
            StatusCode::BAD_REQUEST,
        ),
        (
            "ngsild.georel=nowhere&ngsild.geometry=Point&ngsild.coordinates=%5B1,1%5D",
            StatusCode::BAD_REQUEST,
        ),
        ("properties=speed,colour&ngsild.omit=colour", StatusCode::OK),
    ] {
        let a = get(&f.app, &format!("{CARS}?{query}")).await;
        assert_eq!(a.status, status, "{query}: {}", a.body);
    }
    // ngsild.omit narrows what properties selected.
    let a = get(
        &f.app,
        &format!("{CARS}?properties=speed,colour&ngsild.omit=colour"),
    )
    .await;
    for feature in at(&a.body, "/features").as_array().into_iter().flatten() {
        assert!(
            at(feature, "/properties").get("colour").is_none(),
            "{feature}"
        );
    }
    assert!(a.body.to_string().contains("\"speed\""), "{}", a.body);
    let api = get(&f.app, "/x/views/city/ogc/features/api").await;
    let declared = at(&api.body, "/components/parameters")
        .as_object()
        .ok_or("parameters")?;
    for name in [
        "q",
        "scopeQ",
        "georel",
        "geometry",
        "coordinates",
        "geoproperty",
        "pick",
        "omit",
    ] {
        assert!(
            declared.contains_key(&format!("ngsild.{name}")),
            "ngsild.{name}"
        );
    }
    let items = at(&api.body, "/paths/~1collections~1Car~1items/get/parameters").to_string();
    assert!(items.contains("ngsild.q"), "{items}");
    Ok(())
}

/// ADR-0028 rule 5: past the budget the residual's count is omitted and the page is served.
#[tokio::test(flavor = "multi_thread")]
async fn number_matched_is_omitted_beyond_the_budget() -> Result<(), String> {
    let f = fixture().await?;
    let budget = antares_ops::RESIDUAL_SCAN;
    let mut next = 0;
    while next <= budget {
        let batch: Vec<Value> = (next..(next + 1000).min(budget + 1))
            .map(|i| json!({"id": format!("urn:ngsi-ld:Car:b{i}"), "type": "Car",
                            "location": {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [10.0, 10.0]}}}))
            .collect();
        next += batch.len();
        let a = send(
            &f.app,
            "POST",
            "/ngsi-ld/v1/entityOperations/create",
            &Value::Array(batch).to_string(),
            &[("NGSILD-Tenant", "city")],
        )
        .await;
        assert!(a.status.is_success(), "{} {}", a.status, a.body);
    }
    let residual = get(&f.app, &format!("{CARS}?bbox=9,9,11,11&{WORLD}&limit=5")).await;
    assert_eq!(residual.status, StatusCode::OK, "{}", residual.body);
    assert_eq!(*at(&residual.body, "/numberReturned"), json!(5));
    assert!(
        residual.body.get("numberMatched").is_none(),
        "{}",
        at(&residual.body, "/numberMatched")
    );
    assert!(href(&residual.body, "next").is_some());
    // The pushdown alone still counts exactly.
    let plain = get(&f.app, &format!("{CARS}?bbox=9,9,11,11&limit=5")).await;
    assert_eq!(
        *at(&plain.body, "/numberMatched"),
        json!(budget + 2),
        "and Car 3 without location"
    );
    Ok(())
}

/// An engine that refuses every query (clause 5.7) once locked, as a Policy without a read
/// grant would; unlocked while the fixture seeds.
#[derive(Default)]
struct NoReads(std::sync::atomic::AtomicBool);

impl PolicyEngine for NoReads {
    fn name(&self) -> &'static str {
        "no-reads"
    }

    fn decide<'a>(&'a self, _: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let locked = self.0.load(std::sync::atomic::Ordering::SeqCst);
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

/// A store that refuses the query is the view's refusal with its status, whatever the
/// `ngsild.` options, and names nothing of the Entities.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_query_is_the_views_refusal() -> Result<(), String> {
    schemas_on();
    let engine = Arc::new(NoReads::default());
    let f = fixture_with(TestBroker::new().await?.with_policy(engine.clone())).await?;
    engine.0.store(true, std::sync::atomic::Ordering::SeqCst);
    for query in queries() {
        let a = get(&f.app, &with(&query, "")).await;
        assert!(
            a.status.is_client_error(),
            "{query}: {} {}",
            a.status,
            a.body
        );
        assert_ne!(a.status, StatusCode::BAD_REQUEST, "{query}: {}", a.body);
        assert!(
            !a.body.to_string().contains("urn:ngsi-ld:Car"),
            "{}",
            a.body
        );
    }
    Ok(())
}

/// An answer as frames: its status, its `Content-Length`, each body frame, and whether the
/// body ended in an error.
async fn frames(
    app: &axum::Router,
    method: &str,
    uri: &str,
) -> (StatusCode, bool, Vec<String>, bool) {
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;
    let req = axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap_or_default();
    let Ok(resp) = app.clone().oneshot(req).await;
    let (status, sized) = (resp.status(), resp.headers().contains_key("content-length"));
    let mut body = resp.into_body();
    let mut out = Vec::new();
    while let Some(frame) = body.frame().await {
        let Ok(frame) = frame else {
            return (status, sized, out, true);
        };
        if let Ok(data) = frame.into_data() {
            out.push(String::from_utf8_lossy(&data).into_owned());
        }
    }
    (status, sized, out, false)
}

/// T-6036: past one store read a page is written one store read per frame, each frame holding
/// at most one upstream page of Features, with no length; what the frames make is the page a
/// broker reading it in one store read answers, and its `HEAD` sends none.
#[tokio::test(flavor = "multi_thread")]
async fn a_page_past_one_store_read_streams_and_equals_the_page_read_whole() -> Result<(), String> {
    let whole = fixture().await?;
    let mut streams = 0;
    // One Entity per store read, and two, where Cars 2 and 4 fill a residual's read at the end
    // of the store's answer.
    for max_limit in [1, 2] {
        let streamed = fixture_with(TestBroker::with_max_limit(max_limit).await?).await?;
        let south = format!("bbox=-180,-10,180,10&{WORLD}");
        for query in queries().into_iter().chain([south]) {
            for limit in ["limit=1", "limit=3", "limit=1000"] {
                let uri = with(&query, limit);
                let mut expected = get(&whole.app, &uri).await.body;
                let (status, sized, parts, broken) = frames(&streamed.app, "GET", &uri).await;
                assert_eq!((status, broken), (StatusCode::OK, false), "{uri}");
                let mut got: Value =
                    serde_json::from_str(&parts.concat()).map_err(|e| format!("{uri}: {e}"))?;
                if parts.len() > 1 {
                    streams += 1;
                    assert!(!sized, "{uri}: a streamed page states no length");
                    for part in &parts {
                        assert!(
                            part.matches("\"type\":\"Feature\"").count() <= max_limit,
                            "{uri}: {part}"
                        );
                    }
                }
                // `links` aside: a store read of one Entity cannot tell its answer ended, so
                // `next` may name an empty page; the pages followed hold the same Features.
                for body in [&mut expected, &mut got] {
                    body.as_object_mut()
                        .map(|m| (m.remove("timeStamp"), m.remove("links")));
                }
                assert_eq!(got, expected, "{uri}");
                assert_eq!(
                    pages(&streamed, &uri).await?.0,
                    pages(&whole, &uri).await?.0,
                    "{uri}"
                );
            }
        }
    }
    assert!(streams > 0, "some page went out in frames");
    let streamed = fixture_with(TestBroker::with_max_limit(1).await?).await?;
    let (status, sized, parts, _) = frames(&streamed.app, "HEAD", &with("", "limit=1000")).await;
    assert_eq!(
        (status, sized, parts.concat()),
        (StatusCode::OK, false, String::new())
    );
    let (status, sized, _, _) = frames(&streamed.app, "HEAD", &with("", "limit=1")).await;
    assert_eq!(
        (status, sized),
        (StatusCode::OK, true),
        "a page of one read keeps its length"
    );
    Ok(())
}

/// Once locked, admits as many more queries as its count holds, then refuses each (clause 5.7).
#[derive(Default)]
struct ReadsLeft(
    std::sync::atomic::AtomicBool,
    std::sync::atomic::AtomicUsize,
);

impl PolicyEngine for ReadsLeft {
    fn name(&self) -> &'static str {
        "reads-left"
    }

    fn decide<'a>(&'a self, _: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        use std::sync::atomic::Ordering::SeqCst;
        let refused = self.0.load(SeqCst)
            && op.clause.starts_with("5.7.")
            && self
                .1
                .try_update(SeqCst, SeqCst, |n| n.checked_sub(1))
                .is_err();
        Box::pin(std::future::ready(if refused {
            Decision::Deny("no read".into())
        } else {
            Decision::Allow
        }))
    }

    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

/// T-6036: a store refusing a read after the first byte breaks the body; it never ends as a
/// complete `FeatureCollection` holding fewer Features than the page.
#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_after_the_first_byte_breaks_the_body() -> Result<(), String> {
    schemas_on();
    let engine = Arc::new(ReadsLeft::default());
    let broker = TestBroker::with_max_limit(1)
        .await?
        .with_policy(engine.clone());
    let f = fixture_with(broker).await?;
    let uri = with("", "limit=1000");
    let (_, _, full, _) = frames(&f.app, "GET", &uri).await;
    engine.0.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut broke = 0;
    for left in 0..12 {
        engine.1.store(left, std::sync::atomic::Ordering::SeqCst);
        let (status, _, parts, cut) = frames(&f.app, "GET", &uri).await;
        if status != StatusCode::OK {
            assert!(
                !parts.concat().contains("urn:ngsi-ld:Car"),
                "{left}: {parts:?}"
            );
            continue;
        }
        if cut {
            broke += 1;
            assert!(
                serde_json::from_str::<Value>(&parts.concat()).is_err(),
                "{left}: {parts:?}"
            );
        } else {
            assert_eq!(
                parts.concat().len(),
                full.concat().len(),
                "{left}: the whole page"
            );
        }
    }
    assert!(broke > 0, "some refusal came after the first byte");
    Ok(())
}
