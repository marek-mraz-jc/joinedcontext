//! The `Store` contract, run against every store implementation: the memory store and the
//! Postgres store on the request role must answer every method the same way (C1, C14).

use super::*;
use dataroom_core::{DataroomError, RecordState, Store};
use odata_table::{query, QueryOptions, Scope};

fn names<T>(items: &[T], f: impl Fn(&T) -> String) -> Vec<String> {
    let mut v: Vec<String> = items.iter().map(f).collect();
    v.sort();
    v
}

async fn odata_ids(store: &dyn Store, ctx: &RequestCtx, set: &str) -> Vec<String> {
    let table = store.odata_table(ctx, set).await.expect("odata table");
    let ans = query(
        table.as_ref(),
        &QueryOptions::default(),
        &Scope::all(),
        1000,
    )
    .await
    .expect("odata query");
    let mut ids: Vec<String> = ans
        .rows
        .iter()
        .map(|r| r["id"].as_str().unwrap_or_default().to_owned())
        .collect();
    ids.sort();
    ids
}

/// Catalogue records: upsert, get, list, host routing and host claims.
pub async fn catalogues(store: &dyn Store) {
    let (org, cat) = tenant();
    let host = unique_host("cat");
    let mut c = catalogue(org, cat, "main", &host);
    store.upsert_catalogue(c.clone()).await.expect("create");
    assert_eq!(
        store.get_catalogue(org, cat).await.unwrap(),
        Some(c.clone())
    );

    c.title.insert("en".into(), "Renamed".into());
    store.upsert_catalogue(c.clone()).await.expect("update");
    let got = store.get_catalogue(org, cat).await.unwrap().unwrap();
    assert_eq!(got.title["en"], "Renamed");

    let (_, cat2) = tenant();
    store
        .upsert_catalogue(catalogue(org, cat2, "second", &unique_host("cat2")))
        .await
        .expect("second catalogue");
    let listed = store.list_catalogues(org).await.unwrap();
    assert_eq!(names(&listed, |c| c.name.clone()), ["main", "second"]);
    let (other_org, _) = tenant();
    assert!(store.list_catalogues(other_org).await.unwrap().is_empty());
    assert!(store.get_catalogue(other_org, cat).await.unwrap().is_none());

    // Host routing: exact, any case, with a port; unknown hosts resolve to nothing.
    for asked in [host.clone(), host.to_uppercase(), format!("{host}:8443")] {
        let found = store.resolve_host(&asked).await.unwrap();
        assert_eq!(found.map(|c| c.id), Some(cat), "host {asked}");
    }
    assert!(store
        .resolve_host("nobody.example.org")
        .await
        .unwrap()
        .is_none());

    // Another organization cannot claim the host, in any case.
    let (org_b, cat_b) = tenant();
    for claim in [host.clone(), host.to_uppercase()] {
        let err = store
            .upsert_catalogue(catalogue(org_b, cat_b, "thief", &claim))
            .await
            .expect_err("a second claim on a host is refused");
        assert!(
            matches!(err, DataroomError::Conflict(_)),
            "{claim}: {err:?}"
        );
    }
    assert_eq!(
        store.resolve_host(&host).await.unwrap().map(|c| c.org_id),
        Some(org)
    );
}

