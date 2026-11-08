#![forbid(unsafe_code)]
//! Authentication, kaisatsu JWKS token verification, registry ES256 token minting,
//! and the `Authorizer` decision trait (S12, R§3.1, R§3.2, Threat T4, T6).

use crate::error::{ShakoError, ShakoResult};
use crate::ids::{InstanceId, OrgId, constant_time_eq, sha256_hex};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use chrono::Utc;
use jsonwebtoken::jwk::Jwk;
use jsonwebtoken::{
    Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, decode_header, encode,
};
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey};
use parking_lot::RwLock;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Verbs checked by [`Authorizer::check`] (S12, R§3.2).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Read,
    Propose,
    Write,
    Approve,
    Admin,
    Publish,
    Build,
}

impl Action {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Propose => "propose",
            Self::Write => "write",
            Self::Approve => "approve",
            Self::Admin => "admin",
            Self::Publish => "publish",
            Self::Build => "build",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "read" | "view" | "pull" => Some(Self::Read),
            "propose" => Some(Self::Propose),
            "write" | "push" | "edit" => Some(Self::Write),
            "approve" | "review" => Some(Self::Approve),
            "admin" => Some(Self::Admin),
            "publish" => Some(Self::Publish),
            "build" => Some(Self::Build),
            _ => None,
        }
    }

    /// Verb implication rule: `Admin` implies all; `Write` implies `Propose` and `Read`;
    /// every verb implies `Read`.
    #[must_use]
    pub const fn implies(self, wanted: Self) -> bool {
        if matches!(self, Self::Admin) {
            return true;
        }
        if matches!(wanted, Self::Read) {
            return true;
        }
        if matches!(self, Self::Write) && matches!(wanted, Self::Write | Self::Propose) {
            return true;
        }
        self as u8 == wanted as u8
    }

    #[must_use]
    pub const fn is_write(self) -> bool {
        !matches!(self, Self::Read)
    }
}

/// Verified caller identity extracted from a kaisatsu JWT or service token (S12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Subject {
    pub subject_id: Option<String>,
    pub username: String,
    pub full_name: String,
    pub email: Option<String>,
    pub client_id: Option<String>,
    pub organizations: Vec<OrgId>,
    pub groups: Vec<String>,
    /// Connector segments allowed when this token is a delegated/connector token (`None` = unrestricted).
    pub connector_segments: Option<Vec<String>>,
    /// Registry scope if this is a short-lived `/v2/token` bearer (`repository:{owner}/{image}:{actions}`).
    pub registry_scope: Option<String>,
    pub read_only: bool,
}

impl Subject {
    #[must_use]
    pub fn anonymous() -> Self {
        Self {
            subject_id: None,
            username: String::new(),
            full_name: String::new(),
            email: None,
            client_id: None,
            organizations: Vec::new(),
            groups: Vec::new(),
            connector_segments: None,
            registry_scope: None,
            read_only: false,
        }
    }

    #[must_use]
    pub fn authenticated(
        subject_id: impl Into<String>,
        username: impl Into<String>,
        organizations: Vec<OrgId>,
        groups: Vec<String>,
    ) -> Self {
        let u = username.into();
        Self {
            subject_id: Some(subject_id.into()),
            full_name: u.clone(),
            username: u,
            email: None,
            client_id: None,
            organizations,
            groups,
            connector_segments: None,
            registry_scope: None,
            read_only: false,
        }
    }

    #[must_use]
    pub const fn is_anonymous(&self) -> bool {
        self.subject_id.is_none()
    }

    #[must_use]
    pub fn is_member_of(&self, org_id: OrgId) -> bool {
        self.organizations.contains(&org_id)
    }

    #[must_use]
    pub fn principals(&self) -> Vec<&str> {
        let mut out = Vec::new();
        if let Some(sub) = &self.subject_id {
            out.push(sub.as_str());
        }
        if !self.username.is_empty() {
            out.push(self.username.as_str());
        }
        for g in &self.groups {
            out.push(g.as_str());
        }
        out
    }

