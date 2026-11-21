//! Moving a project to another Organization (T-7305): an operator `ProjectMove` Change, approved
//! by a second operator, re-keys the project's registry row and every row keyed by it in one
//! transaction. Afterwards the rows read only under the new Organization's key (forced RLS), the
//! old Organization keys the project no more, a second move is a no-op, the plan counts rows per
//! table, and nobody but an operator with a second factor reaches the door.
//!
//! The store is the database `JC_PORTAL_TEST_DATABASE_URL` names (CI's portal-test jobs set it). A
//! missing variable fails, never skips. Each test uses Organizations and projects of its own run.

mod common;

use axum::http::StatusCode;
use jc_types::ProjectKey;
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};

use common::{envelope, person, send, REPO};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CHANGES: &str = "/api/v1/operator/changes";

/// A Portal serving as a role apart from the migrating one, without BYPASSRLS, as on the cluster
/// (T-7179): forced row-level security holds for every statement of these tests.
async fn world() -> AppState {
    let owner_url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .expect("set JC_PORTAL_TEST_DATABASE_URL to a PostgreSQL database the Portal migrates");
    let owner = joinedcontext_portal::db::connect(&owner_url)
        .await
        .expect("connect + migrate");
    let role = format!("move{}", common::unique::unique());
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE ROLE \"{role}\" LOGIN PASSWORD 'serving-pw'"
    )))
    .execute(&owner)
    .await
    .expect("a serving role");
    let mut serving_url = url::Url::parse(&owner_url).expect("the test URL");
    serving_url.set_username(&role).expect("a user");
    serving_url
        .set_password(Some("serving-pw"))
        .expect("a password");
    // The grants to a serving role race when two tests of this binary boot at once.
    static BOOT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let booting = BOOT.lock().await;
    let pool = joinedcontext_portal::db::connect_as(serving_url.as_str(), Some(&owner_url))
        .await
        .expect("serve as the serving role");
    drop(booting);
    let mut config = Config::for_tests();
    config.decisions_need_second_factor = false;
    AppState::new(config, None).with_db(pool)
}

fn pool(state: &AppState) -> &sqlx::PgPool {
    state.db.as_ref().expect("db")
}

fn operator(state: &AppState, name: &str) -> Identity {
    let mut who = person(name);
    who.groups = vec![state.config.bootstrap_admins.clone()];
    who.second_factor = true;
    who
}

fn body(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_default()
}

fn run(test: &str) -> String {
    format!("{test}-{}", common::unique::unique())
}

/// `project` filed under Organization `from` in the mirror and the registry, with one draft and
/// one activity row under its key; both Organizations active.
async fn placed(state: &AppState, project: &str, from: &str, to: &str) -> ProjectKey {
    for organization in [from, to] {
        sqlx::query(
            "INSERT INTO organizations (name, host, state, created_by) \
             VALUES ($1, $1 || '.example', 'active', 'test') ON CONFLICT DO NOTHING",
        )
        .bind(organization)
        .execute(pool(state))
        .await
        .expect("an Organization");
    }
    manifest(state, project, from);
    let key = joinedcontext_portal::project_ids::key(pool(state), from, project)
        .await
        .expect("registry")
        .expect("a key");
    let mut tx = joinedcontext_portal::db::tenant_tx(pool(state), key)
        .await
        .expect("keyed");
    sqlx::query(
        "INSERT INTO drafts (project, kind, name, manifest, touched_by, touched_kind) \
         VALUES ($1, 'ContextSpace', 'air', $2, 'test', 'person')",
    )
    .bind(project)
    .bind(json!({ "kind": "ContextSpace", "metadata": { "name": "air", "namespace": project } }))
    .execute(&mut *tx)
    .await
    .expect("a draft");
    sqlx::query(
        "INSERT INTO activity (time, project, kind, source, summary, severity) \
         VALUES (now(), $1, 'Space', 'test', 'air created', 'info')",
    )
    .bind(project)
    .execute(&mut *tx)
    .await
    .expect("an activity row");
    tx.commit().await.expect("commit");
    key
}

fn manifest(state: &AppState, project: &str, organization: &str) {
    state.mirror.upsert(envelope(
        "Project",
        project,
        ORG_NAMESPACE,
        json!({ "organizationRef": organization }),
    ));
}

