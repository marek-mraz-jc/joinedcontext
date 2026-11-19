//! `GET /organization/portal-hosts` (ODP-08, ODP-15, T-6875): for each Organization the caller may
//! make a `kind: Portal` in, the hosts the operator listed for it and its open data base address.
//! The Portals editor offers these hosts as its only choices; the judge checks the submitted host
//! again (`portals::check`), so this list is a convenience, never the authority.
//!
//! `POST` and `DELETE /organization/portal-hosts` (T-7415): an Organization's administrator adds
//! a portal host without an operator when it is one label under a host the Organization serves
//! (`opendata2.bbsk.smartcity.example`) or a name under the domain the Organization verified by
//! DNS TXT, the proof read again at the add. The host enters the registry `approved` with the
//! role `portal-site`, and the reconciler's host wave takes it through DNS, its own HTTP-01
//! certificate (paced) and the edge route to `served`, with no restart. Any other name stays an
//! operator `HostAdd`. Every add and removal writes one `operator_audit` row.

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

use crate::auth::session::Identity;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::host_registry::{self, HostRow, Proposal};
use crate::permissions::ORG_NAMESPACE;
use crate::portals::SITE_ROLE;
use crate::state::AppState;
use jc_core::kinds::Verb;

/// Self-service portal hosts one Organization may add in a rolling 24 hours: each takes one of
/// the instance's paced certificates (`org_hosts::PER_INSTANCE_DAY`), so one Organization never
/// spends them all.
pub const SITES_PER_DAY: i64 = 3;

/// One of the Organization's own portal hosts in the host registry.
#[derive(Debug, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SiteHost {
    /// e.g. `opendata2.bbsk.smartcity.joinedcontext.com`.
    pub host: String,
    /// `approved`, `dns-ok`, `cert-issued` or `served`; only `served` answers.
    pub state: String,
    /// What the host wave last said of it, e.g. a DNS answer or until when a certificate waits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Whether this Portal can remove it (a host jc.toml seeded is the operator's).
    pub removable: bool,
}

/// One Organization the caller manages portals of.
#[derive(Debug, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PortalOrganization {
    /// The Organization's name, the `spec.organizationRef` of its portals, e.g. `bratislava`.
    pub organization: String,
    /// The hosts the operator listed for its portals, e.g. `mobility.bratislava.sk`.
    pub hosts: Vec<String>,
    /// Its open data portal's public base, e.g. `https://opendata.bratislava.sk`; absent when
    /// this Portal knows none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opendata: Option<String>,
    /// Its own portal hosts in the host registry, with their state (T-7415).
    pub sites: Vec<SiteHost>,
    /// The hosts it serves that a new portal host may sit one label under, e.g.
    /// `bbsk.smartcity.joinedcontext.com`.
    pub bases: Vec<String>,
    /// The domain it verified by DNS TXT, under which it may name any host, e.g. `bbsk.sk`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified_domain: Option<String>,
}