/// Publishers: defaults, lookups by id, name and IRI, conflicts, withdraw and merge (C16).
pub async fn publishers(store: &dyn Store) {
    let (org, cat) = tenant();
    store
        .upsert_catalogue(catalogue(org, cat, "pubs", &unique_host("pubs")))
        .await
        .unwrap();
    let ctx = member_ctx(org, cat);

    let city = store
        .upsert_publisher(&ctx, publisher("city"))
        .await
        .unwrap();
    assert_eq!(city.id, "pub-city");
    assert_eq!((city.org_id, city.catalogue_id), (org, cat));
    store
        .upsert_publisher(&ctx, publisher("region"))
        .await
        .unwrap();
    store
        .upsert_publisher(&ctx, publisher("dept"))
        .await
        .unwrap();

    let listed = store.list_publishers(&ctx).await.unwrap();
    assert_eq!(
        listed.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["city", "dept", "region"],
        "publishers are listed by name"
    );
    for key in ["pub-city", "city", "https://example.org/publisher/city"] {
        let got = store.get_publisher(&ctx, key).await.unwrap();
        assert_eq!(got.map(|p| p.id), Some("pub-city".to_owned()), "{key}");
    }
    assert!(store.get_publisher(&ctx, "nobody").await.unwrap().is_none());

    // A second publisher with a taken name is a conflict, not a storage failure.
    let mut dup = publisher("city");
    dup.id = "pub-other".into();
    let err = store.upsert_publisher(&ctx, dup).await.unwrap_err();
    assert!(matches!(err, DataroomError::Conflict(_)), "{err:?}");

    let withdrawn = store.withdraw_publisher(&ctx, "dept").await.unwrap();
    assert_eq!(withdrawn.state, RecordState::Withdrawn);
    assert_eq!(
        store
            .get_publisher(&ctx, "dept")
            .await
            .unwrap()
            .unwrap()
            .state,
        RecordState::Withdrawn
    );
    assert!(matches!(
        store.withdraw_publisher(&ctx, "nobody").await,
        Err(DataroomError::NotFound(_))
    ));

    // Merge: datasets move, the old name redirects to the target.
    store
        .upsert_dataset(&ctx, dataset("air", "region", "public"))
        .await
        .unwrap();
    store
        .upsert_dataset(&ctx, dataset("noise", "pub-region", "public"))
        .await
        .unwrap();
    store
        .upsert_dataset(&ctx, dataset("water", "city", "public"))
        .await
        .unwrap();
    let target = store
        .merge_publishers(&ctx, "region", "city")
        .await
        .unwrap();
    assert_eq!(target.name, "city");
    for ds in store.list_datasets(&ctx).await.unwrap() {
        assert_eq!(ds.publisher_id, "city", "{} moved to the target", ds.name);
    }
    assert_eq!(
        store
            .get_publisher(&ctx, "region")
            .await
            .unwrap()
            .unwrap()
            .name,
        "city",
        "the merged name redirects"
    );
    let source = store
        .list_publishers(&ctx)
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.name == "region")
        .unwrap();
    assert_eq!(source.merged_into.as_deref(), Some("city"));
    assert_eq!(source.state, RecordState::Withdrawn);

    // Merging a publisher into itself, or an unknown one, is refused.
    let err = store
        .merge_publishers(&ctx, "city", "city")
        .await
        .unwrap_err();
    assert!(matches!(err, DataroomError::Validation { .. }), "{err:?}");
    let err = store
        .merge_publishers(&ctx, "ghost", "city")
        .await
        .unwrap_err();
    assert!(matches!(err, DataroomError::NotFound(_)), "{err:?}");
    let err = store
        .merge_publishers(&ctx, "city", "ghost")
        .await
        .unwrap_err();
    assert!(matches!(err, DataroomError::NotFound(_)), "{err:?}");

    // Merging an already merged source does not touch the publisher it redirects to.
    store
        .upsert_publisher(&ctx, publisher("third"))
        .await
        .unwrap();
    store
        .merge_publishers(&ctx, "region", "third")
        .await
        .unwrap();
    for ds in store.list_datasets(&ctx).await.unwrap() {
        assert_eq!(ds.publisher_id, "city", "{} stays with city", ds.name);
    }
    let city = store
        .get_publisher(&ctx, "pub-city")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(city.state, RecordState::Active);
    assert!(city.merged_into.is_none());
}

