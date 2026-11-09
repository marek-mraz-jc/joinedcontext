//! Role split: kaisatsu in one `jc`, the modules the Portal calls in another. A client-credentials
//! token kaisatsu signs with a module's name as audience lets the caller in as that module's
//! service caller (the door swaps it for the module's per-boot credential); a token for another
//! module, a token of a client the door does not name, or no token at all is refused by the
//! module as before (a 4xx; daiya answers an unknown API key 404). The Portal side's loopback forwarder fetches that token itself.

use jc::config::{Cli, Config, DoorSection, FileConfig, RemoteSection};
use jc::door::Remote;
use jc::{Module, MountMode};
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

const REALM: &str = "split";
const CLIENT: &str = "portal-remote";
const OTHER: &str = "other-service";
const SECRET: &str = "split-role-client-secret-0123456789";
/// The modules behind the door and the audience each token names.
const SERVED: [Module; 5] = [
    Module::Shako,
    Module::Ekiben,
    Module::Daiya,
    Module::Dataroom,
    Module::Antares,
];

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// jc on a port of [`free_port`]. The base host names the port before jc binds it, so the port
/// is free only when asked: when another bind took it in between, jc starts on another one.
async fn start_on_a_free_port(cfg_of: impl Fn(u16) -> Config, what: &str) -> (u16, jc::Running) {
    for _ in 0..5 {
        let port = free_port();
        match jc::start(&cfg_of(port), CancellationToken::new()).await {
            Ok(running) => return (port, running),
            Err(jc::StartError::Bind { source, .. })
                if source.kind() == std::io::ErrorKind::AddrInUse => {}
            Err(e) => panic!("{what}: {e}"),
        }
    }
    panic!("{what}: another bind took jc's port 5 times");
}

/// A realm with two service-account clients, each allowed one scope per module; the scope puts
/// the module's name into `aud`.
fn realm(secret_file: &Path) -> Value {
    let scopes: Vec<Value> = SERVED
        .iter()
        .map(|m| {
            json!({
                "name": m.name(),
                "protocol": "openid-connect",
                "attributes": { "include.in.token.scope": "true" },
                "protocolMappers": [{
                    "name": format!("{}-audience", m.name()),
                    "protocol": "openid-connect",
                    "protocolMapper": "oidc-audience-mapper",
                    "consentRequired": false,
                    "config": {
                        "access.token.claim": "true",
                        "id.token.claim": "false",
                        "included.custom.audience": m.name(),
                    },
                }],
            })
        })
        .collect();
    let names: Vec<&str> = SERVED.iter().map(|m| m.name()).collect();
    let client = |id: &str| {
        json!({
            "clientId": id,
            "enabled": true,
            "protocol": "openid-connect",
            "publicClient": false,
            "clientAuthenticatorType": "client-secret",
            "secret": format!("$(file:{})", secret_file.display()),
            "standardFlowEnabled": false,
            "directAccessGrantsEnabled": false,
            "serviceAccountsEnabled": true,
            "defaultClientScopes": [],
            "optionalClientScopes": names,
        })
    };
    json!({
        "realm": REALM,
        "enabled": true,
        "clientScopes": scopes,
        "clients": [client(CLIENT), client(OTHER)],
    })
}

fn config(roles: &str, port: u16, state: &Path, file: FileConfig) -> Config {
    let mut cfg = Config::merge(
        Cli {
            role: Some(roles.into()),
            listen: Some(SocketAddr::from(([127, 0, 0, 1], port))),
            base_host: Some(format!("127.0.0.1:{port}")),
            scheme: Some("http".into()),
            ekiben_file_root: Some(state.join("ekiben-files")),
            shako_repo_root: Some(state.join("repos")),
            ..Cli::default()
        },
        file,
    )
    .expect("config");
    cfg.mode = MountMode::Slug;
    cfg.state_dir = state.join("state");
    cfg.portal_siblings = false;
    cfg
}

/// A service token of `client` for `scope`.
async fn token(token_url: &str, client: &str, scope: &str) -> String {
    let resp = Client::new()
        .post(token_url)
        .form(&[
            ("grant_type", "client_credentials"),
            ("client_id", client),
            ("client_secret", SECRET),
            ("scope", scope),
        ])
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    assert_eq!(status, StatusCode::OK, "{client} {scope}: {body}");
    body["access_token"]
        .as_str()
        .expect("access_token")
        .to_owned()
}

/// One call to `module` that only its service caller may make, with `auth` as the
/// `Authorization` header (none when `None`).
async fn call(base: &str, module: Module, auth: Option<&str>) -> StatusCode {
    let client = Client::new();
    let req = match module {
        Module::Shako => client.get(format!("{base}/api/v1/user")),
        Module::Ekiben => client.get(format!("{base}/streams")),
        Module::Daiya => client.get(format!("{base}/api/org")),
        Module::Dataroom => client
            .post(format!("{base}/api/3/action/organization_create"))
            .json(&json!({"name": format!("split-{}", free_port()), "title": "Split"})),
        Module::Antares => client.get(format!("{base}/q/tenants")),
        Module::Kaisatsu
        | Module::Portal
        | Module::Gateway
        | Module::ModelTools
        | Module::Assistant => unreachable!(),
    };
    let req = match auth {
        Some(value) => req.header("authorization", value),
        None => req,
    };
    req.send().await.unwrap().status()
}

