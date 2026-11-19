//! Automation logins (T-7432): a sign-in an agent or a script holds, such as the
//! `<org>-mcp-admin` behind an Organization's management connector (T-6858, T-7412). The realm
//! knows it as a person, so without a mark it would count as PF-58's second person: it could
//! approve what a person proposed, approve with one login what it proposed with another, and keep
//! an Organization from single-person approvals (T-7388).
//!
//! A login is marked by a row of `automation_logins` (migration 0091), which only an installation
//! operator writes, through the operator door (`AutomationLoginSet`); neither the Organization's
//! administrator, a mapper nor the login itself sets or clears it (IDN-13). A marked login may
//! propose; it never approves a Change of any lane, an access request or an erasure, and the
//! single-person rule counts people without it.

use std::collections::BTreeSet;

use crate::auth::session::Identity;
use crate::error::ApiError;
use crate::state::AppState;

/// One marked login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationLogin {
    /// The Organization whose operator door marked it, e.g. `bbsk`.
    pub organization: String,
    /// The login as the Organization's RoleBinding names it, lower case, e.g. `bbsk-mcp-admin`.
    pub login: String,
    /// The connector the login stands behind, when the operator named one.
    pub connector: Option<String>,
}

fn db(e: sqlx::Error) -> ApiError {
    tracing::error!(error = %e, "automation logins");
    ApiError::Unavailable(
        "the installation's automation logins could not be read; try again".into(),
    )
}

/// The marked logins of `organization`, or of every Organization; none without a database.
pub async fn marked(
    state: &AppState,
    organization: Option<&str>,
) -> Result<Vec<AutomationLogin>, ApiError> {
    let Some(pool) = state.db.as_ref() else {
        return Ok(Vec::new());
    };
    let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT organization, login, connector FROM automation_logins \
         WHERE $1::text IS NULL OR organization = $1 ORDER BY organization, login",
    )
    .bind(organization)
    .fetch_all(pool)
    .await
    .map_err(db)?;
    Ok(rows
        .into_iter()
        .map(|(organization, login, connector)| AutomationLogin {
            organization,
            login,
            connector,
        })
        .collect())
}

/// Whether `login` names the person of `identity`: their user id, username or verified e-mail,
/// any case.
fn names(identity: &Identity, login: &str) -> bool {
    [
        Some(identity.subject.as_str()),
        Some(identity.username.as_str()),
        identity.email.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|name| name.trim().eq_ignore_ascii_case(login))
}

/// The marked login `identity` signs in as, in any Organization: a login marked in one approves
/// nowhere.
pub async fn login_of(
    state: &AppState,
    identity: &Identity,
) -> Result<Option<AutomationLogin>, ApiError> {
    Ok(marked(state, None)
        .await?
        .into_iter()
        .find(|row| names(identity, &row.login)))
}

/// `403` naming the rule when `identity` is an automation login: it may propose `what`, never
/// approve one (T-7432, AG-11, PF-58). A database that cannot answer refuses too.
pub async fn refuse_approver(
    state: &AppState,
    identity: &Identity,
    what: &str,
) -> Result<(), ApiError> {
    let Some(found) = login_of(state, identity).await? else {
        return Ok(());
    };
    tracing::warn!(target: "audit", login = %found.login, organization = %found.organization, what, "refused: an automation login may not approve");
    let connector = found
        .connector
        .as_deref()
        .map(|c| format!(" (connector {c})"))
        .unwrap_or_default();
    Err(ApiError::Denied(format!(
        "'{}' is an automation login of '{}'{connector}: it may propose {what}, never approve one; a person approves it in the Portal (T-7432, AG-11, PF-58)",
        found.login, found.organization
    )))
}

/// `people` (lower-case identifiers, as a RoleBinding names them) without the automation logins
/// marked in `organization`.
pub async fn humans(
    state: &AppState,
    organization: &str,
    people: BTreeSet<String>,
) -> Result<BTreeSet<String>, ApiError> {
    let logins: BTreeSet<String> = marked(state, Some(organization))
        .await?
        .into_iter()
        .map(|row| row.login)
        .collect();
    Ok(people
        .into_iter()
        .filter(|person| !logins.contains(&person.to_ascii_lowercase()))
        .collect())
}

/// Marks `login` of `organization` (`connector` the one it stands behind) or, `on` false, clears
/// the mark: an applied `AutomationLoginSet`. Whether a row was there before.
pub async fn set(
    conn: &mut sqlx::PgConnection,
    organization: &str,
    login: &str,
    connector: Option<&str>,
    on: bool,
    by: &str,
) -> Result<bool, sqlx::Error> {
    let before: Option<(String,)> = sqlx::query_as(
        "SELECT login FROM automation_logins WHERE organization = $1 AND login = $2 FOR UPDATE",
    )
    .bind(organization)
    .bind(login)
    .fetch_optional(&mut *conn)
    .await?;
    if on {
        sqlx::query(
            "INSERT INTO automation_logins (organization, login, connector, marked_by) \
             VALUES ($1, $2, $3, $4) ON CONFLICT (organization, login) DO UPDATE SET \
             connector = EXCLUDED.connector, marked_by = EXCLUDED.marked_by, marked_at = now()",
        )
        .bind(organization)
        .bind(login)
        .bind(connector)
        .bind(by)
        .execute(&mut *conn)
        .await?;
    } else {
        sqlx::query("DELETE FROM automation_logins WHERE organization = $1 AND login = $2")
            .bind(organization)
            .bind(login)
            .execute(&mut *conn)
            .await?;
    }
    Ok(before.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_login_names_its_person_by_any_identifier_any_case() {
        let who = Identity {
            subject: "01a1f497-9f7a-4c00-8000-000000000001".into(),
            username: "BBSK-mcp-admin".into(),
            email: Some("mcp@bbsk.sk".into()),
            name: None,
            roles: vec![],
            groups: vec![],
            client: None,
            second_factor: false,
        };
        for login in [
            "bbsk-mcp-admin",
            "mcp@bbsk.sk",
            "01a1f497-9f7a-4c00-8000-000000000001",
        ] {
            assert!(names(&who, login), "{login}");
        }
        assert!(!names(&who, "bbsk-owner"));
    }
}
