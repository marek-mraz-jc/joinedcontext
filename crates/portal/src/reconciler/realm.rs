//! The Portal's own service-account token, minted once per call that needs one (CC-04, SP-06).
//!
//! Every write the Portal makes to a space surface — a declared `Subscription`, a seed entity
//! put back after drift — is made as this Portal's own client, so the space's Policy decides it
//! like any other client's. The token is never stored, never logged and never leaves the call
//! it was minted for.

use crate::config::ClientAuth;
#[derive(serde::Deserialize)]
struct RealmToken {
    access_token: String,
}

/// A client-credentials token of `client_id` at `issuer`.
///
/// `issuer` is the realm URL, `https://host/realms/{realm}`, without a trailing slash.
pub async fn token(
    http: &reqwest::Client,
    issuer: &str,
    client_id: &str,
    auth: &ClientAuth,
) -> Result<String, String> {
    let response = crate::auth::client_auth::paced(|| async {
        let form = auth
            .form(client_id, &[("grant_type", "client_credentials")])
            .await?;
        http.post(format!("{issuer}/protocol/openid-connect/token"))
            .form(&form)
            .send()
            .await
            .map_err(|err| err.to_string())
    })
    .await?;
    if !response.status().is_success() {
        // The status and nothing else: a realm's error body repeats the client id, and a
        // failed token call is a configuration problem, not something to quote back.
        return Err(format!(
            "the realm refused the reconciler's client: {}",
            response.status()
        ));
    }
    let token: RealmToken = response.json().await.map_err(|err| err.to_string())?;
    Ok(token.access_token)
}
