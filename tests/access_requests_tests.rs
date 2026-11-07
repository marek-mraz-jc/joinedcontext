//! API/01 §47, PL-90…PL-93, T-4863: a pipeline asks another project's space for access, an owner
//! there approves it as a Change of their own project, and revokes it later. Runs when
//! `JC_PORTAL_TEST_DATABASE_URL` points at a PostgreSQL the test may write to; otherwise it says
//! so and returns.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{json, Value};

use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

const KPIS: &str = "city-kpis";
const HELSINKI: &str = "helsinki";

fn unique(base: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or_default();
    format!("{base}-{nanos:x}")
}

fn role(state: &AppState, name: &str, user: &str, project: &str, kinds: &[&str], verbs: &[&str]) {
    state.mirror.upsert(common::envelope(
        "Role",
        name,
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": kinds, "verbs": verbs }] }),
    ));
    state.mirror.upsert(common::envelope(
        "RoleBinding",
        &format!("{name}-{user}"),
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": format!("{user}@hel.fi") }], "role": name, "scope": { "project": project } }),
    ));
}

/// `jana` stewards the KPI pipeline; `olli` owns Helsinki's air space; `eve` reads Helsinki only.
async fn state(pool: sqlx::PgPool, pipeline: &str, pipeline_identity: bool) -> AppState {
    world(pipeline, pipeline_identity).await.with_db(pool)
}

/// The same world without a database: the request form's list and the request door's refusals
/// answer before anything is stored.
/// Each world's forge double, by its pipeline's name, kept for the test's whole run.
static FORGES: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, wiremock::MockServer>>,
> = std::sync::LazyLock::new(Default::default);

async fn world(pipeline: &str, pipeline_identity: bool) -> AppState {
    let gitea = common::forge().await;
    let client = GiteaClient::new(
        gitea.uri().parse().expect("mock url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
    FORGES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(pipeline.to_owned(), gitea);
    let mut config = Config::for_tests();
    config.pipeline_identity = pipeline_identity;
    let state = AppState::new(config, None).with_gitea(Arc::new(client));
    for (project, space) in [(KPIS, "kpi"), (HELSINKI, "air")] {
        state.mirror.upsert(common::envelope(
            "Project",
            project,
            ORG_NAMESPACE,
            json!({}),
        ));
        state.mirror.upsert(common::envelope(
            "ContextSpace",
            space,
            project,
            json!({ "broker": "antares" }),
        ));
    }
    state.mirror.upsert(common::envelope(
        "Pipeline",
        pipeline,
        KPIS,
        json!({ "contextSpaceRef": "kpi", "class": "scheduled", "schedule": "*/15 * * * *" }),
    ));
    state.mirror.upsert(common::envelope(
        "Endpoint",
        "air-quality",
        HELSINKI,
        json!({ "contextSpaceRef": "air", "slug": "zt4qm7ge2xdv6ksb3ncf5arw2y", "audience": "project-list", "allowedProjects": ["traffic"] }),
    ));
    role(
        &state,
        "kpi-steward",
        "jana",
        KPIS,
        &["Pipeline"],
        &["read", "propose"],
    );
    let owner = ["ContextSpace", "Endpoint", "Policy", "Group"];
    let verbs = ["read", "propose", "approve"];
    role(&state, "air-owner", "olli", HELSINKI, &owner, &verbs);
    role(
        &state,
        "air-reader",
        "eve",
        HELSINKI,
        &["ContextSpace", "Endpoint"],
        &["read"],
    );
    state
}

/// Every test approves as `olli` in Helsinki, and an import's check is one draft per person and
/// project in the shared database: one test's import dropped the check of another's.
static DATABASE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn pool() -> Option<sqlx::PgPool> {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|u| !u.trim().is_empty());
    let Some(url) = url else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return None;
    };
    Some(
        joinedcontext_portal::db::connect(&url)
            .await
            .expect("the test database answers and migrates"),
    )
}

fn ask() -> Value {
    json!({
        "endpoint": { "project": HELSINKI, "name": "air-quality" },
        "operations": "read",
        "types": [{ "type": "AirQualityObserved", "attrs": ["pm10", "pm25"] }],
        "until": "2099-01-31T23:59:59Z",
        "reason": "air index for the city dashboard",
    })
}

async fn send(
    state: &AppState,
    who: &str,
    verb: &str,
    uri: &str,
    body: Option<Value>,
) -> common::Answer {
    let mut person = common::person(who);
    person.subject = id_of(who);
    common::send(state, person, verb, uri, body).await
}

/// The Keycloak user id (a UUID, ADR-N-061) the test gives the person signed in as `who`.
fn id_of(who: &str) -> String {
    let hex: String = who.bytes().take(6).map(|b| format!("{b:02x}")).collect();
    format!("00000000-0000-4000-8000-{hex:0>12}")
}

