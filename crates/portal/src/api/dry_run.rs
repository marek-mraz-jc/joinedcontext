use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::change::Lane;
use crate::error::ApiError;
use crate::ops::verdict::{Finding, Level, Verdict};
use crate::plan::PlanDiff;

#[derive(Debug, Default, Deserialize)]
pub struct DryRunQuery {
    #[serde(default, rename = "dryRun")]
    pub dry_run: Option<String>,
    /// `validate_only=true` (AIP-163) is `dryRun=All` under the name a connector's
    /// `plan_change` and other API styles use: every check, nothing saved, the diff (T-5949).
    #[serde(default)]
    pub validate_only: Option<bool>,
    /// Write into this workspace's branch instead of opening a Change (API/01 §22, CC-76).
    #[serde(default)]
    pub workspace: Option<String>,
    /// The resource's name typed back: an administrator's own red-lane change is approved as it
    /// is proposed only with it (PF-58, CC-39).
    #[serde(default)]
    pub confirm: Option<String>,
}

/// Evaluates the `?dryRun` query parameter.
///
/// `None` evaluates to `false` (mutations execute normally).
/// `Some("All")` evaluates to `true` (dry run: plan is computed without mutating state).
/// Any other value is rejected with 400 Bad Request naming `All` as the only accepted value.
pub fn is_dry_run(q: &DryRunQuery) -> Result<bool, ApiError> {
    match q.dry_run.as_deref() {
        None => Ok(q.validate_only == Some(true)),
        Some("All") => Ok(true),
        Some(other) => Err(ApiError::BadRequest(format!(
            "invalid dryRun value '{other}'; 'All' is the only accepted value"
        ))),
    }
}

/// Reviewer-facing validation and diff result returned on `?dryRun=All` (MF-13, R17).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DryRunResult {
    pub valid: bool,
    pub lane: Lane,
    pub plan: PlanDiff,
    /// Whether applying this makes the runner restart the pipeline's stream, so a person sees
    /// it before approving rather than afterwards (T-1056, PL-45). Absent for every other kind.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub restarts_stream: bool,
    /// What one fetch of an `http` DataSource returned (MF-39); absent for every other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe: Option<Probe>,
    /// The verdict this check recorded for the manifest, which its proposal needs (PF-57, T-0956).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<crate::ops::verdict::Verdict>,
    /// Values the manifest writes out where the loader renders them (CC-83): each names the
    /// path and what to write instead. Nothing here blocks the proposal; a copy of the
    /// manifest into another organization would carry the literal with it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<String>,
    /// The references that resolve once another open change is approved (MF-48): the verdict
    /// this check records says so, and the proposal records the changes in its merge request.
    #[serde(skip)]
    pub awaited: Vec<crate::references::Awaited>,
    /// What leaves with a removed resource, with how many (T-3247): a space's entities, a
    /// pipeline's refused records and runs, an App's endpoint and policies, the bindings a
    /// Group's removal edits. Empty for every write that is not a removal.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub goes_with: Vec<Consequence>,
}

/// One thing that leaves with a removed resource (T-3247).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Consequence {
    /// `entities`, `rejectedRecords`, `runs`, `grants` or `bindings`.
    pub what: String,
    pub count: u64,
    /// The resources it names, as `Kind/name`, when they are resources.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
}

/**
One shape for one outcome (T-2234; UI-23, PF-57, MF-13): a check that rejects a manifest answers the
same red verdict every other check answers, with the reasons as its findings, instead of an error the
caller has to read in a second way. A `DataSource` has always answered like this
(`ops::datasource_verdict`); every other kind answered a 4xx whose detail the form put beside the
buttons, so the same mistake met a person on two different pages and the strict gate had to
understand both.

Only the manifest's own faults become a verdict. A refusal of the caller, a project that is not
there, a broken forge or anything else is not a judgement about the manifest and stays an error —
`None` says so, and the caller propagates what it had.
*/
pub fn refused_check(err: &ApiError, manifest: &serde_json::Value) -> Option<DryRunResult> {
    let findings = match err {
        // One entry per violation (CC-24). A producer that names the path of the violation puts it
        // there (`references.rs`: `errors: vec![reference.path]`), which is what makes a finding
        // actionable; one that wrote a sentence instead keeps it as the message.
        ApiError::Invalid { detail, errors } | ApiError::Unprocessable { detail, errors }
            if !errors.is_empty() =>
        {
            errors
                .iter()
                .map(|entry| {
                    if let Some((path, message)) = led_by_its_member(entry) {
                        Finding {
                            level: Level::Error,
                            path: path.to_owned(),
                            message: message.to_owned(),
                        }
                    } else if entry.contains(' ') {
                        Finding {
                            level: Level::Error,
                            path: String::new(),
                            message: entry.clone(),
                        }
                    } else {
                        Finding {
                            level: Level::Error,
                            path: entry.clone(),
                            message: detail.clone(),
                        }
                    }
                })
                .collect()
        }
        ApiError::Invalid { detail, .. }
        | ApiError::Unprocessable { detail, .. }
        | ApiError::BadRequest(detail) => vec![Finding {
            level: Level::Error,
            path: String::new(),
            message: detail.clone(),
        }],
        _ => return None,
    };
    Some(DryRunResult {
        valid: false,
        // Nothing may pass, so the lane says the strictest thing rather than the greenest: there is
        // no change to classify, because the manifest never reached the diff.
        lane: Lane::Red,
        plan: PlanDiff::empty(),
        restarts_stream: false,
        probe: None,
        verdict: Some(Verdict::new(false, findings, None, manifest)),
        findings: Vec::new(),
        awaited: Vec::new(),
        goes_with: Vec::new(),
    })
}

