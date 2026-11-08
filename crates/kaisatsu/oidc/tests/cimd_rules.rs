#![forbid(unsafe_code)]
//! Client ID Metadata Document rules (W1): the fetched document must name its own URL, live on
//! an allowed host, and carry exact, allowed redirect and logo URIs; the fetch goes through the
//! outbound guard.

use kaisatsu_core::RealmConfig;
use kaisatsu_oidc::cimd::fetch_cimd_client;
use kaisatsu_oidc::guard::OutboundGuard;
use kaisatsu_oidc::{CimdMetadata, validate_and_build_cimd_client};
use serde_json::{Value, json};

const ID: &str = "https://claude.ai/oauth/meta";

fn doc(patch: Value) -> CimdMetadata {
    let mut d = json!({ "client_id": ID, "redirect_uris": ["https://claude.ai/cb"] });
    for (k, v) in patch.as_object().unwrap() {
        d[k] = v.clone();
    }
    serde_json::from_value(d).unwrap()
}

#[test]
fn every_rule_refuses_its_violation() {
    let realm = RealmConfig::default();
    let cases = [
        (
            ID,
            json!({"client_id": "https://claude.ai/other"}),
            "does not match",
        ),
        (
            "not a url",
            json!({"client_id": "not a url"}),
            "not a valid URL",
        ),
        (
            "http://claude.ai/m",
            json!({"client_id": "http://claude.ai/m"}),
            "must use https",
        ),
        (
            "https://evil.example/m",
            json!({"client_id": "https://evil.example/m"}),
            "not permitted by realm policy",
        ),
        (ID, json!({"redirect_uris": []}), "no redirect_uris"),
        (
            ID,
            json!({"redirect_uris": ["https://*.claude.ai/cb"]}),
            "wildcards",
        ),
        (
            ID,
            json!({"redirect_uris": ["::bad"]}),
            "invalid redirect_uri",
        ),
        (
            ID,
            json!({"redirect_uris": ["mailto:a@claude.ai"]}),
            "has no host",
        ),
        (
            ID,
            json!({"redirect_uris": ["http://claude.ai/cb"]}),
            "https or loopback",
        ),
        (
            ID,
            json!({"redirect_uris": ["https://evil.example/cb"]}),
            "not permitted",
        ),
        (ID, json!({"logo_uri": "::bad"}), "invalid logo_uri"),
        (
            ID,
            json!({"logo_uri": "http://claude.ai/l.png"}),
            "logo_uri host",
        ),
        (
            ID,
            json!({"logo_uri": "https://evil.example/l.png"}),
            "logo_uri host",
        ),
    ];
    for (expected, patch, want) in cases {
        let e = validate_and_build_cimd_client(&realm, expected, doc(patch.clone()))
            .unwrap_err()
            .to_string();
        assert!(e.contains(want), "{patch}: {e}");
    }
    let ok = validate_and_build_cimd_client(
        &realm,
        ID,
        doc(json!({
            "redirect_uris": ["http://localhost:8765/cb", "http://127.0.0.1/cb", "https://CLAUDE.ai/cb"],
            "logo_uri": "https://claude.ai/logo.png",
            "client_name": "Claude",
            "client_uri": "https://claude.ai"
        })),
    )
    .unwrap();
    assert!(ok.public_client && ok.consent_required && ok.is_cimd);
    assert!(!ok.full_scope_allowed && !ok.service_accounts_enabled);
    assert_eq!(ok.attributes["pkce.code.challenge.method"], "S256");
    assert_eq!(ok.name.as_deref(), Some("Claude"));
}

#[tokio::test]
async fn fetching_goes_through_the_guard_and_its_allow_list() {
    let realm = RealmConfig::default();
    let guard = OutboundGuard::default();
    guard.insert_mock(
        ID,
        200,
        json!({ "client_id": ID, "redirect_uris": ["https://claude.ai/cb"] })
            .to_string()
            .into_bytes(),
    );
    let c = fetch_cimd_client(&guard, &realm, ID).await.unwrap();
    assert_eq!(c.client_id, ID);
    // An unknown member, a failing upstream and a host outside the allow-list all fail the same.
    guard.insert_mock(
        ID,
        200,
        json!({ "client_id": ID, "redirect_uris": [], "extra": 1 })
            .to_string()
            .into_bytes(),
    );
    guard.insert_mock("https://claude.ai/missing", 404, Vec::new());
    for url in [ID, "https://evil.example/m", "https://claude.ai/missing"] {
        let e = fetch_cimd_client(&guard, &realm, url)
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("Metadata fetch failed"), "{url}: {e}");
    }
}
