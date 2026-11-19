//! A portal's uploaded files on its hosts (T-7417): a file answers only on its own catalogue's
//! host, an Organization-wide file on every portal of that Organization, never on another
//! Organization's host or path; an SVG or a document downloads, a raster image shows inline.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use dataroom_api::{router, DataroomState};
use dataroom_core::files::FileStore;
use dataroom_core::{CatalogueId, OrgId, PolicyAuthorizer, Store};
use dataroom_store_memory::MemoryStore;
use http_body_util::BodyExt;
use std::collections::BTreeMap;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

fn org(n: u128) -> OrgId {
    OrgId::from_uuid(Uuid::from_u128(n))
}

fn cat(n: u128) -> CatalogueId {
    CatalogueId::from_uuid(Uuid::from_u128(n))
}

async fn catalogue(store: &dyn Store, org: OrgId, id: CatalogueId, name: &str, host: &str) {
    let cat: dataroom_core::Catalogue = serde_json::from_value(serde_json::json!({
        "orgId": org, "id": id, "name": name, "host": host,
        "title": {"en": name}, "description": {}, "defaultLanguage": "en",
        "languages": ["en"], "mainProfile": "dcat-ap-3.0.1", "activeProfiles": [],
        "modified": "2026-11-19T00:00:00Z",
    }))
    .expect("catalogue");
    store.upsert_catalogue(cat).await.expect("upsert");
}

struct Answer {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Vec<u8>,
}

async fn get(state: &DataroomState, host: &str, path: &str) -> Answer {
    let req = Request::get(path)
        .header(header::HOST, host)
        .body(Body::empty())
        .unwrap();
    let resp = router(state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    Answer {
        status,
        headers,
        body,
    }
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
const SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg"><rect width="1" height="1"/></svg>"#;

#[tokio::test]
async fn a_file_answers_on_its_own_organizations_hosts_only() {
    let (a, b) = (org(0xA), org(0xB));
    let (a_own, a_mobility, b_own) = (cat(0xA0), cat(0xA1), cat(0xB0));
    let store = Arc::new(MemoryStore::new());
    catalogue(store.as_ref(), a, a_own, "default", "opendata.a.test").await;
    catalogue(store.as_ref(), a, a_mobility, "mobility", "mobility.a.test").await;
    catalogue(store.as_ref(), b, b_own, "default", "opendata.b.test").await;
    let root = std::env::temp_dir().join(format!("jc-files-served-{}", Uuid::now_v7()));
    let files = Arc::new(FileStore::new(&root));
    let logo = files.put((a, a_own), "logo.png", PNG, "ana").unwrap();
    let table = files
        .put(
            (a, a_mobility),
            "linky.csv",
            b"linka;smer\n1;centrum\n",
            "ana",
        )
        .unwrap();
    let icon = files.put((a, a_mobility), "icon.svg", SVG, "ana").unwrap();
    let state = DataroomState::new(store, Arc::new(PolicyAuthorizer::default()), a, a_own)
        .with_organization_catalogues(BTreeMap::from([
            ("a".to_owned(), (a, a_own)),
            ("b".to_owned(), (b, b_own)),
        ]))
        .with_files(files);

    // The Organization-wide logo, on both of A's portals, inline and immutable.
    for host in ["opendata.a.test", "mobility.a.test"] {
        let got = get(&state, host, &logo.path()).await;
        assert_eq!(got.status, StatusCode::OK, "{host}");
        assert_eq!(got.body, PNG);
        assert_eq!(got.headers[header::CONTENT_TYPE], "image/png");
        assert_eq!(
            got.headers[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        assert_eq!(got.headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert!(got.headers[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .starts_with("inline"));
    }
    // The mobility portal's own file on its host, as an attachment; not on A's other portal.
    let csv = get(&state, "mobility.a.test", &table.path()).await;
    assert_eq!(csv.status, StatusCode::OK);
    assert!(csv.headers[header::CONTENT_DISPOSITION]
        .to_str()
        .unwrap()
        .starts_with("attachment"));
    assert_eq!(
        get(&state, "opendata.a.test", &table.path()).await.status,
        StatusCode::NOT_FOUND
    );
    // An SVG never renders on the portal's origin: attachment, sandboxed.
    let svg = get(&state, "mobility.a.test", &icon.path()).await;
    assert!(svg.headers[header::CONTENT_DISPOSITION]
        .to_str()
        .unwrap()
        .starts_with("attachment"));
    assert!(svg.headers[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap()
        .contains("sandbox"));
    // Organization B reads none of A's files, by its host or by any name in the path.
    for path in [
        logo.path(),
        table.path(),
        format!("/files/{}/x.png", logo.sha256),
    ] {
        assert_eq!(
            get(&state, "opendata.b.test", &path).await.status,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    // A path that is no hash reads nothing.
    assert_eq!(
        get(&state, "opendata.a.test", "/files/..%2Findex.json/x")
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    let _ = std::fs::remove_dir_all(root);
}
