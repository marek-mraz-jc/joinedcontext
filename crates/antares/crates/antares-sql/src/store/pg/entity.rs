// SPDX-License-Identifier: EUPL-1.2
//! PgStore, first slice: entity CRUD over the `entities`
//! table. Same signatures as the in-memory `Store`, awaited: every method is
//! an `async fn` over sqlx, so a caller waiting on the database holds no
//! thread.
//!
//! Extracted columns are computed in Rust at write time (no triggers):
//! `types`, `scopes`, `created_at`, `modified_at`, `expires_at` and
//! `location`, the default GeoProperty, converted by PostGIS itself from
//! bound GeoJSON text (`ST_GeomFromGeoJSON` rather than a geozero
//! dependency — the DB already owns the conversion, and the value still
//! travels as a bind).

use antares_model::{NgsiError, TenantId};
use antares_ql::{CmpOp, QNode, QValue};
use serde_json::Value;
use sqlx::postgres::PgPool;
use sqlx::Row;

pub struct PgEntityStore {
    pool: PgPool,
    /// When on, every entity write enqueues its change event into the
    /// outbox INSIDE the write transaction (a crash between commit and
    /// publish can never lose an event). Off by default: with `bus = local`
    /// events flow through the in-process hook and undrained rows would only
    /// grow the table. The broker turns this on when `bus = nats`.
    outbox: std::sync::atomic::AtomicBool,
    /// When on, the batch writes (create, upsert, the batch read-modify-write)
    /// commit with `synchronous_commit = off`, set LOCAL to their own
    /// transaction (T-5050): a crash of Postgres loses at most the last
    /// ~3 x `wal_writer_delay` of committed batches, never their atomicity
    /// or the order of the WAL. Notifications already sent for a lost batch
    /// stay sent. Off by default; the broker sets it from
    /// `ANTARES_PG_BATCH_ASYNC_COMMIT` for a feed that rewrites itself every
    /// second, where the next batch restores what a crash lost.
    async_batch: std::sync::atomic::AtomicBool,
    /// The pool the batch writes begin on: `pool` itself, or the write
    /// reserve (`ANTARES_PG_WRITE_RESERVE`, T-5150) when the broker set one,
    /// whose connections no read takes.
    batch_pool: PgPool,
}

// EntityFilter/Page/QueryOutcome live in `store::filter` (pure data,
// shared with the wasm32 build); re-exported here so existing paths hold.
pub use crate::store::filter::{EntityFilter, Page, QueryOutcome};

/// A bound value. Enumerated because the bind list is built dynamically while
/// the SQL is assembled — the alternative would be string interpolation.
enum Bind {
    Text(String),
    TextArr(Vec<String>),
    /// jsonpath; bound as text and cast with `$n::jsonpath` in the SQL
    Path(String),
    /// a distance in metres (geoquery `near`)
    Num(f64),
    /// LIMIT/OFFSET (pagination pushdown)
    Int(i64),
}

/// The internal doc's members that become extracted columns.
pub(crate) struct Extracted {
    types: Vec<String>,
    scopes: Option<Vec<String>>,
    created: String,
    modified: String,
    expires: Option<String>,
    /// The default GeoProperty as GeoJSON text, for `ST_GeomFromGeoJSON`.
    /// `None` (→ SQL NULL) whenever it cannot be represented as ONE geometry.
    location: Option<String>,
    /// True when the doc CARRIES the default GeoProperty but `location` could
    /// not be extracted (multi-instance, non-GeoJSON value). The compiled
    /// geoquery ORs on this column — index-shaped (BitmapOr), unlike the old
    /// `location IS NULL OR …` guard which forced a sequential scan — and
    /// rows without any geoproperty are excluded in SQL, which is exact.
    location_ambiguous: bool,
    /// The earliest `expiresAt` of any Attribute instance (4.22), so the
    /// sweep finds the rows to prune by index instead of reading every doc.
    attrs_expire: Option<String>,
}

/// The four members every stored kind extracts the same way: `type` and
/// `scope` as arrays whether the doc spells them as one string or many, and
/// the two stamps. The temporal tables carry exactly these, entities carry
/// them plus expiry and location, so this is the one implementation of the
/// shared half — two copies is how one of them drifts.
///
/// 4.6.3 allows a comma seconds-fraction in requests; the stamps feed
/// `::timestamptz` casts, which refuse it, so they are canonicalised here and
/// every extracted column holds the instant the client meant.
pub(super) fn types_scopes_stamps(
    doc: &Value,
) -> (Vec<String>, Option<Vec<String>>, String, String) {
    let as_vec = |v: &Value| -> Vec<String> {
        match v {
            Value::String(s) => vec![s.clone()],
            Value::Array(a) => a
                .iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect(),
            _ => vec![],
        }
    };
    let ts = |k: &str| {
        doc.get(k)
            .and_then(Value::as_str)
            .map(|s| antares_store::filter::canonical_datetime(s).into_owned())
    };
    let now = || "1970-01-01T00:00:00Z".to_owned(); // caller always stamps; belt only
    (
        doc.get("type").map(&as_vec).unwrap_or_default(),
        doc.get("scope").map(&as_vec),
        ts("createdAt").unwrap_or_else(now),
        ts("modifiedAt").unwrap_or_else(now),
    )
}

fn extract(doc: &Value) -> Extracted {
    let (types, scopes, created, modified) = types_scopes_stamps(doc);
    let ts = |k: &str| {
        doc.get(k)
            .and_then(Value::as_str)
            .map(|s| antares_store::filter::canonical_datetime(s).into_owned())
    };
    let location = crate::compile::geo::extract_location(doc);
    let location_ambiguous =
        location.is_none() && doc.get(crate::compile::geo::LOCATION_IRI).is_some();
    Extracted {
        types,
        scopes,
        created,
        modified,
        expires: ts("expiresAt"),
        location,
        location_ambiguous,
        attrs_expire: first_instance_expiry(doc),
    }
}

/// The earliest instance `expiresAt` of an internal doc's Attributes at any
/// depth (a sub-Attribute expires too, 4.22), canonicalised for the
/// `::timestamptz` cast; an Entity member never counts.
fn first_instance_expiry(doc: &Value) -> Option<String> {
    fn instances<'a>(doc: &'a Value, out: &mut Vec<&'a str>) {
        let Some(obj) = doc.as_object() else {
            return;
        };
        for inst in obj
            .iter()
            .filter(|(k, _)| k.contains(':'))
            .filter_map(|(_, v)| v.as_array())
            .flatten()
        {
            out.extend(inst.get("expiresAt").and_then(Value::as_str));
            instances(inst, out);
        }
    }
    let mut stamps = Vec::new();
    instances(doc, &mut stamps);
    stamps
        .into_iter()
        .min_by_key(|s| antares_model::dt_key(s))
        .map(|s| antares_store::filter::canonical_datetime(s).into_owned())
}

/// The outbox row's event JSON: what the drain turns into a
/// `ChangeEvent`. Field names are the bus crate's serde names; `seq` and the
/// claim check are the drain's business.
#[allow(clippy::too_many_arguments)] // one param per outbox event field
fn change_event(
    tenant: &TenantId,
    op: &str,
    id: &str,
    types: &[String],
    prev: Option<&Value>,
    next: Option<&Value>,
    version: i64,
    incarnation: &str,
) -> Value {
    // changed_attrs = top-level attribute IRIs that differ between the
    // before- and after-images (meta members excluded). Create lists every
    // attr, delete lists every prior attr.
    fn keys(v: Option<&Value>) -> Vec<&str> {
        v.and_then(Value::as_object)
            .map(|o| {
                o.keys()
                    .map(String::as_str)
                    .filter(|k| !antares_model::is_meta(k))
                    .collect()
            })
            .unwrap_or_default()
    }
    let mut changed: Vec<&str> = Vec::new();
    for k in keys(prev).into_iter().chain(keys(next)) {
        if !changed.contains(&k) && prev.and_then(|p| p.get(k)) != next.and_then(|n| n.get(k)) {
            changed.push(k);
        }
    }
    serde_json::json!({
        "tenant": tenant.as_str(),
        "entity_id": id,
        "types": types,
        "op": op,
        "changed_attrs": changed,
        "payload": next,
        "prev_payload": prev,
        "version": version,
        "incarnation": incarnation,
    })
}

#[allow(clippy::too_many_arguments)] // one param per outbox event field
async fn enqueue_change(
    tx: &mut sqlx::postgres::PgConnection,
    tenant: &TenantId,
    op: &str,
    id: &str,
    types: &[String],
    prev: Option<&Value>,
    next: Option<&Value>,
    version: i64,
    incarnation: &str,
) -> Result<(), sqlx::Error> {
    let ev = change_event(tenant, op, id, types, prev, next, version, incarnation);
    super::outbox::enqueue(tx, tenant, &ev).await.map(|_| ())
}

/// Rows one statement may materialize when SQL narrowed the match set but
/// did not decide it. 5.5.6 leaves the threshold to the implementation;
/// this is it for the entity query path, and it is the only bound a request
/// that carries no page (idPattern, federation, orderBy) has at all.
pub const MAX_UNDECIDED_ROWS: i64 = 10_000;

/// The document `q=` is judged against: the stored document, or, for a row
/// holding an instance past its `expiresAt` (`attrs_expire_at` past, rare
/// until the sweep rewrites it), the document 4.22 lets a read see: expired
/// top-level instances dropped, an Attribute left with none removed, as
/// `filter::strip_expired` does in memory (F-ANT1-007). A live row pays
/// only the CASE test. [`live_q`] evaluates it once per row as [`LIVE_DOC`].
// ponytail: only top-level instances are stripped here; a sub-Attribute past
// its expiresAt still matches a `q` until the sweep, which reaches the row
// through `attrs_expire_at`, rewrites it (one maintenance tick). Recurse here
// if that lag ever matters.
const LIVE_ENTITY: &str = "(CASE WHEN attrs_expire_at IS NULL OR attrs_expire_at >= now() \
     THEN entity ELSE (SELECT coalesce(jsonb_object_agg(l.k, l.v), '{}'::jsonb) FROM \
      (SELECT x.k, CASE WHEN jsonb_typeof(x.v) = 'array' AND strpos(x.k, ':') > 0 \
        THEN (SELECT jsonb_agg(i.v) FROM jsonb_array_elements(x.v) AS i(v) \
         WHERE jsonb_typeof(i.v -> 'expiresAt') IS DISTINCT FROM 'string' \
          OR try_timestamptz(i.v ->> 'expiresAt') IS NULL \
          OR try_timestamptz(i.v ->> 'expiresAt') >= now()) \
        ELSE x.v END AS v FROM jsonb_each(entity) AS x(k, v)) AS l \
      WHERE l.v IS NOT NULL) END)";

/// The column a compiled `q=` reads inside [`live_q`].
const LIVE_DOC: &str = "live.doc";

/// A `q=` compiled over [`LIVE_DOC`] as a WHERE clause of `entities`. A `q`
/// leaf compiles to one jsonpath test per value member (nine), so a `q` of
/// three leaves named the column 27 times: inlined, [`LIVE_ENTITY`] put 54
/// expiry subplans into the statement, and planning and starting them cost
/// a page ~7 ms of PostgreSQL CPU against ~1 ms over the bare column, which
/// held the OData load shape at p95 1.3 s on four cores (T-6351). The
/// `OFFSET 0` keeps PostgreSQL from pulling the subquery up and copying the
/// expression into every reference again.
fn live_q(compiled: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM (SELECT {LIVE_ENTITY} AS doc OFFSET 0) AS live WHERE {compiled})"
    )
}

/// Distinct types a type selection may split into arms; a longer list keeps
/// the plain containment, which reads the Tenant.
const MAX_TYPE_ARMS: usize = 16;

/// A type selection read as leakproof index arms. `types @> $n` cannot be an
/// index condition: `arraycontains` is not leakproof, so under the tables'
/// RLS Postgres keeps it out of every index condition and pages the whole
/// Tenant through `entities_pkey` (T-4781: 18 req/s on prod; T-4852 the
/// same for `type=A,B`). The split reads one arm per type the selection
/// names, the Entities whose first type it is (`i_entities_first_type_id`),
/// plus one arm for the few of two or more types whose first type is none
/// of them (`i_entities_multi_type_id`). The arms are disjoint (an Entity
/// has one first type) and their union is exactly the selection. Only a
/// group of one type gets first-type arms: a match of an AND-group (`A;B`)
/// has two types, so the multi-type arm reaches it, where a first-type arm
/// walked every Entity of A to keep none (T-5498: 53 000 s on dev).
struct TypeSplit {
    /// Bind of each type a one-type group names, one first-type arm each.
    firsts: Vec<usize>,
    /// Bind of those types as an array, `None` when every group is an
    /// AND-group and the multi-type arm alone reads the selection.
    named: Option<usize>,
    /// The selection itself, `types @> $g` per AND-group, ORed.
    selection: String,
    /// A selection of one type is decided by `types[1] = $t` alone.
    exact_first: bool,
}

/// The full WHERE of each arm the statement reads: one, or the arms of a
/// `TypeSplit`.
fn arms(wheres: &[String], split: Option<&TypeSplit>) -> Vec<String> {
    let base = wheres.join(" AND ");
    let Some(s) = split else {
        return vec![base];
    };
    let mut arms: Vec<String> = s
        .firsts
        .iter()
        .map(|n| match s.exact_first {
            true => format!("{base} AND types[1] = ${n}"),
            false => format!("{base} AND types[1] = ${n} AND {}", s.selection),
        })
        .collect();
    arms.push(match s.named {
        Some(n) => format!(
            "{base} AND cardinality(types) > 1 AND types[1] <> ALL(${n}) AND {}",
            s.selection
        ),
        None => format!("{base} AND cardinality(types) > 1 AND {}", s.selection),
    });
    arms
}

/// Does `q` require the Entity to have no `location`, by a top-level
/// `!location` term? Then `location IS NULL` holds for every match: a stored
/// geometry comes from an instance's `value` or `object` (`extract`), which
/// `!location` resolves as present. The conjunct is leakproof, so
/// it reaches `i_entities_type_unlocated` (0018) under RLS, where the negated
/// `@?` read the whole type (T-5136); the `@?` stays and decides.
fn unlocated(q: &QNode, expand: &dyn Fn(&str) -> String) -> bool {
    match q {
        QNode::Exists {
            path,
            negated: true,
        } => {
            path.links.is_empty()
                && path.bracket.is_none()
                && matches!(path.path.as_slice(), [term] if expand(term) == crate::compile::geo::LOCATION_IRI)
        }
        QNode::And(all) => all.iter().any(|n| unlocated(n, expand)),
        _ => false,
    }
}

/// The `name` the core @context gives an Entity (`@vocab` + `name`), the one
/// attribute `entities.name_key` (0020) keys.
/// ponytail: one IRI; a `name` another @context maps elsewhere filters as before.
const NAME_IRI: &str = "https://uri.etsi.org/ngsi-ld/default-context/name";

/// The string a top-level `name=="…"` term of `q` requires. Every match then
/// has `name_key` equal to it or NULL (0020: NULL is each shape the key
/// cannot hold), so `(name_key = $n OR name_key IS NULL)` narrows by a
/// leakproof btree under RLS where the `@?` read the whole Tenant (T-5349);
/// the `@?` stays and decides. A NUL is no Postgres text: no narrowing.
fn named<'q>(q: &'q QNode, expand: &dyn Fn(&str) -> String) -> Option<&'q str> {
    match q {
        QNode::Cmp {
            path,
            op: CmpOp::Eq,
            value: QValue::Str(want),
        } if path.links.is_empty()
            && path.bracket.is_none()
            && !want.contains('\0')
            && matches!(path.path.as_slice(), [term] if expand(term) == NAME_IRI) =>
        {
            Some(want)
        }
        QNode::And(all) => all.iter().find_map(|n| named(n, expand)),
        _ => None,
    }
}

