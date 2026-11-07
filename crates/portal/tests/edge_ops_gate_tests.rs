//! Edge cases of the operation registry's gate (T-2113 – T-2118; AG-11, AG-59, AG-70, PF-50, PF-59).
//!
//! Six functions of `src/ops/mod.rs` decide, for every action the Portal offers, whether this caller
//! may take it. They are the only gate the assistant, MCP, the UI and the REST door share, so a hole
//! in one of them is a hole in all four surfaces at once. The contract each must hold:
//!
//! - `Via::touched_kind` (line 66) — four arrivals, four distinct words, and they are written into a
//!   draft's history, so they are a contract with the UI and with whoever reads an audit trail.
//! - `Caller::grants` (line 107) — the profile's half of an agent run: it only ever narrows, and a
//!   refusal names the operation asked for and nothing else of the profile or the person.
//! - `Caller::may_run` (line 123) — what no profile can grant (AG-11) is refused before the profile
//!   is consulted at all (AG-70), so the reason a person reads is the true one.
//! - `serde_error_path_and_message` (line 252) — covered in the module's own tests, because it is
//!   `pub(crate)`.
//! - `call` (line 277) — the order: the caller's half, then the person's half, then the input, then
//!   the body. An input is never even parsed for a caller who may not run the operation, and the
//!   body never runs for one the gate refused.
//! - `permitted` (line 295) — the person's half, exactly as the REST route of the same action
//!   decides it; a read of a project the caller cannot see is 404 and a write is 403 (PF-59, R20).
//!
//! Tests only (the family's rule). Every case is green; a red one becomes its own task. The cases
//! sweep `ops::registry()` rather than naming operations, so an operation added tomorrow is held to
//! the same contract without anybody remembering to add it here.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::IntoResponse;
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use jc_core::kinds::Verb;
use joinedcontext_portal::agents::access::Access;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::ops::{self, Caller, OpError, Via};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;

const PROJECT: &str = "ovzdusie";
const CSRF: &str = "test-csrf-token-ops-gate";
/// Names that exist in the mirror and may never appear in an answer to a caller with no binding.
const LEAK_CANARY: &str = "kanarik-ovzdusie";
const LEAK_CANARY_ELSEWHERE: &str = "kanarik-doprava";

/// The status a refusal reaches the caller with — the real answer, not a guess about the variant.
fn answered(err: OpError) -> StatusCode {
    err.into_response().status()
}

fn identity(username: &str, groups: &[&str]) -> Identity {
    Identity {
        client: None,
        subject: format!("f:1:{username}"),
        username: username.to_owned(),
        email: Some(format!("{username}@banskabystrica.sk")),
        name: Some(username.to_owned()),
        roles: Vec::new(),
        groups: groups.iter().map(|group| (*group).to_owned()).collect(),
        second_factor: false,
    }
}

fn org(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta::new(name, ORG_NAMESPACE),
        spec,
        status: None,
    }
}

/// A project whose only binding is a read-only one, and a second project nobody here is bound in.
fn state_with_bindings(config: Config) -> AppState {
    let state = AppState::new(config, None);
    state.mirror.upsert(org(
        "Role",
        "read-only-role",
        json!({ "rules": [{ "kinds": ["Endpoint", "ContextSpace"], "verbs": [] }] }),
    ));
    state.mirror.upsert(org(
        "RoleBinding",
        "viewer-binding",
        json!({
            "subjects": [{ "group": "city-viewers" }],
            "role": "read-only-role",
            "scope": { "project": PROJECT },
        }),
    ));
    // Two endpoints with names no answer to a stranger may carry: one in the project with a
    // read-only binding, one in a project nobody here is bound in. An operation that answers a
    // stranger with an empty list is fine; one that answers with these is a leak.
    for (name, project) in [(LEAK_CANARY, PROJECT), (LEAK_CANARY_ELSEWHERE, "doprava")] {
        state.mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: "Endpoint".to_owned(),
            metadata: ObjectMeta::new(name, project),
            spec: json!({
                "contextSpaceRef": project,
                "slug": "zt4qm7ge2xdv6ksb3ncf5arw2y",
                "audience": "internal",
                "enabledRepresentations": ["ngsi-ld"],
            }),
            status: None,
        });
    }
    // A steward of a different project: their binding must not reach this one (PF-50).
    state.mirror.upsert(org(
        "Role",
        "steward-role",
        json!({ "rules": [{ "kinds": ["Endpoint", "ContextSpace", "Pipeline", "Change"],
                            "verbs": ["read", "propose", "approve", "delete"] }] }),
    ));
    state.mirror.upsert(org(
        "RoleBinding",
        "other-project-binding",
        json!({
            "subjects": [{ "group": "other-stewards" }],
            "role": "steward-role",
            "scope": { "project": "doprava" },
        }),
    ));
    state
}

