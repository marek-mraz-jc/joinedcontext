// SPDX-License-Identifier: EUPL-1.2
//! The decisions of README section 3: prohibitions first, default deny, a
//! read narrowed to one rule per selector, a write whole inside one policy.

use crate::document::{context, covers, scope_paths, Policy, Table};
use crate::subject::{Caller, Platform};
use antares_api::policy::metadata::Metadata;
use antares_api::policy::{
    Decision, Filter, Operation, Rule, Warning, FILTERABLE, NAME_READS, WHOLE_TENANT, WRITE_BY_ID,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Writes judged on the Entity they create, so a conditional policy is
/// judged on the request body.
const CREATES: [&str; 2] = ["5.6.1", "5.6.7"];

/// Access control written into the data (platform GW29): an Entity never
/// carries a member of these names, short or expanded.
pub(crate) const POLICY_VOCABULARY: [&str; 6] = [
    "owner",
    "acl",
    "allowedRoles",
    "visibility",
    "permissions",
    "policy",
];

/// Why an operation no policy grants is refused; the same words for every
/// refusal by the rules, so the reason tells nothing about them (GW6).
pub const NOT_GRANTED: &str = "no policy grants this operation";
/// Why a prohibited operation is refused.
pub const PROHIBITED: &str = "a policy prohibits this operation";
/// Why a read under a policy granting every type with a condition is
/// refused when it names no type: a rule needs a type.
pub const NAME_THE_TYPES: &str = "name the Entity types to read";
/// Why a read-only caller's operation other than a read is refused (README section 1).
pub const READ_ONLY: &str = "a read-only caller reads and never writes";
/// What a read-only caller may do besides [`FILTERABLE`] and [`NAME_READS`]: retrieve one
/// Subscription, Registration or Registration Subscription by id. Every other clause is
/// refused, so a clause added later is a write until listed here.
const READ_ONLY_RETRIEVES: [&str; 3] = ["5.8.3", "5.10.1", "5.11.4"];

/// Why a call from inside the cluster is refused (README section 1).
pub const INTERNAL_ONLY: &str =
    "a request without a subject reaches the Context Source operations only";

/// The decision for `op` by `caller` under the Tenant's `table` at `now`.
#[must_use]
pub fn decide(
    table: Option<&Table>,
    caller: &Caller,
    op: &Operation<'_>,
    now: DateTime<Utc>,
) -> Decision {
    let platform = match caller {
        Caller::Internal => {
            return if ["5.9.", "5.10.", "5.11."]
                .iter()
                .any(|p| op.clause.starts_with(p))
            {
                Decision::Allow
            } else {
                Decision::Deny(INTERNAL_ONLY.into())
            }
        }
        Caller::Platform(p) => p,
    };
    if platform.read_only && !reads(op.clause) {
        return Decision::Deny(READ_ONLY.into());
    }
    let Some(table) = table else {
        return Decision::Deny(NOT_GRANTED.into());
    };
    let applying: Vec<&Policy> = held(table, platform, now)
        .filter(|p| p.clauses.contains(op.clause))
        .collect();
    let types = entity_types(op);
    let read = FILTERABLE.contains(&op.clause);
    // T-5842: a read or a write is narrowed by the prohibition entries naming Attributes;
    // any other operation is refused by them whole, as before.
    let by_attribute = read || op.clause.starts_with("5.6.");
    let prohibited = |types: &[String]| {
        applying.iter().any(|p| {
            p.prohibit
                && if by_attribute {
                    refuses_types(p, types, read)
                } else {
                    covers_types(p, types)
                }
        })
    };
    // A read that names types is decided per type, as a read of each alone would be (T-4455):
    // a type a prohibition covers is left out and the others are read. Any other operation,
    // and a read naming no type, is refused whole.
    let per_type = read && !types.is_empty();
    let (types, refused): (Vec<String>, Vec<String>) = if per_type {
        types
            .into_iter()
            .partition(|t| !prohibited(std::slice::from_ref(t)))
    } else {
        (types, Vec::new())
    };
    // GW6: a refusal names no policy, so a caller cannot map the rules
    if (per_type && types.is_empty()) || (!per_type && prohibited(&types)) {
        return Decision::Deny(PROHIBITED.into());
    }
    let withheld = if by_attribute {
        withheld(applying.iter().copied(), &types)
    } else {
        BTreeMap::new()
    };
    // a write touching an Attribute a prohibition takes back is refused whole (T-5842)
    if !read && touches(op, &withheld) {
        return Decision::Deny(PROHIBITED.into());
    }
    let metadata = passed(&applying, op.clause);
    let grants: Vec<&Policy> = applying.into_iter().filter(|p| !p.prohibit).collect();
    if grants.is_empty() {
        return Decision::Deny(NOT_GRANTED.into());
    }
    if read {
        match read_filter(&grants, op, (&types, &refused), withheld, platform) {
            Decision::Allow if metadata.is_some() => Decision::Filter(Filter {
                metadata,
                ..Filter::default()
            }),
            Decision::Filter(f) => Decision::Filter(Filter { metadata, ..f }),
            other => other,
        }
    } else if WHOLE_TENANT.contains(&op.clause) {
        if grants.iter().any(|p| unconditional_whole(p)) {
            Decision::Allow
        } else {
            Decision::Deny(NOT_GRANTED.into())
        }
    } else if op.clause.starts_with("5.6.") {
        write(&grants, op, &types)
    } else if NAME_READS.contains(&op.clause) {
        names(table, platform, now)
    } else {
        Decision::Allow
    }
}

/// The reads of Entity data whose answers carry metadata members.
const METADATA_READS: [&str; 6] = ["5.7.1", "5.7.2", "5.7.3", "5.7.4", "5.14.4", "5.14.5"];

/// The members `applying` pass per Attribute on a read of `clause`
/// (ADR-N-066 2.4): united across the permissions, less what a prohibition
/// lists. `None` when every Attribute keeps `observedAt`, `unitCode`,
/// `createdAt` and `modifiedAt`, the members a read carries without
/// `versions=all`: today's answer, no block, and nothing to strip. A
/// version answer reaches a caller only through the platform gateway's
/// `JC-Read-Versions`, and the gateway strips its `modifiedBy` and
/// `provenance` (DM-140, DM-151).
fn passed(applying: &[&Policy], clause: &str) -> Option<Box<Metadata>> {
    const PLAIN: [&str; 4] = ["observedAt", "unitCode", "createdAt", "modifiedAt"];
    if !METADATA_READS.contains(&clause) {
        return None;
    }
    let of = |m: &Metadata, name: Option<&str>| -> BTreeSet<String> {
        name.and_then(|n| m.named.get(n)).unwrap_or(&m.rest).clone()
    };
    let at = |name: Option<&str>| -> BTreeSet<String> {
        let mut out: BTreeSet<String> = applying
            .iter()
            .filter(|p| !p.prohibit)
            .flat_map(|p| of(&p.metadata, name))
            .collect();
        for p in applying.iter().filter(|p| p.prohibit) {
            let taken = of(&p.metadata, name);
            out.retain(|m| !taken.contains(m));
        }
        out
    };
    let names: BTreeSet<&str> = applying
        .iter()
        .flat_map(|p| p.metadata.named.keys().map(String::as_str))
        .collect();
    let out = Metadata {
        named: names
            .into_iter()
            .map(|n| (n.to_owned(), at(Some(n))))
            .collect(),
        rest: at(None),
    };
    let narrows = out
        .named
        .values()
        .chain([&out.rest])
        .any(|m| PLAIN.iter().any(|p| !m.contains(*p)));
    narrows.then(|| Box::new(out))
}

/// Whether `clause` only reads: what a read-only caller may ask. A query with an `EntityMap`
/// (5.14.4, 5.14.5) is a read: the map it stores is that query's own expiring paging state,
/// never context data (S-49 keeps a read-only caller off Entities, F-R2ANT1-017).
fn reads(clause: &str) -> bool {
    FILTERABLE.contains(&clause)
        || NAME_READS.contains(&clause)
        || READ_ONLY_RETRIEVES.contains(&clause)
}

/// Whether some request of `op.clause` could be allowed for `caller`: the
/// probe a surface lists its tools by (T-3784), where `op` names nothing.
/// A read is decided per type the request names, and a prohibition refuses
/// only the types it names, so a read is offered when a grant applies and
/// no prohibition names every type (a read of another type is narrowed,
/// never refused). A write must fall inside a grant: it is offered when a
/// grant takes the written types or names a type no prohibition covers; a
/// conditional prohibition counts as refusing its whole type, so the probe
/// may hide a write, never offer one every request of which is refused. The
/// whole-Tenant clauses are decided as a request.
#[must_use]
pub fn probe(
    table: Option<&Table>,
    caller: &Caller,
    op: &Operation<'_>,
    now: DateTime<Utc>,
) -> Decision {
    let (Caller::Platform(platform), Some(table)) = (caller, table) else {
        return decide(table, caller, op, now);
    };
    // a read-only caller is offered no write (S-49): decided as a request, which refuses it
    if WHOLE_TENANT.contains(&op.clause) || (platform.read_only && !reads(op.clause)) {
        return decide(Some(table), caller, op, now);
    }
    let applying: Vec<&Policy> = held(table, platform, now)
        .filter(|p| p.clauses.contains(op.clause))
        .collect();
    let refused = |t: &String| {
        applying
            .iter()
            .any(|p| p.prohibit && refuses_types(p, std::slice::from_ref(t), false))
    };
    let every_type_refused = applying
        .iter()
        .any(|p| p.prohibit && p.selectors.is_empty());
    let grants = applying.iter().filter(|p| !p.prohibit);
    let offered = if every_type_refused {
        false
    } else if op.clause.starts_with("5.6.") {
        // a window is about reading history; it admits no write (`admits`)
        grants
            .filter(|p| p.conditions.temporal.is_empty())
            .any(|p| {
                p.selectors.is_empty()
                    || p.selectors
                        .iter()
                        .flat_map(|s| &s.types)
                        .any(|t| !refused(t))
            })
    } else {
        grants.count() > 0
    };
    if offered {
        Decision::Allow
    } else {
        Decision::Deny(NOT_GRANTED.into())
    }
}

/// The policies of `table` whose assignee is `caller` and that are in force
/// at `now`: what a decision starts from, and all the access surface shows.
pub fn held<'t>(
    table: &'t Table,
    caller: &'t Platform,
    now: DateTime<Utc>,
) -> impl Iterator<Item = &'t Policy> {
    table
        .policies
        .iter()
        .filter(move |p| caller.is(&p.assignee) && p.in_force(now))
}