/// The Entity's own members, which `entity_value_rows` (0024) gives no row:
/// an `attrs=` naming one is not narrowed.
const OWN_MEMBERS: &[&str] = &[
    "id",
    "type",
    "scope",
    "@context",
    "createdAt",
    "modifiedAt",
    "deletedAt",
    "expiresAt",
];

/// The longest string `entity_values.txt` (0024) keys, in bytes.
const MAX_KEYED_TEXT: usize = 1024;

/// What a top-level `q` term requires of its Attribute's `entity_values`
/// rows (0024). Every bound is non-strict: a stored number is rounded to
/// float8, monotonically, and the query's f64 is the literal's own value.
#[derive(Debug, PartialEq)]
enum ValueKey<'q> {
    Eq(f64),
    AtLeast(f64),
    AtMost(f64),
    Between(f64, f64),
    Text(&'q str),
}

fn value_key(op: CmpOp, want: &QValue) -> Option<ValueKey<'_>> {
    Some(match (op, want) {
        (CmpOp::Eq, QValue::Num(n)) => ValueKey::Eq(*n),
        (CmpOp::Gt | CmpOp::Ge, QValue::Num(n)) => ValueKey::AtLeast(*n),
        (CmpOp::Lt | CmpOp::Le, QValue::Num(n)) => ValueKey::AtMost(*n),
        (CmpOp::Eq, QValue::Range(lo, hi)) => match (lo.as_ref(), hi.as_ref()) {
            (QValue::Num(a), QValue::Num(b)) => ValueKey::Between(*a, *b),
            _ => return None,
        },
        (CmpOp::Eq, QValue::Str(s)) if !s.contains('\0') => ValueKey::Text(s),
        _ => return None,
    })
}

/// The first top-level `q` term `entity_values` keys: its Attribute's IRI
/// and the rows a match holds. Call it only for a `q` that compiled: then
/// the term's `@?` is exact and every row it matches has such a row.
fn valued<'q>(q: &'q QNode, expand: &dyn Fn(&str) -> String) -> Option<(String, ValueKey<'q>)> {
    match q {
        QNode::Cmp { path, op, value } if path.links.is_empty() && path.bracket.is_none() => {
            let [term] = path.path.as_slice() else {
                return None;
            };
            Some((expand(term), value_key(*op, value)?))
                .filter(|(iri, _)| !iri.contains('\0') && !OWN_MEMBERS.contains(&iri.as_str()))
        }
        QNode::And(all) => all.iter().find_map(|n| valued(n, expand)),
        _ => None,
    }
}

/// One past the most Entity ids an `entity_values` probe (0024) hands the
/// page as `id = ANY(...)` (T-5303). A term matching more is no narrowing
/// worth its primary key reads (about 7 µs each under RLS): the page reads
/// as before, and the probe cost at most this many index entries.
const PROBE_ROWS: i64 = 1001;

/// A read of `entity_values` (0024) for the ids a term's matches are among,
/// `$1` the Tenant: its statement and binds. The statement counts the
/// matching rows up to `PROBE_ROWS` first, from the index alone (every
/// column `cond` reads but `txt` is a key of `i_entity_values_num`), and
/// reads their ids only under the cap: an unselective term (`level>=40`
/// of a 0..99 level) cost 1 001 heap reads per page and threw every id
/// away (T-6056: 4 418 calls, 46.7 s of a 60 s run). NULL past the cap.
struct Probe {
    sql: String,
    binds: Vec<Bind>,
}

impl Probe {
    fn new(tenant: &TenantId, attr: Bind, cond: &str, mut binds: Vec<Bind>) -> Self {
        binds.splice(0..0, [Bind::Text(tenant.as_str().to_owned()), attr]);
        binds.push(Bind::Int(PROBE_ROWS));
        let rows = format!("FROM entity_values WHERE tenant_id = $1 AND {cond}");
        let sql = format!(
            "SELECT CASE WHEN n < ${cap} THEN ARRAY(SELECT id {rows}) END \
             FROM (SELECT count(*) AS n FROM (SELECT 1 {rows} LIMIT ${cap}) r) c",
            cap = binds.len()
        );
        Self { sql, binds }
    }
}

/// `valued`'s term as a probe: the rows that satisfy it, or hold 'NaN', the
/// mark of an Attribute whose values were not keyed (0024). `f >= x` keeps
/// 'NaN', which sorts above every number.
fn value_probe(tenant: &TenantId, iri: String, key: ValueKey<'_>) -> Probe {
    let (cond, binds) = match key {
        ValueKey::Eq(n) => ("f = ANY(ARRAY[$3, 'NaN'::float8])", vec![Bind::Num(n)]),
        ValueKey::AtLeast(n) => ("f >= $3", vec![Bind::Num(n)]),
        ValueKey::AtMost(n) => ("(f <= $3 OR f = 'NaN')", vec![Bind::Num(n)]),
        ValueKey::Between(a, b) => (
            "(f BETWEEN $3 AND $4 OR f = 'NaN')",
            vec![Bind::Num(a), Bind::Num(b)],
        ),
        ValueKey::Text(s) if s.len() > MAX_KEYED_TEXT => ("f = 'NaN'", vec![]),
        ValueKey::Text(s) => ("(txt = $3 OR f = 'NaN')", vec![Bind::Text(s.to_owned())]),
    };
    Probe::new(
        tenant,
        Bind::Text(iri),
        &format!("attr = $2 AND {cond}"),
        binds,
    )
}

/// `attrs=` as a probe of presence rows (0024), `None` when it names one of
/// the Entity's own members, which have none.
fn attrs_probe(tenant: &TenantId, attrs: &[String]) -> Option<Probe> {
    if attrs.iter().any(|a| OWN_MEMBERS.contains(&a.as_str())) {
        return None;
    }
    let names = Bind::TextArr(super::storable(attrs));
    Some(Probe::new(tenant, names, "attr = ANY($2)", vec![]))
}

/// A compiled `scopeQ` as a probe of the scope rows (0032, attr `scope`):
/// per OR group, the normalised scope one of its patterns' matches equals or
/// starts with, or 'NaN', the mark of a scope past `MAX_KEYED_TEXT`. Call it
/// only for a `scopeQ` that compiled: the regex stays and decides.
fn scope_probe(tenant: &TenantId, scope_q: &str) -> Option<Probe> {
    let mut arms = Vec::new();
    let mut binds = Vec::new();
    for key in crate::compile::scope::scope_keys(scope_q)? {
        if let Some(exact) = key.exact {
            binds.push(Bind::Text(exact));
            arms.push(format!("txt = ${}", binds.len().saturating_add(2)));
        }
        if let Some(under) = key.under {
            // `under` ends in `/`; the first text past every one starting
            // with it ends in `0`, the next byte (COLLATE "C")
            let past = format!("{}0", under.trim_end_matches('/'));
            binds.push(Bind::Text(under));
            binds.push(Bind::Text(past));
            let n = binds.len().saturating_add(2);
            arms.push(format!("(txt >= ${} AND txt < ${n})", n.saturating_sub(1)));
        }
    }
    let cond = format!("attr = $2 AND ({} OR f = 'NaN')", arms.join(" OR "));
    Some(Probe::new(
        tenant,
        Bind::Text("scope".to_owned()),
        &cond,
        binds,
    ))
}

/// The distinct ids `p` reads, `None` when there are `PROBE_ROWS` rows or
/// more. Unprepared, as the page statement.
async fn probe_ids(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    p: &Probe,
) -> Result<Option<Vec<String>>, sqlx::Error> {
    let mut qy = sqlx::query_scalar::<_, Option<Vec<String>>>(sqlx::AssertSqlSafe(p.sql.clone()))
        .persistent(false);
    for b in &p.binds {
        qy = match b {
            Bind::Text(s) | Bind::Path(s) => qy.bind(s),
            Bind::TextArr(v) => qy.bind(v),
            Bind::Num(n) => qy.bind(n),
            Bind::Int(n) => qy.bind(n),
        };
    }
    let Some(mut ids) = qy.fetch_one(&mut **tx).await? else {
        return Ok(None);
    };
    ids.sort_unstable();
    ids.dedup();
    Ok(Some(ids))
}

/// The type selection's binds and SQL: `Ok(split)` for at most
/// `MAX_TYPE_ARMS` distinct types, `Err(where)` (the plain containment, or
/// `false` when no group can match) otherwise. A group naming a type with a
/// NUL needs a type no entity has.
fn type_selection(groups: &[Vec<String>], binds: &mut Vec<Bind>) -> Result<TypeSplit, String> {
    let groups: Vec<&Vec<String>> = groups
        .iter()
        .filter(|g| super::storable(g).len() == g.len())
        .collect();
    // the type of each group naming one (`A;A` is `A`): a group of two
    // types is read by the multi-type arm alone
    let mut named: Vec<&String> = groups
        .iter()
        .filter_map(|g| g.first().filter(|f| g.iter().all(|t| t == *f)))
        .collect();
    named.sort_unstable();
    named.dedup();
    let ors: Vec<String> = groups
        .iter()
        .map(|g| {
            binds.push(Bind::TextArr((*g).clone()));
            format!("types @> ${}", binds.len())
        })
        .collect();
    if ors.is_empty() {
        return Err("false".to_owned());
    }
    let selection = format!("({})", ors.join(" OR "));
    // an empty group selects every type, which no set of arms can name
    if named.len() > MAX_TYPE_ARMS || groups.iter().any(|g| g.is_empty()) {
        return Err(selection);
    }
    let firsts = named
        .iter()
        .map(|t| {
            binds.push(Bind::Text((*t).clone()));
            binds.len()
        })
        .collect();
    let named = (!named.is_empty()).then(|| {
        binds.push(Bind::TextArr(named.into_iter().cloned().collect()));
        binds.len()
    });
    Ok(TypeSplit {
        firsts,
        named,
        selection,
        exact_first: matches!(groups.as_slice(), [g] if g.len() == 1),
    })
}

/// count=true's total: a count per arm over exactly the page's WHERE, never
/// a window over the page statement (that kept every matching row, body and
/// all, until the last one was read).
fn count_sql(wheres: &[String], split: Option<&TypeSplit>) -> String {
    let counts: Vec<String> = arms(wheres, split)
        .iter()
        .map(|w| format!("(SELECT count(*) FROM entities WHERE {w})"))
        .collect();
    format!("SELECT {}", counts.join(" + "))
}

/// The ids of every arm of `count_sql`, for a count the broker decides id by
/// id. The arms are disjoint (`arms`), so each id comes once.
fn ids_sql(wheres: &[String], split: Option<&TypeSplit>) -> String {
    arms(wheres, split)
        .iter()
        .map(|w| format!("SELECT id FROM entities WHERE {w}"))
        .collect::<Vec<_>>()
        .join(" UNION ALL ")
}

/// How many ids `sql` streams that any of `res` selects. The ids come from
/// the type and id indexes alone (no document is read), one row at a time.
async fn count_ids(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    sql: &str,
    binds: impl Iterator<Item = &Bind>,
    res: &[std::sync::Arc<regex::Regex>],
) -> Result<i64, sqlx::Error> {
    use futures_util::TryStreamExt;
    // unprepared, as every statement of `query_in`
    let mut qy =
        sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(sql.to_owned())).persistent(false);
    for b in binds {
        qy = match b {
            Bind::Text(s) | Bind::Path(s) => qy.bind(s),
            Bind::TextArr(v) => qy.bind(v),
            Bind::Num(n) => qy.bind(n),
            Bind::Int(n) => qy.bind(n),
        };
    }
    let mut ids = qy.fetch(&mut **tx);
    let mut n: i64 = 0;
    while let Some(id) = ids.try_next().await? {
        if res.iter().any(|re| re.is_match(&id)) {
            n = n.saturating_add(1);
        }
    }
    Ok(n)
}

/// `id_prefixes` as SQL: the bounding byte range of every prefix, which the
/// C-collated id index (0027) answers in its order, ANDed with the OR of
/// each prefix's own range. Prefixes with a NUL match nothing (`storable`).
fn prefix_ranges(prefixes: &[String], binds: &mut Vec<Bind>) -> String {
    const C: &str = "id COLLATE \"C\"";
    let mut ps: Vec<&String> = prefixes.iter().filter(|p| !p.contains('\0')).collect();
    ps.sort_unstable();
    let Some(lo) = ps.first() else {
        return "false".to_owned();
    };
    let mut range = |lo: &str, hi: Option<String>| {
        binds.push(Bind::Text(lo.to_owned()));
        let mut sql = format!("{C} >= ${}", binds.len());
        if let Some(hi) = hi {
            binds.push(Bind::Text(hi));
            sql.push_str(&format!(" AND {C} < ${}", binds.len()));
        }
        sql
    };
    // unbounded above when one prefix has no end
    let hi_end = ps
        .iter()
        .map(|p| antares_store::filter::prefix_end(p))
        .collect::<Option<Vec<_>>>()
        .and_then(|ends| ends.into_iter().max());
    let bound = range(lo, hi_end);
    let each: Vec<String> = ps
        .iter()
        .map(|p| format!("({})", range(p, antares_store::filter::prefix_end(p))))
        .collect();
    format!("({bound} AND ({}))", each.join(" OR "))
}

/// The statement, split out of `query` so its pagination shape is
/// assertable without a database. Either LIMIT/OFFSET over an exactly
/// decided set or a `query_candidates` walk (`true`; one row past the page answers "is there a next
/// page", the total is `count_sql`'s) or the flat `MAX_UNDECIDED_ROWS`
/// safety LIMIT — never no LIMIT. A split reads each arm in id order up to
/// what the page can need and merges them.
fn query_sql(
    select: &str,
    wheres: &[String],
    split: Option<&TypeSplit>,
    page: Option<&Page>,
    decided: bool,
    order: &str,
    binds: &mut Vec<Bind>,
) -> (String, bool) {
    let page = page.filter(|_| decided);
    // Saturating: the caller's limit is only bounded by `i64::MAX`
    // (`ANTARES_DISCOVERY_SCAN_MAX` clamps to exactly that), and one past
    // the largest page is still the largest page.
    let limit = page.map_or(MAX_UNDECIDED_ROWS, |p| p.limit.saturating_add(1));
    binds.push(Bind::Int(limit));
    let lim = binds.len();
    let offset = page.map_or(String::new(), |p| {
        binds.push(Bind::Int(p.offset));
        format!(" OFFSET ${}", binds.len())
    });
    let arms = arms(wheres, split);
    // A deep page skips its OFFSET over ids alone, which the type indexes
    // hand back without a heap visit (0013's INCLUDE), and reads only the
    // page's documents by primary key (T-4907: page 50 of a 180k-entity
    // type walked 5 000 jsonb rows for 100).
    if let Some(p) = page.filter(|p| p.offset > 0) {
        let ids = match arms.as_slice() {
            [w] => {
                format!("SELECT id FROM entities WHERE {w} ORDER BY {order} LIMIT ${lim}{offset}")
            }
            _ => {
                binds.push(Bind::Int(limit.saturating_add(p.offset)));
                let each = binds.len();
                let parts: Vec<String> = arms
                    .iter()
                    .map(|w| {
                        format!(
                            "(SELECT id FROM entities WHERE {w} ORDER BY {order} LIMIT ${each})"
                        )
                    })
                    .collect();
                format!(
                    "SELECT id FROM ({}) AS arms ORDER BY {order} LIMIT ${lim}{offset}",
                    parts.join(" UNION ALL ")
                )
            }
        };
        return (
            format!(
                "SELECT {select} AS entity FROM entities \
                 WHERE tenant_id = $1 AND id IN ({ids}) ORDER BY {order}"
            ),
            true,
        );
    }
    let sql = match arms.as_slice() {
        [w] => format!(
            "SELECT {select} AS entity FROM entities WHERE {w} ORDER BY {order} LIMIT ${lim}{offset}"
        ),
        _ => {
            let each = match page {
                Some(p) => {
                    binds.push(Bind::Int(limit.saturating_add(p.offset)));
                    binds.len()
                }
                None => lim,
            };
            let parts: Vec<String> = arms
                .iter()
                .map(|w| {
                    format!("(SELECT id, entity FROM entities WHERE {w} ORDER BY {order} LIMIT ${each})")
                })
                .collect();
            format!(
                "SELECT {select} AS entity FROM ({}) AS entities \
                 ORDER BY {order} LIMIT ${lim}{offset}",
                parts.join(" UNION ALL ")
            )
        }
    };
    (sql, page.is_some())
}

