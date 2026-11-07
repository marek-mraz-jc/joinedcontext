//! Admission for many context brokers (ADR-N-047, PF-110, PF-115, PF-116).
//!
//! Three refusals, each before a Change exists so every door answers the same: a
//! `ContextBroker` a space lives on is not deleted, a space holding entities does not change
//! broker by an edit (the move job does it, T-3843), and an Endpoint does not serve a
//! representation its space's broker lacks.

use jc_core::kinds::{BrokerFeature, ContextBrokerSpec, EndpointSpec, Representation};
use serde_json::Value;

use crate::error::ApiError;
use crate::state::AppState;
use crate::store::Mirror;

/// Every space living on broker `name`, as `project/space`, sorted.
pub fn spaces_on(mirror: &Mirror, name: &str) -> Vec<String> {
    let mut spaces: Vec<String> = mirror
        .matching(|env| env.kind == "ContextSpace")
        .into_iter()
        .filter_map(|env| {
            let project = env.metadata.namespace.unwrap_or_default();
            (crate::spaces::broker(mirror, &project, &env.metadata.name) == name)
                .then(|| format!("{project}/{}", env.metadata.name))
        })
        .collect();
    spaces.sort();
    spaces
}

/// A `ContextBroker` a space lives on stays (ADR-N-047 §2.6); the refusal lists the spaces.
pub fn check_delete(mirror: &Mirror, kind: &str, name: &str) -> Result<(), ApiError> {
    if kind != "ContextBroker" {
        return Ok(());
    }
    let spaces = spaces_on(mirror, name);
    if spaces.is_empty() {
        return Ok(());
    }
    Err(ApiError::Conflict(format!(
        "broker '{name}' holds the spaces {}; move them to another broker first (PF-115)",
        spaces.join(", ")
    )))
}

/// The broker feature a representation needs, if the broker serves it rather than the gateway.
/// `mcp` is the gateway's own MCP instance (ADR-N-025), not the broker's MCP server.
fn needs(representation: Representation) -> Option<BrokerFeature> {
    match representation {
        Representation::OgcFeatures => Some(BrokerFeature::Ogc),
        Representation::Odata => Some(BrokerFeature::Odata),
        _ => None,
    }
}

/// The declared broker `name`, or `None` for the implicit `main` and an unknown name.
fn declared(mirror: &Mirror, name: &str) -> Option<ContextBrokerSpec> {
    mirror
        .find(|env| env.kind == "ContextBroker" && env.metadata.name == name)
        .and_then(|env| serde_json::from_value(env.spec).ok())
}

/// An Endpoint serves no representation its space's broker lacks (PF-110). The implicit `main`
/// of an organization that declares no broker serves every surface.
pub fn check_endpoint(
    mirror: &Mirror,
    project: &str,
    kind: &str,
    spec: &Value,
) -> Result<(), ApiError> {
    if kind != "Endpoint" {
        return Ok(());
    }
    let Ok(endpoint) = serde_json::from_value::<EndpointSpec>(spec.clone()) else {
        return Ok(());
    };
    let broker = crate::spaces::broker(mirror, project, endpoint.context_space_ref.name());
    let Some(declared) = declared(mirror, &broker) else {
        return Ok(());
    };
    let mut lacking: Vec<&str> = endpoint
        .served_representations()
        .into_iter()
        .filter(|r| needs(*r).is_some_and(|feature| !declared.features.contains(&feature)))
        .map(|r| r.as_str())
        .collect();
    lacking.sort_unstable();
    if lacking.is_empty() {
        return Ok(());
    }
    Err(ApiError::BadRequest(format!(
        "broker '{broker}' does not serve {}; add them to the broker's features or list them in \
         the Endpoint's disabledRepresentations (PF-110)",
        lacking.join(", ")
    )))
}

