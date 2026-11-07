// SPDX-License-Identifier: EUPL-1.2
//! `/q/quality/{tenant}`: the `maxAge` freshness rules of a Tenant's space and
//! the sweep that quarantines what they find stale (T-4018, platform
//! ADR-N-046 2.5 and 2.6, DM-90, DM-91).
//!
//! One document per Tenant, stored beside the view and retention documents
//! and reloaded with them. An entry names a rule, an expanded type and
//! attribute IRI and a `maxAge`; the entries of one name are one rule (a
//! space's rule reaches the broker once per expansion of its type, and an
//! Entity carries one mark per name), judged together. Once a minute every
//! pod sweeps: an Entity of one of the rule's types whose attribute for that
//! type has an instance observed longer ago than `maxAge` gets
//! `quarantinedAt` and the rule's name in `quarantineRules`; the name goes
//! again when no instance is stale (or the attribute is gone), and both
//! Properties go with the last name. Marking twice what is already marked
//! changes nothing, so pods racing each other agree.
//!
//! The mark narrows, never widens: the Portal's permits hide an Entity that
//! carries `quarantinedAt` from every role but the space's owner and steward.
//! It is maintenance metadata, not a client write: the Entity's `modifiedAt`
//! stays as it was (so the mark never holds off retention), and it leaves no
//! history and sends no notification.

use super::Reloader;
use antares_api::AppState;
use antares_model::TenantId;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Weak;
use std::time::Duration;

/// The id prefix a Tenant's quality document is stored under, beside the
/// view documents; a view id is `[a-z0-9-]`, so none shares a key with it.
pub const QUALITY_ID_PREFIX: &str = "quality:";
/// The most rules one Tenant's document holds (a space holds at most 64,
/// DM-92; a Tenant may hold several spaces' worth through a hub).
pub const MAX_QUALITY_RULES: usize = 256;
/// The largest quality document `PUT` accepts, in bytes.
pub const MAX_QUALITY_BYTES: usize = 64 * 1024;
/// The Property that marks a quarantined Entity (DM-90), under the NGSI-LD
/// default context: the expansion of `quarantinedAt` in a payload whose
/// @context does not define it, which is how the gateway's write path marks.
pub const QUARANTINED_AT: &str = "https://uri.etsi.org/ngsi-ld/default-context/quarantinedAt";
/// The Property naming the rules that quarantined an Entity (DM-90).
pub const QUARANTINE_RULES: &str = "https://uri.etsi.org/ngsi-ld/default-context/quarantineRules";
/// Entities one scan reads per page, and pages one rule reads per sweep:
/// a type larger than that is judged over several minutes, never in one
/// long read.
const PAGE: usize = 1000;
const ROUNDS: usize = 50;
/// The bounds of a `maxAge`: one second (the platform's shortest) to ten
/// years.
const SHORTEST: Duration = Duration::from_secs(1);
const LONGEST: Duration = Duration::from_hours(87_600);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleDoc {
    name: String,
    #[serde(rename = "type")]
    type_iri: String,
    attribute: String,
    #[serde(rename = "maxAge")]
    max_age: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    rules: Vec<RuleDoc>,
}

/// One rule as the sweep runs it: every entry of one name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// The name the Portal lists the rule by, written into `quarantineRules`.
    pub name: String,
    /// The expanded Entity types and, for each, the expanded attribute whose
    /// `observedAt` is judged, in the order the document lists them.
    pub checks: Vec<(String, String)>,
    /// How old an observation may be.
    pub max_age: Duration,
}

/// Every Tenant's rules, as the last reload found them.
pub type Rules = Vec<(TenantId, Vec<Rule>)>;

/// Whether `raw` is an absolute IRI as the store holds types and attributes:
/// a short name would depend on a @context the document does not carry.
fn expanded(raw: &str) -> bool {
    raw.split_once(':')
        .is_some_and(|(scheme, rest)| !scheme.is_empty() && !rest.is_empty())
        && !raw.contains(char::is_whitespace)
}

