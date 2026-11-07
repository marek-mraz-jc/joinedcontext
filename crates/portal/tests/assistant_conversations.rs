//! The stored assistant conversations (API/01 §34, API/05 §3.1, AG-99, T-4670): only the
//! assistant's administrator reads them and only one who may delete a deployment erases one,
//! neither a member nor a stranger reaches jc-assistant, the filter goes on as asked, and every
//! call leaves an `assistant.transcript` activity record. Since T-4681: the turns of the same
//! question, 60 calls a minute per person, and a probe of more than 20 misses recorded.

mod common;

use serde_json::{json, Value};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use joinedcontext_portal::activity::ActivityFilter;
use joinedcontext_portal::auth::oidc::OidcClient;
use joinedcontext_portal::auth::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

const ASSISTANT_PATH: &str = "/internal/v1/projects/helsinki/knowledge/conversations";
const LIST: &str = "/api/v1/projects/helsinki/knowledge/conversations";
const ONE: &str = "11111111-1111-4111-8111-111111111111";

async fn rig() -> (AppState, MockServer, MockServer) {
    let realm = MockServer::start().await;
    let issuer = format!("{}/realms/helsinki", realm.uri());
    Mock::given(method("GET"))
        .and(path("/realms/helsinki/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/protocol/openid-connect/auth"),
            "token_endpoint": format!("{issuer}/protocol/openid-connect/token"),
            "jwks_uri": format!("{issuer}/protocol/openid-connect/certs"),
            "response_types_supported": ["code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["ES256"]
        })))
        .mount(&realm)
        .await;
    Mock::given(method("GET"))
        .and(path("/realms/helsinki/protocol/openid-connect/certs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "keys": [] })))
        .mount(&realm)
        .await;
    Mock::given(method("POST"))
        .and(path("/realms/helsinki/protocol/openid-connect/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "the-portals-own-token", "token_type": "Bearer", "expires_in": 300
        })))
        .mount(&realm)
        .await;
    let assistant = MockServer::start().await;
    let mut config = Config::from_vars(|key| {
        match key {
            "JC_OIDC_ISSUER" => Some(issuer.as_str()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001")
            }
            "JC_OIDC_CLIENT_ID" => Some("portal-api"),
            "JC_OIDC_CLIENT_SECRET" => Some("test-secret"),
            "JC_PORTAL_BOOTSTRAP_ADMINS" => Some("portal-approver"),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("config");
    config.knowledge_url = Some(assistant.uri());
    let oidc = OidcClient::discover(
        config.oidc.as_ref().expect("a realm"),
        "https://portal.test/api/v1/auth/callback",
    )
    .await
    .expect("discovery");
    let mirror = Mirror::new();
    mirror.upsert(common::envelope(
        "AssistantDeployment",
        "obcan",
        "helsinki",
        json!({ "publicId": "helsinki-public", "channel": "internal", "sources": [] }),
    ));
    // A member who may read the deployment and approve nothing.
    mirror.upsert(common::envelope(
        "Role",
        "assistant-reader",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["AssistantDeployment", "KnowledgeSource"], "verbs": ["read"] }] }),
    ));
    mirror.upsert(common::envelope(
        "RoleBinding",
        "jana-assistant",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "jana@hel.fi" }], "role": "assistant-reader", "scope": { "project": "helsinki" } }),
    ));
    // The assistant's reviewer: approve without delete.
    mirror.upsert(common::envelope(
        "Role",
        "assistant-reviewer",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["AssistantDeployment"], "verbs": ["read", "approve"] }] }),
    ));
    mirror.upsert(common::envelope(
        "RoleBinding",
        "eva-assistant",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "eva@hel.fi" }], "role": "assistant-reviewer", "scope": { "project": "helsinki" } }),
    ));
    let state = AppState::new(config, Some(oidc)).with_mirror(std::sync::Arc::new(mirror));
    (state, assistant, realm)
}

