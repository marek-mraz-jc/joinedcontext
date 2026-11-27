//! What a `Subscription` manifest declares, written into the space it names (CC-72, DS-16).
//!
//! The manifest is the truth and the broker holds the effect, so this wave is the one place
//! where the repository reaches into live broker state. Three rules keep that from turning into
//! ownership of the broker (Architecture/06):
//!
//! - the id is the manifest's, `urn:ngsi-ld:Subscription:{orgDomain}:{space}:{name}`, so a
//!   second pass updates instead of duplicating;
//! - the credential is resolved at apply time and sent as the `Authorization` header of
//!   `receiverInfo`, so it is in no manifest, no status and no log line (MF-31, PL-17);
//! - only a subscription this reconciler declared in the previous run is ever deleted, so the
//!   ones an application created through the API are left alone even when they are written in
//!   the same URN namespace.
//!
//! Everything goes through the space surface (`/cs/{space}/ngsi-ld/v1/subscriptions`) with the
//! Portal's own service-account token, never straight at the broker: the space's policy decides
//! this write like any other, and the gateway injects the tenant (SP-06, SP-07).

use crate::capped::refusal;
use crate::config::ClientAuth;
use crate::pipeline_secrets::Resolver;
use crate::store::Mirror;
use jc_core::kinds::subscription::SubscriptionSpec;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

/// A broker call that has not answered in this long is a failed sync, not a slow one.
const TIMEOUT: Duration = Duration::from_secs(15);

/// The header a resolved `secretRef` becomes on the notification hop.
const CREDENTIAL_HEADER: &str = "Authorization";

/// What the space says of one subscription's deliveries (TS 104 175 §5.2.14, the
/// NotificationParams members the broker maintains), kept in `status.notifications` so a person
/// sees whether the receiver gets what it watches (T-4866). Never the endpoint, a header or a
/// payload: only an outcome, two counts and three instants.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema,
)]
#[schema(as = SubscriptionDeliveries)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Deliveries {
    /// `ok` when the last notification was delivered, `failed` when it was not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub times_sent: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub times_failed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_notification: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_success: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure: Option<String>,
}

impl Deliveries {
    /// The members of a broker's `notification` object this status carries, each checked: an
    /// outcome other than `ok`/`failed`, a count that is no whole number or an instant that is
    /// no RFC 3339 time is left out rather than shown.
    pub fn from_notification(notification: &Value) -> Self {
        let instant = |key: &str| {
            notification
                .get(key)
                .and_then(Value::as_str)
                .filter(|at| jc_time::parse_rfc3339(at).is_some())
                .map(str::to_owned)
        };
        Self {
            status: notification
                .get("status")
                .and_then(Value::as_str)
                .filter(|status| matches!(*status, "ok" | "failed"))
                .map(str::to_owned),
            times_sent: notification.get("timesSent").and_then(Value::as_u64),
            times_failed: notification.get("timesFailed").and_then(Value::as_u64),
            last_notification: instant("lastNotification"),
            last_success: instant("lastSuccess"),
            last_failure: instant("lastFailure"),
        }
    }
}

/// What happened to one `Subscription` in a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscriptionOutcome {
    /// The space holds what the manifest declares, with what it said of the deliveries when it
    /// answered the read after the write.
    Written(Option<Deliveries>),
    /// Nothing was written and why: a reference that does not resolve, a refusal from the
    /// space, or a gateway that did not answer.
    Error(String),
}

/// One realm token, one HTTP client, and the base of the space surface.
pub struct SubscriptionSync {
    http: reqwest::Client,
    /// The platform host the spaces are served on, without a trailing slash.
    base: String,
    /// `https://host/realms/{realm}`: where the service-account token comes from.
    issuer: String,
    client_id: String,
    auth: ClientAuth,
    /// The organization's domain, the third segment of every URN this instance writes.
    org_domain: String,
}

impl SubscriptionSync {
    /// A sync against one platform host, authenticating as the Portal's own client.
    pub fn new(
        base: impl Into<String>,
        issuer: &str,
        client_id: String,
        auth: ClientAuth,
        org_domain: impl Into<String>,
    ) -> Self {
        Self {
            http: crate::reach::builder()
                .timeout(TIMEOUT)
                .build()
                .unwrap_or_default(),
            base: base.into().trim_end_matches('/').to_owned(),
            issuer: issuer.trim_end_matches('/').to_owned(),
            client_id,
            auth,
            org_domain: org_domain.into(),
        }
    }

