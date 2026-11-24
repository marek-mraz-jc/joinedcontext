//! API/01 §46, DM-142, T-5356: an erasure takes four eyes. The proposer never approves it, the
//! approver types the subject the proposal names, and a declined or expired proposal is approved
//! by nobody. Runs when `JC_PORTAL_TEST_DATABASE_URL` points at a PostgreSQL the test may write
//! to; otherwise it says so and returns.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};

use joinedcontext_portal::config::Config;
use joinedcontext_portal::erasure;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use wiremock::matchers::{method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SPACE: &str = "trips";
const KEY: &str = "a-test-root-secret-key-of-32-bytes!";

fn role(state: &AppState, name: &str, user: &str, project: &str, verbs: &[&str]) {
    state.mirror.upsert(common::envelope(
        "Role",
        name,
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": verbs }] }),
    ));
    state.mirror.upsert(common::envelope(
        "RoleBinding",
        &format!("{name}-{user}"),
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": format!("{user}@hel.fi") }], "role": name, "scope": { "project": project } }),
    ));
}

/// `anna` and `boris` own the space; `eve` reads it.
async fn world() -> Option<(AppState, String)> {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|u| !u.trim().is_empty());
    let Some(url) = url else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return None;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("the test database answers and migrates");
    let project = format!("fe-{}", crate::common::unique::unique());
    let mut config = Config::for_tests();
    config.artifact_store = Some(joinedcontext_portal::artifact_store::Settings {
        endpoint: "http://127.0.0.1:9".into(),
        bucket: "artifacts".into(),
        region: "us-east-1".into(),
        root_access_key: "test".into(),
        root_secret_key: KEY.into(),
    });
    let state = AppState::new(config, None).with_db(pool);
    state.mirror.upsert(common::envelope(
        "Project",
        &project,
        ORG_NAMESPACE,
        json!({}),
    ));
    state.mirror.upsert(common::envelope(
        "ContextSpace",
        SPACE,
        &project,
        json!({ "broker": "antares", "dataModelRef": { "name": "trips" } }),
    ));
    let owner = ["read", "propose", "approve"];
    role(&state, "trips-owner", "anna", &project, &owner);
    role(&state, "trips-owner-2", "boris", &project, &owner);
    role(&state, "trips-reader", "eve", &project, &["read"]);
    // The erasure log is keyed (T-7202) and forced (T-7201): the project's Organization is in the
    // registry and its key recorded, as the proposing door's `project_ids::required` does first.
    let pool = state.db.clone().expect("a database");
    common::keyed_project(&state, &pool, &project, "helsinki").await;
    joinedcontext_portal::project_ids::key_of(&pool, &state.mirror, &project)
        .await
        .expect("the registry")
        .expect("a registered project has a key");
    Some((state, project))
}

/// anna's proposal for the subject `r-42`, `days` old.
async fn proposal(state: &AppState, project: &str, days: i64) -> String {
    let mut proposed = erasure::begin(
        (project, SPACE),
        erasure::subject_hash(KEY, project, SPACE, "r-42"),
        "Art. 17 request 7",
        "user:anna",
        vec![],
    );
    proposed.requested_at -= time::Duration::days(days);
    let pool = state.db.as_ref().expect("a database");
    erasure::insert(pool, &proposed).await.expect("logged");
    proposed.id
}

async fn send(
    state: &AppState,
    who: &str,
    verb: &str,
    uri: &str,
    body: Option<Value>,
) -> common::Answer {
    common::send(state, common::person(who), verb, uri, body).await
}

const LINKML: &str = "id: https://example.org/trips\nname: trips\nslots:\n  rider:\n    range: string\n    annotations: { personalData: true }\nclasses:\n  Trip:\n    annotations: { dataSubjectRef: rider }\n    slots: [rider]\n";
const TRIP: &str = "urn:ngsi-ld:Trip:hel.fi:trips:7";

/// The space `trips` read through a forge holding its model and a gateway holding the subject's
/// one Trip with its history: what an export and an erasure read before they answer.
async fn surfaced(state: AppState, gateway: &MockServer) -> AppState {
    surfaced_with(state, gateway, LINKML).await
}

