//! `POST /apps/{name}/api/services/email/send`: an App's mail reaches people of the organization
//! through the relay, one message per recipient, within its layers and quotas, and stops for a
//! person who unsubscribed (AP-163…AP-165, AP-168, API/06 §3, T-3583).
//!
//! The relay is a fake SMTP server on a local port that keeps every message it is handed; the
//! realm is the process's signing realm. Runs when `JC_PORTAL_TEST_DATABASE_URL` points at a
//! PostgreSQL the test may write to, like the other database suites.

mod common;

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

use common::envelope;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::state::AppState;

const APP: &str = "road-defects";

/// What the fake relay was handed: the envelope recipients and the message, per message.
type Inbox = Arc<Mutex<Vec<(Vec<String>, String)>>>;

/// An SMTP server that says yes to everything and keeps each message.
async fn relay() -> (String, Inbox) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let url = format!("smtp://{}", listener.local_addr().expect("address"));
    let inbox = Inbox::default();
    let kept = inbox.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let kept = kept.clone();
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read).lines();
                let _ = write.write_all(b"220 fake ESMTP\r\n").await;
                let mut to = Vec::new();
                while let Ok(Some(line)) = lines.next_line().await {
                    let verb = line.to_ascii_uppercase();
                    let answer: &[u8] = if verb.starts_with("EHLO") || verb.starts_with("HELO") {
                        b"250 fake\r\n"
                    } else if let Some(rcpt) = verb.strip_prefix("RCPT TO:") {
                        to.push(rcpt.trim().to_ascii_lowercase());
                        b"250 OK\r\n"
                    } else if verb == "DATA" {
                        let _ = write.write_all(b"354 go on\r\n").await;
                        let mut data = String::new();
                        while let Ok(Some(line)) = lines.next_line().await {
                            if line == "." {
                                break;
                            }
                            data.push_str(&line);
                            data.push('\n');
                        }
                        kept.lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .push((std::mem::take(&mut to), data));
                        b"250 kept\r\n"
                    } else if verb == "QUIT" {
                        let _ = write.write_all(b"221 bye\r\n").await;
                        break;
                    } else {
                        b"250 OK\r\n"
                    };
                    let _ = write.write_all(answer).await;
                }
            });
        }
    });
    (url, inbox)
}

/// An SMTP server that refuses every recipient with a 550.
async fn refusing_relay() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let url = format!("smtp://{}", listener.local_addr().expect("address"));
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read).lines();
                let _ = write.write_all(b"220 fake ESMTP\r\n").await;
                while let Ok(Some(line)) = lines.next_line().await {
                    let answer: &[u8] = if line.to_ascii_uppercase().starts_with("RCPT") {
                        b"550 5.1.1 no such mailbox\r\n"
                    } else {
                        b"250 OK\r\n"
                    };
                    let _ = write.write_all(answer).await;
                }
            });
        }
    });
    url
}

fn delivered(inbox: &Inbox) -> Vec<(Vec<String>, String)> {
    inbox.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn app(services: Value, limits: Value) -> Value {
    json!({
        "kind": "static",
        "source": { "path": "." },
        "build": {},
        "visibility": "project",
        "lifecycle": "published",
        "dataNeeds": [],
        "services": services,
        "limits": limits,
    })
}

fn organization(services: Option<Value>) -> joinedcontext_portal::resource::ResourceEnvelope {
    let mut spec = json!({ "domain": "hel.fi", "locales": ["en"], "defaultLocale": "en" });
    if let Some(services) = services {
        spec["policies"] = json!({ "apps": { "services": services } });
    }
    envelope("Organization", "helsinki", "org", spec)
}

/// A project name of its own per test, so their counters never meet in the shared database.
fn project(test: &str) -> String {
    let id = crate::common::unique::unique();
    format!("{test}-{id}")
}

async fn database() -> Option<sqlx::PgPool> {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty());
    let Some(url) = url else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return None;
    };
    Some(
        joinedcontext_portal::db::connect(&url)
            .await
            .expect("the test database answers and migrates"),
    )
}