/// PL-90, PL-91, PL-93: request, approve as a Change of Helsinki, revoke; nothing is granted
/// before the approval and the requester never reads Helsinki's spaces through the request.
#[tokio::test]
async fn a_request_is_approved_in_the_owning_project_and_revoked_there() {
    let _held = DATABASE.lock().await;
    let Some(pool) = pool().await else { return };
    let pipeline = unique("air");
    let state = state(pool, &pipeline, true).await;
    let filed = send(
        &state,
        "jana",
        "POST",
        &format!("/api/v1/projects/{KPIS}/pipelines/{pipeline}/access-requests"),
        Some(ask()),
    )
    .await;
    assert_eq!(filed.status, StatusCode::CREATED, "{}", filed.text);
    let request: Value = serde_json::from_str(&filed.text).expect("json");
    let id = request["id"].as_str().expect("an id").to_owned();
    assert_eq!(request["state"], "pending");
    assert_eq!(request["account"], format!("{KPIS}/pl-{pipeline}"));
    // T-4946: jana cannot read Helsinki, so her answer does not say which space the name reaches.
    assert!(request.get("space").is_none(), "{}", filed.text);
    assert!(
        state
            .mirror
            .list(HELSINKI, "Policy", &Default::default())
            .items
            .is_empty(),
        "a request grants nothing on its own"
    );

    // The same pipeline asking again for the same Endpoint is the open request, not a second one.
    let again = send(
        &state,
        "jana",
        "POST",
        &format!("/api/v1/projects/{KPIS}/pipelines/{pipeline}/access-requests"),
        Some(ask()),
    )
    .await;
    assert_eq!(again.status, StatusCode::CONFLICT, "{}", again.text);

    // A reader of Helsinki who owns no space there sees no request and decides none.
    let inbox = send(
        &state,
        "eve",
        "GET",
        &format!("/api/v1/projects/{HELSINKI}/access-requests"),
        None,
    )
    .await;
    assert_eq!(inbox.status, StatusCode::OK, "{}", inbox.text);
    assert!(!inbox.text.contains(&id), "{}", inbox.text);
    let refused = send(
        &state,
        "eve",
        "POST",
        &format!("/api/v1/projects/{HELSINKI}/access-requests/{id}/approve"),
        None,
    )
    .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text);
    // The requester cannot open Helsinki's inbox at all.
    let theirs = send(
        &state,
        "jana",
        "GET",
        &format!("/api/v1/projects/{HELSINKI}/access-requests"),
        None,
    )
    .await;
    assert_eq!(theirs.status, StatusCode::NOT_FOUND, "{}", theirs.text);

    let inbox = send(
        &state,
        "olli",
        "GET",
        &format!("/api/v1/projects/{HELSINKI}/access-requests"),
        None,
    )
    .await;
    assert!(inbox.text.contains(&id), "{}", inbox.text);
    let approved = send(
        &state,
        "olli",
        "POST",
        &format!("/api/v1/projects/{HELSINKI}/access-requests/{id}/approve"),
        None,
    )
    .await;
    assert_eq!(approved.status, StatusCode::ACCEPTED, "{}", approved.text);
    let change: Value = serde_json::from_str(&approved.text).expect("json");
    assert_eq!(change["kind"], "Change", "{}", approved.text);

    let mine = send(
        &state,
        "jana",
        "GET",
        &format!("/api/v1/projects/{KPIS}/pipelines/{pipeline}/access-requests"),
        None,
    )
    .await;
    let mine: Value = serde_json::from_str(&mine.text).expect("json");
    assert_eq!(mine["items"][0]["state"], "approved");
    assert_eq!(mine["items"][0]["decidedBy"], "olli");
    assert_eq!(mine["items"][0]["change"], change["metadata"]["name"]);

    // The grant merged: the revoke ends it now.
    let request: joinedcontext_portal::access_requests::AccessRequest =
        joinedcontext_portal::access_requests::get(state.db.as_ref().expect("db"), HELSINKI, &id)
            .await
            .expect("read")
            .expect("there");
    let endpoint =
        json!({ "spec": { "audience": "project-list", "allowedProjects": ["traffic"] } });
    let policy =
        joinedcontext_portal::access_requests::grant_bundle(&request, &endpoint)[1].clone();
    state
        .mirror
        .upsert(serde_json::from_value(policy).expect("an envelope"));
    let no_reason = send(
        &state,
        "olli",
        "POST",
        &format!("/api/v1/projects/{HELSINKI}/access-requests/{id}/revoke"),
        Some(json!({ "reason": " " })),
    )
    .await;
    assert_eq!(
        no_reason.status,
        StatusCode::BAD_REQUEST,
        "{}",
        no_reason.text
    );
    let revoked = send(
        &state,
        "olli",
        "POST",
        &format!("/api/v1/projects/{HELSINKI}/access-requests/{id}/revoke"),
        Some(json!({ "reason": "the index moved to another source" })),
    )
    .await;
    assert_eq!(revoked.status, StatusCode::ACCEPTED, "{}", revoked.text);
    let mine = send(
        &state,
        "jana",
        "GET",
        &format!("/api/v1/projects/{KPIS}/pipelines/{pipeline}/access-requests"),
        None,
    )
    .await;
    assert!(
        mine.text.contains("\"revoked\"") && mine.text.contains("the index moved"),
        "{}",
        mine.text
    );

    // PL-93 (T-4882): the revoke merged, so the gateway answers the pipeline's reads 403; the run
    // log names who revoked it, and three such runs in a row pause the pipeline.
    let pool = state.db.as_ref().expect("db");
    let requests = joinedcontext_portal::access_requests::outgoing(pool, KPIS, &pipeline)
        .await
        .expect("read");
    let now = time::OffsetDateTime::now_utc();
    let reason = joinedcontext_portal::access_requests::refusal_reason(&requests, now)
        .expect("a revoked grant refuses");
    assert_eq!(reason, format!("access revoked by olli on {}", now.date()));
    let key = (KPIS.to_owned(), pipeline.clone());
    let log = joinedcontext_portal::pipeline_log::LogStore::new(Some(pool.clone()));
    let line = || joinedcontext_portal::pipeline_log::NewLine {
        record_id: String::new(),
        step: None,
        outcome: joinedcontext_portal::pipeline_log::Outcome::Failed,
        message: format!(
            "{reason}; the gateway answered: HTTP request returned unexpected response code (403)"
        ),
    };
    for (n, run) in ["2026-10-22T01:00:00Z", "2026-10-22T02:00:00Z"]
        .iter()
        .enumerate()
    {
        log.append(KPIS, &pipeline, run, &[line()])
            .await
            .expect("append");
        let streaks = log.refused_streaks(3).await.expect("read");
        assert!(!streaks.contains_key(&key), "{n}: not three yet");
    }
    log.append(KPIS, &pipeline, "2026-10-22T03:00:00Z", &[line()])
        .await
        .expect("append");
    let streaks = log.refused_streaks(3).await.expect("read");
    assert!(
        streaks
            .get(&key)
            .is_some_and(|words| words.starts_with(&reason)),
        "{streaks:?}"
    );
    // The ended Policy grants nothing now; until the revoke merges the old one still does.
    assert!(joinedcontext_portal::access_requests::grant_in_force(
        &state.mirror,
        KPIS,
        &pipeline,
        now
    ));
    let ended = joinedcontext_portal::access_requests::revoke_bundle(
        &request,
        &endpoint,
        &[serde_json::to_value(
            state
                .mirror
                .get(HELSINKI, "Policy", &request.policy_name())
                .expect("the grant"),
        )
        .expect("json")],
        now - time::Duration::seconds(1),
    );
    let ended = ended.last().cloned().expect("the ended Policy");
    state
        .mirror
        .upsert(serde_json::from_value(ended).expect("an envelope"));
    assert!(!joinedcontext_portal::access_requests::grant_in_force(
        &state.mirror,
        KPIS,
        &pipeline,
        now
    ));
}

