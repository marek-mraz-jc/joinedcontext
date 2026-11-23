#![forbid(unsafe_code)]
#![allow(clippy::disallowed_methods)] // Tests read their database URL; jc runs none of this.
//! T-7508: the typed tables answer what the memory store answers, for a write made now and for
//! the log a previous release left, and two dispatchers share one outbox.
//!
//! `SHAKO_TEST_DATABASE_URL` names an admin (superuser) connection. Unset: prints `SKIP`.

use serde_json::{Value, json};
use shako_core::{InstanceId, OrgId, OutboxEvent, RepoId, Store, WebhookId};
use shako_store_memory::MemoryStore;
use shako_store_pg::PgStore;
use sqlx::AssertSqlSafe;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use std::str::FromStr;

/// A database of its own and an app login in it.
struct Db {
    server: PgPool,
    admin: PgPool,
    app: PgConnectOptions,
    name: String,
}

impl Db {
    async fn create(login: &str) -> Option<Self> {
        let Ok(url) = std::env::var("SHAKO_TEST_DATABASE_URL") else {
            println!("SKIP: SHAKO_TEST_DATABASE_URL is unset");
            return None;
        };
        let options = PgConnectOptions::from_str(&url).expect("valid SHAKO_TEST_DATABASE_URL");
        let server = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(options.clone())
            .await
            .expect("SHAKO_TEST_DATABASE_URL is set but unreachable");
        let name = format!("{login}_{}", uuid::Uuid::new_v4().simple());
        sqlx::raw_sql(AssertSqlSafe(format!("CREATE DATABASE {name}")))
            .execute(&server)
            .await
            .expect("create test database");
        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options.clone().database(&name))
            .await
            .expect("connect test database");
        PgStore::run_migrations(&admin).await.expect("migrate");
        let pw = uuid::Uuid::new_v4().simple().to_string();
        sqlx::raw_sql(AssertSqlSafe(format!(
            "DO $$ BEGIN CREATE ROLE {login}; EXCEPTION WHEN duplicate_object THEN NULL; END $$;
             ALTER ROLE {login} LOGIN INHERIT NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE PASSWORD '{pw}';
             GRANT shako_app TO {login};"
        )))
        .execute(&admin)
        .await
        .expect("create request login role");
        let app = options.database(&name).username(login).password(&pw);
        Some(Self {
            server,
            admin,
            app,
            name,
        })
    }

    async fn store(&self) -> PgStore {
        PgStore::connect_with(self.app.clone(), 4)
            .await
            .expect("store starts")
    }

    async fn drop_it(self) {
        self.admin.close().await;
        sqlx::raw_sql(AssertSqlSafe(format!(
            "DROP DATABASE {} WITH (FORCE)",
            self.name
        )))
        .execute(&self.server)
        .await
        .expect("drop test database");
    }
}

const WHEN: &str = "2026-11-20T10:00:00Z";

/// The fixed ids one run writes to both stores.
struct Ids {
    inst: InstanceId,
    org: OrgId,
    repo: RepoId,
    renamed: RepoId,
    gone: RepoId,
    hook: WebhookId,
    other: [uuid::Uuid; 24],
}

impl Ids {
    fn new() -> Self {
        Self {
            inst: InstanceId::new(),
            org: OrgId::new(),
            repo: RepoId::new(),
            renamed: RepoId::new(),
            gone: RepoId::new(),
            hook: WebhookId::new(),
            other: std::array::from_fn(|_| uuid::Uuid::now_v7()),
        }
    }

    /// A record: the tenant's two ids beside `fields`.
    fn record<T: serde::de::DeserializeOwned>(&self, mut fields: Value) -> T {
        fields["instanceId"] = json!(self.inst);
        fields["orgId"] = json!(self.org);
        serde_json::from_value(fields).expect("record")
    }

