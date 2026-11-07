#![forbid(unsafe_code)]

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use chrono::Utc;
use dataroom_api::{router, DataroomState, TokenBinding};
use dataroom_core::ops::{all_operations, execute_operation};
use dataroom_core::{
    Action, Catalogue, CatalogueId, ContactKind, ContactPoint, Dataset, Distribution, GrantRule,
    HarvestSource, HarvestSourceKind, OrgId, PolicyAuthorizer, ProfilePin, Publisher, RecordState,
    RequestCtx, Store, Subject, ThemeTokens, Visibility,
};
use dataroom_harvest::{HarvestEngine, MapFetcher};
use dataroom_store_memory::MemoryStore;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

async fn send_json(
    app: &axum::Router,
    method: Method,
    uri: &str,
    host: &str,
    token: Option<&str>,
    accept_profile: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, host);
    if let Some(tok) = token {
        builder = builder.header(header::AUTHORIZATION, tok);
    }
    if let Some(prof) = accept_profile {
        builder = builder.header("Accept-Profile", prof);
    }
    let req = if let Some(b) = body {
        builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(b.to_string()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, headers, String::from_utf8_lossy(&bytes).into_owned())
}

fn make_catalogue(org_id: OrgId, cat_id: CatalogueId, name: &str, host: &str) -> Catalogue {
    Catalogue {
        org_id,
        id: cat_id,
        name: name.into(),
        host: host.into(),
        title: BTreeMap::from([
            ("sk".into(), format!("Katalóg {name}")),
            ("cs".into(), format!("Katalog {name}")),
            ("en".into(), format!("Catalogue {name}")),
        ]),
        description: BTreeMap::from([
            ("sk".into(), format!("Popis {name}")),
            ("cs".into(), format!("Popis {name}")),
            ("en".into(), format!("Description {name}")),
        ]),
        homepage: Some(format!("https://{host}")),
        default_language: "sk".into(),
        languages: vec!["sk".into(), "cs".into(), "en".into()],
        main_profile: "dcat-ap-3.0.1".into(),
        active_profiles: vec![
            ProfilePin {
                profile_id: "dcat-ap-3.0.1".into(),
                version: "3.0.1".into(),
            },
            ProfilePin {
                profile_id: "dcat-ap-cz".into(),
                version: "2026-09-23".into(),
            },
            ProfilePin {
                profile_id: "dcat-ap-sk-2.0".into(),
                version: "2.0.1".into(),
            },
        ],
        default_licence: Some("cc-by".into()),
        publisher_uri: Some("https://data.gov.sk/id/legal-subject/00313271".into()),
        publisher_name: Some(BTreeMap::from([
            ("sk".into(), "Mesto".into()),
            ("en".into(), "City".into()),
        ])),
        contact_point: Some(ContactPoint {
            uri: None,
            kind: ContactKind::Organization,
            name: BTreeMap::from([
                ("sk".into(), "Oddelenie otvorených dát".into()),
                ("cs".into(), "Oddělení otevřených dat".into()),
                ("en".into(), "Open Data Desk".into()),
            ]),
            email: "opendata@city.example".into(),
            url: None,
            identifier: None,
        }),
        theme: ThemeTokens::default(),
        allow_cross_org_harvest: true,
        allowed_outbound_hosts: vec!["external.example.org".into()],
        applicable_legislation: Vec::new(),
        modified: Utc::now(),
    }
}

