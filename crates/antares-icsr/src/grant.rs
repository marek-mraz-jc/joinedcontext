// SPDX-License-Identifier: EUPL-1.2
//! Export grants (ADR-0025 rule 5): the target Tenant's consent to a
//! source Tenant's registrations reading it in process, written through
//! `/q/grants` and read again on every forward, so a removal stops data
//! at once.

use antares_ops::{ngsi_problem, Ops};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::{Deserialize, Serialize};

/// The largest grant document `/q/grants` accepts, in bytes.
pub const MAX_GRANT_BYTES: usize = 4096;

/// What a grant lets the source do in the target.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Access {
    /// Reads, queries and Subscriptions (threat C6: the default).
    #[default]
    Read,
    /// Reads and the writes of 5.6 too.
    ReadWrite,
}

/// One grant, as written to `/q/grants/{target}/{source}`.
///
/// ```
/// let grant = antares_icsr::ExportGrant::parse(br#"{"target": "b", "source": "a"}"#)
///     .expect("a grant");
/// assert_eq!(grant.access, antares_icsr::Access::Read);
/// assert!(antares_icsr::ExportGrant::parse(br#"{"target": "antares-views", "source": "a"}"#).is_err());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExportGrant {
    /// The Tenant that holds the grant and whose data is read.
    pub target: String,
    /// The Tenant whose registrations may read it.
    pub source: String,
    /// A view of the target narrowing what it exports (ADR-0023); the view
    /// must serve the target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
    /// Read unless it says read-write (C6).
    #[serde(default)]
    pub access: Access,
}

impl ExportGrant {
    /// A grant document, checked.
    ///
    /// # Errors
    /// When it is no grant, names a Tenant no client can (an internal one is
    /// never a target, C8), or names no view a table can hold.
    pub fn parse(body: &[u8]) -> Result<Self, String> {
        let grant: Self = serde_json::from_slice(body).map_err(|e| format!("not a grant: {e}"))?;
        for tenant in [&grant.target, &grant.source] {
            client_tenant(tenant)?;
        }
        if let Some(view) = &grant.view {
            let fine = (1..=63).contains(&view.len())
                && view
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
            if !fine {
                return Err(format!("{view:?} is not a view id"));
            }
        }
        Ok(grant)
    }

    /// Whether it lets the source run a forward that writes when `write`.
    #[must_use]
    pub fn allows(&self, write: bool) -> bool {
        !write || self.access == Access::ReadWrite
    }
}

/// `raw` as a Tenant a client can name, or the reason it is not one: the
/// broker's own Tenants (ADR-0012) are never a party to a grant (C8).
fn client_tenant(raw: &str) -> Result<(), String> {
    antares_ops::TenantId::new(raw)
        .map(|_| ())
        .map_err(|_| format!("{raw:?} names no tenant a client can"))
}

/// The store key of the grant of `target` to `source`; `~` is in no Tenant
/// name, so no two pairs share one.
fn key(target: &str, source: &str) -> String {
    format!("{target}~{source}")
}

/// The grant `target` holds for `source`, if a valid one is stored.
///
/// # Errors
/// The store's.
pub async fn held(ops: &Ops, target: &str, source: &str) -> Result<Option<ExportGrant>, String> {
    if client_tenant(target).is_err() || client_tenant(source).is_err() {
        return Ok(None);
    }
    let Some(doc) = ops.grant_doc(&key(target, source)).await? else {
        return Ok(None);
    };
    match serde_json::from_value::<ExportGrant>(doc) {
        Ok(g) if g.target == target && g.source == source => Ok(Some(g)),
        _ => {
            tracing::warn!(
                target_tenant = target,
                source_tenant = source,
                "a stored export grant does not read as one; it grants nothing"
            );
            Ok(None)
        }
    }
}

/// `/q/grants` (every grant) and `/q/grants/{target}/{source}` (GET, PUT,
/// DELETE), relative to `/q`.
pub fn routes(ops: &Ops) -> Router {
    let (list, one, put, del) = (ops.clone(), ops.clone(), ops.clone(), ops.clone());
    Router::new()
        .route(
            "/grants",
            get(move || async move {
                answer(
                    list.grant_docs()
                        .await
                        .map(|d| axum::Json(d).into_response()),
                )
            }),
        )
        .route(
            "/grants/{target}/{source}",
            get(move |Path((t, s)): Path<(String, String)>| async move {
                answer(get_one(&one, &t, &s).await)
            })
            .put(
                move |Path((t, s)): Path<(String, String)>, body: Bytes| async move {
                    answer(put_one(&put, &t, &s, &body).await)
                },
            )
            .delete(move |Path((t, s)): Path<(String, String)>| async move {
                answer(delete_one(&del, &t, &s).await)
            })
            .layer(DefaultBodyLimit::max(MAX_GRANT_BYTES)),
        )
}

