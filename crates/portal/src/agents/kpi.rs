//! The assistant computes an indicator (T-0583, PF-54, PF-55, UI-17): a deterministic step
//! on the Portal, like the catalog search and the endpoint proposal. The model asks for it
//! with one fenced JSON call; the Portal reads the entities through the proxy, computes the
//! aggregate, renders the `KeyPerformanceIndicator` entity with its formula and provenance,
//! and hands it to the person as a card. The Portal writes nothing: the person writes the
//! indicator through the indicator space's Endpoint with their own session (AG-20).

use jc_core::kpi::{indicator_urn, kpi_space, IndicatorValue, KeyPerformanceIndicator, Period};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agents::share::TOOL_FENCE;
use crate::resource::is_dns1123;
use crate::state::AppState;
use crate::store::ListOptions;

/// How the attribute's values fold into one number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Agg {
    Avg,
    Sum,
    Count,
    Min,
    Max,
}

impl Agg {
    fn as_str(self) -> &'static str {
        match self {
            Self::Avg => "avg",
            Self::Sum => "sum",
            Self::Count => "count",
            Self::Min => "min",
            Self::Max => "max",
        }
    }
}

/// The model's call: `{"tool": "compute_kpi", ...}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputeKpi {
    /// The indicator's name, the `{localId}` of its URN.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The entity type the value is computed over.
    #[serde(rename = "type")]
    pub entity_type: String,
    /// The attribute folded; ignored by `count`.
    #[serde(default)]
    pub attribute: String,
    pub agg: Agg,
    /// A UN/CEFACT common code for the value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// An NGSI-LD `q` narrowing the entities read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// The endpoint read, by name; the conversation's endpoint of the type when absent (AG-76).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

/// The tool call in a model answer, when the answer is one.
pub fn tool_call(answer: &str) -> Option<Result<ComputeKpi, String>> {
    for fence in TOOL_FENCE.captures_iter(answer) {
        let Ok(value) = serde_json::from_str::<Value>(&fence[1]) else {
            continue;
        };
        if value.get("tool").and_then(Value::as_str) != Some("compute_kpi") {
            continue;
        }
        return Some(
            serde_json::from_value::<ComputeKpi>(value)
                .map_err(|err| format!("the compute_kpi call does not parse: {err}")),
        );
    }
    None
}

/// The number a keyValues attribute holds, when it holds one.
fn number_of(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        Value::Object(o) => o.get("value").and_then(number_of),
        _ => None,
    }
}

/// The aggregate over the rows, and how many rows carried a number (every row for `count`).
pub fn compute(rows: &[Value], attribute: &str, agg: Agg) -> (Option<f64>, usize) {
    if agg == Agg::Count {
        return (Some(rows.len() as f64), rows.len());
    }
    let numbers: Vec<f64> = rows
        .iter()
        .filter_map(|row| row.get(attribute).and_then(number_of))
        .collect();
    let n = numbers.len();
    if n == 0 {
        return (None, 0);
    }
    let value = match agg {
        Agg::Sum => numbers.iter().sum(),
        Agg::Avg => numbers.iter().sum::<f64>() / n as f64,
        Agg::Min => numbers.iter().copied().fold(f64::INFINITY, f64::min),
        Agg::Max => numbers.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        Agg::Count => unreachable!("handled above"),
    };
    (Some(value), n)
}

/// `avg(pm10) over AirQualityObserved where pm10>0`: the formula as the entity records it.
pub fn formula(params: &ComputeKpi) -> String {
    let mut text = match params.agg {
        Agg::Count => format!("count({})", params.entity_type),
        agg => format!(
            "{}({}) over {}",
            agg.as_str(),
            params.attribute,
            params.entity_type
        ),
    };
    if let Some(q) = params.q.as_deref().filter(|q| !q.trim().is_empty()) {
        text.push_str(&format!(" where {q}"));
    }
    text
}

/// What the run knows about where the sources came from and who computed the value.
pub struct Provenance<'a> {
    pub org_domain: &'a str,
    pub project: &'a str,
    /// The Endpoint the entities were read through: its space and its name.
    pub endpoint_space: &'a str,
    pub endpoint_name: &'a str,
    pub run_id: &'a str,
    /// RFC 3339, the instant of the computation.
    pub now: &'a str,
}

/// The indicator entity for a computed value (PF-54, PF-55), validated.
pub fn entity(
    params: &ComputeKpi,
    value: f64,
    provenance: &Provenance,
) -> Result<KeyPerformanceIndicator, String> {
    let urn = indicator_urn(provenance.org_domain, provenance.project, &params.name)
        .map_err(|e| format!("the indicator name does not make a URN: {e}"))?;
    let source = jc_core::Urn::new(
        "Endpoint",
        provenance.org_domain,
        provenance.endpoint_space,
        provenance.endpoint_name,
    )
    .map_err(|e| format!("the source endpoint has no URN: {e}"))?;
    let computed_by = jc_core::Urn::new(
        "AgentRun",
        provenance.org_domain,
        provenance.project,
        provenance.run_id,
    )
    .map_err(|e| format!("the run has no URN: {e}"))?;
    let indicator = KeyPerformanceIndicator::new(
        &urn,
        &formula(params),
        IndicatorValue::Number(value),
        params.unit.as_deref().filter(|u| !u.trim().is_empty()),
        Period {
            start: provenance.now.to_owned(),
            end: provenance.now.to_owned(),
        },
        provenance.now,
        jc_core::kpi::Provenance::to(source.to_string()),
        jc_core::kpi::Provenance::to(computed_by.to_string()),
    );
    indicator.validate().map_err(|e| e.to_string())?;
    Ok(indicator)
}

