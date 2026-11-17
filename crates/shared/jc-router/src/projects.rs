//! Project id addresses (ARCH/RTE-05, T-7194): `/{org}/{ProjectId}/api/v1/X` is placed as
//! `/{org}/api/v1/projects/{name}/X` of the live project the id names in that Organization, the
//! project MCP door `/{org}/{ProjectId}/api/mcp` as `/{org}/api/v1/projects/{name}/mcp` (MCP-33),
//! and
//! every refusal on such an address is one 404 with the same bytes, so an answer tells no one
//! whether an id exists or whether they hold a grant on it. The door's protected-resource
//! metadata and its challenge to a caller without credentials (MCP-33, T-7211) answer every
//! well-formed id alike, so an MCP client can start OAuth on the id address and still learns
//! nothing of the id.
//!
//! A branch project (TEN-12, CHG-15; T-7212) answers only on its id address: the router places it
//! under its name `b-{ProjectId}`, refuses its id once its `expires_at` has passed, and refuses
//! every request that names a branch anywhere else in its path or query, so a name never
//! resolves to a branch (RTE-05). The placed id goes to the module as [`IdAddress`], a request
//! extension no client can send.

use std::collections::BTreeMap;

use axum::body::Body;
use axum::http::{Response, StatusCode, header};

/// One project of the registry (`project_ids`) as the router reads it: a live project, or a
/// branch with its expiry (TEN-12, CHG-15). A copy has no id address until it has one of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistryProject<'a> {
    /// The Organization's name, what [`crate::Organization`] carries.
    pub organization: &'a str,
    /// The `ProjectId` in its 26-character path form (TEN-01).
    pub id: &'a str,
    /// The project's name, what `/api/v1/projects/{name}/` reads; a branch's is `b-{id}`.
    pub name: &'a str,
    /// When a branch stops answering, in Unix seconds (CHG-15); `None` for a live project.
    pub expires_at: Option<i64>,
}

/// The project id an id address placed the request under (RTE-05), handed to the module as a
/// request extension: a client can send a header, never an extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdAddress(pub String);

/// The live projects of each Organization by id, swapped in whole by the registry's watcher.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectTable {
    names: BTreeMap<(String, String), (String, Option<i64>)>,
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
                    (r.name.to_owned(), r.expires_at),
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
        let Some((name, expires_at)) = self.names.get(&(organization.to_owned(), id.to_owned()))
        else {
            return Some(Err(()));
        };
        // An expired branch is refused at request time, not at the next registry read (CHG-15).
        if expires_at.is_some_and(|at| at <= unix_now()) {
            return Some(Err(()));
        }
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

/// Now in Unix seconds; a clock before 1970 reads as 0, so no branch outlives its expiry.
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Whether `name` is a branch's name, `b-` and a `ProjectId` in the path form: a name no person
/// can choose (RTE-05, T-7212).
fn branch_name(name: &str) -> bool {
    name.strip_prefix("b-").is_some_and(path_form)
}

/// Whether `path` or `query` names a branch other than `placed` (the branch an id address placed
/// the request under, if any): in any path segment or any query value, read percent-decoded and
/// in lower case, so no spelling of a branch name passes. Such a request is refused with
/// [`not_found`]: a name never resolves to a branch, and an id address never reaches a second one
/// (RTE-05).
pub(crate) fn names_other_branch(path: &str, query: Option<&str>, placed: Option<&str>) -> bool {
    let values = query
        .into_iter()
        .flat_map(|q| q.split('&'))
        .flat_map(|pair| pair.split('='));
    path.split('/').chain(values).any(|token| {
        let token = decoded(token).to_ascii_lowercase();
        branch_name(&token) && placed != Some(token.as_str())
    })
}

