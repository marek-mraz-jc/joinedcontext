//! The Postgres arm of the connector store (AG-142..AG-147): create, edit, read, approve, revoke,
//! the organization overview, the audit and its trim, each in the reserved key of the person's
//! Organization, and what each answers where the request is in none. Runs when
//! `JC_PORTAL_TEST_DATABASE_URL` points at a PostgreSQL the test may write to; otherwise it says
//! so and returns.

mod common;

use std::sync::Arc;

use jc_time::TimestampTz;
use joinedcontext_portal::connectors::{
    Above, Call, ConnectorStore, NewConnector, Trail, DENIALS_FLAG, MAX_PER_PERSON,
};
use joinedcontext_portal::error::ApiError;
use joinedcontext_portal::store::Mirror;
use serde_json::json;

const BASE: &str = "https://portal.test";

struct World {
    pool: sqlx::PgPool,
    /// Serves the Organization `org`, the installation the deployment configured.
    store: ConnectorStore,
    /// Serves a request that is in no Organization: nothing is read or kept.
    unplaced: ConnectorStore,
    org: String,
    owner: String,
}

async fn world() -> Option<World> {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|u| !u.trim().is_empty());
    let Some(url) = url else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return None;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("the test database answers and migrates");
    let org = format!("cov-{}", common::unique::unique());
    sqlx::query(
        "INSERT INTO organizations (name, host, state, created_by) \
         VALUES ($1, $1 || '.example', 'active', 'test')",
    )
    .bind(&org)
    .execute(&pool)
    .await
    .expect("an Organization of the registry");
    let mirror = Arc::new(Mirror::default());
    Some(World {
        store: ConnectorStore::new(Some((pool.clone(), Arc::clone(&mirror))), Some(org.clone())),
        unplaced: ConnectorStore::new(Some((pool.clone(), mirror)), None),
        pool,
        org,
        owner: format!("f:1:{}", common::unique::unique()),
    })
}

fn ask(name: &str) -> NewConnector {
    serde_json::from_value(json!({
        "name": name, "segments": ["data-read"], "scope": [],
        "expiresAt": "2030-01-01T00:00:00Z"
    }))
    .expect("a connector")
}

/// What a request asks above the ceiling: kept pending until an administrator approves it.
fn above() -> Above {
    let mut above = Above::default();
    above
        .lines
        .push("writes: asked above the policy".to_owned());
    above
}

fn call(verdict: &str) -> Call {
    Call {
        at: TimestampTz::now(),
        subject: "sub".into(),
        client: Some("mcp-local".into()),
        tool: "tools/list".into(),
        target: None,
        verdict: verdict.into(),
        duration_ms: 3,
    }
}

#[tokio::test]
async fn a_name_is_held_once_and_twenty_connectors_at_most() {
    let Some(w) = world().await else {
        return;
    };
    w.store
        .create(&w.owner, ask("doprava"), Above::default(), BASE)
        .await
        .expect("created");
    let again = w
        .store
        .create(&w.owner, ask("doprava"), Above::default(), BASE)
        .await;
    assert!(
        matches!(&again, Err(ApiError::Conflict(m)) if m.contains("named doprava")),
        "{again:?}"
    );
    for n in 1..MAX_PER_PERSON {
        w.store
            .create(&w.owner, ask(&format!("c{n}")), Above::default(), BASE)
            .await
            .expect("created");
    }
    let over = w
        .store
        .create(&w.owner, ask("one-more"), Above::default(), BASE)
        .await;
    assert!(
        matches!(&over, Err(ApiError::Conflict(m)) if m.contains("20 live connectors")),
        "{over:?}"
    );
    let listed = w.store.of_owner(&w.owner, BASE).await.expect("list");
    assert_eq!(listed.len(), MAX_PER_PERSON);
}

