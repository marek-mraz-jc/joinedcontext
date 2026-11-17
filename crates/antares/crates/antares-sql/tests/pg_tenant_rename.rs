// SPDX-License-Identifier: EUPL-1.2
//! The Tenant rename (T-7039, migration 0037): every row of one Tenant moves to
//! a new name with nothing lost, the broker's own documents follow, a rollback
//! gives the digest back, and the functions refuse every other caller and
//! target. Needs a live PostGIS; skips (passes vacuously, loudly) when
//! ANTARES_TEST_DATABASE_URL is unset, as tests/pg.rs does.

#![cfg(feature = "postgres")]

use antares_model::TenantId;
use antares_sql::store::pg;
use sqlx::PgPool;

macro_rules! require_db {
    () => {
        match std::env::var("ANTARES_TEST_DATABASE_URL") {
            Ok(url) => url,
            Err(_) => {
                eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
                return;
            }
        }
    };
}

fn run_id() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos()
}

/// A project-key name for `space`, two fixed-shape ids (ARCH/TEN-10).
fn keyed(n: u128, space: &str) -> String {
    let id = |salt: u128| {
        let b32 = b"abcdefghijklmnopqrstuvwxyz234567";
        let v = n.wrapping_mul(2_654_435_761).wrapping_add(salt);
        (0..26)
            .rev()
            .map(|i| char::from(b32[((v >> (i * 5)) & if i == 25 { 7 } else { 31 }) as usize]))
            .collect::<String>()
    };
    format!("{}__{}__{space}", id(1), id(2))
}

/// One Tenant holding every row kind the broker writes (the seeding of
/// tests/pg.rs `purge_tenant_empties_every_tenant_table`), plus broker
/// documents and a registration naming it.
async fn seed(pool: &PgPool, t: &TenantId) {
    use antares_store::{CurrentStateDriver, Kind, TemporalDriver};
    let store =
        antares_sql::store::any::AnyStore::Pg(antares_sql::store::any::PgBackend::new(pool.clone()));
    store.set_outbox(true);
    let doc = |id: &str| {
        serde_json::json!({
            "id": id, "type": "Test",
            "createdAt": "2026-08-04T09:00:00Z", "modifiedAt": "2026-08-04T09:00:00Z",
            "n": {"type": "Property", "value": 1, "observedAt": "2026-08-04T09:00:00Z"}
        })
    };
    pg::ensure_tenant(pool, t).await.expect("tenant row");
    for kind in [
        Kind::Entity,
        Kind::Subscription,
        Kind::Registration,
        Kind::CSourceSubscription,
        Kind::Snapshot,
        Kind::EntityMap,
        Kind::DistSub,
        Kind::DeadLetter,
        Kind::View,
    ] {
        let id = "urn:x:1";
        let d = if kind == Kind::Registration {
            serde_json::json!({"id": id, "type": "ContextSourceRegistration",
                "information": [{"entities": [{"type": "Test"}]}],
                "endpoint": "http://127.0.0.1:9", "tenant": t.as_str()})
        } else if kind == Kind::Subscription {
            let mut s = doc(id);
            s["notification"] = serde_json::json!({"endpoint": {"uri": "http://127.0.0.1:9",
                "receiverInfo": [{"key": "NGSILD-Tenant", "value": t.as_str()}]}});
            s
        } else {
            doc(id)
        };
        assert!(
            CurrentStateDriver::create(&store, t, kind, id, d)
                .await
                .expect("create"),
            "{kind:?}"
        );
    }
    CurrentStateDriver::context_put(
        &store,
        Some(t),
        &format!("ctx-{}", t.as_str()),
        serde_json::json!({"url": format!("http://b.example/ngsi-ld/v1/jsonldContexts/ctx-{}", t.as_str()),
                           "localId": format!("ctx-{}", t.as_str()), "kind": "Hosted",
                           "owner": t.as_str(), "createdAt": "2026-08-04T09:00:00Z",
                           "body": {"@context": {"n": "https://x/n"}}}),
    )
    .await
    .expect("hosted @context");
    TemporalDriver::create(&store, t, "urn:x:1", doc("urn:x:1"))
        .await
        .expect("temporal");
    for value in [2, 3] {
        // the second value for the same instant is a correction: attr_versions
        store
            .temporal_append(
                t,
                "urn:x:1",
                &doc("urn:x:1"),
                &serde_json::json!({"n": [{"type": "Property", "value": value,
                    "observedAt": "2026-08-04T09:01:00Z", "instanceId": "urn:i:1"}]}),
            )
            .await
            .expect("append");
    }
    for stmt in [
        "INSERT INTO iri_ids (tenant_id, iri) VALUES ($1, 'urn:x:1') ON CONFLICT DO NOTHING",
        "INSERT INTO entity_id_remap (tenant_id, old_id, new_id) VALUES ($1, 'urn:x:old:1', 'urn:x:1')",
        "INSERT INTO attr_rows (tenant_id, entity_no, attr_no, instance_no, observed_at,
                                created_at, modified_at, data)
         VALUES ($1, iri_no($1, 'urn:x:1'), 1, gen_random_uuid(), now(), now(), now(), '{}')",
        // the broker's own documents naming the Tenant: a view's member and a
        // policy document keyed by it
        "INSERT INTO views (tenant_id, id, doc) VALUES ('antares-views', 'v-' || $1,
            jsonb_build_object('id', 'v-' || $1, 'tenant', $1, 'text', 'about ' || $1))",
        "INSERT INTO views (tenant_id, id, doc) VALUES ('antares-policy', $1,
            jsonb_build_object('id', $1, 'rules', jsonb_build_array()))",
    ] {
        sqlx::query(stmt)
            .bind(t.as_str())
            .execute(pool)
            .await
            .expect(stmt);
    }
}

