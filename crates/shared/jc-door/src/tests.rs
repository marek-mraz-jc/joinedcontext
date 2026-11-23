use super::*;
use crate::testing::Issuer;

async fn verifier(issuer: &Issuer, clients: &[&str]) -> Verifier {
    let clients: Vec<String> = clients.iter().map(|c| (*c).to_owned()).collect();
    Verifier::new(&issuer.issuer, &issuer.jwks_url, &clients).expect("verifier")
}

async fn client_of(v: &Verifier, token: &str, audience: &str) -> Option<String> {
    v.prepare(token).await;
    v.client(token, audience)
}

#[tokio::test]
async fn an_admitted_clients_token_for_the_audience_names_its_client() {
    let issuer = Issuer::start("https://id.a.example/realms/a", "k1").await;
    let v = verifier(&issuer, &["jc-assistant"]).await;
    let token = issuer.token("jc-assistant", "embedder", 300);
    assert_eq!(
        client_of(&v, &token, "embedder").await.as_deref(),
        Some("jc-assistant")
    );
    assert!(v.verify(&token, "embedder"));
}

#[tokio::test]
async fn wrong_audience_expired_unknown_client_and_other_issuer_are_refused() {
    let issuer = Issuer::start("https://id.a.example/realms/a", "k1").await;
    let other = Issuer::start("https://id.b.example/realms/b", "k1").await;
    let v = verifier(&issuer, &["jc-assistant"]).await;
    assert_eq!(
        client_of(&v, &issuer.token("jc-assistant", "shako", 300), "embedder").await,
        None,
        "another audience"
    );
    assert_eq!(
        client_of(
            &v,
            &issuer.token("jc-assistant", "embedder", -120),
            "embedder"
        )
        .await,
        None,
        "expired past the leeway"
    );
    assert_eq!(
        client_of(
            &v,
            &issuer.token("someone-else", "embedder", 300),
            "embedder"
        )
        .await,
        None,
        "a client the door does not name"
    );
    // The same kid signed by another key: the signature check refuses it.
    assert_eq!(
        client_of(
            &v,
            &other.token("jc-assistant", "embedder", 300),
            "embedder"
        )
        .await,
        None,
        "another issuer's key"
    );
    assert_eq!(client_of(&v, "not-a-jwt", "embedder").await, None);
    let none = verifier(&issuer, &[]).await;
    assert_eq!(
        client_of(
            &none,
            &issuer.token("jc-assistant", "embedder", 300),
            "embedder"
        )
        .await,
        None,
        "no client named admits nobody"
    );
}

#[test]
fn bearer_reads_only_a_bearer_value() {
    let mut headers = HeaderMap::new();
    assert_eq!(bearer(&headers), None);
    headers.insert(header::AUTHORIZATION, "Bearer abc".parse().unwrap());
    assert_eq!(bearer(&headers), Some("abc"));
    headers.insert(header::AUTHORIZATION, "Basic abc".parse().unwrap());
    assert_eq!(bearer(&headers), None);
    headers.insert(header::AUTHORIZATION, "Bearer  ".parse().unwrap());
    assert_eq!(bearer(&headers), None);
}

/// A loopback listener answering every request with `status` and `body`, counting the requests.
async fn answering(
    status: u16,
    body: impl Into<String>,
) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let hits = std::sync::Arc::new(AtomicUsize::new(0));
    let body: String = body.into();
    let counted = hits.clone();
    let app = axum::Router::new().fallback(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        let body = body.clone();
        async move { (http::StatusCode::from_u16(status).unwrap(), body) }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    (format!("http://{addr}/"), hits)
}

/// A bound socket that never listens: a connect to its port is refused while it lives.
fn refused() -> tokio::net::TcpSocket {
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    socket
}

fn hits(n: &std::sync::atomic::AtomicUsize) -> usize {
    n.load(std::sync::atomic::Ordering::SeqCst)
}

