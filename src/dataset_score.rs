//! A dataset's quality score (T-5112): one number from 0 to 100 and the four parts it is the mean
//! of, each with its reason and the time it was measured, so a reader tells a fresh, valid
//! dataset from a stale one and a publisher sees what to fix.
//!
//! - **freshness**: each type a pipeline promises a schedule for, as the daily quality run found
//!   it, and each writing pipeline as the health log judged it at its last beat: a pipeline that
//!   fails or writes nothing makes its types late within one beat ([`crate::health_log::EVERY`]),
//!   not at the next daily run.
//! - **completeness**: how many entities carry each required field of the model, from the daily
//!   run's per-type counts. A field the Endpoint's projection hides is not counted.
//! - **validity**: the entities that follow the model's SHACL shapes over those checked.
//! - **links**: the Endpoint's surfaces as the health probes found them; a slow one counts half.
//!
//! A part nothing measures yet has no score and stays out of the mean. A reader of the public
//! catalogue gets counts and dates only ([`DatasetScore::public`]): no field, type, rule or
//! pipeline name and no fix, since a projection may hide a field and the pipelines are the
//! project's. Values never reach a score at all: it is built from counts.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::health_log::{Check, Kind, Status};
use crate::quality::{FreshState, SpaceQuality};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Part {
    Freshness,
    Completeness,
    Validity,
    Links,
}

/// A message of the score: its key under `quality.score.say` in the UI's locales with the
/// values it names, and its English.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(as = DatasetScoreText)]
pub struct Text {
    pub code: String,
    pub args: BTreeMap<String, String>,
    pub text: String,
}

const SAY: &[(&str, &str)] = &[
    ("notMeasured", "not measured yet"),
    ("noSchedule", "no pipeline promises a schedule for this data"),
    ("onTime", "{fresh} of {total} feeds are on schedule"),
    ("late", "{late} of {total} feeds are late; the newest observation is from {newest}"),
    ("lateUnread", "{late} of {total} feeds are late; none has delivered an observation yet"),
    ("noRequired", "the model marks no field required"),
    ("complete", "{complete} of {fields} required fields are filled in every entity"),
    ("valid", "{valid} of {checked} entities follow the model"),
    ("linksUp", "{up} of {total} access links answer"),
    ("fixLate", "{type} has no new data on schedule: open pipeline {pipeline}, read its newest run and use Run now"),
    ("fixFailing", "Pipeline {pipeline} fails since {since}: open it and fix the step its newest run names"),
    ("fixSilent", "Pipeline {pipeline} writes nothing since {since}: compare its source with its filter and mapping"),
    ("fixMissing", "{attribute} of {type} is missing in {missing} of {count} entities: map it in the pipeline or mark it optional in the model"),
    ("fixInvalid", "Most often {rule} at {path} ({count} entities): open the space's Quality tab, then fix the pipeline's mapping or the model"),
    ("fixLink", "{surface}: {said}"),
];

impl Text {
    fn say(code: &str, args: &[(&str, String)]) -> Self {
        let mut text = SAY
            .iter()
            .find(|(key, _)| *key == code)
            .map_or(code, |(_, english)| english)
            .to_owned();
        for (name, value) in args {
            text = text.replace(&format!("{{{name}}}"), value);
        }
        Self {
            code: code.to_owned(),
            args: args
                .iter()
                .map(|(name, value)| ((*name).to_owned(), value.clone()))
                .collect(),
            text,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = DatasetScorePart)]
pub struct ScorePart {
    pub part: Part,
    /// 0 to 100; absent while nothing measures the part.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<u8>,
    /// When the part was measured, RFC 3339.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured_at: Option<String>,
    pub reason: Text,
    /// What the publisher changes; absent in the public catalogue and when nothing is wrong.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<Text>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DatasetScore {
    /// The mean of the parts that have a score; absent while none has.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<u8>,
    /// The newest measurement among the parts, RFC 3339.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured_at: Option<String>,
    pub parts: Vec<ScorePart>,
}

/// What a score is computed from.
pub struct Inputs<'a> {
    /// The last daily quality run of the Endpoint's space.
    pub report: Option<&'a SpaceQuality>,
    /// The required fields of each type of the model, the hidden ones left out.
    pub required: &'a BTreeMap<String, Vec<String>>,
    /// The health log's current checks of the project.
    pub checks: &'a [Check],
    /// The Endpoint's name.
    pub endpoint: &'a str,
    /// The entity types the Endpoint serves; `None` serves every type of its space.
    pub types: Option<&'a BTreeSet<String>>,
}

