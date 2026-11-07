//! The app's own contract with its Endpoint and the Portal's `/me` (T-0310, T-2617, AP-04,
//! AP-28, AP-39, AP-40, AP-62, AP-109, GW10).
//!
//! Every case runs the real router against a stubbed endpoint, so what is asserted is the
//! request that actually leaves the pod: which URL, which token, which method. The point of
//! the app is that it adds nothing to the caller's authority, and that is only observable
//! from outside the process.

use std::sync::Arc;

use air_quality::{router, App, Config};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const STATION: &str = "urn:ngsi-ld:AirQualityObserved:hel.fi:air-quality:station-01";
const BASE: &str = "/apps/air-quality/";
// Not shaped like a JWT on purpose: a fixture that merely looks like a credential stops the
// secret scan for no reason, and what these tests assert is that the value is carried through
// unchanged, not what is inside it.
const TOKEN: &str = "the-forwarded-access-token-of-demo-steward";

fn app_at(endpoint: &MockServer) -> axum::Router {
    router(Arc::new(App::new(Config {
        base_path: BASE.to_owned(),
        endpoint_url: format!("{}/", endpoint.uri()),
        anonymous: false,
        me_url: Some(format!("{}/me", endpoint.uri())),
        page_config: None,
    })))
}

fn entity() -> Value {
    json!({
        "id": STATION,
        "type": "AirQualityObserved",
        "name": { "type": "LanguageProperty", "languageMap": { "fi": "Kallio 2", "en": "Kallio" } },
        "pm10": { "type": "Property", "value": 34.2, "observedAt": "2026-09-06T10:00:00Z" },
        "pm25": { "type": "Property", "value": 21.0 },
        "location": {
            "type": "GeoProperty",
            "value": { "type": "Point", "coordinates": [19.146, 48.736] }
        }
    })
}

/// A request as the edge delivers it (AP-28): the user's access token in `X-Access-Token` and
/// the userinfo, base64 JSON, in `X-Userinfo`.
fn signed_in(method: &str, path: &str, body: Option<Value>) -> Request<Body> {
    use base64::Engine as _;
    let userinfo = base64::engine::general_purpose::STANDARD.encode(
        json!({
            "sub": "f:1:demo.steward",
            "preferred_username": "demo.steward",
            "email": "demo.steward@hel.fi",
            "name": "Demo Steward",
        })
        .to_string(),
    );
    let builder = Request::builder()
        .method(method)
        .uri(path)
        .header("x-access-token", TOKEN)
        .header("x-userinfo", userinfo);
    match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("a request"),
        None => builder.body(Body::empty()).expect("a request"),
    }
}

fn anonymous(method: &str, path: &str, body: Option<Value>) -> Request<Body> {
    let builder = Request::builder().method(method).uri(path);
    match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("a request"),
        None => builder.body(Body::empty()).expect("a request"),
    }
}

async fn call(app: axum::Router, request: Request<Body>) -> (StatusCode, String) {
    let response = app.oneshot(request).await.expect("the app answers");
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("a readable body")
        .to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

/// Mocks the Portal's `/me`, which answers the caller's roles in the App (AP-109).
async fn roles(endpoint: &MockServer, roles: &[&str]) {
    Mock::given(method("GET"))
        .and(path("/me"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "f:1:demo.steward", "name": "Demo Steward", "roles": roles,
        })))
        .mount(endpoint)
        .await;
}

/// Nothing left the pod: the check this app makes before it forwards anything.
async fn untouched(endpoint: &MockServer) -> bool {
    endpoint
        .received_requests()
        .await
        .is_some_and(|requests| requests.is_empty())
}