fn admin() -> Identity {
    approver("anna")
}

/// An administrator of the assistant with a subject of their own, so a rate window is theirs.
fn approver(name: &str) -> Identity {
    let mut person = common::person(name);
    person.groups = vec!["portal-approver".into()];
    person
}

async fn asked(assistant: &MockServer) -> usize {
    assistant
        .received_requests()
        .await
        .unwrap_or_default()
        .len()
}

async fn recorded(state: &AppState) -> Vec<Value> {
    let filter = ActivityFilter {
        kinds: vec!["assistant.transcript".into()],
        limit: 50,
        ..Default::default()
    };
    let page = state
        .activity
        .list("helsinki", &filter)
        .await
        .expect("a page");
    page.items.iter().map(|e| e.details.clone()).collect()
}

#[tokio::test]
async fn only_the_administrator_reads_and_a_member_or_stranger_never_reaches_the_assistant() {
    let (state, assistant, _realm) = rig().await;
    let detail = format!("{LIST}/{ONE}");
    for who in [common::person("nobody"), common::person("jana")] {
        for (http, uri) in [
            ("GET", LIST),
            ("GET", detail.as_str()),
            ("DELETE", detail.as_str()),
        ] {
            let answer = common::send(&state, who.clone(), http, uri, None).await;
            assert_eq!(
                answer.status, 403,
                "{} {http} {uri}: {}",
                who.username, answer.text
            );
            assert!(
                answer.text.contains("AssistantDeployment"),
                "{}",
                answer.text
            );
        }
    }
    assert_eq!(asked(&assistant).await, 0);
    assert!(recorded(&state).await.is_empty(), "a refusal reads nothing");

    Mock::given(method("GET"))
        .and(path(ASSISTANT_PATH))
        .and(header("authorization", "Bearer the-portals-own-token"))
        .and(query_param("status", "timeout"))
        .and(query_param("sort", "costUsd"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{"id": ONE, "turns": 2, "costUsd": 0.0046}], "nextCursor": null
        })))
        .mount(&assistant)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{ASSISTANT_PATH}/{ONE}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": ONE, "turns": [{"turn": 1, "question": "Kedy?", "steps": []}]
        })))
        .mount(&assistant)
        .await;
    let list = common::send(
        &state,
        common::person("eva"),
        "GET",
        &format!("{LIST}?status=timeout&sort=costUsd"),
        None,
    )
    .await;
    assert_eq!(list.status, 200, "{}", list.text);
    let body: Value = serde_json::from_str(&list.text).expect("json");
    assert_eq!(body["items"][0]["id"], ONE);
    let one = common::send(&state, admin(), "GET", &detail, None).await;
    assert_eq!(one.status, 200, "{}", one.text);
    let body: Value = serde_json::from_str(&one.text).expect("json");
    assert_eq!(body["turns"][0]["question"], "Kedy?");

    let records = recorded(&state).await;
    assert_eq!(records.len(), 2, "{records:?}");
    assert!(records.iter().any(|r| r["actor"] == "eva"
        && r["of"]["status"] == "timeout"
        && r["of"]["sort"] == "costUsd"));
    assert!(records
        .iter()
        .any(|r| r["actor"] == "anna" && r["of"] == ONE));
}

