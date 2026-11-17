//! The tenant is a conclusion, never a request header (T-0145, GW20, EP-21, EP-22, SP-05).
//!
//! A client can put anything in a header. Everything a client says about which tenant it
//! is in, who it is, or what it may see is therefore removed before the request is looked
//! at, and the values the broker sees are put there by the gateway from the endpoint it
//! resolved and the token it verified.
//!
//! The stripping is unconditional and happens first, before routing, authentication or
//! policy evaluation, so no later stage can accidentally read a client-supplied value.

use crate::auth::subject::Subject;
use axum::extract::Request;
use axum::http::header::{AUTHORIZATION, COOKIE};
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use jc_core::names::TenantName;

/// The header that pins the broker's tenant. Only the gateway ever sets it (GW20), and only
/// through [`pin_tenant`] with a [`Tenant`] an Endpoint record holds: no other module names it,
/// so no bare string reaches the hop (design 10.7, a test greps for it).
const TENANT: HeaderName = HeaderName::from_static("ngsild-tenant");
/// The Organization id of the space a hop is pinned to, from the Endpoint record (EP-188): the
/// broker checks it against the tenant's Organization half for every caller who is not a member.
pub const SPACE_ORGANIZATION: HeaderName = HeaderName::from_static("x-jc-space-organization");
/// The verified caller's Organization id, a `jc_org_id` entry of their token, sent beside
/// [`MEMBER`] when it names the tenant's Organization (EP-188): the broker admits a member only
/// when it equals the tenant's Organization half.
pub const ORGANIZATION_ID: HeaderName = HeaderName::from_static("x-jc-organization-id");

/// The tenant a hop to the broker is pinned to (EP-22, PF-130): a [`TenantName`] under
/// `JC_TENANT_NAMES=project-key`, the space's segment under `segment` (the release before the
/// rename). Both are checked when built, so the header value is always legal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tenant {
    value: HeaderValue,
    /// The tenant's Organization half, sent to the broker, which compares it with the name.
    organization: Option<HeaderValue>,
    /// The Organization id (`sk_bbsk`) a member's token names for this space (`jc_org_id`).
    member_claim: Option<String>,
}

impl Tenant {
    /// A tenant `{organization id}__{segment}` (the 2026-11-15 form), which names its
    /// Organization to the broker and whose members' tokens name that same id.
    pub fn named(name: &TenantName) -> jc_core::Result<Self> {
        let org = jc_core::names::OrganizationId::parse(name.organization())?;
        Self::keyed(name, &org)
    }

    /// A tenant whose members' tokens name Organization `claim`: for a project-key tenant
    /// (`{OrgId}__{ProjectId}__{space}`) the broker hears the `OrgId` half, a member is a
    /// caller whose token names the project's Organization `claim` (ARCH/TEN-10).
    pub fn keyed(
        name: &TenantName,
        claim: &jc_core::names::OrganizationId,
    ) -> jc_core::Result<Self> {
        let header = |value: &str| {
            HeaderValue::from_str(value).map_err(|_| jc_core::Error::Name {
                field: "tenant",
                value: value.to_owned(),
                reason: "must be a legal header value",
            })
        };
        Ok(Self {
            value: header(name.as_str())?,
            organization: Some(header(name.organization())?),
            member_claim: Some(claim.as_str().to_owned()),
        })
    }

    /// The tenant of a space under `JC_TENANT_NAMES=segment`: the segment as it is, checked as
    /// a space name, naming no Organization.
    pub fn segment(segment: &str) -> jc_core::Result<Self> {
        jc_core::names::validate_space_name(segment)?;
        Ok(Self {
            // A space name is `[a-z0-9-]`, always a legal header value.
            value: HeaderValue::from_str(segment).map_err(|_| jc_core::Error::Name {
                field: "space",
                value: segment.to_owned(),
                reason: "must be a legal header value",
            })?,
            organization: None,
            member_claim: None,
        })
    }

    /// The tenant as the `NGSILD-Tenant` header carries it.
    pub fn as_str(&self) -> &str {
        self.value.to_str().unwrap_or_default()
    }

    /// The Organization id half, `None` for a segment tenant.
    pub fn organization(&self) -> Option<&str> {
        self.organization.as_ref().and_then(|o| o.to_str().ok())
    }
}

/// A hop whose Endpoint has no tenant (PF-130): its project names no Organization with a
/// verified domain the repository holds, so the space has no name at the broker yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unpinned;

impl Unpinned {
    /// `503`: an operator fixes the Project, the caller can do nothing but wait.
    pub fn problem(self) -> jc_core::error::ProblemDetails {
        jc_core::error::ProblemDetails::new(503, "service-unavailable", "Service Unavailable")
            .with_detail(
                "This Endpoint's space has no tenant yet: its project names no Organization with \
                 a verified domain. An operator names one in the Project's spec.organizationRef.",
            )
    }
}

impl std::fmt::Display for Unpinned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "the Endpoint's project resolves to no Organization, so its space has no tenant",
        )
    }
}

/// The tenant [`pin_tenant`] pinned on `headers`, if any.
pub fn pinned_tenant(headers: &HeaderMap) -> Option<&str> {
    headers.get(TENANT).and_then(|tenant| tenant.to_str().ok())
}

