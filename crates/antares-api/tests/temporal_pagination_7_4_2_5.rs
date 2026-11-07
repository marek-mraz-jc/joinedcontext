// SPDX-License-Identifier: EUPL-1.2
//! Temporal pagination (TS 104 175 7.4.2.5, TS 104 176 6.4.7.3, T-3456):
//! "Temporal pagination can be done in ascending order using the firstN
//! parameter and in descending order using the lastN parameter … offsetN is
//! used to select the respective page in the chosen order." The pages are
//! pointed at by `Link` values with rel `intervalafter` (ascending) and
//! `intervalbefore` (descending), each carrying the request's media type.
//!
//! Every case runs on the memory store and, with the `postgres` feature and
//! `ANTARES_TEST_DATABASE_URL` set, on the PostgreSQL store, where the
//! per-Attribute cap is pushed into SQL.

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

const ID: &str = "urn:ngsi-ld:Vehicle:pages";
const LONG: &str = "urn:ngsi-ld:Vehicle:pages-long";

fn at(i: usize) -> String {
    format!("2020-01-01T00:{i:02}:00Z")
}

/// One `speed` instance per minute, `0..n`.
fn evolution(id: &str, n: usize) -> Value {
    let speed: Vec<Value> = (0..n)
        .map(|i| json!({"type": "Property", "value": i, "observedAt": at(i)}))
        .collect();
    json!({"id": id, "type": "Vehicle", "speed": speed})
}

struct Broker {
    st: AppState,
    tenant: Option<String>,
}

impl Broker {
    async fn send(
        &self,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value, Vec<String>) {
        let mut b = Request::builder().method(method).uri(uri);
        if let Some(t) = &self.tenant {
            b = b.header("NGSILD-Tenant", t);
        }
        let req = match body {
            Some(v) => {
                let payload = v.to_string();
                b.header("Content-Type", "application/json")
                    .header("Content-Length", payload.len())
                    .body(Body::from(payload))
                    .expect("request")
            }
            None => b.body(Body::empty()).expect("request"),
        };
        let res = antares_api::router(self.st.clone())
            .oneshot(req)
            .await
            .expect("response");
        let status = res.status();
        let links = res
            .headers()
            .get_all("Link")
            .iter()
            .filter_map(|v| v.to_str().ok().map(str::to_owned))
            .filter(|l| l.contains("rel=\"interval"))
            .collect();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .expect("body");
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            links,
        )
    }

    async fn seed(&self) {
        for doc in [evolution(ID, 6), evolution(LONG, 20)] {
            let (code, body, _) = self
                .send("POST", "/ngsi-ld/v1/temporal/entities", Some(doc))
                .await;
            assert_eq!(code, StatusCode::CREATED, "{body}");
        }
    }

    /// The `observedAt` of every `speed` instance served for `id`, in the
    /// order the body carries them.
    async fn served(&self, id: &str, query: &str) -> Vec<String> {
        let (code, body, _) = self
            .send(
                "GET",
                &format!("/ngsi-ld/v1/temporal/entities/{id}?{query}"),
                None,
            )
            .await;
        assert!(code.is_success(), "{query}: {code} {body}");
        instants(&body)
    }
}

fn instants(entity: &Value) -> Vec<String> {
    entity["speed"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| i["observedAt"].as_str().map(str::to_owned))
        .collect()
}

/// The memory broker, and the PostgreSQL one when the run names a database.
async fn brokers() -> Vec<Broker> {
    let mut out = vec![Broker {
        st: AppState::new("me".into()),
        tenant: None,
    }];
    #[cfg(feature = "postgres")]
    match std::env::var("ANTARES_TEST_DATABASE_URL") {
        Ok(url) => {
            let pool = antares_sql::store::pg::connect(&url, 5)
                .await
                .expect("connect");
            let store = antares_sql::store::any::AnyStore::Pg(
                antares_sql::store::any::PgBackend::new(pool),
            );
            out.push(Broker {
                st: AppState::with_store("me".into(), std::sync::Arc::new(store), "postgres"),
                tenant: Some(format!(
                    "tp{}",
                    &uuid::Uuid::new_v4().simple().to_string()[..12]
                )),
            });
        }
        Err(_) => eprintln!("SKIP pg: ANTARES_TEST_DATABASE_URL not set"),
    }
    for b in &mut out {
        // the ETSI brokers' 6.3.10 ceiling (T-4800): these cases page 20 instances
        b.st.temporal_instance_limit = 9;
        b.seed().await;
    }
    out
}

fn mins(ms: &[usize]) -> Vec<String> {
    ms.iter().map(|m| at(*m)).collect()
}

