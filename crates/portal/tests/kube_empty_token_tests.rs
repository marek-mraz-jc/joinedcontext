//! An empty ServiceAccount token is refused before a request (T-2566, CC-06, CC-18): a bare
//! `Bearer` is what the API server reads as `system:anonymous`.

use joinedcontext_portal::apps::kube::KubeClient;
use std::path::PathBuf;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn mount(tag: &str, token: &str) -> PathBuf {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after 1970")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("kube-empty-token-{tag}-{now}"));
    std::fs::create_dir_all(&dir).expect("a mount");
    std::fs::write(dir.join("token"), token).expect("the token file");
    dir
}

async fn api() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&server)
        .await;
    server
}

/// CC-06: an empty or blank token file is refused naming the file, and nothing is sent.
#[tokio::test]
async fn an_empty_token_file_is_refused_not_sent_as_an_empty_bearer() {
    for (tag, content) in [("empty", ""), ("blank", " \n"), ("tabs", "\t\r\n")] {
        let dir = mount(tag, content);
        let server = api().await;
        let client = KubeClient::from_mount(&dir, &server.uri())
            .expect("the client builds")
            .expect("a mount with a token file");
        let refused = client
            .get("v1", "ServiceAccount", "apps", "board")
            .await
            .expect_err(tag);
        let said = refused.to_string();
        assert!(
            said.contains(&dir.join("token").display().to_string()),
            "{tag}: {said}"
        );
        assert!(said.contains("empty"), "{tag}: {said}");
        let sent = server.received_requests().await.unwrap_or_default();
        assert!(sent.is_empty(), "{tag}: a request went out");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// CC-06: the token the kubelet writes later is read on the next call, so a mount filled after
/// startup works without a restart; and a fixed empty token is the same refusal.
#[tokio::test]
async fn a_token_written_after_startup_is_used_and_a_fixed_empty_token_is_refused() {
    let dir = mount("late", "");
    let server = api().await;
    let client = KubeClient::from_mount(&dir, &server.uri())
        .expect("the client builds")
        .expect("a mount");
    assert!(client
        .get("v1", "ServiceAccount", "apps", "board")
        .await
        .is_err());
    std::fs::write(dir.join("token"), "sa-token\n").expect("the kubelet writes it");
    client
        .get("v1", "ServiceAccount", "apps", "board")
        .await
        .expect("the filled token is used");
    let sent = server.received_requests().await.unwrap_or_default();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0]
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok()),
        Some("Bearer sa-token")
    );
    let _ = std::fs::remove_dir_all(&dir);

    let fixed = KubeClient::with_token(&server.uri(), "").expect("the client builds");
    assert!(fixed
        .get("v1", "ServiceAccount", "apps", "board")
        .await
        .is_err());
    assert_eq!(
        server.received_requests().await.unwrap_or_default().len(),
        1
    );
}