#[tokio::test]
async fn a_token_without_azp_names_its_client_id_and_trimmed_clients_are_admitted() {
    let issuer = Issuer::start("https://id.a.example/realms/a", "k1").await;
    let v = verifier(&issuer, &[" jc-assistant ", ""]).await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let token = issuer.sign(&serde_json::json!({
        "iss": issuer.issuer, "aud": "embedder", "client_id": "jc-assistant", "exp": now + 300,
    }));
    assert_eq!(
        client_of(&v, &token, "embedder").await.as_deref(),
        Some("jc-assistant")
    );
    let no_exp = issuer.sign(&serde_json::json!({
        "iss": issuer.issuer, "aud": "embedder", "azp": "jc-assistant",
    }));
    assert_eq!(
        client_of(&v, &no_exp, "embedder").await,
        None,
        "exp is required"
    );
    // Within the 30 s leeway an expired token still passes.
    let late = issuer.token("jc-assistant", "embedder", -10);
    assert_eq!(
        client_of(&v, &late, "embedder").await.as_deref(),
        Some("jc-assistant")
    );
    let blank = verifier(&issuer, &["", "  "]).await;
    assert_eq!(
        client_of(&blank, &issuer.token("", "embedder", 300), "embedder").await,
        None,
        "a blank client id is no client"
    );
}

#[tokio::test]
async fn an_hmac_token_and_a_key_published_for_another_algorithm_are_refused() {
    use jsonwebtoken::{EncodingKey, Header, encode};
    let issuer = Issuer::start("https://id.a.example/realms/a", "k1").await;
    let v = verifier(&issuer, &["jc-assistant"]).await;
    v.prepare(&issuer.token("jc-assistant", "embedder", 300))
        .await;
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some("k1".into());
    let hmac = encode(
        &header,
        &serde_json::json!({"iss": issuer.issuer, "aud": "embedder", "azp": "jc-assistant",
            "exp": u64::MAX / 2}),
        &EncodingKey::from_secret(b"guessable"),
    )
    .unwrap();
    assert_eq!(v.client(&hmac, "embedder"), None, "no HMAC");

    // The same Ed25519 key published as an RS256 key verifies no EdDSA token.
    let jwks = reqwest::get(&issuer.jwks_url)
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let (url, _) = answering(200, jwks.replace("\"EdDSA\"", "\"RS256\"")).await;
    let pinned = Verifier::new(&issuer.issuer, &url, &["jc-assistant".to_owned()]).unwrap();
    let token = issuer.token("jc-assistant", "embedder", 300);
    pinned.prepare(&token).await;
    assert_eq!(pinned.client(&token, "embedder"), None);
}

#[tokio::test]
async fn the_jwks_is_fetched_once_per_period_and_a_failed_fetch_keeps_the_old_keys() {
    let issuer = Issuer::start("https://id.a.example/realms/a", "k1").await;
    let jwks = reqwest::get(&issuer.jwks_url)
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let (url, count) = answering(200, jwks).await;
    let v = Verifier::new(&issuer.issuer, &url, &["jc-assistant".to_owned()]).unwrap();
    assert_eq!(v.issuer(), issuer.issuer);
    let debug = format!("{v:?}");
    assert!(
        debug.contains(&issuer.issuer) && debug.contains(&url),
        "{debug}"
    );
    let token = issuer.token("jc-assistant", "embedder", 300);
    v.prepare("not-a-jwt").await;
    assert_eq!(hits(&count), 0, "a token without a kid fetches nothing");
    v.prepare(&token).await;
    assert_eq!(hits(&count), 1);
    v.prepare(&token).await;
    assert_eq!(hits(&count), 1, "a known kid fetches nothing");
    let other = Issuer::start(&issuer.issuer, "k2").await;
    v.prepare(&other.token("jc-assistant", "embedder", 300))
        .await;
    assert_eq!(
        hits(&count),
        1,
        "an unknown kid inside the period fetches nothing"
    );
    assert!(v.verify(&token, "embedder"));

    for (status, body) in [
        (500, String::new()),
        (200, "{".to_owned()),
        (200, "x".repeat(MAX_JWKS_BYTES + 1)),
    ] {
        let (url, count) = answering(status, body).await;
        let v = Verifier::new(&issuer.issuer, &url, &["jc-assistant".to_owned()]).unwrap();
        v.prepare(&token).await;
        assert_eq!(hits(&count), 1);
        assert!(
            !v.verify(&token, "embedder"),
            "{status}: no keys, no client"
        );
    }
    let closed = refused();
    let url = format!("http://{}/", closed.local_addr().unwrap());
    let v = Verifier::new(&issuer.issuer, &url, &["jc-assistant".to_owned()]).unwrap();
    v.prepare(&token).await;
    assert!(
        !v.verify(&token, "embedder"),
        "an unreachable JWKS admits nobody"
    );
}

