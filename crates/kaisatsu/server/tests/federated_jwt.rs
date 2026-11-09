#![forbid(unsafe_code)]
//! A workload's `federated-jwt` login as on prod, where Keycloak's `kubernetes` identity provider
//! reads the API server's JWKS: the pod's ServiceAccount token goes along as bearer, the cluster
//! CA (`ca.crt`) is the only root that fetch trusts, the set is cached and read again for a `kid`
//! it lacks, and the assertion's audience is the realm issuer on the public host, also when the
//! pod calls the internal one. A stub API server answers over TLS with a CA of the test's own.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use jc_router::Mount;
use kaisatsu_core::*;
use kaisatsu_crypto::*;
use kaisatsu_oidc::{ClusterCredentials, OutboundGuard, OutboundPolicy};
use kaisatsu_server::{KaisatsuState, router};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

mod common;
use common::*;

const ISSUER: &str = "https://idm.dev.example.test/realms/dev";
const K8S: &str = "https://kubernetes.default.svc.cluster.local";
const SA: &str = "system:serviceaccount:portal:portal";
const POD_TOKEN: &str = "pod-sa-token-for-the-api-server";

/// The stub API server: the JWKS it serves and the `Authorization` of every request it got.
#[derive(Default)]
struct Stub {
    jwks: Mutex<Value>,
    seen: Mutex<Vec<String>>,
}

/// A CA and a server certificate for `127.0.0.1` it signs, as PEM: (ca, cert, key).
fn cluster_pki() -> (String, String, String) {
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let mut ca = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca_cert = ca.self_signed(&ca_key).unwrap();
    let issuer = rcgen::Issuer::new(ca, ca_key);
    let key = rcgen::KeyPair::generate().unwrap();
    let leaf = rcgen::CertificateParams::new(vec!["127.0.0.1".to_owned()])
        .unwrap()
        .signed_by(&key, &issuer)
        .unwrap();
    (ca_cert.pem(), leaf.pem(), key.serialize_pem())
}

/// Serves `stub` over TLS on a loopback port; returns the JWKS URL.
async fn serve(stub: Arc<Stub>, cert: &str, key: &str) -> String {
    use tokio_rustls::rustls::{self, pki_types::pem::PemObject};
    let certs = rustls::pki_types::CertificateDer::pem_slice_iter(cert.as_bytes())
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    let key = rustls::pki_types::PrivateKeyDer::from_pem_slice(key.as_bytes()).unwrap();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(certs, key)
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let (acceptor, stub) = (acceptor.clone(), stub.clone());
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match tls.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&buf).to_ascii_lowercase();
                let auth = head
                    .lines()
                    .find_map(|l| l.strip_prefix("authorization:"))
                    .map(|v| v.trim().to_owned())
                    .unwrap_or_default();
                stub.seen.lock().unwrap().push(auth);
                let body = stub.jwks.lock().unwrap().to_string();
                let answer = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = tls.write_all(answer.as_bytes()).await;
                let _ = tls.shutdown().await;
            });
        }
    });
    format!("https://127.0.0.1:{port}/openid/v1/jwks")
}

struct Cluster {
    app: axum::Router,
    stub: Arc<Stub>,
    key: SigningKeyRecord,
    kek: Kek,
    guard: OutboundGuard,
    jwks_url: String,
    dir: PathBuf,
}

