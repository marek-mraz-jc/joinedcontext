// SPDX-License-Identifier: EUPL-1.2
//! `$apply` (Data Aggregation 4.0, odata-linkml-mapping Q19, T-3937): each transformation over
//! the Cars the view shows. The view shows `brand`, `owner`, `seats`, `speed` and `since` and
//! hides `note`; the narrowing engine hides `owner` and every Car outside the scope `/open`
//! (Car 5), so no aggregate counts or sums either.

use antares_ops::policy::{
    Decision, DecisionFuture, Filter, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const CITY: &str = "https://example.org/city/";
const ROOT: &str = "/x/views/cars/odata/v4";

const SCHEMA: &str = r#"{
  "id": "https://example.org/city", "name": "city",
  "prefixes": {"city": {"prefix_prefix": "city", "prefix_reference": "https://example.org/city/"}},
  "default_prefix": "city",
  "classes": {"Car": {"slots": ["brand", "owner", "note", "speed", "seats", "since", "day", "at"]}},
  "slots": {"brand": {"range": "string"}, "owner": {"range": "string"}, "note": {"range": "string"},
            "speed": {"range": "float"}, "seats": {"range": "integer"},
            "since": {"range": "datetime"}, "day": {"range": "date"}, "at": {"range": "time"}}
}"#;

/// `(n, brand, owner)` of a Car; its seats are `n`, its speed `10.5 n`, `since` the 15th of
/// month `n` of 2025 in UTC. `None` leaves the Attribute out. Car 5 lies outside `/open`.
const CARS: [(u32, Option<&str>, Option<&str>); 5] = [
    (1, Some("vw"), Some("Ana")),
    (2, Some("vw"), Some("Bob")),
    (3, Some("bmw"), Some("Ana")),
    (4, Some("bmw"), None),
    (5, None, Some("Dan")),
];

/// When it hides anything: `owner` from every query, and every Car outside `/open`.
struct Hiding(bool);

impl PolicyEngine for Hiding {
    fn name(&self) -> &'static str {
        "hiding"
    }
    fn decide<'a>(&'a self, _: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let decision = if self.0 && op.clause.starts_with("5.7.") {
            Decision::Filter(Filter {
                omit: vec![format!("{CITY}owner")],
                scope_q: Some("/open".into()),
                ..Filter::default()
            })
        } else {
            Decision::Allow
        };
        Box::pin(std::future::ready(decision))
    }
    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<String>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("NGSILD-Tenant", "city");
    if let Some(b) = &body {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", b.len());
    }
    let Ok(resp) = app
        .clone()
        .oneshot(
            req.body(Body::from(body.unwrap_or_default()))
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
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, body)
}

async fn broker(hiding: bool) -> Result<(TestBroker, Router), String> {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    let b = TestBroker::new()
        .await?
        .with_policy(Arc::new(Hiding(hiding)));
    let app = b.serve(antares_odata::OData)?;
    let view = json!({
        "id": "cars", "tenant": "city", "@context": {"city": CITY},
        "selector": ["city:Car"],
        "rules": [{"types": ["city:Car"],
                   "attributes": ["city:brand", "city:owner", "city:speed", "city:seats",
                                  "city:since", "city:day", "city:at"]}],
        "operations": ["query_entities", "retrieve_entity"]
    });
    let mut writes = vec![
        ("/q/views/cars".to_owned(), view),
        (
            "/q/schemas/city".to_owned(),
            serde_json::from_str(SCHEMA).map_err(|e| e.to_string())?,
        ),
    ];
    for (n, brand, owner) in CARS {
        let mut car = json!({
            "id": format!("urn:ngsi-ld:Car:{n}"), "type": format!("{CITY}Car"),
            "scope": if n == 5 { "/closed" } else { "/open" },
            format!("{CITY}speed"): {"type": "Property", "value": f64::from(n) * 10.5},
            format!("{CITY}seats"): {"type": "Property", "value": n},
            format!("{CITY}since"): {"type": "Property",
                "value": {"@type": "DateTime", "@value": format!("2025-{n:02}-15T00:00:00Z")}},
            format!("{CITY}day"): {"type": "Property",
                "value": {"@type": "Date", "@value": format!("2025-{n:02}-15")}},
            format!("{CITY}at"): {"type": "Property",
                "value": {"@type": "Time", "@value": format!("{n:02}:00:00")}},
            format!("{CITY}note"): {"type": "Property", "value": "secret"}
        });
        for (name, value) in [("brand", brand), ("owner", owner)] {
            if let (Some(v), Some(members)) = (value, car.as_object_mut()) {
                members.insert(
                    format!("{CITY}{name}"),
                    json!({"type": "Property", "value": v}),
                );
            }
        }
        writes.push(("/ngsi-ld/v1/entities".to_owned(), car));
    }
    for (uri, doc) in writes {
        let method = if uri.starts_with("/q/") {
            Method::PUT
        } else {
            Method::POST
        };
        let (status, answer) = send(&app, method, &uri, Some(doc.to_string())).await;
        if status != StatusCode::CREATED {
            return Err(format!("{uri}: {status} {answer}"));
        }
    }
    Ok((b, app))
}

