// SPDX-License-Identifier: MIT
//! One query table answered by `MemoryTable`, `GitTable` and `PgTable` over the same rows (T-6642):
//! same rows, same order, same count, same next page, same refusal on every source.
//!
//! `PgTable` runs when `ANTARES_TEST_DATABASE_URL` names a PostgreSQL server, and fails (never
//! skips) when the variable is set and the server is unreachable.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr as _;
use std::sync::Arc;

use odata_csdl::{
    ComplexType, CsdlVersion, EntityContainer, EntitySet, EntityType, Model, Property, Schema,
    TypeRef,
};
use odata_table::{
    query, Answer, GitLimits, GitTable, MemoryTable, PgTable, Scope, Table, TableError, TableInfo,
};
use odata_url::{EdmModel, Limits, ODataUri, QueryOptions};
use serde_json::{json, Value};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::PgPool;

/// `?` for a test: any failure as its text.
trait Must<T> {
    fn must(self) -> Result<T, String>;
}

impl<T, E: std::fmt::Debug> Must<T> for Result<T, E> {
    fn must(self) -> Result<T, String> {
        self.map_err(|e| format!("{e:?}"))
    }
}

impl<T> Must<T> for Option<T> {
    fn must(self) -> Result<T, String> {
        self.ok_or_else(|| "nothing there".to_owned())
    }
}