/// The Organizations the caller makes portals in, by name.
#[derive(Debug, Serialize, ToSchema)]
pub struct PortalHosts {
    pub items: Vec<PortalOrganization>,
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/portal-hosts",
    summary = "List Portal Hosts",
    description = "Each Organization the caller may propose a Portal in (its administrators, the installation's for every one), with the hosts the operator listed for its portals and its open data base address (ODP-08, ODP-15). A Portal naming another host is refused at propose with 422 on spec.host.",
    tag = "system",
    responses(
        (status = 200, description = "The caller's Organizations, by name", body = PortalHosts),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "The caller makes portals in no Organization", body = ProblemDetails)
    )
)]
pub async fn list_portal_hosts(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<PortalHosts>, ApiError> {
    let items = entries(&state, &user.0.identity).await?;
    if items.is_empty() {
        return Err(ApiError::Denied(
            "only an Organization's administrator makes its open data portals: it needs propose \
             on Portal (ODP-12)"
                .into(),
        ));
    }
    Ok(Json(PortalHosts { items }))
}

/// The Organizations `identity` makes portals in, each with its hosts.
async fn entries(
    state: &AppState,
    identity: &Identity,
) -> Result<Vec<PortalOrganization>, ApiError> {
    let settings = &state.config.portal_settings;
    let registry = Registry::read(state).await?;
    let mut items = Vec::new();
    for (organization, _) in crate::permissions::organizations(&state.mirror) {
        if !may_make_portals(state, identity, &organization) {
            continue;
        }
        items.push(PortalOrganization {
            hosts: settings
                .hosts
                .get(&organization)
                .cloned()
                .unwrap_or_default(),
            opendata: state
                .organization_catalogues
                .current()
                .iter()
                .find(|c| c.organization == organization)
                .map(|c| c.url.as_str().trim_end_matches('/').to_owned()),
            sites: registry.sites(&organization),
            bases: registry.bases(&organization),
            verified_domain: registry.verified(state, &organization).await?,
            organization,
        });
    }
    Ok(items)
}

/// Whether `identity` may propose a Portal in `organization` (its administrators, the
/// installation's for every one).
fn may_make_portals(state: &AppState, identity: &Identity, organization: &str) -> bool {
    let probe = json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "Portal",
        "metadata": { "name": format!("{organization}-probe"), "namespace": ORG_NAMESPACE },
        "spec": { "organizationRef": organization, "path": "probe" },
    });
    crate::permissions::for_request(state, identity, ORG_NAMESPACE)
        .check("Portal", Verb::Propose, Some(&probe))
        .is_ok()
}

/// `403` for somebody who makes portals in no Organization, before anything else is read.
fn makes_portals_somewhere(state: &AppState, identity: &Identity) -> Result<(), ApiError> {
    let any = crate::permissions::organizations(&state.mirror)
        .into_iter()
        .any(|(organization, _)| may_make_portals(state, identity, &organization));
    if any {
        Ok(())
    } else {
        Err(ApiError::Denied(
            "only an Organization's administrator manages its portal hosts: it needs propose on \
             Portal (ODP-12)"
                .into(),
        ))
    }
}

/// The registry rows read for one answer; empty without a database.
struct Registry {
    rows: Vec<Row>,
}

#[derive(sqlx::FromRow)]
struct Row {
    host: String,
    organization: String,
    organization_id: Option<String>,
    role: String,
    state: String,
    created_by: String,
    detail: Option<String>,
}

impl Registry {
    async fn read(state: &AppState) -> Result<Self, ApiError> {
        let Some(pool) = state.db.as_ref() else {
            return Ok(Self { rows: Vec::new() });
        };
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT host, organization, organization_id, role, state, created_by, detail \
             FROM organization_hosts",
        )
        .fetch_all(pool)
        .await
        .map_err(db)?;
        let registry = Self { rows };
        state
            .config
            .portal_settings
            .sites
            .replace(&registry.host_rows());
        Ok(registry)
    }

    fn host_rows(&self) -> Vec<HostRow> {
        self.rows
            .iter()
            .map(|r| HostRow {
                host: r.host.clone(),
                organization: r.organization.clone(),
                organization_id: r.organization_id.clone(),
                role: r.role.clone(),
                state: r.state.clone(),
            })
            .collect()
    }

    fn sites(&self, organization: &str) -> Vec<SiteHost> {
        self.rows
            .iter()
            .filter(|r| r.organization == organization && r.role == SITE_ROLE)
            .filter(|r| r.state != "removed")
            .map(|r| SiteHost {
                host: r.host.clone(),
                state: r.state.clone(),
                detail: r.detail.clone(),
                removable: r.created_by != host_registry::SEEDED_BY,
            })
            .collect()
    }

    fn bases(&self, organization: &str) -> Vec<String> {
        self.rows
            .iter()
            .filter(|r| r.organization == organization && r.state == "served")
            .filter(|r| r.role == "working")
            .map(|r| r.host.clone())
            .collect()
    }

    /// The domain of the minted Organization id `organization` links to.
    async fn verified(
        &self,
        state: &AppState,
        organization: &str,
    ) -> Result<Option<String>, ApiError> {
        Ok(verified_id(state, organization)
            .await?
            .map(|(_, domain)| domain))
    }
}

