//! What a `Subscription` manifest does to the space it names (T-0931, CC-72, DS-16).
//!
//! The reconciler is the only wave that writes live broker state, so the tests are about what
//! leaves the Portal: the create, the update, the delete of a manifest that is gone, the refusal
//! of a reference that does not resolve — and that the credential is in the request and nowhere
//! else.

use joinedcontext_portal::config::ClientAuth;
use std::sync::Arc;

use joinedcontext_portal::reconciler::subscriptions::{
    Deliveries, SubscriptionOutcome, SubscriptionSync,
};
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const SPACE: &str = "air-quality";
/// What the gateway and every id call the space: `{project}-{name}` when no segment is pinned
/// (PF-84, CC-82).
const SEGMENT: &str = "helsinki-air-quality";
const PROJECT: &str = "helsinki";
const DOMAIN: &str = "hel.fi";
const ID: &str = "urn:ngsi-ld:Subscription:hel.fi:helsinki-air-quality:alerts";

/// The realm, answering the reconciler's client-credentials request.
async fn realm(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/realms/dev/protocol/openid-connect/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "the-reconcilers-token",
            "token_type": "Bearer",
            "expires_in": 300
        })))
        .mount(server)
        .await;
}

fn sync(server: &MockServer) -> SubscriptionSync {
    SubscriptionSync::new(
        server.uri(),
        &format!("{}/realms/dev", server.uri()),
        "portal-reconciler".to_owned(),
        ClientAuth::Secret("s3cr3t".into()),
        DOMAIN,
    )
}

fn mirror_with(manifests: &[(&str, Value)]) -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(envelope(
        "ContextSpace",
        SPACE,
        json!({ "isSandbox": false }),
    ));
    for (name, spec) in manifests {
        mirror.upsert(envelope("Subscription", name, spec.clone()));
    }
    mirror
}

fn envelope(
    kind: &str,
    name: &str,
    spec: Value,
) -> joinedcontext_portal::resource::ResourceEnvelope {
    serde_json::from_value(json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": kind,
        "metadata": { "name": name, "namespace": PROJECT },
        "spec": spec,
    }))
    .expect("an envelope")
}

/// The manifest of `Architecture/06`, without the credential.
fn declared() -> Value {
    json!({
        "contextSpaceRef": { "kind": "ContextSpace", "name": SPACE },
        "entities": [{ "type": "AirQualityObserved" }],
        "watchedAttributes": ["airQualityIndex"],
        "q": "airQualityIndex>50",
        "notification": {
            "endpoint": {
                "uri": "https://alerts.example.fi/hooks/air-quality",
                "accept": "application/json",
                "receiverInfo": [{ "key": "X-Space", "value": SPACE }]
            },
            "attributes": ["airQualityIndex", "location"],
            "format": "normalized"
        },
        "throttling": 60,
        "isActive": true
    })
}

