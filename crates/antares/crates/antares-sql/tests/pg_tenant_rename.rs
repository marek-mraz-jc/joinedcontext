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

/// A database of the test's own, migrated: the rename locks every Tenant table
/// for up to 300 ms per attempt, which in the shared test database times out a
/// parallel test's partition create or drop (55P03).
macro_rules! own_db {
    ($tag:literal) => {
        match std::env::var("ANTARES_TEST_DATABASE_URL") {
            Ok(shared) => OwnDb::new(&shared, $tag).await,
            Err(_) => {
                eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
                return;
            }
        }
    };
}

struct OwnDb {
    admin: PgPool,
    name: String,
    pool: PgPool,
}

impl OwnDb {
    async fn new(shared: &str, tag: &str) -> Self {
        let admin = pg::connect(shared, 2).await.expect("connect+migrate");
        let name = format!("antares_rename_{tag}_{}", run_id());
        // the name is this test's own: a literal tag and a number
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
            .execute(&admin)
            .await
            .expect("own database");
        let (base, _) = shared.rsplit_once('/').expect("a database URL");
        let pool = pg::connect(&format!("{base}/{name}"), 5)
            .await
            .expect("connect+migrate the own database");
        Self { admin, name, pool }
    }

    async fn drop(self) {
        self.pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE {} WITH (FORCE)",
            self.name
        )))
        .execute(&self.admin)
        .await
        .expect("drop own database");
    }
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
    let store = antares_sql::store::any::AnyStore::Pg(antares_sql::store::any::PgBackend::new(
        pool.clone(),
    ));
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
    let db = own_db!("whole");
    let pool = db.pool.clone();
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
    assert_eq!(
        moved["text_still_naming_old"], 1,
        "the view's text: {moved}"
    );
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
    db.drop().await;
}

/// TEN-15: the renamed-away name answers as an unknown Tenant to the store
/// itself, whatever the request door is set to: a write under it is refused
/// instead of re-creating the name, so the rollback stays open.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_under_a_renamed_away_name_is_refused_and_the_rollback_stays_open() {
    use antares_store::{CurrentStateDriver, Kind};
    let db = own_db!("tomb");
    let pool = db.pool.clone();
    let n = run_id();
    let old = format!("tomb{n}-air");
    let t = TenantId::new(&old).expect("tenant");
    seed(&pool, &t).await;
    let new = keyed(n, "air");
    sqlx::query("SELECT antares_tenant_rename($1, $2)")
        .bind(&old)
        .bind(&new)
        .execute(&pool)
        .await
        .expect("renamed");

    let store = antares_sql::store::any::AnyStore::Pg(antares_sql::store::any::PgBackend::new(
        pool.clone(),
    ));
    let doc = serde_json::json!({"id": "urn:x:late", "type": "Test"});
    let late = CurrentStateDriver::create(&store, &t, Kind::Entity, "urn:x:late", doc).await;
    assert!(
        matches!(late, Err(antares_model::NgsiError::NonexistentTenant(_))),
        "a write re-created the renamed-away name: {late:?}"
    );
    let ctx = CurrentStateDriver::context_put(
        &store,
        Some(&t),
        "ctx-late",
        serde_json::json!({"localId": "ctx-late", "kind": "Hosted", "owner": old,
                           "body": {"@context": {"n": "https://x/n"}}}),
    )
    .await;
    assert!(
        matches!(ctx, Err(antares_model::NgsiError::NonexistentTenant(_))),
        "a hosted @context re-created the renamed-away name: {ctx:?}"
    );
    assert_eq!(rows_of(&pool, &old).await, 0);

    sqlx::query("SELECT antares_tenant_rename_rollback($1)")
        .bind(&new)
        .execute(&pool)
        .await
        .expect("the rollback finds the old name empty");
    db.drop().await;
}

async fn rename(pool: &PgPool, old: &str, new: &str) {
    sqlx::query("SELECT antares_tenant_rename($1, $2)")
        .bind(old)
        .bind(new)
        .execute(pool)
        .await
        .expect("renamed");
}

/// A subscription's stored document, read past the store.
async fn stored_sub(pool: &PgPool, tenant: &str, id: &str) -> serde_json::Value {
    sqlx::query_scalar("SELECT subscription FROM subscriptions WHERE tenant_id = $1 AND id = $2")
        .bind(tenant)
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("stored subscription")
}

