//! What a project may hold, and what it holds now (PF-73, PF-74, PF-75).
//!
//! The organization sets the default quota of every project it opens; a project may carry its
//! own. The Portal counts what the repository declares — not what a cluster reports — because
//! the refusal has to happen before a `Change` exists, on every door (PF-74).
//!
//! Four dimensions are countable from the manifests: context spaces, resident pipelines, public
//! endpoints and apps. The rest (`ingestEventsPerSecond`, `entitiesPerSpace`,
//! `requestsPerMinute`, `agentRunsPerDay`) are runtime limits enforced where they happen.

use std::collections::BTreeMap;

use jc_core::kinds::space::Quotas;
use serde_json::Value;

use crate::error::ApiError;
use crate::permissions::ORG_NAMESPACE;
use crate::store::{ListOptions, Mirror};

/// The dimensions a manifest count answers, and the kind each one counts.
const COUNTED: [(&str, &str); 4] = [
    ("contextSpaces", "ContextSpace"),
    ("residentPipelines", "Pipeline"),
    ("publicEndpoints", "Endpoint"),
    ("apps", "App"),
];

/// The quota in force for `project`: its own `spec.quotas` when it has one, else the
/// organization's `spec.projects.quota`, else no limit at all (PF-73).
pub fn effective(mirror: &Mirror, project: &str) -> Quotas {
    let own = mirror
        .get(ORG_NAMESPACE, "Project", project)
        .and_then(|env| serde_json::from_value::<Quotas>(env.spec.get("quotas")?.clone()).ok())
        // `spec.quotas` holding only `mqtt` leaves every PF-73 dimension to the organization.
        .filter(Quotas::sets_dimensions);
    own.unwrap_or_else(|| organization_default(mirror))
}

/// The default every project of the organization starts from (PF-73).
pub fn organization_default(mirror: &Mirror) -> Quotas {
    mirror
        .list(ORG_NAMESPACE, "Organization", &ListOptions::default())
        .items
        .into_iter()
        .find_map(|env| {
            serde_json::from_value::<Quotas>(env.spec.pointer("/projects/quota")?.clone()).ok()
        })
        .unwrap_or_default()
}

/// Whether one manifest counts against `dimension`: every context space, an app in `preview` or
/// `published` of any class, a pipeline the runner keeps resident, an endpoint open to the public.
/// A draft or retired App runs, serves and is built nowhere, so it counts nothing (PF-73, AP-18,
/// AP-21, T-2654). A scheduled poll runs in the run queue and is deleted after each run, so only
/// a pipeline that waits for pushes is resident (PL-78, T-4952).
fn counts(mirror: &Mirror, project: &str, dimension: &str, kind: &str, spec: &Value) -> bool {
    match dimension {
        "contextSpaces" => kind == "ContextSpace",
        "apps" => {
            kind == "App"
                && matches!(
                    spec.get("lifecycle").and_then(Value::as_str),
                    Some("preview" | "published")
                )
        }
        "publicEndpoints" => {
            kind == "Endpoint" && spec.get("audience").and_then(Value::as_str) == Some("public")
        }
        "residentPipelines" => {
            kind == "Pipeline"
                && serde_json::from_value::<jc_core::kinds::pipeline::PipelineSpec>(spec.clone())
                    .is_ok_and(|spec| {
                        crate::reconciler::streams::is_stream_pipeline(&spec)
                            && crate::reconciler::streams::stays_resident(mirror, project, &spec)
                    })
        }
        _ => false,
    }
}

/// What the project holds now, per countable dimension (PF-75).
pub fn usage(mirror: &Mirror, project: &str) -> BTreeMap<String, u32> {
    COUNTED
        .into_iter()
        .map(|(dimension, kind)| {
            let held = mirror
                .list(project, kind, &ListOptions::default())
                .items
                .into_iter()
                .filter(|env| counts(mirror, project, dimension, kind, &env.spec))
                .count() as u32;
            (dimension.to_owned(), held)
        })
        .collect()
}

