//! An issuer of service tokens held in one process: a fresh Ed25519 key per boot, its public
//! JWKS on a loopback listener, tokens signed in process. jc's all-in-one Portal publishes into
//! the dataroom beside it with these, so both sides of the call check a signed, short-lived
//! token through [`crate::Verifier`] as a role split does with kaisatsu's. The private key never
//! leaves memory; the listener answers `/certs` only.

use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

/// One issuer: it signs as `issuer`, its keys are at `jwks_url`.
pub struct LocalIssuer {
    issuer: String,
    jwks_url: String,
    kid: String,
    key: EncodingKey,
}

impl std::fmt::Debug for LocalIssuer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalIssuer")
            .field("issuer", &self.issuer)
            .field("jwks_url", &self.jwks_url)
            .finish_non_exhaustive()
    }
}

impl LocalIssuer {
    /// A fresh key named `kid`, its JWKS served on a free loopback port by a thread of its own
    /// for the life of the process (no caller's runtime holds it); `issuer` empty names the
    /// issuer by that listener's address.
    pub fn start(issuer: &str, kid: &str) -> Result<Self, String> {
        let rng = aws_lc_rs::rand::SystemRandom::new();
        let Some((pair, pkcs8)) = Ed25519KeyPair::generate_pkcs8(&rng)
            .ok()
            .and_then(|pkcs8| Some((Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).ok()?, pkcs8)))
        else {
            return Err("no Ed25519 key from the platform's random source".to_owned());
        };
        let jwks = json!({"keys": [{
            "kty": "OKP", "crv": "Ed25519", "use": "sig", "alg": "EdDSA", "kid": kid,
            "x": URL_SAFE_NO_PAD.encode(pair.public_key().as_ref()),
        }]});
        let addr = match serve(jwks) {
            Ok(addr) => addr,
            Err(e) => return Err(format!("its keys cannot be served: {e}")),
        };
        let base = format!("http://{addr}");
        Ok(Self {
            issuer: if issuer.is_empty() {
                base.clone()
            } else {
                issuer.to_owned()
            },
            jwks_url: format!("{base}/certs"),
            kid: kid.to_owned(),
            key: EncodingKey::from_ed_der(pkcs8.as_ref()),
        })
    }

    /// The `iss` its tokens carry.
    #[must_use]
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// Where its public keys are read.
    #[must_use]
    pub fn jwks_url(&self) -> &str {
        &self.jwks_url
    }

    /// A token of `client` for `audience` that expires `life` seconds from now (negative: in
    /// the past).
    pub fn token(&self, client: &str, audience: &str, life: i64) -> Result<String, String> {
        let Ok(since) = SystemTime::now().duration_since(UNIX_EPOCH) else {
            return Err("the clock is before 1970".to_owned());
        };
        let now = since.as_secs().cast_signed();
        self.sign(&json!({
            "iss": self.issuer, "aud": audience, "azp": client,
            "iat": now, "exp": now + life,
        }))
    }

    /// `claims` signed with this issuer's key under its `kid`.
    pub(crate) fn sign(&self, claims: &serde_json::Value) -> Result<String, String> {
        let mut header = Header::new(Algorithm::EdDSA);
        header.kid = Some(self.kid.clone());
        encode(&header, claims, &self.key).map_err(|e| format!("not signed: {e}"))
    }
}

/// `jwks` on `/certs` of a free loopback port, served by a thread of its own with its own runtime
/// for the life of the process; the address it listens on. Only a platform without sockets,
/// threads or an event loop fails here.
fn serve(jwks: serde_json::Value) -> std::io::Result<std::net::SocketAddr> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()?;
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let addr = listener.local_addr()?;
    // Registered with the issuer's own runtime here, so a caller inside another runtime can start one.
    let listener = {
        let _entered = runtime.enter();
        tokio::net::TcpListener::from_std(listener)?
    };
    let app = axum::Router::new().route(
        "/certs",
        axum::routing::get(move || {
            let jwks = jwks.clone();
            async move { axum::Json(jwks) }
        }),
    );
    std::thread::Builder::new()
        .name("jc-door-issuer".into())
        // axum::serve never completes: it retries a failed accept itself.
        .spawn(move || runtime.block_on(async move { axum::serve(listener, app).await }))?;
    Ok(addr)
}
