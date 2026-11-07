//! A person's visualization in the project's Grafana (T-6509).
//!
//! Each project has one Grafana organization (the deployment's `provision.sh`), and the Portal
//! writes into it with the local administrator, which never reaches a browser. A panel reads as
//! whoever opens it: a closed Endpoint through a datasource that forwards that person's token, a
//! public one through an anonymous datasource. Grafana's database lives and dies with its pod, so
//! the rows here are the record, and a periodic sweep puts back what a restart took.

use std::sync::LazyLock;
use std::time::Duration;

use argon2::password_hash::rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::config::GrafanaConfig;
use crate::error::ApiError;
use crate::state::AppState;

/// The datasource type of the NGSI-LD plugin in the deployment's Grafana image.
const PLUGIN: &str = "ngsild-grafana-datasource";

/// How often the rows are compared with what Grafana holds.
const SWEEP: Duration = Duration::from_secs(300);

/// A datasource uid Grafana accepts at most.
const MAX_UID: usize = 40;

/// The longest Grafana answer read: an organization or one project's datasource list.
const MAX_ANSWER: usize = 1 << 20;

/// Outside calls stop here; redirects are refused so the credentials go nowhere else.
static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_default()
});

/// What a visualization draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Timeseries,
    Stat,
    Table,
    Map,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Timeseries, Kind::Stat, Kind::Table, Kind::Map];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Timeseries => "timeseries",
            Kind::Stat => "stat",
            Kind::Table => "table",
            Kind::Map => "map",
        }
    }

    fn parse(text: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.as_str() == text)
    }

    /// Whether this kind can draw `attribute`: a number for a chart or a tile, a place for a map,
    /// any value but a link for a table. A model that leaves the value open is taken at its word.
    pub fn draws(self, attribute_kind: &str, value_type: Option<&str>) -> bool {
        let numeric = matches!(value_type, None | Some("number" | "integer"));
        match self {
            Kind::Timeseries | Kind::Stat => attribute_kind == "Property" && numeric,
            Kind::Table => attribute_kind != "Relationship",
            Kind::Map => attribute_kind == "GeoProperty",
        }
    }
}

/// A new row's id: 128 random bits, in the hex a Grafana uid takes.
pub fn new_id() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// One saved visualization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Visualization {
    pub id: String,
    pub project: String,
    pub title: String,
    pub endpoint: String,
    pub entity_type: String,
    pub attribute: String,
    pub kind: Kind,
    pub created_at: String,
}

impl Visualization {
    /// The dashboard's uid in Grafana, which the row's id settles once.
    pub fn uid(&self) -> String {
        format!("pv-{}", self.id)
    }
}

type Row = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
);

fn of_row(row: Row) -> Option<Visualization> {
    let (id, project, title, endpoint, entity_type, attribute, kind, created_at) = row;
    Some(Visualization {
        id,
        project,
        title,
        endpoint,
        entity_type,
        attribute,
        kind: Kind::parse(&kind)?,
        created_at,
    })
}

macro_rules! columns {
    () => {
        "SELECT id, project, title, endpoint, entity_type, attribute, kind, \
         to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') \
         FROM grafana_visualizations "
    };
}

/// The project's visualizations, newest first; every project's for `None`.
pub async fn list(pool: &PgPool, project: Option<&str>) -> Result<Vec<Visualization>, sqlx::Error> {
    let rows: Vec<Row> = sqlx::query_as(concat!(
        columns!(),
        "WHERE $1::text IS NULL OR project = $1 ORDER BY created_at DESC"
    ))
    .bind(project)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().filter_map(of_row).collect())
}

/// One visualization of the project.
pub async fn find(
    pool: &PgPool,
    project: &str,
    id: &str,
) -> Result<Option<Visualization>, sqlx::Error> {
    let row: Option<Row> = sqlx::query_as(concat!(columns!(), "WHERE project = $1 AND id = $2"))
        .bind(project)
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(of_row))
}

pub async fn insert(pool: &PgPool, row: &Visualization) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO grafana_visualizations \
         (id, project, title, endpoint, entity_type, attribute, kind) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(&row.id)
    .bind(&row.project)
    .bind(&row.title)
    .bind(&row.endpoint)
    .bind(&row.entity_type)
    .bind(&row.attribute)
    .bind(row.kind.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete(pool: &PgPool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM grafana_visualizations WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// An Endpoint as the page may draw from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source<'a> {
    pub name: &'a str,
    pub slug: &'a str,
    pub public: bool,
}

/// One datasource of an organization: the Endpoint it reads and its uid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Datasource {
    pub slug: String,
    pub uid: String,
}