#[tokio::test]
async fn a_request_in_no_organization_keeps_and_reads_nothing() {
    let Some(w) = world().await else {
        return;
    };
    let made = w
        .store
        .create(&w.owner, ask("doprava"), Above::default(), BASE)
        .await
        .expect("created");
    let refused = w
        .unplaced
        .create(&w.owner, ask("doprava"), Above::default(), BASE)
        .await;
    assert!(
        matches!(&refused, Err(ApiError::Conflict(m)) if m.contains("no active Organization")),
        "{refused:?}"
    );
    assert!(w.unplaced.get(&made.id, BASE).await.expect("get").is_none());
    assert!(w
        .unplaced
        .of_owner(&w.owner, BASE)
        .await
        .expect("list")
        .is_empty());
    assert!(w.unplaced.pending(BASE).await.expect("pending").is_empty());
    assert!(w
        .unplaced
        .overview(BASE)
        .await
        .expect("overview")
        .is_empty());
    assert!(w.unplaced.calls(&made.id).await.expect("calls").is_empty());
    assert_eq!(
        w.unplaced.revoke_all(&w.owner).await.expect("revoke all"),
        0
    );
    assert!(!w.unplaced.revoke(&made.id, &w.owner).await.expect("revoke"));
    assert!(!w
        .unplaced
        .update(&made.id, &w.owner, ask("doprava"), Above::default(), None)
        .await
        .expect("update"));
    assert!(!w
        .unplaced
        .approve(&made.id, "sub-admin", TimestampTz::now())
        .await
        .expect("approve"));
    let unrecorded = w
        .unplaced
        .record(&made.id, call("allowed"))
        .await
        .expect_err("no call runs unrecorded");
    assert!(
        matches!(&unrecorded, ApiError::Conflict(m) if m.contains("audit of MCP calls")),
        "{unrecorded:?}"
    );
    // The Organization's own store still holds it, untouched.
    let held = w
        .store
        .get(&made.id, BASE)
        .await
        .expect("get")
        .expect("held");
    assert_eq!(held.state, "active");
}

#[tokio::test]
async fn a_connector_is_read_by_its_id_and_an_unknown_id_is_none() {
    let Some(w) = world().await else {
        return;
    };
    let made = w
        .store
        .create(&w.owner, ask("doprava"), Above::default(), BASE)
        .await
        .expect("created");
    let read = w
        .store
        .get(&made.id, BASE)
        .await
        .expect("get")
        .expect("there");
    assert_eq!(
        (read.id.as_str(), read.name.as_str()),
        (made.id.as_str(), "doprava")
    );
    assert_eq!(read.url, format!("{BASE}/c/{}", made.id));
    assert!(w
        .store
        .get("no-such-connector", BASE)
        .await
        .expect("get")
        .is_none());
}

#[tokio::test]
async fn an_edit_replaces_the_grant_of_a_live_connector_of_its_owner_alone() {
    let Some(w) = world().await else {
        return;
    };
    let made = w
        .store
        .create(&w.owner, ask("doprava"), Above::default(), BASE)
        .await
        .expect("created");
    let mut edited = ask("ignored-rename");
    edited.segments = vec!["context".into()];
    assert!(w
        .store
        .update(&made.id, &w.owner, edited, Above::default(), None)
        .await
        .expect("update"));
    let read = w
        .store
        .get(&made.id, BASE)
        .await
        .expect("get")
        .expect("there");
    assert_eq!(read.segments, ["context"]);
    assert_eq!(read.name, "doprava", "the name stays");
    assert!(
        read.granted_at > made.granted_at,
        "an edit judged against the ceiling renews its age"
    );
    assert!(!w
        .store
        .update(&made.id, "someone-else", ask("x"), Above::default(), None)
        .await
        .expect("update"));
    assert!(!w
        .store
        .update(
            "no-such-connector",
            &w.owner,
            ask("x"),
            Above::default(),
            None
        )
        .await
        .expect("update"));
    assert!(w.store.revoke(&made.id, &w.owner).await.expect("revoke"));
    assert!(
        !w.store
            .update(&made.id, &w.owner, ask("x"), Above::default(), None)
            .await
            .expect("update"),
        "a revoked one is not edited"
    );
}

#[tokio::test]
async fn a_pending_connector_is_listed_for_an_administrator_and_approved_once() {
    let Some(w) = world().await else {
        return;
    };
    let made = w
        .store
        .create(&w.owner, ask("doprava"), above(), BASE)
        .await
        .expect("created");
    assert_eq!(made.state, "pending");
    let waiting = w.store.pending(BASE).await.expect("pending");
    assert!(waiting.iter().any(|c| c.id == made.id));
    assert!(w
        .store
        .approve(&made.id, "sub-admin", made.granted_at)
        .await
        .expect("approve"));
    assert!(
        !w.store
            .approve(&made.id, "sub-admin-2", made.granted_at)
            .await
            .expect("approve"),
        "a second approval records nothing"
    );
    assert!(!w
        .store
        .approve("no-such-connector", "sub-admin", made.granted_at)
        .await
        .expect("approve"));
    let approved = w
        .store
        .get(&made.id, BASE)
        .await
        .expect("get")
        .expect("there");
    assert_eq!(approved.state, "active");
    assert_eq!(
        approved.approval.as_ref().map(|a| a.by.as_str()),
        Some("sub-admin")
    );
    let waiting = w.store.pending(BASE).await.expect("pending");
    assert!(!waiting.iter().any(|c| c.id == made.id));
}