/// Copies the pinned tenant and its space Organization from `pinned` onto a hop the gateway
/// makes for the same request (a condition probe, an unlinking).
pub fn copy_tenant(pinned: &HeaderMap, headers: &mut HeaderMap) {
    for name in [TENANT, SPACE_ORGANIZATION] {
        while headers.remove(&name).is_some() {}
        if let Some(value) = pinned.get(&name) {
            headers.insert(name, value.clone());
        }
    }
}

/// Removes the tenant from an answer: it is an internal name (SP-05).
pub fn drop_tenant(headers: &mut HeaderMap) {
    while headers.remove(&TENANT).is_some() {}
}

/// Headers a client must never be able to set, because a downstream component would
/// believe them (Deployment/10 section 4, GW25); every name under [`IDENTITY_PREFIX`] is
/// removed besides these.
pub const FORGEABLE: &[&str] = &[
    "ngsild-tenant",
    "x-userinfo",
    "x-access-token",
    "x-allowed-scope-ids",
    "x-endpoint-slug",
    "x-consumer-identity",
];

/// The prefix of every identity header the broker's `joinedcontext` engine believes (T-3641):
/// only the gateway sets one, so a client's is removed whatever its name, including one no
/// code reads yet (T-3816).
pub const IDENTITY_PREFIX: &str = "x-jc-";

/// The prefix of the provenance headers the broker stores with a write (ADR-N-054 2.3, DM-112):
/// only the gateway sets one, so a client's is removed whatever its name.
pub const PROVENANCE_PREFIX: &str = "jc-provenance-";
/// The prefix of the Linkerd mesh headers (T-5869, F-SEC1-028): the proxy in front of the broker
/// sets `l5d-client-id` from the caller's mTLS certificate and the broker believes it (Antares
/// `provenance.rs`), so a client's copy never crosses the gateway, whatever its name.
pub const MESH_PREFIX: &str = "l5d-";
/// The path a write came through: `pipeline`, `ui` or `api`.
pub const PROVENANCE_CHANNEL: HeaderName = HeaderName::from_static("jc-provenance-channel");
/// Who wrote, as the audit log names them (`user:…`, `serviceAccount:…`, a DID), percent-encoded.
pub const PROVENANCE_ACTOR: HeaderName = HeaderName::from_static("jc-provenance-actor");
/// The Organization of the Project the caller was admitted under (ADR-N-054 2.3), percent-encoded.
pub const PROVENANCE_ORGANIZATION: HeaderName =
    HeaderName::from_static("jc-provenance-organization");
/// What kind of principal wrote: `person`, `serviceAccount`, `pipeline` or `agent` (ADR-N-054 2.3).
pub const PROVENANCE_ACTOR_KIND: HeaderName = HeaderName::from_static("jc-provenance-actor-kind");
/// The Keycloak client the verified token was issued to (`azp`), percent-encoded: the MCP client,
/// CLI or App a person's write came through (ADR-N-054 2.3, DM-111). Provenance only: the
/// broker decides on `x-jc-client`, which names a workload alone.
pub const PROVENANCE_CLIENT: HeaderName = HeaderName::from_static("jc-provenance-client");
/// `changes` on a write through an Endpoint with `spec.audit: changes` (ADR-N-054 2.8, DM-136):
/// only the gateway may set it, so it shares the provenance prefix the edge and `drop_identity`
/// remove.
pub const AUDIT: HeaderName = HeaderName::from_static("jc-provenance-audit");
/// `version` on a write asks the broker to keep each changed value as a version (ADR-N-054 2.9,
/// DM-144): the one record header a caller sets. The gateway turns it into [`AUDIT`] and never
/// forwards it.
pub const RECORD: HeaderName = HeaderName::from_static("jc-record");
/// The grant of `versions=all` (ADR-N-054 2.4): only the gateway may set it.
pub const READ_VERSIONS: &str = "jc-read-versions";
/// Why a write was made, in the caller's words (DM-113): the one provenance header a caller sets.
pub const REASON: HeaderName = HeaderName::from_static("jc-reason");
/// The id of the request, which the broker stores in each row's provenance (ADR-N-054 2.3).
pub const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");
/// The longest request id the broker stores (Antares `provenance.rs`).
const MAX_REQUEST_ID: usize = 128;
/// The Keycloak client of the Portal's UI: a person's write through it is channel `ui`.
pub const PORTAL_UI_CLIENT: &str = "portal-ui";

/// Removes every header under [`IDENTITY_PREFIX`], [`PROVENANCE_PREFIX`] and [`MESH_PREFIX`], and
/// [`READ_VERSIONS`], each with all its values.
fn drop_identity(headers: &mut HeaderMap) {
    let names: Vec<HeaderName> = headers
        .keys()
        .filter(|name| {
            let name = name.as_str();
            name.starts_with(IDENTITY_PREFIX)
                || name.starts_with(PROVENANCE_PREFIX)
                || name.starts_with(MESH_PREFIX)
                || name == READ_VERSIONS
        })
        .cloned()
        .collect();
    for name in names {
        while headers.remove(&name).is_some() {}
    }
}

