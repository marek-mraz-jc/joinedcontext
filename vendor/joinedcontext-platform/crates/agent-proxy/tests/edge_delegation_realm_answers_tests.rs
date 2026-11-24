//! What the realm answers to a run's grant, beyond a plain yes or no (ADR-N-038 §3, T-7795).
//!
//! An introspection or an exchange that is not JSON, an exchange with no refresh token, a refresh
//! the realm cannot be reached for or answers with a server error, a revocation it refuses or
//! never hears: each is held to what it costs. A failure of the realm keeps the grant for the
//! next call; a refusal ends it; a revocation that fails drops the grant from memory and says so
//! in the log, never with a token in the line.
//!
//! The realm here is a plain socket that answers each path as the case says, or closes the
//! connection without a word, which is what an unreachable realm looks like to the client.

use agent_proxy::config::Config;
use agent_proxy::delegation::{BindError, GrantError, Grants};
use agent_proxy::runs::RunContext;
use base64::Engine;
use std::io::Write;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const PERSON: &str = "demo.steward@hel.fi";
const PORTAL_TOKEN: &str = "portal-service-account-token";
const PERSON_TOKEN: &str = "person-access-token";
const REFRESH: &str = "delegated-refresh-token";

/// What the realm answers one request: a status and a body, or nothing (the connection closes).
type Answer = Option<(u16, String)>;

/// A realm on a socket: `answer` gets each request's path and form body, and the requests are
/// kept in order of arrival.
struct Realm {
    issuer: String,
    seen: Arc<Mutex<Vec<(String, String)>>>,
}

impl Realm {
    async fn start(answer: impl Fn(&str, &str) -> Answer + Send + Sync + 'static) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port");
        let addr = listener.local_addr().expect("an address");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (answer, log) = (Arc::new(answer), Arc::clone(&seen));
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let (answer, log) = (Arc::clone(&answer), Arc::clone(&log));
                tokio::spawn(async move {
                    let Some((path, body)) = read_request(&mut socket).await else {
                        return;
                    };
                    log.lock().expect("the log").push((path.clone(), body.clone()));
                    if let Some((status, text)) = answer(&path, &body) {
                        let head = format!(
                            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            text.len()
                        );
                        let _ = socket.write_all(head.as_bytes()).await;
                        let _ = socket.write_all(text.as_bytes()).await;
                    }
                });
            }
        });
        Self {
            issuer: format!("http://{addr}/realms/jc"),
            seen,
        }
    }

    fn paths(&self) -> Vec<String> {
        self.seen
            .lock()
            .expect("the log")
            .iter()
            .map(|(path, _)| path.clone())
            .collect()
    }

    fn bodies_at(&self, suffix: &str) -> Vec<String> {
        self.seen
            .lock()
            .expect("the log")
            .iter()
            .filter(|(path, _)| path.ends_with(suffix))
            .map(|(_, body)| body.clone())
            .collect()
    }
}

/// The path and body of one HTTP/1.1 request, read to its `Content-Length`.
async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<(String, String)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    while buf.len() < head_end + length {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let path = head.split_whitespace().nth(1)?.to_owned();
    Some((path, String::from_utf8_lossy(&buf[head_end..]).into_owned()))
}

