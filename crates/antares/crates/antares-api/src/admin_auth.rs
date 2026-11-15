// SPDX-License-Identifier: EUPL-1.2
//! The `/q` admin credential (T-3569): one bearer secret a deployment
//! configures. Configured, every `/q` route except `/q/health` and
//! `/q/ready` (and `/q/metrics` when the deployment leaves it open) answers
//! `401` without it; unconfigured, `/q` serves as it always did and the
//! broker says so once at start. A network position is then no longer the
//! only thing between a caller and a tenant purge.

use crate::state::AppState;
use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

/// The shortest secret accepted, in bytes: a random token of 256 bits.
pub const MIN_LEN: usize = 32;

/// The ProblemDetails type a refused admin call answers with: the broker's
/// own namespace, as for `AccessDenied`, since no ETSI error covers `/q`.
pub const UNAUTHORIZED_TYPE: &str = "urn:antares:error:Unauthorized";

/// The admin secret and whether `/q/metrics` is served without it.
pub struct AdminAuth {
    token: crate::secret::Secret,
    /// `/q/metrics` answers without the credential (a scraper that cannot
    /// send one); `false` puts it behind the credential like the rest.
    pub metrics_open: bool,
}

/// Never prints the secret, so a state dump or a panic message cannot leak it.
impl std::fmt::Debug for AdminAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminAuth")
            .field("metrics_open", &self.metrics_open)
            .finish_non_exhaustive()
    }
}

impl AdminAuth {
    /// The secret as a file or an environment value holds it: one trailing
    /// line break is dropped, anything shorter than [`MIN_LEN`] is refused.
    /// The error names the length only, never the content.
    pub fn new(secret: &str, metrics_open: bool) -> Result<Self, String> {
        let secret = secret
            .strip_suffix('\n')
            .map_or(secret, |s| s.strip_suffix('\r').unwrap_or(s));
        if secret.len() < MIN_LEN {
            return Err(format!(
                "the admin token has {} bytes; at least {MIN_LEN} are required",
                secret.len()
            ));
        }
        Ok(Self {
            token: crate::secret::Secret::new(secret),
            metrics_open,
        })
    }

    /// The credential a deployment configured: `file` is
    /// `ANTARES_ADMIN_TOKEN_FILE` (the secret, mounted from a Secret), and
    /// `metrics` is `ANTARES_ADMIN_METRICS`, `token` (default) or `open`.
    /// No file is no credential (`Ok(None)`); a file that cannot be read
    /// or holds too short a secret, or an unknown `metrics` value, is an
    /// error, so a configured credential never silently becomes none.
    pub fn from_config(file: Option<&str>, metrics: Option<&str>) -> Result<Option<Self>, String> {
        let metrics_open = match metrics.map(str::trim).filter(|v| !v.is_empty()) {
            None | Some("token") => false,
            Some("open") => true,
            Some(other) => {
                return Err(format!(
                    "ANTARES_ADMIN_METRICS: {other:?} is neither token nor open"
                ))
            }
        };
        let Some(path) = file.map(str::trim).filter(|p| !p.is_empty()) else {
            return Ok(None);
        };
        let secret = std::fs::read_to_string(path)
            .map_err(|e| format!("ANTARES_ADMIN_TOKEN_FILE {path}: unreadable ({e})"))?;
        Self::new(&secret, metrics_open)
            .map(Some)
            .map_err(|e| format!("ANTARES_ADMIN_TOKEN_FILE {path}: {e}"))
    }

    /// `Authorization: Bearer <secret>` (scheme case-insensitive, RFC 9110
    /// 11.1), compared without an early exit on the first differing byte.
    /// A surface's internal route asks it too (T-4901).
    #[must_use]
    pub fn admits(&self, authorization: Option<&[u8]>) -> bool {
        let Some(value) = authorization else {
            return false;
        };
        let Some(space) = value.iter().position(|b| *b == b' ') else {
            return false;
        };
        let (scheme, rest) = value.split_at(space);
        let presented = rest.get(1..).unwrap_or_default();
        scheme.eq_ignore_ascii_case(b"bearer")
            && constant_time_eq(self.token.expose().as_bytes(), presented)
    }
}

