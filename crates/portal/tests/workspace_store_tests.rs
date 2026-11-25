//! The workspace registry: one record per branch, unique by name, gone when its TTL is
//! (CC-76, CC-81; T-1232).

use jc_time::TimestampTz;
use joinedcontext_portal::ops::workspaces::{
    Opening, PreviewAccess, PreviewState, Scope, ScopedResource, WorkspaceError, WorkspaceStore,
};

mod common;
use common::unique;

fn opening<'a>(name: &'a str, project: &'a str, ttl_hours: i64) -> Opening<'a> {
    Opening {
        name,
        title: None,
        project,
        owner: "demo.steward@hel.fi",
        base_revision: "8c56954a1f0e2b3c4d5e6f708192a3b4c5d6e7f8",
        scope: Scope::Project {},
        ttl_hours,
    }
}

#[tokio::test]
async fn a_workspace_is_created_read_listed_and_deleted() {
    let store = WorkspaceStore::new(None, Default::default());
    let made = store
        .create(opening("bikes-v2", "helsinki", 24))
        .await
        .expect("created");
    assert_eq!(made.branch(), "workspace/bikes-v2");
    assert_eq!(made.preview_state, PreviewState::None);
    assert!(made.expires_at - made.created_at == jiff::SignedDuration::from_hours(24));

    assert_eq!(store.get("bikes-v2").await.unwrap(), Some(made.clone()));
    assert_eq!(store.live("bikes-v2").await.unwrap(), made);
    store.create(opening("kpi", "helsinki", 1)).await.unwrap();
    store.create(opening("other", "espoo", 1)).await.unwrap();
    let names: Vec<String> = store
        .list("helsinki")
        .await
        .unwrap()
        .into_iter()
        .map(|w| w.name)
        .collect();
    assert_eq!(names, ["bikes-v2", "kpi"]);

    store
        .set_preview_state("bikes-v2", PreviewState::Running)
        .await
        .unwrap();
    assert_eq!(
        store.get("bikes-v2").await.unwrap().unwrap().preview_state,
        PreviewState::Running
    );

    store.delete("bikes-v2").await.expect("deleted");
    assert_eq!(store.get("bikes-v2").await.unwrap(), None);
}

#[tokio::test]
async fn a_second_workspace_of_the_same_name_is_a_conflict_across_projects() {
    let store = WorkspaceStore::new(None, Default::default());
    store
        .create(opening("shared", "helsinki", 2))
        .await
        .unwrap();
    let err = store
        .create(opening("shared", "espoo", 2))
        .await
        .unwrap_err();
    assert!(matches!(err, WorkspaceError::Conflict(_)), "{err}");
}

#[tokio::test]
async fn an_unknown_name_is_not_found_everywhere() {
    let store = WorkspaceStore::new(None, Default::default());
    assert_eq!(store.get("nope").await.unwrap(), None);
    assert!(matches!(
        store.live("nope").await,
        Err(WorkspaceError::NotFound(_))
    ));
    assert!(matches!(
        store.delete("nope").await,
        Err(WorkspaceError::NotFound(_))
    ));
    assert!(matches!(
        store.set_preview_state("nope", PreviewState::Running).await,
        Err(WorkspaceError::NotFound(_))
    ));
}

#[tokio::test]
async fn an_expired_workspace_is_listed_for_the_reaper_and_takes_nothing_more() {
    let store = WorkspaceStore::new(None, Default::default());
    store.create(opening("short", "helsinki", 1)).await.unwrap();
    store.create(opening("long", "helsinki", 48)).await.unwrap();
    let later = TimestampTz::now() + jiff::SignedDuration::from_hours(2);
    let expired: Vec<String> = store
        .expired(later)
        .await
        .unwrap()
        .into_iter()
        .map(|w| w.name)
        .collect();
    assert_eq!(expired, ["short"]);
    assert!(store.expired(TimestampTz::now()).await.unwrap().is_empty());
}

