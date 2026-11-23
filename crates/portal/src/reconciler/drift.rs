//! Drift: what the broker holds and the repository does not declare, or no longer holds as it
//! does (CC-21, CC-38, UI-25, UI-26).
//!
//! Configuration cannot drift — every component reads it from the repository (CC-72) — so the
//! one thing worth comparing is a space's seed entities. The comparison is `jcctl`'s, made
//! against the same space surface every other client reads, as this Portal's own service
//! account: a reader that went straight to the broker would report what no Policy would let a
//! person see.

use crate::config::ClientAuth;
use crate::reconciler::realm;
use jcctl::entities::{action, seed_entities, Action, SeedEntity};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::RwLock;
use std::time::Duration;

/// A space that has not answered in this long is a failed scan, not a slow one.
const TIMEOUT: Duration = Duration::from_secs(15);

/// How a seed entity drifted, which is what decides the two buttons (UI-26).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Kind {
    /// Both sides have it and an attribute the file declares differs.
    #[serde(rename = "MODIFIED")]
    Modified,
    /// The repository declares it and the broker does not hold it.
    #[serde(rename = "MISSING")]
    Missing,
}

/// One drifted seed entity, as the API serves it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Drifted {
    /// The Context Space that holds it.
    pub space: String,
    /// The entity's URN.
    pub id: String,
    /// How it drifted.
    pub drift: Kind,
    /// One entry per declared attribute the broker answers differently.
    pub diff: Vec<Difference>,
    /// Both resolutions, or only `adopt` where reverting would be the one the broker cannot do.
    pub resolutions: Vec<&'static str>,
    /// The file that declares it, so a person can read the change they are resolving.
    pub source: String,
    /// What the file declares, whole. Not served: the list is for reading, and a page that
    /// needed the entity would be showing the file. Revert writes exactly this and nothing
    /// more, so the reverting caller never has to re-stage the repository to find it.
    #[serde(skip)]
    pub declared: Value,
}

/// One attribute the two sides disagree about.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Difference {
    /// The attribute, as a path inside the entity.
    pub path: String,
    /// What the repository declares.
    pub declared: Value,
    /// What the broker answered, absent when it holds none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live: Option<Value>,
}

/// What one scan found in one project.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Found {
    /// When the scan ran.
    pub observed_at: jc_time::TimestampTz,
    /// Every drifted entity of the project, in repository order.
    pub entities: Vec<Drifted>,
}

/// What the last scan found, by project. Read by the API, replaced by the reconciler.
#[derive(Debug, Default)]
pub struct Store {
    found: RwLock<BTreeMap<String, Found>>,
}

impl Store {
    /// What the last scan found in one project, or nothing when none has run.
    pub fn of(&self, project: &str) -> Option<Found> {
        let lock = self.found.read().unwrap_or_else(|p| p.into_inner());
        lock.get(project).cloned()
    }

    /// Replaces everything the previous scan found: a project that no longer drifts has to
    /// stop being reported, and a project the scan could not read keeps nothing stale.
    pub fn replace_all(&self, scanned: BTreeMap<String, Found>) {
        let mut lock = self.found.write().unwrap_or_else(|p| p.into_inner());
        *lock = scanned;
    }
}

/// One realm client and the space surface, for reading and putting back seed entities.
pub struct Watch {
    http: reqwest::Client,
    /// The platform host the spaces are served on, without a trailing slash.
    base: String,
    /// `https://host/realms/{realm}`.
    issuer: String,
    client_id: String,
    auth: ClientAuth,
}

impl Watch {
    /// A watch on one platform host, authenticating as the Portal's own client.
    pub fn new(base: impl Into<String>, issuer: &str, client_id: String, auth: ClientAuth) -> Self {
        Self {
            http: crate::reach::builder()
                .timeout(TIMEOUT)
                .build()
                .unwrap_or_default(),
            base: base.into().trim_end_matches('/').to_owned(),
            issuer: issuer.trim_end_matches('/').to_owned(),
            client_id,
            auth,
        }
    }

    async fn token(&self) -> Result<String, String> {
        realm::token(&self.http, &self.issuer, &self.client_id, &self.auth).await
    }

