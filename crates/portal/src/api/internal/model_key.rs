//! The model key's state, as the agent proxy last saw it (AG-96, API/04 §7.2): metrics for the
//! alerts, and an activity event in the project `org` when the key stops working, runs low or
//! recovers. The proxy is the only holder of the key; nothing here ever sees it.

use std::sync::Mutex;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::Router;
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::AppState;

/// The share of the key's limit below which the credit counts as low (AG-96).
const LOW_SHARE: f64 = 0.2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyState {
    Valid,
    Invalid,
    OutOfCredit,
    Unreachable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Probe,
    Call,
}

/// One report of the proxy's.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyReport {
    pub state: KeyState,
    pub source: Source,
    #[serde(default)]
    pub limit: Option<f64>,
    #[serde(default)]
    pub usage: Option<f64>,
    #[serde(default)]
    pub remaining: Option<f64>,
}

impl KeyReport {
    /// A credit figure is a finite, non-negative number of the provider's credits, or nothing.
    fn check(&self) -> Result<(), String> {
        for (name, value) in [
            ("limit", self.limit),
            ("usage", self.usage),
            ("remaining", self.remaining),
        ] {
            if value.is_some_and(|v| !v.is_finite() || v < 0.0) {
                return Err(format!("{name} is not a number of credits"));
            }
        }
        Ok(())
    }

    /// Whether what is left is below [`LOW_SHARE`] of the limit; a key without a limit never is.
    fn low(&self) -> bool {
        match (self.limit, self.remaining) {
            (Some(limit), Some(remaining)) if limit > 0.0 => remaining < limit * LOW_SHARE,
            _ => false,
        }
    }
}

/// What the Portal last knew: the state and whether the credit was low.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Known {
    state: KeyState,
    low: bool,
}

/// The activity event one report is worth, given what was known before it: a severity and a
/// sentence, or nothing. An `unreachable` probe says nothing about the key and changes nothing.
pub fn transition(before: Option<Known>, report: &KeyReport) -> Option<(&'static str, String)> {
    let low = report.low();
    let was = before.map(|known| known.state);
    match report.state {
        KeyState::Unreachable => None,
        KeyState::Invalid if was != Some(KeyState::Invalid) => Some((
            "error",
            "The model provider refuses the Portal's key: the assistant and the app builder \
             cannot answer until an administrator replaces it."
                .to_owned(),
        )),
        KeyState::OutOfCredit if was != Some(KeyState::OutOfCredit) => Some((
            "error",
            "The model key has no credit left: the assistant and the app builder cannot answer \
             until an administrator tops it up."
                .to_owned(),
        )),
        KeyState::Valid if matches!(was, Some(KeyState::Invalid | KeyState::OutOfCredit)) => {
            Some(("info", "The model key works again.".to_owned()))
        }
        KeyState::Valid if low && !before.is_some_and(|known| known.low) => Some((
            "warning",
            format!(
                "The model key's credit is low: {:.2} of {:.2} left. An administrator tops it up \
                 before the assistant and the app builder stop.",
                report.remaining.unwrap_or_default(),
                report.limit.unwrap_or_default()
            ),
        )),
        _ => None,
    }
}

/// The last report this replica received.
// ponytail: per replica; with several Portal replicas each keeps the reports it was sent, and the
// alerts read the newest series. A shared row is the upgrade when the Portal scales out.
static KNOWN: Mutex<Option<Known>> = Mutex::new(None);

/// Whether the app builder can answer, as far as this replica heard: `false` from the report
/// that the key is refused or out of credit until one says it works again (T-5617). A Portal no
/// report reached yet counts the key as working; the run itself says so when it does not.
pub fn builder_answers() -> bool {
    let known = *KNOWN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    !matches!(
        known.map(|known| known.state),
        Some(KeyState::Invalid | KeyState::OutOfCredit)
    )
}