/// The minted Organization id of `organization` and its verified domain: the registry row's id,
/// else the one its manifest's `spec.domain` derives (as the registry's own check does).
async fn verified_id(
    state: &AppState,
    organization: &str,
) -> Result<Option<(String, String)>, ApiError> {
    let Some(pool) = state.db.as_ref() else {
        return Ok(None);
    };
    let row: Option<Option<String>> = sqlx::query_scalar(
        "SELECT organization_id FROM organizations WHERE name = $1 AND state = 'active'",
    )
    .bind(organization)
    .fetch_optional(pool)
    .await
    .map_err(db)?;
    let Some(id) = row
        .flatten()
        .or_else(|| crate::reconciler::groups::organization_id(&state.mirror, organization))
    else {
        return Ok(None);
    };
    let domain: Option<String> = sqlx::query_scalar(
        "SELECT domain FROM organization_ids WHERE id = $1 AND minted_at IS NOT NULL",
    )
    .bind(&id)
    .fetch_optional(pool)
    .await
    .map_err(db)?;
    Ok(domain.map(|d| (id, d)))
}

/// How a self-service host is under its Organization.
#[derive(Debug, PartialEq, Eq)]
pub enum Under {
    /// One label under a host the Organization serves.
    OwnHost,
    /// Under the Organization's verified domain.
    VerifiedDomain,
}

fn unprocessable(field: &str, message: String) -> ApiError {
    ApiError::Unprocessable {
        detail: message.clone(),
        errors: vec![format!("{field}: {message}")],
    }
}

/// Whether an administrator of `organization` adds `host` without an operator: one label, not
/// reserved by the installation, under one of `bases` (the hosts it serves), or any name under
/// `verified` (its DNS-verified domain). `422` on `host` otherwise, naming what to do instead.
pub fn self_service(
    host: &str,
    bases: &[String],
    verified: Option<&str>,
    reserved: &[&str],
) -> Result<Under, ApiError> {
    let Some((label, parent)) = host.split_once('.') else {
        return Err(unprocessable(
            "host",
            format!("'{host}' is not a full host name, e.g. opendata2.bbsk.smartcity.example"),
        ));
    };
    if bases.iter().any(|b| b == parent) {
        // Under the installation's names the platform's own labels stay its own; under a
        // verified domain of the Organization they collide with nothing.
        if reserved.contains(&label) {
            return Err(unprocessable(
                "host",
                format!(
                    "the label '{label}' is reserved for the platform; choose another, e.g. \
                     opendata2"
                ),
            ));
        }
        return Ok(Under::OwnHost);
    }
    if verified.is_some_and(|domain| host == domain || host.ends_with(&format!(".{domain}"))) {
        return Ok(Under::VerifiedDomain);
    }
    let own = match (bases.first(), verified) {
        (Some(base), Some(domain)) => format!("e.g. opendata2.{base} or data.{domain}"),
        (Some(base), None) => format!("e.g. opendata2.{base}"),
        (None, Some(domain)) => format!("e.g. data.{domain}"),
        (None, None) => "the Organization serves no host and verified no domain yet".into(),
    };
    Err(unprocessable(
        "host",
        format!(
            "'{host}' is neither one label under a host of this Organization nor under its \
             verified domain ({own}); any other host needs an operator's HostAdd"
        ),
    ))
}