/// The slug a datasource reads, from its address; `None` for one that reads no Endpoint.
fn slug_of(url: &str) -> Option<&str> {
    url.trim_end_matches('/')
        .rsplit_once("/api/endpoint/")
        .map(|(_, slug)| slug)
        .filter(|slug| !slug.is_empty() && !slug.contains('/'))
}

/// A uid for the datasource of Endpoint `name`: the name itself, as the provisioned ones, cut to
/// what Grafana takes with the slug keeping two long names apart.
fn datasource_uid(name: &str, slug: &str) -> String {
    if name.len() <= MAX_UID {
        return name.to_owned();
    }
    let tail = &slug[slug.len().saturating_sub(8)..];
    let head = &name[..MAX_UID - tail.len() - 1];
    format!("{head}-{tail}")
}

/// Grafana's admin API, as one administrator.
pub struct Client<'a> {
    config: &'a GrafanaConfig,
}

fn unreachable(what: &str, err: impl std::fmt::Display) -> ApiError {
    tracing::warn!(error = %err, what, "Grafana did not answer");
    ApiError::Unavailable(format!(
        "Grafana did not answer while {what}; try again in a minute"
    ))
}

/// The JSON of a Grafana answer, read up to [`MAX_ANSWER`] bytes.
async fn json_of<T: serde::de::DeserializeOwned>(
    answer: reqwest::Response,
    what: &str,
) -> Result<T, ApiError> {
    let body = jc_core::outbound::read_capped(answer, MAX_ANSWER)
        .await
        .map_err(|err| unreachable(what, err))?;
    serde_json::from_slice(&body).map_err(|err| unreachable(what, err))
}

impl<'a> Client<'a> {
    pub fn new(config: &'a GrafanaConfig) -> Self {
        Self { config }
    }

    fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        org: Option<i64>,
    ) -> reqwest::RequestBuilder {
        let request = HTTP
            .request(method, format!("{}/api{path}", self.config.url))
            .basic_auth(&self.config.user, Some(&self.config.password));
        match org {
            Some(org) => request.header("X-Grafana-Org-Id", org),
            None => request,
        }
    }

    /// The organization of `project`; `None` when the deployment has made none.
    pub async fn organization(&self, project: &str) -> Result<Option<i64>, ApiError> {
        let what = "finding the project's organization";
        let answer = self
            .request(reqwest::Method::GET, &format!("/orgs/name/{project}"), None)
            .send()
            .await
            .map_err(|err| unreachable(what, err))?;
        if answer.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let answer = answer
            .error_for_status()
            .map_err(|err| unreachable(what, err))?;
        let org: Value = json_of(answer, what).await?;
        org["id"]
            .as_i64()
            .map(Some)
            .ok_or_else(|| unreachable(what, "an organization without an id"))
    }

    /// The Endpoints the organization has a datasource for.
    pub async fn datasources(&self, org: i64) -> Result<Vec<Datasource>, ApiError> {
        let what = "listing the project's datasources";
        let answer = self
            .request(reqwest::Method::GET, "/datasources", Some(org))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|err| unreachable(what, err))?;
        let listed: Vec<Value> = json_of(answer, what).await?;
        Ok(listed
            .iter()
            .filter(|source| source["type"] == PLUGIN)
            .filter_map(|source| {
                Some(Datasource {
                    slug: slug_of(source["url"].as_str()?)?.to_owned(),
                    uid: source["uid"].as_str()?.to_owned(),
                })
            })
            .collect())
    }

    /// The anonymous datasource of a public Endpoint. Only such a one is made here: a closed
    /// Endpoint needs its slug in the realm client's audience, which the deployment holds.
    async fn create_public_datasource(
        &self,
        org: i64,
        source: &Source<'_>,
        gateway: &str,
    ) -> Result<String, ApiError> {
        let uid = datasource_uid(source.name, source.slug);
        self.request(reqwest::Method::POST, "/datasources", Some(org))
            .json(&json!({
                "uid": uid,
                "name": uid,
                "type": PLUGIN,
                "access": "proxy",
                "url": format!("{gateway}/api/endpoint/{}", source.slug),
                "jsonData": { "pageSize": 1000 },
            }))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|err| unreachable("adding the Endpoint's datasource", err))?;
        Ok(uid)
    }

    /// Saves `visualization`'s dashboard into the project's folder, replacing an older copy.
    pub async fn save(&self, org: i64, dashboard: &Value, project: &str) -> Result<(), ApiError> {
        self.request(reqwest::Method::POST, "/dashboards/db", Some(org))
            .json(&json!({
                "dashboard": dashboard,
                "folderUid": format!("project-{project}"),
                "overwrite": true,
                "message": "saved from the Portal",
            }))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|err| unreachable("saving the dashboard", err))?;
        Ok(())
    }

    /// Whether the dashboard `uid` is in the organization.
    async fn holds(&self, org: i64, uid: &str) -> Result<bool, ApiError> {
        let answer = self
            .request(
                reqwest::Method::GET,
                &format!("/dashboards/uid/{uid}"),
                Some(org),
            )
            .send()
            .await
            .map_err(|err| unreachable("reading a dashboard", err))?;
        match answer.status() {
            reqwest::StatusCode::NOT_FOUND => Ok(false),
            status if status.is_success() => Ok(true),
            status => Err(unreachable("reading a dashboard", status)),
        }
    }

    /// Takes the dashboard out; one already gone is fine.
    pub async fn remove(&self, org: i64, uid: &str) -> Result<(), ApiError> {
        let answer = self
            .request(
                reqwest::Method::DELETE,
                &format!("/dashboards/uid/{uid}"),
                Some(org),
            )
            .send()
            .await
            .map_err(|err| unreachable("removing the dashboard", err))?;
        if answer.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        answer
            .error_for_status()
            .map_err(|err| unreachable("removing the dashboard", err))?;
        Ok(())
    }

    /// The datasource a panel reads `source` through: the one the organization holds, or a new
    /// anonymous one for a public Endpoint; `None` for a closed Endpoint with none.
    pub async fn datasource_for(
        &self,
        org: i64,
        source: &Source<'_>,
        gateway: Option<&str>,
    ) -> Result<Option<String>, ApiError> {
        let held = self.datasources(org).await?;
        if let Some(found) = held.into_iter().find(|held| held.slug == source.slug) {
            return Ok(Some(found.uid));
        }
        match gateway {
            Some(gateway) if source.public => self
                .create_public_datasource(org, source, gateway)
                .await
                .map(Some),
            _ => Ok(None),
        }
    }
}