pub async fn model_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    crate::auth::internal::authenticate_agent_proxy(&state, &headers).await?;
    let report: KeyReport = serde_json::from_slice(&body)
        .map_err(|err| ApiError::BadRequest(format!("not a model key report: {err}")))?;
    report.check().map_err(ApiError::BadRequest)?;
    if report.state == KeyState::Unreachable {
        tracing::warn!(source = ?report.source, "the proxy could not reach the model provider's key endpoint");
        return Ok(StatusCode::NO_CONTENT);
    }
    crate::telemetry::model_key(
        report.state == KeyState::Valid,
        report.limit,
        report.remaining,
    );
    let event = {
        let mut known = KNOWN
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let event = transition(*known, &report);
        *known = Some(Known {
            state: report.state,
            low: report.low(),
        });
        event
    };
    if let Some((severity, summary)) = event {
        tracing::warn!(state = ?report.state, %summary, "the model key changed");
        let event = crate::activity::ActivityEvent {
            time: chrono::Utc::now(),
            project: crate::permissions::ORG_NAMESPACE.to_owned(),
            space: None,
            kind: "model.key".to_owned(),
            source: "portal".to_owned(),
            summary,
            severity: severity.to_owned(),
            correlation_id: None,
            details: serde_json::json!({
                "state": match report.state {
                    KeyState::Valid => "valid",
                    KeyState::Invalid => "invalid",
                    KeyState::OutOfCredit => "out_of_credit",
                    KeyState::Unreachable => "unreachable",
                },
                "limit": report.limit,
                "remaining": report.remaining,
            }),
        };
        if let Err(err) = state.activity.append(&[event]).await {
            tracing::warn!(error = %err, "the model key's change is not on the activity feed");
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

pub fn router() -> Router<AppState> {
    Router::new().route("/internal/model-key", post(model_key))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(state: KeyState, limit: Option<f64>, remaining: Option<f64>) -> KeyReport {
        KeyReport {
            state,
            source: Source::Probe,
            limit,
            usage: None,
            remaining,
        }
    }

    fn known(state: KeyState, low: bool) -> Option<Known> {
        Some(Known { state, low })
    }

    #[test]
    fn a_key_that_stops_working_is_said_once_and_its_recovery_once() {
        let dead = report(KeyState::Invalid, None, None);
        assert_eq!(transition(None, &dead).map(|(s, _)| s), Some("error"));
        assert_eq!(transition(known(KeyState::Invalid, false), &dead), None);
        let back = report(KeyState::Valid, Some(10.0), Some(9.0));
        assert_eq!(
            transition(known(KeyState::Invalid, false), &back),
            Some(("info", "The model key works again.".to_owned()))
        );
        assert_eq!(transition(known(KeyState::Valid, false), &back), None);
        // A Portal that starts and hears a working key says nothing.
        assert_eq!(transition(None, &back), None);
    }

    #[test]
    fn credit_below_a_fifth_of_the_limit_is_a_warning_once_per_crossing() {
        let low = report(KeyState::Valid, Some(10.0), Some(1.99));
        let (severity, said) = transition(known(KeyState::Valid, false), &low).expect("an event");
        assert_eq!(severity, "warning");
        assert!(said.contains("1.99 of 10.00 left"), "{said}");
        assert_eq!(transition(known(KeyState::Valid, true), &low), None);
        // Exactly a fifth is not below it, and a key without a limit is never low.
        assert!(!report(KeyState::Valid, Some(10.0), Some(2.0)).low());
        assert!(!report(KeyState::Valid, None, Some(0.0)).low());
    }

    #[test]
    fn an_empty_key_and_an_unreachable_provider_are_told_apart() {
        let empty = report(KeyState::OutOfCredit, None, None);
        assert_eq!(
            transition(known(KeyState::Valid, true), &empty).map(|(s, _)| s),
            Some("error")
        );
        let unreachable = report(KeyState::Unreachable, None, None);
        assert_eq!(
            transition(known(KeyState::Valid, false), &unreachable),
            None
        );
    }

    /// T-5617: the gallery offers Generate again only once the key works again.
    #[test]
    fn the_builder_answers_until_the_key_is_refused_or_empty() {
        let set = |known: Option<Known>| {
            *KNOWN
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = known;
        };
        set(None);
        assert!(builder_answers());
        set(known(KeyState::OutOfCredit, false));
        assert!(!builder_answers());
        set(known(KeyState::Invalid, false));
        assert!(!builder_answers());
        set(known(KeyState::Valid, true));
        assert!(builder_answers());
        set(None);
    }

    #[test]
    fn a_report_is_the_documented_shape_and_nothing_else() {
        let body =
            br#"{"state":"valid","source":"probe","limit":10.0,"usage":2.41,"remaining":7.59}"#;
        let parsed: KeyReport = serde_json::from_slice(body).expect("the documented report");
        assert_eq!(parsed.remaining, Some(7.59));
        assert!(serde_json::from_slice::<KeyReport>(
            br#"{"state":"valid","source":"probe","key":"sk-or-..."}"#
        )
        .is_err());
        assert!(
            serde_json::from_slice::<KeyReport>(br#"{"state":"broken","source":"probe"}"#).is_err()
        );
        assert!(report(KeyState::Valid, Some(-1.0), None).check().is_err());
    }
}