/// The user the gateway verified, as the broker's `joinedcontext` engine matches a `user`
/// assignee against it: a person's Keycloak id (PF-126), a client's own service-account user
/// by name (T-3641).
pub const SUBJECT: HeaderName = HeaderName::from_static("x-jc-subject");
/// The groups of the verified caller, each percent-encoded, comma separated.
pub const GROUPS: HeaderName = HeaderName::from_static("x-jc-groups");
/// The roles of the verified caller, each percent-encoded, comma separated; `public` for an anonymous one.
pub const ROLES: HeaderName = HeaderName::from_static("x-jc-roles");
/// The service account of the verified caller.
pub const CLIENT: HeaderName = HeaderName::from_static("x-jc-client");
/// `true` when the caller asked to be told about narrowing (O-3 (b), R22).
pub const RESTRICTED: HeaderName = HeaderName::from_static("x-jc-restricted");
/// `true` when the caller's token is read-only (T-5955): the broker refuses its every write.
pub const READ_ONLY: HeaderName = HeaderName::from_static("x-jc-read-only");
/// The data-space participant a transfer token names (DS-02, map K38, T-3800).
pub const DID: HeaderName = HeaderName::from_static("x-jc-did");
/// `true` when the token's verified `jc_org` names the Organization of the Endpoint's project:
/// the broker grants an `organization` assignee to this caller (ADR-N-067 2.3, Antares T-6419).
pub const MEMBER: HeaderName = HeaderName::from_static("x-jc-member");

/// The service account the gateway's own hops to the broker run as (T-3767): its Keycloak
/// client. The rule document the Portal writes grants it the reads and the unlinking those hops
/// make; a caller never carries it (see [`pin_caller`]).
pub const GATEWAY_CLIENT: &str = "context-gateway";

/// Removes every header a client could use to forge identity or tenancy (EP-21, GW20).
///
/// Removes each name entirely, not just its first value: a repeated header would otherwise
/// leave a copy behind for whoever reads the last one.
pub fn strip_client_headers(request: &mut Request) {
    let headers = request.headers_mut();
    for name in FORGEABLE {
        while headers.remove(*name).is_some() {}
    }
    drop_identity(headers);
}

/// Pins the tenant for the internal hop to the broker (EP-22, GW20), and the Organization of
/// its space when the tenant names one (EP-188).
///
/// Called after the slug resolved, with the tenant the Endpoint record holds, never with
/// anything derived from the request. `insert` replaces every earlier value.
pub fn pin_tenant(headers: &mut HeaderMap, tenant: &Tenant) {
    headers.insert(TENANT, tenant.value.clone());
    while headers.remove(&SPACE_ORGANIZATION).is_some() {}
    if let Some(organization) = &tenant.organization {
        headers.insert(SPACE_ORGANIZATION, organization.clone());
    }
}

/// Pins the tenant and the verified caller for the hop to the broker, and drops the caller's
/// own credentials from it (T-3641, map G15, K36).
///
/// The broker is the audience of neither the caller's token nor their cookie. It decides on the
/// `x-jc-*` headers instead, and reads a request carrying none of them as the cluster's own, so
/// `x-jc-roles` is always set, to `public` at least for an anonymous caller. Every value and
/// list member is percent-encoded (T-3770), so a name with a diacritic or a comma reaches the
/// broker whole. A caller under a data-space agreement is its participant's DID alone
/// (`x-jc-did`, DS-02): no role, no account, no opt-in, so a broker that does not read the
/// header yet takes the hop as the cluster's own and refuses it, never as the public's.
///
/// A member's hop also carries [`ORGANIZATION_ID`], the entry of their verified `jc_org_id` that
/// names the tenant's Organization; a member whose token lists none carries no id, and the
/// broker's Organization check refuses them (EP-188). `Err` when the Endpoint has no tenant.
pub fn pin_caller(
    request: &mut Request,
    tenant: Option<&Tenant>,
    subject: &Subject,
) -> Result<(), Unpinned> {
    let tenant = tenant.ok_or(Unpinned)?;
    let asked = super::response::asked_about_narrowing(request);
    let headers = request.headers_mut();
    for name in [AUTHORIZATION, COOKIE] {
        while headers.remove(&name).is_some() {}
    }
    drop_identity(headers);
    pin_tenant(headers, tenant);
    let (channel, actor) = provenance(subject);
    headers.insert(PROVENANCE_CHANNEL, HeaderValue::from_static(channel));
    one(headers, PROVENANCE_ACTOR, Some(&actor));
    one(
        headers,
        PROVENANCE_ORGANIZATION,
        subject.organization.as_deref(),
    );
    if let Some(kind) = actor_kind(subject) {
        headers.insert(PROVENANCE_ACTOR_KIND, HeaderValue::from_static(kind));
    }
    request_id(headers);
    headers.extend(caller_headers(subject));
    let home = tenant.organization().filter(|_| {
        subject.member_of_organization
            && tenant
                .member_claim
                .as_ref()
                .is_some_and(|claim| subject.organization_ids.contains(claim))
    });
    one(headers, ORGANIZATION_ID, home);
    if subject.agreement.is_some() {
        return Ok(());
    }
    let named = subject.client.as_deref();
    one(
        headers,
        PROVENANCE_CLIENT,
        named.filter(|c| *c != GATEWAY_CLIENT),
    );
    if asked {
        headers.insert(RESTRICTED, HeaderValue::from_static("true"));
    }
    Ok(())
}

