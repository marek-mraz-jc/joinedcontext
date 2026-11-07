// SPDX-License-Identifier: EUPL-1.2
//! The `{Set}_history` sets (T-5908): written through the NGSI-LD temporal API, read through
//! OData as one row per Entity and `observedAt` with the same instances the temporal API
//! answers, in the same order, paged; `$filter` on `observedAt` as the temporal window; no
//! write; a hidden Attribute's history absent; no history where the view offers no Temporal
//! Query.

mod writes;

use axum::http::{Method, StatusCode};
use serde_json::{json, Value};
use writes::{city, odata, send, stored, CITY};

const PAST: &str = "/x/views/past/odata/v4";
const T0: &str = "2026-10-01T10:00:00Z";
const T1: &str = "2026-10-01T10:05:00Z";
const T2: &str = "2026-10-01T10:10:00Z";
const T3: &str = "2026-10-01T10:15:00Z";

fn at(v: &Value, t: &str) -> Value {
    json!({"type": "Property", "value": v, "observedAt": t})
}

/// The `city` fixture and view `past` (Cars' speed and label, the Query Entities, the Temporal
/// Query and the create), with the histories of Car:7 (speed 10 at T0 and 20 at T2, label at
/// T1, the hidden `ownerRef` at T3) and Car:8 (speed 60 at T1 and 70 at T3).
async fn past() -> Result<axum::Router, String> {
    let (b, app) = city().await?;
    std::mem::forget(b);
    let view = json!({"id": "past", "tenant": "city", "@context": {"city": CITY},
        "selector": ["city:Car"],
        "rules": [{"types": ["city:Car"], "attributes": ["city:speed", "city:label"]}],
        "operations": ["query_entities", "retrieve_entity", "temporal_query", "create_entity"]});
    let (status, answer) = send(&app, Method::PUT, "/q/views/past", Some(&view.to_string())).await;
    if status != StatusCode::CREATED {
        return Err(format!("view: {status} {answer}"));
    }
    let speed = format!("{CITY}speed");
    let label = format!("{CITY}label");
    let owner = format!("{CITY}ownerRef");
    let histories = [
        json!({"id": "urn:ngsi-ld:Car:7", "type": format!("{CITY}Car"),
               speed.clone(): [at(&json!(10.0), T0), at(&json!(20.0), T2)],
               label: [at(&json!("seven"), T1)],
               owner: [at(&json!("hidden-in-history"), T3)]}),
        json!({"id": "urn:ngsi-ld:Car:8", "type": format!("{CITY}Car"),
               speed: [at(&json!(60.0), T1), at(&json!(70.0), T3)]}),
    ];
    for h in histories {
        let (status, answer) = send(
            &app,
            Method::POST,
            "/ngsi-ld/v1/temporal/entities",
            Some(&h.to_string()),
        )
        .await;
        if !status.is_success() {
            return Err(format!("{h}: {status} {answer}"));
        }
    }
    Ok(app)
}

async fn get(app: &axum::Router, path: &str) -> (StatusCode, Value) {
    send(app, Method::GET, &format!("{PAST}/{path}"), None).await
}

/// Each row's id, observedAt and speed, in the answer's order.
fn rows(body: &Value) -> Vec<(String, String, Value)> {
    body["value"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| {
            (
                r["id"].as_str().unwrap_or_default().to_owned(),
                r["observedAt"].as_str().unwrap_or_default().to_owned(),
                r["speed"].clone(),
            )
        })
        .collect()
}

