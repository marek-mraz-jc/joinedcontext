//! Project id addresses (ARCH/RTE-05, T-7194): `/{org}/{ProjectId}/api/v1/X` is placed as
//! `/{org}/api/v1/projects/{name}/X` of the live project the id names in that Organization, the
//! project MCP door `/{org}/{ProjectId}/api/mcp` as `/{org}/api/v1/projects/{name}/mcp` (MCP-33),
//! and
//! every refusal on such an address is one 404 with the same bytes, so an answer tells no one
//! whether an id exists or whether they hold a grant on it. The door's protected-resource
//! metadata and its challenge to a caller without credentials (MCP-33, T-7211) answer every
//! well-formed id alike, so an MCP client can start OAuth on the id address and still learns
//! nothing of the id.

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

/// Where an id address goes (RTE-05, MCP-33).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IdPlace {
    /// The path under the project's name.
    Named(String),
    /// The project door's protected-resource metadata: the Portal's MCP metadata with the id
    /// address as its `resource`, for every well-formed id alike.
    Metadata,
    /// The project door asked without credentials: the challenge naming the metadata, for every
    /// well-formed id alike.
    Challenge,
}

/// The Portal's MCP metadata document, which [`IdPlace::Metadata`] answers with its own
/// `resource`.
pub(crate) const PORTAL_METADATA: &str = "/.well-known/oauth-protected-resource/api/v1/mcp";

/// The metadata of a door, as a suffix of its address (as the gateway's doors serve theirs).
const METADATA_SUFFIX: &str = "/.well-known/oauth-protected-resource";

/// The largest metadata document the router rewrites.
const METADATA_LIMIT: usize = 64 * 1024;

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
    /// management address, else its [`IdPlace`]. The door's metadata, and the door itself for a
    /// caller with no `credentials`, are placed before the id is looked up (MCP-33).
    pub(crate) fn place(
        &self,
        organization: &str,
        path: &str,
        credentials: bool,
    ) -> Option<Result<IdPlace, ()>> {
        let rest = path.strip_prefix('/')?;
        let (id, rest) = rest.split_once('/').unwrap_or((rest, ""));
        if !path_form(id) {
            // Another spelling of an id (upper case, a digit outside base32) is refused alike
            // rather than routed by name, so no spelling of an id answers differently.
            return (id.len() == 26 && id.bytes().all(|b| b.is_ascii_alphanumeric()))
                .then_some(Err(()));
        }
        if rest.strip_prefix("api/mcp") == Some(METADATA_SUFFIX) {
            return Some(Ok(IdPlace::Metadata));
        }
        if rest == "api/mcp" && !credentials {
            return Some(Ok(IdPlace::Challenge));
        }
        let Some(name) = self.names.get(&(organization.to_owned(), id.to_owned())) else {
            return Some(Err(()));
        };
        // The project MCP door (MCP-33) is the project's own MCP server.
        if rest == "api/mcp" {
            return Some(Ok(IdPlace::Named(format!("/api/v1/projects/{name}/mcp"))));
        }
        let under = match rest.strip_prefix("api/v1") {
            Some("") => String::new(),
            Some(under) if under.starts_with('/') => under.to_owned(),
            _ => return Some(Err(())),
        };
        Some(Ok(IdPlace::Named(format!(
            "/api/v1/projects/{name}{under}"
        ))))
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

/// The project door's address for `door_base` (`{scheme}://{host}/{org}/{ProjectId}`), the
/// `resource` its metadata names.
fn door(door_base: &str) -> String {
    format!("{door_base}/api/mcp")
}

/// The answer to the project door asked without credentials (MCP-33): 401 naming the door's
/// metadata, the same for every well-formed id but the id the caller sent.
pub(crate) fn challenge(door_base: &str) -> Response<Body> {
    let mut resp = Response::new(Body::from(
        r#"{"type":"about:blank","title":"Unauthorized","status":401,"detail":"Sign in: this MCP server takes an OAuth access token; its metadata names the authorization server."}"#,
    ));
    *resp.status_mut() = StatusCode::UNAUTHORIZED;
    let value = format!(
        "Bearer resource_metadata=\"{}{METADATA_SUFFIX}\", scope=\"mcp:portal\"",
        door(door_base)
    );
    let headers = resp.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/problem+json"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    if let Ok(value) = header::HeaderValue::from_str(&value) {
        headers.insert(header::WWW_AUTHENTICATE, value);
    }
    resp
}