/// Datasets, distributions and resource views.
pub async fn datasets(store: &dyn Store) {
    let (org, cat) = tenant();
    store
        .upsert_catalogue(catalogue(org, cat, "ds", &unique_host("ds")))
        .await
        .unwrap();
    let ctx = member_ctx(org, cat);
    store
        .upsert_publisher(&ctx, publisher("city"))
        .await
        .unwrap();

    let air = store
        .upsert_dataset(&ctx, dataset("air", "city", "public"))
        .await
        .unwrap();
    assert_eq!(air.id, "pkg-air");
    assert_eq!((air.org_id, air.catalogue_id), (org, cat));
    assert_eq!(air.distributions[0].id, "res-air-0");
    assert_eq!(air.distributions[0].dataset_id, "pkg-air");
    store
        .upsert_dataset(&ctx, dataset("budget", "city", "internal"))
        .await
        .unwrap();

    assert_eq!(
        store
            .list_datasets(&ctx)
            .await
            .unwrap()
            .iter()
            .map(|d| d.name.as_str())
            .collect::<Vec<_>>(),
        ["air", "budget"]
    );
    for key in ["pkg-air", "air"] {
        assert_eq!(
            store.get_dataset(&ctx, key).await.unwrap(),
            Some(air.clone()),
            "{key}"
        );
    }
    assert!(store.get_dataset(&ctx, "nothing").await.unwrap().is_none());

    // A second dataset on a taken URL name answers CKAN's field error.
    let mut dup = dataset("air", "city", "public");
    dup.id = "pkg-other".into();
    match store.upsert_dataset(&ctx, dup).await.unwrap_err() {
        DataroomError::Validation { fields, .. } => {
            assert_eq!(fields["name"], ["That URL is already in use."]);
        }
        other => panic!("expected a name field error, got {other:?}"),
    }

    // Distributions: lookup, replacement on update.
    let (ds, dist) = store
        .get_distribution(&ctx, "res-air-0")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (ds.name.as_str(), dist.format.as_deref()),
        ("air", Some("CSV"))
    );
    assert!(store
        .get_distribution(&ctx, "res-none")
        .await
        .unwrap()
        .is_none());
    let mut updated = air.clone();
    updated.distributions[0].id = "res-air-json".into();
    updated.distributions[0].format = Some("JSON".into());
    store.upsert_dataset(&ctx, updated).await.unwrap();
    assert!(store
        .get_distribution(&ctx, "res-air-0")
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store
            .get_distribution(&ctx, "res-air-json")
            .await
            .unwrap()
            .unwrap()
            .1
            .format
            .as_deref(),
        Some("JSON")
    );

    // Resource views.
    store
        .upsert_resource_view(&ctx, view("v2", "res-air-json", 2))
        .await
        .unwrap();
    store
        .upsert_resource_view(&ctx, view("v1", "res-air-json", 1))
        .await
        .unwrap();
    let generated = store
        .upsert_resource_view(&ctx, view("", "res-air-json", 3))
        .await
        .unwrap();
    assert!(generated.id.starts_with("view-"), "{}", generated.id);
    let listed = store
        .list_resource_views(&ctx, "res-air-json")
        .await
        .unwrap();
    assert_eq!(
        listed.iter().map(|v| v.order).collect::<Vec<_>>(),
        [1, 2, 3],
        "views are listed by order"
    );
    assert_eq!(
        store
            .get_resource_view(&ctx, "v1")
            .await
            .unwrap()
            .unwrap()
            .title,
        "View v1"
    );
    store.delete_resource_view(&ctx, "v2").await.unwrap();
    assert!(store.get_resource_view(&ctx, "v2").await.unwrap().is_none());
    assert!(matches!(
        store.delete_resource_view(&ctx, "v2").await,
        Err(DataroomError::NotFound(_))
    ));

    // Withdraw keeps the record; purge removes it, its distributions and their views.
    let w = store.withdraw_dataset(&ctx, "air").await.unwrap();
    assert_eq!(w.state, RecordState::Withdrawn);
    assert_eq!(
        store.get_dataset(&ctx, "air").await.unwrap().unwrap().state,
        RecordState::Withdrawn
    );
    assert!(matches!(
        store.withdraw_dataset(&ctx, "nothing").await,
        Err(DataroomError::NotFound(_))
    ));
    store.purge_dataset(&ctx, "air").await.unwrap();
    assert!(store.get_dataset(&ctx, "air").await.unwrap().is_none());
    assert!(store
        .get_distribution(&ctx, "res-air-json")
        .await
        .unwrap()
        .is_none());
    assert!(store
        .list_resource_views(&ctx, "res-air-json")
        .await
        .unwrap()
        .is_empty());
    assert!(store.get_resource_view(&ctx, "v1").await.unwrap().is_none());
    assert!(matches!(
        store.purge_dataset(&ctx, "air").await,
        Err(DataroomError::NotFound(_))
    ));
}

