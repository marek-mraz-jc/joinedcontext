// SPDX-License-Identifier: MIT
//! The planner's, the write path's and the definition's refusals, each with the words a caller
//! acts on, over a `MemoryTable` (T-7597).

use std::sync::Arc;

use odata_csdl::{
    ComplexType, CsdlVersion, EntityContainer, EntitySet, EntityType, Model, NavigationProperty,
    Property, Schema, TypeRef,
};
use odata_url::{CompareOp, Expr, Literal, MemberPath, Method, OrderBy, SelectItem};
use serde_json::{json, Map, Value};

use crate::{
    allowed, export, key_options, plan, query, write, Change, Cond, EntityDef, Fetching, Field,
    FieldKind, MemoryTable, Operation, Plan, QueryOptions, Rule, Scope, Service, Table, TableError,
    TableInfo,
};

fn model() -> Arc<Model> {
    let mut thing = EntityType::new("Thing");
    thing.key = vec!["id".into()];
    let mut must = Property::new("must", TypeRef::single("Edm.String"));
    must.nullable = false;
    thing.properties = vec![
        Property::new("id", TypeRef::single("Edm.String")),
        Property::new("org", TypeRef::single("Edm.String")),
        Property::new("size", TypeRef::single("Edm.Int64")),
        Property::new("score", TypeRef::single("Edm.Double")),
        Property::new("active", TypeRef::single("Edm.Boolean")),
        Property::new("address", TypeRef::single("Test.Address")),
        Property::new("tags", TypeRef::collection("Edm.String")),
        Property::new("blob", TypeRef::single("Edm.Binary")),
        must,
    ];
    thing.navigation_properties = vec![NavigationProperty::new(
        "owner",
        TypeRef::single("Test.Thing"),
    )];
    let mut address = ComplexType::new("Address");
    address.properties = vec![Property::new("city", TypeRef::single("Edm.String"))];
    let mut schema = Schema::new("Test");
    schema.entity_types.push(thing);
    schema.complex_types.push(address);
    let mut container = EntityContainer::new("Service");
    container
        .entity_sets
        .push(EntitySet::new("Things", "Test.Thing"));
    schema.entity_container = Some(container);
    Arc::new(Model {
        version: CsdlVersion::V4_01,
        references: vec![],
        schemas: vec![schema],
    })
}

fn info() -> TableInfo {
    let mut info = TableInfo::new(model(), "Things").expect("the fixture names a keyed set");
    info.rules = Operation::ALL
        .into_iter()
        .map(|operation| Rule {
            operation,
            permission: None,
        })
        .collect();
    info
}

fn table(rows: Vec<Value>) -> MemoryTable {
    MemoryTable::new(info(), rows).expect("the fixture rows are keyed")
}

fn things() -> Vec<Value> {
    vec![
        json!({"id": "t1", "org": "a", "size": 1, "must": "x"}),
        json!({"id": "t2", "org": "a", "size": 2, "must": "x"}),
        json!({"id": "t3", "org": "b", "size": 3, "must": "x"}),
    ]
}

fn member(path: &[&str]) -> Expr {
    Expr::Member(MemberPath {
        variable: None,
        path: path.iter().map(|s| (*s).to_owned()).collect(),
    })
}

fn text(s: &str) -> Expr {
    Expr::Literal(Literal::String(s.to_owned()))
}

fn filtered(expr: Expr) -> QueryOptions {
    QueryOptions {
        filter: Some(expr),
        ..QueryOptions::default()
    }
}

fn org_is(org: &str) -> QueryOptions {
    filtered(Expr::Compare(
        Box::new(member(&["org"])),
        CompareOp::Eq,
        Box::new(text(org)),
    ))
}

fn planned(options: &QueryOptions) -> Result<Plan, TableError> {
    plan(&info(), options, 10)
}

fn patch(value: Value) -> Change {
    let Value::Object(map) = value else {
        return Change::Update(Map::new());
    };
    Change::Update(map)
}

async fn update(
    t: &MemoryTable,
    scope: &Scope,
    value: Value,
) -> Result<crate::Written, TableError> {
    write(t, &org_is("a"), scope, patch(value), false, 100).await
}