/// The dashboard of one visualization: one panel, in the words of the plugin's own queries.
/// `named` says the class has a `name` to label rows with.
pub fn dashboard_of(row: &Visualization, datasource: &str, named: bool) -> Value {
    let reference = json!({ "type": PLUGIN, "uid": datasource });
    let mut target = json!({
        "refId": "A",
        "datasource": reference,
        "entityType": row.entity_type,
        "limit": 100,
    });
    let label = |target: &mut Value| {
        if named {
            target["entityName"] = json!("name");
            target["namePattern"] = json!("entity_name");
        }
    };
    let mut options = json!({});
    let panel = match row.kind {
        Kind::Timeseries => {
            target["queryType"] = json!("temporal");
            target["attributes"] = json!([row.attribute]);
            label(&mut target);
            "timeseries"
        }
        Kind::Stat => {
            target["queryType"] = json!("entity");
            target["format"] = json!("timeseries");
            target["attributes"] = json!([row.attribute]);
            label(&mut target);
            options = json!({
                "reduceOptions": { "calcs": ["lastNotNull"], "fields": "", "values": false },
                "textMode": "value_and_name",
            });
            "stat"
        }
        Kind::Table => {
            target["queryType"] = json!("entity");
            target["format"] = json!("table");
            let columns: Vec<&str> = named
                .then_some("name")
                .into_iter()
                .chain([row.attribute.as_str()])
                .collect();
            target["attributes"] = json!(columns);
            "table"
        }
        Kind::Map => {
            target["queryType"] = json!("geo");
            target["geoproperty"] = json!(row.attribute);
            label(&mut target);
            options = json!({
                "view": { "id": "fit", "lat": 0, "lon": 0, "zoom": 10 },
                "basemap": { "type": "default" },
                "layers": [{
                    "type": "markers",
                    "name": row.title,
                    "location": { "mode": "auto" },
                    "tooltip": true,
                }],
            });
            "geomap"
        }
    };
    json!({
        "uid": row.uid(),
        "title": row.title,
        "description": "Saved from the Portal. It reads through the Endpoint with your own rights.",
        "tags": ["project", row.project, "portal"],
        "schemaVersion": 39,
        "editable": false,
        "time": { "from": "now-7d", "to": "now" },
        "refresh": "1h",
        "timezone": "browser",
        "panels": [{
            "id": 1,
            "title": row.title,
            "type": panel,
            "datasource": reference,
            "gridPos": { "x": 0, "y": 0, "w": 24, "h": 18 },
            "targets": [target],
            "options": options,
        }],
    })
}