/// The rows of every keyed table under (`key.org`, `key.project`), counted as the tables' owner,
/// past row-level security.
async fn rows_under(project: &str, key: ProjectKey) -> i64 {
    let owner = sqlx::PgPool::connect(
        &std::env::var("JC_PORTAL_TEST_DATABASE_URL").expect("JC_PORTAL_TEST_DATABASE_URL"),
    )
    .await
    .expect("the owner");
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name::text FROM information_schema.columns WHERE table_schema = 'public' \
         AND column_name IN ('org_id', 'project_id') GROUP BY table_name HAVING count(*) = 2",
    )
    .fetch_all(&owner)
    .await
    .expect("catalog");
    let mut rows = 0;
    for table in tables {
        let held: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT count(*) FROM \"{table}\" WHERE org_id = $1::uuid AND project_id = $2::uuid"
        )))
        .bind(key.org.as_uuid().to_string())
        .bind(key.project.as_uuid().to_string())
        .fetch_one(&owner)
        .await
        .unwrap_or_else(|e| panic!("{table} of {project}: {e}"));
        rows += held;
    }
    rows
}

/// The drafts a transaction keyed with `key` reads of `project`.
async fn drafts_under(state: &AppState, key: ProjectKey, project: &str) -> i64 {
    let mut tx = joinedcontext_portal::db::tenant_tx(pool(state), key)
        .await
        .expect("keyed");
    sqlx::query_scalar("SELECT count(*) FROM drafts WHERE project = $1")
        .bind(project)
        .fetch_one(&mut *tx)
        .await
        .expect("drafts")
}

async fn propose(state: &AppState, who: Identity, project: &str, to: &str) -> (StatusCode, Value) {
    let change = json!({ "kind": "ProjectMove", "project": project, "organization": to });
    let answer = send(state, who, "POST", CHANGES, Some(change)).await;
    (answer.status, body(&answer.text))
}

async fn approve(state: &AppState, who: Identity, id: &str) -> (StatusCode, Value) {
    let answer = send(state, who, "POST", &format!("{CHANGES}/{id}/approve"), None).await;
    (answer.status, body(&answer.text))
}

async fn plan(state: &AppState, who: Identity, project: &str, to: &str) -> (StatusCode, Value) {
    let path = format!("/api/v1/operator/project-moves/plan?project={project}&organization={to}");
    let answer = send(state, who, "GET", &path, None).await;
    (answer.status, body(&answer.text))
}

/// Organizations `from` and `to` in the mirror, an org-admin of each and a reading member of
/// `from` (people `{from}-admin`, `{to}-admin`, `{from}-member`), as the cluster binds them.
fn people(state: &AppState, from: &str, to: &str) {
    let m = &state.mirror;
    for organization in [from, to] {
        m.upsert(envelope(
            "Organization",
            organization,
            ORG_NAMESPACE,
            json!({ "domain": format!("{organization}.example") }),
        ));
    }
    let kinds: Vec<&str> = joinedcontext_portal::resource::kinds()
        .map(|info| info.kind)
        .collect();
    for (role, verbs) in [
        ("org-admin", json!(["read", "propose", "approve", "delete"])),
        ("member", json!(["read"])),
    ] {
        m.upsert(envelope(
            "Role",
            role,
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": kinds, "verbs": verbs }] }),
        ));
    }
    for (user, role, organization) in [
        (format!("{from}-admin"), "org-admin", from),
        (format!("{to}-admin"), "org-admin", to),
        (format!("{from}-member"), "member", from),
    ] {
        m.upsert(envelope(
            "RoleBinding",
            &user,
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": format!("{user}@hel.fi") }],
                    "role": role, "scope": { "organization": organization } }),
        ));
    }
}

/// `GET path` as the person `name`: the status and the body.
async fn read_as(state: &AppState, name: &str, path: &str) -> (StatusCode, String) {
    let answer = send(state, person(name), "GET", path, None).await;
    (answer.status, answer.text)
}

fn denied(status: StatusCode) -> bool {
    status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND
}

