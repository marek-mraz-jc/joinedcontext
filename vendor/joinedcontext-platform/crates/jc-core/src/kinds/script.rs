//! `kind: Script`: a stored, reviewed and versioned JavaScript transformation a Pipeline
//! references by name and version (PL-118…PL-121, T-7083).
//!
//! A raw `javascript` processor stays refused in a pipeline (PL-52): the code reaches a stream
//! only through a published version of a Script, which went through a Change with four eyes and
//! carries the SHA-256 the reconciler checks before it renders the code (PL-120). Versions are
//! kept side by side, so a pipeline pinned to `1.0.0` keeps running it after `1.1.0` is
//! published, until its pin moves. What needs other manifests (a published version never
//! changes, a pinned version never goes, a reference resolves) is the Portal's judge; this kind
//! checks the manifest alone.

use crate::envelope::{Kind, ObjectMeta, Scope};
use crate::error::{Error, Result};
use crate::names;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The largest script, in bytes of source (PL-118).
pub const MAX_CODE_BYTES: usize = 64 * 1024;
/// The most versions one Script keeps (PL-118).
pub const MAX_VERSIONS: usize = 20;
/// The most tests one version carries (PL-118).
pub const MAX_TESTS: usize = 20;
/// The longest a script may run per message, in milliseconds (the runner's own ceiling).
pub const MAX_TIMEOUT_MS: u32 = 200;
/// The most memory a script may use per message, in MiB (the runner's own ceiling).
pub const MAX_MEMORY_MB: u32 = 64;
/// The runner's default time per message when a version sets none.
pub const DEFAULT_TIMEOUT_MS: u32 = 50;
/// The runner's default memory per message when a version sets none.
pub const DEFAULT_MEMORY_MB: u32 = 32;

/// Desired specification of a [`Script`][crate::kinds::Script] resource (PL-118).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ScriptSpec {
    /// The language of every version; `javascript` is the only one.
    pub language: ScriptLanguage,
    /// What the script does, for the people who pin it, e.g. `Converts S-JTSK to WGS84`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Every version, oldest first, e.g. `1.0.0` then `1.1.0`.
    pub versions: Vec<ScriptVersion>,
}

/// The language a Script is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ScriptLanguage {
    /// ECMAScript run by the runner's sandboxed `javascript` processor (Bento's v0 API).
    Javascript,
}

/// Whether a version may be pinned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ScriptState {
    /// Being written; no pipeline may reference it.
    Draft,
    /// Reviewed and frozen: its code, digest and limits never change again.
    Published,
}

/// One version of a Script (PL-118).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ScriptVersion {
    /// `MAJOR.MINOR.PATCH`, e.g. `1.1.0`.
    pub version: String,
    /// `draft` or `published`.
    pub state: ScriptState,
    /// The source, at most 64 KiB, using `bento.v0_msg_as_structured()` and the other v0 calls.
    pub code: String,
    /// The lower-case hex SHA-256 of `code`; the reconciler renders nothing on a mismatch.
    pub sha256: String,
    /// Time and memory per message; absent, the runner's defaults (50 ms, 32 MiB).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<ScriptLimits>,
    /// Input records and the records the script must make of them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<ScriptTest>,
}

/// Per-message limits of a version (PL-118).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ScriptLimits {
    /// Milliseconds per message, 1 to 200, e.g. `50`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u32>,
    /// MiB per message, 1 to 64, e.g. `32`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u32>,
}

/// One test of a version: what goes in and what must come out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ScriptTest {
    /// A short name, e.g. `converts-one-point`.
    pub name: String,
    /// The input message, as JSON.
    pub input: serde_json::Value,
    /// The message the script must produce, as JSON.
    pub expected: serde_json::Value,
}

/// A pinned version of a Script, e.g. `{name: sjtsk-to-wgs84, version: 1.0.0}` (PL-119).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ScriptRef {
    /// The Script's name in the same project.
    pub name: String,
    /// A published version of it.
    pub version: String,
}

impl ScriptRef {
    /// The reference's own shape: a DNS-label name and a semver version.
    pub fn validate(&self, field: &str) -> Result<()> {
        names::validate_dns1123_label(&self.name).map_err(|_| {
            invalid(
                format!("{field}.name"),
                format!(
                    "`{}` is not a Script name, e.g. `sjtsk-to-wgs84`",
                    self.name
                ),
            )
        })?;
        semver(&format!("{field}.version"), &self.version).map(|_| ())
    }
}