    /// The client and the host, for a run that writes through an Endpoint (T-4500).
    pub(crate) fn surface(&self) -> (&reqwest::Client, &str) {
        (&self.http, &self.base)
    }

    /// A token for a run that reads many pages: the data-quality run (DM-74).
    pub(crate) async fn scan_token(&self) -> Result<String, String> {
        self.token().await
    }

    /// One page of the entities of `entity_type` in `space`, with their system attributes, as
    /// the space surface answers a client that may read them (DM-74).
    pub async fn page(
        &self,
        token: &str,
        space: &str,
        entity_type: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Value>, String> {
        let response = self
            .http
            .get(format!("{}/cs/{space}/ngsi-ld/v1/entities", self.base))
            .query(&[
                ("type", entity_type),
                ("limit", &limit.to_string()),
                ("offset", &offset.to_string()),
                ("options", "sysAttrs"),
            ])
            .bearer_auth(token)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|err| err.to_string())?;
        if !response.status().is_success() {
            return Err(format!(
                "{space} answered {} for {entity_type}",
                response.status()
            ));
        }
        crate::capped::json(response, crate::capped::ANSWER_BYTES).await
    }

    /// The entities of `entity_type` in the space whose `q` holds, at most 1000 (API/01 §46,
    /// T-4668): the Entities naming one data subject.
    pub(crate) async fn named(
        &self,
        token: &str,
        space: &str,
        entity_type: &str,
        q: &str,
    ) -> Result<Vec<Value>, String> {
        self.fetch(
            token,
            &format!("{}/cs/{space}/ngsi-ld/v1/entities", self.base),
            &[("type", entity_type), ("q", q), ("limit", "1000")],
        )
        .await
        .map(|found| found.as_array().cloned().unwrap_or_default())
    }

    /// The stored versions of one entity's attributes, as the space's temporal surface answers
    /// them: the newest 1,000 per Attribute, the broker's maximum.
    pub(crate) async fn history(
        &self,
        token: &str,
        space: &str,
        id: &str,
    ) -> Result<Value, String> {
        let mut url = url::Url::parse(&format!(
            "{}/cs/{space}/ngsi-ld/v1/temporal/entities",
            self.base
        ))
        .map_err(|err| err.to_string())?;
        url.path_segments_mut()
            .map_err(|()| "the space surface has no path".to_owned())?
            .push(id);
        // the broker's maximum per Attribute: with no lastN it serves its default of 100
        // (T-4804). ponytail: an Attribute past 1,000 versions exports its newest 1,000;
        // following the 206 range is the upgrade.
        self.fetch(token, url.as_str(), &[("lastN", "1000")]).await
    }