impl Drop for Cluster {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn jwks_of(keys: &[&SigningKeyRecord]) -> Value {
    let keys: Vec<Value> = keys
        .iter()
        .map(|k| serde_json::from_str(&k.public_jwk_json).unwrap())
        .collect();
    json!({ "keys": keys })
}

/// The dev realm with its `kubernetes` provider reading the stub; `ca` writes the cluster CA
/// into the pod's credentials, `jwks_url` overrides the provider's JWKS URL.
async fn cluster(ca: bool, jwks_url: Option<&str>, min_refetch: Duration) -> Cluster {
    let (ca_pem, cert, key_pem) = cluster_pki();
    let stub = Arc::new(Stub::default());
    let url = serve(stub.clone(), &cert, &key_pem).await;
    let dir = std::env::temp_dir().join(format!("kaisatsu-sa-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("token"), format!("{POD_TOKEN}\n")).unwrap();
    if ca {
        std::fs::write(dir.join("ca.crt"), &ca_pem).unwrap();
    }
    let kek = Kek::generate_for_tests();
    let key = KeyManager::generate_es256("k8s", &kek, 100, "ACTIVE").unwrap();
    *stub.jwks.lock().unwrap() = jwks_of(&[&key]);

    let (_, store, store_kek) = setup_dev_realm_state().await;
    let mut idp = store.get_idp("dev", "kubernetes").await.unwrap().unwrap();
    idp.config.insert("issuer".into(), K8S.into());
    idp.config
        .insert("jwksUrl".into(), jwks_url.unwrap_or(&url).to_owned());
    store.upsert_idp(idp).await.unwrap();
    let guard = OutboundGuard::new(OutboundPolicy {
        in_cluster_hosts: vec!["127.0.0.1".into()],
        cluster: ClusterCredentials {
            token: dir.join("token"),
            ca: dir.join("ca.crt"),
            jwks_min_refetch: min_refetch,
            ..ClusterCredentials::default()
        },
        ..OutboundPolicy::default()
    });
    let state = KaisatsuState::new(store, store_kek, guard.clone(), Arc::new(LocalAuthorizer));
    let app = router(
        state,
        Mount::new("kaisatsu", "https", "idm.dev.example.test", "").unwrap(),
    );
    Cluster {
        app,
        stub,
        key,
        kek,
        guard,
        jwks_url: url,
        dir,
    }
}

impl Cluster {
    fn assertion(&self, key: &SigningKeyRecord, claims: Value) -> String {
        sign_jwt(key, &self.kek, Some("JWT"), &claims).unwrap()
    }

    fn claims(aud: &str) -> Value {
        let now = chrono::Utc::now().timestamp();
        json!({ "iss": K8S, "sub": SA, "aud": aud, "iat": now, "exp": now + 600 })
    }

    /// A `client_credentials` login with `assertion`, on the host `host` (`None`: the realm's).
    async fn login(&self, assertion: &str, host: Option<&str>) -> (StatusCode, String) {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs([
                ("grant_type", "client_credentials"),
                (
                    "client_assertion_type",
                    "urn:ietf:params:oauth:client-assertion-type:jwt-bearer",
                ),
                ("client_assertion", assertion),
            ])
            .finish();
        let mut req = Request::builder()
            .method(Method::POST)
            .uri("/realms/dev/protocol/openid-connect/token")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        if let Some(host) = host {
            req = req.extension(Mount::new("kaisatsu", "http", host, "").unwrap());
        }
        let resp = self
            .app
            .clone()
            .oneshot(req.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    fn fetches(&self) -> usize {
        self.stub.seen.lock().unwrap().len()
    }
}

#[tokio::test]
async fn a_pod_logs_in_with_its_projected_token_as_on_prod() {
    let c = cluster(true, None, Duration::ZERO).await;
    // Prod's pods mint for the public issuer; the pod calls jc's internal host.
    let (st, body) = c
        .login(
            &c.assertion(&c.key, Cluster::claims(ISSUER)),
            Some("jc.jc.svc.cluster.local:8080"),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let token: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        decode_jwt_payload(token["access_token"].as_str().unwrap())["azp"],
        "portal-api"
    );
    // On the public host too, and the second login reads no JWKS: the set is cached.
    let (st, body) = c
        .login(&c.assertion(&c.key, Cluster::claims(ISSUER)), None)
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(c.fetches(), 1);
    // The API server got the pod's own ServiceAccount token.
    assert_eq!(
        c.stub.seen.lock().unwrap()[0],
        format!("bearer {POD_TOKEN}")
    );
    // The cluster CA is trusted for this fetch only: any other outbound read refuses the stub.
    assert!(
        c.guard
            .get_json::<Value>(&c.jwks_url, None, None)
            .await
            .is_err()
    );

    // A rotated key: a token naming a kid the cached set lacks reads the set again.
    let kek = &c.kek;
    let rotated = KeyManager::generate_es256("k8s-next", kek, 100, "ACTIVE").unwrap();
    *c.stub.jwks.lock().unwrap() = jwks_of(&[&c.key, &rotated]);
    let (st, body) = c
        .login(&c.assertion(&rotated, Cluster::claims(ISSUER)), None)
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(c.fetches(), 2);
}

#[tokio::test]
async fn a_cluster_ca_that_is_not_mounted_trusts_nothing_of_the_cluster() {
    let c = cluster(false, None, Duration::ZERO).await;
    let (st, _) = c
        .login(&c.assertion(&c.key, Cluster::claims(ISSUER)), None)
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert_eq!(
        c.fetches(),
        0,
        "the stub's certificate was trusted without the cluster CA"
    );
}

#[tokio::test]
async fn wrong_assertions_and_jwks_urls_are_refused() {
    // The default window: a token's unknown kid reads no JWKS within 10 s of the last read.
    let c = cluster(true, None, ClusterCredentials::default().jwks_min_refetch).await;
    let now = chrono::Utc::now().timestamp();
    let other = KeyManager::generate_es256("stranger", &c.kek, 100, "ACTIVE").unwrap();
    let refused = [
        ("wrong iss", c.assertion(&c.key, json!({ "iss": "https://other.example", "sub": SA, "aud": ISSUER, "iat": now, "exp": now + 600 }))),
        ("wrong aud", c.assertion(&c.key, Cluster::claims("https://idm.dev.example.test/realms/other"))),
        ("the API server's aud", c.assertion(&c.key, Cluster::claims(K8S))),
        ("the internal host is no audience", c.assertion(&c.key, Cluster::claims("http://jc.jc.svc.cluster.local:8080/realms/other"))),
        ("expired", c.assertion(&c.key, json!({ "iss": K8S, "sub": SA, "aud": ISSUER, "iat": now - 900, "exp": now - 600 }))),
        ("unknown kid", c.assertion(&other, Cluster::claims(ISSUER))),
    ];
    for (what, assertion) in &refused {
        let (st, body) = c
            .login(assertion, Some("jc.jc.svc.cluster.local:8080"))
            .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{what}: {body}");
    }
    // Every refusal above answered from one read: made-up kids read no JWKS within the window.
    for _ in 0..3 {
        let (st, _) = c
            .login(&c.assertion(&other, Cluster::claims(ISSUER)), None)
            .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(c.fetches(), 1, "made-up kids each read the JWKS");

    // A provider whose JWKS URL is outside the in-cluster allow-list is refused before a request:
    // the pod's token never leaves the cluster.
    let outside = cluster(
        true,
        Some("https://jwks.example.com/openid/v1/jwks"),
        Duration::ZERO,
    )
    .await;
    let (st, _) = outside
        .login(
            &outside.assertion(&outside.key, Cluster::claims(ISSUER)),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert_eq!(outside.fetches(), 0);
}
