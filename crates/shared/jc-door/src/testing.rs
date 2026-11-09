//! A signing issuer for tests: an Ed25519 key, its JWKS on a loopback listener, and tokens
//! signed with it for any client, audience and lifetime.

use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

/// One issuer: `issuer` signs, its keys are at `jwks_url`.
pub struct Issuer {
    pub issuer: String,
    pub jwks_url: String,
    kid: String,
    key: EncodingKey,
}

impl Issuer {
    /// A fresh key named `kid`, served on a loopback listener for as long as the runtime runs.
    pub async fn start(issuer: &str, kid: &str) -> Self {
        let pkcs8 =
            Ed25519KeyPair::generate_pkcs8(&aws_lc_rs::rand::SystemRandom::new()).expect("key");
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("key");
        let jwks = json!({"keys": [{
            "kty": "OKP", "crv": "Ed25519", "use": "sig", "alg": "EdDSA", "kid": kid,
            "x": URL_SAFE_NO_PAD.encode(pair.public_key().as_ref()),
        }]});
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let app = axum::Router::new().route(
            "/certs",
            axum::routing::get(move || {
                let jwks = jwks.clone();
                async move { axum::Json(jwks) }
            }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await });
        Self {
            issuer: issuer.to_owned(),
            jwks_url: format!("http://{addr}/certs"),
            kid: kid.to_owned(),
            key: EncodingKey::from_ed_der(pkcs8.as_ref()),
        }
    }

    /// A token of `client` for `audience` that expires `life` seconds from now (negative: in
    /// the past).
    #[must_use]
    pub fn token(&self, client: &str, audience: &str, life: i64) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_secs()
            .cast_signed();
        let mut header = Header::new(Algorithm::EdDSA);
        header.kid = Some(self.kid.clone());
        encode(
            &header,
            &json!({
                "iss": self.issuer, "aud": audience, "azp": client,
                "iat": now, "exp": now + life,
            }),
            &self.key,
        )
        .expect("sign")
    }
}