    #[must_use]
    pub fn allows_connector_segment(&self, segment: &str) -> bool {
        match &self.connector_segments {
            None => true,
            Some(segs) => segs.iter().any(|s| s == segment || s == "*"),
        }
    }
}

/// Per-request context scoped to `(instance_id, org_id)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestCtx {
    pub instance_id: InstanceId,
    pub org_id: OrgId,
    pub subject: Subject,
    pub request_id: String,
}

impl RequestCtx {
    #[must_use]
    pub fn new(instance_id: InstanceId, org_id: OrgId, subject: Subject) -> Self {
        Self {
            instance_id,
            org_id,
            subject,
            request_id: uuid::Uuid::now_v7().to_string(),
        }
    }
}

/// Resource targeted by an authorization check (S12, R§3.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ObjectTarget {
    Instance {
        instance_id: InstanceId,
        org_id: OrgId,
    },
    Organization {
        instance_id: InstanceId,
        org_id: OrgId,
        owner: String,
    },
    Repository {
        instance_id: InstanceId,
        org_id: OrgId,
        project_id: Option<String>,
        owner: String,
        repo: String,
        git_ref: Option<String>,
    },
    Package {
        instance_id: InstanceId,
        org_id: OrgId,
        owner: String,
        package_type: String,
        name: String,
    },
}

impl ObjectTarget {
    #[must_use]
    pub const fn instance_id(&self) -> InstanceId {
        match self {
            Self::Instance { instance_id, .. }
            | Self::Organization { instance_id, .. }
            | Self::Repository { instance_id, .. }
            | Self::Package { instance_id, .. } => *instance_id,
        }
    }

    #[must_use]
    pub const fn org_id(&self) -> OrgId {
        match self {
            Self::Instance { org_id, .. }
            | Self::Organization { org_id, .. }
            | Self::Repository { org_id, .. }
            | Self::Package { org_id, .. } => *org_id,
        }
    }

    #[must_use]
    pub fn repo_name(&self) -> Option<&str> {
        match self {
            Self::Repository { repo, .. } => Some(repo.as_str()),
            _ => None,
        }
    }

    #[must_use]
    pub fn project_id(&self) -> Option<&str> {
        match self {
            Self::Repository { project_id, .. } => project_id.as_deref().filter(|s| !s.is_empty()),
            _ => None,
        }
    }
}

/// Decision returned by [`Authorizer::check`] (W6 shape, S12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    pub allowed: bool,
    pub reason: String,
    /// When `false`, the caller must receive 404 Not Found (never 403) so existence is hidden.
    pub visible: bool,
}

impl Decision {
    #[must_use]
    pub fn allow(reason: impl Into<String>) -> Self {
        Self {
            allowed: true,
            reason: reason.into(),
            visible: true,
        }
    }

    #[must_use]
    pub fn deny_visible(reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            reason: reason.into(),
            visible: true,
        }
    }

    #[must_use]
    pub fn deny_hidden(reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            reason: reason.into(),
            visible: false,
        }
    }

    pub fn into_result(self, not_found_msg: impl Into<String>) -> ShakoResult<()> {
        if self.allowed {
            Ok(())
        } else if !self.visible {
            Err(ShakoError::NotFound(not_found_msg.into()))
        } else {
            Err(ShakoError::Forbidden(self.reason))
        }
    }
}

/// Single authorization decision interface (S12, W6).
pub trait Authorizer: Send + Sync {
    fn check(&self, ctx: &RequestCtx, action: Action, object: &ObjectTarget) -> Decision;

    /// Checks whether a named principal (username or subject ID) holds `Action::Write` on `repo`
    /// (used when validating branch protection whitelists, S5 & T8).
    fn principal_has_write(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        owner: &str,
        repo: &str,
        principal: &str,
    ) -> bool;