/// The Portal's metadata answer `resp` with the project door as its `resource` (MCP-33); any
/// other answer, or one that is no JSON object, is [`not_found`].
pub(crate) async fn metadata(resp: Response<Body>, door_base: &str) -> Response<Body> {
    if resp.status() != StatusCode::OK {
        return not_found();
    }
    let Ok(bytes) = axum::body::to_bytes(resp.into_body(), METADATA_LIMIT).await else {
        return not_found();
    };
    let Ok(serde_json::Value::Object(mut doc)) = serde_json::from_slice(&bytes) else {
        return not_found();
    };
    doc.insert(
        "resource".to_owned(),
        serde_json::Value::String(door(door_base)),
    );
    let mut out = Response::new(Body::from(serde_json::Value::Object(doc).to_string()));
    out.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    out
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
            t.place("bratislava", &format!("/{ID}/api/v1/spaces?x"), true),
            Some(Ok(IdPlace::Named(
                "/api/v1/projects/traffic/spaces?x".to_owned()
            )))
        );
        assert_eq!(
            t.place("bratislava", &format!("/{ID}/api/v1"), true),
            Some(Ok(IdPlace::Named("/api/v1/projects/traffic".to_owned())))
        );
        assert_eq!(
            t.place("bratislava", &format!("/{ID}/api/mcp"), true),
            Some(Ok(IdPlace::Named(
                "/api/v1/projects/traffic/mcp".to_owned()
            )))
        );
        for refused in [
            "/baaaaaaaaaaaaaaaaaaaaaaaaa/api/v1/spaces",
            "/caaaaaaaaaaaaaaaaaaaaaaaaa/api/v1/spaces",
            &format!("/{ID}/api/v1x"),
            &format!("/{ID}/api/v2/spaces"),
            &format!("/{ID}/api/mcp/x"),
            "/baaaaaaaaaaaaaaaaaaaaaaaaa/api/mcp",
            &format!("/{ID}"),
            &format!("/{ID}/"),
        ] {
            assert_eq!(
                t.place("bratislava", refused, true),
                Some(Err(())),
                "{refused}"
            );
        }
        // MCP-33: the door's metadata, and the door without credentials, before the lookup.
        for id in [
            ID,
            "baaaaaaaaaaaaaaaaaaaaaaaaa",
            "caaaaaaaaaaaaaaaaaaaaaaaaa",
        ] {
            assert_eq!(
                t.place(
                    "bratislava",
                    &format!("/{id}/api/mcp/.well-known/oauth-protected-resource"),
                    false
                ),
                Some(Ok(IdPlace::Metadata))
            );
            assert_eq!(
                t.place("bratislava", &format!("/{id}/api/mcp"), false),
                Some(Ok(IdPlace::Challenge))
            );
        }
        assert_eq!(
            t.place("bratislava", "/AAAAAAAAAAAAAAAAAAAAAAAAAA/api/mcp", false),
            Some(Err(()))
        );
        for spelled in [
            "/AAAAAAAAAAAAAAAAAAAAAAAAAA/api/v1",
            "/Aaaaaaaaaaaaaaaaaaaaaaaaaa/api/v1",
            "/iaaaaaaaaaaaaaaaaaaaaaaaaa/api/v1",
            "/aaaaaaaaaaaaaaaaaaaaaaaaa1/api/v1",
        ] {
            assert_eq!(
                t.place("bratislava", spelled, true),
                Some(Err(())),
                "{spelled}"
            );
        }
        for plain in [
            "/api/v1/projects/traffic",
            "/aaaaaaaaaaaaaaaaaaaaaaaaa/api/v1",
            "/%61aaaaaaaaaaaaaaaaaaaaaaaaa/api/v1",
            "/",
            "",
        ] {
            assert_eq!(t.place("bratislava", plain, true), None, "{plain}");
        }
    }
}