    fn repo(&self, id: RepoId, name: &str) -> shako_core::RepoRecord {
        self.record(
            json!({"id": id, "projectId": "doprava", "owner": "jc", "name": name,
            "description": "it's \"quoted\" \\ žltý", "defaultBranch": "main", "private": true,
            "archived": false, "sizeBytes": 7, "nextPrNumber": 1,
            "createdAt": WHEN, "updatedAt": WHEN}),
        )
    }

    fn event(&self, id: uuid::Uuid, key: &str) -> OutboxEvent {
        self.record(json!({"id": id, "repoId": self.repo, "eventKind": "push",
            "idempotencyKey": key, "payload": {"ref": "refs/heads/main"}, "dispatched": false,
            "createdAt": WHEN, "dispatchedAt": null}))
    }
}

/// Every operation of the log, refusals and replacements included.
async fn scenario(s: &dyn Store, n: &Ids) {
    let (i, o, r, id) = (n.inst, n.org, n.repo, &n.other);
    s.upsert_instance(n.record(
        json!({"host": "git.example", "pathPrefix": "", "maxRepos": 9,
        "maxStorageBytes": 900, "maxPushBytes": 90, "createdAt": WHEN, "updatedAt": WHEN}),
    ))
    .await
    .unwrap();
    s.adjust_quota_usage(i, o, 2, 10, 5).await.unwrap();
    s.adjust_quota_usage(i, o, -1, -50, 1).await.unwrap();

    let mut first = s.create_repo(n.repo(r, "config")).await.unwrap();
    s.create_repo(n.repo(n.renamed, "old")).await.unwrap();
    s.update_repo(n.repo(n.renamed, "new")).await.unwrap();
    s.create_repo(n.repo(n.gone, "old")).await.unwrap();
    assert!(
        s.create_repo(n.repo(RepoId::new(), "config"))
            .await
            .is_err()
    );
    assert!(s.update_repo(n.repo(RepoId::new(), "none")).await.is_err());

    assert_eq!(s.allocate_pr_number(i, o, r).await.unwrap(), 1);
    assert_eq!(s.allocate_pr_number(i, o, r).await.unwrap(), 2);
    // A stale copy never moves the numbers back.
    first.description = "changed".into();
    assert_eq!(s.update_repo(first).await.unwrap().next_pr_number, 3);

    for (rule, repo, name) in [
        (id[0], r, "main"),
        (id[1], r, "main"),
        (id[2], r, "rel/*"),
        (id[3], n.gone, "main"),
    ] {
        s.upsert_protection_rule(n.record(json!({"id": rule, "repoId": repo,
            "rule": {"rule_name": name}, "createdAt": WHEN, "updatedAt": WHEN})))
            .await
            .unwrap();
    }
    s.upsert_tag_protection(n.record(json!({"id": id[4], "repoId": r,
        "rule": {"name_pattern": "main", "whitelist_usernames": ["alice"]}, "createdAt": WHEN})))
        .await
        .unwrap();
    assert!(s.delete_protection_rule(i, o, r, "rel/*").await.unwrap());

    let pr = |state: &str| -> shako_core::PullRequestRecord {
        n.record(
            json!({"id": id[5], "repoId": r, "number": 1, "state": state, "title": "Add space",
            "body": "", "headRef": "f", "headSha": "1".repeat(40), "baseRef": "main",
            "baseSha": "0".repeat(40), "mergeBase": "0".repeat(40), "mergeable": true,
            "merged": false, "mergeCommitSha": "", "authorLogin": "alice", "authorName": "Alice",
            "authorEmail": null, "createdAt": WHEN, "updatedAt": WHEN, "closedAt": null}),
        )
    };
    s.create_pull_request(pr("open")).await.unwrap();
    assert!(s.create_pull_request(pr("open")).await.is_err());
    s.update_pull_request(pr("closed")).await.unwrap();
    for (review, verdict) in [(id[6], "REQUEST_CHANGES"), (id[7], "APPROVED")] {
        s.create_review(n.record(
            json!({"id": review, "repoId": r, "prNumber": 1, "verdict": verdict,
            "body": "", "authorLogin": "bob", "commitId": "1".repeat(40), "dismissed": false,
            "createdAt": WHEN}),
        ))
        .await
        .unwrap();
    }
    for (status, state) in [(id[8], "pending"), (id[9], "success")] {
        s.upsert_commit_status(
            n.record(json!({"id": status, "repoId": r, "sha": "1".repeat(40),
            "context": "jc/plan", "state": state, "description": "", "targetUrl": "",
            "updatedAt": WHEN})),
        )
        .await
        .unwrap();
    }
    s.create_webhook(n.record(
        json!({"id": n.hook, "repoId": r, "url": "https://hook.example/",
        "events": ["push"], "active": true, "secretRef": "hooks/a",
        "previousSecretRef": "hooks/b", "format": "shako", "createdAt": WHEN}),
    ))
    .await
    .unwrap();
    s.record_delivery(
        n.record(json!({"id": id[10], "webhookId": n.hook, "event": "push",
        "status": "success", "responseCode": 204, "deliveredAt": WHEN})),
    )
    .await
    .unwrap();
    for (mirror, name) in [(id[11], "up"), (id[12], "up"), (id[13], "spare")] {
        s.upsert_push_mirror(
            n.record(json!({"id": mirror, "repoId": r, "remoteName": name,
            "remoteAddress": "https://git.example/x.git", "credentialSecretRef": "mirrors/up",
            "interval": "8h", "syncOnCommit": true, "branch": "main", "lastSync": null,
            "lastError": "", "createdAt": WHEN})),
        )
        .await
        .unwrap();
    }
    assert!(s.delete_push_mirror(i, o, r, "spare").await.unwrap());

    for package in [id[14], id[15]] {
        let kept = s
            .upsert_package(n.record(json!({"id": package, "owner": "jc-apps",
                "packageType": "container", "name": "app", "createdAt": WHEN})))
            .await
            .unwrap();
        assert_eq!(json!(kept.id), json!(id[14]), "the first package stays");
    }
    for version in [id[16], id[17]] {
        s.upsert_package_version(n.record(json!({"id": version, "packageId": id[14],
            "owner": "jc-apps", "packageType": "container", "packageName": "app", "version": "v1",
            "digest": "sha256:00", "mediaType": "application/json",
            "subjectDigest": null, "artifactType": null, "sizeBytes": 3, "signed": false,
            "createdAt": WHEN})))
            .await
            .unwrap();
    }
    let file = |file: uuid::Uuid| -> shako_core::PackageFileRecord {
        n.record(json!({"id": file, "versionId": id[17], "owner": "jc-apps", "packageName": "app",
            "version": "v1", "filename": "a.tgz", "sha256": "00", "sizeBytes": 1, "createdAt": WHEN}))
    };
    s.insert_package_file(file(id[18])).await.unwrap();
    assert!(s.insert_package_file(file(id[19])).await.is_err());
    for blob in [id[20], id[21]] {
        s.upsert_blob(
            n.record(json!({"id": blob, "owner": "jc-apps", "repoName": "app",
            "digest": "sha256:00", "sizeBytes": 3, "createdAt": WHEN})),
        )
        .await
        .unwrap();
    }
    s.append_outbox(n.event(id[22], "k1")).await.unwrap();
    assert_eq!(
        s.append_outbox(n.event(id[23], "k1")).await.unwrap().id,
        id[22]
    );
    s.append_outbox(n.event(id[23], "k2")).await.unwrap();
    assert!(s.mark_outbox_dispatched(i, o, id[22]).await.unwrap());
    assert!(s.delete_repo(i, o, n.gone).await.unwrap());
}

