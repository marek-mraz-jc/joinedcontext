#![forbid(unsafe_code)]
//! `kaisatsu-import`: Declarative config-as-code apply, offline check, plan/apply with stale-plan
//! refusal, drift detection, secret substitution, and round-trip export (W1, W1b).

use chrono::Utc;
use kaisatsu_core::claims::is_known_mapper_kind;
use kaisatsu_core::*;
use kaisatsu_crypto::hash_password;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

/// Embedded JSON Schema of the accepted realm representation (W1b).
pub const REALM_JSON_SCHEMA: &str = include_str!("../schema/realm.schema.json");

/// Exit codes matching `terraform plan -detailed-exitcode` (W1b):
/// - `0`: no change
/// - `1`: invalid input / policy violation
/// - `2`: changes present (plan/diff) or applied
/// - `3`: stale plan or lock timeout
/// - `4`: refused by authorization
pub const EXIT_NO_CHANGE: i32 = 0;
pub const EXIT_INVALID_INPUT: i32 = 1;
pub const EXIT_CHANGES_OR_APPLIED: i32 = 2;
pub const EXIT_STALE_OR_LOCK_TIMEOUT: i32 = 3;
pub const EXIT_FORBIDDEN: i32 = 4;

/// Apply mode per W1b (`no-delete` default vs `full` / `--prune`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApplyMode {
    NoDelete,
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyOptions {
    pub mode: ApplyMode,
    pub dry_run: bool,
    pub allow_literal_secrets: bool,
    pub secrets_dir: Option<PathBuf>,
    pub env_vars: BTreeMap<String, String>,
}

impl ApplyOptions {
    /// Fills [`ApplyOptions::env_vars`] from the whole process environment, as the standalone
    /// `kaisatsu` CLI and server resolve `$(env:NAME)`. `jc` never calls it: it hands the importer
    /// only the values it names, so a realm file cannot read another module's secret.
    #[must_use]
    #[allow(clippy::disallowed_methods)] // The standalone binary's own environment reader.
    pub fn with_process_env(mut self) -> Self {
        for (name, value) in std::env::vars() {
            self.env_vars.entry(name).or_insert(value);
        }
        self
    }
}

