// SPDX-License-Identifier: EUPL-1.2
//! What a temporal read keeps of an Evolution: the rules per instance (ADR-0023 rule 11) and the 5.7.4.4 window conditions.

use super::*;

/// Whether `rules` decide an instance on the Entity's state at its time (a
/// rule with a condition), which [`rules_per_instance`] rebuilds from the
/// whole history. A rule of types, Attributes and windows decides each
/// instance alone, so the store may prune to the query's window (T-4246).
pub(super) fn replays_state(rules: Option<&crate::policy::Filter>) -> bool {
    rules.is_some_and(|r| r.rules.iter().any(crate::policy::Rule::is_conditional))
}

/// Whether the store's per-Attribute cap keeps every instance `rules` would
/// show of the page: with no rules always; under rules only when no rule
/// reads state and every rule window stays open on the page's side: towards
/// now for lastN, and from no later than the query's `start` (its `after`
/// or `between` instant) oldest first, so the rules keep a leading run of
/// what the store kept. A window that closes before now under lastN, or
/// opens after the start oldest first, would have the store keep instances
/// the rules then drop (T-4246, T-5777). Rule windows judge observedAt, so
/// a window under a cap ordered by another `timeproperty` (a backfill: new
/// createdAt, old observedAt) keeps the cap in the API too.
pub(super) fn cap_keeps_shown(
    rules: Option<&crate::policy::Filter>,
    page: &InstancePage,
    timeprop: &str,
    start: Option<&str>,
) -> bool {
    // an `Ago` bound resolved an hour late: the rules resolve theirs after
    // the store answered, by then a little later than now
    let late = chrono::Utc::now()
        .checked_add_signed(chrono::Duration::hours(1))
        .unwrap_or(chrono::DateTime::<chrono::Utc>::MAX_UTC);
    rules.is_none_or(|r| {
        !replays_state(Some(r))
            && r.rules.iter().all(|rule| {
                rule.temporal.is_empty()
                    || (timeprop == "observedAt"
                        && rule.temporal.iter().all(|w| {
                            if page.desc {
                                w.to.is_none()
                            } else {
                                start.is_some_and(|s| w.opens_by(s, late))
                            }
                        }))
            })
    })
}

/// The Attributes the store may qualify an Entity for a page through under
/// `rules` (T-5616): `Some(None)` any, `Some(Some(list))` only these, `None`
/// where the rules' verdict on an Entity is not the store's, so it may not
/// page. It is where every rule names Entities by type alone (no condition
/// or id; an idPattern only where every rule shares the same ones the store
/// writes exactly, [`crate::policy::shared_id_patterns`], T-6052) through the
/// same windows, every type the rules name
/// shows the same Attributes (the union of its rules: a list longer than a
/// rule holds is split over several rules of the same types, T-5835), no
/// type hides Attributes of its own, each requested type is named by a
/// rule, and the windows hold the whole request window (open towards now,
/// opened by its `after` or `between` instant on `observedAt`): an Entity
/// with an instance of a shown Attribute in the request window then shows
/// that instance, and one without shows none. `attrs`, the request's own,
/// narrow what is shown. Without it a view read every Entity of the type
/// for each page: 577 bike stations, past the 30 s statement timeout.
pub(super) fn page_attrs(
    rules: &crate::policy::Filter,
    types: Option<&[String]>,
    tq: Option<&TemporalQ>,
    attrs: Option<&[String]>,
) -> Option<Option<Vec<String>>> {
    let first = rules.rules.first()?;
    let start = range_start(tq);
    let late = chrono::Utc::now()
        .checked_add_signed(chrono::Duration::hours(1))
        .unwrap_or(chrono::DateTime::<chrono::Utc>::MAX_UTC);
    let windows_hold = first.temporal.is_empty()
        || (tq.is_none_or(|t| t.timeproperty == "observedAt")
            && start.is_some_and(|s| {
                first
                    .temporal
                    .iter()
                    .all(|w| w.to.is_none() && w.opens_by(s, late))
            }));
    // the store selects by the rules' shared idPatterns too (T-6052)
    let patterns = crate::policy::shared_id_patterns(&rules.rules);
    let by_ids = |r: &crate::policy::Rule| {
        r.admits_by_type_alone()
            || patterns
                .is_some_and(|p| !r.is_conditional() && r.ids.is_empty() && r.id_patterns == p)
    };
    let alike = rules
        .rules
        .iter()
        .all(|r| by_ids(r) && r.temporal == first.temporal);
    // what an Entity of `t` shows: every Attribute, or the union of its rules' lists
    let shown = |t: &String| {
        let of = || rules.rules.iter().filter(|r| r.types.contains(t));
        let every = of().any(|r| r.every_attribute);
        let names: std::collections::BTreeSet<&String> = if every {
            Default::default()
        } else {
            of().flat_map(|r| &r.attributes).collect()
        };
        (every, names)
    };
    let mut typed = rules.rules.iter().flat_map(|r| &r.types).map(shown);
    let (every, names) = typed.next()?;
    let uniform = typed.all(|other| other == (every, names.clone()));
    let named = types.is_some_and(|ts| {
        ts.iter()
            .all(|t| rules.rules.iter().any(|r| r.types.contains(t)))
    });
    if !(windows_hold && alike && uniform && named && rules.omit_by_type.is_empty()) {
        return None;
    }
    Some(match (every, attrs) {
        (true, None) => None,
        (true, Some(a)) => Some(a.to_vec()),
        (false, None) => Some(names.into_iter().cloned().collect()),
        (false, Some(a)) => Some(a.iter().filter(|x| names.contains(x)).cloned().collect()),
    })
}