/// A rule name as the platform writes it: a DNS-1123 label.
fn label(raw: &str) -> bool {
    (1..=63).contains(&raw.len())
        && raw
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !raw.starts_with('-')
        && !raw.ends_with('-')
}

/// The rules of one document, its entries grouped by name, or the first
/// reason it holds none.
///
/// # Errors
/// A document that is not one, a name that is no DNS-1123 label, one name
/// with two `maxAge` values or the same type and attribute twice, a type or attribute that is no absolute IRI, a `maxAge` that is
/// no duration of one second to ten years, or more than
/// [`MAX_QUALITY_RULES`] rules.
pub fn parse(body: &[u8]) -> Result<Vec<Rule>, String> {
    let doc: Document =
        serde_json::from_slice(body).map_err(|e| format!("not a quality document: {e}"))?;
    if doc.rules.len() > MAX_QUALITY_RULES {
        return Err(format!(
            "a document holds at most {MAX_QUALITY_RULES} rules"
        ));
    }
    let mut rules: Vec<Rule> = Vec::new();
    for r in doc.rules {
        if !label(&r.name) {
            return Err(format!("name {:?} is not a DNS-1123 label", r.name));
        }
        for (field, iri) in [("type", &r.type_iri), ("attribute", &r.attribute)] {
            if !expanded(iri) {
                return Err(format!("{field} {iri:?} is not an expanded IRI"));
            }
        }
        let max_age = super::retention::parse_duration_within(&r.max_age, SHORTEST, LONGEST)?;
        let check = (r.type_iri, r.attribute);
        match rules.iter_mut().find(|known| known.name == r.name) {
            Some(known) if known.max_age != max_age => {
                return Err(format!("name {:?} has two maxAge values", r.name));
            }
            Some(known) if known.checks.contains(&check) => {
                return Err(format!(
                    "name {:?} lists type {:?} and attribute {:?} twice",
                    r.name, check.0, check.1
                ));
            }
            Some(known) => known.checks.push(check),
            None => rules.push(Rule {
                name: r.name,
                checks: vec![check],
                max_age,
            }),
        }
    }
    Ok(rules)
}

/// The rules of the stored quality documents among `docs`, by Tenant; a
/// stored document that no longer parses is skipped with a warning, so the
/// sweep marks nothing for that Tenant rather than guess.
pub(super) fn load<'a>(docs: impl Iterator<Item = (&'a str, &'a Value)>) -> Rules {
    super::tenant_rules(docs, parse, "quality")
}

/// What one sweep changed for one rule of one Tenant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changed {
    /// The Tenant.
    pub tenant: String,
    /// The rule's expanded types, comma-separated.
    pub type_iri: String,
    /// The rule's name.
    pub rule: String,
    /// `quarantine` (marked) or `lift` (the mark removed).
    pub outcome: &'static str,
    /// How many Entities.
    pub count: u64,
}

/// The instances of an internal attribute: an array of instance objects, or
/// one object.
fn instances(attribute: Option<&Value>) -> Vec<&Value> {
    match attribute {
        Some(Value::Array(all)) => all.iter().collect(),
        Some(one @ Value::Object(_)) => vec![one],
        _ => Vec::new(),
    }
}

/// Whether, for one of `rule`'s types the Entity has, some instance of that
/// type's attribute was observed before `cut`. An instance without a
/// readable `observedAt` is not judged: nothing says it is old.
fn stale(doc: &Value, rule: &Rule, cut: chrono::DateTime<chrono::Utc>) -> bool {
    let types = doc.get("type");
    let has = |ty: &str| match types {
        Some(Value::Array(all)) => all.iter().any(|t| t.as_str() == Some(ty)),
        Some(Value::String(one)) => one == ty,
        _ => false,
    };
    rule.checks
        .iter()
        .filter(|(ty, _)| has(ty))
        .flat_map(|(_, attribute)| instances(doc.get(attribute)))
        .any(|inst| {
            inst.get("observedAt")
                .and_then(Value::as_str)
                // A comma may stand for the fraction's point (ISO 8601).
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s.replacen(',', ".", 1)).ok())
                .is_some_and(|at| at < cut)
        })
}

