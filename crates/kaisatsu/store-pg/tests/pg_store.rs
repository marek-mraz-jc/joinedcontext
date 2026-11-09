#![forbid(unsafe_code)]
//! PostgreSQL-only behaviour of `PgStore` (W2): persistence across store instances, forced RLS
//! through the request role, the boot check, encryption at rest, races and LISTEN/NOTIFY.
//! Every test prints `SKIP` and returns without `KAISATSU_TEST_DATABASE_URL`.

use chrono::{Duration, Utc};
use kaisatsu_core::*;
use kaisatsu_crypto::Kek;
use kaisatsu_store_pg::{PgStore, PgStoreConfig, read_password_file, verify_boot_rls};
use sqlx::postgres::PgPoolOptions;
use sqlx::{AssertSqlSafe, PgPool};
use std::collections::HashMap;
use std::sync::Arc;

mod common;

async fn seed(store: &PgStore) {
    for realm in ["dev", "other"] {
        store
            .upsert_realm(RealmConfig {
                id: realm.into(),
                realm: realm.into(),
                smtp_server: Some(SmtpServerConfig {
                    host: "smtp.example.org".into(),
                    password: "smtp-password-123".into(),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await
            .expect("realm");
    }
    for org in ["helsinki", "zilina"] {
        store
            .upsert_organization(Organization {
                id: org.into(),
                realm_id: "dev".into(),
                name: org.into(),
                alias: org.into(),
                enabled: true,
                description: String::new(),
                domains: Vec::new(),
                attributes: HashMap::new(),
                managed_by: None,
            })
            .await
            .expect("org");
        store
            .create_user(UserRepresentation {
                id: format!("user-{org}"),
                realm_id: "dev".into(),
                org_id: Some(org.into()),
                username: format!("alice-{org}"),
                email: Some(format!("alice@{org}.example")),
                ..Default::default()
            })
            .await
            .expect("user");
        store
            .upsert_client(ClientRepresentation {
                id: format!("client-{org}"),
                realm_id: "dev".into(),
                org_id: Some(org.into()),
                client_id: format!("app-{org}"),
                secret: Some(format!("very-secret-{org}")),
                ..Default::default()
            })
            .await
            .expect("client");
        store
            .append_event(AuditEvent {
                id: format!("ev-{org}"),
                realm_id: "dev".into(),
                org_id: Some(org.into()),
                time: 1,
                event_type: "LOGIN".into(),
                client_id: None,
                user_id: None,
                session_id: None,
                ip_address: "192.0.2.1".into(),
                error: None,
                details: Default::default(),
            })
            .await
            .expect("event");
    }
    store
        .create_user(UserRepresentation {
            id: "user-other-realm".into(),
            realm_id: "other".into(),
            username: "alice-elsewhere".into(),
            ..Default::default()
        })
        .await
        .expect("user in another realm");
}

async fn count_as(
    pool: &PgPool,
    realm: Option<&str>,
    orgs: Option<&str>,
    sql: &'static str,
) -> i64 {
    let mut tx = pool.begin().await.expect("begin");
    if let Some(realm) = realm {
        sqlx::query("SELECT set_config('kaisatsu.realm', $1, true)")
            .bind(realm)
            .execute(&mut *tx)
            .await
            .expect("set realm");
    }
    if let Some(orgs) = orgs {
        sqlx::query("SELECT set_config('kaisatsu.orgs', $1, true)")
            .bind(orgs)
            .execute(&mut *tx)
            .await
            .expect("set orgs");
    }
    let n: i64 = sqlx::query_scalar(sql)
        .fetch_one(&mut *tx)
        .await
        .expect("count");
    tx.rollback().await.expect("rollback");
    n
}

#[tokio::test]
async fn data_survives_a_new_store_instance_and_secrets_stay_encrypted() {
    let Some(db) = common::test_db("data_survives_a_new_store_instance").await else {
        return;
    };
    let kek = Kek::generate_for_tests();
    let first = db.store(kek.clone()).await;
    seed(&first).await;
    first
        .upsert_credential(CredentialRecord {
            id: "cred-1".into(),
            realm_id: "dev".into(),
            user_id: "user-helsinki".into(),
            secret_data: "$argon2id$v=19$stored".into(),
            ..Default::default()
        })
        .await
        .expect("credential");
    let keys_before = first.list_signing_keys("dev").await.expect("keys");
    drop(first);

    let second = db.store(kek.clone()).await;
    let realm = second
        .get_realm("dev")
        .await
        .expect("get")
        .expect("realm persisted");
    assert_eq!(
        realm.smtp_server.map(|s| s.password),
        Some("smtp-password-123".to_owned()),
        "the SMTP password survives, decrypted with the KEK"
    );
    let user = second
        .get_user_by_id("dev", None, "user-helsinki")
        .await
        .expect("get")
        .expect("person persisted");
    assert_eq!(user.username, "alice-helsinki");
    let client = second
        .get_client_by_client_id("dev", "app-helsinki")
        .await
        .expect("get")
        .expect("client persisted");
    assert_eq!(client.secret.as_deref(), Some("very-secret-helsinki"));
    assert_eq!(
        second
            .list_credentials("dev", "user-helsinki")
            .await
            .expect("creds")[0]
            .secret_data,
        "$argon2id$v=19$stored"
    );
    let keys_after = second.list_signing_keys("dev").await.expect("keys");
    assert_eq!(
        keys_before.iter().map(|k| &k.kid).collect::<Vec<_>>(),
        keys_after.iter().map(|k| &k.kid).collect::<Vec<_>>(),
        "a restart keeps the signing keys"
    );
    assert_eq!(
        keys_before[0].encrypted_private_key,
        keys_after[0].encrypted_private_key
    );
    let es = keys_after
        .iter()
        .find(|k| k.algorithm == "ES256")
        .expect("es256");
    let token = kaisatsu_crypto::jose::sign_jwt(
        es,
        &kek,
        None,
        &serde_json::json!({"sub": "x", "exp": Utc::now().timestamp() + 60}),
    )
    .expect("a persisted key still signs");
    assert!(!token.is_empty());

    // At rest: no secret in a document column, ciphertext only.
    let su = db.superuser_pool().await;
    let doc: serde_json::Value = sqlx::query_scalar(
        "SELECT doc FROM kaisatsu.applications WHERE client_id = 'app-helsinki'",
    )
    .fetch_one(&su)
    .await
    .expect("raw client row");
    assert!(
        doc.get("secret").is_none(),
        "client secret must not sit in the document"
    );
    let enc: Vec<u8> = sqlx::query_scalar(
        "SELECT secret_enc FROM kaisatsu.applications WHERE client_id = 'app-helsinki'",
    )
    .fetch_one(&su)
    .await
    .expect("ciphertext");
    assert!(!enc.windows(11).any(|w| w == b"very-secret"));
    let realm_doc: String =
        sqlx::query_scalar("SELECT doc::text FROM kaisatsu.realms WHERE realm_id = 'dev'")
            .fetch_one(&su)
            .await
            .expect("raw realm row");
    assert!(!realm_doc.contains("smtp-password-123"));

    // Another KEK cannot read the secrets.
    let wrong = db.store(Kek::generate_for_tests()).await;
    assert!(
        wrong
            .get_client_by_client_id("dev", "app-helsinki")
            .await
            .is_err()
    );
    assert!(wrong.get_realm("dev").await.is_err());
    su.close().await;
    db.drop_db().await;
}

#[tokio::test]
async fn rls_hides_other_realms_and_organizations_from_the_request_role() {
    let Some(db) = common::test_db("rls_hides_other_realms_and_organizations").await else {
        return;
    };
    let store = db.store(Kek::generate_for_tests()).await;
    seed(&store).await;
    let app = db.app_pool().await;

    // No realm set: nothing at all.
    assert_eq!(
        count_as(&app, None, None, "SELECT count(*) FROM kaisatsu.people").await,
        0
    );
    assert_eq!(
        count_as(&app, None, None, "SELECT count(*) FROM kaisatsu.realms").await,
        0
    );
    // A realm sees only its own rows, even when the SQL asks for another realm.
    assert_eq!(
        count_as(
            &app,
            Some("other"),
            Some("*"),
            "SELECT count(*) FROM kaisatsu.people WHERE realm_id = 'dev'"
        )
        .await,
        0
    );
    assert_eq!(
        count_as(
            &app,
            Some("other"),
            Some("*"),
            "SELECT count(*) FROM kaisatsu.people"
        )
        .await,
        1
    );
    // `*` lists realms but opens no other table.
    assert_eq!(
        count_as(
            &app,
            Some("*"),
            Some("*"),
            "SELECT count(*) FROM kaisatsu.realms"
        )
        .await,
        2
    );
    assert_eq!(
        count_as(
            &app,
            Some("*"),
            Some("*"),
            "SELECT count(*) FROM kaisatsu.people"
        )
        .await,
        0
    );
    // An organization-scoped transaction sees only its organization.
    let hel = Some(r#"["helsinki"]"#);
    for sql in [
        "SELECT count(*) FROM kaisatsu.applications",
        "SELECT count(*) FROM kaisatsu.events",
        "SELECT count(*) FROM kaisatsu.people",
        "SELECT count(*) FROM kaisatsu.organizations",
    ] {
        assert_eq!(count_as(&app, Some("dev"), hel, sql).await, 1, "{sql}");
    }
    assert_eq!(
        count_as(
            &app,
            Some("dev"),
            hel,
            "SELECT count(*) FROM kaisatsu.applications WHERE org_id = 'zilina'"
        )
        .await,
        0
    );
    // Without an organization setting an organization-owned row is invisible.
    assert_eq!(
        count_as(
            &app,
            Some("dev"),
            None,
            "SELECT count(*) FROM kaisatsu.applications"
        )
        .await,
        0
    );
    // A garbage organization setting matches nothing rather than everything.
    assert_eq!(
        count_as(
            &app,
            Some("dev"),
            Some("helsinki"),
            "SELECT count(*) FROM kaisatsu.applications"
        )
        .await,
        0
    );

    // Writes outside the scope are refused by WITH CHECK.
    let mut tx = app.begin().await.expect("begin");
    sqlx::query("SELECT set_config('kaisatsu.realm', 'dev', true), set_config('kaisatsu.orgs', '[\"helsinki\"]', true)")
        .execute(&mut *tx)
        .await
        .expect("scope");
    let other_realm = sqlx::query(
        "INSERT INTO kaisatsu.people (realm_id, id, username_lower, doc) VALUES ('other', 'x', 'x', '{}')",
    )
    .execute(&mut *tx)
    .await;
    assert!(other_realm.is_err(), "a row for another realm is refused");
    tx.rollback().await.expect("rollback");
    let mut tx = app.begin().await.expect("begin");
    sqlx::query("SELECT set_config('kaisatsu.realm', 'dev', true), set_config('kaisatsu.orgs', '[\"helsinki\"]', true)")
        .execute(&mut *tx)
        .await
        .expect("scope");
    let other_org = sqlx::query(
        "INSERT INTO kaisatsu.applications (realm_id, id, org_id, client_id, doc)
         VALUES ('dev', 'y', 'zilina', 'sneaky', '{}')",
    )
    .execute(&mut *tx)
    .await;
    assert!(
        other_org.is_err(),
        "a row for another organization is refused"
    );
    tx.rollback().await.expect("rollback");

    // The same through the store API.
    let hel_only = ["helsinki".to_owned()];
    assert!(
        store
            .get_user_by_id("dev", Some(&hel_only), "user-zilina")
            .await
            .expect("get")
            .is_none()
    );
    assert!(
        !store
            .delete_client("dev", Some(&hel_only), "client-zilina")
            .await
            .expect("delete")
    );
    assert!(
        store
            .get_client_by_uuid("dev", None, "client-zilina")
            .await
            .expect("get")
            .is_some()
    );
    assert!(
        store
            .get_user_by_id("other", None, "user-helsinki")
            .await
            .expect("get")
            .is_none(),
        "no cross-realm read by id"
    );
    assert!(matches!(
        store.get_user_by_id("*", None, "user-helsinki").await,
        Err(KaisatsuError::NotFound(_))
    ));
    app.close().await;
    db.drop_db().await;
}

#[tokio::test]
async fn boot_check_refuses_unsafe_roles_and_tables() {
    let Some(db) = common::test_db("boot_check_refuses_unsafe_roles_and_tables").await else {
        return;
    };
    let su = db.superuser_pool().await;
    let err = verify_boot_rls(&su).await.expect_err("superuser refused");
    assert!(err.to_string().contains("SUPERUSER or BYPASSRLS"), "{err}");
    let err = verify_boot_rls(&db.migrate_pool)
        .await
        .expect_err("table owner refused");
    assert!(err.to_string().contains("owns"), "{err}");
    let app = db.app_pool().await;
    verify_boot_rls(&app)
        .await
        .expect("the request role passes");

    // A role with BYPASSRLS is refused.
    let probe = format!(
        "kaisatsu_bypass_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    );
    let probe_pw = uuid::Uuid::new_v4().simple().to_string();
    sqlx::raw_sql(AssertSqlSafe(format!(
        "CREATE ROLE {probe} LOGIN BYPASSRLS PASSWORD '{probe_pw}'"
    )))
    .execute(&su)
    .await
    .expect("create probe role");
    let bypass = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(db.options_as(&probe, &probe_pw))
        .await
        .expect("connect as probe");
    let err = verify_boot_rls(&bypass)
        .await
        .expect_err("BYPASSRLS refused");
    assert!(err.to_string().contains("BYPASSRLS"), "{err}");
    bypass.close().await;
    sqlx::raw_sql(AssertSqlSafe(format!("DROP ROLE {probe}")))
        .execute(&su)
        .await
        .expect("drop probe role");

    // A kaisatsu table without forced RLS is refused, and accepted once forced.
    sqlx::raw_sql("CREATE TABLE kaisatsu.probe (realm_id text); ALTER TABLE kaisatsu.probe ENABLE ROW LEVEL SECURITY;")
        .execute(&db.migrate_pool)
        .await
        .expect("create probe table");
    let err = verify_boot_rls(&app)
        .await
        .expect_err("unforced table refused");
    assert!(err.to_string().contains("probe"), "{err}");
    assert!(
        PgStore::connect_with(db.app_options(), 1, Kek::generate_for_tests())
            .await
            .is_err()
    );
    sqlx::raw_sql("ALTER TABLE kaisatsu.probe FORCE ROW LEVEL SECURITY")
        .execute(&db.migrate_pool)
        .await
        .expect("force");
    verify_boot_rls(&app).await.expect("forced table accepted");

    // A grant outside schema kaisatsu is refused.
    sqlx::raw_sql("CREATE TABLE public.kaisatsu_probe (x int); GRANT SELECT ON public.kaisatsu_probe TO kaisatsu_app;")
        .execute(&su)
        .await
        .expect("foreign grant");
    let err = verify_boot_rls(&app)
        .await
        .expect_err("foreign grant refused");
    assert!(err.to_string().contains("outside schema"), "{err}");
    sqlx::raw_sql("DROP TABLE public.kaisatsu_probe")
        .execute(&su)
        .await
        .expect("drop");

    // A table owned by the request role is refused.
    sqlx::raw_sql("GRANT CREATE ON SCHEMA public TO kaisatsu_app")
        .execute(&su)
        .await
        .expect("grant create");
    sqlx::raw_sql("CREATE TABLE public.kaisatsu_owned (x int)")
        .execute(&app)
        .await
        .expect("app creates a table");
    let err = verify_boot_rls(&app)
        .await
        .expect_err("owned table refused");
    assert!(err.to_string().contains("owns 1 table"), "{err}");
    sqlx::raw_sql(
        "DROP TABLE public.kaisatsu_owned; REVOKE CREATE ON SCHEMA public FROM kaisatsu_app;",
    )
    .execute(&su)
    .await
    .expect("cleanup");

    // Without the schema's tables the store does not start.
    sqlx::raw_sql("DROP SCHEMA kaisatsu CASCADE")
        .execute(&db.migrate_pool)
        .await
        .expect("drop schema");
    let err = verify_boot_rls(&app)
        .await
        .expect_err("missing schema refused");
    assert!(err.to_string().contains("migrate"), "{err}");
    app.close().await;
    su.close().await;
    db.drop_db().await;
}

#[tokio::test]
async fn migrations_are_versioned_and_run_once() {
    let Some(db) = common::test_db("migrations_are_versioned_and_run_once").await else {
        return;
    };
    PgStore::run_migrations(&db.migrate_pool)
        .await
        .expect("second run is a no-op");
    let versions: Vec<i64> = sqlx::query_scalar(
        "SELECT version FROM kaisatsu_migrations._sqlx_migrations ORDER BY version",
    )
    .fetch_all(&db.migrate_pool)
    .await
    .expect("version table");
    assert_eq!(versions, [1, 2, 3]);
    let app = db.app_pool().await;
    assert!(
        sqlx::query("SELECT 1 FROM kaisatsu_migrations._sqlx_migrations")
            .execute(&app)
            .await
            .is_err(),
        "the request role cannot read the migration table"
    );
    // The request role cannot change the schema.
    assert!(
        sqlx::raw_sql("ALTER TABLE kaisatsu.people DISABLE ROW LEVEL SECURITY")
            .execute(&app)
            .await
            .is_err()
    );
    assert!(
        sqlx::raw_sql("DROP POLICY people_rls ON kaisatsu.people")
            .execute(&app)
            .await
            .is_err()
    );
    app.close().await;
    db.drop_db().await;
}

#[tokio::test]
async fn connect_reads_the_password_from_a_file_and_refuses_bad_input() {
    let Some(db) = common::test_db("connect_reads_the_password_from_a_file").await else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("kaisatsu-pw-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let pw_file = dir.join("password");
    std::fs::write(&pw_file, format!("{}\n", db.app_password())).expect("write password file");
    let opts = db.app_options();
    let url = format!(
        "postgresql://kaisatsu_app@{}:{}/{}",
        opts.get_host(),
        opts.get_port(),
        db.name
    );
    let store = PgStore::connect(
        &PgStoreConfig {
            url_without_password: url.clone(),
            password_file: Some(pw_file.clone()),
            max_connections: 0,
        },
        Kek::generate_for_tests(),
    )
    .await
    .expect("connects with the password file");
    assert!(store.list_realms().await.expect("list").is_empty());

    // The same, correct password inside the URL (argv) or its query is refused before connecting.
    for with_password in [
        format!(
            "postgresql://kaisatsu_app:{}@{}:{}/{}",
            db.app_password(),
            opts.get_host(),
            opts.get_port(),
            db.name
        ),
        format!("{url}?password={}", db.app_password()),
    ] {
        let refused = PgStore::connect(
            &PgStoreConfig {
                url_without_password: with_password,
                password_file: None,
                max_connections: 1,
            },
            Kek::generate_for_tests(),
        )
        .await
        .err()
        .expect("a password in the URL is refused");
        assert!(!refused.to_string().contains(db.app_password()));
    }

    let empty = dir.join("empty");
    std::fs::write(&empty, "  \n").expect("write");
    assert!(read_password_file(&empty).is_err());
    assert!(read_password_file(&dir.join("missing")).is_err());
    let bad_url = PgStore::connect(
        &PgStoreConfig {
            url_without_password: "not a url".into(),
            ..Default::default()
        },
        Kek::generate_for_tests(),
    )
    .await;
    assert!(bad_url.is_err());
    let wrong_pw = dir.join("wrong");
    std::fs::write(&wrong_pw, "wrong-password").expect("write");
    let refused = PgStore::connect(
        &PgStoreConfig {
            url_without_password: url,
            password_file: Some(wrong_pw),
            max_connections: 1,
        },
        Kek::generate_for_tests(),
    )
    .await;
    assert!(refused.is_err());
    assert_eq!(PgStoreConfig::default().max_connections, 5);
    std::fs::remove_dir_all(&dir).expect("cleanup");
    db.drop_db().await;
}

#[test]
fn connect_options_refuse_a_password_in_the_url() {
    let config = |url: &str| PgStoreConfig {
        url_without_password: url.into(),
        ..Default::default()
    };
    for ok in [
        "postgresql://kaisatsu_app@db:5432/kaisatsu",
        "postgres://db/kaisatsu?sslmode=require&application_name=kaisatsu",
        "postgres://kaisatsu_app@db/kaisatsu?options=-c%20search_path%3Dx",
    ] {
        assert!(config(ok).connect_options().is_ok(), "{ok}");
    }
    for refused in [
        "postgresql://kaisatsu_app:secret@db:5432/kaisatsu",
        "postgresql://kaisatsu_app:@db/kaisatsu",
        "postgres://kaisatsu_app@db/kaisatsu?sslmode=require&Password=secret",
        "postgres://kaisatsu_app@db/kaisatsu?password",
    ] {
        let err = config(refused).connect_options().expect_err(refused);
        assert!(!err.to_string().contains("secret"));
    }
    assert!(
        PgStoreConfig {
            password_file: Some("/nonexistent/kaisatsu/password".into()),
            ..Default::default()
        }
        .connect_options()
        .is_err()
    );
}

#[tokio::test]
async fn concurrent_code_redemption_has_exactly_one_winner() {
    let Some(db) = common::test_db("concurrent_code_redemption").await else {
        return;
    };
    let store = Arc::new(db.store(Kek::generate_for_tests()).await);
    store
        .upsert_realm(RealmConfig {
            id: "dev".into(),
            realm: "dev".into(),
            ..Default::default()
        })
        .await
        .expect("realm");
    let now = Utc::now();
    store
        .put_auth_code(AuthorizationCodeRecord {
            code_hash: "race".into(),
            realm_id: "dev".into(),
            org_id: None,
            client_id: "portal".into(),
            user_id: "u".into(),
            sso_session_id: "s".into(),
            redirect_uri: "https://p/cb".into(),
            scope: "openid".into(),
            mapper_scopes: Vec::new(),
            code_challenge: "c".into(),
            code_challenge_method: "S256".into(),
            nonce: None,
            acr: "1".into(),
            amr: Vec::new(),
            dpop_jkt: None,
            auth_time: 1,
            consumed: false,
            created_at: now,
            expires_at: now + Duration::seconds(60),
        })
        .await
        .expect("code");
    let mut handles = Vec::new();
    for _ in 0..8 {
        let s = store.clone();
        handles.push(tokio::spawn(async move {
            s.consume_auth_code("dev", "race").await
        }));
    }
    let mut winners = 0;
    let mut replays = 0;
    for h in handles {
        let (rec, replay) = h.await.expect("join").expect("consume");
        assert!(rec.is_some());
        if replay {
            replays += 1;
        } else {
            winners += 1;
        }
    }
    assert_eq!((winners, replays), (1, 7));
    db.drop_db().await;
}

#[tokio::test]
async fn sweeper_and_change_notifications() {
    let Some(db) = common::test_db("sweeper_and_change_notifications").await else {
        return;
    };
    let store = db.store(Kek::generate_for_tests()).await;
    let listener = store.spawn_change_listener();
    let start = store.reload_generation();
    store
        .upsert_realm(RealmConfig {
            id: "dev".into(),
            realm: "dev".into(),
            ..Default::default()
        })
        .await
        .expect("realm");
    store.notify_changed("dev").await.expect("notify");
    let mut seen = false;
    for _ in 0..50 {
        if store.reload_generation() >= start + 2 {
            seen = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(seen, "a NOTIFY reaches the listener");
    listener.abort();

    let now = Utc::now();
    for (id, at) in [
        ("old", now - Duration::minutes(1)),
        ("new", now + Duration::minutes(5)),
    ] {
        store
            .mark_token_revoked(RevokedTokenRecord {
                jti_hash: id.into(),
                realm_id: "dev".into(),
                expires_at: at,
            })
            .await
            .expect("revoked");
    }
    let n = store.sweep_expired_batch(10).await.expect("sweep");
    assert_eq!(n, 1);
    let su = db.superuser_pool().await;
    let left: Vec<String> = sqlx::query_scalar("SELECT token_hash FROM kaisatsu.revoked_token")
        .fetch_all(&su)
        .await
        .expect("rows");
    assert_eq!(left, ["new"]);
    su.close().await;
    db.drop_db().await;
}
