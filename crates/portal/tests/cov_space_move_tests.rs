//! The failure arms of the space move job: each step that can fail says so and gives the space
//! back (PF-115, T-7823).

mod common;

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::REPO;
use joinedcontext_portal::forge::ShakoClient;
use joinedcontext_portal::space_move::{Move, MoveState, Moves, Plan};

const MANIFEST: &str = "projects/ovzdusie/spaces/air/space.yaml";
const FRESH: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  \
                     name: air\n  namespace: ovzdusie\nspec:\n  title: Air\n";
const HELD: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  \
                    name: air\n  namespace: ovzdusie\n  annotations:\n    \
                    joinedcontext.com/moving-to: other\nspec:\n  title: Air\n";
const EXPORT: &str = "{\"antaresExport\":1}\n{\"end\":true,\"records\":0}\n";
const TENANT: &str = "/q/tenants/ovzdusie-air";
const DEAD: &str = "http://127.0.0.1:1";

fn digest(entities: u64) -> (u16, String) {
    (
        200,
        json!({"entities": entities, "attrInstances": entities * 3, "sha256": "ab".repeat(32)})
            .to_string(),
    )
}

/// What each peer of one move answers; the defaults are a move that succeeds.
struct Rig {
    manifest: Option<&'static str>,
    forge_down: bool,
    commit: u16,
    held: bool,
    gateways: Option<Vec<String>>,
    source: Option<&'static str>,
    target: Option<&'static str>,
    export: u16,
    import: u16,
    source_digest: (u16, String),
    target_first: Option<(u16, String)>,
    target_digest: (u16, String),
    purge: u16,
    patience: Duration,
}

fn rig() -> Rig {
    Rig {
        manifest: Some(FRESH),
        forge_down: false,
        commit: 201,
        held: true,
        gateways: None,
        source: None,
        target: None,
        export: 200,
        import: 204,
        source_digest: digest(1),
        target_first: Some(digest(0)),
        target_digest: digest(1),
        purge: 204,
        patience: Duration::from_millis(300),
    }
}

struct Fixture {
    moves: Arc<Moves>,
    plan: Plan,
    _servers: Vec<MockServer>,
}