impl Inputs<'_> {
    fn serves(&self, entity_type: &str) -> bool {
        self.types.is_none_or(|types| types.contains(entity_type))
    }
}

fn rfc3339(at: chrono::DateTime<chrono::Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// `part / whole` as a whole percentage, rounded down so 99.6 % is not shown as perfect.
fn percent(part: u64, whole: u64) -> u8 {
    part.saturating_mul(100)
        .checked_div(whole)
        .map_or(0, |share| u8::try_from(share.min(100)).unwrap_or(100))
}

fn unmeasured(part: Part) -> ScorePart {
    ScorePart {
        part,
        score: None,
        measured_at: None,
        reason: Text::say("notMeasured", &[]),
        fix: None,
    }
}

fn freshness(inputs: &Inputs) -> ScorePart {
    let Some(report) = inputs.report else {
        return unmeasured(Part::Freshness);
    };
    let rows: Vec<_> = report
        .freshness
        .iter()
        .filter(|row| row.state != FreshState::Untargeted && inputs.serves(&row.entity_type))
        .collect();
    if rows.is_empty() {
        return ScorePart {
            reason: Text::say("noSchedule", &[]),
            measured_at: Some(rfc3339(report.observed_at)),
            ..unmeasured(Part::Freshness)
        };
    }
    let runs = |pipeline: &str| {
        inputs.checks.iter().find(|check| {
            check.kind == Kind::Pipeline && check.name == pipeline && check.check == "runs"
        })
    };
    let mut measured_at = rfc3339(report.observed_at);
    let mut fix = None;
    let mut late = 0_usize;
    for row in &rows {
        let failing = runs(&row.pipeline).filter(|check| check.status != Status::Ok);
        if let Some(check) = failing {
            measured_at = measured_at.max(check.checked_at.clone());
        }
        if row.state == FreshState::Fresh && failing.is_none() {
            continue;
        }
        late += 1;
        fix.get_or_insert_with(|| match failing {
            Some(check) => Text::say(
                if check.status == Status::Error {
                    "fixFailing"
                } else {
                    "fixSilent"
                },
                &[
                    ("pipeline", row.pipeline.clone()),
                    ("since", check.since.clone()),
                ],
            ),
            None => Text::say(
                "fixLate",
                &[
                    ("type", row.entity_type.clone()),
                    ("pipeline", row.pipeline.clone()),
                ],
            ),
        });
    }
    let total = rows.len();
    let fresh = total.saturating_sub(late);
    let reason = if late == 0 {
        Text::say(
            "onTime",
            &[("fresh", fresh.to_string()), ("total", total.to_string())],
        )
    } else {
        let counts = [("late", late.to_string()), ("total", total.to_string())];
        match rows.iter().filter_map(|row| row.newest).max() {
            Some(newest) => Text::say(
                "late",
                &[
                    counts[0].clone(),
                    counts[1].clone(),
                    ("newest", rfc3339(newest)),
                ],
            ),
            None => Text::say("lateUnread", &counts),
        }
    };
    ScorePart {
        part: Part::Freshness,
        score: Some(percent(fresh as u64, total as u64)),
        measured_at: Some(measured_at),
        reason,
        fix,
    }
}

fn completeness(inputs: &Inputs) -> ScorePart {
    let Some(report) = inputs.report else {
        return unmeasured(Part::Completeness);
    };
    // (type, field, present, count) of every required field of a type the run found.
    let mut fields = Vec::new();
    for kind in report
        .types
        .iter()
        .filter(|kind| kind.count > 0 && inputs.serves(&kind.entity_type))
    {
        for field in inputs.required.get(&kind.entity_type).into_iter().flatten() {
            let present = kind
                .attributes
                .iter()
                .find(|attribute| &attribute.name == field)
                .map_or(0, |attribute| attribute.present);
            fields.push((
                &kind.entity_type,
                field,
                present.min(kind.count),
                kind.count,
            ));
        }
    }
    if fields.is_empty() {
        return ScorePart {
            reason: Text::say("noRequired", &[]),
            measured_at: Some(rfc3339(report.observed_at)),
            ..unmeasured(Part::Completeness)
        };
    }
    // each field's share in whole percent, then their mean
    let filled: u64 = fields
        .iter()
        .map(|(_, _, present, count)| u64::from(percent(*present, *count)))
        .sum();
    let complete = fields
        .iter()
        .filter(|(_, _, present, count)| present == count)
        .count();
    let fix = fields
        .iter()
        .filter(|(_, _, present, count)| present < count)
        .max_by_key(|(_, _, present, count)| count - present)
        .map(|(kind, field, present, count)| {
            Text::say(
                "fixMissing",
                &[
                    ("attribute", (*field).clone()),
                    ("type", (*kind).clone()),
                    ("missing", (count - present).to_string()),
                    ("count", count.to_string()),
                ],
            )
        });
    ScorePart {
        part: Part::Completeness,
        score: Some(percent(filled, 100 * fields.len() as u64)),
        measured_at: Some(rfc3339(report.observed_at)),
        reason: Text::say(
            "complete",
            &[
                ("complete", complete.to_string()),
                ("fields", fields.len().to_string()),
            ],
        ),
        fix,
    }
}

fn validity(inputs: &Inputs) -> ScorePart {
    let Some(report) = inputs.report.filter(|report| report.checked > 0) else {
        return unmeasured(Part::Validity);
    };
    let valid = report.checked.saturating_sub(report.invalid);
    ScorePart {
        part: Part::Validity,
        score: Some(percent(valid, report.checked)),
        measured_at: Some(rfc3339(report.observed_at)),
        reason: Text::say(
            "valid",
            &[
                ("valid", valid.to_string()),
                ("checked", report.checked.to_string()),
            ],
        ),
        fix: report
            .rules
            .iter()
            .max_by_key(|rule| rule.count)
            .filter(|_| report.invalid > 0)
            .map(|rule| {
                Text::say(
                    "fixInvalid",
                    &[
                        ("rule", rule.rule.clone()),
                        ("path", rule.path.clone()),
                        ("count", rule.count.to_string()),
                    ],
                )
            }),
    }
}

fn links(inputs: &Inputs) -> ScorePart {
    let surfaces: Vec<&Check> = inputs
        .checks
        .iter()
        .filter(|check| {
            check.kind == Kind::Endpoint
                && check.name == inputs.endpoint
                && check.check != "reconcile"
        })
        .collect();
    if surfaces.is_empty() {
        return unmeasured(Part::Links);
    }
    // in halves: a slow surface is half an answer
    let weight = |check: &&Check| match check.status {
        Status::Ok => 2,
        Status::Warn => 1,
        Status::Error => 0,
    };
    let up = surfaces
        .iter()
        .filter(|check| check.status != Status::Error)
        .count();
    let fix = surfaces
        .iter()
        .min_by_key(|check| check.status)
        .filter(|check| check.status != Status::Ok)
        .map(|check| {
            Text::say(
                "fixLink",
                &[
                    ("surface", check.check.clone()),
                    ("said", check.detail.text.clone()),
                ],
            )
        });
    ScorePart {
        part: Part::Links,
        score: Some(percent(
            surfaces.iter().map(weight).sum(),
            2 * surfaces.len() as u64,
        )),
        measured_at: surfaces.iter().map(|check| check.checked_at.clone()).max(),
        reason: Text::say(
            "linksUp",
            &[
                ("up", up.to_string()),
                ("total", surfaces.len().to_string()),
            ],
        ),
        fix,
    }
}

/// The score of one dataset, in the publisher's form.
pub fn score(inputs: &Inputs) -> DatasetScore {
    let parts = vec![
        freshness(inputs),
        completeness(inputs),
        validity(inputs),
        links(inputs),
    ];
    let scored: Vec<u8> = parts.iter().filter_map(|part| part.score).collect();
    DatasetScore {
        score: (!scored.is_empty()).then(|| {
            let sum: u64 = scored.iter().map(|s| u64::from(*s)).sum();
            percent(sum, 100 * scored.len() as u64)
        }),
        measured_at: parts
            .iter()
            .filter_map(|part| part.measured_at.clone())
            .max(),
        parts,
    }
}

impl DatasetScore {
    /// The form anyone may read: the fixes go, and so does every name a reason holds.
    pub fn public(mut self) -> Self {
        for part in &mut self.parts {
            part.fix = None;
        }
        self
    }
}

/// The required fields of each class of a space's compiled model, less those `hidden`.
pub fn required(
    schema: &crate::pipeline_validation::ModelSchema,
    hidden: &[String],
) -> BTreeMap<String, Vec<String>> {
    schema
        .classes
        .keys()
        .filter_map(|class| {
            let fields: Vec<String> = schema
                .attributes(class)?
                .attributes
                .into_iter()
                .filter(|attribute| attribute.required && !hidden.contains(&attribute.name))
                .map(|attribute| attribute.name)
                .collect();
            Some((class.clone(), fields))
        })
        .collect()
}

/// The entity types `spec` (an Endpoint's) serves: the `entityTypes` it lists, else the types its
/// granting Policies name (`policyRef`/`policyRefs`, else every Policy of `space`); `None` when
/// one of those Policies names no type, so the whole space is served.
fn served_types(
    policies: &[crate::resource::ResourceEnvelope],
    space: &str,
    spec: &serde_json::Value,
) -> Option<BTreeSet<String>> {
    let strings = |value: &serde_json::Value| -> Vec<String> {
        value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect()
    };
    let listed = strings(&spec["entityTypes"]);
    if !listed.is_empty() {
        return Some(listed.into_iter().collect());
    }
    let refs: Vec<String> = spec["policyRef"]
        .as_str()
        .map(|one| vec![one.to_owned()])
        .unwrap_or_else(|| strings(&spec["policyRefs"]))
        .iter()
        .map(|urn| urn.rsplit(':').next().unwrap_or(urn).to_owned())
        .collect();
    let grants = policies
        .iter()
        .filter(|policy| {
            crate::resource::reference_name(&policy.spec["contextSpaceRef"]) == Some(space)
        })
        .filter(|policy| refs.is_empty() || refs.contains(&policy.metadata.name))
        // a Policy is one rule or its `rules[]`, never both (PolicySpec::check_shape)
        .flat_map(|policy| match policy.spec["rules"].as_array() {
            Some(rules) => rules.iter().collect::<Vec<_>>(),
            None => vec![&policy.spec],
        })
        .filter(|grant| {
            !grant["effect"]
                .as_str()
                .is_some_and(|e| e.starts_with("prohibit"))
        });
    let mut types = BTreeSet::new();
    let mut any = false;
    for grant in grants {
        any = true;
        let named: Vec<&str> = grant["information"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|info| info["entities"].as_array().into_iter().flatten())
            .filter_map(|entity| entity["type"].as_str())
            .collect();
        if named.is_empty() {
            return None;
        }
        types.extend(named.into_iter().map(str::to_owned));
    }
    any.then_some(types)
}

/// The score of a Context-Space-backed Endpoint from what this Portal holds now: the space's
/// daily report, its model and the project's health checks.
pub async fn of_endpoint(
    state: &crate::state::AppState,
    project: &str,
    endpoint: &crate::resource::ResourceEnvelope,
) -> DatasetScore {
    let space = crate::resource::reference_name(&endpoint.spec["contextSpaceRef"]);
    let report = space.and_then(|space| state.quality.get(project, space));
    let hidden: Vec<String> = endpoint.spec["projection"]["hiddenAttributes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|name| name.as_str().map(str::to_owned))
        .collect();
    let required_fields = space
        .and_then(|space| state.model_schemas.get(project, space))
        .map(|schema| required(&schema, &hidden))
        .unwrap_or_default();
    let policies = state
        .mirror
        .list(project, "Policy", &crate::store::ListOptions::default())
        .items;
    let types = space.and_then(|space| served_types(&policies, space, &endpoint.spec));
    let checks = state
        .health_log
        .current(project, chrono::Utc::now())
        .await
        .unwrap_or_else(|err| {
            tracing::warn!(error = %err, project, "the health log could not be read for a score");
            Vec::new()
        });
    score(&Inputs {
        report: report.as_ref(),
        required: &required_fields,
        checks: &checks,
        endpoint: &endpoint.metadata.name,
        types: types.as_ref(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health_log::Text as HealthText;
    use crate::quality::{AttributeQuality, Freshness, RuleCount, TypeQuality};

    fn at(text: &str) -> chrono::DateTime<chrono::Utc> {
        text.parse().expect("a time")
    }

    fn report() -> SpaceQuality {
        SpaceQuality {
            observed_at: at("2026-10-22T03:00:00Z"),
            checked: 200,
            invalid: 10,
            truncated: false,
            rules: vec![RuleCount {
                rule: "sh:MinCountConstraintComponent".into(),
                path: "temperature".into(),
                count: 10,
                examples: vec!["urn:ngsi-ld:WeatherObserved:a".into()],
            }],
            freshness: vec![Freshness {
                pipeline: "weather".into(),
                entity_type: "WeatherObserved".into(),
                newest: Some(at("2026-10-22T02:50:00Z")),
                target_seconds: Some(3_600),
                state: FreshState::Fresh,
                paused: false,
            }],
            types: vec![TypeQuality {
                entity_type: "WeatherObserved".into(),
                count: 200,
                newest: None,
                attributes: vec![
                    attribute("temperature", 190),
                    attribute("dateObserved", 200),
                    attribute("secret", 200),
                ],
            }],
        }
    }

    fn attribute(name: &str, present: u64) -> AttributeQuality {
        AttributeQuality {
            name: name.into(),
            present,
            min: Some(-3.0),
            max: Some(41.5),
            outliers: 0,
            outlier_examples: Vec::new(),
        }
    }

    fn required_fields() -> BTreeMap<String, Vec<String>> {
        BTreeMap::from([(
            "WeatherObserved".to_owned(),
            vec!["temperature".to_owned(), "dateObserved".to_owned()],
        )])
    }

    fn check(kind: Kind, name: &str, check: &str, status: Status) -> Check {
        Check {
            project: "bbsk".into(),
            kind,
            name: name.into(),
            check: check.into(),
            status,
            since: "2026-10-22T08:00:00Z".into(),
            last_ok: None,
            checked_at: "2026-10-22T08:04:00Z".into(),
            detail: HealthText {
                text: format!("{check} said something"),
                ..HealthText::default()
            },
            fix: HealthText::default(),
            evidence_url: String::new(),
            history: Vec::new(),
        }
    }

    fn part(score: &DatasetScore, part: Part) -> &ScorePart {
        score
            .parts
            .iter()
            .find(|p| p.part == part)
            .expect("every part is listed")
    }

    fn scored(report: Option<&SpaceQuality>, checks: &[Check]) -> DatasetScore {
        score(&Inputs {
            report,
            required: &required_fields(),
            checks,
            endpoint: "weather",
            types: None,
        })
    }

    #[test]
    fn nothing_measured_is_no_score_and_every_part_says_so() {
        let score = scored(None, &[]);
        assert_eq!(score.score, None);
        assert_eq!(score.parts.len(), 4);
        assert!(score
            .parts
            .iter()
            .all(|p| p.score.is_none() && p.reason.code == "notMeasured"));
    }

    #[test]
    fn freshness_is_on_time_until_the_writing_pipeline_fails_at_the_next_beat() {
        let report = report();
        let ok = [check(Kind::Pipeline, "weather", "runs", Status::Ok)];
        let fresh = scored(Some(&report), &ok);
        assert_eq!(part(&fresh, Part::Freshness).score, Some(100));
        assert_eq!(part(&fresh, Part::Freshness).reason.code, "onTime");
        assert_eq!(part(&fresh, Part::Freshness).fix, None);

        let failing = [check(Kind::Pipeline, "weather", "runs", Status::Error)];
        let late = scored(Some(&report), &failing);
        let freshness = part(&late, Part::Freshness);
        assert_eq!(freshness.score, Some(0));
        assert_eq!(freshness.reason.code, "late");
        assert_eq!(
            freshness.measured_at.as_deref(),
            Some("2026-10-22T08:04:00Z")
        );
        let fix = freshness.fix.as_ref().expect("a fix");
        assert_eq!(fix.code, "fixFailing");
        assert!(fix
            .text
            .contains("weather fails since 2026-10-22T08:00:00Z"));
        assert!(late.score < fresh.score);
    }

    #[test]
    fn a_stale_type_of_the_daily_run_is_late_and_untargeted_ones_do_not_count() {
        let mut report = report();
        report.freshness[0].state = FreshState::Stale;
        report.freshness.push(Freshness {
            pipeline: "other".into(),
            entity_type: "Other".into(),
            newest: None,
            target_seconds: None,
            state: FreshState::Untargeted,
            paused: false,
        });
        let score = scored(Some(&report), &[]);
        let freshness = part(&score, Part::Freshness);
        assert_eq!(freshness.score, Some(0));
        assert_eq!(freshness.reason.args["total"], "1");
        assert_eq!(freshness.reason.args["newest"], "2026-10-22T02:50:00Z");
        assert_eq!(
            freshness.fix.as_ref().map(|f| f.code.as_str()),
            Some("fixLate")
        );

        report.freshness.remove(0);
        let untargeted = scored(Some(&report), &[]);
        assert_eq!(part(&untargeted, Part::Freshness).score, None);
        assert_eq!(part(&untargeted, Part::Freshness).reason.code, "noSchedule");
    }

    // T-6565: a space's other feed, stale, does not lower the Endpoint whose own type is fresh.
    #[test]
    fn another_types_stale_feed_does_not_lower_an_endpoint_whose_type_is_fresh() {
        let mut report = report();
        report.freshness.push(Freshness {
            pipeline: "parking".into(),
            entity_type: "ParkingSpot".into(),
            newest: None,
            target_seconds: Some(300),
            state: FreshState::Stale,
            paused: false,
        });
        let space_wide = scored(Some(&report), &[]);
        assert_eq!(part(&space_wide, Part::Freshness).score, Some(50));

        let weather = BTreeSet::from(["WeatherObserved".to_owned()]);
        let own = score(&Inputs {
            report: Some(&report),
            required: &required_fields(),
            checks: &[],
            endpoint: "weather",
            types: Some(&weather),
        });
        let freshness = part(&own, Part::Freshness);
        assert_eq!(freshness.score, Some(100));
        assert_eq!(freshness.reason.code, "onTime");
        assert_eq!(freshness.reason.args["total"], "1");
    }

    // T-6565: a late feed with no reading yet says so in words, never "from never".
    #[test]
    fn a_late_feed_without_any_reading_says_none_has_arrived() {
        let mut report = report();
        report.freshness[0].state = FreshState::Stale;
        report.freshness[0].newest = None;
        let score = scored(Some(&report), &[]);
        let reason = &part(&score, Part::Freshness).reason;
        assert_eq!(reason.code, "lateUnread");
        assert!(!reason.args.contains_key("newest"));
        assert_eq!(
            reason.text,
            "1 of 1 feeds are late; none has delivered an observation yet"
        );
    }

    #[test]
    fn an_endpoint_serves_its_listed_types_else_its_bound_policies_types() {
        use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
        let policy = |name: &str, spec: serde_json::Value| ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: "Policy".to_owned(),
            metadata: ObjectMeta::new(name, "barcelona"),
            spec,
            status: None,
        };
        let policies = [
            policy(
                "public-charging",
                serde_json::json!({
                    "contextSpaceRef": { "kind": "ContextSpace", "name": "barcelona" },
                    "information": [{ "entities": [{ "type": "EVChargingStation" }] }],
                }),
            ),
            policy(
                "steward",
                serde_json::json!({
                    "contextSpaceRef": "barcelona",
                    "rules": [{ "information": [{ "entities": [{ "type": "ParkingSpot" }] }] }],
                }),
            ),
            policy(
                "no-parking",
                serde_json::json!({
                    "contextSpaceRef": "barcelona",
                    "effect": "prohibit",
                }),
            ),
        ];
        let set = |names: &[&str]| Some(names.iter().map(|n| (*n).to_owned()).collect());
        let served = |spec: serde_json::Value| served_types(&policies, "barcelona", &spec);
        assert_eq!(
            served(
                serde_json::json!({ "policyRef": "urn:ngsi-ld:Policy:barcelona.cat:barcelona:public-charging" })
            ),
            set(&["EVChargingStation"])
        );
        assert_eq!(
            served(serde_json::json!({})),
            set(&["EVChargingStation", "ParkingSpot"])
        );
        assert_eq!(
            served(
                serde_json::json!({ "entityTypes": ["Bus"], "policyRef": "urn:ngsi-ld:Policy:public-charging" })
            ),
            set(&["Bus"])
        );
        assert_eq!(
            served(serde_json::json!({ "policyRefs": ["urn:ngsi-ld:Policy:gone"] })),
            None
        );
        let open = [policy(
            "all",
            serde_json::json!({ "contextSpaceRef": "barcelona" }),
        )];
        assert_eq!(
            served_types(&open, "barcelona", &serde_json::json!({})),
            None
        );
    }

    #[test]
    fn completeness_counts_required_fields_only_and_names_the_most_missing_one() {
        let report = report();
        let score = scored(Some(&report), &[]);
        let completeness = part(&score, Part::Completeness);
        // (190/200 + 200/200) / 2 = 97.5 %, rounded down
        assert_eq!(completeness.score, Some(97));
        assert_eq!(completeness.reason.args["complete"], "1");
        assert_eq!(completeness.reason.args["fields"], "2");
        let fix = completeness.fix.as_ref().expect("a fix");
        assert_eq!(fix.args["attribute"], "temperature");
        assert_eq!(fix.args["missing"], "10");

        let none = score_with(&report, BTreeMap::new());
        assert_eq!(part(&none, Part::Completeness).score, None);
        assert_eq!(part(&none, Part::Completeness).reason.code, "noRequired");
    }

    // The Endpoint's score reads the space's report and model, and a hidden attribute is no
    // required field of it.
    #[tokio::test]
    async fn an_endpoint_is_scored_from_its_space_without_its_hidden_fields() {
        use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        let schema = serde_json::json!({ "definitions": { "WeatherObserved": {
            "type": "object",
            "required": ["id", "type", "temperature", "dateObserved", "secret"],
            "properties": {
                "temperature": { "type": "number", "x-ngsi-ld-kind": "Property" },
                "dateObserved": { "type": "string", "x-ngsi-ld-kind": "Property" },
                "secret": { "type": "string", "x-ngsi-ld-kind": "Property" }
            }
        }}});
        let model = crate::pipeline_validation::ModelSchema::compile(
            "weather",
            "1.0.0",
            &schema,
            &["WeatherObserved".to_owned()],
            false,
        );
        state
            .model_schemas
            .replace(std::collections::HashMap::from([(
                ("bbsk".to_owned(), "weather".to_owned()),
                std::sync::Arc::new(model),
            )]));
        state.quality.put("bbsk", "weather", report());
        let endpoint = |hidden: serde_json::Value| ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: "Endpoint".to_owned(),
            metadata: ObjectMeta::new("weather", "bbsk"),
            spec: serde_json::json!({
                "contextSpaceRef": { "kind": "ContextSpace", "name": "weather" },
                "projection": { "hiddenAttributes": hidden },
            }),
            status: None,
        };

        let hidden = of_endpoint(&state, "bbsk", &endpoint(serde_json::json!(["secret"]))).await;
        let completeness = part(&hidden, Part::Completeness);
        assert_eq!(completeness.reason.args["fields"], "2");
        assert_eq!(completeness.score, Some(97));

        let shown = of_endpoint(&state, "bbsk", &endpoint(serde_json::json!([]))).await;
        // (190 + 200 + 200) / 600 = 98.3 %
        assert_eq!(part(&shown, Part::Completeness).reason.args["fields"], "3");
        assert_eq!(part(&shown, Part::Completeness).score, Some(98));
    }

    fn score_with(report: &SpaceQuality, required: BTreeMap<String, Vec<String>>) -> DatasetScore {
        score(&Inputs {
            report: Some(report),
            required: &required,
            checks: &[],
            endpoint: "weather",
            types: None,
        })
    }

    #[test]
    fn a_required_field_no_entity_carries_is_zero_for_that_field() {
        let mut report = report();
        report.types[0].attributes.remove(0);
        let score = scored(Some(&report), &[]);
        assert_eq!(part(&score, Part::Completeness).score, Some(50));
    }

    #[test]
    fn validity_is_the_share_of_entities_that_follow_the_model() {
        let report = report();
        let score = scored(Some(&report), &[]);
        let validity = part(&score, Part::Validity);
        assert_eq!(validity.score, Some(95));
        assert_eq!(validity.reason.text, "190 of 200 entities follow the model");
        assert_eq!(
            validity.fix.as_ref().map(|f| f.args["path"].as_str()),
            Some("temperature")
        );

        let mut clean = report.clone();
        clean.invalid = 0;
        assert_eq!(part(&scored(Some(&clean), &[]), Part::Validity).fix, None);
        clean.checked = 0;
        assert_eq!(part(&scored(Some(&clean), &[]), Part::Validity).score, None);
    }

    #[test]
    fn links_are_the_endpoints_own_surfaces_a_slow_one_counting_half() {
        let checks = [
            check(Kind::Endpoint, "weather", "ngsi-ld", Status::Ok),
            check(Kind::Endpoint, "weather", "odata", Status::Warn),
            check(Kind::Endpoint, "weather", "ogc-features", Status::Error),
            check(Kind::Endpoint, "weather", "reconcile", Status::Error),
            check(Kind::Endpoint, "other", "ngsi-ld", Status::Error),
        ];
        let score = scored(None, &checks);
        let links = part(&score, Part::Links);
        assert_eq!(links.score, Some(50));
        assert_eq!(links.reason.text, "2 of 3 access links answer");
        assert_eq!(
            links.fix.as_ref().map(|f| f.args["surface"].as_str()),
            Some("ogc-features")
        );
        assert_eq!(score.score, Some(50));
    }

    #[test]
    fn the_score_is_the_mean_of_the_measured_parts() {
        let report = report();
        let checks = [
            check(Kind::Pipeline, "weather", "runs", Status::Ok),
            check(Kind::Endpoint, "weather", "ngsi-ld", Status::Ok),
        ];
        let score = scored(Some(&report), &checks);
        // (100 + 97 + 95 + 100) / 4 = 98
        assert_eq!(score.score, Some(98));
        assert_eq!(score.measured_at.as_deref(), Some("2026-10-22T08:04:00Z"));
    }

    #[test]
    fn the_public_form_holds_no_fix_no_name_and_no_value() {
        let mut report = report();
        report.freshness[0].state = FreshState::Stale;
        let checks = [check(Kind::Endpoint, "weather", "odata", Status::Error)];
        let public = scored(Some(&report), &checks).public();
        let text = serde_json::to_string(&public).expect("serializes");
        for private in [
            "weather\"",
            "temperature",
            "secret",
            "MinCount",
            "urn:ngsi-ld",
            "41.5",
            "said",
        ] {
            assert!(!text.contains(private), "{private} in {text}");
        }
        assert!(public.parts.iter().all(|p| p.fix.is_none()));
    }
}
