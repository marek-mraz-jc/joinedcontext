// SPDX-License-Identifier: EUPL-1.2
//! The service document and `$metadata` over HTTP (T-3506, ADR-0024 decisions 4 and 5): content
//! negotiation, version negotiation, the caller's limits, and the refusals of T-3441 row Q27.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing;
use axum::Router;
use http_body_util::BodyExt;
use odata_axum::{json_level, json_response, metadata_router, ODataRequest, ODataState};
use odata_csdl::{
    CsdlVersion, EntityContainer, EntitySet, EntityType, Model, Property, Schema, TypeRef,
};
use tower::ServiceExt;

const ROOT: &str = "https://example.org/odata/";

fn model() -> Model {
    let mut schema = Schema::new("City");
    let mut building = EntityType::new("Building");
    building.key.push("id".into());
    building
        .properties
        .push(Property::new("id", TypeRef::single("Edm.String")).required());
    building
        .properties
        .push(Property::new("height", TypeRef::single("Edm.Int32")));
    schema.entity_types.push(building);
    let mut container = EntityContainer::new("Service");
    container
        .entity_sets
        .push(EntitySet::new("Buildings", "City.Building"));
    schema.entity_container = Some(container);
    Model {
        version: CsdlVersion::V4_01,
        references: vec![],
        schemas: vec![schema],
    }
}

fn state() -> ODataState {
    ODataState {
        model: Arc::new(model()),
        url_model: None,
        service_root: ROOT.into(),
        url_limits: odata_url::Limits::strict(),
        csdl_limits: odata_csdl::Limits::strict(),
        max_header_len: 64,
        metadata_etag: None,
        custom_options: Vec::new(),
    }
}

fn app(state: ODataState) -> Router {
    Router::new().nest_service("/odata", metadata_router(Arc::new(state)))
}

struct Answer {
    status: StatusCode,
    content_type: String,
    version: Option<String>,
    body: String,
}

async fn get_with(app: Router, uri: &str, headers: &[(&str, &str)]) -> Result<Answer, String> {
    let mut request = Request::get(uri);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request.body(Body::empty()).map_err(|e| e.to_string())?;
    let response = app.oneshot(request).await.map_err(|e| e.to_string())?;
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let (status, content_type, version) = (
        response.status(),
        header("content-type").unwrap_or_default(),
        header("odata-version"),
    );
    let bytes = response
        .into_body()
        .collect()
        .await
        .map_err(|e| e.to_string())?
        .to_bytes();
    Ok(Answer {
        status,
        content_type,
        version,
        body: String::from_utf8(bytes.to_vec()).map_err(|e| e.to_string())?,
    })
}

async fn get(uri: &str, headers: &[(&str, &str)]) -> Result<Answer, String> {
    get_with(app(state()), uri, headers).await
}

const V401: (&str, &str) = ("OData-MaxVersion", "4.01");

#[tokio::test]
async fn the_service_document_lists_the_sets_in_json_and_names_its_version() -> Result<(), String> {
    let answer = get("/odata/", &[]).await?;
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.version.as_deref(), Some("4.0"));
    assert_eq!(
        answer.content_type,
        "application/json;odata.metadata=minimal"
    );
    assert_eq!(
        answer.body,
        r#"{"@odata.context":"https://example.org/odata/$metadata","value":[{"name":"Buildings","kind":"EntitySet","url":"Buildings"}]}"#
    );

    // The ABNF gives the service root no query (`odataRelativeUri`): `?$format` there is 400.
    let answer = get("/odata/?$format=json", &[]).await?;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST);

    let answer = get("/odata", &[V401]).await?;
    assert_eq!(answer.status, StatusCode::OK);
    let answer = get("/odata/", &[V401]).await?;
    assert_eq!(answer.version.as_deref(), Some("4.01"));
    assert_eq!(answer.content_type, "application/json;metadata=minimal");
    assert!(
        answer.body.starts_with(r#"{"@context":"#),
        "{}",
        answer.body
    );
    Ok(())
}

