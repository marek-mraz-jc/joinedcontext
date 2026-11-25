//! The activity store over PostgreSQL: every filter in the SQL, a project with no key, a store
//! whose database is down, and the records the door refuses (T-7823).

use jc_time::TimestampTz;
use serde_json::{json, Value};

use joinedcontext_portal::activity::{
    ActivityError, ActivityEvent, ActivityFilter, ActivityStore, Cursor,
};

mod common;
use common::unique::unique;

async fn pool() -> sqlx::PgPool {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .expect("set JC_PORTAL_TEST_DATABASE_URL to a PostgreSQL database the Portal migrates");
    joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate")
}

fn dead_pool() -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(50))
        .connect_lazy("postgres://nobody@127.0.0.1:1/none")
        .expect("a lazy pool")
}

/// A whole-second instant `minutes` before a fixed anchor of this run.
fn at(anchor: i64, minutes: i64) -> TimestampTz {
    TimestampTz::from_second(anchor - minutes * 60).expect("an instant")
}

struct Seed<'a> {
    project: &'a str,
    anchor: i64,
}

impl Seed<'_> {
    fn event(
        &self,
        minutes: i64,
        space: Option<&str>,
        kind: &str,
        source: &str,
        severity: &str,
        details: Value,
    ) -> ActivityEvent {
        ActivityEvent {
            time: at(self.anchor, minutes),
            project: self.project.to_owned(),
            space: space.map(str::to_owned),
            kind: kind.to_owned(),
            source: source.to_owned(),
            summary: format!("An event {minutes} minutes ago."),
            severity: severity.to_owned(),
            correlation_id: Some(format!("corr-{minutes}")),
            details,
        }
    }
}

/// Five events: ages 0..4 minutes, in two spaces and one of no space.
async fn seeded() -> (ActivityStore, String, i64) {
    let pool = pool().await;
    let project = format!("act-{}", unique());
    let mirror = common::keyed_mirror(&pool, &[&project]).await;
    let store = ActivityStore::new(Some(pool), mirror);
    let anchor = TimestampTz::now().as_second();
    let seed = Seed {
        project: &project,
        anchor,
    };
    store
        .append(&[
            seed.event(
                0,
                Some("air"),
                "mcp.tool",
                "gateway",
                "info",
                json!({ "object": "endpoints/a", "connector": "c1" }),
            ),
            seed.event(
                1,
                Some("air"),
                "access.denied",
                "gateway",
                "warning",
                json!({ "object": "endpoints/b" }),
            ),
            seed.event(
                2,
                Some("water"),
                "pipeline.error",
                "pipeline",
                "error",
                json!({}),
            ),
            seed.event(3, None, "config.applied", "reconciler", "info", Value::Null),
            seed.event(
                4,
                Some("water"),
                "mcp.tool",
                "gateway",
                "info",
                json!({ "connector": "c2" }),
            ),
        ])
        .await
        .expect("appended");
    (store, project, anchor)
}

async fn summaries(store: &ActivityStore, project: &str, filter: ActivityFilter) -> Vec<String> {
    store
        .list(project, &filter)
        .await
        .expect("a page")
        .items
        .into_iter()
        .map(|e| {
            e.summary
                .replace("An event ", "")
                .replace(" minutes ago.", "")
        })
        .collect()
}

fn filter() -> ActivityFilter {
    ActivityFilter {
        limit: 50,
        ..Default::default()
    }
}