#[tokio::test]
async fn an_erase_needs_delete_and_a_bad_id_or_filter_never_reaches_the_assistant() {
    let (state, assistant, _realm) = rig().await;
    let detail = format!("{LIST}/{ONE}");
    let reviewer = common::send(&state, common::person("eva"), "DELETE", &detail, None).await;
    assert_eq!(
        reviewer.status, 403,
        "approve alone erases nothing: {}",
        reviewer.text
    );

    for (uri, status) in [
        (format!("{LIST}/not-a-uuid"), 404),
        (format!("{LIST}/{}", ONE.replace('1', "g")), 404),
        (format!("{LIST}?colour=red"), 400),
        (format!("{LIST}?limit=many"), 400),
    ] {
        let answer = common::send(&state, admin(), "GET", &uri, None).await;
        assert_eq!(answer.status.as_u16(), status, "{uri}: {}", answer.text);
    }
    assert_eq!(asked(&assistant).await, 0);

    Mock::given(method("DELETE"))
        .and(path(format!("{ASSISTANT_PATH}/{ONE}")))
        .and(header("authorization", "Bearer the-portals-own-token"))
        .respond_with(ResponseTemplate::new(204))
        .up_to_n_times(1)
        .mount(&assistant)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("{ASSISTANT_PATH}/{ONE}")))
        .respond_with(
            ResponseTemplate::new(404).set_body_json(json!({"detail": "no such conversation"})),
        )
        .mount(&assistant)
        .await;
    let erased = common::send(&state, admin(), "DELETE", &detail, None).await;
    assert_eq!(erased.status, 204, "{}", erased.text);
    let again = common::send(&state, admin(), "DELETE", &detail, None).await;
    assert_eq!(again.status, 404, "{}", again.text);
    let records = recorded(&state).await;
    assert_eq!(
        records.len(),
        1,
        "the erase is recorded, the refused one not: {records:?}"
    );
    assert_eq!(records[0]["action"], "erased a stored conversation");
}

#[tokio::test]
async fn the_same_question_is_asked_of_the_assistant_for_an_administrator_alone() {
    let (state, assistant, _realm) = rig().await;
    let same = format!("{LIST}/{ONE}/turns/2/same");
    let member = common::send(&state, common::person("jana"), "GET", &same, None).await;
    assert_eq!(member.status, 403, "{}", member.text);
    for uri in [
        format!("{LIST}/{ONE}/turns/0/same"),
        format!("{LIST}/{ONE}/turns/201/same"),
        format!("{LIST}/nope/turns/1/same"),
    ] {
        let answer = common::send(&state, admin(), "GET", &uri, None).await;
        assert_eq!(answer.status, 404, "{uri}: {}", answer.text);
    }
    assert_eq!(asked(&assistant).await, 0);
    Mock::given(method("GET"))
        .and(path(format!("{ASSISTANT_PATH}/{ONE}/turns/2/same")))
        .and(header("authorization", "Bearer the-portals-own-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{"conversation": ONE, "turn": 2, "steps": 7}]
        })))
        .mount(&assistant)
        .await;
    let found = common::send(&state, admin(), "GET", &same, None).await;
    assert_eq!(found.status, 200, "{}", found.text);
    let body: Value = serde_json::from_str(&found.text).expect("json");
    assert_eq!(body["items"][0]["steps"], 7);
    let records = recorded(&state).await;
    assert!(
        records
            .iter()
            .any(|r| r["action"] == "listed the turns of one question" && r["of"]["turn"] == 2),
        "{records:?}"
    );
}

#[tokio::test]
async fn the_sixty_first_call_in_a_minute_waits_and_twenty_one_misses_are_a_recorded_probe() {
    let (state, assistant, _realm) = rig().await;
    Mock::given(method("GET"))
        .and(path(format!("{ASSISTANT_PATH}/{ONE}")))
        .respond_with(
            ResponseTemplate::new(404).set_body_json(json!({"detail": "no such conversation"})),
        )
        .mount(&assistant)
        .await;
    let prober = approver("probing-admin");
    let mut statuses = Vec::new();
    for _ in 0..61 {
        let answer = common::send(
            &state,
            prober.clone(),
            "GET",
            &format!("{LIST}/{ONE}"),
            None,
        )
        .await;
        statuses.push(answer.status.as_u16());
    }
    assert!(statuses[..60].iter().all(|s| *s == 404), "{statuses:?}");
    assert_eq!(statuses[60], 429);
    let probes: Vec<Value> = recorded(&state)
        .await
        .into_iter()
        .filter(|r| r["action"] == "probe")
        .collect();
    assert_eq!(
        probes.len(),
        1,
        "one record for the probe, not one per miss: {probes:?}"
    );
    assert_eq!(probes[0]["actor"], "probing-admin");
    let other = common::send(
        &state,
        approver("calm-admin"),
        "GET",
        &format!("{LIST}/{ONE}"),
        None,
    )
    .await;
    assert_eq!(other.status, 404, "another person's window is their own");
}

