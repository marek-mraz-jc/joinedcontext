#![forbid(unsafe_code)]
//! K9 dispatch passes: every realm is visited, a realm without SMTP settings keeps its row
//! pending with the error recorded (retry with backoff), and the production sender reads the
//! SMTP password from its file only at send time.

use jc_router::Mount;
use jc_time::TimestampTz;
use kaisatsu_core::{EmailOutboxRecord, RealmConfig, SmtpServerConfig, Store};
use kaisatsu_crypto::{Kek, KeyManager};
use kaisatsu_login::{MailSender, OutboxDispatcher, RealmSmtpSender, RecordingSender};
use kaisatsu_store_memory::MemoryStore;
use std::sync::Arc;

async fn realm(store: &Arc<dyn Store>, kek: &Kek, name: &str, smtp: Option<SmtpServerConfig>) {
    store
        .upsert_realm(RealmConfig {
            id: name.into(),
            realm: name.into(),
            smtp_server: smtp,
            ..Default::default()
        })
        .await
        .unwrap();
    store
        .upsert_signing_key(KeyManager::generate_es256(name, kek, 100, "ACTIVE").unwrap())
        .await
        .unwrap();
}

fn row(realm: &str, id: &str) -> EmailOutboxRecord {
    EmailOutboxRecord {
        id: id.into(),
        realm_id: realm.into(),
        org_id: None,
        user_id: "u-1".into(),
        to_address: "person@example.test".into(),
        locale: "en".into(),
        template_kind: "EXECUTE_ACTIONS".into(),
        action_jti: format!("jti-{id}"),
        actions: vec!["UPDATE_PASSWORD".into()],
        client_id: None,
        redirect_uri: None,
        lifespan_secs: 900,
        issuer: format!("https://idm.example.test/realms/{realm}"),
        status: "pending".into(),
        attempts: 0,
        last_error: None,
        next_attempt_at: TimestampTz::now() - jiff::SignedDuration::from_secs(1),
        created_at: TimestampTz::now(),
        sent_at: None,
    }
}

fn mount() -> Mount {
    Mount::new("kaisatsu", "https", "idm.example.test", "").unwrap()
}

/// S3-29: a dispatcher killed between its claim and the send leaves the row `sending`; another
/// dispatcher sharing the store delivers it once the claim's lease ran out, and a failed send
/// goes back to pending and is sent by a later pass.
#[tokio::test]
async fn a_killed_claimers_row_is_sent_by_another_dispatcher_after_the_lease() {
    let kek = Kek::generate_for_tests();
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new(kek.clone()));
    realm(&store, &kek, "alpha", None).await;
    store.enqueue_outbox(row("alpha", "ob-killed")).await.unwrap();
    // The killed dispatcher's claim, with a lease that has run out.
    let claimed = store
        .claim_outbox_batch("alpha", 1, std::time::Duration::from_millis(1), 5)
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    let sink = RecordingSender::new();
    sink.set_failures(1);
    let other = OutboxDispatcher::new(store.clone(), kek, Arc::new(sink.clone()));
    assert_eq!(other.dispatch_all_realms(&mount()).await.unwrap(), 0);
    let failed = store.list_outbox_rows("alpha", None).await.unwrap();
    assert_eq!(failed[0].status, "pending", "a failed send stays pending");
    assert_eq!(failed[0].attempts, 2);
    assert!(failed[0].last_error.is_some());
    // Due again after the backoff: the next pass sends it.
    store.retry_outbox_row("alpha", None, "ob-killed").await.unwrap();
    assert_eq!(other.dispatch_all_realms(&mount()).await.unwrap(), 1);
    let sent: Vec<_> = sink.drain().into_iter().map(|m| m.to_address).collect();
    assert_eq!(sent, ["person@example.test"]);
    assert_eq!(
        store.list_outbox_rows("alpha", None).await.unwrap()[0].status,
        "sent"
    );
    // The killed dispatcher's late outcome changes nothing.
    assert!(
        !store
            .complete_outbox_row(&claimed[0], false, Some("late".into()), None, 5)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn one_pass_visits_every_realm() {
    let kek = Kek::generate_for_tests();
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new(kek.clone()));
    for r in ["alpha", "beta"] {
        realm(&store, &kek, r, None).await;
        store
            .enqueue_outbox(row(r, &format!("ob-{r}")))
            .await
            .unwrap();
    }
    let sink = RecordingSender::new();
    let d = OutboxDispatcher::new(store.clone(), kek, Arc::new(sink.clone()));
    assert_eq!(d.dispatch_all_realms(&mount()).await.unwrap(), 2);
    let mut realms: Vec<_> = sink.drain().into_iter().map(|m| m.realm_id).collect();
    realms.sort();
    assert_eq!(realms, ["alpha", "beta"]);
    assert_eq!(
        d.dispatch_all_realms(&mount()).await.unwrap(),
        0,
        "sent rows are done"
    );
}

