//! Edge cases of `Config::from_lookup` and `Config::require_secrets` (AG-35, F-PLT1-008, T-7795).
//!
//! Every setting a deployment can get wrong is refused at start with the variable it names:
//! an address or URL that does not parse, a number that is not one, an Organization realm list
//! that is not a list or names one realm twice, a secret file that cannot be read. A blank
//! client id falls back to its default instead of naming nobody.

use agent_proxy::config::{Config, ConfigError};

fn with(pairs: &[(&'static str, &str)]) -> Result<Config, ConfigError> {
    let pairs: Vec<(&'static str, String)> =
        pairs.iter().map(|(k, v)| (*k, (*v).to_owned())).collect();
    Config::from_lookup(move |key| {
        pairs
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value.clone())
    })
}

/// An address or a URL that does not parse is refused with the variable that holds it.
#[test]
fn an_unparseable_address_or_url_names_its_variable() {
    match with(&[("JC_PROXY_BIND", "not-an-address")]) {
        Err(ConfigError::InvalidAddr { var, .. }) => assert_eq!(var, "JC_PROXY_BIND"),
        other => panic!("{other:?}"),
    }
    for var in [
        "JC_PORTAL_BASE",
        "JC_GATEWAY_BASE",
        "JC_FORGE_BASE",
        "JC_OIDC_ISSUER",
        "JC_MODEL_BASE",
    ] {
        match with(&[(var, "no scheme here")]) {
            Err(ConfigError::InvalidUrl { var: named, reason }) => {
                assert_eq!(named, var);
                assert!(!reason.is_empty());
            }
            other => panic!("{var}: {other:?}"),
        }
    }
}

/// A daily cap or the probe interval that is not a whole number is refused; a blank one is the
/// default.
#[test]
fn a_number_that_is_not_one_is_refused_and_a_blank_one_is_the_default() {
    for var in [
        "JC_DAILY_TOKENS_ASSISTANT",
        "JC_DAILY_TOKENS_APP_BUILDER",
        "JC_DAILY_TOKENS_OTHER",
        "JC_DAILY_TOKENS_PER_PERSON",
        "JC_MODEL_PROBE_SECS",
    ] {
        for bad in ["-1", "ten", "1.5"] {
            match with(&[(var, bad)]) {
                Err(ConfigError::InvalidNumber { var: named, .. }) => assert_eq!(named, var),
                other => panic!("{var}={bad}: {other:?}"),
            }
        }
    }
    let config = with(&[("JC_MODEL_PROBE_SECS", "  "), ("JC_DAILY_TOKENS_OTHER", " 42 ")])
        .expect("a config");
    assert_eq!(config.model_probe_secs, 900);
    assert_eq!(config.daily_caps.other, 42);
}

/// A blank client id falls back to its default; a set one is taken as written.
#[test]
fn a_blank_client_id_is_its_default_and_a_set_one_is_kept() {
    let blank = with(&[
        ("JC_PORTAL_CLIENT_ID", " "),
        ("JC_PROXY_FUNCTIONS_IDENTITY", ""),
        ("JC_PROXY_PORTAL_IDENTITY", "\t"),
        ("JC_ASSISTANT_CLIENT_ID", " "),
    ])
    .expect("a config");
    assert_eq!(blank.portal_client_id, "portal-api");
    assert_eq!(blank.functions_identity, "jc-functions");
    assert_eq!(blank.portal_identity, "portal");
    assert_eq!(blank.assistant_client_id, "jc-assistant");
    let set = with(&[
        ("JC_PORTAL_CLIENT_ID", "portal-bbsk"),
        ("JC_PROXY_FUNCTIONS_IDENTITY", "fns"),
        ("JC_ASSISTANT_CLIENT_ID", "assistant-bbsk"),
    ])
    .expect("a config");
    assert_eq!(set.portal_client_id, "portal-bbsk");
    assert_eq!(set.functions_identity, "fns");
    assert_eq!(set.assistant_client_id, "assistant-bbsk");
}

/// An Organization realm list that is not JSON, names an Organization blank or twice, or a URL
/// that is not http(s), is refused with the variable, and the reason says which.
#[test]
fn an_organization_realm_list_that_breaks_a_rule_is_refused_with_its_reason() {
    let realm = |org: &str, issuer: &str, jwks: &str| {
        format!(r#"{{"organization":"{org}","issuer":"{issuer}","jwks_url":"{jwks}"}}"#)
    };
    let certs = "https://idm.bbsk.sk/realms/bbsk/protocol/openid-connect/certs";
    let other = "https://idm.ba.sk/realms/ba/protocol/openid-connect/certs";
    for (raw, says) in [
        ("{not json".to_owned(), "a JSON list"),
        (
            format!("[{}]", realm(" ", "https://idm.bbsk.sk/realms/bbsk", certs)),
            "empty or listed twice",
        ),
        (
            format!(
                "[{},{}]",
                realm("bbsk", "https://idm.bbsk.sk/realms/bbsk", certs),
                realm("bbsk", "https://idm.ba.sk/realms/ba", other)
            ),
            "empty or listed twice",
        ),
        (
            format!("[{}]", realm("bbsk", "ftp://idm.bbsk.sk/realms/bbsk", certs)),
            "is not an http(s) URL",
        ),
        (
            format!("[{}]", realm("bbsk", "https://idm.bbsk.sk/realms/bbsk", "not a url")),
            "is not an http(s) URL",
        ),
    ] {
        match with(&[("JC_OIDC_ORGANIZATION_REALMS", &raw)]) {
            Err(ConfigError::InvalidValue { var, reason }) => {
                assert_eq!(var, "JC_OIDC_ORGANIZATION_REALMS");
                assert!(reason.contains(says), "{raw}: {reason}");
            }
            other => panic!("{raw}: {other:?}"),
        }
    }
}

/// A secret file that cannot be read is a startup failure naming the file, never an empty
/// secret; one that can is read without its trailing line break.
#[test]
fn a_secret_file_that_cannot_be_read_names_the_file() {
    let missing = std::env::temp_dir().join("agent-proxy-no-such-secret-file");
    let path = missing.display().to_string();
    match with(&[("JC_MODEL_KEY_FILE", &path)]) {
        Err(ConfigError::SecretFile { path: named, .. }) => assert_eq!(named, missing),
        other => panic!("{other:?}"),
    }
    let file = std::env::temp_dir().join(format!("agent-proxy-forge-{}", std::process::id()));
    std::fs::write(&file, "forge-token-from-file\n").expect("the secret file");
    let config = with(&[("JC_FORGE_TOKEN_FILE", &file.display().to_string())]);
    let _ = std::fs::remove_file(&file);
    assert_eq!(config.expect("a config").forge_token, "forge-token-from-file");
}

/// The proxy does not start without the forge token or the model key, and names which.
#[test]
fn a_proxy_without_its_forge_token_or_model_key_names_the_missing_one() {
    let secret = ("JC_OIDC_CLIENT_SECRET", "s");
    let no_forge = with(&[secret, ("JC_MODEL_KEY", "k")]).expect("a config");
    assert!(matches!(no_forge.require_secrets(), Err(ConfigError::Missing("JC_FORGE_TOKEN_FILE"))));
    let no_key = with(&[secret, ("JC_FORGE_TOKEN", "f")]).expect("a config");
    assert!(matches!(no_key.require_secrets(), Err(ConfigError::Missing("JC_MODEL_KEY_FILE"))));
    let whole = with(&[secret, ("JC_FORGE_TOKEN", "f"), ("JC_MODEL_KEY", "k")]).expect("a config");
    assert!(whole.require_secrets().is_ok());
}

/// The process environment is read the same way: this test process sets none of the proxy's
/// variables, so it is the defaults.
#[test]
fn the_process_environment_is_read_like_any_other_lookup() {
    let config = Config::from_env().expect("the defaults");
    assert_eq!(config.bind.port(), 8080);
    assert_eq!(config.portal_base.as_str(), "http://portal:8080/");
}