#[tokio::test]
async fn each_filter_narrows_the_page_in_the_database() {
    let (store, project, anchor) = seeded().await;
    assert_eq!(
        summaries(&store, &project, filter()).await,
        ["0", "1", "2", "3", "4"]
    );
    let by = |change: &dyn Fn(&mut ActivityFilter)| {
        let mut f = filter();
        change(&mut f);
        f
    };
    assert_eq!(
        summaries(&store, &project, by(&|f| f.space = Some("air".into()))).await,
        ["0", "1"]
    );
    assert_eq!(
        summaries(
            &store,
            &project,
            by(&|f| f.reach = Some(vec!["water".into()]))
        )
        .await,
        ["2", "3", "4"],
        "an event of no space reaches every reader"
    );
    assert_eq!(
        summaries(&store, &project, by(&|f| f.reach = Some(Vec::new()))).await,
        ["3"]
    );
    assert_eq!(
        summaries(
            &store,
            &project,
            by(&|f| f.kinds = vec!["mcp.tool".into(), "config.applied".into()])
        )
        .await,
        ["0", "3", "4"]
    );
    assert_eq!(
        summaries(
            &store,
            &project,
            by(&|f| f.source = Some("pipeline".into()))
        )
        .await,
        ["2"]
    );
    assert_eq!(
        summaries(
            &store,
            &project,
            by(&|f| f.severity = Some("warning".into()))
        )
        .await,
        ["1", "2"]
    );
    assert_eq!(
        summaries(&store, &project, by(&|f| f.severity = Some("error".into()))).await,
        ["2"]
    );
    assert_eq!(
        summaries(&store, &project, by(&|f| f.since = Some(at(anchor, 2)))).await,
        ["0", "1", "2"]
    );
    assert_eq!(
        summaries(
            &store,
            &project,
            by(&|f| f.object = Some("endpoints/b".into()))
        )
        .await,
        ["1"]
    );
    assert_eq!(
        summaries(&store, &project, by(&|f| f.connector = Some("c2".into()))).await,
        ["4"]
    );
    assert!(
        summaries(&store, &project, by(&|f| f.connector = Some("none".into())))
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn every_filter_together_answers_what_each_alone_would_agree_on() {
    let (store, project, anchor) = seeded().await;
    let all = ActivityFilter {
        space: Some("air".into()),
        reach: Some(vec!["air".into()]),
        kinds: vec!["mcp.tool".into()],
        source: Some("gateway".into()),
        severity: Some("info".into()),
        since: Some(at(anchor, 10)),
        object: Some("endpoints/a".into()),
        connector: Some("c1".into()),
        limit: 10,
        cursor: None,
    };
    assert_eq!(summaries(&store, &project, all.clone()).await, ["0"]);
    let page = store.list(&project, &all).await.expect("a page");
    assert!(page.next.is_none());
    assert_eq!(page.items[0].details["connector"], "c1");
}

#[tokio::test]
async fn a_page_pages_on_by_its_cursor_and_a_cut_page_names_where_to_read_on() {
    let (store, project, _) = seeded().await;
    let mut f = ActivityFilter {
        limit: 2,
        ..filter()
    };
    let first = store.list(&project, &f).await.expect("a page");
    assert_eq!(first.items.len(), 2);
    let cursor = Cursor::decode(first.next.as_deref().expect("more")).expect("a cursor");
    f.cursor = Some(cursor);
    let second = store.list(&project, &f).await.expect("a page");
    let names: Vec<&str> = second.items.iter().map(|e| e.summary.as_str()).collect();
    assert_eq!(
        names,
        ["An event 2 minutes ago.", "An event 3 minutes ago."]
    );
    let cut = second.within(1);
    assert_eq!(cut.items.len(), 1);
    assert!(cut.next.is_some(), "the cut names where to read on");
    let whole = store.list(&project, &filter()).await.expect("a page");
    let last_cursor = whole.within(1_000_000);
    assert_eq!(last_cursor.items.len(), 5);
    assert!(
        last_cursor.next.is_none(),
        "nothing cut, nothing to read on"
    );
    let least = store
        .list(&project, &filter())
        .await
        .expect("a page")
        .within(0);
    assert_eq!(least.items.len(), 1, "at least one item always fits");
    assert!(least.next.is_some());
}

#[tokio::test]
async fn a_project_with_no_key_keeps_nothing_says_so_and_lists_nothing() {
    let pool = pool().await;
    let kept = format!("act-{}", unique());
    let unkeyed = format!("unkeyed-{}", unique());
    let mirror = common::keyed_mirror(&pool, &[&kept]).await;
    let store = ActivityStore::new(Some(pool), mirror);
    let anchor = TimestampTz::now().as_second();
    let ok = Seed {
        project: &kept,
        anchor,
    }
    .event(0, None, "config.applied", "reconciler", "info", Value::Null);
    let lost = Seed {
        project: &unkeyed,
        anchor,
    }
    .event(0, None, "config.applied", "reconciler", "info", Value::Null);
    let said = store
        .append(&[ok, lost])
        .await
        .expect_err("one project is unplaced");
    assert!(
        matches!(&said, ActivityError::Unplaced(names) if *names == unkeyed),
        "{said}"
    );
    assert!(said.to_string().contains("no active Organization"));
    assert_eq!(
        summaries(&store, &kept, filter()).await.len(),
        1,
        "the others are stored"
    );
    assert!(summaries(&store, &unkeyed, filter()).await.is_empty());
}

#[tokio::test]
async fn a_store_whose_database_is_down_answers_an_error_not_a_page() {
    let pool = pool().await;
    let project = format!("act-{}", unique());
    let mirror = common::keyed_mirror(&pool, &[&project]).await;
    let store = ActivityStore::new(Some(dead_pool()), mirror);
    let seed = Seed {
        project: &project,
        anchor: TimestampTz::now().as_second(),
    };
    let event = seed.event(0, None, "config.applied", "reconciler", "info", Value::Null);
    let said = store.append(&[event]).await.expect_err("down");
    assert!(matches!(said, ActivityError::Db(_)), "{said}");
    assert!(said.to_string().starts_with("database error: "));
    assert!(matches!(
        store.list(&project, &filter()).await,
        Err(ActivityError::Db(_))
    ));
    assert!(matches!(
        store.trim(TimestampTz::now()).await,
        Err(ActivityError::Db(_))
    ));
    let due = store
        .trim_if_due(TimestampTz::now())
        .await
        .expect("due once");
    assert!(due.is_err());
    assert!(store.trim_if_due(TimestampTz::now()).await.is_none());
}

#[tokio::test]
async fn a_list_that_fails_inside_a_keyed_transaction_is_a_database_error() {
    // A filter value the column cannot take: `details->>'object'` is text, but a NUL byte is not
    // text to PostgreSQL.
    let (store, project, _) = seeded().await;
    let f = ActivityFilter {
        object: Some("a\0b".into()),
        ..filter()
    };
    assert!(matches!(
        store.list(&project, &f).await,
        Err(ActivityError::Db(_))
    ));
}

#[test]
fn a_record_is_refused_for_each_thing_that_makes_it_unstorable() {
    let good = ActivityEvent {
        time: TimestampTz::now(),
        project: "helsinki".into(),
        space: None,
        kind: "config.applied".into(),
        source: "reconciler".into(),
        summary: "Applied.".into(),
        severity: "info".into(),
        correlation_id: None,
        details: Value::Null,
    };
    assert_eq!(good.validate(), Ok(()));
    let refused = |change: &dyn Fn(&mut ActivityEvent)| {
        let mut event = good.clone();
        change(&mut event);
        event.validate().expect_err("refused")
    };
    assert!(refused(&|e| e.project = "Not A Project".into()).contains("not a project name"));
    assert!(refused(&|e| e.kind = "made.up".into()).contains("not in the vocabulary"));
    assert!(refused(&|e| e.source = "somebody".into()).contains("not a known source"));
    assert!(refused(&|e| e.severity = "fatal".into()).contains("not a severity"));
    assert!(refused(&|e| e.summary = "  ".into()).contains("no summary"));
    assert!(refused(&|e| e.summary = "x".repeat(2_001)).contains("at most 2000 characters"));
    assert!(refused(&|e| e.correlation_id = Some("c".repeat(129))).contains("correlationId"));
    assert!(refused(&|e| e.details = json!([1, 2])).contains("object of named values"));
    assert!(refused(&|e| e.details = json!({ "k": "v".repeat(4_100) })).contains("4096 bytes"));
    let mut edge = good.clone();
    edge.summary = "x".repeat(2_000);
    edge.correlation_id = Some("c".repeat(128));
    edge.details = json!({ "k": "v" });
    assert_eq!(edge.validate(), Ok(()));
}

#[test]
fn a_cursor_that_is_not_one_decodes_to_nothing() {
    use base64::Engine as _;
    let encode = |text: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(text);
    assert!(Cursor::decode("%%%").is_none());
    assert!(Cursor::decode(&encode("no-separator")).is_none());
    assert!(Cursor::decode(&encode("12|x")).is_none());
    assert!(Cursor::decode(&encode("x|12")).is_none());
    assert!(Cursor::decode(&encode(&format!("{}|1", i128::MAX))).is_none());
    let cursor = Cursor {
        time: TimestampTz::from_second(1_789_000_000).expect("an instant"),
        id: 7,
    };
    assert_eq!(Cursor::decode(&cursor.encode()), Some(cursor));
}