async fn digest(pool: &PgPool, tenant: &str) -> Vec<(String, i64, String)> {
    sqlx::query_as(
        "SELECT tbl, row_count, digest::text FROM antares_tenant_digest($1) ORDER BY tbl",
    )
    .bind(tenant)
    .fetch_all(pool)
    .await
    .expect("digest")
}

async fn rows_of(pool: &PgPool, tenant: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT coalesce(sum((xpath('/row/n/text()', query_to_xml(
                  format('SELECT count(*) AS n FROM %I WHERE tenant_id = %L', tbl, $1),
                  false, true, '')))[1]::text::bigint), 0)::bigint
           FROM (SELECT tbl FROM tenant_tables() UNION ALL SELECT 'jsonld_contexts') s",
    )
    .bind(tenant)
    .fetch_one(pool)
    .await
    .expect("rows")
}

/// TEN-15, T-7039: a Tenant holding every row kind moves whole: the digest of
/// the new name equals the old one's, its rows are all there, the old name
/// holds none, the broker documents name the new one, the tombstone and the
/// stored Organization are written; the rollback gives the digest back.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_moves_whole_and_rolls_back() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect+migrate");
    let n = run_id();
    let old = format!("ren{n}-air");
    let t = TenantId::new(&old).expect("tenant");
    seed(&pool, &t).await;
    // a neighbour whose name extends the old one is left alone
    let near = TenantId::new(&format!("{old}x")).expect("tenant");
    seed(&pool, &near).await;
    let new = keyed(n, "air");
    let (before, rows, near_before) = (
        digest(&pool, &old).await,
        rows_of(&pool, &old).await,
        digest(&pool, near.as_str()).await,
    );
    let tables: Vec<&str> = before.iter().map(|(t, ..)| t.as_str()).collect();
    for table in [
        "attr_instances",
        "attr_rows",
        "attr_versions",
        "csource_registrations",
        "entities",
        "entity_values",
        "jsonld_contexts",
        "subscriptions",
        "temporal_entities",
        "reserved:views",
    ] {
        assert!(tables.contains(&table), "{table} seeded: {tables:?}");
    }

    let moved: serde_json::Value = sqlx::query_scalar("SELECT antares_tenant_rename($1, $2)")
        .bind(&old)
        .bind(&new)
        .fetch_one(&pool)
        .await
        .expect("renamed");
    assert_eq!(moved["text_still_naming_old"], 1, "the view's text: {moved}");
    assert_eq!(digest(&pool, &new).await, before);
    assert_eq!(rows_of(&pool, &new).await, rows);
    assert_eq!(rows_of(&pool, &old).await, 0);
    assert_eq!(digest(&pool, near.as_str()).await, near_before);
    let (tomb, org): (String, Option<String>) = sqlx::query_as(
        "SELECT r.new, t.organization_id FROM tenant_rename r JOIN tenants t ON t.tenant_id = r.new
          WHERE r.old = $1",
    )
    .bind(&old)
    .fetch_one(&pool)
    .await
    .expect("tombstone");
    assert_eq!(tomb, new);
    assert_eq!(org.as_deref(), Some(&new[..26]));
    let named: (String, String, String) = sqlx::query_as(
        "SELECT (SELECT doc ->> 'tenant' FROM views WHERE tenant_id = 'antares-views' AND id = 'v-' || $1),
                (SELECT id FROM views WHERE tenant_id = 'antares-policy' AND id = $2),
                (SELECT body ->> 'owner' FROM jsonld_contexts WHERE tenant_id = $2)",
    )
    .bind(&old)
    .bind(&new)
    .fetch_one(&pool)
    .await
    .expect("documents");
    assert_eq!(named, (new.clone(), new.clone(), new.clone()));

    sqlx::query("SELECT antares_tenant_rename_rollback($1)")
        .bind(&new)
        .execute(&pool)
        .await
        .expect("rolled back");
    assert_eq!(digest(&pool, &old).await, before);
    assert_eq!(rows_of(&pool, &new).await, 0);
    let tombs: i64 = sqlx::query_scalar("SELECT count(*) FROM tenant_rename WHERE old = $1")
        .bind(&old)
        .fetch_one(&pool)
        .await
        .expect("tombstones");
    assert_eq!(tombs, 0);
}

