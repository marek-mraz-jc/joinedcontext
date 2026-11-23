#![forbid(unsafe_code)]
//! The catalogue search the Postgres store answers in SQL (S11-38): the same page, total and
//! facets as the Rust reference (`search_datasets` over every record) and the memory store, the
//! visibility rules kept, rows written before the search columns still found, views of a
//! dataset in one read, and the cost of a search page against a 10x catalogue.

mod common;

use common::*;
use dataroom_core::{
    Dataset, DatasetFilter, PolicyAuthorizer, Publisher, RequestCtx, SearchCandidate, Store,
};
use dataroom_search::{search_datasets, search_store, SearchOutcome};
use dataroom_store_memory::MemoryStore;
use serde_json::json;
use std::time::Instant;

fn keep_all(_: &Dataset, _: &[Publisher]) -> bool {
    true
}

fn ds(name: &str, publisher: &str, extra: serde_json::Value) -> Dataset {
    let mut base = serde_json::to_value(dataset(name, publisher, "public")).expect("fixture");
    base["id"] = json!(name);
    base["distributions"][0]["id"] = json!(format!("{name}-csv"));
    base["distributions"][0]["datasetId"] = json!(name);
    for (k, v) in extra.as_object().expect("object").clone() {
        base[k] = v;
    }
    serde_json::from_value(base).expect("dataset")
}

/// A catalogue that exercises every search rule: diacritics and declension, hyphens and
/// underscores, every title language, keywords, facets, visibility, state and a withdrawn
/// publisher.
fn fixture() -> (Vec<Publisher>, Vec<Dataset>) {
    let mut gone = publisher("zaniknuty");
    gone.state = dataroom_core::RecordState::Withdrawn;
    let publishers = vec![publisher("mesto"), publisher("kraj"), gone];
    let long_word = "a".repeat(3000);
    let datasets = vec![
        ds(
            "kvalita-ovzdusia",
            "mesto",
            json!({"title": {"sk": "Kvalita ovzdušia v Lučenci"}, "themes": ["ENVI"],
                   "licenceId": "cc-by-4.0", "temporal": {"start": "2020-01-01"}}),
        ),
        ds(
            "dochodky",
            "kraj",
            json!({"title": {"sk": "Dôchodky a dôchodok"}, "description": {"cs": "Důchody"},
                   "themes": ["SOCI", "ECON"], "spatial": ["http://publications.europa.eu/resource/authority/place/SVK"]}),
        ),
        ds(
            "bike-sharing",
            "mesto",
            json!({"title": {"en": "Bike-sharing stations", "fi": "Kaupunkipyörät", "ca": "Bicing"},
                   "tags": ["bike-sharing", "doprava"], "keywords": {"en": ["mobility"]},
                   "distributions": [
                       {"id": "bs-1", "datasetId": "bike-sharing", "title": {"sk": "x"}, "accessUrl": "https://e.org/1", "format": "csv"},
                       {"id": "bs-2", "datasetId": "bike-sharing", "title": {"sk": "y"}, "accessUrl": "https://e.org/2", "format": "GeoJSON"}]}),
        ),
        ds(
            "air_quality_2021",
            "kraj",
            json!({"title": {"de": "Luftqualität"}, "description": {"en": "Hourly PM10 and NO2"},
                   "licenceId": "odc-odbl", "temporal": {"start": "2019-05-01", "end": "2021-12-31"}}),
        ),
        ds(
            "interne-data",
            "mesto",
            json!({"visibility": "internal", "title": {"sk": "Interné ovzdušie"}}),
        ),
        ds(
            "obmedzene",
            "mesto",
            json!({"visibility": "restricted", "restrictedGrants": ["editor"], "title": {"sk": "Obmedzené ovzdušie"}}),
        ),
        ds(
            "navrh",
            "kraj",
            json!({"state": "draft", "title": {"sk": "Návrh ovzdušia"}}),
        ),
        ds(
            "stiahnute",
            "kraj",
            json!({"state": "withdrawn", "title": {"sk": "Stiahnuté ovzdušie"}}),
        ),
        ds(
            "stary-vydavatel",
            "zaniknuty",
            json!({"title": {"sk": "Ovzdušie zaniknutého vydavateľa"}}),
        ),
        ds(
            "dlhe-slovo",
            "kraj",
            json!({"title": {"sk": "Dlhé slovo"}, "description": {"sk": format!("{long_word} koniec")}}),
        ),
        ds(
            "rozpocet",
            "mesto",
            json!({"title": {"sk": "Rozpočet mesta 2024"}, "themes": ["GOVE"], "tags": ["financie"],
                   "modified": "2026-11-05T10:00:00Z"}),
        ),
    ];
    (publishers, datasets)
}

