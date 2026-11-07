//! `KubeClient::from_mount` and the bearer it sends (T-2521; CC-06, CC-18, AG-46): the Portal's
//! in-cluster credential, a projected ServiceAccount token the kubelet rotates, read from disk
//! for every request and never written anywhere else.

use std::path::{Path, PathBuf};

use joinedcontext_portal::apps::kube::{KubeClient, KubeError};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "eyJhbGciOiJSUzI1NiJ9.rotating-token-one";

/// A fresh mount directory of this test alone, holding `files`.
fn mount(tag: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("t2521-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a mount");
    for (name, content) in files {
        std::fs::write(dir.join(name), content).expect("a file");
    }
    dir
}

/// An API server that answers a read of one Service with `{}` when the bearer is `bearer`.
async fn api_expecting(bearer: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/namespaces/apps/services/one"))
        .and(header("authorization", bearer))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&server)
        .await;
    server
}

async fn read_one(client: &KubeClient) -> Result<Option<serde_json::Value>, KubeError> {
    client.get("v1", "Service", "apps", "one").await
}

/// The bearer of every request the server received, in order.
async fn bearers(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter_map(|r| r.headers.get("authorization"))
        .filter_map(|v| v.to_str().ok().map(str::to_owned))
        .collect()
}

fn built(dir: &Path, api: &str) -> KubeClient {
    KubeClient::from_mount(dir, api)
        .expect("built")
        .expect("a token file")
}

