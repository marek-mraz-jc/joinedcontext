// SPDX-License-Identifier: EUPL-1.2
//! OData reads through `ViewOps::scan` (T-3758, ADR-0028): a filter conjunct `q` cannot say is
//! evaluated on what the view answered, pages of it chain to the unpaged answer, a count or a
//! surface sort reads the whole answer within the budget and refuses beyond it, and the
//! `ngsild.*` custom options only narrow. The view shows `speed`, `label` and `seen` of a Car and
//! hides `secret`; every fifth Car has no `speed`.

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use tower::ServiceExt;

const CITY: &str = "https://example.org/city/";
const ROOT: &str = "/x/views/cars/odata/v4";
const CARS: u32 = 30;

const SCHEMA: &str = r#"{
  "id": "https://example.org/city", "name": "city",
  "prefixes": {"city": {"prefix_prefix": "city", "prefix_reference": "https://example.org/city/"}},
  "default_prefix": "city",
  "classes": {"Car": {"slots": ["speed", "secret", "label", "seen", "follows"]}},
  "slots": {"speed": {"range": "float"}, "secret": {"range": "integer"}, "follows": {"range": "Car"},
            "label": {"range": "string"}, "seen": {"range": "datetime"}}
}"#;

struct Answer {
    status: StatusCode,
    body: Value,
}

async fn get(app: &Router, uri: &str, headers: &[(&str, &str)]) -> Answer {
    let mut req = Request::builder().method(Method::GET).uri(uri);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let Ok(resp) = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap_or_default())
        .await;
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    Answer { status, body }
}

async fn put(app: &Router, uri: &str, body: String, tenant: Option<&str>) -> StatusCode {
    let mut req = Request::builder()
        .method(if tenant.is_some() {
            Method::POST
        } else {
            Method::PUT
        })
        .uri(uri)
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len());
    if let Some(t) = tenant {
        req = req.header("NGSILD-Tenant", t);
    }
    let Ok(resp) = app
        .clone()
        .oneshot(req.body(Body::from(body)).unwrap_or_default())
        .await;
    resp.status()
}

fn car(n: u32) -> Value {
    let mut car = json!({
        "id": format!("urn:ngsi-ld:Car:{n:03}"),
        "type": format!("{CITY}Car"),
        "scope": if n.is_multiple_of(2) { "/even" } else { "/odd" },
        format!("{CITY}secret"): {"type": "Property", "value": 7},
        format!("{CITY}label"): {"type": "Property", "value": format!("car {n}")},
        format!("{CITY}follows"): {"type": "Relationship", "object": "urn:ngsi-ld:Car:001"},
    });
    if !n.is_multiple_of(5) {
        if let Some(members) = car.as_object_mut() {
            members.insert(
                format!("{CITY}speed"),
                json!({"type": "Property", "value": f64::from(n)}),
            );
        }
    }
    car
}

/// A broker serving the view `cars` over `cars` Cars, made in batches.
async fn broker(cars: u32) -> Result<(TestBroker, Router), String> {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    seeded(TestBroker::new().await?, cars).await
}

/// `b` serving the cars view over `cars` Cars; the schema bound is set before `b` is made.
async fn seeded(b: TestBroker, cars: u32) -> Result<(TestBroker, Router), String> {
    let app = b.serve(antares_odata::OData)?;
    let view = json!({
        "id": "cars", "tenant": "city",
        "@context": {"city": CITY, "speed": format!("{CITY}speed"), "secret": format!("{CITY}secret"),
                     "label": format!("{CITY}label"), "Car": format!("{CITY}Car")},
        "selector": ["city:Car"],
        "rules": [{"types": ["city:Car"], "attributes": ["city:speed", "city:label", "city:seen", "city:follows"]}],
        "operations": ["query_entities", "retrieve_entity"]
    });
    if put(&app, "/q/views/cars", view.to_string(), None).await != StatusCode::CREATED {
        return Err("the view".into());
    }
    if put(&app, "/q/schemas/city", SCHEMA.to_owned(), None).await != StatusCode::CREATED {
        return Err("the schema".into());
    }
    let all: Vec<Value> = (1..=cars).map(car).collect();
    for batch in all.chunks(1000) {
        let status = put(
            &app,
            "/ngsi-ld/v1/entityOperations/create",
            Value::from(batch.to_vec()).to_string(),
            Some("city"),
        )
        .await;
        if status != StatusCode::CREATED {
            return Err(format!("the cars: {status}"));
        }
    }
    Ok((b, app))
}