/// A Portal that trusts the process's realm and mails through `relay`, holding the organization
/// with email allowed and App `road-defects` of `project` with `app_spec`.
async fn state(relay: &str, db: sqlx::PgPool, project: &str, app_spec: Value) -> AppState {
    let config = Config::from_vars(|key| {
        match key {
            "JC_OIDC_ISSUER" => Some(common::REALM.issuer.as_str()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001")
            }
            "JC_OIDC_CLIENT_ID" => Some("portal-api"),
            "JC_OIDC_CLIENT_SECRET" => Some("secret"),
            "JC_PORTAL_SMTP_URL" => Some(relay),
            "JC_PORTAL_MAIL_FROM" => Some("noreply@dev.joinedcontext.com"),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("config");
    let mut state = AppState::new(config, None);
    state.db = Some(db);
    state
        .mirror
        .upsert(organization(Some(json!(["identity", "data", "email"]))));
    state.mirror.upsert(envelope("App", APP, project, app_spec));
    common::bind(&state.mirror, "jana", project);
    // The project in the Organization's registry row: its mail rows are keyed (T-7200).
    let pool = state.db.clone().expect("a database");
    common::keyed_project(&state, &pool, project, "helsinki").await;
    state
}

async fn call(
    state: &AppState,
    uri: &str,
    method: &str,
    body: Value,
) -> (StatusCode, Value, String) {
    let token = common::REALM.person_token(APP, "jana", &[]);
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("a request");
    let response = joinedcontext_portal::server::app(state.clone())
        .oneshot(request)
        .await
        .expect("a response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("a body");
    let text = String::from_utf8_lossy(&bytes).into_owned();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::Null),
        text,
    )
}

async fn send(state: &AppState, message: Value) -> (StatusCode, Value, String) {
    call(
        state,
        &format!("/apps/{APP}/api/services/email/send"),
        "POST",
        message,
    )
    .await
}

fn to_me(subject: &str) -> Value {
    json!({ "to": "me", "subject": subject, "text": "A pothole on Hlavná 4." })
}

/// AP-168: one send, one message, to the signed-in person's own address, from the installation's
/// sender with the App as its name, carrying the one-click unsubscribe (RFC 8058).
#[tokio::test]
async fn a_send_to_me_hands_the_relay_one_message_with_its_unsubscribe_link() {
    let Some(db) = database().await else { return };
    let (url, inbox) = relay().await;
    let project = project("one");
    let state = state(&url, db, &project, app(json!(["email"]), json!({}))).await;

    let (status, body, text) = send(&state, to_me("New road defect")).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{text}");
    assert!(
        body["id"].as_str().is_some_and(|id| !id.is_empty()),
        "{text}"
    );

    let messages = delivered(&inbox);
    assert_eq!(messages.len(), 1, "one message per send");
    let (to, data) = &messages[0];
    assert_eq!(to, &vec!["<jana@hel.fi>".to_owned()]);
    assert!(data.contains("Subject: New road defect"), "{data}");
    assert!(
        data.contains("From: road-defects <noreply@dev.joinedcontext.com>"),
        "{data}"
    );
    assert!(
        data.contains("List-Unsubscribe-Post: List-Unsubscribe=One-Click"),
        "{data}"
    );
    assert!(data.contains("/mail/unsubscribe?p="), "{data}");
}

/// AP-168: "me" is the signed-in person's own address, so a token whose email is no address
/// names no recipient: the send is refused at `me` and the relay hears nothing.
#[tokio::test]
async fn a_send_to_me_without_an_address_is_refused_at_me() {
    let Some(db) = database().await else { return };
    let (url, inbox) = relay().await;
    let project = project("noaddress");
    let state = state(&url, db, &project, app(json!(["email"]), json!({}))).await;
    let token = common::REALM.person_token_with(
        &format!("app-{APP}"),
        &format!("app-{APP}"),
        "jana",
        &[],
        json!({ "email": "not an address" }),
    );
    let request = Request::builder()
        .method("POST")
        .uri(format!("/apps/{APP}/api/services/email/send"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(to_me("Defect").to_string()))
        .expect("a request");
    let response = joinedcontext_portal::server::app(state.clone())
        .oneshot(request)
        .await
        .expect("a response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("a body");
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["to"], "me", "{body}");
    assert!(delivered(&inbox).is_empty());
}

/// AP-163, AP-164: the email service is on only where the organization, the project and the App
/// all list it, and the refusal names the layer that did not.
#[tokio::test]
async fn a_service_off_at_any_layer_is_refused_with_that_layer() {
    let Some(db) = database().await else { return };
    let (url, inbox) = relay().await;
    let project = project("off");

    let no_email = state(&url, db.clone(), &project, app(json!(["files"]), json!({}))).await;
    let (status, body, text) = send(&no_email, to_me("x")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    assert_eq!(body["type"], "https://joinedcontext.com/errors/service-off");
    assert_eq!(
        (body["service"].as_str(), body["layer"].as_str()),
        (Some("email"), Some("app"))
    );

    let listed = state(&url, db.clone(), &project, app(json!(["email"]), json!({}))).await;
    listed.mirror.upsert(envelope(
        "Project",
        &project,
        "org",
        json!({ "organizationRef": { "kind": "Organization", "name": "helsinki" }, "apps": { "services": ["files"] } }),
    ));
    let (_, body, text) = send(&listed, to_me("x")).await;
    assert_eq!(body["layer"], "project", "{text}");

    // Absent, the organization's list is identity, data, files and jobs: no email.
    let default = state(&url, db, &project, app(json!(["email"]), json!({}))).await;
    default.mirror.upsert(organization(None));
    let (_, body, text) = send(&default, to_me("x")).await;
    assert_eq!(body["layer"], "organization", "{text}");

    assert!(
        delivered(&inbox).is_empty(),
        "a refused send reaches no relay"
    );
}

/// AP-165: the n+1st message of the day is refused with `429`, the quota and when it resets, and
/// never reaches the relay.
#[tokio::test]
async fn the_send_past_the_quota_is_refused_with_429() {
    let Some(db) = database().await else { return };
    let (url, inbox) = relay().await;
    let project = project("quota");
    let state = state(
        &url,
        db,
        &project,
        app(json!(["email"]), json!({ "emailsPerDay": 2 })),
    )
    .await;

    for n in 0..2 {
        let (status, _, text) = send(&state, to_me(&format!("defect {n}"))).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{text}");
    }
    let (status, body, text) = send(&state, to_me("one too many")).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{text}");
    assert_eq!(body["type"], "https://joinedcontext.com/errors/quota");
    assert_eq!(body["quota"], "emailsPerDay");
    assert!(
        body["resetAt"]
            .as_str()
            .is_some_and(|at| at.ends_with("T00:00:00Z")),
        "{text}"
    );
    assert_eq!(delivered(&inbox).len(), 2);
}

/// `state` mailing through `relay`, or through no relay at all.
fn with_relay(state: &AppState, relay: Option<&str>) -> AppState {
    let mut config = (*state.config).clone();
    config.smtp_url = relay.map(str::to_owned);
    let mut state = state.clone();
    state.config = Arc::new(config);
    state
}

/// API/06 §3: a body sent as another media type or that is no message is refused; an
/// installation without a relay, with a relay URL that does not parse or whose relay cannot be
/// reached answers 503, and the relay hands nobody anything.
#[tokio::test]
async fn a_wrong_body_or_a_missing_misconfigured_or_unreachable_relay_sends_nothing() {
    let Some(db) = database().await else { return };
    let (url, inbox) = relay().await;
    let project = project("relays");
    let state = state(&url, db, &project, app(json!(["email"]), json!({}))).await;

    let token = common::REALM.person_token(APP, "jana", &[]);
    let request = Request::post(format!("/apps/{APP}/api/services/email/send"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from(to_me("plain").to_string()))
        .expect("a request");
    let response = joinedcontext_portal::server::app(state.clone())
        .oneshot(request)
        .await
        .expect("a response");
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let (status, _, text) = send(&state, json!({ "to": "me", "body": "x" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    assert!(text.contains("not a message"), "{text}");

    let refusing = refusing_relay().await;
    for (relay, said) in [
        (None, "sends no mail"),
        (Some("relay.example.org:25"), "misconfigured"),
        (Some("smtp://127.0.0.1:1"), "did not take the message"),
        (Some(refusing.as_str()), "did not take the message"),
    ] {
        let (status, _, text) = send(&with_relay(&state, relay), to_me("defect")).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{relay:?}: {text}");
        assert!(text.contains(said), "{relay:?}: {text}");
    }

    // People named by id need the realm's admin client, and a realm that answers.
    let by_id = json!({ "to": ["steward-1"], "subject": "Defect", "text": "Hlavná 4" });
    let (status, _, text) = send(&state, by_id.clone()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{text}");
    assert!(text.contains("no admin client"), "{text}");
    let mut unreachable = state.clone();
    unreachable.people = joinedcontext_portal::people::People::new(
        "http://127.0.0.1:1/realms/banskabystrica",
        "portal-admin".into(),
        joinedcontext_portal::config::ClientAuth::Secret("secret".into()),
    )
    .map(Arc::new);
    let (status, _, text) = send(&unreachable, by_id).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{text}");
    assert!(text.contains("unreachable"), "{text}");
    assert!(delivered(&inbox).is_empty());
}

/// A database that stops answering is a server error, never a message sent or a stop lost
/// silently.
#[tokio::test]
async fn a_database_that_stops_answering_sends_nothing_and_stops_nothing() {
    let Some(db) = database().await else { return };
    let (url, inbox) = relay().await;
    let project = project("db-down");
    let state = state(&url, db.clone(), &project, app(json!(["email"]), json!({}))).await;
    assert_eq!(send(&state, to_me("first")).await.0, StatusCode::ACCEPTED);
    let link = unsubscribe_link(&delivered(&inbox)[0].1);
    db.close().await;
    let (status, _, text) = send(&state, to_me("second")).await;
    assert!(status.is_server_error(), "{status}: {text}");
    let (status, _, text) = call(&state, &link, "POST", Value::Null).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{text}");
    assert_eq!(delivered(&inbox).len(), 1);
}

/// A database of the test's own that refuses the day's counter, then holds no unsubscribes to
/// read or write: each is a server error, and nothing more is sent (T-7921).
#[tokio::test]
async fn a_refused_counter_or_unsubscribe_store_sends_nothing() {
    let db = common::build_lane::fresh_db()
        .await
        .expect("JC_PORTAL_TEST_DATABASE_URL names the test database");
    let (url, inbox) = relay().await;
    let project = project("db-refuses");
    let state = state(&url, db.clone(), &project, app(json!(["email"]), json!({}))).await;
    assert_eq!(send(&state, to_me("first")).await.0, StatusCode::ACCEPTED);
    let link = unsubscribe_link(&delivered(&inbox)[0].1);
    for statement in [
        "CREATE FUNCTION refused() RETURNS trigger LANGUAGE plpgsql AS \
         $$ BEGIN RAISE EXCEPTION 'refused by the test'; END $$",
        "CREATE TRIGGER refused BEFORE INSERT OR UPDATE ON app_service_usage \
         FOR EACH STATEMENT EXECUTE FUNCTION refused()",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(statement))
            .execute(&db)
            .await
            .expect("a refusing trigger");
    }
    let (status, _, text) = send(&state, to_me("counted")).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{text}");
    sqlx::query("DROP TABLE email_unsubscribes")
        .execute(&db)
        .await
        .expect("drop");
    let (status, _, text) = send(&state, to_me("unread")).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{text}");
    let (status, _, text) = call(&state, &link, "POST", Value::Null).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{text}");
    assert_eq!(delivered(&inbox).len(), 1);
}

/// AP-168: an HTML body travels beside the text as an alternative, its unsubscribe link escaped
/// for an attribute.
#[tokio::test]
async fn an_html_body_is_an_alternative_part_with_its_own_unsubscribe_link() {
    let Some(db) = database().await else { return };
    let (url, inbox) = relay().await;
    let project = project("html");
    let state = state(&url, db, &project, app(json!(["email"]), json!({}))).await;
    // The App's title names the sender, its line breaks taken out.
    let mut titled = envelope("App", APP, &project, app(json!(["email"]), json!({})));
    titled.metadata.title = Some(jc_core::i18n::Text::from("Road\ndefects"));
    state.mirror.upsert(titled);
    let (status, _, text) = send(
        &state,
        json!({ "to": "me", "subject": "Defect", "text": "A pothole.", "html": "<p>A pothole.</p>" }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{text}");
    let data = delivered(&inbox)[0]
        .1
        .replace("=\r\n", "")
        .replace("=\n", "");
    assert!(
        data.contains("From: Roaddefects <noreply@dev.joinedcontext.com>")
            || data.contains("From: \"Roaddefects\""),
        "{data}"
    );
    assert!(data.contains("multipart/alternative"), "{data}");
    assert!(data.contains("text/html"), "{data}");
    assert!(data.contains("To stop these messages</a>"), "{data}");
    assert!(
        data.contains("&amp;a=3D") || data.contains("&amp;a="),
        "{data}"
    );
}

/// The unsubscribe link of a delivered message. The plain part is quoted-printable or 7bit; the
/// header's angle-bracketed URL is not folded by lettre below 76 characters per line, so it is
/// read from the text part's footer.
fn unsubscribe_link(data: &str) -> String {
    let unfolded = data.replace("=\n", "").replace("=3D", "=");
    let start = unfolded.find("/mail/unsubscribe?").expect("a link");
    unfolded[start..]
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '>')
        .collect()
}

/// AP-168: past 20 messages a day to one person the App is refused with that quota, even with
/// its day not spent.
#[tokio::test]
async fn one_person_gets_at_most_twenty_messages_a_day() {
    let Some(db) = database().await else { return };
    let (url, inbox) = relay().await;
    let project = project("per-person");
    let state = state(
        &url,
        db,
        &project,
        app(json!(["email"]), json!({ "emailsPerDay": 100 })),
    )
    .await;
    for n in 0..20 {
        let (status, _, text) = send(&state, to_me(&format!("defect {n}"))).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{text}");
    }
    let (status, body, text) = send(&state, to_me("one too many")).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{text}");
    assert_eq!(body["quota"], "emailsPerRecipientPerDay");
    assert_eq!(delivered(&inbox).len(), 20);
}

/// T-3583 security: a line break in the subject is a header injection and is refused before
/// anything is counted or sent; an address in `to` is no person id, so there is no open relay.
#[tokio::test]
async fn header_injection_and_an_address_as_recipient_are_refused() {
    let Some(db) = database().await else { return };
    let (url, inbox) = relay().await;
    let project = project("inject");
    let state = state(&url, db, &project, app(json!(["email"]), json!({}))).await;

    let (status, _, text) = send(&state, to_me("Defect\r\nBcc: everyone@example.org")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    let (status, body, text) = send(
        &state,
        json!({ "to": ["victim@example.org"], "subject": "x", "text": "y" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    assert_eq!(
        body["type"],
        "https://joinedcontext.com/errors/recipient-refused"
    );

    // What a cross-site form could post is no message.
    let token = common::REALM.person_token(APP, "jana", &[]);
    let form = Request::post(format!("/apps/{APP}/api/services/email/send"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from(to_me("x").to_string()))
        .expect("a request");
    let response = joinedcontext_portal::server::app(state.clone())
        .oneshot(form)
        .await
        .expect("a response");
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert!(delivered(&inbox).is_empty());
}

/// AP-168: a person id is looked up in the realm; a verified address gets the message, an id the
/// realm does not hold is refused and nobody gets anything.
#[tokio::test]
async fn people_are_named_by_id_and_an_unknown_one_is_refused() {
    let Some(db) = database().await else { return };
    let (url, inbox) = relay().await;
    let project = project("people");
    let mut state = state(&url, db, &project, app(json!(["email"]), json!({}))).await;
    let realm = common::REALM.server;
    Mock::given(method("POST"))
        .and(path("/realms/banskabystrica/protocol/openid-connect/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "admin", "token_type": "Bearer", "expires_in": 300
        })))
        .mount(realm)
        .await;
    Mock::given(method("GET"))
        .and(path("/admin/realms/banskabystrica/users/steward-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "steward-1", "username": "eva", "email": "eva@bbsk.sk",
            "enabled": true, "emailVerified": true
        })))
        .mount(realm)
        .await;
    Mock::given(method("GET"))
        .and(path("/admin/realms/banskabystrica/users/unverified-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "unverified-1", "username": "ivo", "email": "ivo@bbsk.sk",
            "enabled": true, "emailVerified": false
        })))
        .mount(realm)
        .await;
    state.people = joinedcontext_portal::people::People::new(
        &common::REALM.issuer,
        "portal-admin".into(),
        joinedcontext_portal::config::ClientAuth::Secret("secret".into()),
    )
    .map(Arc::new);

    let message = |to: Value| json!({ "to": to, "subject": "Defect", "text": "Hlavná 4" });
    let (status, _, text) = send(&state, message(json!(["steward-1"]))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{text}");
    for refused in ["nobody-1", "unverified-1"] {
        let (status, body, text) = send(&state, message(json!(["steward-1", refused]))).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
        assert_eq!(body["to"], refused);
    }
    let messages = delivered(&inbox);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].0, vec!["<eva@bbsk.sk>".to_owned()]);
}

/// AP-168, ADR-N-058: with several Organizations an App mails only people of the group subtree of
/// the Organization that owns its project; another Organization's verified person is refused.
#[tokio::test]
async fn a_person_of_another_organization_is_refused() {
    let Some(db) = database().await else { return };
    let (url, inbox) = relay().await;
    let project = project("orgs");
    let mut state = state(&url, db, &project, app(json!(["email"]), json!({}))).await;
    let mut helsinki = organization(Some(json!(["identity", "data", "email"])));
    helsinki.spec["installation"] = json!(true);
    state.mirror.upsert(helsinki);
    state.mirror.upsert(envelope(
        "Organization",
        "zilina",
        "org",
        json!({ "domain": "zilina.sk", "locales": ["en"], "defaultLocale": "en" }),
    ));
    let realm = common::REALM.server;
    Mock::given(method("POST"))
        .and(path("/realms/banskabystrica/protocol/openid-connect/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "admin", "token_type": "Bearer", "expires_in": 300
        })))
        .mount(realm)
        .await;
    for (id, address, group) in [
        ("hel-1", "eva@hel.fi", "/helsinki/stewards"),
        ("zil-1", "zoe@zilina.sk", "/zilina/stewards"),
        ("hel-lookalike-1", "ada@hel.fi", "/helsinki-old/stewards"),
    ] {
        Mock::given(method("GET"))
            .and(path(format!("/admin/realms/banskabystrica/users/{id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": id, "username": address, "email": address,
                "enabled": true, "emailVerified": true
            })))
            .mount(realm)
            .await;
        Mock::given(method("GET"))
            .and(path(format!(
                "/admin/realms/banskabystrica/users/{id}/groups"
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "id": format!("g-{id}"), "name": "stewards", "path": group }
            ])))
            .mount(realm)
            .await;
    }
    // A person whose groups the realm cannot list is not known to be inside.
    Mock::given(method("GET"))
        .and(path("/admin/realms/banskabystrica/users/nogroups-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "nogroups-1", "username": "noa@hel.fi", "email": "noa@hel.fi",
            "enabled": true, "emailVerified": true
        })))
        .mount(realm)
        .await;
    Mock::given(method("GET"))
        .and(path("/admin/realms/banskabystrica/users/nogroups-1/groups"))
        .respond_with(ResponseTemplate::new(404))
        .mount(realm)
        .await;
    state.people = joinedcontext_portal::people::People::new(
        &common::REALM.issuer,
        "portal-admin".into(),
        joinedcontext_portal::config::ClientAuth::Secret("secret".into()),
    )
    .map(Arc::new);

    let message = |to: Value| json!({ "to": to, "subject": "Defect", "text": "Hlavná 4" });
    let (status, _, text) = send(&state, message(json!(["hel-1"]))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{text}");
    for refused in ["zil-1", "hel-lookalike-1", "nogroups-1"] {
        let (status, body, text) = send(&state, message(json!(["hel-1", refused]))).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
        assert_eq!(body["to"], refused);
    }
    let messages = delivered(&inbox);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].0, vec!["<eva@hel.fi>".to_owned()]);
}