/// Equal length and equal bytes, every byte visited: the time taken tells
/// a caller the length at most, which the configured minimum makes public
/// anyway.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let diff = a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y));
    a.len() == b.len() && std::hint::black_box(diff) == 0
}

/// The route layer on every guarded `/q` route: passes through when no
/// credential is configured, refuses `401` without the right one.
pub async fn require(State(st): State<AppState>, req: Request, next: Next) -> Response {
    let Some(auth) = st.admin_auth.as_deref() else {
        return next.run(req).await;
    };
    let presented = req
        .headers()
        .get(header::AUTHORIZATION)
        .map(axum::http::HeaderValue::as_bytes);
    if auth.admits(presented) {
        return next.run(req).await;
    }
    unauthorized("this admin route requires the broker's admin bearer token")
}

/// The NGSI-LD API layer (T-5032). On a broker that requires a subject
/// (`subject_required` on the state), a request carrying none of the subject
/// headers the deployment named is its own control plane or nobody: it is
/// served only with the admin bearer token and answers `401` otherwise,
/// with no credential configured as well. So a pod that merely reaches the
/// API port neither registers a Context Source nor creates a Tenant. A
/// request with a subject goes on to the policy engine unchanged.
pub async fn subject_or_admin(State(st): State<AppState>, req: Request, next: Next) -> Response {
    if admitted(&st, req.headers()) {
        return next.run(req).await;
    }
    unauthorized("a request without a subject needs the broker's admin bearer token")
}

/// Whether [`subject_or_admin`] lets a request with `headers` through.
fn admitted(st: &AppState, headers: &axum::http::HeaderMap) -> bool {
    let presented = headers
        .get(header::AUTHORIZATION)
        .map(axum::http::HeaderValue::as_bytes);
    !st.subject_required
        || vouched_subject(&crate::policy::SUBJECT_HEADERS, st, headers)
        || st
            .admin_auth
            .as_deref()
            .is_some_and(|a| a.admits(presented))
}

/// A subject header from a caller the broker believes (F-ANT2-001): an
/// in-process call, which only this process makes, or, on a broker that
/// names its gateway (`ANTARES_PROVENANCE_FROM`), a request carrying that
/// gateway's mesh identity. A pod that only reaches the API port cannot
/// pass by adding the header. An empty value counts: the gateway sends an
/// anonymous caller as `x-jc-roles` with none.
fn vouched_subject(names: &[String], st: &AppState, headers: &axum::http::HeaderMap) -> bool {
    names.iter().any(|name| headers.contains_key(name.as_str()))
        && (crate::state::in_process_depth() > 0 || from_named_gateway(st, headers))
}

/// The gateway's word for the caller's home Organization id (`sk_bbsk`),
/// believed beside `x-jc-member: true` only (EP-188).
pub const ORGANIZATION_ID_HEADER: &str = "x-jc-organization-id";
/// The gateway's word for the Organization id of the space the Endpoint
/// serves, believed on every other vouched request (EP-188).
pub const SPACE_ORGANIZATION_HEADER: &str = "x-jc-space-organization";
/// `true` when the gateway admitted the caller as a member of the
/// Organization of the Endpoint's project (ADR-N-067 2.3).
const MEMBER_HEADER: &str = "x-jc-member";

/// Why [`organization_admits`] refused a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrganizationRefusal {
    /// The Tenant is no `{organization id}__{segment}`, or a vouched request
    /// named none: the answer a malformed Tenant gets.
    Malformed,
    /// The Organization the gateway vouched is not the Tenant's: the answer
    /// an unknown Tenant gets, so the refusal tells nothing of existence.
    Unknown,
}