/// A read of `Car` with `query`, its body.
async fn read(app: &Router, query: &str) -> Result<Value, String> {
    let uri = format!("{ROOT}/Car?{}", query.replace(' ', "%20"));
    let (status, body) = send(app, Method::GET, &uri, None).await;
    if status != StatusCode::OK {
        return Err(format!("{query}: {status} {body}"));
    }
    Ok(body)
}

/// The rows of `$apply={apply}`.
async fn rows(app: &Router, apply: &str) -> Result<Value, String> {
    let body = read(app, &format!("$apply={apply}")).await?;
    Ok(body.get("value").cloned().unwrap_or_default())
}

/// The ids of the Entities of `$apply={apply}`, in its order.
async fn ids(app: &Router, apply: &str) -> Result<Vec<String>, String> {
    let body = read(app, &format!("$apply={apply}")).await?;
    Ok(body
        .get("value")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|e| Some(e["id"].as_str()?.rsplit(':').next()?.to_owned()))
        .collect())
}

#[tokio::test]
async fn aggregate_folds_every_method_over_the_whole_answer() -> Result<(), String> {
    let (_b, app) = broker(false).await?;
    let all = "aggregate($count as n,seats with sum as s,speed with average as a,\
               seats with min as lo,seats with max as hi,brand with countdistinct as d,\
               since with max as last,brand with min as first)";
    assert_eq!(
        rows(&app, all).await?,
        json!([{"n": 5, "s": 15, "a": 31.5, "lo": 1, "hi": 5, "d": 2,
                "last": "2025-05-15T00:00:00Z", "first": "bmw"}])
    );
    // a float sum stays a float; no instance: one row, a count of 0 and nulls
    assert_eq!(
        rows(&app, "aggregate(speed with sum as s)").await?,
        json!([{"s": 157.5}])
    );
    assert_eq!(
        rows(&app, "filter(seats gt 9)/aggregate($count as n,seats with sum as s,seats with average as a,seats with max as m,seats with countdistinct as d)").await?,
        json!([{"n": 0, "s": null, "a": null, "m": null, "d": 0}])
    );
    // an expression is aggregated as its value
    assert_eq!(
        rows(&app, "aggregate(seats mul 2 with sum as s)").await?,
        json!([{"s": 30}])
    );
    Ok(())
}

#[tokio::test]
async fn groupby_makes_a_row_per_distinct_key_in_the_order_first_seen() -> Result<(), String> {
    let (_b, app) = broker(false).await?;
    assert_eq!(
        rows(
            &app,
            "groupby((brand),aggregate($count as n,seats with sum as s))"
        )
        .await?,
        json!([{"brand": "vw", "n": 2, "s": 3}, {"brand": "bmw", "n": 2, "s": 7},
               {"brand": null, "n": 1, "s": 5}])
    );
    assert_eq!(
        rows(&app, "groupby((brand,owner))").await?,
        json!([{"brand": "vw", "owner": "Ana"}, {"brand": "vw", "owner": "Bob"},
               {"brand": "bmw", "owner": "Ana"}, {"brand": "bmw", "owner": null},
               {"brand": null, "owner": "Dan"}])
    );
    // the rows go on through the later transformations, by their own names
    assert_eq!(
        rows(
            &app,
            "groupby((brand),aggregate(seats with sum as s))/filter(s gt 4)/orderby(s desc)"
        )
        .await?,
        json!([{"brand": "bmw", "s": 7}, {"brand": null, "s": 5}])
    );
    assert_eq!(
        rows(&app, "groupby((brand),aggregate($count as n))/aggregate(n with max as most,$count as groups)").await?,
        json!([{"most": 2, "groups": 3}])
    );
    assert_eq!(
        rows(&app, "groupby((brand),aggregate($count as n))/groupby((n),aggregate($count as k))/orderby(n)/skip(1)/top(1)/identity").await?,
        json!([{"n": 2, "k": 2}])
    );
    // a date, a time and an instant of a row compare as such
    assert_eq!(
        rows(
            &app,
            "groupby((day,at))/filter(day ge 2025-03-01 and at lt 05:00:00)/orderby(day desc)"
        )
        .await?,
        json!([{"day": "2025-04-15", "at": "04:00:00"}, {"day": "2025-03-15", "at": "03:00:00"}])
    );
    assert_eq!(
        rows(&app, "groupby((brand),aggregate(since with max as last))/filter(last gt 2025-03-01T00:00:00Z)/orderby(brand)").await?,
        json!([{"brand": null, "last": "2025-05-15T00:00:00Z"},
               {"brand": "bmw", "last": "2025-04-15T00:00:00Z"}])
    );
    Ok(())
}