/// The limit of each countable dimension, absent where the quota leaves it open.
pub fn limits(quotas: &Quotas) -> BTreeMap<String, u32> {
    quotas
        .dimensions()
        .into_iter()
        .filter(|(dimension, _)| COUNTED.iter().any(|(counted, _)| counted == dimension))
        .filter_map(|(dimension, limit)| limit.map(|limit| (dimension.to_owned(), limit)))
        .collect()
}

/// The refusal every door gives for a quota, so the sentence is the same everywhere (PF-74).
pub fn over(dimension: &str, after: u32, limit: u32, project: &str) -> ApiError {
    ApiError::Denied(format!(
        "quota: {dimension} {after} of {limit} in project {project}; raise the quota on the \
         project or the organization first (PF-73, PF-74)"
    ))
}

/// The manifests of `kind` that stand beyond the project's quota for `dimension`, by name: what
/// the reconciler must not schedule (PF-74). Sorted by name, so the same ones keep running pass
/// after pass rather than taking turns.
pub fn beyond(mirror: &Mirror, project: &str, dimension: &str) -> Vec<String> {
    let Some((_, kind)) = COUNTED.into_iter().find(|(d, _)| *d == dimension) else {
        return Vec::new();
    };
    let Some(limit) = limits(&effective(mirror, project)).get(dimension).copied() else {
        return Vec::new();
    };
    let mut counted: Vec<String> = mirror
        .list(project, kind, &ListOptions::default())
        .items
        .into_iter()
        .filter(|env| counts(mirror, project, dimension, kind, &env.spec))
        .map(|env| env.metadata.name)
        .collect();
    counted.sort();
    counted.split_off((limit as usize).min(counted.len()))
}

/// Refuses the write that would put the project over one of its quotas, naming the limit and the
/// count (PF-74). `name` is the manifest being written, so replacing what is already counted is
/// not counted twice; a deletion never exceeds anything.
pub fn check(
    mirror: &Mirror,
    project: &str,
    kind: &str,
    name: &str,
    spec: &Value,
) -> Result<(), ApiError> {
    let quotas = effective(mirror, project);
    for (dimension, counted_kind) in COUNTED {
        if counted_kind != kind || !counts(mirror, project, dimension, kind, spec) {
            continue;
        }
        let Some(limit) = limits(&quotas).get(dimension).copied() else {
            continue;
        };
        // Rewriting what is already counted changes no count, so it exceeds nothing even in a
        // project already over the limit: the build lane's status.build of a published App, a
        // fix to one (PF-74, T-2633). What stands beyond the quota stays unscheduled (`beyond`).
        if mirror
            .get(project, kind, name)
            .is_some_and(|held| counts(mirror, project, dimension, kind, &held.spec))
        {
            continue;
        }
        let others = mirror
            .list(project, kind, &ListOptions::default())
            .items
            .into_iter()
            .filter(|env| {
                env.metadata.name != name && counts(mirror, project, dimension, kind, &env.spec)
            })
            .count() as u32;
        let after = others + 1;
        if after > limit {
            return Err(over(dimension, after, limit, project));
        }
    }
    if kind == "Pipeline" {
        check_clock(mirror, project, spec)?;
    }
    // What one Endpoint may serve is capped by the project's quota, so the gateway's rate limit
    // can never be rendered above it (PF-74, EP-17).
    if kind == "Endpoint" {
        if let (Some(asked), Some(limit)) = (
            spec.pointer("/rateLimits/requestsPerMinute")
                .and_then(Value::as_u64),
            quotas.requests_per_minute,
        ) {
            if asked > u64::from(limit) {
                return Err(over(
                    "requestsPerMinute",
                    asked.min(u64::from(u32::MAX)) as u32,
                    limit,
                    project,
                ));
            }
        }
    }
    Ok(())
}