fn refusal(result: Result<impl std::fmt::Debug, TableError>) -> TableError {
    match result {
        Ok(v) => panic!("expected a refusal, got {v:?}"),
        Err(e) => e,
    }
}

#[test]
fn a_set_without_one_key_property_has_no_table_info() {
    let mut m = (*model()).clone();
    m.schemas[0].entity_types[0].key = vec!["id".into(), "org".into()];
    assert!(TableInfo::new(Arc::new(m.clone()), "Things").is_none());
    m.schemas[0].entity_types[0].key = vec!["missing".into()];
    assert!(TableInfo::new(Arc::new(m), "Things").is_none());
    assert!(TableInfo::new(model(), "Others").is_none());
}

#[test]
fn every_table_error_says_what_went_wrong() {
    let cases = [
        (
            TableError::NotImplemented("$search".into()),
            "$search is not supported by this service",
        ),
        (
            TableError::UnknownProperty("x/y".into()),
            "the entity type has no property x/y",
        ),
        (TableError::BadRequest("bad".into()), "bad"),
        (TableError::Forbidden("no".into()), "no"),
        (TableError::Conflict("moved".into()), "moved"),
        (TableError::Source("down".into()), "the source failed: down"),
    ];
    for (error, said) in cases {
        assert_eq!(error.to_string(), said);
    }
}

/// A source that implements only what a read-only table must.
struct ReadOnly(TableInfo);

impl Table for ReadOnly {
    fn info(&self) -> &TableInfo {
        &self.0
    }
    fn fetch<'a>(&'a self, _plan: &'a Plan, _scope: &'a Scope) -> Fetching<'a> {
        Box::pin(std::future::ready(Ok(crate::Fetched {
            rows: vec![json!({"id": "t1", "org": "a"})],
            count: None,
        })))
    }
}

#[tokio::test]
async fn a_source_by_default_has_no_etag_and_refuses_writes() {
    let source = ReadOnly(info());
    assert_eq!(source.etag(), None);
    assert!(!source.writable());
    let fetched = source
        .fetch(
            &planned(&QueryOptions::default()).expect("a plan"),
            &Scope::all(),
        )
        .await
        .expect("a read");
    assert_eq!(fetched.rows, [json!({"id": "t1", "org": "a"})]);
    let batch = crate::Batch {
        change: Change::Delete,
        keys: vec![json!("t1")],
        guard: Cond::Const(true),
    };
    assert_eq!(
        source.apply(&batch).await,
        Err(TableError::Forbidden("Things is served read only".into()))
    );
    let decision = allowed(&source, &Scope::all(), Operation::Delete, None)
        .await
        .expect("no source call");
    assert!(!decision.allowed);
    assert_eq!(decision.reason, "Things is served read only");
}

#[test]
fn a_key_is_a_string_or_an_integer() {
    let i = info();
    for bad in [json!(true), json!(1.5), json!(null), json!({"a": 1})] {
        assert_eq!(
            key_options(&i, &bad, None),
            Err(TableError::BadRequest(
                "a key is a string or an integer".into()
            )),
            "{bad}"
        );
    }
    assert!(key_options(&i, &json!(7), None).is_ok());
}

#[tokio::test]
async fn an_export_over_its_row_cap_is_refused() {
    let t = table(things());
    let all = filtered(Expr::Literal(Literal::Bool(true)));
    assert_eq!(
        export(&t, &all, &Scope::all(), 1, 2).await,
        Err(TableError::BadRequest(
            "the export is over 2 rows: narrow it with $filter".into()
        ))
    );
    assert_eq!(
        export(&t, &all, &Scope::all(), 1, 3).await.map(|r| r.len()),
        Ok(3)
    );
}