/// F-POR1-017: two owners approving at once: one Change, and the other gets the 409 before it
/// proposes anything.
#[tokio::test]
async fn two_approvals_at_once_open_one_change() {
    let _held = DATABASE.lock().await;
    let Some(pool) = pool().await else { return };
    let pipeline = unique("air");
    let state = state(pool, &pipeline, true).await;
    let filed = send(
        &state,
        "jana",
        "POST",
        &format!("/api/v1/projects/{KPIS}/pipelines/{pipeline}/access-requests"),
        Some(ask()),
    )
    .await;
    assert_eq!(filed.status, StatusCode::CREATED, "{}", filed.text);
    let id = serde_json::from_str::<Value>(&filed.text).expect("json")["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let uri = format!("/api/v1/projects/{HELSINKI}/access-requests/{id}/approve");
    let (one, two) = tokio::join!(
        send(&state, "olli", "POST", &uri, None),
        send(&state, "olli", "POST", &uri, None)
    );
    let mut statuses = [one.status, two.status];
    statuses.sort();
    assert_eq!(
        statuses,
        [StatusCode::ACCEPTED, StatusCode::CONFLICT],
        "{} / {}",
        one.text,
        two.text
    );
    let forge = FORGES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&pipeline)
        .expect("the world's forge");
    let opened = forge
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/pulls"))
        .count();
    assert_eq!(opened, 1, "one approval, one Change");
}