/// A sentence that starts with the member it is about, `spec.outputs.0.targetEndpoint: …`, as that
/// path and the sentence after it, so the form marks the field (UI-45, T-5040).
fn led_by_its_member(entry: &str) -> Option<(&str, &str)> {
    let (path, message) = entry.split_once(": ")?;
    let member = path
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    (member && (path.starts_with("spec.") || path.starts_with("metadata.")))
        .then_some((path, message))
}

/// Every string under `value` that writes `org_domain` out, as a finding naming its JSON
/// path and `{orgDomain}` as the replacement (CC-74, CC-83).
pub fn literal_domain_findings(value: &serde_json::Value, org_domain: &str) -> Vec<String> {
    fn walk(value: &serde_json::Value, path: &str, domain: &str, found: &mut Vec<String>) {
        match value {
            serde_json::Value::String(text) if domain.contains('.') && writes_out(text, domain) => {
                found.push(format!(
                    "{path} writes the organization's domain out: `{text}`; write `{}` for \
                     `{domain}`, so a copy renders the organization it lands in (CC-83)",
                    text.replace(domain, "{orgDomain}")
                ));
            }
            serde_json::Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    walk(item, &format!("{path}[{index}]"), domain, found);
                }
            }
            serde_json::Value::Object(map) => {
                for (key, item) in map {
                    walk(item, &format!("{path}.{key}"), domain, found);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    walk(value, "spec", org_domain, &mut found);
    found
}

/// Whether `text` names `domain` as a host or a subdomain's end, not as the tail of a longer
/// label: `mesto.banskabystrica.sk` does, `notbanskabystrica.sk` does not (T-1484).
fn writes_out(text: &str, domain: &str) -> bool {
    let label = |c: char| c.is_ascii_alphanumeric() || c == '-';
    text.match_indices(domain).any(|(at, _)| {
        !text[..at].chars().next_back().is_some_and(label)
            && !text[at + domain.len()..].chars().next().is_some_and(label)
    })
}

/// One fetch of a DataSource on the project's runner, or why there was none (MF-39).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    /// Messages after the format split; one for a JSON document, one per element of an array.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub records: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<usize>,
    /// The first record as data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<serde_json::Value>,
    /// Why the source was not fetched: a credential, no runner, a feed that did not answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

impl Probe {
    pub fn skipped(reason: impl Into<String>) -> Self {
        Self {
            skipped: Some(reason.into()),
            ..Self::default()
        }
    }
}

/// Executes a dry run for a candidate manifest (MF-13, R17).
pub async fn execute_dry_run(
    identity: &crate::auth::session::Identity,
    state: &crate::state::AppState,
    project: &str,
    manifest: serde_json::Value,
) -> Result<DryRunResult, ApiError> {
    let kind = manifest
        .get("kind")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("manifest must specify 'kind'".to_string()))?;
    let kind_info = crate::resource::by_kind(kind)
        .ok_or_else(|| ApiError::NotFound(format!("kind '{kind}' is unknown")))?;

    let name = manifest
        .pointer("/metadata/name")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let op = match name
        .as_deref()
        .and_then(|n| state.mirror.get(project, kind, n))
    {
        Some(_) => crate::change::Operation::Update,
        None => crate::change::Operation::Create,
    };

    let user = crate::auth::CurrentUser(crate::auth::Session {
        identity: identity.clone(),
        expires_at: i64::MAX,
        issued_at: 0,
        signed_in_at: None,
        id_token: String::new(),
        access_expires_at: i64::MAX,
        refresh_token: None,
    });

    let response = crate::api::mutate::propose(
        &user,
        state,
        project,
        kind_info.plural,
        name.as_deref(),
        op,
        true,
        manifest,
    )
    .await?;

    let bytes = axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .map_err(|e| ApiError::Internal(format!("failed to read dry-run response: {e}")))?;
    serde_json::from_slice::<DryRunResult>(&bytes)
        .map_err(|e| ApiError::Internal(format!("failed to deserialize dry-run result: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-5040, UI-45: a refusal that starts with the member it is about becomes a finding on that
    /// path, so the form marks the field; a plain sentence stays a finding without one.
    #[test]
    fn a_refusal_led_by_its_member_is_a_finding_on_that_field() {
        let err = ApiError::Unprocessable {
            detail: String::new(),
            errors: vec![
                "spec.outputs.0.targetEndpoint: Pipeline stations output 1 writes as pl-stations"
                    .into(),
                "Policy sensors-write grants a write: name it in a policyRef".into(),
            ],
        };
        let refused = refused_check(&err, &serde_json::json!({})).expect("a red verdict");
        let findings = refused.verdict.expect("a verdict").findings;
        assert_eq!(findings[0].path, "spec.outputs.0.targetEndpoint");
        assert_eq!(
            findings[0].message,
            "Pipeline stations output 1 writes as pl-stations"
        );
        assert_eq!(findings[1].path, "");
        assert_eq!(
            findings[1].message,
            "Policy sensors-write grants a write: name it in a policyRef"
        );
    }

    #[tokio::test]
    async fn execute_dry_run_plans_sandbox_space() {
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        let identity = crate::auth::session::Identity {
            client: None,
            subject: "sub-123".into(),
            username: "demo.developer".into(),
            email: Some("demo@example.com".into()),
            name: Some("Demo Developer".into()),
            roles: vec![],
            groups: vec!["portal-approver".into()],
            second_factor: false,
        };
        let payload = serde_json::json!({
            "apiVersion": crate::resource::API_VERSION,
            "kind": "ContextSpace",
            "metadata": {
                "name": "mobility",
                "namespace": "ovzdusie"
            },
            "spec": {
                "isSandbox": true
            }
        });
        let res = execute_dry_run(&identity, &state, "ovzdusie", payload)
            .await
            .expect("dry run executes");
        assert!(res.valid);
        assert_eq!(res.lane, Lane::Green);
    }

    /// CC-83, T-1484: every string that writes the organization's domain out is found, at any
    /// depth, named by its path; a longer host that only ends like it is not.
    #[test]
    fn literal_domain_findings_names_each_string_that_writes_the_domain_out() {
        let domain = "banskabystrica.sk";
        let spec = serde_json::json!({
            "owner": "jana@banskabystrica.sk",
            "sources": [["https://mesto.banskabystrica.sk/feed"], ["{orgDomain}"]],
            "nested": { "deep": [{ "host": "banskabystrica.sk" }] },
            "other": "https://notbanskabystrica.sk/feed",
            "suffix": "banskabystrica.skyline.example",
            "count": 3
        });
        let found = literal_domain_findings(&spec, domain);
        let paths: Vec<&str> = found
            .iter()
            .filter_map(|finding| finding.split(' ').next())
            .collect();
        assert_eq!(
            paths,
            [
                "spec.nested.deep[0].host",
                "spec.owner",
                "spec.sources[0][0]"
            ],
            "{found:#?}"
        );
        assert!(found[1].contains("jana@{orgDomain}"), "{}", found[1]);

        assert!(literal_domain_findings(&serde_json::json!({}), domain).is_empty());
        assert!(literal_domain_findings(&serde_json::json!("x"), domain).is_empty());
        assert!(
            literal_domain_findings(&spec, "localhost").is_empty(),
            "a domain without a dot finds nothing"
        );
    }

    #[test]
    fn is_dry_run_none_is_false() {
        let q = DryRunQuery {
            workspace: None,
            dry_run: None,
            validate_only: None,
            confirm: None,
        };
        assert!(!is_dry_run(&q).expect("query should succeed"));
    }

    #[test]
    fn is_dry_run_all_is_true() {
        let q = DryRunQuery {
            workspace: None,
            dry_run: Some("All".to_string()),
            validate_only: None,
            confirm: None,
        };
        assert!(is_dry_run(&q).expect("query should succeed"));
    }

    #[test]
    fn validate_only_is_a_dry_run_and_false_is_a_write() {
        let only = |validate_only| DryRunQuery {
            validate_only,
            ..DryRunQuery::default()
        };
        assert!(is_dry_run(&only(Some(true))).expect("validate_only=true"));
        assert!(!is_dry_run(&only(Some(false))).expect("validate_only=false"));
        let both = DryRunQuery {
            dry_run: Some("Foo".into()),
            ..only(Some(true))
        };
        assert!(
            is_dry_run(&both).is_err(),
            "a bad dryRun is refused, not hidden"
        );
    }

    #[test]
    fn is_dry_run_other_value_is_rejected_with_bad_request() {
        for candidate in ["true", "all", "1", ""] {
            let q = DryRunQuery {
                workspace: None,
                dry_run: Some(candidate.to_string()),
                validate_only: None,
                confirm: None,
            };
            let err = is_dry_run(&q).expect_err("should reject unsupported values");
            match err {
                ApiError::BadRequest(msg) => {
                    assert!(
                        msg.contains("All"),
                        "error message should name 'All' as accepted value, got: {msg}"
                    );
                }
                other => panic!("expected ApiError::BadRequest, got {other:?}"),
            }
        }
    }
}