/// A portal host to add.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[schema(example = json!({ "organization": "bbsk", "host": "opendata2.bbsk.smartcity.joinedcontext.com" }))]
pub struct NewSiteHost {
    /// The Organization, e.g. `bbsk`.
    pub organization: String,
    /// The host, e.g. `opendata2.bbsk.smartcity.joinedcontext.com` or `data.bbsk.sk`.
    pub host: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/organization/portal-hosts",
    summary = "Add Portal Host",
    description = "An Organization's administrator adds a host its portals may answer on, without an operator, when it is one label under a host the Organization serves or a name under its DNS-verified domain (the TXT proof is read again now). The host enters the registry approved and turns served once DNS points at the edge and its own certificate is issued (paced; the state and detail say until when). Any other name is an operator HostAdd (T-7415).",
    tag = "system",
    request_body = NewSiteHost,
    responses(
        (status = 201, description = "Added; the Organization's portal hosts now", body = PortalOrganization),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "The caller does not make portals in this Organization", body = ProblemDetails),
        (status = 409, description = "The host is in the registry already (a removed host stays taken)", body = ProblemDetails),
        (status = 422, description = "The host is not this Organization's to add, a reserved label, or its domain proof lapsed", body = ProblemDetails),
        (status = 429, description = "The Organization added its daily share of portal hosts", body = ProblemDetails),
        (status = 503, description = "No database or the installation's domain is unknown here", body = ProblemDetails)
    )
)]
pub async fn add_portal_host(
    user: CurrentUser,
    State(state): State<AppState>,
    client: Option<Extension<jc_router::ClientAddr>>,
    body: Result<Json<NewSiteHost>, axum::extract::rejection::JsonRejection>,
) -> Result<(StatusCode, Json<PortalOrganization>), ApiError> {
    let identity = &user.0.identity;
    // Somebody who makes portals nowhere learns nothing about the body's shape.
    makes_portals_somewhere(&state, identity)?;
    let Json(request) = body.map_err(|rejection| {
        unprocessable(
            "body",
            format!(
                "{}; send organization and host, e.g. {{\"organization\": \"bbsk\", \"host\": \"opendata2.bbsk.smartcity.example\"}}",
                rejection.body_text()
            ),
        )
    })?;
    let organization = request.organization.trim().to_owned();
    if !may_make_portals(&state, identity, &organization) {
        return Err(ApiError::Denied(format!(
            "organization: you do not make portals in '{organization}'; its administrator adds \
             its hosts"
        )));
    }
    let host = request
        .host
        .trim()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let rules = crate::api::operator_changes::rules(&state)?;
    let pool = pool(&state)?;
    let registry = Registry::read(&state).await?;
    let verified = verified_id(&state, &organization).await?;
    let under = self_service(
        &host,
        &registry.bases(&organization),
        verified.as_ref().map(|(_, d)| d.as_str()),
        rules.reserved,
    )?;
    let organization_id = match (&under, &verified) {
        (Under::VerifiedDomain, Some((id, _))) => {
            // The proof is read again now, as an operator's approval reads it.
            crate::api::operator_changes::txt_holds(&state, pool, id)
                .await
                .map_err(|e| match e {
                    ApiError::Unprocessable { detail, .. } => unprocessable("host", detail),
                    other => other,
                })?;
            Some(id.as_str())
        }
        _ => None,
    };
    let by = identity.subject.as_str();
    let mut tx = pool.begin().await.map_err(db)?;
    // One writer per Organization at a time, so the daily share cannot be raced past.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('portal-site:' || $1))")
        .bind(&organization)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    let today: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM organization_hosts WHERE organization = $1 AND role = $2 \
         AND created_by <> $3 AND created_at > now() - interval '24 hours'",
    )
    .bind(&organization)
    .bind(SITE_ROLE)
    .bind(host_registry::SEEDED_BY)
    .fetch_one(&mut *tx)
    .await
    .map_err(db)?;
    if today >= SITES_PER_DAY {
        return Err(ApiError::TooManyRequests(format!(
            "host: {organization} added {SITES_PER_DAY} portal hosts in the last 24 hours, the \
             most a day (each takes one of the installation's paced certificates); add the next \
             one tomorrow"
        )));
    }
    host_registry::add_approved(
        &mut tx,
        &state.mirror,
        &rules,
        &Proposal {
            host: &host,
            organization: &organization,
            organization_id,
            role: SITE_ROLE,
            by,
        },
        by,
    )
    .await?;
    audit(
        &mut tx,
        by,
        "portal-host-add",
        &json!({ "host": host, "organization": organization, "role": SITE_ROLE }),
        source(client),
    )
    .await?;
    tx.commit().await.map_err(db)?;
    Ok((
        StatusCode::CREATED,
        Json(answer(&state, identity, &organization).await?),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/v1/organization/portal-hosts/{host}",
    summary = "Remove Portal Host",
    description = "An Organization's administrator removes one of its portal hosts. The edge stops routing it first; its certificate is deleted after that. A removed name stays taken in the registry. Refused while a Portal answers on it, and for a host jc.toml seeded (the operator's) (T-7415).",
    tag = "system",
    params(("host" = String, Path, description = "The host, e.g. `opendata2.bbsk.smartcity.joinedcontext.com`")),
    responses(
        (status = 200, description = "Removed; the Organization's portal hosts now", body = PortalOrganization),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "The caller does not make portals in the host's Organization, or jc.toml seeded the host", body = ProblemDetails),
        (status = 404, description = "No portal host of that name the caller may see", body = ProblemDetails),
        (status = 409, description = "A Portal answers on the host; move or delete it first", body = ProblemDetails),
        (status = 503, description = "No database here", body = ProblemDetails)
    )
)]
pub async fn remove_portal_host(
    user: CurrentUser,
    State(state): State<AppState>,
    client: Option<Extension<jc_router::ClientAddr>>,
    Path(host): Path<String>,
) -> Result<Json<PortalOrganization>, ApiError> {
    let identity = &user.0.identity;
    makes_portals_somewhere(&state, identity)?;
    let host = host.trim().to_ascii_lowercase();
    let pool = pool(&state)?;
    let registry = Registry::read(&state).await?;
    let missing = || ApiError::NotFound(format!("host: '{host}' is no portal host you manage"));
    let row = registry
        .rows
        .iter()
        .find(|r| r.host == host && r.role == SITE_ROLE && r.state != "removed")
        .ok_or_else(missing)?;
    // Another Organization's host reads as not there.
    if !may_make_portals(&state, identity, &row.organization) {
        return Err(missing());
    }
    if row.created_by == host_registry::SEEDED_BY {
        return Err(ApiError::Denied(format!(
            "host: '{host}' comes from the installation's configuration; an operator removes it"
        )));
    }
    if let Some(portal) = portal_on(&state, &host) {
        return Err(ApiError::Conflict(format!(
            "host: the portal {portal} answers on '{host}'; give it another host or delete it first"
        )));
    }
    let organization = row.organization.clone();
    let by = identity.subject.as_str();
    let mut tx = pool.begin().await.map_err(db)?;
    if !host_registry::remove_in(&mut tx, &host).await.map_err(db)? {
        return Err(missing());
    }
    audit(
        &mut tx,
        by,
        "portal-host-remove",
        &json!({ "host": host, "organization": organization, "role": SITE_ROLE }),
        source(client),
    )
    .await?;
    tx.commit().await.map_err(db)?;
    Ok(Json(answer(&state, identity, &organization).await?))
}

