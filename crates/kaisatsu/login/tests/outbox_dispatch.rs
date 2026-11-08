#![forbid(unsafe_code)]
//! K9 dispatch passes: every realm is visited, a realm without SMTP settings keeps its row
//! pending with the error recorded (retry with backoff), and the production sender reads the
//! SMTP password from its file only at send time.

use chrono::Utc;
use jc_router::Mount;
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
        next_attempt_at: Utc::now() - chrono::Duration::seconds(1),
        created_at: Utc::now(),
        sent_at: None,
    }
}

fn mount() -> Mount {
    Mount::new("kaisatsu", "https", "idm.example.test", "").unwrap()
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
    let sender = RealmSmtpSender::new(store.clone(), missing.clone());

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