/// All a store answers of the tenant, less the times it stamps itself.
async fn answers(s: &dyn Store, n: &Ids) -> Value {
    fn unstamped(v: &mut Value) {
        match v {
            Value::Object(map) => {
                map.remove("updatedAt");
                map.remove("dispatchedAt");
                map.values_mut().for_each(unstamped);
            }
            Value::Array(items) => items.iter_mut().for_each(unstamped),
            _ => {}
        }
    }
    let (i, o, r) = (n.inst, n.org, n.repo);
    let version = s
        .get_package_version(i, o, "jc-apps", "container", "app", "sha256:00")
        .await
        .unwrap();
    let mut all = json!({
        "instance": s.get_instance(i, o).await.unwrap(),
        "quota": s.get_quota_usage(i, o).await.unwrap(),
        "repos": s.list_repos(i, o).await.unwrap(),
        "by_name": s.get_repo_by_name(i, o, "jc", "new").await.unwrap(),
        "by_id": s.get_repo_by_id(i, o, r).await.unwrap(),
        "rule": s.get_protection_rule(i, o, r, "main").await.unwrap(),
        "rules": s.list_protection_rules(i, o, r).await.unwrap(),
        "tags": s.list_tag_protections(i, o, r).await.unwrap(),
        "pr": s.get_pull_request(i, o, r, 1).await.unwrap(),
        "prs": s.list_pull_requests(i, o, None, None).await.unwrap(),
        "prs_closed": s.list_pull_requests(i, o, Some(r), Some("CLOSED")).await.unwrap(),
        "prs_open": s.list_pull_requests(i, o, Some(r), Some("open")).await.unwrap(),
        "reviews": s.list_reviews(i, o, Some(r), Some(1)).await.unwrap(),
        "statuses": s.list_commit_statuses(i, o, r, &"1".repeat(40)).await.unwrap(),
        "hooks": s.list_webhooks(i, o, Some(r)).await.unwrap(),
        "deliveries": s.list_deliveries(i, o, Some(n.hook)).await.unwrap(),
        "mirrors": s.list_push_mirrors(i, o, None).await.unwrap(),
        "packages": s.list_packages(i, o).await.unwrap(),
        "version": version,
        "tags": s.list_package_tags(i, o, "jc-apps", "app", None, 10).await.unwrap(),
        "tags_after": s.list_package_tags(i, o, "jc-apps", "app", Some("v1"), 10).await.unwrap(),
        "referrers": s.list_referrers(i, o, "jc-apps", "app", "sha256:00").await.unwrap(),
        "versions": s.list_package_versions(i, o, Some("jc-apps"), None, None).await.unwrap(),
        "file": s.get_package_file(i, o, "jc-apps", "app", "v1", "a.tgz").await.unwrap(),
        "files": s.list_package_files(i, o).await.unwrap(),
        "blob": s.get_blob(i, o, "jc-apps", None, "sha256:00").await.unwrap(),
        "pending": s.list_undispatched_outbox(i, o).await.unwrap(),
        "events": s.list_outbox_events(i, o).await.unwrap(),
    });
    unstamped(&mut all);
    // The fields no answer serializes are state all the same.
    let hook = s.get_webhook(i, o, n.hook).await.unwrap().unwrap();
    all["hidden"] = json!([
        hook.secret_ref,
        hook.previous_secret_ref,
        s.list_push_mirrors(i, o, None).await.unwrap()[0].credential_secret_ref,
    ]);
    all
}

