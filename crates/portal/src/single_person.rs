//! Single-person approvals (T-7388): in an Organization where one person holds every role, PF-58's
//! second person does not exist, so that person's access Changes could never be approved. An
//! installation operator may turn `approvals.singlePerson` on for such an Organization through
//! the operator door (`SinglePersonSet`); then its one person approves such a Change alone, as a
//! person in the Portal and only after a fresh second factor (a one-time code at most
//! [`FRESH_SECONDS`] ago in this session). Agents, MCP and connectors never do (AG-11, EP-142).
//!
//! The mode is off by default and off for every Organization with two or more people: the moment
//! a second person holds a role, the next read turns it off and records why, and the approval
//! that found it says so.

use serde::Serialize;
use utoipa::ToSchema;

use crate::auth::session::Identity;
use crate::error::ApiError;
use crate::state::AppState;

/// How old the second factor of an approval in single-person mode may be, in seconds.
pub const FRESH_SECONDS: i64 = 300;
/// The clock skew a token's `auth_time` may run ahead of the Portal's clock.
const SKEW_SECONDS: i64 = 60;
/// What the Change, the audit line and the activity feed record as the approver's standing.
pub const APPROVED_BY: &str = "self (single-person mode)";

/// The mode of one Organization as its Administration page and an approval show it.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalMode {
    /// The Organization's name, e.g. `marek-mraz`.
    pub organization: String,
    /// Whether its one person approves their own access Changes after a fresh second factor.
    pub single_person: bool,
    /// How many people hold a role in the Organization now.
    pub people_with_a_role: usize,
    /// When an operator or the Portal last changed the mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub changed_at: Option<jc_time::TimestampTz>,
    /// Why the Portal turned the mode off by itself, e.g. a second person holds a role.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub off_reason: Option<String>,
}

/// Why single-person mode does not let this approval through.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The mode is off for the Organization: PF-58 holds as everywhere.
    Off,
    /// An agent, MCP or a connector approves, never in this mode.
    NotAPerson,
    /// No second factor in this session, or one older than [`FRESH_SECONDS`].
    NotFresh,
    /// Other people hold a role in the Organization.
    OtherPeople(usize),
}

/// The rule alone, for an Organization whose mode is `on`: a person at the Portal (`person`)
/// whose session proved a second factor at `stepped_up_at` (unix seconds, the token's
/// `auth_time`) no more than [`FRESH_SECONDS`] before `now`, with `others` other people holding a
/// role in the Organization.
pub fn verdict(
    on: bool,
    person: bool,
    stepped_up_at: Option<i64>,
    now: i64,
    others: usize,
) -> Result<(), Refusal> {
    if !on {
        return Err(Refusal::Off);
    }
    if !person {
        return Err(Refusal::NotAPerson);
    }
    if others > 0 {
        return Err(Refusal::OtherPeople(others));
    }
    match stepped_up_at {
        Some(at) if at <= now + SKEW_SECONDS && now - at <= FRESH_SECONDS => Ok(()),
        _ => Err(Refusal::NotFresh),
    }
}

/// The people holding a role in `organization` other than `identity`, matched by user id,
/// e-mail or username, any case.
pub fn others(state: &AppState, organization: &str, identity: &Identity) -> usize {
    let me = [
        Some(identity.subject.as_str()),
        identity.email.as_deref(),
        Some(identity.username.as_str()),
    ];
    crate::permissions::people_with_a_role(&state.mirror, organization, jc_time::TimestampTz::now())
        .iter()
        .filter(|person| {
            !me.iter()
                .flatten()
                .any(|mine| mine.trim().eq_ignore_ascii_case(person))
        })
        .count()
}

fn db(e: sqlx::Error) -> ApiError {
    tracing::error!(error = %e, "single-person approvals");
    ApiError::Unavailable("the Organization's approval mode could not be read; try again".into())
}

/// The stored row of `organization`: whether the mode is on, when it changed (unix seconds) and
/// the Portal's reason when it turned it off.
async fn stored(
    pool: &sqlx::PgPool,
    organization: &str,
) -> Result<Option<(bool, i64, Option<String>)>, ApiError> {
    sqlx::query_as(
        "SELECT single_person, extract(epoch FROM changed_at)::bigint, off_reason \
         FROM organization_approval_modes WHERE organization = $1",
    )
    .bind(organization)
    .fetch_optional(pool)
    .await
    .map_err(db)
}

