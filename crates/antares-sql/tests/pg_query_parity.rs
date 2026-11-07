// SPDX-License-Identifier: EUPL-1.2
//! T-3462: the Postgres pushdown of `q`, `scopeQ` and `geoQ` against the
//! in-memory evaluator, on generated entities and generated queries.
//!
//! The store answers `decided` when every predicate it was given compiled
//! exactly; the caller then trusts the rows as they are (no re-check), so a
//! decided answer must hold exactly the entities `eval_q` matches. When it
//! is not decided the caller re-filters, so the rows must hold at least
//! every entity the evaluator keeps (a narrowing never drops a match).
//!
//! The property also covers "an attribute the entity lacks never matches a
//! comparison" (TS 104 175 7.2.3.2: a MemberExpression without a defined
//! value is non-existent), which hidden attributes of an endpoint view rely
//! on.
//!
//! Needs a live PostGIS like the other `pg*` tests; skips loudly without
//! `ANTARES_TEST_DATABASE_URL`. `PROPTEST_CASES` sets the case count (256
//! by default, 10 000 in strict.yml).

// Every test here talks to the Postgres store, which only exists with the
// `postgres` feature; without it the file builds to nothing (T-3465).
#![cfg(feature = "postgres")]

use std::collections::{BTreeSet, HashMap};
use std::sync::OnceLock;

use antares_jsonld::{core_context, expand_entity, Context, ExpandOpts};
use antares_model::TenantId;
use antares_ql::geo::GeoQuery;
use antares_ql::{eval::eval_q, parse_q, scope::scope_matches};
use antares_sql::store::pg;
use antares_sql::store::pg::entity::{EntityFilter, Page, PgEntityStore};
use proptest::prelude::*;
use serde_json::{json, Map, Value};

const ATTRS: &[&str] = &["speed", "name", "flag", "tags", "color", "refA", "count"];
const WORDS: &[&str] = &["a", "b", "Berlin", "berlin", "é", "x y", "10", ""];
/// Stored strings: the query words plus line breaks, where `.`, `$` and
/// negated lists of the two regex dialects part (T-4907).
const VALUES: &[&str] = &[
    "a", "b", "Berlin", "berlin", "é", "x y", "10", "", "Ber\nlin", "b\n",
];
/// `~=` operands: the first 16 are the pushed subset, the rest stay in memory.
const PATTERNS: &[&str] = &[
    "^B",
    "erl",
    "^b.*n$",
    "^.$",
    "B.r",
    "(a|b)+",
    "[A-Z]erlin",
    "^$",
    "x y",
    "é",
    "1+0?",
    "^[a-c]+$",
    "n$",
    "a|",
    "r.l",
    "^b.$",
    "\\d",
    "[^a]",
    "e{1,2}",
    "(?i)^b",
    "^B[^x]*n$",
];

struct Db {
    runtime: tokio::runtime::Runtime,
    store: PgEntityStore,
    pool: sqlx::PgPool,
}

/// One pool and one runtime for every case of the run; each property seeds
/// a tenant of its own, since the harness runs the properties in parallel.
fn db() -> Option<&'static Db> {
    static DB: OnceLock<Option<Db>> = OnceLock::new();
    DB.get_or_init(|| {
        let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
            eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
            return None;
        };
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let pool = runtime
            .block_on(pg::connect(&url, 8))
            .expect("connect and migrate");
        for name in [
            "pgparityq",
            "pgparityexact",
            "pgparityscope",
            "pgparitygeo",
            "pgparitygeo2",
            "pgparitygeowalk",
            "pgparityattrs",
        ] {
            let tenant = TenantId::new(name).expect("tenant");
            runtime
                .block_on(pg::ensure_tenant(&pool, &tenant))
                .expect("tenant row");
        }
        Some(Db {
            runtime,
            store: PgEntityStore::new(pool.clone()),
            pool,
        })
    })
    .as_ref()
}

