// SPDX-License-Identifier: EUPL-1.2
//! T-4477, EP-08, EP-41: `items` as CSV, `/collections` as a ZIP of a CSV per collection
//! (no Excel, owner 2026-10-18), read back and held against the `GeoJSON` answer
//! of the same request. One type per table: a CSV or Parquet of several collections is refused.
#![cfg(feature = "tabular")]

use crate::common::{fixture, fixture_with, get, href, ids, schemas_on, Fixture};
use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use std::io::{Cursor, Read};
use tower::ServiceExt;

const CARS: &str = "/x/views/city/ogc/features/collections/Car/items";
const ALL: &str = "/x/views/city/ogc/features/collections";

type Outcome = Result<(), Box<dyn std::error::Error>>;

/// The status, the headers and the raw bytes of a `GET`.
async fn raw(f: &Fixture, uri: &str, accept: Option<&str>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut req = Request::builder().uri(uri);
    if let Some(accept) = accept {
        req = req.header("Accept", accept);
    }
    let Ok(resp) = f
        .app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap_or_default())
        .await;
    let (status, headers) = (resp.status(), resp.headers().clone());
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(|c| c.to_bytes().to_vec())
        .unwrap_or_default();
    (status, headers, bytes)
}

fn header<'h>(headers: &'h HeaderMap, name: &str) -> Option<&'h str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// One member of a ZIP container as text.
fn member(bytes: &[u8], name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut text = String::new();
    archive.by_name(name)?.read_to_string(&mut text)?;
    Ok(text)
}

fn members(bytes: &[u8]) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut names = archive
        .file_names()
        .map(|name| name.map(std::borrow::Cow::into_owned))
        .collect::<Result<Vec<String>, _>>()?;
    names.sort();
    Ok(names)
}

/// The ids of a CSV: the first field of every row below the header.
fn csv_ids(csv: &str) -> Vec<String> {
    csv.split("\r\n")
        .skip(1)
        .filter(|line| !line.is_empty())
        .map(|line| line.split(',').next().unwrap_or_default().to_owned())
        .collect()
}

#[tokio::test]
async fn a_csv_holds_the_rows_the_geojson_answer_holds() -> Outcome {
    let f = fixture().await?;
    for query in [
        "",
        "bbox=170,-10,180,10",
        "filter=speed%20%3E%2020",
        "properties=speed",
    ] {
        let geojson = get(&f.app, &format!("{CARS}?limit=1000&{query}")).await;
        let (status, headers, bytes) = raw(&f, &format!("{CARS}?f=csv&{query}"), None).await;
        let csv = String::from_utf8(bytes)?;
        assert_eq!(status, StatusCode::OK, "{query}: {csv}");
        assert_eq!(
            header(&headers, "content-type"),
            Some("text/csv; charset=utf-8; header=present")
        );
        assert_eq!(
            header(&headers, "content-disposition"),
            Some("attachment; filename=\"Car.csv\"")
        );
        assert_eq!(csv_ids(&csv), ids(&geojson.body), "{query}");
        assert!(csv.starts_with("id,geometry,"), "{csv}");
        assert!(
            !csv.contains("secret") && !csv.contains("s1"),
            "the view hides it: {csv}"
        );
    }
    let (status, _, bytes) = raw(&f, CARS, Some("text/csv")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an Accept naming CSV asks for it too"
    );
    let csv = String::from_utf8(bytes)?;
    let header_row = csv.split("\r\n").next().unwrap_or_default();
    assert!(
        header_row.contains("speed") && header_row.contains("tags"),
        "{header_row}"
    );
    assert!(
        csv.contains("urn:ngsi-ld:Car:1,\"{\"\"coordinates\"\":[17.1,48.1]"),
        "{csv}"
    );
    Ok(())
}

/// Owner 2026-10-18 (T-4597): no Excel. `f=xlsx` is an unknown parameter on items and on
/// `/collections`, and an `Accept` naming the spreadsheet type gets no workbook.
#[tokio::test]
async fn no_excel_is_written() -> Outcome {
    let f = fixture().await?;
    for uri in [format!("{CARS}?f=xlsx"), format!("{ALL}?f=xlsx")] {
        let (status, _, bytes) = raw(&f, &uri, None).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{uri}: {}",
            String::from_utf8_lossy(&bytes)
        );
    }
    let xlsx = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
    let (status, headers, _) = raw(&f, CARS, Some(xlsx)).await;
    assert_ne!(header(&headers, "content-type"), Some(xlsx), "{status}");
    Ok(())
}