#[tokio::test]
async fn a_large_jwks_is_read_and_a_redirect_is_not_followed() {
    let issuer = Issuer::start("https://id.a.example/realms/a", "k1").await;
    let token = issuer.token("jc-assistant", "embedder", 300);
    let jwks = reqwest::get(&issuer.jwks_url)
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    // A JWKS of 200 KB (many rotated keys) is still read.
    let padded = jwks.replacen(
        '{',
        &format!("{{\"padding\":\"{}\",", "p".repeat(200_000)),
        1,
    );
    let (url, _) = answering(200, padded).await;
    let v = Verifier::new(&issuer.issuer, &url, &["jc-assistant".to_owned()]).unwrap();
    v.prepare(&token).await;
    assert!(v.verify(&token, "embedder"));

    let target = issuer.jwks_url.clone();
    let app = axum::Router::new().fallback(move || {
        let target = target.clone();
        async move { (http::StatusCode::FOUND, [(header::LOCATION, target)]) }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    let v = Verifier::new(
        &issuer.issuer,
        &format!("http://{addr}/"),
        &["jc-assistant".to_owned()],
    )
    .unwrap();
    v.prepare(&token).await;
    assert!(
        !v.verify(&token, "embedder"),
        "the door follows no redirect to a key set"
    );
}

#[test]
fn bearer_takes_any_case_of_the_scheme_and_refuses_an_oversized_token() {
    let mut headers = HeaderMap::new();
    headers.insert(header::AUTHORIZATION, "bearer  abc ".parse().unwrap());
    assert_eq!(bearer(&headers), Some("abc"));
    let at_cap = "a".repeat(MAX_TOKEN_BYTES);
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {at_cap}").parse().unwrap(),
    );
    assert_eq!(bearer(&headers), Some(at_cap.as_str()));
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {at_cap}a").parse().unwrap(),
    );
    assert_eq!(bearer(&headers), None);
    let large = "a".repeat(60_000);
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {large}").parse().unwrap(),
    );
    assert_eq!(
        bearer(&headers),
        Some(large.as_str()),
        "a 60 KB token is taken"
    );
    headers.insert(header::AUTHORIZATION, "Bearer".parse().unwrap());
    assert_eq!(bearer(&headers), None, "no value");
}

#[tokio::test]
async fn capped_reads_up_to_the_cap_and_refuses_one_byte_more() {
    let (url, _) = answering(200, "x".repeat(10)).await;
    let body = capped(reqwest::get(&url).await.unwrap(), 10).await.unwrap();
    assert_eq!(body.len(), 10);
    let err = capped(reqwest::get(&url).await.unwrap(), 9)
        .await
        .unwrap_err();
    assert_eq!(err, "larger than 9 bytes");

    // A peer that promises 100 bytes and hangs up after 3: the read fails, nothing is kept.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        let (mut sock, _) = listener.accept().unwrap();
        let mut head = [0u8; 1024];
        let _ = sock.read(&mut head);
        let _ = sock.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\nabc");
    });
    let cut = reqwest::get(format!("http://{addr}/")).await.unwrap();
    assert!(capped(cut, 1_000).await.is_err());
}