/// [`surfaced`] with a forge holding `linkml` as the space's model.
async fn surfaced_with(state: AppState, gateway: &MockServer, linkml: &str) -> AppState {
    use joinedcontext_portal::auth::client_auth::ClientAuth;
    use joinedcontext_portal::reconciler::drift::Watch;
    let shako = common::forge().await;
    Mock::given(method("GET"))
        .and(path(format!("{}/Shako.File(Path=@p,Ref=@r)", common::REPO)))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "Sha": "blob-trips", "Content": common::encode(linkml) })),
        )
        .mount(&shako)
        .await;
    Mock::given(method("POST"))
        .and(path("/realms/city/protocol/openid-connect/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "access_token": "t" })))
        .mount(gateway)
        .await;
    Mock::given(method("GET"))
        .and(path_regex("^/cs/[^/]+/ngsi-ld/v1/entities$"))
        .and(query_param("q", "rider==\"r-42\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!([{ "id": TRIP, "type": "Trip", "rider": { "type": "Property", "value": "r-42" } }]),
        ))
        .mount(gateway)
        .await;
    Mock::given(method("GET"))
        .and(path_regex("^/cs/[^/]+/ngsi-ld/v1/temporal/entities/.+$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": TRIP })))
        .mount(gateway)
        .await;
    let client = joinedcontext_portal::forge::ShakoClient::new(
        shako.uri().parse().expect("mock url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
    std::mem::forget(shako);
    let mut state = state.with_shako(std::sync::Arc::new(client));
    state.drift_watch = Some(std::sync::Arc::new(Watch::new(
        gateway.uri(),
        &format!("{}/realms/city", gateway.uri()),
        "portal".to_owned(),
        ClientAuth::Secret("s".into()),
    )));
    state
}

fn model(state: &AppState, project: &str) {
    state.mirror.upsert(common::envelope(
        "DataModel",
        "trips",
        project,
        json!({ "linkml": "trips.linkml.yaml", "classes": ["Trip"] }),
    ));
}

/// DM-124: an owner exports the subject's records with their history, answered to the session
/// alone (no-store) and keyed by the subject's hash, never its value.
#[tokio::test]
async fn an_owner_exports_a_subjects_records_with_their_history() {
    let Some((state, project)) = world().await else {
        return;
    };
    let gateway = MockServer::start().await;
    model(&state, &project);
    let state = surfaced(state, &gateway).await;
    let uri = format!("/api/v1/projects/{project}/spaces/{SPACE}/subjects/export");
    let answer = send(
        &state,
        "anna",
        "POST",
        &uri,
        Some(json!({"subject": "r-42", "reason": "Art. 15 request 9"})),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let body: Value = serde_json::from_str(&answer.text).expect("json");
    assert_eq!(body["space"], SPACE);
    assert_eq!(
        body["subject"],
        erasure::subject_hash(KEY, &project, SPACE, "r-42")
    );
    assert_eq!(body["entities"][0]["class"], "Trip");
    assert_eq!(body["entities"][0]["entity"]["id"], TRIP);
    assert_eq!(body["entities"][0]["history"]["id"], TRIP);
}

/// DM-142: an owner proposes (dry run first, which logs nothing), a second owner who types the
/// same subject approves, and the erasure reads back approved with its stores.
#[tokio::test]
async fn a_proposed_erasure_is_previewed_logged_and_approved_by_a_second_owner() {
    let Some((state, project)) = world().await else {
        return;
    };
    let gateway = MockServer::start().await;
    model(&state, &project);
    let state = surfaced(state, &gateway).await;
    let base = format!("/api/v1/projects/{project}/spaces/{SPACE}/erasures");
    let body = json!({"subject": "r-42", "reason": "Art. 17 request 9"});
    let dry = send(
        &state,
        "anna",
        "POST",
        &format!("{base}?dryRun=All"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(dry.status, StatusCode::OK, "{}", dry.text);
    assert!(dry.text.contains(TRIP), "{}", dry.text);
    let listed = send(&state, "anna", "GET", &base, None).await;
    let items: Value = serde_json::from_str(&listed.text).expect("json");
    assert_eq!(items["items"], json!([]), "a dry run logs nothing");

    let proposed = send(&state, "anna", "POST", &base, Some(body)).await;
    assert_eq!(proposed.status, StatusCode::ACCEPTED, "{}", proposed.text);
    let view: Value = serde_json::from_str(&proposed.text).expect("json");
    assert_eq!(view["state"], "proposed");
    let id = view["id"].as_str().expect("an id").to_owned();

    let approved = send(
        &state,
        "boris",
        "POST",
        &format!("{base}/{id}/approve"),
        Some(json!({"subject": "r-42"})),
    )
    .await;
    assert_eq!(approved.status, StatusCode::OK, "{}", approved.text);
    let view: Value = serde_json::from_str(&approved.text).expect("json");
    assert_eq!(view["id"], id.as_str());
    assert_ne!(view["state"], "proposed", "{view}");

    let read = send(&state, "anna", "GET", &format!("{base}/{id}"), None).await;
    assert_eq!(read.status, StatusCode::OK, "{}", read.text);
    // a second approval or a decline of what is no longer a proposal is a 409
    let again = send(
        &state,
        "anna",
        "POST",
        &format!("{base}/{id}/decline"),
        Some(json!({"reason": "too late"})),
    )
    .await;
    assert_eq!(again.status, StatusCode::CONFLICT, "{}", again.text);
    assert!(again.text.contains("only a proposal"), "{}", again.text);
}

/// What an export or erasure needs and does not have is refused with what to do: a space with
/// no model, a model naming no subject, no key, no space surface, a surface that does not answer.
#[tokio::test]
async fn an_export_without_what_it_needs_says_what_is_missing() {
    let Some((state, project)) = world().await else {
        return;
    };
    let uri = format!("/api/v1/projects/{project}/spaces/{SPACE}/subjects/export");
    let body = || Some(json!({"subject": "r-42", "reason": "Art. 15 request 9"}));

    // no model in the mirror yet: the model's own 404
    let answer = send(&state, "anna", "POST", &uri, body()).await;
    assert_eq!(answer.status, StatusCode::NOT_FOUND, "{}", answer.text);

    // no space surface: 503
    model(&state, &project);
    let gateway = MockServer::start().await;
    let mut bare = surfaced(state.clone(), &gateway).await;
    bare.drift_watch = None;
    let answer = send(&bare, "anna", "POST", &uri, body()).await;
    assert_eq!(
        answer.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        answer.text
    );
    assert!(answer.text.contains("no space surface"), "{}", answer.text);

    // a surface whose realm refuses the Portal's token: 503, try again
    let refusing = MockServer::start().await;
    let mut down = surfaced(state.clone(), &gateway).await;
    {
        use joinedcontext_portal::auth::client_auth::ClientAuth;
        use joinedcontext_portal::reconciler::drift::Watch;
        down.drift_watch = Some(std::sync::Arc::new(Watch::new(
            refusing.uri(),
            &format!("{}/realms/city", refusing.uri()),
            "portal".to_owned(),
            ClientAuth::Secret("s".into()),
        )));
    }
    let answer = send(&down, "anna", "POST", &uri, body()).await;
    assert_eq!(
        answer.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        answer.text
    );
    assert!(answer.text.contains("did not answer"), "{}", answer.text);

    // a space with no data model, and one whose model names no subject: 400
    state.mirror.upsert(common::envelope(
        "ContextSpace",
        "loose",
        &project,
        json!({ "broker": "antares" }),
    ));
    let answer = send(
        &state,
        "anna",
        "POST",
        &format!("/api/v1/projects/{project}/spaces/loose/subjects/export"),
        body(),
    )
    .await;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{}", answer.text);
    assert!(answer.text.contains("has no data model"), "{}", answer.text);

    // a model whose classes name no data subject: 400 naming the annotation
    let plain = surfaced_with(
        state.clone(),
        &gateway,
        "id: https://example.org/trips\nname: trips\nslots:\n  rider:\n    range: string\nclasses:\n  Trip:\n    slots: [rider]\n",
    )
    .await;
    let answer = send(&plain, "anna", "POST", &uri, body()).await;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{}", answer.text);
    assert!(
        answer.text.contains("names no data subject"),
        "{}",
        answer.text
    );
    assert!(answer.text.contains("dataSubjectRef"), "{}", answer.text);

    // a subject no record can hold, or a blank reason: 400 before anything is read
    for (subject, reason) in [
        ("", "Art. 15 request 9"),
        ("r\u{7}42", "Art. 15"),
        ("r-42", "  "),
    ] {
        let answer = send(
            &state,
            "anna",
            "POST",
            &uri,
            Some(json!({ "subject": subject, "reason": reason })),
        )
        .await;
        assert_eq!(
            answer.status,
            StatusCode::BAD_REQUEST,
            "{subject:?}: {}",
            answer.text
        );
    }

    // no key to hash the subject with: 503 naming JC_PORTAL_ERASURE_KEY
    let mut keyless = state.clone();
    let mut config = (*keyless.config).clone();
    config.artifact_store = None;
    config.erasure_key = None;
    keyless.config = std::sync::Arc::new(config);
    let answer = send(&keyless, "anna", "POST", &uri, body()).await;
    assert_eq!(
        answer.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        answer.text
    );
    assert!(
        answer.text.contains("JC_PORTAL_ERASURE_KEY"),
        "{}",
        answer.text
    );
}

/// The erasure log down, or refusing writes: reads are a 503, a proposal that was not logged
/// says nothing was proposed, and an id that is no erasure id is a plain 404.
#[tokio::test]
async fn an_erasure_log_that_is_down_or_read_only_proposes_nothing() {
    let Some((state, project)) = world().await else {
        return;
    };
    let base = format!("/api/v1/projects/{project}/spaces/{SPACE}/erasures");
    let unknown = send(&state, "anna", "GET", &format!("{base}/not-an-id"), None).await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND, "{}", unknown.text);

    let id = proposal(&state, &project, 0).await;
    // port 1 is privileged and nothing in a test container listens there
    let down = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_secs(2))
        .connect_lazy("postgres://portal:portal@127.0.0.1:1/portal")
        .expect("a lazy pool");
    let mut dead = state.clone();
    dead.db = Some(down);
    let answer = send(&dead, "anna", "GET", &format!("{base}/{id}"), None).await;
    assert_eq!(
        answer.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        answer.text
    );
    assert!(answer.text.contains("could not be read"), "{}", answer.text);

    // a log that reads but takes no write: the decline and the proposal are refused whole
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL").expect("the test database");
    let options: sqlx::postgres::PgConnectOptions = url.parse().expect("a database url");
    let read_only = sqlx::postgres::PgPoolOptions::new()
        .connect_with(options.options([("default_transaction_read_only", "on")]))
        .await
        .expect("a read-only pool");
    let mut frozen = state.clone();
    frozen.db = Some(read_only);
    let declined = send(
        &frozen,
        "anna",
        "POST",
        &format!("{base}/{id}/decline"),
        Some(json!({"reason": "withdrawn"})),
    )
    .await;
    assert_eq!(
        declined.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        declined.text
    );
    assert!(
        declined.text.contains("could not be written"),
        "{}",
        declined.text
    );
    let still = send(&state, "anna", "GET", &format!("{base}/{id}"), None).await;
    assert!(still.text.contains("\"proposed\""), "{}", still.text);

    let gateway = MockServer::start().await;
    model(&state, &project);
    let frozen = surfaced(frozen, &gateway).await;
    // the proposal's first write, the project's key claim, is refused: nothing is proposed
    let proposed = send(
        &frozen,
        "anna",
        "POST",
        &base,
        Some(json!({"subject": "r-42", "reason": "Art. 17 request 9"})),
    )
    .await;
    assert_eq!(
        proposed.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        proposed.text
    );
    assert!(
        proposed.text.contains("could not be read or written"),
        "{}",
        proposed.text
    );
}

#[tokio::test]
async fn the_proposer_never_approves_and_the_approver_types_the_proposals_subject() {
    let Some((state, project)) = world().await else {
        return;
    };
    let id = proposal(&state, &project, 0).await;
    let base = format!("/api/v1/projects/{project}/spaces/{SPACE}/erasures");
    let approve = format!("{base}/{id}/approve");

    let own = send(
        &state,
        "anna",
        "POST",
        &approve,
        Some(json!({"subject": "r-42"})),
    )
    .await;
    assert_eq!(own.status, StatusCode::FORBIDDEN, "{}", own.text);
    assert!(own.text.contains("self-approval"), "{}", own.text);

    let reader = send(
        &state,
        "eve",
        "POST",
        &approve,
        Some(json!({"subject": "r-42"})),
    )
    .await;
    assert_eq!(reader.status, StatusCode::FORBIDDEN, "{}", reader.text);

    let other = send(
        &state,
        "boris",
        "POST",
        &approve,
        Some(json!({"subject": "r-43"})),
    )
    .await;
    assert_eq!(
        other.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        other.text
    );
    assert!(other.text.contains("subject"), "{}", other.text);
    assert!(
        !other.text.contains("r-43"),
        "the answer never echoes the value: {}",
        other.text
    );

    // a body that is no JSON object of the route's shape is a 400 on every write
    for (who, uri) in [
        ("boris", approve.clone()),
        ("anna", format!("{base}/{id}/decline")),
        (
            "anna",
            format!("/api/v1/projects/{project}/spaces/{SPACE}/subjects/export"),
        ),
        ("anna", base.clone()),
    ] {
        let broken = common::send_body(
            &state,
            common::person(who),
            "POST",
            &uri,
            "application/json",
            axum::body::Body::from("{\"subject\": "),
        )
        .await;
        assert_eq!(
            broken.status,
            StatusCode::BAD_REQUEST,
            "{uri}: {}",
            broken.text
        );
    }

    // a subject no record can hold is refused before the log is read
    for bad in ["", "   ", "r\"42", "r\\42"] {
        let typed = send(
            &state,
            "boris",
            "POST",
            &approve,
            Some(json!({ "subject": bad })),
        )
        .await;
        assert_eq!(
            typed.status,
            StatusCode::BAD_REQUEST,
            "{bad:?}: {}",
            typed.text
        );
        assert!(typed.text.contains("no quote, backslash"), "{}", typed.text);
    }

    let unknown = send(
        &state,
        "boris",
        "POST",
        &format!("{base}/er-20261019-0a1b2c3d4e5f/approve"),
        Some(json!({"subject": "r-42"})),
    )
    .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND, "{}", unknown.text);

    // The second owner finds the proposal in the list; nothing ran.
    let listed = send(&state, "boris", "GET", &base, None).await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text);
    let items: Value = serde_json::from_str(&listed.text).expect("json");
    let item = &items["items"][0];
    assert_eq!(item["id"], id.as_str());
    assert_eq!(item["state"], "proposed");
    assert_eq!(item["proposedBy"], "user:anna");
    assert_eq!(item["stores"]["broker"]["attempts"], 0);
    let refused = send(&state, "eve", "GET", &base, None).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text);
}

