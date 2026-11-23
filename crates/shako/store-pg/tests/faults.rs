#![forbid(unsafe_code)]
#![allow(clippy::disallowed_methods)] // Tests read their database URL; jc runs none of this.
//! T-7664: what the store answers when its configuration, its connection or its database fails,
//! and the copy of manifests a release before T-7523 left in the column.
//!
//! `SHAKO_TEST_DATABASE_URL` names an admin (superuser) connection. Unset: the database tests
//! print `SKIP`; the configuration tests run anyway.

use serde_json::json;
use shako_core::model::{AuditRecord, InstanceRecord};
use shako_core::{InstanceId, OrgId, SecretStore, ShakoError, Store, sha256_hex};
use shako_store_pg::{PgSecretStore, PgStore, PgStoreConfig};
use sqlx::AssertSqlSafe;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use std::str::FromStr;
use std::sync::Mutex;

/// A database of its own, migrated, and two logins in it: one in `shako_app`, one outside it.
struct Db {
    server: PgPool,
    admin: PgPool,
    member: PgConnectOptions,
    outsider: PgConnectOptions,
    name: String,
}

impl Db {
    async fn create() -> Option<Self> {
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
        let name = format!("shako_faults_{}", uuid::Uuid::new_v4().simple());
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
            "CREATE ROLE {name}_m LOGIN INHERIT NOSUPERUSER NOBYPASSRLS PASSWORD '{pw}';
             CREATE ROLE {name}_o LOGIN INHERIT NOSUPERUSER NOBYPASSRLS PASSWORD '{pw}';
             GRANT shako_app TO {name}_m;"
        )))
        .execute(&admin)
        .await
        .expect("create logins");
        let login = |role: &str| options.clone().database(&name).username(role).password(&pw);
        Some(Self {
            member: login(&format!("{name}_m")),
            outsider: login(&format!("{name}_o")),
            server,
            admin,
            name,
        })
    }

    async fn drop_it(self) {
        self.admin.close().await;
        // DROP DATABASE refuses to run inside the transaction a multi-statement query opens.
        let name = &self.name;
        for sql in [
            format!("DROP DATABASE {name} WITH (FORCE)"),
            format!("DROP ROLE {name}_m"),
            format!("DROP ROLE {name}_o"),
        ] {
            sqlx::raw_sql(AssertSqlSafe(sql))
                .execute(&self.server)
                .await
                .expect("drop test database and logins");
        }
    }
}

