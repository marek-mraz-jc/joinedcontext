//! The reconciler mints one scoped credential pair per organization (T-0422, PF-31, PF-32).
//!
//! Against a mocked store admin API: what is actually sent on the wire, that the request is
//! signed with the root credential and not with a derived one, that the body the signature
//! covers is the body that arrives, and that a refusal carries no secret into the error.

use joinedcontext_portal::artifact_store::{Client, Credential, Role, Settings};
use serde_json::Value;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const ROOT_SECRET: &str = "the-root-secret-nobody-else-holds";

fn settings(endpoint: String) -> Settings {
    Settings {
        endpoint,
        bucket: "jc-artifacts".to_owned(),
        region: "us-east-1".to_owned(),
        root_access_key: "jc-root".to_owned(),
        root_secret_key: ROOT_SECRET.to_owned(),
    }
}

async fn accept_everything(server: &MockServer) {
    for route in [
        "/rustfs/admin/v3/add-canned-policy",
        "/rustfs/admin/v3/add-user",
        "/rustfs/admin/v3/set-user-or-group-policy",
    ] {
        Mock::given(method("PUT"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200))
            .mount(server)
            .await;
    }
}

fn requests_to<'a>(received: &'a [Request], route: &str) -> Vec<&'a Request> {
    received
        .iter()
        .filter(|request| request.url.path() == route)
        .collect()
}

#[tokio::test]
async fn an_organization_gets_one_writer_and_one_reader_and_no_third_credential() {
    let server = MockServer::start().await;
    accept_everything(&server).await;

    let client = Client::new(settings(server.uri())).expect("a client");
    let issued = client
        .ensure_organization("hel")
        .await
        .expect("the store accepts the pair");

    assert_eq!(
        issued
            .iter()
            .map(|c| c.access_key.as_str())
            .collect::<Vec<_>>(),
        vec!["jc-hel-writer", "jc-hel-reader"]
    );

    let received = server.received_requests().await.expect("recorded requests");
    let users = requests_to(&received, "/rustfs/admin/v3/add-user");
    let policies = requests_to(&received, "/rustfs/admin/v3/add-canned-policy");
    let attachments = requests_to(&received, "/rustfs/admin/v3/set-user-or-group-policy");
    assert_eq!((users.len(), policies.len(), attachments.len()), (2, 2, 2));
}

#[tokio::test]
async fn the_user_the_store_is_told_about_carries_the_derived_secret_and_is_enabled() {
    let server = MockServer::start().await;
    accept_everything(&server).await;
    let client = Client::new(settings(server.uri())).expect("a client");
    client.ensure_organization("hel").await.expect("accepted");

    let received = server.received_requests().await.expect("recorded requests");
    let reader = Credential::derive(ROOT_SECRET, "hel", Role::Reader);
    let bodies: Vec<Value> = requests_to(&received, "/rustfs/admin/v3/add-user")
        .iter()
        .map(|request| request.body_json().expect("a JSON body"))
        .collect();

    // Plain JSON: the store decrypts a body only on its MinIO-compatible prefix, and this
    // client speaks the native one.
    let reader_body = bodies
        .iter()
        .find(|body| body["secretKey"] == reader.secret_key)
        .expect("the reader's derived secret is what the store is given");
    assert_eq!(reader_body["status"], "enabled");

    let queries: Vec<String> = requests_to(&received, "/rustfs/admin/v3/add-user")
        .iter()
        .map(|request| request.url.query().unwrap_or_default().to_owned())
        .collect();
    assert!(
        queries.contains(&"accessKey=jc-hel-reader".to_owned()),
        "{queries:?}"
    );
}

#[tokio::test]
async fn the_policy_attached_to_the_reader_is_the_readers_own() {
    let server = MockServer::start().await;
    accept_everything(&server).await;
    let client = Client::new(settings(server.uri())).expect("a client");
    client.ensure_organization("hel").await.expect("accepted");

    let received = server.received_requests().await.expect("recorded requests");
    let pairs: Vec<(String, String)> =
        requests_to(&received, "/rustfs/admin/v3/set-user-or-group-policy")
            .iter()
            .map(|request| {
                let query: std::collections::HashMap<_, _> = request.url.query_pairs().collect();
                (
                    query["policyName"].to_string(),
                    query["userOrGroup"].to_string(),
                )
            })
            .collect();
    assert!(pairs.contains(&("jc-hel-reader".to_owned(), "jc-hel-reader".to_owned())));
    assert!(pairs.contains(&("jc-hel-writer".to_owned(), "jc-hel-writer".to_owned())));
    // A group would apply to everyone in it; these are users.
    for request in requests_to(&received, "/rustfs/admin/v3/set-user-or-group-policy") {
        assert!(request
            .url
            .query()
            .unwrap_or_default()
            .contains("isGroup=false"));
    }
}

