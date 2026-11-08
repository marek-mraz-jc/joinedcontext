#![forbid(unsafe_code)]
//! Shared multi-tenant isolation test suite run against both `shako-store-memory`
//! and `shako-store-pg` (S2, T6).

use crate::ids::{InstanceId, OrgId, PackageId, RepoId, WebhookId};
use crate::model::*;
use crate::store::Store;
use chrono::Utc;
use serde_json::json;

/// Runs the shared multi-tenant store isolation and invariant suite against `store` (S2).
pub async fn run_isolation_suite(store: &dyn Store) {
    store
        .verify_rls_enforced()
        .await
        .expect("RLS must be enforced at boot");

    let inst = InstanceId::new();
    let org_a = OrgId::new();
    let org_b = OrgId::new();
    let now = Utc::now();

    store
        .upsert_instance(InstanceRecord::default_for(inst, org_a, "git.city.example"))
        .await
        .expect("instance a");
    store
        .upsert_instance(InstanceRecord::default_for(inst, org_b, "git.city.example"))
        .await
        .expect("instance b");

    let repo_a_id = RepoId::new();
    let repo_b_id = RepoId::new();

    // 1. Create repos in org_a and org_b
    let repo_a = store
        .create_repo(RepoRecord {
            id: repo_a_id,
            instance_id: inst,
            org_id: org_a,
            project_id: "helsinki".into(),
            owner: "helsinki".into(),
            name: "configuration".into(),
            description: "Org A Config".into(),
            default_branch: "main".into(),
            private: true,
            archived: false,
            size_bytes: 1024,
            next_pr_number: 1,
            created_at: now,
            updated_at: now,
        })
        .await
        .expect("create repo_a");

    let _repo_b = store
        .create_repo(RepoRecord {
            id: repo_b_id,
            instance_id: inst,
            org_id: org_b,
            project_id: "bbsk".into(),
            owner: "bbsk".into(),
            name: "secret-repo".into(),
            description: "Org B Confidential".into(),
            default_branch: "main".into(),
            private: true,
            archived: false,
            size_bytes: 2048,
            next_pr_number: 1,
            created_at: now,
            updated_at: now,
        })
        .await
        .expect("create repo_b");

    // Duplicate repo in org_a returns 409 Conflict
    let dup_err = store
        .create_repo(RepoRecord {
            id: RepoId::new(),
            ..repo_a.clone()
        })
        .await
        .expect_err("duplicate repo must conflict");
    assert_eq!(dup_err.status_code(), 409);

    // 2. Cross-org isolation: org_a sees only its own repo, never org_b's by id, name, or list
    let list_a = store.list_repos(inst, org_a).await.expect("list_a");
    assert_eq!(list_a.len(), 1);
    assert_eq!(list_a[0].id, repo_a_id);

    assert!(
        store
            .get_repo_by_id(inst, org_a, repo_b_id)
            .await
            .expect("get_by_id cross-org")
            .is_none()
    );
    assert!(
        store
            .get_repo_by_name(inst, org_a, "bbsk", "secret-repo")
            .await
            .expect("get_by_name cross-org")
            .is_none()
    );

    // 3. Repo rename updates metadata without changing `repo_a.id` (so no disk file moves, S2)
    let mut renamed = repo_a.clone();
    renamed.name = "configuration-v2".into();
    let updated = store.update_repo(renamed).await.expect("rename");
    assert_eq!(updated.id, repo_a_id);
    assert!(
        store
            .get_repo_by_name(inst, org_a, "helsinki", "configuration-v2")
            .await
            .expect("lookup renamed")
            .is_some()
    );

    // 4. Monotonic PR numbers per repo
    let n1 = store
        .allocate_pr_number(inst, org_a, repo_a_id)
        .await
        .expect("pr 1");
    let n2 = store
        .allocate_pr_number(inst, org_a, repo_a_id)
        .await
        .expect("pr 2");
    assert_eq!((n1, n2), (1, 2));

    let pr_a = store
        .create_pull_request(PullRequestRecord {
            id: uuid::Uuid::now_v7(),
            instance_id: inst,
            org_id: org_a,
            repo_id: repo_a_id,
            number: n1,
            state: "open".into(),
            title: "Add space".into(),
            body: "Body".into(),
            head_ref: "feature-1".into(),
            head_sha: "1111111111111111111111111111111111111111".into(),
            base_ref: "main".into(),
            base_sha: "0000000000000000000000000000000000000000".into(),
            merge_base: "0000000000000000000000000000000000000000".into(),
            mergeable: Some(true),
            merged: false,
            merge_commit_sha: String::new(),
            author_login: "alice".into(),
            author_name: "Alice".into(),
            author_email: Some("alice@hel.fi".into()),
            created_at: now,
            updated_at: now,
            closed_at: None,
        })
        .await
        .expect("create pr");
    assert_eq!(pr_a.number, 1);

    // Org B cannot see Org A's pull request
    assert!(
        store
            .get_pull_request(inst, org_b, repo_a_id, 1)
            .await
            .expect("cross-org pr")
            .is_none()
    );
    assert!(
        store
            .list_pull_requests(inst, org_b, None, None)
            .await
            .expect("list_prs b")
            .is_empty()
    );

    // 5. Packages, webhooks, push mirrors, and outbox idempotency isolation
    let hook = store
        .create_webhook(WebhookRecord {
            id: WebhookId::new(),
            instance_id: inst,
            org_id: org_a,
            repo_id: repo_a_id,
            url: "https://portal.city.example/api/v1/webhooks/gitea".into(),
            events: vec!["push".into()],
            active: true,
            secret_ref: "hook-sec".into(),
            previous_secret_ref: None,
            created_at: now,
        })
        .await
        .expect("create webhook");
    assert!(
        store
            .get_webhook(inst, org_b, hook.id)
            .await
            .expect("cross-org webhook")
            .is_none()
    );

    let pkg = store
        .upsert_package(PackageRecord {
            id: PackageId::new(),
            instance_id: inst,
            org_id: org_a,
            owner: "helsinki".into(),
            package_type: "generic".into(),
            name: "app-bundle".into(),
            created_at: now,
        })
        .await
        .expect("upsert package");
    assert_eq!(
        store
            .list_packages(inst, org_b)
            .await
            .expect("pkgs b")
            .len(),
        0
    );
    assert_eq!(pkg.name, "app-bundle");

    let ev = store
        .append_outbox(OutboxEvent {
            id: uuid::Uuid::now_v7(),
            instance_id: inst,
            org_id: org_a,
            repo_id: Some(repo_a_id),
            event_kind: "push".into(),
            idempotency_key: "push:repo_a:1".into(),
            payload: json!({"ref": "refs/heads/main"}),
            dispatched: false,
            created_at: now,
            dispatched_at: None,
        })
        .await
        .expect("append outbox");
    // Duplicate idempotency_key is idempotent (returns existing event, never duplicates)
    let ev_dup = store
        .append_outbox(OutboxEvent {
            id: uuid::Uuid::now_v7(),
            ..ev.clone()
        })
        .await
        .expect("append duplicate outbox");
    assert_eq!(ev.id, ev_dup.id);
    assert_eq!(
        store
            .list_undispatched_outbox(inst, org_a)
            .await
            .expect("undispatched a")
            .len(),
        1
    );
    assert!(
        store
            .list_undispatched_outbox(inst, org_b)
            .await
            .expect("undispatched b")
            .is_empty()
    );
    store
        .mark_outbox_dispatched(inst, org_a, ev.id)
        .await
        .expect("mark dispatched");
    assert!(
        store
            .list_undispatched_outbox(inst, org_a)
            .await
            .expect("undispatched after")
            .is_empty()
    );
}