/// F-R2POR1-007 (T-6225): two owners revoking at once: one revoke Change, and the other gets the
/// 409 before it proposes anything; the request keeps the revoker who won.
#[tokio::test]
async fn two_revokes_at_once_open_one_change() {
    let _held = DATABASE.lock().await;
    let Some(pool) = pool().await else { return };
    let pipeline = unique("air");
    let state = state(pool, &pipeline, true).await;
    let filed = send(
        &state,
        "jana",
        "POST",
        &format!("/api/v1/projects/{KPIS}/pipelines/{pipeline}/access-requests"),
        Some(ask()),
    )
    .await;
    assert_eq!(filed.status, StatusCode::CREATED, "{}", filed.text);
    let id = serde_json::from_str::<Value>(&filed.text).expect("json")["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let approved = send(
        &state,
        "olli",
        "POST",
        &format!("/api/v1/projects/{HELSINKI}/access-requests/{id}/approve"),
        None,
    )
    .await;
    assert_eq!(approved.status, StatusCode::ACCEPTED, "{}", approved.text);
    // The grant merged.
    let request =
        joinedcontext_portal::access_requests::get(state.db.as_ref().expect("db"), HELSINKI, &id)
            .await
            .expect("read")
            .expect("there");
    let endpoint =
        json!({ "spec": { "audience": "project-list", "allowedProjects": ["traffic"] } });
    let policy =
        joinedcontext_portal::access_requests::grant_bundle(&request, &endpoint)[1].clone();
    state
        .mirror
        .upsert(serde_json::from_value(policy).expect("an envelope"));
    let uri = format!("/api/v1/projects/{HELSINKI}/access-requests/{id}/revoke");
    let reason = json!({ "reason": "the KPI project ended" });
    let (one, two) = tokio::join!(
        send(&state, "olli", "POST", &uri, Some(reason.clone())),
        send(&state, "olli", "POST", &uri, Some(reason))
    );
    let mut statuses = [one.status, two.status];
    statuses.sort();
    assert_eq!(
        statuses,
        [StatusCode::ACCEPTED, StatusCode::CONFLICT],
        "{} / {}",
        one.text,
        two.text
    );
    let forge = FORGES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&pipeline)
        .expect("the world's forge");
    let opened = forge
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/pulls"))
        .count();
    assert_eq!(opened, 2, "one approval and one revoke, one Change each");
    let revoked =
        joinedcontext_portal::access_requests::get(state.db.as_ref().expect("db"), HELSINKI, &id)
            .await
            .expect("read")
            .expect("there");
    assert_eq!(revoked.state, "revoked");
}

/// PL-91: the requester never approves their own request, even holding the owner's rights.
#[tokio::test]
async fn the_requester_does_not_approve_their_own_request() {
    let _held = DATABASE.lock().await;
    let Some(pool) = pool().await else { return };
    let pipeline = unique("self");
    let state = state(pool, &pipeline, true).await;
    role(
        &state,
        "air-owner-too",
        "jana",
        HELSINKI,
        &["ContextSpace", "Endpoint", "Policy", "Group"],
        &["read", "propose", "approve"],
    );
    let filed = send(
        &state,
        "jana",
        "POST",
        &format!("/api/v1/projects/{KPIS}/pipelines/{pipeline}/access-requests"),
        Some(ask()),
    )
    .await;
    assert_eq!(filed.status, StatusCode::CREATED, "{}", filed.text);
    let id = serde_json::from_str::<Value>(&filed.text).expect("json")["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let own = send(
        &state,
        "jana",
        "POST",
        &format!("/api/v1/projects/{HELSINKI}/access-requests/{id}/approve"),
        None,
    )
    .await;
    assert_eq!(own.status, StatusCode::FORBIDDEN, "{}", own.text);
    assert!(own.text.contains("self-approval"), "{}", own.text);
    let declined = send(
        &state,
        "olli",
        "POST",
        &format!("/api/v1/projects/{HELSINKI}/access-requests/{id}/decline"),
        Some(json!({ "reason": "use the open endpoint" })),
    )
    .await;
    assert_eq!(declined.status, StatusCode::OK, "{}", declined.text);
    let late = send(
        &state,
        "olli",
        "POST",
        &format!("/api/v1/projects/{HELSINKI}/access-requests/{id}/approve"),
        None,
    )
    .await;
    assert_eq!(late.status, StatusCode::CONFLICT, "{}", late.text);
}