/// The Entity types an operation names: the request's, and the stored
/// Entity's for a write by id.
fn entity_types(op: &Operation<'_>) -> Vec<String> {
    let mut out: Vec<String> = op.types.to_vec();
    if let Some(stored) = op.stored {
        out.extend(stored.types.iter().cloned());
    }
    out.sort();
    out.dedup();
    out
}

/// A prohibition covers an operation that names no type, or a type it
/// names, or every operation when it names none (platform GW4).
#[must_use]
pub fn covers_types(p: &Policy, types: &[String]) -> bool {
    p.selectors.is_empty()
        || types.is_empty()
        || p.selectors
            .iter()
            .any(|s| s.types.iter().any(|t| types.contains(t)))
}

/// A prohibition refuses the whole of `types` (platform GW4) when it names no type, or an
/// entry naming no Attribute names one of them; an entry naming `propertyNames` or
/// `relationshipNames` takes back only those (T-5842, as the gateway PDP since T-5586). An
/// operation naming no type is refused by any prohibition, unless `untyped_narrows`: a read
/// naming no type loses the Attributes instead.
#[must_use]
pub fn refuses_types(p: &Policy, types: &[String], untyped_narrows: bool) -> bool {
    p.selectors.is_empty()
        || (types.is_empty() && !untyped_narrows)
        || p.selectors
            .iter()
            .filter(|s| s.every_attribute)
            .any(|s| types.is_empty() || s.types.iter().any(|t| types.contains(t)))
}

/// Whether an entry of prohibition `p` naming Attributes takes back `attribute` (an IRI)
/// of one of `types`, or of any type when `types` is empty (T-5842).
#[must_use]
pub fn withholds(p: &Policy, types: &[String], attribute: &str) -> bool {
    p.prohibit
        && p.selectors.iter().any(|s| {
            !s.every_attribute
                && (types.is_empty() || s.types.iter().any(|t| types.contains(t)))
                && s.attributes.iter().any(|a| a == attribute)
        })
}

/// The Attributes the prohibitions among `applying` take back, per type: only the types of
/// `types` when it names any (T-5842).
fn withheld<'p>(
    applying: impl Iterator<Item = &'p Policy>,
    types: &[String],
) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for s in applying
        .filter(|p| p.prohibit)
        .flat_map(|p| &p.selectors)
        .filter(|s| !s.every_attribute)
    {
        for t in s
            .types
            .iter()
            .filter(|t| types.is_empty() || types.contains(t))
        {
            let names = out.entry(t.clone()).or_default();
            names.extend(s.attributes.iter().cloned());
            names.sort();
            names.dedup();
        }
    }
    out
}

/// Whether a write touches an Attribute of `withheld`: one it names or its body carries; a
/// replace (5.6.18, and an upsert, 5.6.8, whose mode the operation does not tell) also
/// removes what the stored Entity holds, and a write naming no Attribute and carrying no body
/// (a delete of the Entity) touches everything it holds, or everything when none is stored.
fn touches(op: &Operation<'_>, withheld: &BTreeMap<String, Vec<String>>) -> bool {
    if withheld.is_empty() {
        return false;
    }
    let hit = |name: &str| withheld.values().flatten().any(|w| w == name);
    let whole = op.attrs.is_empty() && op.body.is_none();
    if whole && op.stored.is_none() {
        return true;
    }
    op.attrs.iter().any(|a| hit(a))
        || op
            .body
            .and_then(Value::as_object)
            .is_some_and(|o| o.keys().any(|k| hit(k)))
        || ((whole || ["5.6.18", "5.6.8"].contains(&op.clause))
            && op
                .stored
                .is_some_and(|s| s.attribute_names.iter().any(|a| hit(a))))
}

