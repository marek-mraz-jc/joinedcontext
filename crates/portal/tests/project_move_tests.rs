//! A project moved to another Organization by its one `organizationRef` Change (T-6852,
//! GOAL-multiorg target 1, TEN-06): from the next request on it is the new Organization's alone,
//! its Pipelines' next render mints the new owner segment (ADR-N-068), and moving it back restores
//! both. The move is one Project manifest in the mirror, as the reconciler applies the Change.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::doors::http;
use common::envelope;
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::reconciler::streams::{Bentos, StreamDeployer, StreamOutcome};
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

/// The project that moves, `bbsk`, held by `hel` until it moves to an Organization of its own.
const MOVED: &str = "bbsk";

fn someone(name: &str) -> Identity {
    Identity {
        client: None,
        subject: format!("sub-{name}"),
        username: name.to_owned(),
        email: Some(format!("{name}@example.org")),
        name: Some(name.to_owned()),
        roles: Vec::new(),
        groups: Vec::new(),
        second_factor: false,
    }
}

/// org-admin of `hel`, which holds no installation flag.
fn hel_admin() -> Identity {
    someone("heladmin")
}

/// org-admin of `bbsk`.
fn bbsk_admin() -> Identity {
    someone("bbadmin")
}

/// Moves `MOVED` to `organization`: its Project manifest as the applied Change leaves it.
fn move_to(mirror: &Mirror, organization: &str) {
    mirror.upsert(envelope(
        "Project",
        MOVED,
        ORG_NAMESPACE,
        json!({ "organizationRef": organization }),
    ));
}

/// The installation's `ops` of operators, `hel` (hel.fi) holding `bbsk` and `helsinki`, and the
/// empty `bbsk` (bbsk.sk) it moves to; one org-admin each, and a Pipeline of `bbsk` minting
/// `urn:ngsi-ld:X:{JC_ORG_ID}:...`.
fn world() -> AppState {
    let state = AppState::new(Config::for_tests(), None).with_mirror(Arc::new(Mirror::new()));
    let m = &state.mirror;
    for (name, spec) in [
        (
            "ops",
            json!({ "domain": "ops.example", "installation": true }),
        ),
        ("hel", json!({ "domain": "hel.fi" })),
        ("bbsk", json!({ "domain": "bbsk.sk" })),
    ] {
        m.upsert(envelope("Organization", name, ORG_NAMESPACE, spec));
    }
    m.upsert(envelope(
        "Project",
        "helsinki",
        ORG_NAMESPACE,
        json!({ "organizationRef": "hel" }),
    ));
    move_to(m, "hel");
    let kinds: Vec<&str> = joinedcontext_portal::resource::kinds()
        .map(|info| info.kind)
        .collect();
    let every =
        json!({ "rules": [{ "kinds": kinds, "verbs": ["read", "propose", "approve", "delete"] }] });
    m.upsert(envelope("Role", "org-admin", ORG_NAMESPACE, every));
    for (binding, user, organization) in [
        ("hel-admins", "heladmin", "hel"),
        ("bbsk-admins", "bbadmin", "bbsk"),
    ] {
        m.upsert(envelope(
            "RoleBinding",
            binding,
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": format!("{user}@example.org") }],
                    "role": "org-admin", "scope": { "organization": organization } }),
        ));
    }
    m.upsert(envelope(
        "DataSource",
        "senzory",
        MOVED,
        json!({ "type": "websocket", "webSocket": { "url": "wss://example.org/status" } }),
    ));
    m.upsert(envelope(
        "Endpoint",
        "bbsk-all",
        MOVED,
        json!({ "contextSpaceRef": "bbsk", "slug": "bbsk23456789012345678901234",
                "audience": "public", "enabledRepresentations": ["ngsi-ld"] }),
    ));
    m.upsert(envelope(
        "Pipeline",
        "senzory",
        MOVED,
        json!({
            "class": "resident",
            "source": { "dataSourceRef": { "kind": "DataSource", "name": "senzory" } },
            "compute": { "kind": "bloblang", "bloblang":
                "root.id = \"urn:ngsi-ld:X:\" + env(\"JC_ORG_ID\") + \":\" + env(\"JC_SPACE\") + \":\" + this.id" },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:bbsk.sk:bbsk:bbsk-all"
        }),
    ));
    state
}

