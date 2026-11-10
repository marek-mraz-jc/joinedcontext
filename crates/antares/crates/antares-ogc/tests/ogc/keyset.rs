// SPDX-License-Identifier: EUPL-1.2
//! `items` pages and files read by the store's keyset cursor (T-4907): a `next` token carries
//! the cursor of the read at its offset, so a deep page seeks past the page before instead of
//! re-reading every row it skips. Every Feature comes once and in order while rows are written
//! between pages, the Policy narrows each page, and a token of another view or query, or one
//! whose cursor was altered, is refused. A file dropped while it is written frees its place.

use crate::common::{fixture_with, get, href, ids, local, schemas_on, send, Fixture, SlowOnly};
use antares_ops::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ops::test_kit::{q_filter, TestBroker};
use axum::http::StatusCode;
use serde_json::{json, Value};
use std::sync::Arc;

const CARS: &str = "/x/views/city/ogc/features/collections/Car/items";

/// The fixture over a broker answering two Entities per store read, narrowed to `speed<50`
/// by its Policy, with Cars 5 to 9 added (Car 6 fast).
async fn keyset_fixture(engine: Arc<dyn PolicyEngine>) -> Result<Fixture, String> {
    schemas_on();
    let f = fixture_with(TestBroker::with_max_limit(2).await?.with_policy(engine)).await?;
    for (n, speed) in [(5, 5), (6, 80), (7, 7), (8, 8), (9, 9)] {
        write_car(&f, &n.to_string(), speed).await?;
    }
    Ok(f)
}

async fn write_car(f: &Fixture, n: &str, speed: i64) -> Result<(), String> {
    let car = json!({"id": format!("urn:ngsi-ld:Car:{n}"), "type": "Car",
                     "speed": {"type": "Property", "value": speed}});
    let a = send(
        &f.app,
        "POST",
        "/ngsi-ld/v1/entities",
        &car.to_string(),
        &[("NGSILD-Tenant", "city")],
    )
    .await;
    (a.status == StatusCode::CREATED)
        .then_some(())
        .ok_or_else(|| format!("car {n}: {} {}", a.status, a.body))
}

async fn delete_car(f: &Fixture, n: &str) -> Result<(), String> {
    let a = send(
        &f.app,
        "DELETE",
        &format!("/ngsi-ld/v1/entities/urn:ngsi-ld:Car:{n}"),
        "",
        &[("NGSILD-Tenant", "city")],
    )
    .await;
    (a.status == StatusCode::NO_CONTENT)
        .then_some(())
        .ok_or_else(|| format!("delete {n}: {} {}", a.status, a.body))
}

/// The ids of one page and the path of its `next` link.
async fn page(f: &Fixture, uri: &str) -> Result<(Vec<String>, Option<String>), String> {
    let a = get(&f.app, uri).await;
    if a.status != StatusCode::OK {
        return Err(format!("{uri}: {} {}", a.status, a.body));
    }
    Ok((ids(&a.body), href(&a.body, "next").map(|h| local(&h))))
}

/// The `next` token of a link path.
fn token(path: &str) -> &str {
    path.split(['?', '&'])
        .find_map(|kv| kv.strip_prefix("next="))
        .unwrap_or_default()
}

fn car(n: &str) -> String {
    format!("urn:ngsi-ld:Car:{n}")
}

/// Pages of two follow the cursor: Cars 1 and 3 are deleted and Cars 0 and 85 written after
/// page 2, and page 3 still starts at Car 8 (an offset would skip it), Car 85 comes once in
/// its place and Car 0, before the cursor, never; the fast Car 6 is on no page.
#[tokio::test(flavor = "multi_thread")]
async fn pages_follow_the_cursor_while_rows_are_written() -> Result<(), String> {
    let f = keyset_fixture(Arc::new(SlowOnly(q_filter("speed<50")?))).await?;
    let (first, next) = page(&f, &format!("{CARS}?limit=2")).await?;
    let mut uri = next.ok_or("page 1 has a next link")?;
    let (second, next) = page(&f, &uri).await?;
    assert_eq!([first, second].concat(), [car("1"), car("3"), car("5"), car("7")]);
    uri = next.ok_or("page 2 has a next link")?;
    // part.offset.digest and the store's cursor, which holds a dot of its own
    assert_eq!(token(&uri).split('.').count(), 5, "{uri}");
    delete_car(&f, "1").await?;
    delete_car(&f, "3").await?;
    write_car(&f, "0", 1).await?;
    write_car(&f, "85", 1).await?;
    let mut seen = Vec::new();
    for _ in 0..10 {
        let (ids, next) = page(&f, &uri).await?;
        seen.extend(ids);
        match next {
            Some(next) => uri = next,
            None => break,
        }
    }
    assert_eq!(seen, [car("8"), car("85"), car("9")]);
    Ok(())
}