#[tokio::test]
async fn every_request_is_signed_with_the_root_credential_over_the_body_that_arrives() {
    use sha2::{Digest, Sha256};

    let server = MockServer::start().await;
    accept_everything(&server).await;
    let client = Client::new(settings(server.uri())).expect("a client");
    client.ensure_organization("hel").await.expect("accepted");

    for request in server.received_requests().await.expect("recorded requests") {
        let authorization = request
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        assert!(
            authorization.starts_with("AWS4-HMAC-SHA256 Credential=jc-root/"),
            "signed with something other than the root credential: {authorization}"
        );
        assert!(authorization.contains("/us-east-1/s3/aws4_request"));
        assert!(authorization.contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date"));
        // No secret of any kind travels in a header that is not the signature itself.
        assert!(!authorization.contains(ROOT_SECRET));

        // The hash the signature covers is the hash of the bytes that arrived: a body signed
        // and then changed is a 403 with nothing in it to say why.
        let sent = request
            .headers
            .get("x-amz-content-sha256")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        let actual: String = Sha256::digest(&request.body)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(sent, actual, "the payload hash is not the payload's");
    }
}

#[tokio::test]
async fn a_store_that_refuses_says_which_call_failed_and_carries_no_secret() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/rustfs/admin/v3/add-canned-policy"))
        .and(query_param("name", "jc-hel-writer"))
        // An admin API can echo the request it refused, which is why the body is never in the
        // error: the request carries a derived secret key.
        .respond_with(ResponseTemplate::new(403).set_body_string(format!(
            "refused: secretKey={}",
            Credential::derive(ROOT_SECRET, "hel", Role::Writer).secret_key
        )))
        .mount(&server)
        .await;

    let client = Client::new(settings(server.uri())).expect("a client");
    let error = client
        .ensure_organization("hel")
        .await
        .expect_err("a 403 is a refusal");
    let said = error.to_string();
    assert!(said.contains("add-canned-policy"), "{said}");
    assert!(said.contains("403"), "{said}");
    assert!(
        !said.contains(&Credential::derive(ROOT_SECRET, "hel", Role::Writer).secret_key),
        "the refusal carried the secret it was refusing: {said}"
    );

    // The user was never created: a credential whose policy does not exist would be a key with
    // whatever the store's default rights are.
    let received = server.received_requests().await.expect("recorded requests");
    assert!(requests_to(&received, "/rustfs/admin/v3/add-user").is_empty());
}

#[tokio::test]
async fn a_store_that_is_not_there_is_a_transport_error_and_not_a_panic() {
    // Nothing listens on this port: the client has to fail, in one place, with the operation
    // named, rather than take the reconciler's run down with it.
    let client = Client::new(settings("http://127.0.0.1:1".to_owned())).expect("a client");
    let error = client
        .ensure_organization("hel")
        .await
        .expect_err("nothing answers");
    assert!(error.to_string().contains("add-canned-policy"), "{error}");
}

/// T-0925, PF-32: the reader travels to the workload that serves, the writer never does.
#[test]
fn the_secret_carries_the_reader_and_nothing_that_can_write() {
    let reader = Credential::derive(ROOT_SECRET, "hel", Role::Reader);
    let writer = Credential::derive(ROOT_SECRET, "hel", Role::Writer);
    let secret = joinedcontext_portal::artifact_store::reader_secret("dev", "hel", &reader);

    assert_eq!(secret["kind"], "Secret");
    assert_eq!(secret["metadata"]["name"], "artifact-store-reader-hel");
    assert_eq!(secret["metadata"]["namespace"], "dev");
    assert_eq!(secret["stringData"]["ACCESS_KEY_ID"], "jc-hel-reader");
    assert_eq!(secret["stringData"]["ACCESS_SECRET_KEY"], reader.secret_key);

    let serialised = secret.to_string();
    assert!(
        !serialised.contains(&writer.secret_key),
        "the writer's key is in the Secret the serving pod reads"
    );
    assert!(
        !serialised.contains(ROOT_SECRET),
        "the root secret is in the Secret the serving pod reads"
    );
}

/// The credential is derived, so a rotation of the root secret rewrites every organization's
/// Secret on the next sync and two organizations never share one.
#[test]
fn a_rotated_root_rewrites_the_secret_and_organizations_do_not_share_one() {
    let before = joinedcontext_portal::artifact_store::reader_secret(
        "dev",
        "hel",
        &Credential::derive(ROOT_SECRET, "hel", Role::Reader),
    );
    let after = joinedcontext_portal::artifact_store::reader_secret(
        "dev",
        "hel",
        &Credential::derive("a-rotated-root-secret", "hel", Role::Reader),
    );
    let other = joinedcontext_portal::artifact_store::reader_secret(
        "dev",
        "bb",
        &Credential::derive(ROOT_SECRET, "bb", Role::Reader),
    );

    assert_ne!(
        before["stringData"]["ACCESS_SECRET_KEY"],
        after["stringData"]["ACCESS_SECRET_KEY"]
    );
    assert_eq!(before["metadata"]["name"], after["metadata"]["name"]);
    assert_ne!(
        before["stringData"]["ACCESS_SECRET_KEY"],
        other["stringData"]["ACCESS_SECRET_KEY"]
    );
    assert_eq!(other["metadata"]["name"], "artifact-store-reader-bb");
}