/// The move itself: refused while the manifest names the old Organization, planned with counts,
/// applied by a second operator; then the rows read under the new key only, the old Organization
/// keys the project no more, and the audit holds counts, never a row.
#[tokio::test]
async fn a_moved_project_reads_only_under_its_new_organization() {
    let state = world().await;
    let from = run("hel");
    let to = run("bbsk");
    let project = run("moved");
    let old = placed(&state, &project, &from, &to).await;
    assert_eq!(drafts_under(&state, old, &project).await, 1);
    people(&state, &from, &to);
    let at = format!("/api/v1/projects/{project}");
    let drafts = format!("{at}/drafts");
    for reader in [format!("{from}-admin"), format!("{from}-member")] {
        let (status, text) = read_as(&state, &reader, &at).await;
        assert_eq!(status, StatusCode::OK, "{reader} before the move: {text}");
    }
    let (status, text) = read_as(&state, &format!("{from}-admin"), &drafts).await;
    assert!(
        status == StatusCode::OK && text.contains("\"air\""),
        "{from}-admin reads the draft before the move: {status} {text}"
    );
    let (status, text) = read_as(&state, &format!("{to}-admin"), &at).await;
    assert!(
        denied(status),
        "{to}-admin before the move: {status} {text}"
    );

    // The move is asked for while the manifest still names the old Organization (T-7462): the
    // proposal is what lets the Project Change name the target, and the approval waits for it.
    let (status, made) = propose(&state, operator(&state, "ops-a"), &project, &to).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let id = made["id"].as_str().expect("an id").to_owned();
    let (status, waiting) = plan(&state, operator(&state, "ops-a"), &project, &to).await;
    assert_eq!(status, StatusCode::OK, "{waiting}");
    assert_eq!(waiting["blockers"], json!([]), "{waiting}");
    assert!(
        waiting["waits"]
            .to_string()
            .contains("spec.organizationRef"),
        "{waiting}"
    );
    let (status, refused) = approve(&state, operator(&state, "ops-b"), &id).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(
        refused.to_string().contains("spec.organizationRef"),
        "{refused}"
    );

    manifest(&state, &project, &to);
    let (status, planned) = plan(&state, operator(&state, "ops-a"), &project, &to).await;
    assert_eq!(status, StatusCode::OK, "{planned}");
    assert_eq!(planned["from"], json!(from));
    assert_eq!(planned["blockers"], json!([]));
    assert_eq!(planned["waits"], json!([]));
    assert_eq!(planned["done"], json!(false));
    let tables = planned["tables"].as_array().expect("tables");
    assert!(
        tables.contains(&json!({ "table": "drafts", "rows": 1 }))
            && tables.contains(&json!({ "table": "activity", "rows": 1 })),
        "{planned}"
    );

    let (status, self_approved) = approve(&state, operator(&state, "ops-a"), &id).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{self_approved}");
    let (status, applied) = approve(&state, operator(&state, "ops-b"), &id).await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(applied["state"], json!("applied"));

    assert_eq!(
        drafts_under(&state, old, &project).await,
        0,
        "the old Organization's key reads nothing"
    );
    let new = joinedcontext_portal::project_ids::recorded(pool(&state), &project)
        .await
        .expect("registry")
        .expect("a key");
    assert_eq!(new.project, old.project, "the project keeps its id");
    assert_ne!(new.org, old.org);
    assert_eq!(drafts_under(&state, new, &project).await, 1);

    // Over HTTP, the bounce's ask: the old Organization's people lose the project, the new
    // Organization's administrator reads it and its keyed drafts.
    for reader in [format!("{from}-admin"), format!("{from}-member")] {
        for path in [&at, &drafts] {
            let (status, text) = read_as(&state, &reader, path).await;
            assert!(
                denied(status),
                "{reader} on {path} after the move: {status} {text}"
            );
        }
    }
    let (status, text) = read_as(&state, &format!("{to}-admin"), &at).await;
    assert_eq!(status, StatusCode::OK, "{to}-admin after the move: {text}");
    let (status, text) = read_as(&state, &format!("{to}-admin"), &drafts).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{to}-admin drafts after the move: {text}"
    );
    assert!(
        text.contains("\"air\""),
        "the moved draft reads under B: {text}"
    );
    assert_eq!(
        joinedcontext_portal::project_ids::key(pool(&state), &from, &project)
            .await
            .expect("registry"),
        None,
        "the old Organization keys the project no more"
    );
    assert_eq!(
        joinedcontext_portal::project_ids::key_of(pool(&state), &state.mirror, &project)
            .await
            .expect("registry"),
        Some(new),
        "a request finds the project under its new key"
    );

    let after: Value = sqlx::query_scalar(
        "SELECT after FROM operator_audit WHERE change_id = $1::uuid AND action = 'applied ProjectMove'",
    )
    .bind(&id)
    .fetch_one(pool(&state))
    .await
    .expect("the audit row");
    assert_eq!(after["rows"], json!(2), "{after}");
    assert_eq!(
        after["rows"], planned["rows"],
        "the dry run counts what the apply moves"
    );
    assert_eq!(
        after["tables"], planned["tables"],
        "{after} against {planned}"
    );
    assert_eq!(
        rows_under(&project, old).await,
        0,
        "no keyed row of the project keeps the old Organization's key"
    );
    assert!(
        !after.to_string().contains("air"),
        "no row content: {after}"
    );

    // A second move to the same Organization changes nothing.
    let (status, again) = plan(&state, operator(&state, "ops-a"), &project, &to).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["done"], json!(true));
    let (status, made) = propose(&state, operator(&state, "ops-a"), &project, &to).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let id = made["id"].as_str().expect("an id").to_owned();
    let (status, applied) = approve(&state, operator(&state, "ops-b"), &id).await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(drafts_under(&state, new, &project).await, 1);
}