/// T-4728: the usage summary and the CSV export answer the assistant's administrator alone, pass
/// the range on as asked, keep the CSV as the assistant wrote it, and are recorded.
#[tokio::test]
async fn the_usage_summary_and_the_export_are_the_administrators_alone_and_recorded() {
    let (state, assistant, _realm) = rig().await;
    let usage = format!("{LIST}/usage?deployment=obcan&route=data");
    let export = format!("{LIST}/export?from=2026-10-18T00:00:00Z");
    for who in [common::person("nobody"), common::person("jana")] {
        for uri in [usage.as_str(), export.as_str()] {
            let answer = common::send(&state, who.clone(), "GET", uri, None).await;
            assert_eq!(
                answer.status, 403,
                "{} {uri}: {}",
                who.username, answer.text
            );
        }
    }
    assert_eq!(
        asked(&assistant).await,
        0,
        "a refusal never reaches the assistant"
    );

    Mock::given(method("GET"))
        .and(path(format!("{ASSISTANT_PATH}/usage")))
        .and(header("authorization", "Bearer the-portals-own-token"))
        .and(query_param("deployment", "obcan"))
        .and(query_param("route", "data"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "questions": 3, "uncostedQuestions": 1, "costUsd": {"total": 0.0146}
        })))
        .mount(&assistant)
        .await;
    let csv = "at,question\r\n2026-10-19T07:00:00Z,\"a, \"\"b\"\"\n[e-mail]\"\r\n";
    Mock::given(method("GET"))
        .and(path(format!("{ASSISTANT_PATH}/export")))
        .and(query_param("from", "2026-10-18T00:00:00Z"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/csv; charset=utf-8")
                .set_body_string(csv),
        )
        .mount(&assistant)
        .await;

    let summary = common::send(&state, common::person("eva"), "GET", &usage, None).await;
    assert_eq!(summary.status, 200, "{}", summary.text);
    let body: Value = serde_json::from_str(&summary.text).expect("json");
    assert_eq!(body["uncostedQuestions"], 1);
    let file = common::send(&state, admin(), "GET", &export, None).await;
    assert_eq!(file.status, 200, "{}", file.text);
    assert_eq!(file.text, csv, "the CSV is passed on byte for byte");

    let records = recorded(&state).await;
    assert!(records.iter().any(|r| r["actor"] == "eva"
        && r["action"] == "summarized the stored conversations"
        && r["of"]["route"] == "data"));
    assert!(records
        .iter()
        .any(|r| r["actor"] == "anna" && r["action"] == "exported the stored conversations"));
}

/// The assistant's refusal of a range reaches the caller as the same 400, never as a CSV.
#[tokio::test]
async fn an_export_the_assistant_refuses_is_a_400_with_its_reason() {
    let (state, assistant, _realm) = rig().await;
    Mock::given(method("GET"))
        .and(path(format!("{ASSISTANT_PATH}/export")))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "title": "Bad Request", "status": 400, "detail": "more than 20000 turns; narrow the range"
        })))
        .mount(&assistant)
        .await;
    let answer = common::send(&state, admin(), "GET", &format!("{LIST}/export"), None).await;
    assert_eq!(answer.status, 400, "{}", answer.text);
    assert!(answer.text.contains("narrow the range"), "{}", answer.text);
    let unknown = common::send(
        &state,
        admin(),
        "GET",
        &format!("{LIST}/usage?bogus=1"),
        None,
    )
    .await;
    assert_eq!(unknown.status, 400, "{}", unknown.text);
}