    /// The id a manifest's subscription carries in the broker.
    pub(crate) fn urn(&self, space: &str, name: &str) -> String {
        format!(
            "urn:ngsi-ld:Subscription:{}:{space}:{name}",
            self.org_domain
        )
    }

    pub(crate) async fn token(&self) -> Result<String, String> {
        super::realm::token(&self.http, &self.issuer, &self.client_id, &self.auth).await
    }

    /// Brings every space's subscriptions to what the repository declares.
    ///
    /// `repository` is the staged tree of this sync, which is where a SOPS `secretRef` reads.
    /// Returns one outcome per declared manifest, by `(project, name)`.
    pub async fn converge(
        &self,
        mirror: &Mirror,
        previous: &Mirror,
        repository: &Path,
        secrets: Option<&Resolver>,
    ) -> Vec<(String, String, SubscriptionOutcome)> {
        let mut outcomes = Vec::new();
        let token = match self.token().await {
            Ok(token) => token,
            Err(reason) => {
                // Without a token nothing can be written, and saying so once per run beats
                // saying it once per subscription.
                tracing::warn!(%reason, "no token: subscriptions are read, not written");
                for (namespace, name, _, _) in declared(mirror) {
                    outcomes.push((namespace, name, SubscriptionOutcome::Error(reason.clone())));
                }
                return outcomes;
            }
        };

        // What each space should hold when this run is over, so the sweep below knows what is
        // left over rather than what merely failed today.
        let mut declared_ids: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (namespace, name, space, spec) in declared(mirror) {
            let id = self.urn(&space, &name);
            declared_ids
                .entry(space.clone())
                .or_default()
                .insert(id.clone());
            let body = match self
                .body(&id, &spec, (repository, &namespace), secrets)
                .await
            {
                Ok(body) => body,
                Err(reason) => {
                    outcomes.push((namespace, name, SubscriptionOutcome::Error(reason)));
                    continue;
                }
            };
            let outcome = match self.write(&token, &space, &id, &body).await {
                Ok(()) => SubscriptionOutcome::Written(self.deliveries(&token, &space, &id).await),
                Err(reason) => SubscriptionOutcome::Error(reason),
            };
            outcomes.push((namespace, name, outcome));
        }

        // A manifest that is gone takes its subscription with it: what the run before declared
        // and this one does not. Nothing else is deleted — the space's other subscriptions were
        // written by applications through the API, and this reconciler does not own the space.
        // A manifest removed while the Portal was down leaves its subscription behind, which is
        // the failure to have: an orphan a person can delete, not someone else's deleted.
        for (space, name) in declared(previous)
            .into_iter()
            .map(|(_, name, space, _)| (space, name))
        {
            let id = self.urn(&space, &name);
            if declared_ids
                .get(&space)
                .is_some_and(|declared| declared.contains(&id))
            {
                continue;
            }
            match self.delete(&token, &space, &id).await {
                Ok(()) => tracing::info!(%space, %id, "subscription removed with its manifest"),
                Err(reason) => tracing::warn!(%space, %id, %reason, "subscription not removed"),
            }
        }
        outcomes
    }

