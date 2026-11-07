// SPDX-License-Identifier: EUPL-1.2
//! T-3464: no request text ever becomes SQL text.
//!
//! Every surface reaches storage through the `q`, `scopeQ` and `geoQ`
//! compilers and the store's own filters. A string a client wrote travels
//! as a bind parameter; the statement itself is built only from the
//! compilers' templates. Two checks hold that:
//!
//! - a property over the compilers (no database): injection payloads in
//!   every literal position, and every statement that comes out consists of
//!   template words and template punctuation only, with the payload found in
//!   a bind;
//! - payloads in every filter field of `PgEntityStore::query` against a live
//!   PostGIS: the store answers, and the table it reads is untouched.
//!
//! The fuzz target `q_to_sql` runs the first check on arbitrary bytes.

use antares_ql::geo::{GeoSpec, Rel};
use antares_ql::parse_q;
use antares_sql::compile::{
    geo::compile_geo, q::compile_q, qprefilter::compile_prefilter, scope::compile_scope_q,
};
use proptest::prelude::*;
use serde_json::json;

/// Every word a compiler's template writes into a statement.
const TEMPLATE_WORDS: &[&str] = &[
    "entity",
    "scopes",
    "location",
    "AND",
    "OR",
    "NOT",
    "EXISTS",
    "SELECT",
    "FROM",
    "unnest",
    "AS",
    "s",
    "WHERE",
    "jsonpath",
    "geography",
    "ST_SetSRID",
    "ST_GeomFromGeoJSON",
    "ST_DWithin",
    "ST_Distance",
    "ST_Within",
    "ST_Contains",
    "ST_Intersects",
    "ST_Disjoint",
    "ST_Overlaps",
    "ST_Equals",
    "IS",
    "NULL",
    "location_ambiguous",
    "1",
    "4326",
    "location_cell",
    "ANY",
    "ARRAY",
    "ST_MakeEnvelope",
];

/// Whether `sql` is template text alone: placeholders, template words and
/// the punctuation the templates use. A quote, a semicolon, a comment, a
/// backslash or any word a request could carry fails it.
fn template_only(sql: &str) -> bool {
    let mut rest = sql;
    while let Some(i) = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
        let (word, tail) = rest.split_at(i);
        if !word.is_empty() && !TEMPLATE_WORDS.contains(&word) {
            return false;
        }
        let mut chars = tail.chars();
        let c = chars.next().unwrap_or(' ');
        if c == '$' {
            // a placeholder: `$` and digits only
            let digits = chars.as_str().len()
                - chars
                    .as_str()
                    .trim_start_matches(|d: char| d.is_ascii_digit())
                    .len();
            if digits == 0 {
                return false;
            }
            rest = &chars.as_str()[digits..];
            continue;
        }
        // `=` only as the second half of `>=` / `<=`, or in the geo cover's
        // cell list `= ANY(ARRAY[$n, ...])`: a bare `=` is a comparison no
        // template writes
        let ordering = c == '=' && sql[..sql.len() - tail.len()].ends_with(['>', '<']);
        let cover = (c == '=' && tail.starts_with("= ANY(ARRAY["))
            || (c == '[' && word == "ARRAY")
            || (c == ']' && tail.starts_with("])"));
        // the near box's bounding-box overlap ` && ST_MakeEnvelope(`: one
        // spaced pair, never a lone or a tripled `&`
        let overlap = c == '&'
            && (tail.starts_with("&& ")
                || (sql[..sql.len() - tail.len()].ends_with(" &") && tail.starts_with("& ")));
        if !" ()~@?><,:".contains(c) && !ordering && !cover && !overlap {
            return false;
        }
        rest = chars.as_str();
    }
    rest.is_empty() || TEMPLATE_WORDS.contains(&rest)
}

/// Fragments an injection would try, joined into one payload.
fn payload() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop::sample::select(vec![
            "'",
            "''",
            "\"",
            "\\",
            ";",
            "--",
            "/*",
            "*/",
            "$1",
            "$$",
            "\u{2019}",
            "\u{FF07}",
            "\u{201D}",
            " OR 1=1",
            "') OR ('1'='1",
            "\"); DROP TABLE entities; --",
            "E'\\x27'",
            "\n",
            "\t",
            "inj7q",
            "SELECT",
            "pg_sleep(5)",
            "::text",
            "@?",
            "${0}",
        ]),
        1..6,
    )
    .prop_map(|parts| parts.concat())
}

fn expand(term: &str) -> String {
    format!("https://example.org/{term}")
}