/// The Portal manifest whose `spec.host` is `host`.
fn portal_on(state: &AppState, host: &str) -> Option<String> {
    state
        .mirror
        .list(
            ORG_NAMESPACE,
            "Portal",
            &crate::store::ListOptions::default(),
        )
        .items
        .into_iter()
        .find(|e| {
            e.spec
                .get("host")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|h| h.eq_ignore_ascii_case(host))
        })
        .map(|e| e.metadata.name)
}

/// `organization`'s entry as the list answers it, after a write.
async fn answer(
    state: &AppState,
    identity: &Identity,
    organization: &str,
) -> Result<PortalOrganization, ApiError> {
    entries(state, identity)
        .await?
        .into_iter()
        .find(|o| o.organization == organization)
        .ok_or_else(|| ApiError::NotFound(format!("organization: '{organization}'")))
}

async fn audit(
    conn: &mut sqlx::PgConnection,
    actor: &str,
    action: &str,
    after: &serde_json::Value,
    source: Option<String>,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO operator_audit (actor, action, after, source_ip) VALUES ($1, $2, $3, $4)",
    )
    .bind(actor)
    .bind(action)
    .bind(after)
    .bind(source.as_deref())
    .execute(conn)
    .await
    .map_err(db)?;
    tracing::info!(target: "operator_audit", actor, action, after = %after, "operator audit");
    Ok(())
}