/// A project named like the Organization it moves to (jc-dev's bbsk into bbsk) moves: the
/// Organization's reserved project never answers for a project's name (TEN-11).
#[tokio::test]
async fn a_project_named_like_its_new_organization_moves() {
    let state = world().await;
    let from = run("hel");
    let project = run("bbsk");
    let old = placed(&state, &project, &from, &project).await;
    manifest(&state, &project, &project);
    let (status, made) = propose(&state, operator(&state, "ops-a"), &project, &project).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let id = made["id"].as_str().expect("an id").to_owned();
    let (status, applied) = approve(&state, operator(&state, "ops-b"), &id).await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    let new = joinedcontext_portal::project_ids::recorded(pool(&state), &project)
        .await
        .expect("registry")
        .expect("a key");
    assert_eq!(drafts_under(&state, new, &project).await, 1);
    assert_eq!(drafts_under(&state, old, &project).await, 0);
    let reserved = joinedcontext_portal::project_ids::reserved(pool(&state), &project)
        .await
        .expect("registry")
        .expect("the reserved project");
    assert_ne!(reserved.project, new.project);
}

/// Only an operator with a second factor plans or asks for a move; the registry refuses a moved
/// Organization outside an approved move, and the keyed tables stay forced.
#[tokio::test]
async fn only_an_operator_moves_and_nothing_else_moves_a_project() {
    let state = world().await;
    let from = run("hel");
    let to = run("ba");
    let project = run("guarded");
    placed(&state, &project, &from, &to).await;
    manifest(&state, &project, &to);

    let mut password_only = operator(&state, "ops-a");
    password_only.second_factor = false;
    let administrator = common::organization_admin(&state.mirror, "org-admin");
    for who in [person("someone"), password_only, administrator] {
        let (status, refused) = propose(&state, who.clone(), &project, &to).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
        let (status, refused) = plan(&state, who, &project, &to).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    }
    let (status, refused) = propose(&state, operator(&state, "ops-a"), "Not A Name", &to).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    let (status, refused) = propose(&state, operator(&state, "ops-a"), &project, "nowhere").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(refused.to_string().contains("organization:"), "{refused}");
    let (status, refused) = plan(&state, operator(&state, "ops-a"), "no-such-project", &to).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");

    let moved = sqlx::query(
        "UPDATE project_ids SET organization = $2 WHERE name = $1 AND parent IS NULL AND NOT reserved",
    )
    .bind(&project)
    .bind(&to)
    .execute(pool(&state))
    .await;
    assert!(
        moved.is_err(),
        "the registry refuses a move outside a ProjectMove"
    );

    // The serving role's direct call moves nothing without an approved ProjectMove Change: no
    // Change, the proposer approving their own, or a replay of one applied already.
    let direct = |change: String, approver: String| {
        let pool = pool(&state).clone();
        async move {
            sqlx::query("SELECT * FROM project_move($1::uuid, $2, NULL)")
                .bind(change)
                .bind(approver)
                .execute(&pool)
                .await
        }
    };
    let unknown = direct(uuid::Uuid::new_v4().to_string(), "sub-ops-b".into()).await;
    assert!(unknown.is_err(), "no Change, no move");
    let (status, made) = propose(&state, operator(&state, "ops-a"), &project, &to).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let id = made["id"].as_str().expect("an id").to_owned();
    let self_approved = direct(
        id.clone(),
        made["proposedBy"].as_str().expect("a proposer").to_owned(),
    )
    .await;
    assert!(self_approved.is_err(), "the proposer never approves");
    let unnamed = direct(id.clone(), String::new()).await;
    assert!(unnamed.is_err(), "an approver is named");
    let (status, applied) = approve(&state, operator(&state, "ops-b"), &id).await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(applied["state"], json!("applied"), "{applied}");
    assert_eq!(applied["decidedBy"], json!("sub-ops-b"), "{applied}");
    let replay = direct(id.clone(), "sub-ops-c".into()).await;
    assert!(replay.is_err(), "an applied Change moves nothing again");
    let audited: i64 =
        sqlx::query_scalar("SELECT count(*) FROM operator_audit WHERE change_id = $1::uuid AND action = 'applied ProjectMove'")
            .bind(&id)
            .fetch_one(pool(&state))
            .await
            .expect("the audit");
    assert_eq!(audited, 1, "one audit row per applied move");

    // The tables stay forced: the move lifts FORCE only inside its own transaction.
    let unforced: Vec<String> = sqlx::query_scalar(
        "SELECT relname::text FROM pg_class WHERE relname IN ('drafts', 'activity') AND NOT relforcerowsecurity",
    )
    .fetch_all(pool(&state))
    .await
    .expect("catalog");
    assert_eq!(unforced, Vec::<String>::new());
}

