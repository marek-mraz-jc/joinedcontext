#![forbid(unsafe_code)]
//! Authentication, kaisatsu JWKS token verification, registry ES256 token minting,
//! and the `Authorizer` decision trait (S12, R§3.1, R§3.2, Threat T4, T6).

use crate::error::{ShakoError, ShakoResult};
use crate::ids::{InstanceId, OrgId, constant_time_eq, sha256_hex};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use jc_time::TimestampTz;
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
use std::collections::{BTreeMap, BTreeSet};
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
    /// The only identity of a caller: bindings, allow-lists and authorship key on it.
    pub subject_id: Option<String>,
    /// Display text, renameable: never compared to decide a right.
    pub username: String,
    pub full_name: String,
    pub email: Option<String>,
    pub client_id: Option<String>,
    pub organizations: Vec<OrgId>,
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
    ) -> Self {
        let u = username.into();
        Self {
            subject_id: Some(subject_id.into()),
            full_name: u.clone(),
            username: u,
            email: None,
            client_id: None,
            organizations,
            connector_segments: None,
            registry_scope: None,
            read_only: false,
        }
    }

    /// No subject id, or an empty one, is nobody.
    #[must_use]
    pub fn is_anonymous(&self) -> bool {
        self.login().is_empty()
    }

    /// The Organization the caller acts in: the one its token names. A token naming none or
    /// several acts in none (TEN-04).
    #[must_use]
    pub fn organization(&self) -> Option<OrgId> {
        match self.organizations.as_slice() {
            [only] => Some(*only),
            _ => None,
        }
    }

    /// The caller's account name in the forge: the subject id.
    #[must_use]
    pub fn login(&self) -> &str {
        self.subject_id.as_deref().unwrap_or_default()
    }

    /// Whether an allow-list of subject ids names this caller.
    #[must_use]
    pub fn is_named_in(&self, subject_ids: &[String]) -> bool {
        !self.is_anonymous() && subject_ids.iter().any(|id| id == self.login())
    }

    #[must_use]
    pub fn is_member_of(&self, org_id: OrgId) -> bool {
        self.organizations.contains(&org_id)
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

    /// Checks whether the subject with this id holds `Action::Write` on `repo`
    /// (used when validating branch protection whitelists, S5 & T8).
    fn principal_has_write(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        owner: &str,
        repo: &str,
        subject_id: &str,
    ) -> bool;

    /// Returns the group bindings of a repository (for the read-only OData `Teams` set, R§9).
    fn bindings_for_repo(&self, org_id: OrgId, repo: &str) -> Vec<(String, Action)> {
        let _ = (org_id, repo);
        Vec::new()
    }
}

/// Who a binding names. The kinds never stand in for one another (AUZ-12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Principal {
    /// A subject id, never a username.
    User(String),
    /// A group id of the binding's Organization.
    Group(String),
}

/// A membership row: `subject_id` is in group `group_id` of `org_id`. Groups come from these
/// rows alone, never from a token (IDN-03).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupMember {
    pub org_id: OrgId,
    pub group_id: String,
    pub subject_id: String,
}

/// One role binding in [`LocalAuthorizer`] (S12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoleBinding {
    pub org_id: OrgId,
    pub principal: Principal,
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
    members: Arc<BTreeSet<GroupMember>>,
}

impl LocalAuthorizer {
    #[must_use]
    pub fn new(bindings: Vec<RoleBinding>) -> Self {
        Self {
            bindings: Arc::new(RwLock::new(bindings)),
            members: Arc::default(),
        }
    }

    #[must_use]
    pub fn with_members(mut self, members: Vec<GroupMember>) -> Self {
        self.members = Arc::new(members.into_iter().collect());
        self
    }

    fn names(&self, binding: &RoleBinding, org_id: OrgId, subject_id: &str) -> bool {
        if binding.org_id != org_id || subject_id.is_empty() {
            return false;
        }
        match &binding.principal {
            Principal::User(id) => id == subject_id,
            Principal::Group(group_id) => self.members.contains(&GroupMember {
                org_id,
                group_id: group_id.clone(),
                subject_id: subject_id.to_owned(),
            }),
        }
    }

    pub fn add_binding(&self, binding: RoleBinding) {
        self.bindings.write().push(binding);
    }

