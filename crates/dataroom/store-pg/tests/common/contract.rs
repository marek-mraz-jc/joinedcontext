//! The `Store` contract, run against every store implementation: the memory store and the
//! Postgres store on the request role must answer every method the same way (C1, C14).

use super::*;
use dataroom_core::{DataroomError, RecordState, RequestCtx, Store};
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

    // A declaration writes its own members and no other (T-7520).
    let names_in = |langs: &[&str]| {
        langs
            .iter()
            .map(|lang| ((*lang).to_owned(), "Kraj".to_owned()))
            .collect::<dataroom_core::Texts>()
    };
    let declared = |langs: &[&str]| dataroom_core::CatalogueDeclaration {
        publisher_uri: Some(Some("https://example.sk/id/legal-subject/37828100".into())),
        publisher_name: Some(names_in(langs)),
        ..Default::default()
    };
    let wide = declared(&["en", "sk"]);
    let written = store
        .declare_catalogue(org, cat, &wide)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(Some(written.publisher_uri), wide.publisher_uri);
    assert_eq!(written.title["en"], "Renamed");
    let again = store
        .declare_catalogue(org, cat, &wide)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again.modified, written.modified, "no difference, no write");
    let none = dataroom_core::CatalogueDeclaration {
        publisher_uri: Some(None),
        ..Default::default()
    };
    let cleared = store
        .declare_catalogue(org, cat, &none)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cleared.publisher_uri, None, "declared as none clears");
    let same = store
        .declare_catalogue(org, cat, &none)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(same.modified, cleared.modified, "no IRI twice, no write");
    let narrow = declared(&["sk"]);
    let narrowed = store
        .declare_catalogue(org, cat, &narrow)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        narrowed.publisher_name,
        Some(names_in(&["sk"])),
        "a dropped language goes"
    );

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
    // Withdrawing the merged name touches that record, never the target it redirects to.
    let w = store.withdraw_publisher(&ctx, "region").await.unwrap();
    assert_eq!(w.name, "region");
    let city = store.get_publisher(&ctx, "city").await.unwrap().unwrap();
    assert_eq!(
        city.state,
        RecordState::Active,
        "the merge target stays active"
    );

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
    store
        .upsert_shacl_report(&ctx, shacl_report("pkg-air", "dcat-ap-3.0.1", false))
        .await
        .unwrap();
    store.purge_dataset(&ctx, "air").await.unwrap();
    assert!(store.get_dataset(&ctx, "air").await.unwrap().is_none());
    assert!(
        store
            .list_shacl_reports(&ctx, Some("pkg-air"), None)
            .await
            .unwrap()
            .is_empty(),
        "a purge removes the dataset's SHACL reports"
    );
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
    let stale_before = ts("2026-11-09T02:00:00Z");
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
    let later = ts("2026-11-09T05:00:00Z");
    assert!(store
        .claim_harvest_run(&ctx, running("run-d", "2026-11-09T06:00:00Z"), later)
        .await
        .unwrap());
    let c = store.get_harvest_run(&ctx, "run-c").await.unwrap().unwrap();
    assert_eq!(c.state, dataroom_core::HarvestRunState::Failed);
    assert!(c.error.unwrap().contains("abandoned"));
}