#[tokio::test]
async fn ckan_action_api_answers_all_19_calls_with_jcctl_field_set() {
    let org_id = OrgId::from_uuid(Uuid::from_u128(100));
    let cat_id = CatalogueId::from_uuid(Uuid::from_u128(1000));
    let store = Arc::new(MemoryStore::new());
    store
        .upsert_catalogue(make_catalogue(
            org_id,
            cat_id,
            "opendata",
            "opendata.city.example",
        ))
        .await
        .unwrap();

    let authz = Arc::new(PolicyAuthorizer::new(vec![GrantRule {
        org_id,
        catalogue_id: Some(cat_id),
        principal: "jcctl-publisher".into(),
        actions: vec![
            Action::Read,
            Action::Propose,
            Action::Approve,
            Action::Delete,
            Action::Admin,
        ],
        publisher_id: None,
    }]));

    let state = DataroomState::new(store.clone(), authz, org_id, cat_id).with_tokens(vec![
        TokenBinding {
            token: "secret-jcctl-token".into(),
            subject: Subject::authenticated("jcctl-publisher", vec![org_id], Vec::new()),
        },
    ]);
    let app = router(state);
    let host = "opendata.city.example";
    let tok = Some("secret-jcctl-token");

    // 1. status_show
    let (st, _, body) = send_json(
        &app,
        Method::GET,
        "/api/3/action/status_show",
        host,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["success"], true);
    assert!(v["result"]["ckan_version"].is_string());

    // 2. license_list
    let (st, _, body) = send_json(
        &app,
        Method::GET,
        "/api/3/action/license_list",
        host,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert!(v["result"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l["id"] == "cc-by"));

    // 3. organization_show on missing -> 404
    let (st, _, _) = send_json(
        &app,
        Method::GET,
        "/api/3/action/organization_show?id=helsinki",
        host,
        tok,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // 4. organization_create
    let (st, _, body) = send_json(
        &app,
        Method::POST,
        "/api/3/action/organization_create",
        host,
        tok,
        None,
        Some(json!({ "name": "helsinki", "title": "City of Helsinki" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["result"]["name"], "helsinki");
    assert_eq!(v["result"]["title"], "City of Helsinki");

    // 5. organization_patch
    let (st, _, body) = send_json(
        &app,
        Method::POST,
        "/api/3/action/organization_patch",
        host,
        tok,
        None,
        Some(json!({ "id": "helsinki", "title": "Helsingin kaupunki" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["result"]["title"], "Helsingin kaupunki");

    // 6. organization_show
    let (st, _, body) = send_json(
        &app,
        Method::GET,
        "/api/3/action/organization_show?id=helsinki",
        host,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["result"]["title"], "Helsingin kaupunki");

    // 7. organization_list
    let (st, _, body) = send_json(
        &app,
        Method::GET,
        "/api/3/action/organization_list",
        host,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["result"], json!(["helsinki"]));

    // 8. package_create (with the exact fields jcctl-publish writes)
    let pkg_payload = json!({
        "name": "helsinki-transport",
        "owner_org": "helsinki",
        "title": "Helsinki Transport",
        "notes": "Live public transport entities",
        "url": "https://api.city.example/api/endpoint/transport/",
        "private": false,
        "license_id": "cc-by",
        "tags": [{ "name": "transport" }, { "name": "gtfs" }],
        "resources": [
            {
                "name": "NGSI-LD API",
                "url": "https://api.city.example/api/endpoint/transport/ngsi-ld/v1/",
                "format": "NGSI-LD",
                "mimetype": "application/json",
                "description": "The NGSI-LD API of this dataset"
            },
            {
                "name": "CSV",
                "url": "https://api.city.example/api/endpoint/transport/file.csv",
                "format": "CSV",
                "mimetype": "text/csv",
                "hash": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "hash_algorithm": "sha256"
            }
        ],
        "extras": [
            { "key": "endpoint", "value": "https://api.city.example/api/endpoint/transport/" },
            { "key": "generated_by", "value": "jcctl/ckan-publisher" },
            { "key": "theme", "value": "[\"http://publications.europa.eu/resource/authority/data-theme/TRAN\"]" },
            { "key": "frequency", "value": "http://publications.europa.eu/resource/authority/frequency/CONT" },
            { "key": "spatial_uri", "value": "http://data.europa.eu/nuts/code/FI1B1" },
            { "key": "contact_name", "value": "Transport Desk" },
            { "key": "contact_email", "value": "transport@hel.fi" }
        ]
    });
    let (st, _, body) = send_json(
        &app,
        Method::POST,
        "/api/3/action/package_create",
        host,
        tok,
        None,
        Some(pkg_payload),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let created_pkg: Value = serde_json::from_str::<Value>(&body).unwrap()["result"].clone();
    assert_eq!(created_pkg["name"], "helsinki-transport");
    assert_eq!(created_pkg["organization"]["name"], "helsinki");
    assert_eq!(created_pkg["resources"].as_array().unwrap().len(), 2);
    assert_eq!(created_pkg["resources"][0]["hash"], "");
    let first_res_id = created_pkg["resources"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // 9. package_show
    let (st, _, body) = send_json(
        &app,
        Method::GET,
        "/api/3/action/package_show?id=helsinki-transport",
        host,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let shown_pkg: Value = serde_json::from_str::<Value>(&body).unwrap()["result"].clone();
    assert_eq!(shown_pkg["id"], created_pkg["id"]);
    assert_eq!(shown_pkg["license_id"], "cc-by");
    assert!(shown_pkg["metadata_modified"].is_string());

    // 10. package_list
    let (st, _, body) = send_json(
        &app,
        Method::GET,
        "/api/3/action/package_list",
        host,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["result"], json!(["helsinki-transport"]));

    // 11. package_search
    let (st, _, body) = send_json(
        &app,
        Method::POST,
        "/api/3/action/package_search",
        host,
        None,
        None,
        Some(json!({
            "q": "transport",
            "fq": "organization:helsinki",
            "rows": 20,
            "start": 0,
            "facet.field": ["organization", "res_format"]
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let search_res: Value = serde_json::from_str::<Value>(&body).unwrap()["result"].clone();
    assert_eq!(search_res["count"], 1);
    assert_eq!(search_res["results"][0]["name"], "helsinki-transport");
    assert_eq!(search_res["facets"]["organization"]["helsinki"], 1);

    // 12. package_update & 13. package_patch
    let mut updated_payload = shown_pkg.clone();
    updated_payload["notes"] = json!("Updated description");
    let (st, _, body) = send_json(
        &app,
        Method::POST,
        "/api/3/action/package_update",
        host,
        tok,
        None,
        Some(updated_payload),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["result"]["notes"],
        "Updated description"
    );

    let (st, _, body) = send_json(
        &app,
        Method::POST,
        "/api/3/action/package_patch",
        host,
        tok,
        None,
        Some(json!({ "id": "helsinki-transport", "version": "1.2.0" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["result"]["version"],
        "1.2.0"
    );

    // 14. resource_patch & 15. resource_show
    let (st, _, body) = send_json(
        &app,
        Method::POST,
        "/api/3/action/resource_patch",
        host,
        tok,
        None,
        Some(json!({ "id": first_res_id, "description": "Patched resource description" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["result"]["description"],
        "Patched resource description"
    );

    let (st, _, _) = send_json(
        &app,
        Method::GET,
        &format!("/api/3/action/resource_show?id={first_res_id}"),
        host,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // 16. resource_view_create, 17. resource_view_list, 18. resource_view_update, 19. resource_view_delete
    let (st, _, body) = send_json(
        &app,
        Method::POST,
        "/api/3/action/resource_view_create",
        host,
        tok,
        None,
        Some(json!({
            "resource_id": first_res_id,
            "view_type": "jc_visualization",
            "title": "Speed over time",
            "description": "Line chart",
            "jc_name": "auto-vehicle-speed",
            "spec": "{\"mark\":\"line\"}",
            "source": "{\"type\":\"temporal\"}",
            "endpoint": "transport"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let view_id = serde_json::from_str::<Value>(&body).unwrap()["result"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // Undeclared extra field on jc_visualization view must be rejected (EP-144)
    let (st_bad, _, _) = send_json(
        &app,
        Method::POST,
        "/api/3/action/resource_view_create",
        host,
        tok,
        None,
        Some(json!({
            "resource_id": first_res_id,
            "view_type": "jc_visualization",
            "title": "Bad",
            "unexpected_field": 123
        })),
    )
    .await;
    assert_eq!(st_bad, StatusCode::UNPROCESSABLE_ENTITY);

    let (st, _, body) = send_json(
        &app,
        Method::GET,
        &format!("/api/3/action/resource_view_list?id={first_res_id}"),
        host,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["result"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let (st, _, body) = send_json(
        &app,
        Method::POST,
        "/api/3/action/resource_view_update",
        host,
        tok,
        None,
        Some(json!({
            "id": view_id,
            "resource_id": first_res_id,
            "view_type": "jc_visualization",
            "title": "Updated speed chart",
            "description": "Line chart",
            "jc_name": "auto-vehicle-speed",
            "spec": "{\"mark\":\"line\"}",
            "source": "{\"type\":\"temporal\"}",
            "endpoint": "transport"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["result"]["title"],
        "Updated speed chart"
    );

    let (st, _, _) = send_json(
        &app,
        Method::POST,
        "/api/3/action/resource_view_delete",
        host,
        tok,
        None,
        Some(json!({ "id": view_id })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // resource_delete
    let second_res_id = shown_pkg["resources"][1]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (st, _, _) = send_json(
        &app,
        Method::POST,
        "/api/3/action/resource_delete",
        host,
        tok,
        None,
        Some(json!({ "id": second_res_id })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // package_delete (withdraws dataset)
    let (st, _, _) = send_json(
        &app,
        Method::POST,
        "/api/3/action/package_delete",
        host,
        tok,
        None,
        Some(json!({ "id": "helsinki-transport" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // datastore_search_sql is explicitly refused as Not Supported (501, C3)
    let (st, _, body) = send_json(
        &app,
        Method::POST,
        "/api/3/action/datastore_search_sql",
        host,
        tok,
        None,
        Some(json!({ "sql": "SELECT 1" })),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_IMPLEMENTED, "{body}");
}

#[tokio::test]
async fn two_publishers_in_one_catalogue_with_separate_editor_grants_and_merge() {
    let org_id = OrgId::from_uuid(Uuid::from_u128(200));
    let cat_id = CatalogueId::from_uuid(Uuid::from_u128(2000));
    let store = Arc::new(MemoryStore::new());
    let host = "data.banskabystrica.sk";
    store
        .upsert_catalogue(make_catalogue(org_id, cat_id, "bb", host))
        .await
        .unwrap();

    let ctx_admin = RequestCtx::new(
        org_id,
        cat_id,
        Subject::authenticated("admin", vec![org_id], Vec::new()),
    );

    // Publisher 1: mesto-bb (grants editor_mesto)
    store
        .upsert_publisher(
            &ctx_admin,
            Publisher {
                org_id,
                catalogue_id: cat_id,
                id: "pub-mesto".into(),
                name: "mesto-bb".into(),
                title: BTreeMap::from([("sk".into(), "Mesto Banská Bystrica".into())]),
                description: BTreeMap::new(),
                uri: "https://data.gov.sk/id/legal-subject/00313271".into(),
                publisher_type: None,
                identifier: Some("00313271".into()),
                email: None,
                homepage: None,
                logo_url: None,
                contact_point: None,
                state: RecordState::Active,
                merged_into: None,
                editor_grants: vec!["editor_mesto".into()],
                created: Utc::now(),
                modified: Utc::now(),
            },
        )
        .await
        .unwrap();

    // Publisher 2: dopravny-podnik (grants editor_dp)
    store
        .upsert_publisher(
            &ctx_admin,
            Publisher {
                org_id,
                catalogue_id: cat_id,
                id: "pub-dp".into(),
                name: "dopravny-podnik".into(),
                title: BTreeMap::from([("sk".into(), "Dopravný podnik mesta BB".into())]),
                description: BTreeMap::new(),
                uri: "https://data.gov.sk/id/legal-subject/36019224".into(),
                publisher_type: None,
                identifier: Some("36019224".into()),
                email: None,
                homepage: None,
                logo_url: None,
                contact_point: None,
                state: RecordState::Active,
                merged_into: None,
                editor_grants: vec!["editor_dp".into()],
                created: Utc::now(),
                modified: Utc::now(),
            },
        )
        .await
        .unwrap();

    let authz = Arc::new(PolicyAuthorizer::default());
    let state = DataroomState::new(store.clone(), authz, org_id, cat_id).with_tokens(vec![
        TokenBinding {
            token: "tok-mesto".into(),
            subject: Subject::authenticated("editor_mesto", vec![org_id], Vec::new()),
        },
        TokenBinding {
            token: "tok-dp".into(),
            subject: Subject::authenticated("editor_dp", vec![org_id], Vec::new()),
        },
    ]);
    let app = router(state);

    // 1. editor_mesto can publish under mesto-bb
    let (st, _, _) = send_json(
        &app,
        Method::POST,
        "/api/3/action/package_create",
        host,
        Some("tok-mesto"),
        None,
        Some(json!({
            "name": "mesto-rozpocet",
            "title": "Rozpočet mesta",
            "notes": "Rozpočet mesta Banská Bystrica",
            "owner_org": "mesto-bb",
            "license_id": "cc-by"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // 2. editor_mesto is FORBIDDEN from publishing under dopravny-podnik
    let (st, _, _) = send_json(
        &app,
        Method::POST,
        "/api/3/action/package_create",
        host,
        Some("tok-mesto"),
        None,
        Some(json!({
            "name": "dp-linky",
            "title": "Linky MHD",
            "notes": "Linky MHD",
            "owner_org": "dopravny-podnik",
            "license_id": "cc-by"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // 3. editor_dp can publish under dopravny-podnik, and is FORBIDDEN from editing mesto-rozpocet
    let (st, _, _) = send_json(
        &app,
        Method::POST,
        "/api/3/action/package_create",
        host,
        Some("tok-dp"),
        None,
        Some(json!({
            "name": "dp-linky",
            "title": "Linky MHD",
            "notes": "Linky MHD",
            "owner_org": "dopravny-podnik",
            "license_id": "cc-by"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let (st, _, _) = send_json(
        &app,
        Method::POST,
        "/api/3/action/package_patch",
        host,
        Some("tok-dp"),
        None,
        Some(json!({
            "id": "mesto-rozpocet",
            "title": "Hijacked"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // 4. Merge dopravny-podnik into mesto-bb: datasets move and old publisher feed redirects (C16)
    store
        .merge_publishers(&ctx_admin, "dopravny-podnik", "mesto-bb")
        .await
        .unwrap();
    let moved = store.get_dataset(&ctx_admin, "dp-linky").await.unwrap().unwrap();
    assert_eq!(moved.publisher_id, "mesto-bb");

    let (st_redir, hdrs, _) = send_json(
        &app,
        Method::GET,
        "/publisher/dopravny-podnik/catalog.ttl",
        host,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_redir, StatusCode::PERMANENT_REDIRECT);
    assert_eq!(
        hdrs.get(header::LOCATION).unwrap().to_str().unwrap(),
        "/publisher/mesto-bb/catalog.ttl"
    );
}

#[tokio::test]
async fn dataset_rendered_under_eu_cz_and_sk_profiles_by_accept_profile() {
    let org_id = OrgId::from_uuid(Uuid::from_u128(300));
    let cat_id = CatalogueId::from_uuid(Uuid::from_u128(3000));
    let store = Arc::new(MemoryStore::new());
    let host = "opendata.region.example";
    store
        .upsert_catalogue(make_catalogue(org_id, cat_id, "region", host))
        .await
        .unwrap();

    let ctx = RequestCtx::new(
        org_id,
        cat_id,
        Subject::authenticated("editor", vec![org_id], Vec::new()),
    );
    store
        .upsert_publisher(
            &ctx,
            Publisher {
                org_id,
                catalogue_id: cat_id,
                id: "pub-region".into(),
                name: "region".into(),
                title: BTreeMap::from([
                    ("sk".into(), "Kraj".into()),
                    ("cs".into(), "Kraj".into()),
                    ("en".into(), "Region".into()),
                ]),
                description: BTreeMap::new(),
                uri: "https://data.gov.sk/id/legal-subject/37828100".into(),
                publisher_type: None,
                identifier: None,
                email: None,
                homepage: None,
                logo_url: None,
                contact_point: None,
                state: RecordState::Active,
                merged_into: None,
                editor_grants: vec!["editor".into()],
                created: Utc::now(),
                modified: Utc::now(),
            },
        )
        .await
        .unwrap();

    store
        .upsert_dataset(
            &ctx,
            Dataset {
                org_id,
                catalogue_id: cat_id,
                id: "pkg-timetables".into(),
                name: "timetables".into(),
                uri: Some("https://opendata.region.example/dataset/timetables".into()),
                dataset_type: "dataset".into(),
                title: BTreeMap::from([
                    ("sk".into(), "Cestovné poriadky".into()),
                    ("cs".into(), "Jízdní řády".into()),
                    ("en".into(), "Timetables".into()),
                ]),
                description: BTreeMap::from([
                    ("sk".into(), "Platné cestovné poriadky".into()),
                    ("cs".into(), "Platné jízdní řády".into()),
                    ("en".into(), "Current timetables".into()),
                ]),
                publisher_id: "region".into(),
                publisher: None,
                creator: None,
                contact_point: None,
                visibility: Visibility::Public,
                state: RecordState::Active,
                licence_id: Some("cc-by".into()),
                licence_url: Some("https://creativecommons.org/licenses/by/4.0/".into()),
                themes: vec!["TRAN".into()],
                eurovoc_themes: Vec::new(),
                keywords: BTreeMap::from([
                    ("sk".into(), vec!["doprava".into()]),
                    ("cs".into(), vec!["doprava".into()]),
                    ("en".into(), vec!["transport".into()]),
                ]),
                tags: vec!["transport".into()],
                spatial: vec!["SK032".into()],
                spatial_resolution_in_meters: None,
                temporal: None,
                temporal_resolution: None,
                frequency: Some("DAILY".into()),
                landing_page: None,
                documentation: Vec::new(),
                conforms_to: Vec::new(),
                applicable_legislation: Vec::new(),
                in_series: None,
                relations: Vec::new(),
                sources: Vec::new(),
                languages: vec!["sk".into(), "cs".into(), "en".into()],
                version: None,
                version_notes: BTreeMap::new(),
                access_rights: None,
                provenance: BTreeMap::new(),
                dcat_type: None,
                identifier: Some("timetables".into()),
                alternate_identifiers: Vec::new(),
                endpoint_url: None,
                generated_by: None,
                harvest_source_id: None,
                harvest_source_item_id: None,
                harvest_hash: None,
                custom_shacl_shapes: None,
                distributions: vec![Distribution {
                    id: "res-1".into(),
                    dataset_id: "pkg-timetables".into(),
                    uri: None,
                    title: BTreeMap::from([
                        ("sk".into(), "CSV súbor".into()),
                        ("cs".into(), "CSV soubor".into()),
                        ("en".into(), "CSV file".into()),
                    ]),
                    description: BTreeMap::new(),
                    access_url: "https://opendata.region.example/timetables.csv".into(),
                    download_url: Some("https://opendata.region.example/timetables.csv".into()),
                    format: Some("CSV".into()),
                    media_type: Some("text/csv".into()),
                    compress_format: None,
                    package_format: None,
                    byte_size: None,
                    hash: None,
                    hash_algorithm: None,
                    licence: None,
                    rights: BTreeMap::new(),
                    status: None,
                    availability: None,
                    resource_type: None,
                    conforms_to: Vec::new(),
                    documentation: Vec::new(),
                    languages: Vec::new(),
                    applicable_legislation: Vec::new(),
                    temporal_resolution: None,
                    spatial_resolution_in_meters: None,
                    terms_of_use: None,
                    access_services: Vec::new(),
                    issued: None,
                    modified: None,
                    extras: BTreeMap::new(),
                }],
                extras: BTreeMap::new(),
                restricted_grants: Vec::new(),
                issued: Utc::now(),
                modified: Utc::now(),
            },
        )
        .await
        .unwrap();

    let state = DataroomState::new(
        store,
        Arc::new(PolicyAuthorizer::default()),
        org_id,
        cat_id,
    );
    let app = router(state);

    // 1. EU 3.0.1 profile via Accept-Profile
    let (st, hdrs, eu_ttl) = send_json(
        &app,
        Method::GET,
        "/dataset/timetables.ttl",
        host,
        None,
        Some("<https://semiceu.github.io/DCAT-AP/releases/3.0.1>"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        hdrs.get("content-profile").unwrap().to_str().unwrap(),
        "https://semiceu.github.io/DCAT-AP/releases/3.0.1"
    );
    assert!(
        eu_ttl.contains("http://publications.europa.eu/resource/authority/licence/CC_BY_4_0"),
        "EU profile uses EU licence authority IRI: {eu_ttl}"
    );

    // 2. CZ profile via Accept-Profile
    let (st, hdrs, cz_ttl) = send_json(
        &app,
        Method::GET,
        "/dataset/timetables.ttl",
        host,
        None,
        Some("<https://ofn.gov.cz/dcat-ap-cz>"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        hdrs.get("content-profile").unwrap().to_str().unwrap(),
        "https://ofn.gov.cz/dcat-ap-cz"
    );
    assert!(
        cz_ttl.contains("podmínky-užití/") && cz_ttl.contains("Specifikace"),
        "CZ profile emits pu:Specifikace terms of use: {cz_ttl}"
    );

    // 3. SK 2.0 profile via Accept-Profile (Turtle and JSON-LD)
    let (st, hdrs, sk_ttl) = send_json(
        &app,
        Method::GET,
        "/dataset/timetables.ttl",
        host,
        None,
        Some("<https://datova-kancelaria.github.io/dcat-ap-sk-2.0/>"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        hdrs.get("content-profile").unwrap().to_str().unwrap(),
        "https://datova-kancelaria.github.io/dcat-ap-sk-2.0/"
    );
    assert!(
        sk_ttl.contains("leg:termsOfUse") && sk_ttl.contains("leg:TermsOfUse"),
        "SK profile emits leg:termsOfUse: {sk_ttl}"
    );

    let (st, _, sk_jsonld) = send_json(
        &app,
        Method::GET,
        "/dataset/timetables.jsonld",
        host,
        None,
        Some("dcat-ap-sk-2.0"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let sk_val: Value = serde_json::from_str(&sk_jsonld).unwrap();
    assert_eq!(
        sk_val["@context"],
        "https://datova-kancelaria.github.io/dcat-ap-sk-2.0/kontexty/rozhranie-katalógu-otvorených-dát.jsonld"
    );
    assert_eq!(sk_val["typ"], "Dataset");
    assert_eq!(sk_val["názov"]["sk"], "Cestovné poriadky");
}

#[tokio::test]
async fn tenant_isolation_across_two_organizations_for_every_operation() {
    let org_a = OrgId::from_uuid(Uuid::from_u128(0xAAAA));
    let cat_a = CatalogueId::from_uuid(Uuid::from_u128(0xAAAA_1111));
    let org_b = OrgId::from_uuid(Uuid::from_u128(0xBBBB));
    let cat_b = CatalogueId::from_uuid(Uuid::from_u128(0xBBBB_1111));

    let store = Arc::new(MemoryStore::new());
    store
        .upsert_catalogue(make_catalogue(org_a, cat_a, "shared-name", "a.example.org"))
        .await
        .unwrap();
    store
        .upsert_catalogue(make_catalogue(org_b, cat_b, "shared-name", "b.example.org"))
        .await
        .unwrap();

    let ctx_a = RequestCtx::new(
        org_a,
        cat_a,
        Subject::authenticated("alice", vec![org_a], Vec::new()),
    );
    let ctx_b = RequestCtx::new(
        org_b,
        cat_b,
        Subject::authenticated("bob", vec![org_b], Vec::new()),
    );

    // Seed identical publisher, dataset and harvest source names in both org A and org B,
    // plus a B-only secret dataset `b-only-secret`.
    for (ctx, org, cat, secret_name) in [
        (&ctx_a, org_a, cat_a, "a-only-secret"),
        (&ctx_b, org_b, cat_b, "b-only-secret"),
    ] {
        store
            .upsert_publisher(
                ctx,
                Publisher {
                    org_id: org,
                    catalogue_id: cat,
                    id: "pub-city".into(),
                    name: "city".into(),
                    title: BTreeMap::from([("sk".into(), "City".into())]),
                    description: BTreeMap::new(),
                    uri: "https://data.gov.sk/id/legal-subject/00313271".into(),
                    publisher_type: None,
                    identifier: None,
                    email: None,
                    homepage: None,
                    logo_url: None,
                    contact_point: None,
                    state: RecordState::Active,
                    merged_into: None,
                    editor_grants: Vec::new(),
                    created: Utc::now(),
                    modified: Utc::now(),
                },
            )
            .await
            .unwrap();

        for ds_name in ["common-dataset", secret_name] {
            store
                .upsert_dataset(
                    ctx,
                    Dataset {
                        org_id: org,
                        catalogue_id: cat,
                        id: format!("pkg-{ds_name}"),
                        name: ds_name.into(),
                        uri: None,
                        dataset_type: "dataset".into(),
                        title: BTreeMap::from([("sk".into(), ds_name.into())]),
                        description: BTreeMap::from([("sk".into(), "Desc".into())]),
                        publisher_id: "city".into(),
                        publisher: None,
                        creator: None,
                        contact_point: None,
                        visibility: Visibility::Internal,
                        state: RecordState::Active,
                        licence_id: Some("cc-by".into()),
                        licence_url: None,
                        themes: vec!["GOVE".into()],
                        eurovoc_themes: Vec::new(),
                        keywords: BTreeMap::new(),
                        tags: vec!["tag1".into()],
                        spatial: Vec::new(),
                        spatial_resolution_in_meters: None,
                        temporal: None,
                        temporal_resolution: None,
                        frequency: Some("DAILY".into()),
                        landing_page: None,
                        documentation: Vec::new(),
                        conforms_to: Vec::new(),
                        applicable_legislation: Vec::new(),
                        in_series: None,
                        relations: Vec::new(),
                        sources: Vec::new(),
                        languages: Vec::new(),
                        version: None,
                        version_notes: BTreeMap::new(),
                        access_rights: None,
                        provenance: BTreeMap::new(),
                        dcat_type: None,
                        identifier: None,
                        alternate_identifiers: Vec::new(),
                        endpoint_url: None,
                        generated_by: None,
                        harvest_source_id: None,
                        harvest_source_item_id: None,
                        harvest_hash: None,
                        custom_shacl_shapes: None,
                        distributions: Vec::new(),
                        extras: BTreeMap::new(),
                        restricted_grants: Vec::new(),
                        issued: Utc::now(),
                        modified: Utc::now(),
                    },
                )
                .await
                .unwrap();
        }

        store
            .upsert_harvest_source(
                ctx,
                HarvestSource {
                    org_id: org,
                    catalogue_id: cat,
                    id: format!("hs-{secret_name}"),
                    name: secret_name.into(),
                    kind: HarvestSourceKind::DcatAp,
                    url: "https://external.example.org/catalog.ttl".into(),
                    publisher_id: "city".into(),
                    target_publisher_iri: None,
                    spatial_uri: None,
                    name_prefix: String::new(),
                    allowed_hosts: vec!["external.example.org".into()],
                    rewrite_hosts: BTreeMap::new(),
                    skip_items: BTreeMap::new(),
                    title_overrides: BTreeMap::new(),
                    spelling_fixes: BTreeMap::new(),
                    require_column: BTreeMap::new(),
                    schedule: "23 3 * * *".into(),
                    enabled: true,
                    modified: Utc::now(),
                },
            )
            .await
            .unwrap();
    }

    let authz = Arc::new(PolicyAuthorizer::new(vec![
        GrantRule {
            org_id: org_a,
            catalogue_id: Some(cat_a),
            principal: "alice".into(),
            actions: vec![Action::Admin],
            publisher_id: None,
        },
        GrantRule {
            org_id: org_b,
            catalogue_id: Some(cat_b),
            principal: "bob".into(),
            actions: vec![Action::Admin],
            publisher_id: None,
        },
    ]));

    let fetcher = MapFetcher::new();
    let engine = Arc::new(HarvestEngine::new(Arc::new(fetcher)).with_dns_resolution(false));
    let state = DataroomState::new(store.clone(), authz, org_a, cat_a)
        .with_harvester(engine)
        .with_tokens(vec![TokenBinding {
            token: "tok-alice".into(),
            subject: ctx_a.subject.clone(),
        }]);
    let op_ctx = state.op_context();

    // Walk every registered operation in the operation registry as org A (`ctx_a`) and verify:
    // 1. List/search operations return only org A's records and never mention `b-only-secret` or `org_b`.
    // 2. Probing org B's `b-only-secret` id returns the exact same NotFound error as `nonexistent-xyz`.
    for op in all_operations() {
        let probe_b_input = match op.name {
            "dataset.get" | "dataset.withdraw" => json!({ "name": "b-only-secret" }),
            "harvest_run.start" => json!({ "source": "b-only-secret" }),
            "harvest_run.get" | "harvest_run.logs" => json!({ "run": "run-b-only" }),
            "allowed" => json!({ "operation": "dataset.get", "object": "b-only-secret" }),
            "catalogue.propose" => continue,
            "dataset.propose" => continue,
            "publisher.propose" => continue,
            "harvest_source.propose" => continue,
            "plugin.propose" => continue,
            _ => json!({}),
        };

        let res = execute_operation(&op_ctx, &ctx_a, op.name, probe_b_input).await;
        match op.name {
            "dataset.get" | "dataset.withdraw" | "harvest_run.start" | "harvest_run.get"
            | "harvest_run.logs" => {
                assert!(
                    matches!(res, Err(dataroom_core::DataroomError::NotFound(_))),
                    "op {} probing B id must return NotFound, got {:?}",
                    op.name,
                    res
                );
            }
            "allowed" => {
                let val = res.expect("allowed op");
                assert_eq!(val["allowed"], false);
                assert_eq!(val["reason"], "dataset not found");
            }
            _ => {
                let val = res.unwrap_or_else(|e| panic!("op {} failed: {e}", op.name));
                let text = val.to_string();
                assert!(
                    !text.contains("b-only-secret") && !text.contains(&org_b.to_string()),
                    "op {} leaked org B data: {text}",
                    op.name
                );
            }
        }
    }

    // Also verify OData v4 `/odata/v4/Datasets` on host A never returns org B rows
    let app = router(state);
    let (st, _, odata_body) = send_json(
        &app,
        Method::GET,
        "/odata/v4/Datasets?$count=true",
        "a.example.org",
        Some("tok-alice"),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{odata_body}");
    let odata_val: Value = serde_json::from_str(&odata_body).unwrap();
    assert_eq!(odata_val["@odata.count"], 2);
    assert!(!odata_body.contains("b-only-secret"));
}