/// A profile that names every operation there is, with every verb on every kind it touches: the
/// widest access block an author could write. Nothing it names may widen the person who started the
/// run, and what AG-11 refuses it cannot grant either.
fn access_naming_everything() -> Access {
    let operations: Vec<&str> = ops::registry().iter().map(|op| op.name).collect();
    let kinds: Vec<Value> = ops::registry()
        .iter()
        .map(|op| json!({ "kind": op.kind, "verbs": ["read", "propose", "approve", "delete"] }))
        .collect();
    Access::from_spec(&json!({ "access": { "operations": operations, "kinds": kinds } }))
}

fn cookies(config: &Config, who: Identity) -> String {
    let now = session::now_unix();
    let session = Session {
        identity: who,
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: "id".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = session::store(PrivateCookieJar::new(config.cookie_key.clone()), &session)
        .expect("store the session");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| {
            value
                .to_str()
                .expect("cookie header")
                .split(';')
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    parts.push(format!("{CSRF_COOKIE}={CSRF}"));
    parts.join("; ")
}

// -------------------------------------------------------------------------------------------------
// T-2113 `Via::touched_kind`
// -------------------------------------------------------------------------------------------------

/// AG-61: the word a draft records for whoever last touched it: `person`, `mcp` or `run` (API/01
/// §17). A program with its own Keycloak token is its person's, like the session, since no key
/// stands for anyone any more (T-4616); the one a session writes is `person`
/// (`tests/drafts_tests.rs:88` reads it back over the API).
#[test]
fn the_ways_a_caller_arrives_are_three_words_a_reader_can_tell_apart() {
    let all = [
        (Via::Session, "person"),
        (Via::Bearer, "person"),
        (Via::Mcp, "mcp"),
        (Via::Agent, "run"),
    ];
    for (via, word) in all {
        assert_eq!(via.touched_kind(), word, "{via:?}");
        // The word is written into a draft and read back by the UI: nothing that needs escaping,
        // and nothing that could be read as a second value.
        assert!(
            word.chars().all(|c| c.is_ascii_lowercase()) && !word.is_empty(),
            "{via:?} records {word:?}",
        );
    }
    let words: std::collections::BTreeSet<&str> = all.iter().map(|(_, word)| *word).collect();
    assert_eq!(
        words.into_iter().collect::<Vec<_>>(),
        ["mcp", "person", "run"],
        "the words API/01 lists, and no other"
    );
    // An agent run is never recorded as the person who started it: an audit trail that cannot tell
    // the two apart is an audit trail that cannot answer who did it (AG-70).
    assert_ne!(Via::Agent.touched_kind(), Via::Session.touched_kind());
}

// -------------------------------------------------------------------------------------------------
// T-2114 `Caller::grants`
// -------------------------------------------------------------------------------------------------

/// AG-70: without an access block a profile grants the read-only operations and nothing else; with
/// one, only what it names. A person at a keyboard has no profile and is narrowed by nothing here.
#[test]
fn a_profile_narrows_and_a_caller_without_one_is_narrowed_by_nothing() {
    let person = Caller::new(identity("jana", &["city-viewers"]), Via::Session);
    let empty_profile = Caller::for_run(identity("jana", &["city-viewers"]), Access::default());
    let named = Access::from_spec(&json!({
        "access": { "operations": ["jc_catalog_search"],
                    "kinds": [{ "kind": "*", "verbs": ["read"] }] }
    }));
    let one_operation = Caller::for_run(identity("jana", &["city-viewers"]), named);

    for op in ops::registry() {
        assert!(
            person.grants(op).is_ok(),
            "{} was narrowed for a caller with no profile",
            op.name,
        );
        assert_eq!(
            empty_profile.grants(op).is_ok(),
            op.annotations.read_only_hint,
            "{} with no access block",
            op.name,
        );
        assert_eq!(
            one_operation.grants(op).is_ok(),
            op.name == "jc_catalog_search",
            "{} against a profile naming one operation",
            op.name,
        );
    }
}

/// AG-70, PF-59: the refusal is about the operation asked for. It carries neither the other
/// operations the profile names nor anything of the person behind the run — a run's author reads it
/// in a log, and a profile is not a list to be enumerated through refusals.
#[test]
fn the_profiles_refusal_names_the_operation_and_nothing_of_the_person_or_the_profile() {
    let secret_looking = "jc_run_answer";
    let named = Access::from_spec(&json!({
        "access": { "operations": [secret_looking, "jc_catalog_search"],
                    "kinds": [{ "kind": "*", "verbs": ["read"] }] }
    }));
    let caller = Caller::for_run(
        identity("jana.tajna", &["city-viewers", "secret-group"]),
        named,
    );
    let refused = ops::find("jc_endpoint_propose").expect("a registered proposal");
    let message = match caller.grants(refused) {
        Err(OpError::Api(err)) => format!("{err:?} {err}"),
        other => panic!("a profile that does not name the operation allowed it: {other:?}"),
    };

    assert!(
        message.contains(refused.name),
        "the refusal did not name the operation: {message}",
    );
    for leaked in [
        secret_looking,
        "jc_catalog_search",
        "jana.tajna",
        "secret-group",
        "banskabystrica.sk",
        "f:1:",
    ] {
        assert!(
            !message.contains(leaked),
            "the refusal carried {leaked:?}: {message}",
        );
    }
}

// -------------------------------------------------------------------------------------------------
// T-2115 `Caller::may_run`
// -------------------------------------------------------------------------------------------------

/// AG-11 before AG-70: a change is decided by a person. A profile that names every operation and
/// grants `approve` on every kind still cannot let a run approve or reject one, and the reason the
/// author reads is that no profile can grant it — not that this profile happens not to.
#[test]
fn no_profile_however_wide_lets_a_run_decide_a_change_or_move_a_workspace() {
    let wide = access_naming_everything();
    let run = Caller::for_run(identity("jana", &["city-viewers"]), wide.clone());
    let person = Caller::new(identity("jana", &["city-viewers"]), Via::Session);

    let mut decisions = 0;
    for op in ops::registry() {
        let is_decision = op.kind == "Change" && op.verb.is_some();
        let is_workspace_move = matches!(op.name, "jc_workspace_propose" | "jc_workspace_discard");
        if !is_decision && !is_workspace_move {
            continue;
        }
        decisions += 1;
        // Two independent locks. `Access::names` refuses an approval or a deletion whatever the
        // block says (`src/agents/access.rs:74`), so the widest profile writable cannot even name
        // one; where it can name the operation, AG-11 still refuses it below.
        if matches!(op.verb, Some(Verb::Approve) | Some(Verb::Delete)) {
            assert!(
                !wide.names(op),
                "a profile naming every operation was able to claim {}",
                op.name,
            );
        }
        let message = match run.may_run(op) {
            Err(err) => err.to_string(),
            Ok(()) => panic!("a run was allowed to take {}", op.name),
        };
        assert!(
            !message.contains("agent profile does not grant"),
            "{} was refused with the profile's reason, which explains nothing here: {message}",
            op.name,
        );
        let _ = &wide;
        // The same operation is the person's to take: this is a rule about runs, not a dead route.
        assert!(
            person.may_run(op).is_ok(),
            "{} is refused even for a person at a keyboard",
            op.name,
        );
    }
    assert!(
        decisions >= 3,
        "the registry no longer holds the operations this rule is about ({decisions} found)",
    );
}

/// AG-03: an agent has no ambient rights. The widest profile an author can write, run for a
/// viewer, takes no operation that carries a verb, because every call is decided on the bindings
/// of the person who started the run; the same profile run for a steward of the project passes the
/// gate for the proposal the steward could make themselves, and for nothing a steward cannot do.
#[tokio::test]
async fn an_agent_holds_the_rights_of_the_person_it_runs_for_and_nothing_more() {
    let state = state_with_bindings(Config::for_tests());
    state.mirror.upsert(org(
        "RoleBinding",
        "steward-binding",
        json!({
            "subjects": [{ "group": "city-stewards" }],
            "role": "steward-role",
            "scope": { "project": PROJECT },
        }),
    ));
    let for_viewer = Caller::for_run(
        identity("jana", &["city-viewers"]),
        access_naming_everything(),
    );
    let for_steward = Caller::for_run(
        identity("stela", &["city-stewards"]),
        access_naming_everything(),
    );

    let mut refused = 0;
    for op in ops::registry().iter().filter(|op| op.verb.is_some()) {
        let answer = ops::call(op, &for_viewer, &state, PROJECT, json!({})).await;
        match answer {
            Err(err) => assert!(
                matches!(answered(err), StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
                "{} was refused to a viewer's run for a reason other than who they are",
                op.name,
            ),
            Ok(_) => panic!("{} ran for a viewer's run with the widest profile", op.name),
        }
        refused += 1;
    }
    assert!(refused > 0, "the registry holds no operation with a verb");

    // The happy path: the steward's run passes both halves for a proposal on a kind their role
    // grants, which is the same gate the steward meets at the keyboard.
    let propose = ops::find("jc_endpoint_propose").expect("registered");
    assert!(for_steward.may_run(propose).is_ok());
    assert!(ops::permitted(propose, &for_steward.identity, &state, PROJECT).is_ok());
}

/// AG-59: `may_run` is the caller's half only. It answers on the caller and the operation alone, so
/// it may not be read as permission — a person with no binding anywhere passes it for every
/// operation, and the person's half (`permitted`) is what refuses them. The one exception is a
/// bearer token at a Change's decision, which no grant lifts (AG-133).
#[test]
fn the_callers_half_says_nothing_about_what_the_person_may_do() {
    for via in [Via::Session, Via::Bearer] {
        let nobody = Caller::new(identity("nobody", &[]), via);
        for op in ops::registry() {
            let decides = op.kind == "Change" && op.verb.is_some();
            assert_eq!(
                nobody.may_run(op).is_ok(),
                via == Via::Session || !decides,
                "{} {via:?}: the caller's half answered on more than the caller",
                op.name,
            );
        }
    }
}

// -------------------------------------------------------------------------------------------------
// T-2118 `permitted`
// -------------------------------------------------------------------------------------------------

/// The one operation that passes the gate by name without a route of its own (ADR-N-067, T-6413):
/// a Policy can share an Endpoint with a person who holds no right in its project, so only the
/// Endpoint's Policy can decide. A stranger reads what an anonymous caller reads and nothing else:
/// [`a_stranger_learns_nothing_from_the_data_read`].
const DATA_QUERY: &str = joinedcontext_portal::ops::data_query::NAME;

/// ADR-N-067: the data read needs no binding, so its own answer is the gate. Naming an internal
/// Endpoint without a Policy that shares it reads as naming one that is not there.
#[tokio::test]
async fn a_stranger_learns_nothing_from_the_data_read() {
    let state = state_with_bindings(Config::for_tests());
    let stranger = Caller::new(identity("nobody", &[]), Via::Session);
    let op = ops::find(DATA_QUERY).expect("registered");
    let read = |name: &str| json!({ "endpoint": name, "type": "Vehicle" });

    let there = ops::call(op, &stranger, &state, PROJECT, read(LEAK_CANARY))
        .await
        .expect_err("a stranger read an internal Endpoint");
    let absent = ops::call(op, &stranger, &state, PROJECT, read("no-such-endpoint"))
        .await
        .expect_err("a stranger read an Endpoint that is not there");
    assert_eq!(
        there.to_string().replace(LEAK_CANARY, "x"),
        absent.to_string().replace("no-such-endpoint", "x"),
    );
    assert_eq!(answered(there), StatusCode::NOT_FOUND);
    assert_eq!(answered(absent), StatusCode::NOT_FOUND);
}

/// PF-50, PF-59: not one operation of the registry is open to a caller with no binding in the
/// project. A read answers as a project that is not there, a write says what is missing, and the
/// only operations that pass here are the ones whose own route function checks instead — that list
/// is written out, so adding a name to it is a visible decision rather than a quiet exemption.
#[test]
fn not_one_operation_is_open_to_a_caller_with_no_binding() {
    let state = state_with_bindings(Config::for_tests());
    let nobody = identity("nobody", &[]);
    let waved_through = joinedcontext_portal::ops::resources::CHECKED_BY_THE_ROUTE;

    for op in ops::registry() {
        match ops::permitted(op, &nobody, &state, PROJECT) {
            Ok(()) => assert!(
                waved_through.contains(&op.name) || op.name == DATA_QUERY,
                "{} is open to a caller with no binding and is not checked by its route",
                op.name,
            ),
            Err(err) => {
                let status = answered(err);
                assert!(
                    status == StatusCode::NOT_FOUND || status == StatusCode::FORBIDDEN,
                    "{} refused a stranger with {status}",
                    op.name,
                );
                // A read must not confirm the project exists; a write may name what it needs.
                if op.verb.is_none() && op.annotations.read_only_hint {
                    assert_eq!(
                        status,
                        StatusCode::NOT_FOUND,
                        "{} told a stranger the project is there",
                        op.name,
                    );
                }
            }
        }
    }
}

/// PF-50: the list of operations the registry waves through is only safe while every name on it is a
/// verbless operation that really exists. A name with a verb is dead (the arm above decides it) and
/// a name of no operation is rot — either way the reader of the list is being told something untrue
/// about where the check happens.
#[test]
fn every_operation_the_registry_waves_through_exists_and_is_checked_somewhere_else() {
    let waved_through = joinedcontext_portal::ops::resources::CHECKED_BY_THE_ROUTE;
    let mut seen = std::collections::BTreeSet::new();
    for name in waved_through {
        assert!(seen.insert(*name), "{name} is listed twice");
        let op = ops::find(name).unwrap_or_else(|| panic!("{name} is not a registered operation"));
        assert!(
            op.verb.is_none(),
            "{name} carries a verb, so this entry never decides anything and reads as if it did",
        );
    }
}

/// PF-50: a binding is scoped to a project. A steward of `doprava` is a stranger in `ovzdusie`, and
/// a viewer of `ovzdusie` may take no operation that carries a verb.
#[test]
fn a_binding_of_one_project_does_not_reach_another_and_a_viewer_writes_nothing() {
    let state = state_with_bindings(Config::for_tests());
    let elsewhere = identity("stewart", &["other-stewards"]);
    let viewer = identity("jana", &["city-viewers"]);
    let waved_through = joinedcontext_portal::ops::resources::CHECKED_BY_THE_ROUTE;

    for op in ops::registry() {
        if !waved_through.contains(&op.name) && op.name != DATA_QUERY {
            assert!(
                ops::permitted(op, &elsewhere, &state, PROJECT).is_err(),
                "{} let a steward of another project in",
                op.name,
            );
        }
        // The viewer's role grants no verb at all, so every operation that names one is refused;
        // the verbless ones are reads of a project they may see.
        let decided = ops::permitted(op, &viewer, &state, PROJECT);
        match op.verb {
            Some(_) => assert!(
                decided.is_err(),
                "{} ({:?} on {}) was open to a viewer with no verb granted",
                op.name,
                op.verb,
                op.kind,
            ),
            None => assert!(
                decided.is_ok(),
                "{} was refused to a viewer of the project: {:?}",
                op.name,
                decided.err().map(|err| err.to_string()),
            ),
        }
    }
    // And the same viewer is refused a proposal on the kind their role does name, because the role
    // grants no verbs on it: a kind in a rule is not a permission by itself.
    let propose = ops::find("jc_endpoint_propose").expect("registered");
    assert_eq!(propose.verb, Some(Verb::Propose));
    assert!(ops::permitted(propose, &viewer, &state, PROJECT).is_err());
}

// -------------------------------------------------------------------------------------------------
// T-2117 `call`
// -------------------------------------------------------------------------------------------------

/// AG-59, PF-50: the order is the caller's half, the person's half, the input, then the body. A
/// stranger sending nonsense is refused for who they are, not told what the schema wants: an
/// unauthorised caller must not be able to use validation messages as a map of the operation.
#[tokio::test]
async fn the_gate_answers_before_the_input_is_looked_at() {
    let state = state_with_bindings(Config::for_tests());
    let stranger = Caller::new(identity("nobody", &[]), Via::Session);
    let waved_through = joinedcontext_portal::ops::resources::CHECKED_BY_THE_ROUTE;

    for op in ops::registry() {
        if waved_through.contains(&op.name) || op.name == DATA_QUERY {
            continue; // Their own route function decides; `call` is not their gate.
        }
        for nonsense in [
            json!({ "quiteUnknownField": "x" }),
            json!("not even an object"),
            Value::Null,
        ] {
            let err = ops::call(op, &stranger, &state, PROJECT, nonsense.clone())
                .await
                .expect_err("a stranger ran an operation");
            assert!(
                !matches!(err, OpError::InvalidInput { .. }),
                "{} told a stranger about its input instead of refusing them: {err}",
                op.name,
            );
            let status = answered(err);
            assert!(
                status == StatusCode::NOT_FOUND || status == StatusCode::FORBIDDEN,
                "{} answered a stranger with {status}",
                op.name,
            );
        }
    }
}

/// AG-59: a refused call never reaches the operation's body. This installation has no forge, so a
/// proposal that ran would fail trying to reach one — a `500` here would mean the gate let it in.
#[tokio::test]
async fn a_refused_call_never_reaches_the_body_of_the_operation() {
    let state = state_with_bindings(Config::for_tests());
    let viewer = Caller::new(identity("jana", &["city-viewers"]), Via::Session);
    let propose = ops::find("jc_endpoint_propose").expect("registered");
    let input = json!({
        "name": "air",
        "spec": { "contextSpaceRef": PROJECT, "audience": "public",
                  "enabledRepresentations": ["ngsi-ld"] },
    });

    let err = ops::call(propose, &viewer, &state, PROJECT, input)
        .await
        .expect_err("a viewer proposed an endpoint");
    let status = answered(err);
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the proposal got past the gate and answered {status}",
    );
}

/// PF-50, PF-59, PF-60, AG-59: the same rule through the door anyone can knock on. Every operation
/// the registry waves through is posted by a caller with no binding anywhere. A refusal (403/404) or
/// a refusal of the input (422) is fine; a `500` would mean the check came after something was
/// already attempted. A `200` is allowed only where the route filters manifest by manifest and
/// answers an empty list — so the answer itself is searched for the two endpoints that do exist.
#[tokio::test]
async fn the_operations_checked_by_their_route_tell_a_stranger_nothing() {
    // The cookie is signed with the key of the config the state holds, or the session is no
    // session and every answer would be a 401 that proves nothing about the gate.
    let config = Config::for_tests();
    let app = server::app(state_with_bindings(config.clone()));
    let stranger = cookies(&config, identity("nobody", &[]));

    for name in joinedcontext_portal::ops::resources::CHECKED_BY_THE_ROUTE {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/projects/{PROJECT}/ops/{name}"))
                    .header(header::COOKIE, stranger.clone())
                    .header(CSRF_HEADER, CSRF)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(b"{}".to_vec()))
                    .expect("request"),
            )
            .await
            .expect("response");
        let status = response.status();
        assert!(
            status == StatusCode::OK
                || status == StatusCode::NOT_FOUND
                || status == StatusCode::FORBIDDEN
                || status == StatusCode::UNPROCESSABLE_ENTITY,
            "{name} answered a stranger with {status}",
        );
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let text = String::from_utf8_lossy(&body).into_owned();
        for canary in [LEAK_CANARY, LEAK_CANARY_ELSEWHERE] {
            assert!(
                !text.contains(canary),
                "{name} answered a stranger with {canary}: {text}",
            );
        }
        if status == StatusCode::OK {
            // What a stranger may be answered is nothing: an empty collection, not a filtered one
            // that still counts what it left out.
            let answered: Value = serde_json::from_slice(&body).expect("json answer");
            let emptied = match &answered {
                Value::Array(items) => items.is_empty(),
                Value::Object(fields) => fields.values().all(|value| match value {
                    Value::Array(items) => items.is_empty(),
                    Value::Number(number) => number.as_i64() == Some(0),
                    _ => true,
                }),
                _ => false,
            };
            assert!(
                emptied,
                "{name} answered a stranger with {status} and something in it: {answered}",
            );
        }
    }
}