fn storage_message(err: &ShakoError) -> &str {
    match err {
        ShakoError::Storage(m) | ShakoError::RlsNotEnforced(m) => m,
        other => panic!("expected a storage refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn the_password_file_is_read_trimmed_and_refused_when_missing_or_empty() {
    let dir = std::env::temp_dir().join(format!("shako-faults-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let (file, empty) = (dir.join("password"), dir.join("empty"));
    std::fs::write(&file, "  s3cret\n").expect("password file");
    std::fs::write(&empty, " \n").expect("empty file");

    let config = PgStoreConfig::from_secret_file("postgres://shako@db/shako", &file, 0)
        .await
        .expect("read");
    assert_eq!(config.url_without_password, "postgres://shako@db/shako");
    assert_eq!(
        config.password.as_deref().map(String::as_str),
        Some("s3cret")
    );
    assert_eq!(config.max_connections, 1);
    let config = PgStoreConfig::from_secret_file("postgres://db", &file, 99)
        .await
        .expect("read");
    assert_eq!(config.max_connections, 20);

    let err = PgStoreConfig::from_secret_file("postgres://db", &empty, 4)
        .await
        .expect_err("empty");
    assert!(
        matches!(&err, ShakoError::BadRequest(m) if m == "db password file is empty"),
        "{err:?}"
    );
    let missing = dir.join("missing");
    let err = PgStoreConfig::from_secret_file("postgres://db", &missing, 4)
        .await
        .expect_err("missing");
    let message = storage_message(&err);
    assert!(
        message.starts_with("cannot read db password file") && message.contains("missing"),
        "{message}"
    );
    std::fs::remove_dir_all(&dir).expect("clean up");
}

#[tokio::test]
async fn a_bad_url_or_a_refused_socket_is_a_storage_error_naming_the_step() {
    let config = PgStoreConfig {
        url_without_password: "not a url".into(),
        password: None,
        max_connections: 1,
    };
    let Err(err) = PgStore::connect(&config).await else {
        panic!("a bad url connects");
    };
    assert!(
        storage_message(&err).starts_with("invalid pg url"),
        "{err:?}"
    );

    // A bound socket that never listens refuses every connection.
    let socket = tokio::net::TcpSocket::new_v4().expect("socket");
    socket
        .bind("127.0.0.1:0".parse().expect("addr"))
        .expect("bind");
    let port = socket.local_addr().expect("port").port();
    let config = PgStoreConfig {
        url_without_password: format!("postgres://shako@127.0.0.1:{port}/shako"),
        password: Some("unused".to_owned().into()),
        max_connections: 1,
    };
    let Err(err) = PgStore::connect(&config).await else {
        panic!("a refused socket connects");
    };
    assert!(
        storage_message(&err).starts_with("pg connect failed"),
        "{err:?}"
    );
    drop(socket);
}

#[tokio::test]
async fn a_login_outside_shako_app_is_refused_at_boot_and_in_a_tenant_transaction() {
    let Some(db) = Db::create().await else {
        return;
    };
    let Err(err) = PgStore::connect_with(db.outsider.clone(), 1).await else {
        panic!("a login outside shako_app starts");
    };
    let message = storage_message(&err);
    assert!(
        message.contains(&format!("{}_o", db.name)) && message.contains("member of shako_app"),
        "{message}"
    );
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(db.outsider.clone())
        .await
        .expect("connect");
    let store = PgStore::from_pool_unchecked(pool.clone());
    let err = store
        .list_audit(InstanceId::new(), OrgId::new())
        .await
        .expect_err("cannot become shako_app");
    assert!(
        storage_message(&err).starts_with("set_config failed"),
        "{err:?}"
    );

    // Only the schema owner migrates.
    let err = PgStore::run_migrations(&pool)
        .await
        .expect_err("the outsider migrates");
    assert!(
        storage_message(&err).starts_with("migration failed"),
        "{err:?}"
    );
    pool.close().await;
    db.drop_it().await;
}

#[tokio::test]
async fn a_closed_pool_fails_each_call_with_its_step() {
    let Some(db) = Db::create().await else {
        return;
    };
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(db.member.clone())
        .await
        .expect("connect");
    let store = PgStore::from_pool_unchecked(pool.clone());
    let secrets = PgSecretStore::new(
        &store,
        InstanceId::new(),
        &kaisatsu_crypto::Kek::from_bytes([7; 32]),
    )
    .expect("secret store");
    pool.close().await;

    let err = store.verify_rls_enforced().await.expect_err("closed");
    assert!(
        storage_message(&err).starts_with("inspect pg_roles failed"),
        "{err:?}"
    );
    let (inst, org) = (InstanceId::new(), OrgId::new());
    let err = store.list_audit(inst, org).await.expect_err("closed");
    assert!(
        storage_message(&err).starts_with("begin tx failed"),
        "{err:?}"
    );
    let err = secrets.get_secret(org, "hook").await.expect_err("closed");
    assert!(
        storage_message(&err).starts_with("begin tx failed"),
        "{err:?}"
    );
    let err = secrets
        .put_secret(org, "hook", "v")
        .await
        .expect_err("closed");
    assert!(
        storage_message(&err).starts_with("begin tx failed"),
        "{err:?}"
    );
    db.drop_it().await;
}

/// Each step's refusal, forced by revoking in this test's own database the one privilege it
/// needs (catalogue grants are per database), then granting it back.
#[tokio::test]
async fn a_revoked_privilege_fails_the_step_that_needs_it() {
    let Some(db) = Db::create().await else {
        return;
    };
    let store = PgStore::connect_with(db.member.clone(), 2)
        .await
        .expect("store");
    let (inst, org) = (InstanceId::new(), OrgId::new());
    let secrets = PgSecretStore::new(&store, inst, &kaisatsu_crypto::Kek::from_bytes([7; 32]))
        .expect("secret store");
    secrets.put_secret(org, "hook", "v1").await.expect("put");
    let audit = || AuditRecord {
        id: uuid::Uuid::now_v7(),
        instance_id: inst,
        org_id: org,
        kind: "REPO_CREATE".into(),
        operation: "CreateRepository".into(),
        actor_id: "alice".into(),
        target: "repos/x".into(),
        decision: "allow".into(),
        request_id: "req".into(),
        created_at: jc_time::TimestampTz::now(),
    };
    let instance = || InstanceRecord::default_for(inst, org, "git.city.example");
    let sql = |q: String| {
        let admin = db.admin.clone();
        async move {
            sqlx::raw_sql(AssertSqlSafe(q))
                .execute(&admin)
                .await
                .expect("privilege change");
        }
    };
    let cases: [(&str, &str, &str); 8] = [
        ("SELECT ON shako.secret", "shako_app", "secret read failed"),
        (
            "INSERT, UPDATE ON shako.secret",
            "shako_app",
            "secret write failed",
        ),
        ("INSERT ON shako.audit", "shako_app", "audit write failed"),
        (
            "INSERT ON shako.store_log",
            "shako_app",
            "store log write failed",
        ),
        (
            "SELECT ON pg_catalog.pg_class",
            "PUBLIC",
            "inspect table owner failed",
        ),
        (
            "SELECT ON information_schema.role_table_grants",
            "PUBLIC",
            "inspect role grants failed",
        ),
        (
            "SELECT ON pg_catalog.pg_policy",
            "PUBLIC",
            "inspect pg_class failed",
        ),
        (
            "EXECUTE ON FUNCTION pg_catalog.pg_has_role(name, oid, text)",
            "PUBLIC",
            "inspect shako_app failed",
        ),
    ];
    for (i, (privilege, grantee, step)) in cases.into_iter().enumerate() {
        sql(format!("REVOKE {privilege} FROM {grantee}")).await;
        let err = match i {
            0 => secrets.get_secret(org, "hook").await.map(|_| ()),
            1 => secrets.put_secret(org, "hook", "v2").await,
            2 => store.append_audit(audit()).await,
            3 => store.upsert_instance(instance()).await.map(|_| ()),
            _ => store.verify_rls_enforced().await,
        }
        .expect_err(step);
        sql(format!("GRANT {privilege} TO {grantee}")).await;
        assert!(storage_message(&err).starts_with(step), "{step}: {err:?}");
    }
    // Granted back, every step answers again.
    store.verify_rls_enforced().await.expect("verify");
    assert_eq!(
        secrets
            .get_secret(org, "hook")
            .await
            .expect("get")
            .as_deref(),
        Some("v1")
    );
    store.append_audit(audit()).await.expect("audit");
    store.upsert_instance(instance()).await.expect("instance");
    db.drop_it().await;
}

#[tokio::test]
async fn manifests_left_in_the_column_are_copied_once_and_a_damaged_one_stays() {
    let Some(db) = Db::create().await else {
        return;
    };
    let store = PgStore::connect_with(db.member.clone(), 2)
        .await
        .expect("store");
    let (inst, org) = (InstanceId::new(), OrgId::new());
    let manifest = |n: u8| format!("{{\"schemaVersion\":2,\"n\":{n}}}").into_bytes();
    let digest_of = |bytes: &[u8]| format!("sha256:{}", sha256_hex(bytes));
    let (logged, kept) = (manifest(1), manifest(2));
    for (owner, tag, bytes, digest) in [
        ("city", "logged", &logged, digest_of(&logged)),
        (
            "city",
            "damaged",
            &kept,
            format!("sha256:{}", "0".repeat(64)),
        ),
        ("Bad Owner", "named", &kept, digest_of(&kept)),
        ("city", "kept", &kept, digest_of(&kept)),
    ] {
        let record = json!({
            "id": uuid::Uuid::now_v7(), "instanceId": inst, "orgId": org,
            "packageId": uuid::Uuid::now_v7(), "owner": owner, "packageType": "container",
            "packageName": "image", "version": tag, "digest": digest,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "subjectDigest": null, "artifactType": null, "sizeBytes": bytes.len(),
            "signed": false, "createdAt": "2026-11-01T00:00:00Z", "manifestBytes": bytes,
        });
        sqlx::query("INSERT INTO shako.store_log (instance_id, org_id, op) VALUES ($1, $2, $3)")
            .bind(inst.as_uuid())
            .bind(org.as_uuid())
            .bind(json!({"op": "upsert_package_version", "args": record}))
            .execute(&db.admin)
            .await
            .expect("log entry");
    }
    assert_eq!(store.hydrate(inst, org).await.expect("replay"), 4);

    // A write that fails stops the copy and copies nothing.
    let refused = store
        .copy_manifests(inst, org, |_, _, _| async {
            Err(ShakoError::Storage("disk full".into()))
        })
        .await
        .expect_err("write fails");
    assert_eq!(storage_message(&refused), "disk full");
    assert_eq!(
        store
            .get_quota_usage(inst, org)
            .await
            .expect("quota")
            .package_bytes,
        0
    );

    let written = Mutex::new(Vec::new());
    let copy = || {
        store.copy_manifests(inst, org, |owner, digest, bytes| {
            written.lock().expect("lock").push((owner, digest, bytes));
            async { Ok(()) }
        })
    };
    assert_eq!(copy().await.expect("copy"), 2);
    assert_eq!(copy().await.expect("again"), 0, "it may run again");
    let written = written.into_inner().expect("lock");
    assert_eq!(
        written,
        [
            ("city".to_owned(), digest_of(&logged), logged.clone()),
            ("city".to_owned(), digest_of(&kept), kept.clone()),
        ]
    );
    let quota = store.get_quota_usage(inst, org).await.expect("quota");
    assert_eq!(quota.package_bytes, (logged.len() + kept.len()) as u64);
    let blob = store
        .get_blob(inst, org, "city", Some("image"), &digest_of(&kept))
        .await
        .expect("blob");
    assert_eq!(blob.map(|b| b.size_bytes), Some(kept.len() as u64));
    db.drop_it().await;
}