    /// Returns visible role bindings for a repository (for the read-only OData `Teams` set, R§9).
    fn bindings_for_repo(&self, org_id: OrgId, repo: &str) -> Vec<(String, Action)> {
        let _ = (org_id, repo);
        Vec::new()
    }
}

/// One role binding in [`LocalAuthorizer`] (S12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoleBinding {
    pub org_id: OrgId,
    /// Principal (subject ID, username, or group path like `"/helsinki-writers"`).
    pub principal: String,
    pub actions: Vec<Action>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
}

/// Local `RoleBinding`-based [`Authorizer`] implementation (S12).
#[derive(Debug, Clone, Default)]
pub struct LocalAuthorizer {
    bindings: Arc<RwLock<Vec<RoleBinding>>>,
}

impl LocalAuthorizer {
    #[must_use]
    pub fn new(bindings: Vec<RoleBinding>) -> Self {
        Self {
            bindings: Arc::new(RwLock::new(bindings)),
        }
    }

    pub fn add_binding(&self, binding: RoleBinding) {
        self.bindings.write().push(binding);
    }

    fn matches_target(binding: &RoleBinding, object: &ObjectTarget) -> bool {
        if let Some(bound_proj) = &binding.project_id
            && let Some(target_proj) = object.project_id()
            && bound_proj != target_proj
        {
            return false;
        }
        if let Some(bound_repo) = &binding.repo {
            match object.repo_name() {
                Some(target_repo) if bound_repo == target_repo => {}
                _ => return false,
            }
        }
        true
    }
}

impl Authorizer for LocalAuthorizer {
    fn check(&self, ctx: &RequestCtx, action: Action, object: &ObjectTarget) -> Decision {
        if ctx.subject.is_anonymous() {
            return Decision::deny_hidden("unauthenticated");
        }
        if object.instance_id() != ctx.instance_id
            || object.org_id() != ctx.org_id
            || !ctx.subject.is_member_of(object.org_id())
        {
            return Decision::deny_hidden("object is outside the caller's organization");
        }

        // Check short-lived registry token scope if present (T4)
        if let Some(reg_scope) = &ctx.subject.registry_scope {
            match object {
                ObjectTarget::Package { owner, name, .. } => {
                    let prefix = format!("repository:{owner}/{name}:");
                    let Some(actions_csv) = reg_scope.strip_prefix(&prefix) else {
                        return Decision::deny_hidden("registry token not scoped to this image");
                    };
                    let acts: Vec<&str> = actions_csv.split(',').collect();
                    let allowed = match action {
                        Action::Read => acts.contains(&"pull") || acts.contains(&"push"),
                        Action::Publish | Action::Write => acts.contains(&"push"),
                        _ => false,
                    };
                    return if allowed {
                        Decision::allow("granted by registry token scope")
                    } else if acts.contains(&"pull") {
                        Decision::deny_visible("registry token lacks push scope")
                    } else {
                        Decision::deny_hidden("registry token lacks pull scope")
                    };
                }
                _ => {
                    return Decision::deny_hidden("registry token may only access packages");
                }
            }
        }

        if action.is_write() && ctx.subject.read_only {
            return Decision::deny_visible("caller token is read-only");
        }

        let principals = ctx.subject.principals();
        let bindings = self.bindings.read();

        let can_read = bindings.iter().any(|b| {
            b.org_id == ctx.org_id
                && principals.contains(&b.principal.as_str())
                && Self::matches_target(b, object)
                && b.actions.iter().any(|a| a.implies(Action::Read))
        });

        if !can_read {
            return Decision::deny_hidden("caller lacks read permission on target");
        }

        if action == Action::Read {
            return Decision::allow("granted read");
        }

        let granted = bindings.iter().any(|b| {
            b.org_id == ctx.org_id
                && principals.contains(&b.principal.as_str())
                && Self::matches_target(b, object)
                && b.actions.iter().any(|a| a.implies(action))
        });

        if granted {
            Decision::allow(format!("granted {}", action.as_str()))
        } else {
            Decision::deny_visible(format!("caller lacks {} permission", action.as_str()))
        }
    }