/// The introspection of the person's token and the Portal's, both active.
fn introspection(body: &str) -> Answer {
    if body.contains(&format!("token={PORTAL_TOKEN}")) {
        return Some((200, r#"{"active":true,"azp":"portal-api"}"#.to_owned()));
    }
    Some((
        200,
        format!(r#"{{"active":true,"azp":"edge","username":"{PERSON}"}}"#),
    ))
}

/// An exchange answering a grant that is already due for its refresh.
fn exchange(refresh: Option<&str>) -> Answer {
    let refresh = refresh.map_or(String::new(), |r| format!(r#","refresh_token":"{r}""#));
    Some((
        200,
        format!(r#"{{"access_token":"delegated-1","expires_in":0{refresh}}}"#),
    ))
}

fn config(realm: &Realm, extra: &[(&'static str, String)]) -> Arc<Config> {
    let issuer = realm.issuer.clone();
    let extra = extra.to_vec();
    Arc::new(
        Config::from_lookup(move |key| {
            if let Some((_, value)) = extra.iter().find(|(name, _)| *name == key) {
                return Some(value.clone());
            }
            match key {
                "JC_OIDC_ISSUER" => Some(issuer.clone()),
                "JC_OIDC_CLIENT_ID" => Some("agent-proxy".to_owned()),
                "JC_OIDC_CLIENT_SECRET" => Some("proxy-client-secret".to_owned()),
                _ => None,
            }
        })
        .expect("a complete configuration"),
    )
}

fn run() -> RunContext {
    serde_json::from_value(serde_json::json!({
        "id": "run-1", "project": "helsinki", "appName": "bikes", "endpointSlug": "e",
        "allowsWrite": false, "branch": "b", "pathPrefix": "", "status": "building",
        "ticketHash": "", "maxTokens": 1, "allowedHosts": [], "requestsPerMinute": 1,
        "maxResponseBytes": 1, "createdBy": PERSON, "modelName": "m"
    }))
    .expect("a run")
}

/// Collects every log line of this suite's binary (see edge_delegation_tests.rs: one global
/// subscriber, since a per-test one misses callsites first hit on another thread).
#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<u8>>>);

impl Write for Lines {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("poisoned"))?
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn captured() -> String {
    static LINES: std::sync::OnceLock<Lines> = std::sync::OnceLock::new();
    let lines = LINES.get_or_init(|| {
        let lines = Lines::default();
        let writer = lines.clone();
        tracing::subscriber::set_global_default(
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::TRACE)
                .with_writer(move || writer.clone())
                .finish(),
        )
        .expect("the suite's subscriber is the only one");
        lines
    });
    String::from_utf8_lossy(&lines.0.lock().expect("the lines")).into_owned()
}

/// An introspection answer that is not JSON is the realm's failure, never a yes.
#[tokio::test]
async fn an_introspection_that_is_not_json_judges_nothing() {
    let realm = Realm::start(|_, _| Some((200, "<html>maintenance</html>".to_owned()))).await;
    let grants = Grants::new(config(&realm, &[]));
    let judged = grants.is_portal(PORTAL_TOKEN).await;
    assert!(judged.is_err(), "{judged:?}");
    assert!(!judged.unwrap_err().contains("<html>"));
}

/// The exchange that never answers, answers something unreadable, or grants no refresh token
/// binds nothing, and says which of the realm's faults it was.
#[tokio::test]
async fn an_exchange_the_realm_breaks_off_or_garbles_binds_nothing() {
    for (case, answer) in [
        ("closed", None),
        ("not json", Some((200, "{\"access_token\":".to_owned()))),
    ] {
        let realm = Realm::start(move |path, body| {
            if path.ends_with("/introspect") {
                return introspection(body);
            }
            answer.clone()
        })
        .await;
        let grants = Grants::new(config(&realm, &[]));
        let bound = grants.bind(&run(), PERSON_TOKEN).await;
        assert!(matches!(bound, Err(BindError::Transport(_))), "{case}: {bound:?}");
        assert!(matches!(grants.token("run-1").await, Err(GrantError::Missing)), "{case}");
    }
    let realm = Realm::start(|path, body| {
        if path.ends_with("/introspect") {
            return introspection(body);
        }
        exchange(None)
    })
    .await;
    let grants = Grants::new(config(&realm, &[]));
    match grants.bind(&run(), PERSON_TOKEN).await {
        Err(BindError::Refused(why)) => assert!(why.contains("no refresh token"), "{why}"),
        other => panic!("an exchange without a refresh token bound: {other:?}"),
    }
}

/// A token issued by a realm no Organization holds is the installation's to judge: it is
/// introspected and exchanged at the installation's endpoint, never at an Organization's.
#[tokio::test]
async fn a_token_of_a_realm_no_organization_holds_goes_to_the_installation() {
    let realm = Realm::start(|path, body| {
        if path.ends_with("/introspect") {
            return introspection(body);
        }
        exchange(Some(REFRESH))
    })
    .await;
    let organizations = serde_json::json!([{
        "organization": "bbsk",
        "issuer": "https://idm.bbsk.sk/realms/bbsk",
        "jwks_url": "https://idm.bbsk.sk/realms/bbsk/protocol/openid-connect/certs"
    }])
    .to_string();
    let grants = Grants::new(config(
        &realm,
        &[("JC_OIDC_ORGANIZATION_REALMS", organizations)],
    ));
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(br#"{"iss":"https://idm.elsewhere.example/realms/other/"}"#);
    let token = format!("eyJhbGciOiJSUzI1NiJ9.{payload}.c2ln");
    grants.bind(&run(), &token).await.expect("bound");
    assert!(
        realm.paths().iter().all(|p| p.starts_with("/realms/jc/")),
        "{:?}",
        realm.paths()
    );
}

/// A bound grant's refresh: unreachable, a server error or an unreadable answer is the realm's
/// failure and keeps the grant for the next call; a refresh that names a new refresh token is
/// the one the next refresh sends.
#[tokio::test]
async fn a_refresh_the_realm_fails_keeps_the_grant_and_a_new_refresh_token_is_kept() {
    let step = Arc::new(Mutex::new(0u32));
    let at = Arc::clone(&step);
    let realm = Realm::start(move |path, body| {
        if path.ends_with("/introspect") {
            return introspection(body);
        }
        if body.contains("token-exchange") {
            return exchange(Some(REFRESH));
        }
        match *at.lock().expect("the step") {
            0 => None,
            1 => Some((503, "{}".to_owned())),
            2 => Some((200, "[]".to_owned())),
            3 => exchange(Some("rotated-refresh-token")),
            _ => exchange(None),
        }
    })
    .await;
    let grants = Grants::new(config(&realm, &[]));
    grants.bind(&run(), PERSON_TOKEN).await.expect("bound");
    for case in 0..3 {
        *step.lock().expect("the step") = case;
        let token = grants.token("run-1").await;
        assert!(matches!(token, Err(GrantError::Transport(_))), "{case}: {token:?}");
    }
    *step.lock().expect("the step") = 3;
    assert_eq!(grants.token("run-1").await.expect("a token"), "delegated-1");
    *step.lock().expect("the step") = 4;
    assert_eq!(grants.token("run-1").await.expect("a token"), "delegated-1");
    let refreshes = realm.bodies_at("/token");
    let last = refreshes.last().expect("a refresh");
    assert!(last.contains("refresh_token=rotated-refresh-token"), "{last}");
}

/// Ending a run that holds no grant asks the realm nothing.
#[tokio::test]
async fn ending_a_run_without_a_grant_asks_the_realm_nothing() {
    let realm = Realm::start(|_, _| Some((200, "{}".to_owned()))).await;
    Grants::new(config(&realm, &[])).end("run-without-grant").await;
    assert!(realm.paths().is_empty(), "{:?}", realm.paths());
}

/// A revocation the realm refuses or never hears drops the grant all the same and says so in
/// the log, naming the run and never the refresh token.
#[tokio::test]
async fn a_revocation_refused_or_unheard_drops_the_grant_and_is_logged_without_the_token() {
    captured();
    for (case, revoked) in [("refused", Some((400, "{}".to_owned()))), ("unheard", None)] {
        let realm = Realm::start(move |path, body| {
            if path.ends_with("/introspect") {
                return introspection(body);
            }
            if path.ends_with("/revoke") {
                return revoked.clone();
            }
            exchange(Some(REFRESH))
        })
        .await;
        let grants = Grants::new(config(&realm, &[]));
        grants.bind(&run(), PERSON_TOKEN).await.expect("bound");
        grants.end("run-1").await;
        assert!(matches!(grants.token("run-1").await, Err(GrantError::Missing)), "{case}");
        assert_eq!(realm.bodies_at("/revoke").len(), 1, "{case}");
    }
    let lines = captured();
    assert!(lines.contains("the realm refused to revoke the run's grant"), "{lines}");
    assert!(lines.contains("the run's grant could not be revoked"), "{lines}");
    assert!(!lines.contains(REFRESH), "a refresh token reached a log line");
}

/// A proxy whose client credential cannot be read at the end of a run revokes nothing, drops
/// the grant, and says why.
#[tokio::test]
async fn a_revocation_without_a_readable_client_credential_is_logged_and_sends_nothing() {
    captured();
    let dir = std::env::temp_dir().join(format!("agent-proxy-revoke-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a directory");
    let assertion = dir.join("token");
    std::fs::write(&assertion, "projected-service-account-token").expect("the assertion");
    let realm = Realm::start(|path, body| {
        if path.ends_with("/introspect") {
            return introspection(body);
        }
        exchange(Some(REFRESH))
    })
    .await;
    let issuer = realm.issuer.clone();
    let path = assertion.display().to_string();
    let config = Config::from_lookup(move |key| match key {
        "JC_OIDC_ISSUER" => Some(issuer.clone()),
        "JC_OIDC_CLIENT_ASSERTION_FILE" => Some(path.clone()),
        _ => None,
    })
    .expect("a complete configuration");
    let grants = Grants::new(Arc::new(config));
    grants.bind(&run(), PERSON_TOKEN).await.expect("bound");
    std::fs::remove_file(&assertion).expect("the assertion is gone");
    grants.end("run-1").await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(realm.bodies_at("/revoke").is_empty());
    assert!(matches!(grants.token("run-1").await, Err(GrantError::Missing)));
    assert!(captured().contains("the run's grant could not be revoked"));
}