/// `GET /api/v1/projects/bbsk` as `who`: the status.
async fn project_as(state: &AppState, who: Identity) -> StatusCode {
    http(
        state,
        who,
        "GET",
        &format!("/api/v1/projects/{MOVED}"),
        None,
    )
    .await
    .status
}

fn refused(status: StatusCode) -> bool {
    status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND
}

/// What `bbsk`'s stream sends the runner after one converge over `mirror`: the PUT bodies.
async fn rendered(mirror: &Mirror) -> Vec<String> {
    let runner = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex("/resources/rate_limit/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&runner)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!("/streams/{MOVED}.senzory")))
        .respond_with(ResponseTemplate::new(200))
        .mount(&runner)
        .await;
    let outcomes = StreamDeployer::new(runner.uri())
        .converge(mirror, &Bentos::new(), &Default::default())
        .await;
    assert!(
        outcomes
            .iter()
            .any(|(_, name, outcome)| name == "senzory" && *outcome == StreamOutcome::Live),
        "{outcomes:?}"
    );
    runner
        .received_requests()
        .await
        .expect("recorded")
        .into_iter()
        .filter(|request| request.method.as_str() == "PUT")
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .collect()
}

/// The owner segment `bbsk`'s ids mint, read from the rendered mapping.
async fn minted_owner(mirror: &Mirror) -> Value {
    let bodies = rendered(mirror).await;
    let body = bodies.first().expect("the stream was PUT");
    assert!(
        !body.contains("JC_ORG_ID"),
        "the runner's owner is read: {body}"
    );
    match (body.contains("sk_bbsk"), body.contains("fi_hel")) {
        (true, false) => json!("sk_bbsk"),
        (false, true) => json!("fi_hel"),
        _ => json!(body),
    }
}

/// TEN-06, T-6852: once `bbsk` moves from `hel` to `bbsk`, `hel`'s administrator is refused on its
/// project routes and `bbsk`'s administrator is admitted; before the move it was the other way.
#[tokio::test]
async fn a_moved_project_belongs_to_its_new_organization_only() {
    let state = world();
    assert_eq!(project_as(&state, hel_admin()).await, StatusCode::OK);
    assert!(refused(project_as(&state, bbsk_admin()).await));
    move_to(&state.mirror, "bbsk");
    let hel = project_as(&state, hel_admin()).await;
    assert!(refused(hel), "hel's admin after the move: {hel}");
    assert_eq!(project_as(&state, bbsk_admin()).await, StatusCode::OK);
}

/// ADR-N-068, T-6852: a Pipeline mints the owner segment of its project's Organization, so a moved
/// project's next render names the new owner and no `urn_owner_mismatched` write follows the
/// re-mint. One runner serves every Organization: its own `JC_ORG_ID` is the installation's.
#[tokio::test]
async fn pipelines_of_a_moved_project_mint_the_new_owner_segment() {
    let state = world();
    assert_eq!(minted_owner(&state.mirror).await, json!("fi_hel"));
    move_to(&state.mirror, "bbsk");
    assert_eq!(minted_owner(&state.mirror).await, json!("sk_bbsk"));
}

/// T-6852: the rollback is the move back, the same one Change: `hel` holds `bbsk` again, its
/// Pipeline mints `fi_hel` again, and `bbsk`'s administrator is refused again.
#[tokio::test]
async fn moving_back_restores_the_project() {
    let state = world();
    move_to(&state.mirror, "bbsk");
    assert_eq!(project_as(&state, bbsk_admin()).await, StatusCode::OK);
    move_to(&state.mirror, "hel");
    assert_eq!(project_as(&state, hel_admin()).await, StatusCode::OK);
    assert!(refused(project_as(&state, bbsk_admin()).await));
    assert_eq!(minted_owner(&state.mirror).await, json!("fi_hel"));
}