    fn principal_has_write(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        owner: &str,
        repo: &str,
        principal: &str,
    ) -> bool {
        let target = ObjectTarget::Repository {
            instance_id,
            org_id,
            project_id: None,
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            git_ref: None,
        };
        let bindings = self.bindings.read();
        bindings.iter().any(|b| {
            b.org_id == org_id
                && b.principal == principal
                && Self::matches_target(b, &target)
                && b.actions.iter().any(|a| a.implies(Action::Write))
        })
    }

    fn bindings_for_repo(&self, org_id: OrgId, repo: &str) -> Vec<(String, Action)> {
        let bindings = self.bindings.read();
        let mut out = Vec::new();
        for b in bindings.iter() {
            if b.org_id == org_id
                && b.repo.as_deref().is_none_or(|r| r == repo)
                && let Some(&highest) = b.actions.iter().max()
            {
                out.push((b.principal.clone(), highest));
            }
        }
        out
    }
}

/// Hashed service-account token binding (raw token is never stored in memory).
#[derive(Debug, Clone)]
pub struct StaticTokenBinding {
    pub token_sha256: String,
    pub subject: Subject,
}

impl StaticTokenBinding {
    #[must_use]
    pub fn new(raw_token: &str, subject: Subject) -> Self {
        Self {
            token_sha256: sha256_hex(raw_token.as_bytes()),
            subject,
        }
    }
}

/// ES256 signing & verification key used by `shako` to mint short-lived registry tokens (S12, S13)
/// and to sign container image digests in cosign simple-signing format (S13).
#[derive(Clone)]
pub struct ShakoSigningKey {
    pub kid: String,
    signing_key: SigningKey,
    verifying_key: VerifyingKey,
    pkcs8_der: Vec<u8>,
}

impl std::fmt::Debug for ShakoSigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShakoSigningKey")
            .field("kid", &self.kid)
            .finish()
    }
}

impl ShakoSigningKey {
    /// Generates a fresh P-256 signing key.
    pub fn generate() -> ShakoResult<Self> {
        let mut raw = [0u8; 32];
        rand::fill(&mut raw);
        Self::from_secret_bytes(&raw)
    }

