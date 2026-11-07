//! Edge cases of the three doors that are opened without a session: `detach`, the sync webhook and
//! the Gitea webhook (T-2043, T-2044, T-2045; PF-51, PF-57, AG-59, CC-03, CC-08, CC-18).
//!
//! **The contract, in one sentence:** detaching a source is a `delete` in its project and the person
//! doing it is signed in, while the two webhooks have no session at all and are authorised by one
//! HMAC over the exact bytes of the body — so every refusal happens before the forge, the loop and the
//! JSON parser are reached, and no answer says anything a caller did not already know.
//!
//! The happy paths and the plain refusals live in `sync_source_routes_tests.rs` (who may drive, 404 for
//! a source of another project) and `webhook_tests.rs` (a merged pull request syncs, a wrong, missing
//! or non-hex signature is 401, no secret is 503, an unhandled event is 204). This file is what those
//! leave: the order of the checks, the bytes the signature covers, and the two states a half-finished
//! detach can leave behind.
//!
//! Tests only (the family's rule). Every case here is green; a red one becomes its own task.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use hmac::{Hmac, Mac};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sha2::Sha256;
use tower::ServiceExt;
use wiremock::MockServer;

use common::{envelope, forge, person, send};
use jcctl::sync::{RemoteError, SyncRemote};
use joinedcontext_portal::api::webhook::{EVENT_HEADER, SIGNATURE_HEADER};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::sync::driver::{Driver, Remote};
use joinedcontext_portal::sync::state::States;

const PROJECT: &str = "bb";
const SOURCE: &str = "regional";
const SECRET: &str = "the-current-webhook-secret";
const RETIRED: &str = "the-secret-being-retired";
/// A second source's own secret: what must not open the first one.
const OTHER: &str = "the-neighbouring-sources-secret";
/// The forge's own hook secret, which used to authorise every sync source there was.
const GITEA: &str = "the-forges-own-webhook-secret";
const STATUS: &str = "/api/v1/projects/bb/syncsources/regional/status";
const DETACH: &str = "/api/v1/projects/bb/syncsources/regional/detach";
const HOOK: &str = "/api/v1/webhooks/sync/bb/regional";
const GITEA_HOOK: &str = "/api/v1/webhooks/gitea";

type HmacSha256 = Hmac<Sha256>;

/// An origin nobody reaches: none of these routes should get as far as a fetch.
struct Unreachable;

impl SyncRemote for Unreachable {
    fn revision(&self, _: &jc_core::kinds::SyncOrigin) -> Result<String, RemoteError> {
        Err(RemoteError::Unavailable("not in this test".into()))
    }

    fn checkout(
        &self,
        _: &jc_core::kinds::SyncOrigin,
        _: &str,
        _: &std::path::Path,
    ) -> Result<(), RemoteError> {
        Err(RemoteError::Unavailable("not in this test".into()))
    }
}

/// The hex HMAC-SHA256 a caller presents, as Gitea computes it.
fn sign(secret: &str, body: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac key");
    mac.update(body.as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The Portal over `gitea` with the webhook secrets an installation has configured. The config is
/// behind an `Arc` once the state holds it, so the two secrets are set here rather than afterwards.
fn portal_on(gitea: &MockServer, secret: Option<&str>, previous: Option<&str>) -> AppState {
    let mut config = Config::for_tests();
    config.gitea_webhook_secret = secret.map(str::to_owned);
    config.gitea_webhook_secret_previous = previous.map(str::to_owned);
    let client = GiteaClient::new(
        gitea.uri().parse().expect("mock url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
    AppState::new(config, None).with_gitea(Arc::new(client))
}

/// The Portal with the sync loop over `gitea`, the source `bb/regional` in the mirror, and a role
/// carrying `verbs` on SyncSource bound to `jana`.
fn state_with(
    gitea: &MockServer,
    verbs: &[&str],
    secret: Option<&str>,
    previous: Option<&str>,
) -> AppState {
    let mut state = portal_on(gitea, secret, previous);
    let client = state.gitea.clone().expect("forge client");
    let remote: Remote = Arc::new(Unreachable);
    state.sync = Some(Arc::new(Driver::new(
        client,
        Arc::new(States::new(None)),
        remote,
    )));
    state.mirror.upsert(envelope(
        "SyncSource",
        SOURCE,
        PROJECT,
        json!({
            "source": { "git": { "url": "https://git.region.sk/udp/models.git", "ref": "main" } },
            "schedule": { "every": "1h" },
            "mode": "mirror",
            "conflictPolicy": "replace"
        }),
    ));
    state.mirror.upsert(envelope(
        "Role",
        "driver",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["SyncSource"], "verbs": verbs }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "jana-driver",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "jana@hel.fi" }], "role": "driver",
                "scope": { "project": PROJECT } }),
    ));
    state
}

