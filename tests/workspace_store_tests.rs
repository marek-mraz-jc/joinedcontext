//! The workspace registry: one record per branch, unique by name, gone when its TTL is
//! (CC-76, CC-81; T-1232).

use chrono::{Duration, Utc};
use joinedcontext_portal::ops::workspaces::{
    Opening, PreviewAccess, PreviewState, Scope, ScopedResource, WorkspaceError, WorkspaceStore,
};

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
    let store = WorkspaceStore::new(None);
    let made = store
        .create(opening("bikes-v2", "helsinki", 24))
        .await
        .expect("created");
    assert_eq!(made.branch(), "workspace/bikes-v2");
    assert_eq!(made.preview_state, PreviewState::None);
    assert!(made.expires_at - made.created_at == Duration::hours(24));

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
    let store = WorkspaceStore::new(None);
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
    let store = WorkspaceStore::new(None);
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
    let store = WorkspaceStore::new(None);
    store.create(opening("short", "helsinki", 1)).await.unwrap();
    store.create(opening("long", "helsinki", 48)).await.unwrap();
    let later = Utc::now() + Duration::hours(2);
    let expired: Vec<String> = store
        .expired(later)
        .await
        .unwrap()
        .into_iter()
        .map(|w| w.name)
        .collect();
    assert_eq!(expired, ["short"]);
    assert!(store.expired(Utc::now()).await.unwrap().is_empty());
}

#[tokio::test]
async fn names_ttls_and_scopes_outside_the_rules_are_refused() {
    let store = WorkspaceStore::new(None);
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
    let mut stores = vec![WorkspaceStore::new(None)];
    match std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
    {
        Some(url) => stores.push(WorkspaceStore::new(Some(
            joinedcontext_portal::db::connect(&url)
                .await
                .expect("the test database answers and migrates"),
        ))),
        None => eprintln!("skipped on PostgreSQL: JC_PORTAL_TEST_DATABASE_URL is not set"),
    }
    let name = format!(
        "p{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() % 1_000_000_000)
            .unwrap_or_default()
    );
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