/// The speed instances the NGSI-LD temporal API answers for the Cars of history, as
/// (id, observedAt, value), in id then time order.
async fn temporal(app: &axum::Router, window: &str) -> Vec<(String, String, Value)> {
    let uri = format!(
        "/ngsi-ld/v1/temporal/entities?type={}&attrs={}&{window}",
        urlencode(&format!("{CITY}Car")),
        urlencode(&format!("{CITY}speed"))
    );
    let (status, body) = send(app, Method::GET, &uri, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mut out: Vec<(String, String, Value)> = body
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|e| {
            let id = e["id"].as_str().unwrap_or_default().to_owned();
            let list = match &e[format!("{CITY}speed")] {
                Value::Array(l) => l.clone(),
                one @ Value::Object(_) => vec![one.clone()],
                _ => Vec::new(),
            };
            list.into_iter().map(move |i| {
                (
                    id.clone(),
                    i.get("observedAt")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    i.get("value").cloned().unwrap_or_default(),
                )
            })
        })
        .collect();
    out.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    out
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(b).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The rows of `rows` whose time a speed instance has, the instance's value beside it.
fn observed(
    rows: &[(String, String, Value)],
    instances: &[(String, String, Value)],
) -> Vec<(String, String, Value)> {
    rows.iter()
        .filter(|r| instances.iter().any(|i| i.0 == r.0 && i.1 == r.1))
        .cloned()
        .collect()
}

fn car(n: u8) -> String {
    format!("urn:ngsi-ld:Car:{n}")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_history_holds_a_row_per_observed_at_with_the_values_valid_then() {
    let app = past().await.expect("fixture");
    let (status, body) = get(&app, "Car_history").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        rows(&body),
        [
            (car(7), T0.to_owned(), json!(10.0)),
            (car(7), T1.to_owned(), json!(10.0)),
            (car(7), T2.to_owned(), json!(20.0)),
            (car(8), T1.to_owned(), json!(60.0)),
            (car(8), T3.to_owned(), json!(70.0)),
        ],
        "the hidden ownerRef at T3 makes no Car:7 row: {body}"
    );
    let label: Vec<&Value> = body["value"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| &r["label"])
        .collect();
    assert_eq!(
        label,
        [
            &Value::Null,
            &json!("seven"),
            &json!("seven"),
            &Value::Null,
            &Value::Null
        ]
    );
    let text = body.to_string();
    assert!(
        !text.contains("hidden-in-history") && !text.contains("ownerRef"),
        "{text}"
    );
    let speeds = temporal(&app, &format!("timerel=after&timeAt={T0}")).await;
    assert_eq!(
        observed(&rows(&body), &speeds),
        speeds,
        "each speed instance the temporal API answers is a row holding its value"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_filter_on_observed_at_is_the_temporal_window() {
    let app = past().await.expect("fixture");
    let (status, body) = get(
        &app,
        &format!("Car_history?$filter=observedAt%20ge%20{T1}%20and%20observedAt%20lt%20{T3}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        rows(&body),
        [
            (car(7), T1.to_owned(), json!(10.0)),
            (car(7), T2.to_owned(), json!(20.0)),
            (car(8), T1.to_owned(), json!(60.0)),
        ],
        "a row past the lower bound holds the value observed before it: {body}"
    );
    let speeds = temporal(&app, &format!("timerel=between&timeAt={T1}&endTimeAt={T3}")).await;
    assert_eq!(observed(&rows(&body), &speeds), speeds);
    let (_, fast) = get(
        &app,
        "Car_history?$filter=speed%20gt%2015%20and%20id%20eq%20'urn:ngsi-ld:Car:7'",
    )
    .await;
    assert_eq!(
        rows(&fast),
        [(car(7), T2.to_owned(), json!(20.0))],
        "{fast}"
    );
    let (_, none) = get(
        &app,
        &format!("Car_history?$filter=observedAt%20ge%20{T3}%20and%20observedAt%20lt%20{T1}"),
    )
    .await;
    assert_eq!(rows(&none), []);
}

#[tokio::test(flavor = "multi_thread")]
async fn pages_count_sort_and_one_row_by_key() {
    let app = past().await.expect("fixture");
    let (_, whole) = get(&app, "Car_history").await;
    let (_, top) = get(&app, "Car_history?$top=4").await;
    assert_eq!(rows(&top), rows(&whole)[..4].to_vec());
    assert!(top.get("@nextLink").is_none(), "$top=4 ends at 4: {top}");
    let walk = |first: String, size: u32| {
        let app = app.clone();
        async move {
            let mut walked = Vec::new();
            let mut uri = Some(first);
            while let Some(u) = uri {
                let (status, page) = send_paged(&app, &u, size).await;
                assert_eq!(status, StatusCode::OK, "{page}");
                walked.extend(rows(&page));
                uri = page["@nextLink"]
                    .as_str()
                    .and_then(|l| l.split_once("/x/views"))
                    .map(|(_, p)| format!("/x/views{p}"));
            }
            walked
        }
    };
    assert_eq!(
        walk(format!("{PAST}/Car_history"), 2).await,
        rows(&whole),
        "pages of two walk the same rows in the same order"
    );
    assert_eq!(
        walk(format!("{PAST}/Car_history?$skip=1&$top=3"), 1).await,
        rows(&whole)[1..4].to_vec()
    );
    let (_, count) = get(&app, "Car_history/$count").await;
    assert_eq!(count, json!(5));
    let (_, counted) = get(&app, "Car_history?$count=true&$top=1").await;
    assert_eq!(counted["@count"], json!(5), "{counted}");
    let (_, sorted) = get(&app, "Car_history?$orderby=observedAt%20desc,id").await;
    let order: Vec<(String, String)> = rows(&sorted).into_iter().map(|r| (r.0, r.1)).collect();
    assert_eq!(
        order,
        [
            (car(8), T3.to_owned()),
            (car(7), T2.to_owned()),
            (car(7), T1.to_owned()),
            (car(8), T1.to_owned()),
            (car(7), T0.to_owned()),
        ]
    );
    let (status, one) = get(
        &app,
        &format!("Car_history(id='urn:ngsi-ld:Car:7',observedAt={T1})?$select=label"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{one}");
    assert_eq!(
        (&one["id"], &one["observedAt"], &one["label"]),
        (&json!(car(7)), &json!(T1), &json!("seven"))
    );
    assert!(one.get("speed").is_none(), "{one}");
    let (status, _) = get(
        &app,
        &format!("Car_history(id='urn:ngsi-ld:Car:7',observedAt={T3})"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the hidden ownerRef's time is no row"
    );
}

async fn send_paged(app: &axum::Router, uri: &str, size: u32) -> (StatusCode, Value) {
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let req = Request::builder()
        .uri(uri)
        .header("NGSILD-Tenant", "city")
        .header("OData-MaxVersion", "4.01")
        .header("Prefer", format!("odata.maxpagesize={size}"))
        .body(Body::empty())
        .unwrap_or_default();
    let Ok(resp) = app.clone().oneshot(req).await;
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_history_takes_no_write_and_exists_only_beside_the_temporal_query() {
    let app = past().await.expect("fixture");
    let before = stored(&app, "urn:ngsi-ld:Car:7").await;
    let row = format!("Car_history(id='urn:ngsi-ld:Car:7',observedAt={T0})");
    let body = json!({"speed": 1.0}).to_string();
    for (method, path) in [
        (Method::POST, "Car_history".to_owned()),
        (Method::PATCH, row.clone()),
        (Method::PUT, row.clone()),
        (Method::DELETE, row),
    ] {
        let (status, answer) =
            send(&app, method.clone(), &format!("{PAST}/{path}"), Some(&body)).await;
        assert_eq!(
            status,
            StatusCode::METHOD_NOT_ALLOWED,
            "{method} {path}: {answer}"
        );
    }
    assert_eq!(
        stored(&app, "urn:ngsi-ld:Car:7").await,
        before,
        "no write reached the Entity"
    );
    let (status, metadata) = send(
        &app,
        Method::GET,
        &format!("{PAST}/$metadata?$format=json"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = metadata.to_string();
    assert!(text.contains("Car_history"), "{text}");
    assert!(!text.contains("ownerRef"), "{text}");
    let (status, _) = send(
        &app,
        Method::GET,
        "/x/views/reads/odata/v4/Car_history",
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the reads view offers no Temporal Query"
    );
    let (status, _) = odata(&app, Method::GET, "Car_history", None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "nor does the cars view, which offers the writes"
    );
    let (_, metadata) = send(
        &app,
        Method::GET,
        "/x/views/reads/odata/v4/$metadata?$format=json",
        None,
    )
    .await;
    assert!(!metadata.to_string().contains("_history"), "{metadata}");
}

#[tokio::test(flavor = "multi_thread")]
async fn pages_cross_the_batches_the_temporal_api_is_read_in() {
    let app = past().await.expect("fixture");
    for n in 10..17u8 {
        let h = json!({"id": car(n), "type": format!("{CITY}Car"),
            format!("{CITY}speed"): [at(&json!(f64::from(n)), T0), at(&json!(1.0), T2)]});
        let (status, answer) = send(
            &app,
            Method::POST,
            "/ngsi-ld/v1/temporal/entities",
            Some(&h.to_string()),
        )
        .await;
        assert!(status.is_success(), "{answer}");
    }
    let (_, count) = get(&app, "Car_history/$count").await;
    assert_eq!(count, json!(19));
    let (_, whole) = get(&app, "Car_history?$count=true").await;
    assert_eq!(rows(&whole).len(), 19, "{whole}");
    let mut walked = Vec::new();
    let mut uri = Some(format!("{PAST}/Car_history?$skip=3"));
    while let Some(u) = uri {
        let (status, page) = send_paged(&app, &u, 5).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        walked.extend(rows(&page));
        uri = page["@nextLink"]
            .as_str()
            .and_then(|l| l.split_once("/x/views"))
            .map(|(_, p)| format!("/x/views{p}"));
    }
    assert_eq!(walked, rows(&whole)[3..].to_vec());
}

/// The status and the (id, observedAt) of each row of a read.
async fn read(app: &axum::Router, path: &str) -> (StatusCode, Vec<(String, String)>) {
    let (status, body) = get(app, path).await;
    let rows = rows(&body).into_iter().map(|r| (r.0, r.1)).collect();
    (status, rows)
}

fn key(n: u8, t: &str) -> (String, String) {
    (car(n), t.to_owned())
}

#[tokio::test(flavor = "multi_thread")]
async fn each_bound_on_observed_at_and_each_other_conjunct_narrows_the_rows() {
    let app = past().await.expect("fixture");
    let ok = StatusCode::OK;
    let cases = [
        (
            format!("observedAt%20ge%20{T2}"),
            vec![key(7, T2), key(8, T3)],
        ),
        (
            format!("observedAt%20lt%20{T1}"),
            vec![key(7, T0)],
        ),
        (
            format!("observedAt%20le%20{T1}"),
            vec![key(7, T0), key(7, T1), key(8, T1)],
        ),
        (
            format!("{T1}%20le%20observedAt%20and%20observedAt%20ne%20null%20and%20(speed%20gt%2065%20or%20speed%20lt%2015)"),
            vec![key(7, T1), key(8, T3)],
        ),
        (
            "id%20eq%20'urn:ngsi-ld:Car:7'%20and%20id%20eq%20'urn:ngsi-ld:Car:8'".to_owned(),
            vec![],
        ),
    ];
    for (filter, expected) in cases {
        let got = read(&app, &format!("Car_history?$filter={filter}")).await;
        assert_eq!(got, (ok, expected), "{filter}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn select_orderby_keys_and_what_a_history_does_not_serve() {
    let app = past().await.expect("fixture");
    let (status, body) = get(&app, "Car_history?$select=id,label").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let first = body["value"].get(1).cloned().unwrap_or_default();
    assert_eq!(
        first,
        json!({"id": car(7), "observedAt": T1, "label": "seven"}),
        "the key is always selected"
    );
    let (_, star) = get(&app, "Car_history?$select=*").await;
    assert_eq!(rows(&star).len(), 5);
    let (_, by_speed) = read(&app, "Car_history?$orderby=speed%20desc").await;
    assert_eq!(
        by_speed,
        [key(8, T3), key(8, T1), key(7, T2), key(7, T0), key(7, T1)],
        "equal speeds keep the natural order"
    );
    let (_, natural) = get(&app, "Car_history").await;
    for order in ["id", "id,observedAt"] {
        let (_, got) = get(&app, &format!("Car_history?$orderby={order}")).await;
        assert_eq!(rows(&got), rows(&natural), "{order}");
    }
    let row = format!("Car_history(id='urn:ngsi-ld:Car:7',observedAt={T1})");
    let (status, one) = get(&app, &row).await;
    assert_eq!(
        (status, &one["speed"]),
        (StatusCode::OK, &json!(10.0)),
        "{one}"
    );
    let (status, aliased) = get(
        &app,
        &format!("Car_history(id=@i,observedAt=@t)?@i='urn:ngsi-ld:Car:7'&@t={T1}"),
    )
    .await;
    assert_eq!(
        (status, &aliased["label"]),
        (StatusCode::OK, &json!("seven")),
        "{aliased}"
    );
    let refused = [
        ("Car_history/$ref".to_owned(), StatusCode::NOT_IMPLEMENTED),
        (
            "Car_history?$search=seven".to_owned(),
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            "Car_history?ngsild.q=speed%3E1".to_owned(),
            StatusCode::BAD_REQUEST,
        ),
        (
            "Car_history?$orderby=tolower(label)".to_owned(),
            StatusCode::BAD_REQUEST,
        ),
        (format!("{row}/speed"), StatusCode::NOT_IMPLEMENTED),
        (
            format!("{row}?$filter=speed%20gt%201"),
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            "Car_history('urn:ngsi-ld:Car:7')".to_owned(),
            StatusCode::NOT_FOUND,
        ),
    ];
    for (path, status) in refused {
        let (got, body) = get(&app, &path).await;
        assert_eq!(got, status, "{path}: {body}");
    }
    // Each 501 names its construct from a fixed text, never the request's value.
    let named = [
        ("Car_history/$ref".to_owned(), "$ref of a history set"),
        (
            "Car_history?$search=seven".to_owned(),
            "$expand, $search, $apply or $compute on a history set",
        ),
        (format!("{row}/speed"), "this path below a history set"),
        (
            format!("{row}?$filter=speed%20gt%201"),
            "a query option other than $select on one history row",
        ),
    ];
    for (path, construct) in named {
        let (got, body) = get(&app, &path).await;
        assert_eq!(
            (got, &body["error"]["message"]),
            (
                StatusCode::NOT_IMPLEMENTED,
                &json!(format!(
                    "{construct}: no NGSI-LD equivalent (ADR-0024 decision 5)"
                ))
            ),
            "{path}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_page_ends_short_past_its_reads_and_a_count_past_the_scan_is_refused() {
    let app = past().await.expect("fixture");
    let post = |h: Value| {
        let app = app.clone();
        async move {
            let (status, answer) = send(
                &app,
                Method::POST,
                "/ngsi-ld/v1/temporal/entities",
                Some(&h.to_string()),
            )
            .await;
            assert!(status.is_success(), "{answer}");
        }
    };
    for n in 100..=200u16 {
        post(
            json!({"id": format!("urn:ngsi-ld:Car:{n}"), "type": format!("{CITY}Car"),
            format!("{CITY}speed"): [at(&json!(f64::from(n)), T0)]}),
        )
        .await;
    }
    let (status, first) = send_paged(&app, &format!("{PAST}/Car_history"), 1000).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(
        rows(&first).len(),
        100,
        "25 reads of 4 Entities end the page"
    );
    let mut walked = rows(&first);
    let mut uri = first["@nextLink"]
        .as_str()
        .and_then(|l| l.split_once("/x/views"))
        .map(|(_, p)| format!("/x/views{p}"));
    while let Some(u) = uri {
        let (_, page) = send_paged(&app, &u, 1000).await;
        walked.extend(rows(&page));
        uri = page["@nextLink"]
            .as_str()
            .and_then(|l| l.split_once("/x/views"))
            .map(|(_, p)| format!("/x/views{p}"));
    }
    assert_eq!(walked.len(), 106);
    let (_, count) = get(&app, "Car_history/$count").await;
    assert_eq!(count, json!(106));
    let minute = |m: u32| format!("2026-10-02T{:02}:{:02}:00Z", m / 60, m % 60);
    for n in 300..310u16 {
        let speed: Vec<Value> = (0..1000).map(|m| at(&json!(1.0), &minute(m))).collect();
        post(
            json!({"id": format!("urn:ngsi-ld:Car:{n}"), "type": format!("{CITY}Car"),
            format!("{CITY}speed"): speed}),
        )
        .await;
    }
    let (status, refused) = get(&app, "Car_history/$count").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(
        refused["error"]["code"],
        json!("TooManyResults"),
        "{refused}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_count_whose_filter_keeps_no_row_stops_at_the_scan_bound() {
    let app = past().await.expect("fixture");
    let cars: Vec<Value> = (0..1000)
        .map(|n| {
            json!({"id": format!("urn:ngsi-ld:Car:s{n}"), "type": format!("{CITY}Car"),
                   format!("{CITY}speed"): at(&json!(1.0), T0)})
        })
        .collect();
    for chunk in cars.chunks(1000) {
        let body = Value::from(chunk.to_vec()).to_string();
        let (status, answer) = send(
            &app,
            Method::POST,
            "/ngsi-ld/v1/entityOperations/create",
            Some(&body),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{answer}");
    }
    // 1 002 Entities with a history, none faster than 1000: the count reads 1 000 and refuses
    let (status, refused) = get(&app, "Car_history/$count?$filter=speed%20gt%201000").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(
        refused["error"]["code"],
        json!("TooManyResults"),
        "{refused}"
    );
    let (status, one) = get(
        &app,
        "Car_history/$count?$filter=id%20eq%20'urn:ngsi-ld:Car:7'%20and%20speed%20gt%201000",
    )
    .await;
    assert_eq!((status, one), (StatusCode::OK, json!(0)));
}