#[tokio::test]
async fn a_declined_or_expired_proposal_is_approved_by_nobody() {
    let Some((state, project)) = world().await else {
        return;
    };
    let base = format!("/api/v1/projects/{project}/spaces/{SPACE}/erasures");
    let id = proposal(&state, &project, 0).await;

    let bare = send(
        &state,
        "anna",
        "POST",
        &format!("{base}/{id}/decline"),
        Some(json!({"reason": " "})),
    )
    .await;
    assert_eq!(bare.status, StatusCode::BAD_REQUEST, "{}", bare.text);
    assert!(bare.text.contains("reason-required"), "{}", bare.text);

    let withdrawn = send(
        &state,
        "anna",
        "POST",
        &format!("{base}/{id}/decline"),
        Some(json!({"reason": "the person withdrew the request"})),
    )
    .await;
    assert_eq!(withdrawn.status, StatusCode::OK, "{}", withdrawn.text);
    let view: Value = serde_json::from_str(&withdrawn.text).expect("json");
    assert_eq!(view["state"], "declined");
    assert_eq!(view["declinedBy"], "user:anna");

    let late = send(
        &state,
        "boris",
        "POST",
        &format!("{base}/{id}/approve"),
        Some(json!({"subject": "r-42"})),
    )
    .await;
    assert_eq!(late.status, StatusCode::CONFLICT, "{}", late.text);
    assert!(late.text.contains("declined"), "{}", late.text);

    let old = proposal(&state, &project, erasure::PROPOSAL_DAYS).await;
    let expired = send(
        &state,
        "boris",
        "POST",
        &format!("{base}/{old}/approve"),
        Some(json!({"subject": "r-42"})),
    )
    .await;
    assert_eq!(expired.status, StatusCode::CONFLICT, "{}", expired.text);
    assert!(expired.text.contains("expired"), "{}", expired.text);
    let read = send(&state, "boris", "GET", &format!("{base}/{old}"), None).await;
    assert_eq!(
        serde_json::from_str::<Value>(&read.text).expect("json")["state"],
        "expired"
    );
}