#[tokio::test]
async fn an_approval_judged_on_a_grant_that_was_edited_since_waits_again() {
    let Some(w) = world().await else {
        return;
    };
    let made = w
        .store
        .create(&w.owner, ask("doprava"), above(), BASE)
        .await
        .expect("created");
    let judged = made.granted_at;
    assert!(w
        .store
        .update(&made.id, &w.owner, ask("doprava"), above(), None)
        .await
        .expect("edited"));
    assert!(
        !w.store
            .approve(&made.id, "sub-admin", judged)
            .await
            .expect("approve"),
        "the approver judged the grant as it was before the edit"
    );
    let moved = w
        .store
        .get(&made.id, BASE)
        .await
        .expect("get")
        .expect("there");
    assert_eq!(moved.state, "pending");
    assert!(w
        .store
        .approve(&made.id, "sub-admin", moved.granted_at)
        .await
        .expect("approve"));
}

#[tokio::test]
async fn a_revoke_ends_a_live_connector_of_its_owner_once() {
    let Some(w) = world().await else {
        return;
    };
    let made = w
        .store
        .create(&w.owner, ask("doprava"), Above::default(), BASE)
        .await
        .expect("created");
    assert!(!w
        .store
        .revoke(&made.id, "someone-else")
        .await
        .expect("revoke"));
    assert!(!w
        .store
        .revoke("no-such-connector", &w.owner)
        .await
        .expect("revoke"));
    assert!(w.store.revoke(&made.id, &w.owner).await.expect("revoke"));
    assert!(!w.store.revoke(&made.id, &w.owner).await.expect("revoke"));
    let read = w
        .store
        .get(&made.id, BASE)
        .await
        .expect("get")
        .expect("there");
    assert_eq!(read.state, "revoked");
}

#[tokio::test]
async fn offboarding_revokes_every_live_connector_of_one_person() {
    let Some(w) = world().await else {
        return;
    };
    let other = format!("f:1:{}", common::unique::unique());
    for name in ["a", "b", "c"] {
        w.store
            .create(&w.owner, ask(name), Above::default(), BASE)
            .await
            .expect("created");
    }
    let kept = w
        .store
        .create(&other, ask("a"), Above::default(), BASE)
        .await
        .expect("another person's");
    assert_eq!(w.store.revoke_all(&w.owner).await.expect("revoke all"), 3);
    assert_eq!(w.store.revoke_all(&w.owner).await.expect("revoke all"), 0);
    assert_eq!(w.store.revoke_all("nobody").await.expect("revoke all"), 0);
    let still = w
        .store
        .get(&kept.id, BASE)
        .await
        .expect("get")
        .expect("there");
    assert_eq!(still.state, "active");
}

#[tokio::test]
async fn the_overview_counts_a_connectors_last_day_and_flags_a_probing_client() {
    let Some(w) = world().await else {
        return;
    };
    let busy = w
        .store
        .create(&w.owner, ask("busy"), Above::default(), BASE)
        .await
        .expect("created");
    let quiet = w
        .store
        .create(&w.owner, ask("quiet"), Above::default(), BASE)
        .await
        .expect("created");
    w.store
        .record(&busy.id, call("allowed"))
        .await
        .expect("kept");
    w.store
        .record(&busy.id, call("started"))
        .await
        .expect("kept");
    for _ in 0..DENIALS_FLAG {
        w.store
            .record(&busy.id, call("refused: outside the scope"))
            .await
            .expect("kept");
    }
    w.store
        .record(&quiet.id, call("refused: revoked"))
        .await
        .expect("kept");
    let overview = w.store.overview(BASE).await.expect("overview");
    let of = |id: &str| {
        overview
            .iter()
            .find(|o| o.connector.id == id)
            .expect("listed")
    };
    let busy = of(&busy.id);
    assert_eq!(
        (busy.calls_per_day, busy.denials_per_day, busy.flagged),
        (DENIALS_FLAG + 1, DENIALS_FLAG, true)
    );
    let quiet = of(&quiet.id);
    assert_eq!(
        (quiet.calls_per_day, quiet.denials_per_day, quiet.flagged),
        (1, 1, false)
    );
}