/// The rule names an Entity's `quarantineRules` holds.
fn marked_by(doc: &Value) -> Vec<String> {
    instances(doc.get(QUARANTINE_RULES))
        .first()
        .and_then(|inst| inst.get("value"))
        .map(|v| match v {
            Value::Array(names) => names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            Value::String(one) => vec![one.clone()],
            _ => Vec::new(),
        })
        .unwrap_or_default()
}

/// One Property instance as the write path stores it, stamped at `ts`.
fn property(value: &Value, ts: &str) -> Value {
    json!([{ "type": "Property", "value": value, "createdAt": ts, "modifiedAt": ts }])
}

/// What `rule` would change on `doc` as `cut` judges it now: `Some(true)`
/// mark, `Some(false)` lift, `None` when the Entity already says what the
/// rule says.
fn verdict(doc: &Value, rule: &Rule, cut: chrono::DateTime<chrono::Utc>) -> Option<bool> {
    let is_stale = stale(doc, rule, cut);
    (is_stale != marked_by(doc).contains(&rule.name)).then_some(is_stale)
}

/// Mark or lift `rule` on `doc` as `cut` judges it now, stamped `ts`; what
/// [`verdict`] says, and nothing written on `None` or on a document that is
/// no object.
fn judge(
    doc: &mut Value,
    rule: &Rule,
    cut: chrono::DateTime<chrono::Utc>,
    ts: &str,
) -> Option<bool> {
    let is_stale = verdict(doc, rule, cut)?;
    let mut names = marked_by(doc);
    let object = doc.as_object_mut()?;
    if is_stale {
        names.push(rule.name.clone());
        object
            .entry(QUARANTINED_AT)
            .or_insert_with(|| property(&json!({ "@type": "DateTime", "@value": ts }), ts));
    } else {
        names.retain(|n| *n != rule.name);
    }
    if names.is_empty() {
        object.remove(QUARANTINED_AT);
        object.remove(QUARANTINE_RULES);
    } else {
        object.insert(QUARANTINE_RULES.to_owned(), property(&json!(names), ts));
    }
    Some(is_stale)
}

/// One sweep at `now` over the rules the last reload found. Logs and counts
/// how many Entities each rule marked or lifted, never which.
pub(super) async fn sweep(r: &Reloader, now: chrono::DateTime<chrono::Utc>) -> Vec<Changed> {
    let rules = r.quality_rules();
    let mut out = Vec::new();
    for (t, list) in rules.iter() {
        for rule in list {
            for changed in judge_rule(&r.state, t, rule, now).await {
                metrics::counter!("antares_quality_sweep_total",
                    "tenant" => changed.tenant.clone(), "type" => changed.type_iri.clone(),
                    "rule" => changed.rule.clone(), "outcome" => changed.outcome)
                .increment(changed.count);
                tracing::info!(
                    tenant = changed.tenant,
                    r#type = changed.type_iri,
                    rule = changed.rule,
                    outcome = changed.outcome,
                    count = changed.count,
                    "quality sweep"
                );
                out.push(changed);
            }
        }
    }
    out.extend(lift_orphans(&r.state, &rules, now).await);
    out
}