/// Whether the class declares a `name`, in the model the Endpoint's space pins.
fn named(state: &AppState, project: &str, space: &str, class: &str) -> bool {
    state
        .model_schemas
        .get(project, space)
        .and_then(|model| model.attributes(class))
        .is_some_and(|class| class.attributes.iter().any(|a| a.name == "name"))
}

/// Saves `row` into Grafana as the mirror knows its Endpoint now.
pub async fn push(
    state: &AppState,
    client: &Client<'_>,
    org: i64,
    row: &Visualization,
    source: &Source<'_>,
    space: &str,
) -> Result<bool, ApiError> {
    let gateway = state.config.gateway_url.as_deref();
    let Some(datasource) = client.datasource_for(org, source, gateway).await? else {
        return Ok(false);
    };
    let label = named(state, &row.project, space, &row.entity_type);
    client
        .save(org, &dashboard_of(row, &datasource, label), &row.project)
        .await?;
    Ok(true)
}

/// The space an Endpoint lives in; a manifest names it by a bare or a typed reference.
pub(crate) fn space_of(endpoint: &crate::resource::ResourceEnvelope) -> &str {
    crate::resource::reference_name(&endpoint.spec["contextSpaceRef"]).unwrap_or_default()
}

/// Puts one visualization back when Grafana lost it. An Endpoint since removed stays out.
async fn restore(
    state: &AppState,
    client: &Client<'_>,
    row: &Visualization,
) -> Result<(), ApiError> {
    let Some(endpoint) = state.mirror.get(&row.project, "Endpoint", &row.endpoint) else {
        return Ok(());
    };
    let Some(org) = client.organization(&row.project).await? else {
        return Ok(());
    };
    if client.holds(org, &row.uid()).await? {
        return Ok(());
    }
    let source = Source {
        name: &row.endpoint,
        slug: endpoint.spec["slug"].as_str().unwrap_or_default(),
        public: endpoint.spec["audience"] == "public",
    };
    if push(state, client, org, row, &source, space_of(&endpoint)).await? {
        tracing::info!(uid = %row.uid(), project = %row.project, "a visualization was put back in Grafana");
    }
    Ok(())
}

/// One pass over every row. A Grafana that is away fails each row alike, so the pass stops there
/// and the next one tries again.
pub async fn sweep(state: &AppState, pool: &PgPool, client: &Client<'_>) {
    let rows = match list(pool, None).await {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!(error = %err, "the visualizations could not be read");
            return;
        }
    };
    for row in &rows {
        if restore(state, client, row).await.is_err() {
            return;
        }
    }
}

