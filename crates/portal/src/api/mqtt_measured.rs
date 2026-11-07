//! `GET /api/v1/projects/{project}/mqttbrokers/{name}/measured`: what an MQTT broker measures,
//! read from its own RMQTT HTTP API inside the cluster (API/01 §44, ADR-N-048 §2.8, T-4043).
//!
//! Two reads, both routed for the Portal's mesh identity alone: `GET /api/v1/metrics/history/sum`
//! (a sample every five seconds, newest first) gives the messages per second, and
//! `GET /api/v1/clients` the connected clients, counted per ServiceAccount by their `username`,
//! the account's Keycloak client `{project}-{serviceAccount}`. A broker that does not answer is
//! "not measured", never zero; no topic, client id or address leaves this module.

use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, NaiveDateTime, Utc};
use jc_core::kinds::mqtt_broker::release_name;
use jc_core::kinds::MqttBrokerSpec;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::session::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::state::AppState;

/// The broker's HTTP API port (the chart's `rmqtt-http-api`, ADR-N-048 §2.8).
const API_PORT: u16 = 6060;
/// How long one read may take before the broker counts as not answering.
const TIMEOUT: Duration = Duration::from_secs(5);
/// The connected clients read at once; the chart's `max_row_limit` is 1000.
const CLIENT_ROWS: u32 = 1000;
/// The most one broker answer is read to: 1,000 clients of a few hundred bytes each fit well
/// below it, and a broker that answers without end costs one refused read (F-R2POR2-024).
const ANSWER_BYTES: usize = 4 * 1024 * 1024;

/// What one broker measures now.
#[derive(Debug, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Measured {
    /// `false` when the broker did not answer; `reason` says why and no number is given.
    pub measured: bool,
    #[schema(value_type = String, format = DateTime)]
    pub observed_at: DateTime<Utc>,
    /// `messages.publish` of the two newest samples, per second; `null` with fewer than two.
    pub messages_per_second: Option<f64>,
    /// Connected clients of the accounts whose roles may publish.
    pub publishers: Option<u32>,
    pub reason: Option<String>,
    /// Every ServiceAccount the broker's roles name, in name order.
    pub accounts: Vec<AccountSeen>,
    /// Messages per second between each two neighbouring samples the broker keeps (its
    /// `history_retention`, five minutes in the chart), oldest first; empty when not measured.
    pub rates: Vec<RatePoint>,
    /// Publishes the broker refused since it started (`client.publish.auth.error` of the newest
    /// sample): the gateway's publish check said no, or the client was over its rate.
    pub refused_publishes: Option<u64>,
}

/// The rate between one sample and the one before it, at the newer sample's time.
#[derive(Debug, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RatePoint {
    #[schema(value_type = String, format = DateTime)]
    pub at: DateTime<Utc>,
    pub per_second: f64,
}

/// One ServiceAccount of the broker's roles and its connected clients.
#[derive(Debug, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountSeen {
    pub service_account: String,
    pub connected: u32,
    /// The newest `connected_at` of its connected clients; `null` when none is connected.
    #[schema(value_type = Option<String>, format = DateTime)]
    pub since: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
struct History {
    data: Vec<Sample>,
}

#[derive(Deserialize)]
struct Sample {
    ts: i64,
    #[serde(rename = "messages.publish")]
    published: f64,
    #[serde(rename = "client.publish.auth.error", default)]
    publish_refused: f64,
}

#[derive(Deserialize)]
struct Client {
    #[serde(default)]
    username: String,
    #[serde(default)]
    connected: bool,
    #[serde(default)]
    connected_at: String,
}

/// Messages per second between the two newest samples of `metrics/history/sum`, whatever order
/// they come in; `None` with fewer than two, or two at one instant.
fn rate(history: &History) -> Option<f64> {
    let mut samples: Vec<&Sample> = history.data.iter().collect();
    samples.sort_by_key(|sample| std::cmp::Reverse(sample.ts));
    let (newest, before) = (samples.first()?, samples.get(1)?);
    let seconds = (newest.ts - before.ts) as f64 / 1000.0;
    (seconds > 0.0).then(|| ((newest.published - before.published) / seconds).max(0.0))
}