/// Every request the space surface saw, as `(method, path, body)`.
async fn seen(server: &MockServer) -> Vec<(String, String, Value)> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r: &&Request| r.url.path().contains("/ngsi-ld/v1/subscriptions"))
        .map(|r| {
            (
                r.method.to_string(),
                r.url.path().to_owned(),
                serde_json::from_slice(&r.body).unwrap_or(Value::Null),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_declared_subscription_is_created_in_the_space_it_names() {
    let server = MockServer::start().await;
    realm(&server).await;
    Mock::given(method("GET"))
        .and(path(format!("/cs/{SEGMENT}/ngsi-ld/v1/subscriptions")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/cs/{SEGMENT}/ngsi-ld/v1/subscriptions")))
        .respond_with(ResponseTemplate::new(201))
        .mount(&server)
        .await;

    let mirror = mirror_with(&[("alerts", declared())]);
    let previous = mirror_with(&[]);
    let outcomes = sync(&server)
        .converge(
            &mirror,
            &previous,
            std::path::Path::new("/nonexistent"),
            None,
        )
        .await;

    assert_eq!(
        outcomes,
        vec![(
            PROJECT.to_owned(),
            "alerts".to_owned(),
            SubscriptionOutcome::Written(None)
        )]
    );
    let posted = seen(&server)
        .await
        .into_iter()
        .find(|(verb, _, _)| verb == "POST")
        .expect("the space was asked to create it");
    assert_eq!(posted.2["id"], json!(ID), "the id is the manifest's");
    assert_eq!(posted.2["type"], json!("Subscription"));
    assert_eq!(posted.2["q"], json!("airQualityIndex>50"));
    assert_eq!(posted.2["throttling"], json!(60));
    assert_eq!(
        posted.2["notification"]["endpoint"]["uri"],
        json!("https://alerts.example.fi/hooks/air-quality")
    );
    assert_eq!(
        posted.2["notification"]["endpoint"]["receiverInfo"],
        json!([{ "key": "X-Space", "value": SPACE }]),
        "no credential is invented where the manifest declares none"
    );
}

#[tokio::test]
async fn a_subscription_the_space_already_holds_is_updated_not_duplicated() {
    let server = MockServer::start().await;
    realm(&server).await;
    Mock::given(method("GET"))
        .and(path(format!("/cs/{SEGMENT}/ngsi-ld/v1/subscriptions")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "id": ID }])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/cs/{SEGMENT}/ngsi-ld/v1/subscriptions")))
        .respond_with(ResponseTemplate::new(409).set_body_string("already exists"))
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(format!("/cs/{SEGMENT}/ngsi-ld/v1/subscriptions/{ID}")))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    // T-4866: what the space says of the deliveries is read back after the write.
    Mock::given(method("GET"))
        .and(path(format!("/cs/{SEGMENT}/ngsi-ld/v1/subscriptions/{ID}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": ID,
            "type": "Subscription",
            "notification": {
                "endpoint": { "uri": "https://alerts.example.fi/hooks/air-quality" },
                "status": "failed",
                "timesSent": 5,
                "timesFailed": 2,
                "lastNotification": "2026-10-21T03:20:11Z",
                "lastFailure": "2026-10-21T03:20:11Z",
                "lastSuccess": "2026-10-21T03:10:00Z"
            }
        })))
        .mount(&server)
        .await;

    let mirror = mirror_with(&[("alerts", declared())]);
    let previous = mirror_with(&[]);
    let outcomes = sync(&server)
        .converge(
            &mirror,
            &previous,
            std::path::Path::new("/nonexistent"),
            None,
        )
        .await;

    assert_eq!(
        outcomes[0].2,
        SubscriptionOutcome::Written(Some(Deliveries {
            status: Some("failed".into()),
            times_sent: Some(5),
            times_failed: Some(2),
            last_notification: Some("2026-10-21T03:20:11Z".into()),
            last_success: Some("2026-10-21T03:10:00Z".into()),
            last_failure: Some("2026-10-21T03:20:11Z".into()),
        }))
    );
    let patched = seen(&server)
        .await
        .into_iter()
        .find(|(verb, _, _)| verb == "PATCH")
        .expect("the one it already holds is patched");
    assert!(
        patched.2.get("id").is_none(),
        "a patch does not rename the subscription: {}",
        patched.2
    );
    assert_eq!(patched.2["q"], json!("airQualityIndex>50"));
    // Nothing was deleted: the id is the one the manifest declares.
    assert!(
        !seen(&server)
            .await
            .iter()
            .any(|(verb, _, _)| verb == "DELETE"),
        "a declared subscription must survive its own run"
    );
}

#[tokio::test]
async fn the_subscription_of_a_manifest_that_is_gone_is_removed() {
    let server = MockServer::start().await;
    realm(&server).await;
    // The space also holds one an application created through the API, in the very same URN
    // namespace: it was never declared here, so it is not this reconciler's to remove.
    Mock::given(method("GET"))
        .and(path(format!("/cs/{SEGMENT}/ngsi-ld/v1/subscriptions")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": ID },
            { "id": "urn:ngsi-ld:Subscription:hel.fi:helsinki-air-quality:written-by-an-application" },
        ])))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path_regex(r"^/cs/.+/ngsi-ld/v1/subscriptions/.+$"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    // The run before declared `alerts`; this one declares nothing any more.
    let previous = mirror_with(&[("alerts", declared())]);
    let mirror = mirror_with(&[]);
    let outcomes = sync(&server)
        .converge(
            &mirror,
            &previous,
            std::path::Path::new("/nonexistent"),
            None,
        )
        .await;
    assert!(
        outcomes.is_empty(),
        "nothing was declared, so nothing was written"
    );

    let deleted: Vec<String> = seen(&server)
        .await
        .into_iter()
        .filter(|(verb, _, _)| verb == "DELETE")
        .map(|(_, path, _)| path)
        .collect();
    assert_eq!(
        deleted,
        vec![format!("/cs/{SEGMENT}/ngsi-ld/v1/subscriptions/{ID}")],
        "only what this reconciler wrote is removed; an application's subscription and another \
         organization's are left alone"
    );
}

#[tokio::test]
async fn a_credential_that_cannot_be_resolved_stops_the_subscription_before_any_request() {
    let server = MockServer::start().await;
    realm(&server).await;
    Mock::given(method("GET"))
        .and(path(format!("/cs/{SEGMENT}/ngsi-ld/v1/subscriptions")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;

    let mut spec = declared();
    spec["notification"]["endpoint"]["secretRef"] =
        json!({ "name": "alerts-webhook", "key": "token" });
    let mirror = mirror_with(&[("alerts", spec)]);
    let previous = mirror_with(&[]);

    // No backend is configured, which is the honest shape of a Portal that holds no secrets.
    let outcomes = sync(&server)
        .converge(
            &mirror,
            &previous,
            std::path::Path::new("/nonexistent"),
            None,
        )
        .await;

    let SubscriptionOutcome::Error(reason) = &outcomes[0].2 else {
        panic!("a subscription with an unresolvable credential must not be written: {outcomes:?}");
    };
    assert!(reason.contains("alerts-webhook"), "{reason}");
    assert!(
        !seen(&server)
            .await
            .iter()
            .any(|(verb, _, _)| verb == "POST"),
        "nothing may be posted for a subscription that has no credential"
    );
}

#[tokio::test]
async fn a_realm_that_refuses_the_client_writes_nothing_and_says_so_per_manifest() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/realms/dev/protocol/openid-connect/token"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let mirror = mirror_with(&[("alerts", declared())]);
    let previous = mirror_with(&[]);
    let outcomes = sync(&server)
        .converge(
            &mirror,
            &previous,
            std::path::Path::new("/nonexistent"),
            None,
        )
        .await;

    let SubscriptionOutcome::Error(reason) = &outcomes[0].2 else {
        panic!("without a token nothing is written: {outcomes:?}");
    };
    assert!(
        reason.contains("refused the reconciler's client"),
        "{reason}"
    );
    assert!(
        seen(&server).await.is_empty(),
        "no space was touched at all"
    );
}

#[tokio::test]
async fn a_space_that_refuses_the_write_leaves_the_reason_on_the_manifest() {
    let server = MockServer::start().await;
    realm(&server).await;
    Mock::given(method("POST"))
        .and(path(format!("/cs/{SEGMENT}/ngsi-ld/v1/subscriptions")))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "type": "https://uri.etsi.org/ngsi-ld/errors/OperationNotSupported",
            "title": "the service account may not write this space"
        })))
        .mount(&server)
        .await;

    let mirror = mirror_with(&[("alerts", declared())]);
    let previous = mirror_with(&[]);
    let outcomes = sync(&server)
        .converge(
            &mirror,
            &previous,
            std::path::Path::new("/nonexistent"),
            None,
        )
        .await;

    let SubscriptionOutcome::Error(reason) = &outcomes[0].2 else {
        panic!("a refused write is not a written subscription: {outcomes:?}");
    };
    assert!(reason.contains("403"), "{reason}");
    assert!(
        reason.contains("may not write this space"),
        "the space's own words are what the author needs: {reason}"
    );
    assert!(
        !reason.contains('{') && !reason.contains("uri.etsi.org"),
        "the sentence, not the problem document's JSON (T-4866): {reason}"
    );
}

