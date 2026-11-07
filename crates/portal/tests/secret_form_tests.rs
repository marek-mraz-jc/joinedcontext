//! The secret form (CC-06, MF-24, T-5971): a person's credential goes from the Portal form into
//! the repository encrypted with SOPS + age, readable by the Portal's identity and by nobody
//! holding the file alone; the form refuses what it cannot write, naming the field, and a name
//! another file declares.

use std::sync::{Arc, Mutex};

use age::secrecy::ExposeSecret;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::IntoResponse;
use axum_extra::extract::cookie::PrivateCookieJar;
use base64::Engine as _;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request as Seen, ResponseTemplate};

const CSRF: &str = "secret-form-csrf";
const REPO: &str = "/api/v1/repos/test-owner/test-repo";
const TYPED: &str = "typed-in-the-form-0123456789";

fn person(email: &str) -> Identity {
    Identity {
        client: None,
        subject: format!("f:1:{email}"),
        username: email.split('@').next().unwrap_or(email).to_owned(),
        email: Some(email.to_owned()),
        name: None,
        roles: Vec::new(),
        groups: Vec::new(),
        second_factor: false,
    }
}

fn cookies(config: &Config, identity: Identity) -> String {
    let now = session::now_unix();
    let s = Session {
        identity,
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: "id".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = session::store(PrivateCookieJar::new(config.cookie_key.clone()), &s).expect("store");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| {
            v.to_str()
                .expect("cookie")
                .split(';')
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    parts.push(format!("{CSRF_COOKIE}={CSRF}"));
    parts.join("; ")
}

fn envelope(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta::new(name, ORG_NAMESPACE),
        spec,
        status: None,
    }
}

/// A forge holding `held` (a hand-written SOPS file of project `ovzdusie`, and the same file in
/// project `other`), recording each
/// file write.
async fn forge(held: &str) -> (MockServer, Arc<Mutex<Vec<Value>>>) {
    let server = MockServer::start().await;
    let b64 = |text: &str| base64::engine::general_purpose::STANDARD.encode(text);
    for (route, body) in [
        (REPO.to_owned(), json!({ "default_branch": "main" })),
        (format!("{REPO}/pulls"), json!([])),
        (
            format!("{REPO}/git/trees/main"),
            json!({ "sha": "tree-1", "truncated": false, "tree": [
                { "path": "projects/ovzdusie/secrets/feed.enc.yaml", "type": "blob", "sha": "b1" },
                { "path": "projects/ovzdusie/spaces/air/space.yaml", "type": "blob", "sha": "b2" },
                // Another project of the same repository (layout 1), declaring the same name: its
                // file is neither listed nor a reason to refuse `feed` here (F-R2POR3-002).
                { "path": "projects/other/secrets/feed.enc.yaml", "type": "blob", "sha": "b3" }
            ] }),
        ),
        (
            format!("{REPO}/contents/projects/other/secrets/feed.enc.yaml"),
            json!({ "sha": "b3", "content": b64(held) }),
        ),
        (
            format!("{REPO}/contents/projects/ovzdusie/secrets/feed.enc.yaml"),
            json!({ "sha": "b1", "content": b64(held) }),
        ),
    ] {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path_regex(format!("^{REPO}/contents/.*")))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })))
        .with_priority(9)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{REPO}/branches")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&server)
        .await;
    let writes = Arc::new(Mutex::new(Vec::new()));
    for verb in ["POST", "PUT"] {
        let seen = Arc::clone(&writes);
        Mock::given(path_regex(format!("^{REPO}/contents/.*")))
            .and(method(verb))
            .respond_with(move |request: &Seen| {
                seen.lock()
                    .expect("lock")
                    .push(serde_json::from_slice::<Value>(&request.body).expect("json body"));
                ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "c1" } }))
            })
            .mount(&server)
            .await;
    }
    Mock::given(method("POST"))
        .and(path(format!("{REPO}/pulls")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 7, "html_url": "https://gitea.example.sk/pulls/7", "state": "open",
            "mergeable": true, "merged": false
        })))
        .mount(&server)
        .await;
    (server, writes)
}

struct World {
    app: axum::Router,
    config: Config,
    identity: age::x25519::Identity,
    writes: Arc<Mutex<Vec<Value>>>,
    _forge: MockServer,
    _dir: std::path::PathBuf,
}