#[test]
fn memory_rows_need_a_unique_string_or_integer_key() {
    let bad = TableError::BadRequest("every row of Things needs a string or integer id".into());
    for row in [
        json!({"org": "a"}),
        json!({"id": 1.5}),
        json!({"id": true}),
        json!("t1"),
    ] {
        assert_eq!(
            MemoryTable::new(info(), vec![row.clone()]).err(),
            Some(bad.clone()),
            "{row}"
        );
    }
    assert_eq!(
        MemoryTable::new(info(), vec![json!({"id": 1}), json!({"id": 1})]).err(),
        Some(TableError::BadRequest(
            "two rows of Things share one id".into()
        ))
    );
    assert!(MemoryTable::new(info(), vec![json!({"id": u64::MAX}), json!({"id": "1"})]).is_ok());
}

#[tokio::test]
async fn two_restrictions_must_both_hold() {
    let t = table(things());
    let scope = Scope::all()
        .and(&["org"], vec![json!("a"), json!("b")])
        .and(&["size"], vec![json!(1), json!(3)]);
    let answer = query(&t, &QueryOptions::default(), &scope, 10)
        .await
        .expect("an open read");
    let ids: Vec<&Value> = answer.rows.iter().filter_map(|r| r.get("id")).collect();
    assert_eq!(ids, [&json!("t1"), &json!("t3")]);
    // a restriction allowing nothing answers nothing
    let none = Scope::all().and(&["org"], vec![]);
    let answer = query(&t, &QueryOptions::default(), &none, 10)
        .await
        .expect("an open read");
    assert!(answer.rows.is_empty());
}

#[test]
fn the_planner_names_each_option_it_does_not_serve() {
    let unsupported = |what: &str| Err::<Plan, _>(TableError::NotImplemented(what.into()));
    let parsed = |q: &str| {
        let uri = odata_url::parse(
            &format!("Things?{q}"),
            &odata_url::EdmModel(&model()),
            &odata_url::Limits::strict(),
        )
        .expect("the parser takes it");
        match uri {
            odata_url::ODataUri::Resource { options, .. } => options,
            _ => QueryOptions::default(),
        }
    };
    assert_eq!(planned(&parsed("$search=blue")), unsupported("$search"));
    assert_eq!(
        planned(&parsed("$apply=filter(size gt 1)")),
        unsupported("$apply")
    );
    assert_eq!(
        planned(&parsed("$compute=size add 1 as more")),
        unsupported("$compute")
    );
    assert_eq!(planned(&parsed("$expand=owner")), unsupported("$expand"));
    let by_expression = QueryOptions {
        orderby: Some(vec![OrderBy {
            expr: Expr::Neg(Box::new(member(&["size"]))),
            descending: false,
        }]),
        ..QueryOptions::default()
    };
    assert_eq!(
        planned(&by_expression),
        unsupported("$orderby on an expression")
    );
}

#[test]
fn select_takes_top_level_properties_once() {
    let select = |items: Vec<SelectItem>| {
        planned(&QueryOptions {
            select: Some(items),
            ..QueryOptions::default()
        })
    };
    let path = |p: &[&str]| SelectItem::Path(p.iter().map(|s| (*s).to_owned()).collect());
    assert_eq!(
        select(vec![SelectItem::Star, path(&["org"])]).map(|p| p.select),
        Ok(None)
    );
    assert_eq!(
        select(vec![path(&["org"]), path(&["org"]), path(&["size"])]).map(|p| p.select),
        Ok(Some(vec!["org".to_owned(), "size".to_owned()]))
    );
    assert_eq!(
        select(vec![path(&["nope"])]).err(),
        Some(TableError::UnknownProperty("nope".into()))
    );
    assert_eq!(
        select(vec![path(&["address", "city"])]).err(),
        Some(TableError::NotImplemented(
            "$select of a nested path".into()
        ))
    );
    assert_eq!(
        select(vec![SelectItem::Annotation("@a.b".into())]).err(),
        Some(TableError::NotImplemented(
            "$select of an annotation".into()
        ))
    );
}

#[test]
fn a_skiptoken_this_service_did_not_write_is_refused() {
    let options = QueryOptions {
        skiptoken: Some("x".into()),
        ..QueryOptions::default()
    };
    assert_eq!(
        planned(&options).err(),
        Some(TableError::BadRequest(
            "the $skiptoken is not one this service wrote".into()
        ))
    );
}