/// PL-90: who may not propose the pipeline asks nothing; a wildcard, a shared account or an
/// unknown endpoint is refused before anything is stored.
#[tokio::test]
async fn a_request_is_refused_before_it_is_stored() {
    let _held = DATABASE.lock().await;
    let Some(pool) = pool().await else { return };
    let pipeline = unique("bad");
    let state = state(pool.clone(), &pipeline, true).await;
    let uri = format!("/api/v1/projects/{KPIS}/pipelines/{pipeline}/access-requests");
    assert_eq!(
        send(&state, "eve", "POST", &uri, Some(ask())).await.status,
        StatusCode::NOT_FOUND
    );
    role(&state, "kpi-reader", "rita", KPIS, &["Pipeline"], &["read"]);
    assert_eq!(
        send(&state, "rita", "POST", &uri, Some(ask())).await.status,
        StatusCode::FORBIDDEN
    );
    let mut wildcard = ask();
    wildcard["types"][0]["attrs"] = json!(["*"]);
    assert_eq!(
        send(&state, "jana", "POST", &uri, Some(wildcard))
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    let mut all = ask();
    all["types"][0]["attrs"] = json!([]);
    assert_eq!(
        send(&state, "jana", "POST", &uri, Some(all)).await.status,
        StatusCode::BAD_REQUEST
    );
    let shared = state_shared(pool, &pipeline).await;
    let answer = send(&shared, "jana", "POST", &uri, Some(ask())).await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
    assert!(
        answer.text.contains("JC_PORTAL_PIPELINE_IDENTITY"),
        "{}",
        answer.text
    );
    let mine = send(&state, "jana", "GET", &uri, None).await;
    assert_eq!(
        serde_json::from_str::<Value>(&mine.text).expect("json")["items"],
        json!([]),
        "nothing was stored"
    );

    // T-4946: a name that exists nowhere is answered as an unreadable one is, so the answer never
    // tells which Endpoints exist; it waits with no space, and no owner is ever shown it.
    let mut nowhere = ask();
    nowhere["endpoint"]["name"] = json!("not-there");
    let filed = send(&state, "jana", "POST", &uri, Some(nowhere)).await;
    assert_eq!(filed.status, StatusCode::CREATED, "{}", filed.text);
    assert!(!filed.text.contains("\"space\""), "{}", filed.text);
    let inbox = send(
        &state,
        "olli",
        "GET",
        &format!("/api/v1/projects/{HELSINKI}/access-requests"),
        None,
    )
    .await;
    assert_eq!(inbox.status, StatusCode::OK, "{}", inbox.text);
    assert!(!inbox.text.contains("not-there"), "{}", inbox.text);
}

async fn state_shared(pool: sqlx::PgPool, pipeline: &str) -> AppState {
    state(pool, pipeline, false).await
}

/// Helsinki's `air` space compiles a model whose AirQualityObserved has `pm10` and `pm25`; besides
/// the private `air-quality` it serves `air-open` to the whole organization; the KPI project has an
/// Endpoint on the pipeline's own space; `pia` stewards the pipeline and reads Helsinki too.
fn modelled(state: &AppState) {
    use joinedcontext_portal::pipeline_validation::ModelSchema;
    state
        .model_schemas
        .replace(std::collections::HashMap::from([(
            (HELSINKI.to_owned(), "air".to_owned()),
            Arc::new(ModelSchema::compile(
                "air",
                "1.0.0",
                &json!({ "definitions": { "AirQualityObserved": { "properties": {
                    "pm10": { "type": "number", "x-ngsi-ld-kind": "Property" },
                    "pm25": { "type": "number", "x-ngsi-ld-kind": "Property" }
                }}}}),
                &["AirQualityObserved".to_owned()],
                false,
            )),
        )]));
    state.mirror.upsert(common::envelope(
        "Endpoint",
        "air-open",
        HELSINKI,
        json!({ "contextSpaceRef": "air", "slug": "m3k8w2q7d4x9n5p6r1t0v2b7cy", "audience": "organization" }),
    ));
    state.mirror.upsert(common::envelope(
        "Endpoint",
        "kpi-out",
        KPIS,
        json!({ "contextSpaceRef": "kpi", "slug": "q8m2d7x4k9w3n6p5r1t0v2b7cz", "audience": "project-list" }),
    ));
    role(
        state,
        "kpi-steward",
        "pia",
        KPIS,
        &["Pipeline"],
        &["read", "propose"],
    );
    role(
        state,
        "air-reader",
        "pia",
        HELSINKI,
        &["ContextSpace", "Endpoint"],
        &["read"],
    );
}

fn listed_names(answer: &common::Answer) -> Vec<String> {
    let body: Value = serde_json::from_str(&answer.text).expect("json");
    body["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| {
            format!(
                "{}/{}",
                item["project"].as_str().unwrap_or_default(),
                item["name"].as_str().unwrap_or_default()
            )
        })
        .collect()
}