/// The total a pushed page shows by itself, `None` when only `count_sql`
/// can tell it. Uncounted, the smallest total consistent with "a next page
/// exists", enough for the next/prev links and never shown as a count;
/// counted, a last page that holds rows (or the first, empty one).
fn page_total(p: &Page, rows: usize, more: bool) -> Option<i64> {
    let seen = p.offset.saturating_add(rows as i64);
    match (p.count, more) {
        (false, _) => Some(seen.saturating_add(i64::from(more))),
        (true, false) if rows > 0 || p.offset == 0 => Some(seen),
        (true, _) => None,
    }
}

/// Did the safety LIMIT actually cut the match set? A statement that came
/// back full was truncated at a bound nobody chose, and the caller's
/// evaluator still has rows to drop, so no complete page can be built from
/// it. 5.5.6: "When a query operation is producing so many results that can
/// potentially exhaust client or server resources … implementations shall
/// raise an error of type TooManyResults." Serving the truncated prefix
/// would under-report the answer instead.
pub(crate) fn check_ceiling(paged: bool, rows: usize, ceiling: i64) -> Result<(), sqlx::Error> {
    if paged || (rows as i64) < ceiling {
        return Ok(());
    }
    Err(sqlx::Error::Configuration(Box::new(
        NgsiError::TooManyResults(format!(
            "query matched more than {ceiling} entities before filtering; \
             narrow it or request a smaller offset"
        )),
    )))
}

/// Recover a spec error the store raised itself from the driver error it
/// travels in (the store's only error channel is `sqlx::Error`). `None` =
/// a genuine driver failure, which stays an InternalError at the seam.
pub fn ngsi_error(e: &sqlx::Error) -> Option<&NgsiError> {
    match e {
        sqlx::Error::Configuration(b) => b.downcast_ref::<NgsiError>(),
        _ => None,
    }
}

