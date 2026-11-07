// SPDX-License-Identifier: EUPL-1.2
//! The reads of the view root, one test per rule of ADR-0023 (its
//! Confirmation table) and per row of surfaces-test-plan section 6.

use crate::common::{fixture, get, send, CARS, HISTORY};
use axum::http::{Method, StatusCode};
use serde_json::{json, Value};

const CAR1: &str = "urn:ngsi-ld:Car:1";
const CAR2: &str = "urn:ngsi-ld:Car:2";

/// Rule 1: the count and the pages describe the in-view set, and a page's
/// link stays inside the view.
#[tokio::test(flavor = "multi_thread")]
async fn views_count_and_paging_describe_the_narrowed_set() -> Result<(), String> {
    let f = fixture().await?;
    let first = get(
        &f.app,
        &format!("{CARS}/entities?type=Car&count=true&limit=1"),
    )
    .await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.text);
    assert_eq!(
        first.header("NGSILD-Results-Count").as_deref(),
        Some("2"),
        "Car:3 is out of view"
    );
    assert_eq!(first.ids().len(), 1);
    let next = first.link("next").ok_or("a next page")?;
    assert!(next.starts_with(&format!("{CARS}/entities?")), "{next}");
    let second = get(&f.app, &next).await;
    assert_eq!(second.status, StatusCode::OK, "{}", second.text);
    assert_eq!(second.header("NGSILD-Results-Count").as_deref(), Some("2"));
    let mut both = [first.ids(), second.ids()].concat();
    both.sort();
    assert_eq!(both, [CAR1, CAR2]);
    assert_eq!(
        second.link("next"),
        None,
        "the last in-view page has no next"
    );
    let prev = second.link("prev").ok_or("a previous page")?;
    assert!(prev.starts_with(&format!("{CARS}/entities?")), "{prev}");
    let none = get(
        &f.app,
        &format!("{CARS}/entities?type=Car&count=true&limit=0"),
    )
    .await;
    assert_eq!(none.header("NGSILD-Results-Count").as_deref(), Some("2"));
    Ok(())
}

/// The ids `q` selects among the Cars through `cars`.
async fn selects(app: &axum::Router, q: &str) -> Vec<String> {
    let enc: String = q
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    let a = get(app, &format!("{CARS}/entities?type=Car&q={enc}")).await;
    assert_eq!(a.status, StatusCode::OK, "{q}: {}", a.text);
    let mut ids = a.ids();
    ids.sort();
    ids
}

/// Rule 2: a term on `secret`, which no rule shows, selects exactly what
/// the same term on an Attribute no Car has selects: every operator, its
/// negation and a value on each side of every stored one.
#[tokio::test(flavor = "multi_thread")]
async fn views_a_hidden_attribute_does_not_exist_for_q() -> Result<(), String> {
    let f = fixture().await?;
    let mut terms = vec![
        "secret".to_owned(),
        "!secret".to_owned(),
        "secret~=\".*\"".to_owned(),
        "secret==1..9".to_owned(),
        "secret|speed>20".to_owned(),
        "!secret;speed<20".to_owned(),
    ];
    for v in [0, 1, 5, 9, 10] {
        for op in ["==", "!=", ">", "<", ">=", "<="] {
            terms.push(format!("secret{op}{v}"));
        }
    }
    for term in &terms {
        let hidden = selects(&f.app, term).await;
        let absent = selects(&f.app, &term.replace("secret", "nothere")).await;
        assert_eq!(hidden, absent, "q={term}");
    }
    assert_eq!(selects(&f.app, "secret==1").await, Vec::<String>::new());
    assert_eq!(selects(&f.app, "!secret").await, [CAR1, CAR2]);
    Ok(())
}