fn ids(body: &Value) -> Vec<String> {
    body["value"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["id"].as_str().map(str::to_owned))
        .collect()
}

/// Every page of `uri`, following next links, as one list of ids.
async fn chained(app: &Router, uri: &str, page: usize) -> Result<Vec<String>, String> {
    let prefer = format!("odata.maxpagesize={page}");
    let mut all = Vec::new();
    let mut link = Some(uri.to_owned());
    let mut pages = 0u32;
    while let Some(uri) = link.take() {
        let answer = get(app, &uri, &[("Prefer", &prefer)]).await;
        if answer.status != StatusCode::OK {
            return Err(format!("{uri}: {} {}", answer.status, answer.body));
        }
        let got = ids(&answer.body);
        if got.len() > page {
            return Err(format!("{uri}: {} on a page of {page}", got.len()));
        }
        all.extend(got);
        link = answer
            .body
            .get("@odata.nextLink")
            .and_then(Value::as_str)
            .map(str::to_owned);
        pages = pages.saturating_add(1);
        if pages > 200 {
            return Err("the links never end".into());
        }
    }
    Ok(all)
}

/// The ids of the Cars `keep` admits, in id order.
fn expected(keep: impl Fn(u32) -> bool) -> Vec<String> {
    (1..=CARS)
        .filter(|n| keep(*n))
        .map(|n| format!("urn:ngsi-ld:Car:{n:03}"))
        .collect()
}