/// The rate between each two neighbouring samples, oldest first; a pair at one instant or out of
/// any time order is skipped, and a counter that went back (a restart) reads as zero.
fn rates(history: &History) -> Vec<RatePoint> {
    let mut samples: Vec<&Sample> = history.data.iter().collect();
    samples.sort_by_key(|sample| sample.ts);
    samples
        .windows(2)
        .filter_map(|pair| {
            let (before, newer) = (pair.first()?, pair.get(1)?);
            let seconds = (newer.ts - before.ts) as f64 / 1000.0;
            let at = DateTime::from_timestamp_millis(newer.ts)?;
            (seconds > 0.0).then(|| RatePoint {
                at,
                per_second: ((newer.published - before.published) / seconds).max(0.0),
            })
        })
        .collect()
}

/// `client.publish.auth.error` of the newest sample; `None` without a sample.
fn refused_publishes(history: &History) -> Option<u64> {
    let newest = history.data.iter().max_by_key(|sample| sample.ts)?;
    Some(newest.publish_refused.max(0.0) as u64)
}

/// The broker's connected clients per account of its roles, and the publishers among them.
fn tally(project: &str, spec: &MqttBrokerSpec, clients: &[Client]) -> (Vec<AccountSeen>, u32) {
    let mut accounts: BTreeMap<&str, AccountSeen> = BTreeMap::new();
    for account in spec.roles.iter().flat_map(|role| &role.service_accounts) {
        accounts.entry(account).or_insert_with(|| AccountSeen {
            service_account: account.clone(),
            connected: 0,
            since: None,
        });
    }
    for client in clients.iter().filter(|client| client.connected) {
        let Some(account) = client
            .username
            .strip_prefix(project)
            .and_then(|rest| rest.strip_prefix('-'))
        else {
            continue;
        };
        let Some(seen) = accounts.get_mut(account) else {
            continue;
        };
        seen.connected += 1;
        // RMQTT writes `connected_at` in UTC without a zone.
        let at = NaiveDateTime::parse_from_str(&client.connected_at, "%Y-%m-%d %H:%M:%S")
            .ok()
            .map(|at| at.and_utc());
        seen.since = seen.since.max(at);
    }
    let publishers = spec
        .roles
        .iter()
        .filter(|role| !role.publish.is_empty())
        .flat_map(|role| &role.service_accounts)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .filter_map(|account| accounts.get(account.as_str()))
        .map(|seen| seen.connected)
        .sum();
    (accounts.into_values().collect(), publishers)
}

pub(crate) fn http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default()
    })
}

async fn read<T: for<'de> Deserialize<'de>>(base: &str, path: &str) -> Result<T, String> {
    let answer = http()
        .get(format!("{base}{path}"))
        .send()
        .await
        .map_err(|_| "the broker did not answer; try again in a minute".to_owned())?;
    if !answer.status().is_success() {
        return Err(format!(
            "the broker answered {} to the Portal's read; check that the mesh admits the Portal on its HTTP API",
            answer.status().as_u16()
        ));
    }
    crate::capped::json(answer, ANSWER_BYTES)
        .await
        .map_err(|_| "the broker answered something other than its HTTP API".to_owned())
}

/// Reads the broker at `base` (`http://host:port`) and counts what it holds for `spec`.
pub async fn measure(base: &str, project: &str, spec: &MqttBrokerSpec) -> Measured {
    let observed_at = Utc::now();
    let history = read::<History>(base, "/api/v1/metrics/history/sum");
    let clients_path = format!("/api/v1/clients?_limit={CLIENT_ROWS}");
    let clients = read::<Vec<Client>>(base, &clients_path);
    match tokio::join!(history, clients) {
        (Ok(history), Ok(clients)) => {
            let (accounts, publishers) = tally(project, spec, &clients);
            Measured {
                measured: true,
                observed_at,
                messages_per_second: rate(&history),
                publishers: Some(publishers),
                reason: None,
                accounts,
                rates: rates(&history),
                refused_publishes: refused_publishes(&history),
            }
        }
        (Err(reason), _) | (_, Err(reason)) => not_measured(observed_at, reason),
    }
}