/// `token` with each `%XX` and `+` decoded; an invalid escape is kept as it is.
fn decoded(token: &str) -> String {
    let bytes = token.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b'+', _) => {
                out.push(b' ');
                i += 1;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
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
    const LIVE_BRANCH: &str = "daaaaaaaaaaaaaaaaaaaaaaaaa";
    const EXPIRED_BRANCH: &str = "eaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn table() -> ProjectTable {
        ProjectTable::new(&[
            RegistryProject {
                organization: "bratislava",
                id: ID,
                name: "traffic",
                expires_at: None,
            },
            RegistryProject {
                organization: "kosice",
                id: "baaaaaaaaaaaaaaaaaaaaaaaaa",
                name: "air",
                expires_at: None,
            },
            RegistryProject {
                organization: "bratislava",
                id: "AAAAAAAAAAAAAAAAAAAAAAAAAA",
                name: "shouted",
                expires_at: None,
            },
            RegistryProject {
                organization: "bratislava",
                id: LIVE_BRANCH,
                name: "b-daaaaaaaaaaaaaaaaaaaaaaaaa",
                expires_at: Some(i64::MAX),
            },
            RegistryProject {
                organization: "bratislava",
                id: EXPIRED_BRANCH,
                name: "b-eaaaaaaaaaaaaaaaaaaaaaaaaa",
                expires_at: Some(1),
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

    /// CHG-15, RTE-05 (T-7212): a live branch answers on its id address under its own name; an
    /// expired one is refused at request time with the same answer as an unknown id, even
    /// though the registry still lists it.
    #[test]
    fn expired_or_tombstoned_branch_id_is_refused_like_an_unknown_one() {
        let t = table();
        assert_eq!(
            t.place("bratislava", &format!("/{LIVE_BRANCH}/api/v1/spaces"), true),
            Some(Ok(IdPlace::Named(
                "/api/v1/projects/b-daaaaaaaaaaaaaaaaaaaaaaaaa/spaces".to_owned()
            )))
        );
        let unknown = t.place(
            "bratislava",
            "/caaaaaaaaaaaaaaaaaaaaaaaaa/api/v1/spaces",
            true,
        );
        assert_eq!(unknown, Some(Err(())));
        for path in [
            format!("/{EXPIRED_BRANCH}/api/v1/spaces"),
            format!("/{EXPIRED_BRANCH}/api/mcp"),
        ] {
            assert_eq!(t.place("bratislava", &path, true), unknown, "{path}");
        }
        // Another Organization never reaches the branch.
        assert_eq!(
            t.place("kosice", &format!("/{LIVE_BRANCH}/api/v1/spaces"), true),
            unknown
        );
    }

    /// RTE-05 (T-7212): a branch's name in any path segment or query value, in any spelling, is
    /// refused unless it is the branch the id address placed; ordinary names pass.
    #[test]
    fn a_name_never_resolves_to_a_branch() {
        let branch = "b-daaaaaaaaaaaaaaaaaaaaaaaaa";
        for (path, query) in [
            (format!("/api/v1/projects/{branch}/spaces"), None),
            (format!("/api/v1/projects/{branch}"), None),
            (
                "/api/v1/projects/B-DAAAAAAAAAAAAAAAAAAAAAAAAA/mcp".to_owned(),
                None,
            ),
            (
                "/api/v1/projects/b%2Ddaaaaaaaaaaaaaaaaaaaaaaaaa/x".to_owned(),
                None,
            ),
            (format!("/c/{branch}/x"), None),
            ("/api/v1/mcp".to_owned(), Some(format!("project={branch}"))),
            (
                "/api/v1/x".to_owned(),
                Some("a=1&project=b%2ddaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned()),
            ),
        ] {
            assert!(
                names_other_branch(&path, query.as_deref(), None),
                "{path} {query:?}"
            );
        }
        // The placed branch passes; a second branch beside it does not.
        assert!(!names_other_branch(
            &format!("/api/v1/projects/{branch}/spaces"),
            None,
            Some(branch)
        ));
        assert!(names_other_branch(
            &format!("/api/v1/projects/{branch}/spaces"),
            Some("project=b-eaaaaaaaaaaaaaaaaaaaaaaaaa"),
            Some(branch)
        ));
        for (path, query) in [
            ("/api/v1/projects/traffic/spaces", Some("project=b-short")),
            ("/api/v1/projects/b-iaaaaaaaaaaaaaaaaaaaaaaaaa", None),
            ("/api/v1/projects/b-daaaaaaaaaaaaaaaaaaaaaaaaaa", None),
            ("/", None),
        ] {
            assert!(!names_other_branch(path, query, None), "{path} {query:?}");
        }
    }
}