#[tokio::test]
async fn every_collection_is_a_csv_of_one_bundle() -> Outcome {
    let f = fixture().await?;
    let (status, headers, bytes) = raw(&f, &format!("{ALL}?f=zip"), None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    assert_eq!(header(&headers, "content-type"), Some("application/zip"));
    assert_eq!(
        header(&headers, "content-disposition"),
        Some("attachment; filename=\"city.zip\"")
    );
    let mut want = vec!["Car.csv", "README.md", "Stop.csv", "provenance.json"];
    if cfg!(feature = "parquet") {
        want.extend(["Car.parquet", "Stop.parquet"]);
        want.sort_unstable();
    }
    assert_eq!(members(&bytes)?, want);
    let cars = get(&f.app, &format!("{CARS}?limit=1000")).await;
    assert_eq!(csv_ids(&member(&bytes, "Car.csv")?), ids(&cars.body));
    assert_eq!(
        csv_ids(&member(&bytes, "Stop.csv")?),
        ["urn:ngsi-ld:Stop:1"]
    );
    let provenance: Value = serde_json::from_str(&member(&bytes, "provenance.json")?)?;
    assert_eq!(provenance.pointer("/view"), Some(&Value::from("city")));
    assert_eq!(
        provenance.pointer("/collections"),
        Some(&Value::from("Car,Stop"))
    );
    assert_eq!(provenance.pointer("/rows"), Some(&Value::from("5")));
    let readme = member(&bytes, "README.md")?;
    assert!(
        readme.starts_with("# city\n") && readme.contains("| `Stop.csv` | the collection as CSV |"),
        "{readme}"
    );
    // bbox narrows every collection; a Feature without a geometry is kept (Part 1 Req 24 C).
    let (status, _, bytes) = raw(
        &f,
        &format!("{ALL}?bbox=17,48,18,49&collections=Car"),
        Some("application/zip"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let names = members(&bytes)?;
    assert!(
        names.contains(&"Car.csv".to_owned()) && !names.contains(&"Stop.csv".to_owned()),
        "{names:?}"
    );
    assert_eq!(
        csv_ids(&member(&bytes, "Car.csv")?),
        ["urn:ngsi-ld:Car:1", "urn:ngsi-ld:Car:3"]
    );
    let provenance: Value = serde_json::from_str(&member(&bytes, "provenance.json")?)?;
    assert_eq!(
        provenance.pointer("/query"),
        Some(&Value::from("bbox=17,48,18,49&collections=Car"))
    );
    Ok(())
}

#[tokio::test]
async fn several_collections_are_never_one_table() -> Outcome {
    let f = fixture().await?;
    for one in ["csv", "parquet"] {
        let refused = get(&f.app, &format!("{ALL}?f={one}")).await;
        assert_eq!(
            refused.status,
            StatusCode::BAD_REQUEST,
            "{one}: {}",
            refused.body
        );
        let detail = refused
            .body
            .pointer("/detail")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            detail.contains("f=zip") && !detail.contains("xlsx"),
            "{detail}"
        );
    }
    for (query, why) in [
        (
            "f=zip&collections=Depot",
            "a hidden collection is an unknown one",
        ),
        ("f=zip&collections=Nope", "an unknown collection"),
        (
            "f=zip&limit=5",
            "a parameter several collections do not take",
        ),
        ("f=zip&bbox=1,2", "a broken bbox"),
        ("f=zip&datetime=..%2F..", "a datetime open at both ends"),
    ] {
        let refused = get(&f.app, &format!("{ALL}?{query}")).await;
        assert_eq!(
            refused.status,
            StatusCode::BAD_REQUEST,
            "{why}: {}",
            refused.body
        );
    }
    let page = get(&f.app, &format!("{CARS}?limit=2")).await;
    let links = page
        .body
        .pointer("/links")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let alternates: Vec<&str> = links
        .iter()
        .filter(|l| l.pointer("/rel").and_then(Value::as_str) == Some("alternate"))
        .filter_map(|l| l.pointer("/href").and_then(Value::as_str))
        .collect();
    assert!(
        alternates
            .iter()
            .any(|h| h.ends_with("/items?limit=2&f=csv")),
        "{alternates:?}"
    );
    assert!(
        !alternates.iter().any(|h| h.contains("xlsx")),
        "{alternates:?}"
    );
    assert!(href(&page.body, "alternate").is_some());
    Ok(())
}

#[tokio::test]
async fn a_file_past_the_answer_ceiling_is_refused_whole() -> Outcome {
    schemas_on();
    let f = fixture_with(TestBroker::with_max_answer_bytes(300).await?).await?;
    for uri in [format!("{CARS}?f=csv"), format!("{ALL}?f=zip")] {
        let (status, headers, bytes) = raw(&f, &uri, None).await;
        assert_eq!(
            status,
            StatusCode::PAYLOAD_TOO_LARGE,
            "{uri}: {}",
            String::from_utf8_lossy(&bytes)
        );
        assert_eq!(
            header(&headers, "content-type"),
            Some("application/problem+json"),
            "{uri}"
        );
    }
    Ok(())
}

/// T-5100: a bundle without `collections` holds the current state only; a `{type}-history`
/// reads the type's whole history and joins the bundle only when named.
#[tokio::test(flavor = "multi_thread")]
async fn a_history_joins_a_bundle_only_when_named() -> Outcome {
    let f = crate::history::past(crate::history::fresh().await?).await?;
    let all = "/x/views/past/ogc/features/collections";
    let (status, _, bytes) = raw(&f, &format!("{all}?f=zip"), None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let names = members(&bytes)?;
    assert!(
        names.contains(&"Car.csv".to_owned()) && !names.iter().any(|n| n.contains("-history")),
        "{names:?}"
    );
    let (status, _, bytes) = raw(&f, &format!("{all}?f=zip&collections=Car-history"), None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let names = members(&bytes)?;
    assert!(
        names.contains(&"Car-history.csv".to_owned()) && !names.contains(&"Car.csv".to_owned()),
        "{names:?}"
    );
    assert_eq!(
        csv_ids(&member(&bytes, "Car-history.csv")?),
        [
            "urn:ngsi-ld:Car:1@2026-10-01T10:00:00Z",
            "urn:ngsi-ld:Car:1@2026-10-01T10:05:00Z",
            "urn:ngsi-ld:Car:1@2026-10-01T10:10:00Z",
            "urn:ngsi-ld:Car:2@2026-10-01T10:05:00Z",
            "urn:ngsi-ld:Car:4@2026-10-01T10:00:00Z"
        ]
    );
    Ok(())
}