/// RFC 8058, AP-168: the link in a message stops that App's mail to that person; the next send
/// is still accepted and reaches nobody. A link with another signature stops nothing.
#[tokio::test]
async fn an_unsubscribed_person_gets_nothing() {
    let Some(db) = database().await else { return };
    let (url, inbox) = relay().await;
    let project = project("unsub");
    let state = state(&url, db, &project, app(json!(["email"]), json!({}))).await;

    assert_eq!(send(&state, to_me("first")).await.0, StatusCode::ACCEPTED);
    let link = unsubscribe_link(&delivered(&inbox)[0].1);

    let forged = link.replace("&k=", "&k=AAAA");
    let (status, _, _) = call(&state, &forged, "POST", Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, text) = call(&state, &forged, "GET", Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(text.contains("not valid"), "{text}");
    let (status, _, text) = call(&state, &link, "GET", Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert!(
        text.contains("<form method=post>"),
        "a GET asks first: {text}"
    );
    let (status, _, text) = call(&state, &link, "POST", Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{text}");

    let (status, _, text) = send(&state, to_me("second")).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{text}");
    assert_eq!(
        delivered(&inbox).len(),
        1,
        "the unsubscribed person got nothing more"
    );

    // Without its database the choice cannot be kept; a project that is gone has nothing to stop.
    let mut no_db = state.clone();
    no_db.db = None;
    assert_eq!(
        call(&no_db, &link, "POST", Value::Null).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    state
        .mirror
        .remove(&joinedcontext_portal::resource::ResourceKey {
            namespace: joinedcontext_portal::permissions::ORG_NAMESPACE.into(),
            kind: "Project".into(),
            name: project.clone(),
        })
        .expect("the project was in the mirror");
    let (status, _, text) = call(&state, &link, "POST", Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{text}");
    assert!(text.contains("nothing to stop"), "{text}");
}

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Every log line the Portal writes in this test binary, from every test: a thread's own
/// subscriber misses the lines of a callsite another thread registered first.
static LOGS: std::sync::LazyLock<Captured> = std::sync::LazyLock::new(|| {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter("joinedcontext_portal=trace")
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    // A second global subscriber cannot be set; the first one is this one.
    let _ = tracing::subscriber::set_global_default(subscriber);
    captured
});

/// T-3583: what a send logs names the App and a count, never an address, a subject or the relay,
/// in this test or any other of the binary.
#[tokio::test]
async fn no_address_subject_or_relay_reaches_a_log_line() {
    let captured = LOGS.clone();
    let Some(db) = database().await else { return };
    let (url, _inbox) = relay().await;
    let project = project("logs");
    let state = state(
        &url,
        db,
        &project,
        app(json!(["email"]), json!({ "emailsPerDay": 1 })),
    )
    .await;

    assert_eq!(
        send(&state, to_me("Secret subject")).await.0,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        send(&state, to_me("Secret subject")).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );

    let logs =
        String::from_utf8_lossy(&captured.0.lock().unwrap_or_else(|e| e.into_inner())).into_owned();
    assert!(logs.contains(&format!("project={project}")), "{logs}");
    assert!(logs.contains("app mail sent"), "{logs}");
    for leaked in [
        "@hel.fi",
        "@bbsk.sk",
        "Secret subject",
        "smtp://",
        "noreply@",
    ] {
        assert!(!logs.contains(leaked), "{leaked} in the log: {logs}");
    }
}