fn ctx() -> &'static Context {
    static CTX: OnceLock<Context> = OnceLock::new();
    CTX.get_or_init(core_context)
}

// ---------------------------------------------------------------- entities

fn scalar() -> impl Strategy<Value = Value> {
    prop_oneof![
        (-3i64..=3).prop_map(Value::from),
        prop::sample::select(vec![1.5f64, -0.5, 2.25]).prop_map(Value::from),
        prop::sample::select(VALUES.to_vec()).prop_map(Value::from),
        any::<bool>().prop_map(Value::from),
    ]
}

/// One attribute in the compact form a client writes, in every shape the
/// query language treats differently.
fn attribute() -> impl Strategy<Value = Value> {
    prop_oneof![
        3 => scalar().prop_map(|v| json!({"type": "Property", "value": v})),
        1 => prop::collection::vec(scalar(), 1..4)
            .prop_map(|v| json!({"type": "Property", "value": v})),
        1 => prop::sample::select(vec!["urn:ngsi-ld:T:1", "urn:ngsi-ld:T:2"])
            .prop_map(|o| json!({"type": "Relationship", "object": o})),
        1 => (prop::sample::select(WORDS.to_vec()), prop::sample::select(WORDS.to_vec()))
            .prop_map(|(en, fi)| json!({"type": "LanguageProperty", "languageMap": {"en": en, "fi": fi}})),
        1 => (scalar(), scalar()).prop_map(|(a, b)| json!([
            {"type": "Property", "value": a, "datasetId": "urn:ngsi-ld:Dataset:1"},
            {"type": "Property", "value": b, "datasetId": "urn:ngsi-ld:Dataset:2"}
        ])),
        1 => (prop::sample::select(WORDS.to_vec()), -3i64..=3)
            .prop_map(|(city, n)| json!({"type": "Property", "value": {"city": city, "n": n}})),
        // T-5303: the shapes `entity_values` (0024) keys at depth, or marks
        // 'NaN' (more than 32 scalars, a string past 1 024 bytes)
        1 => (scalar(), scalar())
            .prop_map(|(a, b)| json!({"type": "Property", "value": [[a], [b, 1]]})),
        1 => (-3i64..=3)
            .prop_map(|n| json!({"type": "Property", "value": (n..n + 33).collect::<Vec<_>>()})),
        1 => prop::sample::select(VALUES.to_vec())
            .prop_map(|v| json!({"type": "Property", "value": [v, "\u{e9}".repeat(600)]})),
    ]
}

fn point() -> impl Strategy<Value = Value> {
    (0i32..=4, 0i32..=4)
        .prop_map(|(x, y)| json!({"type": "Point", "coordinates": [f64::from(x), f64::from(y)]}))
}

/// Compact entities; an attribute missing from the map is absent.
fn entities() -> impl Strategy<Value = Vec<Value>> {
    prop::collection::vec(
        (
            prop::collection::btree_map(prop::sample::select(ATTRS.to_vec()), attribute(), 0..5),
            prop::option::of(point()),
            prop::collection::btree_set(
                prop::sample::select(vec!["/a", "/a/b", "/a/b/c", "/c"]),
                0..3,
            ),
        ),
        1..7,
    )
    .prop_map(|items| {
        items
            .into_iter()
            .enumerate()
            .map(|(i, (attrs, location, scopes))| {
                let mut doc = Map::new();
                doc.insert("id".into(), json!(format!("urn:ngsi-ld:T:p{i}")));
                doc.insert("type".into(), json!("T"));
                for (name, attr) in attrs {
                    doc.insert(name.into(), attr);
                }
                if let Some(geometry) = location {
                    doc.insert(
                        "location".into(),
                        json!({"type": "GeoProperty", "value": geometry}),
                    );
                }
                if !scopes.is_empty() {
                    doc.insert(
                        "scope".into(),
                        json!(scopes.into_iter().collect::<Vec<_>>()),
                    );
                }
                Value::Object(doc)
            })
            .collect()
    })
}