fn model() -> Arc<Model> {
    let mut thing = EntityType::new("Thing");
    thing.key = vec!["id".into()];
    thing.properties = vec![
        Property::new("id", TypeRef::single("Edm.String")),
        Property::new("org", TypeRef::single("Edm.String")),
        Property::new("name", TypeRef::single("Edm.String")),
        Property::new("size", TypeRef::single("Edm.Int64")),
        Property::new("score", TypeRef::single("Edm.Double")),
        Property::new("active", TypeRef::single("Edm.Boolean")),
        Property::new("address", TypeRef::single("Test.Address")),
    ];
    let mut address = ComplexType::new("Address");
    address.properties = vec![
        Property::new("city", TypeRef::single("Edm.String")),
        Property::new("zip", TypeRef::single("Edm.String")),
    ];
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

/// The fixture: nulls, missing members, nested values, shared sort values, two organizations.
/// Integral doubles are written as integers, as PostgreSQL writes them back.
fn rows() -> Vec<Value> {
    vec![
        json!({"id": "t1", "org": "a", "name": "Alpha", "size": 10, "score": 2.5, "active": true, "address": {"city": "Ruzomberok", "zip": "034 01"}}),
        json!({"id": "t2", "org": "a", "name": "beta", "size": null, "score": 7, "active": false, "address": {"city": "Zilina"}}),
        json!({"id": "t3", "org": "b", "name": "Gamma ray", "size": 3, "score": -1, "active": true, "address": null}),
        json!({"id": "t4", "org": "b", "name": "delta_50%", "size": 10, "score": 7, "active": null, "address": {"city": "Ruzomberok", "zip": null}}),
        json!({"id": "t5", "org": "a", "name": null, "size": 42, "active": true}),
        json!({"id": "t6", "org": "c", "name": "Alphabet", "size": -5, "score": 0.25, "active": false, "address": {"city": "Kosice", "zip": "040 01"}}),
        json!({"id": "t7", "org": "a", "name": "it's", "size": 7, "score": 100, "active": true, "address": {"city": "Žilina", "zip": "010 01"}}),
    ]
}

fn options(model: &Model, query: &str) -> Result<QueryOptions, String> {
    let uri = odata_url::parse(
        &format!("Things?{query}"),
        &EdmModel(model),
        &Limits::strict(),
    )
    .map_err(|e| format!("{query}: {e:?}"))?;
    let ODataUri::Resource { options, .. } = uri else {
        return Err(format!("{query}: not a resource"));
    };
    Ok(options)
}

/// Every query of the table with the ids the reference semantics answers, in order.
const TABLE: &[(&str, &[&str])] = &[
    ("$filter=size eq 10", &["t1", "t4"]),
    ("$filter=size ne 10", &["t2", "t3", "t5", "t6", "t7"]),
    ("$filter=size gt 7", &["t1", "t4", "t5"]),
    ("$filter=size ge 7", &["t1", "t4", "t5", "t7"]),
    ("$filter=size lt 7", &["t3", "t6"]),
    ("$filter=size le 7", &["t3", "t6", "t7"]),
    ("$filter=7 lt size", &["t1", "t4", "t5"]),
    ("$filter=score gt 2.4", &["t1", "t2", "t4", "t7"]),
    ("$filter=size eq null", &["t2"]),
    (
        "$filter=size ne null",
        &["t1", "t3", "t4", "t5", "t6", "t7"],
    ),
    ("$filter=score eq null", &["t5"]),
    ("$filter=active eq true", &["t1", "t3", "t5", "t7"]),
    ("$filter=active", &["t1", "t3", "t5", "t7"]),
    ("$filter=not (active eq true)", &["t2", "t4", "t6"]),
    ("$filter=not (size gt 7)", &["t2", "t3", "t6", "t7"]),
    ("$filter=size gt 5 and org eq 'a'", &["t1", "t5", "t7"]),
    ("$filter=org eq 'c' or size eq 3", &["t3", "t6"]),
    ("$filter=contains(name,'lpha')", &["t1", "t6"]),
    ("$filter=contains(name,'_50%25')", &["t4"]),
    ("$filter=contains(name,'%25')", &["t4"]),
    ("$filter=startswith(name,'Alpha')", &["t1", "t6"]),
    ("$filter=endswith(name,'ray')", &["t3"]),
    ("$filter=not contains(name,'a')", &["t5", "t7"]),
    ("$filter=name eq 'it''s'", &["t7"]),
    ("$filter=name gt 'Z'", &["t2", "t4", "t7"]),
    ("$filter=size in (3, 42, 99)", &["t3", "t5"]),
    ("$filter=name in ('beta', null)", &["t2", "t5"]),
    ("$filter=address/city eq 'Ruzomberok'", &["t1", "t4"]),
    ("$filter=address/zip eq null", &["t2", "t3", "t4", "t5"]),
    ("$filter=startswith(address/city,'%C5%BD')", &["t7"]),
    ("$filter=name eq 10", &[]),
    ("$filter=name eq 'x''%20OR%201%3D1%20--'", &[]),
    ("$orderby=size", &["t2", "t6", "t3", "t7", "t1", "t4", "t5"]),
    (
        "$orderby=size desc",
        &["t5", "t1", "t4", "t7", "t3", "t6", "t2"],
    ),
    (
        "$orderby=score desc,size",
        &["t7", "t2", "t4", "t1", "t6", "t3", "t5"],
    ),
    ("$orderby=name", &["t5", "t1", "t6", "t3", "t2", "t4", "t7"]),
    (
        "$orderby=active desc,id desc",
        &["t7", "t5", "t3", "t1", "t6", "t2", "t4"],
    ),
    (
        "$orderby=address/city,id desc",
        &["t5", "t3", "t6", "t4", "t1", "t2", "t7"],
    ),
    ("$select=name,size&$filter=org eq 'b'", &[]),
    ("$top=2&$orderby=size desc", &["t5", "t1"]),
    ("$skip=5", &["t6", "t7"]),
    ("$skip=1&$top=2&$count=true", &["t2", "t3"]),
    ("$count=true&$filter=org eq 'a'", &["t1", "t2", "t5", "t7"]),
    ("$top=0&$count=true", &[]),
];

async fn answer(
    table: &dyn Table,
    q: &str,
    scope: &Scope,
    page: u64,
) -> Result<Result<Answer, TableError>, String> {
    Ok(query(table, &options(&model(), q)?, scope, page).await)
}

fn ids(answer: &Answer) -> Vec<&str> {
    answer
        .rows
        .iter()
        .filter_map(|r| r.get("id")?.as_str())
        .collect()
}

/// A repository holding one file a row: block YAML for t1, JSON in .json and .yaml for the rest.
fn repository(rows: &[Value]) -> Result<(PathBuf, String), String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("odata-table-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(dir.join("things/a")).must()?;
    std::fs::create_dir_all(dir.join("things/b")).must()?;
    for (at, row) in rows.iter().enumerate() {
        let id = row["id"].as_str().must()?;
        let (sub, file, text) = match at {
            0 => (
                "a",
                format!("{id}.yaml"),
                "id: t1\norg: a\nname: Alpha\nsize: 10\nscore: 2.5\nactive: true\naddress:\n  city: Ruzomberok\n  zip: \"034 01\"\n".to_owned(),
            ),
            n if n % 2 == 0 => ("a", format!("{id}.json"), row.to_string()),
            _ => ("b", format!("{id}.yml"), row.to_string()),
        };
        std::fs::write(dir.join("things").join(sub).join(file), text).must()?;
    }
    // a file outside the pattern and one the pattern matches by directory only
    std::fs::write(dir.join("README.md"), "not a row").must()?;
    git(&dir, &["init", "-q"])?;
    git(&dir, &["add", "-A"])?;
    git(
        &dir,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.org",
            "commit",
            "-q",
            "-m",
            "rows",
        ],
    )?;
    let sha = String::from_utf8(git(&dir, &["rev-parse", "HEAD"])?)
        .must()?
        .trim()
        .to_owned();
    Ok((dir, sha))
}

