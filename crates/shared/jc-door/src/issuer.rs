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
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).map_err(|e| format!("no key: {e}"))?;
        let pair =
            Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).map_err(|e| format!("no key: {e}"))?;
        let jwks = json!({"keys": [{
            "kty": "OKP", "crv": "Ed25519", "use": "sig", "alg": "EdDSA", "kid": kid,
            "x": URL_SAFE_NO_PAD.encode(pair.public_key().as_ref()),
        }]});
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .and_then(|l| l.set_nonblocking(true).map(|()| l))
            .map_err(|e| format!("no loopback listener for its keys: {e}"))?;
        let addr = listener
            .local_addr()
            .map_err(|e| format!("no loopback listener for its keys: {e}"))?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .build()
            .map_err(|e| format!("no runtime for its keys: {e}"))?;
        let app = axum::Router::new().route(
            "/certs",
            axum::routing::get(move || {
                let jwks = jwks.clone();
                async move { axum::Json(jwks) }
            }),
        );
        std::thread::Builder::new()
            .name("jc-door-issuer".into())
            .spawn(move || {
                runtime.block_on(async move {
                    match tokio::net::TcpListener::from_std(listener) {
                        Ok(listener) => {
                            if let Err(e) = axum::serve(listener, app).await {
                                tracing::error!(error = %e, "jc-door issuer: its keys are no longer served");
                            }
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "jc-door issuer: no listener for its keys");
                        }
                    }
                });
            })
            .map_err(|e| format!("no thread for its keys: {e}"))?;
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
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| format!("clock before 1970: {e}"))?
            .as_secs()
            .cast_signed();
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
