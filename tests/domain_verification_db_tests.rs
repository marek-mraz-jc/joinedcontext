//! An Organization's domain verification as the reconciler stores it (PF-41, T-2377).
//!
//! Runs when `JC_PORTAL_TEST_DATABASE_URL` points at a PostgreSQL the test may write to (locally:
//! `docker run -e POSTGRES_PASSWORD=… postgres:17-alpine`); otherwise it says so and returns, like
//! the other database suites, so the fast lane without a service container stays green.

use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{DateTime, Duration, Utc};
use joinedcontext_portal::domain_verification::{
    gate_states, Lookup, Method, State, Unanswered, Verifier,
};
use serde_json::Value;

fn database_url() -> Option<String> {
    std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
}

/// An Organization nobody else's run writes to, so tests share one database without a fixture.
fn fresh_organization() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("org-{nanos}")
}

/// A resolver that publishes whatever TXT strings the case gives it, and counts the looks.
struct Published {
    txt: Vec<String>,
    looks: AtomicUsize,
}

impl Published {
    fn new(txt: &[String]) -> Self {
        Self {
            txt: txt.to_vec(),
            looks: AtomicUsize::new(0),
        }
    }
}

impl Lookup for &Published {
    async fn txt(&self, _name: &str) -> Result<Vec<String>, Unanswered> {
        self.looks.fetch_add(1, Ordering::SeqCst);
        Ok(self.txt.clone())
    }
    async fn did_document(&self, _domain: &str) -> Result<Value, Unanswered> {
        Ok(Value::Null)
    }
}

async fn pool(url: &str) -> sqlx::PgPool {
    joinedcontext_portal::db::connect(url)
        .await
        .expect("the test database answers and migrates")
}

fn now() -> DateTime<Utc> {
    Utc::now()
}

/// PF-41: the first look mints the Organization's challenge and fails until it is published;
/// the challenge outlives the process, and once published the domain verifies with it.
#[tokio::test]
async fn the_challenge_is_minted_once_and_verifies_once_published() {
    let Some(url) = database_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let organization = fresh_organization();
    let nothing = Published::new(&[]);
    let first = Verifier::new(pool(&url).await, &nothing, "portal.hel.fi")
        .verify(&organization, "hel.fi", now())
        .await
        .expect("stored");
    assert_eq!(first.state, State::Failed);
    assert_eq!(
        first.record,
        format!(
            "_joinedcontext.hel.fi TXT \"jc-verify={}\"",
            first.challenge
        )
    );

    // Another replica, or this one after a restart: the same challenge, not a new one.
    let published = Published::new(&[format!("jc-verify={}", first.challenge)]);
    let later = now() + Duration::minutes(11);
    let second = Verifier::new(pool(&url).await, &published, "portal.hel.fi")
        .verify(&organization, "hel.fi", later)
        .await
        .expect("stored");
    assert_eq!(second.challenge, first.challenge);
    assert_eq!(second.state, State::Verified);
    assert_eq!(second.method, Some(Method::DnsTxt));
}

/// PF-41: nothing is looked up before it is due, a verified domain stays verified for 30 days,
/// and a changed domain starts over with the same challenge.
#[tokio::test]
async fn a_look_happens_only_when_due_and_a_new_domain_starts_over() {
    let Some(url) = database_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let organization = fresh_organization();
    let pool = pool(&url).await;
    let probe = Published::new(&[]);
    let minted = Verifier::new(pool.clone(), &probe, "portal.hel.fi")
        .verify(&organization, "hel.fi", now())
        .await
        .expect("stored");

    let published = Published::new(&[format!("jc-verify={}", minted.challenge)]);
    let verifier = Verifier::new(pool, &published, "portal.hel.fi");
    // A failed domain waits ten minutes before the next look.
    let soon = verifier
        .verify(&organization, "hel.fi", now() + Duration::minutes(5))
        .await
        .expect("stored");
    assert_eq!(soon.state, State::Failed);
    assert_eq!(published.looks.load(Ordering::SeqCst), 0);

    let checked = now() + Duration::minutes(11);
    let verified = verifier
        .verify(&organization, "hel.fi", checked)
        .await
        .expect("stored");
    assert_eq!(verified.state, State::Verified);
    assert_eq!(published.looks.load(Ordering::SeqCst), 1);

    // Twenty-nine days on it is still the stored answer; at thirty it is looked at again.
    verifier
        .verify(&organization, "hel.fi", checked + Duration::days(29))
        .await
        .expect("stored");
    assert_eq!(published.looks.load(Ordering::SeqCst), 1);
    verifier
        .verify(&organization, "hel.fi", checked + Duration::days(30))
        .await
        .expect("stored");
    assert_eq!(published.looks.load(Ordering::SeqCst), 2);

    // Another domain is looked at at once, with the Organization's own challenge and a record
    // named for the new domain.
    let moved = verifier
        .verify(&organization, "espoo.fi", checked + Duration::days(30))
        .await
        .expect("stored");
    assert_eq!(moved.challenge, minted.challenge);
    assert_eq!(published.looks.load(Ordering::SeqCst), 3);
    assert!(
        moved.record.starts_with("_joinedcontext.espoo.fi TXT"),
        "{}",
        moved.record
    );
}

/// Architecture/03 §3, T-2572: what the gateway reads is the stored state of each Organization
/// for the domain it declares now; a domain changed since the last look, and an Organization
/// never looked at, are `pending`, whichever replica answers.
#[tokio::test]
async fn the_gate_reads_the_stored_state_for_the_declared_domain() {
    let Some(url) = database_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let organization = fresh_organization();
    let never_looked_at = fresh_organization();
    let pool = pool(&url).await;
    let first = Verifier::new(pool.clone(), &Published::new(&[]), "portal.hel.fi")
        .verify(&organization, "hel.fi", now())
        .await
        .expect("stored");
    let published = Published::new(&[format!("jc-verify={}", first.challenge)]);
    Verifier::new(pool.clone(), &published, "portal.hel.fi")
        .verify(&organization, "hel.fi", now() + Duration::minutes(11))
        .await
        .expect("stored");

    let asked = [
        (organization.clone(), "hel.fi".to_owned()),
        (never_looked_at.clone(), "espoo.fi".to_owned()),
    ];
    let states = gate_states(Some(&pool), &asked).await.expect("read");
    assert_eq!(states[0].state, State::Verified);
    assert_eq!(states[1].state, State::Pending);

    // The same Organization now declaring another domain is not verified by the old record.
    let moved = [(organization.clone(), "vantaa.fi".to_owned())];
    let states = gate_states(Some(&pool), &moved).await.expect("read");
    assert_eq!(states[0].state, State::Pending);
}