#[tokio::test]
async fn the_rows_page_count_and_name_their_context() -> Result<(), String> {
    let (_b, app) = broker(false).await?;
    let first = read(&app, "$apply=groupby((brand))&$count=true&$top=2").await?;
    assert_eq!(first["@odata.count"], 3, "{first}");
    assert_eq!(first["value"], json!([{"brand": "vw"}, {"brand": "bmw"}]));
    assert!(
        first["@odata.nextLink"].is_null(),
        "$top ends the answer: {first}"
    );
    assert!(
        first["@odata.context"]
            .as_str()
            .is_some_and(|c| c.ends_with("$metadata#Car(brand)")),
        "{first}"
    );
    let uri = format!("{ROOT}/Car?$apply=groupby((brand))");
    let (status, page) = send(&app, Method::GET, &uri, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["value"].as_array().map(Vec::len), Some(3));
    let req = Request::builder()
        .uri(&uri)
        .header("NGSILD-Tenant", "city")
        .header("Prefer", "odata.maxpagesize=2")
        .header("OData-Version", "4.01")
        .header("Accept", "application/json;odata.metadata=none")
        .body(Body::empty())
        .map_err(|e| e.to_string())?;
    let Ok(resp) = app.clone().oneshot(req).await;
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    let paged: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    assert!(paged.get("@context").is_none(), "no metadata: {paged}");
    assert_eq!(paged["value"], json!([{"brand": "vw"}, {"brand": "bmw"}]));
    let next = paged["@nextLink"].as_str().unwrap_or_default();
    let next = next.split_once("/x/").map(|(_, rest)| format!("/x/{rest}"));
    let (status, last) = send(&app, Method::GET, &next.unwrap_or_default(), None).await;
    assert_eq!(status, StatusCode::OK, "{last}");
    assert_eq!(last["value"], json!([{"brand": null}]));
    Ok(())
}

#[tokio::test]
async fn transformations_that_keep_the_entities_answer_entities() -> Result<(), String> {
    let (_b, app) = broker(false).await?;
    assert_eq!(ids(&app, "identity").await?, ["1", "2", "3", "4", "5"]);
    assert_eq!(
        ids(&app, "filter(seats gt 1)/orderby(seats desc)/top(2)").await?,
        ["5", "4"]
    );
    assert_eq!(ids(&app, "skip(3)").await?, ["4", "5"]);
    assert_eq!(ids(&app, "top(3)/filter(brand eq 'vw')").await?, ["1", "2"]);
    let body = read(
        &app,
        "$apply=orderby(brand,seats desc)&$count=true&$skip=1&$top=1",
    )
    .await?;
    assert_eq!(body["@odata.count"], 5, "{body}");
    assert_eq!(body["value"][0]["id"], "urn:ngsi-ld:Car:4", "{body}");
    assert_eq!(body["value"][0]["seats"], 4, "the Entity whole: {body}");
    Ok(())
}

