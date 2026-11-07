// SPDX-License-Identifier: EUPL-1.2
//! Part 1 Core: landing page, conformance, HTTP (mapping rows C1, C3, C4, C9, C21, C22).

use crate::common::{fixture, get, href, send};
use axum::http::StatusCode;
use serde_json::json;

const ROOT: &str = "/x/views/city/ogc/features";

#[tokio::test(flavor = "multi_thread")]
async fn landing_page_links() -> Result<(), String> {
    let f = fixture().await?;
    for root in [ROOT.to_owned(), format!("{ROOT}/")] {
        let a = get(&f.app, &root).await;
        assert_eq!(a.status, StatusCode::OK, "{root}: {}", a.body);
        assert_eq!(
            a.headers
                .get("content-type")
                .map(axum::http::HeaderValue::as_bytes),
            Some(&b"application/json"[..])
        );
        for (rel, tail) in [
            ("self", ""),
            ("service-desc", "/api"),
            ("conformance", "/conformance"),
            ("data", "/collections"),
        ] {
            assert_eq!(
                href(&a.body, rel),
                Some(format!("http://localhost:9090{ROOT}{tail}")),
                "{rel}"
            );
        }
        let links = a
            .body
            .get("links")
            .and_then(|l| l.as_array())
            .cloned()
            .unwrap_or_default();
        assert!(
            links.iter().all(|l| l.get("type").is_some()),
            "every link has a type (Req 29)"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn conformance_lists_what_is_served() -> Result<(), String> {
    let f = fixture().await?;
    let a = get(&f.app, &format!("{ROOT}/conformance")).await;
    assert_eq!(a.status, StatusCode::OK);
    let read_only: Vec<&str> = antares_ogc::CONFORMANCE
        .iter()
        .copied()
        .filter(|c| !c.contains("ogcapi-features-4"))
        .collect();
    assert_eq!(
        a.body,
        json!({"conformsTo": read_only}),
        "a view that offers no write declares no Part 4 class (W4)"
    );
    // `/functions` lists none, so CQL2 `functions` is not declared (T-5204).
    assert!(!antares_ogc::CONFORMANCE
        .iter()
        .any(|c| c.ends_with("/conf/functions")));
    let fleet = get(&f.app, "/x/views/fleet/ogc/features/conformance").await;
    assert_eq!(
        fleet.body,
        json!({"conformsTo": antares_ogc::CONFORMANCE}),
        "the fleet view offers all four writes"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn head_is_the_get_without_its_body() -> Result<(), String> {
    let f = fixture().await?;
    for path in [
        "",
        "/conformance",
        "/api",
        "/collections",
        "/collections/Car",
        "/collections/Car/items",
        "/collections/Car/items/urn:ngsi-ld:Car:1",
        "/collections/Nope",
    ] {
        let uri = format!("{ROOT}{path}");
        let got = get(&f.app, &uri).await;
        let head = send(&f.app, "HEAD", &uri, "", &[]).await;
        assert_eq!(head.status, got.status, "{uri}");
        assert_eq!(head.bytes, 0, "{uri}");
        let lengths: Vec<_> = head.headers.get_all("content-length").iter().collect();
        assert_eq!(lengths.len(), 1, "{uri}: one Content-Length");
        assert_eq!(
            lengths.first().and_then(|v| v.to_str().ok()),
            Some(got.bytes.to_string().as_str()),
            "{uri}"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_and_invalid_parameters_are_400() -> Result<(), String> {
    let f = fixture().await?;
    for uri in [
        format!("{ROOT}?f=xml"),
        format!("{ROOT}/conformance?x=1"),
        format!("{ROOT}/collections?limit=1"),
        format!("{ROOT}/collections/Car?bbox=0,0,1,1"),
        format!("{ROOT}/collections/Car/items/urn:ngsi-ld:Car:1?limit=1"),
        // `speed=10` is a queryable parameter (Part 3, T-3745); an unknown name is not.
        format!("{ROOT}/collections/Car/items?wheels=10"),
        format!("{ROOT}/collections/Car/items?speed=fast"),
        format!("{ROOT}/collections/Car/items?crs=http://www.opengis.net/def/crs/EPSG/0/4326"),
        format!("{ROOT}/collections/Car/items?limit=0"),
        format!("{ROOT}/collections/Car/items?limit=10001"),
        format!("{ROOT}/collections/Car/items?bbox=1,2,3"),
        format!("{ROOT}/collections/Car/items?datetime=yesterday"),
        format!("{ROOT}/collections/Car/items?limit=1&limit=2"),
        format!("{ROOT}/collections/Car/items?next=garbage"),
        format!("{ROOT}/collections/Car/schema?f=html"),
        format!("{ROOT}/collections/Car/sortables?limit=1"),
        format!("{ROOT}/collections/Car/items/urn:ngsi-ld:Car:1?sortby=speed"),
        format!("{ROOT}/collections/Car/items?sortby=wheels"),
        format!("{ROOT}/collections/Car/items?properties=wheels"),
        format!("{ROOT}/collections/Car/items?profile=rel-as-none"),
    ] {
        let a = get(&f.app, &uri).await;
        assert_eq!(a.status, StatusCode::BAD_REQUEST, "{uri}: {}", a.body);
        assert_eq!(
            a.headers
                .get("content-type")
                .map(axum::http::HeaderValue::as_bytes),
            Some(&b"application/problem+json"[..]),
            "{uri}"
        );
        assert_eq!(a.body.get("status"), Some(&json!(400)), "{uri}");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn html_only_accept_is_406() -> Result<(), String> {
    let f = fixture().await?;
    let a = send(&f.app, "GET", ROOT, "", &[("Accept", "text/html")]).await;
    assert_eq!(a.status, StatusCode::NOT_ACCEPTABLE, "{}", a.body);
    let a = send(
        &f.app,
        "GET",
        ROOT,
        "",
        &[("Accept", "text/html, application/json;q=0.5")],
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "JSON among the ranges is served");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn gml_only_accept_is_406() -> Result<(), String> {
    let f = fixture().await?;
    let a = send(
        &f.app,
        "GET",
        &format!("{ROOT}/collections/Car/items"),
        "",
        &[("Accept", "application/gml+xml")],
    )
    .await;
    assert_eq!(a.status, StatusCode::NOT_ACCEPTABLE, "{}", a.body);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_view_a_view_without_schema_and_an_unknown_path_are_404() -> Result<(), String> {
    let f = fixture().await?;
    for uri in [
        "/x/views/nope/ogc/features".to_owned(),
        format!("{ROOT}/elsewhere"),
        format!("{ROOT}/collections/Car/items/a/b"),
    ] {
        assert_eq!(
            get(&f.app, &uri).await.status,
            StatusCode::NOT_FOUND,
            "{uri}"
        );
    }
    let other = json!({"id": "plain", "tenant": "other", "selector": ["Car"], "rules": [{"types": ["Car"], "attributes": ["speed"]}], "operations": ["query_entities"]});
    assert_eq!(
        send(&f.app, "PUT", "/q/views/plain", &other.to_string(), &[])
            .await
            .status,
        StatusCode::CREATED
    );
    assert_eq!(
        get(&f.app, "/x/views/plain/ogc/features").await.status,
        StatusCode::NOT_FOUND,
        "the Tenant has no schema"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_write_the_view_does_not_offer_is_405_with_allow() -> Result<(), String> {
    let f = fixture().await?;
    for method in ["POST", "PUT", "PATCH", "DELETE"] {
        let a = send(
            &f.app,
            method,
            &format!("{ROOT}/collections/Car/items"),
            "{}",
            &[],
        )
        .await;
        assert_eq!(a.status, StatusCode::METHOD_NOT_ALLOWED, "{method}");
        assert_eq!(
            a.headers
                .get("allow")
                .map(axum::http::HeaderValue::as_bytes),
            Some(&b"GET, HEAD, OPTIONS"[..]),
            "{method}"
        );
    }
    Ok(())
}

/// T-5204: `f=json` and `f=geojson` choose the JSON this surface writes, whatever `Accept` says.
#[tokio::test(flavor = "multi_thread")]
async fn f_json_is_the_json_encoding() -> Result<(), String> {
    let f = fixture().await?;
    for uri in [
        format!("{ROOT}?f=json"),
        format!("{ROOT}/collections?f=json"),
        format!("{ROOT}/collections/Car/items?f=geojson&limit=1"),
        format!("{ROOT}/collections/Car/schema?f=json"),
    ] {
        let a = send(&f.app, "GET", &uri, "", &[("Accept", "text/html")]).await;
        assert_eq!(a.status, StatusCode::OK, "{uri}: {}", a.body);
    }
    Ok(())
}