fn unconditional_whole(p: &Policy) -> bool {
    p.selectors.is_empty() && !p.is_conditional()
}

/// List and Serve @context (ADR-0023 rule 5, T-4009): a stored `@context`
/// may define a term for any Attribute, so only a caller who may query every
/// Entity whole, and is prohibited none, sees the tenant's. Anyone else gets
/// a narrowing with one rule that shows nothing: the core `@context` alone.
fn names(table: &Table, caller: &Platform, now: DateTime<Utc>) -> Decision {
    let query = || held(table, caller, now).filter(|p| p.clauses.contains("5.7.2"));
    if !query().any(|p| p.prohibit) && query().any(|p| !p.prohibit && unconditional_whole(p)) {
        return Decision::Allow;
    }
    Decision::Filter(Filter {
        rules: vec![Rule::default()],
        restricted: caller.restricted,
        ..Filter::default()
    })
}

/// A read: one rule per selector of every grant (ADR-0023 rule 11), none of which shows a type
/// of `refused`, the requested types a prohibition covers (T-4455), and each type without the
/// Attributes `withheld` takes back (T-5842): the broker reads them as absent, so a `q` or an
/// `attrs` naming one selects nothing (T-1862).
fn read_filter(
    grants: &[&Policy],
    op: &Operation<'_>,
    (types, refused): (&[String], &[String]),
    withheld: BTreeMap<String, Vec<String>>,
    caller: &Platform,
) -> Decision {
    // With a type left out, a whole grant is a rule per type read, never the whole Tenant.
    if refused.is_empty() && grants.iter().any(|p| unconditional_whole(p)) {
        if withheld.is_empty() {
            return Decision::Allow;
        }
        return Decision::Filter(Filter {
            warning: left_out(&[], op, types, &withheld),
            omit_by_type: withheld,
            restricted: caller.restricted,
            ..Filter::default()
        });
    }
    let mut rules: Vec<Rule> = Vec::new();
    for p in grants {
        if p.selectors.is_empty() {
            // every type, under a condition: a rule per type the request names
            if types.is_empty() {
                return Decision::Deny(NAME_THE_TYPES.into());
            }
            rules.extend(types.iter().map(|t| Rule {
                types: vec![t.clone()],
                ..p.conditions.clone()
            }));
        } else {
            rules.extend(p.selectors.iter().cloned());
        }
    }
    // a rule every Attribute of which is taken back shows nothing, as the gateway's Deny
    let all_withheld = |rule: &Rule| {
        !rule.every_attribute
            && rule.attributes.iter().all(|a| {
                rule.types
                    .iter()
                    .all(|t| withheld.get(t).is_some_and(|w| w.contains(a)))
            })
    };
    let before = rules.len();
    rules.retain(|rule| !all_withheld(rule));
    if !refused.is_empty() || rules.len() < before {
        for rule in &mut rules {
            rule.types.retain(|t| !refused.contains(t));
        }
        rules.retain(|rule| !rule.types.is_empty());
        if rules.is_empty() {
            // No rule at all would show everything; one naming no type holds for nothing.
            rules.push(Rule::default());
        }
    }
    Decision::Filter(Filter {
        warning: left_out(&rules, op, types, &withheld),
        omit_by_type: withheld,
        rules,
        restricted: caller.restricted,
        ..Filter::default()
    })
}

/// The advisory naming the types the request's filter leaves out (map K18, MP-02), as the
/// gateway's oracle sent it: a type whose rules do not show an Attribute `q` names, or, without
/// a `q`, none of the Attributes `attrs` names, answers nothing, and the caller would read that
/// as no data. Only types the grants show (and the request names, when it names any) are
/// named, so the text tells nothing the caller may not read. An `orderBy` leaves no Entity out
/// and is not counted. An Attribute of `withheld` is one its type does not serve; with no
/// rule (a whole grant) every type it names shows every other Attribute.
fn left_out(
    rules: &[Rule],
    op: &Operation<'_>,
    types: &[String],
    withheld: &BTreeMap<String, Vec<String>>,
) -> Option<Warning> {
    let ctx = context();
    // The handler hands `q` over as written; its names expand as the grants' own did.
    let referenced: Vec<String> =
        op.q.map(|q| q.attribute_paths())
            .unwrap_or_default()
            .into_iter()
            .filter(|name| antares_ql::is_non_system_attr(name))
            .map(|name| ctx.expand_key(name))
            .collect();
    let asked: Vec<&str> = match op.q {
        Some(_) => Vec::new(),
        None => op.attrs.iter().map(String::as_str).collect(),
    };
    if referenced.is_empty() && asked.is_empty() {
        return None;
    }
    // What each type shows: `None` is every Attribute.
    let mut shown: BTreeMap<&str, Option<BTreeSet<&str>>> = BTreeMap::new();
    for rule in rules {
        for t in rule
            .types
            .iter()
            .filter(|t| types.is_empty() || types.contains(t))
        {
            let entry = shown.entry(t).or_insert_with(|| Some(BTreeSet::new()));
            match (entry.as_mut(), rule.every_attribute) {
                (Some(held), false) => held.extend(rule.attributes.iter().map(String::as_str)),
                _ => *entry = None,
            }
        }
    }
    if rules.is_empty() {
        for t in withheld.keys() {
            shown.insert(t, None);
        }
    }
    let names: Vec<String> = shown
        .into_iter()
        .filter_map(|(t, attributes)| {
            let hidden = withheld.get(t);
            let serves = |a: &str| {
                attributes.as_ref().is_none_or(|s| s.contains(a))
                    && hidden.is_none_or(|h| !h.iter().any(|w| w == a))
            };
            let unserved = referenced.iter().any(|a| !serves(a))
                || (!asked.is_empty() && !asked.iter().any(|a| serves(a)));
            unserved.then(|| ctx.compact_iri(t))
        })
        .collect();
    warning(&names)
}

/// The gateway's text for `names`, as many of them as one header line holds.
fn warning(names: &[String]) -> Option<Warning> {
    let verb = if names.len() == 1 { "was" } else { "were" };
    let tail = |shown: usize| match names.len().saturating_sub(shown) {
        0 => format!(
            " {verb} not queried: this request selects or orders on an attribute they do not \
             serve on this endpoint"
        ),
        more => format!(
            " and {more} more {verb} not queried: this request selects or orders on an \
             attribute they do not serve on this endpoint"
        ),
    };
    let mut listed = String::new();
    let mut text = None;
    for (at, name) in names.iter().enumerate() {
        if !listed.is_empty() {
            listed.push_str(", ");
        }
        listed.push_str(name);
        let line = format!("{listed}{}", tail(at.saturating_add(1)));
        if line.len() > Warning::MAX_LEN {
            break;
        }
        text = Some(line);
    }
    Warning::new(&text?).ok()
}

/// How one grant admits a write.
enum Admit {
    Whole,
    If(Rule),
}