#[tokio::test]
async fn the_audit_of_the_organization_is_kept_in_its_key_and_marks_the_connector_used() {
    let Some(w) = world().await else {
        return;
    };
    let made = w
        .store
        .create(&w.owner, ask("doprava"), Above::default(), BASE)
        .await
        .expect("created");
    assert!(made.last_used_at.is_none());
    w.store
        .keep(
            &Trail::new(Some(made.id.clone()), Some(w.org.clone())),
            call("allowed"),
        )
        .await
        .expect("kept under the named Organization");
    let used = w
        .store
        .get(&made.id, BASE)
        .await
        .expect("get")
        .expect("there");
    assert!(used.last_used_at.is_some());
    let calls = w.store.calls(&made.id).await.expect("calls");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].verdict, "allowed");
}

#[tokio::test]
async fn a_call_of_an_organization_the_registry_lacks_is_not_kept() {
    let Some(w) = world().await else {
        return;
    };
    let ghost = format!("ghost-{}", common::unique::unique());
    let err = w
        .store
        .keep(&Trail::new(None, Some(ghost)), call("allowed"))
        .await
        .expect_err("no call runs unrecorded");
    assert!(
        matches!(&err, ApiError::Conflict(m) if m.contains("audit of MCP calls")),
        "{err:?}"
    );
}

#[tokio::test]
async fn a_started_row_hides_behind_its_outcome_and_shows_alone() {
    let Some(w) = world().await else {
        return;
    };
    let made = w
        .store
        .create(&w.owner, ask("doprava"), Above::default(), BASE)
        .await
        .expect("created");
    let ended = Trail::new(Some(made.id.clone()), None);
    w.store.keep(&ended, call("started")).await.expect("kept");
    w.store.keep(&ended, call("allowed")).await.expect("kept");
    let lost = Trail::new(Some(made.id.clone()), None);
    w.store.keep(&lost, call("started")).await.expect("kept");
    let verdicts: Vec<String> = w
        .store
        .calls(&made.id)
        .await
        .expect("calls")
        .into_iter()
        .map(|c| c.verdict)
        .collect();
    assert_eq!(verdicts.len(), 2, "{verdicts:?}");
    assert!(verdicts.contains(&"allowed".to_owned()));
    assert!(verdicts.contains(&"started".to_owned()));
}

#[tokio::test]
async fn the_trim_deletes_old_audit_rows_in_batches_and_keeps_the_recent_ones() {
    let Some(w) = world().await else {
        return;
    };
    let made = w
        .store
        .create(&w.owner, ask("doprava"), Above::default(), BASE)
        .await
        .expect("created");
    w.store
        .record(&made.id, call("allowed"))
        .await
        .expect("kept");
    let key = joinedcontext_portal::project_ids::reserved(&w.pool, &w.org)
        .await
        .expect("reserved")
        .expect("the Organization's key");
    // One batch is 10 000 rows: one more takes the loop round again.
    let mut tx = joinedcontext_portal::db::tenant_tx(&w.pool, key)
        .await
        .expect("a transaction");
    sqlx::query(
        "INSERT INTO connector_calls (connector, call_id, at, subject, client, tool, target, \
         verdict, duration_ms) \
         SELECT NULL::text, NULL::text, '2001-01-01T00:00:00Z'::timestamptz + (n || ' seconds')::interval, \
                'cov', NULL::text, 'tools/list', NULL::text, 'allowed', 1 \
         FROM generate_series(1, 10001) n",
    )
    .execute(&mut *tx)
    .await
    .expect("old rows");
    tx.commit().await.expect("committed");
    let deleted = w
        .store
        .trim_audit(TimestampTz::now(), 3650)
        .await
        .expect("trim");
    assert!(deleted >= 10_001, "{deleted}");
    assert_eq!(
        w.store.calls(&made.id).await.expect("calls").len(),
        1,
        "a recent row stays"
    );
    let due = w
        .store
        .trim_audit_if_due(TimestampTz::now(), 3650)
        .await
        .expect("never run by this process yet");
    assert!(due.is_ok());
    assert!(w
        .store
        .trim_audit_if_due(TimestampTz::now(), 3650)
        .await
        .is_none());
}