proptest! {
    /// A payload in a `q` string value, a `scopeQ` segment or a geometry
    /// never reaches the statement text, and does reach its binds.
    #[test]
    fn compiled_statements_carry_request_text_only_in_binds(p in payload()) {
        // q: the payload as a string value (the q grammar escapes nothing,
        // so a payload with `"` cannot be one value; the parser refuses it)
        let q = format!("name==\"{p}\"");
        if let Ok(ast) = parse_q(&q) {
            if let Some(c) = compile_q(&ast, "entity", 1, &expand) {
                prop_assert!(template_only(&c.sql), "q={q:?} wrote {}", c.sql);
                // whatever the payload split the query into, each bind is a
                // jsonpath the database parses as data
                prop_assert!(c.binds.iter().all(|b| b.starts_with("$.")), "q={q:?} binds {:?}", c.binds);
            }
        }
        // scopeQ: the payload as a segment
        let sq = format!("/a/{}", p.replace(['/', ',', '|', ';', '(', ')'], "x"));
        if let Some(c) = compile_scope_q(&sq, "scopes", 3) {
            prop_assert!(template_only(&c.sql), "scopeQ={sq:?} wrote {}", c.sql);
        }
        // geoQ: the payload inside the coordinates (the API validates them;
        // the compiler must not depend on that)
        let coordinates = json!([p, 1.0]);
        for rel in [Rel::Within, Rel::Intersects, Rel::Near { max: Some(10.0), min: Some(1.0) }] {
            let spec = GeoSpec { rel, geometry: "Point", coordinates: &coordinates, geoproperty_iri: "" };
            if let Some(c) = compile_geo(&spec, "location", 2) {
                prop_assert!(template_only(&c.sql), "geo wrote {}", c.sql);
                prop_assert!(c.geo_binds.iter().any(|b| b.contains("coordinates")));
            }
        }
    }
}

proptest! {
    /// The temporal store's `q` prefilter (store/pg/temporal.rs) spells its
    /// own attribute keys into the statement; a payload in a string value or
    /// a bracketed member name reaches only the binds (T-4778).
    #[test]
    fn temporal_prefilter_carries_request_text_only_in_binds(p in payload()) {
        let member = p.replace(['[', ']', '"', ';', '|', ',', '(', ')', '.', '=', '!', '<', '>', '~'], "x");
        for q in [format!("name==\"{p}\""), format!("name[{member}]==\"x\""), format!("name[{member}]")] {
            let Ok(ast) = parse_q(&q) else { continue };
            if let Some(c) = compile_prefilter(&ast, None, "m", 1, &expand) {
                for leak in ["inj7q", "DROP", "pg_sleep", "--", "/*", ";", "\\", "\u{2019}", "\u{FF07}"] {
                    prop_assert!(!c.sql.contains(leak), "q={q:?} wrote {leak:?} into {}", c.sql);
                }
                if p.contains("inj7q") {
                    prop_assert!(c.binds.iter().any(|b| b.contains("inj7q")), "q={q:?} binds {:?}", c.binds);
                }
            }
        }
    }
}

/// The property above is not vacuous: a member name and a value both
/// compile, and land in the binds.
#[test]
fn the_temporal_prefilter_binds_member_and_value() {
    for q in ["name[inj7q]==\"x\"", "name==\"inj7q\""] {
        let ast = parse_q(q).expect("parses");
        let c = compile_prefilter(&ast, None, "m", 1, &expand).expect("compiles");
        assert!(!c.sql.contains("inj7q"), "{}", c.sql);
        assert!(c.binds.iter().any(|b| b.contains("inj7q")), "{:?}", c.binds);
    }
}

#[test]
fn the_template_check_refuses_what_an_injection_writes() {
    assert!(template_only(
        "(entity @? $2::jsonpath OR entity @? $3::jsonpath)"
    ));
    assert!(template_only(
        "EXISTS (SELECT 1 FROM unnest(scopes) AS s WHERE s ~ $4)"
    ));
    for written in [
        "entity @? '$.x'::jsonpath",
        "entity = 1; DROP TABLE entities",
        "entity @? $2 -- comment",
        "location IS NULL OR 1=1",
        "entity @? $::jsonpath",
    ] {
        assert!(!template_only(written), "{written}");
    }
}

#[cfg(feature = "postgres")]
mod against_postgres {
    use antares_model::TenantId;
    use antares_sql::store::filter::IdLiteral;
    use antares_sql::store::pg;
    use antares_sql::store::pg::entity::{EntityFilter, PgEntityStore};
    use serde_json::json;

    const PAYLOADS: &[&str] = &[
        "'",
        "x' OR '1'='1",
        "\"); DROP TABLE entities; --",
        "$1",
        "\\",
        "\u{2019} OR 1=1",
        "/* */",
        "a\0b",
    ];

    fn expand(term: &str) -> String {
        format!("https://example.org/{term}")
    }

