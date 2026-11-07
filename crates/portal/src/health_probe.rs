//! The health log's probes (T-5067): each surface a live Endpoint serves is asked once every
//! [`EVERY`], and each Context Space is counted on its broker.
//!
//! A probe sends no credential at all: a public Endpoint must answer it, a non-public one must
//! answer it or refuse it with 401/403; both prove the surface is up, and the probe never holds
//! more rights than an anonymous visitor (never a steward's token). It reads the status and the
//! time to the first byte, never the body. The count comes from the broker's admin route through
//! [`crate::space_usage::Counter`], the same reading the space page shows.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use chrono::{DateTime, Duration, Utc};
use futures_util::stream::{self, StreamExt};
use url::Url;

use crate::health_log::{Finding, Kind, Status, Text};
use crate::resource::Phase;
use crate::space_usage::{Counter, SpaceUsage};
use crate::store::Mirror;

/// How often the probes run; a broken surface turns red within this and the log's own beat.
pub const EVERY: Duration = Duration::minutes(5);
/// An answer slower than this is a warning.
pub const SLOW_MS: u64 = 3_000;
const TIMEOUT: StdDuration = StdDuration::from_secs(10);
const AT_ONCE: usize = 8;

/// What one surface answered: its status, or why it did not answer, and how long it took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub status: Option<u16>,
    pub ms: u64,
    pub error: Option<String>,
}

/// The request that asks surface `surface` of the Endpoint at `base` whether it is up; `None`
/// for a surface the probes do not ask.
fn request(client: &reqwest::Client, base: &str, surface: &str) -> Option<reqwest::RequestBuilder> {
    let get = |path: &str| client.get(format!("{base}{path}"));
    let json = "application/json";
    Some(match surface {
        "ngsi-ld" => get("ngsi-ld/v1/types").header(reqwest::header::ACCEPT, json),
        "odata" => get("odata/v4/$metadata"),
        "ogc-features" => get("ogc/features/collections").header(reqwest::header::ACCEPT, json),
        "sta" => get("sta/v1.1"),
        "csv" => get("file.csv"),
        "ical" => get("ical/v1/events.ics"),
        "websocket" => get("ws")
            .header(reqwest::header::CONNECTION, "Upgrade")
            .header(reqwest::header::UPGRADE, "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ=="),
        "mcp" => client
            .post(format!("{base}mcp"))
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})),
        _ => return None,
    })
}

/// The check of one surface from its answer.
pub fn judge_surface(
    project: &str,
    endpoint: &str,
    surface: &str,
    public: bool,
    answer: &Answer,
) -> Finding {
    let finding = Finding::new(project, Kind::Endpoint, endpoint, surface);
    let named = |status: u16| {
        vec![
            ("surface", surface.to_owned()),
            ("status", status.to_string()),
            ("ms", answer.ms.to_string()),
        ]
    };
    let Some(status) = answer.status else {
        let reason = answer.error.clone().unwrap_or_else(|| "no answer".into());
        return finding.judged(
            Status::Error,
            Text::say(
                "noAnswer",
                &[
                    ("surface", surface.to_owned()),
                    ("reason", Text::raw(&reason).text),
                ],
            ),
            Text::say("serviceDown", &[]),
        );
    };
    let up = (200..300).contains(&status) || (surface == "websocket" && status == 101);
    match status {
        _ if up && answer.ms > SLOW_MS => finding.judged(
            Status::Warn,
            Text::say("slow", &named(status)),
            Text::say("slowSurface", &[]),
        ),
        _ if up => finding.judged(
            Status::Ok,
            Text::say("answered", &named(status)),
            Text::default(),
        ),
        401 | 403 if public => finding.judged(
            Status::Error,
            Text::say("anonymousRefused", &named(status)),
            Text::say("publicRefused", &[]),
        ),
        401 | 403 => finding.judged(
            Status::Ok,
            Text::say("answered", &named(status)),
            Text::default(),
        ),
        404 | 405 | 501 => finding.judged(
            Status::Error,
            Text::say("notServed", &named(status)),
            Text::say("surfaceMissing", &[]),
        ),
        500.. => finding.judged(
            Status::Error,
            Text::say("notServed", &named(status)),
            Text::say("serviceDown", &[]),
        ),
        _ => finding.judged(
            Status::Warn,
            Text::say("notServed", &named(status)),
            Text::say("surfaceMissing", &[]),
        ),
    }
}