#[test]
fn text_functions_take_a_property_and_a_string() {
    let call = |method: Method, args: Vec<Expr>| planned(&filtered(Expr::Call(method, args)));
    assert_eq!(
        call(
            Method::Contains,
            vec![member(&["org"]), Expr::Literal(Literal::Null)]
        )
        .map(|p| p.filter),
        Ok(Cond::Const(false))
    );
    assert_eq!(
        call(
            Method::EndsWith,
            vec![member(&["org"]), Expr::Literal(Literal::Int(1))]
        )
        .err(),
        Some(TableError::BadRequest("endswith takes a string".into()))
    );
    assert_eq!(
        call(Method::ToLower, vec![member(&["org"])]).err(),
        Some(TableError::NotImplemented("tolower".into()))
    );
    assert_eq!(
        call(Method::Contains, vec![member(&["org"])]).err(),
        Some(TableError::NotImplemented("contains".into()))
    );
    assert_eq!(
        call(
            Method::StartsWith,
            vec![
                Expr::Call(Method::ToLower, vec![member(&["org"])]),
                text("a")
            ]
        )
        .err(),
        Some(TableError::NotImplemented("this function".into()))
    );
    assert_eq!(
        call(
            Method::StartsWith,
            vec![
                Expr::Arith(
                    Box::new(member(&["size"])),
                    odata_url::ArithOp::Add,
                    Box::new(Expr::Literal(Literal::Int(1)))
                ),
                text("a")
            ]
        )
        .err(),
        Some(TableError::NotImplemented("arithmetic operators".into()))
    );
    assert_eq!(
        call(Method::StartsWith, vec![member(&["org"]), member(&["org"])]).err(),
        Some(TableError::NotImplemented("this expression".into()))
    );
}

#[test]
fn comparisons_read_either_side_and_nulls() {
    let compare = |l: Expr, op: CompareOp, r: Expr| {
        planned(&filtered(Expr::Compare(Box::new(l), op, Box::new(r)))).map(|p| p.filter)
    };
    let size = || vec!["size".to_owned()];
    assert_eq!(
        compare(
            Expr::Literal(Literal::Int(2)),
            CompareOp::Lt,
            member(&["size"])
        ),
        Ok(Cond::Compare {
            path: size(),
            op: CompareOp::Gt,
            value: json!(2)
        })
    );
    for (op, flipped) in [
        (CompareOp::Le, CompareOp::Ge),
        (CompareOp::Gt, CompareOp::Lt),
        (CompareOp::Ge, CompareOp::Le),
        (CompareOp::Eq, CompareOp::Eq),
    ] {
        assert_eq!(
            compare(Expr::Literal(Literal::Int(2)), op, member(&["size"])),
            Ok(Cond::Compare {
                path: size(),
                op: flipped,
                value: json!(2)
            })
        );
    }
    assert_eq!(
        compare(
            member(&["size"]),
            CompareOp::Ne,
            Expr::Literal(Literal::Null)
        ),
        Ok(Cond::Not(Box::new(Cond::IsNull { path: size() })))
    );
    assert_eq!(
        compare(
            member(&["size"]),
            CompareOp::Gt,
            Expr::Literal(Literal::Null)
        ),
        Ok(Cond::Const(false))
    );
    assert_eq!(
        compare(
            Expr::Literal(Literal::Int(1)),
            CompareOp::Eq,
            Expr::Literal(Literal::Int(1))
        )
        .err(),
        Some(TableError::NotImplemented(
            "a comparison without a property".into()
        ))
    );
    // a Boolean property alone is `property eq true`
    assert_eq!(
        planned(&filtered(member(&["active"]))).map(|p| p.filter),
        Ok(Cond::Compare {
            path: vec!["active".into()],
            op: CompareOp::Eq,
            value: json!(true)
        })
    );
}