/// Rule 3: an out-of-view Entity is not found, with the bytes an absent one
/// gets (the id the caller sent aside): the Entity, its Attribute, its
/// history, and a hidden Attribute of an in-view Entity.
#[tokio::test(flavor = "multi_thread")]
async fn views_an_out_of_view_entity_is_not_found() -> Result<(), String> {
    let f = fixture().await?;
    for (hidden, absent) in [
        (
            "/entities/urn:ngsi-ld:Car:3".to_owned(),
            "/entities/urn:ngsi-ld:Car:404".to_owned(),
        ),
        (
            "/entities/urn:ngsi-ld:Org:o".to_owned(),
            "/entities/urn:ngsi-ld:Org:404".to_owned(),
        ),
        (
            "/entities/urn:ngsi-ld:Car:3/attrs/speed".to_owned(),
            "/entities/urn:ngsi-ld:Car:404/attrs/speed".to_owned(),
        ),
        (
            format!("/entities/{CAR1}/attrs/secret"),
            format!("/entities/{CAR1}/attrs/nothere"),
        ),
        (
            "/temporal/entities/urn:ngsi-ld:Org:o".to_owned(),
            "/temporal/entities/urn:ngsi-ld:Org:404".to_owned(),
        ),
    ] {
        let h = get(&f.app, &format!("{CARS}{hidden}")).await;
        let a = get(&f.app, &format!("{CARS}{absent}")).await;
        assert_eq!(h.status, StatusCode::NOT_FOUND, "{hidden}: {}", h.text);
        assert_eq!(a.status, h.status, "{absent}");
        let sent = |p: &str| p.rsplit('/').next().unwrap_or_default().to_owned();
        let h_text = h
            .text
            .replace(&sent(&hidden), "X")
            .replace("Car:3", "X")
            .replace("Org:o", "X");
        let a_text = a
            .text
            .replace(&sent(&absent), "X")
            .replace("Car:404", "X")
            .replace("Org:404", "X");
        assert_eq!(h_text, a_text, "{hidden} answers apart from {absent}");
    }
    Ok(())
}

/// Rule 3 for a history under rule 8: a Car the view's `q` leaves out now
/// has no history through it, with the bytes an absent one gets.
#[tokio::test(flavor = "multi_thread")]
async fn views_an_entity_out_of_view_now_has_no_history() -> Result<(), String> {
    let f = fixture().await?;
    let h = get(
        &f.app,
        &format!("{HISTORY}/temporal/entities/urn:ngsi-ld:Car:3"),
    )
    .await;
    let a = get(
        &f.app,
        &format!("{HISTORY}/temporal/entities/urn:ngsi-ld:Car:404"),
    )
    .await;
    assert_eq!(h.status, StatusCode::NOT_FOUND, "{}", h.text);
    assert_eq!(h.text.replace("Car:3", "X"), a.text.replace("Car:404", "X"));
    Ok(())
}