/// Entities of one to three distinct types out of A, B, C, in any order, so
/// the first type varies: `p0` may be `[B, A]`, `p1` `[A]`.
fn typed() -> impl Strategy<Value = Vec<Vec<&'static str>>> {
    let one = prop::sample::subsequence(vec!["A", "B", "C"], 1..=3).prop_shuffle();
    prop::collection::vec(one, 1..9)
}

/// A type selection (4.17): one to three OR-ed groups, each an AND of one or
/// two types (`A`, `A;B`, `A,B;C`), a repeated type (`A;A`) included.
fn type_groups() -> impl Strategy<Value = Vec<Vec<&'static str>>> {
    let ty = prop::sample::select(vec!["A", "B", "C"]);
    prop::collection::vec(prop::collection::vec(ty, 1..=2), 1..=3)
}

// ----------------------------------------------------------------- queries

/// A single value: the operand every operator takes.
fn q_scalar() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => (-3i64..=3).prop_map(|n| n.to_string()),
        1 => Just("1.5".to_owned()),
        3 => prop::sample::select(WORDS.to_vec()).prop_map(|w| format!("\"{w}\"")),
        1 => prop::sample::select(vec!["true", "false"]).prop_map(str::to_owned),
        1 => Just("\"urn:ngsi-ld:T:1\"".to_owned()),
    ]
}

/// A value list or a range, which only `==` and `!=` take.
fn q_compound() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("1,2".to_owned()),
        Just("\"a\",\"Berlin\"".to_owned()),
        Just("-1..2".to_owned()),
        Just("true,3".to_owned()),
    ]
}

/// The plain attribute is the shape the compiler pushes down; the others
/// (a trailing path, a language, a sub-attribute) stay with the evaluator.
fn q_target() -> impl Strategy<Value = String> {
    let attr = prop::sample::select(ATTRS.to_vec());
    prop_oneof![
        8 => attr.clone().prop_map(str::to_owned),
        1 => attr.clone().prop_map(|a| format!("{a}[city]")),
        1 => attr.clone().prop_map(|a| format!("{a}[en]")),
        1 => attr.prop_map(|a| format!("{a}.observedAt")),
    ]
}

fn q_leaf() -> impl Strategy<Value = String> {
    prop_oneof![
        1 => q_target(),
        1 => q_target().prop_map(|t| format!("!{t}")),
        6 => (q_target(), prop::sample::select(vec!["==", "==", "!=", ">", ">=", "<", "<="]), q_scalar())
            .prop_map(|(t, op, v)| format!("{t}{op}{v}")),
        2 => (q_target(), prop::sample::select(vec!["==", "==", "!="]), q_compound())
            .prop_map(|(t, op, v)| format!("{t}{op}{v}")),
        2 => (q_target(), prop::sample::select(PATTERNS.to_vec())).prop_map(|(t, p)| format!("{t}~=\"{p}\"")),
        1 => (q_target(), prop::sample::select(PATTERNS.to_vec())).prop_map(|(t, p)| format!("{t}!~=\"{p}\"")),
    ]
}