/// The mails say what old prod's Keycloak e-mail theme said (shot from the old image): its
/// subject, the realm's display name and the link's lifetime in words, per language.
#[tokio::test]
async fn mails_carry_keycloaks_words_and_the_realms_display_name() {
    let kek = Kek::generate_for_tests();
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new(kek.clone()));
    realm(&store, &kek, "smartcity", None).await;
    let mut r = store.get_realm("smartcity").await.unwrap().unwrap();
    r.display_name = "joinedcontext demo".into();
    store.upsert_realm(r).await.unwrap();
    let mail = |id: &str, kind: &str, locale: &str, secs: i64| EmailOutboxRecord {
        template_kind: kind.into(),
        locale: locale.into(),
        lifespan_secs: secs,
        ..row("smartcity", id)
    };
    for m in [
        mail("ob-reset", "RESET_CREDENTIALS", "en", 300),
        mail("ob-verify", "VERIFY_EMAIL", "en", 300),
        mail("ob-actions", "EXECUTE_ACTIONS", "sk", 43_200),
    ] {
        store.enqueue_outbox(m).await.unwrap();
    }
    let sink = RecordingSender::new();
    let d = OutboxDispatcher::new(store.clone(), kek, Arc::new(sink.clone()));
    assert_eq!(d.dispatch_all_realms(&mount()).await.unwrap(), 3);
    let sent = sink.drain();
    let by = |subject: &str| {
        let m = sent
            .iter()
            .find(|m| m.subject == subject)
            .unwrap_or_else(|| {
                panic!(
                    "no mail `{subject}` in {:?}",
                    sent.iter().map(|m| &m.subject).collect::<Vec<_>>()
                )
            });
        m.body.replace(&m.action_link, "<link>")
    };
    assert_eq!(
        by("Reset password"),
        "Someone just requested to change your joinedcontext demo account's credentials. If this was you, click on the link below to reset them.\n\n<link>\n\nThis link and code will expire within 5 minutes.\n\nIf you don't want to reset your credentials, just ignore this message and nothing will be changed."
    );
    assert_eq!(
        by("Verify email"),
        "Someone has created a joinedcontext demo account with this email address. If this was you, click the link below to verify your email address\n\n<link>\n\nThis link will expire within 5 minutes.\n\nIf you didn't create this account, just ignore this message."
    );
    let sk = by("Aktualizujte svoj účet");
    assert!(
        sk.contains("joinedcontext demo")
            && sk.contains("Aktualizovať heslo")
            && sk.contains("12 hodín"),
        "{sk}"
    );
}

#[tokio::test]
async fn realm_smtp_sender_refuses_without_settings_and_keeps_the_row_for_retry() {
    let kek = Kek::generate_for_tests();
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new(kek.clone()));
    realm(&store, &kek, "plain", None).await;
    let tls = SmtpServerConfig {
        host: "127.0.0.1".into(),
        port: "1".into(),
        from: "idm@example.test".into(),
        starttls: "true".into(),
        ..Default::default()
    };
    realm(&store, &kek, "mailing", Some(tls)).await;
    let missing = std::env::temp_dir().join(format!("kaisatsu-no-pw-{}", uuid::Uuid::now_v7()));
    let sender = RealmSmtpSender::new(store.clone(), missing.clone(), "127.0.0.1".into());

    let err = sender
        .send_mail("plain", "a@example.test", "s", "b", "l")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("smtpServer"), "{err}");
    let err = sender
        .send_mail("mailing", "a@example.test", "s", "b", "l")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("SMTP password file"), "{err}");
    // A realm naming another host gets no mail: the deployment's password stays with its host.
    let elsewhere = RealmSmtpSender::new(store.clone(), missing.clone(), "smtp.example.org".into());
    let err = elsewhere
        .send_mail("mailing", "a@example.test", "s", "b", "l")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("not the deployment's SMTP host"),
        "{err}"
    );

    store
        .enqueue_outbox(row("plain", "ob-plain"))
        .await
        .unwrap();
    let d = OutboxDispatcher::new(store.clone(), kek, Arc::new(sender));
    assert_eq!(d.dispatch_all_realms(&mount()).await.unwrap(), 0);
    let rows = store.list_outbox_rows("plain", None).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].attempts, 1);
    assert_eq!(rows[0].status, "pending", "retried later with backoff");
    assert!(
        rows[0]
            .last_error
            .as_deref()
            .unwrap_or_default()
            .contains("smtpServer")
    );
}