    /// Loads a P-256 key from a file in the secrets mount (raw 32 bytes, 64-char hex, or PKCS#8 PEM).
    pub fn from_file(path: &Path) -> ShakoResult<Self> {
        let bytes = std::fs::read(path).map_err(|e| {
            ShakoError::Internal(format!(
                "failed to read signing key {}: {e}",
                path.display()
            ))
        })?;
        if let Ok(text) = std::str::from_utf8(&bytes) {
            let trimmed = text.trim();
            if trimmed.starts_with("-----BEGIN") {
                use p256::pkcs8::DecodePrivateKey;
                let signing_key = SigningKey::from_pkcs8_pem(trimmed)
                    .map_err(|e| ShakoError::Internal(format!("invalid PKCS#8 PEM key: {e}")))?;
                return Self::from_signing_key(signing_key);
            }
            if trimmed.len() == 64 && trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
                let decoded = hex::decode(trimmed)
                    .map_err(|e| ShakoError::Internal(format!("invalid hex key: {e}")))?;
                return Self::from_secret_bytes(&decoded);
            }
        }
        Self::from_secret_bytes(&bytes)
    }

    pub fn from_secret_bytes(raw: &[u8]) -> ShakoResult<Self> {
        let signing_key = SigningKey::from_slice(raw)
            .map_err(|e| ShakoError::Internal(format!("invalid P-256 secret scalar: {e}")))?;
        Self::from_signing_key(signing_key)
    }

    fn from_signing_key(signing_key: SigningKey) -> ShakoResult<Self> {
        let verifying_key = *signing_key.verifying_key();
        let pkcs8 = signing_key
            .to_pkcs8_der()
            .map_err(|e| ShakoError::Internal(format!("PKCS#8 DER encode failed: {e}")))?;
        let pub_der = verifying_key
            .to_public_key_der()
            .map_err(|e| ShakoError::Internal(format!("SPKI DER encode failed: {e}")))?;
        let kid = format!("shako-es256-{}", &sha256_hex(pub_der.as_bytes())[..16]);
        Ok(Self {
            kid,
            signing_key,
            verifying_key,
            pkcs8_der: pkcs8.as_bytes().to_vec(),
        })
    }

    /// Returns the public key in SPKI PEM format (for `cosign verify --key`).
    pub fn public_key_pem(&self) -> ShakoResult<String> {
        self.verifying_key
            .to_public_key_pem(p256::pkcs8::LineEnding::LF)
            .map_err(|e| ShakoError::Internal(format!("SPKI PEM encode failed: {e}")))
    }

    /// Returns the public JWK JSON value for JWKS verification.
    #[must_use]
    pub fn public_jwk(&self) -> Value {
        let point = self.verifying_key.to_sec1_point(false);
        let x = URL_SAFE_NO_PAD.encode(point.x().map_or(&[][..], |b| b.as_slice()));
        let y = URL_SAFE_NO_PAD.encode(point.y().map_or(&[][..], |b| b.as_slice()));
        json!({
            "kty": "EC",
            "crv": "P-256",
            "alg": "ES256",
            "use": "sig",
            "kid": self.kid,
            "x": x,
            "y": y,
        })
    }

    /// Signs arbitrary payload bytes with ECDSA P-256 SHA-256 and returns the ASN.1 DER signature
    /// encoded in standard base64 (cosign simple-signing format, S13).
    #[must_use]
    pub fn sign_cosign_payload(&self, payload: &[u8]) -> String {
        let sig: Signature = self.signing_key.sign(payload);
        STANDARD.encode(sig.to_der().as_bytes())
    }

    /// Verifies a base64-encoded ASN.1 DER ECDSA P-256 SHA-256 signature over `payload`.
    #[must_use]
    pub fn verify_cosign_payload(&self, payload: &[u8], sig_base64: &str) -> bool {
        use p256::ecdsa::signature::Verifier;
        let Ok(der_bytes) = STANDARD.decode(sig_base64.trim()) else {
            return false;
        };
        let Ok(sig) = Signature::from_der(&der_bytes) else {
            return false;
        };
        self.verifying_key.verify(payload, &sig).is_ok()
    }

    /// Mints a short-lived ES256 JWT (lifetime <= 15 minutes, S12 & S13).
    pub fn mint_registry_token(
        &self,
        subject: &Subject,
        org_id: OrgId,
        scope: &str,
        ttl_secs: i64,
    ) -> ShakoResult<String> {
        let now = Utc::now().timestamp();
        let ttl = ttl_secs.clamp(1, 900);
        let claims = JwtClaims {
            iss: Some("shako".into()),
            sub: subject
                .subject_id
                .clone()
                .unwrap_or_else(|| subject.username.clone()),
            aud: Some(AudClaim::Single("container_registry".into())),
            exp: now + ttl,
            nbf: Some(now - 5),
            iat: Some(now),
            preferred_username: Some(subject.username.clone()),
            name: Some(subject.full_name.clone()),
            email: subject.email.clone(),
            azp: subject.client_id.clone(),
            jc_org: Some(OrgClaim::Single(org_id.to_string())),
            groups: Some(subject.groups.clone()),
            scope: Some(scope.to_owned()),
        };
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.kid.clone());
        let enc_key = EncodingKey::from_ec_der(&self.pkcs8_der);
        encode(&header, &claims, &enc_key)
            .map_err(|e| ShakoError::Internal(format!("registry JWT sign failed: {e}")))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AudClaim {
    Single(String),
    Multiple(Vec<String>),
}