    async fn fetch(&self, token: &str, url: &str, query: &[(&str, &str)]) -> Result<Value, String> {
        let response = self
            .http
            .get(url)
            .query(query)
            .bearer_auth(token)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|err| err.without_url().to_string())?;
        // A query that matches nothing answers `[]` and a held Entity has a history; a 404 is
        // the surface refusing the Portal, and an export or erasure must not read it as
        // "nothing about this person" (T-5098, T-5168).
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err("the space surface did not let the Portal read this space".into());
        }
        if !response.status().is_success() {
            return Err(format!("the space answered {}", response.status()));
        }
        crate::capped::json(response, crate::capped::ANSWER_BYTES).await
    }

    /// The entity's address in the space, the id one path segment: an id holding `#` or `?`
    /// would otherwise read another resource and be reported as missing.
    fn entity_url(&self, space: &str, id: &str) -> Result<url::Url, String> {
        let mut url = url::Url::parse(&format!("{}/cs/{space}/ngsi-ld/v1/entities", self.base))
            .map_err(|err| err.to_string())?;
        url.path_segments_mut()
            .map_err(|()| "the space surface has no path".to_owned())?
            .push(id);
        Ok(url)
    }

    /// The entity the space holds under this id, or `None` when it holds none.
    pub async fn live(&self, token: &str, space: &str, id: &str) -> Result<Option<Value>, String> {
        let response = self
            .http
            .get(self.entity_url(space, id)?)
            .bearer_auth(token)
            // Normalized JSON without the `@context` document: what the repository declares.
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|err| err.to_string())?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(format!("{space} answered {}", response.status()));
        }
        crate::capped::json(response, crate::capped::ANSWER_BYTES)
            .await
            .map(Some)
    }

    /// Writes the declared entity back into the space it belongs to (UI-26, revert).
    ///
    /// An upsert, so the attributes the file does not declare are left as they are: reverting
    /// restores what Git owns and deletes nothing (CC-19).
    pub async fn put(&self, space: &str, entity: &Value) -> Result<(), String> {
        let token = self.token().await?;
        let response = self
            .http
            .post(format!(
                "{}/cs/{space}/ngsi-ld/v1/entityOperations/upsert",
                self.base
            ))
            .bearer_auth(&token)
            .json(&json!([entity]))
            .send()
            .await
            .map_err(|err| err.to_string())?;
        match response.status().is_success() {
            true => Ok(()),
            false => Err(format!("{space} refused the write: {}", response.status())),
        }
    }

    /// Compares every seed entity of a staged checkout with the space that holds it.
    ///
    /// One token for the whole scan, and the entities of every project in one map: a scan is
    /// one pass of the reconciler, not one call per page view.
    pub async fn scan(&self, repo_dir: &Path) -> Result<BTreeMap<String, Found>, String> {
        let declared = seed_entities(repo_dir).map_err(|e| e.to_string())?;
        let observed_at = jc_time::TimestampTz::now();
        let mut found: BTreeMap<String, Found> = BTreeMap::new();
        if declared.is_empty() {
            return Ok(found);
        }

        let token = self.token().await?;
        for entity in &declared {
            let live = self.live(&token, &entity.space, &entity.id).await?;
            let Some(drifted) = drifted(entity, live.as_ref()) else {
                continue;
            };
            found
                .entry(entity.project.clone())
                .or_insert_with(|| Found {
                    observed_at,
                    entities: Vec::new(),
                })
                .entities
                .push(drifted);
        }
        // A project whose entities all match still has to answer "nothing drifted, as of now".
        for entity in &declared {
            found
                .entry(entity.project.clone())
                .or_insert_with(|| Found {
                    observed_at,
                    entities: Vec::new(),
                });
        }
        Ok(found)
    }
}

/// One declared entity and what the broker answered, as the API serves it, or `None` when the
/// two agree.
fn drifted(entity: &SeedEntity, live: Option<&Value>) -> Option<Drifted> {
    let source = entity.source.to_string_lossy().to_string();
    // The staged checkout is a temporary directory, and its name is nobody's business: the
    // path a person reads is the one inside the repository.
    let source = match source.find("projects/") {
        Some(at) => source[at..].to_owned(),
        None => source,
    };
    match action(&entity.body, live) {
        Action::Unchanged => None,
        Action::Create => Some(Drifted {
            space: entity.space.clone(),
            id: entity.id.clone(),
            drift: Kind::Missing,
            diff: Vec::new(),
            // Adopting an entity the broker does not hold would propose an empty file.
            resolutions: vec!["revert"],
            source,
            declared: entity.body.clone(),
        }),
        Action::Update => Some(Drifted {
            space: entity.space.clone(),
            id: entity.id.clone(),
            drift: Kind::Modified,
            diff: differences(&entity.body, live),
            resolutions: vec!["revert", "adopt"],
            source,
            declared: entity.body.clone(),
        }),
    }
}