/// The shortest clock a Pipeline of `project` may run on, in seconds, and the field that sets
/// it: the stricter of the project's own `spec.quotas` and the organization's default, so a
/// project only tightens the floor its organization sets (PL-116).
pub fn min_pipeline_period(mirror: &Mirror, project: &str) -> Option<(u32, String)> {
    let own = mirror
        .get(ORG_NAMESPACE, "Project", project)
        .and_then(|env| serde_json::from_value::<Quotas>(env.spec.get("quotas")?.clone()).ok())
        .and_then(|q| q.min_pipeline_period_seconds)
        .map(|floor| {
            (
                floor,
                format!("spec.quotas.minPipelinePeriodSeconds of project {project}"),
            )
        });
    let organization = organization_default(mirror)
        .min_pipeline_period_seconds
        .map(|floor| {
            (
                floor,
                "spec.projects.quota.minPipelinePeriodSeconds of the organization".to_owned(),
            )
        });
    own.into_iter()
        .chain(organization)
        .max_by_key(|(floor, _)| *floor)
}

/// Refuses a Pipeline whose period or schedule runs faster than the project's floor, naming the
/// field, the floor and where it is set (PL-116, S-22).
fn check_clock(mirror: &Mirror, project: &str, spec: &Value) -> Result<(), ApiError> {
    let Some((floor, set_by)) = min_pipeline_period(mirror, project) else {
        return Ok(());
    };
    let Ok(pipeline) = serde_json::from_value::<jc_core::kinds::PipelineSpec>(spec.clone()) else {
        return Ok(());
    };
    let Some(shortest) = pipeline.shortest_clock_seconds() else {
        return Ok(());
    };
    if shortest >= u64::from(floor) {
        return Ok(());
    }
    let field = if pipeline.period_seconds() == Some(shortest) {
        "spec.period"
    } else {
        "spec.schedule"
    };
    Err(ApiError::Unprocessable {
        detail: format!(
            "the pipeline runs every {shortest} s, faster than the minimum period of {floor} s \
             project {project} allows (PL-116)"
        ),
        errors: vec![format!(
            "{field}: runs every {shortest} s, below the minimum period of {floor} s \
             ({set_by}); choose {floor} s or longer, or ask an admin to lower the minimum"
        )],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
    use serde_json::json;

    fn envelope(kind: &str, name: &str, namespace: &str, spec: Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: ObjectMeta::new(name, namespace),
            spec,
            status: None,
        }
    }

    fn resident(name: &str) -> ResourceEnvelope {
        envelope(
            "Pipeline",
            name,
            "ovzdusie",
            json!({
                "class": "resident",
                "source": { "dataSourceRef": { "kind": "DataSource", "name": "mqtt" } },
                "compute": { "kind": "bloblang", "bloblang": "root = this" },
                "targetEndpoint": "urn:ngsi-ld:Endpoint:bb.sk:ovzdusie:air"
            }),
        )
    }

    /// The MQTT feed every `resident` pipeline reads: it waits for pushes, so it stays loaded.
    fn mqtt_source() -> ResourceEnvelope {
        envelope(
            "DataSource",
            "mqtt",
            "ovzdusie",
            json!({ "type": "mqtt", "mqtt": { "urls": ["tcp://mqtt.bb.sk:1883"], "topics": ["air/#"] } }),
        )
    }

    fn mirror_with(items: Vec<ResourceEnvelope>) -> Mirror {
        let mirror = Mirror::new();
        mirror.upsert(mqtt_source());
        for item in items {
            mirror.upsert(item);
        }
        mirror
    }

    #[test]
    fn the_projects_own_quota_wins_over_the_organizations_default() {
        let mirror = mirror_with(vec![
            envelope(
                "Organization",
                "bb",
                ORG_NAMESPACE,
                json!({ "domain": "bb.sk", "projects": { "quota": { "residentPipelines": 3 } } }),
            ),
            envelope(
                "Project",
                "ovzdusie",
                ORG_NAMESPACE,
                json!({ "organizationRef": { "name": "bb" }, "quotas": { "residentPipelines": 1 } }),
            ),
        ]);
        assert_eq!(effective(&mirror, "ovzdusie").resident_pipelines, Some(1));
        assert_eq!(effective(&mirror, "doprava").resident_pipelines, Some(3));
        mirror.upsert(envelope(
            "Project",
            "mqtt-only",
            ORG_NAMESPACE,
            json!({ "organizationRef": { "name": "bb" }, "quotas": { "mqtt": { "enabled": true } } }),
        ));
        assert_eq!(
            effective(&mirror, "mqtt-only").resident_pipelines,
            Some(3),
            "an MQTT allowance alone lifts no other quota"
        );
    }

    #[test]
    fn the_next_one_over_the_limit_is_refused_and_the_message_names_both_numbers() {
        let mirror = mirror_with(vec![
            envelope(
                "Organization",
                "bb",
                ORG_NAMESPACE,
                json!({ "domain": "bb.sk", "projects": { "quota": { "residentPipelines": 2 } } }),
            ),
            resident("one"),
            resident("two"),
        ]);
        assert_eq!(
            usage(&mirror, "ovzdusie").get("residentPipelines"),
            Some(&2)
        );

        let refused = check(
            &mirror,
            "ovzdusie",
            "Pipeline",
            "three",
            &resident("three").spec,
        )
        .expect_err("the third resident pipeline");
        assert!(
            refused.to_string().contains("residentPipelines 3 of 2"),
            "{refused}"
        );

        // Writing one of the two again is not a third: the same name replaces itself.
        check(
            &mirror,
            "ovzdusie",
            "Pipeline",
            "two",
            &resident("two").spec,
        )
        .expect("an update of what is already counted");
    }

    #[test]
    fn rewriting_an_app_already_counted_is_not_refused_in_a_project_over_its_quota() {
        let app =
            |name: &str| envelope("App", name, "helsinki", json!({ "lifecycle": "published" }));
        let mirror = mirror_with(vec![
            envelope(
                "Organization",
                "hel",
                ORG_NAMESPACE,
                json!({ "domain": "hel.fi", "projects": { "quota": { "apps": 2 } } }),
            ),
            app("alerts"),
            app("bikes"),
            app("events"),
        ]);
        let published = json!({ "lifecycle": "published" });
        // Three held against a limit of two: the build lane writes status.build of one of them.
        assert!(check(&mirror, "helsinki", "App", "events", &published).is_ok());
        // A fourth is still refused, and so is a new one under the limit's count.
        let refused = check(&mirror, "helsinki", "App", "hsl", &published).unwrap_err();
        assert!(refused.to_string().contains("apps 4 of 2"), "{refused}");
    }

    /// PF-73, T-2654: the apps quota counts what is live, `preview` or `published`, of every class;
    /// a draft or retired App counts nothing, holds no slot beyond, and can always be written.
    #[test]
    fn only_a_live_app_counts_against_the_apps_quota() {
        let app = |name: &str, class: &str, lifecycle: Option<&str>| {
            let mut spec = json!({ "kind": class });
            if let Some(lifecycle) = lifecycle {
                spec["lifecycle"] = json!(lifecycle);
            }
            envelope("App", name, "helsinki", spec)
        };
        let mirror = mirror_with(vec![
            envelope(
                "Organization",
                "hel",
                ORG_NAMESPACE,
                json!({ "domain": "hel.fi", "projects": { "quota": { "apps": 2 } } }),
            ),
            app("alerts", "static", Some("published")),
            app("allerts", "static", Some("retired")),
            app("citiy-bike-station", "static", Some("retired")),
            app("drafted", "fullstack", None),
            app("sketch", "fullstack", Some("draft")),
            app("transport", "fullstack", Some("preview")),
        ]);
        assert_eq!(usage(&mirror, "helsinki").get("apps"), Some(&2));
        assert!(
            beyond(&mirror, "helsinki", "apps").is_empty(),
            "the two live ones fit; the retired and drafts hold no slot"
        );

        for lifecycle in ["draft", "retired"] {
            check(
                &mirror,
                "helsinki",
                "App",
                "another",
                &json!({ "lifecycle": lifecycle }),
            )
            .unwrap_or_else(|err| panic!("a {lifecycle} App counts nothing: {err}"));
        }
        check(
            &mirror,
            "helsinki",
            "App",
            "alerts",
            &json!({ "lifecycle": "retired" }),
        )
        .expect("retiring a counted App frees its slot");
        for lifecycle in ["preview", "published"] {
            let refused = check(
                &mirror,
                "helsinki",
                "App",
                "sketch",
                &json!({ "kind": "static", "lifecycle": lifecycle }),
            )
            .expect_err("a draft that goes live is counted, whatever its class");
            assert!(refused.to_string().contains("apps 3 of 2"), "{refused}");
        }

        let crowded = mirror_with(vec![
            envelope(
                "Organization",
                "hel",
                ORG_NAMESPACE,
                json!({ "domain": "hel.fi", "projects": { "quota": { "apps": 1 } } }),
            ),
            app("aaa-retired", "fullstack", Some("retired")),
            app("bbb", "fullstack", Some("published")),
            app("ccc", "static", Some("published")),
        ]);
        assert_eq!(
            beyond(&crowded, "helsinki", "apps"),
            ["ccc"],
            "a retired App ahead by name takes no slot from a live one"
        );
    }

    #[test]
    fn what_stands_beyond_the_quota_is_the_newest_by_name_and_is_not_scheduled() {
        let mirror = mirror_with(vec![
            envelope(
                "Organization",
                "bb",
                ORG_NAMESPACE,
                json!({ "domain": "bb.sk", "projects": { "quota": { "residentPipelines": 2 } } }),
            ),
            resident("alpha"),
            resident("beta"),
            resident("gamma"),
        ]);
        // The same two keep running pass after pass rather than taking turns.
        assert_eq!(beyond(&mirror, "ovzdusie", "residentPipelines"), ["gamma"]);
        assert!(beyond(&mirror, "ovzdusie", "apps").is_empty());
    }

    #[test]
    fn a_scheduled_feed_pipeline_holds_no_resident_slot() {
        let daily = |name: &str| {
            envelope(
                "Pipeline",
                name,
                "ovzdusie",
                json!({
                    "class": "auto",
                    "period": "24h",
                    "schedule": "44 0 * * *",
                    "source": { "dataSourceRef": { "kind": "DataSource", "name": "feed" } },
                    "compute": { "kind": "bloblang", "bloblang": "root = this" },
                    "targetEndpoint": "urn:ngsi-ld:Endpoint:bb.sk:ovzdusie:air"
                }),
            )
        };
        let mirror = mirror_with(vec![
            envelope(
                "Organization",
                "bb",
                ORG_NAMESPACE,
                json!({ "domain": "bb.sk", "projects": { "quota": { "residentPipelines": 1 } } }),
            ),
            envelope(
                "DataSource",
                "feed",
                "ovzdusie",
                json!({ "type": "http", "http": { "url": "https://www.bb.sk/feed.xml" } }),
            ),
            resident("alpha"),
            daily("daily-a"),
            daily("daily-b"),
        ]);
        // The run queue creates a poll for its run and deletes it after (PL-78, T-4952).
        assert_eq!(
            usage(&mirror, "ovzdusie").get("residentPipelines"),
            Some(&1)
        );
        assert!(beyond(&mirror, "ovzdusie", "residentPipelines").is_empty());
        check(
            &mirror,
            "ovzdusie",
            "Pipeline",
            "daily-c",
            &daily("daily-c").spec,
        )
        .expect("a daily feed is not resident");
        // The cap still holds for what waits for pushes.
        let refused = check(
            &mirror,
            "ovzdusie",
            "Pipeline",
            "beta",
            &resident("beta").spec,
        )
        .expect_err("a second resident pipeline");
        assert!(
            refused.to_string().contains("residentPipelines 2 of 1"),
            "{refused}"
        );
    }

    #[test]
    fn an_endpoint_may_not_ask_for_a_rate_above_what_the_project_may_serve() {
        let mirror = mirror_with(vec![envelope(
            "Organization",
            "bb",
            ORG_NAMESPACE,
            json!({ "domain": "bb.sk", "projects": { "quota": { "requestsPerMinute": 120 } } }),
        )]);
        let endpoint = |rate: u64| {
            json!({
                "contextSpaceRef": "ovzdusie",
                "audience": "organization",
                "rateLimits": { "requestsPerMinute": rate }
            })
        };
        check(&mirror, "ovzdusie", "Endpoint", "air", &endpoint(120)).expect("at the ceiling");
        let refused = check(&mirror, "ovzdusie", "Endpoint", "air", &endpoint(600))
            .expect_err("above the ceiling");
        assert!(
            refused.to_string().contains("requestsPerMinute 600 of 120"),
            "{refused}"
        );
    }

    #[test]
    fn a_dimension_nobody_limited_never_refuses_anything() {
        let mirror = mirror_with(vec![resident("one"), resident("two"), resident("three")]);
        check(
            &mirror,
            "ovzdusie",
            "Pipeline",
            "four",
            &resident("four").spec,
        )
        .expect("no quota, no limit");
    }

    #[test]
    fn only_a_public_endpoint_counts_against_the_public_endpoint_quota() {
        let mirror = mirror_with(vec![
            envelope(
                "Organization",
                "bb",
                ORG_NAMESPACE,
                json!({ "domain": "bb.sk", "projects": { "quota": { "publicEndpoints": 1 } } }),
            ),
            envelope(
                "Endpoint",
                "air",
                "ovzdusie",
                json!({ "contextSpaceRef": "ovzdusie", "slug": "zt4qm7ge2xdv6ksb3ncf5arw2y", "audience": "public" }),
            ),
        ]);
        // An internal one is not counted, however many there are.
        check(
            &mirror,
            "ovzdusie",
            "Endpoint",
            "traffic",
            &json!({ "contextSpaceRef": "ovzdusie", "audience": "organization" }),
        )
        .expect("an internal endpoint");

        let refused = check(
            &mirror,
            "ovzdusie",
            "Endpoint",
            "traffic",
            &json!({ "contextSpaceRef": "ovzdusie", "audience": "public" }),
        )
        .expect_err("the second public endpoint");
        assert!(
            refused.to_string().contains("publicEndpoints 2 of 1"),
            "{refused}"
        );
    }

    /// PL-116, S-22: the stricter floor holds, a slower clock passes, and the refusal names the
    /// field, the floor and where it is set.
    #[test]
    fn a_pipeline_faster_than_the_floor_is_refused_naming_it() {
        let mirror = mirror_with(vec![
            envelope(
                "Organization",
                "bb",
                ORG_NAMESPACE,
                json!({ "domain": "bb.sk", "projects": { "quota": { "minPipelinePeriodSeconds": 300 } } }),
            ),
            envelope(
                "Project",
                "ovzdusie",
                ORG_NAMESPACE,
                json!({ "organizationRef": { "name": "bb" }, "quotas": { "minPipelinePeriodSeconds": 900 } }),
            ),
        ]);
        let poll = |period: &str, schedule: Option<&str>| {
            let mut spec = json!({ "class": "auto", "period": period, "source": { "dataSourceRef": { "kind": "DataSource", "name": "mqtt" } } });
            if let Some(cron) = schedule {
                spec = json!({ "class": "scheduled", "schedule": cron, "source": { "dataSourceRef": { "kind": "DataSource", "name": "mqtt" } } });
            }
            check(&mirror, "ovzdusie", "Pipeline", "counters", &spec)
        };
        assert!(poll("15m", None).is_ok(), "on the floor passes");
        assert!(poll("1h", None).is_ok());
        let Err(ApiError::Unprocessable { errors, .. }) = poll("5m", None) else {
            panic!("the project's stricter floor of 900 s holds over the organization's 300 s");
        };
        assert!(
            errors[0]
                .starts_with("spec.period: runs every 300 s, below the minimum period of 900 s")
                && errors[0].contains("spec.quotas.minPipelinePeriodSeconds of project ovzdusie"),
            "{errors:?}"
        );
        let Err(ApiError::Unprocessable { errors, .. }) = poll("", Some("*/5 * * * *")) else {
            panic!("a schedule every five minutes is under the floor too");
        };
        assert!(
            errors[0].starts_with("spec.schedule: runs every 300 s"),
            "{errors:?}"
        );
        assert!(poll("", Some("*/15 * * * *")).is_ok());
        assert!(
            check(
                &mirror,
                "doprava",
                "Pipeline",
                "x",
                &json!({ "class": "auto", "period": "1m", "source": { "dataSourceRef": { "kind": "DataSource", "name": "mqtt" } } })
            )
            .is_err(),
            "a project without its own floor keeps the organization's"
        );
    }
}
