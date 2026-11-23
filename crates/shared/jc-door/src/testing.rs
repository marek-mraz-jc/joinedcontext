//! A signing issuer for tests: [`crate::issuer::LocalIssuer`] with its fields at hand.

use crate::issuer::LocalIssuer;

/// One issuer: `issuer` signs, its keys are at `jwks_url`.
pub struct Issuer {
    pub issuer: String,
    pub jwks_url: String,
    inner: LocalIssuer,
}

impl Issuer {
    /// A fresh key named `kid`, served on a loopback listener for as long as the runtime runs.
    pub async fn start(issuer: &str, kid: &str) -> Self {
        let inner = LocalIssuer::start(issuer, kid).expect("issuer");
        Self {
            issuer: inner.issuer().to_owned(),
            jwks_url: inner.jwks_url().to_owned(),
            inner,
        }
    }

    /// A token of `client` for `audience` that expires `life` seconds from now (negative: in
    /// the past).
    #[must_use]
    pub fn token(&self, client: &str, audience: &str, life: i64) -> String {
        self.inner.token(client, audience, life).expect("sign")
    }
}