/// Who `subject` is to the broker, as the `x-jc-*` headers it decides on (Antares
/// `addons/joinedcontext-policy/README.md` section 1): what [`pin_caller`] writes on every hop,
/// and what any other server reading the broker for a caller sends, so both name the caller
/// alike. Built from the verified subject alone, never from a request's headers.
pub fn caller_headers(subject: &Subject) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if subject.read_only {
        headers.insert(READ_ONLY, HeaderValue::from_static("true"));
    }
    if subject.agreement.is_some() {
        one(&mut headers, DID, subject.did.as_deref());
        return headers;
    }
    one(&mut headers, SUBJECT, subject.user_principal());
    // Only the gateway's own hops speak as the gateway, never a token of its client. A project's
    // account is `{project}/{name}`, the form a Policy assignee names it by (PL-92): the broker
    // compares the header with the assignee as written, and a bare name would let two projects'
    // same-named accounts share each other's grants (F-SEC1-003, T-5860).
    let client = subject
        .service_account
        .as_deref()
        .filter(|c| *c != GATEWAY_CLIENT)
        .map(|name| match subject.service_account_project.as_deref() {
            Some(project) => format!("{project}/{name}"),
            None => name.to_owned(),
        });
    one(&mut headers, CLIENT, client.as_deref());
    // The broker matches a `group` assignee as the gateway does (ADR-N-067 2.3).
    list(&mut headers, GROUPS, subject.grant_groups(), false);
    list(&mut headers, ROLES, &subject.roles, true);
    if subject.member_of_organization {
        headers.insert(MEMBER, HeaderValue::from_static("true"));
    }
    headers
}

/// The channel and the actor of a write by `subject` (ADR-N-054 2.3, DM-111): a Pipeline's
/// derived account is `pipeline`, a person signed in to the Portal's UI is `ui`, everyone else
/// `api`; the actor is the verified principal, never anything the request says.
pub fn provenance(subject: &Subject) -> (&'static str, String) {
    let channel = match (&subject.user, &subject.service_account) {
        (None, Some(account)) if jc_core::kinds::pipeline_identity::is_derived(account) => {
            "pipeline"
        }
        (Some(_), _) if subject.client.as_deref() == Some(PORTAL_UI_CLIENT) => "ui",
        _ => "api",
    };
    (channel, crate::auth::subject::principal_of(subject))
}

/// Keeps the edge's `X-Request-Id` when the broker would store it, else sets a fresh one, so each
/// written row names the request that wrote it; a correlation id, never a grant.
fn request_id(headers: &mut HeaderMap) {
    let mut ids = headers.get_all(REQUEST_ID).iter();
    let kept = match (ids.next(), ids.next()) {
        (Some(id), None) => {
            let id = id.as_bytes();
            (1..=MAX_REQUEST_ID).contains(&id.len())
                && id
                    .iter()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        }
        _ => false,
    };
    if kept {
        return;
    }
    while headers.remove(REQUEST_ID).is_some() {}
    if let Some(id) = crate::creates::uuid_v4()
        .ok()
        .and_then(|id| HeaderValue::from_str(&id).ok())
    {
        headers.insert(REQUEST_ID, id);
    }
}

/// The kind of principal `subject` is (ADR-N-054 2.3): a Pipeline's derived account is
/// `pipeline`, another account `serviceAccount`, a person's token another client exchanged (an
/// assistant) `agent`, a person `person`. An anonymous caller and a data-space participant name
/// none; an MCP call is made `agent` by [`as_agent`].
pub fn actor_kind(subject: &Subject) -> Option<&'static str> {
    match (&subject.user, &subject.service_account, &subject.via) {
        (Some(_), _, Some(_)) => Some("agent"),
        (Some(_), _, None) => Some("person"),
        (None, Some(account), _) if jc_core::kinds::pipeline_identity::is_derived(account) => {
            Some("pipeline")
        }
        (None, Some(_), _) => Some("serviceAccount"),
        (None, None, _) => None,
    }
}

/// Marks a hop [`pin_caller`] pinned as an AI agent's: an MCP tool call acts for the person or
/// client its token names, who stays the actor (ADR-N-054 2.3).
pub fn as_agent(headers: &mut HeaderMap) {
    if headers.contains_key(PROVENANCE_ACTOR_KIND) {
        headers.insert(PROVENANCE_ACTOR_KIND, HeaderValue::from_static("agent"));
    }
}

fn one(headers: &mut HeaderMap, name: HeaderName, value: Option<&str>) {
    if let Some(value) = value.and_then(|v| HeaderValue::from_str(&encode(v)).ok()) {
        headers.insert(name, value);
    }
}

/// One name as the broker reads it (Antares `addons/joinedcontext-policy/README.md` section 1):
/// percent-encoded UTF-8, the RFC 3986 unreserved characters kept, so a diacritic travels and a
/// comma never splits a list member.
fn encode(value: &str) -> String {
    crate::query::encode(value)
}

/// The value of [`READ_VERSIONS`] for these attributes (`None`: every one): `true`, the value a
/// broker before ADR-N-066 reads too, or the names percent-encoded and comma separated; `None`
/// when the list is empty, which grants nothing.
pub fn read_versions(names: Option<&std::collections::BTreeSet<String>>) -> Option<HeaderValue> {
    let value = match names {
        None => "true".to_owned(),
        Some(names) if names.is_empty() => return None,
        Some(names) => names
            .iter()
            .map(|n| encode(n))
            .collect::<Vec<_>>()
            .join(","),
    };
    HeaderValue::from_str(&value).ok()
}

