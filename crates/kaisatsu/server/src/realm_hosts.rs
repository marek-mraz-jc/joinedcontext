//! Realms pinned to a sign-in host of their own (T-7001, evidence/T-7001/design.md §2): an
//! Organization's realm answers on its `idm.` host alone, that host answers its realm alone, and
//! every issuer, link and cookie kaisatsu builds for it comes from this registry, never from the
//! request's `Host` or `X-Forwarded-Host`.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use jc_router::Mount;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Which realm answers on which sign-in host.
#[derive(Debug, Clone, Default)]
pub struct RealmHosts {
    by_realm: BTreeMap<String, Mount>,
    by_host: BTreeMap<String, String>,
    /// The hosts sibling modules call kaisatsu on (loopback): a pinned realm answers there its
    /// back channel alone ([`BACK_CHANNEL`]), under the registry's issuer.
    internal: Vec<String>,
}

/// What a resource server or a client fetches from a realm without a person at the browser.
const BACK_CHANNEL: [&str; 6] = [
    ".well-known/openid-configuration",
    "protocol/openid-connect/certs",
    "protocol/openid-connect/token",
    "protocol/openid-connect/token/introspect",
    "protocol/openid-connect/revoke",
    "protocol/openid-connect/userinfo",
];

/// What the registry decides for one request.
#[derive(Debug, PartialEq, Eq)]
enum Pin {
    /// Neither the host nor the realm is pinned: served as before.
    Free,
    /// The realm's own host: served under the registry's Mount.
    Own(Mount),
    /// A pinned realm on another host, or another realm on a pinned host.
    Refused,
}

fn bare(host: &str) -> String {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    match host.rsplit_once(':') {
        Some((h, port)) if port.bytes().all(|b| b.is_ascii_digit()) => h.to_owned(),
        _ => host,
    }
}

/// The realm a kaisatsu path names and the rest after it: `/realms/{r}`, `/admin/realms/{r}`,
/// `/api/v1/realms/{r}`, `/.well-known/{document}/realms/{r}`. `Some(("", ""))` for
/// `/admin/realms` (the list of every realm).
fn realm_of(path: &str) -> Option<(&str, &str)> {
    let path = path.trim_start_matches('/');
    let after = match path.split('/').next()? {
        "realms" => path,
        "admin" => path.strip_prefix("admin/")?,
        "api" => path.strip_prefix("api/v1/")?,
        ".well-known" => path.splitn(3, '/').nth(2)?,
        _ => return None,
    };
    let after = match after.strip_prefix("realms") {
        Some("") => return Some(("", "")),
        Some(rest) => rest.strip_prefix('/')?,
        None => return None,
    };
    Some(after.split_once('/').unwrap_or((after, "")))
}

impl RealmHosts {
    /// Pins `realm` to `mount` (its sign-in host, at the host's root). A host or realm pinned
    /// twice keeps the first pin; jc's configuration refuses that before it gets here.
    #[must_use]
    pub fn pin(mut self, realm: &str, mount: Mount) -> Self {
        let host = bare(&mount.host);
        if self.by_realm.contains_key(realm) || self.by_host.contains_key(&host) {
            return self;
        }
        self.by_host.insert(host, realm.to_owned());
        self.by_realm.insert(realm.to_owned(), mount);
        self
    }

    /// Names a host sibling modules reach kaisatsu on (`localhost:8080`).
    #[must_use]
    pub fn internal(mut self, host: &str) -> Self {
        self.internal.push(bare(host));
        self
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_realm.is_empty()
    }

    fn decide(&self, host: &str, path: &str) -> Pin {
        let named = realm_of(path);
        let realm = named.map(|(r, _)| r);
        // Realm names hold no `%`: an encoded segment could name a pinned realm past the
        // comparison, so it is refused wherever a realm is pinned.
        if realm.is_some_and(|r| r.contains('%')) {
            return Pin::Refused;
        }
        match self.by_host.get(&bare(host)) {
            Some(own) => match realm {
                Some(r) if r == own => Pin::Own(self.by_realm[own].clone()),
                None if path.starts_with("/resources/kaisatsu/") => Pin::Free,
                _ => Pin::Refused,
            },
            None => match named {
                Some((r, rest))
                    if self.internal.contains(&bare(host))
                        && BACK_CHANNEL.contains(&rest)
                        && self.by_realm.contains_key(r) =>
                {
                    Pin::Own(self.by_realm[r].clone())
                }
                Some((r, _)) if self.by_realm.keys().any(|p| p.eq_ignore_ascii_case(r)) => {
                    Pin::Refused
                }
                _ => Pin::Free,
            },
        }
    }
}

