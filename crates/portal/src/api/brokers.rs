//! `GET /api/v1/organization/brokers`: every context broker of the organization with its spaces,
//! version, health and load (API/01 §43, PF-116, ADR-N-047). The Portal asks each broker; the
//! browser never reaches one.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use jc_core::kinds::{BrokerFeature, BrokerSize, ContextBrokerSpec, IMPLICIT_BROKER};
use jc_time::TimestampTz;
use serde::Serialize;
use utoipa::ToSchema;

use crate::auth::session::CurrentUser;
use crate::error::ApiError;
use crate::state::AppState;

/// The organization's brokers.
#[derive(Debug, Serialize, ToSchema)]
pub struct Brokers {
    pub items: Vec<Broker>,
}

/// One broker as the Portal sees it.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Broker {
    pub name: String,
    /// `false` for the implicit `main` of an organization that declares no broker.
    pub declared: bool,
    pub default: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>)]
    pub size: Option<BrokerSize>,
    #[schema(value_type = Vec<String>)]
    pub features: Vec<BrokerFeature>,
    pub health: Health,
    pub version: Option<String>,
    /// Why the broker is `down` or `unknown`, or why a space's count is missing.
    pub reason: Option<String>,
    /// The spaces living on it, as `project/space`.
    pub spaces: Vec<String>,
    /// The load: the entities its spaces hold, as API/01 §29 counts them.
    pub entities: u64,
    #[schema(value_type = String, format = DateTime)]
    pub observed_at: TimestampTz,
}