fn filters() -> Vec<DatasetFilter> {
    let base = |q: &str| DatasetFilter {
        q: q.into(),
        limit: 20,
        ..DatasetFilter::default()
    };
    let mut out: Vec<DatasetFilter> = [
        "",
        "ovzdušie",
        "Lučenec dôchodok",
        "dôchodok",
        "bike-sharing",
        "air_q",
        "pyörät",
        "luftqual",
        "2021",
        "quality",
        "nič-také",
        "aaaa",
        "koniec rozpočet",
        "*:*",
    ]
    .iter()
    .map(|q| base(q))
    .collect();
    // A query word past a tsvector's 2046 bytes is cut like the index word it matches.
    out.push(base(&"a".repeat(3000)));
    out.push(DatasetFilter {
        publishers: vec!["mesto".into()],
        ..base("")
    });
    out.push(DatasetFilter {
        themes: vec!["envi".into()],
        formats: vec!["CSV".into()],
        ..base("")
    });
    out.push(DatasetFilter {
        licences: vec!["cc-by-4.0".into()],
        years: vec![2020],
        ..base("")
    });
    for sort in ["name desc", "title asc", "metadata_modified desc"] {
        out.push(DatasetFilter {
            sort: Some(sort.into()),
            ..base("")
        });
    }
    out.push(DatasetFilter {
        offset: 2,
        limit: 3,
        ..base("")
    });
    out.push(DatasetFilter {
        include_private: true,
        include_drafts: true,
        include_withdrawn: true,
        ..base("ovzdušie")
    });
    out
}

async fn seed(store: &dyn Store, ctx: &RequestCtx, publishers: &[Publisher], datasets: &[Dataset]) {
    let host = unique_host("search");
    store
        .upsert_catalogue(catalogue(ctx.org_id, ctx.catalogue_id, "search", &host))
        .await
        .expect("catalogue");
    for p in publishers {
        store
            .upsert_publisher(ctx, p.clone())
            .await
            .expect("publisher");
    }
    for d in datasets {
        store.upsert_dataset(ctx, d.clone()).await.expect("dataset");
    }
}

fn ids(outcome: &SearchOutcome) -> Vec<String> {
    outcome.datasets.iter().map(|d| d.id.clone()).collect()
}

fn assert_same(label: &str, want: &SearchOutcome, got: &SearchOutcome) {
    assert_eq!(ids(want), ids(got), "{label}: page");
    assert_eq!(want.total, got.total, "{label}: total");
    assert_eq!(want.facets, got.facets, "{label}: facets");
    assert_eq!(want.datasets, got.datasets, "{label}: full records");
}