#[test]
fn a_parameter_alias_needs_a_value() {
    let by_alias = |aliases: Vec<(String, Literal)>| {
        planned(&QueryOptions {
            aliases,
            ..filtered(Expr::Compare(
                Box::new(member(&["org"])),
                CompareOp::Eq,
                Box::new(Expr::Alias("@o".into())),
            ))
        })
        .map(|p| p.filter)
    };
    assert_eq!(
        by_alias(vec![]).err(),
        Some(TableError::BadRequest(
            "the parameter alias @o has no value".into()
        ))
    );
    assert_eq!(
        by_alias(vec![("@o".into(), Literal::String("a".into()))]),
        Ok(Cond::Compare {
            path: vec!["org".into()],
            op: CompareOp::Eq,
            value: json!("a")
        })
    );
}

#[test]
fn literals_become_json_values_or_a_refusal() {
    let equal = |literal: Literal| {
        planned(&filtered(Expr::Compare(
            Box::new(member(&["score"])),
            CompareOp::Eq,
            Box::new(Expr::Literal(literal)),
        )))
        .map(|p| p.filter)
    };
    let score = |value: Value| Cond::Compare {
        path: vec!["score".into()],
        op: CompareOp::Eq,
        value,
    };
    assert_eq!(equal(Literal::Decimal("2.5".into())), Ok(score(json!(2.5))));
    assert_eq!(
        equal(Literal::Decimal("1e999".into())).err(),
        Some(TableError::BadRequest(
            "1e999 is not a finite number".into()
        ))
    );
    assert_eq!(
        equal(Literal::Date("2026-01-01".into())),
        Ok(score(json!("2026-01-01")))
    );
    assert_eq!(equal(Literal::Bool(false)), Ok(score(json!(false))));
    assert_eq!(
        equal(Literal::Binary("AQ".into())).err(),
        Some(TableError::NotImplemented("binary literals".into()))
    );
    // `in` reads null as "is null"
    assert_eq!(
        planned(&filtered(Expr::In(
            Box::new(member(&["score"])),
            vec![Literal::Null]
        )))
        .map(|p| p.filter),
        Ok(Cond::Or(
            Box::new(Cond::Const(false)),
            Box::new(Cond::IsNull {
                path: vec!["score".into()]
            })
        ))
    );
}

#[test]
fn a_path_ends_in_a_single_primitive_property() {
    let path_of = |expr: Expr| {
        planned(&filtered(Expr::Compare(
            Box::new(expr),
            CompareOp::Eq,
            Box::new(text("x")),
        )))
        .err()
    };
    let unsupported = |what: &str| Some(TableError::NotImplemented(what.into()));
    assert_eq!(
        path_of(Expr::Member(MemberPath {
            variable: Some("t".into()),
            path: vec!["org".into()]
        })),
        unsupported("lambda variables")
    );
    assert_eq!(
        path_of(member(&["owner"])),
        unsupported("navigation properties")
    );
    assert_eq!(
        path_of(member(&["tags"])),
        unsupported("collection-valued properties")
    );
    assert_eq!(
        path_of(member(&["address"])),
        unsupported("a path ending in a complex property")
    );
    assert_eq!(
        path_of(member(&["address", "zip"])),
        Some(TableError::UnknownProperty("address/zip".into()))
    );
    assert_eq!(path_of(member(&["address", "city"])), None);
    let any = Expr::Any {
        collection: MemberPath {
            variable: None,
            path: vec!["tags".into()],
        },
        lambda: None,
    };
    assert_eq!(
        planned(&filtered(any)).err(),
        unsupported("lambda operators")
    );
}

#[tokio::test]
async fn a_write_takes_a_filter_and_nothing_else() {
    let t = table(things());
    let scope = Scope::all();
    let with_top = QueryOptions {
        top: Some(1),
        ..org_is("a")
    };
    assert_eq!(
        refusal(write(&t, &with_top, &scope, Change::Delete, false, 10).await),
        TableError::BadRequest("a write takes $filter only, e.g. $filter=org eq 'a'".into())
    );
    assert_eq!(
        refusal(
            write(
                &t,
                &QueryOptions::default(),
                &scope,
                Change::Delete,
                false,
                10
            )
            .await
        ),
        TableError::BadRequest("a write to the set needs $filter, e.g. $filter=org eq 'a'".into())
    );
}