/// The Endpoint of the project's indicator space, when the space has one: its slug and name.
pub fn kpi_endpoint(state: &AppState, project: &str) -> Option<(String, String)> {
    if !is_dns1123(project) {
        return None;
    }
    let space = kpi_space(project);
    state
        .mirror
        .list(project, "Endpoint", &ListOptions::default())
        .items
        .into_iter()
        .find(|env| {
            crate::api::assistant::ref_name(&env.spec["contextSpaceRef"]).as_deref()
                == Some(space.as_str())
        })
        .and_then(|env| {
            env.spec["slug"]
                .as_str()
                .map(|slug| (slug.to_owned(), env.metadata.name.clone()))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn params(agg: Agg) -> ComputeKpi {
        ComputeKpi {
            name: "average-pm10".into(),
            title: None,
            entity_type: "AirQualityObserved".into(),
            attribute: "pm10".into(),
            agg,
            unit: Some("GQ".into()),
            q: None,
            endpoint: None,
        }
    }

    #[test]
    fn the_call_is_read_out_of_the_fence_and_anything_else_is_not() {
        let answer = "Here is the indicator.\n\n```json\n{\"tool\":\"compute_kpi\",\"name\":\"average-pm10\",\"type\":\"AirQualityObserved\",\"attribute\":\"pm10\",\"agg\":\"avg\",\"unit\":\"GQ\"}\n```";
        let call = tool_call(answer).expect("a call").expect("parses");
        assert_eq!(call.agg, Agg::Avg);
        assert_eq!(call.entity_type, "AirQualityObserved");
        assert!(tool_call("```json\n{\"tool\":\"propose_endpoint\"}\n```").is_none());
        assert!(tool_call("no fence").is_none());
        assert!(
            tool_call("```json\n{\"tool\":\"compute_kpi\",\"name\":\"x\"}\n```")
                .expect("a call")
                .is_err()
        );
    }

    #[test]
    fn aggregates_fold_numbers_and_skip_what_is_not_one() {
        let rows = vec![
            json!({ "id": "a", "pm10": 10 }),
            json!({ "id": "b", "pm10": "20" }),
            json!({ "id": "c", "pm10": { "value": 30 } }),
            json!({ "id": "d", "pm10": "n/a" }),
            json!({ "id": "e" }),
        ];
        assert_eq!(compute(&rows, "pm10", Agg::Avg), (Some(20.0), 3));
        assert_eq!(compute(&rows, "pm10", Agg::Sum), (Some(60.0), 3));
        assert_eq!(compute(&rows, "pm10", Agg::Min), (Some(10.0), 3));
        assert_eq!(compute(&rows, "pm10", Agg::Max), (Some(30.0), 3));
        assert_eq!(compute(&rows, "pm10", Agg::Count), (Some(5.0), 5));
        assert_eq!(compute(&rows, "pm25", Agg::Avg), (None, 0));
        assert_eq!(compute(&[], "pm10", Agg::Count), (Some(0.0), 0));
    }

    #[test]
    fn the_formula_and_the_entity_carry_the_provenance() {
        let mut p = params(Agg::Avg);
        p.q = Some("pm10>0".into());
        assert_eq!(
            formula(&p),
            "avg(pm10) over AirQualityObserved where pm10>0"
        );
        assert_eq!(formula(&params(Agg::Count)), "count(AirQualityObserved)");
        let provenance = Provenance {
            org_domain: "hel.fi",
            project: "helsinki",
            endpoint_space: "helsinki",
            endpoint_name: "helsinki-all",
            run_id: "3f9c2a1e",
            now: "2026-09-13T08:00:12Z",
        };
        let kpi = entity(&p, 18.4, &provenance).expect("an indicator");
        assert_eq!(
            kpi.id,
            "urn:ngsi-ld:KeyPerformanceIndicator:hel.fi:helsinki-kpi:average-pm10"
        );
        let json = kpi.to_json();
        assert_eq!(json["currentValue"]["value"], 18.4);
        assert_eq!(json["currentValue"]["unitCode"], "GQ");
        assert_eq!(
            json["derivedFrom"]["value"],
            "urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all"
        );
        assert_eq!(
            json["computedBy"]["value"],
            "urn:ngsi-ld:AgentRun:hel.fi:helsinki:3f9c2a1e"
        );
        let mut bad = params(Agg::Avg);
        bad.name = "not a name".into();
        assert!(entity(&bad, 1.0, &provenance).is_err());
    }
}