/// What is written now, and what a previous release's log projects to, answer as the memory
/// store does.
#[tokio::test]
async fn the_tables_and_a_projected_log_answer_as_the_memory_store() {
    let Some(db) = Db::create("shako_projection_login").await else {
        return;
    };
    let ids = Ids::new();
    let memory = MemoryStore::new();
    scenario(&memory, &ids).await;
    let expected = answers(&memory, &ids).await;

    let live = db.store().await;
    scenario(&live, &ids).await;
    assert_eq!(answers(&live, &ids).await, expected, "written now");
    assert_eq!(
        answers(&db.store().await, &ids).await,
        expected,
        "a second replica"
    );

    // What a previous release leaves: the log (with the writes it refused), the repository rows
    // in their last state, an event without an entry, and no other row.
    sqlx::query(
        "INSERT INTO shako.store_log (instance_id, org_id, op)
         SELECT instance_id, org_id, jsonb_set(op, '{args,id}', to_jsonb(gen_random_uuid()))
         FROM shako.store_log WHERE op ->> 'op' = 'create_repo' AND op #>> '{args,name}' = 'config'",
    )
    .execute(&db.admin)
    .await
    .unwrap();
    for table in [
        "projection",
        "instance",
        "quota_usage",
        "protection_rule",
        "pull_request",
        "review",
        "commit_status",
        "webhook",
        "delivery",
        "push_mirror",
        "package",
        "package_version",
        "package_file",
        "blob",
        "outbox",
    ] {
        sqlx::query(AssertSqlSafe(format!("DELETE FROM shako.{table}")))
            .execute(&db.admin)
            .await
            .unwrap();
    }
    let stray = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO shako.outbox (id, instance_id, org_id, event_kind, idempotency_key, payload)
         VALUES ($1, $2, $3, 'push', 'stray', '{}')",
    )
    .bind(stray)
    .bind(ids.inst.as_uuid())
    .bind(ids.org.as_uuid())
    .execute(&db.admin)
    .await
    .unwrap();

    let upgraded = db.store().await;
    let mut got = answers(&upgraded, &ids).await;
    let events = got["events"].as_array_mut().unwrap();
    let at = events
        .iter()
        .position(|e| e["id"] == json!(stray))
        .expect("no row is lost");
    assert_eq!(
        events.remove(at)["dispatched"],
        json!(true),
        "an event of no entry is never sent"
    );
    assert_eq!(got, expected, "projected from the log");
    assert_eq!(
        upgraded.hydrate(ids.inst, ids.org).await.unwrap(),
        0,
        "projected once"
    );

    // A rollback: that release appended to the log alone.
    sqlx::query("INSERT INTO shako.store_log (instance_id, org_id, op) VALUES ($1, $2, $3)")
        .bind(ids.inst.as_uuid())
        .bind(ids.org.as_uuid())
        .bind(json!({"op": "allocate_pr_number", "args": {"repo_id": ids.repo}}))
        .execute(&db.admin)
        .await
        .unwrap();
    let rolled_forward = db.store().await;
    assert_eq!(
        rolled_forward
            .allocate_pr_number(ids.inst, ids.org, ids.repo)
            .await
            .unwrap(),
        4
    );
    for store in [live, upgraded, rolled_forward] {
        store.pool().close().await;
    }
    db.drop_it().await;
}