/// The rename refuses a target holding rows, a tombstoned or reserved name,
/// the default Tenant, and a rollback nobody recorded; nothing moves.
#[tokio::test(flavor = "multi_thread")]
async fn the_rename_refuses_a_taken_target_and_names_it_must_not_move() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect+migrate");
    let n = run_id();
    let (a, b) = (format!("rena{n}"), format!("renb{n}"));
    for name in [&a, &b] {
        seed(&pool, &TenantId::new(name).expect("tenant")).await;
    }
    let before = digest(&pool, &a).await;
    let refused = |old: String, new: String| {
        let pool = pool.clone();
        async move {
            sqlx::query("SELECT antares_tenant_rename($1, $2)")
                .bind(old)
                .bind(new)
                .execute(&pool)
                .await
                .expect_err("refused")
                .to_string()
        }
    };
    assert!(refused(a.clone(), b.clone()).await.contains("holds rows"));
    assert!(refused(a.clone(), "antares-views".into()).await.contains("keep their names"));
    assert!(refused("default".into(), keyed(n, "d")).await.contains("keep their names"));
    assert!(refused(a.clone(), "a b".into()).await.contains("letters, digits"));
    assert!(refused(format!("none{n}"), keyed(n, "x")).await.contains("no Tenant"));
    let new = keyed(n, "b");
    sqlx::query("SELECT antares_tenant_rename($1, $2)")
        .bind(&b)
        .bind(&new)
        .execute(&pool)
        .await
        .expect("renamed");
    assert!(refused(a.clone(), b.clone()).await.contains("never used again"));
    let stray = sqlx::query("SELECT antares_tenant_rename_rollback($1)")
        .bind(keyed(n, "never"))
        .execute(&pool)
        .await
        .expect_err("refused")
        .to_string();
    assert!(stray.contains("no rename"), "{stray}");
    assert_eq!(digest(&pool, &a).await, before);
}

/// The functions run only as a role that bypasses row-level security: the
/// broker's own role can never move a Tenant or read every row of one.
#[tokio::test(flavor = "multi_thread")]
async fn the_rename_refuses_a_role_under_row_level_security() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect+migrate");
    let n = run_id();
    let old = format!("renr{n}");
    seed(&pool, &TenantId::new(&old).expect("tenant")).await;
    for sql in [
        "SELECT antares_tenant_rename($1, $2)",
        "SELECT antares_tenant_rename_rollback($2), $1::text",
        "SELECT *, $2::text FROM antares_tenant_digest($1)",
    ] {
        let role = format!("renamerole{n}");
        let mut tx = pool.begin().await.expect("tx");
        for stmt in [
            format!("CREATE ROLE {role} NOSUPERUSER NOBYPASSRLS"),
            format!(
                "GRANT EXECUTE ON FUNCTION tenant_rename_guard(text, text), tenant_rename_apply(text, text), \
                 antares_tenant_rename(text, text), antares_tenant_rename_rollback(text), \
                 antares_tenant_digest(text) TO {role}"
            ),
            format!("SET LOCAL ROLE {role}"),
        ] {
            // the role name is this test's own
            sqlx::query(sqlx::AssertSqlSafe(stmt.clone()))
                .execute(&mut *tx)
                .await
                .expect(&stmt);
        }
        let err = sqlx::query(sql)
            .bind(&old)
            .bind(keyed(n, "r"))
            .execute(&mut *tx)
            .await
            .expect_err("refused")
            .to_string();
        assert!(err.contains("bypasses row-level security"), "{sql}: {err}");
    }
    assert!(rows_of(&pool, &old).await > 0);
}