    /// The NGSI-LD subscription one manifest asks for, with the credential resolved into it.
    async fn body(
        &self,
        id: &str,
        spec: &SubscriptionSpec,
        (repository, project): (&Path, &str),
        secrets: Option<&Resolver>,
    ) -> Result<Value, String> {
        let mut endpoint = Map::new();
        endpoint.insert("uri".to_owned(), json!(spec.notification.endpoint.uri));
        if let Some(accept) = &spec.notification.endpoint.accept {
            endpoint.insert("accept".to_owned(), json!(accept));
        }
        let mut receiver_info: Vec<Value> = spec
            .notification
            .endpoint
            .receiver_info
            .iter()
            .map(|pair| json!({ "key": pair.key, "value": pair.value }))
            .collect();
        if let Some(reference) = &spec.notification.endpoint.secret_ref {
            let resolver = secrets.ok_or_else(|| {
                format!(
                    "the notification credential {} cannot be resolved: no secret backend is \
                     configured",
                    reference.name
                )
            })?;
            let value = resolver
                .one(repository, project, reference)
                .await
                .map_err(|err| err.to_string())?;
            receiver_info.push(json!({ "key": CREDENTIAL_HEADER, "value": value }));
        }
        if !receiver_info.is_empty() {
            endpoint.insert("receiverInfo".to_owned(), Value::Array(receiver_info));
        }

        let mut notification = Map::new();
        notification.insert("endpoint".to_owned(), Value::Object(endpoint));
        if !spec.notification.attributes.is_empty() {
            notification.insert("attributes".to_owned(), json!(spec.notification.attributes));
        }
        if let Some(format) = &spec.notification.format {
            notification.insert("format".to_owned(), json!(format));
        }

        let mut body = Map::new();
        body.insert("id".to_owned(), json!(id));
        body.insert("type".to_owned(), json!("Subscription"));
        body.insert("notification".to_owned(), Value::Object(notification));
        if let Some(title) = &spec.subscription_name {
            body.insert("subscriptionName".to_owned(), json!(title));
        }
        if let Some(description) = &spec.description {
            body.insert("description".to_owned(), json!(description));
        }
        if !spec.entities.is_empty() {
            body.insert("entities".to_owned(), json!(spec.entities));
        }
        if !spec.watched_attributes.is_empty() {
            body.insert(
                "watchedAttributes".to_owned(),
                json!(spec.watched_attributes),
            );
        }
        if let Some(q) = &spec.q {
            body.insert("q".to_owned(), json!(q));
        }
        if let Some(geo_q) = &spec.geo_q {
            body.insert("geoQ".to_owned(), geo_query(geo_q)?);
        }
        if let Some(throttling) = spec.throttling {
            body.insert("throttling".to_owned(), json!(throttling));
        }
        if let Some(expires) = spec.expires_at {
            body.insert(
                "expiresAt".to_owned(),
                json!(expires.to_rfc3339_opts(jc_time::SecondsFormat::Secs, true)),
            );
        }
        body.insert("isActive".to_owned(), json!(spec.is_active));
        Ok(Value::Object(body))
    }

    /// Creates the subscription, or patches the one that is already there.
    pub(crate) async fn write(
        &self,
        token: &str,
        space: &str,
        id: &str,
        body: &Value,
    ) -> Result<(), String> {
        let created = self
            .http
            .post(format!("{}/cs/{space}/ngsi-ld/v1/subscriptions", self.base))
            .bearer_auth(token)
            .json(body)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        if created.status().is_success() {
            return Ok(());
        }
        if created.status() != reqwest::StatusCode::CONFLICT {
            return Err(refusal("the space refused the subscription", created, None).await);
        }
        // Already there: the manifest is still the truth, so it is patched onto it. The id is
        // not sent again — a PATCH that renames the resource is not what this is.
        let mut patch = body.clone();
        if let Some(object) = patch.as_object_mut() {
            object.remove("id");
        }
        let updated = self
            .http
            .patch(format!(
                "{}/cs/{space}/ngsi-ld/v1/subscriptions/{id}",
                self.base
            ))
            .bearer_auth(token)
            .json(&patch)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        match updated.status().is_success() {
            true => Ok(()),
            false => Err(refusal("the space refused the update", updated, None).await),
        }
    }

    /// The deliveries the space reports for `id`, or `None` when it does not answer the read:
    /// the write stood, so a refused read is no error, only nothing to show.
    async fn deliveries(&self, token: &str, space: &str, id: &str) -> Option<Deliveries> {
        let response = self
            .http
            .get(format!(
                "{}/cs/{space}/ngsi-ld/v1/subscriptions/{id}",
                self.base
            ))
            .bearer_auth(token)
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            tracing::debug!(%space, %id, status = %response.status(), "deliveries not read");
            return None;
        }
        let stored: Value = crate::capped::json(response, crate::capped::ANSWER_BYTES)
            .await
            .ok()?;
        stored
            .get("notification")
            .map(Deliveries::from_notification)
    }

    async fn delete(&self, token: &str, space: &str, id: &str) -> Result<(), String> {
        let response = self
            .http
            .delete(format!(
                "{}/cs/{space}/ngsi-ld/v1/subscriptions/{id}",
                self.base
            ))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        match response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND
        {
            true => Ok(()),
            false => Err(refusal("the space refused the delete", response, None).await),
        }
    }
}