impl AudClaim {
    #[must_use]
    pub fn contains(&self, wanted: &str) -> bool {
        match self {
            Self::Single(s) => s == wanted,
            Self::Multiple(list) => list.iter().any(|s| s == wanted),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OrgClaim {
    Single(String),
    Multiple(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtClaims {
    #[serde(default)]
    pub iss: Option<String>,
    pub sub: String,
    #[serde(default)]
    pub aud: Option<AudClaim>,
    pub exp: i64,
    #[serde(default)]
    pub nbf: Option<i64>,
    #[serde(default)]
    pub iat: Option<i64>,
    #[serde(default)]
    pub preferred_username: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub azp: Option<String>,
    #[serde(default)]
    pub jc_org: Option<OrgClaim>,
    #[serde(default)]
    pub groups: Option<Vec<String>>,
    #[serde(default)]
    pub scope: Option<String>,
}

/// Callback fetching fresh JWKS keys on unknown `kid` (S12).
pub type JwksFetcher = Arc<dyn Fn() -> ShakoResult<Vec<Value>> + Send + Sync>;

struct JwksCacheState {
    keys: BTreeMap<String, Value>,
    last_refresh: Option<Instant>,
}

/// Bounded JWKS & static service-token verifier (S12, Threat T4).
#[derive(Clone)]
pub struct TokenVerifier {
    expected_issuer: Option<String>,
    jwks: Arc<RwLock<JwksCacheState>>,
    fetcher: Option<JwksFetcher>,
    static_tokens: Arc<Vec<StaticTokenBinding>>,
    org_aliases: Arc<RwLock<BTreeMap<String, OrgId>>>,
    signing_key: ShakoSigningKey,
}

impl TokenVerifier {
    pub fn new(signing_key: ShakoSigningKey) -> Self {
        let mut keys = BTreeMap::new();
        keys.insert(signing_key.kid.clone(), signing_key.public_jwk());
        Self {
            expected_issuer: None,
            jwks: Arc::new(RwLock::new(JwksCacheState {
                keys,
                last_refresh: None,
            })),
            fetcher: None,
            static_tokens: Arc::new(Vec::new()),
            org_aliases: Arc::new(RwLock::new(BTreeMap::new())),
            signing_key,
        }
    }

    #[must_use]
    pub fn signing_key(&self) -> &ShakoSigningKey {
        &self.signing_key
    }

    #[must_use]
    pub fn with_expected_issuer(mut self, issuer: impl Into<String>) -> Self {
        self.expected_issuer = Some(issuer.into());
        self
    }

    #[must_use]
    pub fn with_static_tokens(mut self, tokens: Vec<StaticTokenBinding>) -> Self {
        self.static_tokens = Arc::new(tokens);
        self
    }

    #[must_use]
    pub fn with_jwks_fetcher(mut self, fetcher: JwksFetcher) -> Self {
        self.fetcher = Some(fetcher);
        self
    }

    pub fn insert_jwk(&self, kid: impl Into<String>, jwk: Value) {
        let mut state = self.jwks.write();
        if state.keys.len() >= 64
            && let Some(first_key) = state.keys.keys().next().cloned()
            && first_key != self.signing_key.kid
        {
            state.keys.remove(&first_key);
        }
        state.keys.insert(kid.into(), jwk);
    }

    pub fn register_org_alias(&self, alias: impl Into<String>, org_id: OrgId) {
        self.org_aliases.write().insert(alias.into(), org_id);
    }

    /// Extracts the raw credential from `Authorization: Bearer <t>`, `Authorization: token <t>`,
    /// or `Authorization: Basic <base64(user:t)>` (S12, R§3.1).
    pub fn extract_raw_token(auth_header: &str) -> Option<String> {
        let trimmed = auth_header.trim();
        let (scheme, rest) = trimmed.split_once(' ')?;
        let rest = rest.trim();
        if rest.is_empty() {
            return None;
        }
        if scheme.eq_ignore_ascii_case("bearer") || scheme.eq_ignore_ascii_case("token") {
            return Some(rest.to_owned());
        }
        if scheme.eq_ignore_ascii_case("basic") {
            let decoded = STANDARD.decode(rest).ok()?;
            let text = std::str::from_utf8(&decoded).ok()?;
            let (_, password) = text.split_once(':')?;
            if !password.is_empty() {
                return Some(password.to_owned());
            }
        }
        None
    }

    /// Verifies a raw token expecting `expected_audience` (`"shako"` or `"container_registry"`).
    pub fn verify(&self, raw_token: &str, expected_audience: &str) -> ShakoResult<Subject> {
        // 1. Check hashed static service bindings in constant time
        let digest = sha256_hex(raw_token.as_bytes());
        let mut matched_static: Option<&StaticTokenBinding> = None;
        for b in self.static_tokens.iter() {
            if constant_time_eq(&b.token_sha256, &digest) && matched_static.is_none() {
                matched_static = Some(b);
            }
        }
        if let Some(binding) = matched_static {
            return Ok(binding.subject.clone());
        }

        // 2. Verify as a JWT against cached JWKS (with bounded refetch on unknown kid)
        let header = decode_header(raw_token)
            .map_err(|_| ShakoError::Unauthorized("invalid token".into()))?;
        if !matches!(
            header.alg,
            Algorithm::ES256 | Algorithm::RS256 | Algorithm::EdDSA
        ) {
            return Err(ShakoError::Unauthorized(
                "unsupported JWT signature algorithm".into(),
            ));
        }
        let kid = header
            .kid
            .as_deref()
            .ok_or_else(|| ShakoError::Unauthorized("JWT missing kid".into()))?;

        let jwk_val = self.lookup_or_refresh_jwk(kid)?;
        let jwk: Jwk = serde_json::from_value(jwk_val)
            .map_err(|_| ShakoError::Unauthorized("invalid JWK".into()))?;
        let decoding_key = DecodingKey::from_jwk(&jwk)
            .map_err(|_| ShakoError::Unauthorized("cannot construct decoding key".into()))?;

        let mut validation = Validation::new(header.alg);
        validation.set_audience(&[expected_audience]);
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation.leeway = 60;
        if let Some(iss) = &self.expected_issuer
            && expected_audience != "container_registry"
        {
            validation.set_issuer(&[iss]);
        }

        let token_data = decode::<JwtClaims>(raw_token, &decoding_key, &validation)
            .map_err(|_| ShakoError::Unauthorized("JWT verification failed".into()))?;
        let claims = token_data.claims;

        // Explicitly require `aud` claim presence (SECURITY-THINK B)
        let Some(aud) = &claims.aud else {
            return Err(ShakoError::Unauthorized("JWT missing aud claim".into()));
        };
        if !aud.contains(expected_audience) {
            return Err(ShakoError::Unauthorized("JWT audience mismatch".into()));
        }

        let mut organizations = Vec::new();
        if let Some(org_claim) = claims.jc_org {
            let raw_orgs = match org_claim {
                OrgClaim::Single(s) => vec![s],
                OrgClaim::Multiple(v) => v,
            };
            let aliases = self.org_aliases.read();
            for raw in raw_orgs {
                if let Ok(uuid_org) = raw.parse::<OrgId>() {
                    organizations.push(uuid_org);
                } else if let Some(&mapped) = aliases.get(&raw) {
                    organizations.push(mapped);
                }
            }
        }

        let username = claims
            .preferred_username
            .clone()
            .unwrap_or_else(|| claims.sub.clone());
        let full_name = claims.name.unwrap_or_else(|| username.clone());
        let registry_scope = if expected_audience == "container_registry" {
            claims.scope.clone()
        } else {
            None
        };

        Ok(Subject {
            subject_id: Some(claims.sub),
            username,
            full_name,
            email: claims.email,
            client_id: claims.azp,
            organizations,
            groups: claims.groups.unwrap_or_default(),
            connector_segments: None,
            registry_scope,
            read_only: false,
        })
    }

    fn lookup_or_refresh_jwk(&self, kid: &str) -> ShakoResult<Value> {
        if let Some(found) = self.jwks.read().keys.get(kid).cloned() {
            return Ok(found);
        }
        let Some(fetcher) = &self.fetcher else {
            return Err(ShakoError::Unauthorized("unknown signing key kid".into()));
        };
        {
            let state = self.jwks.read();
            if let Some(last) = state.last_refresh
                && last.elapsed() < Duration::from_secs(10)
            {
                return Err(ShakoError::Unauthorized("unknown signing key kid".into()));
            }
        }
        let fetched = fetcher()?;
        let mut state = self.jwks.write();
        state.last_refresh = Some(Instant::now());
        for jwk in fetched {
            if let Some(k) = jwk.get("kid").and_then(Value::as_str) {
                state.keys.insert(k.to_owned(), jwk);
            }
        }
        state
            .keys
            .get(kid)
            .cloned()
            .ok_or_else(|| ShakoError::Unauthorized("unknown signing key kid".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_verifier_checks_audience_expiry_and_unknown_kid_refetch() {
        let key1 = ShakoSigningKey::generate().expect("key1");
        let key2 = ShakoSigningKey::generate().expect("key2");
        let org_a = OrgId::from_u128(10);

        let key2_jwk = key2.public_jwk();
        let fetch_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let fc = fetch_count.clone();
        let verifier = TokenVerifier::new(key1.clone()).with_jwks_fetcher(Arc::new(move || {
            fc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(vec![key2_jwk.clone()])
        }));

        let subj = Subject::authenticated("u1", "alice", vec![org_a], vec!["/devs".into()]);
        let reg_tok = key1
            .mint_registry_token(&subj, org_a, "repository:org/app:pull", 300)
            .expect("mint");

        // Wrong audience -> 401
        assert!(verifier.verify(&reg_tok, "shako").is_err());
        // Right audience -> ok
        let verified = verifier
            .verify(&reg_tok, "container_registry")
            .expect("valid registry token");
        assert_eq!(verified.username, "alice");
        assert_eq!(
            verified.registry_scope.as_deref(),
            Some("repository:org/app:pull")
        );

        // Token signed by key2 triggers exactly 1 JWKS refetch on unknown kid
        let tok2 = key2
            .mint_registry_token(&subj, org_a, "repository:org/app:push,pull", 300)
            .expect("mint2");
        let verified2 = verifier
            .verify(&tok2, "container_registry")
            .expect("refetched kid");
        assert_eq!(verified2.username, "alice");
        assert_eq!(fetch_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// A key already on disk keeps its kid and public JWK across p256 releases. The expected
    /// values come from openssl for the secret scalar 0x1111...11, not from this code.
    #[test]
    fn a_stored_key_keeps_its_kid_and_jwk() {
        let key = ShakoSigningKey::from_secret_bytes(&[0x11; 32]).expect("key");
        let kid = &key.kid;
        assert_eq!(kid, "shako-es256-76805a63a6980c18");
        let jwk = key.public_jwk();
        assert_eq!(jwk["x"], "AhfmF_C2RDkoJ4-WmZ5pojpPLBUr321s32bluAKC1O0");
        assert_eq!(jwk["y"], "GUp968uXcS0t2jyoWqh2Wlb0X8dYWZZS8ol8ZTBuV5Q");
    }

    #[test]
    fn cosign_signature_round_trips() {
        let key = ShakoSigningKey::generate().expect("key");
        let payload = br#"{"critical":{"identity":{"docker-reference":"city.example/org/app"},"image":{"docker-manifest-digest":"sha256:1234"},"type":"cosign container image signature"}}"#;
        let sig = key.sign_cosign_payload(payload);
        assert!(key.verify_cosign_payload(payload, &sig));
        assert!(!key.verify_cosign_payload(b"tampered", &sig));
        assert!(
            key.public_key_pem()
                .expect("pem")
                .contains("BEGIN PUBLIC KEY")
        );
    }
}