/// T-7274, S5-01: with ANTARES_SECRET_KEY set, a renamed Tenant's
/// subscription still opens and records its deliveries, its sealed
/// `NGSILD-Tenant` header names the new Tenant, the boot pass reseals it under
/// the new name and forgets the old one, and a value written after the rename
/// opens again after the rollback. No step stores the credential in plaintext.
#[tokio::test(flavor = "multi_thread")]
async fn a_sealed_subscription_opens_and_notifies_after_the_rename() {
    use antares_sql::store::pg::doc::{DocKind, PgDocStore};
    use antares_sql::store::pg::seal::SecretKey;
    let db = own_db!("sealed");
    let pool = db.pool.clone();
    let n = run_id();
    let old = format!("seal{n}-air");
    let new = keyed(n, "air");
    let docs = PgDocStore::new(pool.clone()).with_secret_key(
        SecretKey::from_base64("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").expect("key"),
    );
    let token = "Bearer the-webhook-token";
    let id = "urn:ngsi-ld:Subscription:sealed";
    let t_old = TenantId::new(&old).expect("tenant");
    pg::ensure_tenant(&pool, &t_old).await.expect("tenant row");
    let sub = serde_json::json!({"id": id, "type": "Subscription",
        "notification": {"endpoint": {"uri": "http://localhost:9", "receiverInfo": [
            {"key": "Authorization", "value": token},
            {"key": "NGSILD-Tenant", "value": old}]}}});
    assert!(docs
        .create(&t_old, DocKind::Subscription, id, &sub)
        .await
        .expect("create"));
    let at_rest = |doc: serde_json::Value| {
        let text = doc.to_string();
        assert!(
            !text.contains("the-webhook-token"),
            "plaintext at rest: {text}"
        );
        text
    };
    let before = at_rest(stored_sub(&pool, &old, id).await);

    rename(&pool, &old, &new).await;
    let t_new = TenantId::new(&new).expect("tenant");
    assert_eq!(at_rest(stored_sub(&pool, &new, id).await), before);
    let got = docs
        .get(&t_new, DocKind::Subscription, id)
        .await
        .expect("opens under the new name")
        .expect("there");
    let pairs = &got["notification"]["endpoint"]["receiverInfo"];
    assert_eq!(pairs[0]["value"], token);
    assert_eq!(pairs[1]["value"], new.as_str());
    let sent = docs
        .record_deliveries(
            &t_new,
            DocKind::Subscription,
            &[(id, "2026-11-18T10:00:00Z", 1)],
        )
        .await
        .expect("a delivery is recorded");
    assert_eq!(sent[id].0["notification"]["timesSent"], 1);
    assert_eq!(
        sent[id].0["notification"]["endpoint"]["receiverInfo"][0]["value"],
        token
    );

    // the boot pass reseals under the new name and forgets the old one
    assert!(docs.seal_plaintext().await.expect("boot pass") >= 1);
    let resealed = at_rest(stored_sub(&pool, &new, id).await);
    assert_ne!(resealed, before);
    let aliases: i64 = sqlx::query_scalar("SELECT count(*) FROM tenant_seal_alias")
        .fetch_one(&pool)
        .await
        .expect("aliases");
    assert_eq!(aliases, 0);
    let got = docs
        .get(&t_new, DocKind::Subscription, id)
        .await
        .expect("opens without a former name")
        .expect("there");
    assert_eq!(
        got["notification"]["endpoint"]["receiverInfo"][0]["value"],
        token
    );
    assert_eq!(docs.seal_plaintext().await.expect("again"), 0);

    // sealed under the new name, opened under the old after the rollback
    sqlx::query("SELECT antares_tenant_rename_rollback($1)")
        .bind(&new)
        .execute(&pool)
        .await
        .expect("rolled back");
    let got = docs
        .get(&t_old, DocKind::Subscription, id)
        .await
        .expect("opens after the rollback")
        .expect("there");
    assert_eq!(
        got["notification"]["endpoint"]["receiverInfo"][0]["value"],
        token
    );
    db.drop().await;
}