impl Default for ApplyOptions {
    fn default() -> Self {
        Self {
            mode: ApplyMode::NoDelete,
            dry_run: false,
            allow_literal_secrets: false,
            secrets_dir: None,
            env_vars: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeAction {
    Create,
    Update,
    Delete,
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectChange {
    pub kind: String,
    pub id: String,
    pub action: ChangeAction,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed_fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyReport {
    pub realm: String,
    pub dry_run: bool,
    pub checksum: String,
    pub base_state_checksum: String,
    pub changes: Vec<ObjectChange>,
    pub warnings: Vec<String>,
}

impl ApplyReport {
    #[must_use]
    pub fn has_changes(&self) -> bool {
        self.changes
            .iter()
            .any(|c| c.action != ChangeAction::Unchanged)
    }

    #[must_use]
    pub fn exit_code(&self) -> i32 {
        if self.has_changes() {
            EXIT_CHANGES_OR_APPLIED
        } else {
            EXIT_NO_CHANGE
        }
    }

    /// Formats a short Markdown summary for a PR comment with no secrets (W1b).
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let creates = self
            .changes
            .iter()
            .filter(|c| c.action == ChangeAction::Create)
            .count();
        let updates = self
            .changes
            .iter()
            .filter(|c| c.action == ChangeAction::Update)
            .count();
        let deletes = self
            .changes
            .iter()
            .filter(|c| c.action == ChangeAction::Delete)
            .count();
        let unchanged = self
            .changes
            .iter()
            .filter(|c| c.action == ChangeAction::Unchanged)
            .count();
        let mut md = format!(
            "### kaisatsu plan for realm `{}` {}\n\n| Action | Count |\n|---|---:|\n| Create | {} |\n| Update | {} |\n| Delete | {} |\n| Unchanged | {} |\n",
            self.realm,
            if self.dry_run { "(dry-run)" } else { "" },
            creates,
            updates,
            deletes,
            unchanged
        );
        if !self.warnings.is_empty() {
            md.push_str("\n#### Warnings\n");
            for w in &self.warnings {
                md.push_str(&format!("- {w}\n"));
            }
        }
        md
    }
}

/// Saved plan artifact for `kaisatsu plan -f <files> -o plan.json` and `kaisatsu apply plan.json` (W1b).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyPlan {
    pub realm: String,
    pub base_state_checksum: String,
    pub merged_realm_json: Value,
    pub mode: ApplyMode,
    pub report: ApplyReport,
}

/// Known top-level fields of a Keycloak / keycloak-config-cli realm representation. Any field
/// outside this list produces a warning with its JSON path (W1, W1b).
const KNOWN_REALM_FIELDS: &[&str] = &[
    "id",
    "realm",
    "displayName",
    "notBefore",
    "defaultSignatureAlgorithm",
    "revokeRefreshToken",
    "refreshTokenMaxReuse",
    "accessTokenLifespan",
    "accessTokenLifespanForImplicitFlow",
    "ssoSessionIdleTimeout",
    "ssoSessionMaxLifespan",
    "ssoSessionIdleTimeoutRememberMe",
    "ssoSessionMaxLifespanRememberMe",
    "offlineSessionIdleTimeout",
    "offlineSessionMaxLifespanEnabled",
    "offlineSessionMaxLifespan",
    "clientSessionIdleTimeout",
    "clientSessionMaxLifespan",
    "clientOfflineSessionIdleTimeout",
    "clientOfflineSessionMaxLifespan",
    "accessCodeLifespan",
    "accessCodeLifespanUserAction",
    "accessCodeLifespanLogin",
    "actionTokenGeneratedByAdminLifespan",
    "actionTokenGeneratedByUserLifespan",
    "enabled",
    "adminPermissionsEnabled",
    "sslRequired",
    "registrationAllowed",
    "registrationEmailAsUsername",
    "rememberMe",
    "verifyEmail",
    "loginWithEmailAllowed",
    "duplicateEmailsAllowed",
    "resetPasswordAllowed",
    "editUsernameAllowed",
    "bruteForceProtected",
    "permanentLockout",
    "maxFailureWaitSeconds",
    "minimumQuickLoginWaitSeconds",
    "waitIncrementSeconds",
    "quickLoginCheckMilliSeconds",
    "maxDeltaTimeSeconds",
    "failureFactor",
    "defaultRoles",
    "passwordPolicy",
    "requiredCredentials",
    "otpPolicyType",
    "otpPolicyAlgorithm",
    "otpPolicyInitialCounter",
    "otpPolicyDigits",
    "otpPolicyLookAheadWindow",
    "otpPolicyPeriod",
    "otpSupportedApplications",
    "webAuthnPolicyRpEntityName",
    "webAuthnPolicySignatureAlgorithms",
    "webAuthnPolicyRpId",
    "webAuthnPolicyAttestationConveyancePreference",
    "webAuthnPolicyAuthenticatorAttachment",
    "webAuthnPolicyRequireResidentKey",
    "webAuthnPolicyUserVerificationRequirement",
    "webAuthnPolicyCreateTimeout",
    "webAuthnPolicyAvoidSameAuthenticatorRegister",
    "webAuthnPolicyAcceptableAaguids",
    "webAuthnPolicyPasswordlessRpEntityName",
    "webAuthnPolicyPasswordlessSignatureAlgorithms",
    "webAuthnPolicyPasswordlessRpId",
    "webAuthnPolicyPasswordlessAttestationConveyancePreference",
    "webAuthnPolicyPasswordlessAuthenticatorAttachment",
    "webAuthnPolicyPasswordlessRequireResidentKey",
    "webAuthnPolicyPasswordlessUserVerificationRequirement",
    "webAuthnPolicyPasswordlessCreateTimeout",
    "webAuthnPolicyPasswordlessAvoidSameAuthenticatorRegister",
    "webAuthnPolicyPasswordlessAcceptableAaguids",
    "browserSecurityHeaders",
    "smtpServer",
    "loginTheme",
    "accountTheme",
    "adminTheme",
    "emailTheme",
    "eventsEnabled",
    "eventsExpiration",
    "eventsListeners",
    "enabledEventTypes",
    "adminEventsEnabled",
    "adminEventsDetailsEnabled",
    "internationalizationEnabled",
    "supportedLocales",
    "defaultLocale",
    "browserFlow",
    "registrationFlow",
    "directGrantFlow",
    "resetCredentialsFlow",
    "clientAuthenticationFlow",
    "dockerAuthenticationFlow",
    "attributes",
    "userManagedAccessAllowed",
    "components",
    "defaultOptionalClientScopes",
    "clientProfiles",
    "clientPolicies",
    "clientScopes",
    "users",
    "clients",
    "groups",
    "defaultGroups",
    "identityProviders",
    "roles",
    "requiredActions",
    "authenticatorConfig",
    "authenticationFlows",
    "organizations",
];

/// Known authenticator execution kinds from RI's `authenticationFlows`. Any unknown authenticator
/// or unmappable flow fails validation (W1, W1b).
pub const KNOWN_AUTHENTICATORS: &[&str] = &[
    "auth-cookie",
    "identity-provider-redirector",
    "conditional-user-role",
    "auth-otp-form",
    "conditional-level-of-authentication",
    "auth-username-password-form",
    "direct-grant-validate-username",
    "direct-grant-validate-password",
    "direct-grant-validate-otp",
    "conditional-user-configured",
    "webauthn-authenticator",
    // Client authentication (the `clients` flow): the methods kaisatsu implements at its token
    // endpoint. Keycloak's `client-secret-jwt` and `client-x509` stay unknown.
    "client-secret",
    "client-jwt",
    "federated-jwt",
    // Password reset (the `reset credentials` flow): kaisatsu's login handler chooses the user,
    // mails a signed single-use reset link and sets the new password (W3, K10).
    "reset-credentials-choose-user",
    "reset-credential-email",
    "reset-password",
];

pub const KNOWN_FLOW_ALIASES: &[&str] = &[
    "custom browser",
    "custom browser forms",
    "custom browser required 2fa",
    "custom browser password",
    "custom browser step-up",
    "custom browser 2fa",
    "custom direct grant",
    "custom direct grant required otp",
    "custom direct grant otp",
    "master browser",
    "master browser forms",
    "browser",
    "direct grant",
    "reset credentials",
    "first broker login",
    "registration",
    "clients",
    "docker auth",
];

fn sha256_json(val: &Value) -> String {
    let bytes = serde_json::to_vec(val).unwrap_or_default();
    let digest = Sha256::digest(&bytes);
    let mut s = String::with_capacity(64);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// True when `raw` is a `$(NAME)` / `$(env:NAME)` reference to a variable that is set nowhere.
fn secret_variable_unset(raw: &str, opts: &ApplyOptions) -> bool {
    let Some(inner) = raw
        .trim()
        .strip_prefix("$(")
        .and_then(|s| s.strip_suffix(')'))
    else {
        return false;
    };
    if inner.starts_with("file:") {
        return false;
    }
    let name = inner.strip_prefix("env:").unwrap_or(inner).trim();
    !opts.env_vars.contains_key(name)
}

/// Resolves `$(env:NAME)`, `$(NAME)`, and `$(file:/path)` in a secret field (W1b).
/// Refuses literal secrets unless `opts.allow_literal_secrets` is true, and refuses `$(file:...)`
/// outside `opts.secrets_dir`.
fn resolve_secret_value(raw: &str, field_path: &str, opts: &ApplyOptions) -> Result<String> {
    let trimmed = raw.trim();
    if let Some(inner) = trimmed.strip_prefix("$(").and_then(|s| s.strip_suffix(')')) {
        if let Some(file_path) = inner.strip_prefix("file:") {
            let base_dir = opts.secrets_dir.as_deref().ok_or_else(|| {
                KaisatsuError::BadRequest(format!(
                    "{field_path}: $(file:...) requires a configured secrets directory"
                ))
            })?;
            // A relative name is read under the secrets directory, so a realm file and its
            // secrets copied to another path together still apply.
            let candidate = base_dir.join(file_path.trim());
            let canonical_base = base_dir
                .canonicalize()
                .unwrap_or_else(|_| base_dir.to_path_buf());
            let canonical_target = candidate.canonicalize().map_err(|_| {
                KaisatsuError::BadRequest(format!(
                    "{field_path}: secret file does not exist or is outside secrets directory"
                ))
            })?;
            if !canonical_target.starts_with(&canonical_base) {
                return Err(KaisatsuError::BadRequest(format!(
                    "{field_path}: $(file:...) path escapes the configured secrets directory"
                )));
            }
            let content = std::fs::read_to_string(&canonical_target).map_err(|_| {
                KaisatsuError::BadRequest(format!("{field_path}: cannot read secret file"))
            })?;
            return Ok(content.trim().to_owned());
        }
        let var_name = inner.strip_prefix("env:").unwrap_or(inner).trim();
        if let Some(v) = opts.env_vars.get(var_name).cloned() {
            return Ok(v);
        }
        // A dry run (plan, check) may lack the variable and validates with a placeholder; a real
        // apply never stores a guessable placeholder as a secret.
        if opts.dry_run {
            return Ok(format!("resolved:{var_name}"));
        }
        return Err(KaisatsuError::BadRequest(format!(
            "{field_path}: secret variable {var_name} is not set"
        )));
    }
    if !opts.allow_literal_secrets {
        return Err(KaisatsuError::BadRequest(format!(
            "{field_path}: literal secret is refused by policy; use $(env:NAME) or $(file:/path)"
        )));
    }
    Ok(trimmed.to_owned())
}

/// Checks that no non-secret field contains `$(env:...)` or `$(file:...)` substitution (W1b).
fn check_no_secret_substitution_in_plain_fields(val: &Value, path: &str) -> Result<()> {
    match val {
        Value::String(s) => {
            if (s.contains("$(env:") || s.contains("$(file:"))
                && !path.ends_with(".secret")
                && !path.ends_with(".password")
                && !path.ends_with(".value")
                && !path.ends_with(".secretData")
                && !path.ends_with(".pairwiseSubAlgorithmSalt")
                && !path.ends_with(".salt")
                && !path.ends_with(".clientSecret")
            {
                return Err(KaisatsuError::BadRequest(format!(
                    "{path}: secret substitution is only allowed in fields marked secret"
                )));
            }
            Ok(())
        }
        Value::Array(arr) => {
            for (idx, item) in arr.iter().enumerate() {
                check_no_secret_substitution_in_plain_fields(item, &format!("{path}[{idx}]"))?;
            }
            Ok(())
        }
        Value::Object(map) => {
            for (k, v) in map {
                check_no_secret_substitution_in_plain_fields(v, &format!("{path}.{k}"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Merges multiple realm JSON documents in file-name order, as `keycloak-config-cli` does (W1b).
pub fn merge_realm_files(files: &[(String, Value)]) -> Result<Value> {
    let mut sorted = files.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut merged = Map::new();
    for (_, doc) in sorted {
        let obj = doc.as_object().ok_or_else(|| {
            KaisatsuError::BadRequest("realm file must be a JSON/YAML object".into())
        })?;
        for (k, v) in obj {
            match (merged.get_mut(k), v) {
                (Some(Value::Array(existing)), Value::Array(more)) => {
                    existing.extend(more.clone());
                }
                (Some(Value::Object(existing)), Value::Object(more)) => {
                    for (sk, sv) in more {
                        existing.insert(sk.clone(), sv.clone());
                    }
                }
                _ => {
                    merged.insert(k.clone(), v.clone());
                }
            }
        }
    }
    Ok(Value::Object(merged))
}

/// Runs offline validation (`kaisatsu check`, W1b):
/// - Schema & field checks (unknown plain fields returned as warnings with their JSON path).
/// - Every reference resolves (a client's default/optional scope, a user's group or role, a role's
///   composite, an execution's authenticatorConfig).
/// - Unknown mapper kinds, unknown authenticator kinds, and unmappable flows fail.
/// - Policy rules: no `implicitFlowEnabled: true`, no wildcard host in `redirectUris`, PKCE S256
///   on public clients, no literal secrets unless `allow_literal_secrets`.
pub fn check_realm_value(doc: &Value, opts: &ApplyOptions) -> Result<Vec<String>> {
    // The check validates references; whether a variable is set is decided by the apply.
    let opts = &ApplyOptions {
        dry_run: true,
        ..opts.clone()
    };
    check_no_secret_substitution_in_plain_fields(doc, "$")?;

    let obj = doc
        .as_object()
        .ok_or_else(|| KaisatsuError::BadRequest("realm document must be an object".into()))?;
    let realm_name = obj
        .get("realm")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| KaisatsuError::BadRequest("$.realm is required".into()))?;

    let mut warnings = Vec::new();
    for k in obj.keys() {
        if !KNOWN_REALM_FIELDS.contains(&k.as_str()) {
            warnings.push(format!("unknown field $.{k}"));
        }
    }

    // 1. Collect known scopes (built-in + declared in `clientScopes`)
    let mut known_scopes: BTreeSet<String> = std::iter::once("openid")
        .chain(kaisatsu_core::BUILTIN_CLIENT_SCOPES)
        .map(str::to_owned)
        .collect();

    if let Some(scopes) = obj.get("clientScopes").and_then(Value::as_array) {
        for (i, sc) in scopes.iter().enumerate() {
            let name = sc.get("name").and_then(Value::as_str).ok_or_else(|| {
                KaisatsuError::BadRequest(format!("$.clientScopes[{i}].name missing"))
            })?;
            known_scopes.insert(name.to_owned());
            if let Some(mappers) = sc.get("protocolMappers").and_then(Value::as_array) {
                for (mi, m) in mappers.iter().enumerate() {
                    let kind = m
                        .get("protocolMapper")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if !is_known_mapper_kind(kind) {
                        return Err(KaisatsuError::BadRequest(format!(
                            "$.clientScopes[{i}].protocolMappers[{mi}].protocolMapper: unknown mapper kind '{kind}'"
                        )));
                    }
                }
            }
        }
    }

    if let Some(def_opt) = obj
        .get("defaultOptionalClientScopes")
        .and_then(Value::as_array)
    {
        for (i, s) in def_opt.iter().enumerate() {
            let name = s.as_str().unwrap_or_default();
            if !known_scopes.contains(name) {
                return Err(KaisatsuError::BadRequest(format!(
                    "$.defaultOptionalClientScopes[{i}]: dangling reference to unknown scope '{name}'"
                )));
            }
        }
    }

    // 2. Collect known roles (built-in + declared in `roles.realm`)
    let mut known_realm_roles: BTreeSet<String> = [
        "offline_access",
        "uma_authorization",
        &format!("default-roles-{realm_name}"),
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();

    if let Some(realm_roles) = obj
        .get("roles")
        .and_then(|r| r.get("realm"))
        .and_then(Value::as_array)
    {
        for (i, r) in realm_roles.iter().enumerate() {
            let name = r.get("name").and_then(Value::as_str).ok_or_else(|| {
                KaisatsuError::BadRequest(format!("$.roles.realm[{i}].name missing"))
            })?;
            known_realm_roles.insert(name.to_owned());
        }
        for (i, r) in realm_roles.iter().enumerate() {
            if let Some(comps) = r.pointer("/composites/realm").and_then(Value::as_array) {
                for (ci, c) in comps.iter().enumerate() {
                    let cname = c.as_str().unwrap_or_default();
                    if !known_realm_roles.contains(cname) {
                        return Err(KaisatsuError::BadRequest(format!(
                            "$.roles.realm[{i}].composites.realm[{ci}]: dangling reference to unknown role '{cname}'"
                        )));
                    }
                }
            }
        }
    }

    // 3. Collect known groups (recursively)
    let mut known_group_paths: BTreeSet<String> = BTreeSet::new();
    fn collect_groups(arr: &[Value], out: &mut BTreeSet<String>) {
        for g in arr {
            if let Some(p) = g.get("path").and_then(Value::as_str) {
                out.insert(p.to_owned());
            } else if let Some(n) = g.get("name").and_then(Value::as_str) {
                out.insert(format!("/{n}"));
            }
            if let Some(subs) = g.get("subGroups").and_then(Value::as_array) {
                collect_groups(subs, out);
            }
        }
    }
    if let Some(groups) = obj.get("groups").and_then(Value::as_array) {
        collect_groups(groups, &mut known_group_paths);
    }
    if let Some(def_groups) = obj.get("defaultGroups").and_then(Value::as_array) {
        for (i, dg) in def_groups.iter().enumerate() {
            let path = dg.as_str().unwrap_or_default();
            if !known_group_paths.contains(path) {
                return Err(KaisatsuError::BadRequest(format!(
                    "$.defaultGroups[{i}]: dangling reference to unknown group '{path}'"
                )));
            }
        }
    }

    // 4. Validate clients, policies, mappers, scope references, and secrets
    let mut known_idps: BTreeSet<String> = BTreeSet::new();
    if let Some(idps) = obj.get("identityProviders").and_then(Value::as_array) {
        for (i, idp) in idps.iter().enumerate() {
            let alias = idp.get("alias").and_then(Value::as_str).ok_or_else(|| {
                KaisatsuError::BadRequest(format!("$.identityProviders[{i}].alias missing"))
            })?;
            known_idps.insert(alias.to_owned());
        }
    }

    if let Some(smtp) = obj.get("smtpServer").and_then(Value::as_object)
        && let Some(pw) = smtp.get("password").and_then(Value::as_str)
    {
        let _ = resolve_secret_value(pw, "$.smtpServer.password", opts)?;
    }

    if let Some(clients) = obj.get("clients").and_then(Value::as_array) {
        for (i, c) in clients.iter().enumerate() {
            let cid = c.get("clientId").and_then(Value::as_str).ok_or_else(|| {
                KaisatsuError::BadRequest(format!("$.clients[{i}].clientId missing"))
            })?;
            // Policy: no implicit flow
            if c.get("implicitFlowEnabled").and_then(Value::as_bool) == Some(true) {
                return Err(KaisatsuError::BadRequest(format!(
                    "$.clients[{i}] ({cid}): implicitFlowEnabled=true is forbidden by policy"
                )));
            }
            // Policy: PKCE S256 on public browser clients
            let is_public = c
                .get("publicClient")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let is_std = c
                .get("standardFlowEnabled")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            if is_public && is_std {
                let pkce = c
                    .pointer("/attributes/pkce.code.challenge.method")
                    .and_then(Value::as_str)
                    .unwrap_or("S256");
                if pkce != "S256" {
                    return Err(KaisatsuError::BadRequest(format!(
                        "$.clients[{i}] ({cid}): public standard-flow client must require PKCE S256"
                    )));
                }
            }
            // Policy: no wildcard in redirect URI host
            if let Some(uris) = c.get("redirectUris").and_then(Value::as_array) {
                for (ui, u) in uris.iter().enumerate() {
                    let u_str = u.as_str().unwrap_or_default();
                    let without_trailing = u_str.strip_suffix("/*").unwrap_or(u_str);
                    if without_trailing.contains('*') {
                        return Err(KaisatsuError::BadRequest(format!(
                            "$.clients[{i}].redirectUris[{ui}]: wildcard in host or mid-path is forbidden: {u_str}"
                        )));
                    }
                }
            }
            for attribute in SIGNATURE_ALG_ATTRIBUTES {
                if let Some(alg) = c
                    .get("attributes")
                    .and_then(|a| a.get(attribute))
                    .and_then(Value::as_str)
                {
                    check_signature_alg(cid, attribute, alg)
                        .map_err(|e| KaisatsuError::BadRequest(format!("$.clients[{i}]: {e}")))?;
                }
            }
            if let Some(sec) = c.get("secret").and_then(Value::as_str) {
                let _ = resolve_secret_value(sec, &format!("$.clients[{i}].secret"), opts)?;
            }
            if let Some(fed_iss) = c
                .pointer("/attributes/jwt.credential.issuer")
                .and_then(Value::as_str)
                && !known_idps.contains(fed_iss)
            {
                return Err(KaisatsuError::BadRequest(format!(
                    "$.clients[{i}].attributes['jwt.credential.issuer']: dangling reference to IdP '{fed_iss}'"
                )));
            }
            for field in ["defaultClientScopes", "optionalClientScopes"] {
                if let Some(sc_list) = c.get(field).and_then(Value::as_array) {
                    for (si, sc_val) in sc_list.iter().enumerate() {
                        let sc_name = sc_val.as_str().unwrap_or_default();
                        if !known_scopes.contains(sc_name) {
                            return Err(KaisatsuError::BadRequest(format!(
                                "$.clients[{i}].{field}[{si}]: dangling reference to unknown clientScope '{sc_name}'"
                            )));
                        }
                    }
                }
            }
            if let Some(mappers) = c.get("protocolMappers").and_then(Value::as_array) {
                for (mi, m) in mappers.iter().enumerate() {
                    let kind = m
                        .get("protocolMapper")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if !is_known_mapper_kind(kind) {
                        return Err(KaisatsuError::BadRequest(format!(
                            "$.clients[{i}].protocolMappers[{mi}].protocolMapper: unknown mapper kind '{kind}'"
                        )));
                    }
                }
            }
        }
    }

    // 5. Validate users' group and role references and credentials
    if let Some(users) = obj.get("users").and_then(Value::as_array) {
        for (i, u) in users.iter().enumerate() {
            if let Some(ugroups) = u.get("groups").and_then(Value::as_array) {
                for (gi, g) in ugroups.iter().enumerate() {
                    let gpath = g.as_str().unwrap_or_default();
                    let norm = if gpath.starts_with('/') {
                        gpath.to_owned()
                    } else {
                        format!("/{gpath}")
                    };
                    if !known_group_paths.contains(&norm) {
                        return Err(KaisatsuError::BadRequest(format!(
                            "$.users[{i}].groups[{gi}]: dangling reference to unknown group '{gpath}'"
                        )));
                    }
                }
            }
            if let Some(uroles) = u.get("realmRoles").and_then(Value::as_array) {
                for (ri, r) in uroles.iter().enumerate() {
                    let rname = r.as_str().unwrap_or_default();
                    if !known_realm_roles.contains(rname) {
                        return Err(KaisatsuError::BadRequest(format!(
                            "$.users[{i}].realmRoles[{ri}]: dangling reference to unknown realm role '{rname}'"
                        )));
                    }
                }
            }
            if let Some(creds) = u.get("credentials").and_then(Value::as_array) {
                for (ci, cred) in creds.iter().enumerate() {
                    if let Some(val) = cred.get("value").and_then(Value::as_str) {
                        let _ = resolve_secret_value(
                            val,
                            &format!("$.users[{i}].credentials[{ci}].value"),
                            opts,
                        )?;
                    }
                }
            }
        }
    }

    // 6. Validate authenticationFlows & authenticatorConfig (W1: map RI's flows to W7 state machines
    // and fail on any unknown authenticator or unmappable flow).
    let mut known_auth_configs: BTreeSet<String> = BTreeSet::new();
    if let Some(cfgs) = obj.get("authenticatorConfig").and_then(Value::as_array) {
        for c in cfgs {
            if let Some(alias) = c.get("alias").and_then(Value::as_str) {
                known_auth_configs.insert(alias.to_owned());
            }
        }
    }
    if let Some(flows) = obj.get("authenticationFlows").and_then(Value::as_array) {
        for (fi, flow) in flows.iter().enumerate() {
            let alias = flow
                .get("alias")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !KNOWN_FLOW_ALIASES.contains(&alias) {
                return Err(KaisatsuError::BadRequest(format!(
                    "$.authenticationFlows[{fi}].alias: unmappable authentication flow '{alias}'"
                )));
            }
            if let Some(execs) = flow
                .get("authenticationExecutions")
                .and_then(Value::as_array)
            {
                for (ei, ex) in execs.iter().enumerate() {
                    if let Some(auth) = ex.get("authenticator").and_then(Value::as_str)
                        && !KNOWN_AUTHENTICATORS.contains(&auth)
                    {
                        return Err(KaisatsuError::BadRequest(format!(
                            "$.authenticationFlows[{fi}].authenticationExecutions[{ei}].authenticator: unknown authenticator '{auth}'"
                        )));
                    }
                    if let Some(cfg_alias) = ex.get("authenticatorConfig").and_then(Value::as_str)
                        && !known_auth_configs.contains(cfg_alias)
                    {
                        return Err(KaisatsuError::BadRequest(format!(
                            "$.authenticationFlows[{fi}].authenticationExecutions[{ei}].authenticatorConfig: dangling reference '{cfg_alias}'"
                        )));
                    }
                }
            }
        }
    }

    Ok(warnings)
}

/// Computes a deterministic checksum of the current realm state in `store` for `plan` / `diff` (W1b).
pub async fn compute_realm_state_checksum(
    store: &Arc<dyn Store>,
    realm_id: &str,
) -> Result<String> {
    let Some(r) = store.get_realm(realm_id).await? else {
        return Ok("empty".into());
    };
    let exported = export_realm(store, &r.id, true).await?;
    Ok(sha256_json(&exported))
}

/// Before a real apply writes anything: every secret variable the documents name is set, except
/// where an existing client or SMTP configuration keeps its current secret (all or nothing).
async fn refuse_unset_secret_variables(
    store: &Arc<dyn Store>,
    doc: &Value,
    realm_id: &str,
    existing_realm: Option<&RealmConfig>,
    opts: &ApplyOptions,
) -> Result<()> {
    let unset = |raw: Option<&str>| raw.is_some_and(|r| secret_variable_unset(r, opts));
    let refuse = |path: String| {
        Err(KaisatsuError::BadRequest(format!(
            "{path}: secret variable is not set"
        )))
    };
    if unset(doc.pointer("/smtpServer/password").and_then(Value::as_str))
        && existing_realm
            .and_then(|r| r.smtp_server.as_ref())
            .is_none_or(|s| s.password.is_empty())
    {
        return refuse("$.smtpServer.password".into());
    }
    for (i, c) in doc
        .get("clients")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        if unset(c.get("secret").and_then(Value::as_str)) {
            let client_id = c
                .get("clientId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let has_secret = store
                .get_client_by_client_id(realm_id, client_id)
                .await?
                .is_some_and(|p| p.secret.is_some());
            if !has_secret {
                return refuse(format!("$.clients[{i}].secret"));
            }
        }
    }
    for (i, u) in doc
        .get("users")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        for (j, cred) in u
            .get("credentials")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let otp_value = cred
                .get("secretData")
                .and_then(Value::as_str)
                .and_then(|sd| serde_json::from_str::<Value>(sd).ok())
                .and_then(|sd| sd.get("value").and_then(Value::as_str).map(str::to_owned));
            if unset(cred.get("value").and_then(Value::as_str)) || unset(otp_value.as_deref()) {
                return refuse(format!("$.users[{i}].credentials[{j}]"));
            }
        }
    }
    Ok(())
}

/// Applies `files` (one or more JSON/YAML realm documents for one realm) to `store` (W1b).
pub async fn apply_realm_files(
    store: &Arc<dyn Store>,
    files: &[(String, Value)],
    opts: &ApplyOptions,
) -> Result<ApplyReport> {
    let merged = merge_realm_files(files)?;
    let warnings = check_realm_value(&merged, opts)?;
    let file_checksum = sha256_json(&merged);

    let obj = merged
        .as_object()
        .ok_or_else(|| KaisatsuError::BadRequest("not an object".into()))?;
    let realm_name = obj["realm"].as_str().unwrap_or("default").to_owned();
    let realm_id = obj
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(&realm_name)
        .to_owned();

    let existing_realm = store.get_realm(&realm_id).await?;
    if !opts.dry_run {
        refuse_unset_secret_variables(store, &merged, &realm_id, existing_realm.as_ref(), opts)
            .await?;
    }
    let base_state_checksum = compute_realm_state_checksum(store, &realm_id).await?;

    let mut changes: Vec<ObjectChange> = Vec::new();

    // Fast path: if checksum matches the last apply and mode is NoDelete, everything is Unchanged
    if existing_realm
        .as_ref()
        .and_then(|r| r.apply_checksum.as_deref())
        == Some(file_checksum.as_str())
        && opts.mode == ApplyMode::NoDelete
    {
        changes.push(ObjectChange {
            kind: "Realm".into(),
            id: realm_id,
            action: ChangeAction::Unchanged,
            changed_fields: Vec::new(),
        });
        return Ok(ApplyReport {
            realm: realm_name,
            dry_run: opts.dry_run,
            checksum: file_checksum,
            base_state_checksum,
            changes,
            warnings,
        });
    }

    // 1. RealmConfig
    let mut realm_cfg: RealmConfig = serde_json::from_value(merged.clone()).unwrap_or_default();
    realm_cfg.id = realm_id.clone();
    realm_cfg.realm = realm_name.clone();
    realm_cfg.apply_checksum = Some(file_checksum.clone());
    if let Some(smtp) = realm_cfg.smtp_server.as_mut()
        && !smtp.password.is_empty()
    {
        let raw_pw = std::mem::take(&mut smtp.password);
        let kept = existing_realm
            .as_ref()
            .and_then(|r| r.smtp_server.as_ref())
            .map(|s| s.password.clone())
            .filter(|p| !p.is_empty());
        smtp.password = match kept {
            Some(kept) if secret_variable_unset(&raw_pw, opts) => kept,
            _ => resolve_secret_value(&raw_pw, "$.smtpServer.password", opts)?,
        };
    }

    let realm_action = match &existing_realm {
        None => ChangeAction::Create,
        Some(prev)
            if prev.display_name == realm_cfg.display_name
                && prev.access_token_lifespan == realm_cfg.access_token_lifespan
                && prev.password_policy == realm_cfg.password_policy =>
        {
            ChangeAction::Unchanged
        }
        Some(_) => ChangeAction::Update,
    };
    changes.push(ObjectChange {
        kind: "Realm".into(),
        id: realm_id.clone(),
        action: realm_action,
        changed_fields: Vec::new(),
    });
    if !opts.dry_run {
        store.upsert_realm(realm_cfg.clone()).await?;
    }

    // 2. ClientScopes
    let existing_scopes = store.list_client_scopes(&realm_id).await?;
    let mut file_scope_names = BTreeSet::new();
    if let Some(scopes) = obj.get("clientScopes").and_then(Value::as_array) {
        for sc_val in scopes {
            let mut sc: ClientScopeRepresentation = serde_json::from_value(sc_val.clone())
                .map_err(|e| KaisatsuError::BadRequest(e.to_string()))?;
            sc.realm_id = realm_id.clone();
            sc.managed_by = Some("apply".into());
            file_scope_names.insert(sc.name.clone());
            let prev = existing_scopes.iter().find(|s| s.name == sc.name);
            let act = match prev {
                None => ChangeAction::Create,
                Some(p) if p.description == sc.description && p.attributes == sc.attributes => {
                    ChangeAction::Unchanged
                }
                Some(_) => ChangeAction::Update,
            };
            changes.push(ObjectChange {
                kind: "ClientScope".into(),
                id: sc.name.clone(),
                action: act,
                changed_fields: Vec::new(),
            });
            if !opts.dry_run {
                store.upsert_client_scope(sc).await?;
            }
        }
    }
    if opts.mode == ApplyMode::Full {
        for old in existing_scopes.iter().filter(|s| {
            s.managed_by.as_deref() == Some("apply") && !file_scope_names.contains(&s.name)
        }) {
            changes.push(ObjectChange {
                kind: "ClientScope".into(),
                id: old.name.clone(),
                action: ChangeAction::Delete,
                changed_fields: Vec::new(),
            });
            if !opts.dry_run {
                store.delete_client_scope(&realm_id, &old.id).await?;
            }
        }
    }

    // 3. Roles (`roles.realm`)
    let existing_roles = store.list_roles(&realm_id, None).await?;
    let mut file_role_names = BTreeSet::new();
    if let Some(r_arr) = obj
        .get("roles")
        .and_then(|r| r.get("realm"))
        .and_then(Value::as_array)
    {
        for r_val in r_arr {
            let mut role: RoleRepresentation = serde_json::from_value(r_val.clone())
                .map_err(|e| KaisatsuError::BadRequest(e.to_string()))?;
            role.realm_id = realm_id.clone();
            role.managed_by = Some("apply".into());
            file_role_names.insert(role.name.clone());
            let prev = existing_roles.iter().find(|r| r.name == role.name);
            let act = match prev {
                None => ChangeAction::Create,
                Some(p) if p.description == role.description && p.composite == role.composite => {
                    ChangeAction::Unchanged
                }
                Some(_) => ChangeAction::Update,
            };
            changes.push(ObjectChange {
                kind: "Role".into(),
                id: role.name.clone(),
                action: act,
                changed_fields: Vec::new(),
            });
            if !opts.dry_run {
                store.upsert_role(role).await?;
            }
        }
    }
    if opts.mode == ApplyMode::Full {
        for old in existing_roles.iter().filter(|r| {
            r.managed_by.as_deref() == Some("apply") && !file_role_names.contains(&r.name)
        }) {
            changes.push(ObjectChange {
                kind: "Role".into(),
                id: old.name.clone(),
                action: ChangeAction::Delete,
                changed_fields: Vec::new(),
            });
            if !opts.dry_run {
                store.delete_role(&realm_id, None, &old.name).await?;
            }
        }
    }

    // 4. Groups (including nested `subGroups`)
    let mut file_group_paths = BTreeSet::new();
    if let Some(g_arr) = obj.get("groups").and_then(Value::as_array) {
        for g_val in g_arr {
            let mut grp: GroupRepresentation = serde_json::from_value(g_val.clone())
                .map_err(|e| KaisatsuError::BadRequest(e.to_string()))?;
            grp.realm_id = realm_id.clone();
            grp.managed_by = Some("apply".into());
            for sub in &mut grp.sub_groups {
                sub.managed_by = Some("apply".into());
                file_group_paths.insert(sub.path.clone());
            }
            file_group_paths.insert(grp.path.clone());
            let prev = store.get_group_by_path(&realm_id, None, &grp.path).await?;
            let act = match prev {
                None => ChangeAction::Create,
                Some(p) if p.attributes == grp.attributes => ChangeAction::Unchanged,
                Some(_) => ChangeAction::Update,
            };
            changes.push(ObjectChange {
                kind: "Group".into(),
                id: grp.path.clone(),
                action: act,
                changed_fields: Vec::new(),
            });
            if !opts.dry_run {
                store.upsert_group(grp).await?;
            }
        }
    }

    // 5. IdentityProviders
    if let Some(idps) = obj.get("identityProviders").and_then(Value::as_array) {
        for idp_val in idps {
            let mut idp: IdentityProviderRepresentation =
                serde_json::from_value(idp_val.clone())
                    .map_err(|e| KaisatsuError::BadRequest(e.to_string()))?;
            idp.realm_id = realm_id.clone();
            idp.managed_by = Some("apply".into());
            let prev = store.get_idp(&realm_id, &idp.alias).await?;
            let act = match prev {
                None => ChangeAction::Create,
                Some(p) if p.config == idp.config && p.enabled == idp.enabled => {
                    ChangeAction::Unchanged
                }
                Some(_) => ChangeAction::Update,
            };
            changes.push(ObjectChange {
                kind: "IdentityProvider".into(),
                id: idp.alias.clone(),
                action: act,
                changed_fields: Vec::new(),
            });
            if !opts.dry_run {
                store.upsert_idp(idp).await?;
            }
        }
    }

    // 6. Clients
    let existing_clients = store.list_clients(&realm_id, None, None, false).await?;
    let mut file_client_ids = BTreeSet::new();
    if let Some(c_arr) = obj.get("clients").and_then(Value::as_array) {
        for c_val in c_arr {
            let mut client: ClientRepresentation = serde_json::from_value(c_val.clone())
                .map_err(|e| KaisatsuError::BadRequest(e.to_string()))?;
            client.realm_id = realm_id.clone();
            if client.managed_by.is_none() {
                client.managed_by = Some("apply".into());
            }
            file_client_ids.insert(client.client_id.clone());
            let prev = existing_clients
                .iter()
                .find(|c| c.client_id == client.client_id);
            // A file without protocol mappers keeps the stored ones (no-delete apply).
            if client.protocol_mappers.is_empty()
                && let Some(p) = prev
            {
                client.protocol_mappers = p.protocol_mappers.clone();
            }
            if let Some(raw_sec) = client.secret.clone() {
                // An unset variable keeps the client's current secret (re-applying an export);
                // a new client without its variable fails the apply.
                client.secret = match prev.and_then(|p| p.secret.clone()) {
                    Some(kept) if secret_variable_unset(&raw_sec, opts) => Some(kept),
                    _ => Some(resolve_secret_value(&raw_sec, "client.secret", opts)?),
                };
            }
            let act = match prev {
                None => ChangeAction::Create,
                Some(p)
                    if p.redirect_uris == client.redirect_uris
                        && p.default_client_scopes == client.default_client_scopes
                        && p.optional_client_scopes == client.optional_client_scopes
                        && p.attributes == client.attributes =>
                {
                    ChangeAction::Unchanged
                }
                Some(_) => ChangeAction::Update,
            };
            changes.push(ObjectChange {
                kind: "Client".into(),
                id: client.client_id.clone(),
                action: act,
                changed_fields: Vec::new(),
            });
            if !opts.dry_run {
                store.upsert_client(client).await?;
            }
        }
    }
    if opts.mode == ApplyMode::Full {
        // W1b: `--prune` deletes ONLY objects marked `managedBy: apply`; objects created in the
        // Portal (`managed-by: joinedcontext`) are never pruned!
        for old in existing_clients.iter().filter(|c| {
            c.managed_by.as_deref() == Some("apply") && !file_client_ids.contains(&c.client_id)
        }) {
            changes.push(ObjectChange {
                kind: "Client".into(),
                id: old.client_id.clone(),
                action: ChangeAction::Delete,
                changed_fields: Vec::new(),
            });
            if !opts.dry_run {
                store.delete_client(&realm_id, None, &old.id).await?;
            }
        }
    }

    // 7. Users & Credentials
    if let Some(u_arr) = obj.get("users").and_then(Value::as_array) {
        for u_val in u_arr {
            let mut user: UserRepresentation = serde_json::from_value(u_val.clone())
                .map_err(|e| KaisatsuError::BadRequest(e.to_string()))?;
            user.realm_id = realm_id.clone();
            if user.managed_by.is_none() {
                user.managed_by = Some("apply".into());
            }
            let prev = store
                .get_user_by_username_or_email(&realm_id, None, &user.username)
                .await?;
            let act = match &prev {
                None => ChangeAction::Create,
                Some(p)
                    if p.first_name == user.first_name
                        && p.last_name == user.last_name
                        && p.realm_roles == user.realm_roles
                        && p.groups == user.groups =>
                {
                    ChangeAction::Unchanged
                }
                Some(_) => ChangeAction::Update,
            };
            if let Some(p) = &prev
                && user.id.is_empty()
            {
                user.id = p.id.clone();
            }
            changes.push(ObjectChange {
                kind: "User".into(),
                id: user.username.clone(),
                action: act.clone(),
                changed_fields: Vec::new(),
            });
            if !opts.dry_run {
                let saved = if prev.is_some() {
                    store.update_user(user.clone()).await?
                } else {
                    store.create_user(user.clone()).await?
                };
                // Link groups
                for gpath in &saved.groups {
                    if let Some(grp) = store.get_group_by_path(&realm_id, None, gpath).await? {
                        let _ = store.add_user_to_group(&realm_id, &saved.id, &grp.id).await;
                    }
                }
                // Import credentials (`password`, `otp`, `webauthn`)
                if let Some(creds) = u_val.get("credentials").and_then(Value::as_array) {
                    for cred_val in creds {
                        let ctype = cred_val
                            .get("type")
                            .and_then(Value::as_str)
                            .unwrap_or("password");
                        let cid = cred_val
                            .get("id")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                            .unwrap_or_else(|| format!("{}:{}:{ctype}", realm_id, saved.id));
                        if ctype == "password" {
                            if let Some(raw_pw) = cred_val.get("value").and_then(Value::as_str) {
                                let resolved =
                                    resolve_secret_value(raw_pw, "user.credentials.value", opts)?;
                                let phc = hash_password(&resolved)?;
                                store
                                    .upsert_credential(CredentialRecord {
                                        id: cid,
                                        realm_id: realm_id.clone(),
                                        user_id: saved.id.clone(),
                                        credential_type: "password".into(),
                                        user_label: cred_val
                                            .get("userLabel")
                                            .and_then(Value::as_str)
                                            .map(str::to_owned),
                                        created_date: Utc::now().timestamp_millis(),
                                        secret_data: phc,
                                        credential_data: String::new(),
                                        temporary: cred_val
                                            .get("temporary")
                                            .and_then(Value::as_bool)
                                            .unwrap_or(false),
                                    })
                                    .await?;
                            } else if let Some(sec_data) =
                                cred_val.get("secretData").and_then(Value::as_str)
                            {
                                // Migrated Keycloak password hash (`secretData` + `credentialData`, W1)
                                store
                                    .upsert_credential(CredentialRecord {
                                        id: cid,
                                        realm_id: realm_id.clone(),
                                        user_id: saved.id.clone(),
                                        credential_type: "password".into(),
                                        user_label: cred_val
                                            .get("userLabel")
                                            .and_then(Value::as_str)
                                            .map(str::to_owned),
                                        created_date: Utc::now().timestamp_millis(),
                                        secret_data: sec_data.to_owned(),
                                        credential_data: cred_val
                                            .get("credentialData")
                                            .and_then(Value::as_str)
                                            .unwrap_or_default()
                                            .to_owned(),
                                        temporary: false,
                                    })
                                    .await?;
                            }
                        } else if ctype == "otp" {
                            let raw_sec_data = cred_val
                                .get("secretData")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            // The seed is a secret like any other: a `$(...)` reference, or a
                            // literal only where literal secrets are allowed.
                            let seed = serde_json::from_str::<Value>(raw_sec_data)
                                .ok()
                                .and_then(|sd| {
                                    sd.get("value").and_then(Value::as_str).map(str::to_owned)
                                })
                                .unwrap_or_else(|| raw_sec_data.to_owned());
                            let resolved =
                                resolve_secret_value(&seed, "otp.secretData.value", opts)?;
                            let secret_data = serde_json::json!({ "value": resolved }).to_string();
                            store
                                .upsert_credential(CredentialRecord {
                                    id: cid,
                                    realm_id: realm_id.clone(),
                                    user_id: saved.id.clone(),
                                    credential_type: "otp".into(),
                                    user_label: cred_val
                                        .get("userLabel")
                                        .and_then(Value::as_str)
                                        .map(str::to_owned),
                                    created_date: Utc::now().timestamp_millis(),
                                    secret_data,
                                    credential_data: cred_val
                                        .get("credentialData")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_owned(),
                                    temporary: false,
                                })
                                .await?;
                        }
                    }
                }
            }
        }
    }

    // Record one AdminEvent per changed object with the apply's file checksum as the cause (W1b)
    if !opts.dry_run {
        for ch in changes
            .iter()
            .filter(|c| c.action != ChangeAction::Unchanged)
        {
            store
                .append_admin_event(AdminEvent {
                    id: uuid::Uuid::now_v7().to_string(),
                    realm_id: realm_id.clone(),
                    org_id: None,
                    time: Utc::now().timestamp_millis(),
                    operation_type: format!("APPLY_{:?}", ch.action).to_ascii_uppercase(),
                    resource_type: ch.kind.clone(),
                    resource_path: ch.id.clone(),
                    actor_id: format!("apply:{file_checksum}"),
                    actor_client_id: None,
                    representation: None,
                })
                .await?;
        }
    }

    Ok(ApplyReport {
        realm: realm_name,
        dry_run: opts.dry_run,
        checksum: file_checksum,
        base_state_checksum,
        changes,
        warnings,
    })
}

/// Builds a [`ApplyPlan`] (`kaisatsu plan`, W1b) recording the realm's current `base_state_checksum`.
pub async fn create_plan(
    store: &Arc<dyn Store>,
    files: &[(String, Value)],
    opts: &ApplyOptions,
) -> Result<ApplyPlan> {
    let mut dry_opts = opts.clone();
    dry_opts.dry_run = true;
    let merged = merge_realm_files(files)?;
    let report = apply_realm_files(store, files, &dry_opts).await?;
    Ok(ApplyPlan {
        realm: report.realm.clone(),
        base_state_checksum: report.base_state_checksum.clone(),
        merged_realm_json: merged,
        mode: opts.mode,
        report,
    })
}

/// Executes a saved [`ApplyPlan`] (`kaisatsu apply plan.json`, W1b), refusing with
/// [`KaisatsuError::StalePlan`] (exit code 3) if the realm state changed since the plan was computed.
pub async fn apply_plan(
    store: &Arc<dyn Store>,
    plan: &ApplyPlan,
    opts: &ApplyOptions,
) -> Result<ApplyReport> {
    let current_checksum = compute_realm_state_checksum(store, &plan.realm).await?;
    if current_checksum != plan.base_state_checksum {
        return Err(KaisatsuError::StalePlan(format!(
            "realm '{}' changed since plan was created (expected checksum {}, found {})",
            plan.realm, plan.base_state_checksum, current_checksum
        )));
    }
    let mut run_opts = opts.clone();
    run_opts.mode = plan.mode;
    run_opts.dry_run = false;
    apply_realm_files(
        store,
        &[("plan.json".to_owned(), plan.merged_realm_json.clone())],
        &run_opts,
    )
    .await
}

/// Exports a realm back into the `keycloak-config-cli` JSON representation so `apply -> export -> apply`
/// is a fixed point (W1b).
pub async fn export_realm(
    store: &Arc<dyn Store>,
    realm_id: &str,
    with_users: bool,
) -> Result<Value> {
    let realm = store
        .get_realm(realm_id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("realm not found".into()))?;
    let scopes = store.list_client_scopes(realm_id).await?;
    let mut clients = store.list_clients(realm_id, None, None, false).await?;
    for c in &mut clients {
        if c.secret.is_some() {
            c.secret = Some(format!(
                "$(CLIENT_SECRET_{})",
                c.client_id.to_ascii_uppercase().replace('-', "_")
            ));
        }
        for m in &mut c.protocol_mappers {
            if m.protocol_mapper == "oidc-sha256-pairwise-sub-mapper" {
                for key in ["pairwiseSubAlgorithmSalt", "salt"] {
                    if m.config.contains_key(key) {
                        m.config.insert(
                            key.into(),
                            format!(
                                "$(PAIRWISE_SALT_{})",
                                c.client_id.to_ascii_uppercase().replace('-', "_")
                            ),
                        );
                    }
                }
            }
        }
    }
    let roles = store.list_roles(realm_id, None).await?;
    let groups = store.list_groups(realm_id, None, None, false).await?;
    let idps = store.list_idps(realm_id).await?;

    let mut out =
        serde_json::to_value(&realm).map_err(|e| KaisatsuError::Storage(e.to_string()))?;
    if let Value::Object(ref mut map) = out {
        map.remove("applyChecksum");
        map.insert(
            "clientScopes".into(),
            serde_json::to_value(&scopes).unwrap_or(Value::Array(Vec::new())),
        );
        map.insert(
            "clients".into(),
            serde_json::to_value(&clients).unwrap_or(Value::Array(Vec::new())),
        );
        map.insert("roles".into(), serde_json::json!({ "realm": roles }));
        map.insert(
            "groups".into(),
            serde_json::to_value(&groups).unwrap_or(Value::Array(Vec::new())),
        );
        map.insert(
            "identityProviders".into(),
            serde_json::to_value(&idps).unwrap_or(Value::Array(Vec::new())),
        );
        if with_users {
            let users = store
                .list_users(realm_id, None, None, None, None, false, 0, 10_000)
                .await?;
            map.insert(
                "users".into(),
                serde_json::to_value(&users).unwrap_or(Value::Array(Vec::new())),
            );
        }
    }
    let _ = HashMap::<String, String>::new();
    Ok(out)
}
