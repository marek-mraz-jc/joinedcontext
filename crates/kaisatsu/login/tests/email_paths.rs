#![forbid(unsafe_code)]
//! E-mail sender refusals (W7): addresses, TLS-only configuration, a server that does not
//! answer, and action links that are not action tokens or have no key to sign with.

use kaisatsu_core::{RealmConfig, SmtpServerConfig, Store};
use kaisatsu_crypto::{Kek, KeyManager, sign_jwt};
use kaisatsu_login::EmailSender;
use kaisatsu_store_memory::MemoryStore;
use std::sync::Arc;

const ISS: &str = "https://idm.example.test/realms/dev";

fn sender(cfg: SmtpServerConfig) -> EmailSender {
    let dir = std::env::temp_dir().join(format!("kaisatsu-smtp-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let pw = dir.join("pw");
    std::fs::write(&pw, "smtp-pass\n").unwrap();
    let s = EmailSender::from_secret_mount(cfg, &pw).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    s
}

fn closed_port() -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    port.to_string()
}

#[tokio::test]
async fn sending_fails_closed_without_leaking_the_password() {
    let base = SmtpServerConfig {
        host: "127.0.0.1".into(),
        port: closed_port(),
        from: "IdM <idm@example.org>".into(),
        user: "idm".into(),
        starttls: "true".into(),
        auth: "true".into(),
        ssl: "false".into(),
        ..SmtpServerConfig::default()
    };
    for cfg in [
        base.clone(),
        SmtpServerConfig {
            ssl: "true".into(),
            starttls: "false".into(),
            auth: "false".into(),
            ..base.clone()
        },
    ] {
        let s = sender(cfg);
        let msg = s
            .build_message("anna@example.org", "subject", "body".into())
            .unwrap();
        let err = s.send(msg).await.unwrap_err().to_string();
        assert!(err.contains("SMTP delivery failed"), "{err}");
        assert!(!err.contains("smtp-pass"));
    }
    let bad_port = sender(SmtpServerConfig {
        port: "not-a-port".into(),
        ..base.clone()
    });
    let msg = bad_port
        .build_message("anna@example.org", "s", "b".into())
        .unwrap();
    assert!(
        bad_port
            .send(msg.clone())
            .await
            .unwrap_err()
            .to_string()
            .contains("invalid SMTP port")
    );
    assert!(
        EmailSender::default()
            .send(msg)
            .await
            .unwrap_err()
            .to_string()
            .contains("not configured")
    );
    let bad_from = sender(SmtpServerConfig {
        from: "not an address".into(),
        ..base.clone()
    });
    assert!(
        bad_from
            .build_message("anna@example.org", "s", "b".into())
            .is_err()
    );
    let ok = sender(base);
    assert!(ok.build_message("no address", "s", "b".into()).is_err());
    let raw = String::from_utf8(
        ok.build_message("anna@example.org", "Hello", "Link".into())
            .unwrap()
            .formatted(),
    )
    .unwrap();
    assert!(raw.contains("From: IdM <idm@example.org>") && raw.contains("Subject: Hello"));
    let (subject, body) = ok
        .build_localized_email("anna@example.org", "sk", "https://idm/l")
        .unwrap();
    assert!(!subject.is_empty() && body.ends_with("https://idm/l"));
}

async fn store_with_realm() -> (Arc<dyn Store>, Kek) {
    let kek = Kek::generate_for_tests();
    let mem = MemoryStore::new(kek.clone());
    mem.upsert_realm(RealmConfig {
        id: "dev".into(),
        realm: "dev".into(),
        ..Default::default()
    })
    .await
    .unwrap();
    let store: Arc<dyn Store> = Arc::new(mem);
    (store, kek)
}

#[tokio::test]
async fn action_links_need_the_action_type_and_the_realm_key() {
    let (store, kek) = store_with_realm().await;
    // The lifespan is capped at 12 hours.
    let long = EmailSender::mint_action_token(
        &store,
        &kek,
        "dev",
        ISS,
        "u1",
        vec!["VERIFY_EMAIL".into()],
        None,
        None,
        Some(10 * 86_400),
    )
    .await
    .unwrap();
    let claims = EmailSender::consume_action_token(&store, "dev", ISS, &long)
        .await
        .unwrap();
    assert_eq!(claims.exp - claims.iat, 43_200);
    assert!(claims.client_id.is_none());

    // An access token signed by the realm is not an action link; nor is another issuer's token.
    let key = store
        .list_signing_keys("dev")
        .await
        .unwrap()
        .into_iter()
        .find(|k| k.status == "ACTIVE")
        .unwrap();
    let now = chrono::Utc::now().timestamp();
    let access = sign_jwt(
        &key,
        &kek,
        None,
        &serde_json::json!({"iss": ISS, "aud": ISS, "sub": "u1", "jti": "j", "typ": "Bearer",
            "actions": [], "iat": now, "exp": now + 60}),
    )
    .unwrap();
    let err = EmailSender::consume_action_token(&store, "dev", ISS, &access)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not an action token"));
    let foreign = KeyManager::generate_es256("dev", &kek, 1, "ACTIVE").unwrap();
    let forged = sign_jwt(
        &foreign,
        &kek,
        None,
        &serde_json::json!({"iss": ISS, "aud": ISS, "sub": "u1", "jti": "j2", "typ": "Action",
            "actions": [], "iat": now, "exp": now + 60}),
    )
    .unwrap();
    assert!(
        EmailSender::consume_action_token(&store, "dev", ISS, &forged)
            .await
            .is_err()
    );
    assert!(
        EmailSender::consume_action_token(&store, "dev", "https://other/realms/dev", &long)
            .await
            .is_err()
    );
}
