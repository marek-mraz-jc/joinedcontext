//! Project id addresses (ARCH/RTE-05, T-7194): `/{org}/{ProjectId}/api/v1/X` is placed as
//! `/{org}/api/v1/projects/{name}/X` of the live project the id names in that Organization, and
//! every refusal on such an address is one 404 with the same bytes, so an answer tells no one
//! whether an id exists or whether they hold a grant on it.

use std::collections::BTreeMap;

use axum::body::Body;
use axum::http::{Response, StatusCode, header};

/// One live project of the registry (`project_ids`, a row without a parent) as the router reads
/// it: a branch or copy has no id address until it has one of its own (TEN-12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistryProject<'a> {
    /// The Organization's name, what [`crate::Organization`] carries.
    pub organization: &'a str,
    /// The `ProjectId` in its 26-character path form (TEN-01).
    pub id: &'a str,
    /// The project's name, what `/api/v1/projects/{name}/` reads.
    pub name: &'a str,
}

/// The live projects of each Organization by id, swapped in whole by the registry's watcher.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectTable {
    names: BTreeMap<(String, String), String>,
}

/// The one body of every refusal on an id address (RTE-05, body P).
const NOT_FOUND: &str = r#"{"type":"about:blank","title":"Not Found","status":404,"detail":"No project answers at this address for you. Check the project id, or ask the project's owner for access."}"#;

impl ProjectTable {
    /// The table of `rows`; a row whose id is not in the path form is left out, never repaired.
    #[must_use]
    pub fn new(rows: &[RegistryProject<'_>]) -> Self {
        let names = rows
            .iter()
            .filter(|r| path_form(r.id))
            .map(|r| {
                (
                    (r.organization.to_owned(), r.id.to_owned()),
                    r.name.to_owned(),
                )
            })
            .collect();
        Self { names }
    }

    /// Where `path` (the Organization's segment removed) goes for `organization`: `None` when its
    /// first segment is no id in the path form (the path is routed as it is), `Some(Err(()))`
    /// when it is one this Organization holds no live project under or the rest is no
    /// management address, else the path under the project's name.
    pub(crate) fn place(&self, organization: &str, path: &str) -> Option<Result<String, ()>> {
        let rest = path.strip_prefix('/')?;
        let (id, rest) = rest.split_once('/').unwrap_or((rest, ""));
        if !path_form(id) {
            // Another spelling of an id (upper case, a digit outside base32) is refused alike
            // rather than routed by name, so no spelling of an id answers differently.
            return (id.len() == 26 && id.bytes().all(|b| b.is_ascii_alphanumeric()))
                .then_some(Err(()));
        }
        let Some(name) = self.names.get(&(organization.to_owned(), id.to_owned())) else {
            return Some(Err(()));
        };
        let under = match rest.strip_prefix("api/v1") {
            Some("") => String::new(),
            Some(under) if under.starts_with('/') => under.to_owned(),
            _ => return Some(Err(())),
        };
        Some(Ok(format!("/api/v1/projects/{name}{under}")))
    }
}

/// Whether `id` is a `ProjectId` in the path form: 26 characters of lower-case base32 starting
/// `a`..`h` (a UUID's 128 bits, TEN-01).
fn path_form(id: &str) -> bool {
    id.len() == 26
        && (b'a'..=b'h').contains(&id.as_bytes()[0])
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
}

/// The answer to every refusal on an id address: nothing of the module's answer is kept, so no
/// `WWW-Authenticate`, cookie or length tells one case from another.
pub(crate) fn not_found() -> Response<Body> {
    let mut resp = Response::new(Body::from(NOT_FOUND));
    *resp.status_mut() = StatusCode::NOT_FOUND;
    let headers = resp.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/problem+json"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    resp
}

/// Whether the module's answer on an id address is one [`not_found`] replaces: a refusal (401,
/// 403, 404, 405) or a redirect, whose `Location` would name the project by its name.
pub(crate) fn refused(status: StatusCode) -> bool {
    status.is_redirection()
        || matches!(
            status,
            StatusCode::UNAUTHORIZED
                | StatusCode::FORBIDDEN
                | StatusCode::NOT_FOUND
                | StatusCode::METHOD_NOT_ALLOWED
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn table() -> ProjectTable {
        ProjectTable::new(&[
            RegistryProject {
                organization: "bratislava",
                id: ID,
                name: "traffic",
            },
            RegistryProject {
                organization: "kosice",
                id: "baaaaaaaaaaaaaaaaaaaaaaaaa",
                name: "air",
            },
            RegistryProject {
                organization: "bratislava",
                id: "AAAAAAAAAAAAAAAAAAAAAAAAAA",
                name: "shouted",
            },
        ])
    }

    /// RTE-05: an id of this Organization goes to its name; another Organization's id, an
    /// unknown one and a path beside the management API are refused alike; anything not in the
    /// path form is routed as it is.
    #[test]
    fn an_id_places_its_own_project_and_nothing_else() {
        let t = table();
        assert_eq!(
            t.place("bratislava", &format!("/{ID}/api/v1/spaces?x")),
            Some(Ok("/api/v1/projects/traffic/spaces?x".to_owned()))
        );
        assert_eq!(
            t.place("bratislava", &format!("/{ID}/api/v1")),
            Some(Ok("/api/v1/projects/traffic".to_owned()))
        );
        for refused in [
            "/baaaaaaaaaaaaaaaaaaaaaaaaa/api/v1/spaces",
            "/caaaaaaaaaaaaaaaaaaaaaaaaa/api/v1/spaces",
            &format!("/{ID}/api/v1x"),
            &format!("/{ID}/api/v2/spaces"),
            &format!("/{ID}"),
            &format!("/{ID}/"),
        ] {
            assert_eq!(t.place("bratislava", refused), Some(Err(())), "{refused}");
        }
        for spelled in [
            "/AAAAAAAAAAAAAAAAAAAAAAAAAA/api/v1",
            "/Aaaaaaaaaaaaaaaaaaaaaaaaaa/api/v1",
            "/iaaaaaaaaaaaaaaaaaaaaaaaaa/api/v1",
            "/aaaaaaaaaaaaaaaaaaaaaaaaa1/api/v1",
        ] {
            assert_eq!(t.place("bratislava", spelled), Some(Err(())), "{spelled}");
        }
        for plain in [
            "/api/v1/projects/traffic",
            "/aaaaaaaaaaaaaaaaaaaaaaaaa/api/v1",
            "/%61aaaaaaaaaaaaaaaaaaaaaaaaa/api/v1",
            "/",
            "",
        ] {
            assert_eq!(t.place("bratislava", plain), None, "{plain}");
        }
    }
}