impl ScriptLimits {
    /// The time per message the runner enforces.
    pub fn timeout_ms(&self) -> u32 {
        self.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS)
    }

    /// The memory per message the runner enforces.
    pub fn memory_mb(&self) -> u32 {
        self.memory_mb.unwrap_or(DEFAULT_MEMORY_MB)
    }
}

/// The lower-case hex SHA-256 of `code`, as a version's `sha256` holds it.
pub fn digest(code: &str) -> String {
    Sha256::digest(code.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl ScriptVersion {
    /// Whether `code` is still the code `sha256` was taken of.
    pub fn intact(&self) -> bool {
        digest(&self.code) == self.sha256
    }
}

impl ScriptSpec {
    /// The version `version`, if the Script keeps it.
    pub fn version(&self, version: &str) -> Option<&ScriptVersion> {
        self.versions.iter().find(|v| v.version == version)
    }

    /// The manifest's own rules (PL-118).
    pub fn validate(&self) -> Result<()> {
        if self.versions.is_empty() || self.versions.len() > MAX_VERSIONS {
            return Err(invalid(
                "spec.versions",
                format!("a Script keeps 1 to {MAX_VERSIONS} versions"),
            ));
        }
        let mut previous: Option<(u64, u64, u64)> = None;
        for (i, v) in self.versions.iter().enumerate() {
            let at = format!("spec.versions[{i}]");
            let number = semver(&format!("{at}.version"), &v.version)?;
            if previous.is_some_and(|p| p >= number) {
                return Err(invalid(
                    format!("{at}.version"),
                    format!(
                        "`{}` must be higher than the version before it: versions are listed \
                         oldest first, each one once",
                        v.version
                    ),
                ));
            }
            previous = Some(number);
            if v.code.trim().is_empty() || v.code.len() > MAX_CODE_BYTES {
                return Err(invalid(
                    format!("{at}.code"),
                    format!("the code of a version is 1 to {MAX_CODE_BYTES} bytes"),
                ));
            }
            if !v.intact() {
                return Err(invalid(
                    format!("{at}.sha256"),
                    format!(
                        "does not match the code of version {}: it is the lower-case hex \
                         SHA-256 of `code`",
                        v.version
                    ),
                ));
            }
            if let Some(limits) = &v.limits {
                bounded(
                    &format!("{at}.limits.timeoutMs"),
                    limits.timeout_ms,
                    MAX_TIMEOUT_MS,
                )?;
                bounded(
                    &format!("{at}.limits.memoryMb"),
                    limits.memory_mb,
                    MAX_MEMORY_MB,
                )?;
            }
            if v.tests.len() > MAX_TESTS {
                return Err(invalid(
                    format!("{at}.tests"),
                    format!("a version carries at most {MAX_TESTS} tests"),
                ));
            }
            let mut seen = std::collections::BTreeSet::new();
            for (t, test) in v.tests.iter().enumerate() {
                if names::validate_dns1123_label(&test.name).is_err() || !seen.insert(&test.name) {
                    return Err(invalid(
                        format!("{at}.tests[{t}].name"),
                        format!(
                            "`{}` is not a unique test name, e.g. `converts-one-point`",
                            test.name
                        ),
                    ));
                }
            }
        }
        Ok(())
    }
}

impl Kind for ScriptSpec {
    const KIND: &'static str = "Script";
    const PLURAL: &'static str = "scripts";
    const SCOPE: Scope = Scope::Project;
    const PATH_TEMPLATE: &'static str = "projects/{project}/scripts/{name}/script.yaml";

    fn validate_spec(&self, meta: &ObjectMeta) -> Result<()> {
        names::validate_dns1123_label(&meta.name)?;
        self.validate()
    }
}

fn invalid(field: impl Into<String>, reason: impl Into<String>) -> Error {
    Error::Invalid {
        field: field.into(),
        reason: reason.into(),
    }
}

fn bounded(field: &str, value: Option<u32>, max: u32) -> Result<()> {
    match value {
        Some(v) if v == 0 || v > max => Err(invalid(field, format!("{v} is outside 1 to {max}"))),
        _ => Ok(()),
    }
}

/// `MAJOR.MINOR.PATCH` of plain numbers, without a pre-release or build part.
fn semver(field: &str, text: &str) -> Result<(u64, u64, u64)> {
    let parts: Vec<&str> = text.split('.').collect();
    let number = |p: &str| {
        (!p.is_empty()
            && p.len() <= 9
            && p.bytes().all(|b| b.is_ascii_digit())
            && (p == "0" || !p.starts_with('0')))
        .then(|| p.parse::<u64>().ok())
        .flatten()
    };
    match parts.as_slice() {
        [a, b, c] => match (number(a), number(b), number(c)) {
            (Some(a), Some(b), Some(c)) => Ok((a, b, c)),
            _ => Err(not_semver(field, text)),
        },
        _ => Err(not_semver(field, text)),
    }
}

fn not_semver(field: &str, text: &str) -> Error {
    invalid(
        field,
        format!("`{text}` is not MAJOR.MINOR.PATCH, e.g. `1.1.0`"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::ResourceEnvelope;

    const CODE: &str =
        "let m = bento.v0_msg_as_structured(); m.ok = true; bento.v0_msg_set_structured(m);";

    fn manifest(versions: serde_json::Value) -> String {
        serde_json::json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "Script",
            "metadata": { "name": "flag-ok", "namespace": "helsinki" },
            "spec": { "language": "javascript", "versions": versions }
        })
        .to_string()
    }

    fn version(v: &str, state: &str) -> serde_json::Value {
        serde_json::json!({ "version": v, "state": state, "code": CODE, "sha256": digest(CODE) })
    }

    fn validate(versions: serde_json::Value) -> Result<()> {
        ResourceEnvelope::<ScriptSpec>::from_yaml(&manifest(versions))
            .map_err(|e| Error::Parse(e.to_string()))
            .and_then(|m| m.validate())
    }

    fn refused_field(versions: serde_json::Value) -> String {
        match validate(versions).expect_err("refused") {
            Error::Invalid { field, .. } => field,
            other => panic!("unexpected {other}"),
        }
    }

    #[test]
    fn two_versions_side_by_side_are_valid() {
        validate(serde_json::json!([
            version("1.0.0", "published"),
            version("1.1.0", "draft")
        ]))
        .expect("valid");
    }

    #[test]
    fn each_broken_field_is_refused_by_name() {
        let mut tampered = version("1.0.0", "published");
        tampered["code"] = serde_json::json!("bento.v0_msg_set_string('x')");
        assert_eq!(
            refused_field(serde_json::json!([tampered])),
            "spec.versions[0].sha256"
        );
        assert_eq!(
            refused_field(serde_json::json!([version("1.0", "draft")])),
            "spec.versions[0].version"
        );
        assert_eq!(
            refused_field(serde_json::json!([
                version("1.1.0", "published"),
                version("1.0.0", "draft")
            ])),
            "spec.versions[1].version"
        );
        assert_eq!(refused_field(serde_json::json!([])), "spec.versions");
        let big = "x".repeat(MAX_CODE_BYTES + 1);
        let mut large = version("1.0.0", "draft");
        large["code"] = serde_json::json!(big);
        large["sha256"] = serde_json::json!(digest(&big));
        assert_eq!(
            refused_field(serde_json::json!([large])),
            "spec.versions[0].code"
        );
        let mut slow = version("1.0.0", "draft");
        slow["limits"] = serde_json::json!({ "timeoutMs": 201 });
        assert_eq!(
            refused_field(serde_json::json!([slow])),
            "spec.versions[0].limits.timeoutMs"
        );
        let mut greedy = version("1.0.0", "draft");
        greedy["limits"] = serde_json::json!({ "memoryMb": 0 });
        assert_eq!(
            refused_field(serde_json::json!([greedy])),
            "spec.versions[0].limits.memoryMb"
        );
        let mut twice = version("1.0.0", "draft");
        twice["tests"] = serde_json::json!([
            { "name": "a", "input": {}, "expected": {} },
            { "name": "a", "input": {}, "expected": {} }
        ]);
        assert_eq!(
            refused_field(serde_json::json!([twice])),
            "spec.versions[0].tests[1].name"
        );
    }

    #[test]
    fn an_unknown_field_or_language_does_not_parse() {
        let mut extra = version("1.0.0", "draft");
        extra["file"] = serde_json::json!("script.js");
        assert!(matches!(
            validate(serde_json::json!([extra])),
            Err(Error::Parse(_))
        ));
        let python = manifest(serde_json::json!([version("1.0.0", "draft")]))
            .replace("javascript", "python");
        assert!(ResourceEnvelope::<ScriptSpec>::from_yaml(&python).is_err());
    }

    #[test]
    fn a_reference_checks_its_name_and_version() {
        let good = ScriptRef {
            name: "flag-ok".into(),
            version: "1.0.0".into(),
        };
        good.validate("spec.steps[0].scriptRef").expect("valid");
        let bad = ScriptRef {
            name: "flag-ok".into(),
            version: "01.0.0".into(),
        };
        assert!(bad.validate("f").is_err());
        let bad = ScriptRef {
            name: "Flag".into(),
            version: "1.0.0".into(),
        };
        assert!(bad.validate("f").is_err());
    }
}