#[tokio::test]
async fn names_ttls_and_scopes_outside_the_rules_are_refused() {
    let store = WorkspaceStore::new(None, Default::default());
    for name in [
        "",
        "Bikes",
        "a-name-longer-than-twenty",
        "under_score",
        "-x",
    ] {
        assert!(
            matches!(
                store.create(opening(name, "helsinki", 1)).await,
                Err(WorkspaceError::Invalid(_))
            ),
            "{name:?}"
        );
    }
    for ttl in [0, -1, 14 * 24 + 1] {
        assert!(
            matches!(
                store.create(opening("ttl", "helsinki", ttl)).await,
                Err(WorkspaceError::Invalid(_))
            ),
            "{ttl}"
        );
    }
    let mut empty = opening("empty", "helsinki", 1);
    empty.scope = Scope::Resources { items: Vec::new() };
    assert!(matches!(
        store.create(empty).await,
        Err(WorkspaceError::Invalid(_))
    ));
    let mut bad_space = opening("space", "helsinki", 1);
    bad_space.scope = Scope::Space {
        name: "Not A Name".into(),
    };
    assert!(matches!(
        store.create(bad_space).await,
        Err(WorkspaceError::Invalid(_))
    ));
    let mut unknown = opening("res", "helsinki", 1);
    unknown.scope = Scope::Resources {
        items: vec![ScopedResource {
            kind: "Nope".into(),
            name: "x".into(),
        }],
    };
    assert!(
        matches!(store.create(unknown).await, Err(WorkspaceError::Invalid(_))),
        "a kind nobody serves"
    );
    let mut good = opening("res", "helsinki", 1);
    good.scope = Scope::Resources {
        items: vec![ScopedResource {
            kind: "Pipeline".into(),
            name: "bikes-ingest".into(),
        }],
    };
    store.create(good).await.expect("a served kind and a name");
}

#[test]
fn a_scope_reads_the_shape_the_api_documents() {
    let space: Scope =
        serde_json::from_value(serde_json::json!({ "kind": "space", "name": "helsinki" })).unwrap();
    assert!(space.covers("Pipeline", "bikes", Some("helsinki")));
    assert!(space.covers("ContextSpace", "helsinki", None));
    assert!(!space.covers("Pipeline", "bikes", Some("hub")));
    let items: Scope = serde_json::from_value(serde_json::json!({ "kind": "resources", "items": [{ "kind": "Pipeline", "name": "bikes-ingest" }] })).unwrap();
    assert!(items.covers("Pipeline", "bikes-ingest", None));
    assert!(!items.covers("Endpoint", "bikes-ingest", None));
    assert_eq!(
        serde_json::to_value(Scope::Project {}).unwrap(),
        serde_json::json!({ "kind": "project" })
    );
    assert!(
        serde_json::from_value::<Scope>(serde_json::json!({ "kind": "project", "extra": 1 }))
            .is_err()
    );
}

/// T-5760 (CC-91, CC-92): a start stores who started the preview and its salt, the previewing
/// list hands both back, and the next start replaces them; on PostgreSQL when
/// `JC_PORTAL_TEST_DATABASE_URL` is set, in memory always.
#[tokio::test]
async fn a_preview_start_keeps_its_author_and_salt_until_the_next_start() {
    let mut stores = vec![WorkspaceStore::new(None, Default::default())];
    match std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
    {
        Some(url) => {
            let pool = joinedcontext_portal::db::connect(&url)
                .await
                .expect("the test database answers and migrates");
            let mirror = common::keyed_mirror(&pool, &["helsinki"]).await;
            stores.push(WorkspaceStore::new(Some(pool), mirror));
        }
        None => eprintln!("skipped on PostgreSQL: JC_PORTAL_TEST_DATABASE_URL is not set"),
    }
    // A workspace name is a DNS label of at most 20 characters: the unique tail, the low 40 bits
    // of the clock with the process id and counter.
    let name = format!("p{}", &unique::unique()[7..]);
    for store in stores {
        store.create(opening(&name, "helsinki", 1)).await.unwrap();
        assert_eq!(store.get(&name).await.unwrap().unwrap().preview, None);
        let first = PreviewAccess {
            salt: "Q3vR8kM2pL9xT4nW7yB1cZ".into(),
            author: "3f2a9c1e-7b4d-4e8a-9c61-0d5e2b7f8a14".into(),
        };
        store.start_preview(&name, first.clone()).await.unwrap();
        let listed = store.previewing().await.unwrap();
        let mine = listed.iter().find(|w| w.name == name).expect("it previews");
        assert_eq!(mine.preview_state, PreviewState::Starting);
        assert_eq!(mine.preview.as_ref(), Some(&first));
        let again = PreviewAccess {
            salt: "z9Y8x7W6v5U4t3S2r1Q0pO".into(),
            ..first.clone()
        };
        store.start_preview(&name, again.clone()).await.unwrap();
        assert_eq!(
            store.get(&name).await.unwrap().unwrap().preview,
            Some(again)
        );
        assert!(
            !format!("{first:?}").contains(&first.salt),
            "Debug hides the salt"
        );
        assert!(matches!(
            store.start_preview("nobody-here", first).await,
            Err(WorkspaceError::NotFound(_))
        ));
        store.delete(&name).await.unwrap();
    }
}