/// The Organization id half of a Tenant `{organization id}__{segment}`
/// (PF-130): split at the first `__`, so the left half holds no `__`; the
/// segment holds no `_` and neither half is empty. The full grammar of each
/// half is the platform's (`jc_core::names::TenantName`); this is the shape
/// the comparison needs and never a prefix match (`sk_a` is not `sk_ab`).
#[must_use]
pub fn tenant_organization(tenant: &str) -> Option<&str> {
    antares_model::TenantId::organization_of(tenant)
}

/// The one value of header `name`: `None` when absent, repeated or not text,
/// so two values can never be read as either.
fn one_header<'a>(headers: &'a axum::http::HeaderMap, name: &str) -> Option<&'a str> {
    let mut all = headers.get_all(name).iter();
    match (all.next(), all.next()) {
        (Some(value), None) => value.to_str().ok(),
        _ => None,
    }
}

/// The Organization check (EP-188, `ANTARES_TENANT_ORGANIZATION=enforce`),
/// one rule for every door that resolves a client Tenant: a consistency
/// check between the gateway's two derivations of the Organization, not a
/// control against a compromised gateway (it sets both).
///
/// - Off, or a call this process makes (in-process callers act for a Tenant
///   the door already checked and send no caller headers): admitted.
/// - Without a vouched subject (the admin bearer): a named Tenant must have
///   the `{organization id}__{segment}` shape, else `Malformed`.
/// - From the gateway: a Tenant is required and must have the shape
///   (`Malformed`); a member (`x-jc-member: true`) is admitted when
///   `x-jc-organization-id` equals the Tenant's Organization, every other
///   caller when `x-jc-space-organization` does; else `Unknown`, counted in
///   `antares_tenant_organization_refused_total` and logged: it is a gateway
///   bug every time.
pub fn organization_admits(
    st: &AppState,
    tenant: Option<&str>,
    headers: &axum::http::HeaderMap,
) -> Result<(), OrganizationRefusal> {
    if !st.tenant_organization || crate::state::in_process_depth() > 0 {
        return Ok(());
    }
    let vouched = vouched_subject(&crate::policy::SUBJECT_HEADERS, st, headers);
    let Some(tenant) = tenant else {
        return if vouched {
            Err(OrganizationRefusal::Malformed)
        } else {
            Ok(())
        };
    };
    if antares_model::TenantId::is_reserved_str(tenant) && !vouched {
        return Ok(());
    }
    let organization = tenant_organization(tenant).ok_or(OrganizationRefusal::Malformed)?;
    if !vouched {
        return Ok(());
    }
    let member = one_header(headers, MEMBER_HEADER) == Some("true");
    let header = if member {
        ORGANIZATION_ID_HEADER
    } else {
        SPACE_ORGANIZATION_HEADER
    };
    let vouched_for = one_header(headers, header);
    if vouched_for == Some(organization) {
        return Ok(());
    }
    metrics::counter!("antares_tenant_organization_refused_total").increment(1);
    tracing::warn!(
        tenant_organization = organization,
        vouched_organization = vouched_for.unwrap_or("<none>"),
        header,
        "a gateway request names a Tenant of another Organization (EP-188): refused as an unknown Tenant"
    );
    Err(OrganizationRefusal::Unknown)
}