#[tokio::test]
async fn a_module_in_another_container_admits_a_kaisatsu_token_for_its_audience_only() {
    let dir = tempfile::tempdir().unwrap();
    let secret_dir = dir.path().join("secrets");
    let import = dir.path().join("realms");
    std::fs::create_dir_all(&secret_dir).unwrap();
    std::fs::create_dir_all(&import).unwrap();
    let secret_file = secret_dir.join("portal-remote");
    std::fs::write(&secret_file, SECRET).unwrap();
    std::fs::write(import.join("split.json"), realm(&secret_file).to_string()).unwrap();

    // The identity container.
    let (id_port, id) = start_on_a_free_port(
        |port| {
            let mut id_cfg = config(
                "kaisatsu",
                port,
                &dir.path().join("a"),
                FileConfig::default(),
            );
            id_cfg.kaisatsu.import_dir = Some(import.clone());
            id_cfg.kaisatsu.import_secrets_dir = Some(secret_dir.clone());
            id_cfg
        },
        "kaisatsu starts",
    )
    .await;
    let realm_base = format!("http://127.0.0.1:{id_port}/id/realms/{REALM}");
    let token_url = format!("{realm_base}/protocol/openid-connect/token");

    // The modules' container, behind the door.
    let (mod_port, modules) = start_on_a_free_port(
        |port| {
            let door = DoorSection {
                issuer: realm_base.clone(),
                jwks_url: format!("{realm_base}/protocol/openid-connect/certs"),
                clients: vec![CLIENT.into()],
            };
            config(
                "shako,ekiben,daiya,dataroom,antares",
                port,
                &dir.path().join("b"),
                FileConfig {
                    door: Some(door),
                    ..FileConfig::default()
                },
            )
        },
        "the modules start behind the door",
    )
    .await;

    let mut wrong_audience_admitted = Vec::new();
    for module in SERVED {
        let base = format!("http://127.0.0.1:{mod_port}/{}", module.slug());
        let own = token(&token_url, CLIENT, module.name()).await;
        let status = call(&base, module, Some(&format!("Bearer {own}"))).await;
        assert!(status.is_success(), "{module}: its own token -> {status}");

        // Red proof per module: a token kaisatsu signed for the same client with another
        // module's audience is not this module's service caller.
        let other_module = SERVED.iter().find(|m| **m != module).unwrap();
        let wrong = token(&token_url, CLIENT, other_module.name()).await;
        let status = call(&base, module, Some(&format!("Bearer {wrong}"))).await;
        if !status.is_client_error() {
            wrong_audience_admitted
                .push(format!("{module}: a token for {other_module} -> {status}"));
        }
        // A client the door does not name, with the right audience.
        let stranger = token(&token_url, OTHER, module.name()).await;
        let status = call(&base, module, Some(&format!("Bearer {stranger}"))).await;
        assert!(
            status.is_client_error(),
            "{module}: a token of {OTHER} -> {status}"
        );
        let status = call(&base, module, None).await;
        assert!(status.is_client_error(), "{module}: no token -> {status}");
    }

    assert!(
        wrong_audience_admitted.is_empty(),
        "{wrong_audience_admitted:#?}"
    );

    // The Portal's side: the forwarder swaps the per-boot credential the Portal holds for a
    // token, and passes any other credential on unchanged.
    let remote = Arc::new(
        Remote::new(
            &RemoteSection {
                token_url: token_url.clone(),
                client_id: CLIENT.into(),
                client_secret_file: None,
                modules: BTreeMap::new(),
            },
            jc_secret::Secret::new(SECRET.into()),
        )
        .unwrap(),
    );
    let local = "per-boot-local-credential";
    for module in SERVED {
        let base = format!("http://127.0.0.1:{mod_port}/{}", module.slug());
        let (listener, router) = remote
            .forward(module, base, jc_secret::Secret::new(local.into()))
            .await
            .unwrap();
        let at = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await });
        let forwarder = format!("http://{at}");
        let sent = match module {
            Module::Daiya => format!(
                "Basic {}",
                base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    format!("portal:{local}")
                )
            ),
            _ => format!("Bearer {local}"),
        };
        let status = call(&forwarder, module, Some(&sent)).await;
        assert!(
            status.is_success(),
            "{module} via the forwarder -> {status}"
        );
        let status = call(&forwarder, module, Some("Bearer not-the-local-one")).await;
        assert!(
            status.is_client_error(),
            "{module}: another credential through the forwarder -> {status}"
        );
        server.abort();
    }

    modules.stop().await.expect("stop");
    id.stop().await.expect("stop");
}