/// Lifts, in every Tenant, each name in `quarantineRules` that no current
/// rule of that Tenant carries (F-ANT4-011): a rule renamed or deleted, its
/// whole document included, leaves no Entity quarantined (hidden) for good.
/// Read like [`judge_rule`]: at most [`ROUNDS`] pages of the Entities that
/// hold a mark, then each one rewritten under its row lock.
async fn lift_orphans(
    state: &AppState,
    rules: &Rules,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<Changed> {
    let mut tenants: Vec<String> = state.tenant_names().await.unwrap_or_default();
    tenants.extend(rules.iter().map(|(t, _)| t.as_str().to_owned()));
    tenants.sort();
    tenants.dedup();
    let ts = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let mut out = Vec::new();
    for t in tenants.iter().filter_map(|n| TenantId::new(n).ok()) {
        let tenant = t.as_str().to_owned();
        let current: Vec<&str> = rules
            .iter()
            .filter(|(rt, _)| *rt == t)
            .flat_map(|(_, list)| list.iter().map(|r| r.name.as_str()))
            .collect();
        if !may_hold_marks(state, &t).await {
            continue;
        }
        let lifted = match lift_in(state, &t, &current, &ts).await {
            Ok(lifted) => lifted,
            Err(e) => {
                tracing::warn!(tenant, error = %e, "quality sweep failed");
                continue;
            }
        };
        for (rule, count) in lifted {
            metrics::counter!("antares_quality_sweep_total",
                "tenant" => tenant.clone(), "type" => "", "rule" => rule.clone(), "outcome" => "lift")
            .increment(count);
            tracing::info!(
                tenant,
                rule,
                count,
                "quality sweep lifted a mark no rule carries"
            );
            out.push(Changed {
                tenant: tenant.clone(),
                type_iri: String::new(),
                rule,
                outcome: "lift",
                count,
            });
        }
    }
    out
}

/// The names in `doc`'s `quarantineRules` that none of `current` carries.
fn orphaned(doc: &Value, current: &[&str]) -> Vec<String> {
    marked_by(doc)
        .into_iter()
        .filter(|n| !current.contains(&n.as_str()))
        .collect()
}

/// Takes the orphaned names off `doc`, stamped `ts`, and the quarantine with
/// the last one; the names it took, empty when it changed nothing.
fn strip_orphans(doc: &mut Value, current: &[&str], ts: &str) -> Vec<String> {
    let gone = orphaned(doc, current);
    let keep: Vec<String> = marked_by(doc)
        .into_iter()
        .filter(|n| !gone.contains(n))
        .collect();
    match doc.as_object_mut() {
        Some(object) if !gone.is_empty() => {
            if keep.is_empty() {
                object.remove(QUARANTINED_AT);
                object.remove(QUARANTINE_RULES);
            } else {
                object.insert(QUARANTINE_RULES.to_owned(), property(&json!(keep), ts));
            }
            gone
        }
        _ => Vec::new(),
    }
}

/// Whether Tenant `t`'s Entities may hold a quarantine mark: false only
/// when its attribute catalogue never listed one, so [`lift_in`] skips a
/// scan that under forced RLS reads every Entity of the Tenant
/// (F-R2ANT3-008). A backend without a catalogue, or one that failed to
/// answer, scans.
async fn may_hold_marks(state: &AppState, t: &TenantId) -> bool {
    match state.store.entity_attr_pairs(t).await {
        Ok(Some(pairs)) => pairs.iter().any(|(_, a)| a == QUARANTINE_RULES),
        _ => true,
    }
}

/// [`lift_orphans`] in one Tenant: how many Entities lost each name.
async fn lift_in(
    state: &AppState,
    t: &TenantId,
    current: &[&str],
    ts: &str,
) -> Result<BTreeMap<String, u64>, String> {
    let attrs = [QUARANTINE_RULES.to_owned()];
    let mut pending: Vec<String> = Vec::new();
    for round in 0..ROUNDS {
        let rows = state
            .scan_entities(t, &[], &attrs, round.saturating_mul(PAGE), PAGE)
            .await?;
        let last = rows.len() < PAGE;
        pending.extend(
            rows.iter()
                .filter(|d| !orphaned(d, current).is_empty())
                .filter_map(|d| d.get("id").and_then(Value::as_str).map(str::to_owned)),
        );
        if last {
            break;
        }
    }
    let mut lifted: BTreeMap<String, u64> = BTreeMap::new();
    for batch in pending.chunks(PAGE) {
        // the names each Entity lost, by id: a rerun closure overwrites
        let mut gone_by: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let written = state
            .mutate_entities(t, batch, |id, doc| {
                let gone = strip_orphans(doc, current, ts);
                let changed = !gone.is_empty();
                gone_by.insert(id.to_owned(), gone);
                changed
            })
            .await?;
        for (id, _) in batch.iter().zip(written).filter(|(_, w)| *w) {
            for name in gone_by.remove(id.as_str()).unwrap_or_default() {
                let n = lifted.entry(name).or_default();
                *n = n.saturating_add(1);
            }
        }
    }
    Ok(lifted)
}

/// The Entities of `rule`'s types in Tenant `t` whose mark the rule would
/// change at `now`, read in at most [`ROUNDS`] pages of [`PAGE`]; then each
/// one marked or lifted under its row lock, judged again on what the store
/// holds at that moment.
async fn judge_rule(
    state: &AppState,
    t: &TenantId,
    rule: &Rule,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<Changed> {
    let tenant = t.as_str();
    let mut types: Vec<String> = Vec::new();
    let mut attrs = vec![QUARANTINE_RULES.to_owned()];
    for (ty, attribute) in &rule.checks {
        if !types.contains(ty) {
            types.push(ty.clone());
        }
        if !attrs.contains(attribute) {
            attrs.push(attribute.clone());
        }
    }
    let type_iri = types.join(",");
    let failed = |e: &str| {
        tracing::warn!(tenant, r#type = type_iri.as_str(), rule = rule.name.as_str(), error = %e, "quality sweep failed");
    };
    // A maxAge holds at most ten years, so the cut is always in range.
    let cut = chrono::Duration::from_std(rule.max_age)
        .ok()
        .and_then(|age| now.checked_sub_signed(age))
        .unwrap_or(chrono::DateTime::<chrono::Utc>::MIN_UTC);
    let ts = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let mut pending: Vec<String> = Vec::new();
    for round in 0..ROUNDS {
        let rows = match state
            .scan_entities(t, &types, &attrs, round.saturating_mul(PAGE), PAGE)
            .await
        {
            Ok(rows) => rows,
            Err(e) => {
                failed(&e);
                return Vec::new();
            }
        };
        let last = rows.len() < PAGE;
        pending.extend(rows.iter().filter_map(|doc| {
            verdict(doc, rule, cut)
                .and_then(|_| doc.get("id"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        }));
        if last {
            break;
        }
    }
    let (mut marked, mut lifted) = (0u64, 0u64);
    for batch in pending.chunks(PAGE) {
        // What the closure wrote, judged again under each row lock: one
        // entry per Entity marked (`true`) or lifted (`false`).
        let mut wrote = Vec::new();
        let written = state
            .mutate_entities(t, batch, |_, doc| {
                judge(doc, rule, cut, &ts)
                    .map(|is_stale| wrote.push(is_stale))
                    .is_some()
            })
            .await;
        if let Err(e) = written {
            failed(&e);
            break;
        }
        let now_marked = wrote.iter().filter(|m| **m).count();
        marked = marked.saturating_add(u64::try_from(now_marked).unwrap_or(u64::MAX));
        lifted = lifted.saturating_add(
            u64::try_from(wrote.len().saturating_sub(now_marked)).unwrap_or(u64::MAX),
        );
    }
    [("quarantine", marked), ("lift", lifted)]
        .into_iter()
        .filter(|(_, count)| *count > 0)
        .map(|(outcome, count)| Changed {
            tenant: tenant.to_owned(),
            type_iri: type_iri.clone(),
            rule: rule.name.clone(),
            outcome,
            count,
        })
        .collect()
}

/// `/q/quality/{tenant}`: GET the document as written, PUT one, DELETE it.
pub(super) fn routes(weak: &Weak<Reloader>) -> Router {
    Router::new().route("/quality/{tenant}", DOC.handlers(weak))
}

static DOC: super::TextDoc = super::TextDoc {
    prefix: QUALITY_ID_PREFIX,
    what: "quality",
    max: MAX_QUALITY_BYTES,
    check: |body| parse(body).map(drop),
};

#[cfg(test)]
mod tests {
    use super::*;

    /// F-R2ANT3-008: a Tenant whose catalogue never listed the mark is not
    /// scanned; one whose Entity carries it is.
    #[tokio::test]
    async fn only_a_tenant_whose_catalogue_lists_the_mark_is_scanned() {
        let state = AppState::new("antares-test".into());
        for (tenant, doc) in [
            (
                "plain",
                json!({"id": "urn:v:1", "type": "V", "speed": {"type": "Property", "value": 1}}),
            ),
            (
                "marked",
                json!({"id": "urn:v:2", "type": "V",
                "quarantineRules": {"type": "Property", "value": ["gone"]}}),
            ),
        ] {
            let body = doc.to_string();
            let req = axum::http::Request::post("/ngsi-ld/v1/entities")
                .header("content-type", "application/json")
                .header("content-length", body.len())
                .header("ngsild-tenant", tenant)
                .body(axum::body::Body::from(body))
                .expect("request");
            let answer = state.call(&axum::http::HeaderMap::new(), req).await;
            assert_eq!(answer.status(), axum::http::StatusCode::CREATED, "{tenant}");
        }
        let plain = TenantId::new("plain").expect("tenant");
        let marked = TenantId::new("marked").expect("tenant");
        assert!(
            !may_hold_marks(&state, &plain).await,
            "nothing to lift there"
        );
        assert!(may_hold_marks(&state, &marked).await);
    }

    const V: &str = "https://e.org/Vehicle";
    const LOCATION: &str = "https://e.org/location";

    /// F-ANT4-011: only names no current rule carries leave; the last one
    /// takes the quarantine along, and a document that is no object stays.
    #[test]
    fn strip_orphans_takes_only_orphaned_names() {
        let ts = "2026-10-26T00:00:00.000Z";
        let mut doc = json!({ QUARANTINED_AT: property(&json!("x"), ts),
            QUARANTINE_RULES: property(&json!(["a", "b"]), ts) });
        assert_eq!(strip_orphans(&mut doc, &["a"], ts), ["b"]);
        assert_eq!(marked_by(&doc), ["a"]);
        assert_eq!(strip_orphans(&mut doc, &["a"], ts), Vec::<String>::new());
        assert_eq!(strip_orphans(&mut doc, &[], ts), ["a"]);
        assert_eq!(doc.get(QUARANTINED_AT), None);
        assert_eq!(doc.get(QUARANTINE_RULES), None);
        assert_eq!(
            strip_orphans(&mut json!([1]), &[], ts),
            Vec::<String>::new()
        );
    }

    fn rule() -> Rule {
        Rule {
            name: "stale-position".into(),
            checks: vec![(V.into(), LOCATION.into())],
            max_age: Duration::from_secs(600),
        }
    }

    fn entry(name: &str, ty: &str, attribute: &str, max_age: &str) -> Value {
        json!({"name": name, "type": ty, "attribute": attribute, "maxAge": max_age})
    }

    fn doc(entries: &[Value]) -> Vec<u8> {
        json!({ "rules": entries }).to_string().into_bytes()
    }

    /// `now` less `minutes`, as an `observedAt`.
    fn at(now: chrono::DateTime<chrono::Utc>, minutes: i64) -> String {
        now.checked_sub_signed(chrono::Duration::minutes(minutes))
            .unwrap_or(now)
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }

    fn cut_of(now: chrono::DateTime<chrono::Utc>) -> chrono::DateTime<chrono::Utc> {
        now.checked_sub_signed(chrono::Duration::minutes(10))
            .unwrap_or(now)
    }

    #[test]
    fn a_document_names_labels_and_expanded_iris_once() {
        assert_eq!(
            parse(&doc(&[entry("stale-position", V, LOCATION, "PT10M")])),
            Ok(vec![rule()])
        );
        assert!(parse(&doc(&[entry("a1", V, LOCATION, "PT1S")])).is_ok());
        let too_many: Vec<Value> = (0..=MAX_QUALITY_RULES)
            .map(|n| entry(&format!("r{n}"), V, LOCATION, "PT1M"))
            .collect();
        for bad in [
            doc(&[entry("A", V, LOCATION, "PT1M")]),
            doc(&[entry("-a", V, LOCATION, "PT1M")]),
            doc(&[entry("a-", V, LOCATION, "PT1M")]),
            doc(&[entry("a_b", V, LOCATION, "PT1M")]),
            doc(&[entry(&"a".repeat(64), V, LOCATION, "PT1M")]),
            doc(&[entry("a", ":Vehicle", LOCATION, "PT1M")]),
            doc(&[entry("a", "urn:", LOCATION, "PT1M")]),
            doc(&[entry("a", "https://e.org/a b", LOCATION, "PT1M")]),
            doc(&[entry("a", "Vehicle", LOCATION, "PT1M")]),
            doc(&[entry("a", V, "speed", "PT1M")]),
            doc(&[entry("a", V, LOCATION, "PT0S")]),
            doc(&[entry("a", V, LOCATION, "P3651D")]),
            doc(&[
                entry("a", V, LOCATION, "PT1M"),
                entry("a", V, LOCATION, "PT1M"),
            ]),
            doc(&[
                entry("a", V, LOCATION, "PT1M"),
                entry("a", "https://e.org/S", LOCATION, "PT2M"),
            ]),
            json!({"rules": [{"name": "a", "type": V, "attribute": LOCATION, "maxAge": "PT1M",
                "outcome": "flag"}]})
            .to_string()
            .into_bytes(),
            doc(&too_many),
            b"not json".to_vec(),
        ] {
            assert!(parse(&bad).is_err(), "{}", String::from_utf8_lossy(&bad));
        }
    }

    #[test]
    fn the_entries_of_one_name_are_one_rule() {
        let rules = parse(&doc(&[
            entry("a", V, "https://e.org/x", "PT1M"),
            entry("b", V, "https://e.org/x", "PT5M"),
            entry("a", "https://e.org/S", "https://e.org/x", "PT1M"),
            entry("a", V, "https://e.org/y", "PT1M"),
        ]))
        .unwrap_or_default();
        let shape: Vec<(&str, usize)> = rules
            .iter()
            .map(|r| (r.name.as_str(), r.checks.len()))
            .collect();
        assert_eq!(shape, [("a", 3), ("b", 1)]);
    }

    #[test]
    fn a_stored_document_that_no_longer_parses_is_skipped() {
        let good = json!({"text": String::from_utf8_lossy(&doc(&[entry("stale-position", V, LOCATION, "PT10M")]))});
        let broken = json!({"text": "not json"});
        let rules = load([("city", &good), ("port", &broken), ("Not One", &good)].into_iter());
        let tenants: Vec<&str> = rules.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(tenants, ["city"]);
    }

    #[test]
    fn a_stale_entity_is_marked_once_and_lifted_when_fresh() {
        let r = rule();
        let now = chrono::Utc::now();
        let cut = cut_of(now);
        let mut doc = json!({"id": "urn:v:1", "type": [V],
            LOCATION: [{"type": "Property", "value": 1, "observedAt": at(now, 11)}]});
        assert_eq!(judge(&mut doc, &r, cut, "t1"), Some(true));
        assert_eq!(marked_by(&doc), ["stale-position"]);
        assert!(doc.get(QUARANTINED_AT).is_some());
        assert_eq!(
            judge(&mut doc, &r, cut, "t2"),
            None,
            "marked twice is marked once"
        );

        doc[LOCATION] = json!([{"type": "Property", "value": 2, "observedAt": at(now, 1)}]);
        assert_eq!(judge(&mut doc, &r, cut, "t3"), Some(false));
        assert!(doc.get(QUARANTINED_AT).is_none(), "{doc}");
        assert!(doc.get(QUARANTINE_RULES).is_none(), "{doc}");

        // Another rule's mark stays when this one lifts; the attribute gone lifts too.
        doc[QUARANTINE_RULES] = property(&json!(["stale-position", "other"]), "t4");
        doc[QUARANTINED_AT] = property(&json!({"@type": "DateTime", "@value": "t4"}), "t4");
        doc.as_object_mut().map(|o| o.remove(LOCATION));
        assert_eq!(judge(&mut doc, &r, cut, "t5"), Some(false));
        assert_eq!(marked_by(&doc), ["other"]);
        assert!(doc.get(QUARANTINED_AT).is_some());
    }

    /// The mark of another rule keeps its `quarantinedAt`; one attribute
    /// instance as an object, a type and a `quarantineRules` value as one
    /// string read as their arrays do.
    #[test]
    fn a_mark_beside_another_rules_keeps_the_first_quarantined_at() {
        let r = rule();
        let now = chrono::Utc::now();
        let mut doc = json!({"id": "urn:v:1", "type": V,
            LOCATION: {"type": "Property", "value": 1, "observedAt": at(now, 30)},
            QUARANTINE_RULES: [{"type": "Property", "value": "other"}],
            QUARANTINED_AT: [{"type": "Property", "value": "first"}]});
        assert_eq!(judge(&mut doc, &r, cut_of(now), "t1"), Some(true));
        assert_eq!(marked_by(&doc), ["other", "stale-position"]);
        assert_eq!(doc[QUARANTINED_AT][0]["value"], "first");

        let mut odd = json!({"id": "urn:v:2", "type": 7,
            LOCATION: [{"type": "Property", "value": 1, "observedAt": at(now, 30)}],
            QUARANTINE_RULES: [{"type": "Property", "value": 7}]});
        assert_eq!(marked_by(&odd), Vec::<String>::new());
        assert_eq!(
            judge(&mut odd, &r, cut_of(now), "t2"),
            None,
            "no type is not the rule's"
        );
        assert_eq!(judge(&mut json!(1), &r, cut_of(now), "t3"), None);
    }

    /// An Entity of two types one name judges: the stale attribute of one
    /// type marks it, the fresh one of the other does not lift it, so two
    /// entries of a name never undo each other sweep after sweep.
    #[test]
    fn two_entries_of_one_name_never_undo_each_other() {
        let r = Rule {
            name: "a".into(),
            checks: vec![
                (V.into(), "https://e.org/x".into()),
                ("https://e.org/S".into(), "https://e.org/y".into()),
            ],
            max_age: Duration::from_secs(600),
        };
        let now = chrono::Utc::now();
        let cut = cut_of(now);
        let mut doc = json!({"id": "urn:v:1", "type": [V, "https://e.org/S"],
            "https://e.org/x": [{"type": "Property", "value": 1, "observedAt": at(now, 30)}],
            "https://e.org/y": [{"type": "Property", "value": 1, "observedAt": at(now, 1)}]});
        assert_eq!(judge(&mut doc, &r, cut, "t1"), Some(true));
        assert_eq!(judge(&mut doc, &r, cut, "t2"), None, "the mark stays");
        // The attribute of a type the Entity lacks is not judged.
        let mut other = json!({"id": "urn:s:1", "type": ["https://e.org/S"],
            "https://e.org/x": [{"type": "Property", "value": 1, "observedAt": at(now, 30)}]});
        assert_eq!(judge(&mut other, &r, cut, "t3"), None);
    }

    #[test]
    fn an_instance_without_observed_at_is_not_judged() {
        let mut doc =
            json!({"id": "urn:v:1", "type": [V], LOCATION: [{"type": "Property", "value": 1}]});
        assert_eq!(judge(&mut doc, &rule(), chrono::Utc::now(), "t"), None);
    }
}
