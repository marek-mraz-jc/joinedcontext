//! API/01 §46, DM-126, DM-127, T-4668: the erasure log on PostgreSQL. One store that does not
//! answer keeps the erasure open and is asked again; the Portal's own copies go at once; the
//! erasure is done only once every store is. Runs when `JC_PORTAL_TEST_DATABASE_URL` points at a
//! PostgreSQL the test may write to; otherwise it says so and returns.

use joinedcontext_portal::config::Config;
use joinedcontext_portal::erasure::{self, Group, Store, StoreState};
use joinedcontext_portal::state::AppState;
use serde_json::json;

#[tokio::test]
async fn a_store_that_does_not_answer_keeps_the_erasure_open_until_it_does() {
    let Some(url) = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
    else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("the test database answers and migrates");
    let project = format!(
        "erase-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    // No broker and no assistant: the broker fails, the conversations have nothing to hold.
    let state = AppState::new(Config::for_tests(), None).with_db(pool.clone());
    let trip = "urn:ngsi-ld:BikeTrip:hel.fi:t-42";
    let other = "urn:ngsi-ld:BikeTrip:hel.fi:t-43";
    for urn in [trip, other] {
        state
            .trash
            .keep(
                (&project, "trips", "f:1:anna"),
                urn,
                "BikeTrip",
                &json!({"id": urn, "type": "BikeTrip"}),
            )
            .await
            .expect("kept");
    }
    sqlx::query(
        "INSERT INTO entity_comments (project, space, urn, author, author_name, body) \
         VALUES ($1, 'trips', $2, 'f:1:anna', 'Anna', 'Ján rode it')",
    )
    .bind(&project)
    .bind(trip)
    .execute(&pool)
    .await
    .expect("a comment");

    let groups = vec![Group {
        class: "BikeTrip".into(),
        entities: vec![trip.into()],
        attrs: Some(vec!["rider".into()]),
    }];
    let begun = erasure::begin(
        (&project, "trips"),
        "ab".repeat(16),
        "Art. 17",
        "user:anna",
        groups.clone(),
    );
    erasure::insert(&pool, &begun).await.expect("logged");
    // A proposal reaches no store, not even when asked to (DM-142).
    erasure::attempt(&state, &pool, &begun, Some("Ján Nováček")).await;
    let waiting = erasure::get(&pool, &project, "trips", &begun.id)
        .await
        .expect("read")
        .expect("held");
    assert_eq!(waiting.state(), "proposed");
    assert_eq!(waiting.stores["portal"].attempts, 0);
    assert!(!erasure::open(&pool)
        .await
        .expect("open")
        .iter()
        .any(|e| e.id == begun.id));
    let begun = erasure::approve(&pool, (&project, "trips", &begun.id), "user:boris", &groups)
        .await
        .expect("approved")
        .expect("a proposal of someone else");
    erasure::attempt(&state, &pool, &begun, Some("Ján Nováček")).await;

    let now = erasure::get(&pool, &project, "trips", &begun.id)
        .await
        .expect("read")
        .expect("held");
    assert_eq!(now.state(), "open");
    let broker = &now.stores["broker"];
    assert_eq!((broker.state, broker.attempts), (StoreState::Pending, 1));
    assert!(
        broker
            .error
            .as_deref()
            .is_some_and(|e| e.contains("broker")),
        "{broker:?}"
    );
    assert_eq!(
        now.stores["portal"],
        Store {
            state: StoreState::Done,
            count: 2,
            attempts: 1,
            ..Store::default()
        }
    );
    assert_eq!(now.stores["conversations"].state, StoreState::Done);
    assert!(
        !now.stores.contains_key("ckanDatastore"),
        "CKAN holds no rows (T-5060)"
    );
    // Only the erased Entity's copies went.
    assert_eq!(
        state
            .trash
            .list((&project, "trips", "f:1:anna"))
            .await
            .expect("listed")
            .len(),
        1
    );
    assert!(erasure::open(&pool)
        .await
        .expect("open")
        .iter()
        .any(|e| e.id == begun.id));
    // Another space reads none of it; the log holds the hash, never the subject's words.
    assert!(erasure::get(&pool, &project, "bikes", &begun.id)
        .await
        .expect("read")
        .is_none());
    let row: serde_json::Value =
        sqlx::query_scalar("SELECT to_jsonb(e) FROM erasures e WHERE id = $1")
            .bind(&begun.id)
            .fetch_one(&pool)
            .await
            .expect("row");
    assert!(!row.to_string().contains("Nováček"));

    // A retry asks the broker again, the done stores never.
    erasure::attempt(&state, &pool, &now, None).await;
    let again = erasure::get(&pool, &project, "trips", &begun.id)
        .await
        .expect("read")
        .expect("held");
    assert_eq!(again.stores["broker"].attempts, 2);
    assert_eq!(again.stores["portal"].attempts, 1);

    for name in ["broker"] {
        erasure::save(
            &pool,
            &begun.id,
            name,
            &again.stores[name].after(Ok((1, None))),
        )
        .await
        .expect("saved");
    }
    let done = erasure::get(&pool, &project, "trips", &begun.id)
        .await
        .expect("read")
        .expect("held");
    assert_eq!(done.state(), "done");
    assert!(done.completed_at.is_some());
    assert!(!erasure::open(&pool)
        .await
        .expect("open")
        .iter()
        .any(|e| e.id == begun.id));
}

/// DM-142, T-5356: a proposal is approved once, by someone other than its proposer, before it
/// expires; a declined or expired one is approved by nobody.
#[tokio::test]
async fn a_proposal_is_approved_once_by_a_second_person_before_it_expires() {
    let Some(url) = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
    else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("the test database answers and migrates");
    let project = format!(
        "four-eyes-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    let groups = vec![Group {
        class: "BikeTrip".into(),
        entities: vec!["urn:ngsi-ld:BikeTrip:t-1".into()],
        attrs: None,
    }];
    let propose = || {
        erasure::begin(
            (&project, "trips"),
            "cd".repeat(16),
            "Art. 17",
            "user:anna",
            vec![],
        )
    };
    fn key<'a>(project: &'a str, e: &'a erasure::Erasure) -> (&'a str, &'a str, &'a str) {
        (project, "trips", e.id.as_str())
    }

    let first = propose();
    erasure::insert(&pool, &first).await.expect("logged");
    let own = erasure::approve(&pool, key(&project, &first), "user:anna", &groups)
        .await
        .expect("asked");
    assert!(own.is_none(), "the proposer never approves");
    let approved = erasure::approve(&pool, key(&project, &first), "user:boris", &groups)
        .await
        .expect("asked")
        .expect("approved");
    assert_eq!(approved.state(), "open");
    assert_eq!(approved.decided_by.as_deref(), Some("user:boris"));
    assert_eq!(approved.groups, groups, "the Entities found at approval");
    assert!(
        erasure::approve(&pool, key(&project, &first), "user:carla", &groups)
            .await
            .expect("asked")
            .is_none()
    );
    assert!(
        erasure::decline(&pool, key(&project, &first), "user:carla", "late")
            .await
            .expect("asked")
            .is_none()
    );

    let second = propose();
    erasure::insert(&pool, &second).await.expect("logged");
    let declined = erasure::decline(
        &pool,
        key(&project, &second),
        "user:anna",
        "the person withdrew",
    )
    .await
    .expect("asked")
    .expect("declined");
    assert_eq!(declined.state(), "declined");
    assert!(
        erasure::approve(&pool, key(&project, &second), "user:boris", &groups)
            .await
            .expect("asked")
            .is_none()
    );

    let mut third = propose();
    third.requested_at -= time::Duration::days(erasure::PROPOSAL_DAYS);
    erasure::insert(&pool, &third).await.expect("logged");
    assert_eq!(third.state(), "expired");
    assert!(
        erasure::approve(&pool, key(&project, &third), "user:boris", &groups)
            .await
            .expect("asked")
            .is_none()
    );

    let listed = erasure::list(&pool, &project, "trips")
        .await
        .expect("listed");
    assert_eq!(listed.len(), 3);
    assert!(!erasure::open(&pool)
        .await
        .expect("open")
        .iter()
        .any(|e| e.id == second.id || e.id == third.id));
}