/// T-7462 (jc-dev t6025-full, 2026-11-21): the gateway binds a policyRef on the domains of the
/// project and of the Organization its manifest names, so a move from `{from}.example` to
/// `{to}.example` left every `urn:ngsi-ld:Policy:{from}.example:...` granting nothing at the next
/// load. The plan waits for an Endpoint listing the target's URN beside the old one, the Project
/// Change naming the target is refused until then (its dry run says so), and the approval waits
/// too, even when the manifest names the target already; once the Endpoint lists both, the move
/// applies and nothing of it binds on the old domain alone.
#[tokio::test]
async fn a_move_waits_for_endpoints_whose_policy_refs_name_the_old_domain() {
    let state = world().await;
    let from = run("hel");
    let to = run("ctlb");
    let project = run("refs");
    people(&state, &from, &to);
    placed(&state, &project, &from, &to).await;
    let segment = joinedcontext_portal::spaces::segment(&state.mirror, &project, "air");
    let urn = |organization: &str| {
        format!("urn:ngsi-ld:Policy:{organization}.example:{segment}:public-read")
    };
    let m = &state.mirror;
    m.upsert(envelope("ContextSpace", "air", &project, json!({})));
    m.upsert(envelope(
        "Policy",
        "public-read",
        &project,
        json!({ "contextSpaceRef": { "kind": "ContextSpace", "name": "air" },
                "assignee": { "kind": "role", "id": "public" }, "operations": ["retrieveOps"] }),
    ));
    let endpoint = |refs: Vec<String>| {
        envelope(
            "Endpoint",
            "stations",
            &project,
            json!({ "contextSpaceRef": "air", "audience": "public", "policyRefs": refs,
                    "slug": "zt4qm7ge2xdv6ksb3ncf5arw2y", "enabledRepresentations": ["ngsi-ld"] }),
        )
    };
    m.upsert(endpoint(vec![urn(&from)]));

    let (status, made) = propose(&state, operator(&state, "ops-a"), &project, &to).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let id = made["id"].as_str().expect("an id").to_owned();
    let (status, planned) = plan(&state, operator(&state, "ops-a"), &project, &to).await;
    assert_eq!(status, StatusCode::OK, "{planned}");
    let waits = planned["waits"].to_string();
    assert!(
        waits.contains("Endpoint 'stations'")
            && waits.contains(&urn(&from))
            && waits.contains(&urn(&to)),
        "{planned}"
    );
    let check = || {
        let state = state.clone();
        let project = project.clone();
        let to = to.clone();
        async move {
            joinedcontext_portal::project_move::check_manifest(
                pool(&state),
                &state.mirror,
                &project,
                &json!({ "organizationRef": to }),
            )
            .await
            .expect("registry")
        }
    };
    let refused = check().await.expect_err("the Endpoint would grant nothing");
    assert!(
        refused.starts_with("spec.organizationRef:") && refused.contains(&urn(&to)),
        "{refused}"
    );
    // The Project door's dry run gives the same finding.
    let door = send(
        &state,
        operator(&state, "ops-a"),
        "PUT",
        &format!("/api/v1/projects/{ORG_NAMESPACE}/projects/{project}?dryRun=All"),
        Some(json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "Project",
            "metadata": { "name": project, "namespace": ORG_NAMESPACE },
            "spec": { "organizationRef": to }
        })),
    )
    .await;
    assert_eq!(door.status, StatusCode::OK, "{}", door.text);
    let finding = &body(&door.text)["verdict"]["findings"][0];
    assert_eq!(
        finding["path"],
        json!("spec.organizationRef"),
        "{}",
        door.text
    );
    assert!(
        finding["message"]
            .as_str()
            .is_some_and(|m| m.contains(&urn(&from))),
        "{}",
        door.text
    );
    // The Endpoint door lets the target's URN in because the move is asked for.
    assert_eq!(
        joinedcontext_portal::project_move::moving_domains(&state, &project)
            .await
            .expect("operator changes"),
        [format!("{to}.example")]
    );

    // A manifest that names the target already (a Change from before the check) still waits.
    manifest(&state, &project, &to);
    let (status, refused) = approve(&state, operator(&state, "ops-b"), &id).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(refused.to_string().contains(&urn(&to)), "{refused}");

    manifest(&state, &project, &from);
    m.upsert(endpoint(vec![urn(&from), urn(&to)]));
    assert_eq!(check().await, Ok(()), "the Endpoint grants under both");
    manifest(&state, &project, &to);
    let (status, applied) = approve(&state, operator(&state, "ops-b"), &id).await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert!(joinedcontext_portal::project_move::stray_policy_refs(
        &state.mirror,
        &project,
        Some(&to),
        &to
    )
    .is_empty());
    assert!(
        joinedcontext_portal::permissions::project_domains(&state.mirror, &project)
            .contains(&format!("{to}.example")),
        "the gateway binds the Endpoint's second URN"
    );
}