/// One unsigned-in POST with the headers and the exact body bytes a caller sends. No cookie and no
/// CSRF token: these two doors are the ones that have none.
async fn hook(
    state: &AppState,
    uri: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (StatusCode, String) {
    let mut request = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = server::app(state.clone())
        .oneshot(
            request
                .body(Body::from(body.to_owned()))
                .expect("a request"),
        )
        .await
        .expect("a response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Everything the forge was asked to do, as `VERB /path`, so a case can prove it was asked nothing.
async fn touched(gitea: &MockServer) -> Vec<String> {
    gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|request| format!("{} {}", request.method, request.url.path()))
        .collect()
}

// -------------------------------------------------------------------------------------------------
// T-2043 detach
// -------------------------------------------------------------------------------------------------

/// PF-51: detaching is a `delete`, and the refusal comes before the loop and the forge. A caller who
/// may read and propose but not delete is told so without a branch, a commit or a merge request being
/// made, and a source that is not there is not disclosed to them either.
#[tokio::test]
async fn detaching_without_delete_touches_neither_the_loop_nor_the_forge() {
    let gitea = forge().await;
    let state = state_with(&gitea, &["read", "propose"], None, None);

    let answer = send(&state, person("jana"), "POST", DETACH, None).await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.text);

    // Not even the source's own name comes back, and the loop was not touched: the source is still
    // running, which a detach would have switched off before opening its merge request.
    let status = send(&state, person("jana"), "GET", STATUS, None).await;
    assert_eq!(status.status, StatusCode::OK, "{}", status.text);
    let reported: Value = serde_json::from_str(&status.text).expect("a status");
    assert_eq!(reported["paused"], json!(false), "{}", status.text);

    // The forge was read for nothing and written to not at all.
    let writes: Vec<String> = touched(&gitea)
        .await
        .into_iter()
        .filter(|call| !call.starts_with("GET "))
        .collect();
    assert!(writes.is_empty(), "the forge was asked to {writes:?}");
}

/// PF-51: the order of the three checks a detach makes is caller, then loop, then forge, then source.
/// A source that does not exist is refused by name last, so the answer to a probe depends on what the
/// installation has rather than on which sources it holds — and a caller with `delete` on a name that
/// could not be a source is told the same as one asking for a source that is simply absent.
#[tokio::test]
async fn a_detach_of_a_source_that_is_not_there_is_the_same_answer_for_every_name() {
    let gitea = forge().await;
    let state = state_with(&gitea, &["read", "propose", "delete"], None, None);

    // A name that is not there, one that differs only in case, one that walks up a directory and one
    // with a trailing space: the same problem document for all four, and the only thing that varies
    // in it is the name the caller asked for, spelled back to them.
    for (name, asked) in [
        ("nothing-like-this", "nothing-like-this"),
        ("Regional", "Regional"),
        ("%2e%2e", ".."),
        ("regional%20", "regional "),
    ] {
        let uri = format!("/api/v1/projects/{PROJECT}/syncsources/{name}/detach");
        let answer = send(&state, person("jana"), "POST", &uri, None).await;
        assert_eq!(
            answer.status,
            StatusCode::NOT_FOUND,
            "{name}: {}",
            answer.text
        );
        let problem: Value = serde_json::from_str(&answer.text).expect("a problem document");
        assert_eq!(problem["title"], json!("Resource Not Found"), "{name}");
        assert_eq!(
            problem["detail"],
            json!(format!(
                "sync source '{asked}' not found in project '{PROJECT}'"
            )),
            "{name} is answered with more than the name it asked for",
        );
    }

    // Nothing was written for any of them: a probe of four names costs the forge nothing.
    let writes: Vec<String> = touched(&gitea)
        .await
        .into_iter()
        .filter(|call| !call.starts_with("GET "))
        .collect();
    assert!(writes.is_empty(), "the forge was asked to {writes:?}");
}

/// A detach whose merge request the forge refuses leaves the source paused and nothing to review.
///
/// This is today's behaviour, not the wanted one, and it is deliberate in the handler: the pause is
/// taken before the proposal is opened (`src/api/sync_sources.rs:239`) so that a source cannot keep
/// opening merge requests while its own removal waits for review. When the forge then fails, the
/// source is off, no merge request exists and nothing turns it back on — the project silently stops
/// receiving the standard it syncs. Written down in `/workspace/chyby.md`; when the handler learns to
/// put the pause back on a failed proposal, the second half of this case is the one to change.
#[tokio::test]
async fn a_detach_the_forge_refuses_leaves_the_source_paused_and_no_merge_request() {
    // A forge with nothing mounted: every call it gets answers 404, which is what a repository the
    // Portal cannot read looks like from inside `proposal::open`.
    let gitea = MockServer::start().await;
    let state = state_with(&gitea, &["read", "propose", "delete"], None, None);

    let answer = send(&state, person("jana"), "POST", DETACH, None).await;
    assert!(
        answer.status.is_client_error() || answer.status.is_server_error(),
        "a detach against an unreadable forge answered {}: {}",
        answer.status,
        answer.text,
    );
    assert!(
        !answer.text.contains("token-xyz"),
        "the forge token is in the answer: {}",
        answer.text,
    );

    let status = send(&state, person("jana"), "GET", STATUS, None).await;
    assert_eq!(status.status, StatusCode::OK, "{}", status.text);
    let reported: Value = serde_json::from_str(&status.text).expect("a status");
    assert_eq!(
        reported["paused"],
        json!(true),
        "the pause is taken before the proposal, so a failed detach leaves it: {}",
        status.text,
    );
}

// -------------------------------------------------------------------------------------------------
// T-2044 the sync webhook
// -------------------------------------------------------------------------------------------------

/// CC-18, PF-57: the sync webhook has no session, so the signature is the whole of its authority and
/// is checked first. A body nobody signed, a signature of the same JSON spaced differently, a
/// signature in the wrong alphabet and no signature at all are all 401 — before the project and the
/// source in the path are looked at, so the door says nothing about what the installation syncs.
#[tokio::test]
async fn the_sync_webhook_refuses_before_it_reads_the_project_or_the_source() {
    let gitea = forge().await;
    let state = state_with(&gitea, &["read", "propose", "delete"], Some(SECRET), None);
    let body = r#"{"pushed":true}"#;

    // The signature covers the exact bytes: the same JSON with one space in it is a different body.
    let spaced = r#"{"pushed": true}"#;
    let cases: Vec<(&str, String, &str)> = vec![
        ("no signature", String::new(), body),
        ("a signature of nothing", sign(SECRET, ""), body),
        ("another secret's signature", sign("guessed", body), body),
        (
            "a signature of the compact body",
            sign(SECRET, body),
            spaced,
        ),
        (
            "half a signature",
            sign(SECRET, body)[..32].to_owned(),
            body,
        ),
        (
            "one hex digit too many",
            format!("{}0", sign(SECRET, body)),
            body,
        ),
        ("not hex at all", "zzzz".into(), body),
    ];

    for (what, signature, sent) in cases {
        // The same request against a source nobody has: still 401, so a probe learns nothing.
        for uri in [HOOK, "/api/v1/webhooks/sync/nothing/at-all"] {
            let headers: Vec<(&str, &str)> = if signature.is_empty() {
                Vec::new()
            } else {
                vec![(SIGNATURE_HEADER, signature.as_str())]
            };
            let (status, text) = hook(&state, uri, &headers, sent).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{what} on {uri}: {text}");
            assert!(
                !text.contains(SECRET) && !text.contains(SOURCE),
                "{what} on {uri} answered with the secret or the source: {text}",
            );
        }
    }

    // And no refusal reached the forge.
    let writes: Vec<String> = touched(&gitea)
        .await
        .into_iter()
        .filter(|call| !call.starts_with("GET "))
        .collect();
    assert!(writes.is_empty(), "the forge was asked to {writes:?}");
}

/// MF-44, T-2297: a run through the sync webhook is authorised by **that source's own** secret, and
/// by nothing platform-wide.
///
/// The signature covers the request body and not the path, so one secret for every source made the
/// same signed body work on every path: the origin of one project's source could force a run of
/// another project's. Each source now carries `spec.webhook.secretRef`, the reconciler resolves it
/// per pass, and the route asks what it left (`src/sync/webhook_secrets.rs`).
#[tokio::test]
async fn a_source_is_run_by_its_own_secret_and_by_nobody_elses() {
    let gitea = forge().await;
    // The platform's own Gitea secret is a third value here, so an answer that took it would show.
    let state = state_with(&gitea, &["read", "propose", "delete"], Some(GITEA), None);
    state
        .webhook_secrets
        .replace_all(std::collections::BTreeMap::from([
            (
                (PROJECT.to_owned(), SOURCE.to_owned()),
                vec![SECRET.to_owned()],
            ),
            (
                (PROJECT.to_owned(), "neighbours".to_owned()),
                vec![OTHER.to_owned()],
            ),
        ]));
    let body = r#"{"pushed":true}"#;

    // Its own secret: past the signature and into the loop, which has no remote it can reach, so
    // the run fails rather than being refused.
    let (status, text) = hook(
        &state,
        HOOK,
        &[(SIGNATURE_HEADER, &sign(SECRET, body))],
        body,
    )
    .await;
    assert_ne!(status, StatusCode::UNAUTHORIZED, "{text}");
    assert_ne!(status, StatusCode::NOT_FOUND, "{text}");

    // The same signed body on the neighbouring source's path, and the neighbour's secret on this
    // one: both refused. This is the whole point of a secret per source.
    for (uri, signature) in [
        ("/api/v1/webhooks/sync/bb/neighbours", sign(SECRET, body)),
        (HOOK, sign(OTHER, body)),
    ] {
        let (status, text) = hook(&state, uri, &[(SIGNATURE_HEADER, &signature)], body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}: {text}");
    }

    // And the forge's own webhook secret, which used to open every source, opens none of them —
    // while the Gitea hook it belongs to still takes it.
    let forge_signed = sign(GITEA, body);
    let (status, text) = hook(&state, HOOK, &[(SIGNATURE_HEADER, &forge_signed)], body).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{text}");
    let (status, text) = hook(
        &state,
        GITEA_HOOK,
        &[(SIGNATURE_HEADER, &forge_signed), (EVENT_HEADER, "ping")],
        body,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{text}");
}

/// MF-44, T-0982: the secret being retired is accepted beside the current one, so the value is
/// written in one commit and the origin's own hook moved in another. Without that window every
/// rotation is an outage, which is why nobody rotates.
#[tokio::test]
async fn the_retiring_secret_is_accepted_until_a_pass_drops_it() {
    let gitea = forge().await;
    let state = state_with(&gitea, &["read", "propose", "delete"], Some(GITEA), None);
    let body = r#"{"pushed":true}"#;
    let held = |secrets: Vec<String>| {
        state
            .webhook_secrets
            .replace_all(std::collections::BTreeMap::from([(
                (PROJECT.to_owned(), SOURCE.to_owned()),
                secrets,
            )]))
    };

    held(vec![SECRET.to_owned(), RETIRED.to_owned()]);
    for secret in [SECRET, RETIRED] {
        let (status, text) = hook(
            &state,
            HOOK,
            &[(SIGNATURE_HEADER, &sign(secret, body))],
            body,
        )
        .await;
        assert_ne!(status, StatusCode::UNAUTHORIZED, "{secret}: {text}");
    }

    // The manifest drops `previousSecretRef`, the next pass resolves one secret, and the old one
    // stops working that pass.
    held(vec![SECRET.to_owned()]);
    let (status, text) = hook(
        &state,
        HOOK,
        &[(SIGNATURE_HEADER, &sign(RETIRED, body))],
        body,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{text}");
}

/// PF-59, R20: the sync webhook is unauthenticated until the signature verifies, so every refusal
/// at it is one status with one body.
///
/// A source that is not there, a source with no `spec.webhook`, one whose reference this instance
/// could not resolve, a wrong signature and a missing one are all the same `401`. A `404` would be
/// an existence oracle over the sources of every project, and a `503` would say which installations
/// have no secret backend — to a caller who has proved nothing.
#[tokio::test]
async fn every_refusal_at_the_sync_webhook_is_the_same_answer() {
    let gitea = forge().await;
    let state = state_with(&gitea, &["read", "propose", "delete"], None, None);
    assert!(
        state.webhook_secrets.is_empty(),
        "this case is about a Portal that has resolved no webhook secret at all",
    );
    let body = "{}";
    let wrong = sign(SECRET, body);

    let mut answers = Vec::new();
    for (uri, headers) in [
        // The source this installation holds, which no pass has resolved a secret for.
        (HOOK, vec![(SIGNATURE_HEADER, wrong.as_str())]),
        // A source it does not hold at all.
        (
            "/api/v1/webhooks/sync/nothing/at-all",
            vec![(SIGNATURE_HEADER, wrong.as_str())],
        ),
        // No signature at all, and one that is not hex.
        (HOOK, Vec::new()),
        (HOOK, vec![(SIGNATURE_HEADER, "not-a-signature")]),
    ] {
        let (status, text) = hook(&state, uri, &headers, body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}: {text}");
        answers.push(text);
    }
    assert!(
        answers.windows(2).all(|pair| pair[0] == pair[1]),
        "the refusals are not worded alike: {answers:?}",
    );

    let writes: Vec<String> = touched(&gitea)
        .await
        .into_iter()
        .filter(|call| !call.starts_with("GET "))
        .collect();
    assert!(writes.is_empty(), "the forge was asked to {writes:?}");
}

// -------------------------------------------------------------------------------------------------
// T-2045 the Gitea webhook
// -------------------------------------------------------------------------------------------------

/// CC-03, CC-08: the Gitea hook's signature is checked before the body is parsed, and what it accepts
/// as a signature is exactly one HMAC of exactly these bytes. Mixed-case hex and a padded value are
/// the same signature; a truncated, lengthened or re-spaced one is not; and a body that is not JSON is
/// refused only after the signature was good, so an unauthorised caller never reaches the parser.
#[tokio::test]
async fn the_gitea_hook_takes_one_signature_of_these_bytes_and_parses_nothing_before_it() {
    let gitea = MockServer::start().await;
    let state = portal_on(&gitea, Some(SECRET), None);
    let body = r#"{"ref":"refs/heads/main"}"#;
    let signature = sign(SECRET, body);

    // Mixed case and surrounding whitespace are the same signature: `decode_hex` reads either case
    // and the value is trimmed (`src/api/webhook.rs:39`). Both are accepted, which is today's
    // behaviour and harmless — the bytes compared are the same bytes.
    for presented in [
        signature.clone(),
        signature.to_uppercase(),
        format!("  {signature} "),
    ] {
        let (status, text) =
            hook(&state, GITEA_HOOK, &[(SIGNATURE_HEADER, &presented)], body).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{presented}: {text}");
    }

    // A signature that is not this one, in every way it can fail to be: all 401, and the body is
    // never parsed, so a caller cannot use the parser's answer to tell a good secret from a bad one.
    let malformed = [
        signature[..62].to_owned(),
        format!("{signature}ab"),
        signature.replace('a', "g"),
        sign(SECRET, r#"{"ref":"refs/heads/other"}"#),
        "00".repeat(32),
    ];
    for presented in malformed {
        for sent in [body, "not json at all", ""] {
            let (status, text) =
                hook(&state, GITEA_HOOK, &[(SIGNATURE_HEADER, &presented)], sent).await;
            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "{presented} / {sent}: {text}"
            );
            assert!(
                !text.contains(SECRET),
                "the secret is in the answer: {text}"
            );
        }
    }

    // Signed, and not JSON: 400 now that the signature was good, and still nothing synced.
    for sent in ["not json at all", "", "[", "{\"ref\":}"] {
        let presented = sign(SECRET, sent);
        let (status, text) =
            hook(&state, GITEA_HOOK, &[(SIGNATURE_HEADER, &presented)], sent).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{sent:?}: {text}");
    }

    assert!(
        touched(&gitea).await.is_empty(),
        "the forge was touched by a webhook that synced nothing",
    );
}

/// CC-08: only a merge into the default branch and a merged pull request are worth a sync. A tag, a
/// side branch, an event the Portal does not handle and a payload whose `ref` is not a string are all
/// 204 — accepted, so the forge stops retrying, and nothing is read or written because of them.
#[tokio::test]
async fn a_signed_event_that_changes_nothing_is_accepted_and_syncs_nothing() {
    let gitea = MockServer::start().await;
    let state = portal_on(&gitea, Some(SECRET), None);

    let nothing_to_do = [
        ("a tag", "push", json!({ "ref": "refs/tags/v1.0.0" })),
        (
            "a side branch",
            "push",
            json!({ "ref": "refs/heads/feature/x",
                    "repository": { "default_branch": "main" } }),
        ),
        (
            "a branch whose name is main's with a space",
            "push",
            json!({ "ref": "refs/heads/main ", "repository": { "default_branch": "main" } }),
        ),
        (
            "a ref that is not a string",
            "push",
            json!({ "ref": ["refs/heads/main"] }),
        ),
        ("a null ref", "push", json!({ "ref": Value::Null })),
        ("an empty object", "push", json!({})),
        (
            "a pull request that was closed unmerged",
            "pull_request",
            json!({ "action": "closed", "pull_request": { "merged": false } }),
        ),
        (
            "a pull request that was merged but not closed",
            "pull_request",
            json!({ "action": "reopened", "pull_request": { "merged": true } }),
        ),
        (
            "an event nobody handles, carrying a merged pull request",
            "issue_comment",
            json!({ "action": "closed", "pull_request": { "merged": true } }),
        ),
        (
            "a release, carrying a push to main",
            "release",
            json!({ "ref": "refs/heads/main", "repository": { "default_branch": "main" } }),
        ),
    ];

    for (what, event, payload) in nothing_to_do {
        let body = payload.to_string();
        let presented = sign(SECRET, &body);
        let (status, text) = hook(
            &state,
            GITEA_HOOK,
            &[(SIGNATURE_HEADER, &presented), (EVENT_HEADER, event)],
            &body,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{what}: {text}");
        assert!(text.is_empty(), "{what} answered a body: {text}");
    }

    assert!(
        touched(&gitea).await.is_empty(),
        "the forge was touched for an event that changes nothing",
    );
}

/// CC-08: with no `x-gitea-event` header the payload's own shape decides, and a header the Portal
/// cannot read as text is the same as none. The event name is read case-insensitively, so `PUSH` and
/// `Push` are the event `push` — worth pinning, because the branch a push carries decides whether the
/// mirror is read again.
#[tokio::test]
async fn the_event_header_is_read_case_insensitively_and_its_absence_reads_the_payload() {
    let gitea = MockServer::start().await;
    let state = portal_on(&gitea, Some(SECRET), None);

    // No syncer is configured, so an accepted event is a 202 that reads nothing: this case is about
    // which events are accepted, and `webhook_tests.rs` is about the sync one of them triggers.
    let main = json!({ "ref": "refs/heads/main", "repository": { "default_branch": "main" } });
    let merged = json!({ "action": "closed", "pull_request": { "merged": true } });

    let accepted: Vec<(&str, Option<&str>, &Value)> = vec![
        ("PUSH", Some("PUSH"), &main),
        ("Push", Some("Push"), &main),
        ("no header, a push payload", None, &main),
        ("no header, a merged pull request", None, &merged),
        ("PULL_REQUEST", Some("PULL_REQUEST"), &merged),
    ];
    for (what, event, payload) in accepted {
        let body = payload.to_string();
        let presented = sign(SECRET, &body);
        let mut headers = vec![(SIGNATURE_HEADER, presented.as_str())];
        if let Some(event) = event {
            headers.push((EVENT_HEADER, event));
        }
        let (status, text) = hook(&state, GITEA_HOOK, &headers, &body).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{what}: {text}");
    }

    // A payload that carries neither a ref nor a pull request, with no header: nothing to do.
    let body = json!({ "commits": [] }).to_string();
    let presented = sign(SECRET, &body);
    let (status, text) = hook(&state, GITEA_HOOK, &[(SIGNATURE_HEADER, &presented)], &body).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{text}");

    assert!(
        touched(&gitea).await.is_empty(),
        "the forge was touched although no syncer is configured",
    );
}