#[tokio::test]
async fn an_update_names_known_properties_with_values_of_their_type() {
    let t = table(things());
    let s = Scope::all();
    let bad = |why: &str| TableError::BadRequest(why.into());
    let cases = [
        (
            json!({}),
            bad("an update names at least one property, e.g. {\"name\": \"x\"}"),
        ),
        (
            json!({"id": "t9"}),
            bad("id is the key and cannot be changed"),
        ),
        (
            json!({"nope": 1}),
            TableError::UnknownProperty("nope".into()),
        ),
        (json!({"must": null}), bad("must takes a value, not null")),
        (json!({"tags": "a"}), bad("tags takes a list")),
        (json!({"tags": ["a", 1]}), bad("tags takes text")),
        (json!({"org": 1}), bad("org takes text")),
        (json!({"size": 1.5}), bad("size takes a whole number")),
        (json!({"score": "high"}), bad("score takes a number")),
        (json!({"active": 1}), bad("active takes true or false")),
        (json!({"address": "Zilina"}), bad("address takes an object")),
        (
            json!({"address": {"zip": "1"}}),
            TableError::UnknownProperty("address/zip".into()),
        ),
        (
            json!({"address": {"city": 1}}),
            bad("address/city takes text"),
        ),
        (
            json!({"blob": "AA=="}),
            TableError::NotImplemented("writing Edm.Binary".into()),
        ),
    ];
    for (value, expected) in cases {
        assert_eq!(
            refusal(update(&t, &s, value.clone()).await),
            expected,
            "{value}"
        );
    }
    let written = update(
        &t,
        &s,
        json!({"size": null, "score": 1, "active": true, "address": {"city": "Zilina"}, "tags": ["x"]}),
    )
    .await
    .expect("every value fits its type");
    assert_eq!(written.keys, [json!("t1"), json!("t2")]);
}

#[tokio::test]
async fn an_update_of_a_set_whose_type_left_the_model_is_refused() {
    let mut lost = info();
    lost.entity_type = "Demo.Gone".into();
    let t = MemoryTable::new(lost, things()).expect("the rows are keyed");
    assert_eq!(
        refusal(update(&t, &Scope::all(), json!({"size": 0})).await),
        TableError::Source("Things has no entity type".into())
    );
}

#[tokio::test]
async fn a_write_refuses_rows_outside_what_the_caller_may_change() {
    let rows: Vec<Value> = (0..12)
        .map(|i| json!({"id": format!("r{i:02}"), "org": "a", "size": i, "must": "x"}))
        .collect();
    let t = table(rows);
    let scope = Scope::all().changing(&["size"], vec![json!(-1)]);
    let TableError::Forbidden(why) = refusal(update(&t, &scope, json!({"size": 0})).await) else {
        panic!("not a 403");
    };
    assert!(
        why.starts_with("12 of the 12 rows the filter matches are outside what you may change (\"r00\", \"r01\""),
        "{why}"
    );
    assert!(why.contains("\"r09\", ...); nothing was changed"), "{why}");
    assert!(!why.contains("r10"), "{why}");
}

#[tokio::test]
async fn an_update_cannot_move_a_row_out_of_the_scope() {
    let t = table(things());
    let scope = Scope::all().and(&["org"], vec![json!("a")]);
    assert_eq!(
        refusal(update(&t, &scope, json!({"org": "b"})).await),
        TableError::Forbidden(
            "the change would move \"t1\" outside what you may change; nothing was changed".into()
        )
    );
    // nothing changed
    let left = query(&t, &org_is("a"), &Scope::all(), 10)
        .await
        .expect("a read");
    assert_eq!(left.rows.len(), 2);
}

#[tokio::test]
async fn a_dry_run_and_an_empty_match_change_nothing() {
    let t = table(things());
    let dry = write(&t, &org_is("a"), &Scope::all(), Change::Delete, true, 10)
        .await
        .expect("a dry run");
    assert!(dry.dry_run);
    assert_eq!(dry.keys, [json!("t1"), json!("t2")]);
    let none = write(&t, &org_is("zz"), &Scope::all(), Change::Delete, false, 10)
        .await
        .expect("no rows");
    assert!(!none.dry_run && none.keys.is_empty());
    let all = query(&t, &QueryOptions::default(), &Scope::all(), 10)
        .await
        .expect("a read");
    assert_eq!(all.rows.len(), 3);
}