async fn get_one(ops: &Ops, target: &str, source: &str) -> Result<Response, String> {
    Ok(match held(ops, target, source).await? {
        Some(g) => axum::Json(g).into_response(),
        None => not_found(),
    })
}

async fn put_one(ops: &Ops, target: &str, source: &str, body: &[u8]) -> Result<Response, String> {
    let grant = match ExportGrant::parse(body) {
        Ok(g) if g.target == target && g.source == source => g,
        Ok(_) => {
            return Ok(ngsi_problem(
                StatusCode::BAD_REQUEST,
                "the document names another target or source than the path",
            ))
        }
        Err(why) => return Ok(ngsi_problem(StatusCode::BAD_REQUEST, &why)),
    };
    let doc = serde_json::to_value(&grant).map_err(|e| e.to_string())?;
    let replaced = ops.put_grant_doc(&key(target, source), doc).await?;
    tracing::info!(
        target_tenant = target,
        source_tenant = source,
        access = ?grant.access,
        view = grant.view.as_deref().unwrap_or(""),
        "export grant written"
    );
    Ok(if replaced {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::CREATED
    }
    .into_response())
}

async fn delete_one(ops: &Ops, target: &str, source: &str) -> Result<Response, String> {
    if client_tenant(target).is_err()
        || client_tenant(source).is_err()
        || !ops.delete_grant_doc(&key(target, source)).await?
    {
        return Ok(not_found());
    }
    tracing::info!(
        target_tenant = target,
        source_tenant = source,
        "export grant removed"
    );
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// The response, or a 500 that names no store detail (it goes to the log).
fn answer(result: Result<Response, String>) -> Response {
    result.unwrap_or_else(|e| {
        tracing::error!(error = e.as_str(), "export grants: the store failed");
        ngsi_problem(StatusCode::INTERNAL_SERVER_ERROR, "the grant store failed")
    })
}

fn not_found() -> Response {
    ngsi_problem(StatusCode::NOT_FOUND, "no such grant")
}

#[cfg(test)]
mod tests {
    use super::{Access, ExportGrant};

    #[test]
    fn a_grant_reads_only_unless_it_says_read_write() {
        let read = ExportGrant::parse(br#"{"target":"b","source":"a","view":"b-1"}"#);
        assert_eq!(
            read.map(|g| (g.allows(false), g.allows(true))),
            Ok((true, false))
        );
        let rw = ExportGrant::parse(br#"{"target":"b","source":"a","access":"read-write"}"#);
        assert_eq!(
            rw.map(|g| (g.access, g.allows(true))),
            Ok((Access::ReadWrite, true))
        );
    }

    #[test]
    fn a_grant_naming_no_client_tenant_or_view_or_an_unknown_member_is_refused() {
        for bad in [
            r#"{"target":"snap-1","source":"a"}"#,
            r#"{"target":"b","source":"antares-grants"}"#,
            r#"{"target":"b c","source":"a"}"#,
            r#"{"target":"","source":"a"}"#,
            r#"{"target":"b","source":"a","view":"Cars"}"#,
            r#"{"target":"b","source":"a","view":""}"#,
            r#"{"target":"b","source":"a","access":"write"}"#,
            r#"{"target":"b","source":"a","subject":"alice"}"#,
            r#"{"target":"b"}"#,
            "[]",
        ] {
            assert!(ExportGrant::parse(bad.as_bytes()).is_err(), "{bad}");
        }
        let long = format!(
            r#"{{"target":"b","source":"a","view":"{}"}}"#,
            "v".repeat(64)
        );
        assert!(ExportGrant::parse(long.as_bytes()).is_err());
    }

    #[test]
    fn no_internal_tenant_is_a_party() {
        for internal in ["antares-grants", "antares-views", "distsub-index", "snap-1"] {
            for doc in [
                format!(r#"{{"target":"{internal}","source":"a"}}"#),
                format!(r#"{{"target":"b","source":"{internal}"}}"#),
            ] {
                assert!(ExportGrant::parse(doc.as_bytes()).is_err(), "{doc}");
            }
        }
    }
}