/// A write: whole inside one grant, or refused whole (platform GW16 to
/// GW19); a grant with a condition on the stored Entity answers `AllowIf`
/// on a write by id.
fn write(grants: &[&Policy], op: &Operation<'_>, types: &[String]) -> Decision {
    if let Some(name) = smuggled(op) {
        return Decision::Deny(format!(
            "{name:?} is access control and does not belong in an Entity"
        ));
    }
    // Every type the Entity has or gets needs a grant of its own (F-R2ANT1-001): a grant of
    // one type matching would let a body naming it open an Entity of any other type.
    let each: Vec<&[String]> = if types.is_empty() {
        vec![types]
    } else {
        types.iter().map(std::slice::from_ref).collect()
    };
    let mut conditional = Vec::new();
    let mut conditional_types = 0usize;
    for one in each {
        let mut rules = Vec::new();
        let mut whole = false;
        for admit in grants.iter().flat_map(|p| admits(p, op, one)) {
            match admit {
                Admit::Whole => whole = true,
                Admit::If(rule) => rules.push(rule),
            }
        }
        if whole {
            continue;
        }
        if rules.is_empty() {
            return Decision::Deny(NOT_GRANTED.into());
        }
        conditional_types = conditional_types.saturating_add(1);
        conditional.extend(rules);
    }
    // Decided (worker-t6211, owner may override): a filter's rules are or-ed, so conditions
    // of two types cannot both be required of one Entity; such a write is refused.
    match conditional_types {
        0 => Decision::Allow,
        1 => Decision::AllowIf(Filter {
            rules: conditional,
            ..Filter::default()
        }),
        _ => Decision::Deny(NOT_GRANTED.into()),
    }
}

/// Every way `p` admits the write: one per selector it falls inside.
fn admits(p: &Policy, op: &Operation<'_>, types: &[String]) -> Vec<Admit> {
    // a window is about reading history; it admits no write
    if !p.conditions.temporal.is_empty() {
        return Vec::new();
    }
    // a grant of every type without a condition takes the write whole, a write naming no type
    // included (5.6.11 names none; a write by id of an absent Entity has no stored type)
    if p.selectors.is_empty() && !p.is_conditional() {
        return vec![Admit::Whole];
    }
    let every_type: Vec<Rule>;
    let selectors = if p.selectors.is_empty() {
        every_type = types
            .iter()
            .map(|t| Rule {
                types: vec![t.clone()],
                ..p.conditions.clone()
            })
            .collect();
        &every_type
    } else {
        &p.selectors
    };
    let ctx = context();
    selectors
        .iter()
        .filter(|s| s.types.iter().any(|t| types.contains(t)))
        .filter(|s| op.ids.iter().all(|id| s.selects_id(id)))
        .filter(|s| s.attributes.is_empty() || op.attrs.iter().all(|a| s.attributes.contains(a)))
        .filter(|s| inside_scope(s, op.body))
        .filter(|s| inside_area(s, op.body))
        .filter_map(|s| {
            if !p.is_conditional() {
                Some(Admit::Whole)
            } else if CREATES.contains(&op.clause) {
                op.body
                    .is_some_and(|b| s.holds(b, ctx))
                    .then_some(Admit::Whole)
            } else if WRITE_BY_ID.contains(&op.clause) {
                Some(Admit::If(Rule {
                    attributes: Vec::new(),
                    every_attribute: false,
                    temporal: Vec::new(),
                    ..s.clone()
                }))
            } else {
                None
            }
        })
        .collect()
}

/// The body's `scope` lies inside the policy's scope tree (R29, R30).
fn inside_scope(s: &Rule, body: Option<&Value>) -> bool {
    let Some(granted) = s.scope_q.as_deref() else {
        return true;
    };
    let scopes = match body.and_then(|b| b.get("scope")) {
        None => return true,
        Some(Value::String(one)) => vec![one.as_str()],
        Some(Value::Array(many)) => many.iter().filter_map(Value::as_str).collect(),
        Some(_) => return false,
    };
    scopes
        .iter()
        .all(|scope| scope_paths(granted).any(|path| covers(path, scope)))
}

/// The body's own location lies inside the policy's area (GW16); a body
/// that carries none is not placed outside it.
fn inside_area(s: &Rule, body: Option<&Value>) -> bool {
    let ctx = context();
    s.areas().all(|area| {
        let iri = if area.geoproperty.is_empty() {
            antares_ql::geo::LOCATION_IRI.to_owned()
        } else {
            ctx.expand_key(&area.geoproperty)
        };
        body.is_none_or(|b| b.get(&iri).is_none() || area.matches(b, ctx))
    })
}