/// T-4946: the request form lists the Endpoints of other spaces the caller may read, or whose
/// audience is the organization, with their model's types and attributes; never the pipeline's own
/// space, never a private Endpoint of a project the caller cannot read, and nothing to a person
/// who may not propose the pipeline.
#[tokio::test]
async fn the_form_lists_endpoints_of_other_spaces_with_their_types() {
    let pipeline = unique("air");
    let state = world(&pipeline, true).await;
    modelled(&state);
    let uri = format!("/api/v1/projects/{KPIS}/pipelines/{pipeline}/access-requests/endpoints");

    // Jana proposes the pipeline and reads nothing of Helsinki: only the organization's Endpoint.
    let listed = send(&state, "jana", "GET", &uri, None).await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text);
    assert_eq!(
        listed_names(&listed),
        [format!("{HELSINKI}/air-open")],
        "{}",
        listed.text
    );
    let body: Value = serde_json::from_str(&listed.text).expect("json");
    assert_eq!(body["items"][0]["types"][0]["type"], "AirQualityObserved");
    assert_eq!(
        body["items"][0]["types"][0]["attrs"],
        json!(["pm10", "pm25"])
    );
    assert!(!listed.text.contains("air-quality"), "{}", listed.text);

    // Pia reads Helsinki's Endpoints as well: the private one is hers to choose too.
    let read = send(&state, "pia", "GET", &uri, None).await;
    assert_eq!(read.status, StatusCode::OK, "{}", read.text);
    assert_eq!(
        listed_names(&read),
        [
            format!("{HELSINKI}/air-open"),
            format!("{HELSINKI}/air-quality")
        ],
        "{}",
        read.text
    );
    assert!(!read.text.contains("kpi-out"), "{}", read.text);

    // Eve may not propose the pipeline: she learns nothing of it, nor of the Endpoints.
    let refused = send(&state, "eve", "GET", &uri, None).await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND, "{}", refused.text);
}

/// T-4946: the request door checks a listed Endpoint's types and attributes against its model and
/// refuses naming the field; an Endpoint the caller may not read and one that exists nowhere get
/// one answer, with no model check that would tell its types (here both reach the missing store);
/// the pipeline's own space is refused, and so is a name that is no name.
#[tokio::test]
async fn the_door_refuses_what_the_form_would_not_offer() {
    let pipeline = unique("air");
    let state = world(&pipeline, true).await;
    modelled(&state);
    let uri = format!("/api/v1/projects/{KPIS}/pipelines/{pipeline}/access-requests");
    let with = |endpoint: Value, types: Value| {
        let mut body = ask();
        body["endpoint"] = endpoint;
        body["types"] = types;
        Some(body)
    };
    let open = json!({ "project": HELSINKI, "name": "air-open" });
    let types = json!([{ "type": "AirQualityObserved", "attrs": ["pm10"] }]);

    let allowed = send(
        &state,
        "jana",
        "POST",
        &uri,
        with(open.clone(), types.clone()),
    )
    .await;
    assert_eq!(
        allowed.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        allowed.text
    );

    let no_type = with(
        open.clone(),
        json!([{ "type": "WeatherObserved", "attrs": ["pm10"] }]),
    );
    let refused = send(&state, "jana", "POST", &uri, no_type).await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.text);
    assert!(refused.text.contains("types[0].type"), "{}", refused.text);

    let no_attr = with(
        open,
        json!([{ "type": "AirQualityObserved", "attrs": ["pm10", "secret"] }]),
    );
    let refused = send(&state, "jana", "POST", &uri, no_attr).await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.text);
    assert!(
        refused.names_field("types[0].attrs", "'secret'"),
        "{}",
        refused.text
    );

    // Unreadable and unknown: the same answer, whatever the types name.
    let guess = json!([{ "type": "WeatherObserved", "attrs": ["secret"] }]);
    let unreadable = with(
        json!({ "project": HELSINKI, "name": "air-quality" }),
        guess.clone(),
    );
    let unreadable = send(&state, "jana", "POST", &uri, unreadable).await;
    let unknown = with(
        json!({ "project": "other-org-city", "name": "air-quality" }),
        guess,
    );
    let unknown = send(&state, "jana", "POST", &uri, unknown).await;
    for answer in [&unreadable, &unknown] {
        assert_eq!(
            answer.status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{}",
            answer.text
        );
    }
    assert_eq!(unreadable.text, unknown.text);

    let own = with(json!({ "project": KPIS, "name": "kpi-out" }), types.clone());
    let own = send(&state, "jana", "POST", &uri, own).await;
    let bad = with(
        json!({ "project": "Helsinki", "name": "air quality" }),
        types,
    );
    let bad = send(&state, "jana", "POST", &uri, bad).await;
    for answer in [&own, &bad] {
        assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{}", answer.text);
        assert!(answer.text.contains("endpoint.name"), "{}", answer.text);
    }
}

fn person_ask(types: &[&str]) -> Value {
    json!({ "types": types, "until": "2099-01-31T23:59:59Z", "reason": "weekly air report for the district office" })
}

const MINE: &str = "/api/v1/projects/helsinki/endpoints/air-quality/access-requests";