/// The layer: the request's Mount (set by jc-router, else `default`) names the host.
pub(crate) async fn pin_realm(
    State((hosts, default)): State<(Arc<RealmHosts>, Arc<Mount>)>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let host = req
        .extensions()
        .get::<Mount>()
        .map_or_else(|| default.host.clone(), |m| m.host.clone());
    match hosts.decide(&host, req.uri().path()) {
        Pin::Free => next.run(req).await,
        Pin::Own(mount) => {
            req.extensions_mut().insert(mount);
            next.run(req).await
        }
        Pin::Refused => (StatusCode::NOT_FOUND, "Realm not found").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts() -> RealmHosts {
        let m = |h: &str| Mount::new("kaisatsu", "https", h, "").unwrap();
        RealmHosts::default()
            .pin("bbsk", m("idm.bbsk.sk"))
            .pin("hel", m("idm.hel.fi"))
            .internal("localhost:8080")
    }

    #[test]
    fn a_pinned_realm_answers_on_its_own_host_alone() {
        let h = hosts();
        assert!(matches!(
            h.decide("IDM.bbsk.sk:443", "/realms/bbsk/.well-known/openid-configuration"),
            Pin::Own(m) if m.host == "idm.bbsk.sk"
        ));
        for (host, path) in [
            ("idm.hel.fi", "/realms/bbsk/protocol/openid-connect/auth"),
            ("login.city.sk", "/realms/bbsk"),
            ("login.city.sk", "/realms/BBSK"),
            ("login.city.sk", "/admin/realms/bbsk/users"),
            ("login.city.sk", "/api/v1/realms/bbsk/people"),
            ("login.city.sk", "/realms/bb%73k"),
            ("idm.bbsk.sk", "/realms/dev"),
            ("idm.bbsk.sk", "/admin/realms"),
            ("idm.bbsk.sk", "/api/v1/realms/hel/people"),
            ("idm.bbsk.sk", "/"),
            ("idm.bbsk.sk", "/realms/b%62sk"),
            (
                "login.city.sk",
                "/.well-known/openid-configuration/realms/bbsk",
            ),
            (
                "idm.bbsk.sk",
                "/.well-known/oauth-authorization-server/realms/dev",
            ),
            (
                "localhost:8080",
                "/realms/bbsk/protocol/openid-connect/auth",
            ),
            ("localhost:8080", "/realms/bbsk/login-actions/authenticate"),
            ("localhost:8080", "/admin/realms/bbsk/users"),
        ] {
            assert_eq!(h.decide(host, path), Pin::Refused, "{host}{path}");
        }
        for path in [
            "/realms/bbsk/.well-known/openid-configuration",
            "/realms/bbsk/protocol/openid-connect/token",
            "/realms/bbsk/protocol/openid-connect/certs",
        ] {
            assert!(
                matches!(h.decide("localhost:8080", path), Pin::Own(m) if m.host == "idm.bbsk.sk"),
                "{path}"
            );
        }
        assert!(matches!(
            h.decide(
                "idm.bbsk.sk",
                "/.well-known/openid-configuration/realms/bbsk"
            ),
            Pin::Own(_)
        ));
        for (host, path) in [
            (
                "login.city.sk",
                "/realms/dev/.well-known/openid-configuration",
            ),
            ("login.city.sk", "/admin/realms"),
            ("idm.bbsk.sk", "/resources/kaisatsu/1/login.css"),
        ] {
            assert_eq!(h.decide(host, path), Pin::Free, "{host}{path}");
        }
    }

    #[test]
    fn realm_of_reads_the_three_realm_path_forms() {
        assert_eq!(realm_of("/realms/a/x/y"), Some(("a", "x/y")));
        assert_eq!(realm_of("/realms/a"), Some(("a", "")));
        assert_eq!(realm_of("/admin/realms/a"), Some(("a", "")));
        assert_eq!(realm_of("/admin/realms"), Some(("", "")));
        assert_eq!(realm_of("/api/v1/realms/a/people"), Some(("a", "people")));
        assert_eq!(
            realm_of("/.well-known/openid-configuration/realms/a"),
            Some(("a", ""))
        );
        assert_eq!(realm_of("/api/v2/realms/a"), None);
        assert_eq!(realm_of("/admin/realmsx"), None);
        assert_eq!(realm_of("/resources/kaisatsu/x"), None);
    }
}