#[tokio::test]
async fn an_aggregate_never_counts_or_sums_what_the_view_or_the_engine_hides() -> Result<(), String>
{
    let (_b, app) = broker(true).await?;
    // Car 5 is outside the engine's scope: never counted, its seats never summed
    assert_eq!(
        rows(
            &app,
            "aggregate($count as n,seats with sum as s,brand with countdistinct as d)"
        )
        .await?,
        json!([{"n": 4, "s": 10, "d": 2}])
    );
    // `owner` reads as absent on every Car the engine serves
    assert_eq!(
        rows(
            &app,
            "aggregate(owner with countdistinct as d,owner with max as m)"
        )
        .await?,
        json!([{"d": 0, "m": null}])
    );
    assert_eq!(
        rows(&app, "groupby((owner),aggregate($count as n))").await?,
        json!([{"owner": null, "n": 4}])
    );
    assert_eq!(
        ids(&app, "filter(owner eq 'Dan')").await?,
        Vec::<String>::new()
    );
    // the view hides `note`: the 400 of an unknown name
    for apply in [
        "aggregate(note with countdistinct as d)",
        "aggregate(note with sum as s)",
        "groupby((note))",
    ] {
        let uri = format!("{ROOT}/Car?$apply={}", apply.replace(' ', "%20"));
        let (status, body) = send(&app, Method::GET, &uri, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{apply}: {body}");
    }
    Ok(())
}

#[tokio::test]
async fn apply_is_refused_where_it_does_not_apply() -> Result<(), String> {
    let (_b, app) = broker(false).await?;
    for (uri, want) in [
        // a transformation not served, a rollup, a custom aggregate
        ("Car?$apply=expand(owner)", StatusCode::NOT_IMPLEMENTED),
        (
            "Car?$apply=groupby((rollup(brand)))",
            StatusCode::NOT_IMPLEMENTED,
        ),
        ("Car?$apply=aggregate(seats)", StatusCode::NOT_IMPLEMENTED),
        // the other options beside it, and the forms it does not reach
        (
            "Car?$apply=identity&$filter=seats gt 1",
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            "Car?$apply=identity&$orderby=seats",
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            "Car?$apply=identity&$select=seats",
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            "Car?$apply=identity&$search=vw",
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            "Car?$apply=identity&$compute=1 as x",
            StatusCode::NOT_IMPLEMENTED,
        ),
        ("Car?$apply=identity&$expand=*", StatusCode::NOT_IMPLEMENTED),
        ("Car/$count?$apply=identity", StatusCode::NOT_IMPLEMENTED),
        ("Car/$ref?$apply=identity", StatusCode::NOT_IMPLEMENTED),
        (
            "Car('urn:ngsi-ld:Car:1')?$apply=identity",
            StatusCode::NOT_IMPLEMENTED,
        ),
        // sum and average take numbers; unknown and produced-only names
        (
            "Car?$apply=aggregate(brand with sum as s)",
            StatusCode::BAD_REQUEST,
        ),
        (
            "Car?$apply=aggregate(since with average as a)",
            StatusCode::BAD_REQUEST,
        ),
        ("Car?$apply=groupby((nothing))", StatusCode::BAD_REQUEST),
        (
            "Car?$apply=aggregate($count as n)/filter(seats gt 1)",
            StatusCode::BAD_REQUEST,
        ),
        (
            "Car?$apply=groupby((brand),aggregate($count as n))/aggregate(brand with sum as s)",
            StatusCode::BAD_REQUEST,
        ),
        (
            "Car?$apply=filter(brand add 1 eq 2)",
            StatusCode::BAD_REQUEST,
        ),
        (
            "Car('urn:ngsi-ld:Car:1')/brand?$apply=identity",
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            "Car?$apply=groupby((brand),aggregate($count as n))/filter(brand add 1 eq 2)",
            StatusCode::BAD_REQUEST,
        ),
        (
            "Car?$apply=identity&$skiptoken=nonsense",
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let uri = uri.replace(' ', "%20");
        let (status, body) = send(&app, Method::GET, &format!("{ROOT}/{uri}"), None).await;
        assert_eq!(status, want, "{uri}: {body}");
        assert!(body["error"]["code"].is_string(), "{uri}: {body}");
    }
    Ok(())
}

#[tokio::test]
async fn the_metadata_says_which_transformations_apply() -> Result<(), String> {
    let (_b, app) = broker(false).await?;
    let (status, body) = send(&app, Method::GET, &format!("{ROOT}/$metadata"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let text = body.as_str().unwrap_or_default();
    assert!(text.contains("Org.OData.Aggregation.V1"), "{text}");
    assert!(text.contains("Aggregation.ApplySupported"), "{text}");
    for word in [
        "identity",
        "filter",
        "orderby",
        "top",
        "skip",
        "aggregate",
        "groupby",
        "topcount",
        "bottompercent",
        "concat",
        "compute",
        "search",
    ] {
        assert!(
            text.contains(&format!("<String>{word}</String>")),
            "{word}: {text}"
        );
    }
    assert!(!text.contains("<String>expand</String>"), "{text}");
    Ok(())
}

#[tokio::test]
async fn top_and_bottom_keep_the_highest_or_lowest_in_rank_order() -> Result<(), String> {
    let (_b, app) = broker(false).await?;
    for (apply, want) in [
        ("topcount(2,seats)", &["5", "4"][..]),
        ("bottomcount(2,seats)", &["1", "2"]),
        ("topcount(0,seats)", &[]),
        // the fewest whose sum reaches 9, the fewest from the bottom that reach 4
        ("topsum(9,seats)", &["5", "4"]),
        ("bottomsum(4,seats)", &["1", "2", "3"]),
        // 50 and 20 percent of 15 seats
        ("toppercent(50,seats)", &["5", "4"]),
        ("bottompercent(20,seats)", &["1", "2"]),
        ("toppercent(100,seats mul 2)", &["5", "4", "3", "2", "1"]),
        // ties keep their order; an instance without a value is not ranked
        ("topcount(3,brand)", &["1", "2", "3"]),
        ("bottomcount(9,brand)", &["3", "4", "1", "2"]),
        ("filter(seats lt 4)/topcount(9,seats)", &["3", "2", "1"]),
    ] {
        assert_eq!(ids(&app, apply).await?, want, "{apply}");
    }
    assert_eq!(
        rows(
            &app,
            "groupby((brand),aggregate(seats with sum as s))/topcount(1,s)"
        )
        .await?,
        json!([{"brand": "bmw", "s": 7}])
    );
    for apply in [
        "topsum(5,brand)",
        "bottompercent(5,since)",
        "topcount(-1,seats)",
        "topcount('two',seats)",
        "topcount(seats,seats)",
    ] {
        let uri = format!("{ROOT}/Car?$apply={}", apply.replace(' ', "%20"));
        let (status, body) = send(&app, Method::GET, &uri, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{apply}: {body}");
    }
    Ok(())
}

#[tokio::test]
async fn concat_answers_each_sequence_one_after_the_other() -> Result<(), String> {
    let (_b, app) = broker(false).await?;
    assert_eq!(
        ids(&app, "concat(topcount(1,seats),bottomcount(1,seats))").await?,
        ["5", "1"]
    );
    assert_eq!(
        ids(
            &app,
            "filter(brand eq 'vw')/concat(identity,orderby(seats desc))/top(3)"
        )
        .await?,
        ["1", "2", "2"]
    );
    // rows under every name of any sequence, null where a sequence has none
    assert_eq!(
        rows(
            &app,
            "concat(aggregate($count as n),groupby((brand),aggregate($count as k)))"
        )
        .await?,
        json!([{"n": 5, "brand": null, "k": null},
               {"n": null, "brand": "vw", "k": 2},
               {"n": null, "brand": "bmw", "k": 2},
               {"n": null, "brand": null, "k": 1}])
    );
    assert_eq!(
        rows(
            &app,
            "groupby((brand),aggregate($count as n))/concat(topcount(1,n),bottomcount(1,n))"
        )
        .await?,
        json!([{"brand": "vw", "n": 2}, {"brand": null, "n": 1}])
    );
    // a mix of Entities and rows is not served
    let uri = format!("{ROOT}/Car?$apply=concat(identity,aggregate($count%20as%20n))");
    let (status, body) = send(&app, Method::GET, &uri, None).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{body}");
    Ok(())
}

#[tokio::test]
async fn compute_and_search_inside_apply() -> Result<(), String> {
    let (_b, app) = broker(false).await?;
    // a computed name is read later and written with the Entities
    let body = read(
        &app,
        "$apply=compute(seats mul 2 as d)/filter(d gt 6)/orderby(d desc)",
    )
    .await?;
    assert_eq!(body["value"][0]["id"], "urn:ngsi-ld:Car:5", "{body}");
    assert_eq!(body["value"][0]["d"], 10, "{body}");
    assert_eq!(body["value"].as_array().map(Vec::len), Some(2), "{body}");
    assert_eq!(
        rows(
            &app,
            "compute(seats mul 2 as d)/compute(d add 1 as e)/aggregate(e with sum as s)"
        )
        .await?,
        json!([{"s": 35}])
    );
    assert_eq!(
        rows(
            &app,
            "groupby((brand),aggregate(seats with sum as s))/compute(s mul 10 as t)/filter(t gt 60)"
        )
        .await?,
        json!([{"brand": "bmw", "s": 7, "t": 70}])
    );
    assert_eq!(ids(&app, "search(vw)").await?, ["1", "2"]);
    assert_eq!(
        ids(&app, "search(vw OR bmw)/topcount(1,seats)").await?,
        ["4"]
    );
    for (apply, want) in [
        ("groupby((brand))/search(vw)", StatusCode::NOT_IMPLEMENTED),
        ("compute(nothing as x)", StatusCode::BAD_REQUEST),
        ("concat(identity,topsum(5,brand))", StatusCode::BAD_REQUEST),
        (
            "compute(brand as x)/aggregate(x with sum as s)",
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let uri = format!("{ROOT}/Car?$apply={}", apply.replace(' ', "%20"));
        let (status, body) = send(&app, Method::GET, &uri, None).await;
        assert_eq!(status, want, "{apply}: {body}");
    }
    Ok(())
}

#[tokio::test]
async fn ranking_concat_and_search_never_reach_what_is_hidden() -> Result<(), String> {
    let (_b, app) = broker(true).await?;
    // Car 5 is outside the scope, `owner` hidden by the engine
    assert_eq!(ids(&app, "topcount(9,seats)").await?, ["4", "3", "2", "1"]);
    assert_eq!(
        ids(&app, "toppercent(100,seats)").await?,
        ["4", "3", "2", "1"]
    );
    assert_eq!(ids(&app, "topcount(9,owner)").await?, Vec::<String>::new());
    assert_eq!(
        ids(&app, "concat(identity,bottomcount(1,seats))").await?,
        ["1", "2", "3", "4", "1"]
    );
    assert_eq!(ids(&app, "search(Dan)").await?, Vec::<String>::new());
    assert_eq!(
        rows(
            &app,
            "compute(seats as k)/aggregate(k with sum as s,$count as n)"
        )
        .await?,
        json!([{"s": 10, "n": 4}])
    );
    // the view hides `note`: an unknown name
    for apply in [
        "topcount(1,note)",
        "compute(note as x)",
        "concat(identity,filter(note eq 'x'))",
    ] {
        let uri = format!("{ROOT}/Car?$apply={}", apply.replace(' ', "%20"));
        let (status, body) = send(&app, Method::GET, &uri, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{apply}: {body}");
    }
    Ok(())
}

#[tokio::test]
async fn a_chain_of_concat_is_refused_before_it_outgrows_one_scan() -> Result<(), String> {
    let (_b, app) = broker(false).await?;
    // each step doubles the Entities: 2^40 of them would never fit in memory (T-4017)
    let apply = ["concat(identity,identity)"; 40].join("/");
    let started = std::time::Instant::now();
    let (status, body) = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$apply={apply}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.to_string().contains("TooManyResults"), "{body}");
    assert!(started.elapsed().as_secs() < 10, "{:?}", started.elapsed());
    // a few doublings stay within the cap and answer every copy
    assert_eq!(
        ids(&app, &["concat(identity,identity)"; 3].join("/"))
            .await?
            .len(),
        40
    );
    Ok(())
}

/// F-ANT4-003: `$metadata` names what the engine lets the caller read, as
/// the LinkML surface does; the same view without the engine names `owner`.
#[tokio::test]
async fn the_metadata_hides_what_the_engine_hides() -> Result<(), String> {
    for (hiding, named) in [(true, false), (false, true)] {
        let (_b, app) = broker(hiding).await?;
        for doc in ["$metadata"] {
            let (status, body) = send(&app, Method::GET, &format!("{ROOT}/{doc}"), None).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            let text = body.to_string();
            assert!(text.contains("brand"), "{text}");
            assert_eq!(text.contains("owner"), named, "hiding {hiding}: {text}");
        }
    }
    Ok(())
}
