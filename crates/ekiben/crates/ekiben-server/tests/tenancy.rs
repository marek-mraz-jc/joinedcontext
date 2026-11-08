//! E1: hosted mode. Streams are keyed by organization, `X-Jc-Org` is honoured only for the
//! admin token, `require_auth` closes every route but the probes, organization tokens come from a
//! digest file, and organization variables fill `${VAR}` for that organization only while `GET`
//! answers the config as posted.

use std::sync::Arc;
use std::time::Duration;

use ekiben_core::{EnvAccess, OrgId, ResourceManager};
use ekiben_server::tokens::token_line;
use ekiben_server::{
    build_router, OrgTokenFile, OrgVarsDir, ServerAuthConfig, ServerState, StreamManager,
};
use ekiben_testkit::full_registry;
use reqwest::StatusCode;
use tokio::net::TcpListener;

const ADMIN: &str = "admin-e1-token";

async fn serve(auth: ServerAuthConfig, resources: ResourceManager) -> String {
    let manager = StreamManager::new(full_registry(), resources, None, EnvAccess::AllowListed);
    let app = build_router(ServerState::new(manager, auth));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

fn two_orgs() -> ServerAuthConfig {
    ServerAuthConfig {
        admin_bearer_token: Some(ADMIN.to_string()),
        org_tokens: vec![
            ("tok-a".to_string(), OrgId::new("org-a")),
            ("tok-b".to_string(), OrgId::new("org-b")),
        ],
        ..Default::default()
    }
}

const RESIDENT: &str =
    "input:\n  generate:\n    interval: 1h\n    mapping: 'root = {}'\noutput:\n  drop: {}\n";

#[tokio::test]
async fn the_same_stream_id_lives_separately_in_two_organizations() {
    let base = serve(two_orgs(), ResourceManager::new()).await;
    let c = reqwest::Client::new();
    for tok in ["tok-a", "tok-b"] {
        let res = c
            .post(format!("{base}/streams/shared"))
            .bearer_auth(tok)
            .body(RESIDENT)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{tok}");
    }
    let res = c
        .delete(format!("{base}/streams/shared"))
        .bearer_auth("tok-a")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let a = c
        .get(format!("{base}/streams/shared"))
        .bearer_auth("tok-a")
        .send()
        .await
        .unwrap();
    assert_eq!(a.status(), StatusCode::NOT_FOUND);
    let b = c
        .get(format!("{base}/streams/shared"))
        .bearer_auth("tok-b")
        .send()
        .await
        .unwrap();
    assert_eq!(b.status(), StatusCode::OK, "org-b's stream is untouched");
    let list: serde_json::Value = c
        .get(format!("{base}/streams"))
        .bearer_auth("tok-a")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(list.as_object().unwrap().is_empty(), "{list}");
}

#[tokio::test]
async fn x_jc_org_is_honoured_for_the_admin_only() {
    let base = serve(two_orgs(), ResourceManager::new()).await;
    let c = reqwest::Client::new();
    // The admin acts for org-a through the header ...
    let res = c
        .post(format!("{base}/streams/by-admin"))
        .bearer_auth(ADMIN)
        .header("X-Jc-Org", "org-a")
        .body(RESIDENT)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let res = c
        .get(format!("{base}/streams/by-admin"))
        .bearer_auth("tok-a")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    // ... an organization token cannot name another organization ...
    let res = c
        .get(format!("{base}/streams/by-admin"))
        .bearer_auth("tok-b")
        .header("X-Jc-Org", "org-a")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    // ... naming its own is fine, and a malformed header is refused.
    let res = c
        .get(format!("{base}/streams/by-admin"))
        .bearer_auth("tok-a")
        .header("X-Jc-Org", "org-a")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let res = c
        .get(format!("{base}/streams"))
        .bearer_auth(ADMIN)
        .header("X-Jc-Org", "../org a")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn without_auth_the_header_is_ignored_and_everything_is_the_default_organization() {
    let base = serve(ServerAuthConfig::default(), ResourceManager::new()).await;
    let c = reqwest::Client::new();
    let res = c
        .post(format!("{base}/streams/open1"))
        .header("X-Jc-Org", "org-b")
        .body(RESIDENT)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let res = c.get(format!("{base}/streams/open1")).send().await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "created in the default organization"
    );
    let metrics = c
        .get(format!("{base}/metrics"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(metrics.contains("stream=\"open1\""), "{metrics}");
    assert!(!metrics.contains("org=\"org-b\""), "{metrics}");
}

#[tokio::test]
async fn require_auth_closes_every_route_but_the_probes() {
    let auth = ServerAuthConfig {
        require_auth: true,
        ..Default::default()
    };
    let base = serve(auth, ResourceManager::new()).await;
    let c = reqwest::Client::new();
    for path in [
        "/streams",
        "/metrics",
        "/version",
        "/bento/streams",
        "/streams/x",
    ] {
        let res = c.get(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "{path}");
    }
    let res = c
        .post(format!("{base}/streams/x"))
        .body(RESIDENT)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    for path in ["/ping", "/ready"] {
        let res = c.get(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{path}");
    }
}

#[tokio::test]
async fn a_wrong_and_a_missing_token_get_the_same_answer() {
    let base = serve(two_orgs(), ResourceManager::new()).await;
    let c = reqwest::Client::new();
    let missing = c.get(format!("{base}/streams")).send().await.unwrap();
    let wrong = c
        .get(format!("{base}/streams"))
        .bearer_auth("tok-c")
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(missing.text().await.unwrap(), wrong.text().await.unwrap());
}

#[tokio::test]
async fn organization_tokens_come_from_a_digest_file_and_go_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tokens");
    std::fs::write(
        &path,
        format!("# org tokens\n{}\n", token_line("org-f", "file-token-e1")),
    )
    .unwrap();
    assert!(!std::fs::read_to_string(&path)
        .unwrap()
        .contains("file-token-e1"));
    let file = Arc::new(OrgTokenFile::load(&path).unwrap());
    assert_eq!(file.len(), 1);
    assert!(!format!("{file:?}").contains("file-token-e1"));
    let auth = ServerAuthConfig {
        org_token_file: Some(file.clone()),
        ..Default::default()
    };
    assert!(!format!("{auth:?}").contains("file-token-e1"));
    let base = serve(auth, ResourceManager::new()).await;
    let c = reqwest::Client::new();
    let res = c
        .post(format!("{base}/streams/from-file"))
        .bearer_auth("file-token-e1")
        .body(RESIDENT)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let res = c
        .get(format!("{base}/streams"))
        .bearer_auth("file-token-e2")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    std::fs::remove_file(&path).unwrap();
    file.reload();
    assert!(file.is_empty(), "a removed file revokes every token");
    let res = c
        .get(format!("{base}/streams"))
        .bearer_auth("file-token-e1")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[test]
fn a_token_file_that_does_not_parse_names_the_line_not_its_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tokens");
    std::fs::write(&path, "org-x not-a-digest-but-a-raw-token\n").unwrap();
    let err = OrgTokenFile::load(&path).expect_err("refused");
    assert!(err.contains("line 1"), "{err}");
    assert!(!err.contains("not-a-digest-but-a-raw-token"), "{err}");
}

#[tokio::test]
async fn organization_variables_fill_only_their_organizations_configs_and_get_shows_them_unfilled()
{
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("org-a.yaml"),
        "GREETING: hello-from-org-a\nAPI_PASSWORD: pw-e1-secret\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("org-b.yaml"), "OTHER: 1\n").unwrap();
    let resources = ResourceManager::new();
    let mut vars = OrgVarsDir::new(dir.path());
    assert!(vars.sync(&resources).is_empty());
    let base = serve(two_orgs(), resources.clone()).await;
    let c = reqwest::Client::new();

    let conf = "input:\n  generate:\n    interval: 1h\n    mapping: 'root.g = \"${GREETING}\"'\noutput:\n  drop: {}\n  processors:\n    - mapping: 'root.password = \"${API_PASSWORD}\"'\n";
    let res = c
        .post(format!("{base}/streams/vars"))
        .bearer_auth("tok-a")
        .body(conf)
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "{}",
        res.text().await.unwrap()
    );
    let got = c
        .get(format!("{base}/streams/vars"))
        .bearer_auth("tok-a")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        got.contains("${GREETING}"),
        "GET answers the config as posted: {got}"
    );
    assert!(!got.contains("hello-from-org-a"), "{got}");
    assert!(!got.contains("pw-e1-secret"), "{got}");

    // org-b has no GREETING: the same config does not lint there, and nothing of org-a leaks.
    let res = c
        .post(format!("{base}/streams/vars"))
        .bearer_auth("tok-b")
        .body(conf)
        .send()
        .await
        .unwrap();
    assert!(!res.status().is_success());
    let body = res.text().await.unwrap();
    assert!(body.contains("GREETING"), "{body}");
    assert!(
        !body.contains("hello-from-org-a") && !body.contains("pw-e1-secret"),
        "{body}"
    );

    // A changed file replaces the whole map; a removed file empties it.
    std::fs::write(dir.path().join("org-a.yaml"), "GREETING: v2\n").unwrap();
    assert!(vars.sync(&resources).is_empty());
    let snap = resources
        .get_or_create_org(&OrgId::new("org-a"))
        .secrets_snapshot();
    assert_eq!(snap.get("GREETING").map(String::as_str), Some("v2"));
    assert!(!snap.contains_key("API_PASSWORD"));
    std::fs::remove_file(dir.path().join("org-a.yaml")).unwrap();
    vars.sync(&resources);
    assert!(resources
        .get_or_create_org(&OrgId::new("org-a"))
        .secrets_snapshot()
        .is_empty());
}

#[test]
fn an_org_vars_file_that_does_not_parse_keeps_the_previous_map_and_shows_no_value() {
    let dir = tempfile::tempdir().unwrap();
    let resources = ResourceManager::new();
    std::fs::write(dir.path().join("org-a.yaml"), "KEY: old-value\n").unwrap();
    let mut vars = OrgVarsDir::new(dir.path());
    assert!(vars.sync(&resources).is_empty());
    std::fs::write(dir.path().join("org-a.yaml"), "KEY: [new-secret-value]\n").unwrap();
    let errs = vars.sync(&resources);
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(
        errs[0].contains("KEY") && !errs[0].contains("new-secret-value"),
        "{errs:?}"
    );
    let snap = resources
        .get_or_create_org(&OrgId::new("org-a"))
        .secrets_snapshot();
    assert_eq!(snap.get("KEY").map(String::as_str), Some("old-value"));
}

#[tokio::test]
async fn ready_names_only_the_callers_own_streams() {
    let base = serve(two_orgs(), ResourceManager::new()).await;
    let c = reqwest::Client::new();
    // TEST-NET-1 is never reachable: the input stays unconnected.
    let unreachable = "input:\n  websocket:\n    url: ws://192.0.2.1:9/ws\noutput:\n  drop: {}\n";
    for (tok, id) in [("tok-a", "a-hidden-name"), ("tok-b", "b-hidden-name")] {
        let res = c
            .post(format!("{base}/streams/{id}"))
            .bearer_auth(tok)
            .body(unreachable)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }
    let mut open = String::new();
    for _ in 0..50 {
        let res = c.get(format!("{base}/ready")).send().await.unwrap();
        if res.status() == StatusCode::SERVICE_UNAVAILABLE {
            open = res.text().await.unwrap();
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!open.is_empty(), "the streams are reported not ready");
    assert!(
        !open.contains("hidden-name"),
        "an anonymous probe sees no names: {open}"
    );
    let a = c
        .get(format!("{base}/ready"))
        .bearer_auth("tok-a")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        a.contains("a-hidden-name") && !a.contains("b-hidden-name"),
        "{a}"
    );
    let all = c
        .get(format!("{base}/ready"))
        .bearer_auth(ADMIN)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        all.contains("a-hidden-name") && all.contains("b-hidden-name"),
        "{all}"
    );
}