/// The instant a window read oldest first starts at: `timeAt` of `after`
/// and `between`; `before` and no window start at the beginning of time.
pub(super) fn range_start(tq: Option<&TemporalQ>) -> Option<&str> {
    tq.filter(|t| matches!(t.timerel.as_str(), "after" | "between"))
        .map(|t| t.time_at.as_str())
}

/// 5.7.4.4 S2/S3: the values filter and the geoquery are judged on the
/// Attribute instances WITHIN the temporal-query interval, so an
/// out-of-window instance must not satisfy either.
/// ADR-0023 rule 11, temporal per instance: an instance of an Attribute
/// is kept only where a rule showing the Attribute held at the instance's
/// time (`observedAt`, else `modifiedAt`). A rule without a condition holds
/// at every time; a conditional one is read on the Entity's state at that
/// time ([`state_at`]) and does not hold where a member its condition reads
/// has no instance yet, or where it reads `scope`, which keeps no history:
/// a state that cannot be rebuilt hides the instance (fail closed). `None`
/// when no rule names one of the Entity's types.
pub(crate) fn rules_per_instance(doc: &Value, rules: &crate::policy::Filter) -> Option<Value> {
    let ctx = crate::repr::policy_context();
    let types = crate::policy::entity_types(doc);
    let typed: Vec<&crate::policy::Rule> = rules.rules.iter().filter(|r| r.selects(doc)).collect();
    if !rules.rules.is_empty() && typed.is_empty() {
        return None;
    }
    let omitted: std::collections::BTreeSet<&str> = types
        .iter()
        .filter_map(|t| rules.omit_by_type.get(*t))
        .flatten()
        .map(String::as_str)
        .collect();
    // a rule with windows decides per instance too (gap G6)
    let per_instance = |r: &crate::policy::Rule| r.is_conditional() || !r.temporal.is_empty();
    let always: std::collections::BTreeSet<&str> = typed
        .iter()
        .filter(|r| !per_instance(r))
        .flat_map(|r| r.attributes.iter().map(String::as_str))
        .collect();
    let conditional: Vec<(&crate::policy::Rule, Vec<String>)> = typed
        .iter()
        .filter(|r| per_instance(r) && r.scope_q.is_none())
        .map(|r| (*r, r.reads(ctx)))
        .collect();
    // an unconditioned rule over every Attribute keeps every instance
    let every = typed.iter().any(|r| !per_instance(r) && r.every_attribute);
    let now = chrono::Utc::now();
    let Some(obj) = doc.as_object() else {
        return Some(doc.clone());
    };
    let mut out = Map::new();
    for (k, v) in obj {
        if is_meta(k) {
            out.insert(k.clone(), v.clone());
            continue;
        }
        if omitted.contains(k.as_str()) {
            continue;
        }
        if rules.rules.is_empty() || every || always.contains(k.as_str()) {
            out.insert(k.clone(), v.clone());
            continue;
        }
        let deciding: Vec<&(&crate::policy::Rule, Vec<String>)> = conditional
            .iter()
            .filter(|(r, _)| r.every_attribute || r.attributes.iter().any(|a| a == k))
            .collect();
        let Some(instances) = v.as_array().filter(|_| !deciding.is_empty()) else {
            continue;
        };
        // ponytail: a condition rebuilds the state per instance, O(instances x
        // history); sort each Attribute's history once if long histories show
        // up under one. A rule without a condition reads no state: `typed`
        // already selected the Entity by its types and ids, which is all
        // `holds` reads of it, so only its windows decide (T-4246).
        let reads_state = deciding.iter().any(|(r, _)| r.is_conditional());
        let kept: Vec<Value> = instances
            .iter()
            .filter(|inst| {
                instant(inst).is_some_and(|t| {
                    let state = reads_state.then(|| state_at(doc, t));
                    let observed = inst.get("observedAt").and_then(Value::as_str);
                    deciding.iter().any(|(r, reads)| {
                        (!r.is_conditional()
                            || state.as_ref().is_some_and(|state| {
                                reads.iter().all(|a| state.get(a).is_some()) && r.holds(state, ctx)
                            }))
                            && r.temporal
                                .iter()
                                .all(|w| observed.is_some_and(|o| w.admits(o, now)))
                    })
                })
            })
            .cloned()
            .collect();
        if !kept.is_empty() {
            out.insert(k.clone(), Value::Array(kept));
        }
    }
    Some(Value::Object(out))
}