/// The `entities` and `storage` checks of one space from its count, against the count before.
pub fn judge_count(
    project: &str,
    space: &str,
    before: Option<u64>,
    usage: Result<&SpaceUsage, &str>,
    has_retention: bool,
) -> Vec<Finding> {
    let entities = Finding::new(project, Kind::Space, space, "entities");
    let usage = match usage {
        Ok(usage) => usage,
        Err(reason) => {
            return vec![entities.judged(
                Status::Error,
                Text::say("notCounted", &[("reason", Text::raw(reason).text)]),
                Text::say("serviceDown", &[]),
            )]
        }
    };
    let now = usage.entities;
    let history = usage.bytes.as_ref().map(|b| b.attr_instances);
    let counted = Text::say(
        "counted",
        &[
            ("entities", now.to_string()),
            (
                "history",
                history.map_or_else(|| "?".into(), |h| h.to_string()),
            ),
        ],
    );
    let entities = match before {
        Some(before) if before > 0 && now == 0 => entities.judged(
            Status::Error,
            Text::say("countFellToZero", &[("before", before.to_string())]),
            Text::say("countFell", &[]),
        ),
        Some(before) if before >= 10 && now * 2 < before => entities.judged(
            Status::Warn,
            Text::say(
                "countHalved",
                &[("before", before.to_string()), ("now", now.to_string())],
            ),
            Text::say("countFell", &[]),
        ),
        _ => entities.judged(Status::Ok, counted.clone(), Text::default()),
    };
    let mut findings = vec![entities];
    if let Some(history) = history {
        let storage = Finding::new(project, Kind::Space, space, "storage");
        findings.push(if history > 0 && !has_retention {
            storage.judged(Status::Warn, counted, Text::say("retention", &[]))
        } else {
            storage.judged(Status::Ok, counted, Text::default())
        });
    }
    findings
}

/// The probes and what they found last.
pub struct Probes {
    gateway: Option<Url>,
    counter: Option<Arc<Counter>>,
    client: reqwest::Client,
    counts: tokio::sync::Mutex<HashMap<(String, String), u64>>,
    kept: tokio::sync::Mutex<Option<(DateTime<Utc>, Vec<Finding>)>>,
}