/// Entries of `n`'s log, all or those of one operation.
async fn logged(db: &Db, n: &Ids, op: Option<&str>) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM shako.store_log
         WHERE instance_id = $1 AND org_id = $2 AND ($3::text IS NULL OR op ->> 'op' = $3)",
    )
    .bind(n.inst.as_uuid())
    .bind(n.org.as_uuid())
    .bind(op)
    .fetch_one(&db.admin)
    .await
    .unwrap()
}

/// T-7564 (S6-29): the log stops growing. What the tables hold leaves once a rollback can no
/// longer need it, the request role cannot move that gate, and the store answers the same.
#[tokio::test]
async fn the_log_is_trimmed_to_what_a_rollback_can_still_need() {
    let Some(db) = Db::create("shako_trim_login").await else {
        return;
    };
    let (ids, other) = (Ids::new(), Ids::new());
    let store = db.store().await;
    scenario(&store, &ids).await;
    scenario(&store, &other).await;
    let backdate = |n: &Ids| {
        sqlx::query(
            "UPDATE shako.store_log SET created_at = now() - interval '40 days'
             WHERE instance_id = $1 AND org_id = $2",
        )
        .bind(n.inst.as_uuid())
        .bind(n.org.as_uuid())
    };
    backdate(&ids).execute(&db.admin).await.unwrap();
    backdate(&other).execute(&db.admin).await.unwrap();
    assert_eq!(
        store.trim_log(ids.inst, ids.org).await.unwrap(),
        0,
        "projected today: a rollback may still need the whole log"
    );

    // The request role may neither move its gate nor delete past it.
    let app = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(db.app.clone())
        .await
        .unwrap();
    let mut tx = app.begin().await.unwrap();
    sqlx::query("SELECT set_config('shako.instance', $1, true), set_config('shako.org', $2, true)")
        .bind(ids.inst.to_string())
        .bind(ids.org.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let deleted = sqlx::query("DELETE FROM shako.store_log")
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        deleted.rows_affected(),
        0,
        "deleted inside the rollback window"
    );
    let moved = sqlx::query("UPDATE shako.projection SET since = now() - interval '40 days'")
        .execute(&mut *tx)
        .await;
    assert!(moved.is_err(), "the request role moved its own trim gate");
    drop(tx);
    app.close().await;

    for n in [&ids, &other] {
        sqlx::query(
            "UPDATE shako.projection SET since = now() - interval '40 days'
             WHERE instance_id = $1 AND org_id = $2",
        )
        .bind(n.inst.as_uuid())
        .bind(n.org.as_uuid())
        .execute(&db.admin)
        .await
        .unwrap();
    }
    // Written after the backdating: inside the window, so it stays.
    store
        .allocate_pr_number(ids.inst, ids.org, ids.repo)
        .await
        .unwrap();
    let expected = answers(&store, &ids).await;
    let (all, creates) = (
        logged(&db, &ids, None).await,
        logged(&db, &ids, Some("create_repo")).await,
    );
    let trimmed = store.trim_log(ids.inst, ids.org).await.unwrap();
    assert_eq!(i64::try_from(trimmed).unwrap(), all - creates - 1);
    assert_eq!(
        logged(&db, &ids, None).await,
        creates + 1,
        "create_repo and the new entry stay"
    );
    assert!(
        logged(&db, &other, None).await > creates,
        "another tenant's log is its own"
    );

    let restarted = db.store().await;
    assert_eq!(restarted.hydrate(ids.inst, ids.org).await.unwrap(), 0);
    assert_eq!(answers(&restarted, &ids).await, expected, "no record lost");
    assert_eq!(
        restarted
            .allocate_pr_number(ids.inst, ids.org, ids.repo)
            .await
            .unwrap(),
        expected_next_pr(&expected),
    );
    for s in [store, restarted] {
        s.pool().close().await;
    }
    db.drop_it().await;
}