/// Rule 5: discovery answers from what the view shows: a type out of the
/// selector is absent and its `/types/{t}` is 404 like a type nobody has, a
/// hidden Attribute is absent from every list, and the @context list is
/// the core one alone.
#[tokio::test(flavor = "multi_thread")]
async fn views_discovery_is_computed_from_the_view() -> Result<(), String> {
    let f = fixture().await?;
    let types = get(&f.app, &format!("{CARS}/types")).await;
    assert_eq!(types.status, StatusCode::OK, "{}", types.text);
    assert!(
        types.text.contains("Car") && types.text.contains("Person"),
        "{}",
        types.text
    );
    assert!(!types.text.contains("Org"), "{}", types.text);

    for (hidden, absent) in [
        ("/types/Org", "/types/Nothing"),
        ("/attributes/secret", "/attributes/nothere"),
        ("/attributes/salary", "/attributes/nothere"),
    ] {
        let h = get(&f.app, &format!("{CARS}{hidden}")).await;
        let a = get(&f.app, &format!("{CARS}{absent}")).await;
        assert_eq!(h.status, StatusCode::NOT_FOUND, "{hidden}: {}", h.text);
        assert_eq!(a.status, StatusCode::NOT_FOUND, "{absent}: {}", a.text);
    }
    for uri in [
        "/attributes",
        "/attributes?details=true",
        "/types?details=true",
        "/types/Car",
        "/types/Person",
    ] {
        let a = get(&f.app, &format!("{CARS}{uri}")).await;
        assert_eq!(a.status, StatusCode::OK, "{uri}: {}", a.text);
        for hidden in ["secret", "salary", "Org"] {
            assert!(!a.text.contains(hidden), "{uri} names {hidden}: {}", a.text);
        }
    }
    let attrs = get(&f.app, &format!("{CARS}/attributes")).await;
    for shown in ["speed", "colour", "name"] {
        assert!(attrs.text.contains(shown), "{shown}: {}", attrs.text);
    }

    // a context the tenant stored defines a term for the hidden Attribute
    let ctx = json!({"@context": {"secret": "https://example.org/secret"}});
    let stored = send(
        &f.app,
        Method::POST,
        "/ngsi-ld/v1/jsonldContexts",
        &[("NGSILD-Tenant", "city")],
        Some(&ctx),
    )
    .await;
    assert_eq!(stored.status, StatusCode::CREATED, "{}", stored.text);
    let list = get(&f.app, &format!("{CARS}/jsonldContexts")).await;
    assert_eq!(list.status, StatusCode::OK, "{}", list.text);
    let urls: Vec<&str> = list
        .json
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        urls.iter().all(|u| u.contains("uri.etsi.org")),
        "the core @context alone: {}",
        list.text
    );
    let details = get(&f.app, &format!("{CARS}/jsonldContexts?details=true")).await;
    assert_eq!(details.status, StatusCode::OK, "{}", details.text);
    assert!(
        !details.text.contains("jsonldContexts/"),
        "no stored @context: {}",
        details.text
    );
    let direct = send(
        &f.app,
        Method::GET,
        "/ngsi-ld/v1/jsonldContexts",
        &[("NGSILD-Tenant", "city")],
        None,
    )
    .await;
    assert!(
        direct.json.as_array().is_some_and(|l| l.len() > urls.len()),
        "the NGSI-LD root still lists it: {}",
        direct.text
    );
    Ok(())
}

/// Rule 6: a join embeds a target the view shows, projected by its own
/// rule, and leaves an out-of-view target out while its Relationship stays.
#[tokio::test(flavor = "multi_thread")]
async fn views_join_omits_out_of_view_targets_and_keeps_the_relationship() -> Result<(), String> {
    let f = fixture().await?;
    for uri in [
        format!("{CARS}/entities/{CAR1}?join=inline&joinLevel=2"),
        format!("{CARS}/entities?type=Car&join=flat&joinLevel=2"),
        format!("{CARS}/entities?type=Car&join=inline&joinLevel=1"),
    ] {
        let a = get(&f.app, &uri).await;
        assert_eq!(a.status, StatusCode::OK, "{uri}: {}", a.text);
        assert!(
            a.text.contains("Pat"),
            "{uri}: the in-view owner is embedded: {}",
            a.text
        );
        assert!(
            a.text.contains("urn:ngsi-ld:Org:o"),
            "{uri}: the Relationship to the Org stays: {}",
            a.text
        );
        assert!(
            !a.text.contains("Acme"),
            "{uri}: the out-of-view Org is embedded: {}",
            a.text
        );
        assert!(
            !a.text.contains("987654") && !a.text.contains("salary"),
            "{uri}: the owner's hidden salary: {}",
            a.text
        );
    }
    Ok(())
}