fn not_measured(observed_at: DateTime<Utc>, reason: String) -> Measured {
    Measured {
        measured: false,
        observed_at,
        messages_per_second: None,
        publishers: None,
        reason: Some(reason),
        accounts: Vec::new(),
        rates: Vec::new(),
        refused_publishes: None,
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/mqttbrokers/{name}/measured",
    summary = "What an MQTT Broker Measures",
    description = "Connected clients per ServiceAccount and messages per second, read from the broker's own HTTP API (API/01 §44, ADR-N-048 §2.8). Whoever may read the project's MqttBrokers.",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "MqttBroker name"),
    ),
    responses(
        (status = 200, description = "What the broker measures, or why it is not measured", body = Measured),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 404, description = "No such broker, or not readable by the caller", body = ProblemDetails),
    )
)]
pub async fn get_measured(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<Measured>, ApiError> {
    let not_found = || {
        ApiError::NotFound(format!(
            "MQTT broker '{name}' not found in project '{project}'"
        ))
    };
    // PF-59: a caller who may not read the project's brokers learns nothing of this one.
    let effective = crate::permissions::for_request(&state, &user.0.identity, &project);
    if !effective.may_read_project() || !effective.may_read("MqttBroker") {
        return Err(not_found());
    }
    let manifest = state
        .mirror
        .get(&project, "MqttBroker", &name)
        .ok_or_else(not_found)?;
    let spec: MqttBrokerSpec = serde_json::from_value(manifest.spec).map_err(|_| not_found())?;
    let release = release_name(&project, &name);
    Ok(Json(measured_now(&state, &project, &spec, &release).await))
}

/// What broker `release` of `project` measures now, read in the instance's MQTT namespace.
pub(crate) async fn measured_now(
    state: &AppState,
    project: &str,
    spec: &MqttBrokerSpec,
    release: &str,
) -> Measured {
    let Some(namespace) = state.config.mqtt_namespace.as_deref() else {
        return not_measured(
            Utc::now(),
            "this instance runs no MQTT brokers namespace (JC_PORTAL_MQTT_NAMESPACE)".to_owned(),
        );
    };
    let base = format!("http://{release}.{namespace}.svc.cluster.local:{API_PORT}");
    measure(&base, project, spec).await
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/projects/{project}/mqttbrokers/{name}/measured",
        get(get_measured),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F-R2POR2-024 (T-6226): a broker answer past the cap is refused, not held.
    #[tokio::test]
    async fn a_broker_answer_past_the_cap_is_not_measured() {
        use wiremock::{matchers::path, Mock, MockServer, ResponseTemplate};
        let broker = MockServer::start().await;
        let mut huge = b"[".to_vec();
        huge.resize(ANSWER_BYTES + 1, b' ');
        Mock::given(path("/big"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(huge))
            .mount(&broker)
            .await;
        Mock::given(path("/small"))
            .respond_with(ResponseTemplate::new(200).set_body_string("[]"))
            .mount(&broker)
            .await;
        let refused = read::<Vec<serde_json::Value>>(&broker.uri(), "/big").await;
        assert!(refused.is_err(), "an answer past the cap was read");
        let read_whole = read::<Vec<serde_json::Value>>(&broker.uri(), "/small").await;
        assert_eq!(
            read_whole.expect("a small answer is read"),
            Vec::<serde_json::Value>::new()
        );
    }

    fn spec() -> MqttBrokerSpec {
        serde_json::from_value(serde_json::json!({
            "roles": [
                {"name": "camera", "serviceAccounts": ["cam-west", "cam-east"], "publish": ["cameras/${clientid}/#"]},
                {"name": "viewer", "serviceAccounts": ["dashboard"], "subscribe": ["dashboard/${clientid}/#"]},
            ],
        }))
        .expect("a broker reads")
    }

    fn sample(ts: i64, published: f64) -> Sample {
        Sample {
            ts,
            published,
            publish_refused: 0.0,
        }
    }

    #[test]
    fn the_rates_are_each_neighbouring_pair_oldest_first() {
        let history = History {
            data: vec![
                sample(10_000, 40.0),
                sample(0, 10.0),
                sample(5_000, 20.0),
                sample(5_000, 20.0),
                // A restart: the counter starts again.
                sample(15_000, 3.0),
            ],
        };
        let seen: Vec<(i64, f64)> = rates(&history)
            .iter()
            .map(|point| (point.at.timestamp_millis(), point.per_second))
            .collect();
        assert_eq!(seen, [(5_000, 2.0), (10_000, 4.0), (15_000, 0.0)]);
        assert!(rates(&History {
            data: vec![sample(1, 5.0)]
        })
        .is_empty());
    }

    #[test]
    fn the_refused_publishes_are_the_newest_samples_counter() {
        let mut newest = sample(9_000, 1.0);
        newest.publish_refused = 812.0;
        let mut older = sample(4_000, 1.0);
        older.publish_refused = 800.0;
        assert_eq!(
            refused_publishes(&History {
                data: vec![newest, older]
            }),
            Some(812)
        );
        assert_eq!(refused_publishes(&History { data: Vec::new() }), None);
    }

    #[test]
    fn the_rate_is_the_two_newest_samples_in_any_order() {
        let history = History {
            data: vec![
                sample(1_000, 10.0),
                sample(11_000, 40.0),
                sample(6_000, 20.0),
            ],
        };
        assert_eq!(rate(&history), Some(4.0));
        assert_eq!(
            rate(&History {
                data: vec![sample(1, 5.0)]
            }),
            None
        );
        assert_eq!(rate(&History { data: Vec::new() }), None);
        // Two samples at one instant, or a counter that went back (a restart), are no rate below zero.
        assert_eq!(
            rate(&History {
                data: vec![sample(5, 1.0), sample(5, 9.0)]
            }),
            None
        );
        assert_eq!(
            rate(&History {
                data: vec![sample(5_000, 1.0), sample(0, 9.0)]
            }),
            Some(0.0)
        );
    }

    #[test]
    fn clients_count_for_their_own_account_and_no_other() {
        let client = |username: &str, connected: bool, at: &str| Client {
            username: username.to_owned(),
            connected,
            connected_at: at.to_owned(),
        };
        let clients = [
            client("helsinki-cam-west", true, "2026-10-12 08:46:51"),
            client("helsinki-cam-west", true, "2026-10-12 08:50:00"),
            client("helsinki-cam-east", false, "2026-10-12 07:00:00"),
            client("helsinki-dashboard", true, "2026-10-12 08:00:00"),
            // Another project's account of the same name, the runner, and an anonymous client.
            client("espoo-cam-west", true, "2026-10-12 08:55:00"),
            client("pipeline-runner", true, "2026-10-12 08:00:00"),
            client("undefined", true, "2026-10-12 08:00:00"),
            client("helsinkicam-west", true, "2026-10-12 08:00:00"),
        ];
        let (accounts, publishers) = tally("helsinki", &spec(), &clients);
        let names: Vec<&str> = accounts
            .iter()
            .map(|a| a.service_account.as_str())
            .collect();
        assert_eq!(names, ["cam-east", "cam-west", "dashboard"]);
        assert_eq!((accounts[0].connected, accounts[0].since), (0, None));
        assert_eq!(accounts[1].connected, 2);
        assert_eq!(
            accounts[1].since.map(|at| at.to_rfc3339()),
            Some("2026-10-12T08:50:00+00:00".to_owned())
        );
        assert_eq!(accounts[2].connected, 1);
        // The dashboard only subscribes: it is no publisher.
        assert_eq!(publishers, 2);
    }

    #[test]
    fn an_unreadable_connected_at_counts_the_client_without_a_time() {
        let clients = [Client {
            username: "helsinki-cam-west".to_owned(),
            connected: true,
            connected_at: "yesterday".to_owned(),
        }];
        let (accounts, publishers) = tally("helsinki", &spec(), &clients);
        assert_eq!(
            (accounts[1].connected, accounts[1].since, publishers),
            (1, None, 1)
        );
    }
}