/// Whether a broker answered its health.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    Up,
    Down,
    /// The Portal has no address for it.
    Unknown,
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/brokers",
    summary = "List Context Brokers",
    description = "Every context broker of the organization with its spaces, version, health and load (API/01 §43, PF-116). Organization administrators only.",
    tag = "system",
    responses(
        (status = 200, description = "The brokers", body = Brokers),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "Not an organization administrator", body = crate::error::ProblemDetails),
    )
)]
pub async fn list_brokers(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<Brokers>, ApiError> {
    crate::permissions::require_organization_admin(&state, &user.0.identity, "the Brokers page")?;
    let mut declared: Vec<(String, ContextBrokerSpec)> = state
        .mirror
        .matching(|env| env.kind == "ContextBroker")
        .into_iter()
        .filter_map(|env| Some((env.metadata.name, serde_json::from_value(env.spec).ok()?)))
        .collect();
    declared.sort_by(|a, b| a.0.cmp(&b.0));
    let rows: Vec<(String, Option<ContextBrokerSpec>)> = match declared.is_empty() {
        true => vec![(IMPLICIT_BROKER.to_owned(), None)],
        false => declared
            .into_iter()
            .map(|(name, spec)| (name, Some(spec)))
            .collect(),
    };
    // Asked eight at a time, in order: one broker that does not answer costs one timeout, not
    // one per broker after it.
    use futures_util::StreamExt;
    let items = futures_util::stream::iter(rows)
        .map(|(name, spec)| row(&state, name, spec))
        .buffered(8)
        .collect()
        .await;
    Ok(Json(Brokers { items }))
}

async fn row(state: &AppState, name: String, spec: Option<ContextBrokerSpec>) -> Broker {
    let counter = &state.space_usage;
    let (health, version, mut reason) = match (counter.reaches(&name), counter.health(&name).await)
    {
        (false, Err(reason)) => (Health::Unknown, None, Some(reason)),
        (_, Ok(version)) => (Health::Up, Some(version).filter(|v| !v.is_empty()), None),
        (_, Err(reason)) => (Health::Down, None, Some(reason)),
    };
    let spaces = crate::brokers::spaces_on(&state.mirror, &name);
    let mut entities = 0;
    if health == Health::Up {
        for space in &spaces {
            let (project, space) = space.split_once('/').unwrap_or_default();
            let tenant = crate::spaces::tenant(&state.mirror, project, space);
            let usage = usage_of(counter, &name, tenant).await;
            match usage {
                Ok(usage) => entities += usage.entities,
                Err(why) => reason = Some(format!("{project}/{space}: {why}")),
            }
        }
    }
    Broker {
        declared: spec.is_some(),
        default: spec.as_ref().is_none_or(|spec| spec.default),
        size: spec.as_ref().map(|spec| spec.size),
        features: spec.map(|spec| spec.features).unwrap_or_default(),
        name,
        health,
        version,
        reason,
        spaces,
        entities,
        observed_at: TimestampTz::now(),
    }
}

/// The usage of a space on broker `name`, or why its tenant is not known.
async fn usage_of(
    counter: &crate::space_usage::Counter,
    name: &str,
    tenant: Result<String, String>,
) -> Result<crate::space_usage::SpaceUsage, String> {
    counter.usage(name, &tenant?).await
}

pub fn router() -> Router<AppState> {
    Router::new().route("/organization/brokers", get(list_brokers))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// T-7917 (PF-130): a space whose tenant is not known reads as the reason, never as a count,
    /// and the broker is not asked.
    #[tokio::test]
    async fn a_space_without_a_tenant_reads_as_why() {
        let counter = crate::space_usage::Counter::new(None, None);
        let reason = usage_of(
            &counter,
            "main",
            Err("project `helsinki` has no id in the Portal's project registry yet".to_owned()),
        )
        .await
        .expect_err("no tenant");
        assert!(
            reason.contains("no id in the Portal's project registry"),
            "{reason}"
        );
    }

    /// T-7917 (PF-116): a broker that answers its health is `up` with its version, and its load
    /// is the entities of every space living on it.
    #[tokio::test]
    async fn a_broker_that_answers_is_up_and_carries_the_load_of_its_spaces() {
        use wiremock::matchers::{method, path, path_regex};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let broker = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/q/health"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "version": "1.4.2" })))
            .mount(&broker)
            .await;
        Mock::given(method("GET"))
            .and(path_regex("^/q/tenants/[^/]+$"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "counts": { "entities": 5 } })),
            )
            .mount(&broker)
            .await;
        let token = std::env::temp_dir().join(format!("brokers-token-{}", std::process::id()));
        std::fs::write(&token, "admin-token\n").expect("a token file");
        let mut state = AppState::new(crate::config::Config::for_tests(), None);
        state.space_usage = std::sync::Arc::new(crate::space_usage::Counter::new(
            Some(broker.uri()),
            Some(token.clone()),
        ));
        for space in ["air", "bikes"] {
            state.mirror.upsert(crate::resource::ResourceEnvelope {
                api_version: crate::resource::API_VERSION.to_owned(),
                kind: "ContextSpace".to_owned(),
                metadata: crate::resource::ObjectMeta::new(space, "helsinki"),
                spec: json!({}),
                status: None,
            });
        }
        let user = CurrentUser(crate::auth::session::Session {
            identity: crate::auth::Identity {
                client: None,
                subject: "sub-admin".to_owned(),
                username: "admin".to_owned(),
                email: None,
                name: None,
                roles: Vec::new(),
                groups: vec!["portal-approver".to_owned()],
                second_factor: false,
            },
            expires_at: 9_999_999_999,
            issued_at: 0,
            signed_in_at: None,
            stepped_up_at: None,
            id_token: "t".to_owned(),
            access_expires_at: 9_999_999_999,
            refresh_token: None,
        });
        let answer = list_brokers(user, State(state)).await;
        let _ = std::fs::remove_file(&token);
        let listed = answer.expect("the brokers").0;
        let main = listed.items.first().expect("the implicit broker");
        assert_eq!(main.health, Health::Up);
        assert_eq!(main.version.as_deref(), Some("1.4.2"));
        assert_eq!(main.spaces, ["helsinki/air", "helsinki/bikes"]);
        assert_eq!(main.entities, 10);
        assert!(main.reason.is_none(), "{:?}", main.reason);
    }
}