/// Rule 8: the history of the Entities in the view now, every instance of
/// a visible Attribute (Car:2 was at 90 before it slowed down), no hidden
/// Attribute, `lastN` counted per visible Attribute.
#[tokio::test(flavor = "multi_thread")]
async fn views_temporal_serves_current_members_and_limits_after_the_allowlist() -> Result<(), String>
{
    let f = fixture().await?;
    let all = get(&f.app, &format!("{HISTORY}/temporal/entities?type=Car&timerel=after&timeAt=1970-01-01T00:00:00Z&timeproperty=modifiedAt")).await;
    assert_eq!(all.status, StatusCode::OK, "{}", all.text);
    let mut ids = all.ids();
    ids.sort();
    assert_eq!(ids, [CAR1, CAR2], "Car:3 is at 90 now: {}", all.text);
    assert!(!all.text.contains("secret"), "{}", all.text);

    let car2 = get(&f.app, &format!("{HISTORY}/temporal/entities/{CAR2}")).await;
    assert_eq!(car2.status, StatusCode::OK, "{}", car2.text);
    let speeds: Vec<i64> = car2
        .json
        .get("speed")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|i| i.get("value").and_then(Value::as_i64))
        .collect();
    assert_eq!(
        speeds.len(),
        2,
        "both instances of the current member: {}",
        car2.text
    );
    assert!(
        speeds.contains(&90) && speeds.contains(&50),
        "{}",
        car2.text
    );

    let last = get(
        &f.app,
        &format!("{HISTORY}/temporal/entities/{CAR2}?lastN=1"),
    )
    .await;
    assert_eq!(last.status, StatusCode::OK, "{}", last.text);
    let shown = |a: &str| {
        last.json
            .get(a)
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    };
    assert_eq!(shown("speed"), 1, "{}", last.text);
    assert_eq!(shown("colour"), 1, "{}", last.text);
    assert!(last.json.get("secret").is_none(), "{}", last.text);

    let agg = get(
        &f.app,
        &format!("{HISTORY}/temporal/entities/{CAR2}?options=aggregatedValues&aggrMethods=max,min&timeproperty=modifiedAt"),
    )
    .await;
    assert_eq!(agg.status, StatusCode::OK, "{}", agg.text);
    assert!(
        agg.json.get("speed").is_some() && agg.json.get("secret").is_none(),
        "{}",
        agg.text
    );

    let gone = get(
        &f.app,
        &format!("{HISTORY}/temporal/entities/urn:ngsi-ld:Car:3"),
    )
    .await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND, "{}", gone.text);
    Ok(())
}

/// Rule 10: a view serves its one Tenant. Another Tenant named is 400 and
/// nothing switches; the same one is served and echoed; none named is
/// served in the view's Tenant and the answer does not name it (F4/V18).
#[tokio::test(flavor = "multi_thread")]
async fn views_pin_their_tenant() -> Result<(), String> {
    let f = fixture().await?;
    let uri = format!("{CARS}/entities?type=Car");
    let other = send(
        &f.app,
        Method::GET,
        &uri,
        &[("NGSILD-Tenant", "other")],
        None,
    )
    .await;
    assert_eq!(other.status, StatusCode::BAD_REQUEST, "{}", other.text);
    assert_eq!(
        other.json.get("title"),
        Some(&json!("BadRequestData")),
        "{}",
        other.text
    );
    assert!(
        !other.text.contains("city") && !other.text.contains("Car:9"),
        "{}",
        other.text
    );
    for bad in ["CITY", "not a tenant", ""] {
        let a = send(&f.app, Method::GET, &uri, &[("NGSILD-Tenant", bad)], None).await;
        assert_eq!(a.status, StatusCode::BAD_REQUEST, "{bad:?}: {}", a.text);
    }
    let twice = send(
        &f.app,
        Method::GET,
        &uri,
        &[("NGSILD-Tenant", "city"), ("NGSILD-Tenant", "city")],
        None,
    )
    .await;
    assert_eq!(twice.status, StatusCode::BAD_REQUEST, "{}", twice.text);

    let same = send(
        &f.app,
        Method::GET,
        &uri,
        &[("NGSILD-Tenant", "city")],
        None,
    )
    .await;
    assert_eq!(same.status, StatusCode::OK, "{}", same.text);
    assert_eq!(same.header("NGSILD-Tenant").as_deref(), Some("city"));
    let none = get(&f.app, &uri).await;
    assert_eq!(none.status, StatusCode::OK, "{}", none.text);
    assert_eq!(none.ids(), same.ids());
    assert!(
        !none.ids().iter().any(|i| i.ends_with("Car:9")),
        "{}",
        none.text
    );
    assert!(
        none.headers.get("NGSILD-Tenant").is_none(),
        "the pinned tenant is named to a caller who never sent it"
    );
    Ok(())
}