/// The answers no other case reaches: an unknown catalogue or source, a refused save that
/// records no activity, a stale save of a dataset named only by its slug, a merge redirect to
/// a publisher that is not there, and a report keeping the id it came with.
pub async fn edges(store: &dyn Store) {
    let (org, cat) = tenant();
    assert!(!store.delete_catalogue(org, cat).await.unwrap());
    let host = unique_host("edges");
    store
        .upsert_catalogue(catalogue(org, cat, "edges", &host))
        .await
        .unwrap();
    let ctx = member_ctx(org, cat);

    // Another catalogue's host: refused, and no activity row is left behind.
    let (org2, cat2) = tenant();
    let taken = store
        .save_catalogue(
            catalogue(org2, cat2, "thief", &host),
            None,
            activity("act-cat", "", "2026-11-09T10:00:00Z"),
        )
        .await;
    assert!(
        matches!(taken, Err(DataroomError::Conflict(_))),
        "{taken:?}"
    );
    assert!(store
        .list_activity(&member_ctx(org2, cat2), None, 10)
        .await
        .unwrap()
        .is_empty());

    store
        .upsert_publisher(&ctx, publisher("city"))
        .await
        .unwrap();
    store
        .upsert_dataset(&ctx, dataset("air", "city", "public"))
        .await
        .unwrap();
    // A dataset named only by its slug and read at another time: changed since read.
    let mut stale = dataset("air", "city", "public");
    stale.id.clear();
    let refused = store
        .save_dataset(
            &ctx,
            stale,
            Some(ts("2020-01-01T00:00:00Z")),
            activity("act-1", "", "2026-11-09T10:00:00Z"),
        )
        .await;
    assert!(
        matches!(&refused, Err(DataroomError::Conflict(m)) if m.contains("'air' was changed")),
        "{refused:?}"
    );
    // A second dataset under a slug in use: refused, no activity row.
    let mut twin = dataset("air", "city", "public");
    twin.id = "pkg-twin".into();
    let refused = store
        .save_dataset(
            &ctx,
            twin,
            None,
            activity("act-2", "", "2026-11-09T10:00:00Z"),
        )
        .await;
    assert!(refused.is_err(), "{refused:?}");
    assert!(store
        .list_activity(&ctx, None, 10)
        .await
        .unwrap()
        .is_empty());

    // A redirect to a publisher that is not there answers the publisher itself.
    let mut moved = publisher("moved");
    moved.merged_into = Some("pub-ghost".into());
    store.upsert_publisher(&ctx, moved).await.unwrap();
    let got = store.get_publisher(&ctx, "moved").await.unwrap().unwrap();
    assert_eq!(
        (got.name.as_str(), got.merged_into.as_deref()),
        ("moved", Some("pub-ghost"))
    );

    // A merge into a publisher merged before lands on the one it was merged into.
    for name in ["old", "new", "late"] {
        store.upsert_publisher(&ctx, publisher(name)).await.unwrap();
    }
    store
        .upsert_dataset(&ctx, dataset("noise", "late", "public"))
        .await
        .unwrap();
    store.merge_publishers(&ctx, "old", "new").await.unwrap();
    let target = store.merge_publishers(&ctx, "late", "old").await.unwrap();
    assert_eq!(target.name, "new");
    let late = store
        .list_publishers(&ctx)
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.name == "late")
        .unwrap();
    assert_eq!(late.merged_into.as_deref(), Some("new"));
    let noise = store.get_dataset(&ctx, "noise").await.unwrap().unwrap();
    assert_eq!(noise.publisher_id, "new");

    // A save naming its own activity object keeps it.
    let saved = store
        .save_dataset(
            &ctx,
            dataset("water", "city", "public"),
            None,
            activity("act-water", "water-object", "2026-11-09T10:00:00Z"),
        )
        .await
        .unwrap();
    assert_eq!(saved.id, "pkg-water");
    let held = store.get_catalogue(org, cat).await.unwrap().unwrap();
    store
        .save_catalogue(
            held.clone(),
            Some(held.modified),
            activity("act-cat", "catalogue-object", "2026-11-09T10:00:00Z"),
        )
        .await
        .unwrap();
    for object in ["water-object", "catalogue-object"] {
        let rows = store.list_activity(&ctx, Some(object), 10).await.unwrap();
        assert_eq!(rows.len(), 1, "{object}");
    }

    // A resource id another dataset holds: refused.
    let mut copy = dataset("copy", "city", "public");
    copy.distributions[0].id = saved.distributions[0].id.clone();
    let refused = store.upsert_dataset(&ctx, copy).await;
    assert!(refused.is_err(), "{refused:?}");
    assert!(store.get_dataset(&ctx, "copy").await.unwrap().is_none());

    let mut report = shacl_report("pkg-air", "dcat-ap-3.0.1", true);
    report.id = "report-kept".into();
    let stored = store.upsert_shacl_report(&ctx, report).await.unwrap();
    assert_eq!(stored.id, "report-kept");

    // A harvest source goes with its runs; another source's runs stay; a second delete finds
    // nothing.
    store
        .upsert_harvest_source(&ctx, harvest_source("nkod"))
        .await
        .unwrap();
    store
        .upsert_harvest_source(&ctx, harvest_source("arcgis"))
        .await
        .unwrap();
    for (id, source) in [("run-n", "hs-nkod"), ("run-a", "hs-arcgis")] {
        store
            .upsert_harvest_run(&ctx, harvest_run(id, source, "2026-11-09T03:00:00Z"))
            .await
            .unwrap();
    }
    assert!(!store.delete_harvest_source(&ctx, "nobody").await.unwrap());
    assert!(store.delete_harvest_source(&ctx, "nkod").await.unwrap());
    assert!(store
        .get_harvest_source(&ctx, "hs-nkod")
        .await
        .unwrap()
        .is_none());
    let runs = store.list_harvest_runs(&ctx, None).await.unwrap();
    assert_eq!(names(&runs, |r| r.id.clone()), ["run-a"]);
    assert!(!store.delete_harvest_source(&ctx, "hs-nkod").await.unwrap());
    assert!(store
        .get_harvest_source(&ctx, "arcgis")
        .await
        .unwrap()
        .is_some());

    // Deleting the catalogue empties every kind of row it held and no other catalogue's.
    let (_, cat_b) = tenant();
    store
        .upsert_catalogue(catalogue(org, cat_b, "edges-b", &unique_host("edges-b")))
        .await
        .unwrap();
    let ctx_b = member_ctx(org, cat_b);
    store
        .upsert_publisher(&ctx_b, publisher("kept"))
        .await
        .unwrap();
    // The other catalogue holds a row of every kind, and its host is its own in the
    // Organization too.
    let b = store
        .upsert_dataset(&ctx_b, dataset("b-data", "kept", "public"))
        .await
        .unwrap();
    let b_resource = b.distributions[0].id.clone();
    store
        .upsert_resource_view(&ctx_b, view("view-b", &b_resource, 1))
        .await
        .unwrap();
    store
        .upsert_harvest_source(&ctx_b, harvest_source("b-src"))
        .await
        .unwrap();
    store
        .record_activity(
            &ctx_b,
            activity("act-b", "b-object", "2026-11-09T10:00:00Z"),
        )
        .await
        .unwrap();
    store
        .upsert_shacl_report(&ctx_b, shacl_report("pkg-b-data", "eu", true))
        .await
        .unwrap();
    store
        .upsert_profile(&ctx_b, profile("b-profile", "1.0"))
        .await
        .unwrap();
    store
        .upsert_page(&ctx_b, page("b-about", "sk"))
        .await
        .unwrap();
    store.upsert_plugin(&ctx_b, plugin("b-map")).await.unwrap();
    let b_host = store.get_catalogue(org, cat_b).await.unwrap().unwrap().host;
    let (_, cat_c) = tenant();
    let same_host = store
        .upsert_catalogue(catalogue(org, cat_c, "edges-c", &b_host))
        .await;
    assert!(
        matches!(same_host, Err(DataroomError::Conflict(_))),
        "{same_host:?}"
    );
    let resource = saved.distributions[0].id.clone();
    store
        .upsert_resource_view(&ctx, view("view-1", &resource, 1))
        .await
        .unwrap();
    store
        .upsert_profile(&ctx, profile("local", "1.0"))
        .await
        .unwrap();
    store.upsert_page(&ctx, page("about", "sk")).await.unwrap();
    store.upsert_plugin(&ctx, plugin("map")).await.unwrap();
    assert!(store.delete_catalogue(org, cat).await.unwrap());
    let gone = store.get_catalogue(org, cat).await.unwrap().unwrap();
    assert!(gone.deleted.is_some());
    assert_eq!(
        names(&store.list_publishers(&ctx_b).await.unwrap(), |p| p
            .name
            .clone()),
        ["kept"]
    );
    assert!(store.list_publishers(&ctx).await.unwrap().is_empty());
    assert!(store
        .list_harvest_runs(&ctx, None)
        .await
        .unwrap()
        .is_empty());
    let profiles = store.list_profiles(&ctx_b).await.unwrap();
    let kept = [
        store
            .list_resource_views(&ctx_b, &b_resource)
            .await
            .unwrap()
            .len(),
        store.list_harvest_sources(&ctx_b).await.unwrap().len(),
        store
            .list_activity(&ctx_b, Some("b-object"), 10)
            .await
            .unwrap()
            .len(),
        store
            .list_shacl_reports(&ctx_b, None, None)
            .await
            .unwrap()
            .len(),
        profiles.iter().filter(|p| p.id == "b-profile").count(),
        store.list_pages(&ctx_b).await.unwrap().len(),
        store.list_plugins(&ctx_b).await.unwrap().len(),
    ];
    assert_eq!(
        kept, [1; 7],
        "views, sources, activity, reports, profiles, pages, plugins"
    );
}

