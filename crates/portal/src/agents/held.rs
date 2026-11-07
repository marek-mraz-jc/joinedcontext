//! A run's data needs against the caller's own grants (AP-44, AP-132, PF-70).
//!
//! What an App may do is compiled from its `dataNeeds` exactly (AP-06), so a need wider than
//! what the person who asked for it holds would give them, through the App, what the platform
//! refuses them directly. The context gateway's `/access` document is the one statement of what
//! a caller holds on an endpoint (EP-55, EP-60); the Portal reads it with the caller's own token
//! and compares, rather than evaluating Policies a second time.

use std::collections::BTreeMap;
use std::time::Duration;

use jc_core::kinds::policy::OperationRef;
use serde_json::Value;

use crate::agents::endpoints::{self, RunEndpoint};
use crate::error::ApiError;

/// How long one `/access` read may take before the run is refused as unavailable.
const ACCESS_TIMEOUT: Duration = Duration::from_secs(10);

/// What a caller holds on one endpoint, per entity type: the operations its permissions grant
/// minus the ones a prohibition takes away (GW8).
#[derive(Debug, Default, PartialEq)]
pub struct Held {
    granted: BTreeMap<String, Vec<String>>,
    prohibited: BTreeMap<String, Vec<String>>,
}

impl Held {
    /// The gateway's `/access` document, as `permissions` and `prohibitions` of
    /// `{resource: {type}, actions}` entries.
    pub fn from_document(document: &Value) -> Self {
        let read = |key: &str| {
            let mut by_type: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for entry in document[key].as_array().into_iter().flatten() {
                let Some(entity_type) = entry["resource"]["type"].as_str() else {
                    continue;
                };
                let actions = entry["actions"].as_array().into_iter().flatten();
                by_type
                    .entry(entity_type.to_owned())
                    .or_default()
                    .extend(actions.filter_map(Value::as_str).map(str::to_owned));
            }
            by_type
        };
        Self {
            granted: read("permissions"),
            prohibited: read("prohibitions"),
        }
    }

    pub fn holds(&self, entity_type: &str, operation: &str) -> bool {
        // A grant on `*` covers every type, as the Access panel reads it.
        let named = |map: &BTreeMap<String, Vec<String>>| {
            [entity_type, "*"].iter().any(|key| {
                map.get(*key)
                    .is_some_and(|ops| ops.iter().any(|op| op == operation))
            })
        };
        named(&self.granted) && !named(&self.prohibited)
    }
}

/// The single operation names one need asks for, a group expanded into its members.
fn operations_of(need: &Value) -> Vec<String> {
    need["operations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|op| serde_json::from_value::<OperationRef>(op.clone()).ok())
        .flat_map(|op| match op {
            OperationRef::Single(single) => vec![single.as_str().to_owned()],
            OperationRef::Group(group) => group
                .operations()
                .iter()
                .map(|single| single.as_str().to_owned())
                .collect(),
        })
        .collect()
}

/// Whether any need asks for a write, as jc-core counts one (AP-98): `appendAttrs`, `deleteAttrs`
/// and the batch writes included.
pub fn writes(needs: &[Value]) -> bool {
    needs.iter().any(|need| {
        need["operations"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|op| serde_json::from_value::<OperationRef>(op.clone()).ok())
            .any(|op| op.is_write())
    })
}

/// Every operation a need asks for on one of its types that the caller does not hold on the
/// need's endpoint, one sentence each. `held` is aligned with `run_endpoints`; a need is checked
/// against the first endpoint it belongs to (AP-44).
pub fn beyond(run_endpoints: &[RunEndpoint], held: &[Held], needs: &[Value]) -> Vec<String> {
    let mut violations = Vec::new();
    for (idx, need) in needs.iter().enumerate() {
        let Some(&at) = endpoints::of_need(run_endpoints, need).first() else {
            continue;
        };
        let (Some(endpoint), Some(held)) = (run_endpoints.get(at), held.get(at)) else {
            continue;
        };
        let types = need["types"].as_array().into_iter().flatten();
        for entity_type in types.filter_map(Value::as_str) {
            for operation in operations_of(need) {
                if !held.holds(entity_type, &operation) {
                    violations.push(format!(
                        "dataNeeds[{idx}]: you may not {operation} {entity_type} on endpoint \
                         '{}', so the app may not either (AP-132, PF-70)",
                        endpoint.name
                    ));
                }
            }
        }
    }
    violations
}

/// The caller's `/access` document of one endpoint, read from the context gateway with their token.
async fn access_document(
    http: &reqwest::Client,
    gateway: &str,
    slug: &str,
    token: &str,
) -> Result<Value, ApiError> {
    let url = format!(
        "{}/api/endpoint/{slug}/access",
        gateway.trim_end_matches('/')
    );
    let unavailable = |reason: String| {
        ApiError::Unavailable(format!(
            "the context gateway did not say what you may do on endpoint '{slug}', so the run's \
             data needs cannot be checked; try again in a moment ({reason})"
        ))
    };
    let response = http
        .get(url)
        .bearer_auth(token)
        .header(reqwest::header::ACCEPT, "application/json")
        .timeout(ACCESS_TIMEOUT)
        .send()
        .await
        .map_err(|err| unavailable(err.without_url().to_string()))?;
    match response.status() {
        status if status.is_success() => response
            .json()
            .await
            .map_err(|err| unavailable(err.without_url().to_string())),
        // Not admitted to the endpoint at all: every need on it is beyond the caller.
        reqwest::StatusCode::UNAUTHORIZED
        | reqwest::StatusCode::FORBIDDEN
        | reqwest::StatusCode::NOT_FOUND => Ok(Value::Null),
        status => Err(unavailable(format!("HTTP {status}"))),
    }
}

/// The Portal's own read check of a run Endpoint for `identity`: the judge of a session that
/// carries no token for the gateway.
pub fn portal_reads<'a>(
    state: &'a crate::state::AppState,
    identity: &'a crate::auth::Identity,
    project: &'a str,
) -> impl Fn(&RunEndpoint) -> bool + Sync + 'a {
    let effective = crate::permissions::for_request(state, identity, project);
    move |endpoint| {
        state
            .mirror
            .get(project, "Endpoint", &endpoint.name)
            .and_then(|manifest| serde_json::to_value(manifest).ok())
            .is_some_and(|manifest| effective.may_read_manifest("Endpoint", &manifest))
    }
}