/// Only the shapes `antares_ql::sql` documents as exact: a plain attribute,
/// existence, `==` and the numeric orderings, value lists and numeric
/// ranges. Every one of them must be decided by SQL.
fn q_compilable() -> impl Strategy<Value = String> {
    let attr = || prop::sample::select(ATTRS.to_vec());
    let number = || {
        prop_oneof![
            (-3i64..=3).prop_map(|n| n.to_string()),
            Just("1.5".to_owned())
        ]
    };
    let leaf = prop_oneof![
        1 => attr().prop_map(str::to_owned),
        1 => attr().prop_map(|a| format!("!{a}")),
        3 => (attr(), prop::sample::select(vec![">", ">=", "<", "<=", "=="]), number())
            .prop_map(|(a, op, v)| format!("{a}{op}{v}")),
        2 => (attr(), prop::sample::select(WORDS.to_vec()))
            .prop_map(|(a, w)| format!("{a}==\"{w}\"")),
        1 => (attr(), prop::sample::select(vec!["true", "false"])).prop_map(|(a, b)| format!("{a}=={b}")),
        2 => (attr(), prop::sample::select(PATTERNS[..16].to_vec())).prop_map(|(a, p)| format!("{a}~=\"{p}\"")),
        1 => (attr(), prop::sample::select(vec!["1,2", "\"a\",\"Berlin\"", "-1..2", "true,3"]))
            .prop_map(|(a, v)| format!("{a}=={v}")),
    ];
    leaf.prop_recursive(2, 8, 3, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 2..4).prop_map(|v| format!("({})", v.join(";"))),
            prop::collection::vec(inner, 2..4).prop_map(|v| format!("({})", v.join("|"))),
        ]
    })
}

fn q_string() -> impl Strategy<Value = String> {
    q_leaf().prop_recursive(2, 8, 3, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 2..4).prop_map(|v| format!("({})", v.join(";"))),
            prop::collection::vec(inner, 2..4).prop_map(|v| format!("({})", v.join("|"))),
        ]
    })
}

fn scope_q() -> impl Strategy<Value = String> {
    prop::sample::select(vec![
        "/a",
        "/a/#",
        "/a/+",
        "/a/b",
        "/c",
        "/a/+/c",
        "/#",
        "/a;/c",
        "/c|/a/b",
        "(/a;/a/b),/c",
    ])
    .prop_map(str::to_owned)
}

fn geo_q() -> impl Strategy<Value = HashMap<String, String>> {
    let polygon = "[[[0.5,0.5],[2.5,0.5],[2.5,2.5],[0.5,2.5],[0.5,0.5]]]";
    prop_oneof![
        (
            0i32..=4,
            0i32..=4,
            prop::sample::select(vec![
                "near;maxDistance==120000",
                "near;minDistance==120000",
                "equals",
                "intersects",
                "disjoint"
            ])
        )
            .prop_map(|(x, y, rel)| (
                rel.to_owned(),
                "Point".to_owned(),
                format!("[{x},{y}]")
            )),
        prop::sample::select(vec!["within", "intersects", "disjoint", "contains"])
            .prop_map(move |rel| (rel.to_owned(), "Polygon".to_owned(), polygon.to_owned())),
    ]
    .prop_map(|(georel, geometry, coordinates)| {
        HashMap::from([
            ("georel".to_owned(), georel),
            ("geometry".to_owned(), geometry),
            ("coordinates".to_owned(), coordinates),
        ])
    })
}

// --------------------------------------------------------------- the check

/// Seeds the tenant with `docs` (nothing else), answering their expanded form.
fn seed(db: &Db, tenant: &TenantId, docs: &[Value]) -> Vec<Value> {
    let expanded: Vec<Value> = docs
        .iter()
        .map(|d| {
            expand_entity(d.as_object().expect("object"), ctx(), ExpandOpts::default())
                .expect("a generated entity expands")
        })
        .collect();
    db.runtime.block_on(async {
        sqlx::query("DELETE FROM entities WHERE tenant_id = $1")
            .bind(tenant.as_str())
            .execute(&db.pool)
            .await
            .expect("clear the tenant");
        for doc in &expanded {
            let id = doc["id"].as_str().expect("id");
            assert!(
                db.store.create(tenant, id, doc).await.expect("create"),
                "seed {id}"
            );
        }
    });
    expanded
}

fn ids(rows: &[Value]) -> BTreeSet<String> {
    rows.iter()
        .filter_map(|r| r["id"].as_str().map(str::to_owned))
        .collect()
}