/// A member of [`POLICY_VOCABULARY`] the write carries, if any.
fn smuggled(op: &Operation<'_>) -> Option<&'static str> {
    let ctx = context();
    POLICY_VOCABULARY.into_iter().find(|name| {
        let iri = ctx.expand_key(name);
        op.attrs.iter().any(|a| a == name || *a == iri)
            || op
                .body
                .and_then(Value::as_object)
                .is_some_and(|o| o.contains_key(*name) || o.contains_key(&iri))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::compile;
    use antares_api::policy::StoredImage;
    use serde_json::json;

    const AREA: &str =
        "georel=within;geometry=Polygon;coordinates=[[[0,0],[1,0],[1,1],[0,1],[0,0]]]";

    fn table(policy: &Value) -> Result<Table, String> {
        let mut p = json!({"id": "p", "assignee": {"kind": "role", "id": "r"},
                           "operations": ["5.6.1", "5.6.8", "5.8.1"],
                           "information": [{"entities": [{"type": "Car"}]}]});
        if let (Some(o), Some(extra)) = (p.as_object_mut(), policy.as_object()) {
            o.extend(extra.clone());
        }
        compile(&json!({ "policies": [p] }))
    }

    fn caller() -> Caller {
        Caller::Platform(Platform {
            roles: ["r".to_owned()].into(),
            ..Platform::default()
        })
    }

    fn car() -> Vec<String> {
        vec![context().expand_key("Car")]
    }

    fn on(table: &Table, clause: &'static str, body: Option<&Value>) -> Decision {
        let types = car();
        let op = Operation {
            types: &types,
            body,
            ..Operation::new(clause)
        };
        decide(Some(table), &caller(), &op, Utc::now())
    }

    fn iri(name: &str) -> String {
        context().expand_key(name)
    }

    /// `on`, with the request's types, Attributes and the caller's
    /// restriction given.
    fn with(
        table: &Table,
        clause: &'static str,
        types: &[String],
        attrs: &[String],
        body: Option<&Value>,
        restricted: bool,
    ) -> Decision {
        let op = Operation {
            types,
            attrs,
            body,
            ..Operation::new(clause)
        };
        let caller = Caller::Platform(Platform {
            roles: ["r".to_owned()].into(),
            restricted,
            ..Platform::default()
        });
        decide(Some(table), &caller, &op, Utc::now())
    }

    /// T-3729: a prohibition naming no type covers a typed operation, and
    /// one naming types covers an operation that names none.
    /// Every clause the operation layer gates (`gate!` in antares-api and antares-ops).
    const CLAUSES: [&str; 62] = [
        "5.6.1", "5.6.2", "5.6.3", "5.6.4", "5.6.5", "5.6.6", "5.6.7", "5.6.8", "5.6.9", "5.6.10",
        "5.6.11", "5.6.12", "5.6.13", "5.6.14", "5.6.15", "5.6.16", "5.6.17", "5.6.18", "5.6.19",
        "5.6.20", "5.6.21", "5.7.1", "5.7.2", "5.7.3", "5.7.4", "5.7.5", "5.7.6", "5.7.7", "5.7.8",
        "5.7.9", "5.7.10", "5.8.1", "5.8.2", "5.8.3", "5.8.4", "5.8.5", "5.9.2", "5.9.3", "5.9.4",
        "5.10.1", "5.10.2", "5.11.2", "5.11.3", "5.11.4", "5.11.5", "5.11.6", "5.13.2", "5.13.3",
        "5.13.4", "5.13.5", "5.14.1", "5.14.2", "5.14.3", "5.14.4", "5.14.5", "5.15.1", "5.16.1",
        "5.16.2", "5.16.3", "5.16.4", "5.16.5", "5.16.7",
    ];

    /// S-49 (T-5955): a read-only caller granted every clause is refused each one that is no
    /// read, by the request and by the tool probe, and decided on each read as before.
    #[test]
    fn a_read_only_caller_is_refused_every_write_whatever_it_is_granted() -> Result<(), String> {
        let t = compile(&json!({ "policies": [{"id": "all",
            "assignee": {"kind": "role", "id": "r"}, "operations": CLAUSES.to_vec()}] }))?;
        let full = caller();
        let read_only = Caller::Platform(Platform {
            roles: ["r".to_owned()].into(),
            read_only: true,
            ..Platform::default()
        });
        let types = car();
        let mut refused = Vec::new();
        for clause in CLAUSES {
            let op = Operation {
                types: &types,
                ..Operation::new(clause)
            };
            let asked = decide(Some(&t), &read_only, &op, Utc::now());
            let offered = probe(Some(&t), &read_only, &op, Utc::now());
            if asked == Decision::Deny(READ_ONLY.into()) {
                assert_eq!(offered, asked, "{clause}");
                refused.push(clause);
            } else {
                assert_eq!(asked, decide(Some(&t), &full, &op, Utc::now()), "{clause}");
            }
        }
        let read: Vec<&str> = CLAUSES.into_iter().filter(|c| reads(c)).collect();
        assert_eq!(
            refused.len(),
            CLAUSES.len().saturating_sub(read.len()),
            "{refused:?}"
        );
        assert!(refused.iter().all(|c| !reads(c)), "{refused:?}");
        // a read-only caller with no rule document is refused a write for being read-only
        let op = Operation::new("5.6.1");
        assert_eq!(
            decide(None, &read_only, &op, Utc::now()),
            Decision::Deny(READ_ONLY.into())
        );
        Ok(())
    }

    #[test]
    fn a_prohibition_covers_what_its_types_do_not_tell_apart() -> Result<(), String> {
        let prohibited = Decision::Deny(PROHIBITED.into());
        let every_type = table(&json!({"effect": "prohibit", "information": [],
                                       "operations": ["5.7.2"]}))?;
        assert_eq!(
            with(&every_type, "5.7.2", &car(), &[], None, false),
            prohibited
        );
        let cars = table(&json!({"effect": "prohibit", "operations": ["5.7.2"]}))?;
        assert_eq!(with(&cars, "5.7.2", &[], &[], None, false), prohibited);
        assert_eq!(
            with(&cars, "5.7.2", &[iri("Bus")], &[], None, false),
            Decision::Deny(NOT_GRANTED.into()),
            "another type is not prohibited, nor granted"
        );
        Ok(())
    }

    /// T-4855: a grant without information and without a condition admits a write that names
    /// no type (Upsert Temporal, 5.6.11, names none); a conditional one still refuses it, and a
    /// prohibition naming no type still wins.
    #[test]
    fn a_grant_of_every_type_admits_a_write_naming_none() -> Result<(), String> {
        let every_type = table(&json!({"information": [], "operations": ["5.6.11", "5.6.16"]}))?;
        for clause in ["5.6.11", "5.6.16"] {
            assert_eq!(
                with(&every_type, clause, &[], &[], None, false),
                Decision::Allow
            );
        }
        let conditional = table(&json!({"information": [], "operations": ["5.6.11"],
                                        "q": "speed<100"}))?;
        assert_eq!(
            with(&conditional, "5.6.11", &[], &[], None, false),
            Decision::Deny(NOT_GRANTED.into())
        );
        let grant = json!({"id": "all", "assignee": {"kind": "role", "id": "r"},
                           "operations": ["5.6.11"]});
        let prohibit = json!({"id": "no", "assignee": {"kind": "role", "id": "r"},
                              "effect": "prohibit", "operations": ["5.6.11"]});
        let both = compile(&json!({"policies": [grant, prohibit]}))?;
        assert_eq!(
            with(&both, "5.6.11", &[], &[], None, false),
            Decision::Deny(PROHIBITED.into())
        );
        let smuggled = json!({"id": "urn:ngsi-ld:Car:1", "type": "Car",
                              "acl": {"type": "Property", "value": "x"}});
        assert!(matches!(
            with(&every_type, "5.6.11", &[], &[], Some(&smuggled), false),
            Decision::Deny(_)
        ));
        Ok(())
    }

    /// T-4455: a read naming several types is decided per type. The prohibited type is left
    /// out of every rule and of the warning and the other is read; refused whole only when
    /// every type it names is prohibited. A write naming a prohibited type is still refused.
    #[test]
    fn a_read_of_several_types_leaves_out_the_prohibited_one() -> Result<(), String> {
        let prohibit_bus = json!({"id": "no-bus", "assignee": {"kind": "role", "id": "r"},
            "effect": "prohibit", "operations": ["5.7.2", "5.6.1"],
            "information": [{"entities": [{"type": "Bus"}]}]});
        let both = [iri("Car"), iri("Bus")];
        let selected = compile(&json!({"policies": [
            {"id": "read", "assignee": {"kind": "role", "id": "r"},
             "operations": ["5.7.2", "5.6.1"],
             "information": [{"entities": [{"type": "Car"}, {"type": "Bus"}]}]},
            prohibit_bus.clone()]}))?;
        let whole = compile(&json!({"policies": [
            {"id": "read", "assignee": {"kind": "role", "id": "r"},
             "operations": ["5.7.2", "5.6.1"], "information": []},
            prohibit_bus]}))?;
        for t in [&selected, &whole] {
            let Decision::Filter(f) = with(t, "5.7.2", &both, &[], None, false) else {
                return Err("a read of Car and Bus reads Car".into());
            };
            let shown: Vec<&String> = f.rules.iter().flat_map(|r| &r.types).collect();
            assert_eq!(shown, [&iri("Car")], "no rule shows Bus");
            assert!(f.rules.iter().all(|r| r.every_attribute));
            assert!(f.warning.is_none(), "the refusal names nothing");
            assert_eq!(
                with(t, "5.7.2", &[iri("Bus")], &[], None, false),
                Decision::Deny(PROHIBITED.into())
            );
            assert_eq!(
                with(t, "5.6.1", &both, &[], None, false),
                Decision::Deny(PROHIBITED.into()),
                "a write is refused whole"
            );
        }
        assert_eq!(
            with(&whole, "5.7.2", &car(), &[], None, false),
            Decision::Allow,
            "nothing left out: the whole grant"
        );
        // a grant of Bus alone, read with Car beside it: no rule shows anything
        let bus_only = compile(&json!({"policies": [
            {"id": "read", "assignee": {"kind": "role", "id": "r"}, "operations": ["5.7.2"],
             "information": [{"entities": [{"type": "Bus"}]}]},
            {"id": "no-bus", "assignee": {"kind": "role", "id": "r"}, "effect": "prohibit",
             "operations": ["5.7.2"], "information": [{"entities": [{"type": "Bus"}]}]}]}))?;
        let Decision::Filter(f) = with(&bus_only, "5.7.2", &both, &[], None, false) else {
            return Err("a read of Car and Bus is narrowed".into());
        };
        assert_eq!(
            f.rules,
            vec![Rule::default()],
            "a rule that holds for nothing"
        );
        Ok(())
    }

    /// T-3729: a narrowed read carries the caller's wish to be told.
    #[test]
    fn a_read_carries_the_callers_restriction() -> Result<(), String> {
        let t = table(&json!({"operations": ["5.7.2"]}))?;
        for restricted in [true, false] {
            let Decision::Filter(f) = with(&t, "5.7.2", &car(), &[], None, restricted) else {
                return Err("a read by selector is a filter".into());
            };
            assert_eq!(f.restricted, restricted);
        }
        Ok(())
    }

    /// T-3729: the condition a write by id is held to is the selector's
    /// condition alone, never what it shows or a window of its history.
    #[test]
    fn a_held_write_carries_the_condition_alone() -> Result<(), String> {
        let speed = [iri("speed")];
        let mut t = table(&json!({"q": "speed>1", "operations": ["5.6.3"],
            "information": [{"entities": [{"type": "Car"}], "propertyNames": ["speed"]}]}))?;
        let whole = table(&json!({"q": "speed>1", "operations": ["5.6.3"]}))?;
        let window = table(&json!({"temporalQ": "timerel=after;timeAt=P-1D"}))?
            .policies
            .first()
            .map(|p| p.conditions.temporal.clone())
            .filter(|w| !w.is_empty())
            .ok_or("no window")?;
        // a selector given a window of its own still admits no history
        if let Some(s) = t.policies.first_mut().and_then(|p| p.selectors.first_mut()) {
            s.temporal = window;
        }
        for t in [&t, &whole] {
            let Decision::AllowIf(held) = with(t, "5.6.3", &car(), &speed, None, false) else {
                return Err("a conditioned write by id is held".into());
            };
            let rule = held.rules.first().ok_or("no rule")?;
            assert!(rule.q.is_some() && rule.types == car());
            assert!(
                rule.attributes.is_empty() && !rule.every_attribute && rule.temporal.is_empty()
            );
        }
        Ok(())
    }

    /// T-3729: a write by id whose body moves the Entity out of the granted
    /// scope is refused before it is held to any condition.
    #[test]
    fn a_write_by_id_into_another_scope_is_refused() -> Result<(), String> {
        let t = table(&json!({"scopeQ": "/city", "operations": ["5.6.3"]}))?;
        let body = |s: &str| json!({ "scope": s });
        assert!(matches!(
            with(&t, "5.6.3", &car(), &[], Some(&body("/city/a")), false),
            Decision::AllowIf(_)
        ));
        assert_eq!(
            with(&t, "5.6.3", &car(), &[], Some(&body("/town")), false),
            Decision::Deny(NOT_GRANTED.into())
        );
        Ok(())
    }

    /// T-3729: access control written into the data is refused in each
    /// place it can hide: an Attribute named short or expanded, a body
    /// member short or expanded.
    #[test]
    fn access_control_in_the_data_is_refused_in_every_form() -> Result<(), String> {
        let t = table(&json!({}))?;
        let plain = json!({ "type": car() });
        assert_eq!(
            with(&t, "5.6.1", &car(), &[], Some(&plain), false),
            Decision::Allow
        );
        let member = |k: String| json!({ "type": car(), (k): 1 });
        for (attrs, body) in [
            (vec!["owner".to_owned()], plain.clone()),
            (vec![iri("owner")], plain.clone()),
            (Vec::new(), member("acl".into())),
            (Vec::new(), member(iri("acl"))),
        ] {
            let d = with(&t, "5.6.1", &car(), &attrs, Some(&body), false);
            assert!(
                matches!(&d, Decision::Deny(why) if why.contains("is access control")),
                "{attrs:?} {body}: {d:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn the_stored_contexts_are_listed_only_to_a_caller_who_queries_every_entity_whole(
    ) -> Result<(), String> {
        // T-4009: a stored @context may define a term for an Attribute the
        // caller's grant hides, so a narrowed reader lists the core alone
        let ops = json!(["5.7.2", "5.13.3", "5.13.4"]);
        let whole = table(&json!({"operations": ops, "information": []}))?;
        let cars = table(&json!({"operations": ops}))?;
        let speed = table(&json!({"operations": ops,
            "information": [{"entities": [{"type": "Car"}], "propertyNames": ["speed"]}]}))?;
        let no_query = table(&json!({"operations": ["5.13.3", "5.13.4"], "information": []}))?;
        for clause in ["5.13.3", "5.13.4"] {
            assert_eq!(on(&whole, clause, None), Decision::Allow, "{clause}");
            for (name, t) in [("cars", &cars), ("speed", &speed), ("no query", &no_query)] {
                let Decision::Filter(f) = on(t, clause, None) else {
                    return Err(format!("{name} {clause}: a narrowing"));
                };
                assert!(!f.is_empty(), "{name} {clause}");
            }
        }
        let not_listed = table(&json!({"operations": ["5.7.2"], "information": []}))?;
        assert_eq!(
            on(&not_listed, "5.13.3", None),
            Decision::Deny(NOT_GRANTED.into())
        );
        Ok(())
    }

    #[test]
    fn a_granted_clause_outside_the_entity_operations_is_allowed() -> Result<(), String> {
        let t = table(&json!({"q": "speed>1"}))?;
        assert_eq!(on(&t, "5.8.1", None), Decision::Allow);
        Ok(())
    }

    #[test]
    fn a_conditional_grant_holds_a_batch_upsert_by_id_and_a_window_no_write() -> Result<(), String>
    {
        // T-3668: an upserted member is judged on the stored Entity inside
        // the store write, and a conditioned upsert never creates
        let t = table(&json!({"q": "speed>1"}))?;
        let Decision::AllowIf(held) = on(&t, "5.6.8", None) else {
            return Err("a conditioned upsert is held to the condition".into());
        };
        assert_eq!(held.rules.len(), 1);
        assert!(held
            .rules
            .first()
            .is_some_and(|r| r.q.is_some() && r.types == car()));
        let t = table(&json!({"temporalQ": "timerel=after;timeAt=P-1D"}))?;
        assert_eq!(on(&t, "5.6.1", None), Decision::Deny(NOT_GRANTED.into()));
        Ok(())
    }

    #[test]
    fn a_body_scope_is_judged_in_each_shape() -> Result<(), String> {
        let t = table(&json!({"scopeQ": "/city"}))?;
        let scope = |s: Value| json!({ "type": car(), "scope": s });
        for (body, granted) in [
            // a create outside every scope does not hold the condition
            (json!({ "type": car() }), false),
            (scope(json!("/city/north")), true),
            (scope(json!(["/city", "/city/a"])), true),
            (scope(json!("/town")), false),
            (scope(json!(7)), false),
        ] {
            let want = if granted {
                Decision::Allow
            } else {
                Decision::Deny(NOT_GRANTED.into())
            };
            assert_eq!(on(&t, "5.6.1", Some(&body)), want, "{body}");
        }
        Ok(())
    }

    #[test]
    fn an_area_on_a_named_geoproperty_reads_that_member() -> Result<(), String> {
        let t = table(&json!({"geoQ": format!("{AREA};geoproperty=parking")}))?;
        let at = |x: f64| {
            json!({ "type": car(), context().expand_key("parking"): [{"type": "GeoProperty",
                "value": {"type": "Point", "coordinates": [x, 0.5]}}] })
        };
        assert_eq!(on(&t, "5.6.1", Some(&at(0.5))), Decision::Allow);
        assert_eq!(
            on(&t, "5.6.1", Some(&at(5.0))),
            Decision::Deny(NOT_GRANTED.into())
        );
        Ok(())
    }

    /// The warning a read by `q` (or `attrs` alone) carries, over a grant
    /// showing `speed` of a Car and `name` of a Bus.
    fn left_out_by(
        types: &[&str],
        q: Option<&str>,
        attrs: &[&str],
    ) -> Result<Option<String>, String> {
        let t = table(&json!({"operations": ["5.7.2"], "information": [
            {"entities": [{"type": "Car"}], "propertyNames": ["speed"]},
            {"entities": [{"type": "Bus"}], "propertyNames": ["name"]}]}))?;
        let types: Vec<String> = types.iter().map(|n| iri(n)).collect();
        let attrs: Vec<String> = attrs.iter().map(|n| iri(n)).collect();
        let q = q
            .map(antares_ql::parse_q)
            .transpose()
            .map_err(|e| e.to_string())?;
        let op = Operation {
            types: &types,
            attrs: &attrs,
            q: q.as_ref(),
            ..Operation::new("5.7.2")
        };
        let Decision::Filter(f) = decide(Some(&t), &caller(), &op, Utc::now()) else {
            return Err("a read by selector is a filter".into());
        };
        Ok(f.warning.map(|w| w.texts().join("; ")))
    }

    const WAS: &str = "was not queried: this request selects or orders on an attribute they do \
                       not serve on this endpoint";
    const WERE: &str = "were not queried: this request selects or orders on an attribute they \
                        do not serve on this endpoint";

    /// T-3796 (MP-02): a filter on an attribute a type does not show names
    /// that type, as the gateway did.
    #[test]
    fn a_filter_names_the_one_type_it_left_out() -> Result<(), String> {
        assert_eq!(
            left_out_by(&["Car", "Bus"], Some("speed>1"), &[])?,
            Some(format!("Bus {WAS}"))
        );
        assert_eq!(
            left_out_by(&["Car", "Bus"], None, &["name"])?,
            Some(format!("Car {WAS}")),
            "attrs alone leaves out the type that shows none of them"
        );
        Ok(())
    }

    /// T-3796: two types left out are both named; none left out is no warning.
    #[test]
    fn a_filter_names_two_types_or_none() -> Result<(), String> {
        assert_eq!(
            left_out_by(&[], Some("colour==\"red\""), &[])?,
            Some(format!("Bus, Car {WERE}"))
        );
        assert_eq!(left_out_by(&["Car"], Some("speed>1"), &[])?, None);
        assert_eq!(
            left_out_by(&["Car", "Bus"], None, &["speed", "name"])?,
            None
        );
        assert_eq!(left_out_by(&["Car", "Bus"], None, &[])?, None);
        assert_eq!(
            left_out_by(&["Car"], Some("createdAt>\"2020-01-01T00:00:00Z\""), &[])?,
            None,
            "a system member is never left out"
        );
        Ok(())
    }

    /// T-3796: a type the subject may not read is never named, and a type
    /// shown whole leaves nothing out.
    #[test]
    fn a_type_the_subject_may_not_read_is_never_named() -> Result<(), String> {
        assert_eq!(
            left_out_by(&["Car", "Secret"], Some("name==\"x\""), &[])?,
            Some(format!("Car {WAS}"))
        );
        let whole = table(&json!({"operations": ["5.7.2"]}))?;
        let q = antares_ql::parse_q("anything>1").map_err(|e| e.to_string())?;
        let op = Operation {
            types: &car(),
            q: Some(&q),
            ..Operation::new("5.7.2")
        };
        let Decision::Filter(f) = decide(Some(&whole), &caller(), &op, Utc::now()) else {
            return Err("a read by selector is a filter".into());
        };
        assert_eq!(f.warning, None);
        Ok(())
    }

    /// A write shape: clause, Attributes, body, stored image, and whether it is allowed.
    type Case<'a> = (
        &'static str,
        &'a [String],
        Option<&'a Value>,
        Option<&'a StoredImage>,
        bool,
    );

    /// T-5842: a whole grant and a prohibition taking back `colour` of Car.
    fn colour_withheld(ops: &[&str]) -> Result<Table, String> {
        let role = json!({"kind": "role", "id": "r"});
        compile(&json!({"policies": [
            {"id": "all", "assignee": role, "operations": ops},
            {"id": "no", "effect": "prohibit", "assignee": role, "operations": ops,
             "information": [{"entities": [{"type": "Car"}], "propertyNames": ["colour"]}]}
        ]}))
    }

    /// T-5842: a read under a whole grant loses the withheld Attribute, and a `q` on it names
    /// the type in the warning; a `q` on another Attribute warns of nothing.
    #[test]
    fn a_whole_read_omits_a_withheld_attribute_and_warns_of_a_q_on_it() -> Result<(), String> {
        let t = colour_withheld(&["5.7.2"])?;
        for (q, warned) in [("colour==\"red\"", true), ("speed>1", false)] {
            let q = antares_ql::parse_q(q).map_err(|e| e.to_string())?;
            let op = Operation {
                types: &car(),
                q: Some(&q),
                ..Operation::new("5.7.2")
            };
            let Decision::Filter(f) = decide(Some(&t), &caller(), &op, Utc::now()) else {
                return Err("a read narrowed by an Attribute is a filter".into());
            };
            assert_eq!(f.omit_by_type.get(&iri("Car")), Some(&vec![iri("colour")]));
            assert_eq!(f.rules, Vec::new());
            assert_eq!(f.warning.is_some(), warned);
        }
        // a read naming no type loses it too, and is not refused
        let Decision::Filter(f) = decide(Some(&t), &caller(), &Operation::new("5.7.2"), Utc::now())
        else {
            return Err("an untyped read narrowed by an Attribute is a filter".into());
        };
        assert_eq!(f.omit_by_type.len(), 1);
        Ok(())
    }

    /// T-5842: which writes touch a withheld Attribute: one it names or its body carries, a
    /// replace of a stored Entity holding it, and a delete of the Entity.
    #[test]
    fn a_write_touching_a_withheld_attribute_is_refused() -> Result<(), String> {
        let ops = ["5.6.2", "5.6.6", "5.6.18"];
        let t = colour_withheld(&ops)?;
        let types = car();
        let holding = StoredImage {
            types: types.clone(),
            scope: Vec::new(),
            attribute_names: vec![iri("colour"), iri("speed")],
        };
        let without = StoredImage {
            attribute_names: vec![iri("speed")],
            ..holding.clone()
        };
        let speed = vec![iri("speed")];
        let colour = vec![iri("colour")];
        let body = json!({ (iri("speed")): [] });
        let coloured = json!({ (iri("colour")): [] });
        let cases: [Case; 9] = [
            ("5.6.2", &speed, Some(&body), Some(&holding), true),
            ("5.6.2", &colour, Some(&coloured), Some(&holding), false),
            ("5.6.2", &[], Some(&coloured), Some(&holding), false),
            ("5.6.18", &speed, Some(&body), Some(&without), true),
            ("5.6.18", &speed, Some(&body), Some(&holding), false),
            ("5.6.6", &[], None, Some(&without), true),
            ("5.6.6", &[], None, Some(&holding), false),
            ("5.6.6", &[], None, None, false),
            ("5.6.2", &speed, None, None, true),
        ];
        for (clause, attrs, body, stored, allowed) in cases {
            let op = Operation {
                types: &types,
                attrs,
                body,
                stored,
                ..Operation::new(clause)
            };
            let d = decide(Some(&t), &caller(), &op, Utc::now());
            assert_eq!(d == Decision::Allow, allowed, "{clause} {attrs:?} {d:?}");
        }
        Ok(())
    }

    /// ADR-N-066 2.1: a grant editing `speed` but not `colour` allows a write of the first
    /// and refuses one of the second, and a history read of `colour` it never gives.
    #[test]
    fn a_capability_block_decides_each_attribute_per_operation() -> Result<(), String> {
        let t = table(&json!({"operations": ["5.6.2", "5.7.2", "5.7.3"],
            "information": [{"entities": [{"type": "Car"}],
                             "propertyNames": ["speed", "colour"],
                             "attributes": {"*": {"write": [], "history": false},
                                            "speed": {"write": ["update"], "history": true}}}]}))?;
        let types = car();
        let (speed, colour) = (vec![iri("speed")], vec![iri("colour")]);
        let write =
            |attrs: &[String], body: &Value| with(&t, "5.6.2", &types, attrs, Some(body), false);
        assert_eq!(
            write(&speed, &json!({ (iri("speed")): [] })),
            Decision::Allow
        );
        assert!(matches!(
            write(&colour, &json!({ (iri("colour")): [] })),
            Decision::Deny(_)
        ));
        let shown = |clause: &'static str| match on(&t, clause, None) {
            Decision::Filter(f) => Ok(f
                .rules
                .iter()
                .flat_map(|r| r.attributes.clone())
                .collect::<Vec<_>>()),
            other => Err(format!("{clause}: {other:?}")),
        };
        assert_eq!(shown("5.7.2")?, [iri("speed"), iri("colour")]);
        assert_eq!(shown("5.7.3")?, [iri("speed")]);
        Ok(())
    }

    /// ADR-N-066 2.4: a block passes per Attribute the members it lists, else `*`'s, else
    /// the audience defaults; a prohibition takes back what it lists; no block narrows none.
    #[test]
    fn a_capability_block_passes_metadata_members_per_attribute() -> Result<(), String> {
        let members = |d: Decision| match d {
            Decision::Filter(f) => Ok(f.metadata),
            other => Err(format!("{other:?}")),
        };
        let blockless = table(&json!({"operations": ["5.7.2"]}))?;
        assert_eq!(members(on(&blockless, "5.7.2", None))?, None);
        let t = table(&json!({"operations": ["5.7.2", "5.7.6"],
            "information": [{"entities": [{"type": "Car"}],
                             "propertyNames": ["speed", "colour"],
                             "attributes": {"speed": {"metadata": ["observedAt"]}}}]}))?;
        let got = members(on(&t, "5.7.2", None))?.ok_or("no members")?;
        let set = |m: &[&str]| m.iter().map(|s| (*s).to_owned()).collect::<BTreeSet<_>>();
        assert_eq!(got.named.get(&iri("speed")), Some(&set(&["observedAt"])));
        assert_eq!(
            got.rest,
            set(&[
                "observedAt",
                "unitCode",
                "modifiedAt",
                "modifiedBy.organization",
                "modifiedBy.actorKind"
            ])
        );
        // a type list carries no data, so it strips nothing
        assert!(matches!(
            on(&t, "5.7.6", None),
            Decision::Allow | Decision::Filter(Filter { metadata: None, .. })
        ));
        let with_prohibition = compile(&json!({"policies": [
            {"id": "p", "assignee": {"kind": "role", "id": "r"}, "operations": ["5.7.2"],
             "information": [{"entities": [{"type": "Car"}]}]},
            {"id": "n", "effect": "prohibit", "assignee": {"kind": "role", "id": "r"},
             "operations": ["5.7.2"],
             "information": [{"entities": [{"type": "Car"}], "propertyNames": ["colour"],
                              "attributes": {"*": {"metadata": ["unitCode"]}}}]}]}))?;
        let got = members(on(&with_prohibition, "5.7.2", None))?.ok_or("no members")?;
        assert!(!got.rest.contains("unitCode") && got.rest.contains("observedAt"));
        Ok(())
    }

    /// T-3796: the names one header line cannot hold are counted, not cut.
    #[test]
    fn the_warning_fits_one_header_line() -> Result<(), String> {
        let names: Vec<String> = (0..40)
            .map(|i| format!("AVeryLongTypeName{i:02}"))
            .collect();
        let w = warning(&names).ok_or("no warning")?;
        let text = w.texts().join("");
        assert!(text.len() <= Warning::MAX_LEN, "{text}");
        assert!(
            text.starts_with("AVeryLongTypeName00, AVeryLongTypeName01"),
            "{text}"
        );
        assert!(text.contains(" more were not queried"), "{text}");
        assert_eq!(warning(&[]), None);
        assert_eq!(
            warning(&["x".repeat(300)]),
            None,
            "a name past the bound is not cut"
        );
        Ok(())
    }
}