#[tokio::test]
async fn the_station_list_is_read_with_the_users_own_token_and_flattened_for_the_browser() {
    let endpoint = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ngsi-ld/v1/entities"))
        .and(query_param("type", "AirQualityObserved"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([entity()])))
        .expect(1)
        .mount(&endpoint)
        .await;

    let (status, body) = call(
        app_at(&endpoint),
        signed_in("GET", &format!("{BASE}api/stations"), None),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let stations: Value = serde_json::from_str(&body).expect("a list");
    assert_eq!(stations[0]["id"], json!(STATION));
    assert_eq!(stations[0]["pm10"], json!(34.2));
    assert_eq!(stations[0]["coordinates"], json!([19.146, 48.736]));
    assert_eq!(stations[0]["name"], json!("Kallio"));
    assert_eq!(
        stations[0]["names"],
        json!({ "fi": "Kallio 2", "en": "Kallio" })
    );
    // No `source`: a station a steward added, which the page offers to remove.
    assert_eq!(stations[0]["own"], json!(true));
}

/// AP-40, the property this whole app exists to demonstrate. Without `X-Access-Token` there is
/// no write of any kind, and no second attempt without one either.
#[tokio::test]
async fn a_write_without_an_access_token_is_401_and_never_reaches_the_endpoint() {
    for (verb, uri, body) in [
        (
            "POST",
            format!("{BASE}api/stations"),
            Some(json!({ "localId": "s9", "name": { "fi": "Uusi" }, "coordinates": [24.9, 60.2] })),
        ),
        (
            "PATCH",
            format!("{BASE}api/stations/{STATION}"),
            Some(json!({ "stewardNote": "Sensor cleaned." })),
        ),
        ("DELETE", format!("{BASE}api/stations/{STATION}"), None),
    ] {
        let endpoint = MockServer::start().await;
        let (status, body) = call(app_at(&endpoint), anonymous(verb, &uri, body)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{verb}: {body}");
        assert!(body.contains("signed-in"), "{body}");
        assert!(
            untouched(&endpoint).await,
            "{verb}: the app must not ask the endpoint on the caller's behalf"
        );
    }
}

/// AP-62: a correction is one PATCH of exactly the attributes the form sent, with the user's
/// token and nothing of the app's own.
#[tokio::test]
async fn a_correction_is_one_patch_of_the_sent_attributes_with_the_forwarded_token() {
    let endpoint = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path(format!("/ngsi-ld/v1/entities/{STATION}/attrs")))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .and(body_json(json!({
            "name": { "type": "LanguageProperty", "languageMap": { "en": "Kallio", "fi": "Kallio" } },
            "location": { "type": "GeoProperty", "value": { "type": "Point", "coordinates": [24.95, 60.18] } },
            "stewardNote": { "type": "Property", "value": "Sensor cleaned." },
        })))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&endpoint)
        .await;

    let (status, body) = call(
        app_at(&endpoint),
        signed_in(
            "PATCH",
            &format!("{BASE}api/stations/{STATION}"),
            Some(json!({
                // An empty language is left out rather than written as "".
                "name": { "fi": " Kallio ", "en": "Kallio", "sv": "" },
                "coordinates": [24.95, 60.18],
                "stewardNote": "  Sensor cleaned.  ",
            })),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
}

/// Measured values are never writable: the form's body has no field for them, and one sent
/// anyway is refused before anything leaves the pod.
#[tokio::test]
async fn a_measured_value_or_the_source_is_refused_by_the_app() {
    for field in [
        "pm10",
        "pm25",
        "airQualityIndex",
        "dateObserved",
        "source",
        "id",
    ] {
        let endpoint = MockServer::start().await;
        let (status, body) = call(
            app_at(&endpoint),
            signed_in(
                "PATCH",
                &format!("{BASE}api/stations/{STATION}"),
                Some(json!({ field: 1, "stewardNote": "x" })),
            ),
        )
        .await;
        assert!(status.is_client_error(), "{field}: {status} {body}");
        assert!(untouched(&endpoint).await, "{field} reached the endpoint");
    }
}

/// What the app refuses itself, each before a request leaves the pod.
#[tokio::test]
async fn an_invalid_field_is_refused_by_the_app_and_not_by_the_broker() {
    for (verb, uri, sent) in [
        (
            "PATCH",
            format!("{BASE}api/stations/{STATION}"),
            json!({ "stewardNote": "   " }),
        ),
        (
            "PATCH",
            format!("{BASE}api/stations/{STATION}"),
            json!({ "stewardNote": "x".repeat(501) }),
        ),
        ("PATCH", format!("{BASE}api/stations/{STATION}"), json!({})),
        (
            "PATCH",
            format!("{BASE}api/stations/{STATION}"),
            json!({ "name": { "fi": "  " } }),
        ),
        (
            "PATCH",
            format!("{BASE}api/stations/{STATION}"),
            json!({ "name": { "FI; x": "Kallio" } }),
        ),
        (
            "PATCH",
            format!("{BASE}api/stations/{STATION}"),
            json!({ "coordinates": [181.0, 60.0] }),
        ),
        (
            "PATCH",
            format!("{BASE}api/stations/{STATION}"),
            json!({ "coordinates": [24.9, -91.0] }),
        ),
        (
            "PATCH",
            format!("{BASE}api/stations/{STATION}"),
            json!({ "localId": "other" }),
        ),
        (
            "POST",
            format!("{BASE}api/stations"),
            json!({ "localId": "../x", "name": { "fi": "A" }, "coordinates": [24.9, 60.2] }),
        ),
        (
            "POST",
            format!("{BASE}api/stations"),
            json!({ "localId": "", "name": { "fi": "A" }, "coordinates": [24.9, 60.2] }),
        ),
        (
            "POST",
            format!("{BASE}api/stations"),
            json!({ "localId": "s9", "coordinates": [24.9, 60.2] }),
        ),
        (
            "POST",
            format!("{BASE}api/stations"),
            json!({ "localId": "s9", "name": { "fi": "A" } }),
        ),
    ] {
        let endpoint = MockServer::start().await;
        let (status, body) =
            call(app_at(&endpoint), signed_in(verb, &uri, Some(sent.clone()))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{sent}: {body}");
        assert!(untouched(&endpoint).await, "{sent} reached the endpoint");
    }
}

/// A new station is one POST under the organization and space of the stations the endpoint
/// already serves, with the id the steward chose as its last segment.
#[tokio::test]
async fn a_new_station_is_one_post_under_the_spaces_own_id_prefix() {
    let endpoint = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ngsi-ld/v1/entities"))
        .and(query_param("limit", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([entity()])))
        .mount(&endpoint)
        .await;
    let id = "urn:ngsi-ld:AirQualityObserved:hel.fi:air-quality:kumpula-2";
    Mock::given(method("POST"))
        .and(path("/ngsi-ld/v1/entities"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .and(body_json(json!({
            "id": id,
            "type": "AirQualityObserved",
            "name": { "type": "LanguageProperty", "languageMap": { "fi": "Kumpula" } },
            "location": { "type": "GeoProperty", "value": { "type": "Point", "coordinates": [24.96, 60.2] } },
        })))
        .respond_with(ResponseTemplate::new(201))
        .expect(1)
        .mount(&endpoint)
        .await;

    let (status, body) = call(
        app_at(&endpoint),
        signed_in(
            "POST",
            &format!("{BASE}api/stations"),
            Some(json!({ "localId": "kumpula-2", "name": { "fi": "Kumpula" }, "coordinates": [24.96, 60.2] })),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).expect("json")["id"],
        json!(id)
    );
}

/// With no station to read the prefix from the app says so, and guesses no id.
#[tokio::test]
async fn a_new_station_in_an_empty_space_is_409_and_writes_nothing() {
    let endpoint = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ngsi-ld/v1/entities"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&endpoint)
        .await;

    let (status, body) = call(
        app_at(&endpoint),
        signed_in(
            "POST",
            &format!("{BASE}api/stations"),
            Some(json!({ "localId": "s9", "name": { "fi": "A" }, "coordinates": [24.9, 60.2] })),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let requests = endpoint.received_requests().await.expect("recorded");
    assert!(requests
        .iter()
        .all(|request| request.method.as_str() == "GET"));
}

/// A removal is one DELETE with the user's token; which station may go is the gateway's call.
#[tokio::test]
async fn a_removal_is_one_delete_with_the_forwarded_token() {
    let endpoint = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(format!("/ngsi-ld/v1/entities/{STATION}")))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&endpoint)
        .await;

    let (status, body) = call(
        app_at(&endpoint),
        signed_in("DELETE", &format!("{BASE}api/stations/{STATION}"), None),
    )
    .await;

    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
}

/// GW10, AP-40: the gateway refuses a signed-in viewer, and the app repeats the refusal rather
/// than paraphrasing it, for every kind of write.
#[tokio::test]
async fn the_endpoints_refusal_reaches_the_browser_word_for_word() {
    let problem = json!({
        "type": "https://joinedcontext.com/errors/forbidden",
        "title": "Forbidden",
        "status": 403,
        "detail": "updateAttrs on AirQualityObserved needs the steward role",
    });
    for (verb, upstream, body) in [
        (
            "PATCH",
            format!("/ngsi-ld/v1/entities/{STATION}/attrs"),
            Some(json!({ "stewardNote": "x" })),
        ),
        ("DELETE", format!("/ngsi-ld/v1/entities/{STATION}"), None),
    ] {
        let endpoint = MockServer::start().await;
        Mock::given(method(verb))
            .and(path(upstream))
            .respond_with(
                ResponseTemplate::new(403)
                    .set_body_json(problem.clone())
                    .insert_header("content-type", "application/problem+json"),
            )
            .mount(&endpoint)
            .await;

        let (status, shown) = call(
            app_at(&endpoint),
            signed_in(verb, &format!("{BASE}api/stations/{STATION}"), body),
        )
        .await;

        assert_eq!(status, StatusCode::FORBIDDEN, "{verb}");
        assert_eq!(
            serde_json::from_str::<Value>(&shown).expect("a problem"),
            problem
        );
    }
}

/// AP-109: the roles come from the Portal's `/me`, asked with the forwarded token, and the
/// identity from the edge.
#[tokio::test]
async fn the_roles_come_from_the_portal_and_the_identity_from_the_edge() {
    let endpoint = MockServer::start().await;
    roles(&endpoint, &["steward"]).await;

    let (status, body) = call(
        app_at(&endpoint),
        signed_in("GET", &format!("{BASE}api/me"), None),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let me: Value = serde_json::from_str(&body).expect("an identity");
    assert_eq!(me["signedIn"], json!(true));
    assert_eq!(me["email"], json!("demo.steward@hel.fi"));
    assert_eq!(me["user"], json!("demo.steward"));
    assert_eq!(me["roles"], json!(["steward"]));
}

/// The pod's readiness probe: at the root, outside the base path, and never a data call.
#[tokio::test]
async fn the_readiness_probe_answers_at_the_root() {
    let endpoint = MockServer::start().await;
    let (status, body) = call(app_at(&endpoint), anonymous("GET", "/healthz", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "ok");
    assert!(untouched(&endpoint).await);
}

/// AP-67, AP-126: the front page carries the reconciler's `JC_APP_CONFIG` as `#jc-config`, so
/// the map finds the project's basemap; a value that spells `</script>` cannot end the element.
#[tokio::test]
async fn the_front_page_carries_the_reconcilers_configuration_with_its_basemap() {
    let endpoint = MockServer::start().await;
    let basemap = "https://portal.hel.fi/api/v1/projects/hel/basemap/default/style.json";
    let raw =
        json!({ "slug": "s", "appName": "</script><script>alert(1)</script>", "basemap": basemap });
    let app = router(Arc::new(App::new(Config {
        base_path: BASE.to_owned(),
        endpoint_url: format!("{}/", endpoint.uri()),
        anonymous: true,
        me_url: None,
        page_config: Some(air_quality::page_config(&raw.to_string()).expect("an object")),
    })));

    let (status, page) = call(app, anonymous("GET", BASE, None)).await;
    assert_eq!(status, StatusCode::OK);
    let open = r#"<script id="jc-config" type="application/json">"#;
    let start = page.find(open).expect("the page carries #jc-config") + open.len();
    let end = start
        + page[start..]
            .find("</script>")
            .expect("the element is closed");
    let config: Value = serde_json::from_str(&page[start..end]).expect("the element holds JSON");
    assert_eq!(config["basemap"], basemap);
    assert_eq!(
        config["appName"], raw["appName"],
        "read back as it was handed over"
    );
    assert!(untouched(&endpoint).await);

    // Without one the page is served as built.
    let (_, plain) = call(app_at(&endpoint), anonymous("GET", BASE, None)).await;
    assert!(!plain.contains("jc-config"), "{plain}");
}

#[test]
fn a_configuration_that_is_not_one_json_object_is_refused() {
    for raw in ["not json", "[1]", "\"basemap\""] {
        assert!(air_quality::page_config(raw).is_err(), "{raw} was accepted");
    }
}

/// A userinfo header that is not base64 JSON is nobody, not an error: the token still decides.
#[tokio::test]
async fn an_unreadable_userinfo_is_no_identity() {
    let endpoint = MockServer::start().await;
    roles(&endpoint, &[]).await;
    let request = Request::builder()
        .method("GET")
        .uri(format!("{BASE}api/me"))
        .header("x-access-token", TOKEN)
        .header("x-userinfo", "not base64 at all")
        .body(Body::empty())
        .expect("a request");

    let (status, body) = call(app_at(&endpoint), request).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let me: Value = serde_json::from_str(&body).expect("an identity");
    assert_eq!(me["signedIn"], json!(true));
    assert_eq!(me["email"], json!(null));
    assert_eq!(me["user"], json!(null));
}

/// A viewer holds a role and still gets no form; a Portal that cannot answer means no role
/// (fail closed).
#[tokio::test]
async fn a_viewer_or_an_unanswered_me_holds_no_steward_role() {
    let endpoint = MockServer::start().await;
    roles(&endpoint, &["viewer"]).await;
    let (_, body) = call(
        app_at(&endpoint),
        signed_in("GET", &format!("{BASE}api/me"), None),
    )
    .await;
    let me: Value = serde_json::from_str(&body).expect("an identity");
    assert_eq!(me["roles"], json!(["viewer"]));

    let down = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/me"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&down)
        .await;
    let (_, body) = call(
        app_at(&down),
        signed_in("GET", &format!("{BASE}api/me"), None),
    )
    .await;
    let me: Value = serde_json::from_str(&body).expect("an identity");
    assert_eq!(me["signedIn"], json!(true));
    assert_eq!(me["roles"], json!([]));
}

/// An anonymous reader never even asks: no token, no roles, no round trip.
#[tokio::test]
async fn an_anonymous_reader_asks_the_portal_nothing() {
    let endpoint = MockServer::start().await;

    let (_, body) = call(
        app_at(&endpoint),
        anonymous("GET", &format!("{BASE}api/me"), None),
    )
    .await;

    let me: Value = serde_json::from_str(&body).expect("an identity");
    assert_eq!(me["signedIn"], json!(false));
    assert_eq!(me["roles"], json!([]));
    assert!(untouched(&endpoint).await);
}

/// An id is a path segment upstream, so a caller cannot use it to reach another URL.
#[tokio::test]
async fn an_id_that_is_not_a_urn_is_refused_before_anything_leaves_the_pod() {
    let endpoint = MockServer::start().await;

    let (status, _) = call(
        app_at(&endpoint),
        signed_in(
            "PATCH",
            &format!("{BASE}api/stations/urn:ngsi-ld:x%2f..%2f..%2fadmin"),
            Some(json!({ "stewardNote": "hello" })),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(untouched(&endpoint).await);
}

#[tokio::test]
async fn the_history_asks_the_temporal_surface_for_the_last_day() {
    let endpoint = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/ngsi-ld/v1/temporal/entities/{STATION}")))
        .and(query_param("timerel", "after"))
        .and(query_param("attrs", "pm10,pm25"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": STATION })))
        .expect(1)
        .mount(&endpoint)
        .await;

    let (status, body) = call(
        app_at(&endpoint),
        signed_in(
            "GET",
            &format!("{BASE}api/stations/{STATION}/history"),
            None,
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
}

/// The app lives under its base path and owns nothing above it: the edge routes one prefix,
/// and a request outside it is not this app's to answer.
#[tokio::test]
async fn nothing_is_served_above_the_apps_own_base_path() {
    let endpoint = MockServer::start().await;
    let (status, _) = call(app_at(&endpoint), anonymous("GET", "/api/stations", None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    for front in [BASE, BASE.trim_end_matches('/')] {
        let (status, body) = call(app_at(&endpoint), anonymous("GET", front, None)).await;
        assert_eq!(status, StatusCode::OK, "{front}");
        assert!(body.contains("<!doctype html>"), "{body}");
    }
}

/// T-2909: on its own host the base path is `/`; the router must build there (it panicked at
/// start on dev with an empty route) and serve the station list under it.
#[tokio::test]
async fn the_app_starts_on_its_own_host_at_the_root() {
    let endpoint = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ngsi-ld/v1/entities"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([entity()])))
        .mount(&endpoint)
        .await;
    let app = router(Arc::new(App::new(Config {
        base_path: "/".to_owned(),
        endpoint_url: format!("{}/", endpoint.uri()),
        anonymous: false,
        me_url: Some(format!("{}/me", endpoint.uri())),
        page_config: None,
    })));
    let (status, _) = call(
        app.clone(),
        Request::builder()
            .uri("/healthz")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call(app, signed_in("GET", "/api/stations", None)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// T-3374: the configuration a pod is started with, read without the process's environment.
#[test]
fn the_configuration_is_read_from_the_pods_variables() {
    let vars = |pairs: &'static [(&'static str, &'static str)]| {
        move |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    };
    assert_eq!(
        Config::from_vars(vars(&[])).err().as_deref(),
        Some("JC_ENDPOINT_URL is not set")
    );
    let plain = Config::from_vars(vars(&[("JC_ENDPOINT_URL", "https://h/api/endpoint/air")]))
        .expect("a configuration");
    assert_eq!(plain.endpoint_url, "https://h/api/endpoint/air/");
    assert_eq!(plain.base_path, "/");
    assert!(!plain.anonymous);
    assert_eq!(plain.me_url, None);
    assert_eq!(plain.page_config, None);

    let full = Config::from_vars(vars(&[
        ("JC_ENDPOINT_URL", "https://h/api/endpoint/air/"),
        ("JC_BASE_PATH", BASE),
        ("JC_ANONYMOUS", "true"),
        ("JC_ME_URL", ""),
        ("JC_APP_CONFIG", r#"{"basemap":"https://h/style.json"}"#),
    ]))
    .expect("a configuration");
    assert_eq!(full.endpoint_url, "https://h/api/endpoint/air/");
    assert_eq!(full.base_path, BASE);
    assert!(full.anonymous);
    assert_eq!(full.me_url, None, "an empty /me URL is none");
    assert!(full
        .page_config
        .expect("the page config")
        .contains("basemap"));

    let blank = Config::from_vars(vars(&[
        ("JC_ENDPOINT_URL", "https://h/"),
        ("JC_APP_CONFIG", "  "),
    ]))
    .expect("a configuration");
    assert_eq!(blank.page_config, None);
    assert!(Config::from_vars(vars(&[
        ("JC_ENDPOINT_URL", "https://h/"),
        ("JC_APP_CONFIG", "[1]")
    ]))
    .is_err());
    assert!(
        !Config::from_vars(vars(&[
            ("JC_ENDPOINT_URL", "https://h/"),
            ("JC_ANONYMOUS", "yes")
        ]))
        .expect("a configuration")
        .anonymous
    );
}

/// A file the bundle does not have is a 404; a path without an extension is a page of the App.
#[tokio::test]
async fn a_missing_file_is_404_and_a_client_route_is_the_front_page() {
    let endpoint = MockServer::start().await;
    let (status, _) = call(
        app_at(&endpoint),
        anonymous("GET", &format!("{BASE}assets/missing-1a2b.js"), None),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = call(
        app_at(&endpoint),
        anonymous("GET", &format!("{BASE}stations/kallio"), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<!doctype html>"), "{body}");
}

/// An endpoint that does not answer is a 502 the browser can say, not a hang or a panic.
#[tokio::test]
async fn an_endpoint_that_does_not_answer_is_a_502() {
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
        listener.local_addr().expect("its address")
    };
    let app = router(Arc::new(App::new(Config {
        base_path: BASE.to_owned(),
        endpoint_url: format!("http://{closed}/"),
        anonymous: false,
        me_url: Some(format!("http://{closed}/me")),
        page_config: None,
    })));
    let (status, body) = call(
        app.clone(),
        signed_in("GET", &format!("{BASE}api/stations"), None),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(body.contains("the endpoint did not answer"), "{body}");
    // Nobody could be asked for the roles: the identity holds none (fail closed).
    let (status, body) = call(app, signed_in("GET", &format!("{BASE}api/me"), None)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(r#""roles":[]"#), "{body}");
}

/// A read the endpoint refuses, or answers with something that is not JSON, reaches the browser
/// as the refusal or a 502; a station's day under a bad id never leaves the pod.
#[tokio::test]
async fn a_refused_or_garbled_read_and_a_bad_history_id() {
    let endpoint = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ngsi-ld/v1/entities"))
        .respond_with(
            ResponseTemplate::new(403).set_body_json(json!({ "detail": "outside the grant" })),
        )
        .mount(&endpoint)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/ngsi-ld/v1/temporal/entities/{STATION}")))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>not json</html>"))
        .mount(&endpoint)
        .await;
    let (status, body) = call(
        app_at(&endpoint),
        signed_in("GET", &format!("{BASE}api/stations"), None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body.contains("outside the grant"), "{body}");
    let encoded = STATION.replace(':', "%3A");
    let (status, _) = call(
        app_at(&endpoint),
        signed_in(
            "GET",
            &format!("{BASE}api/stations/{encoded}/history"),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    let before = endpoint.received_requests().await.unwrap_or_default().len();
    let (status, body) = call(
        app_at(&endpoint),
        signed_in(
            "GET",
            &format!("{BASE}api/stations/not-a-urn/history"),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("NGSI-LD URN"), "{body}");
    assert_eq!(
        endpoint.received_requests().await.unwrap_or_default().len(),
        before
    );
}

/// The roles are none when `/me` refuses; a name over 200 characters and a removal under a bad id
/// are refused by the App.
#[tokio::test]
async fn a_refused_me_a_long_name_and_a_bad_removal_id() {
    let endpoint = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/me"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&endpoint)
        .await;
    let (status, body) = call(
        app_at(&endpoint),
        signed_in("GET", &format!("{BASE}api/me"), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(r#""roles":[]"#), "{body}");

    let encoded = STATION.replace(':', "%3A");
    let long = "x".repeat(201);
    let (status, body) = call(
        app_at(&endpoint),
        signed_in(
            "PATCH",
            &format!("{BASE}api/stations/{encoded}"),
            Some(json!({ "name": { "fi": long } })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("at most 200 characters"), "{body}");
    let (status, body) = call(
        app_at(&endpoint),
        signed_in("DELETE", &format!("{BASE}api/stations/not-a-urn"), None),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("NGSI-LD URN"), "{body}");
    assert!(untouched_writes(&endpoint).await);
}

async fn untouched_writes(endpoint: &MockServer) -> bool {
    endpoint
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .all(|request| request.method.as_str() == "GET")
}

/// The process's own environment goes through the same parsing: without an endpoint, no start.
#[test]
fn the_pod_does_not_start_without_its_endpoint() {
    assert_eq!(
        Config::from_env().is_ok(),
        std::env::var("JC_ENDPOINT_URL").is_ok()
    );
}

/// An id prefix the listing hands back that is not a station's is never written under.
#[tokio::test]
async fn a_new_station_is_not_written_under_another_types_prefix() {
    let endpoint = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ngsi-ld/v1/entities"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!([{ "id": "urn:ngsi-ld:Other:hel.fi:x:one", "type": "Other" }]),
            ),
        )
        .mount(&endpoint)
        .await;
    let (status, body) = call(
        app_at(&endpoint),
        signed_in(
            "POST",
            &format!("{BASE}api/stations"),
            Some(json!({ "localId": "s9", "name": { "fi": "A" }, "coordinates": [24.9, 60.2] })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert!(untouched_writes(&endpoint).await);
}