impl PgEntityStore {
    pub fn new(pool: PgPool) -> Self {
        Self {
            batch_pool: pool.clone(),
            pool,
            outbox: std::sync::atomic::AtomicBool::new(false),
            async_batch: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Run the batch writes on `reserve` instead of the shared pool.
    pub fn with_batch_pool(mut self, reserve: PgPool) -> Self {
        self.batch_pool = reserve;
        self
    }

    /// Asynchronous commit for the batch writes (see the field).
    pub fn set_async_batch_commit(&self, on: bool) {
        self.async_batch
            .store(on, std::sync::atomic::Ordering::Relaxed);
    }

    /// A batch write's transaction, with `synchronous_commit = off` set
    /// LOCAL to it when [`Self::set_async_batch_commit`] is on.
    async fn begin_batch(&self) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, sqlx::Error> {
        let mut tx = super::begin(&self.batch_pool).await?;
        if self.async_batch.load(std::sync::atomic::Ordering::Relaxed) {
            sqlx::query("SET LOCAL synchronous_commit = off")
                .execute(&mut *tx)
                .await?;
        }
        Ok(tx)
    }

    /// Outbox producer switch — the broker enables this exactly when `bus=nats`.
    pub fn set_outbox(&self, on: bool) {
        self.outbox.store(on, std::sync::atomic::Ordering::Relaxed);
    }

    fn outbox_on(&self) -> bool {
        self.outbox.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// 5.6.1-shaped create: `false` when the id already exists (→ 409).
    ///
    /// 4.22: "expiresAt is defined as the system temporal Property at which a
    /// certain Entity, Property or Relationship shall become invalid" — an
    /// entity whose expiry has passed is already invalid, whatever the
    /// reaping lag, so it must not 409 a create that reads (and GETs) as
    /// absent. The conflict clause replaces such a row and reports CREATED;
    /// a live row still takes the DO NOTHING path and `rows_affected() == 0`.
    pub async fn create(
        &self,
        tenant: &TenantId,
        id: &str,
        doc: &Value,
    ) -> Result<bool, sqlx::Error> {
        let e = extract(doc);
        let mut tx = super::begin(&self.pool).await?;
        crate::store::pg::set_tenant(&mut tx, tenant).await?;
        crate::store::pg::claim_tenant(&mut tx, tenant).await?;
        let done = sqlx::query(
            "INSERT INTO entities
               (tenant_id, id, entity, types, scopes, created_at, modified_at, expires_at,
                location, location_ambiguous, attrs_expire_at)
             VALUES ($1, $2, $3, $4, $5, $6::timestamptz, $7::timestamptz, $8::timestamptz,
                     CASE WHEN ST_IsValid(try_geomfromgeojson($9))
                          THEN try_geomfromgeojson($9) END,
                     $10 OR ($9 IS NOT NULL
                             AND NOT COALESCE(ST_IsValid(try_geomfromgeojson($9)), false)),
                     try_timestamptz($11))
             ON CONFLICT (tenant_id, id) DO UPDATE SET
               entity = EXCLUDED.entity, types = EXCLUDED.types,
               scopes = EXCLUDED.scopes, created_at = EXCLUDED.created_at,
               modified_at = EXCLUDED.modified_at, expires_at = EXCLUDED.expires_at,
               location = EXCLUDED.location,
               location_ambiguous = EXCLUDED.location_ambiguous,
               attrs_expire_at = EXCLUDED.attrs_expire_at,
               version = 1
             WHERE entities.expires_at IS NOT NULL AND entities.expires_at <= now()",
        )
        .bind(tenant.as_str())
        .bind(id)
        .bind(doc)
        .bind(&e.types)
        .bind(&e.scopes)
        .bind(&e.created)
        .bind(&e.modified)
        .bind(&e.expires)
        .bind(&e.location)
        .bind(e.location_ambiguous)
        .bind(&e.attrs_expire)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if done == 1 && self.outbox_on() {
            enqueue_change(
                &mut tx,
                tenant,
                "create",
                id,
                &e.types,
                None,
                Some(doc),
                1,
                &e.created,
            )
            .await?;
        }
        tx.commit().await?;
        Ok(done == 1)
    }

    pub async fn get(&self, tenant: &TenantId, id: &str) -> Result<Option<Value>, sqlx::Error> {
        let mut tx = super::begin(&self.pool).await?;
        crate::store::pg::set_tenant(&mut tx, tenant).await?;
        // 4.22: expired rows are invalid context until the sweep reaps them
        let row = sqlx::query(
            "SELECT entity FROM entities WHERE tenant_id = $1 AND id = $2
               AND (expires_at IS NULL OR expires_at > now())",
        )
        .bind(tenant.as_str())
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row.map(|r| r.get::<Value, _>(0)))
    }

    /// The types the Tenant's live Entities carry, sorted (T-4610): the
    /// names from the type catalogue the writes keep (migration 0012,
    /// T-4683), each kept while the types index finds a live Entity of it.
    /// No `entity` document and no pass over the Tenant's rows.
    pub async fn type_names(&self, tenant: &TenantId) -> Result<Vec<String>, sqlx::Error> {
        self.catalogued(tenant, None).await
    }

    /// The catalogued types a live Entity carries, beside one of `any_of`
    /// when given. 4.22: expired rows are invalid context until the sweep
    /// reaps them.
    async fn catalogued(
        &self,
        tenant: &TenantId,
        any_of: Option<&[String]>,
    ) -> Result<Vec<String>, sqlx::Error> {
        let mut tx = super::begin(&self.pool).await?;
        crate::store::pg::set_tenant(&mut tx, tenant).await?;
        // A type is live when it is some live Entity's first type, which a
        // skip scan of the first-type btree reads as one index probe per
        // type, or else, beside another, on an Entity of two or more types,
        // which the partial GIN alone indexes (0012). The plain types GIN
        // built a bitmap of every Entity of each type: 18 ms for five types
        // of 20 000, and a generic plan chose a sequential scan for an
        // EXISTS on the btree; this is 0.1 ms either way.
        const ALL: &str = "WITH RECURSIVE firsts AS (
               SELECT (SELECT min(e.types[1]) FROM entities e WHERE e.tenant_id = $1
                         AND (e.expires_at IS NULL OR e.expires_at > now())) AS ty
               UNION ALL
               SELECT (SELECT min(e.types[1]) FROM entities e
                        WHERE e.tenant_id = $1 AND e.types[1] > f.ty
                          AND (e.expires_at IS NULL OR e.expires_at > now()))
                 FROM firsts f WHERE f.ty IS NOT NULL)
             SELECT c.type FROM entity_types c WHERE c.tenant_id = $1
               AND (c.type IN (SELECT ty FROM firsts WHERE ty IS NOT NULL)
                 OR EXISTS (SELECT 1 FROM entities e WHERE e.tenant_id = c.tenant_id
                      AND cardinality(e.types) > 1 AND e.types @> ARRAY[c.type]
                      AND (e.expires_at IS NULL OR e.expires_at > now())))
             ORDER BY c.type";
        const BESIDE: &str = "SELECT c.type FROM entity_types c WHERE c.tenant_id = $1
               AND EXISTS (SELECT 1 FROM entities e WHERE e.tenant_id = c.tenant_id
                 AND cardinality(e.types) > 1
                 AND e.types @> ARRAY[c.type] AND e.types && $2
                 AND (e.expires_at IS NULL OR e.expires_at > now()))
             ORDER BY c.type";
        let names = match any_of {
            None => {
                sqlx::query_scalar(ALL)
                    .bind(tenant.as_str())
                    .fetch_all(&mut *tx)
                    .await?
            }
            Some(of) => {
                sqlx::query_scalar(BESIDE)
                    .bind(tenant.as_str())
                    .bind(super::storable(of))
                    .fetch_all(&mut *tx)
                    .await?
            }
        };
        tx.commit().await?;
        Ok(names)
    }

    /// Every (type, member name) pair the Tenant's Entities ever carried,
    /// sorted (T-4934): the attribute catalogue the writes keep (migration
    /// 0025). A pair whose last carrier went stays listed; the reader checks
    /// each against a live Entity, as it does a catalogued type.
    pub async fn attr_pairs(
        &self,
        tenant: &TenantId,
    ) -> Result<Vec<(String, String)>, sqlx::Error> {
        let mut tx = super::begin(&self.pool).await?;
        crate::store::pg::set_tenant(&mut tx, tenant).await?;
        let pairs = sqlx::query_as(
            "SELECT type, attr FROM entity_attrs WHERE tenant_id = $1 ORDER BY type, attr",
        )
        .bind(tenant.as_str())
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(pairs)
    }

    /// The types of the Tenant's live Entities of two or more types that carry
    /// one of `any_of`, sorted (T-4683): one probe of the multi-type index per
    /// catalogued type.
    pub async fn types_beside(
        &self,
        tenant: &TenantId,
        any_of: &[String],
    ) -> Result<Vec<String>, sqlx::Error> {
        self.catalogued(tenant, Some(any_of)).await
    }

    /// [`antares_store::CurrentStateDriver::entity_id_segments`] over the C-collated id
    /// index (0027): one seek per segment, the next one past the segment's
    /// own bound, so the cost follows the segments, not the ids (T-5661).
    /// Expired Entities count too: a segment only narrows, never decides.
    pub async fn id_segments(
        &self,
        tenant: &TenantId,
        lead: &str,
        sep: char,
        max: usize,
    ) -> Result<Option<Vec<String>>, sqlx::Error> {
        use antares_store::filter::prefix_end;
        let Some(end) = prefix_end(lead).filter(|_| !lead.contains('\0')) else {
            return Ok(None);
        };
        let mut tx = super::begin(&self.pool).await?;
        crate::store::pg::set_tenant(&mut tx, tenant).await?;
        let mut from = lead.to_owned();
        let mut out = Vec::new();
        // a step per segment and per id that holds no `sep` after `lead`
        for _ in 0..=max {
            let id: Option<String> = sqlx::query_scalar(
                "SELECT id FROM entities WHERE tenant_id = $1
                   AND id COLLATE \"C\" >= $2 AND id COLLATE \"C\" < $3
                 ORDER BY id COLLATE \"C\" LIMIT 1",
            )
            .bind(tenant.as_str())
            .bind(&from)
            .bind(&end)
            .fetch_optional(&mut *tx)
            .await?;
            let Some(id) = id else {
                tx.commit().await?;
                return Ok(Some(out));
            };
            let at = id.get(lead.len()..).and_then(|rest| rest.find(sep));
            from = match at.map(|k| &id[..lead.len().saturating_add(k)]) {
                Some(seg) => {
                    out.push(seg.to_owned());
                    match prefix_end(&format!("{seg}{sep}")) {
                        Some(next) => next,
                        None => break,
                    }
                }
                // no id is greater than `id` and less than this one
                None => format!("{id}\u{1}"),
            };
        }
        tx.commit().await?;
        Ok(None)
    }

    /// Several Entities in one transaction (T-4085): one per id that exists,
    /// keyed by id. A write's Relationship targets are read this way, so a
    /// fragment with five Relationships costs one BEGIN, not five.
    pub async fn get_many(
        &self,
        tenant: &TenantId,
        ids: &[&str],
    ) -> Result<std::collections::HashMap<String, Value>, sqlx::Error> {
        if ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let mut tx = super::begin(&self.pool).await?;
        crate::store::pg::set_tenant(&mut tx, tenant).await?;
        // 4.22: expired rows are invalid context until the sweep reaps them
        let rows = sqlx::query(
            "SELECT id, entity FROM entities WHERE tenant_id = $1 AND id = ANY($2)
               AND (expires_at IS NULL OR expires_at > now())",
        )
        .bind(tenant.as_str())
        .bind(ids)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<String, _>(0), r.get::<Value, _>(1)))
            .collect())
    }

    /// Returns the deleted document (the before-image, captured in the same
    /// transaction as the DELETE — never re-read outside it), `None` = absent.
    pub async fn delete(&self, tenant: &TenantId, id: &str) -> Result<Option<Value>, sqlx::Error> {
        let mut tx = super::begin(&self.pool).await?;
        crate::store::pg::set_tenant(&mut tx, tenant).await?;
        // 4.22: an already-expired row is invalid, so deleting it is a
        // 404 exactly as retrieving it is — never a 204 for an entity the
        // API has stopped serving.
        let row = sqlx::query(
            "DELETE FROM entities WHERE tenant_id = $1 AND id = $2
               AND (expires_at IS NULL OR expires_at > now())
             RETURNING entity, types, version, created_at::text",
        )
        .bind(tenant.as_str())
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let mut prev_out = None;
        if let Some(r) = &row {
            let prev: Value = r.get(0);
            if self.outbox_on() {
                let types: Vec<String> = r.get(1);
                enqueue_change(
                    &mut tx,
                    tenant,
                    "delete",
                    id,
                    &types,
                    Some(&prev),
                    None,
                    r.get::<i64, _>(2),
                    r.get::<&str, _>(3),
                )
                .await?;
            }
            prev_out = Some(prev);
        }
        tx.commit().await?;
        Ok(prev_out)
    }

    /// Delete one entity only if `keep` accepts the stored document, under
    /// one lock: the DELETE itself takes the row lock and produces the
    /// document that decides, and a refusal rolls the transaction back
    /// instead of committing it. A `SELECT` followed by a `DELETE` would
    /// leave the window this exists to close — between them the row can be
    /// deleted and recreated under the same id, and the delete then lands on
    /// a document nobody inspected.
    ///
    /// 4.22 as in `delete`: an already-expired row is invalid, so it is
    /// absent here too.
    pub async fn delete_if(
        &self,
        tenant: &TenantId,
        id: &str,
        keep: &(dyn for<'v> Fn(&'v Value) -> bool + Sync),
    ) -> Result<Option<Value>, sqlx::Error> {
        let mut tx = super::begin(&self.pool).await?;
        crate::store::pg::set_tenant(&mut tx, tenant).await?;
        let row = sqlx::query(
            "DELETE FROM entities WHERE tenant_id = $1 AND id = $2
               AND (expires_at IS NULL OR expires_at > now())
             RETURNING entity, types, version, created_at::text",
        )
        .bind(tenant.as_str())
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(r) = &row else {
            tx.commit().await?;
            return Ok(None);
        };
        let prev: Value = r.get(0);
        if !keep(&prev) {
            tx.rollback().await?;
            return Ok(None);
        }
        if self.outbox_on() {
            let types: Vec<String> = r.get(1);
            enqueue_change(
                &mut tx,
                tenant,
                "delete",
                id,
                &types,
                Some(&prev),
                None,
                r.get::<i64, _>(2),
                r.get::<&str, _>(3),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(Some(prev))
    }

    /// Query pushdown. The predicates that compile EXACTLY go to
    /// Postgres; everything else is simply left out of the WHERE clause, so
    /// the result is always a superset of the answer and the caller's
    /// in-memory evaluator remains the arbiter. That is the property that
    /// makes store modes agree: SQL removes rows, it never decides them.
    ///
    /// Every value here is a bind. The only text this function
    /// concatenates is its own operators and `$n` placeholders.
    ///
    /// Every statement it builds is bounded: an exactly decided query by the
    /// caller's page, everything else by `MAX_UNDECIDED_ROWS`. A set that
    /// reaches that ceiling is refused with TooManyResults (5.5.6) rather
    /// than returned as a prefix the caller would page over as if complete.
    pub async fn query(
        &self,
        tenant: &TenantId,
        f: &EntityFilter<'_>,
    ) -> Result<QueryOutcome, sqlx::Error> {
        self.query_in(tenant, f, false).await
    }

    /// `query` for a caller that re-decides every row and walks the match
    /// set in id-ordered pages: `f.page` is pushed over the candidates even
    /// when SQL cannot decide them (`paged` without `decided`), so a page of
    /// a geoquery reads that page's candidates, not up to
    /// `MAX_UNDECIDED_ROWS` (T-5088).
    pub async fn query_candidates(
        &self,
        tenant: &TenantId,
        f: &EntityFilter<'_>,
    ) -> Result<QueryOutcome, sqlx::Error> {
        self.query_in(tenant, f, true).await
    }

    /// `query` with each row as the stored JSON text, never a `Value` tree
    /// (T-6055).
    pub async fn query_text(
        &self,
        tenant: &TenantId,
        f: &EntityFilter<'_>,
    ) -> Result<QueryOutcome<Box<serde_json::value::RawValue>>, sqlx::Error> {
        let o = self
            .query_in::<sqlx::types::Json<Box<serde_json::value::RawValue>>>(tenant, f, false)
            .await?;
        Ok(QueryOutcome {
            rows: o.rows.into_iter().map(|j| j.0).collect(),
            decided: o.decided,
            paged: o.paged,
            total: o.total,
        })
    }

    async fn query_in<R>(
        &self,
        tenant: &TenantId,
        f: &EntityFilter<'_>,
        candidates: bool,
    ) -> Result<QueryOutcome<R>, sqlx::Error>
    where
        R: for<'r> sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
    {
        let mut binds: Vec<Bind> = vec![Bind::Text(tenant.as_str().to_owned())];
        let mut wheres = vec![
            "tenant_id = $1".to_owned(),
            // 4.22: expired entities never enter a result (or a page/total)
            "(expires_at IS NULL OR expires_at > now())".to_owned(),
        ];
        // Exactness: ids/types/attrs translate exactly by
        // construction; q is exact IF it compiles (the compiler's contract);
        // scopeQ is documented loose-or-equal, geo has a metric residual
        // (`near` geography vs haversine) — both therefore forfeit
        // `decided`, they only narrow.
        let mut decided = true;
        let mut probes = Vec::new();

        if let Some(ids) = f.ids {
            binds.push(Bind::TextArr(super::storable(ids)));
            wheres.push(format!("id = ANY(${})", binds.len()));
        }
        // T-5661: exact id prefixes as byte ranges of the C-collated id
        // index (0027). Their bounding range is that index's condition and
        // its order the walk's, so a probe reads the range and never walks
        // a whole type in id order for a few late matches; each prefix's
        // own range decides.
        let order = match f.id_prefixes {
            Some(ps) => {
                wheres.push(prefix_ranges(ps, &mut binds));
                "id COLLATE \"C\""
            }
            None => "id",
        };
        // keyset: the pkey (tenant_id, id) seeks straight to the next page
        if let Some(after) = f.after_id {
            binds.push(Bind::Text(after.to_owned()));
            wheres.push(format!("{order} > ${}", binds.len()));
        }
        // 5.2.33 idPattern: the literal the regex forces is a plain string
        // test on the id column, exact as a narrowing; the regex itself stays
        // with the caller, so `decided` is untouched.
        if let Some(l) = f.id_literal {
            if l.text.contains('\0') {
                // no stored id holds a NUL (`storable`)
                wheres.push("false".to_owned());
            } else {
                binds.push(Bind::Text(l.text.to_owned()));
                wheres.push(if l.anchored {
                    format!("starts_with(id, ${})", binds.len())
                } else {
                    format!("position(${} in id) > 0", binds.len())
                });
            }
        }
        // a rule's idPatterns, exact where each reads alike as an ARE
        // (T-5826); a non-leakproof regex under RLS filters the rows the
        // tenant, type and prefix keys found, it reaches no index.
        // `regexp_like`, not `~`: the entity query is planned with its
        // values, and `~`'s estimator reads every fixed character of a
        // dataset pattern as a 1-in-5 filter, so it expects about one match
        // and sorts the whole prefix range (164k Trees, 10 s on dev) instead
        // of reading the C-collated id index in order up to the page's LIMIT.
        // A function has no estimator and keeps the default third.
        // The count does not use it: PostgreSQL's regex engine takes 3-12 µs
        // an id for these patterns (241 752 Gardens: 2.9 s on one core
        // against 0.2 s for the plain count, T-6066), so `count_ids` matches
        // the ids in the broker with the rule's own regex instead.
        let mut id_regexes = None;
        if let Some(ps) = f.id_patterns {
            match ps
                .iter()
                .map(|p| antares_store::filter::id_pattern_sql(p))
                .collect::<Option<Vec<_>>>()
            {
                Some(res) => {
                    // the regex `Rule::selects_id` judges with; a pattern it
                    // cannot compile leaves the count to SQL
                    id_regexes = ps
                        .iter()
                        .map(|p| antares_ql::regex::compile(&format!("^(?:{p})$")).ok())
                        .collect::<Option<Vec<_>>>()
                        .map(|re| (wheres.len(), re));
                    let any: Vec<String> = res
                        .into_iter()
                        .map(|re| {
                            binds.push(Bind::Text(re));
                            format!("regexp_like(id COLLATE \"C\", ${})", binds.len())
                        })
                        .collect();
                    wheres.push(match any.as_slice() {
                        [] => "false".to_owned(),
                        _ => format!("({})", any.join(" OR ")),
                    });
                }
                None => decided = false,
            }
        }
        // OR of AND-groups, mirroring the Entity Type Selection Language
        // (4.17) the caller already parsed, read as `TypeSplit` arms.
        let mut split = None;
        if let Some(groups) = f.types.filter(|g| !g.is_empty()) {
            match type_selection(groups, &mut binds) {
                Ok(s) => split = Some(s),
                Err(w) => wheres.push(w),
            }
        }
        // `attrs`: the entity holds at least one of them NOW — the
        // evaluator's `has_live_attr` after 4.22 stripped expired instances.
        // `?|` alone also matched an Attribute whose every instance expired.
        // Only a row with an expired instance (`attrs_expire_at` past, rare)
        // pays for the per-instance check, so the predicate stays decided
        // and the page stays pushed down.
        if let Some(attrs) = f.attrs {
            binds.push(Bind::TextArr(super::storable(attrs)));
            let n = binds.len();
            wheres.push(format!(
                "entity ?| ${n} AND (attrs_expire_at IS NULL OR attrs_expire_at >= now() \
                 OR EXISTS (SELECT 1 FROM unnest(${n}::text[]) AS w(a) \
                  WHERE entity ? w.a AND (jsonb_typeof(entity -> w.a) <> 'array' \
                   OR strpos(w.a, ':') = 0 OR jsonb_array_length(entity -> w.a) = 0 \
                   OR EXISTS (SELECT 1 FROM jsonb_array_elements(entity -> w.a) AS i(v) \
                    WHERE jsonb_typeof(i.v -> 'expiresAt') IS DISTINCT FROM 'string' \
                     OR try_timestamptz(i.v ->> 'expiresAt') IS NULL \
                     OR try_timestamptz(i.v ->> 'expiresAt') >= now()))))"
            ));
            probes.extend(attrs_probe(tenant, attrs));
        }
        if let Some(node) = f.q {
            match crate::compile::q::compile_q(
                node,
                LIVE_DOC,
                binds.len().saturating_add(1),
                f.expand,
            ) {
                Some(c) => {
                    wheres.push(live_q(&c.sql));
                    binds.extend(c.binds.into_iter().map(Bind::Path));
                    if let Some((iri, key)) = valued(node, f.expand) {
                        probes.push(value_probe(tenant, iri, key));
                    }
                }
                None => decided = false,
            }
            if unlocated(node, f.expand) {
                wheres.push("location IS NULL".to_owned());
            }
            if let Some(name) = named(node, f.expand) {
                binds.push(Bind::Text(name.to_owned()));
                wheres.push(format!("(name_key = ${} OR name_key IS NULL)", binds.len()));
            }
        }
        if let Some(sq) = f.scope_q {
            decided = false;
            if let Some(c) =
                crate::compile::scope::compile_scope_q(sq, "scopes", binds.len().saturating_add(1))
            {
                wheres.push(c.sql);
                binds.extend(c.binds.into_iter().map(Bind::Text));
                probes.extend(scope_probe(tenant, sq));
            }
        }
        let mut tx = self
            .begin_probed(tenant, &probes, &mut wheres, &mut binds)
            .await?;
        for spec in f.geo {
            decided = false;
            // A client may send a self-intersecting polygon. GEOS raises on
            // one (`side location conflict`), which would turn a query into a
            // 500 in `postgres` mode while `memory` mode answers happily from
            // the evaluator. Probing validity once here keeps the two modes
            // identical: invalid ⇒ no pushdown, evaluator decides. Stored
            // geometries can't be invalid — the write path NULLs those. The
            // probe runs on the query's own transaction, and a failed probe
            // fails the query (F-ANT1-016): a pool timeout answers 503
            // instead of costing the pushdown.
            if geometry_is_valid(&mut tx, spec).await? {
                if let Some(c) = crate::compile::geo::compile_geo(
                    spec,
                    "location",
                    binds.len().saturating_add(1),
                ) {
                    wheres.push(c.sql);
                    // geo binds first, then the numeric ones, then the cell
                    // cover — the order `compile_geo` numbered them in.
                    binds.extend(c.geo_binds.into_iter().map(Bind::Text));
                    binds.extend(c.num_binds.into_iter().map(Bind::Num));
                    binds.extend(c.cell_binds.into_iter().map(Bind::Text));
                }
            }
        }

        // every bind up to here belongs to the WHERE clause — the count-only
        // fallback statement below reuses exactly this prefix
        let where_binds = binds.len();

        // Projection pushdown, only once SQL decides row membership: the
        // kept doc must only need to feed `repr::apply`, never a re-check.
        // `pick` keeps listed attrs + every non-attribute member (attribute
        // keys are expanded IRIs — `http…` — so core members never match the
        // LIKE and always survive; a non-http attr IRI merely stays
        // unprojected, which is the safe direction). `omit` drops exactly the
        // listed top-level IRIs.
        let mut select = "entity".to_owned();
        if decided {
            if let Some(keep) = f.keep_attrs {
                binds.push(Bind::TextArr(super::storable(keep)));
                select = format!(
                    "(SELECT COALESCE(jsonb_object_agg(t.k, t.v), '{{}}'::jsonb) \
                      FROM jsonb_each(entity) AS t(k, v) \
                      WHERE t.k NOT LIKE 'http%' OR t.k = ANY(${}))",
                    binds.len()
                );
            } else if let Some(drop) = f.drop_attrs {
                binds.push(Bind::TextArr(super::storable(drop)));
                select = format!(
                    "(SELECT COALESCE(jsonb_object_agg(t.k, t.v), '{{}}'::jsonb) \
                      FROM jsonb_each(entity) AS t(k, v) \
                      WHERE NOT (t.k = ANY(${})))",
                    binds.len()
                );
            }
        }

        // Pagination pushdown: ORDER BY id is the store's default order
        // either way; the total for count= comes from `count_sql` below,
        // only when the page itself cannot tell it. When the page cannot be pushed the statement still carries
        // the safety LIMIT — without one, a single undecided request (any
        // `scopeQ`, any `georel`, a `q=` shape the compiler declined)
        // materializes the whole tenant into the Vec below.
        let ceiling = MAX_UNDECIDED_ROWS;
        let (sql, paged) = query_sql(
            &select,
            &wheres,
            split.as_ref(),
            f.page.as_ref(),
            decided || candidates,
            order,
            &mut binds,
        );
        // sqlx 0.9 makes dynamic SQL opt-in. The assertion holds by
        // construction: `sql` is built from this function's own literals
        // plus `$n` placeholders — no caller-supplied text reaches it.
        // The audit lives here, next to the builder.
        // Unprepared: a prepared statement goes to a generic plan after five
        // runs on a connection, and that plan cannot see a selective value:
        // 1.6 s instead of 1.5 ms at 100 000 entities when a GIN still
        // existed (T-3735). The text varies with every q, geo and
        // projection shape, so the statement cache saved little anyway.
        let mut qy = sqlx::query(sqlx::AssertSqlSafe(sql.clone())).persistent(false);
        for b in &binds {
            qy = match b {
                Bind::Text(s) | Bind::Path(s) => qy.bind(s),
                Bind::TextArr(v) => qy.bind(v),
                Bind::Num(n) => qy.bind(n),
                Bind::Int(n) => qy.bind(n),
            };
        }
        // Stream rows, decode each to its Value and
        // drop the PgRow — the full row set never sits in memory twice.
        let mut docs: Vec<R> = Vec::new();
        let mut total: Option<i64> = None;
        // candidates are no match set to count
        let counted = paged && decided && f.page.as_ref().is_some_and(|p| p.count);
        {
            use futures_util::TryStreamExt;
            let mut stream = qy.fetch(&mut *tx);
            while let Some(row) = stream.try_next().await? {
                docs.push(row.try_get::<R, _>(0)?);
            }
        }
        check_ceiling(paged, docs.len(), ceiling)?;
        // the extra row says a next page exists. Uncounted, the total
        // handed back is the smallest one consistent with that, enough for
        // the next/prev links and never shown as a count; counted, a last
        // page that holds rows is the total itself.
        if let Some(p) = f.page.as_ref().filter(|_| paged) {
            let more = docs.len() as i64 > p.limit;
            docs.truncate(p.limit as usize);
            total = page_total(p, docs.len(), more).filter(|_| decided);
        }
        // count=true with more rows past the page, or an off-the-end page:
        // count the match set under the same WHERE, binds and transaction
        if counted && total.is_none() {
            // same wheres ⇒ same bind prefix; stop before the
            // projection/page binds, which the count statement lacks
            let where_binds = binds.iter().take(where_binds);
            total = Some(match &id_regexes {
                Some((at, res)) => {
                    let mut wheres = wheres.clone();
                    // its bind stays numbered and unused: sqlx sends every
                    // parameter's type, so Postgres needs no reference to it
                    if let Some(w) = wheres.get_mut(*at) {
                        "true".clone_into(w);
                    }
                    let sql = ids_sql(&wheres, split.as_ref());
                    count_ids(&mut tx, &sql, where_binds, res).await?
                }
                None => {
                    // unprepared for the same reason as the statement above
                    let mut cq = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(count_sql(
                        &wheres,
                        split.as_ref(),
                    )))
                    .persistent(false);
                    for b in where_binds {
                        cq = match b {
                            Bind::Text(s) | Bind::Path(s) => cq.bind(s),
                            Bind::TextArr(v) => cq.bind(v),
                            Bind::Num(n) => cq.bind(n),
                            Bind::Int(n) => cq.bind(n),
                        };
                    }
                    cq.fetch_one(&mut *tx).await?
                }
            });
        }
        tx.commit().await?;
        Ok(QueryOutcome {
            rows: docs,
            decided,
            paged,
            total,
        })
    }

    /// The page's transaction with the Tenant set, and each probe's ids
    /// ANDed to `wheres` as `id = ANY(...)` (T-5303). A probed page reads in
    /// one snapshot (repeatable read), so no write between the probe and the
    /// page drops a match.
    async fn begin_probed(
        &self,
        tenant: &TenantId,
        probes: &[Probe],
        wheres: &mut Vec<String>,
        binds: &mut Vec<Bind>,
    ) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, sqlx::Error> {
        let mut tx = super::begin(&self.pool).await?;
        if !probes.is_empty() {
            sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
                .execute(&mut *tx)
                .await?;
        }
        crate::store::pg::set_tenant(&mut tx, tenant).await?;
        for p in probes {
            if let Some(ids) = probe_ids(&mut tx, p).await? {
                binds.push(Bind::TextArr(ids));
                wheres.push(format!("id = ANY(${})", binds.len()));
            }
        }
        Ok(tx)
    }

    /// Id-ordered snapshot for one tenant (the v0 `list` shape — still the
    /// path for every non-entity kind and for callers with no filter).
    ///
    /// Bounded like every other read: the statement carries the same
    /// `MAX_UNDECIDED_ROWS` safety LIMIT, and a tenant that reaches it is
    /// refused with TooManyResults (5.5.6) rather than served a silent
    /// prefix.
    pub async fn list(&self, tenant: &TenantId) -> Result<Vec<Value>, sqlx::Error> {
        let mut tx = super::begin(&self.pool).await?;
        crate::store::pg::set_tenant(&mut tx, tenant).await?;
        let mut docs: Vec<Value> = Vec::new();
        {
            use futures_util::TryStreamExt;
            let mut stream = sqlx::query(
                "SELECT entity FROM entities WHERE tenant_id = $1
                       AND (expires_at IS NULL OR expires_at > now())
                 ORDER BY id LIMIT $2",
            )
            .bind(tenant.as_str())
            .bind(MAX_UNDECIDED_ROWS)
            .fetch(&mut *tx);
            while let Some(row) = stream.try_next().await? {
                docs.push(row.get::<Value, _>(0));
            }
        }
        tx.commit().await?;
        check_ceiling(false, docs.len(), MAX_UNDECIDED_ROWS)?;
        Ok(docs)
    }

    /// One id-ordered page of entities: ids strictly greater than `after`,
    /// at most `limit`.
    ///
    /// No ceiling, and no fold of the tenant to build the page.
    /// `MAX_UNDECIDED_ROWS` on `list` exists so a large tenant cannot be
    /// materialized into one `Vec`; a page bounds that by construction, so
    /// refusing here would only break the readers that must see every row
    /// and have no TooManyResults to raise — 5.9.2.4's registration-vs-entity
    /// conflict check among them. Keyset over the primary key, not OFFSET:
    /// the walk runs against a table being written to. `after = None` is
    /// bound as the empty string, which is below every id because an
    /// Entity id is a URI and a URI is never empty.
    ///
    /// 4.22 at the entity level is in the statement, so a full page is a
    /// full page: the caller reads a short page as the end of the tenant,
    /// and a filter applied after the LIMIT would end the walk early.
    pub async fn list_page(
        &self,
        tenant: &TenantId,
        after: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Value>, sqlx::Error> {
        let mut tx = super::begin(&self.pool).await?;
        crate::store::pg::set_tenant(&mut tx, tenant).await?;
        let rows = sqlx::query(
            "SELECT entity FROM entities WHERE tenant_id = $1 AND id > $2
                   AND (expires_at IS NULL OR expires_at > now())
             ORDER BY id LIMIT $3",
        )
        .bind(tenant.as_str())
        .bind(after.unwrap_or(""))
        .bind(limit)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows.into_iter().map(|r| r.get::<Value, _>(0)).collect())
    }

    /// One id-ordered `LIMIT`/`OFFSET` window of the Tenant's live Entities
    /// and their count, in one snapshot: the window bounds the read, so no
    /// row ceiling applies (`CurrentStateDriver::list_slice`, F-ANT1-011).
    pub async fn list_slice(
        &self,
        tenant: &TenantId,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<Value>, i64), sqlx::Error> {
        let mut tx = super::begin(&self.pool).await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await?;
        crate::store::pg::set_tenant(&mut tx, tenant).await?;
        let rows = sqlx::query(
            "SELECT entity FROM entities WHERE tenant_id = $1
                   AND (expires_at IS NULL OR expires_at > now())
             ORDER BY id LIMIT $2 OFFSET $3",
        )
        .bind(tenant.as_str())
        .bind(limit)
        .bind(offset)
        .fetch_all(&mut *tx)
        .await?;
        let total: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM entities WHERE tenant_id = $1
                   AND (expires_at IS NULL OR expires_at > now())",
        )
        .bind(tenant.as_str())
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok((
            rows.into_iter().map(|r| r.get::<Value, _>(0)).collect(),
            total,
        ))
    }

    /// Read-modify-write: row lock via `SELECT … FOR NO KEY UPDATE`, closure
    /// applied in Rust, `version` bumped under the lock. Two racing PATCHes
    /// serialize in Postgres, neither is lost. `Ok(None)` = entity absent.
    ///
    /// No write changes the key (tenant_id, id), so the lock is the one the
    /// UPDATE itself takes: it serializes writers and leaves the history
    /// append's `FOR KEY SHARE` alone (T-4669: a `FOR UPDATE` here waited on
    /// every in-flight append and made the append wait on it in turn, until
    /// `lock_timeout` dropped the history of a whole drained request).
    pub async fn mutate<T, E>(
        &self,
        tenant: &TenantId,
        id: &str,
        f: impl FnOnce(&mut Value) -> Result<T, E>,
    ) -> Result<Option<Result<T, E>>, sqlx::Error> {
        let mut tx = super::begin(&self.pool).await?;
        crate::store::pg::set_tenant(&mut tx, tenant).await?;
        // 4.22: an expired row is invalid, so a patch of it is a 404 —
        // the same answer the retrieve it would follow already gives.
        let row = sqlx::query(
            "SELECT entity FROM entities WHERE tenant_id = $1 AND id = $2
               AND (expires_at IS NULL OR expires_at > now()) FOR NO KEY UPDATE",
        )
        .bind(tenant.as_str())
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            tx.commit().await?;
            return Ok(None);
        };
        let mut doc: Value = row.get(0);
        let before = self.outbox_on().then(|| doc.clone());
        match f(&mut doc) {
            Ok(t) => {
                let e = extract(&doc);
                let updated = sqlx::query(
                    "UPDATE entities SET entity = $3, types = $4, scopes = $5,
                       modified_at = $6::timestamptz, expires_at = $7::timestamptz,
                       location = CASE WHEN ST_IsValid(try_geomfromgeojson($8))
                                       THEN try_geomfromgeojson($8) END,
                       location_ambiguous = $9 OR ($8 IS NOT NULL
                           AND NOT COALESCE(ST_IsValid(try_geomfromgeojson($8)), false)),
                       attrs_expire_at = try_timestamptz($10),
                       version = version + 1
                     WHERE tenant_id = $1 AND id = $2
                     RETURNING version, created_at::text",
                )
                .bind(tenant.as_str())
                .bind(id)
                .bind(&doc)
                .bind(&e.types)
                .bind(&e.scopes)
                .bind(&e.modified)
                .bind(&e.expires)
                .bind(&e.location)
                .bind(e.location_ambiguous)
                .bind(&e.attrs_expire)
                .fetch_one(&mut *tx)
                .await?;
                if let Some(before) = &before {
                    enqueue_change(
                        &mut tx,
                        tenant,
                        "update",
                        id,
                        &e.types,
                        Some(before),
                        Some(&doc),
                        updated.get::<i64, _>(0),
                        updated.get::<&str, _>(1),
                    )
                    .await?;
                }
                tx.commit().await?;
                Ok(Some(Ok(t)))
            }
            Err(e) => {
                tx.rollback().await?;
                Ok(Some(Err(e)))
            }
        }
    }

    /// Batch create: ONE multi-row INSERT for the whole batch (the jsonb
    /// elements form of UNNEST), one transaction, one commit.
    /// Returns a created-flag per input item, input order preserved.
    /// Duplicate ids within one batch are pre-deduped here: `ON CONFLICT`
    /// raises "cannot affect row a second time" otherwise — the later
    /// duplicate reports `false` (5.5.11.1: the first instance wins).
    ///
    /// 5.6.7.4: "For each of the NGSI-LD Entities included in the input
    /// Array execute the behaviour defined by clause 5.6.1, but limited to a
    /// local operation" — so the conflict clause is `create`'s, and an
    /// entity past its `expiresAt` is absent (4.22) and gets created over
    /// rather than reported as an id that already exists.
    pub async fn batch_create(
        &self,
        tenant: &TenantId,
        items: &[(String, Value)],
    ) -> Result<Vec<bool>, sqlx::Error> {
        let mut seen = std::collections::HashSet::new();
        let mut payload = Vec::new();
        for (id, doc) in items {
            if seen.insert(id.as_str()) {
                let e = extract(doc);
                payload.push(serde_json::json!({
                    "id": id, "doc": doc, "types": e.types, "scopes": e.scopes,
                    "created": e.created, "modified": e.modified, "expires": e.expires,
                    "location": e.location, "loc_ambiguous": e.location_ambiguous,
                    "attrs_expire": e.attrs_expire,
                }));
            }
        }
        let mut tx = self.begin_batch().await?;
        crate::store::pg::set_tenant(&mut tx, tenant).await?;
        crate::store::pg::claim_tenant(&mut tx, tenant).await?;
        let rows = sqlx::query(
            "INSERT INTO entities
               (tenant_id, id, entity, types, scopes, created_at, modified_at, expires_at,
                location, location_ambiguous, attrs_expire_at)
             SELECT $1, e->>'id', e->'doc',
                    ARRAY(SELECT jsonb_array_elements_text(e->'types')),
                    CASE WHEN e->'scopes' = 'null'::jsonb THEN NULL
                         ELSE ARRAY(SELECT jsonb_array_elements_text(e->'scopes')) END,
                    (e->>'created')::timestamptz, (e->>'modified')::timestamptz,
                    (e->>'expires')::timestamptz,
                    CASE WHEN ST_IsValid(try_geomfromgeojson(e->>'location'))
                         THEN try_geomfromgeojson(e->>'location') END,
                    COALESCE((e->>'loc_ambiguous')::bool, false)
                      OR (e->>'location' IS NOT NULL
                          AND NOT COALESCE(ST_IsValid(try_geomfromgeojson(e->>'location')), false)),
                    try_timestamptz(e->>'attrs_expire')
             FROM jsonb_array_elements($2::jsonb) AS e
             ON CONFLICT (tenant_id, id) DO UPDATE SET
                    entity = EXCLUDED.entity, types = EXCLUDED.types,
                    scopes = EXCLUDED.scopes, created_at = EXCLUDED.created_at,
                    modified_at = EXCLUDED.modified_at,
                    expires_at = EXCLUDED.expires_at, location = EXCLUDED.location,
                    location_ambiguous = EXCLUDED.location_ambiguous,
                    attrs_expire_at = EXCLUDED.attrs_expire_at,
                    version = 1
             WHERE entities.expires_at IS NOT NULL AND entities.expires_at <= now()
             RETURNING id",
        )
        .bind(tenant.as_str())
        .bind(Value::Array(payload))
        .fetch_all(&mut *tx)
        .await?;
        let created_now: std::collections::HashSet<String> =
            rows.into_iter().map(|r| r.get::<String, _>(0)).collect();
        if self.outbox_on() {
            let mut seen_ev = std::collections::HashSet::new();
            let mut events = Vec::new();
            for (id, doc) in items {
                if created_now.contains(id.as_str()) && seen_ev.insert(id.as_str()) {
                    let e = extract(doc);
                    events.push(change_event(
                        tenant,
                        "create",
                        id,
                        &e.types,
                        None,
                        Some(doc),
                        1,
                        &e.created,
                    ));
                }
            }
            super::outbox::enqueue_many(&mut tx, tenant, &events).await?;
        }
        tx.commit().await?;
        let mut created = created_now;
        // consume-once: a duplicate of a created id still reports false
        Ok(items
            .iter()
            .map(|(id, _)| created.remove(id.as_str()))
            .collect())
    }

    /// Batch delete: ONE statement, returning each deleted row's previous
    /// document (the change-hook before-image).
    ///
    /// 5.6.10.4: "For each of the NGSI-LD Entity IDs included in the input
    /// Array execute the behaviour defined by clause 5.6.6, but limited to a
    /// local operation" — so an entity past its `expiresAt` is absent (4.22)
    /// and is not deleted here either, exactly as `delete` refuses it.
    pub async fn batch_delete(
        &self,
        tenant: &TenantId,
        ids: &[String],
    ) -> Result<Vec<(String, Value)>, sqlx::Error> {
        let mut tx = super::begin(&self.pool).await?;
        crate::store::pg::set_tenant(&mut tx, tenant).await?;
        let rows = sqlx::query(
            "DELETE FROM entities WHERE tenant_id = $1 AND id = ANY($2)
           AND (expires_at IS NULL OR expires_at > now())
         RETURNING id, entity, types, version, created_at::text",
        )
        .bind(tenant.as_str())
        .bind(ids)
        .fetch_all(&mut *tx)
        .await?;
        if self.outbox_on() {
            let events: Vec<Value> = rows
                .iter()
                .map(|r| {
                    let id: String = r.get(0);
                    let prev: Value = r.get(1);
                    let types: Vec<String> = r.get(2);
                    change_event(
                        tenant,
                        "delete",
                        &id,
                        &types,
                        Some(&prev),
                        None,
                        r.get::<i64, _>(3),
                        r.get::<&str, _>(4),
                    )
                })
                .collect();
            super::outbox::enqueue_many(&mut tx, tenant, &events).await?;
        }
        tx.commit().await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<String, _>(0), r.get::<Value, _>(1)))
            .collect())
    }

    /// Batch upsert in REPLACE semantics — ONE
    /// `INSERT … ON CONFLICT DO UPDATE` for the whole batch, one transaction.
    /// Returns a created-flag per input item (input order; duplicates of one
    /// id report the first outcome). Before-images for the change events are
    /// captured by a single `FOR NO KEY UPDATE` select in the same
    /// transaction (see [`Self::mutate`]). Returns per input item: (created?,
    /// before-image) — the before-image is for the caller's change hook and
    /// comes from the same-tx lock.
    ///
    /// 5.6.8.4: "Create the Entity locally if it does not exist (i.e. no
    /// Entity with the same Entity ID is present) executing the behaviour
    /// defined by clause 5.6.1, but limited to a local operation" — an
    /// entity past its `expiresAt` does not exist (4.22), so it takes the
    /// creation branch: fresh `created_at`, `version` back to 1 and a
    /// created-flag, which is what splits 201 from 204 at the API.
    pub async fn batch_upsert_replace(
        &self,
        tenant: &TenantId,
        items: &[(String, Value)],
    ) -> Result<Vec<(bool, Option<Value>)>, sqlx::Error> {
        let mut seen = std::collections::HashSet::new();
        let mut payload = Vec::new();
        let mut ids = Vec::new();
        for (id, doc) in items {
            if seen.insert(id.as_str()) {
                let e = extract(doc);
                ids.push(id.clone());
                payload.push(serde_json::json!({
                    "id": id, "doc": doc, "types": e.types, "scopes": e.scopes,
                    "created": e.created, "modified": e.modified, "expires": e.expires,
                    "location": e.location, "loc_ambiguous": e.location_ambiguous,
                    "attrs_expire": e.attrs_expire,
                }));
            }
        }
        let mut tx = self.begin_batch().await?;
        crate::store::pg::set_tenant(&mut tx, tenant).await?;
        crate::store::pg::claim_tenant(&mut tx, tenant).await?;
        // lock + before-images in one statement (ordered: stable lock order)
        let prev_rows = sqlx::query(
            "SELECT id, entity,
                    (expires_at IS NOT NULL AND expires_at <= now()) AS expired
             FROM entities WHERE tenant_id = $1 AND id = ANY($2)
             ORDER BY id FOR NO KEY UPDATE",
        )
        .bind(tenant.as_str())
        .bind(&ids)
        .fetch_all(&mut *tx)
        .await?;
        // 4.22: an expired row is already invalid, so it is not a
        // before-image — the upsert that replaces it is a creation and
        // its change event has nothing before it. The lock still covers
        // the row, so the stable lock order is unchanged.
        let prevs: std::collections::HashMap<String, Value> = prev_rows
            .into_iter()
            .filter(|r| !r.get::<bool, _>(2))
            .map(|r| (r.get::<String, _>(0), r.get::<Value, _>(1)))
            .collect();
        let rows = sqlx::query(
            "INSERT INTO entities
               (tenant_id, id, entity, types, scopes, created_at, modified_at, expires_at,
                location, location_ambiguous, attrs_expire_at)
             SELECT $1, e->>'id', e->'doc',
                    ARRAY(SELECT jsonb_array_elements_text(e->'types')),
                    CASE WHEN e->'scopes' = 'null'::jsonb THEN NULL
                         ELSE ARRAY(SELECT jsonb_array_elements_text(e->'scopes')) END,
                    (e->>'created')::timestamptz, (e->>'modified')::timestamptz,
                    (e->>'expires')::timestamptz,
                    CASE WHEN ST_IsValid(try_geomfromgeojson(e->>'location'))
                         THEN try_geomfromgeojson(e->>'location') END,
                    COALESCE((e->>'loc_ambiguous')::bool, false)
                      OR (e->>'location' IS NOT NULL
                          AND NOT COALESCE(ST_IsValid(try_geomfromgeojson(e->>'location')), false)),
                    try_timestamptz(e->>'attrs_expire')
             FROM jsonb_array_elements($2::jsonb) AS e
             ON CONFLICT (tenant_id, id) DO UPDATE SET
                    entity = EXCLUDED.entity, types = EXCLUDED.types,
                    scopes = EXCLUDED.scopes, modified_at = EXCLUDED.modified_at,
                    expires_at = EXCLUDED.expires_at, location = EXCLUDED.location,
                    location_ambiguous = EXCLUDED.location_ambiguous,
                    attrs_expire_at = EXCLUDED.attrs_expire_at,
                    created_at = CASE WHEN entities.expires_at IS NOT NULL
                                       AND entities.expires_at <= now()
                                      THEN EXCLUDED.created_at
                                      ELSE entities.created_at END,
                    version = CASE WHEN entities.expires_at IS NOT NULL
                                    AND entities.expires_at <= now()
                                   THEN 1 ELSE entities.version + 1 END
             RETURNING id, (xmax = 0 OR version = 1) AS inserted,
                       version, created_at::text",
        )
        .bind(tenant.as_str())
        .bind(Value::Array(payload))
        .fetch_all(&mut *tx)
        .await?;
        let mut created: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
        let mut events = Vec::new();
        for r in &rows {
            let id: String = r.get(0);
            let inserted: bool = r.get(1);
            // the id came out of `items`, so the lookup answers; an
            // outbox row for a document that is not there would carry no
            // payload, so there is nothing to emit either way
            if let (true, Some(doc)) = (
                self.outbox_on(),
                items.iter().find(|(i, _)| *i == id).map(|(_, d)| d),
            ) {
                let e = extract(doc);
                events.push(change_event(
                    tenant,
                    if inserted { "create" } else { "update" },
                    &id,
                    &e.types,
                    prevs.get(&id),
                    Some(doc),
                    r.get::<i64, _>(2),
                    r.get::<&str, _>(3),
                ));
            }
            created.insert(id, inserted);
        }
        super::outbox::enqueue_many(&mut tx, tenant, &events).await?;
        tx.commit().await?;
        Ok(items
            .iter()
            .map(|(id, _)| {
                (
                    created.get(id).copied().unwrap_or(false),
                    prevs.get(id).cloned(),
                )
            })
            .collect())
    }

    /// Batch read-modify-write — ONE transaction,
    /// all rows locked in a single ordered `FOR NO KEY UPDATE` select (see
    /// [`Self::mutate`]), the closure
    /// applied per doc in Rust, and ONE multi-row UPDATE writeback. Per-item
    /// results align with `ids`: `None` = absent, `Some(Err)` = the closure
    /// rejected that item (its write is skipped, the rest proceed).
    pub async fn batch_mutate<E>(
        &self,
        tenant: &TenantId,
        ids: &[String],
        mut f: impl FnMut(&str, &mut Value) -> Result<(), E>,
    ) -> Result<Vec<Option<Result<(), E>>>, sqlx::Error> {
        let mut tx = self.begin_batch().await?;
        crate::store::pg::set_tenant(&mut tx, tenant).await?;
        let rows = sqlx::query(
            // 4.22: an expired entity is invalid, so it is not a row this
            // update can find — 5.6.9.4 is clause 5.6.3 run per item, and
            // 5.6.3 on an absent entity is ResourceNotFound. Without the
            // predicate the batch reported the id as updated, wrote to
            // it, and emitted a change event for an entity every read
            // refuses. ORDER BY id keeps the lock order stable.
            "SELECT id, entity FROM entities
          WHERE tenant_id = $1 AND id = ANY($2)
            AND (expires_at IS NULL OR expires_at > now())
         ORDER BY id FOR NO KEY UPDATE",
        )
        .bind(tenant.as_str())
        .bind(ids)
        .fetch_all(&mut *tx)
        .await?;
        let mut docs: std::collections::HashMap<String, Value> = rows
            .into_iter()
            .map(|r| (r.get::<String, _>(0), r.get::<Value, _>(1)))
            .collect();
        let mut results: Vec<Option<Result<(), E>>> = Vec::with_capacity(ids.len());
        // 5.6.9.4 is clause 5.6.3 run per item, so a repeated id applies
        // its closure once per item onto the SAME document and the last
        // application is the state the row must end at. The writeback
        // below is one `UPDATE … FROM`, which updates a target row once
        // from whichever source row the join happens to reach, so the
        // statement is fed one row per id — the document as the whole
        // batch left it — rather than one per input item.
        let mut write_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut changed: Vec<(String, Value, Value)> = Vec::new(); // id, before, after
        for id in ids {
            match docs.get_mut(id) {
                None => results.push(None),
                Some(doc) => {
                    let before = doc.clone();
                    match f(id, doc) {
                        Err(e) => {
                            *doc = before; // closure failed: discard its edits
                            results.push(Some(Err(e)));
                        }
                        Ok(()) => {
                            if *doc != before {
                                write_ids.insert(id.as_str());
                                changed.push((id.clone(), before, doc.clone()));
                            }
                            results.push(Some(Ok(())));
                        }
                    }
                }
            }
        }
        let payload: Vec<Value> = write_ids
            .iter()
            .filter_map(|id| docs.get(*id).map(|doc| (*id, doc)))
            .map(|(id, doc)| {
                let e = extract(doc);
                serde_json::json!({
                    "id": id, "doc": doc, "types": e.types,
                    "scopes": e.scopes, "modified": e.modified,
                    "expires": e.expires, "location": e.location,
                    "loc_ambiguous": e.location_ambiguous,
                    "attrs_expire": e.attrs_expire,
                })
            })
            .collect();
        if !payload.is_empty() {
            let updated = sqlx::query(
                "UPDATE entities t SET
                    entity = e->'doc',
                    types = ARRAY(SELECT jsonb_array_elements_text(e->'types')),
                    scopes = CASE WHEN e->'scopes' = 'null'::jsonb THEN NULL
                                  ELSE ARRAY(SELECT jsonb_array_elements_text(e->'scopes')) END,
                    modified_at = (e->>'modified')::timestamptz,
                    expires_at = (e->>'expires')::timestamptz,
                    location = CASE WHEN ST_IsValid(try_geomfromgeojson(e->>'location'))
                                    THEN try_geomfromgeojson(e->>'location') END,
                    location_ambiguous = COALESCE((e->>'loc_ambiguous')::bool, false)
                      OR (e->>'location' IS NOT NULL
                          AND NOT COALESCE(ST_IsValid(try_geomfromgeojson(e->>'location')), false)),
                    attrs_expire_at = try_timestamptz(e->>'attrs_expire'),
                    version = t.version + 1
             FROM jsonb_array_elements($2::jsonb) AS e
             WHERE t.tenant_id = $1 AND t.id = e->>'id'
             RETURNING t.id, t.version, t.created_at::text",
            )
            .bind(tenant.as_str())
            .bind(Value::Array(payload))
            .fetch_all(&mut *tx)
            .await?;
            if self.outbox_on() {
                let meta: std::collections::HashMap<String, (i64, String)> = updated
                    .into_iter()
                    .map(|r| {
                        (
                            r.get::<String, _>(0),
                            (r.get::<i64, _>(1), r.get::<String, _>(2)),
                        )
                    })
                    .collect();
                let events: Vec<Value> = changed
                    .iter()
                    .filter_map(|(id, before, after)| {
                        let e = extract(after);
                        let (version, incarnation) = meta.get(id)?;
                        Some(change_event(
                            tenant,
                            "update",
                            id,
                            &e.types,
                            Some(before),
                            Some(after),
                            *version,
                            incarnation,
                        ))
                    })
                    .collect();
                super::outbox::enqueue_many(&mut tx, tenant, &events).await?;
            }
        }
        tx.commit().await?;
        Ok(results)
    }

    /// Current row version (test hook for the version-monotonicity assertions).
    #[cfg(any(test, feature = "test-kit"))]
    pub async fn version(&self, tenant: &TenantId, id: &str) -> Result<Option<i64>, sqlx::Error> {
        let mut tx = super::begin(&self.pool).await?;
        crate::store::pg::set_tenant(&mut tx, tenant).await?;
        let row = sqlx::query("SELECT version FROM entities WHERE tenant_id = $1 AND id = $2")
            .bind(tenant.as_str())
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(row.map(|r| r.get::<i64, _>(0)))
    }
}