fn query(db: &Db, tenant: &TenantId, filter: &EntityFilter<'_>) -> (BTreeSet<String>, bool) {
    let outcome = db
        .runtime
        .block_on(db.store.query(tenant, filter))
        .expect("the store answers a query it was given");
    (ids(&outcome.rows), outcome.decided)
}

fn expand(term: &str) -> String {
    ctx().expand_key(term)
}

proptest! {
    #![proptest_config(ProptestConfig { failure_persistence: None, ..ProptestConfig::default() })]

    /// TS 104 175 7.2.3: the pushdown of `q` keeps exactly the evaluator's
    /// entities when it decides, and never fewer when it does not.
    #[test]
    fn q_pushdown_agrees_with_the_evaluator(docs in entities(), q in q_string()) {
        let Some(db) = db() else { return Ok(()) };
        let Ok(ast) = parse_q(&q) else { return Ok(()) };
        let tenant = TenantId::new("pgparityq").expect("tenant");
        let stored = seed(db, &tenant, &docs);
        let want: BTreeSet<String> = stored
            .iter()
            .filter(|doc| eval_q(&ast, doc, ctx(), &|_| None))
            .filter_map(|doc| doc["id"].as_str().map(str::to_owned))
            .collect();
        let (got, decided) = query(db, &tenant, &EntityFilter { q: Some(&ast), expand: &expand, ..Default::default() });
        if decided {
            prop_assert_eq!(&got, &want, "q={} decided by SQL", q);
        } else {
            prop_assert!(got.is_superset(&want), "q={} dropped {:?}", q, want.difference(&got).collect::<Vec<_>>());
        }
    }

    /// The exact subset is decided by SQL, and decided exactly: the rows the
    /// caller then trusts without a re-check are the evaluator's.
    #[test]
    fn a_compilable_q_is_decided_by_sql_and_exact(docs in entities(), q in q_compilable()) {
        let Some(db) = db() else { return Ok(()) };
        let ast = parse_q(&q).map_err(|e| TestCaseError::fail(format!("{q}: {e:?}")))?;
        let tenant = TenantId::new("pgparityexact").expect("tenant");
        let stored = seed(db, &tenant, &docs);
        let want: BTreeSet<String> = stored
            .iter()
            .filter(|doc| eval_q(&ast, doc, ctx(), &|_| None))
            .filter_map(|doc| doc["id"].as_str().map(str::to_owned))
            .collect();
        let (got, decided) = query(db, &tenant, &EntityFilter { q: Some(&ast), expand: &expand, ..Default::default() });
        prop_assert!(decided, "q={} is in the exact subset and was not decided", q);
        prop_assert_eq!(&got, &want, "q={}", q);
    }

    /// `attrs=` keeps exactly the entities carrying one of the names, now
    /// also through `entity_values` (T-5303), and a member of the Entity
    /// itself in the list leaves that narrowing out.
    #[test]
    fn attrs_pushdown_is_decided_and_exact(
        docs in entities(),
        names in prop::collection::btree_set(prop::sample::select(vec!["speed", "name", "tags", "location", "id"]), 1..3),
    ) {
        let Some(db) = db() else { return Ok(()) };
        let tenant = TenantId::new("pgparityattrs").expect("tenant");
        let stored = seed(db, &tenant, &docs);
        let attrs: Vec<String> = names.iter().map(|n| if *n == "id" { "id".to_owned() } else { expand(n) }).collect();
        let want: BTreeSet<String> = stored
            .iter()
            .filter(|doc| attrs.iter().any(|a| doc.get(a).is_some()))
            .filter_map(|doc| doc["id"].as_str().map(str::to_owned))
            .collect();
        let (got, decided) = query(db, &tenant, &EntityFilter { attrs: Some(&attrs), expand: &expand, ..Default::default() });
        prop_assert!(decided, "attrs={:?}", attrs);
        prop_assert_eq!(&got, &want, "attrs={:?}", attrs);
    }

    /// TS 104 175 7.2.5: the `scopeQ` narrowing never drops an entity the
    /// evaluator keeps.
    #[test]
    fn scope_q_pushdown_never_drops_a_match(docs in entities(), sq in scope_q()) {
        let Some(db) = db() else { return Ok(()) };
        let tenant = TenantId::new("pgparityscope").expect("tenant");
        let stored = seed(db, &tenant, &docs);
        let want: BTreeSet<String> = stored
            .iter()
            .filter(|doc| scope_matches(&sq, doc))
            .filter_map(|doc| doc["id"].as_str().map(str::to_owned))
            .collect();
        let (got, _) = query(db, &tenant, &EntityFilter { scope_q: Some(&sq), expand: &expand, ..Default::default() });
        prop_assert!(got.is_superset(&want), "scopeQ={} dropped {:?}", sq, want.difference(&got).collect::<Vec<_>>());
    }

    /// TS 104 175 7.2.4: the geo narrowing never drops an entity the
    /// evaluator keeps.
    #[test]
    fn geo_q_pushdown_never_drops_a_match(docs in entities(), params in geo_q()) {
        let Some(db) = db() else { return Ok(()) };
        let Ok(Some(geo)) = GeoQuery::from_params(&params) else { return Ok(()) };
        let tenant = TenantId::new("pgparitygeo").expect("tenant");
        let stored = seed(db, &tenant, &docs);
        let want: BTreeSet<String> = stored
            .iter()
            .filter(|doc| geo.matches(doc, ctx()))
            .filter_map(|doc| doc["id"].as_str().map(str::to_owned))
            .collect();
        let spec = geo.to_sql_spec(ctx());
        let (got, _) = query(db, &tenant, &EntityFilter { geo: spec.as_slice(), expand: &expand, ..Default::default() });
        prop_assert!(got.is_superset(&want), "geoQ={:?} dropped {:?}", params, want.difference(&got).collect::<Vec<_>>());
    }

    /// T-5088: a geoquery walked through `query_candidates` in pages of 3,
    /// each candidate re-decided by the evaluator as the API does, finds
    /// exactly the evaluator's matches; every page is pushed and none claims
    /// `decided`.
    #[test]
    fn a_geo_candidate_walk_in_pages_finds_every_match(docs in entities(), params in geo_q()) {
        let Some(db) = db() else { return Ok(()) };
        let Ok(Some(geo)) = GeoQuery::from_params(&params) else { return Ok(()) };
        let tenant = TenantId::new("pgparitygeowalk").expect("tenant");
        let stored = seed(db, &tenant, &docs);
        let want: BTreeSet<String> = stored
            .iter()
            .filter(|doc| geo.matches(doc, ctx()))
            .filter_map(|doc| doc["id"].as_str().map(str::to_owned))
            .collect();
        let spec = geo.to_sql_spec(ctx());
        let (mut got, mut offset) = (BTreeSet::new(), 0i64);
        loop {
            let page = Page { offset, limit: 3, count: true };
            let f = EntityFilter { geo: spec.as_slice(), page: Some(page), expand: &expand, ..Default::default() };
            let out = db.runtime.block_on(db.store.query_candidates(&tenant, &f)).expect("candidates");
            prop_assert!(out.paged && !out.decided, "a candidate page is pushed, never decided");
            prop_assert_eq!(out.total, None, "candidates are never counted");
            let n = out.rows.len() as i64;
            got.extend(out.rows.iter().filter(|d| geo.matches(d, ctx())).filter_map(|d| d["id"].as_str().map(str::to_owned)));
            if n < 3 {
                break;
            }
            offset += 3;
        }
        prop_assert_eq!(&got, &want, "geoQ={:?}", params);
    }

    /// ADR-0023 rule 1 (T-3484): two geoqueries in one store query, the
    /// request's and a policy narrowing's, are conjoined. The store keeps
    /// every entity the evaluator keeps under both, and nothing either one
    /// alone would have dropped.
    #[test]
    fn two_geo_qs_are_conjoined_like_the_evaluator(docs in entities(), a in geo_q(), b in geo_q()) {
        let Some(db) = db() else { return Ok(()) };
        let (Ok(Some(ga)), Ok(Some(gb))) = (GeoQuery::from_params(&a), GeoQuery::from_params(&b)) else { return Ok(()) };
        let tenant = TenantId::new("pgparitygeo2").expect("tenant");
        let stored = seed(db, &tenant, &docs);
        let want: BTreeSet<String> = stored
            .iter()
            .filter(|doc| ga.matches(doc, ctx()) && gb.matches(doc, ctx()))
            .filter_map(|doc| doc["id"].as_str().map(str::to_owned))
            .collect();
        let specs: Vec<_> = [&ga, &gb].iter().filter_map(|g| g.to_sql_spec(ctx())).collect();
        prop_assert_eq!(specs.len(), 2, "both default-geoproperty queries compile");
        let (got, decided) = query(db, &tenant, &EntityFilter { geo: &specs, expand: &expand, ..Default::default() });
        prop_assert!(!decided, "geo never claims decided");
        prop_assert!(got.is_superset(&want), "a={:?} b={:?} dropped {:?}", a, b, want.difference(&got).collect::<Vec<_>>());
        let (only_a, _) = query(db, &tenant, &EntityFilter { geo: &specs[..1], expand: &expand, ..Default::default() });
        let (only_b, _) = query(db, &tenant, &EntityFilter { geo: &specs[1..], expand: &expand, ..Default::default() });
        prop_assert!(got.is_subset(&only_a) && got.is_subset(&only_b), "a={:?} b={:?}: the pair kept {:?}, more than one alone", a, b, got);
    }

    /// T-5136: `!location` reads only the Entities with no stored geometry
    /// (`location IS NULL`, a leakproof index path) and stays decided and
    /// exact, also beside a location the store cannot extract (two
    /// instances, `location_ambiguous`) and under another conjunct.
    #[test]
    fn a_missing_location_is_decided_exactly(docs in entities(), rest in prop::option::of(q_compilable())) {
        let Some(db) = db() else { return Ok(()) };
        let q = rest.map_or_else(|| "!location".to_owned(), |r| format!("!location;{r}"));
        let ast = parse_q(&q).map_err(|e| TestCaseError::fail(format!("{q}: {e:?}")))?;
        let mut docs = docs;
        let geometry = json!({"type": "Point", "coordinates": [1.0, 1.0]});
        docs.push(json!({"id": "urn:ngsi-ld:T:twice", "type": "T", "location": [
            {"type": "GeoProperty", "value": geometry, "datasetId": "urn:ngsi-ld:Dataset:1"},
            {"type": "GeoProperty", "value": geometry, "datasetId": "urn:ngsi-ld:Dataset:2"},
        ]}));
        let tenant = TenantId::new("pgparityunlocated").expect("tenant");
        let stored = seed(db, &tenant, &docs);
        let want: BTreeSet<String> = stored
            .iter()
            .filter(|doc| eval_q(&ast, doc, ctx(), &|_| None))
            .filter_map(|doc| doc["id"].as_str().map(str::to_owned))
            .collect();
        let (got, decided) = query(db, &tenant, &EntityFilter { q: Some(&ast), expand: &expand, ..Default::default() });
        prop_assert!(decided, "q={} is in the exact subset and was not decided", q);
        prop_assert_eq!(&got, &want, "q={}", q);
    }

    /// 4.17, T-5498: a type selection with AND-groups is decided by SQL and
    /// exact, unpaged and walked in counted pages, over Entities whose first
    /// type is A and second B as well as the other way round.
    #[test]
    fn a_type_selection_is_decided_and_exact(docs in typed(), groups in type_groups()) {
        let Some(db) = db() else { return Ok(()) };
        let tenant = TenantId::new("pgparitytypes").expect("tenant");
        let items: Vec<Value> = docs
            .iter()
            .enumerate()
            .map(|(i, ts)| json!({"id": format!("urn:ngsi-ld:T:p{i}"), "type": ts}))
            .collect();
        seed(db, &tenant, &items);
        let want: BTreeSet<String> = docs
            .iter()
            .enumerate()
            .filter(|(_, ts)| groups.iter().any(|g| g.iter().all(|t| ts.contains(t))))
            .map(|(i, _)| format!("urn:ngsi-ld:T:p{i}"))
            .collect();
        let expanded: Vec<Vec<String>> = groups.iter().map(|g| g.iter().map(|t| expand(t)).collect()).collect();
        let (got, decided) = query(db, &tenant, &EntityFilter { types: Some(&expanded), expand: &expand, ..Default::default() });
        prop_assert!(decided, "type={:?} was not decided", groups);
        prop_assert_eq!(&got, &want, "type={:?}", groups);
        let (mut paged, mut offset) = (BTreeSet::new(), 0i64);
        loop {
            let page = Page { offset, limit: 2, count: true };
            let f = EntityFilter { types: Some(&expanded), page: Some(page), expand: &expand, ..Default::default() };
            let out = db.runtime.block_on(db.store.query(&tenant, &f)).expect("page");
            prop_assert_eq!(out.total.unwrap_or(want.len() as i64), want.len() as i64, "type={:?} total", groups);
            let n = out.rows.len();
            paged.extend(ids(&out.rows));
            if n < 2 {
                break;
            }
            offset += 2;
        }
        prop_assert_eq!(&paged, &want, "type={:?} paged", groups);
    }
}

