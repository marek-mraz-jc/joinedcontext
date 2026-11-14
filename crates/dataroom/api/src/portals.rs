//! The further portals of an Organization (ODP-08): `default` answers at `/{org}/opendata/`,
//! every other portal at its slug address `/{org}/opendata/-/{path}/` or on a host the operator
//! listed for the Organization. The `-` segment is no dataset, group or page name, so a portal
//! never shadows an address of `default`. The catalogue writer fills both tables at runtime.

use crate::problem_response;
use axum::extract::{Request, State};
use axum::http::{header, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Router;
use dataroom_core::{CatalogueId, DataroomError, OrgId};
use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError, RwLock};

/// The catalogue behind one slug address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalAddress {
    pub org_id: OrgId,
    pub id: CatalogueId,
    /// The base of the portal's own host (`https://mobility.city.sk`) when its slug address
    /// answers `308` there (`slug: redirect`); `None` serves the slug address itself.
    pub redirect: Option<String>,
}

/// Every further portal's slug address and own host, swapped by the catalogue writer.
#[derive(Debug, Clone, Default)]
pub struct PortalTables {
    /// `(Organization, path)` to its catalogue.
    pub slugs: Arc<RwLock<BTreeMap<(String, String), PortalAddress>>>,
    /// A portal's own host to its catalogue.
    pub hosts: Arc<RwLock<BTreeMap<String, (OrgId, CatalogueId)>>>,
}

impl PortalTables {
    /// The catalogue of `org`'s portal `path`, when it has one.
    pub fn slug(&self, org: &str, path: &str) -> Option<PortalAddress> {
        read(&self.slugs)
            .get(&(org.to_owned(), path.to_owned()))
            .cloned()
    }

    /// The catalogue of the portal on `host`, when one is there.
    pub fn host(&self, host: &str) -> Option<(OrgId, CatalogueId)> {
        read(&self.hosts).get(host).copied()
    }

    /// Routes catalogue `id` at `(org, path)`, on `host` and on each host in `left` it no
    /// longer has (its catalogue row keeps them), dropping its previous slug: an IRI pinned on
    /// a left host still reaches its dataset. A left host another portal has taken stays that
    /// portal's.
    pub fn route(
        &self,
        id: CatalogueId,
        org: &str,
        path: &str,
        host: Option<&str>,
        left: &[String],
        address: PortalAddress,
    ) {
        write(&self.slugs).retain(|_, a| a.id != id);
        let mut hosts = write(&self.hosts);
        for gone in left {
            hosts.entry(gone.clone()).or_insert((address.org_id, id));
        }
        if let Some(host) = host {
            hosts.insert(host.to_owned(), (address.org_id, id));
        }
        drop(hosts);
        write(&self.slugs).insert((org.to_owned(), path.to_owned()), address);
    }

    /// Takes every route of catalogue `id` away.
    pub fn unroute(&self, id: CatalogueId) {
        write(&self.slugs).retain(|_, a| a.id != id);
        write(&self.hosts).retain(|_, (_, c)| *c != id);
    }
}

/// The host a path-mode portal's catalogue row stores (its host column is unique and not
/// null): `.invalid` never resolves, so no request reaches a catalogue through it.
pub fn synthetic_host(org: &str, path: &str) -> String {
    format!("{path}.{org}.portal.invalid")
}

/// The portal a slug address named, for [`crate::RequestOrigin`].
#[derive(Debug, Clone)]
pub(crate) struct PortalPath(pub String);

/// `app` behind the slug addresses: `/-/{path}/rest` of an Organization the router decided is
/// `/rest` of that portal, `308` to its own host when it redirects, and `404` when the
/// Organization has no such portal (the name is not echoed).
pub(crate) fn with_portal_slugs(app: Router, tables: PortalTables) -> Router {
    Router::new()
        .fallback_service(app)
        .layer(axum::middleware::from_fn_with_state(tables, slug_address))
}

async fn slug_address(
    State(tables): State<PortalTables>,
    mut req: Request,
    next: Next,
) -> Response {
    let org = req
        .extensions()
        .get::<jc_router::Organization>()
        .map(|o| o.0.clone());
    let (Some(org), Some(rest)) = (org, req.uri().path().strip_prefix("/-/")) else {
        return next.run(req).await;
    };
    let (path, tail) = rest.find('/').map_or((rest, "/"), |i| rest.split_at(i));
    let Some(found) = tables.slug(&org, path) else {
        return problem_response(DataroomError::NotFound("no portal here".into()));
    };
    let query = req
        .uri()
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    if let Some(base) = &found.redirect {
        let to = format!("{}{tail}{query}", base.trim_end_matches('/'));
        return (StatusCode::PERMANENT_REDIRECT, [(header::LOCATION, to)]).into_response();
    }
    let Ok(uri) = format!("{tail}{query}").parse::<Uri>() else {
        return problem_response(DataroomError::NotFound("no portal here".into()));
    };
    let path = path.to_owned();
    *req.uri_mut() = uri;
    req.extensions_mut().insert(PortalPath(path));
    next.run(req).await
}

fn read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

fn write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}