/// The pull request number `config` hands out next, as `answers` shows it.
fn expected_next_pr(answers: &Value) -> u64 {
    answers["repos"]
        .as_array()
        .and_then(|repos| repos.iter().find(|r| r["name"] == "config"))
        .and_then(|r| r["nextPrNumber"].as_u64())
        .expect("config's next number")
}

/// T-7564 (S6-29, false since T-7508): a start reads no more of a projected log as it grows.
#[tokio::test]
async fn a_start_reads_a_bounded_log_as_the_log_grows() {
    let Some(db) = Db::create("shako_replay_login").await else {
        return;
    };
    let ids = Ids::new();
    let store = db.store().await;
    scenario(&store, &ids).await;
    store.pool().close().await;
    let mut took = Vec::new();
    for size in [1_000_i64, 100_000] {
        sqlx::query(
            "INSERT INTO shako.store_log (instance_id, org_id, op)
             SELECT $1, $2, $3 FROM generate_series(1, $4)",
        )
        .bind(ids.inst.as_uuid())
        .bind(ids.org.as_uuid())
        .bind(json!({"op": "adjust_quota_usage", "args": {"repo_delta": 0, "git_bytes_delta": 0, "package_bytes_delta": 0}}))
        .bind(size)
        .execute(&db.admin)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE shako.projection SET seq = (SELECT max(seq) FROM shako.store_log)
             WHERE instance_id = $1 AND org_id = $2",
        )
        .bind(ids.inst.as_uuid())
        .bind(ids.org.as_uuid())
        .execute(&db.admin)
        .await
        .unwrap();
        let fresh = db.store().await;
        let started = std::time::Instant::now();
        assert_eq!(
            fresh.hydrate(ids.inst, ids.org).await.unwrap(),
            0,
            "read past the projection"
        );
        took.push(started.elapsed());
        println!(
            "log of {} entries: the start read 0 in {:?}",
            logged(&db, &ids, None).await,
            took[took.len() - 1]
        );
        fresh.pool().close().await;
    }
    assert!(
        took[1] < took[0] * 20 + std::time::Duration::from_millis(500),
        "start time grew with the log: {took:?}"
    );
    db.drop_it().await;
}