#[tokio::test]
async fn a_batch_whose_row_moved_is_refused_whole() {
    let t = table(things());
    let batch = crate::Batch {
        change: Change::Delete,
        keys: vec![json!("t1"), json!("t3")],
        guard: Cond::Compare {
            path: vec!["org".into()],
            op: CompareOp::Eq,
            value: json!("a"),
        },
    };
    assert_eq!(
        t.apply(&batch).await,
        Err(TableError::Conflict(
            "\"t3\" changed since the filter matched it; nothing was changed, read the rows again and retry".into()
        ))
    );
    let all = query(&t, &QueryOptions::default(), &Scope::all(), 10)
        .await
        .expect("a read");
    assert_eq!(all.rows.len(), 3);
}

fn service() -> Service {
    let address = FieldKind::Object {
        name: "Address".into(),
        fields: vec![Field::new("city", FieldKind::String, "")],
    };
    let mut key = Field::new("n", FieldKind::Integer, "the number");
    key.nullable = true;
    let mut fixed = Field::new("fixed", FieldKind::Boolean, "");
    fixed.nullable = false;
    let rules = |ops: &[Operation]| {
        ops.iter()
            .map(|&operation| Rule {
                operation,
                permission: Some(format!("things.{}", operation.as_str())),
            })
            .collect()
    };
    Service {
        namespace: "Test".into(),
        title: "Test".into(),
        sets: vec![
            EntityDef {
                set: "Counted".into(),
                type_name: "Count".into(),
                description: "counted things".into(),
                key: "n".into(),
                fields: vec![
                    key,
                    Field::new("score", FieldKind::Number, ""),
                    Field::new("at", FieldKind::DateTime, ""),
                    fixed,
                    Field::new("home", address.clone(), "where"),
                    Field::new("work", address, ""),
                ],
                rules: rules(&Operation::ALL),
            },
            EntityDef {
                set: "Named".into(),
                type_name: "Name".into(),
                description: String::new(),
                key: "id".into(),
                fields: vec![Field::new("id", FieldKind::String, "")],
                rules: rules(&[Operation::Read]),
            },
        ],
    }
}

#[test]
fn the_model_types_each_field_and_each_object_once() {
    let model = service().model();
    let schema = &model.schemas[0];
    let count = &schema.entity_types[0];
    let types: Vec<(&str, &str, bool)> = count
        .properties
        .iter()
        .map(|p| (p.name.as_str(), p.ty.name.as_str(), p.nullable))
        .collect();
    assert_eq!(
        types,
        [
            ("n", "Edm.Int64", false),
            ("score", "Edm.Double", true),
            ("at", "Edm.DateTimeOffset", true),
            ("fixed", "Edm.Boolean", false),
            ("home", "Test.Address", true),
            ("work", "Test.Address", true),
        ]
    );
    let complex: Vec<&str> = schema
        .complex_types
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(complex, ["Address"]);
}

#[test]
fn the_openapi_document_follows_the_definition() {
    let doc = service().openapi("/odata");
    let count = &doc["components"]["schemas"]["Count"];
    assert_eq!(count["description"], "counted things");
    assert_eq!(count["required"], json!(["n"]));
    assert_eq!(
        count["properties"]["n"],
        json!({"type": "integer", "format": "int64", "description": "the number"})
    );
    assert_eq!(
        count["properties"]["score"]["type"],
        json!(["number", "null"])
    );
    assert_eq!(
        count["properties"]["at"],
        json!({"type": ["string", "null"], "format": "date-time"})
    );
    assert_eq!(count["properties"]["fixed"], json!({"type": "boolean"}));
    assert_eq!(
        count["properties"]["home"],
        json!({"anyOf": [{"$ref": "#/components/schemas/Address"}, {"type": "null"}], "description": "where"})
    );
    let single = doc["paths"]
        .as_object()
        .and_then(|p| {
            p.iter()
                .find(|(k, _)| k.contains("Counted("))
                .map(|(_, v)| v)
        })
        .expect("an entity path");
    assert_eq!(single["delete"]["operationId"], "Counted.delete");
    assert_eq!(
        single["get"]["parameters"][0]["schema"],
        json!({"type": "integer", "format": "int64"})
    );
    let collection = doc["paths"]
        .as_object()
        .and_then(|p| {
            p.iter()
                .find(|(k, _)| k.ends_with("/Counted"))
                .map(|(_, v)| v)
        })
        .expect("a collection path");
    assert_eq!(collection["delete"]["operationId"], "Counted.deleteWhere");
    let named = doc["paths"]
        .as_object()
        .and_then(|p| p.iter().find(|(k, _)| k.contains("Named(")).map(|(_, v)| v))
        .expect("an entity path");
    assert_eq!(
        named["get"]["parameters"][0]["schema"],
        json!({"type": "string"})
    );
    assert!(named.get("delete").is_none() && named.get("patch").is_none());
}