/// When an instance holds: its `observedAt`, else its `modifiedAt`.
pub(super) fn instant(inst: &Value) -> Option<&str> {
    inst.get("observedAt")
        .or_else(|| inst.get("modifiedAt"))
        .and_then(Value::as_str)
}

/// The Entity as it stood at `t`, rebuilt from its history: the frame, and
/// for each Attribute the latest instance at or before `t`, if any.
pub(super) fn state_at(doc: &Value, t: &str) -> Value {
    let at = dt_key(t);
    let Some(obj) = doc.as_object() else {
        return Value::Null;
    };
    let mut out = Map::new();
    for (k, v) in obj {
        if is_meta(k) {
            if k == "id" || k == "type" {
                out.insert(k.clone(), v.clone());
            }
            continue;
        }
        let latest = v
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|inst| instant(inst).map(|i| (dt_key(i), inst)))
            .filter(|(i, _)| *i <= at)
            .max_by(|a, b| a.0.cmp(&b.0))
            .map(|(_, inst)| inst.clone());
        if let Some(inst) = latest {
            out.insert(k.clone(), Value::Array(vec![inst]));
        }
    }
    Value::Object(out)
}

pub(super) fn window_conditions_match(
    doc: &Value,
    tq: Option<&TemporalQ>,
    q_ast: Option<&antares_ql::QNode>,
    geos: &[&antares_ql::geo::GeoQuery],
    ctx: &Context,
) -> bool {
    // 5.7.4.4 S2/S3: the values filter and geoquery are checked against
    // the Attribute instances WITHIN the temporal-query interval — an
    // out-of-window instance must not satisfy them
    let mut eval_doc = doc.clone();
    if let (Some(tqv), Some(o)) = (tq, eval_doc.as_object_mut()) {
        for (k, v) in o.iter_mut() {
            if is_meta(k) {
                continue;
            }
            if let Some(arr) = v.as_array_mut() {
                arr.retain(|inst| tqv.instance_matches(inst));
            }
        }
    }
    if let Some(ast) = q_ast {
        if !antares_ql::eval::eval_q(ast, &eval_doc, ctx, &|_| None) {
            return false;
        }
    }
    geos.iter().all(|g| g.matches(&eval_doc, ctx))
}