/// PL-115, EP-55, PF-58 (T-5615): a reader of an Endpoint who gets nothing through it asks for a read of
/// one of its types; an owner approves it as a Change; the grant is one Policy for that reader.
#[tokio::test]
async fn a_person_asks_on_the_endpoint_and_an_owner_approves() {
    let _held = DATABASE.lock().await;
    let Some(pool) = pool().await else { return };
    let state = state(pool, &unique("air"), true).await;
    modelled(&state);
    // Its own owner: the import check is kept per person and project, and the other tests'
    // owner proposes into Helsinki at the same time.
    let owner = ["ContextSpace", "Endpoint", "Policy", "Group"];
    role(
        &state,
        "air-owner",
        "otto",
        HELSINKI,
        &owner,
        &["read", "propose", "approve"],
    );
    // Its own Endpoint, so a request left in the database by an earlier run is no second one.
    let name = unique("air-person");
    state.mirror.upsert(common::envelope(
        "Endpoint",
        &name,
        HELSINKI,
        json!({ "contextSpaceRef": "air", "slug": "zt4qm7ge2xdv6ksb3ncf5arw2y", "audience": "project-list" }),
    ));
    let mine_uri = format!("/api/v1/projects/{HELSINKI}/endpoints/{name}/access-requests");
    let mine = mine_uri.as_str();
    let offer = send(&state, "eve", "GET", mine, None).await;
    assert_eq!(offer.status, StatusCode::OK, "{}", offer.text);
    let offer: Value = serde_json::from_str(&offer.text).expect("json");
    assert_eq!(offer["types"], json!(["AirQualityObserved"]));
    assert_eq!(offer["refusal"], Value::Null);

    let filed = send(
        &state,
        "eve",
        "POST",
        mine,
        Some(person_ask(&["AirQualityObserved"])),
    )
    .await;
    assert_eq!(filed.status, StatusCode::CREATED, "{}", filed.text);
    let request: Value = serde_json::from_str(&filed.text).expect("json");
    let id = request["id"].as_str().expect("an id").to_owned();
    assert_eq!(request["person"], id_of("eve"));
    assert_eq!(request["pipeline"], Value::Null);
    assert_eq!(request["account"], id_of("eve"));
    assert_eq!(request["state"], "pending");
    let again = send(
        &state,
        "eve",
        "POST",
        mine,
        Some(person_ask(&["AirQualityObserved"])),
    )
    .await;
    assert_eq!(again.status, StatusCode::CONFLICT, "{}", again.text);

    // Another person's requests stay theirs: otto sees none of eve's on the page.
    let otto = send(&state, "otto", "GET", mine, None).await;
    assert!(!otto.text.contains(&id), "{}", otto.text);
    let inbox = send(
        &state,
        "otto",
        "GET",
        "/api/v1/projects/helsinki/access-requests",
        None,
    )
    .await;
    assert!(inbox.text.contains(&id), "{}", inbox.text);
    let approved = send(
        &state,
        "otto",
        "POST",
        &format!("/api/v1/projects/{HELSINKI}/access-requests/{id}/approve"),
        None,
    )
    .await;
    assert_eq!(approved.status, StatusCode::ACCEPTED, "{}", approved.text);
    let answer = send(&state, "eve", "GET", mine, None).await;
    let answer: Value = serde_json::from_str(&answer.text).expect("json");
    assert_eq!(answer["items"][0]["state"], "approved");
    assert_eq!(answer["items"][0]["decidedBy"], "otto");

    // What the approval proposed: one Policy, for eve alone, on the one type she named.
    let stored =
        joinedcontext_portal::access_requests::get(state.db.as_ref().expect("db"), HELSINKI, &id)
            .await
            .expect("read")
            .expect("there");
    let endpoint = serde_json::to_value(
        state
            .mirror
            .get(HELSINKI, "Endpoint", &name)
            .expect("endpoint"),
    )
    .expect("json");
    let bundle = joinedcontext_portal::access_requests::grant_bundle(&stored, &endpoint);
    assert_eq!(bundle.len(), 1, "{bundle:?}");
    assert_eq!(
        bundle[0]["spec"]["assignee"],
        json!({ "kind": "user", "id": id_of("eve") })
    );
    assert_eq!(
        bundle[0]["spec"]["information"],
        json!([{ "entities": [{ "type": "AirQualityObserved" }] }])
    );

    // A request filed before ADR-N-061 names the person by username: approving it would write
    // that name into Git, so it is refused and the owner is told what to do.
    let legacy = joinedcontext_portal::access_requests::AccessRequest {
        id: joinedcontext_portal::access_requests::new_id(),
        requester: joinedcontext_portal::access_requests::Requester::Person("eve".into()),
        state: "pending".into(),
        decided_by: None,
        decided_at: None,
        change: None,
        ..stored.clone()
    };
    joinedcontext_portal::access_requests::insert(state.db.as_ref().expect("db"), &legacy)
        .await
        .expect("insert");
    let refused = send(
        &state,
        "otto",
        "POST",
        &format!(
            "/api/v1/projects/{HELSINKI}/access-requests/{}/approve",
            legacy.id
        ),
        None,
    )
    .await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.text);
    assert!(refused.text.contains("request again"), "{}", refused.text);

    // The owner who asks for themself does not approve their own request (CC-34).
    let own = send(
        &state,
        "otto",
        "POST",
        mine,
        Some(person_ask(&["AirQualityObserved"])),
    )
    .await;
    assert_eq!(own.status, StatusCode::CREATED, "{}", own.text);
    let own: Value = serde_json::from_str(&own.text).expect("json");
    let selfie = send(
        &state,
        "otto",
        "POST",
        &format!(
            "/api/v1/projects/{HELSINKI}/access-requests/{}/approve",
            own["id"].as_str().unwrap_or_default()
        ),
        None,
    )
    .await;
    assert_eq!(selfie.status, StatusCode::FORBIDDEN, "{}", selfie.text);
}