/// T-7274, S5-19: the rename moves only the members that name a Tenant by
/// contract (a registration's `tenant`, an `NGSILD-Tenant` pair); the old
/// name as an attribute, a key or plain text stays, in the renamed Tenant's
/// rows and in every other Tenant's.
#[tokio::test(flavor = "multi_thread")]
async fn the_rename_rewrites_only_the_members_that_name_a_tenant() {
    use antares_sql::store::pg::doc::{DocKind, PgDocStore};
    let db = own_db!("contract");
    let pool = db.pool.clone();
    let n = run_id();
    let old = format!("word{n}");
    let new = keyed(n, "word");
    let docs = PgDocStore::new(pool.clone());
    let sub = |tenant_header: &str| {
        serde_json::json!({"id": "urn:ngsi-ld:Subscription:s", "type": "Subscription",
            "description": old, "watchedAttributes": [old], "q": format!("{old}==1"),
            "entities": [{"type": old}], "jsonldContext": {old.as_str(): "https://x/n"},
            "notification": {"attributes": [old], "endpoint": {"uri": "http://localhost:9",
                "receiverInfo": [{"key": "X-Note", "value": old},
                                 {"key": "ngsild-tenant", "value": tenant_header}]}}})
    };
    let mut want = Vec::new();
    for name in [old.clone(), format!("other{n}")] {
        let t = TenantId::new(&name).expect("tenant");
        pg::ensure_tenant(&pool, &t).await.expect("tenant row");
        assert!(docs
            .create(
                &t,
                DocKind::Subscription,
                "urn:ngsi-ld:Subscription:s",
                &sub(&old)
            )
            .await
            .expect("create"));
        let reg = serde_json::json!({"id": "urn:ngsi-ld:R:r", "type": "ContextSourceRegistration",
            "information": [{"entities": [{"type": old}]}], "endpoint": "http://localhost:9",
            "tenant": old, "description": old,
            "contextSourceInfo": [{"key": "NGSILD-Tenant", "value": old}, {"key": "X-Note", "value": old}]});
        assert!(docs
            .create(&t, DocKind::Registration, "urn:ngsi-ld:R:r", &reg)
            .await
            .expect("create registration"));
        let mut s = sub(&new);
        let mut r = reg.clone();
        r["tenant"] = new.clone().into();
        r["contextSourceInfo"][0]["value"] = new.clone().into();
        s["notification"]["endpoint"]["receiverInfo"][1]["value"] = new.clone().into();
        let after = if name == old { new.clone() } else { name };
        want.push((after, s, r));
    }

    rename(&pool, &old, &new).await;
    for (tenant, s, r) in want {
        let mut got = stored_sub(&pool, &tenant, "urn:ngsi-ld:Subscription:s").await;
        for member in ["timesSent", "status"] {
            got["notification"]
                .as_object_mut()
                .expect("notification")
                .remove(member);
        }
        let mut s = s;
        for member in ["timesSent", "status"] {
            s["notification"]
                .as_object_mut()
                .expect("notification")
                .remove(member);
        }
        assert_eq!(got, s, "{tenant}");
        let reg: serde_json::Value = sqlx::query_scalar(
            "SELECT registration FROM csource_registrations WHERE tenant_id = $1 AND id = 'urn:ngsi-ld:R:r'",
        )
        .bind(&tenant)
        .fetch_one(&pool)
        .await
        .expect("registration");
        assert_eq!(reg, r, "{tenant}");
        // the index keeps no copy of the forward's headers (T-7296)
        let headers: Option<serde_json::Value> = sqlx::query_scalar(
            "SELECT headers FROM csource_index WHERE tenant_id = $1 AND registration_id = 'urn:ngsi-ld:R:r' LIMIT 1",
        )
        .bind(&tenant)
        .fetch_one(&pool)
        .await
        .expect("index row");
        assert_eq!(headers, None, "{tenant}");
    }
    db.drop().await;
}