#[tokio::test]
async fn allowed_answers_each_refusal_in_words() {
    let t = table(things());
    let decide = |scope: Scope, op: Operation, key: Option<Value>| {
        let t = &t;
        async move {
            allowed(t, &scope, op, key.as_ref())
                .await
                .expect("a memory source never fails")
        }
    };
    let mut narrow = info();
    narrow.rules = vec![Rule {
        operation: Operation::Update,
        permission: Some("things.write".into()),
    }];
    let guarded = MemoryTable::new(narrow, things()).expect("keyed rows");
    let no_rule = allowed(&guarded, &Scope::all(), Operation::Read, None)
        .await
        .expect("no fetch");
    assert_eq!(
        (no_rule.allowed, no_rule.reason.as_str()),
        (false, "Things does not offer read")
    );
    let unheld = allowed(
        &guarded,
        &Scope::all().holding(&[]),
        Operation::Update,
        None,
    )
    .await
    .expect("no fetch");
    assert_eq!(unheld.reason, "update needs the permission things.write");
    assert_eq!(unheld.permission.as_deref(), Some("things.write"));
    assert_eq!(
        unheld.to_json(),
        json!({"allowed": false, "reason": "update needs the permission things.write", "permission": "things.write"})
    );

    let create = decide(Scope::all(), Operation::Create, None).await;
    assert_eq!(create.reason, "Things takes no create through this service");
    let missing = decide(Scope::all(), Operation::Read, Some(json!("t9"))).await;
    assert!(missing.not_found && !missing.allowed);
    assert_eq!(missing.reason, "Things has no such entity");
    let hidden = decide(
        Scope::all().and(&["org"], vec![json!("b")]),
        Operation::Read,
        Some(json!("t1")),
    )
    .await;
    assert!(
        hidden.not_found,
        "a row outside the read scope answers as missing"
    );
    let fixed = decide(
        Scope::all().changing(&["org"], vec![json!("b")]),
        Operation::Delete,
        Some(json!("t1")),
    )
    .await;
    assert_eq!(
        fixed.reason,
        "this entity of Things is outside what you may change"
    );
    assert!(!fixed.not_found);
    let ok = decide(Scope::all(), Operation::Delete, Some(json!("t1"))).await;
    assert!(ok.allowed);
    assert_eq!(ok.reason, "allowed");
}

#[test]
fn values_of_different_kinds_never_compare_equal() {
    let row = json!({"id": "t1", "org": "a", "address": {"city": "x"}, "tags": ["a"]});
    let cond = |path: &str, op: CompareOp, value: Value| Cond::Compare {
        path: vec![path.into()],
        op,
        value,
    };
    assert!(!crate::matches(&row, &cond("org", CompareOp::Eq, json!(1))));
    assert!(!crate::matches(
        &row,
        &cond("org", CompareOp::Ne, json!(null))
    ));
    assert!(!crate::matches(
        &row,
        &cond("address", CompareOp::Eq, json!("x"))
    ));
    assert!(crate::matches(
        &row,
        &cond("address", CompareOp::Eq, json!({"city": "x"}))
    ));
    assert!(crate::matches(
        &row,
        &cond("tags", CompareOp::Ge, json!(["a"]))
    ));
}