#[tokio::test]
async fn fetch_token_reuses_a_token_until_30_s_before_it_expires() {
    let http = http_client().unwrap();
    let secret = jc_secret::Secret::new("s3cret".to_owned());
    let fetch = |url: String| {
        let http = http.clone();
        let secret = jc_secret::Secret::new(secret.expose().to_owned());
        async move { fetch_token(&http, &url, "jc-assistant", &secret, "embedder").await }
    };
    let (url, _) = answering(200, r#"{"access_token":"t1","expires_in":300}"#).await;
    let (token, life) = fetch(url).await.unwrap();
    assert_eq!((token.expose(), life), ("t1", Duration::from_secs(270)));
    let (url, _) = answering(200, r#"{"access_token":"t2"}"#).await;
    assert_eq!(
        fetch(url).await.unwrap().1,
        Duration::from_secs(30),
        "60 s by default"
    );
    let (url, _) = answering(200, r#"{"access_token":"t3","expires_in":10}"#).await;
    assert_eq!(
        fetch(url).await.unwrap().1,
        Duration::ZERO,
        "a short token is not reused"
    );

    let (url, _) = answering(401, "denied").await;
    assert_eq!(
        fetch(url).await.unwrap_err(),
        "the token endpoint answered 401 Unauthorized"
    );
    let (url, _) = answering(200, r#"{"token_type":"bearer"}"#).await;
    assert_eq!(
        fetch(url).await.unwrap_err(),
        "the token endpoint answered no token"
    );
    let (url, _) = answering(200, "x".repeat(MAX_TOKEN_BYTES + 1)).await;
    assert!(fetch(url).await.unwrap_err().starts_with("larger than"));
    let closed = refused();
    let err = fetch(format!("http://{}/", closed.local_addr().unwrap()))
        .await
        .unwrap_err();
    assert!(
        err.starts_with("the token endpoint is unreachable"),
        "{err}"
    );
    assert!(!err.contains("s3cret"), "{err}");
}

/// The issuer's Debug names where it signs and serves, never its key; an empty issuer is named
/// by its listener, and a token carries its kid and the claims it was given.
#[test]
fn a_local_issuer_debug_hides_its_key_and_its_tokens_carry_kid_and_claims() {
    let issuer =
        crate::issuer::LocalIssuer::start("https://id.a.example/realms/a", "k7").expect("issuer");
    assert_eq!(issuer.issuer(), "https://id.a.example/realms/a");
    assert!(issuer.jwks_url().starts_with("http://127.0.0.1:"));
    assert!(issuer.jwks_url().ends_with("/certs"));
    let debug = format!("{issuer:?}");
    assert_eq!(
        debug,
        format!(
            "LocalIssuer {{ issuer: \"https://id.a.example/realms/a\", jwks_url: {:?}, .. }}",
            issuer.jwks_url()
        )
    );
    let token = issuer
        .token("portal-publisher", "dataroom", 60)
        .expect("token");
    assert_eq!(
        jsonwebtoken::decode_header(&token)
            .expect("header")
            .kid
            .as_deref(),
        Some("k7")
    );
    let claims: serde_json::Value = serde_json::from_slice(
        &base64::Engine::decode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            token.split('.').nth(1).expect("payload"),
        )
        .expect("base64"),
    )
    .expect("json");
    assert_eq!(claims["iss"], "https://id.a.example/realms/a");
    assert_eq!(claims["azp"], "portal-publisher");
    assert_eq!(claims["aud"], "dataroom");
    assert_eq!(
        claims["exp"].as_i64().expect("exp") - claims["iat"].as_i64().expect("iat"),
        60
    );
}

/// The in-process issuer serves its keys on a thread of its own: a token it signs verifies in a
/// runtime started after the one that created it ended, and only for the client it names.
#[test]
fn a_local_issuer_outlives_the_runtime_that_started_it() {
    let issuer = tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(async { crate::issuer::LocalIssuer::start("", "k1").expect("issuer") });
    assert!(issuer.issuer().starts_with("http://127.0.0.1:"));
    tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(async {
            let verifier = Verifier::new(
                issuer.issuer(),
                issuer.jwks_url(),
                &["portal-publisher".to_owned()],
            )
            .expect("verifier");
            let token = issuer
                .token("portal-publisher", "dataroom", 60)
                .expect("token");
            verifier.prepare(&token).await;
            assert_eq!(
                verifier.client(&token, "dataroom").as_deref(),
                Some("portal-publisher")
            );
            assert!(!verifier.verify(&token, "shako"), "another audience");
            let other = issuer.token("other", "dataroom", 60).expect("token");
            assert!(!verifier.verify(&other, "dataroom"), "another client");
            let expired = issuer
                .token("portal-publisher", "dataroom", -120)
                .expect("token");
            assert!(!verifier.verify(&expired, "dataroom"), "expired");
        });
}