#[tokio::test]
async fn sql_search_answers_like_the_rust_reference() {
    let Some(pg) = common::pg("sql_search_answers_like_the_rust_reference").await else {
        return;
    };
    let pg_store = pg.app_store().await;
    let memory = MemoryStore::new();
    let (org, cat) = tenant();
    let member = member_ctx(org, cat);
    let (publishers, datasets) = fixture();
    seed(&pg_store, &member, &publishers, &datasets).await;
    seed(&memory, &member, &publishers, &datasets).await;
    let authorizer = PolicyAuthorizer::default();
    let stored = pg_store.list_datasets(&member).await.expect("list");
    let stored_pubs = pg_store.list_publishers(&member).await.expect("publishers");

    for ctx in [RequestCtx::anonymous(org, cat), member.clone()] {
        for filter in filters() {
            let label = format!("{:?} {filter:?}", ctx.subject.actor_id);
            let want = search_datasets(&ctx, &authorizer, &stored, &stored_pubs, &filter, "sk")
                .expect("reference");
            let (got, got_pubs) =
                search_store(&pg_store, &ctx, &authorizer, &filter, "sk", &keep_all, None)
                    .await
                    .expect("sql search");
            assert_same(&format!("postgres {label}"), &want, &got);
            assert_eq!(got_pubs, stored_pubs, "{label}: publishers");
            let (mem, _) = search_store(&memory, &ctx, &authorizer, &filter, "sk", &keep_all, None)
                .await
                .expect("memory search");
            assert_same(&format!("memory {label}"), &want, &mem);
        }
    }

    // A query word a fixture never holds proves the comparisons are not all empty.
    let hits = |q: &str| {
        let f = DatasetFilter {
            q: q.into(),
            limit: 20,
            ..DatasetFilter::default()
        };
        search_datasets(&member, &authorizer, &stored, &stored_pubs, &f, "sk")
            .expect("reference")
            .total
    };
    assert!(hits("ovzdušie") >= 2 && hits("dôchodok") == 1 && hits("nič-také") == 0);
    // A Solr local parameter is refused before any read.
    let bad = DatasetFilter {
        q: "{!lucene}x".into(),
        ..DatasetFilter::default()
    };
    assert!(
        search_store(&pg_store, &member, &authorizer, &bad, "sk", &keep_all, None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn anonymous_search_sees_no_private_draft_or_withdrawn_dataset() {
    let Some(pg) = common::pg("anonymous_search_sees_no_private_draft_or_withdrawn_dataset").await
    else {
        return;
    };
    let store = pg.app_store().await;
    let (org, cat) = tenant();
    let member = member_ctx(org, cat);
    let (publishers, datasets) = fixture();
    seed(&store, &member, &publishers, &datasets).await;
    let authorizer = PolicyAuthorizer::default();
    let everything = DatasetFilter {
        limit: 100,
        include_private: true,
        include_drafts: true,
        include_withdrawn: true,
        ..DatasetFilter::default()
    };
    let sorted = |o: &SearchOutcome| {
        let mut v = ids(o);
        v.sort();
        v
    };
    let anon = RequestCtx::anonymous(org, cat);
    let (seen, _) = search_store(
        &store,
        &anon,
        &authorizer,
        &everything,
        "sk",
        &keep_all,
        None,
    )
    .await
    .expect("anonymous search");
    // The flags widen an editor's search only (`dataset_visible`): no internal, restricted,
    // draft, withdrawn or withdrawn publisher's dataset for an anonymous caller.
    assert_eq!(
        sorted(&seen),
        [
            "air_quality_2021",
            "bike-sharing",
            "dlhe-slovo",
            "dochodky",
            "kvalita-ovzdusia",
            "rozpocet"
        ]
    );
    let (editor, _) = search_store(
        &store,
        &member,
        &authorizer,
        &everything,
        "sk",
        &keep_all,
        None,
    )
    .await
    .expect("editor search");
    let stored = store.list_datasets(&member).await.expect("list");
    let pubs = store.list_publishers(&member).await.expect("pubs");
    let want =
        search_datasets(&member, &authorizer, &stored, &pubs, &everything, "sk").expect("ref");
    assert_eq!(sorted(&want), sorted(&editor));
    // The editor's grant opens the restricted one, drafts and withdrawn records; an internal
    // dataset needs a read rule this authorizer does not hold.
    for id in ["navrh", "obmedzene", "stary-vydavatel", "stiahnute"] {
        assert!(sorted(&editor).iter().any(|e| e == id), "{id}");
    }
    assert!(!sorted(&editor).iter().any(|e| e == "interne-data"));
    // Another tenant's search finds none of it (forced RLS).
    let (other_org, other_cat) = tenant();
    let (none, _) = search_store(
        &store,
        &member_ctx(other_org, other_cat),
        &authorizer,
        &everything,
        "sk",
        &keep_all,
        None,
    )
    .await
    .expect("other tenant");
    assert_eq!(none.total, 0);
}

#[tokio::test]
async fn rows_written_before_the_search_columns_are_found_then_reindexed() {
    let Some(pg) =
        common::pg("rows_written_before_the_search_columns_are_found_then_reindexed").await
    else {
        return;
    };
    let store = pg.app_store().await;
    let (org, cat) = tenant();
    let member = member_ctx(org, cat);
    let (publishers, datasets) = fixture();
    seed(&store, &member, &publishers, &datasets).await;
    // What an older build left: no search columns.
    sqlx::query(
        "UPDATE dataroom.cat_dataset SET search_v = 0, search_name = '', search_title = '',
                search_kw = '', search_desc = ''
         WHERE org_id = $1 AND catalogue_id = $2 AND id IN ('dochodky', 'rozpocet')",
    )
    .bind(org.as_uuid())
    .bind(cat.as_uuid())
    .execute(&pg.admin)
    .await
    .expect("age two rows");
    let authorizer = PolicyAuthorizer::default();
    let filter = DatasetFilter {
        q: "dôchodok".into(),
        limit: 20,
        ..DatasetFilter::default()
    };
    let mut ranks = Vec::new();
    let mut pick = |_: &[Publisher], c: Vec<SearchCandidate>| {
        ranks = c.iter().map(|c| (c.dataset.id.clone(), c.rank)).collect();
        Ok(Vec::new())
    };
    let query = dataroom_core::SearchQuery {
        terms: dataroom_search::term_candidates("dôchodok").expect("terms"),
        ..Default::default()
    };
    store
        .search_page(&member, &query, &mut pick)
        .await
        .expect("page");
    // Both aged rows come whole and unranked, for the Rust match; no indexed row matches.
    ranks.sort();
    assert_eq!(
        ranks,
        [("dochodky".to_owned(), None), ("rozpocet".to_owned(), None)]
    );
    let (found, _) = search_store(&store, &member, &authorizer, &filter, "sk", &keep_all, None)
        .await
        .expect("search");
    assert_eq!(ids(&found), ["dochodky"]);

    dataroom_store_pg::PgStore::from_pool_unchecked(pg.admin.clone())
        .migrate()
        .await
        .expect("migrate reindexes");
    let (left,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM dataroom.cat_dataset
         WHERE org_id = $1 AND catalogue_id = $2 AND search_v < 1",
    )
    .bind(org.as_uuid())
    .bind(cat.as_uuid())
    .fetch_one(&pg.admin)
    .await
    .expect("count");
    assert_eq!(left, 0);
    let (rls,): (bool,) = sqlx::query_as(
        "SELECT relforcerowsecurity FROM pg_class WHERE oid = 'dataroom.cat_dataset'::regclass",
    )
    .fetch_one(&pg.admin)
    .await
    .expect("rls");
    assert!(rls, "the reindex forces row level security again");
    let (again, _) = search_store(&store, &member, &authorizer, &filter, "sk", &keep_all, None)
        .await
        .expect("search");
    assert_eq!(ids(&again), ["dochodky"]);
}

async fn views_and_types(store: &dyn Store) {
    let (org, cat) = tenant();
    let ctx = member_ctx(org, cat);
    let (publishers, datasets) = fixture();
    seed(store, &ctx, &publishers, &datasets).await;
    let mut board = ds("nastenka", "mesto", json!({"datasetType": "dashboard"}));
    board.distributions.clear();
    store.upsert_dataset(&ctx, board).await.expect("dashboard");
    for (id, resource, order) in [("v3", "bs-2", 0), ("v2", "bs-1", 1), ("v1", "bs-1", 0)] {
        store
            .upsert_resource_view(&ctx, view(id, resource, order))
            .await
            .expect("view");
    }
    let dist = vec!["bs-2".to_owned(), "bs-1".to_owned(), "none".to_owned()];
    let one_read: Vec<String> = store
        .list_views_of(&ctx, &dist)
        .await
        .expect("views")
        .into_iter()
        .map(|v| v.id)
        .collect();
    assert_eq!(one_read, ["v3", "v1", "v2"]);
    assert!(store
        .list_views_of(&ctx, &[])
        .await
        .expect("none")
        .is_empty());
    let boards: Vec<String> = store
        .list_datasets_of_type(&ctx, "dashboard")
        .await
        .expect("type")
        .into_iter()
        .map(|d| d.id)
        .collect();
    assert_eq!(boards, ["nastenka"]);
}

#[tokio::test]
async fn views_of_a_dataset_and_datasets_of_a_type() {
    views_and_types(&MemoryStore::new()).await;
    let Some(pg) = common::pg("views_of_a_dataset_and_datasets_of_a_type").await else {
        return;
    };
    views_and_types(&pg.app_store().await).await;
}

/// A search page against 3000 datasets (10x a city catalogue) with a 2 KB description and three
/// distributions each: the bytes that reach Rust and the time, before (every record, then
/// `search_datasets`) and after (`search_store`).
#[tokio::test]
async fn search_page_cost_at_ten_times_the_catalogue() {
    let Some(pg) = common::pg("search_page_cost_at_ten_times_the_catalogue").await else {
        return;
    };
    let store = pg.app_store().await;
    let (org, cat) = tenant();
    let ctx = member_ctx(org, cat);
    let host = unique_host("perf");
    store
        .upsert_catalogue(catalogue(org, cat, "perf", &host))
        .await
        .expect("catalogue");
    store
        .upsert_publisher(&ctx, publisher("mesto"))
        .await
        .expect("publisher");
    let description = "Údaje o meraní hluku a dopravy v meste, merané každú hodinu. ".repeat(32);
    for i in 0..3000 {
        let name = format!("dataset-{i:04}");
        let dists: Vec<serde_json::Value> = (0..3)
            .map(|d| {
                json!({"id": format!("{name}-{d}"), "datasetId": name, "title": {"sk": format!("Súbor {d}")},
                       "description": {"sk": description.clone()},
                       "accessUrl": format!("https://data.example.org/{name}/{d}.csv"), "format": "CSV"})
            })
            .collect();
        let topic = if i % 10 == 0 {
            "ovzdušie"
        } else {
            "cyklotrasy"
        };
        let d = ds(
            &name,
            "mesto",
            json!({"title": {"sk": format!("Dataset {i} {topic}")},
                   "description": {"sk": description.clone()}, "tags": [], "distributions": dists}),
        );
        store.upsert_dataset(&ctx, d).await.expect("dataset");
    }
    let authorizer = PolicyAuthorizer::default();
    for q in ["", "ovzdušie"] {
        let filter = DatasetFilter {
            q: q.into(),
            limit: 20,
            ..DatasetFilter::default()
        };
        let before = Instant::now();
        let all = store.list_datasets(&ctx).await.expect("list");
        let pubs = store.list_publishers(&ctx).await.expect("publishers");
        let want = search_datasets(&ctx, &authorizer, &all, &pubs, &filter, "sk").expect("old");
        let old_time = before.elapsed();
        let old_bytes: usize = all
            .iter()
            .map(|d| serde_json::to_vec(d).expect("json").len())
            .sum();
        drop(all);

        let mut new_bytes = 0usize;
        let after = Instant::now();
        let query = dataroom_core::SearchQuery {
            terms: dataroom_search::term_candidates(q).expect("terms"),
            ..Default::default()
        };
        let mut pick = |_: &[Publisher], c: Vec<SearchCandidate>| {
            new_bytes = c
                .iter()
                .map(|c| serde_json::to_vec(&c.dataset).expect("json").len())
                .sum();
            Ok(Vec::new())
        };
        store
            .search_page(&ctx, &query, &mut pick)
            .await
            .expect("page");
        let (got, _) = search_store(&store, &ctx, &authorizer, &filter, "sk", &keep_all, None)
            .await
            .expect("new");
        let new_time = after.elapsed();
        assert_same(&format!("perf q={q:?}"), &want, &got);
        println!(
            "q={q:?} matches={} before: {old_bytes} B in {old_time:?}; after: {new_bytes} B \
             (+ {} page records) in {new_time:?}",
            want.total,
            got.datasets.len()
        );
        // The slim rows carry no description and no distribution body: a tenth of the bytes.
        assert!(
            new_bytes * 10 <= old_bytes,
            "q={q:?}: {new_bytes} B reached Rust, {old_bytes} B before"
        );
        assert!(
            new_time.as_secs() < 10,
            "q={q:?}: a search page took {new_time:?}"
        );
    }
}