/// Every store, in memory and (when `JC_PORTAL_TEST_DATABASE_URL` is set) on PostgreSQL: a
/// second store on the same database knows no names yet and finds each workspace by looking.
async fn stores() -> Vec<(WorkspaceStore, WorkspaceStore)> {
    let memory = WorkspaceStore::new(None, Default::default());
    let mut stores = vec![(memory.clone(), memory)];
    if let Some(url) = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
    {
        let pool = joinedcontext_portal::db::connect(&url)
            .await
            .expect("the test database answers and migrates");
        let mirror = common::keyed_mirror(&pool, &["helsinki"]).await;
        stores.push((
            WorkspaceStore::new(Some(pool.clone()), mirror.clone()),
            WorkspaceStore::new(Some(pool), mirror),
        ));
    }
    stores
}

/// The registry's whole life on each store: a taken name is a conflict, a list holds the
/// project's workspaces, every preview state is written and read back, a base revision moves,
/// the reaper sees a workspace once its TTL has passed, and a name nobody holds is not found by
/// any write.
#[tokio::test]
async fn a_workspace_moves_through_every_state_and_an_unknown_one_is_not_found() {
    for (store, fresh) in stores().await {
        let name = format!("s{}", &unique::unique()[7..]);
        let made = store.create(opening(&name, "helsinki", 1)).await.unwrap();
        assert!(matches!(
            store.create(opening(&name, "helsinki", 1)).await,
            Err(WorkspaceError::Conflict(taken)) if taken == name
        ));
        let listed = fresh.list("helsinki").await.unwrap();
        assert!(listed.iter().any(|w| w == &made), "{listed:?}");
        assert!(fresh.list("nowhere-at-all").await.unwrap().is_empty());

        for state in [
            PreviewState::Starting,
            PreviewState::Running,
            PreviewState::Stopped,
            PreviewState::Error,
            PreviewState::None,
        ] {
            fresh.set_preview_state(&name, state).await.unwrap();
            assert_eq!(store.live(&name).await.unwrap().preview_state, state);
        }
        let moved = "0a1b2c3d4e5f60718293a4b5c6d7e8f901234567";
        store.set_base_revision(&name, moved).await.unwrap();
        assert_eq!(
            fresh.get(&name).await.unwrap().unwrap().base_revision,
            moved
        );

        let later = TimestampTz::now() + jiff::SignedDuration::from_hours(2);
        let gone = fresh.expired(later).await.unwrap();
        assert!(gone.iter().any(|w| w.name == name), "{gone:?}");
        let now = fresh.expired(TimestampTz::now()).await.unwrap();
        assert!(!now.iter().any(|w| w.name == name), "{now:?}");

        let nobody = "nobody-holds-this";
        assert!(matches!(
            store.set_preview_state(nobody, PreviewState::Running).await,
            Err(WorkspaceError::NotFound(n)) if n == nobody
        ));
        assert!(matches!(
            store.set_base_revision(nobody, moved).await,
            Err(WorkspaceError::NotFound(n)) if n == nobody
        ));
        assert!(matches!(
            fresh.live(nobody).await,
            Err(WorkspaceError::NotFound(n)) if n == nobody
        ));
        assert_eq!(store.delete(&name).await.unwrap().name, name);
        assert!(matches!(
            fresh.delete(&name).await,
            Err(WorkspaceError::NotFound(n)) if n == name
        ));
    }
}

/// A workspace of a project the database holds no key for is refused as unplaced, before
/// anything is written (STO-07).
#[tokio::test]
async fn a_workspace_of_an_unkeyed_project_is_unplaced() {
    for (store, _) in stores().await.into_iter().skip(1) {
        let name = format!("u{}", &unique::unique()[7..]);
        assert!(matches!(
            store.create(opening(&name, "no-key-here", 1)).await,
            Err(WorkspaceError::Unplaced(project)) if project == "no-key-here"
        ));
        assert_eq!(store.get(&name).await.unwrap(), None);
    }
}

/// Each registry refusal answers as the API's problem of its kind, with the message a person
/// reads: an unplaced project is not found, naming the step that places it.
#[test]
fn each_workspace_refusal_is_the_api_problem_of_its_kind() {
    use joinedcontext_portal::error::ApiError;
    let api = |err: WorkspaceError| ApiError::from(err);
    assert!(
        matches!(api(WorkspaceError::Invalid("bad".into())), ApiError::BadRequest(m) if m == "bad")
    );
    assert!(matches!(
        api(WorkspaceError::Conflict("kpi".into())),
        ApiError::Conflict(m) if m == "a workspace named 'kpi' already exists"
    ));
    assert!(matches!(
        api(WorkspaceError::NotFound("kpi".into())),
        ApiError::NotFound(m) if m == "no workspace named 'kpi'"
    ));
    assert!(matches!(api(WorkspaceError::Db("down".into())), ApiError::Internal(m) if m == "down"));
    assert!(matches!(
        api(WorkspaceError::Unplaced("doprava".into())),
        ApiError::NotFound(m) if m.starts_with("the project doprava belongs to no active Organization")
            && m.ends_with("on the Organizations page first")
    ));
}