/// T-2521, CC-06: a token the Portal cannot read is an error naming the file, never its content.
/// The file is replaced by a directory, which no user, root included, reads as text.
#[tokio::test]
async fn an_unreadable_token_file_is_an_error_naming_the_path_never_the_content() {
    let dir = mount("unreadable", &[("token", TOKEN)]);
    let server = api_expecting(&format!("Bearer {TOKEN}")).await;
    let client = built(&dir, &server.uri());
    std::fs::remove_file(dir.join("token")).expect("removed");
    std::fs::create_dir(dir.join("token")).expect("a directory in its place");

    let failed = read_one(&client).await.expect_err("unreadable");
    assert!(
        matches!(failed, KubeError::ServiceAccount { .. }),
        "{failed:?}"
    );
    let said = failed.to_string();
    assert!(
        said.contains(&dir.join("token").display().to_string()),
        "{said}"
    );
    assert!(!said.contains("rotating-token"), "{said}");
    assert!(server
        .received_requests()
        .await
        .unwrap_or_default()
        .is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// T-2521, CC-06: the kubelet rotates the token; each request reads the file as it is then.
#[tokio::test]
async fn a_token_file_that_is_rotated_between_two_calls_is_reread_each_time() {
    let dir = mount("rotated", &[("token", "first-token")]);
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&server)
        .await;
    let client = built(&dir, &server.uri());
    read_one(&client).await.expect("first");
    std::fs::write(dir.join("token"), "second-token").expect("rotated");
    read_one(&client).await.expect("second");
    assert_eq!(
        bearers(&server).await,
        ["Bearer first-token", "Bearer second-token"]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// T-2521: a token that cannot be a header value is a client error, and the request is not
/// sent; its text is not in the error.
#[tokio::test]
async fn a_token_containing_a_character_invalid_in_a_header_is_an_error_not_a_panic() {
    for (tag, token) in [
        ("newline", "abc\ndef"),
        ("nul", "abc\0def"),
        ("del", "abc\u{7f}def"),
    ] {
        let dir = mount(tag, &[("token", token)]);
        let server = MockServer::start().await;
        let client = built(&dir, &server.uri());
        let failed = read_one(&client).await.expect_err(tag);
        assert!(matches!(failed, KubeError::Client(_)), "{tag}: {failed:?}");
        assert!(!failed.to_string().contains("abc"), "{tag}: {failed}");
        assert!(
            server
                .received_requests()
                .await
                .unwrap_or_default()
                .is_empty(),
            "{tag}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// T-2521: outside a cluster there is no token and no client, whatever the API address says.
#[test]
fn no_token_file_returns_none_not_an_error() {
    let empty = mount("none", &[("ca.crt", "not read")]);
    assert!(KubeClient::from_mount(&empty, "https://10.0.0.1")
        .expect("no error")
        .is_none());
    assert!(KubeClient::from_mount(&empty, "not a url")
        .expect("no error")
        .is_none());
    // A directory named `token` is not a token file.
    std::fs::create_dir(empty.join("token")).expect("a directory");
    assert!(KubeClient::from_mount(&empty, "https://10.0.0.1")
        .expect("no error")
        .is_none());
    let missing = std::env::temp_dir().join("t2521-no-such-mount-at-all");
    assert!(KubeClient::from_mount(&missing, "https://10.0.0.1")
        .expect("no error")
        .is_none());
    let _ = std::fs::remove_dir_all(&empty);
}

/// T-2521: without `ca.crt` the client trusts the system's roots; a `ca.crt` that is not a
/// certificate is refused when the client is built.
#[test]
fn a_missing_ca_file_still_builds_a_client_the_way_the_code_says() {
    let without = mount("noca", &[("token", TOKEN)]);
    assert!(KubeClient::from_mount(&without, "https://10.0.0.1")
        .expect("built")
        .is_some());
    let broken = mount(
        "badca",
        &[
            ("token", TOKEN),
            ("ca.crt", "-----BEGIN CERTIFICATE-----\nnope\n"),
        ],
    );
    let refused = KubeClient::from_mount(&broken, "https://10.0.0.1").expect_err("a broken CA");
    assert!(matches!(refused, KubeError::Client(_)), "{refused:?}");
    assert!(!refused.to_string().contains("rotating-token"), "{refused}");
    let _ = std::fs::remove_dir_all(&without);
    let _ = std::fs::remove_dir_all(&broken);
}

/// T-2521: an API address that is not a URL is a client error. The token file is only looked
/// at (`is_file`), and the CA is read before the address is parsed, so "before any file is
/// touched" is struck: a readable mount and a bad address give the address's error.
#[test]
fn an_invalid_api_url_is_an_error_before_any_file_is_touched() {
    let dir = mount("badurl", &[("token", TOKEN)]);
    for api in ["", "not a url", "://nohost", "http//missing-colon"] {
        let refused = KubeClient::from_mount(&dir, api).expect_err(api);
        assert!(
            matches!(refused, KubeError::Client(_)),
            "{api}: {refused:?}"
        );
        assert!(
            !refused.to_string().contains("rotating-token"),
            "{api}: {refused}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// T-2521: the kubelet's file may end in a newline; the bearer does not.
#[tokio::test]
async fn a_token_with_a_trailing_newline_is_trimmed() {
    for (tag, content) in [
        ("lf", format!("{TOKEN}\n")),
        ("crlf", format!("{TOKEN}\r\n")),
        ("spaces", format!("  {TOKEN}  \n")),
    ] {
        let dir = mount(tag, &[("token", content.as_str())]);
        let server = api_expecting(&format!("Bearer {TOKEN}")).await;
        let client = built(&dir, &server.uri());
        assert!(read_one(&client).await.expect(tag).is_some(), "{tag}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// T-2521, CC-06: a token that disappears after the client was built (a pod whose mount went
/// away) fails the request with the file's name and sends nothing.
#[tokio::test]
async fn a_token_file_that_disappears_after_the_client_was_built_fails_the_request_with_a_named_error(
) {
    let dir = mount("gone", &[("token", TOKEN)]);
    let server = api_expecting(&format!("Bearer {TOKEN}")).await;
    let client = built(&dir, &server.uri());
    assert!(read_one(&client)
        .await
        .expect("while it is there")
        .is_some());
    std::fs::remove_file(dir.join("token")).expect("removed");
    let failed = read_one(&client).await.expect_err("gone");
    match &failed {
        KubeError::ServiceAccount { path, .. } => assert_eq!(path, &dir.join("token")),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        server.received_requests().await.unwrap_or_default().len(),
        1
    );
    let _ = std::fs::remove_dir_all(&dir);
}