fn list<'a>(
    headers: &mut HeaderMap,
    name: HeaderName,
    members: impl IntoIterator<Item = &'a String>,
    always: bool,
) {
    let joined = members
        .into_iter()
        .filter(|member| !member.is_empty())
        .map(|member| encode(member))
        .collect::<Vec<_>>()
        .join(",");
    // Encoded, every member is visible ASCII, which a header always carries.
    if let Ok(value) = HeaderValue::from_str(&joined) {
        if always || !joined.is_empty() {
            headers.insert(name, value);
        }
    }
}

/// Marks a hop the gateway makes on its own account (T-3767): the condition probe of a write,
/// the unlinking of a held delete, the read of a stored Subscription. It speaks as
/// [`GATEWAY_CLIENT`] with no role, so the engine decides it by the gateway's own grant and a
/// hop that forgot this is refused, never served as the cluster's own.
pub fn as_gateway(headers: &mut HeaderMap) {
    for name in [AUTHORIZATION, COOKIE] {
        while headers.remove(&name).is_some() {}
    }
    // The space's Organization is the Endpoint record's, not the caller's: it stays.
    let space_organization = headers.get(&SPACE_ORGANIZATION).cloned();
    drop_identity(headers);
    if let Some(organization) = space_organization {
        headers.insert(SPACE_ORGANIZATION, organization);
    }
    headers.insert(CLIENT, HeaderValue::from_static(GATEWAY_CLIENT));
    headers.insert(ROLES, HeaderValue::from_static(""));
}