/// Every declared subscription of the mirror: `(project, name, space, spec)`.
fn declared(mirror: &Mirror) -> Vec<(String, String, String, SubscriptionSpec)> {
    let mut found = Vec::new();
    for namespace in mirror.namespaces() {
        let page = mirror.list(
            &namespace,
            "Subscription",
            &crate::store::ListOptions::default(),
        );
        for envelope in page.items {
            let name = envelope.metadata.name.clone();
            match serde_json::from_value::<SubscriptionSpec>(envelope.spec.clone()) {
                Ok(spec) => {
                    // The tenant and the id carry the space's rendered segment (PF-84).
                    let space =
                        crate::spaces::segment(mirror, &namespace, spec.context_space_ref.name());
                    found.push((namespace.clone(), name, space, spec));
                }
                Err(err) => tracing::warn!(
                    namespace = %namespace,
                    subscription = %name,
                    error = %err,
                    "a Subscription manifest does not parse and is not written"
                ),
            }
        }
    }
    found
}

/// `georel=near;maxDistance==2000&geometry=Point&coordinates=[13.4,52.5]` as the object CIM 009
/// asks for in a body. A `geoQ` that names none of the three is refused rather than sent half.
fn geo_query(written: &str) -> Result<Value, String> {
    let mut object = Map::new();
    for pair in written.split('&').filter(|p| !p.is_empty()) {
        let Some((key, value)) = pair.split_once('=') else {
            return Err(format!("geoQ '{written}' is not key=value pairs"));
        };
        let value = match value.trim_start().starts_with('[') {
            true => serde_json::from_str(value)
                .map_err(|err| format!("geoQ coordinates '{value}' are not JSON: {err}"))?,
            false => json!(value),
        };
        object.insert(key.trim().to_owned(), value);
    }
    for required in ["georel", "geometry", "coordinates"] {
        if !object.contains_key(required) {
            return Err(format!("geoQ '{written}' names no {required}"));
        }
    }
    Ok(Value::Object(object))
}