#[tokio::test]
async fn each_subscription_is_written_to_the_space_its_manifest_names() {
    let server = MockServer::start().await;
    realm(&server).await;
    for space in [SEGMENT, "helsinki-traffic"] {
        Mock::given(method("POST"))
            .and(path(format!("/cs/{space}/ngsi-ld/v1/subscriptions")))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;
    }

    let mut elsewhere = declared();
    elsewhere["contextSpaceRef"] = json!({ "kind": "ContextSpace", "name": "traffic" });
    // A geoQ rides along: the manifest writes it as a query string and the broker takes an
    // object, so this is where the two meet.
    elsewhere["geoQ"] =
        json!("georel=near;maxDistance==2000&geometry=Point&coordinates=[24.9,60.2]");
    let mirror = mirror_with(&[("alerts", declared()), ("jams", elsewhere)]);
    let previous = mirror_with(&[]);
    let outcomes = sync(&server)
        .converge(
            &mirror,
            &previous,
            std::path::Path::new("/nonexistent"),
            None,
        )
        .await;

    assert!(
        outcomes
            .iter()
            .all(|(_, _, outcome)| matches!(outcome, SubscriptionOutcome::Written(_))),
        "{outcomes:?}"
    );
    let posted = seen(&server).await;
    let air = posted
        .iter()
        .find(|(verb, path, _)| verb == "POST" && path.contains("air-quality"))
        .expect("the air-quality subscription");
    let traffic = posted
        .iter()
        .find(|(verb, path, _)| verb == "POST" && path.contains("traffic"))
        .expect("the traffic subscription");
    assert_eq!(air.2["id"], json!(ID));
    assert_eq!(
        traffic.2["id"],
        json!("urn:ngsi-ld:Subscription:hel.fi:helsinki-traffic:jams")
    );
    assert_eq!(traffic.2["geoQ"]["geometry"], json!("Point"));
    assert_eq!(traffic.2["geoQ"]["coordinates"], json!([24.9, 60.2]));
    assert!(
        air.2.get("geoQ").is_none(),
        "a manifest without a geoQ sends none"
    );
}

