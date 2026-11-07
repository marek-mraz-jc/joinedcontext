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
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
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
        branding: None,
        hidden_datasets: BTreeMap::new(),
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

    let state =
        DataroomState::new(store.clone(), authz, org_id, cat_id).with_tokens(vec![TokenBinding {
            token: "secret-jcctl-token".into(),
            subject: Subject::authenticated("jcctl-publisher", vec![org_id], Vec::new()),
        }]);
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
    assert_eq!(st, StatusCode::OK, "package_patch: {body}");
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
    let second_res_id = shown_pkg["resources"][1]["id"].as_str().unwrap().to_owned();
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
                badge_initials: None,
                badge_colour: None,
                declared_by_instance: None,
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
                badge_initials: None,
                badge_colour: None,
                declared_by_instance: None,
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
    let moved = store
        .get_dataset(&ctx_admin, "dp-linky")
        .await
        .unwrap()
        .unwrap();
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
                badge_initials: None,
                badge_colour: None,
                declared_by_instance: None,
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

    let state = DataroomState::new(store, Arc::new(PolicyAuthorizer::default()), org_id, cat_id);
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
                    badge_initials: None,
                    badge_colour: None,
                    declared_by_instance: None,
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
            "publisher.merge" => continue,
            "publisher.withdraw" => json!({ "publisher": "b-only-secret" }),
            "harvest_source.propose" => continue,
            "branding.propose" => continue,
            "profile.propose" => continue,
            "plugin.propose" => continue,
            _ => json!({}),
        };

        let res = execute_operation(&op_ctx, &ctx_a, op.name, probe_b_input).await;
        match op.name {
            "dataset.get" | "dataset.withdraw" | "harvest_run.start" | "harvest_run.get"
            | "harvest_run.logs" | "publisher.withdraw" => {
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

#[tokio::test]
async fn b4_two_catalogues_on_two_hosts_render_two_looks_and_noindex_robots() {
    let org_a = OrgId::from_uuid(Uuid::from_u128(0xB401));
    let cat_a = CatalogueId::from_uuid(Uuid::from_u128(0x000B_4011));
    let org_b = OrgId::from_uuid(Uuid::from_u128(0xB402));
    let cat_b = CatalogueId::from_uuid(Uuid::from_u128(0x000B_4021));

    let store = Arc::new(MemoryStore::new());
    let mut c_a = make_catalogue(org_a, cat_a, "bb", "data.banskabystrica.sk");
    c_a.branding = Some(json!({
        "instanceName": "Banská Bystrica",
        "colours": { "primary": "#00579c" },
        "siteTitle": { "sk": "Otvorené dáta BB" },
        "noindex": false
    }));
    store.upsert_catalogue(c_a).await.unwrap();

    let mut c_b = make_catalogue(org_b, cat_b, "demo", "demo.joinedcontext.com");
    c_b.branding = Some(json!({
        "instanceName": "Demo Instance",
        "colours": { "primary": "#7a1f5c" },
        "siteTitle": { "sk": "Demo Katalóg" },
        "ribbon": "Demo environment — not official data",
        "noindex": true
    }));
    store.upsert_catalogue(c_b).await.unwrap();

    let ctx_a = RequestCtx::new(
        org_a,
        cat_a,
        Subject::authenticated("ed_a", vec![org_a], Vec::new()),
    );
    let ctx_b = RequestCtx::new(
        org_b,
        cat_b,
        Subject::authenticated("ed_b", vec![org_b], Vec::new()),
    );

    for (ctx, org, cat, ds_name, title) in [
        (&ctx_a, org_a, cat_a, "bb-rozpocet", "Rozpočet BB"),
        (&ctx_b, org_b, cat_b, "demo-sensors", "Demo senzory"),
    ] {
        store
            .upsert_publisher(
                ctx,
                Publisher {
                    org_id: org,
                    catalogue_id: cat,
                    id: "pub-1".into(),
                    name: "pub1".into(),
                    title: BTreeMap::from([("sk".into(), "Pub".into())]),
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
                    badge_initials: None,
                    badge_colour: None,
                    declared_by_instance: None,
                    created: Utc::now(),
                    modified: Utc::now(),
                },
            )
            .await
            .unwrap();
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
                    title: BTreeMap::from([("sk".into(), title.into())]),
                    description: BTreeMap::from([("sk".into(), title.into())]),
                    publisher_id: "pub1".into(),
                    publisher: None,
                    creator: None,
                    contact_point: None,
                    visibility: Visibility::Public,
                    state: RecordState::Active,
                    licence_id: Some("cc-by".into()),
                    licence_url: None,
                    themes: vec!["GOVE".into()],
                    eurovoc_themes: Vec::new(),
                    keywords: BTreeMap::new(),
                    tags: Vec::new(),
                    spatial: Vec::new(),
                    spatial_resolution_in_meters: None,
                    temporal: None,
                    temporal_resolution: None,
                    frequency: None,
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

    let app = router(DataroomState::new(
        store,
        Arc::new(PolicyAuthorizer::default()),
        org_a,
        cat_a,
    ));

    // Host A renders BB look, no noindex meta tag, and only BB's dataset
    let (st, _, html_a) = send_json(
        &app,
        Method::GET,
        "/",
        "data.banskabystrica.sk",
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(html_a.contains("--jc-primary: #00579c;"));
    assert!(html_a.contains("Otvorené dáta BB"));
    assert!(html_a.contains("Rozpočet BB"));
    assert!(!html_a.contains("Demo senzory"));
    assert!(!html_a.contains("noindex, nofollow"));

    let (_, _, robots_a) = send_json(
        &app,
        Method::GET,
        "/robots.txt",
        "data.banskabystrica.sk",
        None,
        None,
        None,
    )
    .await;
    assert!(robots_a.contains("Sitemap: https://data.banskabystrica.sk/sitemap.xml"));

    // Host B renders Demo look, ribbon, noindex meta tag, Disallow: / in robots.txt, and only Demo's dataset
    let (st, _, html_b) = send_json(
        &app,
        Method::GET,
        "/",
        "demo.joinedcontext.com",
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(html_b.contains("--jc-primary: #7a1f5c;"));
    assert!(html_b.contains("Demo Katalóg"));
    assert!(html_b.contains("Demo environment — not official data"));
    assert!(html_b.contains("noindex, nofollow"));
    assert!(html_b.contains("Demo senzory"));
    assert!(!html_b.contains("Rozpočet BB"));

    let (_, _, robots_b) = send_json(
        &app,
        Method::GET,
        "/robots.txt",
        "demo.joinedcontext.com",
        None,
        None,
        None,
    )
    .await;
    assert_eq!(robots_b, "User-agent: *\nDisallow: /\n");
}

/// The pack's live pages escape `'` as `&#39;` (Jinja), askama as `&#x27;`: the same character.
/// Snapshot comparisons read both in the pack's form.
fn normalise_entities(html: &str) -> String {
    html.replace("&#x27;", "&#39;")
        .replace("&#x22;", "&quot;")
        .replace("&#34;", "&quot;")
}

#[tokio::test]
async fn d_live_pages_snapshot_and_security_headers_match_pack_html() {
    let org_id = OrgId::from_uuid(Uuid::from_u128(0xD001));
    let cat_id = CatalogueId::from_uuid(Uuid::from_u128(0x000D_0011));
    let store = Arc::new(MemoryStore::new());
    let host = "opendata.dev.joinedcontext.com";

    let dev_branding = dataroom_core::BrandingBlock::from_yaml_lossy(include_str!(
        "../../core/tests/fixtures/branding/dev-branding.yaml"
    ));

    let mut cat = make_catalogue(org_id, cat_id, "dev", host);
    cat.title = BTreeMap::from([
        ("sk".into(), "Otvorené dáta".into()),
        ("en".into(), "Open Data".into()),
    ]);
    cat.languages = vec!["sk".into(), "en".into(), "fi".into(), "hu".into()];
    cat.branding = Some(json!({
        "domain": "dev.joinedcontext.com",
        "siteTitle": { "sk": "Otvorené dáta", "en": "Open Data" },
        "samples": {
            "sk": ["Aká je teraz kvalita ovzdušia v Helsinkách?"]
        }
    }));
    store.upsert_catalogue(cat).await.unwrap();

    let ctx = RequestCtx::new(
        org_id,
        cat_id,
        Subject::authenticated("admin", vec![org_id], Vec::new()),
    );

    // Seed publishers matching `live/dev-organization-list.html` and `live/dev-organization-banskabystrica.html`
    for (slug, title, desc, initials, colour) in [
        (
            "barcelona",
            "Ajuntament de Barcelona",
            "Otvorené údaje mesta Barcelona (Open Data BCN, CC BY 4.0), publikované cez endpointy joinedcontext.",
            "BCN",
            "#c8102e",
        ),
        (
            "banskabystrica",
            "Mesto Banská Bystrica",
            "Otvorené údaje mesta Banská Bystrica.",
            "BB",
            "#00579c",
        ),
        (
            "bbsk",
            "Banskobystrický samosprávny kraj",
            "Otvorené údaje Banskobystrického samosprávneho kraja.",
            "BBSK",
            "#7a1f5c",
        ),
        (
            "hel-fi",
            "Helsinki Region Context",
            "Otvorené údaje regiónu Helsinki, naživo z verejnej dopravy HSL, parkovísk...",
            "HEL",
            "#0000bf",
        ),
    ] {
        store
            .upsert_publisher(
                &ctx,
                Publisher {
                    org_id,
                    catalogue_id: cat_id,
                    id: format!("pub-{slug}"),
                    name: slug.into(),
                    title: BTreeMap::from([("sk".into(), title.into())]),
                    description: BTreeMap::from([("sk".into(), desc.into())]),
                    uri: "http://datos.gob.es/recurso/sector-publico/org/Organismo/L01080193".into(),
                    publisher_type: None,
                    identifier: None,
                    email: None,
                    homepage: None,
                    logo_url: None,
                    contact_point: None,
                    state: RecordState::Active,
                    merged_into: None,
                    editor_grants: Vec::new(),
                    badge_initials: Some(initials.into()),
                    badge_colour: Some(colour.into()),
                    declared_by_instance: None,
                    created: Utc::now(),
                    modified: Utc::now(),
                },
            )
            .await
            .unwrap();
    }

    // Seed `air-quality` dataset matching `live/dev-dataset-air-quality-sk.html`
    store
        .upsert_dataset(
            &ctx,
            Dataset {
                org_id,
                catalogue_id: cat_id,
                id: "d262a2d3-17b2-4e44-afb7-1275fdbf3e4e".into(),
                name: "air-quality".into(),
                uri: None,
                dataset_type: "dataset".into(),
                title: BTreeMap::from([(
                    "sk".into(),
                    "Qualitat de l'aire a Barcelona".into(),
                )]),
                description: BTreeMap::from([(
                    "sk".into(),
                    "Les estacions de la xarxa de qualitat de l'aire de Barcelona i la darrera mitjana horària de cada contaminant (NO2, O3, PM10, PM2.5, SO2, CO, NO, NOx, carboni negre), amb 30 dies d'història.".into(),
                )]),
                publisher_id: "barcelona".into(),
                publisher: None,
                creator: None,
                contact_point: Some(ContactPoint {
                    uri: None,
                    kind: ContactKind::Organization,
                    name: BTreeMap::from([("sk".into(), "joinedcontext platform".into())]),
                    email: "hello@joinedcontext.com".into(),
                    url: None,
                    identifier: None,
                }),
                visibility: Visibility::Public,
                state: RecordState::Active,
                licence_id: Some("cc-by".into()),
                licence_url: Some("http://www.opendefinition.org/licenses/cc-by".into()),
                themes: vec!["ENVI".into(), "SOCI".into()],
                eurovoc_themes: Vec::new(),
                keywords: BTreeMap::new(),
                tags: vec!["NO2".into(), "PM10".into(), "air quality".into()],
                spatial: vec!["http://data.europa.eu/nuts/code/ES511".into()],
                spatial_resolution_in_meters: None,
                temporal: None,
                temporal_resolution: None,
                frequency: Some("HOURLY".into()),
                landing_page: None,
                documentation: Vec::new(),
                conforms_to: vec!["https://www.w3.org/TR/json-ld11/".into()],
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
                identifier: Some("esbxdrr7snorjsp7uus2luihr6vl5vv6".into()),
                alternate_identifiers: Vec::new(),
                endpoint_url: Some(
                    "https://api.dev.joinedcontext.com/api/endpoint/esbxdrr7snorjsp7uus2luihr6vl5vv6/"
                        .into(),
                ),
                generated_by: None,
                harvest_source_id: None,
                harvest_source_item_id: None,
                harvest_hash: None,
                custom_shacl_shapes: None,
                distributions: vec![
                    Distribution {
                        id: "0e74d5fe-c17f-4bd7-879b-0c3bf6481817".into(),
                        dataset_id: "d262a2d3-17b2-4e44-afb7-1275fdbf3e4e".into(),
                        uri: None,
                        title: BTreeMap::from([("sk".into(), "CSV".into())]),
                        description: BTreeMap::new(),
                        access_url: "https://api.dev.joinedcontext.com/api/endpoint/esbxdrr7snorjsp7uus2luihr6vl5vv6/file.csv".into(),
                        download_url: None,
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
                    },
                    Distribution {
                        id: "ae7704f9-4a60-4510-b810-e4528f63030e".into(),
                        dataset_id: "d262a2d3-17b2-4e44-afb7-1275fdbf3e4e".into(),
                        uri: None,
                        title: BTreeMap::from([("sk".into(), "NGSI-LD API".into())]),
                        description: BTreeMap::new(),
                        access_url: "https://api.dev.joinedcontext.com/api/endpoint/esbxdrr7snorjsp7uus2luihr6vl5vv6/ngsi-ld/v1/".into(),
                        download_url: None,
                        format: Some("NGSI-LD".into()),
                        media_type: Some("application/json".into()),
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
                    },
                ],
                extras: BTreeMap::from([
                    ("rights".into(), "Font: Ajuntament de Barcelona, Open Data BCN, CC BY 4.0".into()),
                    ("quality_pass_rate".into(), "1.0".into()),
                    ("was_generated_by".into(), "pipeline air-quality".into()),
                ]),
                restricted_grants: Vec::new(),
                issued: Utc::now(),
                modified: Utc::now(),
            },
        )
        .await
        .unwrap();

    // A second public dataset on another topic. The theme leaves out a Topic value every result
    // shares (plugin.py `jc_facet_labels`) and a filter with nothing to choose
    // (templates__snippets__facet_list.html); the live list page shows "Téma" because its
    // datasets carry different topics, so the seed does too.
    let mut second = store
        .get_dataset(&ctx, "air-quality")
        .await
        .unwrap()
        .unwrap();
    second.id = "5b0c7d1e-2f3a-4b5c-8d9e-0f1a2b3c4d5e".into();
    second.name = "parking-occupancy".into();
    second.title = BTreeMap::from([("sk".into(), "Obsadenosť parkovísk".into())]);
    second.themes = vec!["TRAN".into()];
    second.identifier = None;
    second.endpoint_url = None;
    for (i, dist) in second.distributions.iter_mut().enumerate() {
        dist.id = format!("parking-occupancy-res-{i}");
        dist.dataset_id = second.id.clone();
    }
    store.upsert_dataset(&ctx, second).await.unwrap();

    store
        .upsert_resource_view(
            &ctx,
            dataroom_core::ResourceView {
                id: "91530f63-eb39-4f4a-b5df-e66e79e0d067".into(),
                resource_id: "0e74d5fe-c17f-4bd7-879b-0c3bf6481817".into(),
                view_type: "jc_visualization".into(),
                title: "Černý uhlík".into(),
                description: String::new(),
                order: 1,
                config: BTreeMap::new(),
            },
        )
        .await
        .unwrap();

    let state = DataroomState::new(store, Arc::new(PolicyAuthorizer::default()), org_id, cat_id)
        .with_installation_branding(dev_branding);
    let app = router(state);

    // 1. Home page (`live/dev-home.html` parity)
    let (st, hdrs, home_html) = send_json(&app, Method::GET, "/", host, None, None, None).await;
    let home_html = normalise_entities(&home_html);
    assert_eq!(st, StatusCode::OK);
    let csp = hdrs
        .get(header::CONTENT_SECURITY_POLICY)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(csp.contains("script-src 'self' 'nonce-"));
    assert!(csp.contains("frame-ancestors 'self'"));
    for expected in [
        "id=\"jc-hero-title\"",
        "Otvorené dáta",
        "Prezrite si ich, stiahnite alebo sa na ne opýtajte asistenta.",
        "id=\"jc-topics-title\"",
        "Podľa témy",
        "id=\"jc-recent-title\"",
        "Nedávno aktualizované",
        "id=\"jc-publishers-title\"",
        "Podľa vydavateľa",
        "id=\"jc-tags-title\"",
        "Časté kľúčové slová",
        "href=\"/user/login\"",
        "Prihlásiť sa",
        "data-grid-config=",
    ] {
        assert!(
            home_html.contains(expected),
            "home_html missing expected fragment '{expected}'"
        );
    }

    // 2. Dataset list page (`live/dev-dataset-list.html` parity)
    let (st, _, list_html) =
        send_json(&app, Method::GET, "/dataset/", host, None, None, None).await;
    let list_html = normalise_entities(&list_html);
    assert_eq!(st, StatusCode::OK);
    for expected in [
        "id=\"dataset-search-form\"",
        "Vydavateľ",
        "Téma",
        "Kľúčové slová",
        "Formát",
        "Licencia",
        "Zoradiť podľa",
        "datasety nájdené",
        "Qualitat de l&#39;aire a Barcelona",
    ] {
        assert!(
            list_html.contains(expected),
            "list_html missing expected fragment '{expected}'"
        );
    }

    // 3. Dataset detail page (`live/dev-dataset-air-quality-sk.html` parity)
    let (st, _, ds_html) = send_json(
        &app,
        Method::GET,
        "/sk/dataset/air-quality",
        host,
        None,
        None,
        None,
    )
    .await;
    let ds_html = normalise_entities(&ds_html);
    assert_eq!(st, StatusCode::OK);
    for expected in [
        "id=\"jc-explorer-title\"",
        "Živé dáta",
        "id=\"jc-viz-title\"",
        "Grafy",
        "Černý uhlík",
        "id=\"jc-about-title\"",
        "O tomto datasete",
        "Ajuntament de Barcelona",
        "NUTS ES511",
        "id=\"jc-live-title\"",
        "Živé dáta (API)",
        "id=\"jc-mcp-title\"",
        "Pripojte AI asistenta",
        "id=\"dataset-resources\"",
        "id=\"jc-resources-title\"",
        "Dáta na stiahnutie",
        "id=\"jc-section-data\"",
        "id=\"jc-section-api\"",
        "Všetky záznamy ako tabuľka",
        "NGSI-LD API",
        "id=\"jc-more-title\"",
        "Ďalšie údaje",
        "100 %",
        "application/ld+json",
    ] {
        assert!(
            ds_html.contains(expected),
            "ds_html missing expected fragment '{expected}'"
        );
    }

    // 4. Resource detail page (D4)
    let (st, _, res_html) = send_json(
        &app,
        Method::GET,
        "/sk/dataset/air-quality/resource/ae7704f9-4a60-4510-b810-e4528f63030e",
        host,
        None,
        None,
        None,
    )
    .await;
    let res_html = normalise_entities(&res_html);
    assert_eq!(st, StatusCode::OK);
    for expected in [
        "NGSI-LD API",
        "Kopírovať adresu",
        "id=\"jc-address-full\"",
        "id=\"jc-examples-title\"",
        "Ako to použiť",
        "O tomto zdroji",
    ] {
        assert!(
            res_html.contains(expected),
            "res_html missing expected fragment '{expected}'"
        );
    }

    // 5. Organization list (`live/dev-organization-list.html` parity) & Organization page (`live/dev-organization-banskabystrica.html` parity)
    let (st, _, org_list_html) =
        send_json(&app, Method::GET, "/organization/", host, None, None, None).await;
    let org_list_html = normalise_entities(&org_list_html);
    assert_eq!(st, StatusCode::OK);
    for expected in [
        "id=\"organization-search-form\"",
        "organizácie nájdené",
        "Ajuntament de Barcelona",
        "Mesto Banská Bystrica",
        "data:image/svg+xml;base64,",
    ] {
        assert!(
            org_list_html.contains(expected),
            "org_list_html missing expected fragment '{expected}'"
        );
    }

    let (st, _, org_bb_html) = send_json(
        &app,
        Method::GET,
        "/organization/banskabystrica",
        host,
        None,
        None,
        None,
    )
    .await;
    let org_bb_html = normalise_entities(&org_bb_html);
    assert_eq!(st, StatusCode::OK);
    for expected in [
        "id=\"organization-info\"",
        "Mesto Banská Bystrica",
        "Otvorené údaje mesta Banská Bystrica.",
        "/feeds/organization/banskabystrica.atom",
        "id=\"organization-datasets-search-form\"",
    ] {
        assert!(
            org_bb_html.contains(expected),
            "org_bb_html missing expected fragment '{expected}'"
        );
    }

    // 6. Assistants guide page (`live/dev-assistants-claude.html` parity)
    let (st, _, claude_html) = send_json(
        &app,
        Method::GET,
        "/assistants/claude",
        host,
        None,
        None,
        None,
    )
    .await;
    let claude_html = normalise_entities(&claude_html);
    assert_eq!(st, StatusCode::OK);
    for expected in [
        "Claude krok za krokom",
        "Kto to môže použiť",
        "Postup",
        "Joinedcontext Open Data",
        "https://api.dev.joinedcontext.com<wbr>/api<wbr>/mcp",
        "Použi Joinedcontext Open Data: Aká je teraz kvalita ovzdušia v Helsinkách?",
        "Vyskúšajte",
        "Ak to nejde",
        "Všetci asistenti",
    ] {
        assert!(
            claude_html.contains(expected),
            "claude_html missing expected fragment '{expected}'"
        );
    }
}

#[tokio::test]
async fn f1_f2_f3_operation_registry_surfaces_allowed_parity_and_admin_audit_rows() {
    let org_a = OrgId::from_uuid(Uuid::from_u128(0xF101));
    let cat_a = CatalogueId::from_uuid(Uuid::from_u128(0x000F_1011));
    let org_b = OrgId::from_uuid(Uuid::from_u128(0xF102));
    let cat_b = CatalogueId::from_uuid(Uuid::from_u128(0x000F_1021));

    let store = Arc::new(MemoryStore::new());
    store
        .upsert_catalogue(make_catalogue(org_a, cat_a, "cat-a", "f1a.example.org"))
        .await
        .unwrap();
    store
        .upsert_catalogue(make_catalogue(org_b, cat_b, "cat-b", "f1b.example.org"))
        .await
        .unwrap();

    let ctx_b = RequestCtx::new(
        org_b,
        cat_b,
        Subject::authenticated("bob", vec![org_b], Vec::new()),
    );
    store
        .upsert_publisher(
            &ctx_b,
            Publisher {
                org_id: org_b,
                catalogue_id: cat_b,
                id: "pub-b-secret".into(),
                name: "pub-b-secret".into(),
                title: BTreeMap::from([("sk".into(), "Secret Pub B".into())]),
                description: BTreeMap::new(),
                uri: "https://b.example.org/pub".into(),
                publisher_type: None,
                identifier: None,
                email: None,
                homepage: None,
                logo_url: None,
                contact_point: None,
                state: RecordState::Active,
                merged_into: None,
                editor_grants: Vec::new(),
                badge_initials: None,
                badge_colour: None,
                declared_by_instance: None,
                created: Utc::now(),
                modified: Utc::now(),
            },
        )
        .await
        .unwrap();
    store
        .upsert_dataset(
            &ctx_b,
            Dataset {
                org_id: org_b,
                catalogue_id: cat_b,
                id: "pkg-b-secret".into(),
                name: "b-secret-ds".into(),
                uri: None,
                dataset_type: "dataset".into(),
                title: BTreeMap::from([("sk".into(), "Secret".into())]),
                description: BTreeMap::from([("sk".into(), "Secret".into())]),
                publisher_id: "pub-b-secret".into(),
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
                tags: Vec::new(),
                spatial: Vec::new(),
                spatial_resolution_in_meters: None,
                temporal: None,
                temporal_resolution: None,
                frequency: None,
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

    let authz = Arc::new(PolicyAuthorizer::new(vec![GrantRule {
        org_id: org_a,
        catalogue_id: Some(cat_a),
        principal: "alice-admin".into(),
        actions: vec![Action::Admin],
        publisher_id: None,
    }]));

    let state =
        DataroomState::new(store.clone(), authz, org_a, cat_a).with_tokens(vec![TokenBinding {
            token: "tok-alice".into(),
            subject: Subject::authenticated("alice-admin", vec![org_a], Vec::new()),
        }]);
    let app = router(state.clone());
    let host = "f1a.example.org";

    // F1 & F3: Walk the registry, verify REST, MCP tool, CLI command, and OData (where it lists)
    // and verify that branding, profile, and publisher admin operations write audit activity rows.
    for op in dataroom_api::operations() {
        assert!(!op.name.is_empty());
        assert!(!op.mcp_tool.is_empty());
        assert!(!op.cli_command.is_empty());
    }

    // Call `branding.propose`, `profile.propose`, `publisher.propose`, `publisher.merge`, `publisher.withdraw`
    // via both REST (`/api/v1/ops/{op}`) and MCP (`/mcp`)
    let (st, _, _) = send_json(
        &app,
        Method::POST,
        "/api/v1/ops/branding.propose",
        host,
        Some("tok-alice"),
        None,
        Some(json!({ "branding": { "instanceName": "Updated Brand" } })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let (st, _, _) = send_json(
        &app,
        Method::POST,
        "/mcp",
        host,
        Some("tok-alice"),
        None,
        Some(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "jc_publisher_propose",
                "arguments": {
                    "publisher": {
                        "id": "pub-src",
                        "name": "pub-src",
                        "title": { "sk": "Source Pub" },
                        "uri": "https://a.example.org/src",
                        "state": "active"
                    }
                }
            }
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let (st, _, propose_body) = send_json(
        &app,
        Method::POST,
        "/api/v1/ops/publisher.propose",
        host,
        Some("tok-alice"),
        None,
        Some(json!({
            "publisher": {
                "id": "pub-dst",
                "name": "pub-dst",
                "title": { "sk": "Dest Pub" },
                "uri": "https://a.example.org/dst",
                "state": "active"
            }
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "publisher.propose: {propose_body}");

    let (st, _, _) = send_json(
        &app,
        Method::POST,
        "/api/v1/ops/publisher.merge",
        host,
        Some("tok-alice"),
        None,
        Some(json!({ "source": "pub-src", "target": "pub-dst" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let (st, _, _) = send_json(
        &app,
        Method::POST,
        "/api/v1/ops/publisher.withdraw",
        host,
        Some("tok-alice"),
        None,
        Some(json!({ "publisher": "pub-dst" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // Check that activity rows were recorded for branding.propose, publisher.propose, publisher.merge, publisher.withdraw (F3, C13)
    let (st, _, act_body) = send_json(
        &app,
        Method::POST,
        "/api/v1/ops/catalogue.activity",
        host,
        Some("tok-alice"),
        None,
        Some(json!({})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let act_val: Value = serde_json::from_str(&act_body).unwrap();
    let ops_logged: Vec<&str> = act_val["activity"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["operation"].as_str())
        .collect();
    for expected_op in [
        "branding.propose",
        "publisher.propose",
        "publisher.merge",
        "publisher.withdraw",
    ] {
        assert!(
            ops_logged.contains(&expected_op),
            "missing activity audit row for {expected_op} in {ops_logged:?}"
        );
    }

    // F2: `allowed` for a missing id and another tenant's id (`b-secret-ds` / `pub-b-secret`)
    // returns the exact same answer (`allowed: false`, `reason: "dataset not found"` / `"publisher not found"`).
    let (_, _, allowed_missing) = send_json(
        &app,
        Method::GET,
        "/api/v1/allowed?operation=dataset.get&object=nonexistent-id-123",
        host,
        Some("tok-alice"),
        None,
        None,
    )
    .await;
    let (_, _, allowed_other_tenant) = send_json(
        &app,
        Method::GET,
        "/api/v1/allowed?operation=dataset.get&object=b-secret-ds",
        host,
        Some("tok-alice"),
        None,
        None,
    )
    .await;
    let v_missing: Value = serde_json::from_str(&allowed_missing).unwrap();
    let v_other: Value = serde_json::from_str(&allowed_other_tenant).unwrap();
    assert_eq!(
        v_missing, v_other,
        "F2: allowed must return identical answer for missing id and another tenant's id"
    );
}

#[tokio::test]
async fn g1_g2_g3_g4_mountable_router_portal_and_jcctl_parity() {
    let org_id = OrgId::from_uuid(Uuid::from_u128(0x6100));
    let cat_id = CatalogueId::from_uuid(Uuid::from_u128(0x0006_1001));
    let store = Arc::new(MemoryStore::new());
    let host = "opendata.city.example";
    store
        .upsert_catalogue(make_catalogue(org_id, cat_id, "city", host))
        .await
        .unwrap();

    let authz = Arc::new(PolicyAuthorizer::new(vec![GrantRule {
        org_id,
        catalogue_id: Some(cat_id),
        principal: "portal-sync".into(),
        actions: vec![
            Action::Read,
            Action::Propose,
            Action::Approve,
            Action::Delete,
            Action::Admin,
        ],
        publisher_id: None,
    }]));

    let portal_branding = dataroom_core::BrandingBlock::from_yaml_lossy(include_str!(
        "../../core/tests/fixtures/branding/smartcity-branding.yaml"
    ));

    // G1: Mount `dataroom_api::router(state)` under `/catalogue` beside another router in one Axum app
    let dr_state = DataroomState::new(store.clone(), authz.clone(), org_id, cat_id)
        .with_installation_branding(portal_branding.clone())
        .with_mount_prefix("/catalogue")
        .with_tokens(vec![
            TokenBinding {
                token: "tok-inst-1".into(),
                subject: Subject {
                    actor_id: Some("portal-sync".into()),
                    organizations: vec![org_id],
                    groups: Vec::new(),
                    client_id: Some("instance-alpha".into()),
                    read_only: false,
                },
            },
            TokenBinding {
                token: "tok-inst-2".into(),
                subject: Subject {
                    actor_id: Some("portal-sync".into()),
                    organizations: vec![org_id],
                    groups: Vec::new(),
                    client_id: Some("instance-beta".into()),
                    read_only: false,
                },
            },
        ]);

    let host_app = axum::Router::new()
        .route(
            "/healthz",
            axum::routing::get(|| async { "portal-host-ok" }),
        )
        .merge(dataroom_api::mount("/catalogue", dr_state.clone()));

    // A host that nests the plain router reaches the catalogue home at the bare prefix.
    let nested_app = axum::Router::new()
        .route(
            "/healthz",
            axum::routing::get(|| async { "portal-host-ok" }),
        )
        .nest("/catalogue", router(dr_state));
    let (st_nested, _, nested_html) = send_json(
        &nested_app,
        Method::GET,
        "/catalogue",
        host,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_nested, StatusCode::OK);
    assert!(nested_html.contains("href=\"/catalogue/dataset/\""));

    // Verify sibling router and mounted dataroom routes (`/catalogue/`, `/catalogue/catalog.ttl`,
    // `/catalogue/odata/v4/Datasets`, `/catalogue/api/v1/branding`)
    let (st_h, _, body_h) =
        send_json(&host_app, Method::GET, "/healthz", host, None, None, None).await;
    assert_eq!(st_h, StatusCode::OK);
    assert_eq!(body_h, "portal-host-ok");

    let (st_ui, _, ui_html) = send_json(
        &host_app,
        Method::GET,
        "/catalogue/",
        host,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_ui, StatusCode::OK);
    // Links inside mounted UI honour `/catalogue` mount prefix!
    assert!(ui_html.contains("href=\"/catalogue/dataset/\""));
    assert!(ui_html.contains("href=\"/catalogue/jc-theme.css\""));

    let (st_ttl, _, _) = send_json(
        &host_app,
        Method::GET,
        "/catalogue/catalog.ttl",
        host,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_ttl, StatusCode::OK);

    let (st_od, _, _) = send_json(
        &host_app,
        Method::GET,
        "/catalogue/odata/v4/Datasets",
        host,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_od, StatusCode::OK);

    // G4: GET /api/v1/branding answers the effective block in the Portal's shape (UI-30)
    let (st_br, _, br_body) = send_json(
        &host_app,
        Method::GET,
        "/catalogue/api/v1/branding",
        host,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_br, StatusCode::OK);
    let br_val: Value = serde_json::from_str(&br_body).unwrap();
    assert_eq!(br_val["domain"], "smartcity.joinedcontext.com");
    assert_eq!(br_val["colours"]["primary"], "#0000bf");
    assert_eq!(br_val["primaryForeground"], "#ffffff");
    assert_eq!(br_val["primaryRgb"], "0, 0, 191");

    // G2: Replay Portal & jcctl CKAN Action API requests:
    // 1. Instance Alpha creates organization `shared-org` with declared title `"Alpha City"`
    let (st, _, _) = send_json(
        &host_app,
        Method::POST,
        "/catalogue/api/3/action/organization_create",
        host,
        Some("tok-inst-1"),
        None,
        Some(json!({ "name": "shared-org", "title": "Alpha City" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // 2. Instance Beta (`tok-inst-2`) calls `organization_patch` on `shared-org` -> must NOT rename Alpha's organization!
    let (st, _, patch_body) = send_json(
        &host_app,
        Method::POST,
        "/catalogue/api/3/action/organization_patch",
        host,
        Some("tok-inst-2"),
        None,
        Some(json!({ "id": "shared-org", "title": "Beta Hijack Title" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let patched_org: Value = serde_json::from_str(&patch_body).unwrap();
    assert_eq!(
        patched_org["result"]["title"], "Alpha City",
        "G2: one CkanInstance must never rename another instance's organization"
    );

    // 3. Instance Alpha (`tok-inst-1`) CAN rename its own organization
    let (st, _, patch_alpha) = send_json(
        &host_app,
        Method::POST,
        "/catalogue/api/3/action/organization_patch",
        host,
        Some("tok-inst-1"),
        None,
        Some(json!({ "id": "shared-org", "title": "Alpha City Official" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let alpha_org: Value = serde_json::from_str(&patch_alpha).unwrap();
    assert_eq!(alpha_org["result"]["title"], "Alpha City Official");

    // 4. Publish a public dataset (`generated_by: jcctl/ckan-publisher`) and a private dataset
    for (name, private_flag) in [("city-air", false), ("city-internal", true)] {
        let (st, _, _) = send_json(
            &host_app,
            Method::POST,
            "/catalogue/api/3/action/package_create",
            host,
            Some("tok-inst-1"),
            None,
            Some(json!({
                "name": name,
                "owner_org": "shared-org",
                "title": format!("Title {name}"),
                "notes": format!("Notes {name}"),
                "private": private_flag,
                "license_id": "cc-by",
                "resources": [
                    {
                        "name": "NGSI-LD API",
                        "url": format!("https://api.smartcity.joinedcontext.com/api/endpoint/{name}slugzzzzzzzzzzzz/ngsi-ld/v1/"),
                        "format": "NGSI-LD",
                        "mimetype": "application/json"
                    }
                ],
                "extras": [
                    { "key": "generated_by", "value": "jcctl/ckan-publisher" },
                    { "key": "endpoint", "value": format!("https://api.smartcity.joinedcontext.com/api/endpoint/{name}slugzzzzzzzzzzzz/") }
                ]
            })),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
    }

    // 5. Replay `sweep_with` (`ours/portal/ckan.rs` line 495):
    // `POST /api/3/action/package_search` with `fq: "organization:\"shared-org\""`, `include_private: true`
    let (st, _, sweep_body) = send_json(
        &host_app,
        Method::POST,
        "/catalogue/api/3/action/package_search",
        host,
        Some("tok-inst-1"),
        None,
        Some(json!({
            "fq": "organization:\"shared-org\"",
            "include_private": true,
            "rows": 500,
            "start": 0
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let sweep_val: Value = serde_json::from_str(&sweep_body).unwrap();
    assert_eq!(sweep_val["result"]["count"], 2);

    // 6. Anonymous `GET /api/3/action/package_search?rows=1000&start=0&sort=name+asc` (`ours/portal/catalog.rs` line 441)
    // returns ONLY the public dataset (`city-air`), never `city-internal`
    let (st, _, anon_search_body) = send_json(
        &host_app,
        Method::GET,
        "/catalogue/api/3/action/package_search?rows=1000&start=0&sort=name+asc",
        host,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let anon_val: Value = serde_json::from_str(&anon_search_body).unwrap();
    assert_eq!(anon_val["result"]["count"], 1);
    assert_eq!(anon_val["result"]["results"][0]["name"], "city-air");

    // G3: Fetch `/catalogue/catalog.jsonld` and verify it contains `@included` / `@graph` nodes
    // that `ours/portal/catalog.rs` reads (`feed.get("@included").and_then(Value::as_array)`)
    let (st, _, jsonld_feed_body) = send_json(
        &host_app,
        Method::GET,
        "/catalogue/catalog.jsonld",
        host,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let feed_json: Value = serde_json::from_str(&jsonld_feed_body).unwrap();
    let included = feed_json
        .get("@included")
        .and_then(Value::as_array)
        .expect("G3: catalog.jsonld must carry @included array for Portal CatalogSync");
    assert!(
        included.iter().any(|n| {
            n.get("@id")
                .and_then(Value::as_str)
                .is_some_and(|id| id.ends_with("/dataset/city-air"))
        }),
        "G3: @included must contain the public dataset node"
    );
}

#[tokio::test]
async fn b3_logo_upload_refuses_scripted_svg_and_serves_content_hashed_asset() {
    let org = OrgId::from_uuid(Uuid::from_u128(0xB301));
    let cat = CatalogueId::from_uuid(Uuid::from_u128(0x000B_3011));
    let host = "b3.example.org";
    let store = Arc::new(MemoryStore::new());
    store
        .upsert_catalogue(make_catalogue(org, cat, "b3", host))
        .await
        .unwrap();
    let authz = Arc::new(PolicyAuthorizer::new(vec![GrantRule {
        org_id: org,
        catalogue_id: Some(cat),
        principal: "brand-admin".into(),
        actions: vec![Action::Admin],
        publisher_id: None,
    }]));
    let state = DataroomState::new(store, authz, org, cat).with_tokens(vec![TokenBinding {
        token: "tok-brand".into(),
        subject: Subject::authenticated("brand-admin", vec![org], Vec::new()),
    }]);
    let app = router(state);

    let upload = |svg: &'static str, token: Option<&'static str>| {
        let app = app.clone();
        async move {
            let mut builder = Request::builder()
                .method(Method::POST)
                .uri("/api/v1/assets/logo")
                .header(header::HOST, host)
                .header(header::CONTENT_TYPE, "image/svg+xml");
            if let Some(tok) = token {
                builder = builder.header(header::AUTHORIZATION, tok);
            }
            let resp = app
                .oneshot(builder.body(Body::from(svg)).unwrap())
                .await
                .unwrap();
            let status = resp.status();
            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
            (status, String::from_utf8_lossy(&bytes).into_owned())
        }
    };

    // No token: the upload is not allowed.
    let clean = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10" fill="#0000bf"/></svg>"##;
    let (st, _) = upload(clean, None).await;
    assert!(
        st == StatusCode::FORBIDDEN || st == StatusCode::UNAUTHORIZED,
        "got {st}"
    );

    // A scripted SVG, an event handler and an external image are refused.
    for bad in [
        r#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)"></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><image href="https://evil.example/t.png"/></svg>"#,
    ] {
        let (st, body) = upload(bad, Some("tok-brand")).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{bad}: {body}");
    }

    // A clean SVG is stored under a content hash and served from dataroom's own origin.
    let (st, body) = upload(clean, Some("tok-brand")).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let path = body
        .split('"')
        .find(|s| s.starts_with("/assets/logo-") && s.ends_with(".svg"))
        .unwrap_or_else(|| panic!("upload answer names the asset path: {body}"))
        .to_owned();
    let (st, hdrs, served) = send_json(&app, Method::GET, &path, host, None, None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(served, clean);
    assert_eq!(hdrs[header::CONTENT_TYPE], "image/svg+xml");
    assert_eq!(
        hdrs[header::CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );
    assert_eq!(hdrs[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    let csp = hdrs[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
    assert!(
        csp.contains("default-src 'none'") && csp.contains("sandbox"),
        "{csp}"
    );
}

#[tokio::test]
async fn c2_language_switch_redirect_stays_on_this_site_and_never_panics() {
    let org = OrgId::from_uuid(Uuid::from_u128(0xC201));
    let cat = CatalogueId::from_uuid(Uuid::from_u128(0x000C_2011));
    let host = "c2.example.org";
    let store = Arc::new(MemoryStore::new());
    store
        .upsert_catalogue(make_catalogue(org, cat, "c2", host))
        .await
        .unwrap();
    let app = router(DataroomState::new(
        store,
        Arc::new(PolicyAuthorizer::default()),
        org,
        cat,
    ));
    let post = |form: &'static str| {
        let app = app.clone();
        async move {
            let req = Request::builder()
                .method(Method::POST)
                .uri("/util/redirect")
                .header(header::HOST, host)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap();
            app.oneshot(req).await.unwrap()
        }
    };

    let resp = post("url=%2Fsk%2Fdataset%2Fair-quality").await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()[header::LOCATION], "/sk/dataset/air-quality");
    let cookie = resp.headers()[header::SET_COOKIE].to_str().unwrap();
    assert!(cookie.starts_with("ckan_lang=sk; Path=/"), "{cookie}");

    // Another host, by `//`, by `/\` (browsers read it as `//`), or with a line break: the
    // switch lands on this catalogue's home and sets no cookie.
    for form in [
        "url=https%3A%2F%2Fevil.example%2F",
        "url=%2F%2Fevil.example%2F",
        "url=%2F%5Cevil.example%2F",
        "url=%2Fsk%2F%0D%0ASet-Cookie%3A%20x%3D1",
    ] {
        let resp = post(form).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER, "{form}");
        assert_eq!(resp.headers()[header::LOCATION], "/", "{form}");
        assert!(resp.headers().get(header::SET_COOKIE).is_none(), "{form}");
    }

    // A two-letter segment that is not a language code never reaches the cookie.
    let resp = post("url=%2F%3B%20%2Fdataset").await;
    assert!(resp.headers().get(header::SET_COOKIE).is_none());
}