/// Each Subscription's status from what its space answered (T-0931, CC-72).
pub(super) fn record(fresh_mirror: &Mirror, outcomes: Vec<(String, String, SubscriptionOutcome)>) {
    for (namespace, name, outcome) in outcomes {
        let Some(mut envelope) = fresh_mirror.get(&namespace, "Subscription", &name) else {
            continue;
        };
        if let Some(status) = envelope.status.as_mut() {
            match &outcome {
                SubscriptionOutcome::Written(notifications) => {
                    status.phase = crate::resource::Phase::Live;
                    status.conditions = Vec::new();
                    status.notifications = notifications.clone();
                }
                SubscriptionOutcome::Error(reason) => {
                    status.phase = crate::resource::Phase::Error;
                    status.conditions = vec![super::streams::make_condition(
                        "SubscriptionWritten",
                        "False",
                        "SpaceRefused",
                        reason,
                    )];
                }
            }
        }
        fresh_mirror.upsert(envelope);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_geo_query_becomes_the_object_the_broker_takes() {
        let query =
            geo_query("georel=near;maxDistance==2000&geometry=Point&coordinates=[13.4,52.5]")
                .expect("a full geoQ");
        assert_eq!(query["georel"], json!("near;maxDistance==2000"));
        assert_eq!(query["geometry"], json!("Point"));
        assert_eq!(query["coordinates"], json!([13.4, 52.5]));
    }

    #[test]
    fn half_a_geo_query_is_refused_rather_than_sent() {
        let reason = geo_query("georel=near&geometry=Point").expect_err("no coordinates");
        assert!(reason.contains("coordinates"), "{reason}");
    }

    #[test]
    fn the_deliveries_keep_only_well_formed_members() {
        let read = Deliveries::from_notification(&json!({
            "endpoint": { "uri": "https://hooks.example/x", "receiverInfo": [{ "key": "Authorization", "value": "Bearer s" }] },
            "status": "failed",
            "timesSent": 4,
            "timesFailed": 1,
            "lastNotification": "2026-10-21T03:20:11.120Z",
            "lastSuccess": "2026-10-21T03:19:40Z",
            "lastFailure": "yesterday",
        }));
        assert_eq!(
            read,
            Deliveries {
                status: Some("failed".into()),
                times_sent: Some(4),
                times_failed: Some(1),
                last_notification: Some("2026-10-21T03:20:11.120Z".into()),
                last_success: Some("2026-10-21T03:19:40Z".into()),
                last_failure: None,
            }
        );
        let odd = Deliveries::from_notification(&json!({ "status": "<b>ok</b>", "timesSent": -1 }));
        assert_eq!(odd, Deliveries::default());
        // Nothing of the endpoint or its headers reaches the status.
        let shown = serde_json::to_string(&read).unwrap_or_default();
        assert!(!shown.contains("hooks.example") && !shown.contains("Bearer"));
    }

    #[test]
    fn a_geo_query_that_is_no_pairs_or_no_json_is_refused() {
        assert_eq!(
            geo_query("georel"),
            Err("geoQ 'georel' is not key=value pairs".to_owned())
        );
        let reason =
            geo_query("georel=near&&geometry=Point&coordinates=[13.4,").expect_err("half an array");
        assert!(
            reason.starts_with("geoQ coordinates '[13.4,' are not JSON"),
            "{reason}"
        );
        assert_eq!(
            geo_query("geometry=Point&coordinates= [1,2]"),
            Err("geoQ 'geometry=Point&coordinates= [1,2]' names no georel".to_owned())
        );
    }

    fn subscription(namespace: &str, name: &str, spec: Value) -> crate::resource::ResourceEnvelope {
        serde_json::from_value(json!({
            "apiVersion": "joinedcontext.com/v1alpha1", "kind": "Subscription",
            "metadata": { "name": name, "namespace": namespace },
            "spec": spec,
            "status": { "phase": "Pending" }
        }))
        .expect("a Subscription")
    }

    fn spec(space: &str, extra: Value) -> Value {
        let mut spec = json!({
            "contextSpaceRef": { "kind": "ContextSpace", "name": space },
            "notification": { "endpoint": { "uri": "https://hooks.example/air" } }
        });
        for (key, value) in extra.as_object().into_iter().flatten() {
            spec[key] = value.clone();
        }
        spec
    }

    fn sync_on(server: &wiremock::MockServer) -> SubscriptionSync {
        SubscriptionSync::new(
            server.uri(),
            &format!("{}/realms/city", server.uri()),
            "portal".to_owned(),
            ClientAuth::Secret("s".into()),
            "hel.fi",
        )
    }

    async fn realm_answers(server: &wiremock::MockServer) {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, ResponseTemplate};
        Mock::given(method("POST"))
            .and(path("/realms/city/protocol/openid-connect/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"access_token": "t"})))
            .mount(server)
            .await;
    }

    /// CC-72: a new manifest is created with every member it declares, one the space already
    /// holds is patched without its id, the deliveries are read back, and a manifest that went
    /// since the last run takes its subscription with it while the space's others stay.
    #[tokio::test]
    async fn a_converge_creates_patches_reads_back_and_removes_only_its_own() {
        use wiremock::matchers::{bearer_token, body_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        realm_answers(&server).await;
        let base = "/cs/mesto-air/ngsi-ld/v1/subscriptions";
        let alerts = "urn:ngsi-ld:Subscription:hel.fi:mesto-air:alerts";
        let quiet = "urn:ngsi-ld:Subscription:hel.fi:mesto-air:quiet";
        Mock::given(method("POST"))
            .and(path(base))
            .and(bearer_token("t"))
            .and(body_json(json!({
                "id": alerts, "type": "Subscription",
                "subscriptionName": "Alerts", "description": "pm10 over 50",
                "entities": [{ "type": "AirQualityObserved" }],
                "watchedAttributes": ["pm10"], "q": "pm10>50",
                "geoQ": { "georel": "near;maxDistance==2000", "geometry": "Point", "coordinates": [13.4, 52.5] },
                "throttling": 60, "expiresAt": "2027-01-01T00:00:00Z", "isActive": true,
                "notification": {
                    "endpoint": { "uri": "https://hooks.example/air", "accept": "application/json",
                                  "receiverInfo": [{ "key": "X-City", "value": "hel" }] },
                    "attributes": ["pm10"], "format": "keyValues"
                }
            })))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{base}/{alerts}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": alerts, "notification": { "status": "ok", "timesSent": 2 }
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/cs/mesto-traffic/ngsi-ld/v1/subscriptions"))
            .respond_with(ResponseTemplate::new(409))
            .mount(&server)
            .await;
        let held = "urn:ngsi-ld:Subscription:hel.fi:mesto-traffic:alerts";
        Mock::given(method("PATCH"))
            .and(path(format!(
                "/cs/mesto-traffic/ngsi-ld/v1/subscriptions/{held}"
            )))
            .and(body_json(json!({
                "type": "Subscription", "isActive": false,
                "notification": { "endpoint": { "uri": "https://hooks.example/air" } }
            })))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!(
                "/cs/mesto-traffic/ngsi-ld/v1/subscriptions/{held}"
            )))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path(format!("{base}/{quiet}")))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path(format!("{base}/{alerts}")))
            .respond_with(ResponseTemplate::new(204))
            .expect(0)
            .mount(&server)
            .await;

        let mirror = Mirror::new();
        mirror.upsert(subscription(
            "mesto",
            "alerts",
            spec(
                "air",
                json!({
                    "subscriptionName": "Alerts", "description": "pm10 over 50",
                    "entities": [{ "type": "AirQualityObserved" }], "watchedAttributes": ["pm10"],
                    "q": "pm10>50",
                    "geoQ": "georel=near;maxDistance==2000&geometry=Point&coordinates=[13.4,52.5]",
                    "throttling": 60, "expiresAt": "2027-01-01T00:00:00Z",
                    "notification": {
                        "endpoint": { "uri": "https://hooks.example/air", "accept": "application/json",
                                      "receiverInfo": [{ "key": "X-City", "value": "hel" }] },
                        "attributes": ["pm10"], "format": "keyValues"
                    }
                }),
            ),
        ));
        let previous = Mirror::new();
        previous.upsert(subscription("mesto", "alerts", spec("air", json!({}))));
        previous.upsert(subscription("mesto", "quiet", spec("air", json!({}))));
        mirror.upsert(subscription(
            "mesto",
            "broken",
            json!({ "contextSpaceRef": "air" }),
        ));

        let sync = sync_on(&server);
        let outcomes = sync
            .converge(&mirror, &previous, Path::new("/nonexistent"), None)
            .await;
        assert_eq!(
            outcomes,
            [(
                "mesto".to_owned(),
                "alerts".to_owned(),
                SubscriptionOutcome::Written(Some(Deliveries {
                    status: Some("ok".into()),
                    times_sent: Some(2),
                    ..Deliveries::default()
                }))
            )],
            "a manifest that does not parse is not written"
        );

        // The held one in another space is patched; its deliveries unread are nothing to show.
        let other = Mirror::new();
        other.upsert(subscription(
            "mesto",
            "alerts",
            spec("traffic", json!({ "isActive": false })),
        ));
        let outcomes = sync
            .converge(&other, &Mirror::new(), Path::new("/nonexistent"), None)
            .await;
        assert_eq!(
            outcomes,
            [(
                "mesto".to_owned(),
                "alerts".to_owned(),
                SubscriptionOutcome::Written(None)
            )]
        );
    }

    /// MF-31: a credential with no backend, a refusal and a failed patch are errors of their
    /// own manifest; a delete the space refuses leaves the run going.
    #[tokio::test]
    async fn each_failure_is_its_own_manifests_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        realm_answers(&server).await;
        Mock::given(method("POST"))
            .and(path("/cs/mesto-closed/ngsi-ld/v1/subscriptions"))
            .respond_with(ResponseTemplate::new(403).set_body_json(json!({"title": "no"})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/cs/mesto-held/ngsi-ld/v1/subscriptions"))
            .respond_with(ResponseTemplate::new(409))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({"title": "bad"})))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;
        let mirror = Mirror::new();
        let secret = json!({ "notification": { "endpoint": {
            "uri": "https://hooks.example/air", "secretRef": { "name": "hook-token" } } } });
        mirror.upsert(subscription("mesto", "secret", spec("air", secret.clone())));
        mirror.upsert(subscription("mesto", "sealed", spec("air", secret)));
        mirror.upsert(subscription("mesto", "closed", spec("closed", json!({}))));
        mirror.upsert(subscription("mesto", "held", spec("held", json!({}))));
        mirror.upsert(subscription(
            "mesto",
            "fenced",
            spec("air", json!({ "geoQ": "georel=near" })),
        ));
        let previous = Mirror::new();
        previous.upsert(subscription("mesto", "gone", spec("air", json!({}))));

        let sync = sync_on(&server);
        let resolver = Resolver::new(crate::pipeline_secrets::Backend::Sops {
            age_key_file: std::path::PathBuf::from("/nonexistent"),
        });
        let outcomes = sync
            .converge(&mirror, &previous, Path::new("/nonexistent"), None)
            .await;
        let error = |name: &str| {
            let outcome = outcomes
                .iter()
                .find(|(_, n, _)| n == name)
                .map(|(_, _, outcome)| outcome.clone())
                .expect("each manifest has an outcome");
            use SubscriptionOutcome::Error as E;
            let E(why) = outcome else { panic!() };
            why
        };
        assert_eq!(
            error("secret"),
            "the notification credential hook-token cannot be resolved: no secret backend is configured"
        );
        assert!(
            error("closed").starts_with("the space refused the subscription"),
            "{}",
            error("closed")
        );
        assert!(
            error("held").starts_with("the space refused the update"),
            "{}",
            error("held")
        );
        assert_eq!(error("fenced"), "geoQ 'georel=near' names no geometry");
        assert_eq!(outcomes.len(), 5);

        // A backend that cannot read the reference refuses it, and the value is in no outcome.
        let only = Mirror::new();
        only.upsert(subscription(
            "mesto",
            "sealed",
            spec(
                "air",
                json!({ "notification": { "endpoint": {
            "uri": "https://hooks.example/air", "secretRef": { "name": "hook-token" } } } }),
            ),
        ));
        let sealed = sync
            .converge(
                &only,
                &Mirror::new(),
                Path::new("/nonexistent"),
                Some(&resolver),
            )
            .await;
        assert!(
            matches!(&sealed[0].2, SubscriptionOutcome::Error(reason) if reason.contains("hook-token")),
            "{sealed:?}"
        );
    }

    /// A subscription the space no longer holds is removed already; any other refusal is said.
    #[tokio::test]
    async fn a_delete_of_what_the_space_lacks_is_done_and_a_refusal_is_not() {
        use wiremock::matchers::{bearer_token, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let at = |name: &str| {
            format!("/cs/mesto-air/ngsi-ld/v1/subscriptions/urn:ngsi-ld:Subscription:hel.fi:mesto-air:{name}")
        };
        for (name, status) in [("held", 204), ("lost", 404), ("kept", 409)] {
            Mock::given(method("DELETE"))
                .and(path(at(name)))
                .and(bearer_token("t"))
                .respond_with(ResponseTemplate::new(status))
                .mount(&server)
                .await;
        }
        let sync = sync_on(&server);
        let id = |name: &str| sync.urn("mesto-air", name);
        assert_eq!(sync.delete("t", "mesto-air", &id("held")).await, Ok(()));
        assert_eq!(sync.delete("t", "mesto-air", &id("lost")).await, Ok(()));
        let refused = sync
            .delete("t", "mesto-air", &id("kept"))
            .await
            .expect_err("a conflict");
        assert!(
            refused.starts_with("the space refused the delete"),
            "{refused}"
        );
    }

    /// A one-shot HTTP peer: each connection gets the next scripted answer, or is dropped
    /// unanswered when the script says `None`. Answers the address to call.
    fn scripted(answers: Vec<Option<&'static str>>) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
        let base = format!("http://{}", listener.local_addr().expect("its address"));
        std::thread::spawn(move || {
            for answer in answers {
                let (mut stream, _) = listener.accept().expect("a connection");
                if let Some(answer) = answer {
                    let mut request = [0u8; 8192];
                    let _ = stream.read(&mut request);
                    let _ = stream.write_all(answer.as_bytes());
                }
            }
        });
        base
    }

    const CONFLICT: &str =
        "HTTP/1.1 409 Conflict\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
    const CREATED: &str = "HTTP/1.1 201 Created\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";

    /// A space that stops answering mid-write is an error of that write; one that stops before
    /// the read-back leaves the write standing with nothing to show.
    #[tokio::test]
    async fn a_space_that_stops_answering_fails_the_write_and_not_the_read_back() {
        let sync = |base: String| {
            SubscriptionSync::new(
                base,
                "https://id.example/realms/city",
                "portal".to_owned(),
                ClientAuth::Secret("s".into()),
                "hel.fi",
            )
        };
        let body = json!({ "id": "urn:x", "type": "Subscription" });
        let silent = sync(scripted(vec![None]));
        assert!(silent.write("t", "air", "urn:x", &body).await.is_err());
        let patch_lost = sync(scripted(vec![Some(CONFLICT), None]));
        assert!(patch_lost.write("t", "air", "urn:x", &body).await.is_err());
        let gone = sync(scripted(vec![None]));
        assert!(gone.delete("t", "air", "urn:x").await.is_err());
        let read_lost = sync(scripted(vec![Some(CREATED), None]));
        assert_eq!(read_lost.write("t", "air", "urn:x", &body).await, Ok(()));
        assert_eq!(read_lost.deliveries("t", "air", "urn:x").await, None);
    }

    /// A read-back that is no JSON object with a notification shows nothing.
    #[tokio::test]
    async fn a_read_back_that_does_not_read_shows_nothing() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/cs/air/ngsi-ld/v1/subscriptions/urn:a"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/cs/air/ngsi-ld/v1/subscriptions/urn:b"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "urn:b" })))
            .mount(&server)
            .await;
        let sync = sync_on(&server);
        assert_eq!(sync.deliveries("t", "air", "urn:a").await, None);
        assert_eq!(sync.deliveries("t", "air", "urn:b").await, None);
    }

    /// Without a token nothing is written: every manifest says why, once, and nothing is deleted.
    #[tokio::test]
    async fn a_refused_token_writes_nothing_and_every_manifest_says_so() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401))
            .expect(1)
            .mount(&server)
            .await;
        let mirror = Mirror::new();
        mirror.upsert(subscription("mesto", "a", spec("air", json!({}))));
        mirror.upsert(subscription("mesto", "b", spec("air", json!({}))));
        let previous = Mirror::new();
        previous.upsert(subscription("mesto", "gone", spec("air", json!({}))));
        let outcomes = sync_on(&server)
            .converge(&mirror, &previous, Path::new("/nonexistent"), None)
            .await;
        let reason = "the realm refused the reconciler's client: 401 Unauthorized".to_owned();
        assert_eq!(
            outcomes,
            [
                (
                    "mesto".to_owned(),
                    "a".to_owned(),
                    SubscriptionOutcome::Error(reason.clone())
                ),
                (
                    "mesto".to_owned(),
                    "b".to_owned(),
                    SubscriptionOutcome::Error(reason)
                ),
            ]
        );
    }

    /// T-0931: a written subscription is Live with its deliveries and no condition; a refused one
    /// is Error with the reason; one that left the mirror meanwhile is not brought back.
    #[test]
    fn the_outcomes_become_each_manifests_status() {
        let mirror = Mirror::new();
        mirror.upsert(subscription("mesto", "ok", spec("air", json!({}))));
        mirror.upsert(subscription("mesto", "refused", spec("air", json!({}))));
        let shown = Deliveries {
            status: Some("ok".into()),
            ..Deliveries::default()
        };
        record(
            &mirror,
            vec![
                (
                    "mesto".into(),
                    "ok".into(),
                    SubscriptionOutcome::Written(Some(shown.clone())),
                ),
                (
                    "mesto".into(),
                    "refused".into(),
                    SubscriptionOutcome::Error("403 from the space".into()),
                ),
                (
                    "mesto".into(),
                    "gone".into(),
                    SubscriptionOutcome::Error("x".into()),
                ),
            ],
        );
        let status = |name: &str| {
            mirror
                .get("mesto", "Subscription", name)
                .and_then(|env| env.status)
                .expect("a status")
        };
        let ok = status("ok");
        assert_eq!(ok.phase, crate::resource::Phase::Live);
        assert!(ok.conditions.is_empty());
        assert_eq!(ok.notifications, Some(shown));
        let refused = status("refused");
        assert_eq!(refused.phase, crate::resource::Phase::Error);
        assert_eq!(
            refused.conditions[0].reason.as_deref(),
            Some("SpaceRefused")
        );
        assert_eq!(
            refused.conditions[0].message.as_deref(),
            Some("403 from the space")
        );
        assert!(mirror.get("mesto", "Subscription", "gone").is_none());

        // A manifest without a status yet is kept as it is.
        let mut bare = subscription("mesto", "bare", spec("air", json!({})));
        bare.status = None;
        mirror.upsert(bare);
        record(
            &mirror,
            vec![(
                "mesto".into(),
                "bare".into(),
                SubscriptionOutcome::Error("x".into()),
            )],
        );
        assert!(mirror
            .get("mesto", "Subscription", "bare")
            .is_some_and(|env| env.status.is_none()));
    }

    #[test]
    fn the_id_is_the_manifests_and_names_the_space() {
        let sync = SubscriptionSync::new(
            "https://city.example/",
            "https://idm.example/realms/dev",
            "portal-reconciler".to_owned(),
            ClientAuth::Secret("s3cr3t".into()),
            "hel.fi",
        );
        assert_eq!(
            sync.urn("air-quality", "alerts"),
            "urn:ngsi-ld:Subscription:hel.fi:air-quality:alerts"
        );
        // The same manifest in another space is another subscription; the space is in the id.
        assert_ne!(
            sync.urn("traffic", "alerts"),
            sync.urn("air-quality", "alerts")
        );
        // The base keeps no trailing slash, so no path is built with a double one.
        assert_eq!(sync.base, "https://city.example");
    }
}