/// One client for every `/access` read, without redirects: the gateway's address is configuration.
pub fn client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(ACCESS_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default()
    })
}

/// AP-132: the run's needs checked against what the caller holds. `gateway` is the context
/// gateway's address; an installation without one serves no data, so nothing the run could ask
/// for is reachable and there is nothing to compare against. A session without a token (the
/// Portal's own cookie) cannot be checked at the gateway, so it may not ask for a write, and its
/// reads are judged by `may_read`, the Portal's own grants on each run Endpoint (F-POR1-004).
pub async fn check(
    http: &reqwest::Client,
    gateway: Option<&str>,
    token: Option<&str>,
    run_endpoints: &[RunEndpoint],
    needs: &[Value],
    may_read: &(dyn Fn(&RunEndpoint) -> bool + Sync),
) -> Result<(), ApiError> {
    let Some(gateway) = gateway else {
        return Ok(());
    };
    let Some(token) = token else {
        if writes(needs) {
            return Err(ApiError::Invalid {
                detail: "declared dataNeeds exceed what the caller holds (AP-44)".to_owned(),
                errors: vec![
                    "a write data need is checked against your own grants, and this sign-in \
                     carries no token to check them with: open the Portal through its address \
                     and sign in there (AP-132)"
                        .to_owned(),
                ],
            });
        }
        let unreadable: Vec<String> = run_endpoints
            .iter()
            .filter(|endpoint| !may_read(endpoint))
            .map(|endpoint| {
                format!(
                    "endpoint '{}': you may not read it, so the app may not either (AP-132, PF-70)",
                    endpoint.name
                )
            })
            .collect();
        if !unreadable.is_empty() {
            return Err(ApiError::Invalid {
                detail: "declared dataNeeds exceed what the caller holds (AP-44)".to_owned(),
                errors: unreadable,
            });
        }
        return Ok(());
    };
    let mut held = Vec::with_capacity(run_endpoints.len());
    for endpoint in run_endpoints {
        let document = access_document(http, gateway, &endpoint.slug, token).await?;
        held.push(Held::from_document(&document));
    }
    let violations = beyond(run_endpoints, &held, needs);
    if violations.is_empty() {
        Ok(())
    } else {
        Err(ApiError::Invalid {
            detail: "declared dataNeeds exceed what the caller holds (AP-44)".to_owned(),
            errors: violations,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn endpoints() -> Vec<RunEndpoint> {
        vec![
            RunEndpoint {
                name: "alerts".into(),
                slug: "s1".into(),
                space: "traffic".into(),
            },
            RunEndpoint {
                name: "stations".into(),
                slug: "s2".into(),
                space: "air".into(),
            },
        ]
    }

    fn need(space: &str, types: &[&str], operations: &[&str]) -> Value {
        json!({
            "contextSpaceRef": { "kind": "ContextSpace", "name": space },
            "types": types,
            "operations": operations,
        })
    }

    fn document(permissions: Value, prohibitions: Value) -> Value {
        json!({ "permissions": permissions, "prohibitions": prohibitions })
    }

    #[test]
    fn a_need_within_the_callers_grant_passes_and_a_wider_one_is_named() {
        let reader = Held::from_document(&document(
            json!([{ "resource": { "type": "Alert" }, "actions": ["queryEntity", "retrieveEntity"] }]),
            json!([]),
        ));
        let editor = Held::from_document(&document(
            json!([{ "resource": { "type": "Station" }, "actions": ["queryEntity", "updateAttrs"] }]),
            json!([]),
        ));
        let held = [reader, editor];
        let within = [
            need("traffic", &["Alert"], &["queryEntity", "retrieveEntity"]),
            need("air", &["Station"], &["queryEntity", "updateAttrs"]),
        ];
        assert!(beyond(&endpoints(), &held, &within).is_empty());

        let wider = [need("traffic", &["Alert"], &["queryEntity", "updateAttrs"])];
        let found = beyond(&endpoints(), &held, &wider);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].contains("updateAttrs Alert on endpoint 'alerts'"),
            "{found:?}"
        );
    }

    #[test]
    fn a_prohibition_an_other_type_or_no_admission_hold_nothing() {
        let held = Held::from_document(&document(
            json!([
                { "resource": { "type": "Alert" }, "actions": ["queryEntity", "deleteEntity"] },
                { "resource": { "type": "Road" }, "actions": ["createEntity"] }
            ]),
            json!([{ "resource": { "type": "Alert" }, "actions": ["deleteEntity"] }]),
        ));
        assert!(held.holds("Alert", "queryEntity"));
        assert!(
            !held.holds("Alert", "deleteEntity"),
            "the prohibition wins (GW8)"
        );
        assert!(
            !held.holds("Alert", "createEntity"),
            "a grant on another type"
        );
        let everything = Held::from_document(&document(
            json!([{ "resource": { "type": "*" }, "actions": ["updateAttrs"] }]),
            json!([{ "resource": { "type": "*" }, "actions": ["deleteEntity"] }]),
        ));
        assert!(everything.holds("Alert", "updateAttrs"));
        assert!(!everything.holds("Alert", "deleteEntity"));
        assert_eq!(Held::from_document(&Value::Null), Held::default());
        assert!(!Held::default().holds("Alert", "queryEntity"));
    }

    #[test]
    fn a_group_is_checked_member_by_member_and_every_write_counts() {
        let held = [
            Held::from_document(&document(
                json!([{ "resource": { "type": "Alert" }, "actions": ["queryEntity"] }]),
                json!([]),
            )),
            Held::default(),
        ];
        let grouped = [need("traffic", &["Alert"], &["queryEntity", "updateOps"])];
        assert!(!beyond(&endpoints(), &held, &grouped).is_empty());
        assert!(writes(&grouped));
        assert!(writes(&[need("traffic", &["Alert"], &["appendAttrs"])]));
        assert!(writes(&[need("traffic", &["Alert"], &["deleteAttrs"])]));
        assert!(!writes(&[need(
            "traffic",
            &["Alert"],
            &[
                "queryEntity",
                "retrieveEntity",
                "queryTemporal",
                "retrieveTemporal"
            ]
        )]));
    }

    #[test]
    fn a_need_of_another_space_is_checked_on_the_primary() {
        let held = [
            Held::from_document(&document(
                json!([{ "resource": { "type": "Alert" }, "actions": ["queryEntity"] }]),
                json!([]),
            )),
            Held::default(),
        ];
        let elsewhere = [need("parking", &["Alert"], &["queryEntity"])];
        assert!(beyond(&endpoints(), &held, &elsewhere).is_empty());
    }

    #[tokio::test]
    async fn a_session_without_a_token_starts_no_write_and_no_gateway_checks_nothing() {
        let http = reqwest::Client::new();
        let write = [need("traffic", &["Alert"], &["updateAttrs"])];
        let read = [need("traffic", &["Alert"], &["queryEntity"])];
        let reads_all = |_: &RunEndpoint| true;
        let refused = check(
            &http,
            Some("http://gw"),
            None,
            &endpoints(),
            &write,
            &reads_all,
        )
        .await;
        assert!(
            matches!(refused, Err(ApiError::Invalid { .. })),
            "{refused:?}"
        );
        assert!(check(
            &http,
            Some("http://gw"),
            None,
            &endpoints(),
            &read,
            &reads_all
        )
        .await
        .is_ok());
        assert!(
            check(&http, None, Some("t"), &endpoints(), &write, &reads_all)
                .await
                .is_ok()
        );
    }

    /// F-POR1-004: a session without a token reads only the Endpoints the Portal lets it read.
    #[tokio::test]
    async fn a_session_without_a_token_reads_only_endpoints_its_grants_reach() {
        let http = reqwest::Client::new();
        let read = [need("traffic", &["Alert"], &["queryEntity"])];
        let only_alerts = |endpoint: &RunEndpoint| endpoint.name == "alerts";
        let refused = check(
            &http,
            Some("http://gw"),
            None,
            &endpoints(),
            &read,
            &only_alerts,
        )
        .await;
        match refused {
            Err(ApiError::Invalid { errors, .. }) => {
                assert_eq!(errors.len(), 1, "{errors:?}");
                assert!(
                    errors.first().is_some_and(|e| e.contains("'stations'")),
                    "{errors:?}"
                );
            }
            other => panic!("a read of an unreadable endpoint passed: {other:?}"),
        }
    }
}