#[tokio::test]
async fn the_service_document_takes_its_metadata_level_and_refuses_other_formats(
) -> Result<(), String> {
    let full = get(
        "/odata/",
        &[V401, ("Accept", "application/json;metadata=full")],
    )
    .await?;
    assert_eq!(full.status, StatusCode::OK);
    assert_eq!(full.content_type, "application/json;metadata=full");

    for (headers, content_type) in [
        (
            vec![("Accept", "application/json;metadata=none")],
            "application/json;odata.metadata=none",
        ),
        (
            vec![("Accept", "application/json;odata.metadata=full")],
            "application/json;odata.metadata=full",
        ),
        (
            vec![V401, ("Accept", "application/json;odata.metadata=none")],
            "application/json;metadata=none",
        ),
        // A parameter without a value and an empty range are skipped, not refused.
        (
            vec![("Accept", "application/json;charset, ,")],
            "application/json;odata.metadata=minimal",
        ),
        (
            vec![("Accept", "text/html;q=0.9, */*;q=0.1")],
            "application/json;odata.metadata=minimal",
        ),
        (
            vec![("Accept", "application/*")],
            "application/json;odata.metadata=minimal",
        ),
    ] {
        let answer = get("/odata/", &headers).await?;
        assert_eq!(answer.status, StatusCode::OK, "{headers:?}");
        assert_eq!(answer.content_type, content_type, "{headers:?}");
    }

    for (uri, headers) in [
        ("/odata/", vec![("Accept", "text/html")]),
        ("/odata/", vec![("Accept", "application/xml")]),
        (
            "/odata/",
            vec![("Accept", "application/json;metadata=most")],
        ),
    ] {
        let answer = get(uri, &headers).await?;
        assert_eq!(
            answer.status,
            StatusCode::NOT_ACCEPTABLE,
            "{uri} {headers:?}"
        );
        assert!(
            answer.body.contains(r#""code":"NotAcceptable""#),
            "{}",
            answer.body
        );
    }
    Ok(())
}

#[tokio::test]
async fn metadata_is_csdl_xml_unless_json_is_asked() -> Result<(), String> {
    for headers in [
        vec![],
        vec![("Accept", "*/*")],
        vec![("Accept", "application/*")],
    ] {
        let answer = get("/odata/$metadata", &headers).await?;
        assert_eq!(answer.status, StatusCode::OK, "{headers:?}");
        assert_eq!(answer.content_type, "application/xml");
        assert!(answer.body.starts_with("<?xml"), "{}", answer.body);
        assert!(
            answer.body.contains(r#"<EntitySet Name="Buildings""#),
            "{}",
            answer.body
        );
    }
    Ok(())
}

#[tokio::test]
async fn metadata_is_csdl_json_on_4_01_with_format_or_accept() -> Result<(), String> {
    for (uri, headers) in [
        ("/odata/$metadata?$format=json", vec![V401]),
        ("/odata/$metadata?$format=application/json", vec![V401]),
        (
            "/odata/$metadata",
            vec![V401, ("Accept", "application/json")],
        ),
        // $format overrides Accept (Protocol 4.01 8.2.1).
        (
            "/odata/$metadata?$format=json",
            vec![V401, ("Accept", "application/xml")],
        ),
        // By quality, not by position.
        (
            "/odata/$metadata",
            vec![
                V401,
                ("Accept", "application/xml;q=0.4, application/json;q=0.9"),
            ],
        ),
        (
            "/odata/$metadata",
            vec![("OData-Version", "4.01"), ("Accept", "application/json")],
        ),
    ] {
        let answer = get(uri, &headers).await?;
        assert_eq!(answer.status, StatusCode::OK, "{uri} {headers:?}");
        assert_eq!(answer.content_type, "application/json");
        assert_eq!(answer.version.as_deref(), Some("4.01"));
        assert!(
            answer.body.starts_with("{\n  \"$Version\": \"4.01\""),
            "{}",
            answer.body
        );
    }

    let xml = get(
        "/odata/$metadata",
        &[V401, ("Accept", "application/json;q=0.2, application/xml")],
    )
    .await?;
    assert_eq!(xml.content_type, "application/xml");
    let xml = get(
        "/odata/$metadata?$format=xml",
        &[V401, ("Accept", "application/json")],
    )
    .await?;
    assert_eq!(xml.content_type, "application/xml");
    Ok(())
}

#[tokio::test]
async fn json_metadata_asked_on_4_0_is_406_and_never_a_document_it_cannot_read(
) -> Result<(), String> {
    for headers in [
        vec![],
        vec![("OData-MaxVersion", "4.0")],
        vec![("OData-Version", "4.01"), ("OData-MaxVersion", "4.0")],
    ] {
        let answer = get("/odata/$metadata?$format=json", &headers).await?;
        assert_eq!(answer.status, StatusCode::NOT_ACCEPTABLE, "{headers:?}");
        assert_eq!(answer.version.as_deref(), Some("4.0"));
        assert!(!answer.body.contains("$Version"), "{}", answer.body);
    }
    Ok(())
}

#[tokio::test]
async fn a_format_that_does_not_decode_is_400() -> Result<(), String> {
    // The URL parser refuses an escape that is no UTF-8 before a format is read (T-3742).
    let answer = get("/odata/$metadata?$format=application%2Fjson%3Bx%3D%FF", &[]).await?;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{}", answer.body);
    Ok(())
}

#[tokio::test]
async fn an_unacceptable_accept_or_format_is_406() -> Result<(), String> {
    for (uri, headers) in [
        ("/odata/$metadata", vec![("Accept", "text/html")]),
        ("/odata/$metadata", vec![("Accept", "application/xml;q=0")]),
        ("/odata/$metadata?$format=atom", vec![]),
        ("/odata/$metadata?$format=text/plain", vec![]),
    ] {
        let answer = get(uri, &headers).await?;
        assert_eq!(
            answer.status,
            StatusCode::NOT_ACCEPTABLE,
            "{uri} {headers:?}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_version_or_header_this_service_cannot_read_is_400() -> Result<(), String> {
    let long = "4".repeat(65);
    for headers in [
        vec![("OData-MaxVersion", "3.0")],
        vec![("OData-Version", "5.0")],
        vec![("OData-Version", "four")],
        vec![("OData-MaxVersion", long.as_str())],
        vec![("Accept", "application/json;q=2")],
        vec![("Accept", long.as_str())],
    ] {
        let answer = get("/odata/$metadata", &headers).await?;
        assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{headers:?}");
        assert!(
            answer.body.contains(r#""code":"BadRequest""#),
            "{}",
            answer.body
        );
        assert!(!answer.body.contains(&long), "no request value is echoed");
    }
    Ok(())
}

#[tokio::test]
async fn unknown_system_and_custom_query_options_are_400_and_not_echoed() -> Result<(), String> {
    for uri in [
        "/odata/$metadata?$foo=1",
        "/odata/$metadata?foo=1",
        "/odata/?foo=1",
        "/odata/$metadata?$top=1",
    ] {
        let answer = get(uri, &[V401]).await?;
        assert_eq!(
            answer.status,
            StatusCode::BAD_REQUEST,
            "{uri}: {}",
            answer.body
        );
        assert_eq!(answer.version.as_deref(), Some("4.01"));
        assert!(!answer.body.contains("foo"), "{}", answer.body);
        assert!(!answer.body.contains("innererror"), "{}", answer.body);
    }
    Ok(())
}

/// A 4.0 document leaves an `Edm.Untyped` property out (CSDL XML 3.4); the URLs resolve
/// against `url_model`, which names it, so it is selectable and an unknown name stays 400.
#[tokio::test]
async fn urls_resolve_against_the_url_model_when_one_is_set() -> Result<(), String> {
    let mut named = model();
    if let Some(building) = named
        .schemas
        .iter_mut()
        .flat_map(|s| s.entity_types.iter_mut())
        .next()
    {
        building
            .properties
            .push(Property::new("notes", TypeRef::single("Edm.Untyped")));
    }
    let wide = ODataState {
        url_model: Some(Arc::new(named)),
        ..state()
    };
    let routes = Router::new()
        .route("/{*rest}", routing::get(echo))
        .with_state(Arc::new(wide));
    let app = || Router::new().nest("/odata", routes.clone());
    let answer = get_with(app(), "/odata/Buildings?$select=notes", &[]).await?;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    let answer = get_with(app(), "/odata/Buildings?$select=width", &[]).await?;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{}", answer.body);
    let plain = Router::new()
        .route("/{*rest}", routing::get(echo))
        .with_state(Arc::new(state()));
    let answer = get_with(
        Router::new().nest("/odata", plain),
        "/odata/Buildings?$select=notes",
        &[],
    )
    .await?;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{}", answer.body);
    Ok(())
}

/// URL 5.2: the custom options the caller names are the only ones accepted, matched by their
/// exact name; every other one stays 400.
#[tokio::test]
async fn only_the_custom_options_the_service_names_are_accepted() -> Result<(), String> {
    let named = ODataState {
        custom_options: vec!["ngsild.q".into()],
        ..state()
    };
    let routes = Router::new()
        .route("/{*rest}", routing::get(echo))
        .with_state(Arc::new(named));
    let app = || Router::new().nest("/odata", routes.clone());
    let answer = get_with(app(), "/odata/Buildings?ngsild.q=height%3E1", &[V401]).await?;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    assert_eq!(answer.body, "4.01 Buildings?ngsild.q=height%3E1");
    let metadata = Router::new().nest_service(
        "/odata",
        metadata_router(Arc::new(ODataState {
            custom_options: vec!["ngsild.q".into()],
            ..state()
        })),
    );
    let answer = get_with(metadata, "/odata/$metadata?ngsild.q=a", &[]).await?;
    assert_eq!(
        answer.status,
        StatusCode::BAD_REQUEST,
        "a named option is a resource's"
    );
    for uri in [
        "/odata/Buildings?ngsild.q=a&mine=yes",
        "/odata/Buildings?NGSILD.q=a",
        "/odata/Buildings?ngsild.qq=a",
        "/odata/$entity?$id=Buildings('b1')&ngsild.q=a",
    ] {
        let answer = get_with(app(), uri, &[]).await?;
        assert_eq!(
            answer.status,
            StatusCode::BAD_REQUEST,
            "{uri}: {}",
            answer.body
        );
        assert!(!answer.body.contains("mine") && !answer.body.contains("NGSILD"));
    }
    Ok(())
}

#[tokio::test]
async fn the_callers_limits_hold_on_every_request() -> Result<(), String> {
    let short = ODataState {
        url_limits: odata_url::Limits {
            max_query_len: 4,
            ..odata_url::Limits::strict()
        },
        ..state()
    };
    let answer = get_with(app(short), "/odata/$metadata?$format=json", &[V401]).await?;
    assert_eq!(answer.status, StatusCode::URI_TOO_LONG);
    assert!(
        answer.body.contains(r#""code":"URITooLong""#),
        "{}",
        answer.body
    );

    let small = ODataState {
        csdl_limits: odata_csdl::Limits {
            max_elements: 1,
            ..odata_csdl::Limits::strict()
        },
        ..state()
    };
    for format in ["xml", "json"] {
        let answer = get_with(
            app(small.clone()),
            &format!("/odata/$metadata?$format={format}"),
            &[V401],
        )
        .await?;
        assert_eq!(answer.status, StatusCode::INTERNAL_SERVER_ERROR, "{format}");
        assert_eq!(
            answer.body,
            r#"{"error":{"code":"InternalError","message":"the service model could not be written"}}"#
        );
    }
    Ok(())
}

async fn echo(request: ODataRequest) -> String {
    format!("{} {}", request.version, request.uri)
}

/// The extractor a surface adds its own routes with: the URL relative to the nest, parsed
/// against the model, every refusal an OData error with its status.
#[tokio::test]
async fn the_extractor_parses_below_the_service_root_and_answers_refusals_with_their_status(
) -> Result<(), String> {
    let routes = Router::new()
        .route("/{*rest}", routing::get(echo))
        .with_state(Arc::new(state()));
    let app = || Router::new().nest("/odata", routes.clone());

    let answer = get_with(app(), "/odata/Buildings('b1')?$top=2", &[V401]).await?;
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.body, "4.01 Buildings('b1')?$top=2");

    for (uri, status) in [
        ("/odata/Towers", StatusCode::NOT_FOUND),
        (
            "/odata/Buildings?$filter=width%20eq%201",
            StatusCode::BAD_REQUEST,
        ),
        ("/odata/Buildings?$index=1", StatusCode::NOT_IMPLEMENTED),
        ("/odata/Buildings?$top=1&$top=2", StatusCode::BAD_REQUEST),
        ("/odata/Buildings?$top=x", StatusCode::BAD_REQUEST),
        ("/odata/Buildings?mine=yes", StatusCode::BAD_REQUEST),
    ] {
        let answer = get_with(app(), uri, &[]).await?;
        assert_eq!(answer.status, status, "{uri}: {}", answer.body);
        assert_eq!(answer.content_type, "application/json");
        assert!(!answer.body.contains("Towers") && !answer.body.contains("width"));
    }

    let deep = ODataState {
        url_limits: odata_url::Limits {
            max_orderby_items: 1,
            ..odata_url::Limits::strict()
        },
        ..state()
    };
    let routes = Router::new()
        .route("/{*rest}", routing::get(echo))
        .with_state(Arc::new(deep));
    let answer = get_with(
        Router::new().nest("/odata", routes),
        "/odata/Buildings?$orderby=height,id",
        &[],
    )
    .await?;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    assert!(
        answer.body.contains(r#""code":"LimitExceeded""#),
        "{}",
        answer.body
    );
    Ok(())
}

async fn level(request: ODataRequest, headers: axum::http::HeaderMap) -> axum::response::Response {
    match json_level(&request, &headers, 64) {
        Ok(level) => json_response(request.version, level, b"{}".to_vec()),
        Err(refused) => axum::response::IntoResponse::into_response(refused),
    }
}

/// A surface's own resource: JSON at the level `$format` or `Accept` asks, never XML.
#[tokio::test]
async fn a_resource_is_json_at_the_level_asked_and_xml_is_406() -> Result<(), String> {
    let routes = Router::new()
        .route("/{*rest}", routing::get(level))
        .with_state(Arc::new(state()));
    let app = || Router::new().nest("/odata", routes.clone());
    for (uri, headers, status, content_type) in [
        (
            "/odata/Buildings",
            &[][..],
            StatusCode::OK,
            "application/json;odata.metadata=minimal",
        ),
        (
            "/odata/Buildings",
            &[V401][..],
            StatusCode::OK,
            "application/json;metadata=minimal",
        ),
        (
            "/odata/Buildings?$format=application/json;metadata=full",
            &[V401][..],
            StatusCode::OK,
            "application/json;metadata=full",
        ),
        (
            // Go's url.QueryEscape: read decoded (T-3742).
            "/odata/Buildings?$format=application%2Fjson%3Bodata.metadata%3Dnone",
            &[][..],
            StatusCode::OK,
            "application/json;odata.metadata=none",
        ),
        (
            "/odata/Buildings",
            &[("Accept", "application/json;odata.metadata=none")][..],
            StatusCode::OK,
            "application/json;odata.metadata=none",
        ),
        (
            "/odata/Buildings?$format=xml",
            &[][..],
            StatusCode::NOT_ACCEPTABLE,
            "application/json",
        ),
        (
            "/odata/Buildings",
            &[("Accept", "application/xml")][..],
            StatusCode::NOT_ACCEPTABLE,
            "application/json",
        ),
        (
            "/odata/Buildings",
            &[("Accept", "text/plain")][..],
            StatusCode::NOT_ACCEPTABLE,
            "application/json",
        ),
    ] {
        let answer = get_with(app(), uri, headers).await?;
        assert_eq!(answer.status, status, "{uri} {headers:?}: {}", answer.body);
        assert_eq!(answer.content_type, content_type, "{uri} {headers:?}");
        assert!(
            answer.version.is_some(),
            "{uri}: OData-Version on every answer"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_write_on_the_documents_is_an_odata_405() -> Result<(), String> {
    for (method, uri) in [
        ("POST", "/odata/"),
        ("PUT", "/odata/$metadata"),
        ("DELETE", "/odata/$metadata"),
    ] {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("OData-MaxVersion", "4.01")
            .body(Body::empty())
            .map_err(|e| e.to_string())?;
        let response = app(state())
            .oneshot(request)
            .await
            .map_err(|e| e.to_string())?;
        assert_eq!(
            response.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "{method} {uri}"
        );
        assert_eq!(response.headers()["allow"], "GET", "{method} {uri}");
        assert_eq!(
            response.headers()["odata-version"],
            "4.01",
            "{method} {uri}"
        );
        let bytes = response
            .into_body()
            .collect()
            .await
            .map_err(|e| e.to_string())?
            .to_bytes();
        let body = String::from_utf8_lossy(&bytes);
        assert!(
            body.contains(r#""code":"MethodNotAllowed""#),
            "{method} {uri}: {body}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn metadata_carries_a_strong_etag_per_serialisation_and_revalidates() -> Result<(), String> {
    let head = |uri: &'static str, headers: Vec<(&'static str, String)>| async move {
        let mut request = Request::get(uri).header("OData-MaxVersion", "4.01");
        for (k, v) in headers {
            request = request.header(k, v);
        }
        let request = request.body(Body::empty()).map_err(|e| e.to_string())?;
        let response = app(state())
            .oneshot(request)
            .await
            .map_err(|e| e.to_string())?;
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_owned()
        };
        let (status, etag, cache) = (response.status(), header("etag"), header("cache-control"));
        let bytes = response
            .into_body()
            .collect()
            .await
            .map_err(|e| e.to_string())?
            .to_bytes();
        Ok::<_, String>((status, etag, cache, bytes.len()))
    };
    let (status, xml_tag, cache, _) = head("/odata/$metadata", vec![]).await?;
    assert_eq!(status, StatusCode::OK);
    assert!(
        xml_tag.starts_with('"') && xml_tag.ends_with('"') && xml_tag.len() == 18,
        "{xml_tag}"
    );
    assert_eq!(cache, "private, no-cache");
    let (_, again, ..) = head("/odata/$metadata", vec![]).await?;
    assert_eq!(again, xml_tag, "the same bytes, the same tag");
    let (_, json_tag, ..) = head("/odata/$metadata?$format=json", vec![]).await?;
    assert_ne!(json_tag, xml_tag, "one tag per serialisation");
    for matching in [
        xml_tag.clone(),
        format!("W/{xml_tag}"),
        format!("\"x\", {xml_tag}"),
        "*".to_owned(),
    ] {
        let (status, tag, cache, len) = head(
            "/odata/$metadata",
            vec![("If-None-Match", matching.clone())],
        )
        .await?;
        assert_eq!((status, len), (StatusCode::NOT_MODIFIED, 0), "{matching}");
        assert_eq!(
            (tag.as_str(), cache.as_str()),
            (xml_tag.as_str(), "private, no-cache")
        );
    }
    let (status, ..) = head("/odata/$metadata", vec![("If-None-Match", json_tag)]).await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "another document's tag does not match"
    );
    Ok(())
}

#[tokio::test]
async fn the_service_document_names_the_metadata_documents_tag() -> Result<(), String> {
    let tag = odata_axum::metadata_etag(&model(), &odata_csdl::Limits::strict());
    let tagged = ODataState {
        metadata_etag: tag.clone(),
        ..state()
    };
    let xml = get_with(app(tagged.clone()), "/odata/$metadata", &[]).await?;
    let request = Request::get("/odata/$metadata")
        .body(Body::empty())
        .map_err(|e| e.to_string())?;
    let response = app(tagged.clone())
        .oneshot(request)
        .await
        .map_err(|e| e.to_string())?;
    let header = response.headers().get("etag").and_then(|v| v.to_str().ok());
    assert_eq!(header, tag.as_deref(), "the XML document's own tag");
    assert_eq!(xml.status, StatusCode::OK);
    let service = get_with(app(tagged), "/odata/", &[]).await?;
    let written = format!(
        r#""@odata.metadataEtag":"{}""#,
        tag.as_deref().unwrap_or("none").replace('"', "\\\"")
    );
    assert!(service.body.contains(&written), "{}", service.body);
    let past_limits = odata_csdl::Limits {
        max_elements: 1,
        ..odata_csdl::Limits::strict()
    };
    assert_eq!(
        odata_axum::metadata_etag(&model(), &past_limits),
        None,
        "no document, no tag"
    );
    Ok(())
}