/// Writes the mode of `organization`: an operator's applied `SinglePersonSet` (`off_reason`
/// `None`), or the Portal turning it off by itself.
pub async fn set(
    conn: &mut sqlx::PgConnection,
    organization: &str,
    on: bool,
    by: &str,
    off_reason: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO organization_approval_modes (organization, single_person, changed_by, off_reason) \
         VALUES ($1, $2, $3, $4) ON CONFLICT (organization) DO UPDATE SET \
         single_person = EXCLUDED.single_person, changed_by = EXCLUDED.changed_by, \
         changed_at = now(), off_reason = EXCLUDED.off_reason",
    )
    .bind(organization)
    .bind(on)
    .bind(by)
    .bind(off_reason)
    .execute(conn)
    .await
    .map(drop)
}

/// The reason the Portal records when a second person turns the mode off.
fn second_person_reason(people: usize) -> String {
    format!(
        "{people} people hold a role in the Organization now, and single-person approvals need exactly one. A second person approves access changes again."
    )
}

/// The mode of `organization` now. A mode found on while more than one person holds a role is
/// turned off here, with its reason and an audit line, before it is answered.
pub async fn mode(state: &AppState, organization: &str) -> Result<ApprovalMode, ApiError> {
    let people = crate::permissions::people_with_a_role(
        &state.mirror,
        organization,
        jc_time::TimestampTz::now(),
    )
    .len();
    let row = match state.db.as_ref() {
        Some(pool) => stored(pool, organization).await?,
        None => None,
    };
    let (mut on, mut changed, mut off_reason) = row.unwrap_or((false, 0, None));
    if on && people > 1 {
        let reason = second_person_reason(people);
        if let Some(pool) = state.db.as_ref() {
            let mut conn = pool.acquire().await.map_err(db)?;
            set(&mut conn, organization, false, "portal", Some(&reason))
                .await
                .map_err(db)?;
        }
        tracing::warn!(target: "audit", organization, people, "single-person approvals turned off: a second person holds a role");
        on = false;
        changed = crate::auth::session::now_unix();
        off_reason = Some(reason);
    }
    Ok(ApprovalMode {
        organization: organization.to_owned(),
        single_person: on,
        people_with_a_role: people,
        changed_at: (changed > 0)
            .then(|| jc_time::TimestampTz::from_second(changed))
            .flatten(),
        off_reason,
    })
}

/// Whether `identity` approves a Change of `project` that PF-58 sends to a second person alone,
/// in single-person mode: `Ok(false)` when the mode is off or an agent approves (PF-58's own
/// refusal follows), `Ok(true)` when it lets the approval through, and the refusal a person can
/// act on when the mode is on and the approval does not meet it.
pub async fn approves_alone(
    state: &AppState,
    identity: &Identity,
    project: &str,
    person: bool,
    stepped_up_at: Option<i64>,
) -> Result<bool, ApiError> {
    let Some(organization) = crate::permissions::organization_of(&state.mirror, project) else {
        return Ok(false);
    };
    let person = person && identity.client.is_none();
    let found = mode(state, &organization).await?;
    let others = others(state, &organization, identity);
    match verdict(
        found.single_person,
        person,
        stepped_up_at,
        crate::auth::session::now_unix(),
        others,
    ) {
        Ok(()) => Ok(true),
        Err(Refusal::Off | Refusal::NotAPerson) => Ok(false),
        // `mode` turned it off already when the count is past one; the person alone with a role
        // under another identifier (an e-mail and a user id of two people) is refused the same.
        Err(Refusal::OtherPeople(n)) => Err(ApiError::SelfApproval(format!(
            "single-person approvals are off for '{organization}': {n} other people hold a role there, so a second person approves this change (PF-58, T-7388)"
        ))),
        Err(Refusal::NotFresh) => {
            tracing::info!(organization, approver = %identity.username, "single-person approval asks for a fresh second factor");
            Err(ApiError::SecondFactorRequired)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    #[test]
    fn a_fresh_second_factor_of_the_one_person_approves() {
        assert_eq!(verdict(true, true, Some(NOW - 10), NOW, 0), Ok(()));
        assert_eq!(
            verdict(true, true, Some(NOW - FRESH_SECONDS), NOW, 0),
            Ok(())
        );
    }

    #[test]
    fn each_refusal() {
        assert_eq!(verdict(false, true, Some(NOW), NOW, 0), Err(Refusal::Off));
        assert_eq!(
            verdict(true, false, Some(NOW), NOW, 0),
            Err(Refusal::NotAPerson)
        );
        assert_eq!(verdict(true, true, None, NOW, 0), Err(Refusal::NotFresh));
        assert_eq!(
            verdict(true, true, Some(NOW - FRESH_SECONDS - 1), NOW, 0),
            Err(Refusal::NotFresh)
        );
        assert_eq!(
            verdict(true, true, Some(NOW + SKEW_SECONDS + 1), NOW, 0),
            Err(Refusal::NotFresh)
        );
        assert_eq!(
            verdict(true, true, Some(NOW), NOW, 1),
            Err(Refusal::OtherPeople(1))
        );
    }
}