/// A forge holding one open Change of `project`, #5, which adds the Context Space `air`.
async fn forge_with_open_change(project: &str) -> MockServer {
    let shako = common::forge().await;
    let branch = "portal/create-contextspace-air-00000005";
    let pull = json!({
        "Number": 5,
        "HtmlUrl": "https://shako.example/pulls/5",
        "State": "open",
        "Title": "air",
        "Head": branch,
        "Base": "main",
        "CreatedAt": "2026-11-20T09:00:00Z",
        "Author": "author",
        "AuthorName": "Author",
        "AuthorEmail": "author@hel.fi",
        "Mergeable": true,
        "Merged": false
    });
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/PullRequests")))
        .and(query_param("$filter", "State eq 'open'"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": [pull] })))
        .mount(&shako)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/PullRequests(5)")))
        .respond_with(ResponseTemplate::new(200).set_body_json(pull))
        .mount(&shako)
        .await;
    let file = format!("projects/{project}/spaces/air/space.yaml");
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/PullRequests(5)/Files")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [{ "Filename": file, "Status": "added" }]
        })))
        .mount(&shako)
        .await;
    let space = serde_norway::to_string(&json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "ContextSpace",
        "metadata": { "name": "air", "namespace": project },
        "spec": {}
    }))
    .expect("yaml");
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/Shako.File(Path=@p,Ref=@r)")))
        .and(query_param("@p", format!("'{file}'")))
        .and(query_param("@r", format!("'{branch}'")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Sha": "blob-5",
            "Content": common::encode(&space)
        })))
        .mount(&shako)
        .await;
    shako
}