/// An OpenBao holding the webhook token `alerts-webhook/token` (T-5565).
async fn openbao(server: &MockServer) -> joinedcontext_portal::pipeline_secrets::Resolver {
    use joinedcontext_portal::pipeline_secrets::{Backend, Resolver};
    Mock::given(method("POST"))
        .and(path("/v1/auth/kubernetes/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "auth": { "client_token": "a-session", "lease_duration": 3600, "renewable": true }
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/secret/data/alerts-webhook"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "data": { "token": "Bearer the-webhook-token" }, "metadata": { "version": 1 } }
        })))
        .mount(server)
        .await;
    let jwt =
        std::env::temp_dir().join(format!("jc-portal-t5565-token-{}", server.address().port()));
    std::fs::write(&jwt, "a.service.account.token").expect("write the token");
    Resolver::new(Backend::OpenBao {
        address: server.uri(),
        role: "portal".to_owned(),
        jwt_path: jwt,
    })
}

#[tokio::test]
async fn the_credential_is_sent_as_a_key_value_pair() {
    let server = MockServer::start().await;
    realm(&server).await;
    Mock::given(method("GET"))
        .and(path(format!("/cs/{SEGMENT}/ngsi-ld/v1/subscriptions")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/cs/{SEGMENT}/ngsi-ld/v1/subscriptions")))
        .respond_with(ResponseTemplate::new(201))
        .mount(&server)
        .await;
    let bao = MockServer::start().await;
    let resolver = openbao(&bao).await;

    let mut spec = declared();
    spec["notification"]["endpoint"]["secretRef"] =
        json!({ "name": "alerts-webhook", "key": "token" });
    let mirror = mirror_with(&[("alerts", spec)]);
    let outcomes = sync(&server)
        .converge(
            &mirror,
            &mirror_with(&[]),
            std::path::Path::new("/nonexistent"),
            Some(&resolver),
        )
        .await;

    assert!(
        !matches!(outcomes[0].2, SubscriptionOutcome::Error(_)),
        "{outcomes:?}"
    );
    let posted = seen(&server)
        .await
        .into_iter()
        .find(|(verb, _, _)| verb == "POST")
        .expect("the space was asked to create it");
    // Antares refuses any receiverInfo entry that is not a {key, value} pair (CIM 009 5.2.22).
    assert_eq!(
        posted.2["notification"]["endpoint"]["receiverInfo"],
        json!([
            { "key": "X-Space", "value": SPACE },
            { "key": "Authorization", "value": "Bearer the-webhook-token" }
        ])
    );
}