/// Harvest sources and runs, activity, SHACL reports, profiles, pages and plugins.
/// One Running run per source: two claims at once, one wins; a finished run frees the source;
/// a Running run older than the deadline is marked failed and frees it too.
pub async fn harvest_lock(store: &dyn Store) {
    let (org, cat) = tenant();
    store
        .upsert_catalogue(catalogue(org, cat, "lock", &unique_host("lock")))
        .await
        .unwrap();
    let ctx = member_ctx(org, cat);
    let running = |id: &str, started: &str| {
        let mut r = harvest_run(id, "hs-nkod", started);
        r.state = dataroom_core::HarvestRunState::Running;
        r.finished_at = None;
        r
    };
    let stale_before = chrono::DateTime::parse_from_rfc3339("2026-11-09T02:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let (a, b) = tokio::join!(
        store.claim_harvest_run(&ctx, running("run-a", "2026-11-09T03:23:00Z"), stale_before),
        store.claim_harvest_run(&ctx, running("run-b", "2026-11-09T03:23:00Z"), stale_before),
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert!(a ^ b, "exactly one claim wins: {a} {b}");
    let winner = if a { "run-a" } else { "run-b" };
    assert_eq!(
        store
            .list_harvest_runs(&ctx, Some("hs-nkod"))
            .await
            .unwrap()
            .len(),
        1
    );
    // Another source is not held by it.
    let mut other = running("run-o", "2026-11-09T03:23:00Z");
    other.source_id = "hs-arcgis".into();
    assert!(store
        .claim_harvest_run(&ctx, other, stale_before)
        .await
        .unwrap());

    // Finished: the source is free.
    let mut done = store.get_harvest_run(&ctx, winner).await.unwrap().unwrap();
    done.state = dataroom_core::HarvestRunState::Succeeded;
    store.upsert_harvest_run(&ctx, done).await.unwrap();
    assert!(store
        .claim_harvest_run(&ctx, running("run-c", "2026-11-09T03:24:00Z"), stale_before)
        .await
        .unwrap());

    // run-c is left Running; a claim whose deadline has passed it abandons run-c.
    let later = chrono::DateTime::parse_from_rfc3339("2026-11-09T05:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    assert!(store
        .claim_harvest_run(&ctx, running("run-d", "2026-11-09T06:00:00Z"), later)
        .await
        .unwrap());
    let c = store.get_harvest_run(&ctx, "run-c").await.unwrap().unwrap();
    assert_eq!(c.state, dataroom_core::HarvestRunState::Failed);
    assert!(c.error.unwrap().contains("abandoned"));
}

pub async fn records(store: &dyn Store) {
    let (org, cat) = tenant();
    store
        .upsert_catalogue(catalogue(org, cat, "rec", &unique_host("rec")))
        .await
        .unwrap();
    let ctx = member_ctx(org, cat);

    let hs = store
        .upsert_harvest_source(&ctx, harvest_source("nkod"))
        .await
        .unwrap();
    assert_eq!(hs.id, "hs-nkod");
    store
        .upsert_harvest_source(&ctx, harvest_source("arcgis"))
        .await
        .unwrap();
    let listed = store.list_harvest_sources(&ctx).await.unwrap();
    assert_eq!(names(&listed, |s| s.name.clone()), ["arcgis", "nkod"]);
    for key in ["hs-nkod", "nkod"] {
        assert_eq!(
            store
                .get_harvest_source(&ctx, key)
                .await
                .unwrap()
                .map(|s| s.url),
            Some("https://harvest.example.org/nkod.ttl".to_owned())
        );
    }
    assert!(store
        .get_harvest_source(&ctx, "none")
        .await
        .unwrap()
        .is_none());
    let mut disabled = hs.clone();
    disabled.enabled = false;
    store.upsert_harvest_source(&ctx, disabled).await.unwrap();
    assert!(
        !store
            .get_harvest_source(&ctx, "nkod")
            .await
            .unwrap()
            .unwrap()
            .enabled
    );

    store
        .upsert_harvest_run(
            &ctx,
            harvest_run("run-1", "hs-nkod", "2026-11-01T10:00:00Z"),
        )
        .await
        .unwrap();
    store
        .upsert_harvest_run(
            &ctx,
            harvest_run("run-2", "hs-nkod", "2026-11-03T10:00:00Z"),
        )
        .await
        .unwrap();
    store
        .upsert_harvest_run(
            &ctx,
            harvest_run("run-3", "hs-arcgis", "2026-11-02T10:00:00Z"),
        )
        .await
        .unwrap();
    let all = store.list_harvest_runs(&ctx, None).await.unwrap();
    assert_eq!(
        all.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ["run-2", "run-3", "run-1"],
        "runs are listed newest first"
    );
    let nkod = store
        .list_harvest_runs(&ctx, Some("hs-nkod"))
        .await
        .unwrap();
    assert_eq!(
        nkod.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ["run-2", "run-1"]
    );
    let mut failed = harvest_run("run-1", "hs-nkod", "2026-11-01T10:00:00Z");
    failed.state = dataroom_core::HarvestRunState::Failed;
    failed.error = Some("timeout".into());
    store.upsert_harvest_run(&ctx, failed).await.unwrap();
    let got = store.get_harvest_run(&ctx, "run-1").await.unwrap().unwrap();
    assert_eq!(got.error.as_deref(), Some("timeout"));
    assert_eq!((got.org_id, got.catalogue_id), (org, cat));
    assert!(store
        .get_harvest_run(&ctx, "run-9")
        .await
        .unwrap()
        .is_none());

    for (id, obj, at) in [
        ("a1", "pkg-air", "2026-11-01T10:00:00Z"),
        ("a2", "pkg-noise", "2026-11-02T10:00:00Z"),
        ("a3", "pkg-air", "2026-11-03T10:00:00Z"),
    ] {
        store
            .record_activity(&ctx, activity(id, obj, at))
            .await
            .unwrap();
    }
    let rows = store.list_activity(&ctx, None, 10).await.unwrap();
    assert_eq!(
        rows.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
        ["a3", "a2", "a1"],
        "activity is listed newest first"
    );
    assert_eq!(rows[0].org_id, org);
    let air = store
        .list_activity(&ctx, Some("pkg-air"), 10)
        .await
        .unwrap();
    assert_eq!(
        air.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
        ["a3", "a1"]
    );
    assert_eq!(store.list_activity(&ctx, None, 1).await.unwrap().len(), 1);
    assert_eq!(
        store
            .list_activity(&ctx, None, usize::MAX)
            .await
            .unwrap()
            .len(),
        3,
        "an unbounded limit lists everything"
    );

    let rep = store
        .upsert_shacl_report(&ctx, shacl_report("pkg-air", "dcat-ap-3.0.1", false))
        .await
        .unwrap();
    assert_eq!(rep.id, "pkg-air:dcat-ap-3.0.1:1");
    store
        .upsert_shacl_report(&ctx, shacl_report("pkg-air", "dcat-ap-sk-2.0", true))
        .await
        .unwrap();
    store
        .upsert_shacl_report(&ctx, shacl_report("pkg-noise", "dcat-ap-3.0.1", true))
        .await
        .unwrap();
    store
        .upsert_shacl_report(&ctx, shacl_report("pkg-air", "dcat-ap-3.0.1", true))
        .await
        .unwrap();
    let all = store.list_shacl_reports(&ctx, None, None).await.unwrap();
    assert_eq!(all.len(), 3);
    let air = store
        .list_shacl_reports(&ctx, Some("pkg-air"), None)
        .await
        .unwrap();
    assert_eq!(air.len(), 2);
    let eu = store
        .list_shacl_reports(&ctx, Some("pkg-air"), Some("dcat-ap-3.0.1"))
        .await
        .unwrap();
    assert_eq!(eu.len(), 1);
    assert!(eu[0].conforms, "the second upsert replaced the report");
    assert_eq!(
        store
            .list_shacl_reports(&ctx, None, Some("dcat-ap-3.0.1"))
            .await
            .unwrap()
            .len(),
        2
    );

    store
        .upsert_profile(&ctx, profile("city-ext", "2"))
        .await
        .unwrap();
    store
        .upsert_profile(&ctx, profile("city-ext", "1"))
        .await
        .unwrap();
    let mut changed = profile("city-ext", "1");
    changed.shapes_turtle = "@prefix sh: <http://www.w3.org/ns/shacl#> .".into();
    store.upsert_profile(&ctx, changed).await.unwrap();
    let profiles = store.list_profiles(&ctx).await.unwrap();
    assert_eq!(
        profiles
            .iter()
            .map(|p| format!("{}@{}", p.id, p.version))
            .collect::<Vec<_>>(),
        ["city-ext@1", "city-ext@2"]
    );
    assert!(profiles[0].shapes_turtle.starts_with("@prefix"));

    store.upsert_page(&ctx, page("about", "sk")).await.unwrap();
    store.upsert_page(&ctx, page("about", "en")).await.unwrap();
    let mut about = page("about", "sk");
    about.markdown = "# Ahoj".into();
    let saved = store.upsert_page(&ctx, about).await.unwrap();
    assert_eq!((saved.org_id, saved.catalogue_id), (org, cat));
    let pages = store.list_pages(&ctx).await.unwrap();
    assert_eq!(
        pages
            .iter()
            .map(|p| format!("{}/{}", p.slug, p.lang))
            .collect::<Vec<_>>(),
        ["about/en", "about/sk"]
    );
    assert_eq!(pages[1].markdown, "# Ahoj");

    store.upsert_plugin(&ctx, plugin("maps")).await.unwrap();
    let mut off = plugin("maps");
    off.enabled = false;
    store.upsert_plugin(&ctx, off).await.unwrap();
    store.upsert_plugin(&ctx, plugin("charts")).await.unwrap();
    let plugins = store.list_plugins(&ctx).await.unwrap();
    assert_eq!(
        plugins.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["charts", "maps"]
    );
    assert!(!plugins[1].enabled);
}

/// OData entity sets: each set answers the request's catalogue rows only (C14).
pub async fn odata(store: &dyn Store) {
    let (org, cat) = tenant();
    let (org_b, cat_b) = tenant();
    let ctx = member_ctx(org, cat);
    let ctx_b = member_ctx(org_b, cat_b);
    for (c, o, k, n) in [(&ctx, org, cat, "oda"), (&ctx_b, org_b, cat_b, "odb")] {
        store
            .upsert_catalogue(catalogue(o, k, n, &unique_host(n)))
            .await
            .unwrap();
        store.upsert_publisher(c, publisher(n)).await.unwrap();
        store
            .upsert_dataset(c, dataset(&format!("{n}-air"), n, "public"))
            .await
            .unwrap();
        store
            .upsert_harvest_source(c, harvest_source(n))
            .await
            .unwrap();
        store
            .upsert_harvest_run(
                c,
                harvest_run(&format!("{n}-run"), n, "2026-11-01T10:00:00Z"),
            )
            .await
            .unwrap();
        store
            .record_activity(
                c,
                activity(&format!("{n}-act"), "x", "2026-11-01T10:00:00Z"),
            )
            .await
            .unwrap();
        store
            .upsert_shacl_report(c, shacl_report(&format!("pkg-{n}-air"), "eu", true))
            .await
            .unwrap();
    }
    // A second catalogue of the same organization stays out of the first one's sets.
    let (_, cat_a2) = tenant();
    let ctx_a2 = member_ctx(org, cat_a2);
    store
        .upsert_catalogue(catalogue(org, cat_a2, "oda2", &unique_host("oda2")))
        .await
        .unwrap();
    store
        .upsert_dataset(&ctx_a2, dataset("oda2-air", "x", "public"))
        .await
        .unwrap();

    assert_eq!(odata_ids(store, &ctx, "Catalogues").await.len(), 2);
    assert!(odata_ids(store, &ctx, "Catalogues")
        .await
        .contains(&cat.to_string()));
    assert_eq!(odata_ids(store, &ctx, "Datasets").await, ["pkg-oda-air"]);
    assert_eq!(
        odata_ids(store, &ctx, "Distributions").await,
        ["res-oda-air-0"]
    );
    assert_eq!(odata_ids(store, &ctx, "Publishers").await, ["pub-oda"]);
    assert_eq!(odata_ids(store, &ctx, "HarvestSources").await, ["hs-oda"]);
    assert_eq!(odata_ids(store, &ctx, "HarvestRuns").await, ["oda-run"]);
    assert_eq!(odata_ids(store, &ctx, "ActivityRows").await, ["oda-act"]);
    assert_eq!(
        odata_ids(store, &ctx, "ShaclResults").await,
        ["pkg-oda-air:eu:1"]
    );
    assert_eq!(odata_ids(store, &ctx_b, "Datasets").await, ["pkg-odb-air"]);
    assert_eq!(
        odata_ids(store, &ctx_a2, "Datasets").await,
        ["pkg-oda2-air"]
    );

    // Rows carry the same fields in every store.
    let table = store.odata_table(&ctx, "Datasets").await.unwrap();
    let ans = query(table.as_ref(), &QueryOptions::default(), &Scope::all(), 10)
        .await
        .unwrap();
    let row = &ans.rows[0];
    assert_eq!(row["org_id"], org.to_string());
    assert_eq!(row["catalogue_id"], cat.to_string());
    assert_eq!(
        row["title"], "Dataset oda-air",
        "title in the catalogue default language"
    );
    assert_eq!(row["visibility"], "public");

    assert!(matches!(
        store.odata_table(&ctx, "Nope").await,
        Err(DataroomError::NotFound(_))
    ));
}

/// Two organizations with the same names: neither sees the other's records by any route.
pub async fn isolation(store: &dyn Store) {
    let (org_a, cat_a) = tenant();
    let (org_b, cat_b) = tenant();
    let a = member_ctx(org_a, cat_a);
    let b = member_ctx(org_b, cat_b);
    for (c, o, k) in [(&a, org_a, cat_a), (&b, org_b, cat_b)] {
        store
            .upsert_catalogue(catalogue(o, k, "same", &unique_host("iso")))
            .await
            .unwrap();
        store.upsert_publisher(c, publisher("city")).await.unwrap();
        store
            .upsert_dataset(c, dataset("air", "city", "internal"))
            .await
            .unwrap();
        store
            .upsert_resource_view(c, view("v1", "res-air-0", 1))
            .await
            .unwrap();
        store
            .upsert_harvest_source(c, harvest_source("nkod"))
            .await
            .unwrap();
        store
            .upsert_harvest_run(c, harvest_run("run-1", "hs-nkod", "2026-11-01T10:00:00Z"))
            .await
            .unwrap();
        store
            .record_activity(c, activity("act-1", "pkg-air", "2026-11-01T10:00:00Z"))
            .await
            .unwrap();
        store
            .upsert_shacl_report(c, shacl_report("pkg-air", "eu", true))
            .await
            .unwrap();
        store.upsert_profile(c, profile("p", "1")).await.unwrap();
        store.upsert_page(c, page("about", "sk")).await.unwrap();
        store.upsert_plugin(c, plugin("maps")).await.unwrap();
    }
    // B changes every record; A's copies stay as they were.
    let mut ds_b = store.get_dataset(&b, "air").await.unwrap().unwrap();
    ds_b.title.insert("sk".into(), "B only".into());
    store.upsert_dataset(&b, ds_b).await.unwrap();
    store.withdraw_publisher(&b, "city").await.unwrap();
    store.delete_resource_view(&b, "v1").await.unwrap();
    store.purge_dataset(&b, "air").await.unwrap();

    let ds_a = store.get_dataset(&a, "air").await.unwrap().unwrap();
    assert_eq!(ds_a.title["sk"], "Dataset air");
    assert_eq!(ds_a.org_id, org_a);
    assert_eq!(
        store
            .get_publisher(&a, "city")
            .await
            .unwrap()
            .unwrap()
            .state,
        RecordState::Active
    );
    assert!(store.get_resource_view(&a, "v1").await.unwrap().is_some());
    assert!(store
        .get_distribution(&a, "res-air-0")
        .await
        .unwrap()
        .is_some());
    assert!(store.get_dataset(&b, "air").await.unwrap().is_none());

    for c in [&a, &b] {
        assert_eq!(store.list_publishers(c).await.unwrap().len(), 1);
        assert_eq!(store.list_harvest_sources(c).await.unwrap().len(), 1);
        assert_eq!(store.list_harvest_runs(c, None).await.unwrap().len(), 1);
        assert_eq!(store.list_activity(c, None, 100).await.unwrap().len(), 1);
        assert_eq!(
            store.list_shacl_reports(c, None, None).await.unwrap().len(),
            1
        );
        assert_eq!(store.list_profiles(c).await.unwrap().len(), 1);
        assert_eq!(store.list_pages(c).await.unwrap().len(), 1);
        assert_eq!(store.list_plugins(c).await.unwrap().len(), 1);
    }
    assert_eq!(store.list_datasets(&a).await.unwrap().len(), 1);
    assert!(store.list_datasets(&b).await.unwrap().is_empty());

    // A context naming B's catalogue under A's organization reaches nothing.
    let crossed = member_ctx(org_a, cat_b);
    assert!(store.list_publishers(&crossed).await.unwrap().is_empty());
    assert!(store
        .get_publisher(&crossed, "city")
        .await
        .unwrap()
        .is_none());
    assert!(store
        .list_harvest_sources(&crossed)
        .await
        .unwrap()
        .is_empty());
    assert!(store.get_catalogue(org_a, cat_b).await.unwrap().is_none());
}

/// A deleted catalogue takes no more writes (a harvest run still going when it was deleted
/// cannot bring rows back), a re-create starts empty, and its id belongs to its Organization
/// alone, tombstone included (ODP-09, ODP-12).
pub async fn deleted_catalogues(store: &dyn Store) {
    let (org, cat) = tenant();
    let (other_org, _) = tenant();
    let host = unique_host("gone");
    store
        .upsert_catalogue(catalogue(org, cat, "gone", &host))
        .await
        .unwrap();
    let ctx = member_ctx(org, cat);
    store
        .upsert_publisher(&ctx, publisher("city"))
        .await
        .unwrap();
    store
        .upsert_dataset(&ctx, dataset("air", "city", "public"))
        .await
        .unwrap();
    assert!(store.delete_catalogue(org, cat).await.unwrap());

    let gone = |r: Result<(), DataroomError>, what: &str| {
        assert!(matches!(r, Err(DataroomError::Gone(_))), "{what}: {r:?}");
    };
    gone(
        store
            .upsert_dataset(&ctx, dataset("late", "city", "public"))
            .await
            .map(drop),
        "dataset",
    );
    gone(
        store
            .upsert_harvest_run(
                &ctx,
                harvest_run("run-9", "hs-nkod", "2026-11-14T10:00:00Z"),
            )
            .await
            .map(drop),
        "run",
    );
    gone(
        store
            .upsert_harvest_source(&ctx, harvest_source("nkod"))
            .await
            .map(drop),
        "source",
    );
    let stale_before = chrono::Utc::now();
    gone(
        store
            .claim_harvest_run(
                &ctx,
                harvest_run("run-10", "hs-nkod", "2026-11-14T10:00:00Z"),
                stale_before,
            )
            .await
            .map(drop),
        "claim",
    );

    let theirs = catalogue(other_org, cat, "taken", &unique_host("taken"));
    assert!(
        matches!(
            store.upsert_catalogue(theirs).await,
            Err(DataroomError::Conflict(_))
        ),
        "another Organization cannot take a tombstoned id"
    );

    store
        .upsert_catalogue(catalogue(org, cat, "gone", &host))
        .await
        .expect("the owner re-creates it");
    assert!(store.list_datasets(&ctx).await.unwrap().is_empty());
    store
        .upsert_publisher(&ctx, publisher("city"))
        .await
        .unwrap();
    store
        .upsert_dataset(&ctx, dataset("air", "city", "public"))
        .await
        .expect("a re-created catalogue takes writes");
    let theirs = catalogue(other_org, cat, "taken", &unique_host("taken"));
    assert!(matches!(
        store.upsert_catalogue(theirs).await,
        Err(DataroomError::Conflict(_))
    ));
}