/// Copies the caller's provenance, reason and request id from `pinned` (the caller's hop after
/// [`pin_caller`]) onto a hop the gateway makes for them as itself, so the rows it writes name
/// the caller and why, as the caller's own write does (ADR-N-054 2.3, DM-113, F-SEC1-026). The
/// grant stays the gateway's: no identity header is copied.
pub fn keep_provenance(pinned: &HeaderMap, headers: &mut HeaderMap) {
    for (name, value) in pinned {
        if name.as_str().starts_with(PROVENANCE_PREFIX) || *name == REASON || *name == REQUEST_ID {
            headers.append(name.clone(), value.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn air() -> Tenant {
        Tenant::segment("air").expect("a space name")
    }

    /// ADR-N-066 2.3 (DM-149): every attribute is `true`, a list is its names encoded, and an empty
    /// list sends nothing, which the broker reads as no grant.
    #[test]
    fn read_versions_is_true_or_the_encoded_names() {
        assert_eq!(
            read_versions(None).as_ref().map(HeaderValue::as_bytes),
            Some(&b"true"[..])
        );
        let names: std::collections::BTreeSet<String> =
            ["name".to_owned(), "air:pm 10".to_owned()].into();
        assert_eq!(
            read_versions(Some(&names))
                .as_ref()
                .map(HeaderValue::as_bytes),
            Some(&b"air%3Apm%2010,name"[..])
        );
        assert_eq!(read_versions(Some(&Default::default())), None);
    }

    #[test]
    fn a_clients_mesh_headers_never_reach_the_broker() {
        let mut request = Request::builder()
            .uri("/ngsi-ld/v1/entities")
            .header(
                "l5d-client-id",
                "context-gateway.jc.serviceaccount.identity.linkerd.cluster.local",
            )
            .header("l5d-dst-override", "broker.other:8080")
            .header("L5D-Remote-Ip", "10.0.0.1")
            .header("jc-reason", "kept")
            .body(axum::body::Body::empty())
            .expect("request");
        strip_client_headers(&mut request);
        let left: Vec<&str> = request.headers().keys().map(HeaderName::as_str).collect();
        assert_eq!(left, ["jc-reason"]);
    }

    #[test]
    fn the_channel_and_actor_come_from_the_verified_subject() {
        let account = |name: &str| Subject {
            service_account: Some(name.to_owned()),
            ..Subject::default()
        };
        assert_eq!(
            provenance(&account("pl-parking")),
            ("pipeline", "serviceAccount:pl-parking".to_owned())
        );
        assert_eq!(provenance(&account("reporting")).0, "api");
        let person = |client: Option<&str>| Subject {
            user: Some("jana".to_owned()),
            client: client.map(str::to_owned),
            ..Subject::default()
        };
        assert_eq!(
            provenance(&person(Some(PORTAL_UI_CLIENT))),
            ("ui", "user:jana".to_owned())
        );
        assert_eq!(provenance(&person(Some("my-app"))).0, "api");
        assert_eq!(provenance(&person(None)).0, "api");
        // a person is never `pipeline`, whatever account their call names
        let mut odd = person(None);
        odd.service_account = Some("pl-x".to_owned());
        assert_eq!(provenance(&odd).0, "api");
        assert_eq!(
            provenance(&Subject::default()),
            ("api", "anonymous".to_owned())
        );
    }

    #[test]
    fn every_hop_names_one_well_formed_request_id() {
        let id = |sent: &[&str]| {
            let mut headers = HeaderMap::new();
            for value in sent {
                if let Ok(value) = HeaderValue::from_str(value) {
                    headers.append(REQUEST_ID, value);
                }
            }
            request_id(&mut headers);
            headers
                .get_all(REQUEST_ID)
                .iter()
                .map(|v| v.to_str().unwrap_or_default().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(id(&["4f1c-9a07"]), ["4f1c-9a07"]);
        let long = "a".repeat(MAX_REQUEST_ID + 1);
        for sent in [
            &[][..],
            &["a b"],
            &["x;y"],
            &["one", "two"],
            &[long.as_str()],
        ] {
            let fresh = id(sent);
            assert_eq!(fresh.len(), 1, "{sent:?} -> {fresh:?}");
            assert_eq!(fresh.first().map(String::len), Some(36), "{fresh:?}");
        }
    }

    #[test]
    fn the_actor_kind_comes_from_the_verified_subject() {
        let account = |name: &str| Subject {
            service_account: Some(name.to_owned()),
            ..Subject::default()
        };
        let person = Subject {
            user: Some("jana".to_owned()),
            ..Subject::default()
        };
        let assistant = Subject {
            via: Some("assistant".to_owned()),
            ..person.clone()
        };
        assert_eq!(actor_kind(&account("pl-parking")), Some("pipeline"));
        assert_eq!(actor_kind(&account("reporting")), Some("serviceAccount"));
        assert_eq!(actor_kind(&person), Some("person"));
        assert_eq!(actor_kind(&assistant), Some("agent"));
        assert_eq!(actor_kind(&Subject::anonymous()), None);
        let participant = Subject {
            did: Some("did:web:example.org".to_owned()),
            ..Subject::default()
        };
        assert_eq!(actor_kind(&participant), None);
    }

    #[test]
    fn a_persons_write_names_the_client_it_came_through() {
        let mut request = Request::new(axum::body::Body::empty());
        request
            .headers_mut()
            .insert(PROVENANCE_CLIENT, HeaderValue::from_static("forged"));
        let person = Subject {
            user: Some("jana".to_owned()),
            client: Some("claude desktop".to_owned()),
            ..Subject::default()
        };
        pin_caller(&mut request, Some(&air()), &person).expect("a usable space");
        let named = request.headers().get_all(PROVENANCE_CLIENT);
        assert_eq!(named.iter().collect::<Vec<_>>(), ["claude%20desktop"]);
        // The client is provenance only: the broker still decides on the person alone.
        assert!(!request.headers().contains_key(CLIENT));
        // The gateway's own client and a participant under an agreement name no client.
        for caller in [
            Subject {
                client: Some(GATEWAY_CLIENT.to_owned()),
                ..Subject::default()
            },
            Subject {
                did: Some("did:web:example.org".to_owned()),
                agreement: Some("a1".to_owned()),
                client: Some("connector".to_owned()),
                ..Subject::default()
            },
        ] {
            let mut hop = Request::new(axum::body::Body::empty());
            pin_caller(&mut hop, Some(&air()), &caller).expect("a usable space");
            assert!(!hop.headers().contains_key(PROVENANCE_CLIENT));
        }
    }

    /// EP-169 (T-6409): the broker hears the groups a `group` grant is matched against: the
    /// person's groups in the Endpoint's Organization once the token carries `jc_groups`.
    #[test]
    fn the_broker_hears_the_groups_of_the_organization_when_the_token_has_them() {
        let groups = |organization_groups: Option<&[&str]>| {
            let person = Subject {
                user: Some("jana".to_owned()),
                groups: ["analysts".to_owned(), "helsinki/analysts".to_owned()].into(),
                organization_groups: organization_groups
                    .map(|names| names.iter().map(|name| (*name).to_owned()).collect()),
                ..Subject::default()
            };
            let mut request = Request::new(axum::body::Body::empty());
            pin_caller(&mut request, Some(&air()), &person).expect("a usable space");
            request.headers()[GROUPS]
                .to_str()
                .map(str::to_owned)
                .expect("ascii")
        };
        assert_eq!(groups(Some(&["analysts"])), "analysts");
        assert_eq!(groups(None), "analysts,helsinki%2Fanalysts");
    }

    /// ADR-N-067 2.3: the broker hears `x-jc-member` only for a member the gateway verified; a
    /// client's own copy never crosses.
    #[test]
    fn the_broker_hears_a_member_only_when_the_gateway_verified_one() {
        let member = |member_of_organization: bool| {
            let mut request = Request::new(axum::body::Body::empty());
            request
                .headers_mut()
                .insert(MEMBER, HeaderValue::from_static("true"));
            let person = Subject {
                user: Some("jana".to_owned()),
                member_of_organization,
                ..Subject::default()
            };
            pin_caller(&mut request, Some(&air()), &person).expect("a usable space");
            request
                .headers()
                .get_all(MEMBER)
                .iter()
                .map(|v| v.to_str().map(str::to_owned).expect("ascii"))
                .collect::<Vec<_>>()
        };
        assert_eq!(member(true), ["true"]);
        assert!(member(false).is_empty(), "{:?}", member(false));
    }

    #[test]
    fn a_hop_names_the_organization_and_kind_and_an_mcp_call_is_an_agents() {
        let mut request = Request::new(axum::body::Body::empty());
        request
            .headers_mut()
            .insert(PROVENANCE_ORGANIZATION, HeaderValue::from_static("forged"));
        let person = Subject {
            user: Some("jana".to_owned()),
            organization: Some("Banská Bystrica".to_owned()),
            ..Subject::default()
        };
        pin_caller(&mut request, Some(&air()), &person).expect("a usable space");
        let headers = request.headers_mut();
        assert_eq!(
            headers
                .get_all(PROVENANCE_ORGANIZATION)
                .iter()
                .collect::<Vec<_>>(),
            ["Bansk%C3%A1%20Bystrica"]
        );
        assert_eq!(headers[PROVENANCE_ACTOR_KIND], "person");
        as_agent(headers);
        assert_eq!(headers[PROVENANCE_ACTOR_KIND], "agent");
        // An anonymous hop names no kind, and an MCP call does not invent one.
        let mut anonymous = Request::new(axum::body::Body::empty());
        pin_caller(&mut anonymous, Some(&air()), &Subject::anonymous()).expect("a usable space");
        as_agent(anonymous.headers_mut());
        assert!(!anonymous.headers().contains_key(PROVENANCE_ACTOR_KIND));
        assert!(!anonymous.headers().contains_key(PROVENANCE_ORGANIZATION));
    }

    #[test]
    fn a_clients_provenance_headers_never_survive() {
        let mut headers = HeaderMap::new();
        for name in [
            "jc-provenance-actor",
            "jc-provenance-organization",
            "jc-provenance-actor-kind",
            "jc-provenance-audit",
            "jc-provenance-x",
            "jc-read-versions",
        ] {
            headers.append(name, HeaderValue::from_static("forged"));
            headers.append(name, HeaderValue::from_static("twice"));
        }
        headers.insert(REASON, HeaderValue::from_static("kept"));
        drop_identity(&mut headers);
        assert_eq!(headers.len(), 1, "{headers:?}");
    }

    /// One copy of the rules (jc-preview-subject): every `x-jc-*` header a hop carries about the
    /// caller is [`caller_headers`]' for that subject, for each kind of caller, and a forged
    /// identity header the request brought never survives beside them.
    #[test]
    fn a_hop_names_the_caller_exactly_as_caller_headers_do() {
        let person = Subject {
            user: Some("jana".to_owned()),
            id: Some("2f0c".to_owned()),
            groups: ["analysts".to_owned()].into(),
            organization_groups: Some(["helsinki-team".to_owned()].into()),
            member_of_organization: true,
            roles: ["endpoint:city/air".to_owned()].into(),
            read_only: true,
            ..Subject::default()
        };
        let account = Subject {
            service_account: Some("ingest".to_owned()),
            service_account_project: Some("city".to_owned()),
            roles: ["writer".to_owned()].into(),
            ..Subject::default()
        };
        let participant = Subject {
            did: Some("did:web:a.example".to_owned()),
            agreement: Some("agr-1".to_owned()),
            ..Subject::default()
        };
        for subject in [
            Subject::anonymous(),
            person,
            account,
            participant,
            Subject::default(),
        ] {
            let mut request = Request::new(axum::body::Body::empty());
            for name in [SUBJECT, ROLES, GROUPS, MEMBER, CLIENT, DID] {
                request
                    .headers_mut()
                    .insert(name, HeaderValue::from_static("forged"));
            }
            pin_caller(&mut request, Some(&air()), &subject).expect("a usable space");
            let identity = |headers: &HeaderMap| {
                let mut pairs: Vec<(String, String)> = headers
                    .iter()
                    .filter(|(name, _)| {
                        [
                            &SUBJECT, &ROLES, &GROUPS, &MEMBER, &CLIENT, &DID, &READ_ONLY,
                        ]
                        .contains(name)
                    })
                    .map(|(name, value)| {
                        (name.to_string(), value.to_str().unwrap_or("?").to_owned())
                    })
                    .collect();
                pairs.sort();
                pairs
            };
            assert_eq!(
                identity(request.headers()),
                identity(&caller_headers(&subject)),
                "{subject:?}"
            );
            assert!(
                !request.headers().values().any(|v| v == "forged"),
                "{subject:?}: {:?}",
                request.headers()
            );
        }
    }

    fn named(raw: &str) -> Tenant {
        Tenant::named(&TenantName::parse(raw).expect("a tenant name")).expect("a header value")
    }

    /// EP-188, PF-130, ARCH/TEN-10: an Organization tenant (`{org}__{segment}`, and the
    /// project-key `{OrgId}__{ProjectId}__{space}`) pins its name and its Organization half on
    /// every hop, and a member's hop, whose `jc_org_id` names the space's Organization, carries
    /// that half, per admission path; what a client sent under those names never survives.
    #[test]
    fn an_organization_tenant_names_its_organization_per_admission_path() {
        let ids = |ids: &[&str]| ids.iter().map(|id| (*id).to_owned()).collect();
        let member = |organization_ids| Subject {
            user: Some("jana".to_owned()),
            id: Some("2f0c".to_owned()),
            member_of_organization: true,
            organization_ids,
            ..Subject::default()
        };
        let cases = [
            (
                "member",
                member(ids(&["fi_hel", "sk_bbsk"])),
                Some("sk_bbsk"),
            ),
            ("member without the id", member(ids(&["fi_hel"])), None),
            ("member, realm without the mapper", member(ids(&[])), None),
            (
                "grant to another Organization",
                Subject {
                    user: Some("eva".to_owned()),
                    organization_ids: ids(&["sk_bbsk"]),
                    ..Subject::default()
                },
                None,
            ),
            (
                "service account",
                Subject {
                    service_account: Some("ingest".to_owned()),
                    service_account_project: Some("mobility".to_owned()),
                    ..Subject::default()
                },
                None,
            ),
            ("public", Subject::anonymous(), None),
            (
                "agreement",
                Subject {
                    did: Some("did:web:a.example".to_owned()),
                    agreement: Some("agr-1".to_owned()),
                    ..Subject::default()
                },
                None,
            ),
        ];
        let key = jc_types::ProjectKey::new(jc_types::OrgId::mint(), jc_types::ProjectId::mint());
        let keyed = Tenant::keyed(
            &TenantName::of_space(key, "mobility").expect("a space name"),
            &jc_core::names::OrganizationId::parse("sk_bbsk").expect("an id"),
        )
        .expect("a header value");
        for (tenant, path, subject, home) in [named("sk_bbsk__mobility"), keyed]
            .into_iter()
            .flat_map(|tenant| {
                cases
                    .iter()
                    .map(move |(path, subject, home)| (tenant.clone(), path, subject, home))
            })
        {
            let org = tenant.organization().expect("an Organization half").to_owned();
            let home = home.map(|_| org.as_str());
            let mut request = Request::new(axum::body::Body::empty());
            for (name, value) in [
                ("ngsild-tenant", "sk_evil__mobility"),
                ("x-jc-organization-id", "sk_evil"),
                ("x-jc-space-organization", "sk_evil"),
            ] {
                request
                    .headers_mut()
                    .append(name, HeaderValue::from_static(value));
            }
            strip_client_headers(&mut request);
            pin_caller(&mut request, Some(&tenant), subject).expect("a tenant");
            let headers = request.headers();
            let all = |name: &str| {
                headers
                    .get_all(name)
                    .iter()
                    .map(|v| v.to_str().unwrap_or("?"))
                    .collect::<Vec<_>>()
            };
            assert_eq!(all("ngsild-tenant"), [tenant.as_str()], "{path}");
            assert_eq!(all("x-jc-space-organization"), [org.as_str()], "{path}");
            assert_eq!(
                all("x-jc-organization-id"),
                home.into_iter().collect::<Vec<_>>(),
                "{path}"
            );
            // The gateway's own hops for the request keep the space's Organization alone.
            let mut own = HeaderMap::new();
            copy_tenant(headers, &mut own);
            as_gateway(&mut own);
            assert_eq!(pinned_tenant(&own), Some(tenant.as_str()), "{path}");
            assert_eq!(own[&SPACE_ORGANIZATION], org.as_str(), "{path}");
            assert!(!own.contains_key(&ORGANIZATION_ID), "{path}");
        }
    }

    /// PF-130: two Organizations with the same project and space names get two tenants; a
    /// segment tenant names no Organization, so even a member's hop carries none; an Endpoint
    /// without a tenant pins nothing and answers 503.
    #[test]
    fn a_segment_tenant_names_no_organization_and_no_tenant_is_503() {
        let (bbsk, hel) = (named("sk_bbsk__city-air"), named("fi_hel__city-air"));
        assert_ne!(bbsk, hel);
        assert_eq!(
            (bbsk.organization(), hel.organization()),
            (Some("sk_bbsk"), Some("fi_hel"))
        );
        let member = Subject {
            member_of_organization: true,
            organization_ids: ["sk_bbsk".to_owned()].into(),
            ..Subject::default()
        };
        let mut request = Request::new(axum::body::Body::empty());
        pin_caller(&mut request, Some(&air()), &member).expect("a tenant");
        assert_eq!(pinned_tenant(request.headers()), Some("air"));
        assert!(!request.headers().contains_key(&SPACE_ORGANIZATION));
        assert!(!request.headers().contains_key(&ORGANIZATION_ID));
        let mut request = Request::new(axum::body::Body::empty());
        assert_eq!(pin_caller(&mut request, None, &member), Err(Unpinned));
        assert!(pinned_tenant(request.headers()).is_none());
        assert_eq!(Unpinned.problem().status, 503);
        for bad in ["", "Air", "air\r\nx-userinfo: root", "sk_bbsk__air"] {
            assert!(Tenant::segment(bad).is_err(), "{bad}");
        }
    }

    /// Design 10.7: no module but this one names the tenant header, so every pin goes through
    /// [`pin_tenant`] with a [`Tenant`]; and only the store builds a segment tenant.
    #[test]
    fn only_this_module_names_the_tenant_header() {
        fn sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).expect("src").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    sources(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        sources(&root, &mut files);
        assert!(files.len() > 50);
        let mut found = Vec::new();
        for file in files {
            let text = std::fs::read_to_string(&file).expect("a source file");
            // Test modules forge the header on purpose.
            let code = ["#[cfg(test)]\nmod ", "#[cfg(test)]\npub(crate) mod "]
                .iter()
                .filter_map(|marker| text.find(marker))
                .min()
                .map_or(text.as_str(), |end| &text[..end]);
            let name = file
                .strip_prefix(&root)
                .unwrap_or(&file)
                .display()
                .to_string();
            let lowered = code.to_ascii_lowercase();
            if name != "middleware/tenancy.rs"
                && (lowered.contains("ngsild-tenant") || code.contains("tenancy::TENANT"))
            {
                found.push(format!("{name}: names the tenant header"));
            }
            if !["middleware/tenancy.rs", "store.rs"].contains(&name.as_str())
                && code.contains("Tenant::segment(")
            {
                found.push(format!("{name}: builds a segment tenant"));
            }
        }
        assert!(found.is_empty(), "{found:#?}");
    }
}
