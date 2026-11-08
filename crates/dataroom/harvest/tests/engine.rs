//! The engine's own checks: a fetcher that ignores the byte cap, a redirect to an unusable
//! Location, a feed that never stops paging, a source URL that does not parse, and a record a
//! source keeps leaving out.

mod common;

use common::*;
use dataroom_core::ops::HarvestRunner;
use dataroom_core::store::BoxFuture;
use dataroom_core::{Result, Store};
use dataroom_harvest::{
    FetchResponse, HarvestEngine, MapFetcher, OutboundFetcher, ValidatedTarget,
};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A fetcher that hands back a fixed body whatever cap it is given, and records the limits.
struct Careless {
    body: String,
    seen: Mutex<Vec<(usize, Duration, String)>>,
}

impl OutboundFetcher for Careless {
    fn fetch_hop<'a>(
        &'a self,
        target: &'a ValidatedTarget,
        _accept: &'a str,
        max_bytes: usize,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<FetchResponse>> {
        self.seen
            .lock()
            .unwrap()
            .push((max_bytes, timeout, target.url.to_string()));
        Box::pin(std::future::ready(Ok(FetchResponse::Ok {
            content_type: None,
            body: self.body.clone(),
        })))
    }
}

#[tokio::test]
async fn the_engine_holds_its_byte_cap_even_when_a_fetcher_does_not() {
    let w = world(&[]).await;
    let src = source(
        "dcat-ap",
        "feed",
        "https://feeds.example.org/c.ttl",
        &["feeds.example.org"],
    );
    let fetcher = Arc::new(Careless {
        body: "x".repeat(65),
        seen: Mutex::default(),
    });
    let engine = HarvestEngine::new(fetcher.clone())
        .with_dns_resolution(false)
        .with_custom_dns(|_, port| Ok(vec![SocketAddr::from(([93, 184, 216, 34], port))]))
        .with_max_bytes(64)
        .with_timeout(Duration::from_millis(1500));
    let err = engine.run_source(&w.ctx, &w.store, &src).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("body size 65 exceeded cap of 64 bytes"),
        "{err}"
    );
    assert_eq!(
        fetcher.seen.lock().unwrap().as_slice(),
        [(
            64,
            Duration::from_millis(1500),
            "https://feeds.example.org/c.ttl".to_string()
        )]
    );
}

#[tokio::test]
async fn a_redirect_to_a_location_that_does_not_parse_fails() {
    let w = world(&[]).await;
    let url = "https://feeds.example.org/c.ttl";
    let src = source("dcat-ap", "feed", url, &["feeds.example.org"]);
    let f = MapFetcher::new();
    f.insert_redirect(url, 302, "https://[::1");
    let err = engine(&f)
        .run_source(&w.ctx, &w.store, &src)
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("invalid redirect Location 'https://[::1'"),
        "{err}"
    );
}

#[tokio::test]
async fn a_feed_that_pages_past_the_cap_withdraws_nothing() {
    let w = world(&[]).await;
    let base = "https://feeds.example.org/c.ttl";
    let src = source("dcat-ap", "feed", base, &["feeds.example.org"]);
    let f = MapFetcher::new();
    let page = |n: usize| {
        if n == 0 {
            base.to_string()
        } else {
            format!("{base}?page={n}")
        }
    };
    for n in 0..=50 {
        let iri = format!("https://feeds.example.org/ds/d{n}");
        f.insert(
            page(n),
            Some("text/turtle"),
            dcat_turtle(&[(iri.as_str(), "Dataset")], Some(&page(n + 1))),
        );
    }
    let err = engine(&f)
        .run_source(&w.ctx, &w.store, &src)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("still pages after 50 pages"),
        "{err}"
    );
    assert!(active_names(&w).await.is_empty());
}

#[tokio::test]
async fn a_source_url_that_does_not_parse_is_refused() {
    let w = world(&[]).await;
    for url in ["not a url", "data:text/plain,hello"] {
        let src = source("dcat-ap", "feed", url, &["feeds.example.org"]);
        let err = engine(&MapFetcher::new())
            .run_source(&w.ctx, &w.store, &src)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("refused") || err.to_string().contains("valid URL"),
            "{url}: {err}"
        );
    }
}

#[tokio::test]
async fn a_record_already_withdrawn_is_not_withdrawn_again() {
    let w = world(&[]).await;
    let url = "https://feeds.example.org/c.ttl";
    let src = source("dcat-ap", "feed", url, &["feeds.example.org"]);
    let f = MapFetcher::new();
    let air = ("https://feeds.example.org/ds/air", "Ovzdušie");
    let noise = ("https://feeds.example.org/ds/noise", "Hluk");
    f.insert(url, Some("text/turtle"), dcat_turtle(&[air, noise], None));
    let e = engine(&f);
    e.run_source(&w.ctx, &w.store, &src).await.unwrap();
    f.insert(url, Some("text/turtle"), dcat_turtle(&[air], None));
    let run = e.run_source(&w.ctx, &w.store, &src).await.unwrap();
    assert_eq!(run.withdrawn_count, 1);
    let run = e.run_source(&w.ctx, &w.store, &src).await.unwrap();
    assert_eq!((run.withdrawn_count, run.unchanged_count), (0, 1));
}

#[tokio::test]
async fn records_from_the_legacy_harvester_are_recognised_by_their_extras() {
    let w = world(&[]).await;
    let url = "https://feeds.example.org/c.ttl";
    let src = source("dcat-ap", "feed", url, &["feeds.example.org"]);
    let air = "https://feeds.example.org/ds/air";
    // Records the CKAN harvester left: no harvest fields, only `harvest_source` and an item key
    // in `source_id`, `source_uri` or nothing (then the name).
    for (id, name, key) in [
        ("old-1", "air", Some(("source_id", air))),
        (
            "old-2",
            "water",
            Some(("source_uri", "https://feeds.example.org/ds/water")),
        ),
        ("old-3", "noise", None),
    ] {
        let mut ds = local_dataset(id, name, name);
        ds.extras.insert("harvest_source".into(), "feed".into());
        if let Some((k, v)) = key {
            ds.extras.insert(k.into(), v.into());
        }
        w.store.upsert_dataset(&w.ctx, ds).await.unwrap();
    }
    let f = MapFetcher::new();
    f.insert(
        url,
        Some("text/turtle"),
        dcat_turtle(&[(air, "Ovzdušie")], None),
    );
    let run = engine(&f).run_source(&w.ctx, &w.store, &src).await.unwrap();
    assert!(run.skipped.is_empty(), "{:?}", run.skipped);
    assert_eq!(
        (run.updated_count, run.created_count, run.withdrawn_count),
        (1, 0, 2)
    );
    let kept = w.store.get_dataset(&w.ctx, "air").await.unwrap().unwrap();
    assert_eq!(kept.id, "old-1", "the old record is updated in place");
    assert_eq!(active_names(&w).await, ["air"]);
}