impl Probes {
    /// Probes the Endpoints through `gateway` (none: no surface is asked) and counts spaces with
    /// `counter` (none: no space is counted).
    pub fn new(gateway: Option<Url>, counter: Option<Arc<Counter>>) -> Self {
        Self {
            gateway,
            counter,
            client: reqwest::Client::builder()
                .user_agent("joinedcontext-portal health")
                .timeout(TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap_or_default(),
            counts: tokio::sync::Mutex::default(),
            kept: tokio::sync::Mutex::default(),
        }
    }

    /// The findings of the last probe run, run again when it is [`EVERY`] old.
    pub async fn findings(&self, mirror: &Mirror, now: DateTime<Utc>) -> Vec<Finding> {
        let mut kept = self.kept.lock().await;
        if let Some((at, findings)) = kept.as_ref() {
            if now - *at < EVERY {
                return findings.clone();
            }
        }
        let mut findings = self.surfaces(mirror).await;
        findings.extend(self.spaces(mirror).await);
        *kept = Some((now, findings.clone()));
        findings
    }

    async fn surfaces(&self, mirror: &Mirror) -> Vec<Finding> {
        let Some(gateway) = &self.gateway else {
            return Vec::new();
        };
        let mut asks = Vec::new();
        for endpoint in mirror.matching(|env| env.kind == "Endpoint") {
            let (Some(project), Some(slug)) = (
                endpoint.metadata.namespace.clone(),
                endpoint
                    .spec
                    .get("slug")
                    .and_then(serde_json::Value::as_str),
            ) else {
                continue;
            };
            let live = endpoint
                .status
                .as_ref()
                .is_some_and(|s| s.phase == Phase::Live);
            if !live || !crate::resource::is_dns1123(slug) {
                continue;
            }
            let public = endpoint
                .spec
                .get("audience")
                .and_then(serde_json::Value::as_str)
                == Some("public");
            let base = crate::api::catalogue::endpoint_url(gateway, slug);
            for surface in crate::api::catalogue::served_representations(&endpoint.spec) {
                if let Some(ask) = request(&self.client, &base, &surface) {
                    asks.push((
                        project.clone(),
                        endpoint.metadata.name.clone(),
                        surface,
                        public,
                        ask,
                    ));
                }
            }
        }
        stream::iter(asks)
            .map(|(project, name, surface, public, ask)| async move {
                let started = Instant::now();
                let sent = ask.send().await;
                let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                let answer = match sent {
                    Ok(response) => Answer {
                        status: Some(response.status().as_u16()),
                        ms,
                        error: None,
                    },
                    Err(error) => Answer {
                        status: None,
                        ms,
                        error: Some(if error.is_timeout() {
                            format!("no answer within {} s", TIMEOUT.as_secs())
                        } else {
                            "the connection failed".into()
                        }),
                    },
                };
                judge_surface(&project, &name, &surface, public, &answer)
            })
            .buffer_unordered(AT_ONCE)
            .collect()
            .await
    }

    async fn spaces(&self, mirror: &Mirror) -> Vec<Finding> {
        let Some(counter) = &self.counter else {
            return Vec::new();
        };
        let mut counts = self.counts.lock().await;
        let mut findings = Vec::new();
        for space in mirror.matching(|env| env.kind == "ContextSpace") {
            let Some(project) = space.metadata.namespace.clone() else {
                continue;
            };
            let name = space.metadata.name.clone();
            let broker = crate::spaces::broker(mirror, &project, &name);
            if !counter.reaches(&broker) {
                continue;
            }
            let tenant = crate::spaces::segment(mirror, &project, &name);
            let usage = counter.usage(&broker, &tenant).await;
            let key = (project.clone(), name.clone());
            let retention = space.spec.get("retention").is_some_and(|r| !r.is_null());
            findings.extend(judge_count(
                &project,
                &name,
                counts.get(&key).copied(),
                usage.as_ref().map_err(String::as_str),
                retention,
            ));
            if let Ok(usage) = usage {
                counts.insert(key, usage.entities);
            }
        }
        findings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(status: u16, ms: u64) -> Answer {
        Answer {
            status: Some(status),
            ms,
            error: None,
        }
    }

    #[test]
    fn a_public_surface_is_ok_slow_refusing_missing_or_down() {
        let judge = |status, ms, public| {
            judge_surface("bbsk", "events", "odata", public, &answer(status, ms)).status
        };
        assert_eq!(judge(200, 120, true), Status::Ok);
        assert_eq!(judge(200, 4_000, true), Status::Warn);
        assert_eq!(judge(401, 50, true), Status::Error);
        assert_eq!(judge(404, 50, true), Status::Error);
        assert_eq!(judge(503, 50, true), Status::Error);
        let refused = judge_surface("bbsk", "events", "odata", true, &answer(403, 50));
        assert_eq!(refused.fix.code, "publicRefused");
        assert_eq!(
            refused.detail.args.get("status").map(String::as_str),
            Some("403")
        );
        assert_eq!(refused.evidence_url, "/projects/bbsk/endpoints/events");
    }

    #[test]
    fn a_private_surface_that_asks_for_a_token_is_up() {
        let guarded = judge_surface("bbsk", "internal", "ngsi-ld", false, &answer(401, 40));
        assert_eq!(guarded.status, Status::Ok);
        assert!(guarded.fix.text.is_empty());
    }

    #[test]
    fn a_websocket_upgrade_and_an_unanswered_surface() {
        assert_eq!(
            judge_surface("p", "e", "websocket", true, &answer(101, 30)).status,
            Status::Ok
        );
        let down = judge_surface(
            "p",
            "e",
            "mcp",
            true,
            &Answer {
                status: None,
                ms: 10_000,
                error: Some("no answer within 10 s".into()),
            },
        );
        assert_eq!(
            (down.status, down.fix.code.as_str()),
            (Status::Error, "serviceDown")
        );
        assert!(
            down.detail.text.contains("mcp did not answer"),
            "{}",
            down.detail.text
        );
    }

    fn usage(entities: u64, history: u64) -> SpaceUsage {
        SpaceUsage {
            entities,
            broker: "main".into(),
            observed_at: Utc::now(),
            measured_at: Utc::now(),
            bytes: Some(crate::space_usage::SpaceBytes {
                entities,
                attr_instances: history,
                by_type: std::collections::BTreeMap::new(),
            }),
            attr_instances: None,
            estimated: false,
        }
    }

    #[test]
    fn a_space_that_emptied_or_halved_is_flagged_and_recovers() {
        let status =
            |before, now: &SpaceUsage| judge_count("bbsk", "air", before, Ok(now), true)[0].status;
        assert_eq!(status(None, &usage(100, 0)), Status::Ok);
        assert_eq!(status(Some(100), &usage(0, 0)), Status::Error);
        assert_eq!(status(Some(100), &usage(40, 0)), Status::Warn);
        assert_eq!(status(Some(0), &usage(100, 0)), Status::Ok, "recovered");
        let failed = judge_count("bbsk", "air", Some(5), Err("the broker answered 503"), true);
        assert_eq!((failed.len(), failed[0].status), (1, Status::Error));
    }

    #[test]
    fn history_without_retention_warns() {
        let findings = judge_count("bbsk", "air", None, Ok(&usage(10, 5_000)), false);
        let storage = findings
            .iter()
            .find(|f| f.check == "storage")
            .expect("storage");
        assert_eq!(
            (storage.status, storage.fix.code.as_str()),
            (Status::Warn, "retention")
        );
        let kept = judge_count("bbsk", "air", None, Ok(&usage(10, 5_000)), true);
        assert!(kept.iter().all(|f| f.status == Status::Ok));
    }

    #[test]
    fn only_known_surfaces_are_asked_and_never_with_a_credential() {
        let client = reqwest::Client::new();
        let base = "http://gateway.test/api/endpoint/events/";
        for surface in [
            "ngsi-ld",
            "odata",
            "ogc-features",
            "sta",
            "csv",
            "ical",
            "websocket",
            "mcp",
        ] {
            let built = request(&client, base, surface)
                .expect("asked")
                .build()
                .expect("a request");
            assert!(built.url().as_str().starts_with(base), "{}", built.url());
            assert!(built
                .headers()
                .get(reqwest::header::AUTHORIZATION)
                .is_none());
            assert!(built.headers().get(reqwest::header::COOKIE).is_none());
        }
        assert!(request(&client, base, "xlsx").is_none());
    }

    #[tokio::test]
    async fn a_broken_surface_turns_red_and_green_again_after_the_fix() {
        use std::sync::atomic::{AtomicU16, Ordering};
        let ngsi = Arc::new(AtomicU16::new(503));
        let answer = Arc::clone(&ngsi);
        let app = axum::Router::new().fallback(move |uri: axum::http::Uri| {
            let answer = Arc::clone(&answer);
            async move {
                let status = if uri.path().ends_with("/ngsi-ld/v1/types") {
                    answer.load(Ordering::SeqCst)
                } else {
                    200
                };
                axum::http::StatusCode::from_u16(status).unwrap_or(axum::http::StatusCode::OK)
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port");
        let gateway = Url::parse(&format!(
            "http://{}/",
            listener.local_addr().expect("an address")
        ))
        .expect("a url");
        tokio::spawn(async move { axum::serve(listener, app).await });
        let mirror = Mirror::default();
        mirror.upsert(
            serde_json::from_value(serde_json::json!({
                "apiVersion": crate::resource::API_VERSION, "kind": "Endpoint",
                "metadata": {"name": "events-api", "namespace": "bbsk"},
                "spec": {"slug": "events", "audience": "public", "enabledRepresentations": ["odata"], "mcp": false},
                "status": {"phase": "Live"},
            }))
            .expect("an endpoint"),
        );
        let probes = Probes::new(Some(gateway), None);
        let t0 = Utc::now();
        let red = probes.findings(&mirror, t0).await;
        let check = |findings: &[Finding], surface: &str| {
            findings
                .iter()
                .find(|f| f.check == surface)
                .map(|f| f.status)
        };
        assert_eq!(check(&red, "ngsi-ld"), Some(Status::Error), "{red:?}");
        assert_eq!(check(&red, "odata"), Some(Status::Ok));
        assert_eq!(red.len(), 2, "mcp is off: {red:?}");
        ngsi.store(200, Ordering::SeqCst);
        let kept = probes.findings(&mirror, t0 + Duration::minutes(1)).await;
        assert_eq!(
            check(&kept, "ngsi-ld"),
            Some(Status::Error),
            "kept until the next run"
        );
        let green = probes.findings(&mirror, t0 + EVERY).await;
        assert_eq!(check(&green, "ngsi-ld"), Some(Status::Ok));
    }
}