fn speed(n: u32) -> Option<u32> {
    (!n.is_multiple_of(5)).then_some(n)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_residual_page_and_its_next_links_are_the_unpaged_answer() -> Result<(), String> {
    let (_b, app) = broker(CARS).await?;
    // F11: `not (speed gt 20)` holds where speed is absent (URL 5.1.1.1.3)
    let filter = "$filter=not%20(speed%20gt%2020)";
    let want = expected(|n| speed(n).is_none_or(|s| s <= 20));
    let whole = chained(&app, &format!("{ROOT}/Car?{filter}"), 1000).await?;
    assert_eq!(whole, want, "in id order, the order the scan reads in");
    for page in [1, 2, 3, 7, 22, 23] {
        let paged = chained(&app, &format!("{ROOT}/Car?{filter}"), page).await?;
        assert_eq!(paged, whole, "pages of {page}: no gap, no repeat");
    }
    // `$skip` and `$top` are of the filtered answer, and the token carries what is left
    for (skip, top) in [(0, 5), (2, 7), (9, 100), (24, 3), (40, 2)] {
        let paged = chained(
            &app,
            &format!("{ROOT}/Car?{filter}&$skip={skip}&$top={top}"),
            2,
        )
        .await?;
        let end = whole.len().min(skip + top);
        assert_eq!(
            paged,
            whole.get(skip.min(end)..end).unwrap_or_default(),
            "$skip={skip}&$top={top}"
        );
    }
    // a pushed conjunct beside it narrows the same way
    let both = chained(
        &app,
        &format!("{ROOT}/Car?{filter}%20and%20speed%20gt%2010"),
        4,
    )
    .await?;
    assert_eq!(
        both.len(),
        expected(|n| speed(n).is_some_and(|s| s > 10 && s <= 20)).len()
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_residual_count_is_the_served_total() -> Result<(), String> {
    let (_b, app) = broker(CARS).await?;
    for filter in [
        "not%20(speed%20gt%2020)",
        "not%20(speed%20lt%203%20or%20label%20eq%20'car%207')",
        "not%20startswith(label,'car%201')",
    ] {
        let served = chained(&app, &format!("{ROOT}/Car?$filter={filter}"), 1000).await?;
        let counted = get(
            &app,
            &format!("{ROOT}/Car?$filter={filter}&$count=true"),
            &[("Prefer", "odata.maxpagesize=4")],
        )
        .await;
        assert_eq!(counted.status, StatusCode::OK, "{}", counted.body);
        assert_eq!(
            counted.body["@odata.count"],
            json!(served.len()),
            "{filter}"
        );
        assert_eq!(ids(&counted.body).len(), 4.min(served.len()));
        let paged = chained(&app, &format!("{ROOT}/Car?$filter={filter}&$count=true"), 4).await?;
        assert_eq!(paged, served, "a counted answer pages the same Entities");
        let bare = get(&app, &format!("{ROOT}/Car/$count?$filter={filter}"), &[]).await;
        assert_eq!(bare.body, json!(served.len()), "{filter}");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_count_or_a_sort_read_in_store_reads_of_one_or_three_equals_the_read_whole(
) -> Result<(), String> {
    // T-6049: a counted or surface-sorted read keeps only what its page serves while the
    // scan reads on; a broker reading one or three Entities at a time answers as one reading
    // the whole set at once, ties (the Cars without a speed) in id order
    let (_w, whole) = broker(CARS).await?;
    let filter = "$filter=not%20(speed%20gt%2020)";
    let uris = [
        format!("{ROOT}/Car?$orderby=speed"),
        format!("{ROOT}/Car?$orderby=speed&$top=3"),
        format!("{ROOT}/Car?$orderby=speed%20desc&$skip=2&$top=4"),
        format!("{ROOT}/Car?$orderby=speed%20desc&{filter}&$count=true"),
        format!("{ROOT}/Car?{filter}&$count=true"),
        format!("{ROOT}/Car?{filter}&$count=true&$skip=3&$top=5"),
        format!("{ROOT}/Car?{filter}&$count=true&$skip=40"),
        format!("{ROOT}/Car/$count?{filter}"),
    ];
    for limit in [1, 3] {
        std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
        let (_b, parts) = seeded(TestBroker::with_max_limit(limit).await?, CARS).await?;
        for uri in &uris {
            let want = get(&whole, uri, &[]).await;
            let got = get(&parts, uri, &[]).await;
            assert_eq!(want.status, StatusCode::OK, "{uri}: {}", want.body);
            assert_eq!(got.body, want.body, "{uri} at {limit}");
            if !uri.contains("/$count") {
                let paged = chained(&parts, uri, 2).await?;
                assert_eq!(paged, chained(&whole, uri, 2).await?, "{uri} at {limit}");
            }
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_count_or_a_surface_sort_over_the_budget_is_400_and_a_page_is_served(
) -> Result<(), String> {
    let over = u32::try_from(antares_ops::RESIDUAL_SCAN).map_err(|e| e.to_string())? + 1;
    let (_b, app) = broker(over).await?;
    let filter = "$filter=not%20(speed%20gt%2020)";
    for uri in [
        format!("{ROOT}/Car?{filter}&$count=true"),
        format!("{ROOT}/Car/$count?{filter}"),
        format!("{ROOT}/Car?$orderby=createdAt"),
        // an aggregate reads the whole answer: refused past the bound, never one over a part
        format!("{ROOT}/Car?$apply=aggregate($count%20as%20n)"),
    ] {
        let answer = get(&app, &uri, &[]).await;
        assert_eq!(
            answer.status,
            StatusCode::BAD_REQUEST,
            "{uri}: {}",
            answer.body
        );
        assert_eq!(answer.body["error"]["code"], "TooManyResults", "{uri}");
        assert!(answer.body.get("value").is_none(), "never a partial body");
    }
    let page = get(
        &app,
        &format!("{ROOT}/Car?{filter}"),
        &[("Prefer", "odata.maxpagesize=5")],
    )
    .await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert_eq!(ids(&page.body).len(), 5);
    assert!(page.body["@odata.nextLink"].is_string());
    // the broker's own count still answers a filter it can say whole
    let pushed = get(
        &app,
        &format!("{ROOT}/Car/$count?$filter=speed%20gt%2020"),
        &[],
    )
    .await;
    assert_eq!(pushed.status, StatusCode::OK, "{}", pushed.body);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn orderby_created_at_is_sorted_by_the_surface_and_pages_keep_it() -> Result<(), String> {
    let (_b, app) = broker(CARS).await?;
    for (order, descending) in [("createdAt", false), ("createdAt%20desc", true)] {
        let uri = format!("{ROOT}/Car?$orderby={order},id&$select=createdAt,label");
        let answer = get(&app, &uri, &[]).await;
        assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
        let stamps: Vec<String> = answer.body["value"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|e| e["createdAt"].as_str().unwrap_or_default().to_owned())
            .collect();
        assert_eq!(stamps.len(), usize::try_from(CARS).unwrap_or_default());
        let mut sorted = stamps.clone();
        sorted.sort();
        if descending {
            sorted.reverse();
        }
        assert_eq!(stamps, sorted, "{order}");
        let whole = ids(&answer.body);
        assert_eq!(
            chained(&app, &uri, 4).await?,
            whole,
            "pages of a sorted answer"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn ngsild_options_narrow() -> Result<(), String> {
    let (_b, app) = broker(CARS).await?;
    let answer = |query: String| {
        let app = app.clone();
        async move {
            let ids = chained(&app, &format!("{ROOT}/Car?{query}"), 1000).await?;
            Ok::<BTreeSet<String>, String>(ids.into_iter().collect())
        }
    };
    let filters = [
        "",
        "$filter=speed%20gt%2010",
        "$filter=not%20(speed%20gt%2020)",
        "$filter=label%20eq%20'car%205'%20or%20speed%20lt%204",
    ];
    let options = [
        "ngsild.q=speed%3E15",
        "ngsild.q=speed%3C8%7Clabel==%22car%2025%22",
        "ngsild.q=!speed",
        "ngsild.scopeQ=/even",
        "ngsild.q=speed%3E2&ngsild.scopeQ=/odd",
    ];
    for filter in filters {
        let odata = answer(filter.to_owned()).await?;
        for option in options {
            let ngsild = answer(option.to_owned()).await?;
            let sep = if filter.is_empty() { "" } else { "&" };
            let both = answer(format!("{filter}{sep}{option}")).await?;
            let meet: BTreeSet<String> = odata.intersection(&ngsild).cloned().collect();
            assert_eq!(both, meet, "{filter} & {option}");
        }
    }
    // a hidden Attribute answers as an absent one, pushed or evaluated
    let all = answer(String::new()).await?;
    assert_eq!(answer("ngsild.q=secret==7".into()).await?, BTreeSet::new());
    assert_eq!(answer("ngsild.q=!secret".into()).await?, all);
    assert_eq!(
        answer("ngsild.q=!secret".into()).await?,
        answer("ngsild.q=!nosuch".into()).await?
    );
    for filter in [
        "not%20(secret%20gt%201)",
        "not%20(nosuch%20gt%201)",
        "secret%20gt%201",
    ] {
        let refused = get(&app, &format!("{ROOT}/Car?$filter={filter}"), &[]).await;
        assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{filter}");
    }
    // pick and omit drop members, never Entities (TS 104 175 4.21)
    let picked = get(
        &app,
        &format!("{ROOT}/Car?ngsild.pick=label&$filter=speed%20gt%2010"),
        &[],
    )
    .await;
    assert_eq!(picked.status, StatusCode::OK, "{}", picked.body);
    let first = &picked.body["value"][0];
    assert!(
        first.get("label").is_some() && first.get("speed").is_none(),
        "{first}"
    );
    assert_eq!(
        ids(&picked.body).len(),
        answer("$filter=speed%20gt%2010".into()).await?.len()
    );
    let omitted = get(
        &app,
        &format!("{ROOT}/Car?ngsild.omit=label,secret&ngsild.q=speed%3E28"),
        &[],
    )
    .await;
    let first = &omitted.body["value"][0];
    assert!(
        first.get("label").is_none() && first.get("speed").is_some(),
        "{first}"
    );
    assert_eq!(ids(&omitted.body).len(), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn other_custom_options_and_misplaced_ngsild_ones_are_400() -> Result<(), String> {
    let (_b, app) = broker(5).await?;
    for uri in [
        format!("{ROOT}/Car?colour=red"),
        format!("{ROOT}/Car?ngsild.limit=1"),
        format!("{ROOT}/Car?NGSILD.q=speed%3E1"),
        format!("{ROOT}/Car?ngsild.q=speed%3E1&ngsild.q=speed%3E2"),
        format!("{ROOT}/Car?ngsild.q"),
        format!("{ROOT}/Car?ngsild.q=(speed"),
        format!("{ROOT}/Car?ngsild.pick=a%7Bb%7D"),
        format!("{ROOT}/Car?ngsild.geoproperty=a,b"),
        format!("{ROOT}/Car?ngsild.georel=nowhere"),
        format!("{ROOT}/Car('urn:ngsi-ld:Car:001')?ngsild.q=speed%3E1"),
        format!("{ROOT}/Car('urn:ngsi-ld:Car:001')/label?ngsild.q=speed%3E1"),
        format!("{ROOT}/$metadata?ngsild.q=speed%3E1"),
        format!("{ROOT}/?ngsild.q=speed%3E1"),
    ] {
        let answer = get(&app, &uri, &[]).await;
        assert_eq!(
            answer.status,
            StatusCode::BAD_REQUEST,
            "{uri}: {}",
            answer.body
        );
        assert!(
            !answer.body.to_string().contains("colour"),
            "{}",
            answer.body
        );
    }
    let counted = get(&app, &format!("{ROOT}/Car/$count?ngsild.q=speed%3E2"), &[]).await;
    assert_eq!((counted.status, counted.body), (StatusCode::OK, json!(2)));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_token_of_the_earlier_form_or_an_edited_cursor_is_handled() -> Result<(), String> {
    let (_b, app) = broker(CARS).await?;
    let first = get(
        &app,
        &format!("{ROOT}/Car?$filter=not%20(speed%20gt%2020)"),
        &[("Prefer", "odata.maxpagesize=3")],
    )
    .await;
    let next = first.body["@odata.nextLink"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let token = next
        .split("$skiptoken=")
        .nth(1)
        .unwrap_or_default()
        .to_owned();
    assert!(token.starts_with("2."), "{token}");
    let digest = token.rsplit('.').next().unwrap_or_default();
    let old = format!("3.-.{digest}");
    let refused = get(
        &app,
        &format!("{ROOT}/Car?$filter=not%20(speed%20gt%2020)&$skiptoken={old}"),
        &[],
    )
    .await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.body);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_scanned_read_beside_a_join_serves_the_expansion() -> Result<(), String> {
    // T-3823: the scan reads the store's truncation warning, so a residual, a residual count
    // and a surface sort may expand entities and follow a hop
    let (_b, app) = broker(CARS).await?;
    let residual = "$filter=not%20(speed%20gt%2020)";
    let want = expected(|n| speed(n).is_none_or(|s| s <= 20));
    for (uri, want) in [
        (format!("{ROOT}/Car?{residual}&$expand=follows&$top=1000"), &want),
        (format!("{ROOT}/Car?$orderby=createdAt&$expand=follows&$top=1000"), &expected(|_| true)),
        (
            format!("{ROOT}/Car?$count=true&{residual}&$expand=follows($select=label)&$top=1000"),
            &want,
        ),
        (format!("{ROOT}/Car?{residual}&$expand=follows/$ref&$top=1000"), &want),
        // every Car follows Car 1, of speed 1: a hop beside a residual
        (
            format!("{ROOT}/Car?$filter=follows/speed%20ge%201%20and%20not%20(speed%20gt%2020)&$top=1000"),
            &want,
        ),
    ] {
        let answer = get(&app, &uri, &[]).await;
        assert_eq!(answer.status, StatusCode::OK, "{uri}: {}", answer.body);
        assert_eq!(ids(&answer.body), *want, "{uri}");
        if uri.contains("$expand") {
            let follows = &answer.body["value"][0]["follows"];
            assert!(!follows.is_null(), "{uri}: {}", answer.body);
        }
    }
    let counted = get(
        &app,
        &format!("{ROOT}/Car?$count=true&{residual}&$expand=follows"),
        &[],
    )
    .await;
    assert_eq!(
        counted.body["@odata.count"],
        json!(want.len()),
        "{}",
        counted.body
    );
    let label = get(
        &app,
        &format!("{ROOT}/Car?{residual}&$expand=follows($select=label)"),
        &[],
    )
    .await;
    assert_eq!(
        label.body["value"][0]["follows"]["label"], "car 1",
        "{}",
        label.body
    );
    // a system member selected inside the expansion is read with sysAttrs through the scan
    let created = get(
        &app,
        &format!("{ROOT}/Car?{residual}&$expand=follows($select=createdAt)"),
        &[],
    )
    .await;
    let follows = &created.body["value"][0]["follows"];
    assert!(follows["createdAt"].is_string(), "{}", created.body);
    let hop = "$filter=follows/speed%20ge%201%20and%20not%20(speed%20gt%2020)";
    let bare = get(&app, &format!("{ROOT}/Car/$count?{hop}"), &[]).await;
    assert_eq!(bare.body, json!(want.len()), "{}", bare.body);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_join_past_the_lookup_budget_is_400_scanned_or_pushed() -> Result<(), String> {
    // T-3823: Car 1 follows Car 2 more times than one request may look up (4.5.23.1); a
    // partial expansion is never served, whether the store or the scan answers
    let (_b, app) = broker(CARS).await?;
    let long = json!({format!("{CITY}follows"): {"type": "ListRelationship",
        "objectList": vec!["urn:ngsi-ld:Car:002"; 1001]}});
    let status = put(
        &app,
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:001/attrs",
        long.to_string(),
        Some("city"),
    )
    .await;
    assert!(status.is_success(), "{status}");
    for query in [
        "$filter=speed%20lt%2030&$expand=follows",
        "$filter=not%20(speed%20gt%2020)&$expand=follows",
        "$orderby=createdAt&$expand=follows",
    ] {
        let answer = get(&app, &format!("{ROOT}/Car?{query}"), &[]).await;
        assert_eq!(
            answer.status,
            StatusCode::BAD_REQUEST,
            "{query}: {}",
            answer.body
        );
        assert!(
            answer.body.to_string().contains("expansion reaches"),
            "{}",
            answer.body
        );
    }
    Ok(())
}

/// A key every Car holds stays pushed beside a residual: the store's order is
/// URL 5.1.5's when nothing lacks the key (T-3853).
#[tokio::test(flavor = "multi_thread")]
async fn a_key_every_entity_holds_stays_pushed_beside_a_residual() -> Result<(), String> {
    let (_b, app) = broker(CARS).await?;
    let residual = "$filter=not%20(speed%20gt%2020)";
    let by_label = |filter: &str| format!("{ROOT}/Car?{filter}&$orderby=label%20desc&$top=1000");
    let answer = get(&app, &by_label(residual), &[]).await;
    let pushed = get(
        &app,
        &by_label("$filter=speed%20le%2020%20or%20speed%20eq%20null"),
        &[],
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    assert_eq!(ids(&answer.body), ids(&pushed.body));
    assert_eq!(ids(&answer.body).len(), 22);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_scanned_read_keeps_select_orderby_and_the_stores_refusals() -> Result<(), String> {
    let (_b, app) = broker(CARS).await?;
    let residual = "$filter=not%20(speed%20gt%2020)";
    // a pushed $orderby beside a residual
    let answer = get(
        &app,
        &format!("{ROOT}/Car?{residual}&$orderby=speed%20desc"),
        &[],
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    // the order the store gives the same Cars when the whole filter is pushed
    let pushed = get(
        &app,
        &format!(
            "{ROOT}/Car?$filter=speed%20le%2020%20or%20speed%20eq%20null&$orderby=speed%20desc"
        ),
        &[],
    )
    .await;
    assert_eq!(pushed.status, StatusCode::OK, "{}", pushed.body);
    assert_eq!(ids(&answer.body), ids(&pushed.body));
    assert_eq!(ids(&answer.body).len(), 22);
    // a surface sort reads its value keys whether $select names them or not
    // (`label`: every Car has one; a sparse one drops Cars until T-3822)
    for select in ["label", "label,speed"] {
        let uri = format!("{ROOT}/Car?$orderby=createdAt,speed%20desc&$select={select}");
        let answer = get(&app, &uri, &[]).await;
        assert_eq!(answer.status, StatusCode::OK, "{uri}: {}", answer.body);
        assert_eq!(ids(&answer.body).len(), 30, "{uri}");
        let first = &answer.body["value"][0];
        assert_eq!(first.get("speed").is_some(), select != "label", "{first}");
    }
    // a surface sort beside a residual sorts what the residual kept
    let answer = get(
        &app,
        &format!("{ROOT}/Car?{residual}&$orderby=createdAt%20desc"),
        &[],
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    let mut sorted = ids(&pushed.body);
    sorted.sort();
    let mut got = ids(&answer.body);
    got.sort();
    assert_eq!(got, sorted);
    // a counted window past the end serves nothing and links nowhere
    let answer = get(
        &app,
        &format!("{ROOT}/Car?{residual}&$count=true&$skip=100"),
        &[],
    )
    .await;
    assert_eq!(
        (
            answer.status,
            ids(&answer.body).len(),
            answer.body.get("@odata.count")
        ),
        (StatusCode::OK, 0, Some(&json!(22))),
        "{}",
        answer.body
    );
    assert!(
        answer.body.get("@odata.nextLink").is_none(),
        "{}",
        answer.body
    );
    // a counted page that ends the window has no next link
    let answer = get(
        &app,
        &format!("{ROOT}/Car?{residual}&$count=true&$top=3"),
        &[],
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    assert_eq!(ids(&answer.body).len(), 3);
    assert!(
        answer.body.get("@odata.nextLink").is_none(),
        "{}",
        answer.body
    );
    // what the store answers on a pushed read it answers on a scanned one
    for (option, status) in [
        ("ngsild.georel=nowhere", StatusCode::BAD_REQUEST),
        ("ngsild.geoproperty=label", StatusCode::OK),
    ] {
        let pushed = get(&app, &format!("{ROOT}/Car?{option}"), &[]).await;
        let scanned = get(&app, &format!("{ROOT}/Car?{residual}&{option}"), &[]).await;
        assert_eq!(pushed.status, status, "{option}: {}", pushed.body);
        assert_eq!(scanned.status, pushed.status, "{option}: {}", scanned.body);
    }
    // the residual's options are the collection GET's alone
    let status = put(
        &app,
        &format!("{ROOT}/Car?ngsild.q=speed%3E1"),
        "{}".into(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_scanned_read_of_an_empty_tenant_or_a_refused_view_answers_as_a_pushed_one(
) -> Result<(), String> {
    let (_b, app) = broker(3).await?;
    let view = |id: &str, tenant: &str, operations: Value| {
        json!({
            "id": id, "tenant": tenant,
            "@context": {"city": CITY},
            "selector": ["city:Car"],
            "rules": [{"types": ["city:Car"], "attributes": ["city:speed"]}],
            "operations": operations
        })
        .to_string()
    };
    let empty = view(
        "empty",
        "nobody",
        json!(["query_entities", "retrieve_entity"]),
    );
    let shut = view("shut", "city", json!(["retrieve_entity"]));
    for (uri, body) in [
        ("/q/views/empty", empty),
        ("/q/schemas/nobody", SCHEMA.to_owned()),
        ("/q/views/shut", shut),
    ] {
        let status = put(&app, uri, body, None).await;
        assert_eq!(status, StatusCode::CREATED, "{uri}");
    }
    let residual = "$filter=not%20(speed%20gt%2020)";
    for id in ["empty", "shut"] {
        let root = format!("/x/views/{id}/odata/v4/Car");
        let pushed = get(&app, &root, &[]).await;
        let scanned = get(&app, &format!("{root}?{residual}"), &[]).await;
        assert_eq!(
            (scanned.status, ids(&scanned.body)),
            (pushed.status, ids(&pushed.body)),
            "{id}: {} / {}",
            scanned.body,
            pushed.body
        );
    }
    let empty = get(
        &app,
        &format!("/x/views/empty/odata/v4/Car?{residual}"),
        &[],
    )
    .await;
    assert_eq!(empty.status, StatusCode::OK, "{}", empty.body);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_select_projects_and_never_selects() -> Result<(), String> {
    // T-3822, URL 5.1.4: `$select` (and `ngsild.pick`/`ngsild.omit`) narrow members, never
    // Entities: no Car holds `seen`, 6 hold no `speed`
    let (_b, app) = broker(CARS).await?;
    let all = expected(|_| true);
    let unfiltered = expected(|n| speed(n).is_none_or(|s| s <= 20));
    let cases = [
        ("$select=seen", &all, Some("seen")),
        ("$select=speed", &all, Some("speed")),
        ("$select=seen,createdAt", &all, Some("seen")),
        ("ngsild.pick=seen", &all, None),
        ("ngsild.omit=speed,label,seen", &all, None),
        ("$select=seen&$expand=follows", &all, Some("seen")),
        (
            "$select=seen&$filter=not%20(speed%20gt%2020)",
            &unfiltered,
            Some("seen"),
        ),
        (
            "ngsild.pick=seen&$filter=not%20(speed%20gt%2020)",
            &unfiltered,
            None,
        ),
    ];
    for (query, want, null) in cases {
        let page = get(
            &app,
            &format!("{ROOT}/Car?{query}&$count=true&$top=1000"),
            &[],
        )
        .await;
        assert_eq!(page.status, StatusCode::OK, "{query}: {}", page.body);
        assert_eq!(page.body["@odata.count"], json!(want.len()), "{query}");
        assert_eq!(ids(&page.body), *want, "{query}");
        if let Some(member) = null {
            let first = &page.body["value"][0];
            assert!(first.get(member).is_some(), "{query}: {first}");
            assert!(first.get("label").is_none(), "{query}: {first}");
        }
    }
    let no_speed = get(
        &app,
        &format!("{ROOT}/Car('urn:ngsi-ld:Car:005')?$select=speed"),
        &[],
    )
    .await;
    assert_eq!(no_speed.status, StatusCode::OK, "{}", no_speed.body);
    assert_eq!(no_speed.body["speed"], Value::Null, "{}", no_speed.body);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn orderby_puts_a_missing_value_first_ascending_and_last_descending() -> Result<(), String> {
    // T-3853, URL 5.1.5: null first ascending, last descending; ties in id order
    let (_b, app) = broker(CARS).await?;
    let id = |n: u32| format!("urn:ngsi-ld:Car:{n:03}");
    let speedless: Vec<String> = (1..=CARS).filter(|n| speed(*n).is_none()).map(id).collect();
    let mut fast: Vec<u32> = (1..=CARS).filter(|n| speed(*n).is_some()).collect();
    let rising: Vec<String> = speedless
        .iter()
        .cloned()
        .chain(fast.iter().copied().map(id))
        .collect();
    // the speedless tie on `speed`, so `label` orders them, as strings: "car 10" before "car 5"
    let mut labelled = speedless.clone();
    labelled.sort_by_key(|i| {
        format!(
            "car {}",
            i.rsplit(':').next().unwrap_or("").trim_start_matches('0')
        )
    });
    let by_label: Vec<String> = labelled
        .into_iter()
        .chain(fast.iter().copied().map(id))
        .collect();
    fast.reverse();
    let falling: Vec<String> = fast
        .into_iter()
        .map(id)
        .chain(speedless.iter().cloned())
        .collect();
    for (orderby, want) in [
        ("speed", &rising),
        ("speed%20desc", &falling),
        ("speed,label", &by_label),
        ("speed%20desc,id", &falling),
    ] {
        for page in [1000, 7] {
            let got = chained(&app, &format!("{ROOT}/Car?$orderby={orderby}"), page).await?;
            assert_eq!(got, *want, "$orderby={orderby}, pages of {page}");
        }
        // beside a residual, through the scan
        let got = chained(
            &app,
            &format!("{ROOT}/Car?$orderby={orderby}&$filter=not%20(speed%20gt%2020)"),
            4,
        )
        .await?;
        let kept: Vec<String> = want
            .iter()
            .filter(|i| {
                let n: u32 = i
                    .rsplit(':')
                    .next()
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0);
                speed(n).is_none_or(|s| s <= 20)
            })
            .cloned()
            .collect();
        assert_eq!(got, kept, "$orderby={orderby} beside a residual");
    }
    // a key every Entity holds stays the store's order
    let labels = chained(&app, &format!("{ROOT}/Car?$orderby=label%20desc"), 7).await?;
    let mut want: Vec<(String, String)> = (1..=CARS).map(|n| (format!("car {n}"), id(n))).collect();
    want.sort();
    want.reverse();
    assert_eq!(labels, want.into_iter().map(|(_, i)| i).collect::<Vec<_>>());
    Ok(())
}
