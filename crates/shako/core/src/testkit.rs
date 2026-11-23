#![forbid(unsafe_code)]
//! Shared multi-tenant isolation test suite run against both `shako-store-memory`
//! and `shako-store-pg` (S2, T6).

use crate::ids::{InstanceId, OrgId, PackageId, RepoId, WebhookId};
use crate::model::*;
use crate::store::Store;
use jc_time::TimestampTz;
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
    let now = TimestampTz::now();

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

    // A record read before those numbers were allocated, written back whole, does not hand a
    // number out again; a number already taken is never overwritten.
    store
        .update_repo(updated.clone())
        .await
        .expect("write back a record read earlier");
    let n3 = store
        .allocate_pr_number(inst, org_a, repo_a_id)
        .await
        .expect("pr 3");
    assert_eq!(n3, 3);
    let taken = store
        .create_pull_request(PullRequestRecord {
            id: uuid::Uuid::now_v7(),
            title: "Another change".into(),
            ..pr_a.clone()
        })
        .await
        .expect_err("pull request number 1 is taken");
    assert!(matches!(taken, crate::error::ShakoError::Conflict(_)));
    assert_eq!(
        store
            .get_pull_request(inst, org_a, repo_a_id, 1)
            .await
            .expect("pr 1")
            .map(|p| p.title),
        Some("Add space".into())
    );

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
            url: "https://portal.city.example/api/v1/webhooks/shako".into(),
            events: vec!["push".into()],
            active: true,
            secret_ref: "hook-sec".into(),
            previous_secret_ref: None,
            format: WebhookFormat::Shako,
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

    // A mirror sync (T-7522): one claim, a mark during it stays due, no other Organization's.
    let mark = store
        .mark_mirror_due(inst, org_a, repo_a_id)
        .await
        .expect("mark due");
    let claimed = |org, repo| store.claim_mirror_sync(inst, org, repo);
    assert_eq!(claimed(org_b, None).await.expect("claim b"), None);
    assert_eq!(
        claimed(org_a, None).await.expect("claim a"),
        Some((repo_a_id, mark))
    );
    assert_eq!(claimed(org_a, None).await.expect("claimed"), None);
    let during = store
        .mark_mirror_due(inst, org_a, repo_a_id)
        .await
        .expect("mark during");
    assert!(during > mark);
    store
        .finish_mirror_sync(inst, org_a, repo_a_id, mark)
        .await
        .expect("finish");
    for (org, at, synced) in [
        (org_a, mark, true),
        (org_a, during, false),
        (org_b, mark, false),
    ] {
        let found = store.mirror_synced(inst, org, repo_a_id, at).await;
        assert_eq!(found.expect("synced"), synced);
    }
    assert_eq!(
        claimed(org_a, Some(repo_a_id)).await.expect("claim again"),
        Some((repo_a_id, during))
    );
}