/// T-6070: a `near` whose radius reaches past the antimeridian keeps the
/// point on its other side in SQL, as PostGIS measures it; the bounding box
/// `predicate` adds before the geography distance would end at 180.1 and
/// drop it. The evaluator, which decides, measures without the wrap (its
/// documented ceiling, `antares_ql::geo::min_distance_m`) and keeps the east
/// point alone: SQL stays a superset of it.
#[test]
fn a_near_across_the_antimeridian_keeps_the_point_beyond_it() {
    let Some(db) = db() else { return };
    let tenant = TenantId::new("pgparityantimeridian").expect("tenant");
    let docs: Vec<Value> = [("east", 179.995), ("west", -179.995)]
        .iter()
        .map(|(n, x)| {
            json!({"id": format!("urn:ngsi-ld:T:{n}"), "type": "T",
                   "location": {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [x, 0.0]}}})
        })
        .collect();
    let stored = seed(db, &tenant, &docs);
    let params = HashMap::from([
        ("georel".to_owned(), "near;maxDistance==10000".to_owned()),
        ("geometry".to_owned(), "Point".to_owned()),
        ("coordinates".to_owned(), "[179.99,0]".to_owned()),
    ]);
    let geo = GeoQuery::from_params(&params)
        .expect("parses")
        .expect("a geoquery");
    let want: BTreeSet<String> = stored
        .iter()
        .filter(|doc| geo.matches(doc, ctx()))
        .filter_map(|doc| doc["id"].as_str().map(str::to_owned))
        .collect();
    let spec = geo.to_sql_spec(ctx());
    let (got, _) = query(
        db,
        &tenant,
        &EntityFilter {
            geo: spec.as_slice(),
            expand: &expand,
            ..Default::default()
        },
    );
    assert!(
        got.is_superset(&want),
        "dropped {:?}",
        want.difference(&got).collect::<Vec<_>>()
    );
    assert_eq!(
        got,
        BTreeSet::from([
            "urn:ngsi-ld:T:east".to_owned(),
            "urn:ngsi-ld:T:west".to_owned()
        ]),
        "PostGIS keeps both points"
    );
}