/// 6.4.7.3: "where the parameter firstN is present the pagination shall
/// happen in ascending order".
#[tokio::test(flavor = "multi_thread")]
async fn first_n_returns_the_oldest_n_instances_ascending() {
    for b in brokers().await {
        assert_eq!(b.served(ID, "firstN=2").await, mins(&[0, 1]));
        assert_eq!(b.served(ID, "lastN=2").await, mins(&[5, 4]));
        // a window first, then the page inside it
        assert_eq!(
            b.served(ID, &format!("firstN=2&timerel=after&timeAt={}", at(3)))
                .await,
            mins(&[3, 4])
        );
    }
}

/// 7.4.2.5: offsetN "is used to select the respective page in the chosen
/// order": lastN=2&offsetN=2 is the third and fourth newest.
#[tokio::test(flavor = "multi_thread")]
async fn offset_n_skips_instances_in_the_chosen_order() {
    for b in brokers().await {
        assert_eq!(b.served(ID, "lastN=2&offsetN=2").await, mins(&[3, 2]));
        assert_eq!(b.served(ID, "firstN=2&offsetN=2").await, mins(&[2, 3]));
        assert_eq!(b.served(ID, "firstN=2&offsetN=0").await, mins(&[0, 1]));
        // neither firstN nor lastN: ascending, from the offset
        assert_eq!(b.served(ID, "offsetN=4").await, mins(&[4, 5]));
    }
}

/// Table 6.4.7.3-2: "If offsetN is set to a value larger than the result
/// set, the offsetN should be assumed to be equal to the size of the result
/// set, i.e. only the last element of the result set is to be returned if
/// there are any results."
#[tokio::test(flavor = "multi_thread")]
async fn offset_n_past_the_end_returns_the_last_instance() {
    for b in brokers().await {
        assert_eq!(b.served(ID, "firstN=2&offsetN=50").await, mins(&[5]));
        assert_eq!(b.served(ID, "lastN=2&offsetN=6").await, mins(&[0]));
    }
}

fn link_with(links: &[String], rel: &str) -> Option<String> {
    links
        .iter()
        .find(|l| l.contains(&format!("rel=\"{rel}\"")))
        .cloned()
}

/// The target of a Link value, `<…>`.
fn target(link: &str) -> &str {
    link.split_once('<')
        .and_then(|(_, r)| r.split_once('>'))
        .map_or("", |(t, _)| t)
}

/// 6.4.7.3: the next page (ascending) is `intervalafter`, the previous one
/// `intervalbefore`, each with the request's media type, and following the
/// link serves exactly the page after.
#[tokio::test(flavor = "multi_thread")]
async fn temporal_truncation_emits_intervalafter_link() {
    for b in brokers().await {
        // the broker's own ceiling cut a request naming no page size
        let (code, body, links) = b
            .send(
                "GET",
                &format!("/ngsi-ld/v1/temporal/entities/{LONG}"),
                None,
            )
            .await;
        assert!(code.is_success(), "{code} {body}");
        let first = instants(&body);
        let next = link_with(&links, "intervalafter").expect("an intervalafter link");
        assert!(next.contains("type=\"application/json\""), "{next}");
        assert!(link_with(&links, "intervalbefore").is_none(), "{links:?}");
        let (_, body, links) = b.send("GET", target(&next), None).await;
        let second = instants(&body);
        assert_eq!(second.first(), Some(&at(first.len())), "{next}");
        assert!(link_with(&links, "intervalbefore").is_some(), "{links:?}");

        // a page the caller sized
        let (_, _, links) = b
            .send(
                "GET",
                &format!("/ngsi-ld/v1/temporal/entities/{ID}?firstN=2&offsetN=2"),
                None,
            )
            .await;
        let next = link_with(&links, "intervalafter").expect("next page");
        assert!(target(&next).contains("offsetN=4"), "{next}");
        let prev = link_with(&links, "intervalbefore").expect("previous page");
        assert!(target(&prev).contains("offsetN=0"), "{prev}");

        // lastN pages backwards: the next page is the older one
        let (_, _, links) = b
            .send(
                "GET",
                &format!("/ngsi-ld/v1/temporal/entities/{ID}?lastN=2"),
                None,
            )
            .await;
        let older = link_with(&links, "intervalbefore").expect("older page");
        assert!(target(&older).contains("offsetN=2"), "{older}");
        assert!(target(&older).contains("lastN=2"), "{older}");
        assert!(link_with(&links, "intervalafter").is_none(), "{links:?}");

        // the last page links to nothing further
        let (_, _, links) = b
            .send(
                "GET",
                &format!("/ngsi-ld/v1/temporal/entities/{ID}?firstN=6"),
                None,
            )
            .await;
        assert!(links.is_empty(), "{links:?}");

        // the query surface links the same way, with its own path
        let (code, _, links) = b
            .send("GET", "/ngsi-ld/v1/temporal/entities?type=Vehicle&firstN=2&timerel=after&timeAt=2019-01-01T00:00:00Z", None)
            .await;
        assert!(code.is_success());
        let next = link_with(&links, "intervalafter").expect("query next page");
        assert!(
            target(&next).starts_with("/ngsi-ld/v1/temporal/entities?"),
            "{next}"
        );
        assert!(target(&next).contains("offsetN=2"), "{next}");
    }
}