/// Two catalogues of one Organization holding the same dataset slug, distribution id, view
/// and report: every read and the purge stay inside the catalogue asked.
pub async fn twin_catalogues(store: &dyn Store) {
    let (org, cat_a) = tenant();
    let (_, cat_b) = tenant();
    let mut ctxs = Vec::new();
    for (cat, name) in [(cat_a, "twin-a"), (cat_b, "twin-b")] {
        store
            .upsert_catalogue(catalogue(org, cat, name, &unique_host(name)))
            .await
            .unwrap();
        let ctx = member_ctx(org, cat);
        store
            .upsert_publisher(&ctx, publisher("city"))
            .await
            .unwrap();
        let mut twin = dataset("twin", "city", "public");
        twin.title = [("sk".to_owned(), name.to_owned())].into();
        let twin = store.upsert_dataset(&ctx, twin).await.unwrap();
        let resource = twin.distributions[0].id.clone();
        store
            .upsert_resource_view(&ctx, view(&format!("view-{name}"), &resource, 1))
            .await
            .unwrap();
        store
            .upsert_shacl_report(&ctx, shacl_report(&twin.id, "eu", true))
            .await
            .unwrap();
        ctxs.push((ctx, resource));
    }
    let [(a, resource), (b, _)] = <[_; 2]>::try_from(ctxs).unwrap();
    let datasets = store.list_datasets(&a).await.unwrap();
    assert_eq!(names(&datasets, |d| d.title["sk"].clone()), ["twin-a"]);
    let (ds, _) = store
        .get_distribution(&a, &resource)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ds.title["sk"], "twin-a");
    let views = store.list_resource_views(&a, &resource).await.unwrap();
    assert_eq!(names(&views, |v| v.id.clone()), ["view-twin-a"]);

    store.purge_dataset(&a, "twin").await.unwrap();
    assert!(store
        .list_resource_views(&a, &resource)
        .await
        .unwrap()
        .is_empty());
    assert!(store
        .list_shacl_reports(&a, None, None)
        .await
        .unwrap()
        .is_empty());
    let views = store.list_resource_views(&b, &resource).await.unwrap();
    assert_eq!(names(&views, |v| v.id.clone()), ["view-twin-b"]);
    assert_eq!(
        store
            .list_shacl_reports(&b, None, None)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(store.get_dataset(&b, "twin").await.unwrap().is_some());
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
    // Members-only sets answer empty to anyone outside the Organization, unscoped as asked.
    let anon = RequestCtx::anonymous(ctx.org_id, ctx.catalogue_id);
    for set in dataroom_core::odata_defs::MEMBERS_ONLY_SETS {
        assert!(odata_ids(store, &anon, set).await.is_empty(), "{set}");
    }
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
        // b's report was of the dataset b purged.
        assert_eq!(
            store.list_shacl_reports(c, None, None).await.unwrap().len(),
            usize::from(std::ptr::eq(c, &a))
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
    let stale_before = jc_time::TimestampTz::now();
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

/// An activity row naming another tenant: the store files it under the write's own.
fn audit_row(id: &str, operation: &str) -> dataroom_core::ActivityRow {
    let (org_id, catalogue_id) = tenant();
    dataroom_core::ActivityRow {
        org_id,
        catalogue_id,
        id: id.into(),
        at: jc_time::TimestampTz::now(),
        actor: "editor".into(),
        operation: operation.into(),
        object_type: "Dataset".into(),
        object_id: String::new(),
        summary: operation.into(),
        details: serde_json::json!({}),
    }
}

/// `save_dataset` / `save_catalogue`: a write based on a version another write replaced is
/// refused and leaves nothing, the accepted one commits with its activity row (S11-25, S11-13).
pub async fn guarded_writes(store: &dyn Store) {
    let (org, cat) = tenant();
    store
        .upsert_catalogue(catalogue(org, cat, "gw", &unique_host("gw")))
        .await
        .unwrap();
    let ctx = member_ctx(org, cat);
    store
        .upsert_publisher(&ctx, publisher("city"))
        .await
        .unwrap();
    let read = store
        .upsert_dataset(&ctx, dataset("air", "city", "public"))
        .await
        .unwrap();

    // Two edits built on the same read: the first wins, the second is a conflict.
    let mut first = read.clone();
    first.tags = vec!["first".into()];
    let saved = store
        .save_dataset(
            &ctx,
            first,
            Some(read.modified),
            audit_row("act-1", "resource.propose"),
        )
        .await
        .unwrap();
    assert!(
        saved.modified > read.modified,
        "every guarded write moves the version"
    );
    let mut second = read.clone();
    second.tags = vec!["second".into()];
    let lost = store
        .save_dataset(
            &ctx,
            second,
            Some(read.modified),
            audit_row("act-2", "resource.propose"),
        )
        .await;
    assert!(matches!(lost, Err(DataroomError::Conflict(_))), "{lost:?}");
    let stored = store.get_dataset(&ctx, "air").await.unwrap().unwrap();
    assert_eq!(stored.tags, ["first"]);
    assert_eq!(
        stored.modified, saved.modified,
        "the read version round-trips exactly"
    );
    let rows = store
        .list_activity(&ctx, Some("pkg-air"), 10)
        .await
        .unwrap();
    assert_eq!(
        names(&rows, |r| r.id.clone()),
        ["act-1"],
        "the refused write left no row"
    );
    assert_eq!((rows[0].org_id, rows[0].catalogue_id), (org, cat));

    // A retry on a fresh read goes through; no expected version writes whatever is stored.
    let mut retry = stored.clone();
    retry.tags = vec!["second".into()];
    store
        .save_dataset(
            &ctx,
            retry,
            Some(stored.modified),
            audit_row("act-3", "resource.propose"),
        )
        .await
        .unwrap();
    let mut blind = stored.clone();
    blind.name = "fresh".into();
    blind.id = String::new();
    blind.distributions.clear();
    let fresh = store
        .save_dataset(&ctx, blind, None, audit_row("act-4", "dataset.propose"))
        .await
        .unwrap();
    assert_eq!(
        store
            .list_activity(&ctx, Some(&fresh.id), 10)
            .await
            .unwrap()
            .len(),
        1
    );
    // An expected version of a record that is not stored is a conflict too.
    let mut gone = stored.clone();
    gone.id = "pkg-nothing".into();
    gone.name = "nothing".into();
    assert!(matches!(
        store
            .save_dataset(
                &ctx,
                gone,
                Some(stored.modified),
                audit_row("act-5", "resource.propose")
            )
            .await,
        Err(DataroomError::Conflict(_))
    ));

    // The catalogue row: a branding write and a profile write built on one read.
    let c0 = store.get_catalogue(org, cat).await.unwrap().unwrap();
    let mut branded = c0.clone();
    branded.branding = Some(serde_json::json!({"primary": "#123456"}));
    let c1 = store
        .save_catalogue(
            branded,
            Some(c0.modified),
            audit_row("act-c1", "branding.propose"),
        )
        .await
        .unwrap();
    assert!(c1.modified > c0.modified);
    let mut profiled = c0.clone();
    profiled.main_profile = "other".into();
    assert!(matches!(
        store
            .save_catalogue(
                profiled,
                Some(c0.modified),
                audit_row("act-c2", "profile.activate")
            )
            .await,
        Err(DataroomError::Conflict(_))
    ));
    let stored = store.get_catalogue(org, cat).await.unwrap().unwrap();
    assert_eq!(stored.branding, c1.branding);
    assert_eq!(stored.main_profile, c0.main_profile);
    let rows = store
        .list_activity(&ctx, Some(&cat.to_string()), 10)
        .await
        .unwrap();
    assert_eq!(names(&rows, |r| r.id.clone()), ["act-c1"]);
}