/// Every declared attribute the broker answers differently, as one path each.
///
/// Only what the file declares: a space also holds what its pipelines and its devices write,
/// and telemetry beside a seeded entity is not drift (CC-07, CC-69).
fn differences(declared: &Value, live: Option<&Value>) -> Vec<Difference> {
    let (Some(declared), Some(live)) = (declared.as_object(), live.and_then(Value::as_object))
    else {
        return Vec::new();
    };
    let mut differences = Vec::new();
    for (name, value) in declared {
        if matches!(name.as_str(), "id" | "createdAt" | "modifiedAt") {
            continue;
        }
        let held = live.get(name);
        if held == Some(value) {
            continue;
        }
        // One level in: an NGSI-LD attribute is an object, and "airQualityIndex.value" is what
        // a person reads, not the whole Property beside the whole Property.
        match (value.as_object(), held.and_then(Value::as_object)) {
            (Some(members), Some(held_members)) => {
                for (member, value) in members {
                    let held = held_members.get(member);
                    if held != Some(value) {
                        differences.push(Difference {
                            path: format!("{name}.{member}"),
                            declared: value.clone(),
                            live: held.cloned(),
                        });
                    }
                }
            }
            _ => differences.push(Difference {
                path: name.clone(),
                declared: value.clone(),
                live: held.cloned(),
            }),
        }
    }
    differences
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entity(local: &str, index: u32) -> SeedEntity {
        SeedEntity {
            project: "mesto".to_owned(),
            space: "ovzdusie".to_owned(),
            id: format!("urn:ngsi-ld:AirQualityObserved:bb.sk:ovzdusie:{local}"),
            body: json!({
                "id": format!("urn:ngsi-ld:AirQualityObserved:bb.sk:ovzdusie:{local}"),
                "type": "AirQualityObserved",
                "airQualityIndex": { "type": "Property", "value": index }
            }),
            source: PathBuf::from(
                "/tmp/stage-1234/projects/mesto/spaces/ovzdusie/entities/seed/s.json",
            ),
        }
    }

    #[test]
    fn an_entity_the_broker_holds_as_declared_is_not_reported() {
        let entity = entity("1", 42);
        assert!(drifted(&entity, Some(&entity.body)).is_none());
    }

    #[test]
    fn a_changed_attribute_is_modified_with_both_values_and_both_resolutions() {
        let entity = entity("1", 42);
        let mut live = entity.body.clone();
        live["airQualityIndex"]["value"] = json!(7);
        live["temperature"] = json!({ "type": "Property", "value": 17.4 });

        let drifted = drifted(&entity, Some(&live)).expect("the value moved");
        assert_eq!(drifted.drift, Kind::Modified);
        assert_eq!(drifted.resolutions, vec!["revert", "adopt"]);
        assert_eq!(
            drifted.diff,
            vec![Difference {
                path: "airQualityIndex.value".to_owned(),
                declared: json!(42),
                live: Some(json!(7)),
            }],
            "only the declared attribute, and the telemetry beside it is not drift"
        );
        assert_eq!(
            drifted.source, "projects/mesto/spaces/ovzdusie/entities/seed/s.json",
            "the path inside the repository, not the staging directory"
        );
    }

    #[test]
    fn an_entity_the_broker_lost_is_missing_and_cannot_be_adopted() {
        let drifted = drifted(&entity("1", 42), None).expect("the broker holds none");
        assert_eq!(drifted.drift, Kind::Missing);
        assert_eq!(
            drifted.resolutions,
            vec!["revert"],
            "adopting nothing would propose an empty file"
        );
        assert!(drifted.diff.is_empty());
    }

    #[test]
    fn a_changed_whole_attribute_and_one_the_broker_lacks_are_one_path_each() {
        let mut entity = entity("1", 42);
        entity.body["name"] = json!("Námestie SNP");
        entity.body["address"] = json!({ "type": "Property", "value": "SNP 1" });
        entity.source = PathBuf::from("seed/s.json");
        let mut live = entity.body.clone();
        live["name"] = json!("Namestie");
        live["createdAt"] = json!("2026-01-01T00:00:00Z");
        live.as_object_mut().expect("an object").remove("address");
        live["id"] = json!("urn:ngsi-ld:AirQualityObserved:other");

        let drifted = drifted(&entity, Some(&live)).expect("two attributes moved");
        assert_eq!(
            drifted.diff,
            vec![
                Difference {
                    path: "address".to_owned(),
                    declared: json!({ "type": "Property", "value": "SNP 1" }),
                    live: None,
                },
                Difference {
                    path: "name".to_owned(),
                    declared: json!("Námestie SNP"),
                    live: Some(json!("Namestie")),
                },
            ],
            "a plain value is one path, a missing attribute has no live side, and id is no attribute"
        );
        assert_eq!(
            drifted.source, "seed/s.json",
            "a path outside projects/ is kept"
        );
        assert_eq!(drifted.declared, entity.body);
    }

    #[test]
    fn a_side_that_is_no_object_has_no_attribute_to_compare() {
        assert!(differences(&json!("text"), Some(&json!({ "a": 1 }))).is_empty());
        assert!(differences(&json!({ "a": 1 }), Some(&json!([1]))).is_empty());
        assert!(differences(&json!({ "a": 1 }), None).is_empty());
    }

    #[test]
    fn the_store_answers_per_project_and_forgets_what_a_scan_no_longer_finds() {
        let store = Store::default();
        assert!(store.of("mesto").is_none());

        let drifted = drifted(&entity("1", 42), None).expect("missing");
        store.replace_all(BTreeMap::from([(
            "mesto".to_owned(),
            Found {
                observed_at: jc_time::TimestampTz::now(),
                entities: vec![drifted],
            },
        )]));
        let found = store.of("mesto").expect("scanned");
        assert_eq!(found.entities.len(), 1);
        assert_eq!(found.entities[0].space, "ovzdusie");

        store.replace_all(BTreeMap::from([(
            "mesto".to_owned(),
            Found {
                observed_at: jc_time::TimestampTz::now(),
                entities: Vec::new(),
            },
        )]));
        assert!(store.of("mesto").expect("scanned").entities.is_empty());
        assert!(
            store.of("mesto").is_some(),
            "the project still answers: nothing drifted, as of the last scan"
        );
    }

    /// T-4804: the broker serves 100 values per Attribute when no lastN is named, so a
    /// data-subject export asks its maximum, 1,000.
    #[tokio::test]
    async fn an_entity_history_asks_the_broker_s_maximum_per_attribute() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(
                "/cs/ovzdusie/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:A:1",
            ))
            .and(query_param("lastN", "1000"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"id": "urn:ngsi-ld:A:1"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let watch = Watch::new(
            server.uri(),
            &format!("{}/realms/city", server.uri()),
            "portal".to_owned(),
            ClientAuth::Secret("s".into()),
        );
        let history = watch
            .history("t", "ovzdusie", "urn:ngsi-ld:A:1")
            .await
            .expect("the history");
        assert_eq!(history["id"], "urn:ngsi-ld:A:1");
    }

    /// A watch on `server`, its realm `city` answering the Portal's client with token `t`.
    async fn watch_on(server: &wiremock::MockServer) -> Watch {
        use wiremock::matchers::{body_string_contains, method, path};
        use wiremock::{Mock, ResponseTemplate};
        Mock::given(method("POST"))
            .and(path("/realms/city/protocol/openid-connect/token"))
            .and(body_string_contains("client_id=portal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"access_token": "t"})))
            .mount(server)
            .await;
        Watch::new(
            format!("{}/", server.uri()),
            &format!("{}/realms/city/", server.uri()),
            "portal".to_owned(),
            ClientAuth::Secret("s".into()),
        )
    }

    /// DM-74: a page asks the type, the window and the system attributes with the scan's token.
    #[tokio::test]
    async fn a_page_reads_one_window_of_a_type_and_a_refusal_names_the_space() {
        use wiremock::matchers::{bearer_token, method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let watch = watch_on(&server).await;
        Mock::given(method("GET"))
            .and(path("/cs/ovzdusie/ngsi-ld/v1/entities"))
            .and(query_param("type", "AirQualityObserved"))
            .and(query_param("limit", "50"))
            .and(query_param("offset", "100"))
            .and(query_param("options", "sysAttrs"))
            .and(bearer_token("t"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"id": "urn:a"}])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/cs/closed/ngsi-ld/v1/entities"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        let token = watch.scan_token().await.expect("the realm answers");
        assert_eq!(token, "t");
        let page = watch
            .page(&token, "ovzdusie", "AirQualityObserved", 100, 50)
            .await;
        assert_eq!(page, Ok(vec![json!({"id": "urn:a"})]));
        let refused = watch.page(&token, "closed", "Vehicle", 0, 10).await;
        assert_eq!(
            refused,
            Err("closed answered 403 Forbidden for Vehicle".to_owned())
        );
        let (_, base) = watch.surface();
        assert_eq!(base, server.uri(), "the base keeps no trailing slash");
    }

    /// UI-26: revert upserts the declared entity alone, with a fresh token.
    #[tokio::test]
    async fn a_revert_upserts_the_declared_entity_and_a_refusal_is_an_error() {
        use wiremock::matchers::{bearer_token, body_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let watch = watch_on(&server).await;
        let declared = entity("1", 42).body;
        Mock::given(method("POST"))
            .and(path("/cs/ovzdusie/ngsi-ld/v1/entityOperations/upsert"))
            .and(bearer_token("t"))
            .and(body_json(json!([declared.clone()])))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/cs/closed/ngsi-ld/v1/entityOperations/upsert"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        assert_eq!(watch.put("ovzdusie", &declared).await, Ok(()));
        assert_eq!(
            watch.put("closed", &declared).await,
            Err("closed refused the write: 403 Forbidden".to_owned())
        );
    }

    #[tokio::test]
    async fn a_revert_without_a_token_writes_nothing() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401))
            .expect(1)
            .mount(&server)
            .await;
        let watch = Watch::new(
            server.uri(),
            &format!("{}/realms/city", server.uri()),
            "portal".to_owned(),
            ClientAuth::Secret("s".into()),
        );
        let refused = watch.put("ovzdusie", &json!({})).await;
        assert_eq!(
            refused,
            Err("the realm refused the reconciler's client: 401 Unauthorized".to_owned())
        );
    }

    /// A staged checkout with the projects `mesto` (entities 1 and 2 in `ovzdusie`) and `obec`
    /// (entity 3 in `voda`), under a fresh directory of this test's own.
    fn checkout(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jc-drift-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (project, space, local, index) in [
            ("mesto", "ovzdusie", "1", 42),
            ("mesto", "ovzdusie", "2", 7),
            ("obec", "voda", "3", 1),
        ] {
            let seed = dir.join(format!("projects/{project}/spaces/{space}/entities/seed"));
            std::fs::create_dir_all(&seed).expect("the seed folder");
            let body = entity(local, index).body;
            std::fs::write(seed.join(format!("e{local}.json")), body.to_string()).expect("a seed");
        }
        dir
    }

    /// CC-21: one token for the scan; a lost entity and a moved one are reported per project, and
    /// a project whose entities all match still answers with nothing drifted.
    #[tokio::test]
    async fn a_scan_reports_each_project_and_one_that_matches_answers_empty() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let watch = watch_on(&server).await;
        let at = |space: &str, local: &str| {
            format!("/cs/{space}/ngsi-ld/v1/entities/urn:ngsi-ld:AirQualityObserved:bb.sk:ovzdusie:{local}")
        };
        let mut moved = entity("1", 42).body;
        moved["airQualityIndex"]["value"] = json!(5);
        Mock::given(method("GET"))
            .and(path(at("ovzdusie", "1")))
            .respond_with(ResponseTemplate::new(200).set_body_json(moved))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(at("ovzdusie", "2")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(at("voda", "3")))
            .respond_with(ResponseTemplate::new(200).set_body_json(entity("3", 1).body))
            .mount(&server)
            .await;
        let repo = checkout("scan");
        let found = watch.scan(&repo).await.expect("the scan");
        let tokens = server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|r| r.method.as_str() == "POST")
            .count();
        assert_eq!(tokens, 1, "one token for the whole scan");
        assert_eq!(
            found.keys().map(String::as_str).collect::<Vec<_>>(),
            ["mesto", "obec"]
        );
        let mesto = &found["mesto"].entities;
        assert_eq!(
            mesto.iter().map(|d| d.drift).collect::<Vec<_>>(),
            [Kind::Modified, Kind::Missing]
        );
        assert_eq!(mesto[0].diff[0].path, "airQualityIndex.value");
        assert_eq!(
            mesto[1].source,
            "projects/mesto/spaces/ovzdusie/entities/seed/e2.json"
        );
        assert!(found["obec"].entities.is_empty(), "nothing drifted in obec");
        assert_eq!(found["obec"].observed_at, found["mesto"].observed_at);

        // A space that fails the read fails the scan rather than report a half.
        server.reset().await;
        let watch = watch_on(&server).await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        assert_eq!(
            watch.scan(&repo).await,
            Err("ovzdusie answered 503 Service Unavailable".to_owned())
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[tokio::test]
    async fn a_checkout_without_seeds_asks_nobody_and_an_unreadable_one_is_an_error() {
        use wiremock::MockServer;
        let server = MockServer::start().await;
        let watch = watch_on(&server).await;
        let empty = std::env::temp_dir().join(format!("jc-drift-{}-empty", std::process::id()));
        std::fs::create_dir_all(&empty).expect("a checkout");
        assert_eq!(watch.scan(&empty).await, Ok(BTreeMap::new()));
        assert!(
            server
                .received_requests()
                .await
                .unwrap_or_default()
                .is_empty(),
            "no token, no read"
        );
        let missing = empty.join("gone");
        let unreadable = watch.scan(&missing).await.expect_err("no checkout");
        assert!(unreadable.contains("gone"), "{unreadable}");
        let _ = std::fs::remove_dir_all(&empty);
    }

    /// A lock a panicking reader left poisoned still answers and still takes the next scan.
    #[test]
    fn a_poisoned_store_still_answers_and_takes_writes() {
        let store = std::sync::Arc::new(Store::default());
        let held = std::sync::Arc::clone(&store);
        let _ = std::thread::spawn(move || {
            let _guard = held.found.write();
            panic!("a reader died holding the lock");
        })
        .join();
        assert!(store.found.is_poisoned());
        assert!(store.of("mesto").is_none());
        store.replace_all(BTreeMap::from([("mesto".to_owned(), Found::default())]));
        assert!(store.of("mesto").is_some());
    }

    /// A space surface that does not answer is an error of every call, never an empty answer.
    #[tokio::test]
    async fn a_surface_that_does_not_answer_is_an_error_of_every_call() {
        let closed = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
        let base = format!("http://{}", closed.local_addr().expect("its address"));
        drop(closed);
        let realm = wiremock::MockServer::start().await;
        let answering = watch_on(&realm).await;
        let watch = Watch::new(
            base,
            &format!("{}/realms/city", realm.uri()),
            "portal".to_owned(),
            ClientAuth::Secret("s".into()),
        );
        assert!(watch.page("t", "ovzdusie", "A", 0, 1).await.is_err());
        assert!(watch.named("t", "ovzdusie", "A", "a==1").await.is_err());
        assert!(watch.live("t", "ovzdusie", "urn:a").await.is_err());
        assert!(watch.put("ovzdusie", &json!({})).await.is_err());
        assert_eq!(answering.scan_token().await.as_deref(), Ok("t"));
    }

    /// A realm that refuses the scan's client fails the scan before any space is read.
    #[tokio::test]
    async fn a_scan_without_a_token_reads_no_space() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let watch = Watch::new(
            server.uri(),
            &format!("{}/realms/city", server.uri()),
            "portal".to_owned(),
            ClientAuth::Secret("s".into()),
        );
        let repo = checkout("no-token");
        assert_eq!(
            watch.scan(&repo).await,
            Err("the realm refused the reconciler's client: 401 Unauthorized".to_owned())
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    /// A host that is no hierarchical URL builds no entity address: the read fails, it never
    /// reaches another resource.
    #[tokio::test]
    async fn a_host_that_is_no_url_reads_nothing() {
        for (base, why) in [
            ("mailto:ops", "the space surface has no path"),
            ("", "relative URL without a base"),
        ] {
            let watch = Watch::new(
                base,
                "https://id.example/realms/city",
                "portal".to_owned(),
                ClientAuth::Secret("s".into()),
            );
            assert_eq!(
                watch.live("t", "ovzdusie", "urn:a").await,
                Err(why.to_owned()),
                "{base:?}"
            );
            assert_eq!(
                watch.history("t", "ovzdusie", "urn:a").await,
                Err(why.to_owned()),
                "{base:?}"
            );
        }
    }

    #[tokio::test]
    async fn an_id_holding_a_hash_reads_its_own_entity() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/cs/ovzdusie/ngsi-ld/v1/entities/urn:ngsi-ld:T:a%23b"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"id": "urn:ngsi-ld:T:a#b"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let watch = Watch::new(
            server.uri(),
            &format!("{}/realms/city", server.uri()),
            "portal".to_owned(),
            ClientAuth::Secret("s".into()),
        );
        let live = watch
            .live("t", "ovzdusie", "urn:ngsi-ld:T:a#b")
            .await
            .expect("the read");
        assert_eq!(live.expect("held")["id"], "urn:ngsi-ld:T:a#b");
    }

    /// T-5098: a space surface that refuses the Portal (404) is an error, never "no Entity names
    /// the subject": an erasure must not report done while the person's data stays.
    #[tokio::test]
    async fn a_subject_lookup_the_space_refuses_is_an_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/cs/refused/ngsi-ld/v1/entities"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/cs/failing/ngsi-ld/v1/entities"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(
                "/cs/refused/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Rider:1",
            ))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/cs/empty/ngsi-ld/v1/entities"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&server)
            .await;
        let watch = Watch::new(
            server.uri(),
            &format!("{}/realms/city", server.uri()),
            "portal".to_owned(),
            ClientAuth::Secret("s".into()),
        );
        let refused = watch.named("t", "refused", "Rider", "rider==\"r-1\"").await;
        assert_eq!(
            refused,
            Err("the space surface did not let the Portal read this space".to_owned())
        );
        let empty = watch.named("t", "empty", "Rider", "rider==\"r-1\"").await;
        assert_eq!(empty, Ok(Vec::new()));
        let failing = watch.named("t", "failing", "Rider", "rider==\"r-1\"").await;
        assert_eq!(
            failing,
            Err("the space answered 503 Service Unavailable".to_owned())
        );
        let unread = watch.history("t", "refused", "urn:ngsi-ld:Rider:1").await;
        assert_eq!(
            unread,
            Err("the space surface did not let the Portal read this space".to_owned())
        );
    }

    /// T-5168: the subject's Entity the space holds is found by its subject slot, and its
    /// history comes with every stored version.
    #[tokio::test]
    async fn a_subject_s_entity_is_found_with_every_stored_version() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let id = "urn:ngsi-ld:Rider:r-1";
        Mock::given(method("GET"))
            .and(path("/cs/riders/ngsi-ld/v1/entities"))
            .and(query_param("type", "Rider"))
            .and(query_param("q", "rider==\"r-1\""))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "id": id, "type": "Rider", "rider": { "type": "Property", "value": "r-1" } }
            ])))
            .expect(1)
            .mount(&server)
            .await;
        let versions = json!([
            { "type": "Property", "value": "Jana", "observedAt": "2026-10-22T00:30:00Z" },
            { "type": "Property", "value": "Jana N.", "observedAt": "2026-10-22T00:40:00Z" }
        ]);
        Mock::given(method("GET"))
            .and(path(format!(
                "/cs/riders/ngsi-ld/v1/temporal/entities/{id}"
            )))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "id": id, "type": "Rider", "riderName": versions })),
            )
            .expect(1)
            .mount(&server)
            .await;
        let watch = Watch::new(
            server.uri(),
            &format!("{}/realms/city", server.uri()),
            "portal".to_owned(),
            ClientAuth::Secret("s".into()),
        );
        let found = watch
            .named(
                "t",
                "riders",
                "Rider",
                &crate::erasure::subject_query("rider", "r-1"),
            )
            .await
            .expect("the subject's Entities");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0]["id"], id);
        let history = watch.history("t", "riders", id).await.expect("its history");
        assert_eq!(history["riderName"], versions);
    }

    #[tokio::test]
    async fn the_live_entity_is_none_when_the_space_holds_none_and_an_error_when_it_fails() {
        use wiremock::matchers::{bearer_token, header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let at = |id: &str| format!("/cs/ovzdusie/ngsi-ld/v1/entities/{id}");
        Mock::given(method("GET"))
            .and(path(at("urn:ngsi-ld:A:1")))
            .and(bearer_token("t"))
            .and(header("Accept", "application/json"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"id": "urn:ngsi-ld:A:1"})),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(at("urn:ngsi-ld:A:2")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(at("urn:ngsi-ld:A:3")))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let watch = Watch::new(
            server.uri(),
            &format!("{}/realms/city", server.uri()),
            "portal".to_owned(),
            ClientAuth::Secret("s".into()),
        );
        let held = watch.live("t", "ovzdusie", "urn:ngsi-ld:A:1").await;
        assert_eq!(held, Ok(Some(json!({"id": "urn:ngsi-ld:A:1"}))));
        assert_eq!(
            watch.live("t", "ovzdusie", "urn:ngsi-ld:A:2").await,
            Ok(None)
        );
        let failed = watch.live("t", "ovzdusie", "urn:ngsi-ld:A:3").await;
        assert_eq!(
            failed,
            Err("ovzdusie answered 503 Service Unavailable".to_owned())
        );
    }
}