fn source(client: Option<Extension<jc_router::ClientAddr>>) -> Option<String> {
    client.map(|Extension(addr)| addr.0.to_string())
}

fn pool(state: &AppState) -> Result<&sqlx::PgPool, ApiError> {
    state.db.as_ref().ok_or_else(|| {
        ApiError::Unavailable(
            "portal hosts are kept in the database, and this Portal has none".into(),
        )
    })
}

fn db(e: sqlx::Error) -> ApiError {
    tracing::error!(error = %e, "portal hosts");
    ApiError::Unavailable("the host registry could not be read or written; try again".into())
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/organization/portal-hosts",
            get(list_portal_hosts).post(add_portal_host),
        )
        .route(
            "/organization/portal-hosts/{host}",
            delete(remove_portal_host),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESERVED: &[&str] = &["api", "idm", "login", "opendata"];

    fn bases() -> Vec<String> {
        vec!["bbsk.smartcity.example".into()]
    }

    fn field(err: ApiError) -> String {
        match err {
            ApiError::Unprocessable { errors, .. } => errors.join(" "),
            other => panic!("not 422: {other:?}"),
        }
    }

    #[test]
    fn one_label_under_an_own_host_or_a_name_under_the_verified_domain_is_self_service() {
        let b = bases();
        assert_eq!(
            self_service(
                "opendata2.bbsk.smartcity.example",
                &b,
                Some("bbsk.sk"),
                RESERVED
            )
            .ok(),
            Some(Under::OwnHost)
        );
        for host in ["data.bbsk.sk", "a.b.bbsk.sk"] {
            assert_eq!(
                self_service(host, &b, Some("bbsk.sk"), RESERVED).ok(),
                Some(Under::VerifiedDomain),
                "{host}"
            );
        }
    }

    #[test]
    fn another_organizations_host_an_unverified_domain_and_two_labels_are_refused_on_host() {
        let b = bases();
        for host in [
            // under another Organization's host
            "opendata2.praha.smartcity.example",
            // two labels under the own host
            "a.b.bbsk.smartcity.example",
            // a look-alike of the verified domain
            "data.xbbsk.sk",
            "bbsk.sk.evil.example",
            // an unverified domain
            "data.praha.eu",
            "localhost",
        ] {
            let line = field(self_service(host, &b, Some("bbsk.sk"), RESERVED).unwrap_err());
            assert!(line.starts_with("host: "), "{host}: {line}");
        }
        let none = field(self_service("data.bbsk.sk", &b, None, RESERVED).unwrap_err());
        assert!(none.contains("HostAdd"), "{none}");
    }

    #[test]
    fn a_label_the_installation_reserves_is_refused() {
        for label in RESERVED {
            let host = format!("{label}.bbsk.smartcity.example");
            let line = field(self_service(&host, &bases(), None, RESERVED).unwrap_err());
            assert!(line.contains("reserved"), "{host}: {line}");
        }
    }
}
