//! A deployment's kaisatsu for the client: a token endpoint handing out tokens of [`Issuer`].

use jc_door::testing::Issuer;
use std::sync::Arc;

/// Starts a token endpoint that answers any client-credentials request with a token of
/// `client` for `audience`; its URL.
pub async fn token_endpoint(
    issuer: Arc<Issuer>,
    client: &'static str,
    audience: &'static str,
) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let app = axum::Router::new().route(
        "/token",
        axum::routing::post(move || {
            let issuer = Arc::clone(&issuer);
            async move {
                axum::Json(serde_json::json!({
                    "access_token": issuer.token(client, audience, 300),
                    "token_type": "Bearer",
                    "expires_in": 300,
                }))
            }
        }),
    );
    tokio::spawn(async move { axum::serve(listener, app).await });
    format!("http://{addr}/token")
}

/// Serves `app` on loopback; its base URL.
pub async fn serve(app: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, app).await });
    format!("http://{addr}")
}