/// The answer of one door to a Tenant the Organization check refuses
/// (EP-188), or `None` to serve it: [`organization_admits`], then the
/// Tenant's stored facts (design 10.1, 10.5): a name the rename moved away
/// (`tenant_rename.old`), or a row whose stored `organization_id` is not its
/// name's, answers as an unknown Tenant, so neither is served nor created.
/// With `must_exist`, a Tenant the store lacks answers so too: a door that
/// serves no implicit creation (the WebSocket upgrade) then tells a refused
/// Tenant from an absent one by nothing.
///
/// The answers are the API's own: 400 for a malformed Tenant, the 5.5.10
/// NonexistentTenant body otherwise, byte for byte.
/// ponytail: one indexed query per outer request under `enforce`; cache the
/// standing beside `known_tenants` if it shows in the request profile.
pub async fn tenant_door(
    st: &AppState,
    named: Option<&str>,
    headers: &axum::http::HeaderMap,
    must_exist: bool,
) -> Option<Response> {
    if !st.tenant_organization || crate::state::in_process_depth() > 0 {
        return None;
    }
    let malformed = || {
        crate::negotiate::ApiError::from(antares_model::NgsiError::BadRequestData(format!(
            "invalid NGSILD-Tenant value: {:?}",
            named.unwrap_or_default()
        )))
        .into_response()
    };
    match organization_admits(st, named, headers) {
        Ok(()) => {}
        Err(OrganizationRefusal::Malformed) => return Some(malformed()),
        Err(OrganizationRefusal::Unknown) => {
            return Some(
                match named.and_then(|raw| antares_model::TenantId::new(raw).ok()) {
                    Some(tenant) => crate::router::nonexistent_tenant(&tenant),
                    None => malformed(),
                },
            )
        }
    }
    let raw = named?;
    let tenant = antares_model::TenantId::new(raw).ok()?;
    let standing = match st.store.tenant_standing(&tenant).await {
        Ok(standing) => standing,
        Err(e) => return Some(crate::negotiate::ApiError::from(e).into_response()),
    };
    let reassigned = standing
        .organization
        .as_deref()
        .is_some_and(|stored| Some(stored) != tenant_organization(raw));
    if standing.tombstoned || reassigned {
        metrics::counter!("antares_tenant_organization_refused_total").increment(1);
        tracing::warn!(
            tenant = raw,
            tombstoned = standing.tombstoned,
            stored_organization = standing.organization.as_deref().unwrap_or("<none>"),
            "a request names a renamed Tenant or one whose stored Organization is not its name's (EP-188): refused as an unknown Tenant"
        );
        return Some(crate::router::nonexistent_tenant(&tenant));
    }
    if must_exist && !tenant.is_internal() {
        match st.store.tenant_exists(&tenant).await {
            Ok(true) => {}
            Ok(false) => return Some(crate::router::nonexistent_tenant(&tenant)),
            Err(e) => return Some(crate::negotiate::ApiError::from(e).into_response()),
        }
    }
    None
}

/// [`tenant_door`] on the NGSI-LD API (EP-188): between [`subject_or_admin`]
/// and the Tenant existence check, on the one `NGSILD-Tenant` value the
/// handler reads too. A refusal answers what a malformed or an unknown Tenant
/// gets there, before any store read or implicit creation.
pub async fn organization_layer(State(st): State<AppState>, req: Request, next: Next) -> Response {
    door_layer(&st, req, next, false).await
}

/// [`tenant_door`] with `must_exist` on a surface that names its Tenant in
/// `NGSILD-Tenant` and creates none (the channels, ADR-0031): under
/// `enforce` a refused and an absent Tenant answer alike there.
pub async fn existing_tenant_layer(
    State(st): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    door_layer(&st, req, next, true).await
}

async fn door_layer(st: &AppState, req: Request, next: Next, must_exist: bool) -> Response {
    if !st.tenant_organization {
        return next.run(req).await;
    }
    let named = match crate::negotiate::single_header(req.headers(), "NGSILD-Tenant") {
        Ok(named) => named,
        Err(e) => return e.into_response(),
    };
    match tenant_door(st, named.as_deref(), req.headers(), must_exist).await {
        None => next.run(req).await,
        Some(refused) => refused,
    }
}

tokio::task_local! {
    /// The headers of the request the view root is serving, for
    /// [`view_admits`]: a view's Tenant is known only once its id is looked up.
    static VIEW_CALLER: axum::http::HeaderMap;
}

/// The view root's half of the Organization check (EP-188): keeps the
/// request's headers in scope while a view surface serves it, so the view
/// lookup can ask [`view_admits`] on the view's own Tenant. Off, a request
/// passes as before.
pub async fn view_organization_layer(
    State(st): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    if !st.tenant_organization {
        return next.run(req).await;
    }
    VIEW_CALLER
        .scope(req.headers().clone(), next.run(req))
        .await
}