async fn world() -> World {
    let identity = age::x25519::Identity::generate();
    let dir = std::env::temp_dir().join(format!(
        "secret-form-{}-{}",
        std::process::id(),
        rand_suffix()
    ));
    std::fs::create_dir_all(&dir).expect("dir");
    let key_file = dir.join("keys.txt");
    std::fs::write(
        &key_file,
        format!("{}\n", identity.to_string().expose_secret()),
    )
    .expect("key");
    let held = joinedcontext_portal::secret_files::encrypt(
        "feed",
        &[(
            "password".to_owned(),
            zeroize::Zeroizing::new("old".to_owned()),
        )]
        .into(),
        &[identity.to_public().to_string()],
        "2026-10-28T00:00:00Z",
    )
    .expect("held file");
    let (forge, writes) = forge(&held).await;

    let mut config = Config::for_tests();
    config.pipeline_secrets = Some(joinedcontext_portal::pipeline_secrets::Backend::Sops {
        age_key_file: key_file,
    });
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(
        joinedcontext_portal::git::GiteaClient::new(
            forge.uri().parse().expect("url"),
            "test-owner",
            "test-repo",
            "token-secret-form",
        )
        .expect("gitea"),
    ));
    state.mirror.upsert(envelope(
        "Role",
        "source-proposer",
        json!({ "rules": [{ "kinds": ["DataSource"], "verbs": ["read", "propose"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "pia-proposes",
        json!({ "subjects": [{ "user": "pia@hel.fi" }], "role": "source-proposer",
                "scope": { "project": "ovzdusie" } }),
    ));
    World {
        app: server::app(state),
        config,
        identity,
        writes,
        _forge: forge,
        _dir: dir,
    }
}

fn rand_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
}

impl World {
    async fn send(&self, email: &str, verb: &str, body: Option<Value>) -> (StatusCode, String) {
        let mut request = Request::builder()
            .method(verb)
            .uri("/api/v1/projects/ovzdusie/secrets")
            .header(header::COOKIE, cookies(&self.config, person(email)))
            .header(CSRF_HEADER, CSRF);
        if body.is_some() {
            request = request.header(header::CONTENT_TYPE, "application/json");
        }
        let request = request
            .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
            .expect("request");
        let response = self.app.clone().oneshot(request).await.expect("answer");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }
}

#[tokio::test]
async fn a_typed_secret_lands_encrypted_in_a_change_and_only_the_portal_reads_it() {
    let world = world().await;
    let (status, listed) = world.send("pia@hel.fi", "GET", None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let listed: Value = serde_json::from_str(&listed).expect("json");
    assert_eq!(
        listed["items"],
        json!([{ "name": "feed", "keys": ["password"],
                 "path": "projects/ovzdusie/secrets/feed.enc.yaml" }])
    );
    assert!(
        !listed.to_string().contains("ENC["),
        "a list names, it never carries a value"
    );

    let (status, answer) = world
        .send(
            "pia@hel.fi",
            "POST",
            Some(json!({ "name": "counters-api", "values": { "apiKey": TYPED } })),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{answer}");
    assert!(!answer.contains(TYPED));

    let writes = world.writes.lock().expect("lock").clone();
    assert_eq!(writes.len(), 1, "one file written: {writes:?}");
    let written = &writes[0];
    assert!(!written.to_string().contains(TYPED));
    let text = String::from_utf8(
        base64::engine::general_purpose::STANDARD
            .decode(written["content"].as_str().expect("content"))
            .expect("base64"),
    )
    .expect("utf-8");
    assert!(!text.contains(TYPED), "the file holds no plain value");
    let file = world._dir.join("counters-api.enc.yaml");
    std::fs::write(&file, &text).expect("write");
    let store =
        jcctl::secrets::SecretStore::load_file(&file, std::slice::from_ref(&world.identity))
            .expect("the Portal's identity opens it");
    let reference: jc_core::envelope::SecretRef =
        serde_json::from_value(json!({ "name": "counters-api", "key": "apiKey" })).expect("ref");
    assert_eq!(store.resolve(&reference).expect("resolved").expose(), TYPED);
    assert!(
        jcctl::secrets::SecretStore::load_file(&file, &[age::x25519::Identity::generate()])
            .is_err()
    );
}

#[tokio::test]
async fn the_form_refuses_a_bad_field_a_taken_name_and_a_reader() {
    let world = world().await;
    let (status, answer) = world
        .send(
            "pia@hel.fi",
            "POST",
            Some(json!({ "name": "../escape", "values": { "api key": TYPED, "ok": "" } })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");
    for field in [
        "\"path\":\"name\"",
        "values.api key:",
        "\"path\":\"values.ok\"",
    ] {
        assert!(answer.contains(field), "{field} named in {answer}");
    }
    assert!(!answer.contains(TYPED));

    let (status, answer) = world
        .send(
            "pia@hel.fi",
            "POST",
            Some(json!({ "name": "feed", "values": { "password": TYPED } })),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "the secret's own file is replaced: {answer}"
    );
    let (status, answer) = world
        .send(
            "eva@hel.fi",
            "POST",
            Some(json!({ "name": "x", "values": { "k": TYPED } })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{answer}");
    let (status, _) = world.send("eva@hel.fi", "GET", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(
        world.writes.lock().expect("lock").len() == 1,
        "only the replace wrote"
    );
}