/// 4.22 for the current state: drop every Attribute instance whose
/// `expiresAt` passed from the stored docs, at most `batch` of them, inside
/// the sweep's transaction `tx` (service escape on). The rows come by the
/// `attrs_expire_at` index; each Tenant's are locked and rewritten under that
/// Tenant, as 0005 grants the escape no UPDATE. `modified_at` stays: an
/// expiry is no modification, so no change event is owed either. A row a
/// writer holds is skipped and taken on the next tick. Returns the rows
/// rewritten.
pub(super) async fn prune_expired_current(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    batch: i64,
) -> Result<u64, sqlx::Error> {
    let due: Vec<(String, String)> = sqlx::query_as(
        "SELECT tenant_id, id FROM entities
          WHERE attrs_expire_at IS NOT NULL AND attrs_expire_at < now()
          ORDER BY attrs_expire_at LIMIT $1",
    )
    .bind(batch)
    .fetch_all(&mut **tx)
    .await?;
    let mut by_tenant: std::collections::BTreeMap<&str, Vec<&str>> =
        std::collections::BTreeMap::new();
    for (tenant, id) in &due {
        by_tenant.entry(tenant).or_default().push(id);
    }
    let mut n = 0u64;
    for (tenant, ids) in by_tenant {
        sqlx::query(crate::SET_TENANT_SQL)
            .bind(tenant)
            .execute(&mut **tx)
            .await?;
        let rows = sqlx::query(
            "SELECT id, entity,
                    to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')
               FROM entities
              WHERE tenant_id = $1 AND id = ANY($2::text[]) AND attrs_expire_at < now()
              FOR NO KEY UPDATE SKIP LOCKED",
        )
        .bind(tenant)
        .bind(&ids)
        .fetch_all(&mut **tx)
        .await?;
        for r in rows {
            let id: String = r.get(0);
            let mut doc: Value = r.get(1);
            let now: String = r.get(2);
            antares_store::filter::prune_expired(&mut doc, &now);
            let e = extract(&doc);
            let rewritten = sqlx::query(
                "UPDATE entities SET entity = $3,
                   location = CASE WHEN ST_IsValid(try_geomfromgeojson($4))
                                   THEN try_geomfromgeojson($4) END,
                   location_ambiguous = $5 OR ($4 IS NOT NULL
                       AND NOT COALESCE(ST_IsValid(try_geomfromgeojson($4)), false)),
                   attrs_expire_at = try_timestamptz($6),
                   version = version + 1
                 WHERE tenant_id = $1 AND id = $2",
            )
            .bind(tenant)
            .bind(&id)
            .bind(&doc)
            .bind(&e.location)
            .bind(e.location_ambiguous)
            .bind(&e.attrs_expire)
            .execute(&mut **tx)
            .await?
            .rows_affected();
            n = n.saturating_add(rewritten);
        }
    }
    Ok(n)
}