/// Runs the answers both stores owe the server beyond isolation: an unknown tenant, repository
/// or event, the pull-request filter and order, package look-ups, blob dedupe and its storage
/// limit, the outbox claim and retry order, and the audit trail (T-7664).
pub async fn run_contract_suite(store: &dyn Store) {
    use crate::error::ShakoError;
    use std::time::Duration;

    let inst = InstanceId::new();
    let org = OrgId::new();
    let stranger = OrgId::new();
    let now = TimestampTz::now();
    let later = TimestampTz::from_millis(now.as_millis() + 60_000).expect("a minute later");

    // An Organization that never wrote anything reads as empty, never as an error.
    let quota = store.get_quota_usage(inst, stranger).await.expect("quota");
    assert_eq!(
        (quota.instance_id, quota.org_id, quota.total_bytes()),
        (inst, stranger, 0)
    );
    assert_eq!(quota.repo_count, 0);
    let nobody = RepoId::new();
    assert!(
        !store
            .delete_repo(inst, stranger, nobody)
            .await
            .expect("delete")
    );
    assert!(
        store
            .list_pull_requests(inst, stranger, None, None)
            .await
            .expect("prs")
            .is_empty()
    );
    let version = store
        .get_package_version(inst, stranger, "hel", "container", "app", "1.0")
        .await
        .expect("version");
    assert!(version.is_none());
    let tags = store.list_package_tags(inst, stranger, "hel", "app", None, 10);
    assert!(tags.await.expect("tags").is_empty());
    let referrers = store.list_referrers(inst, stranger, "hel", "app", "sha256:aa");
    assert!(referrers.await.expect("referrers").is_empty());
    let blob = store.get_blob(inst, stranger, "hel", None, "sha256:b1");
    assert!(blob.await.expect("blob").is_none());
    let unknown_event = uuid::Uuid::now_v7();
    let marked = store.mark_outbox_dispatched(inst, stranger, unknown_event);
    assert!(!marked.await.expect("mark"));
    assert!(
        store
            .claim_outbox(inst, stranger)
            .await
            .expect("claim")
            .is_none()
    );
    let retried = store.retry_outbox(inst, stranger, unknown_event, Duration::ZERO);
    assert!(!retried.await.expect("retry"));
    assert!(
        store
            .list_audit(inst, stranger)
            .await
            .expect("audit")
            .is_empty()
    );
    let events = store.list_outbox_events(inst, stranger);
    assert!(events.await.expect("events").is_empty());

    store
        .upsert_instance(InstanceRecord::default_for(inst, org, "git.city.example"))
        .await
        .expect("instance");
    let repo = |name: &str| RepoRecord {
        id: RepoId::new(),
        instance_id: inst,
        org_id: org,
        project_id: "helsinki".into(),
        owner: "hel".into(),
        name: name.into(),
        description: String::new(),
        default_branch: "main".into(),
        private: true,
        archived: false,
        size_bytes: 0,
        next_pr_number: 1,
        created_at: now,
        updated_at: now,
    };
    let site = store.create_repo(repo("site")).await.expect("site");
    let docs = store.create_repo(repo("docs")).await.expect("docs");
    // Postgres holds no NUL in text, so no repository has such a name.
    let nul = store.get_repo_by_name(inst, org, "hel", "si\0te").await;
    assert!(nul.expect("lookup").is_none());

    // An unknown repository in a known Organization: not found, nothing deleted, nothing due.
    let missing = store.allocate_pr_number(inst, org, nobody).await;
    assert!(
        matches!(missing, Err(ShakoError::NotFound(_))),
        "{missing:?}"
    );
    assert!(!store.delete_repo(inst, org, nobody).await.expect("delete"));
    store
        .finish_mirror_sync(inst, org, site.id, 1)
        .await
        .expect("finish a sync never claimed");
    assert_eq!(
        store
            .claim_mirror_sync(inst, org, None)
            .await
            .expect("claim"),
        None
    );
    // A sync finished at the mark it claimed leaves nothing due.
    let mark = store
        .mark_mirror_due(inst, org, docs.id)
        .await
        .expect("due");
    let claim = store.claim_mirror_sync(inst, org, Some(docs.id)).await;
    assert_eq!(claim.expect("claim"), Some((docs.id, mark)));
    store
        .finish_mirror_sync(inst, org, docs.id, mark)
        .await
        .expect("finish");
    let again = store.claim_mirror_sync(inst, org, Some(docs.id)).await;
    assert_eq!(again.expect("claim again"), None);

    // Pull requests: newest update first, the number breaking a tie; the state filter ignores
    // case and "all" keeps every state.
    let pr = |number: u64, state: &str, updated_at| PullRequestRecord {
        id: uuid::Uuid::now_v7(),
        instance_id: inst,
        org_id: org,
        repo_id: site.id,
        number,
        state: state.into(),
        title: format!("change {number}"),
        body: String::new(),
        head_ref: format!("feature-{number}"),
        head_sha: "1111111111111111111111111111111111111111".into(),
        base_ref: "main".into(),
        base_sha: "0000000000000000000000000000000000000000".into(),
        merge_base: "0000000000000000000000000000000000000000".into(),
        mergeable: None,
        merged: false,
        merge_commit_sha: String::new(),
        author_login: "alice".into(),
        author_name: "Alice".into(),
        author_email: None,
        created_at: now,
        updated_at,
        closed_at: None,
    };
    for (number, state, at) in [(1, "open", later), (2, "closed", now), (3, "open", now)] {
        store
            .create_pull_request(pr(number, state, at))
            .await
            .expect("create pr");
    }
    let numbers = |state: Option<&'static str>, repo_id: Option<RepoId>| async move {
        store
            .list_pull_requests(inst, org, repo_id, state)
            .await
            .expect("list prs")
            .iter()
            .map(|p| p.number)
            .collect::<Vec<_>>()
    };
    assert_eq!(numbers(None, None).await, [1, 3, 2]);
    assert_eq!(numbers(Some("all"), Some(site.id)).await, [1, 3, 2]);
    assert_eq!(numbers(Some("OPEN"), None).await, [1, 3]);
    assert_eq!(numbers(Some("closed"), None).await, [2]);
    assert!(numbers(None, Some(docs.id)).await.is_empty());
    store
        .upsert_push_mirror(PushMirrorRecord {
            id: uuid::Uuid::now_v7(),
            instance_id: inst,
            org_id: org,
            repo_id: site.id,
            remote_name: "up".into(),
            remote_address: "https://git.example/site.git".into(),
            credential_secret_ref: String::new(),
            interval: "8h".into(),
            sync_on_commit: false,
            branch: String::new(),
            last_sync: None,
            last_error: String::new(),
            created_at: now,
        })
        .await
        .expect("mirror");
    let mirrors = |repo_id| async move {
        store
            .list_push_mirrors(inst, org, repo_id)
            .await
            .expect("mirrors")
            .iter()
            .map(|m| (m.remote_name.clone(), m.interval.clone()))
            .collect::<Vec<_>>()
    };
    let up = vec![("up".to_owned(), "8h".to_owned())];
    assert_eq!(mirrors(Some(site.id)).await, up);
    assert_eq!(mirrors(None).await, up);
    assert!(mirrors(Some(docs.id)).await.is_empty());

    // Package versions: found by version or by digest, tags without digests in byte order
    // after the cursor, referrers by subject.
    let pkg = store
        .upsert_package(PackageRecord {
            id: PackageId::new(),
            instance_id: inst,
            org_id: org,
            owner: "hel".into(),
            package_type: "container".into(),
            name: "app".into(),
            created_at: now,
        })
        .await
        .expect("package");
    let ver = |version: &str, digest: &str, subject: Option<&str>| PackageVersionRecord {
        id: uuid::Uuid::now_v7(),
        instance_id: inst,
        org_id: org,
        package_id: pkg.id,
        owner: "hel".into(),
        package_type: "container".into(),
        package_name: "app".into(),
        version: version.into(),
        digest: digest.into(),
        media_type: "application/vnd.oci.image.manifest.v1+json".into(),
        subject_digest: subject.map(str::to_owned),
        artifact_type: None,
        size_bytes: 10,
        signed: false,
        created_at: now,
    };
    for v in [
        ver("2.0", "sha256:b2", None),
        ver("1.0", "sha256:a1", None),
        ver("sha256:c3", "sha256:c3", Some("sha256:a1")),
    ] {
        store.upsert_package_version(v).await.expect("version");
    }
    let found = |key: &'static str| async move {
        store
            .get_package_version(inst, org, "hel", "container", "app", key)
            .await
            .expect("get version")
            .map(|v| v.version)
    };
    assert_eq!(found("1.0").await.as_deref(), Some("1.0"));
    assert_eq!(found("sha256:b2").await.as_deref(), Some("2.0"));
    assert_eq!(found("3.0").await, None);
    let versions = |owner: Option<&'static str>, kind, name| async move {
        let mut found: Vec<String> = store
            .list_package_versions(inst, org, owner, kind, name)
            .await
            .expect("versions")
            .into_iter()
            .map(|v| v.version)
            .collect();
        found.sort();
        found
    };
    let all = ["1.0", "2.0", "sha256:c3"];
    assert_eq!(
        versions(Some("hel"), Some("container"), Some("app")).await,
        all
    );
    assert_eq!(versions(None, None, None).await, all);
    for (owner, kind, name) in [
        (Some("bbsk"), None, None),
        (None, Some("generic"), None),
        (None, None, Some("other")),
    ] {
        assert!(versions(owner, kind, name).await.is_empty());
    }
    let tags = |after: Option<&'static str>, limit| async move {
        store
            .list_package_tags(inst, org, "hel", "app", after, limit)
            .await
            .expect("tags")
    };
    assert_eq!(tags(None, 10).await, ["1.0", "2.0"]);
    assert_eq!(tags(None, 1).await, ["1.0"]);
    assert_eq!(tags(Some("1.0"), 10).await, ["2.0"]);
    let referrers = store
        .list_referrers(inst, org, "hel", "app", "sha256:a1")
        .await
        .expect("referrers");
    assert_eq!(
        referrers
            .iter()
            .map(|v| v.digest.as_str())
            .collect::<Vec<_>>(),
        ["sha256:c3"]
    );

    // Blobs: a second write of a held digest answers the held row and counts nothing; the
    // storage limit refuses a blob past it, and no limit stores it.
    let blob = |repo_name: &str, digest: &str| BlobRecord {
        id: uuid::Uuid::now_v7(),
        instance_id: inst,
        org_id: org,
        owner: "hel".into(),
        repo_name: repo_name.into(),
        digest: digest.into(),
        size_bytes: 100,
        created_at: now,
    };
    let first = store
        .store_blob(blob("app", "sha256:b1"), Some(150))
        .await
        .expect("first blob");
    let again = store
        .store_blob(blob("app", "sha256:b1"), Some(150))
        .await
        .expect("held blob");
    assert_eq!(again.id, first.id);
    let used = |org| async move {
        store
            .get_quota_usage(inst, org)
            .await
            .expect("quota")
            .package_bytes
    };
    assert_eq!(used(org).await, 100);
    let refused = store
        .store_blob(blob("app", "sha256:b2"), Some(150))
        .await
        .expect_err("over the limit");
    assert!(
        matches!(&refused, ShakoError::QuotaExceeded { limit: 150, .. }),
        "{refused:?}"
    );
    assert_eq!(used(org).await, 100);
    store
        .store_blob(blob("app", "sha256:b2"), Some(200))
        .await
        .expect("at the limit");
    store
        .store_blob(blob("app", "sha256:b3"), None)
        .await
        .expect("no limit");
    assert_eq!(used(org).await, 300);
    let get = |repo: Option<&'static str>, owner: &'static str| async move {
        store
            .get_blob(inst, org, owner, repo, "sha256:b1")
            .await
            .expect("get blob")
            .map(|b| b.id)
    };
    assert_eq!(get(Some("app"), "hel").await, Some(first.id));
    assert_eq!(get(Some("other"), "hel").await, None);
    assert_eq!(get(None, "hel").await, Some(first.id));
    assert_eq!(get(None, "bbsk").await, None);

    // Outbox: the oldest event of a repository first; one waiting for a retry holds back its
    // repository's later events only; a retry counts its attempts; a dispatched event is done.
    let event = |repo_id: RepoId, key: &str| OutboxEvent {
        id: uuid::Uuid::now_v7(),
        instance_id: inst,
        org_id: org,
        repo_id: Some(repo_id),
        event_kind: "push".into(),
        idempotency_key: key.into(),
        payload: json!({"key": key}),
        dispatched: false,
        created_at: now,
        dispatched_at: None,
    };
    let mut appended = Vec::new();
    for (repo_id, key) in [
        (site.id, "site-1"),
        (site.id, "site-2"),
        (docs.id, "docs-1"),
    ] {
        appended.push(
            store
                .append_outbox(event(repo_id, key))
                .await
                .expect("append"),
        );
    }
    let [s1, s2, d1] = [0, 1, 2].map(|i| appended[i].id);
    let claim = || async {
        store
            .claim_outbox(inst, org)
            .await
            .expect("claim")
            .map(|c| (c.event.id, c.attempts))
    };
    assert_eq!(claim().await, Some((s1, 0)));
    let wait = Duration::from_secs(3600);
    assert!(
        store
            .retry_outbox(inst, org, s1, wait)
            .await
            .expect("retry")
    );
    assert_eq!(claim().await, Some((d1, 0)));
    assert!(
        store
            .mark_outbox_dispatched(inst, org, d1)
            .await
            .expect("mark")
    );
    assert_eq!(claim().await, None);
    let now_due = Duration::ZERO;
    assert!(
        store
            .retry_outbox(inst, org, s1, now_due)
            .await
            .expect("retry")
    );
    assert_eq!(claim().await, Some((s1, 2)));
    assert!(
        store
            .mark_outbox_dispatched(inst, org, s1)
            .await
            .expect("mark")
    );
    assert!(
        !store
            .retry_outbox(inst, org, s1, now_due)
            .await
            .expect("done")
    );
    assert!(
        !store
            .mark_outbox_dispatched(inst, org, unknown_event)
            .await
            .expect("unknown")
    );
    assert_eq!(claim().await, Some((s2, 0)));
    let undispatched = store.list_outbox_events(inst, org).await.expect("events");
    assert_eq!(
        undispatched
            .iter()
            .map(|e| (e.id, e.dispatched))
            .collect::<Vec<_>>(),
        [(s1, true), (s2, false), (d1, true)]
    );

    // Audit: rows come back in the order written, only to their own Organization.
    let audit = |kind: &str, created_at| AuditRecord {
        id: uuid::Uuid::now_v7(),
        instance_id: inst,
        org_id: org,
        kind: kind.into(),
        operation: "CreateRepository".into(),
        actor_id: "alice".into(),
        target: format!("repos/{}", site.id),
        decision: "allow".into(),
        request_id: "req-1".into(),
        created_at,
    };
    let rows = [audit("REPO_CREATE", now), audit("REPO_DELETE", later)];
    for row in rows.clone() {
        store.append_audit(row).await.expect("audit");
    }
    let listed = store.list_audit(inst, org).await.expect("audit");
    assert_eq!(
        listed
            .iter()
            .map(|r| (r.id, r.kind.as_str(), r.org_id, r.target.as_str()))
            .collect::<Vec<_>>(),
        rows.iter()
            .map(|r| (r.id, r.kind.as_str(), org, r.target.as_str()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        (
            listed[1].actor_id.as_str(),
            listed[1].decision.as_str(),
            listed[1].request_id.as_str()
        ),
        ("alice", "allow", "req-1")
    );
    assert_eq!(listed[0].operation, "CreateRepository");
    assert!(
        store
            .list_audit(inst, stranger)
            .await
            .expect("audit")
            .is_empty()
    );

    // Quota: each count moves by its own delta and never below zero.
    let before = store.get_quota_usage(inst, org).await.expect("quota");
    let moved = store
        .adjust_quota_usage(inst, org, 2, 30, 7)
        .await
        .expect("adjust");
    assert_eq!(
        (moved.repo_count, moved.git_bytes, moved.package_bytes),
        (
            before.repo_count + 2,
            before.git_bytes + 30,
            before.package_bytes + 7
        )
    );
    let floored = store
        .adjust_quota_usage(inst, org, -1_000_000, -1_000_000, -1_000_000)
        .await
        .expect("adjust");
    assert_eq!(
        (floored.repo_count, floored.git_bytes, floored.total_bytes()),
        (0, 0, 0)
    );

    // Two repositories with a row of every kind: each list answers its own repository's rows,
    // and deleting one repository takes its rows only.
    store
        .create_pull_request(PullRequestRecord {
            repo_id: docs.id,
            ..pr(1, "open", now)
        })
        .await
        .expect("docs pr");
    let sha = "1111111111111111111111111111111111111111";
    let mut hooks = Vec::new();
    for (repo_id, name) in [(site.id, "site"), (docs.id, "docs")] {
        let rule = BranchProtectionRule {
            rule_name: format!("{name}-main"),
            ..BranchProtectionRule::default()
        };
        store
            .upsert_protection_rule(ProtectionRuleRecord {
                id: uuid::Uuid::now_v7(),
                instance_id: inst,
                org_id: org,
                repo_id,
                rule,
                created_at: now,
                updated_at: now,
            })
            .await
            .expect("branch rule");
        let rule = TagProtectionRule {
            name_pattern: format!("{name}-v*"),
            whitelist_usernames: Vec::new(),
        };
        store
            .upsert_tag_protection(TagProtectionRecord {
                id: uuid::Uuid::now_v7(),
                instance_id: inst,
                org_id: org,
                repo_id,
                rule,
                created_at: now,
            })
            .await
            .expect("tag rule");
        store
            .create_review(ReviewRecord {
                id: uuid::Uuid::now_v7(),
                instance_id: inst,
                org_id: org,
                repo_id,
                pr_number: 1,
                verdict: ReviewVerdict::Approved,
                body: name.into(),
                author_login: "bob".into(),
                commit_id: sha.into(),
                dismissed: false,
                created_at: now,
            })
            .await
            .expect("review");
        store
            .upsert_commit_status(CommitStatusRecord {
                id: uuid::Uuid::now_v7(),
                instance_id: inst,
                org_id: org,
                repo_id,
                sha: sha.into(),
                context: format!("{name}-ci"),
                state: "success".into(),
                description: String::new(),
                target_url: String::new(),
                updated_at: now,
            })
            .await
            .expect("status");
        let hook = store
            .create_webhook(WebhookRecord {
                id: WebhookId::new(),
                instance_id: inst,
                org_id: org,
                repo_id,
                url: format!("https://portal.city.example/hooks/{name}"),
                events: vec!["push".into()],
                active: true,
                secret_ref: format!("hooks/{name}"),
                previous_secret_ref: None,
                format: WebhookFormat::Shako,
                created_at: now,
            })
            .await
            .expect("webhook");
        store
            .record_delivery(DeliveryRecord {
                id: uuid::Uuid::now_v7(),
                instance_id: inst,
                org_id: org,
                webhook_id: hook.id,
                event: name.into(),
                status: "delivered".into(),
                response_code: Some(200),
                delivered_at: now,
            })
            .await
            .expect("delivery");
        hooks.push(hook.id);
    }
    let held = |repo_id: RepoId| async move {
        let rules = store.list_protection_rules(inst, org, repo_id).await;
        let tags = store.list_tag_protections(inst, org, repo_id).await;
        let reviews = store.list_reviews(inst, org, Some(repo_id), Some(1)).await;
        let statuses = store.list_commit_statuses(inst, org, repo_id, sha).await;
        let webhooks = store.list_webhooks(inst, org, Some(repo_id)).await;
        let prs = store
            .list_pull_requests(inst, org, Some(repo_id), None)
            .await;
        (
            rules
                .expect("rules")
                .into_iter()
                .map(|r| r.rule.rule_name)
                .collect::<Vec<_>>(),
            tags.expect("tags")
                .into_iter()
                .map(|r| r.rule.name_pattern)
                .collect::<Vec<_>>(),
            reviews
                .expect("reviews")
                .into_iter()
                .map(|r| r.body)
                .collect::<Vec<_>>(),
            statuses
                .expect("statuses")
                .into_iter()
                .map(|r| r.context)
                .collect::<Vec<_>>(),
            webhooks
                .expect("hooks")
                .into_iter()
                .map(|r| r.secret_ref)
                .collect::<Vec<_>>(),
            prs.expect("prs").len(),
        )
    };
    let site_rows = (
        vec!["site-main".to_owned()],
        vec!["site-v*".to_owned()],
        vec!["site".to_owned()],
        vec!["site-ci".to_owned()],
        vec!["hooks/site".to_owned()],
        3,
    );
    assert_eq!(held(site.id).await, site_rows);
    assert_eq!(held(docs.id).await.4, ["hooks/docs"]);
    let other_pr = store.list_reviews(inst, org, Some(site.id), Some(2)).await;
    assert!(other_pr.expect("reviews").is_empty());
    let other_sha = store.list_commit_statuses(inst, org, site.id, "2222").await;
    assert!(other_sha.expect("statuses").is_empty());
    let all_reviews = store.list_reviews(inst, org, None, None).await;
    assert_eq!(all_reviews.expect("reviews").len(), 2);
    let deliveries = store.list_deliveries(inst, org, Some(hooks[0])).await;
    let events: Vec<String> = deliveries
        .expect("deliveries")
        .into_iter()
        .map(|d| d.event)
        .collect();
    assert_eq!(events, ["site"]);
    assert!(
        store
            .delete_repo(inst, org, docs.id)
            .await
            .expect("delete docs")
    );
    assert_eq!(held(site.id).await, site_rows);
    assert_eq!(mirrors(Some(site.id)).await, up);
    let gone = held(docs.id).await;
    assert!(
        gone.0.is_empty() && gone.1.is_empty() && gone.2.is_empty(),
        "{gone:?}"
    );
    assert_eq!(gone.5, 0);
}