/// T-7462: a project with an open Change moves only once it is decided. The move may be asked
/// for, the plan names the Change, and the approval is refused naming it.
#[tokio::test]
async fn a_project_with_open_changes_waits_for_them() {
    let project = run("busy");
    let shako = forge_with_open_change(&project).await;
    let client = joinedcontext_portal::forge::ShakoClient::new(
        shako.uri().parse().expect("mock url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
    let state = world().await.with_shako(std::sync::Arc::new(client));
    let from = run("hel");
    let to = run("zilina");
    placed(&state, &project, &from, &to).await;
    manifest(&state, &project, &to);

    let (status, planned) = plan(&state, operator(&state, "ops-a"), &project, &to).await;
    assert_eq!(status, StatusCode::OK, "{planned}");
    assert!(
        planned["waits"].to_string().contains("chg-00000005"),
        "{planned}"
    );
    let (status, made) = propose(&state, operator(&state, "ops-a"), &project, &to).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let id = made["id"].as_str().expect("an id").to_owned();
    let (status, refused) = approve(&state, operator(&state, "ops-b"), &id).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(refused.to_string().contains("open Changes"), "{refused}");
    assert!(refused.to_string().contains("chg-00000005"), "{refused}");
}

/// T-7462: a Project manifest places the project where the registry files it, or where a
/// proposed ProjectMove takes it. Before the move is asked for, a manifest naming the target is
/// refused; once asked for it passes; after the move, one naming the old Organization or the
/// installation claim (jc-prod's forge says `hel` for its registry's `smartcity`) is refused.
#[tokio::test]
async fn a_project_manifest_names_the_organization_the_registry_files_it_under() {
    let state = world().await;
    let from = run("smartcity");
    let to = run("barcelona");
    let project = run("placed");
    let claim = run("hel");
    // The forge names the installation `claim`; the registry holds it as `from`.
    state.mirror.set_configured_installation(&from);
    state.mirror.upsert(envelope(
        "Organization",
        &claim,
        ORG_NAMESPACE,
        json!({ "installation": true }),
    ));
    placed(&state, &project, &from, &to).await;
    let check = |named: String| {
        let state = state.clone();
        let project = project.clone();
        async move {
            joinedcontext_portal::project_move::check_manifest(
                pool(&state),
                &state.mirror,
                &project,
                &json!({ "organizationRef": named }),
            )
            .await
            .expect("registry")
        }
    };
    assert_eq!(check(from.clone()).await, Ok(()));
    assert_eq!(
        check(claim.clone()).await,
        Ok(()),
        "the claim is the installation"
    );
    let refused = check(to.clone()).await.expect_err("no move asked for");
    assert!(
        refused.starts_with("spec.organizationRef:")
            && refused.contains(&from)
            && refused.contains(&to),
        "{refused}"
    );

    // The Project door answers the same refusal at its dry run, naming the field.
    let door = send(
        &state,
        operator(&state, "ops-a"),
        "PUT",
        &format!("/api/v1/projects/{ORG_NAMESPACE}/projects/{project}?dryRun=All"),
        Some(json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "Project",
            "metadata": { "name": project, "namespace": ORG_NAMESPACE },
            "spec": { "organizationRef": to }
        })),
    )
    .await;
    assert_eq!(door.status, StatusCode::OK, "{}", door.text);
    let finding = &body(&door.text)["verdict"]["findings"][0];
    assert_eq!(
        finding["path"],
        json!("spec.organizationRef"),
        "{}",
        door.text
    );
    assert!(
        finding["message"]
            .as_str()
            .is_some_and(|m| m.starts_with("the registry files project")),
        "{}",
        door.text
    );

    let (status, made) = propose(&state, operator(&state, "ops-a"), &project, &to).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let id = made["id"].as_str().expect("an id").to_owned();
    assert_eq!(check(to.clone()).await, Ok(()), "the move is asked for");
    manifest(&state, &project, &to);
    let (status, applied) = approve(&state, operator(&state, "ops-b"), &id).await;
    assert_eq!(status, StatusCode::OK, "{applied}");

    assert_eq!(check(to.clone()).await, Ok(()));
    for named in [from.clone(), claim.clone()] {
        let refused = check(named.clone()).await.expect_err("moved away");
        assert!(
            refused.contains(&to) && refused.contains(&named),
            "{refused}"
        );
    }
}