/// firstN asks for ascending order and lastN for descending; one page
/// cannot be both, so the pair is refused like any other invalid value.
#[tokio::test(flavor = "multi_thread")]
async fn first_n_and_last_n_together_is_400() {
    let b = Broker {
        st: AppState::new("me".into()),
        tenant: None,
    };
    b.seed().await;
    for q in [
        "firstN=2&lastN=2",
        "firstN=0",
        "firstN=x",
        "offsetN=-1",
        "offsetN=x",
        "offsetN=99999999999999999999",
    ] {
        let (code, body, _) = b
            .send(
                "GET",
                &format!("/ngsi-ld/v1/temporal/entities/{ID}?{q}"),
                None,
            )
            .await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "{q}: {body}");
        assert_eq!(body["title"], "BadRequestData", "{q}: {body}");
    }
}

/// The JSON `temporalQ` of a Query carries firstN and offsetN too (5.2.5:
/// both are TemporalParams scoped within temporalQ).
#[tokio::test(flavor = "multi_thread")]
async fn temporal_q_carries_first_n_and_offset_n() {
    for b in brokers().await {
        let query = |mut tq: Value| {
            tq["timerel"] = json!("after");
            tq["timeAt"] = json!("2019-01-01T00:00:00Z");
            json!({"type": "Query", "entities": [{"id": ID, "type": "Vehicle"}], "temporalQ": tq})
        };
        let (code, body, _) = b
            .send(
                "POST",
                "/ngsi-ld/v1/temporal/entityOperations/query",
                Some(query(json!({"firstN": 2, "offsetN": 1}))),
            )
            .await;
        assert_eq!(code, StatusCode::OK, "{body}");
        assert_eq!(instants(&body[0]), mins(&[1, 2]));
        for bad in [
            json!({"firstN": 2, "lastN": 2}),
            json!({"firstN": 0}),
            json!({"offsetN": -1}),
            json!({"offsetN": "1"}),
        ] {
            let (code, body, _) = b
                .send(
                    "POST",
                    "/ngsi-ld/v1/temporal/entityOperations/query",
                    Some(query(bad.clone())),
                )
                .await;
            assert_eq!(code, StatusCode::BAD_REQUEST, "{bad}: {body}");
            assert_eq!(body["title"], "BadRequestData", "{bad}: {body}");
        }
    }
}

/// Every instant served per (Entity, Attribute) following `rel` Links from
/// `uri` to the last page, and how many pages that took.
async fn follow(
    b: &Broker,
    uri: &str,
    rel: &str,
) -> (
    std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
    usize,
) {
    let mut seen = std::collections::BTreeMap::<String, std::collections::BTreeSet<String>>::new();
    let mut next = Some(uri.to_owned());
    let mut pages = 0;
    while let Some(uri) = next {
        pages += 1;
        assert!(pages <= 40, "the Links do not end: {uri}");
        let (code, body, links) = b.send("GET", &uri, None).await;
        assert!(code.is_success(), "{uri}: {code} {body}");
        let docs = match body {
            Value::Array(docs) => docs,
            doc => vec![doc],
        };
        for doc in &docs {
            for (attr, instances) in doc.as_object().into_iter().flatten() {
                for t in instances.as_array().into_iter().flatten() {
                    if let Some(t) = t["observedAt"].as_str() {
                        seen.entry(format!("{} {attr}", doc["id"].as_str().unwrap_or("")))
                            .or_default()
                            .insert(t.to_owned());
                    }
                }
            }
        }
        next = link_with(&links, rel).map(|l| target(&l).to_owned());
    }
    (seen, pages)
}