fn git(dir: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .must()?;
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(out.stdout)
}

fn info() -> Result<TableInfo, String> {
    TableInfo::new(model(), "Things").must()
}

/// The PostgreSQL pool when the variable is set; panics when set and unreachable.
async fn admin() -> Result<Option<PgPool>, String> {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        return Ok(None);
    };
    Ok(Some(
        PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .map_err(|e| format!("ANTARES_TEST_DATABASE_URL is set and unreachable: {e}"))?,
    ))
}

/// A fresh relation holding `rows`, named after `test`.
async fn relation(pool: &PgPool, test: &str, rows: &[Value]) -> Result<String, String> {
    let name = format!("odata_table_{test}_{}", std::process::id());
    for stmt in [
        format!("DROP TABLE IF EXISTS {name}"),
        format!(
            "CREATE TABLE {name} (id text PRIMARY KEY, org text, name text, size bigint, \
             score double precision, active boolean, address jsonb)"
        ),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(stmt))
            .execute(pool)
            .await
            .must()?;
    }
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO {name} SELECT * FROM jsonb_populate_recordset(NULL::{name}, $1)"
    )))
    .bind(Value::Array(rows.to_vec()))
    .execute(pool)
    .await
    .must()?;
    Ok(name)
}

/// Every source over `rows`: memory and Git always, PostgreSQL when configured.
async fn sources(
    test: &str,
    rows: &[Value],
) -> Result<Vec<(&'static str, Box<dyn Table>)>, String> {
    let (repo, sha) = repository(rows)?;
    let mut out: Vec<(&'static str, Box<dyn Table>)> = vec![
        (
            "memory",
            Box::new(MemoryTable::new(info()?, rows.to_vec()).must()?),
        ),
        (
            "git",
            Box::new(
                GitTable::open(info()?, &repo, &sha, "things/*/*", GitLimits::strict()).must()?,
            ),
        ),
    ];
    if let Some(pool) = admin().await? {
        let name = relation(&pool, test, rows).await?;
        out.push((
            "postgres",
            Box::new(PgTable::new(info()?, pool, &name).must()?),
        ));
    }
    Ok(out)
}

#[tokio::test]
async fn every_source_answers_the_query_table_alike() -> Result<(), String> {
    let sources = sources("table", &rows()).await?;
    for (q, expected) in TABLE {
        let reference = answer(sources[0].1.as_ref(), q, &Scope::all(), 100)
            .await?
            .must()?;
        if !q.starts_with("$select") {
            assert_eq!(ids(&reference), *expected, "memory: {q}");
        }
        for (name, table) in &sources[1..] {
            let got = answer(table.as_ref(), q, &Scope::all(), 100)
                .await?
                .must()?;
            assert_eq!(got, reference, "{name} differs from memory on {q}");
        }
    }
    Ok(())
}

#[tokio::test]
async fn select_projects_and_count_counts_on_every_source() -> Result<(), String> {
    for (name, table) in sources("select", &rows()).await? {
        let got = answer(
            table.as_ref(),
            "$select=name,size&$filter=org eq 'b'&$count=true",
            &Scope::all(),
            100,
        )
        .await?
        .must()?;
        assert_eq!(
            got.rows,
            vec![
                json!({"name": "Gamma ray", "size": 3}),
                json!({"name": "delta_50%", "size": 10})
            ],
            "{name}"
        );
        assert_eq!(got.count, Some(2), "{name}");
        let all = answer(table.as_ref(), "$filter=id eq 't5'", &Scope::all(), 100)
            .await?
            .must()?;
        assert_eq!(
            all.rows,
            vec![
                json!({"id": "t5", "org": "a", "name": null, "size": 42, "score": null, "active": true, "address": null})
            ],
            "{name}: a missing member answers null"
        );
    }
    Ok(())
}