    fn matches_target(binding: &RoleBinding, object: &ObjectTarget) -> bool {
        // A project's binding grants on that project's repositories alone: an instance,
        // organization or package target, or a repository of no project, names none.
        if let Some(bound_proj) = &binding.project_id
            && object.project_id() != Some(bound_proj.as_str())
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
                    let Some(actions_csv) = reg_scope
                        .split(' ')
                        .find_map(|entry| entry.strip_prefix(prefix.as_str()))
                    else {
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

        let caller = ctx.subject.login();
        let bindings = self.bindings.read();

        let can_read = bindings.iter().any(|b| {
            self.names(b, ctx.org_id, caller)
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
            self.names(b, ctx.org_id, caller)
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
        subject_id: &str,
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
            self.names(b, org_id, subject_id)
                && Self::matches_target(b, &target)
                && b.actions.iter().any(|a| a.implies(Action::Write))
        })
    }

    fn bindings_for_repo(&self, org_id: OrgId, repo: &str) -> Vec<(String, Action)> {
        let bindings = self.bindings.read();
        let mut out = Vec::new();
        for b in bindings.iter() {
            if b.org_id == org_id
                && let Principal::Group(group_id) = &b.principal
                && b.repo.as_deref().is_none_or(|r| r == repo)
                && let Some(&highest) = b.actions.iter().max()
            {
                out.push((group_id.clone(), highest));
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
        Self::from_text(&bytes)
    }

    /// Parses a P-256 key as [`ShakoSigningKey::from_file`] reads one (raw 32 bytes, 64-char hex,
    /// or PKCS#8 PEM), from the bytes a host already holds.
    pub fn from_text(bytes: &[u8]) -> ShakoResult<Self> {
        if let Ok(text) = std::str::from_utf8(bytes) {
            let trimmed = text.trim();
            if trimmed.starts_with("-----BEGIN") {
                use p256::pkcs8::DecodePrivateKey;
                let signing_key = SigningKey::from_pkcs8_pem(trimmed)
                    .map_err(|e| ShakoError::Internal(format!("invalid PKCS#8 PEM key: {e}")))?;
                return Self::from_signing_key(signing_key);
            }
            if trimmed.len() == 64
                && let Ok(decoded) = hex::decode(trimmed)
            {
                return Self::from_secret_bytes(&decoded);
            }
        }
        Self::from_secret_bytes(bytes)
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
        if subject.is_anonymous() {
            return Err(ShakoError::Unauthorized("token has no subject".into()));
        }
        let now = TimestampTz::now().as_second();
        let ttl = ttl_secs.clamp(1, 900);
        let claims = JwtClaims {
            iss: Some("shako".into()),
            sub: subject.login().to_owned(),
            aud: Some(AudClaim::Single("container_registry".into())),
            exp: now + ttl,
            nbf: Some(now - 5),
            iat: Some(now),
            preferred_username: Some(subject.username.clone()),
            name: Some(subject.full_name.clone()),
            email: subject.email.clone(),
            azp: subject.client_id.clone(),
            jc_org: Some(OrgClaim::Single(org_id.to_string())),
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

    /// Extracts the raw credential from `Authorization: Bearer <t>`, `Authorization: token <t>`,
    /// or `Authorization: Basic <base64(user:t)>` (S12, R§3.1).
    pub fn extract_raw_token(auth_header: &str) -> Option<String> {
        let trimmed = auth_header.trim();
        let (scheme, rest) = trimmed.split_once(' ')?;
        // `trimmed` ends in a non-space, so `rest` is never empty.
        let rest = rest.trim();
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

        // A registry bearer's scope stands in for the bindings, so only shako itself issues one.
        if expected_audience == "container_registry" && kid != self.signing_key.kid {
            return Err(ShakoError::Unauthorized(
                "a registry token is issued by shako alone".into(),
            ));
        }
        let jwk_val = self.lookup_or_refresh_jwk(kid)?;
        let jwk: Jwk = serde_json::from_value(jwk_val)
            .map_err(|_| ShakoError::Unauthorized("invalid JWK".into()))?;
        let decoding_key = DecodingKey::from_jwk(&jwk)
            .map_err(|_| ShakoError::Unauthorized("cannot construct decoding key".into()))?;

        let mut validation = Validation::new(header.alg);
        // The audience is checked once, after the signature, below: a token without `aud` and
        // one for another audience are each refused by name there.
        validation.validate_aud = false;
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

        if claims.sub.is_empty() {
            return Err(ShakoError::Unauthorized("JWT names no subject".into()));
        }

        let mut organizations = Vec::new();
        if let Some(org_claim) = claims.jc_org {
            let raw_orgs = match org_claim {
                OrgClaim::Single(s) => vec![s],
                OrgClaim::Multiple(v) => v,
            };
            // Only an Organization id names an Organization; a name a tenant can choose (a
            // repository owner, a slug) never does.
            organizations.extend(raw_orgs.iter().filter_map(|raw| raw.parse::<OrgId>().ok()));
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
    fn a_project_binding_grants_on_that_projects_repositories_alone() {
        let (inst, org) = (InstanceId::from_u128(1), OrgId::from_u128(10));
        let authz = LocalAuthorizer::new(vec![RoleBinding {
            org_id: org,
            principal: Principal::User("proj-admin".into()),
            actions: vec![Action::Admin],
            project_id: Some("proj-a".into()),
            repo: None,
        }]);
        let ctx = RequestCtx::new(
            inst,
            org,
            Subject::authenticated("proj-admin", "proj-admin", vec![org]),
        );
        let repo = |project: Option<&str>| ObjectTarget::Repository {
            instance_id: inst,
            org_id: org,
            project_id: project.map(str::to_owned),
            owner: "jc".into(),
            repo: "cfg".into(),
            git_ref: None,
        };
        assert!(
            authz
                .check(&ctx, Action::Write, &repo(Some("proj-a")))
                .allowed
        );
        for target in [
            repo(Some("proj-b")),
            repo(None),
            ObjectTarget::Instance {
                instance_id: inst,
                org_id: org,
            },
            ObjectTarget::Organization {
                instance_id: inst,
                org_id: org,
                owner: "jc".into(),
            },
        ] {
            assert!(
                !authz.check(&ctx, Action::Admin, &target).allowed,
                "{target:?}"
            );
        }
        assert!(!authz.principal_has_write(inst, org, "jc", "cfg", "proj-admin"));
    }

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

        let subj = Subject::authenticated("u1", "alice", vec![org_a]);
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
        let registry_tok2 = key2
            .mint_registry_token(&subj, org_a, "repository:org/app:push,pull", 300)
            .expect("mint2");
        assert!(
            verifier
                .verify(&registry_tok2, "container_registry")
                .is_err(),
            "another key mints no registry scope"
        );
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(key2.kid.clone());
        let tok2 = encode(
            &header,
            &json!({"sub": "u1", "preferred_username": "alice", "aud": "shako",
                    "exp": TimestampTz::now().as_second() + 300}),
            &EncodingKey::from_ec_der(&key2.pkcs8_der),
        )
        .expect("sign2");
        let verified2 = verifier.verify(&tok2, "shako").expect("refetched kid");
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

    fn package(owner: &str, name: &str) -> ObjectTarget {
        ObjectTarget::Package {
            instance_id: InstanceId::from_u128(1),
            org_id: OrgId::from_u128(10),
            owner: owner.into(),
            package_type: "container".into(),
            name: name.into(),
        }
    }

    fn repo_target(repo: &str) -> ObjectTarget {
        ObjectTarget::Repository {
            instance_id: InstanceId::from_u128(1),
            org_id: OrgId::from_u128(10),
            project_id: Some(String::new()),
            owner: "jc".into(),
            repo: repo.into(),
            git_ref: None,
        }
    }

    fn ctx_of(subject: Subject) -> RequestCtx {
        RequestCtx::new(InstanceId::from_u128(1), OrgId::from_u128(10), subject)
    }

    fn user(id: &str) -> Subject {
        Subject::authenticated(id, id, vec![OrgId::from_u128(10)])
    }

    fn binding(principal: Principal, actions: Vec<Action>, repo: Option<&str>) -> RoleBinding {
        RoleBinding {
            org_id: OrgId::from_u128(10),
            principal,
            actions,
            project_id: None,
            repo: repo.map(str::to_owned),
        }
    }

    #[test]
    fn each_action_parses_by_its_aliases_and_implies_as_documented() {
        let all = [
            Action::Read,
            Action::Propose,
            Action::Write,
            Action::Approve,
            Action::Admin,
            Action::Publish,
            Action::Build,
        ];
        for action in all {
            assert_eq!(Action::parse(action.as_str()), Some(action));
            assert!(Action::Admin.implies(action) && action.implies(Action::Read));
            assert_eq!(action.is_write(), action != Action::Read);
        }
        for (alias, action) in [
            ("VIEW", Action::Read),
            ("pull", Action::Read),
            ("push", Action::Write),
            ("edit", Action::Write),
            ("review", Action::Approve),
        ] {
            assert_eq!(Action::parse(alias), Some(action), "{alias}");
        }
        assert_eq!(Action::parse("delete"), None);
        assert!(Action::Write.implies(Action::Propose));
        assert!(!Action::Write.implies(Action::Approve));
        assert!(!Action::Propose.implies(Action::Write));
        assert!(!Action::Publish.implies(Action::Build));
    }

    #[test]
    fn a_subject_acts_in_exactly_one_organization_and_anonymity_names_nobody() {
        let nobody = Subject::anonymous();
        assert!(nobody.is_anonymous());
        assert_eq!(nobody.login(), "");
        assert!(
            !nobody.is_named_in(&[String::new()]),
            "an empty id names no anonymous caller"
        );
        assert_eq!(nobody.organization(), None);
        let alice = user("alice");
        assert_eq!(alice.organization(), Some(OrgId::from_u128(10)));
        assert!(alice.is_named_in(&["bob".into(), "alice".into()]));
        assert!(!alice.is_named_in(&[]));
        let mut two = alice.clone();
        two.organizations.push(OrgId::from_u128(11));
        assert_eq!(two.organization(), None);
        let mut empty_id = alice.clone();
        empty_id.subject_id = Some(String::new());
        assert!(empty_id.is_anonymous());

        assert!(alice.allows_connector_segment("anything"));
        let mut scoped = alice.clone();
        scoped.connector_segments = Some(vec!["repos".into()]);
        assert!(scoped.allows_connector_segment("repos"));
        assert!(!scoped.allows_connector_segment("packages"));
        scoped.connector_segments = Some(vec!["*".into()]);
        assert!(scoped.allows_connector_segment("packages"));
    }

    #[test]
    fn a_target_names_its_tenant_and_a_decision_becomes_404_or_403() {
        let pkg = package("jc", "app");
        assert_eq!(
            (pkg.instance_id(), pkg.org_id()),
            (InstanceId::from_u128(1), OrgId::from_u128(10))
        );
        assert_eq!((pkg.repo_name(), pkg.project_id()), (None, None));
        let repo = repo_target("cfg");
        assert_eq!(repo.repo_name(), Some("cfg"));
        assert_eq!(repo.project_id(), None, "an empty project id is none");
        assert_eq!(Decision::allow("ok").into_result("gone"), Ok(()));
        assert_eq!(
            Decision::deny_hidden("why").into_result("repository not found"),
            Err(ShakoError::NotFound("repository not found".into()))
        );
        assert_eq!(
            Decision::deny_visible("lacks write").into_result("gone"),
            Err(ShakoError::Forbidden("lacks write".into()))
        );
    }

    #[test]
    fn the_local_authorizer_hides_other_tenants_and_grants_by_user_group_and_repository() {
        let authz = LocalAuthorizer::new(vec![
            binding(Principal::User("reader".into()), vec![Action::Read], None),
            binding(
                Principal::Group("ops".into()),
                vec![Action::Read, Action::Write],
                Some("cfg"),
            ),
        ])
        .with_members(vec![GroupMember {
            org_id: OrgId::from_u128(10),
            group_id: "ops".into(),
            subject_id: "olga".into(),
        }]);
        let check = |subject: Subject, action, target: &ObjectTarget| {
            authz.check(&ctx_of(subject), action, target)
        };
        let cfg = repo_target("cfg");
        let other = repo_target("other");

        let hidden = check(Subject::anonymous(), Action::Read, &cfg);
        assert_eq!(
            (hidden.allowed, hidden.visible, hidden.reason.as_str()),
            (false, false, "unauthenticated")
        );
        let mut outsider = user("reader");
        outsider.organizations = vec![OrgId::from_u128(99)];
        assert!(!check(outsider, Action::Read, &cfg).visible, "not a member");
        let foreign = RequestCtx::new(
            InstanceId::from_u128(2),
            OrgId::from_u128(10),
            user("reader"),
        );
        assert!(
            !authz.check(&foreign, Action::Read, &cfg).visible,
            "another instance"
        );

        assert!(check(user("reader"), Action::Read, &cfg).allowed);
        let denied = check(user("reader"), Action::Write, &cfg);
        assert_eq!(
            (denied.allowed, denied.visible, denied.reason.as_str()),
            (false, true, "caller lacks write permission")
        );
        assert!(!check(user("stranger"), Action::Read, &cfg).visible);
        assert!(
            check(user("olga"), Action::Write, &cfg).allowed,
            "through the group"
        );
        assert!(
            !check(user("olga"), Action::Read, &other).visible,
            "the group's binding names cfg alone"
        );
        let mut read_only = user("olga");
        read_only.read_only = true;
        let refused = check(read_only, Action::Write, &cfg);
        assert_eq!(
            (refused.visible, refused.reason.as_str()),
            (true, "caller token is read-only")
        );

        assert!(authz.principal_has_write(
            InstanceId::from_u128(1),
            OrgId::from_u128(10),
            "jc",
            "cfg",
            "olga"
        ));
        assert!(!authz.principal_has_write(
            InstanceId::from_u128(1),
            OrgId::from_u128(10),
            "jc",
            "cfg",
            "reader"
        ));
        assert!(!authz.principal_has_write(
            InstanceId::from_u128(1),
            OrgId::from_u128(10),
            "jc",
            "cfg",
            ""
        ));
        assert_eq!(
            authz.bindings_for_repo(OrgId::from_u128(10), "cfg"),
            [("ops".to_owned(), Action::Write)]
        );
        assert!(
            authz
                .bindings_for_repo(OrgId::from_u128(10), "other")
                .is_empty()
        );
        assert!(
            authz
                .bindings_for_repo(OrgId::from_u128(11), "cfg")
                .is_empty()
        );

        authz.add_binding(binding(
            Principal::User("late".into()),
            vec![Action::Admin],
            None,
        ));
        assert!(check(user("late"), Action::Admin, &other).allowed);
    }

    #[test]
    fn a_registry_token_reaches_the_image_it_names_with_the_actions_it_names() {
        let authz = LocalAuthorizer::new(Vec::new());
        let token = |scope: &str| {
            let mut s = user("bot");
            s.registry_scope = Some(scope.into());
            ctx_of(s)
        };
        let app = package("jc", "app");
        let pull = token("repository:jc/other:push repository:jc/app:pull");
        assert!(authz.check(&pull, Action::Read, &app).allowed);
        let no_push = authz.check(&pull, Action::Publish, &app);
        assert_eq!((no_push.allowed, no_push.visible), (false, true));
        let push = token("repository:jc/app:push");
        for action in [Action::Read, Action::Write, Action::Publish] {
            assert!(authz.check(&push, action, &app).allowed, "{action:?}");
        }
        let admin = authz.check(&push, Action::Admin, &app);
        assert_eq!(
            (admin.allowed, admin.visible),
            (false, false),
            "no pull: hidden"
        );
        let other_image = authz.check(&push, Action::Read, &package("jc", "db"));
        assert_eq!(
            other_image.reason,
            "registry token not scoped to this image"
        );
        let delete_only = token("repository:jc/app:delete");
        assert!(!authz.check(&delete_only, Action::Read, &app).visible);
        let not_a_package = authz.check(&push, Action::Read, &repo_target("cfg"));
        assert_eq!(
            not_a_package.reason,
            "registry token may only access packages"
        );
    }

    #[test]
    fn an_authorizer_lists_no_group_bindings_unless_it_says_so() {
        struct Nobody;
        impl Authorizer for Nobody {
            fn check(&self, _: &RequestCtx, _: Action, _: &ObjectTarget) -> Decision {
                Decision::deny_hidden("nobody")
            }
            fn principal_has_write(
                &self,
                _: InstanceId,
                _: OrgId,
                _: &str,
                _: &str,
                _: &str,
            ) -> bool {
                false
            }
        }
        let nobody = Nobody;
        assert!(
            nobody
                .bindings_for_repo(OrgId::from_u128(10), "cfg")
                .is_empty()
        );
        assert!(
            !nobody
                .check(&ctx_of(user("a")), Action::Read, &repo_target("cfg"))
                .allowed
        );
        assert!(!nobody.principal_has_write(
            InstanceId::from_u128(1),
            OrgId::from_u128(10),
            "jc",
            "cfg",
            "a"
        ));
    }

    #[test]
    fn a_signing_key_loads_from_raw_hex_or_pem_and_refuses_anything_else() {
        let raw = [0x11u8; 32];
        let kid = ShakoSigningKey::from_secret_bytes(&raw).expect("raw").kid;
        assert_eq!(ShakoSigningKey::from_text(&raw).expect("raw text").kid, kid);
        // Raw bytes that are not UTF-8 skip the text forms.
        let binary = [0x80u8; 32];
        assert_eq!(
            ShakoSigningKey::from_text(&binary).expect("binary").kid,
            ShakoSigningKey::from_secret_bytes(&binary)
                .expect("binary raw")
                .kid
        );
        let hex = format!(" {}\n", "11".repeat(32));
        assert_eq!(
            ShakoSigningKey::from_text(hex.as_bytes()).expect("hex").kid,
            kid
        );
        let pem = SigningKey::from_slice(&raw)
            .expect("key")
            .to_pkcs8_pem(p256::pkcs8::LineEnding::LF)
            .expect("pem");
        assert_eq!(
            ShakoSigningKey::from_text(pem.as_bytes()).expect("pem").kid,
            kid
        );
        for bad in [
            &b"-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----"[..],
            &[0u8; 32][..],
            &b"short"[..],
            // 64 characters but not hex: read as 64 raw bytes, which is no scalar.
            "zz".repeat(32).as_bytes(),
        ] {
            let err = ShakoSigningKey::from_text(bad).expect_err("not a key");
            assert_eq!(err.status_code(), 500, "{err}");
        }
        let file = std::env::temp_dir().join(format!("shako-key-{}.hex", uuid::Uuid::now_v7()));
        std::fs::write(&file, &hex).expect("write");
        let key = ShakoSigningKey::from_file(&file);
        std::fs::remove_file(&file).expect("clean up");
        let key = key.expect("from file");
        assert_eq!(key.kid, kid);
        let err = ShakoSigningKey::from_file(&file).expect_err("missing");
        assert!(
            err.to_string().contains("failed to read signing key"),
            "{err}"
        );
        let shown = format!("{key:?}");
        assert!(shown.contains(&kid) && !shown.contains("1111"), "{shown}");
        assert!(!key.verify_cosign_payload(b"x", "%%% not base64"));
        assert!(!key.verify_cosign_payload(b"x", "AAAA"));
    }

    #[test]
    fn a_registry_token_needs_a_subject_and_lives_between_one_second_and_fifteen_minutes() {
        let key = ShakoSigningKey::generate().expect("key");
        let org = OrgId::from_u128(10);
        let err = key
            .mint_registry_token(&Subject::anonymous(), org, "repository:jc/app:pull", 60)
            .expect_err("nobody");
        assert_eq!(err.status_code(), 401);
        let verifier = TokenVerifier::new(key.clone());
        let lifetime = |ttl| {
            let token = key
                .mint_registry_token(&user("bot"), org, "s", ttl)
                .expect("mint");
            let payload = token.split('.').nth(1).expect("payload");
            let claims: JwtClaims =
                serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).expect("base64"))
                    .expect("claims");
            assert!(verifier.verify(&token, "container_registry").is_ok());
            claims.exp - claims.iat.expect("iat")
        };
        assert_eq!(lifetime(0), 1);
        assert_eq!(lifetime(100_000), 900);
        assert!(std::ptr::eq(verifier.signing_key(), &verifier.signing_key));
    }

    #[test]
    fn a_raw_credential_comes_from_bearer_token_or_basic_and_nothing_else() {
        let basic = |text: &str| format!("Basic {}", STANDARD.encode(text));
        let (pw, colons, empty, no_colon) = (
            basic("user:pw"),
            basic("user:a:b"),
            basic("user:"),
            basic("no-colon"),
        );
        let not_utf8 = format!("Basic {}", STANDARD.encode([0xff, 0x3a, 0x41]));
        let cases = [
            ("Bearer abc", Some("abc")),
            ("  token   abc  ", Some("abc")),
            (pw.as_str(), Some("pw")),
            (colons.as_str(), Some("a:b")),
            (empty.as_str(), None),
            (no_colon.as_str(), None),
            ("Basic %%%", None),
            (not_utf8.as_str(), None),
            ("Bearer", None),
            ("Bearer    ", None),
            ("Digest abc", None),
            ("", None),
        ];
        for (header, want) in cases {
            assert_eq!(
                TokenVerifier::extract_raw_token(header).as_deref(),
                want,
                "{header:?}"
            );
        }
    }

    #[test]
    fn a_static_token_matches_by_hash_alone() {
        let key = ShakoSigningKey::generate().expect("key");
        let robot = user("robot");
        let binding = StaticTokenBinding::new("raw-service-token", robot.clone());
        assert_ne!(binding.token_sha256, "raw-service-token");
        let verifier = TokenVerifier::new(key).with_static_tokens(vec![
            StaticTokenBinding::new("raw-service-token", robot.clone()),
            StaticTokenBinding::new("raw-service-token", user("shadow")),
        ]);
        assert_eq!(
            verifier
                .verify("raw-service-token", "shako")
                .expect("static"),
            robot,
            "the first binding"
        );
        assert_eq!(
            verifier
                .verify("other-token", "shako")
                .expect_err("unknown"),
            ShakoError::Unauthorized("invalid token".into())
        );
    }

    /// A key whose private part does not parse signs no registry token: an internal error
    /// naming the signing, never a token.
    #[test]
    fn a_registry_token_is_not_minted_with_a_broken_key() {
        let mut key = ShakoSigningKey::generate().expect("key");
        key.pkcs8_der = b"not a pkcs8 key".to_vec();
        let subject = Subject::authenticated("u1", "alice", vec![OrgId::from_u128(10)]);
        let err = key
            .mint_registry_token(&subject, OrgId::from_u128(10), "repository:o/a:pull", 60)
            .expect_err("no token");
        assert!(
            matches!(&err, ShakoError::Internal(m) if m.starts_with("registry JWT sign failed")),
            "{err:?}"
        );
    }

    #[test]
    fn a_jwt_is_refused_for_each_broken_part_and_read_for_each_optional_claim() {
        let key = ShakoSigningKey::generate().expect("key");
        let now = TimestampTz::now().as_second();
        let sign = |claims: Value| {
            let mut header = Header::new(Algorithm::ES256);
            header.kid = Some(key.kid.clone());
            encode(&header, &claims, &EncodingKey::from_ec_der(&key.pkcs8_der)).expect("sign")
        };
        let verifier =
            TokenVerifier::new(key.clone()).with_expected_issuer("https://kc.example/realms/jc");
        let refused = |token: &str, says: &str| {
            let err = verifier.verify(token, "shako").expect_err(says);
            assert_eq!(err, ShakoError::Unauthorized(says.into()));
        };
        let iss = "https://kc.example/realms/jc";
        refused(
            &sign(
                json!({"sub": "u", "aud": "shako", "iss": "https://evil.example", "exp": now + 60}),
            ),
            "JWT verification failed",
        );
        refused(
            &sign(json!({"sub": "u", "aud": "shako", "iss": iss, "exp": now - 3600})),
            "JWT verification failed",
        );
        refused(
            &sign(json!({"sub": "u", "iss": iss, "exp": now + 60})),
            "JWT missing aud claim",
        );
        // Another audience, alone or among others, an empty list, and the registry's audience
        // for a shako token: each refused by the audience check.
        for aud in [
            json!("account"),
            json!(["account", "portal"]),
            json!([]),
            json!("container_registry"),
        ] {
            refused(
                &sign(json!({"sub": "u", "aud": aud, "iss": iss, "exp": now + 60})),
                "JWT audience mismatch",
            );
        }
        refused(
            &sign(json!({"sub": "", "aud": "shako", "iss": iss, "exp": now + 60})),
            "JWT names no subject",
        );
        let hs = encode(
            &Header::new(Algorithm::HS256),
            &json!({"sub": "u", "exp": now + 60}),
            &EncodingKey::from_secret(b"k"),
        )
        .expect("hs256");
        refused(&hs, "unsupported JWT signature algorithm");
        let no_kid = encode(
            &Header::new(Algorithm::ES256),
            &json!({"sub": "u", "exp": now + 60}),
            &EncodingKey::from_ec_der(&key.pkcs8_der),
        )
        .expect("no kid");
        refused(&no_kid, "JWT missing kid");
        refused("not-a-jwt", "invalid token");

        let org = OrgId::from_u128(10);
        let full = verifier
            .verify(
                &sign(json!({"sub": "u1", "aud": ["account", "shako"], "iss": iss, "exp": now + 60,
                    "preferred_username": "alice", "name": "Alice A", "email": "a@hel.fi", "azp": "portal",
                    "jc_org": [org.to_string(), "helsinki"], "scope": "ignored"})),
                "shako",
            )
            .expect("verified");
        assert_eq!(
            (
                full.username.as_str(),
                full.full_name.as_str(),
                full.email.as_deref(),
                full.client_id.as_deref()
            ),
            ("alice", "Alice A", Some("a@hel.fi"), Some("portal"))
        );
        assert_eq!(full.organizations, [org], "a name is no Organization id");
        assert_eq!(
            full.registry_scope, None,
            "a scope reaches a registry token alone"
        );
        let bare = verifier
            .verify(&sign(json!({"sub": "u2", "aud": "shako", "iss": iss, "exp": now + 60, "jc_org": org.to_string()})), "shako")
            .expect("verified");
        assert_eq!(
            (bare.username.as_str(), bare.full_name.as_str()),
            ("u2", "u2")
        );
        assert_eq!(bare.organizations, [org]);
    }

    #[test]
    fn an_unknown_kid_refetches_at_most_every_ten_seconds_and_needs_a_usable_jwk() {
        let key = ShakoSigningKey::generate().expect("key");
        let other = ShakoSigningKey::generate().expect("other");
        let now = TimestampTz::now().as_second();
        let token = |kid: &str| {
            let mut header = Header::new(Algorithm::ES256);
            header.kid = Some(kid.to_owned());
            encode(
                &header,
                &json!({"sub": "u", "aud": "shako", "exp": now + 60}),
                &EncodingKey::from_ec_der(&other.pkcs8_der),
            )
            .expect("sign")
        };
        let plain = TokenVerifier::new(key.clone());
        assert_eq!(
            plain
                .verify(&token("nope"), "shako")
                .expect_err("no fetcher"),
            ShakoError::Unauthorized("unknown signing key kid".into())
        );

        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = calls.clone();
        let fetched = vec![
            json!({"kid": "broken", "kty": "EC", "use": 7}),
            json!({"kid": "bad-point", "kty": "EC", "crv": "P-256", "x": "!!", "y": "!!", "alg": "ES256"}),
            json!({"no": "kid"}),
        ];
        let verifier = TokenVerifier::new(key.clone()).with_jwks_fetcher(Arc::new(move || {
            seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(fetched.clone())
        }));
        assert_eq!(
            verifier
                .verify(&token("broken"), "shako")
                .expect_err("bad jwk"),
            ShakoError::Unauthorized("invalid JWK".into())
        );
        assert_eq!(
            verifier
                .verify(&token("bad-point"), "shako")
                .expect_err("bad point"),
            ShakoError::Unauthorized("cannot construct decoding key".into())
        );
        // A kid the fetch did not bring, asked again within ten seconds: no second fetch.
        assert!(verifier.verify(&token("missing"), "shako").is_err());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);

        let failing = TokenVerifier::new(key.clone())
            .with_jwks_fetcher(Arc::new(|| Err(ShakoError::Internal("jwks down".into()))));
        assert_eq!(
            failing
                .verify(&token("x"), "shako")
                .expect_err("fetch failed"),
            ShakoError::Internal("jwks down".into())
        );
        let empty = TokenVerifier::new(key).with_jwks_fetcher(Arc::new(|| Ok(Vec::new())));
        assert_eq!(
            empty.verify(&token("x"), "shako").expect_err("not fetched"),
            ShakoError::Unauthorized("unknown signing key kid".into())
        );
    }

    #[test]
    fn the_key_cache_holds_sixty_four_keys_and_never_drops_its_own() {
        let key = ShakoSigningKey::generate().expect("key");
        let verifier = TokenVerifier::new(key.clone());
        for i in 0..70 {
            verifier.insert_jwk(format!("a{i:02}"), json!({"kid": i}));
        }
        assert_eq!(verifier.jwks.read().keys.len(), 64);
        assert!(
            !verifier.jwks.read().keys.contains_key("a00"),
            "the first key went"
        );
        assert!(verifier.jwks.read().keys.contains_key(&key.kid));
        // With its own key first in order, the cache keeps it and grows past the bound.
        let own_first = TokenVerifier::new(key.clone());
        for i in 0..70 {
            own_first.insert_jwk(format!("z{i:02}"), json!({}));
        }
        assert_eq!(own_first.jwks.read().keys.len(), 71);
        let sub = user("bot");
        let token = key
            .mint_registry_token(&sub, OrgId::from_u128(10), "s", 60)
            .expect("mint");
        assert!(own_first.verify(&token, "container_registry").is_ok());
        assert!(AudClaim::Multiple(vec!["a".into(), "b".into()]).contains("b"));
        assert!(!AudClaim::Single("a".into()).contains("b"));
    }
}