/// T-3613: the ceiling cuts every Attribute at one time boundary (T-3612),
/// so an Attribute sparser than the boundary one serves fewer instances.
/// Following the next Links to the end still serves every instance of every
/// Attribute, ascending and descending, on one Entity and on a query: the
/// offsetN Link moves by the fewest instances a cut Attribute served, and
/// when one served none it continues by time from the boundary.
#[tokio::test(flavor = "multi_thread")]
async fn following_the_links_serves_every_instance_of_every_attribute() {
    let series = |minutes: &mut dyn Iterator<Item = usize>| -> Vec<Value> {
        minutes
            .map(|i| json!({"type": "Property", "value": i, "observedAt": at(i)}))
            .collect()
    };
    // speed every minute, fuel every other, late only after the first
    // boundary: cut to nothing on the first ascending page
    let sparse = json!({
        "id": "urn:ngsi-ld:Car:sparse", "type": "Car",
        "speed": series(&mut (0..20)),
        "fuel": series(&mut (1..40).step_by(2)),
    });
    let late = json!({
        "id": "urn:ngsi-ld:Car:late", "type": "Car",
        "speed": series(&mut (0..20)),
        "late": series(&mut (30..36)),
    });
    for b in brokers().await {
        let mut want = std::collections::BTreeMap::new();
        for doc in [&sparse, &late] {
            let (code, body, _) = b
                .send("POST", "/ngsi-ld/v1/temporal/entities", Some(doc.clone()))
                .await;
            assert_eq!(code, StatusCode::CREATED, "{body}");
            for (attr, instances) in doc.as_object().into_iter().flatten() {
                for t in instances.as_array().into_iter().flatten() {
                    want.entry(format!("{} {attr}", doc["id"].as_str().unwrap_or("")))
                        .or_insert_with(std::collections::BTreeSet::new)
                        .insert(t["observedAt"].as_str().unwrap_or("").to_owned());
                }
            }
        }
        let single = |id: &str| {
            want.iter()
                .filter(|(k, _)| k.starts_with(id))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<std::collections::BTreeMap<_, _>>()
        };
        for id in ["urn:ngsi-ld:Car:sparse", "urn:ngsi-ld:Car:late"] {
            for (query, rel) in [("", "intervalafter"), ("lastN=50", "intervalbefore")] {
                let uri = format!("/ngsi-ld/v1/temporal/entities/{id}?{query}");
                let (seen, pages) = follow(&b, &uri, rel).await;
                assert_eq!(seen, single(id), "{uri}");
                assert!(pages > 2, "{uri}: the ceiling never cut, {pages} page(s)");
            }
        }
        for (query, rel) in [("", "intervalafter"), ("&lastN=50", "intervalbefore")] {
            let uri = format!(
                "/ngsi-ld/v1/temporal/entities?type=Car&timerel=after&timeAt=1970-01-01T00:00:00Z{query}"
            );
            let (seen, _) = follow(&b, &uri, rel).await;
            assert_eq!(seen, want, "{uri}");
        }
    }
}

/// F-ANT1-017: a temporal query with no firstN/lastN is cut by the broker's
/// default trim (206, the oldest instances first) on both stores alike, and
/// the PostgreSQL store, given the cap the API now passes, reconstructs at
/// most the ceiling plus one instance per Attribute, not the whole history.
#[tokio::test(flavor = "multi_thread")]
async fn a_query_without_n_is_cut_alike_and_the_store_reads_the_cap() {
    for b in brokers().await {
        let (code, body, _) = b
            .send(
                "GET",
                "/ngsi-ld/v1/temporal/entities?type=Vehicle&timerel=after&timeAt=2019-01-01T00:00:00Z",
                None,
            )
            .await;
        assert_eq!(code, StatusCode::PARTIAL_CONTENT, "{body}");
        let long = body
            .as_array()
            .into_iter()
            .flatten()
            .find(|e| e["id"] == LONG)
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(instants(&long), mins(&[0, 1, 2, 3, 4, 5, 6, 7, 8]));
        let Some(tenant) = &b.tenant else {
            continue;
        };
        let tenant = antares_model::TenantId::new(tenant).expect("tenant");
        let ids = [LONG];
        let filter = antares_store::filter::TemporalFilter {
            ids: Some(&ids),
            last_n: Some(10),
            oldest_first: true,
            timeproperty: "observedAt",
            ..Default::default()
        };
        let read =
            b.st.temporal
                .query_temporal(&tenant, &filter)
                .await
                .expect("query");
        let most = read
            .rows
            .iter()
            .filter_map(Value::as_object)
            .flat_map(|o| o.values())
            .filter_map(Value::as_array)
            .map(Vec::len)
            .max();
        assert_eq!(most, Some(10), "{:?}", read.rows);
    }
}