/// 5.7.4.4 S9: of each Attribute `q` reads, keep the instances that meet
/// the query: an instance meets it when `q` holds on the Entity with that
/// Attribute narrowed to the one instance (the other Attributes in the
/// window), so a term on another Attribute still acts at Entity level.
pub(super) fn retain_q_instances(
    doc: &mut Value,
    tq: Option<&TemporalQ>,
    ast: &antares_ql::QNode,
    ctx: &Context,
) {
    let mut heads: Vec<String> = ast
        .attribute_paths()
        .into_iter()
        .filter_map(|p| p.split(['.', '[', '{']).next().map(|h| ctx.expand_key(h)))
        .collect();
    heads.sort();
    heads.dedup();
    let mut eval_doc = doc.clone();
    if let (Some(tqv), Some(o)) = (tq, eval_doc.as_object_mut()) {
        for (k, v) in o.iter_mut() {
            if !is_meta(k) {
                if let Some(arr) = v.as_array_mut() {
                    arr.retain(|inst| tqv.instance_matches(inst));
                }
            }
        }
    }
    let (Some(obj), Value::Object(eval)) = (doc.as_object_mut(), &mut eval_doc) else {
        return;
    };
    for head in heads {
        let Some(arr) = obj.get_mut(&head).and_then(Value::as_array_mut) else {
            continue;
        };
        let windowed = eval.get(&head).cloned();
        arr.retain(|inst| {
            eval.insert(head.clone(), Value::Array(vec![inst.clone()]));
            // `eval` is lent back as a document for the one evaluation
            let probe = Value::Object(std::mem::take(eval));
            let holds = antares_ql::eval::eval_q(ast, &probe, ctx, &|_| None);
            if let Value::Object(m) = probe {
                *eval = m;
            }
            holds
        });
        if let Some(w) = windowed {
            eval.insert(head, w);
        }
    }
}

/// 5.7.4.4 S4 — and S7: the federation merge precedes this check, so
/// split and aggregated entities are re-filtered here too. The entity
/// qualifies if one of its scope's 4.18 validity intervals intersects
/// the query window; attribute instances outside every matching interval
/// are excluded (annex C.5.16).
pub(super) fn scope_window_retain(
    doc: &mut Value,
    sq: &str,
    tq: Option<&TemporalQ>,
    timeprop: &str,
) -> bool {
    // every comparison on dt_key: raw strings put `…00.500Z` below `…00Z`
    let iv: Vec<(String, Option<String>)> = scope_match_intervals(doc, sq)
        .into_iter()
        .map(|(s, e)| (dt_key(&s), e.as_deref().map(dt_key)))
        .collect();
    let (wstart, wend) = match tq {
        Some(t) => match t.timerel.as_str() {
            "before" => (None, Some(dt_key(&t.time_at))),
            "after" => (Some(dt_key(&t.time_at)), None),
            "between" => (
                Some(dt_key(&t.time_at)),
                t.end_time_at.as_deref().map(dt_key),
            ),
            _ => (None, None),
        },
        None => (None, None),
    };
    let intersects = iv.iter().any(|(s, e)| {
        wend.as_deref().is_none_or(|w| s.as_str() < w)
            && e.as_deref()
                .is_none_or(|e| wstart.as_deref().is_none_or(|w| e > w))
    });
    if !intersects {
        return false;
    }
    if let Some(o) = doc.as_object_mut() {
        for (k, v) in o.iter_mut() {
            if is_meta(k) {
                continue;
            }
            if let Some(arr) = v.as_array_mut() {
                arr.retain(|inst| {
                    inst.get(timeprop)
                        .and_then(Value::as_str)
                        .map(dt_key)
                        .is_some_and(|t| {
                            let t = t.as_str();
                            iv.iter()
                                .any(|(s, e)| t >= s.as_str() && e.as_deref().is_none_or(|e| t < e))
                        })
                });
            }
        }
    }
    true
}