/// PL-115: each field is checked before anything is stored, and the refusal names the field; a
/// body naming a grantee is no request; who cannot read the Endpoint learns nothing of it.
#[tokio::test]
async fn a_persons_request_is_refused_field_by_field() {
    let state = world(&unique("air"), true).await;
    modelled(&state);
    for (body, (field, message)) in [
        (
            person_ask(&["Vehicle"]),
            ("types[0]", "'Vehicle' is no type this endpoint serves"),
        ),
        (person_ask(&[]), ("types", "choose one")),
        (
            json!({ "types": ["AirQualityObserved"], "until": "2020-01-01T00:00:00Z", "reason": "r" }),
            ("until", "until has passed"),
        ),
        (
            json!({ "types": ["AirQualityObserved"], "reason": "  " }),
            ("reason", "reason is required"),
        ),
    ] {
        let refused = send(&state, "eve", "POST", MINE, Some(body)).await;
        assert_eq!(
            refused.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{field}: {}",
            refused.text
        );
        assert!(
            refused.names_field(field, message),
            "{field}: {}",
            refused.text
        );
    }
    let grantee = json!({ "types": ["AirQualityObserved"], "reason": "r", "person": "olli" });
    let refused = send(&state, "eve", "POST", MINE, Some(grantee)).await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.text);
    for verb in ["GET", "POST"] {
        let hidden = send(
            &state,
            "jana",
            verb,
            MINE,
            Some(person_ask(&["AirQualityObserved"])),
        )
        .await;
        assert_eq!(
            hidden.status,
            StatusCode::NOT_FOUND,
            "{verb}: {}",
            hidden.text
        );
    }
    // An Endpoint serving only the Policies it names cannot carry a new grant: said, not stored.
    state.mirror.upsert(common::envelope(
        "Endpoint",
        "air-quality",
        HELSINKI,
        json!({ "contextSpaceRef": "air", "slug": "zt4qm7ge2xdv6ksb3ncf5arw2y", "policyRefs": ["urn:ngsi-ld:Policy:air:read"] }),
    ));
    let fixed = send(
        &state,
        "eve",
        "POST",
        MINE,
        Some(person_ask(&["AirQualityObserved"])),
    )
    .await;
    assert_eq!(fixed.status, StatusCode::CONFLICT, "{}", fixed.text);
    assert!(fixed.text.contains("policy list"), "{}", fixed.text);
}

/// T-5567, AUT-2: approving, declining and revoking an access request are decisions; an owner
/// whose session proved no second factor is refused at each, the request stays as it was, and
/// the same owner after a step-up login decides it.
#[tokio::test]
async fn an_owner_decides_a_request_only_after_a_second_factor() {
    let _held = DATABASE.lock().await;
    let Some(pool) = pool().await else { return };
    let pipeline = unique("mfa");
    let mut state = state(pool, &pipeline, true).await;
    state.config = Arc::new(Config {
        decisions_need_second_factor: true,
        ..(*state.config).clone()
    });
    let filed = send(
        &state,
        "jana",
        "POST",
        &format!("/api/v1/projects/{KPIS}/pipelines/{pipeline}/access-requests"),
        Some(ask()),
    )
    .await;
    assert_eq!(filed.status, StatusCode::CREATED, "{}", filed.text);
    let id = serde_json::from_str::<Value>(&filed.text).expect("json")["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let base = format!("/api/v1/projects/{HELSINKI}/access-requests/{id}");
    for (door, body) in [
        ("approve", None),
        (
            "decline",
            Some(json!({ "reason": "use the open endpoint" })),
        ),
        ("revoke", Some(json!({ "reason": "the project ended" }))),
    ] {
        let refused = send(&state, "olli", "POST", &format!("{base}/{door}"), body).await;
        assert_eq!(
            refused.status,
            StatusCode::FORBIDDEN,
            "{door}: {}",
            refused.text
        );
        assert!(
            refused.text.contains("second-factor-required"),
            "{door}: {}",
            refused.text
        );
    }
    let mut olli = common::person("olli");
    olli.second_factor = true;
    let approved = common::send(&state, olli, "POST", &format!("{base}/approve"), None).await;
    assert_eq!(approved.status, StatusCode::ACCEPTED, "{}", approved.text);
}