/// One cheap probe on `tx`: is the client's query geometry OGC-valid?
/// Unparseable GeoJSON counts as invalid — same outcome, no pushdown — and
/// reaches that answer through `try_geomfromgeojson` (0001_init.sql), so only
/// the connection can make the probe fail.
async fn geometry_is_valid(
    tx: &mut sqlx::PgConnection,
    spec: &crate::compile::geo::GeoSpec<'_>,
) -> Result<bool, sqlx::Error> {
    let geojson = serde_json::to_string(&serde_json::json!({
        "type": spec.geometry, "coordinates": spec.coordinates
    }))
    .unwrap_or_default();
    sqlx::query_scalar::<_, bool>("SELECT COALESCE(ST_IsValid(try_geomfromgeojson($1)), false)")
        .bind(&geojson)
        .fetch_one(&mut *tx)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn wheres() -> Vec<String> {
        vec!["tenant_id = $1".to_owned()]
    }

    /// T-6351: a `q` of three leaves names the expiry-stripped document 27
    /// times, and the statement still holds its rebuild once.
    #[test]
    fn a_q_holds_the_expiry_rebuild_once() {
        let expand = |t: &str| format!("https://x/{t}");
        let node = antares_ql::parse_q(r#"level>=10;level<20;category=="c3""#).expect("q parses");
        let c = crate::compile::q::compile_q(&node, LIVE_DOC, 2, &expand).expect("compiles");
        assert!(c.sql.matches(LIVE_DOC).count() >= 27, "{}", c.sql);
        let sql = live_q(&c.sql);
        assert_eq!(sql.matches("jsonb_each(entity)").count(), 1, "{sql}");
        assert_eq!(sql.matches(LIVE_ENTITY).count(), 1, "{sql}");
        assert!(sql.contains("OFFSET 0) AS live WHERE"), "{sql}");
    }

    /// T-5136: only a `!location` every match must satisfy narrows to
    /// `location IS NULL`; an alternative, a sub-path or another
    /// geoproperty does not.
    #[test]
    fn only_a_required_missing_location_narrows_to_no_geometry() {
        let expand = |t: &str| match t {
            "location" => crate::compile::geo::LOCATION_IRI.to_owned(),
            t => format!("https://x/{t}"),
        };
        let narrows = |q: &str| unlocated(&antares_ql::parse_q(q).expect("q parses"), &expand);
        for q in ["!location", "a>1;!location", "(a==1;!location);b"] {
            assert!(narrows(q), "{q}");
        }
        for q in [
            "location",
            "!location|a",
            "!location.observedAt",
            "!place",
            "a;b",
            "!location[x]",
        ] {
            assert!(!narrows(q), "{q}");
        }
    }

    /// T-5349: only a `name=="…"` string every match must equal narrows to
    /// `name_key`; an alternative, another operator or value, a sub-path or
    /// another attribute does not.
    /// T-5303: the top-level terms `entity_values` keys, each probing the
    /// rows a match must hold; everything else is left to the `@?`.
    #[test]
    fn a_top_level_value_term_probes_entity_values() {
        let expand = |t: &str| format!("https://x/{t}");
        let tenant = TenantId::new("t").expect("tenant");
        let long = "é".repeat(600);
        let long_q = format!(r#"b=="{long}""#);
        for (q, attr, cond, nums, text) in [
            (
                "a==42",
                "https://x/a",
                "f = ANY(ARRAY[$3, 'NaN'::float8])",
                vec![42.0],
                None,
            ),
            ("a>1.5", "https://x/a", "f >= $3", vec![1.5], None),
            (
                "x==1;a>=2",
                "https://x/x",
                "f = ANY(ARRAY[$3, 'NaN'::float8])",
                vec![1.0],
                None,
            ),
            (
                "a<0",
                "https://x/a",
                "(f <= $3 OR f = 'NaN')",
                vec![0.0],
                None,
            ),
            (
                "a<=-1",
                "https://x/a",
                "(f <= $3 OR f = 'NaN')",
                vec![-1.0],
                None,
            ),
            (
                "a==1..3",
                "https://x/a",
                "(f BETWEEN $3 AND $4 OR f = 'NaN')",
                vec![1.0, 3.0],
                None,
            ),
            (
                r#"a=="x""#,
                "https://x/a",
                "(txt = $3 OR f = 'NaN')",
                vec![],
                Some("x"),
            ),
            (long_q.as_str(), "https://x/b", "f = 'NaN'", vec![], None),
        ] {
            let node = antares_ql::parse_q(q).expect("q parses");
            let (iri, key) = valued(&node, &expand).expect(q);
            assert_eq!(iri, attr, "{q}");
            let p = value_probe(&tenant, iri, key);
            let limit = p.binds.len();
            let rows = format!("FROM entity_values WHERE tenant_id = $1 AND attr = $2 AND {cond}");
            assert_eq!(
                p.sql,
                format!(
                    "SELECT CASE WHEN n < ${limit} THEN ARRAY(SELECT id {rows}) END \
                     FROM (SELECT count(*) AS n FROM (SELECT 1 {rows} LIMIT ${limit}) r) c"
                ),
                "{q}"
            );
            let texts: Vec<&str> = p
                .binds
                .iter()
                .filter_map(|b| match b {
                    Bind::Text(t) => Some(t.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(
                texts,
                ["t", attr].into_iter().chain(text).collect::<Vec<_>>(),
                "{q}"
            );
            let got_nums: Vec<f64> = p
                .binds
                .iter()
                .filter_map(|b| match b {
                    Bind::Num(n) => Some(*n),
                    _ => None,
                })
                .collect();
            assert_eq!(got_nums, nums, "{q}");
            assert!(matches!(p.binds.last(), Some(Bind::Int(PROBE_ROWS))), "{q}");
        }
        for q in [
            "a",
            "!a",
            "a!=1",
            "a==1|b==2",
            "a==true",
            "a==1,2",
            r#"a=="x".."y""#,
            r#"a~="x""#,
            "a.sub==1",
            "a[k]==1",
            r#"a>"x""#,
        ] {
            let node = antares_ql::parse_q(q).expect("q parses");
            assert_eq!(valued(&node, &expand), None, "{q}");
        }
    }

    /// T-5303: `attrs=` probes presence rows unless it names one of the
    /// Entity's own members, which have none.
    #[test]
    fn attrs_probe_presence_rows_unless_they_name_an_own_member() {
        let tenant = TenantId::new("t").expect("tenant");
        let attrs = ["https://x/a".to_owned(), "https://x/b".to_owned()];
        let p = attrs_probe(&tenant, &attrs).expect("probe");
        assert_eq!(
            p.sql,
            "SELECT CASE WHEN n < $3 THEN ARRAY(SELECT id FROM entity_values WHERE tenant_id = $1 \
             AND attr = ANY($2)) END FROM (SELECT count(*) AS n FROM (SELECT 1 FROM entity_values \
             WHERE tenant_id = $1 AND attr = ANY($2) LIMIT $3) r) c"
        );
        assert!(matches!(p.binds.get(1), Some(Bind::TextArr(v)) if v.as_slice() == attrs));
        for own in OWN_MEMBERS {
            let attrs = ["https://x/a".to_owned(), (*own).to_owned()];
            assert!(attrs_probe(&tenant, &attrs).is_none(), "{own}");
        }
    }

    #[test]
    fn only_a_required_name_string_narrows_to_the_name_key() {
        let expand = |t: &str| match t {
            "name" => NAME_IRI.to_owned(),
            t => format!("https://x/{t}"),
        };
        let wants =
            |q: &str| named(&antares_ql::parse_q(q).expect("q parses"), &expand).map(str::to_owned);
        for (q, want) in [
            (r#"name=="Bratislavský""#, "Bratislavský"),
            (r#"a>1;name=="x""#, "x"),
            (r#"(a==1;name=="");b"#, ""),
        ] {
            assert_eq!(wants(q).as_deref(), Some(want), "{q}");
        }
        for q in [
            "name",
            r#"name!="x""#,
            r#"name=="x"|a==1"#,
            "name==1",
            r#"name=="x","y""#,
            r#"name~="x""#,
            r#"name.sub=="x""#,
            r#"name[x]=="x""#,
            r#"title=="x""#,
        ] {
            assert_eq!(wants(q), None, "{q}");
        }
    }

    /// `location_ambiguous` is the single bit that keeps geoquery pushdown
    /// honest: `compile_geo` ORs it into every predicate so a row whose
    /// default GeoProperty could not be reduced to ONE geometry still reaches
    /// the evaluator. It must be true exactly when the doc CARRIES the
    /// geoproperty and extraction refused — a row with no geoproperty at all
    /// can never match a geoquery and is excluded in SQL, which is the whole
    /// point of the column.
    #[test]
    fn location_ambiguous_is_carried_but_unextractable() {
        let loc = crate::compile::geo::LOCATION_IRI;
        let point = json!({"type": "Point", "coordinates": [2.29, 48.85]});

        // one default GeoProperty instance with a geometry: extracted, exact
        let e = extract(&json!({"id": "urn:x", loc: [{"value": point}]}));
        assert!(e.location.is_some());
        assert!(!e.location_ambiguous);

        // multi-instance: no single geometry, so the evaluator arbitrates
        let e = extract(&json!({"id": "urn:x", loc: [{"value": point}, {"value": point}]}));
        assert!(e.location.is_none());
        assert!(
            e.location_ambiguous,
            "a multi-instance location must reach the evaluator"
        );

        // a GeometryCollection and a non-GeoJSON value are the same case
        for v in [
            json!({"type": "GeometryCollection", "geometries": []}),
            json!("somewhere"),
            json!({"coordinates": [1, 2]}),
        ] {
            let e = extract(&json!({"id": "urn:x", loc: [{"value": v}]}));
            assert!(e.location.is_none(), "{v} must not become a geometry");
            assert!(e.location_ambiguous, "{v} must reach the evaluator");
        }

        // NO geoproperty: not ambiguous — the row is excluded in SQL
        let e = extract(&json!({"id": "urn:x", "https://a/speed": [{"value": 1}]}));
        assert!(e.location.is_none());
        assert!(
            !e.location_ambiguous,
            "a row without a location must stay excludable in SQL"
        );
    }

    /// F-R2ANT4-008: a sub-Attribute's `expiresAt` counts for
    /// `attrs_expire_at`, so the sweep and LIVE_ENTITY reach the row.
    #[test]
    fn extract_reads_the_earliest_expiry_at_any_depth() {
        let e = extract(&json!({"id": "urn:x", "https://a/attr": [{"value": 1,
            "expiresAt": "2030-01-01T00:00:00Z",
            "https://a/sub": [{"value": 3, "expiresAt": "2029-01-01T00:00:00Z"}]}]}));
        assert_eq!(e.attrs_expire.as_deref(), Some("2029-01-01T00:00:00Z"));
    }

    /// The rest of `extract`: `type`/`scope` accept the string form and the
    /// array form, an absent `scope` is SQL NULL (never an empty array, which
    /// would mean "scoped to nothing"), and a missing system timestamp falls
    /// back rather than failing the write.
    #[test]
    fn extract_reads_types_scopes_and_stamps() {
        let e = extract(&json!({"id": "urn:x", "type": "T", "scope": "/a"}));
        assert_eq!(e.types, ["T"]);
        assert_eq!(e.scopes.as_deref(), Some(&["/a".to_owned()][..]));
        let e = extract(&json!({"id": "urn:x", "type": ["T", "U"], "scope": ["/a", "/b"]}));
        assert_eq!(e.types, ["T", "U"]);
        assert_eq!(e.scopes.expect("scopes").len(), 2);
        let e = extract(&json!({"id": "urn:x"}));
        assert!(e.types.is_empty());
        assert!(e.scopes.is_none(), "absent scope must be SQL NULL");
        assert!(e.expires.is_none());
        assert_eq!(e.created, "1970-01-01T00:00:00Z");
        // a non-string, non-array scope is present but names nothing
        let e = extract(&json!({"id": "urn:x", "scope": 7}));
        assert_eq!(e.scopes.as_deref(), Some(&[][..]));
    }

    /// The outbox row IS the wire contract the drain deserializes, and the
    /// drain DELETES any row it cannot decode — a renamed key loses the event
    /// silently. Pin the key set and the operation vocabulary here, where the
    /// producer lives.
    #[test]
    fn the_change_event_carries_exactly_the_wire_keys() {
        let t = TenantId::default();
        let doc = json!({"id": "urn:e", "https://a/speed": [{"value": 1}]});
        let ev = change_event(
            &t,
            "create",
            "urn:e",
            &["T".to_owned()],
            None,
            Some(&doc),
            1,
            "inc",
        );
        let keys: Vec<&str> = ev
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "changed_attrs",
                "entity_id",
                "incarnation",
                "op",
                "payload",
                "prev_payload",
                "tenant",
                "types",
                "version",
            ]
        );
        assert_eq!(ev["op"], "create");
        assert_eq!(ev["tenant"], t.as_str());
        assert!(ev["prev_payload"].is_null(), "a create has no before-image");
        for op in ["create", "update", "delete"] {
            assert_eq!(
                change_event(&t, op, "urn:e", &[], None, None, 1, "inc")["op"],
                op
            );
        }
    }

    /// `changed_attrs` names the top-level ATTRIBUTES that differ. A create
    /// lists every attribute, a delete lists every prior one — and, the half
    /// that actually bounds notification traffic, an attribute present in
    /// both images with an EQUAL value is not listed, nor is any system
    /// member that changed on every write.
    #[test]
    fn changed_attrs_lists_the_differences_and_nothing_else() {
        let t = TenantId::default();
        let ev = |prev: Option<&Value>, next: Option<&Value>| {
            change_event(&t, "update", "urn:e", &[], prev, next, 1, "inc")["changed_attrs"].clone()
        };
        let a = json!({"id": "urn:e", "type": ["T"], "createdAt": "t0", "modifiedAt": "t0",
                       "https://a/x": [{"value": 1}], "https://a/y": [{"value": 2}]});
        let b = json!({"id": "urn:e", "type": ["T"], "createdAt": "t0", "modifiedAt": "t9",
                       "https://a/x": [{"value": 1}], "https://a/y": [{"value": 3}]});
        assert_eq!(ev(Some(&a), Some(&b)), json!(["https://a/y"]));
        assert_eq!(ev(None, Some(&a)), json!(["https://a/x", "https://a/y"]));
        assert_eq!(ev(Some(&a), None), json!(["https://a/x", "https://a/y"]));
        // identical images change nothing at all
        assert_eq!(ev(Some(&a), Some(&a)), json!([]));
        // and no system member is ever named
        let ids = json!({"id": "urn:other", "type": ["U"], "scope": "/s",
                         "modifiedAt": "t1", "deletedAt": "t1", "expiresAt": "t1"});
        assert_eq!(
            ev(Some(&a), Some(&ids)),
            json!(["https://a/x", "https://a/y"])
        );
    }

    fn page(offset: i64, limit: i64) -> Page {
        Page {
            offset,
            limit,
            count: true,
        }
    }

    /// 5.5.9.1: a query SQL decided exactly keeps the pushed page — LIMIT
    /// and OFFSET. count=true carries no window either (T-4781): the window
    /// kept every match, body and all, before the LIMIT could cut.
    #[test]
    fn a_decided_page_pushes_limit_and_offset() {
        let mut binds = vec![Bind::Text("t".to_owned())];
        let (sql, paged) = query_sql(
            "entity",
            &wheres(),
            None,
            Some(&page(20, 10)),
            true,
            "id",
            &mut binds,
        );
        assert!(paged);
        assert_eq!(
            sql,
            "SELECT entity AS entity FROM entities WHERE tenant_id = $1 AND id IN (\
             SELECT id FROM entities WHERE tenant_id = $1 ORDER BY id LIMIT $2 OFFSET $3) ORDER BY id"
        );
        assert!(!sql.contains("OVER"), "{sql}");
        assert!(matches!(binds[1], Bind::Int(11)));
        assert!(matches!(binds[2], Bind::Int(20)));
        // the first page reads its documents directly
        let mut binds = vec![Bind::Text("t".to_owned())];
        let (sql, _) = query_sql(
            "entity",
            &wheres(),
            None,
            Some(&page(0, 10)),
            true,
            "id",
            &mut binds,
        );
        assert!(
            sql.starts_with("SELECT entity AS entity FROM entities WHERE tenant_id"),
            "{sql}"
        );
        assert!(sql.ends_with("ORDER BY id LIMIT $2 OFFSET $3"), "{sql}");
    }

    /// T-4781: one named type reads two leakproof index arms, each in id
    /// order and bounded by what the page can need, never `types @>` alone
    /// (which RLS keeps out of every index condition).
    /// T-5661: id prefixes are one bounding byte range ANDed with each
    /// prefix's own, and a page over them orders by bytes, the C-collated
    /// index's order; no prefix selects nothing.
    #[test]
    fn id_prefixes_are_byte_ranges_in_byte_order() {
        let mut binds = vec![Bind::Text("t".to_owned())];
        let ps = ["b:".to_owned(), "a:".to_owned(), "z\0".to_owned()];
        let w = prefix_ranges(&ps, &mut binds);
        let c = "id COLLATE \"C\"";
        assert_eq!(
            w,
            format!(
                "({c} >= $2 AND {c} < $3 AND (({c} >= $4 AND {c} < $5) OR ({c} >= $6 AND {c} < $7)))"
            )
        );
        let texts: Vec<&str> = binds
            .iter()
            .filter_map(|b| match b {
                Bind::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, ["t", "a:", "b;", "a:", "a;", "b:", "b;"]);
        assert_eq!(prefix_ranges(&[], &mut binds), "false");
        assert_eq!(prefix_ranges(&["\0".to_owned()], &mut binds), "false");
        let mut binds = vec![Bind::Text("t".to_owned())];
        let (sql, _) = query_sql(
            "entity",
            &["tenant_id = $1".to_owned()],
            None,
            Some(&page(0, 10)),
            true,
            c,
            &mut binds,
        );
        assert_eq!(
            sql,
            format!(
                "SELECT entity AS entity FROM entities WHERE tenant_id = $1 ORDER BY {c} LIMIT $2 OFFSET $3"
            )
        );
    }

    #[test]
    fn one_type_pages_through_two_bounded_arms() {
        let mut binds = vec![Bind::Text("t".to_owned())];
        let split = type_selection(&[vec!["T".to_owned()]], &mut binds).expect("split");
        assert!(matches!(&binds[2], Bind::Text(t) if t == "T"));
        let (sql, paged) = query_sql(
            "entity",
            &wheres(),
            Some(&split),
            Some(&page(20, 10)),
            true,
            "id",
            &mut binds,
        );
        assert!(paged);
        assert_eq!(
            sql,
            "SELECT entity AS entity FROM entities WHERE tenant_id = $1 AND id IN (SELECT id FROM (\
             (SELECT id FROM entities WHERE tenant_id = $1 AND types[1] = $3 \
             ORDER BY id LIMIT $7) UNION ALL \
             (SELECT id FROM entities WHERE tenant_id = $1 AND cardinality(types) > 1 \
             AND types[1] <> ALL($4) AND (types @> $2) ORDER BY id LIMIT $7)) AS arms \
             ORDER BY id LIMIT $5 OFFSET $6) ORDER BY id"
        );
        assert!(matches!(binds[4], Bind::Int(11)));
        assert!(matches!(binds[5], Bind::Int(20)));
        assert!(
            matches!(binds[6], Bind::Int(31)),
            "each arm: offset + limit + 1"
        );
        // the undecided ceiling bounds each arm and the merge alike
        let mut binds = vec![Bind::Text("t".to_owned())];
        let split = type_selection(&[vec!["T".to_owned()]], &mut binds).expect("split");
        let (sql, paged) = query_sql(
            "entity",
            &wheres(),
            Some(&split),
            None,
            false,
            "id",
            &mut binds,
        );
        assert!(!paged);
        assert_eq!(sql.matches("LIMIT $5").count(), 3, "{sql}");
        assert!(!sql.contains("OFFSET"), "{sql}");
    }

    /// T-4852: `type=A,B` and `type=A;B` read one first-type arm per named
    /// type, each filtered by the whole selection unless one type decides
    /// it, plus the multi-type arm; a selection the arms cannot name keeps
    /// the plain containment.
    #[test]
    fn a_type_list_reads_one_arm_per_type() {
        let t = |s: &str| s.to_owned();
        let mut binds = vec![Bind::Text(t("t"))];
        let split = type_selection(&[vec![t("B")], vec![t("A")]], &mut binds).expect("split");
        let w = arms(&[t("tenant_id = $1")], Some(&split));
        assert_eq!(
            w,
            [
                "tenant_id = $1 AND types[1] = $4 AND (types @> $2 OR types @> $3)",
                "tenant_id = $1 AND types[1] = $5 AND (types @> $2 OR types @> $3)",
                "tenant_id = $1 AND cardinality(types) > 1 AND types[1] <> ALL($6) \
                 AND (types @> $2 OR types @> $3)",
            ]
        );
        assert!(matches!(&binds[3], Bind::Text(x) if x == "A"), "sorted");
        assert!(matches!(&binds[5], Bind::TextArr(v) if v == &[t("A"), t("B")]));

        let mut binds = vec![Bind::Text(t("t"))];
        let split = type_selection(&[vec![t("A"), t("B")]], &mut binds).expect("split");
        assert!(!split.exact_first, "A;B is not decided by the first type");

        let mut binds = vec![Bind::Text(t("t"))];
        let many: Vec<Vec<String>> = (0..=MAX_TYPE_ARMS).map(|i| vec![format!("T{i}")]).collect();
        let w = type_selection(&many, &mut binds)
            .err()
            .expect("too many arms");
        assert!(w.starts_with("(types @> $2 OR"), "{w}");
        let mut binds = vec![Bind::Text(t("t"))];
        assert!(type_selection(&[vec![t("A")], vec![]], &mut binds).is_err());
        let mut binds = vec![Bind::Text(t("t"))];
        assert_eq!(
            type_selection(&[vec![t("A\0")]], &mut binds)
                .err()
                .as_deref(),
            Some("false")
        );
    }

    /// T-5498: a match of `A;B` has two types, so the multi-type arm alone
    /// reads it; a first-type arm walked every Entity of A and kept none.
    /// A one-type group beside it (`A,B;C`) keeps its first-type arm, and
    /// `A;A` is the one type A.
    #[test]
    fn an_and_group_reads_the_multi_type_arm_only() {
        let t = |s: &str| s.to_owned();
        let mut binds = vec![Bind::Text(t("t"))];
        let split = type_selection(&[vec![t("A"), t("B")]], &mut binds).expect("split");
        assert_eq!(
            arms(&[t("tenant_id = $1")], Some(&split)),
            ["tenant_id = $1 AND cardinality(types) > 1 AND (types @> $2)"]
        );
        assert_eq!(binds.len(), 2, "no first-type binds");

        let mut binds = vec![Bind::Text(t("t"))];
        let split =
            type_selection(&[vec![t("A")], vec![t("B"), t("C")]], &mut binds).expect("split");
        assert_eq!(
            arms(&[t("tenant_id = $1")], Some(&split)),
            [
                "tenant_id = $1 AND types[1] = $4 AND (types @> $2 OR types @> $3)",
                "tenant_id = $1 AND cardinality(types) > 1 AND types[1] <> ALL($5) \
                 AND (types @> $2 OR types @> $3)",
            ]
        );
        assert!(matches!(&binds[3], Bind::Text(x) if x == "A"));
        assert!(matches!(&binds[4], Bind::TextArr(v) if v == &[t("A")]));

        let mut binds = vec![Bind::Text(t("t"))];
        let split = type_selection(&[vec![t("A"), t("A")]], &mut binds).expect("split");
        assert_eq!(
            arms(&[t("tenant_id = $1")], Some(&split)).len(),
            2,
            "A;A is A"
        );
    }

    /// count=false never asks for a count; count=true asks only when the
    /// page cannot show the total: more rows past it, or an off-the-end page.
    #[test]
    fn only_a_counted_page_that_cannot_show_its_total_asks_for_a_count() {
        let p = |offset, count| Page {
            offset,
            limit: 10,
            count,
        };
        for (offset, rows, more) in [(0, 10, true), (20, 0, false), (20, 3, false), (0, 0, false)] {
            assert!(page_total(&p(offset, false), rows, more).is_some());
        }
        assert_eq!(page_total(&p(20, false), 10, true), Some(31));
        assert_eq!(page_total(&p(0, true), 10, true), None);
        assert_eq!(page_total(&p(20, true), 0, false), None, "off the end");
        assert_eq!(page_total(&p(20, true), 3, false), Some(23), "last page");
        assert_eq!(
            page_total(&p(0, true), 0, false),
            Some(0),
            "nothing matches"
        );
    }

    /// The count runs over exactly the page's arms: the same WHERE and the
    /// same binds, a count per arm, no window.
    #[test]
    fn the_count_reads_the_same_arms_as_the_page() {
        let w = vec!["tenant_id = $1".to_owned(), "entity ?| $4".to_owned()];
        assert_eq!(
            count_sql(&w, None),
            "SELECT (SELECT count(*) FROM entities WHERE tenant_id = $1 AND entity ?| $4)"
        );
        let split = TypeSplit {
            firsts: vec![2],
            named: Some(3),
            selection: "(types @> $5)".to_owned(),
            exact_first: true,
        };
        let sql = count_sql(&w, Some(&split));
        assert_eq!(
            sql,
            "SELECT (SELECT count(*) FROM entities WHERE tenant_id = $1 AND entity ?| $4 \
             AND types[1] = $2) + (SELECT count(*) FROM entities WHERE tenant_id = $1 \
             AND entity ?| $4 AND cardinality(types) > 1 AND types[1] <> ALL($3) \
             AND (types @> $5))"
        );
    }

    /// T-6066: a count the broker decides reads the ids of the same arms,
    /// one id per row, never a document.
    #[test]
    fn a_counted_id_walk_reads_the_ids_of_the_counts_arms() {
        let w = vec!["tenant_id = $1".to_owned(), "true".to_owned()];
        assert_eq!(
            ids_sql(&w, None),
            "SELECT id FROM entities WHERE tenant_id = $1 AND true"
        );
        let split = TypeSplit {
            firsts: vec![2],
            named: Some(3),
            selection: "(types @> $5)".to_owned(),
            exact_first: true,
        };
        assert_eq!(
            ids_sql(&w, Some(&split)),
            "SELECT id FROM entities WHERE tenant_id = $1 AND true AND types[1] = $2 \
             UNION ALL SELECT id FROM entities WHERE tenant_id = $1 AND true \
             AND cardinality(types) > 1 AND types[1] <> ALL($3) AND (types @> $5)"
        );
    }

    /// 6.3.10: without count=true no total is owed, so the statement carries
    /// no window count (which forced a walk of the whole match set) and
    /// fetches one row past the page to learn whether a next page exists.
    #[test]
    fn an_uncounted_page_fetches_one_extra_row_and_no_window_total() {
        let mut binds = vec![Bind::Text("t".to_owned())];
        let p = Page {
            offset: 20,
            limit: 10,
            count: false,
        };
        let (sql, paged) = query_sql("entity", &wheres(), None, Some(&p), true, "id", &mut binds);
        assert!(paged);
        assert!(!sql.contains("count(*) OVER ()"), "{sql}");
        assert!(sql.contains("LIMIT $2 OFFSET $3"), "{sql}");
        assert!(matches!(binds[1], Bind::Int(11)), "limit + 1 must be bound");
        assert!(matches!(binds[2], Bind::Int(20)));
    }

    /// The largest page a caller can build still has to produce a usable
    /// LIMIT. `types_attrs.rs` clamps its discovery scan to `i64::MAX` on
    /// purpose so the cast is safe; the uncounted path then binds
    /// `limit + 1`, which is where that clamp stops being enough. The
    /// addition overflows: a panic in a debug build, and in release a wrap
    /// to `i64::MIN`, which Postgres refuses as a negative LIMIT.
    #[test]
    fn the_largest_page_still_binds_a_usable_limit() {
        let mut binds = vec![Bind::Text("t".to_owned())];
        let p = Page {
            offset: 0,
            limit: i64::MAX,
            count: false,
        };
        let (sql, paged) = query_sql("entity", &wheres(), None, Some(&p), true, "id", &mut binds);
        assert!(paged);
        assert!(sql.contains("LIMIT $2"), "{sql}");
        assert!(
            matches!(binds[1], Bind::Int(i64::MAX)),
            "the bound limit must stay positive"
        );
    }

    /// The undecided path must never leave the statement unbounded: SQL only
    /// narrowed the set (`scopeQ`, `georel`, a `q=` the compiler declined),
    /// so the rows come back to be filtered — under a LIMIT, and without an
    /// OFFSET, which would page over the wrong set.
    #[test]
    fn an_undecided_query_carries_a_safety_limit_and_no_offset() {
        let mut binds = vec![Bind::Text("t".to_owned())];
        let (sql, paged) = query_sql(
            "entity",
            &wheres(),
            None,
            Some(&page(20, 10)),
            false,
            "id",
            &mut binds,
        );
        assert!(!paged);
        assert!(sql.contains("LIMIT $2"), "{sql}");
        assert!(!sql.contains("OFFSET"), "offset cannot be pushed: {sql}");
        assert!(!sql.contains("count(*) OVER ()"), "{sql}");
        // the candidate ceiling, not the page: the caller's evaluator still
        // filters these rows, so a page-sized fetch would refuse legitimate
        // queries whenever candidates outnumber matches
        assert!(matches!(binds[1], Bind::Int(MAX_UNDECIDED_ROWS)));
    }

    /// A caller with no page (idPattern, federation, orderBy) hands the store
    /// no bound at all — the ceiling is then the only one.
    #[test]
    fn a_query_without_a_page_falls_back_to_the_ceiling() {
        let mut binds = vec![Bind::Text("t".to_owned())];
        let (sql, paged) = query_sql("entity", &wheres(), None, None, false, "id", &mut binds);
        assert!(!paged);
        assert!(sql.contains("LIMIT $2"), "{sql}");
        assert!(matches!(binds[1], Bind::Int(MAX_UNDECIDED_ROWS)));
        // an exactly decided query with no page is materialized whole too
        let mut binds = vec![Bind::Text("t".to_owned())];
        let (sql, _) = query_sql("entity", &wheres(), None, None, true, "id", &mut binds);
        assert!(sql.contains("LIMIT $2"), "{sql}");
    }

    /// 5.5.6 / Table 6.3.2-1: reaching the ceiling means the statement was
    /// truncated, so the page built from it would silently under-report —
    /// the operation is refused with TooManyResults (403) instead.
    #[test]
    fn a_ceiling_hit_is_too_many_results_not_a_short_page() {
        assert!(check_ceiling(false, 30, 31).is_ok(), "under the ceiling");
        let err = check_ceiling(false, 31, 31).expect_err("ceiling reached");
        let ngsi = ngsi_error(&err).expect("the spec error travels in the driver error");
        assert_eq!(ngsi.kind(), "TooManyResults");
        assert_eq!(ngsi.status(), 403);
        // a pushed page is exact by construction: a full page is a page, not
        // a truncation
        assert!(check_ceiling(true, 1000, 1000).is_ok());
    }

    /// A real driver failure must stay a driver failure — only the store's
    /// own spec errors are recovered, or every 500 would become a 403.
    #[test]
    fn a_driver_error_is_not_mistaken_for_a_spec_error() {
        assert!(ngsi_error(&sqlx::Error::RowNotFound).is_none());
        assert!(ngsi_error(&sqlx::Error::PoolTimedOut).is_none());
        assert!(ngsi_error(&sqlx::Error::Configuration("boom".into())).is_none());
    }
}
