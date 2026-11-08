//! E1: which process variables a stream reads (`EnvAccess`), through `${VAR}` and `env()`.

use std::collections::HashMap;

use ekiben_core::config::{allow_listed_name, build_blobl_env_for_stream};
use ekiben_core::{redact_config_value, EnvAccess, OrgId, StreamConfig, ENV_VAR_ALLOW_PREFIX};
use serde_json::json;

fn env_of(mapping: &str, vars: &HashMap<String, String>, access: EnvAccess) -> serde_json::Value {
    let env = build_blobl_env_for_stream(vars, access);
    let exec = env.compile_mapping(mapping).expect("mapping");
    let out = exec
        .exec_on_value(ekiben_bloblang::Value::Null)
        .expect("runs");
    out.to_json_value()
}

#[test]
fn allow_listed_reads_only_the_prefixed_process_variables() {
    std::env::set_var("EKIBEN_VAR_E1_ALLOWED_T1", "allowed-value");
    std::env::set_var("E1_SECRET_OF_ANOTHER_MODULE_T1", "must-not-leak");
    let access = EnvAccess::AllowListed;
    assert_eq!(
        access.lookup("E1_ALLOWED_T1").as_deref(),
        Some("allowed-value")
    );
    assert_eq!(
        access.lookup("EKIBEN_VAR_E1_ALLOWED_T1").as_deref(),
        Some("allowed-value")
    );
    assert_eq!(access.lookup("E1_SECRET_OF_ANOTHER_MODULE_T1"), None);
    assert_eq!(
        access.lookup("PATH"),
        None,
        "PATH is not an EKIBEN_VAR_ variable"
    );
    assert_eq!(EnvAccess::None.lookup("EKIBEN_VAR_E1_ALLOWED_T1"), None);
    assert!(EnvAccess::Process.lookup("PATH").is_some());
    assert_eq!(allow_listed_name("X"), format!("{ENV_VAR_ALLOW_PREFIX}X"));
}

#[test]
fn the_organization_map_comes_before_the_process_environment() {
    std::env::set_var("EKIBEN_VAR_E1_BOTH_T2", "from-process");
    let mut org = HashMap::new();
    org.insert("E1_BOTH_T2".to_string(), "from-org".to_string());
    assert_eq!(
        EnvAccess::AllowListed
            .resolve(&org, "E1_BOTH_T2")
            .as_deref(),
        Some("from-org")
    );
    assert_eq!(
        EnvAccess::AllowListed
            .resolve(&HashMap::new(), "E1_BOTH_T2")
            .as_deref(),
        Some("from-process")
    );
    assert_eq!(
        EnvAccess::None.resolve(&org, "E1_BOTH_T2").as_deref(),
        Some("from-org")
    );
}

#[test]
fn bloblang_env_follows_the_same_access() {
    std::env::set_var("EKIBEN_VAR_E1_BLOBL_T3", "ok");
    std::env::set_var("E1_HIDDEN_T3", "hidden");
    let vars = HashMap::from([("ORG_VAR".to_string(), "org-value".to_string())]);
    let out = env_of(
        r#"root.a = env("E1_BLOBL_T3")
root.b = env("E1_HIDDEN_T3")
root.c = env("ORG_VAR")"#,
        &vars,
        EnvAccess::AllowListed,
    );
    assert_eq!(out["a"], "ok");
    assert!(out["b"].is_null(), "{out}");
    assert_eq!(out["c"], "org-value");
    let none = env_of(r#"root.a = env("E1_BLOBL_T3")"#, &vars, EnvAccess::None);
    assert!(none["a"].is_null(), "{none}");
    let all = env_of(r#"root.b = env("E1_HIDDEN_T3")"#, &vars, EnvAccess::Process);
    assert_eq!(all["b"], "hidden");
}

#[test]
fn an_unset_variable_is_a_lint_error_not_the_process_value() {
    std::env::set_var("E1_UNPREFIXED_T4", "process-only");
    let raw =
        "input:\n  generate:\n    mapping: 'root = \"${E1_UNPREFIXED_T4}\"'\noutput:\n  drop: {}\n";
    let err = StreamConfig::parse_str_with_env(raw, false, |k| EnvAccess::AllowListed.lookup(k))
        .expect_err("an unset variable fails the strict parse");
    assert!(err.to_string().contains("E1_UNPREFIXED_T4"), "{err}");
    assert!(!err.to_string().contains("process-only"), "{err}");
    let (_, lints) =
        StreamConfig::parse_str_with_env(raw, true, |k| EnvAccess::AllowListed.lookup(k))
            .expect("a chilled parse goes on");
    assert!(
        lints.iter().any(|l| l.contains("E1_UNPREFIXED_T4")),
        "{lints:?}"
    );
}

#[test]
fn bool_access_keeps_the_old_meaning() {
    assert_eq!(EnvAccess::from(true), EnvAccess::Process);
    assert_eq!(EnvAccess::from(false), EnvAccess::None);
    assert_eq!(EnvAccess::default(), EnvAccess::None);
}

#[test]
fn the_organization_is_read_before_interpolation_and_posted_config_is_kept() {
    let raw = "org: org-a\ninput:\n  http_server:\n    auth:\n      bearer: ${TOKEN}\noutput:\n  drop: {}\n";
    assert_eq!(StreamConfig::peek_org(raw), OrgId::new("org-a"));
    assert_eq!(StreamConfig::peek_org("input: {}"), OrgId::default());
    let posted = StreamConfig::posted_json(raw).expect("parses");
    assert_eq!(posted["input"]["http_server"]["auth"]["bearer"], "${TOKEN}");
}

#[test]
fn a_bearer_field_is_redacted() {
    let red = redact_config_value(&json!({"auth": {"bearer": "tok-e1"}}));
    assert!(!red.to_string().contains("tok-e1"), "{red}");
}

#[test]
fn org_variables_are_replaced_whole_and_never_show_in_debug() {
    let ctx = ekiben_core::OrgContext::new(OrgId::new("o"), ekiben_core::OrgQuotas::default());
    ctx.replace_secrets(HashMap::from([(
        "K".to_string(),
        "secret-e1-value".to_string(),
    )]));
    assert_eq!(ctx.secrets_snapshot()["K"], "secret-e1-value");
    let held = ekiben_core::SecretString::new("secret-e1-value");
    assert!(!format!("{held:?}").contains("secret-e1-value"));
    ctx.replace_secrets(HashMap::new());
    assert!(ctx.secrets_snapshot().is_empty());
}