    /// Payloads in every filter field are data to the store: it answers each
    /// query without an error, and the canary entity is still there after.
    #[tokio::test(flavor = "multi_thread")]
    async fn every_filter_field_takes_an_injection_as_data() {
        let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
            eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
            return;
        };
        let pool = pg::connect(&url, 4).await.expect("connect");
        let store = PgEntityStore::new(pool.clone());
        let tenant = TenantId::new("pginjection").expect("tenant");
        pg::ensure_tenant(&pool, &tenant).await.expect("tenant row");
        let canary = "urn:ngsi-ld:T:canary";
        let _ = store.delete(&tenant, canary).await;
        let doc = json!({
            "id": canary, "type": "https://example.org/T",
            "createdAt": "2026-10-10T00:00:00Z", "modifiedAt": "2026-10-10T00:00:00Z",
            "https://example.org/name": [{"type": "https://uri.etsi.org/ngsi-ld/Property", "value": "x"}]
        });
        assert!(store.create(&tenant, canary, &doc).await.expect("create"));

        for p in PAYLOADS {
            let one = [*p];
            let list = vec![(*p).to_owned()];
            let groups = vec![list.clone()];
            let q_text = format!("name==\"{}\"", p.replace('"', ""));
            let ast = antares_ql::parse_q(&q_text).ok();
            let filters = [
                EntityFilter {
                    ids: Some(&one),
                    expand: &expand,
                    ..Default::default()
                },
                EntityFilter {
                    id_literal: Some(IdLiteral {
                        text: p,
                        anchored: true,
                    }),
                    expand: &expand,
                    ..Default::default()
                },
                EntityFilter {
                    id_literal: Some(IdLiteral {
                        text: p,
                        anchored: false,
                    }),
                    expand: &expand,
                    ..Default::default()
                },
                EntityFilter {
                    types: Some(&groups),
                    expand: &expand,
                    ..Default::default()
                },
                EntityFilter {
                    attrs: Some(&list),
                    expand: &expand,
                    ..Default::default()
                },
                EntityFilter {
                    q: ast.as_ref(),
                    expand: &expand,
                    ..Default::default()
                },
                EntityFilter {
                    scope_q: Some(p),
                    expand: &expand,
                    ..Default::default()
                },
                EntityFilter {
                    keep_attrs: Some(&list),
                    expand: &expand,
                    ..Default::default()
                },
                EntityFilter {
                    drop_attrs: Some(&list),
                    expand: &expand,
                    ..Default::default()
                },
            ];
            for (field, filter) in filters.iter().enumerate() {
                let answer = store.query(&tenant, filter).await;
                assert!(
                    answer.is_ok(),
                    "payload {p:?} in filter field #{field}: {:?}",
                    answer.err()
                );
            }
        }
        let still = store
            .query(
                &tenant,
                &EntityFilter {
                    expand: &expand,
                    ..Default::default()
                },
            )
            .await
            .expect("read back");
        assert!(
            still.rows.iter().any(|r| r["id"] == canary),
            "the canary is gone"
        );
        let tables: i64 =
            sqlx::query_scalar("SELECT count(*) FROM pg_tables WHERE tablename = 'entities'")
                .fetch_one(&pool)
                .await
                .expect("catalog");
        assert_eq!(tables, 1, "the entities table is still there");
    }
}

/// The fuzz crash of train 20261020-1946 (fuzz/regressions/q_to_sql): a valid
/// point writes the geo cover over `location_cell`, template text whose cells
/// travel in binds; a bare `=` or bracket anywhere else still fails.
#[test]
fn the_geo_cover_is_template_text() {
    let coordinates = json!([10.0, 50.0]);
    let spec = GeoSpec {
        rel: Rel::Within,
        geometry: "Point",
        coordinates: &coordinates,
        geoproperty_iri: "",
    };
    let c = compile_geo(&spec, "location", 1).expect("a point compiles");
    assert!(
        c.sql.contains("location_cell = ANY(ARRAY["),
        "no cover: {}",
        c.sql
    );
    assert!(template_only(&c.sql), "cover wrote {}", c.sql);
    assert!(!c.cell_binds.is_empty());
    assert!(!template_only("location_cell = $1"));
    assert!(!template_only("location[$1]"));
}

#[test]
fn the_near_box_is_template_text() {
    let coordinates = json!([10.0, 50.0]);
    let spec = GeoSpec {
        rel: Rel::Near {
            max: Some(1500.0),
            min: None,
        },
        geometry: "Point",
        coordinates: &coordinates,
        geoproperty_iri: "",
    };
    let c = compile_geo(&spec, "location", 1).expect("a point compiles");
    assert!(
        c.sql.contains("location && ST_MakeEnvelope($"),
        "no box: {}",
        c.sql
    );
    assert!(template_only(&c.sql), "near wrote {}", c.sql);
    for written in [
        "location & $1",
        "location &&& $1",
        "location&&$1",
        "location && $1 && $2 & $3",
    ] {
        assert!(!template_only(written), "{written}");
    }
}