/// A token is refused on another view and under another query, and with its cursor altered;
/// one read with another page size stays a page of the same answer.
#[tokio::test(flavor = "multi_thread")]
async fn a_token_of_another_view_query_or_cursor_is_refused() -> Result<(), String> {
    let f = keyset_fixture(Arc::new(SlowOnly(q_filter("speed<50")?))).await?;
    let (_, next) = page(&f, &format!("{CARS}?limit=2")).await?;
    let (_, next) = page(&f, &next.ok_or("page 1 has a next link")?).await?;
    let uri = next.ok_or("page 2 has a next link")?;
    let t = token(&uri).to_owned();
    assert_eq!(t.split('.').count(), 5, "{t}");
    let slow = "/x/views/slow/ogc/features/collections/Car/items";
    let tag_flipped = {
        let mut chars: Vec<char> = t.chars().collect();
        if let Some(last) = chars.last_mut() {
            *last = if *last == '0' { '1' } else { '0' };
        }
        chars.into_iter().collect::<String>()
    };
    // the cursor names Car 7: name Car 1 instead, the tag unchanged
    let (head, rest) = t.rsplit_once('-').ok_or("a cursor")?;
    let (_, tag) = rest.split_once('.').ok_or("a cursor tag")?;
    let other_id = format!("{head}-{}.{tag}", hex(&car("1")));
    for (what, refused) in [
        ("another view", format!("{slow}?limit=2&next={t}")),
        ("another query", format!("{CARS}?limit=2&filter=speed%3C40&next={t}")),
        ("a cursor tag altered", format!("{CARS}?limit=2&next={tag_flipped}")),
        ("a cursor id altered", format!("{CARS}?limit=2&next={other_id}")),
        ("no cursor text", format!("{CARS}?limit=2&next={t}x")),
    ] {
        let a = get(&f.app, &refused).await;
        assert_eq!(a.status, StatusCode::BAD_REQUEST, "{what}: {}", a.body);
        assert!(ids(&a.body).is_empty(), "{what}: {}", a.body);
    }
    let (ids, _) = page(&f, &format!("{CARS}?limit=3&next={t}")).await?;
    assert_eq!(ids, [car("8"), car("9")], "another page size");
    Ok(())
}

fn hex(text: &str) -> String {
    text.bytes().map(|b| format!("{b:02x}")).collect()
}

/// A file read in store reads of two holds each Feature the Policy shows, once and in order.
#[cfg(feature = "tabular")]
#[tokio::test(flavor = "multi_thread")]
async fn a_file_read_by_cursor_holds_every_row_once() -> Result<(), String> {
    let f = keyset_fixture(Arc::new(SlowOnly(q_filter("speed<50")?))).await?;
    let (status, _, bytes) = raw(&f, &format!("{CARS}?f=csv")).await;
    assert_eq!(status, StatusCode::OK);
    let csv = String::from_utf8(bytes).map_err(|e| e.to_string())?;
    let rows: Vec<&str> = csv
        .split("\r\n")
        .skip(1)
        .filter(|l| !l.is_empty())
        .filter_map(|l| l.split(',').next())
        .collect();
    let want: Vec<String> = ["1", "3", "5", "7", "8", "9"].map(car).to_vec();
    assert_eq!(rows, want);
    Ok(())
}

#[cfg(feature = "tabular")]
async fn raw(f: &Fixture, uri: &str) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap_or_default();
    let Ok(resp) = f.app.clone().oneshot(req).await;
    let (status, headers) = (resp.status(), resp.headers().clone());
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(|c| c.to_bytes().to_vec())
        .unwrap_or_default();
    (status, headers, bytes)
}

/// Once holding, every query (clause 5.7) waits and never answers: a file being written stays
/// in its store read until the request is dropped.
#[derive(Default)]
struct Hold(std::sync::atomic::AtomicBool);

impl PolicyEngine for Hold {
    fn name(&self) -> &'static str {
        "hold"
    }

    fn decide<'a>(&'a self, _: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        if self.0.load(std::sync::atomic::Ordering::SeqCst) && op.clause.starts_with("5.7.") {
            return Box::pin(std::future::pending());
        }
        Box::pin(std::future::ready(Decision::Allow))
    }

    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

/// The two files a router writes at once (`ANTARES_EXPORT_FILES` unset) are dropped mid-read,
/// as a caller cut at the gateway's deadline is: each frees its place, and the next file is
/// written instead of refused 503.
#[cfg(feature = "tabular")]
#[tokio::test(flavor = "multi_thread")]
async fn a_file_dropped_while_written_frees_its_place() -> Result<(), String> {
    use std::sync::atomic::Ordering::SeqCst;
    let engine = Arc::new(Hold::default());
    let f = keyset_fixture(engine.clone()).await?;
    engine.0.store(true, SeqCst);
    let uri = format!("{CARS}?f=csv");
    for _ in 0..2 {
        let held = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            raw(&f, &uri),
        )
        .await;
        assert!(held.is_err(), "the file waits in its store read");
    }
    engine.0.store(false, SeqCst);
    let (status, _, _) = raw(&f, &uri).await;
    assert_eq!(status, StatusCode::OK, "both places were freed");
    Ok(())
}