/// The rename refuses a target holding rows, a tombstoned or reserved name,
/// the default Tenant, and a rollback nobody recorded; nothing moves.
#[tokio::test(flavor = "multi_thread")]
async fn the_rename_refuses_a_taken_target_and_names_it_must_not_move() {
    let db = own_db!("refuse");
    let pool = db.pool.clone();
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
    assert!(refused(a.clone(), "antares-views".into())
        .await
        .contains("keep their names"));
    assert!(refused("default".into(), keyed(n, "d"))
        .await
        .contains("keep their names"));
    assert!(refused(a.clone(), "a b".into())
        .await
        .contains("letters, digits"));
    assert!(refused(format!("none{n}"), keyed(n, "x"))
        .await
        .contains("no Tenant"));
    let new = keyed(n, "b");
    sqlx::query("SELECT antares_tenant_rename($1, $2)")
        .bind(&b)
        .bind(&new)
        .execute(&pool)
        .await
        .expect("renamed");
    assert!(refused(a.clone(), b.clone())
        .await
        .contains("never used again"));
    let stray = sqlx::query("SELECT antares_tenant_rename_rollback($1)")
        .bind(keyed(n, "never"))
        .execute(&pool)
        .await
        .expect_err("refused")
        .to_string();
    assert!(stray.contains("no rename"), "{stray}");
    assert_eq!(digest(&pool, &a).await, before);
    db.drop().await;
}

/// A database owner that bypasses row-level security without being superuser
/// runs the rename with `entities`' triggers on (a superuser skips them): the
/// Tenant moves the same, and the transaction keeps its triggers afterwards.
#[tokio::test(flavor = "multi_thread")]
async fn a_bypassrls_owner_moves_the_tenant_the_same_with_the_triggers_on() {
    let db = own_db!("owner");
    let pool = db.pool.clone();
    let n = run_id();
    let old = format!("renu{n}-air");
    seed(&pool, &TenantId::new(&old).expect("tenant")).await;
    let before = digest(&pool, &old).await;
    let new = keyed(n, "air");
    let role = format!("renameowner{n}");
    // The grants commit before the rename's transaction: granted there, they
    // would hold every table's pg_class row through the rename and deadlock a
    // parallel ANALYZE that waits on them while the rename waits on its table.
    let own = |stmts: Vec<String>| {
        let pool = pool.clone();
        async move {
            let mut setup = pool.begin().await.expect("tx");
            for stmt in stmts {
                // the role name is this test's own
                sqlx::query(sqlx::AssertSqlSafe(stmt.clone()))
                    .execute(&mut *setup)
                    .await
                    .expect(&stmt);
            }
            setup.commit().await.expect("commit");
        }
    };
    own(vec![
        format!("CREATE ROLE {role} NOSUPERUSER BYPASSRLS"),
        format!("GRANT ALL ON ALL TABLES IN SCHEMA public TO {role}"),
        format!("GRANT ALL ON ALL SEQUENCES IN SCHEMA public TO {role}"),
        format!("GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA public TO {role}"),
    ])
    .await;
    let mut tx = pool.begin().await.expect("tx");
    sqlx::query(sqlx::AssertSqlSafe(format!("SET LOCAL ROLE {role}")))
        .execute(&mut *tx)
        .await
        .expect("set role");
    sqlx::query("SELECT antares_tenant_rename($1, $2)")
        .bind(&old)
        .bind(&new)
        .execute(&mut *tx)
        .await
        .expect("renamed by the owner");
    let after: Vec<(String, i64, String)> = sqlx::query_as(
        "SELECT tbl, row_count, digest::text FROM antares_tenant_digest($1) ORDER BY tbl",
    )
    .bind(&new)
    .fetch_all(&mut *tx)
    .await
    .expect("digest");
    let replication: String =
        sqlx::query_scalar("SELECT current_setting('session_replication_role')")
            .fetch_one(&mut *tx)
            .await
            .expect("setting");
    tx.rollback().await.expect("rollback");
    own(vec![
        format!("DROP OWNED BY {role}"),
        format!("DROP ROLE {role}"),
    ])
    .await;
    assert_eq!(after, before);
    assert_eq!(replication, "origin");
    db.drop().await;
}

/// The functions run only as a role that bypasses row-level security: the
/// broker's own role can never move a Tenant or read every row of one.
#[tokio::test(flavor = "multi_thread")]
async fn the_rename_refuses_a_role_under_row_level_security() {
    let db = own_db!("rls");
    let pool = db.pool.clone();
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
    db.drop().await;
}