async fn fixture(rig: Rig) -> Fixture {
    let forge = if rig.forge_down {
        MockServer::start().await
    } else {
        common::forge().await
    };
    if let Some(manifest) = rig.manifest {
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/Shako.File(Path=@p,Ref=@r)")))
            .and(query_param("@p", format!("'{MANIFEST}'")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Sha": "blob-1",
                "Content": base64::engine::general_purpose::STANDARD.encode(manifest)
            })))
            .with_priority(1)
            .mount(&forge)
            .await;
    }
    if rig.commit != 201 {
        Mock::given(method("POST"))
            .and(path(format!("{REPO}/Shako.CommitFiles")))
            .respond_with(ResponseTemplate::new(rig.commit))
            .with_priority(1)
            .mount(&forge)
            .await;
    }
    let gateway = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/metrics"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            "jc_gateway_space_held{{space=\"ovzdusie-air\"}} {}\n",
            u8::from(rig.held)
        )))
        .mount(&gateway)
        .await;
    let source = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("{TENANT}/export")))
        .respond_with(ResponseTemplate::new(rig.export).set_body_string(EXPORT))
        .mount(&source)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{TENANT}/digest")))
        .respond_with(
            ResponseTemplate::new(rig.source_digest.0).set_body_string(rig.source_digest.1),
        )
        .mount(&source)
        .await;
    let target = MockServer::start().await;
    if let Some((status, body)) = rig.target_first {
        Mock::given(method("GET"))
            .and(path(format!("{TENANT}/digest")))
            .respond_with(ResponseTemplate::new(status).set_body_string(body))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&target)
            .await;
    }
    Mock::given(method("GET"))
        .and(path(format!("{TENANT}/digest")))
        .respond_with(
            ResponseTemplate::new(rig.target_digest.0).set_body_string(rig.target_digest.1),
        )
        .mount(&target)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{TENANT}/import")))
        .respond_with(ResponseTemplate::new(rig.import))
        .mount(&target)
        .await;
    Mock::given(method("DELETE"))
        .and(path(TENANT))
        .respond_with(ResponseTemplate::new(rig.purge))
        .mount(&target)
        .await;
    let client = ShakoClient::new(
        forge.uri().parse().expect("mock url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
    let plan = Plan {
        project: "ovzdusie".into(),
        space: "air".into(),
        tenant: "ovzdusie-air".into(),
        from: "main".into(),
        to: "other".into(),
        source: rig.source.map_or_else(|| source.uri(), str::to_owned),
        target: rig.target.map_or_else(|| target.uri(), str::to_owned),
        token: "admin-token".into(),
        gateways: rig.gateways.unwrap_or_else(|| vec![gateway.uri()]),
        forge: Arc::new(client),
        path: MANIFEST.into(),
        by: "jana".into(),
    };
    Fixture {
        moves: Arc::new(Moves::new(Duration::from_millis(10), rig.patience)),
        plan,
        _servers: vec![forge, gateway, source, target],
    }
}

/// Starts the move and waits for it to end (the last answer is returned either way).
async fn run(fixture: Fixture) -> Move {
    let key = format!("{}/{}", fixture.plan.project, fixture.plan.space);
    fixture.moves.start(fixture.plan).expect("started");
    wait(&fixture.moves, &key).await
}

async fn wait(moves: &Moves, key: &str) -> Move {
    for _ in 0..400 {
        let now = moves.get(key).expect("the move is known");
        if matches!(now.state, MoveState::Done | MoveState::Failed) {
            return now;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    moves.get(key).expect("the move is known")
}

fn reason(done: &Move) -> String {
    done.reason.clone().unwrap_or_default()
}

#[tokio::test]
async fn a_move_that_meets_no_trouble_is_done_with_both_digests() {
    let done = run(fixture(rig()).await).await;
    assert_eq!(done.state, MoveState::Done, "{:?}", done.reason);
    assert!(done.reason.is_none());
    assert_eq!(done.source, done.target);
    assert!(done.ended_at.is_some());
}

#[tokio::test]
async fn a_space_that_is_moving_is_not_started_again_until_that_move_ends() {
    let held_never = Rig {
        held: false,
        patience: Duration::from_millis(400),
        ..rig()
    };
    let fixture = fixture(held_never).await;
    let key = "ovzdusie/air".to_owned();
    let again = Plan {
        by: "pavel".into(),
        ..clone_plan(&fixture.plan)
    };
    let first = fixture.moves.start(fixture.plan).expect("started");
    assert_eq!(first.state, MoveState::Holding);
    let refused = fixture.moves.start(again).expect_err("already moving");
    assert!(
        refused.contains("'ovzdusie/air' is already moving"),
        "{refused}"
    );
    let ended = wait(&fixture.moves, &key).await;
    assert_eq!(ended.state, MoveState::Failed);
    assert!(reason(&ended).contains("not every gateway pod took the hold"));
    assert!(fixture.moves.get("ovzdusie/other").is_none());
}

#[tokio::test]
async fn a_failed_copy_that_cannot_be_undone_says_what_stayed() {
    // The hold is already on the manifest (a resumed move), the digests differ, the target
    // refuses the purge and the repository refuses the release.
    let done = run(fixture(Rig {
        manifest: Some(HELD),
        commit: 500,
        target_first: None,
        target_digest: digest(2),
        purge: 500,
        ..rig()
    })
    .await)
    .await;
    assert_eq!(done.state, MoveState::Failed);
    let said = reason(&done);
    assert!(said.contains("the copy does not match"), "{said}");
    assert!(said.contains("the target's copy was not purged"), "{said}");
    assert!(said.contains("the broker refused the purge"), "{said}");
    assert!(said.contains("the hold was not released"), "{said}");
    assert!(done.source.is_some() && done.target.is_some());
}

#[tokio::test]
async fn a_repository_that_is_down_fails_the_move_before_anything_is_held() {
    let done = run(fixture(Rig {
        forge_down: true,
        ..rig()
    })
    .await)
    .await;
    assert_eq!(done.state, MoveState::Failed);
    let said = reason(&done);
    assert!(
        said.starts_with("the repository refused the commit"),
        "{said}"
    );
    assert!(said.contains("the hold was not released"), "{said}");
}

#[tokio::test]
async fn a_space_whose_manifest_is_not_in_the_repository_fails_the_move() {
    let done = run(fixture(Rig {
        manifest: None,
        ..rig()
    })
    .await)
    .await;
    assert_eq!(done.state, MoveState::Failed);
    assert!(
        reason(&done).contains("is not in the repository"),
        "{}",
        reason(&done)
    );
}

#[tokio::test]
async fn a_manifest_that_does_not_parse_as_a_mapping_fails_the_move() {
    let done = run(fixture(Rig {
        manifest: Some("- just\n- a list\n"),
        ..rig()
    })
    .await)
    .await;
    assert_eq!(done.state, MoveState::Failed);
    assert!(
        reason(&done).contains("the space's manifest does not parse"),
        "{}",
        reason(&done)
    );
}

#[tokio::test]
async fn a_manifest_whose_metadata_and_spec_are_no_mappings_is_repaired_by_the_move() {
    let done = run(fixture(Rig {
        manifest: Some("metadata: text\nspec: 3\n"),
        ..rig()
    })
    .await)
    .await;
    assert_eq!(done.state, MoveState::Done, "{:?}", done.reason);
}

#[tokio::test]
async fn a_gateway_that_does_not_resolve_or_does_not_answer_never_holds() {
    for gateways in [
        vec!["http://no-such-gateway.invalid:9".to_owned()],
        Vec::new(),
        vec![DEAD.to_owned()],
        vec!["not a url".to_owned()],
    ] {
        let done = run(fixture(Rig {
            gateways: Some(gateways.clone()),
            patience: Duration::from_millis(100),
            ..rig()
        })
        .await)
        .await;
        assert_eq!(done.state, MoveState::Failed, "{gateways:?}");
        assert!(
            reason(&done).contains("not every gateway pod took the hold"),
            "{gateways:?}: {}",
            reason(&done)
        );
    }
}

#[tokio::test]
async fn a_gateway_that_answers_an_error_status_never_holds() {
    let rig = Rig {
        patience: Duration::from_millis(100),
        ..rig()
    };
    let fixture = fixture(rig).await;
    let broken = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/metrics"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&broken)
        .await;
    let plan = Plan {
        gateways: vec![broken.uri()],
        ..clone_plan(&fixture.plan)
    };
    let key = "ovzdusie/air";
    fixture.moves.start(plan).expect("started");
    let done = wait(&fixture.moves, key).await;
    assert_eq!(done.state, MoveState::Failed);
    assert!(reason(&done).contains("not every gateway pod took the hold"));
}

fn clone_plan(plan: &Plan) -> Plan {
    Plan {
        project: plan.project.clone(),
        space: plan.space.clone(),
        tenant: plan.tenant.clone(),
        from: plan.from.clone(),
        to: plan.to.clone(),
        source: plan.source.clone(),
        target: plan.target.clone(),
        token: plan.token.clone(),
        gateways: plan.gateways.clone(),
        forge: Arc::clone(&plan.forge),
        path: plan.path.clone(),
        by: plan.by.clone(),
    }
}

#[tokio::test]
async fn a_source_that_is_down_or_refuses_the_export_fails_the_copy() {
    let down = run(fixture(Rig {
        source: Some(DEAD),
        target_first: Some(digest(0)),
        ..rig()
    })
    .await)
    .await;
    assert_eq!(down.state, MoveState::Failed);
    assert!(
        reason(&down).contains("the source broker did not answer the export"),
        "{}",
        reason(&down)
    );
    let refused = run(fixture(Rig {
        export: 500,
        ..rig()
    })
    .await)
    .await;
    assert!(
        reason(&refused).contains("the source broker refused the export: 500"),
        "{}",
        reason(&refused)
    );
}

#[tokio::test]
async fn a_target_that_is_down_or_refuses_the_import_fails_the_copy() {
    let down = run(fixture(Rig {
        manifest: Some(HELD),
        target: Some(DEAD),
        target_first: None,
        ..rig()
    })
    .await)
    .await;
    assert_eq!(down.state, MoveState::Failed);
    assert!(
        reason(&down).contains("the target broker did not take the import"),
        "{}",
        reason(&down)
    );
    let refused = run(fixture(Rig {
        import: 507,
        ..rig()
    })
    .await)
    .await;
    assert!(
        reason(&refused).contains("the target broker refused the import: 507"),
        "{}",
        reason(&refused)
    );
}

#[tokio::test]
async fn a_digest_that_cannot_be_had_or_read_fails_the_move() {
    let down = run(fixture(Rig {
        target: Some(DEAD),
        ..rig()
    })
    .await)
    .await;
    assert!(
        reason(&down).contains("a broker did not answer its digest"),
        "{}",
        reason(&down)
    );
    let refused = run(fixture(Rig {
        target_first: Some((500, String::new())),
        ..rig()
    })
    .await)
    .await;
    assert!(
        reason(&refused).contains("a broker refused its digest: 500"),
        "{}",
        reason(&refused)
    );
    let unreadable = run(fixture(Rig {
        source_digest: (200, "not json".to_owned()),
        ..rig()
    })
    .await)
    .await;
    assert!(
        reason(&unreadable).contains("a broker's digest does not read"),
        "{}",
        reason(&unreadable)
    );
}

#[tokio::test]
async fn a_target_that_already_holds_data_keeps_it_and_the_move_fails() {
    let done = run(fixture(Rig {
        target_first: Some(digest(5)),
        ..rig()
    })
    .await)
    .await;
    assert_eq!(done.state, MoveState::Failed);
    assert!(
        reason(&done).contains("already holds 5 entities"),
        "{}",
        reason(&done)
    );
}
