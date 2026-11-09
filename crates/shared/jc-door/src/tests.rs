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