/// A space that holds entities changes broker only by the move job (ADR-N-047 §2.6, PF-115):
/// an edit of its `brokerRef` that would leave its entities behind is refused.
pub async fn check_move(
    state: &AppState,
    project: &str,
    kind: &str,
    name: &str,
    spec: &Value,
) -> Result<(), ApiError> {
    if kind != "ContextSpace" || state.mirror.get(project, kind, name).is_none() {
        return Ok(());
    }
    let now = crate::spaces::broker(&state.mirror, project, name);
    let declared: Vec<(String, ContextBrokerSpec)> = state
        .mirror
        .matching(|env| env.kind == "ContextBroker")
        .into_iter()
        .filter_map(|env| Some((env.metadata.name, serde_json::from_value(env.spec).ok()?)))
        .collect();
    let broker_ref = spec.get("brokerRef").and_then(Value::as_str);
    let next = jc_core::kinds::broker_of(
        broker_ref,
        declared.iter().map(|(name, spec)| (name.as_str(), spec)),
    )
    .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    if next == now {
        return Ok(());
    }
    let tenant = crate::spaces::segment(&state.mirror, project, name);
    // a fresh count: a kept zero up to hours old must not let entities stay behind
    match state.space_usage.measure_now(&now, &tenant).await {
        Ok(usage) if usage.entities == 0 => Ok(()),
        Ok(usage) => Err(ApiError::Conflict(format!(
            "space '{name}' holds {} entities on broker '{now}'; use the move job to take it to \
             '{next}' (PF-115)",
            usage.entities
        ))),
        Err(reason) => Err(ApiError::Conflict(format!(
            "space '{name}' could not be counted on broker '{now}' ({reason}), so it is not \
             moved to '{next}'; use the move job (PF-115)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
    use serde_json::json;

    fn envelope(kind: &str, namespace: &str, name: &str, spec: Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: kind.into(),
            metadata: ObjectMeta {
                name: name.into(),
                namespace: Some(namespace.into()),
                ..Default::default()
            },
            spec,
            status: None,
        }
    }

    fn mirror(features: Value) -> Mirror {
        let image = format!("ghcr.io/x/antares@sha256:{}", "a".repeat(64));
        let mirror = Mirror::new();
        for item in [
            envelope(
                "ContextBroker",
                "org",
                "main",
                json!({"default": true, "image": image}),
            ),
            envelope(
                "ContextBroker",
                "org",
                "canary",
                json!({"image": image, "features": features}),
            ),
            envelope("ContextSpace", "city", "air", json!({})),
            envelope(
                "ContextSpace",
                "city",
                "cars",
                json!({"brokerRef": "canary"}),
            ),
            envelope(
                "ContextSpace",
                "port",
                "ships",
                json!({"brokerRef": "canary"}),
            ),
        ] {
            mirror.upsert(item);
        }
        mirror
    }

    fn endpoint(extra: Value) -> Value {
        let mut spec = json!({
            "contextSpaceRef": "cars",
            "slug": "abcdefghijklmnopqrstuvwxyz234567",
            "audience": "public",
        });
        spec.as_object_mut()
            .expect("an object")
            .extend(extra.as_object().expect("an object").clone());
        spec
    }

    #[test]
    fn a_broker_a_space_lives_on_is_not_deleted_and_the_refusal_lists_the_spaces() {
        let mirror = mirror(json!([]));
        let err = check_delete(&mirror, "ContextBroker", "canary").unwrap_err();
        assert!(
            matches!(&err, ApiError::Conflict(m) if m.contains("city/cars, port/ships")),
            "{err:?}"
        );
        // `air` names no broker: it lives on the default, so the default is held too.
        assert!(check_delete(&mirror, "ContextBroker", "main").is_err());
        assert!(check_delete(&mirror, "ContextBroker", "spare").is_ok());
        assert!(check_delete(&mirror, "ContextSpace", "canary").is_ok());
    }

    #[test]
    fn an_endpoint_needing_a_surface_its_broker_lacks_is_refused_naming_the_broker() {
        let mirror = mirror(json!(["odata"]));
        let err = check_endpoint(&mirror, "city", "Endpoint", &endpoint(json!({}))).unwrap_err();
        assert!(
            matches!(&err, ApiError::BadRequest(m) if m.contains("'canary'") && m.contains("ogc-features") && !m.contains("odata")),
            "{err:?}"
        );
        let without = endpoint(json!({"disabledRepresentations": ["ogc-features"]}));
        assert!(check_endpoint(&mirror, "city", "Endpoint", &without).is_ok());
        let listed = endpoint(json!({"enabledRepresentations": ["ngsi-ld", "odata"]}));
        assert!(check_endpoint(&mirror, "city", "Endpoint", &listed).is_ok());
    }

    /// PF-115: a space holding entities does not change broker by an edit; an empty one does,
    /// and an edit that keeps its broker asks the broker nothing.
    #[tokio::test]
    async fn a_space_holding_entities_moves_only_by_the_move_job() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let broker = MockServer::start().await;
        for (tenant, count) in [("city-air", 12), ("port-ships", 0)] {
            Mock::given(method("GET"))
                .and(path(format!("/q/tenants/{tenant}")))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"counts": {"entities": count}})),
                )
                .expect(1)
                .mount(&broker)
                .await;
        }
        let token = std::env::temp_dir().join(format!("brokers-token-{}", std::process::id()));
        std::fs::write(&token, "t").expect("write the token");
        let mut config = crate::config::Config::for_tests();
        config.broker_url = Some(broker.uri());
        config.broker_admin_token_file = Some(token.clone());
        let state = AppState::new(config, None);
        state.mirror.replace_all(&mirror(json!([])));
        state
            .mirror
            .upsert(envelope("ContextSpace", "port", "ships", json!({})));

        let to_canary = json!({"brokerRef": "canary"});
        let held = check_move(&state, "city", "ContextSpace", "air", &to_canary).await;
        let empty = check_move(&state, "port", "ContextSpace", "ships", &to_canary).await;
        let kept = check_move(&state, "city", "ContextSpace", "cars", &to_canary).await;
        let new = check_move(&state, "city", "ContextSpace", "fresh", &to_canary).await;
        let unknown = check_move(
            &state,
            "city",
            "ContextSpace",
            "air",
            &json!({"brokerRef": "x"}),
        )
        .await;
        let _ = std::fs::remove_file(&token);

        assert!(
            matches!(&held, Err(ApiError::Conflict(m)) if m.contains("12 entities") && m.contains("move job")),
            "{held:?}"
        );
        assert!(empty.is_ok(), "{empty:?}");
        assert!(kept.is_ok() && new.is_ok());
        assert!(
            matches!(unknown, Err(ApiError::BadRequest(_))),
            "{unknown:?}"
        );
    }

    /// T-6208: a zero the Portal keeps for hours lets no space move while its broker is away;
    /// the move check counts the space again and refuses when it cannot.
    #[tokio::test]
    async fn a_kept_zero_moves_no_space_while_its_broker_is_away() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let broker = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/q/tenants/port-ships"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"counts": {"entities": 0}})),
            )
            .up_to_n_times(1)
            .mount(&broker)
            .await;
        Mock::given(method("GET"))
            .and(path("/q/tenants/port-ships"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&broker)
            .await;
        let token = std::env::temp_dir().join(format!("brokers-away-{}", std::process::id()));
        std::fs::write(&token, "t").expect("write the token");
        let mut config = crate::config::Config::for_tests();
        config.broker_url = Some(broker.uri());
        config.broker_admin_token_file = Some(token.clone());
        let state = AppState::new(config, None);
        state.mirror.replace_all(&mirror(json!([])));
        state
            .mirror
            .upsert(envelope("ContextSpace", "port", "ships", json!({})));

        let kept = state.space_usage.usage("main", "port-ships").await;
        let moved = check_move(
            &state,
            "port",
            "ContextSpace",
            "ships",
            &json!({"brokerRef": "canary"}),
        )
        .await;
        let _ = std::fs::remove_file(&token);

        assert_eq!(kept.map(|u| u.entities), Ok(0));
        assert!(
            matches!(&moved, Err(ApiError::Conflict(m)) if m.contains("could not be counted") && m.contains("503")),
            "{moved:?}"
        );
    }

    #[test]
    fn the_implicit_main_serves_every_surface() {
        let mirror = Mirror::new();
        mirror.upsert(envelope("ContextSpace", "city", "cars", json!({})));
        assert!(check_endpoint(&mirror, "city", "Endpoint", &endpoint(json!({}))).is_ok());
    }
}