/// What a restart of Grafana took, put back on the leader every [`SWEEP`].
pub(crate) fn spawn_periodic(state: AppState) {
    let (Some(config), Some(pool)) = (state.config.grafana.clone(), state.db.clone()) else {
        return;
    };
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(SWEEP);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if state.leads() {
                sweep(&state, &pool, &Client::new(&config)).await;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    use super::*;

    fn row(kind: Kind) -> Visualization {
        Visualization {
            id: "ab12".into(),
            project: "helsinki".into(),
            title: "Air".into(),
            endpoint: "air".into(),
            entity_type: "AirQualityObserved".into(),
            attribute: "pm25".into(),
            kind,
            created_at: "2026-11-02T00:00:00Z".into(),
        }
    }

    #[test]
    fn each_kind_draws_only_what_it_can_show() {
        let number = ("Property", Some("number"));
        let text = ("Property", Some("string"));
        let open = ("Property", None);
        let place = ("GeoProperty", None);
        let link = ("Relationship", None);
        let draws =
            |kind: Kind, (attribute, value): (&str, Option<&str>)| kind.draws(attribute, value);
        for kind in [Kind::Timeseries, Kind::Stat] {
            assert!(draws(kind, number) && draws(kind, open));
            assert!(!draws(kind, text) && !draws(kind, place) && !draws(kind, link));
        }
        assert!(draws(Kind::Table, text) && draws(Kind::Table, place) && !draws(Kind::Table, link));
        assert!(draws(Kind::Map, place) && !draws(Kind::Map, number));
    }

    #[test]
    fn a_dashboard_holds_the_query_of_its_kind_and_no_credential() {
        let series = dashboard_of(&row(Kind::Timeseries), "air", true);
        let target = &series["panels"][0]["targets"][0];
        assert_eq!(series["panels"][0]["type"], "timeseries");
        assert_eq!(target["queryType"], "temporal");
        assert_eq!(target["attributes"], json!(["pm25"]));
        assert_eq!(target["entityName"], "name");

        let map = dashboard_of(&row(Kind::Map), "air", false);
        assert_eq!(map["panels"][0]["type"], "geomap");
        assert_eq!(map["panels"][0]["targets"][0]["geoproperty"], "pm25");
        assert!(map["panels"][0]["targets"][0].get("entityName").is_none());

        let table = dashboard_of(&row(Kind::Table), "air", true);
        assert_eq!(
            table["panels"][0]["targets"][0]["attributes"],
            json!(["name", "pm25"])
        );
        assert_eq!(
            dashboard_of(&row(Kind::Stat), "air", true)["panels"][0]["type"],
            "stat"
        );

        assert_eq!(series["editable"], false);
        assert_eq!(series["uid"], "pv-ab12");
        let text = series.to_string();
        assert!(
            !text.contains("oken") && !text.contains("assword"),
            "{text}"
        );
    }

    #[test]
    fn a_datasource_names_its_endpoint_by_its_address() {
        assert_eq!(slug_of("http://gw:8080/api/endpoint/abc"), Some("abc"));
        assert_eq!(slug_of("http://gw:8080/api/endpoint/abc/"), Some("abc"));
        assert_eq!(slug_of("http://prometheus:9090"), None);
        assert_eq!(slug_of("http://gw/api/endpoint/abc/more"), None);
    }

    #[test]
    fn a_long_endpoint_name_keeps_a_uid_grafana_takes() {
        assert_eq!(datasource_uid("air", "abcdefgh"), "air");
        let uid = datasource_uid(&"n".repeat(63), &"s".repeat(32));
        assert_eq!(uid.len(), MAX_UID);
        assert!(uid.ends_with("-ssssssss"));
    }

    fn config(server: &wiremock::MockServer) -> GrafanaConfig {
        GrafanaConfig {
            url: server.uri(),
            public_url: "https://dev.example/grafana".into(),
            user: "grafana_admin".into(),
            password: "unused".into(),
        }
    }

    async fn answer_with(server: &wiremock::MockServer, verb: &str, route: &str, body: Value) {
        Mock::given(method(verb))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn an_answer_past_the_cap_is_refused_unread() {
        let server = wiremock::MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/datasources"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(format!("[{}]", " ".repeat(MAX_ANSWER))),
            )
            .mount(&server)
            .await;
        let config = config(&server);
        let refused = Client::new(&config).datasources(7).await;
        assert!(
            matches!(refused, Err(ApiError::Unavailable(_))),
            "{refused:?}"
        );
    }

    #[tokio::test]
    async fn the_datasources_of_an_organization_are_those_reading_an_endpoint() {
        let server = wiremock::MockServer::start().await;
        answer_with(
            &server,
            "GET",
            "/api/datasources",
            json!([
                { "type": PLUGIN, "uid": "air", "url": "http://gw:8080/api/endpoint/airslug" },
                { "type": PLUGIN, "uid": "other", "url": "http://elsewhere" },
                { "type": "prometheus", "uid": "metrics", "url": "http://gw/api/endpoint/x" },
            ]),
        )
        .await;
        let config = config(&server);
        let held = Client::new(&config).datasources(7).await.expect("a list");
        assert_eq!(
            held,
            [Datasource {
                slug: "airslug".into(),
                uid: "air".into()
            }]
        );
    }

    #[tokio::test]
    async fn a_push_makes_a_public_datasource_and_stops_at_a_closed_endpoint() {
        let server = wiremock::MockServer::start().await;
        answer_with(&server, "GET", "/api/datasources", json!([])).await;
        answer_with(&server, "POST", "/api/datasources", json!({})).await;
        answer_with(&server, "POST", "/api/dashboards/db", json!({})).await;
        let config = config(&server);
        let client = Client::new(&config);
        let mut settings = crate::config::Config::for_tests();
        settings.gateway_url = Some("http://gw:8080".into());
        let state = AppState::new(settings, None);
        let row = row(Kind::Stat);

        let closed = Source {
            name: "air",
            slug: "airslug",
            public: false,
        };
        assert!(!push(&state, &client, 7, &row, &closed, "air")
            .await
            .expect("an answer"));

        let open = Source {
            public: true,
            ..closed
        };
        assert!(push(&state, &client, 7, &row, &open, "air")
            .await
            .expect("an answer"));
        let requests = server.received_requests().await.expect("recorded");
        let posted: Vec<&str> = requests
            .iter()
            .filter(|request| request.method.as_str() == "POST")
            .map(|request| request.url.path())
            .collect();
        assert_eq!(posted, ["/api/datasources", "/api/dashboards/db"]);
    }
}