#[tokio::test]
async fn pages_follow_the_skiptoken_on_every_source() -> Result<(), String> {
    for (name, table) in sources("pages", &rows()).await? {
        let mut seen = Vec::new();
        let mut q = "$orderby=size desc&$top=6&$count=true".to_owned();
        loop {
            let got = answer(table.as_ref(), &q, &Scope::all(), 4).await?.must()?;
            assert_eq!(got.count, Some(7), "{name}");
            seen.extend(ids(&got).into_iter().map(str::to_owned));
            match got.next {
                Some(token) => {
                    q = format!("$orderby=size desc&$top=6&$count=true&$skiptoken={token}");
                }
                None => break,
            }
        }
        assert_eq!(
            seen,
            ["t5", "t1", "t4", "t7", "t3", "t6"],
            "{name}: $top bounds the pages"
        );
        let bad = answer(table.as_ref(), "$skiptoken=x", &Scope::all(), 4)
            .await?
            .err()
            .must()?;
        assert!(matches!(bad, TableError::BadRequest(_)), "{name}");
    }
    Ok(())
}

#[tokio::test]
async fn an_unsupported_construct_is_501_and_an_unknown_property_400_on_every_source(
) -> Result<(), String> {
    let unknown = QueryOptions {
        filter: Some(odata_url::Expr::Compare(
            Box::new(odata_url::Expr::Member(odata_url::MemberPath {
                variable: None,
                path: vec!["name\" OR 1=1 --".into()],
            })),
            odata_url::CompareOp::Eq,
            Box::new(odata_url::Expr::Literal(odata_url::Literal::Int(1))),
        )),
        ..QueryOptions::default()
    };
    let unknown_order = QueryOptions {
        orderby: Some(vec![odata_url::OrderBy {
            expr: odata_url::Expr::Member(odata_url::MemberPath {
                variable: None,
                path: vec!["address".into(), "nope".into()],
            }),
            descending: false,
        }]),
        ..QueryOptions::default()
    };
    for (name, table) in sources("refusals", &rows()).await? {
        for q in [
            "$filter=length(name) eq 5",
            "$filter=size add 1 eq 11",
            "$filter=tolower(name) eq 'beta'",
            "$search=alpha",
            "$orderby=length(name)",
            "$filter=address eq null",
            "$select=address/city",
        ] {
            let refused = answer(table.as_ref(), q, &Scope::all(), 100)
                .await?
                .err()
                .must()?;
            assert!(
                matches!(refused, TableError::NotImplemented(_)),
                "{name}: {q} answered {refused:?}"
            );
        }
        for options in [&unknown, &unknown_order] {
            let refused = query(table.as_ref(), options, &Scope::all(), 100)
                .await
                .err()
                .must()?;
            assert!(
                matches!(refused, TableError::UnknownProperty(_)),
                "{name}: {refused:?}"
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn a_filter_cannot_reach_rows_outside_the_scope() -> Result<(), String> {
    let scope = Scope::all().and(&["org"], vec![json!("a")]);
    for (name, table) in sources("scope", &rows()).await? {
        for (q, expected) in [
            ("$filter=true or org eq 'b'", vec!["t1", "t2", "t5", "t7"]),
            ("$filter=org eq 'b'", vec![]),
            ("$filter=id eq 't3'", vec![]),
            ("$filter=not (org eq 'a')", vec![]),
            ("$filter=address/city eq 'Kosice'", vec![]),
            ("$filter=org ne 'a' or id in ('t3', 't4', 't6')", vec![]),
        ] {
            let got = answer(table.as_ref(), &format!("{q}&$count=true"), &scope, 100)
                .await?
                .must()?;
            assert_eq!(ids(&got), expected, "{name}: {q}");
            assert_eq!(
                got.count,
                Some(u64::try_from(expected.len()).must()?),
                "{name}: {q} counted"
            );
        }
        let nested = Scope::all().and(&["address", "city"], vec![json!("Ruzomberok")]);
        let got = answer(
            table.as_ref(),
            "$filter=org eq 'b' or org eq 'c'",
            &nested,
            100,
        )
        .await?
        .must()?;
        assert_eq!(ids(&got), ["t4"], "{name}: a nested scope");
        let nothing = Scope::all().and(&["org"], vec![]);
        let got = answer(table.as_ref(), "$count=true", &nothing, 100)
            .await?
            .must()?;
        assert_eq!(
            (got.rows.len(), got.count),
            (0, Some(0)),
            "{name}: an empty restriction admits nothing"
        );
    }
    Ok(())
}

#[test]
fn a_git_set_stays_under_the_repository_root() -> Result<(), String> {
    let (repo, sha) = repository(&rows())?;
    for pattern in ["../*", "things/../../*", "/etc/*"] {
        let refused = GitTable::open(info()?, &repo, &sha, pattern, GitLimits::strict())
            .err()
            .must()?;
        assert!(matches!(refused, TableError::BadRequest(_)), "{pattern}");
    }
    for commit in ["HEAD", "--output=/tmp/x", "abc"] {
        assert!(
            GitTable::open(info()?, &repo, commit, "things/*/*", GitLimits::strict()).is_err(),
            "{commit}"
        );
    }
    // a symbolic link out of the root, matched by the pattern, refuses the set
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/etc/hostname", repo.join("things/a/escape.yaml")).must()?;
        git(&repo, &["add", "-A"])?;
        git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.org",
                "commit",
                "-q",
                "-m",
                "link",
            ],
        )?;
        let linked = String::from_utf8(git(&repo, &["rev-parse", "HEAD"])?)
            .must()?
            .trim()
            .to_owned();
        let refused = GitTable::open(info()?, &repo, &linked, "things/*/*", GitLimits::strict())
            .err()
            .must()?;
        assert!(
            matches!(&refused, TableError::BadRequest(m) if m.contains("symbolic link")),
            "{refused:?}"
        );
    }
    let small = GitLimits {
        max_files: 2,
        ..GitLimits::strict()
    };
    assert!(GitTable::open(info()?, &repo, &sha, "things/*/*", small).is_err());
    let table = GitTable::open(info()?, &repo, &sha, "things/*/*", GitLimits::strict()).must()?;
    assert_eq!(table.etag(), Some(sha.as_str()));
    Ok(())
}

#[tokio::test]
async fn a_pg_table_names_only_plain_identifiers() -> Result<(), String> {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://localhost/x")
        .must()?;
    for relation in ["things; DROP TABLE x", "a.b.c", "\"q\"", "", "t--"] {
        assert!(
            PgTable::new(info()?, pool.clone(), relation).is_err(),
            "{relation}"
        );
    }
    assert!(PgTable::new(info()?, pool, "public.things").is_ok());
    Ok(())
}

#[tokio::test]
async fn a_pg_table_keeps_forced_row_level_security() -> Result<(), String> {
    let Some(admin) = admin().await? else {
        return Ok(());
    };
    let name = relation(&admin, "rls", &rows()).await?;
    let url = std::env::var("ANTARES_TEST_DATABASE_URL").must()?;
    // Role DDL is cluster-global: the advisory lock other suites take queues the setups.
    let mut tx = admin.begin().await.must()?;
    for stmt in [
        "SELECT pg_advisory_xact_lock(4334)".to_owned(),
        "DO $$ BEGIN CREATE ROLE odata_table_app LOGIN PASSWORD 'app' NOSUPERUSER NOBYPASSRLS; \
         EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$"
            .to_owned(),
        "GRANT USAGE ON SCHEMA public TO odata_table_app".to_owned(),
        format!("GRANT SELECT ON {name} TO odata_table_app"),
        format!("ALTER TABLE {name} ENABLE ROW LEVEL SECURITY"),
        format!("ALTER TABLE {name} FORCE ROW LEVEL SECURITY"),
        format!("CREATE POLICY only_a ON {name} USING (org = 'a')"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(stmt))
            .execute(&mut *tx)
            .await
            .must()?;
    }
    tx.commit().await.must()?;
    let options = PgConnectOptions::from_str(&url)
        .must()?
        .username("odata_table_app")
        .password("app");
    let app = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .must()?;
    let bypass: bool = sqlx::query_scalar(
        "SELECT rolbypassrls OR rolsuper FROM pg_roles WHERE rolname = current_user",
    )
    .fetch_one(&app)
    .await
    .must()?;
    assert!(!bypass, "the probe role must not bypass row-level security");
    let table = PgTable::new(info()?, app, &name).must()?;
    for q in [
        "$count=true",
        "$filter=org eq 'b' or true&$count=true",
        "$filter=id eq 't3'&$count=true",
    ] {
        let got = answer(&table, q, &Scope::all(), 100).await?.must()?;
        assert!(
            ids(&got)
                .iter()
                .all(|id| ["t1", "t2", "t5", "t7"].contains(id)),
            "{q}: {:?}",
            ids(&got)
        );
        assert!(got.count.must()? <= 4, "{q}");
    }
    Ok(())
}