/// Whether the caller the view root is serving may see a view of `tenant`
/// ([`organization_admits`]); a refused one is answered as an unknown view.
/// Outside a view root request (a lookup this process makes for itself) there
/// is no caller to check, and the view is served.
#[must_use]
pub fn view_admits(st: &AppState, tenant: &str) -> bool {
    !st.tenant_organization
        || VIEW_CALLER
            .try_with(|headers| organization_admits(st, Some(tenant), headers).is_ok())
            .unwrap_or(true)
}

/// No gateway named, or the request carries its mesh identity.
#[cfg(feature = "jc")]
fn from_named_gateway(st: &AppState, headers: &axum::http::HeaderMap) -> bool {
    st.provenance_from
        .as_deref()
        .is_none_or(|gateway| crate::provenance::from_gateway(headers, Some(gateway)))
}

/// Vanilla Antares reads no gateway identity (ANTARES_PROVENANCE_FROM is a
/// platform key), so no gateway is named.
#[cfg(not(feature = "jc"))]
fn from_named_gateway(_st: &AppState, _headers: &axum::http::HeaderMap) -> bool {
    true
}

/// `401` with a bearer challenge and `detail`.
fn unauthorized(detail: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [
            (header::WWW_AUTHENTICATE, "Bearer realm=\"antares-admin\""),
            (header::CONTENT_TYPE, "application/json"),
        ],
        axum::Json(serde_json::json!({
            "type": UNAUTHORIZED_TYPE,
            "title": "Unauthorized",
            "status": StatusCode::UNAUTHORIZED.as_u16(),
            "detail": detail,
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "0123456789abcdef0123456789abcdef";

    fn auth() -> AdminAuth {
        AdminAuth::new(SECRET, false).expect("a 32-byte secret is accepted")
    }

    #[test]
    fn a_short_secret_is_refused_and_the_error_holds_no_secret() {
        let short = "0123456789abcdef0123456789abcde";
        let err = AdminAuth::new(short, false).expect_err("31 bytes");
        assert!(err.contains("31 bytes"), "{err}");
        assert!(!err.contains(short), "{err}");
        assert!(AdminAuth::new("", true).is_err());
        // a trailing line break is not part of the secret
        assert!(AdminAuth::new(&format!("{short}\n"), false).is_err());
    }

    #[test]
    fn one_trailing_line_break_is_dropped() {
        for raw in [format!("{SECRET}\n"), format!("{SECRET}\r\n")] {
            let a = AdminAuth::new(&raw, false).expect("secret");
            assert!(a.admits(Some(format!("Bearer {SECRET}").as_bytes())));
        }
    }

    #[test]
    fn only_the_exact_bearer_secret_is_admitted() {
        let a = auth();
        assert!(a.admits(Some(format!("Bearer {SECRET}").as_bytes())));
        assert!(a.admits(Some(format!("bearer {SECRET}").as_bytes())));
        for refused in [
            String::new(),
            "Bearer".into(),
            "Bearer ".into(),
            format!("Basic {SECRET}"),
            format!("Bearer {SECRET}x"),
            format!("Bearer {}", &SECRET[1..]),
            format!("Bearer  {SECRET}"),
            format!("Bearer {}0", &SECRET[..31]),
            SECRET.into(),
        ] {
            assert!(!a.admits(Some(refused.as_bytes())), "{refused:?}");
        }
        assert!(!a.admits(None));
    }

    #[test]
    fn constant_time_eq_needs_equal_length_and_bytes() {
        assert!(constant_time_eq(b"", b""));
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(!constant_time_eq(b"ab", b"abc"));
        // two wrong bytes whose differences cancel under XOR: an accumulator
        // that XORs instead of ORs would admit this guess (T-4778 mutant)
        assert!(!constant_time_eq(b"abc", b"`cc"));
        assert!(!auth().admits(Some(b"Bearer 1023456789abcdef0123456789abcdef")));
    }

    #[test]
    fn the_config_reads_the_file_and_refuses_what_it_cannot_use() {
        let dir = std::env::temp_dir().join(format!("antares-admin-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).expect("temp dir");
        let good = dir.join("token");
        std::fs::write(&good, format!("{SECRET}\n")).expect("write");
        let short = dir.join("short");
        std::fs::write(&short, "0123").expect("write");
        let good_s = good.to_str().expect("utf-8 path");

        assert!(AdminAuth::from_config(None, None).expect("unset").is_none());
        assert!(AdminAuth::from_config(Some(" "), Some("open"))
            .expect("blank")
            .is_none());
        let a = AdminAuth::from_config(Some(good_s), None)
            .expect("ok")
            .expect("some");
        assert!(!a.metrics_open);
        assert!(a.admits(Some(format!("Bearer {SECRET}").as_bytes())));
        let open = AdminAuth::from_config(Some(good_s), Some("open"))
            .expect("ok")
            .expect("some");
        assert!(open.metrics_open);
        assert!(
            !AdminAuth::from_config(Some(good_s), Some("token"))
                .expect("ok")
                .expect("some")
                .metrics_open
        );

        let err = AdminAuth::from_config(Some(good_s), Some("public")).expect_err("unknown");
        assert!(err.contains("ANTARES_ADMIN_METRICS"), "{err}");
        let err =
            AdminAuth::from_config(Some(short.to_str().expect("path")), None).expect_err("short");
        assert!(err.contains("4 bytes") && !err.contains("0123\""), "{err}");
        let missing = dir.join("absent");
        let err = AdminAuth::from_config(missing.to_str(), None).expect_err("absent");
        assert!(err.contains("unreadable"), "{err}");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn debug_never_prints_the_secret() {
        let shown = format!("{:?}", auth());
        assert!(!shown.contains(SECRET), "{shown}");
        assert!(shown.contains("metrics_open"), "{shown}");
    }

    #[cfg(feature = "jc")]
    const GATEWAY: &str = "context-gateway.jc.serviceaccount.identity.linkerd.cluster.local";

    #[cfg(feature = "jc")]
    fn headers(pairs: &[(&'static str, &'static str)]) -> axum::http::HeaderMap {
        let mut out = axum::http::HeaderMap::new();
        for (k, v) in pairs {
            out.insert(*k, axum::http::HeaderValue::from_static(v));
        }
        out
    }

    /// F-ANT2-001: a bare subject header passes the T-5032 wall only from
    /// the gateway's mesh identity or an in-process call, once the broker
    /// names its gateway.
    #[cfg(feature = "jc")]
    #[tokio::test]
    async fn a_subject_is_believed_only_from_the_gateway_or_this_process() {
        let names = vec!["x-jc-subject".to_owned()];
        let mut st = AppState::new("subject-wall".into());
        st.subject_required = true;
        let bare = headers(&[("x-jc-subject", "anyone")]);
        let other = headers(&[("x-jc-subject", "anyone"), ("l5d-client-id", "pod.other")]);
        let gateway = headers(&[("x-jc-subject", "anyone"), ("l5d-client-id", GATEWAY)]);
        let no_subject = headers(&[("l5d-client-id", GATEWAY)]);
        // no gateway named: the header alone, as before
        assert!(vouched_subject(&names, &st, &bare));
        st.provenance_from = Some(GATEWAY.to_owned());
        assert!(!vouched_subject(&names, &st, &bare));
        assert!(!vouched_subject(&names, &st, &other));
        assert!(vouched_subject(&names, &st, &gateway));
        assert!(!vouched_subject(&names, &st, &no_subject));
        let in_process = crate::state::INPROCESS_DEPTH
            .scope(1, async { vouched_subject(&names, &st, &bare) })
            .await;
        assert!(in_process);
        // the wall itself: neither subject nor bearer
        assert!(!admitted(&st, &bare));
    }
}
