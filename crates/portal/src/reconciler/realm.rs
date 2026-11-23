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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// A realm on loopback answering `status` and `body` at the token endpoint of realm `r`; the
    /// forms it was sent.
    async fn realm(
        status: StatusCode,
        body: &'static str,
    ) -> (String, Arc<Mutex<Vec<HashMap<String, String>>>>) {
        let forms = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&forms);
        let app = axum::Router::new().route(
            "/realms/r/protocol/openid-connect/token",
            axum::routing::post(
                move |axum::Form(form): axum::Form<HashMap<String, String>>| {
                    let seen = Arc::clone(&seen);
                    async move {
                        seen.lock().expect("lock").push(form);
                        (status, body)
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move { axum::serve(listener, app).await });
        (format!("http://{addr}/realms/r"), forms)
    }

    #[tokio::test]
    async fn the_token_is_minted_with_the_clients_own_secret() {
        let (issuer, forms) =
            realm(StatusCode::OK, r#"{"access_token":"t-1","expires_in":60}"#).await;
        let token = token(
            &reqwest::Client::new(),
            &issuer,
            "jc-portal",
            &ClientAuth::Secret("s3cr3t".into()),
        )
        .await
        .expect("token");
        assert_eq!(token, "t-1");
        let forms = forms.lock().expect("lock");
        assert_eq!(forms.len(), 1);
        assert_eq!(forms[0]["grant_type"], "client_credentials");
        assert_eq!(forms[0]["client_id"], "jc-portal");
        assert_eq!(forms[0]["client_secret"], "s3cr3t");
    }

    #[tokio::test]
    async fn a_refusal_names_the_status_and_never_the_realms_body() {
        let (issuer, _) = realm(
            StatusCode::UNAUTHORIZED,
            r#"{"error":"unauthorized_client","error_description":"jc-portal secret s3cr3t"}"#,
        )
        .await;
        let err = token(
            &reqwest::Client::new(),
            &issuer,
            "jc-portal",
            &ClientAuth::Secret("s3cr3t".into()),
        )
        .await
        .expect_err("refused");
        assert_eq!(
            err,
            "the realm refused the reconciler's client: 401 Unauthorized"
        );
    }

    #[tokio::test]
    async fn an_answer_without_a_token_or_no_realm_at_all_is_an_error() {
        let (issuer, _) = realm(StatusCode::OK, r#"{"token_type":"Bearer"}"#).await;
        let auth = ClientAuth::Secret("s".into());
        let http = reqwest::Client::new();
        assert!(token(&http, &issuer, "jc-portal", &auth).await.is_err());
        // A port held bound but not listening: the connect is refused.
        let closed = tokio::net::TcpSocket::new_v4().expect("socket");
        closed
            .bind("127.0.0.1:0".parse().expect("addr"))
            .expect("bind");
        let issuer = format!("http://{}/realms/r", closed.local_addr().expect("addr"));
        assert!(token(&http, &issuer, "jc-portal", &auth).await.is_err());
    }

    #[tokio::test]
    async fn an_unreadable_client_token_sends_no_request() {
        let (issuer, forms) = realm(StatusCode::OK, r#"{"access_token":"t"}"#).await;
        let missing = std::env::temp_dir().join("jc-portal-realm-test-no-such-token");
        let err = token(
            &reqwest::Client::new(),
            &issuer,
            "jc-portal",
            &ClientAuth::Assertion(missing),
        )
        .await
        .expect_err("no token file");
        assert!(err.contains("no-such-token"), "{err}");
        assert!(forms.lock().expect("lock").is_empty());
    }
}