/// S6-29 (false since T-7508): one Organization's writes never wait for another's.
#[tokio::test]
async fn two_organizations_write_in_parallel() {
    let Some(db) = Db::create("shako_parallel_login").await else {
        return;
    };
    let (a, b) = (Ids::new(), Ids::new());
    let store = db.store().await;
    // Another replica holds A's writer lock in an open transaction.
    let mut held = db.admin.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("shako.store:{}:{}", a.inst, a.org))
        .execute(&mut *held)
        .await
        .unwrap();
    let wait = std::time::Duration::from_secs(5);
    tokio::time::timeout(wait, store.create_repo(b.repo(b.repo, "config")))
        .await
        .expect("B waited for A's lock")
        .unwrap();
    let a_write = store.create_repo(a.repo(a.repo, "config"));
    tokio::pin!(a_write);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(300), &mut a_write)
            .await
            .is_err(),
        "A wrote past its own lock"
    );
    held.rollback().await.unwrap();
    tokio::time::timeout(wait, a_write)
        .await
        .expect("A still waits")
        .unwrap();
    store.pool().close().await;
    db.drop_it().await;
}

/// Two dispatchers, one outbox: each event is one claimant's, oldest first, no other tenant's.
#[tokio::test]
async fn two_dispatchers_claim_each_event_once() {
    let Some(db) = Db::create("shako_dispatch_login").await else {
        return;
    };
    let ids = Ids::new();
    let (one, two) = (db.store().await, db.store().await);
    let mut sent = Vec::new();
    for n in 0..8 {
        let id = uuid::Uuid::now_v7();
        one.append_outbox(ids.event(id, &format!("k{n}")))
            .await
            .unwrap();
        sent.push(id);
    }
    let (i, o) = (ids.inst, ids.org);
    let stranger = OrgId::new();
    assert!(two.claim_outbox(i, stranger).await.unwrap().is_none());
    assert!(
        two.list_outbox_events(i, stranger)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !two.mark_outbox_dispatched(i, stranger, sent[0])
            .await
            .unwrap()
    );

    for id in &sent {
        let (a, b, c, d) = tokio::join!(
            one.claim_outbox(i, o),
            two.claim_outbox(i, o),
            one.claim_outbox(i, o),
            two.claim_outbox(i, o),
        );
        let claimed: Vec<uuid::Uuid> = [a, b, c, d]
            .into_iter()
            .filter_map(|claim| claim.unwrap())
            .map(|event| event.id)
            .collect();
        assert_eq!(claimed, [*id], "one claimant, the oldest event");
        assert!(one.mark_outbox_dispatched(i, o, *id).await.unwrap());
    }
    assert!(two.claim_outbox(i, o).await.unwrap().is_none());

    // A claimant that died: its event is another's once the lease ran out.
    let id = uuid::Uuid::now_v7();
    one.append_outbox(ids.event(id, "late")).await.unwrap();
    assert!(one.claim_outbox(i, o).await.unwrap().is_some());
    assert!(two.claim_outbox(i, o).await.unwrap().is_none());
    sqlx::query(
        "UPDATE shako.outbox SET claimed_until = now() - interval '1 second' WHERE id = $1",
    )
    .bind(id)
    .execute(&db.admin)
    .await
    .unwrap();
    assert_eq!(two.claim_outbox(i, o).await.unwrap().unwrap().id, id);

    one.pool().close().await;
    two.pool().close().await;
    db.drop_it().await;
}
