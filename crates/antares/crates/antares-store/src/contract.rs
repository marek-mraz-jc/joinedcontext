// SPDX-License-Identifier: EUPL-1.2
//! The driver contract, as executable code.
//!
//! Every rule here is one a caller in `antares-api` relies on and no
//! backend may decide for itself. A driver that passes both functions can be
//! dropped into the broker; one that does not will break requests in ways
//! its own unit tests are free to miss, because a backend's tests assert
//! what that backend does, not what the seam promises.
//!
//! Both functions PANIC on the first violation, naming the rule. The caller
//! supplies two tenants that already exist (a backend may require a tenant
//! row before it accepts writes) and a prefix that makes the ids of this run
//! unique, so a shared database can host several runs at once.
//!
//! Both functions are `async`: a driver is awaited, never blocked on.

use crate::{CurrentStateDriver, CurrentStateDriverExt, Kind, TemporalDriver, TemporalDriverExt};
use antares_model::TenantId;
use serde_json::{json, Value};

fn doc(id: &str) -> Value {
    json!({"id": id, "type": "https://uri.etsi.org/ngsi-ld/default-context/T"})
}

/// Hold a current-state driver to the contract `antares-api` writes against.
///
/// `a` and `b` are two existing tenants; `prefix` namespaces the ids this
/// run creates and deletes. Panics on the first rule broken.
pub async fn run_current_state_contract(
    d: &dyn CurrentStateDriver,
    a: &TenantId,
    b: &TenantId,
    prefix: &str,
) {
    let e1 = format!("urn:ngsi-ld:{prefix}:1");
    let e2 = format!("urn:ngsi-ld:{prefix}:2");
    let gone = format!("urn:ngsi-ld:{prefix}:absent");

    // An absent row is absent, not an error and not an empty document.
    assert!(
        d.get(a, Kind::Entity, &gone).await.expect("get").is_none(),
        "get of an absent row must answer None"
    );
    assert!(
        !d.delete(a, Kind::Entity, &gone).await.expect("delete"),
        "delete of an absent row must answer false"
    );

    // ADR-0005 / ETSI 047_06: a mutate is a read-modify-write under the row
    // lock, and a missing row is None — NEVER an insert. A get+upsert
    // implementation lets a bookkeeping writeback racing a DELETE resurrect
    // the deleted row.
    let missed = d
        .mutate::<(), ()>(a, Kind::Entity, &gone, |_| Ok(()))
        .await
        .expect("mutate");
    assert!(missed.is_none(), "mutate of an absent row must answer None");
    assert!(
        d.get(a, Kind::Entity, &gone).await.expect("get").is_none(),
        "mutate of an absent row must not insert it (047_06)"
    );

    // create is create-if-absent: the second one is refused and changes
    // nothing.
    assert!(
        d.create(a, Kind::Entity, &e1, doc(&e1))
            .await
            .expect("create"),
        "the first create must report true"
    );
    assert!(
        !d.create(a, Kind::Entity, &e1, doc("urn:overwritten"))
            .await
            .expect("create"),
        "a create over an existing id must report false"
    );
    let row = d
        .get(a, Kind::Entity, &e1)
        .await
        .expect("get")
        .expect("row");
    assert_eq!(
        row["id"],
        e1.as_str(),
        "the refused create must not overwrite"
    );

    // A mutate that returns Ok commits; one that returns Err commits nothing.
    let applied = d
        .mutate::<(), ()>(a, Kind::Entity, &e1, |v| {
            v["marker"] = json!(1);
            Ok(())
        })
        .await
        .expect("mutate");
    assert_eq!(applied, Some(Ok(())), "a mutate over a present row applies");
    assert_eq!(
        d.get(a, Kind::Entity, &e1)
            .await
            .expect("get")
            .expect("row")["marker"],
        json!(1),
        "an accepted mutate must be visible to the next read"
    );
    let rejected = d
        .mutate::<(), &'static str>(a, Kind::Entity, &e1, |v| {
            v["marker"] = json!(2);
            Err("rejected")
        })
        .await
        .expect("mutate");
    assert_eq!(
        rejected,
        Some(Err("rejected")),
        "the closure's error crosses back"
    );
    assert_eq!(
        d.get(a, Kind::Entity, &e1)
            .await
            .expect("get")
            .expect("row")["marker"],
        json!(1),
        "a rejected mutate must commit nothing"
    );

    // A type list, where the driver keeps one, names the type of a live
    // Entity (T-4610): `/types` answers from it without a fold.
    if let Some(names) = d.entity_type_names(a).await.expect("type list") {
        assert!(
            names
                .iter()
                .any(|t| t == "https://uri.etsi.org/ngsi-ld/default-context/T"),
            "the type list must name the type of a live Entity: {names:?}"
        );
    }

    // An attribute catalogue, where the driver keeps one, lists every
    // member a live Entity carries under each of its types (T-4934), the
    // member a write added included.
    if let Some(pairs) = d.entity_attr_pairs(a).await.expect("attribute list") {
        let t = "https://uri.etsi.org/ngsi-ld/default-context/T";
        assert!(
            pairs.iter().any(|(ty, attr)| ty == t && attr == "marker"),
            "the attribute list must name a live Entity's member: {pairs:?}"
        );
    }

    // Tenant isolation, on every read and every write path.
    assert!(
        d.get(b, Kind::Entity, &e1).await.expect("get").is_none(),
        "another tenant must not read the row"
    );
    assert!(
        !d.delete(b, Kind::Entity, &e1).await.expect("delete"),
        "another tenant must not delete the row"
    );
    assert!(
        d.mutate::<(), ()>(b, Kind::Entity, &e1, |_| Ok(()))
            .await
            .expect("mutate")
            .is_none(),
        "another tenant must not mutate the row"
    );
    assert!(
        d.list(b, Kind::Entity)
            .await
            .expect("list")
            .iter()
            .all(|r| r["id"] != e1.as_str()),
        "another tenant must not list the row"
    );

    // Kinds are separate namespaces: the same id under two kinds is two rows.
    assert!(
        d.create(a, Kind::Subscription, &e1, doc(&e1))
            .await
            .expect("create"),
        "the same id under another kind is a different row"
    );
    assert!(d.delete(a, Kind::Subscription, &e1).await.expect("delete"));
    assert!(
        d.get(a, Kind::Entity, &e1).await.expect("get").is_some(),
        "deleting one kind must not touch another"
    );

    // upsert answers whether a document was ALREADY there, and batch_upsert
    // answers the opposite polarity — created-flags, which the batch path
    // needs to split 201 from 204 (5.6.8). Getting one of the two backwards
    // is invisible until a batch reports every create as an update.
    assert!(
        !d.upsert(a, Kind::Entity, &e2, doc(&e2))
            .await
            .expect("upsert"),
        "upsert that created the row reports false"
    );
    assert!(
        d.upsert(a, Kind::Entity, &e2, doc(&e2))
            .await
            .expect("upsert"),
        "upsert over an existing row reports true"
    );
    let e3 = format!("urn:ngsi-ld:{prefix}:3");
    let flags = d
        .batch_upsert(a, vec![(e3.clone(), doc(&e3)), (e2.clone(), doc(&e2))])
        .await
        .expect("batch_upsert");
    assert_eq!(
        flags,
        vec![true, false],
        "batch_upsert answers created-flags in input order, the opposite \
         polarity of upsert"
    );
    assert!(d.delete(a, Kind::Entity, &e3).await.expect("delete"));
    let ids = vec![e2.clone(), gone.clone(), e1.clone()];
    let batch = d
        .batch_mutate::<()>(a, &ids, |_, v| {
            v["batched"] = json!(true);
            Ok(())
        })
        .await
        .expect("batch_mutate");
    assert_eq!(batch.len(), ids.len(), "one result per input id");
    assert!(batch[0].is_some(), "results align with ids: {ids:?}");
    assert!(batch[1].is_none(), "an absent id answers None, in place");
    assert!(batch[2].is_some(), "results align with ids: {ids:?}");

    // A member whose closure refuses commits nothing while the others
    // commit: a batch write conditioned on each stored Entity (gap G12)
    // refuses member by member, and a refused member stays as it was.
    let pair = vec![e1.clone(), e2.clone()];
    let judged = d
        .batch_mutate::<&'static str>(a, &pair, |id, v| {
            v["judged"] = json!(true);
            if id == e2.as_str() {
                Err("refused")
            } else {
                Ok(())
            }
        })
        .await
        .expect("batch_mutate");
    assert_eq!(
        judged,
        vec![Some(Ok(())), Some(Err("refused"))],
        "each member's own answer, in place"
    );
    let accepted = d.get(a, Kind::Entity, &e1).await.expect("get");
    assert_eq!(
        accepted.expect("row")["judged"],
        json!(true),
        "an accepted member commits"
    );
    let refused = d.get(a, Kind::Entity, &e2).await.expect("get");
    assert!(
        refused.expect("row").get("judged").is_none(),
        "a refused member must commit nothing"
    );

    // A query may over-return (the caller re-checks whatever the backend
    // could not decide) but must never drop a matching row, and must never
    // cross a tenant.
    let ids_ref: Vec<&str> = vec![&e1, &e2];
    let mine = d
        .query_entities(
            a,
            &crate::filter::EntityFilter {
                ids: Some(&ids_ref),
                ..Default::default()
            },
        )
        .await
        .expect("query_entities");
    for want in [&e1, &e2] {
        assert!(
            mine.rows.iter().any(|r| r["id"] == want.as_str()),
            "query must not drop a matching row: {want}"
        );
    }
    let theirs = d
        .query_entities(
            b,
            &crate::filter::EntityFilter {
                ids: Some(&ids_ref),
                ..Default::default()
            },
        )
        .await
        .expect("query_entities");
    assert!(
        theirs.rows.is_empty(),
        "a query must never cross a tenant: {:?}",
        theirs.rows
    );

    // Paging pushdown is optional; claiming it and not doing it is not.
    let paged = d
        .query_entities(
            a,
            &crate::filter::EntityFilter {
                ids: Some(&ids_ref),
                page: Some(crate::filter::Page {
                    offset: 0,
                    limit: 1,
                    count: true,
                }),
                ..Default::default()
            },
        )
        .await
        .expect("query_entities");
    if paged.paged {
        assert!(
            paged.rows.len() <= 1,
            "a driver reporting paged=true has applied the LIMIT"
        );
        assert!(
            paged.decided,
            "paged implies decided: a LIMIT over an undecided set pages the wrong rows"
        );
    }
    // Keyset (T-4683): `after_id` is optional too, but a driver that pages
    // must honour it, or a discovery walk reads its first page forever.
    let first = std::cmp::min(e1.as_str(), e2.as_str());
    let after = d
        .query_entities(
            a,
            &crate::filter::EntityFilter {
                ids: Some(&ids_ref),
                after_id: Some(first),
                page: Some(crate::filter::Page {
                    offset: 0,
                    limit: 10,
                    count: false,
                }),
                ..Default::default()
            },
        )
        .await
        .expect("query_entities");
    if after.paged {
        assert!(
            after.rows.iter().all(|r| r["id"].as_str() > Some(first)),
            "a paging driver returns only ids after `after_id`: {:?}",
            after.rows
        );
        assert_eq!(after.rows.len(), 1, "the one id after {first} is kept");
    }

    // `subscription_tenants` is the iteration domain the interval sweep and
    // both mirror hydrations walk. A backend may return MORE tenants than
    // hold a subscription — every caller lists per tenant afterwards and an
    // empty list costs nothing. It may never return FEWER: a tenant missing
    // here is a tenant whose periodic notifications never fire and whose
    // subscriptions never reach the mirror, with no error anywhere.
    let sub = format!("urn:ngsi-ld:Subscription:{prefix}:1");
    let sub_doc = json!({
        "id": &sub,
        "type": "Subscription",
        "status": "active",
        "notification": {"endpoint": {"uri": "http://sink.invalid/n"}},
    });
    d.upsert(a, Kind::Subscription, &sub, sub_doc)
        .await
        .expect("upsert subscription");
    let domain = d
        .subscription_tenants()
        .await
        .expect("subscription_tenants");
    assert!(
        domain.iter().any(|t| t == a.as_str()),
        "a tenant holding a subscription must appear in subscription_tenants: {domain:?}"
    );
    // A REGISTRATION puts a tenant in the domain too. One of the hydrations
    // that walks this domain fills the registration mirror, and the
    // federation path reads that mirror alone once it is installed — so a
    // tenant holding registrations and no subscription is a tenant that
    // silently forwards to no Context Source.
    let reg_only = TenantId::new(&format!("{prefix}regonly")).expect("tenant");
    let reg = format!("urn:ngsi-ld:ContextSourceRegistration:{prefix}:1");
    d.upsert(
        &reg_only,
        Kind::Registration,
        &reg,
        json!({
            "id": &reg,
            "type": "ContextSourceRegistration",
            "endpoint": "http://cs.invalid/ngsi-ld/v1",
            "information": [{"entities": [{"type": "Vehicle"}]}],
        }),
    )
    .await
    .expect("upsert registration");
    let domain = d
        .subscription_tenants()
        .await
        .expect("subscription_tenants");
    assert!(
        domain.iter().any(|t| t == reg_only.as_str()),
        "a tenant holding only a registration must appear in the mirror-hydration \
         domain, or its registration mirror hydrates empty: {domain:?}"
    );

    // Table 5.2.9-2 forward bookkeeping, pinned here for the same reason as
    // 5.2.14.2's: a backend may write it as one statement instead of a
    // read-modify-write, and the result must not depend on which it chose.
    let f1 = "2020-02-01T00:00:00.000Z";
    let f2 = "2020-02-02T00:00:00.000Z";
    let ok1 = d
        .record_forward(&reg_only, &reg, f1, true)
        .await
        .expect("record_forward")
        .expect("the registration is there");
    assert_eq!(
        ok1["timesSent"], 1,
        "the first forward moves timesSent to 1"
    );
    assert_eq!(ok1["lastSuccess"], f1, "lastSuccess takes the stamp");
    assert_eq!(ok1["status"], "ok", "a 2xx leaves the registration ok");
    assert!(
        ok1.get("timesFailed").is_none() && ok1.get("lastFailure").is_none(),
        "a registration that has only succeeded carries no failure members: {ok1}"
    );

    let bad = d
        .record_forward(&reg_only, &reg, f2, false)
        .await
        .expect("record_forward")
        .expect("the registration is still there");
    assert_eq!(
        bad["timesSent"], 2,
        "timesSent counts the failed attempt too (Table 5.2.9-2)"
    );
    assert_eq!(bad["timesFailed"], 1, "the failure moves timesFailed");
    assert_eq!(bad["lastFailure"], f2, "lastFailure takes the stamp");
    assert_eq!(bad["status"], "failed", "status names the LAST attempt");
    assert_eq!(
        bad["lastSuccess"], f1,
        "a failure never rewinds the last success"
    );

    // A registration deleted while its forward was in flight has no row to
    // book against, and the writeback must not resurrect it.
    let vanished = format!("urn:ngsi-ld:ContextSourceRegistration:{prefix}:gone");
    assert!(
        d.record_forward(&reg_only, &vanished, f1, true)
            .await
            .expect("record_forward")
            .is_none(),
        "an absent registration books nothing"
    );
    assert!(
        d.get(&reg_only, Kind::Registration, &vanished)
            .await
            .expect("get")
            .is_none(),
        "a bookkeeping writeback must never insert the row it missed"
    );

    d.delete(&reg_only, Kind::Registration, &reg)
        .await
        .expect("delete");
    // `list_page` is how the readers that must see EVERY document read —
    // the mirror seed above all — so a backend may not refuse it for volume
    // the way `list` may (5.5.6 licenses TooManyResults for a query
    // operation, which an internal bootstrap is not). Every arm must agree
    // on the walk, including the one that takes the trait default: ids
    // strictly greater than `after`, id-ordered, at most `limit`, a short
    // page means the end, and every stored document served exactly once.
    let mut ids: Vec<String> = (0..7)
        .map(|i| format!("urn:ngsi-ld:Subscription:{prefix}:page:{i}"))
        .collect();
    for id in &ids {
        d.upsert(
            a,
            Kind::Subscription,
            id,
            json!({"id": id, "type": "Subscription"}),
        )
        .await
        .expect("upsert page subscription");
    }
    ids.push(sub.clone());
    ids.sort();

    let mut walked: Vec<String> = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let page = d
            .list_page(a, Kind::Subscription, after.as_deref(), 3)
            .await
            .expect("list_page");
        assert!(page.len() <= 3, "a page may not exceed its limit: {page:?}");
        let short = page.len() < 3;
        for doc in &page {
            let id = doc["id"].as_str().expect("a stored doc keeps its id");
            assert!(
                after.as_deref().is_none_or(|prev| id > prev),
                "`after` is exclusive and the walk ascends: {id} after {after:?}"
            );
            after = Some(id.to_owned());
            walked.push(id.to_owned());
        }
        if short {
            break;
        }
    }
    assert_eq!(
        walked, ids,
        "the walk must serve every subscription of the tenant exactly once, in id order"
    );
    // The other tenant's documents are not on this walk. 4.14: operations
    // "only apply to the information of the specified `Tenant` in isolation".
    assert!(
        d.list_page(b, Kind::Subscription, None, 100)
            .await
            .expect("list_page")
            .is_empty(),
        "a tenant with no subscriptions pages empty, whatever another tenant holds"
    );
    // Entities page like every other kind. 5.9.2.4's registration-vs-entity
    // conflict check reads them this way — it must see every Entity of the
    // tenant and has no TooManyResults to raise — so a backend may not refuse
    // this walk for volume either, and may not build the page by
    // materializing the tenant first.
    //
    // 4.22 applies INSIDE the page: an expired Entity is not there to be
    // served, and dropping one after the limit was applied would hand back a
    // short page, which every walker reads as the end of the tenant.
    let stem = format!("urn:ngsi-ld:{prefix}:page:");
    let live: Vec<String> = (0..4).map(|i| format!("{stem}live{i}")).collect();
    // sorts between live1 and live2, so it falls inside the first page below
    let expired = format!("{stem}live1x");
    for id in &live {
        d.upsert(a, Kind::Entity, id, doc(id))
            .await
            .expect("upsert page entity");
    }
    let mut gone_doc = doc(&expired);
    gone_doc["expiresAt"] = json!("2000-01-01T00:00:00.000Z");
    d.upsert(a, Kind::Entity, &expired, gone_doc)
        .await
        .expect("upsert expired entity");

    let page = d
        .list_page(a, Kind::Entity, Some(&stem), 3)
        .await
        .expect("list_page entities");
    let served: Vec<&str> = page.iter().filter_map(|d| d["id"].as_str()).collect();
    assert_eq!(
        served,
        [live[0].as_str(), live[1].as_str(), live[2].as_str()],
        "an expired entity may neither be served nor consume a slot of the page"
    );

    let mut walked: Vec<String> = Vec::new();
    let mut after = stem.clone();
    loop {
        let page = d
            .list_page(a, Kind::Entity, Some(&after), 2)
            .await
            .expect("list_page entities");
        let short = page.len() < 2;
        let mut moved = false;
        for doc in &page {
            let id = doc["id"].as_str().expect("a stored entity keeps its id");
            assert!(
                id > after.as_str(),
                "`after` is exclusive: {id} after {after}"
            );
            after = id.to_owned();
            moved = true;
            if id.starts_with(stem.as_str()) {
                walked.push(id.to_owned());
            }
        }
        if short || !moved || !after.starts_with(stem.as_str()) {
            break;
        }
    }
    assert_eq!(
        walked, live,
        "the walk must serve every live entity of the tenant exactly once, in id \
         order, and never the expired one"
    );
    for id in live.iter().chain(std::iter::once(&expired)) {
        d.delete(a, Kind::Entity, id)
            .await
            .expect("delete page entity");
    }

    // 4.22 at the read boundary, the part a backend must not decide for
    // itself. The stamp marks "a certain Entity, Property or Relationship"
    // invalid; it is not a store-wide rule that hides any document carrying
    // one, and it reaches every Attribute of an Entity, sub-Attributes
    // included.
    let exp_e = format!("urn:ngsi-ld:{prefix}:expiry:entity");
    let mut expired_entity = doc(&exp_e);
    expired_entity["expiresAt"] = json!("2000-01-01T00:00:00.000Z");
    d.upsert(a, Kind::Entity, &exp_e, expired_entity)
        .await
        .expect("upsert expired entity");
    assert!(
        d.get(a, Kind::Entity, &exp_e).await.expect("get").is_none(),
        "an Entity past its expiresAt reads absent, not just out of a page"
    );
    d.delete(a, Kind::Entity, &exp_e).await.expect("delete");

    // An Attribute whose every instance expired leaves; the Entity carrying
    // it does not, and neither does a live sibling.
    let exp_a = format!("urn:ngsi-ld:{prefix}:expiry:attr");
    let iri = |n: &str| format!("https://uri.etsi.org/ngsi-ld/default-context/{n}");
    let mut with_attrs = doc(&exp_a);
    with_attrs[iri("gone")] = json!([{"value": 1, "expiresAt": "2000-01-01T00:00:00.000Z"}]);
    with_attrs[iri("kept")] = json!([{
        "value": 2,
        iri("subgone"): [{"value": 3, "expiresAt": "2000-01-01T00:00:00.000Z"}],
        iri("subkept"): [{"value": 4}],
    }]);
    // 4.6.3 leaves the seconds-fraction separator open, and ',' (0x2C) sorts
    // before both '.' and 'Z', so the two stamps have to be read as instants:
    // a backend that compares them as bytes serves this live Attribute as
    // expired. Every backend answers the same here or it is not the same
    // store.
    with_attrs[iri("commakept")] = json!([{"value": 5, "expiresAt": "2999-01-01T00:00:00,500Z"}]);
    d.upsert(a, Kind::Entity, &exp_a, with_attrs)
        .await
        .expect("upsert entity with an expired attribute");
    let served = d
        .get(a, Kind::Entity, &exp_a)
        .await
        .expect("get")
        .expect("an Entity outlives the expiry of one of its Attributes");
    assert!(
        served.get(iri("gone")).is_none(),
        "an Attribute whose only instance expired is not served"
    );
    assert_eq!(
        served[iri("kept")][0]["value"],
        json!(2),
        "a live Attribute survives its sibling's expiry"
    );
    assert!(
        served[iri("kept")][0].get(iri("subgone")).is_none(),
        "a sub-Attribute is a Property or Relationship too: past its stamp it \
         is not served"
    );
    assert_eq!(
        served[iri("kept")][0][iri("subkept")][0]["value"],
        json!(4),
        "a live sub-Attribute survives its sibling's expiry"
    );
    assert_eq!(
        served[iri("commakept")][0]["value"],
        json!(5),
        "a comma seconds-fraction is an instant, not a byte string"
    );
    // attrs= asks for an Entity that holds one of them NOW: an Attribute
    // whose every instance expired is not held (4.22), so an answer that
    // claims to be decided must not name the Entity for it. An undecided
    // one is re-judged by the caller over the served (stripped) rows.
    let only_gone = [iri("gone")];
    let by_attr = d
        .query_entities(
            a,
            &crate::filter::EntityFilter {
                attrs: Some(&only_gone),
                ..Default::default()
            },
        )
        .await
        .expect("query_entities");
    assert!(
        !by_attr.decided || by_attr.rows.iter().all(|r| r["id"] != exp_a.as_str()),
        "attrs= over an Attribute whose only instance expired: a decided \
         answer named the Entity"
    );
    assert!(
        by_attr
            .rows
            .iter()
            .filter(|r| r["id"] == exp_a.as_str())
            .all(|r| r.get(iri("gone")).is_none()),
        "a query served an expired Attribute"
    );
    d.delete(a, Kind::Entity, &exp_a).await.expect("delete");

    // batch_upsert takes each id once: the API splits repeats into rounds
    // (5.6.8 / 5.6.10), and what a backend does with a repeat inside one
    // call is its own guess (Postgres refuses an ON CONFLICT row affected
    // twice). A repeat is refused on every arm, and nothing is written.
    let dup = format!("urn:ngsi-ld:{prefix}:dup");
    assert!(
        d.batch_upsert(a, vec![(dup.clone(), doc(&dup)), (dup.clone(), doc(&dup))])
            .await
            .is_err(),
        "batch_upsert with an id twice must be refused"
    );
    assert!(
        d.get(a, Kind::Entity, &dup).await.expect("get").is_none(),
        "a refused batch_upsert wrote nothing"
    );

    // 5.8.6: an expired SUBSCRIPTION is not deleted and stays retrievable —
    // the API turns the stamp into status "expired" and keeps it updatable.
    // A backend that hid every document with a past expiresAt would lose it.
    let exp_s = format!("urn:ngsi-ld:{prefix}:expiry:sub");
    d.upsert(
        a,
        Kind::Subscription,
        &exp_s,
        json!({
            "id": exp_s,
            "type": "Subscription",
            "expiresAt": "2000-01-01T00:00:00.000Z",
        }),
    )
    .await
    .expect("upsert expired subscription");
    assert!(
        d.get(a, Kind::Subscription, &exp_s)
            .await
            .expect("get")
            .is_some(),
        "an expired Subscription stays retrievable (5.8.6)"
    );
    d.delete(a, Kind::Subscription, &exp_s)
        .await
        .expect("delete");

    // `delete_entity_if` decides and deletes under one lock, and it decides
    // on the STORED document. 5.6.6.4: an Entity the caller's selector
    // excludes "is not known" for the operation, which is the same answer an
    // absent Entity gets — so a refusal and a miss are both `false`, and a
    // refusal must leave the document exactly where it was.
    let cond = format!("urn:ngsi-ld:{prefix}:cond");
    d.upsert(a, Kind::Entity, &cond, doc(&cond))
        .await
        .expect("upsert conditional-delete entity");
    assert!(
        !d.delete_entity_if(a, &cond, &|_| false)
            .await
            .expect("delete_entity_if"),
        "a refused predicate reports no deletion"
    );
    assert!(
        d.get(a, Kind::Entity, &cond).await.expect("get").is_some(),
        "a refused delete leaves the document in place"
    );
    // 4.14: the predicate never even runs for another tenant's document.
    assert!(
        !d.delete_entity_if(b, &cond, &|_| panic!(
            "another tenant's document reached the predicate"
        ))
        .await
        .expect("delete_entity_if"),
        "an Entity of another tenant is absent here"
    );
    assert!(
        d.get(a, Kind::Entity, &cond).await.expect("get").is_some(),
        "a delete addressed to another tenant may not touch this one"
    );
    assert!(
        d.delete_entity_if(a, &cond, &|v| v["id"] == cond.as_str())
            .await
            .expect("delete_entity_if"),
        "the predicate is handed the stored document, not the id"
    );
    assert!(
        d.get(a, Kind::Entity, &cond).await.expect("get").is_none(),
        "an accepted predicate deletes"
    );
    assert!(
        !d.delete_entity_if(a, &cond, &|_| true)
            .await
            .expect("delete_entity_if"),
        "an absent Entity is not deleted, whatever the predicate answers"
    );

    // `list_slice` is the window a 5.5.9.2 limit/offset listing serves from,
    // and it must agree with the walk above on both the order and the set:
    // the same ids, the same order, and a total that counts the whole match
    // set rather than the page. A backend may not refuse it for volume — the
    // window bounds the result by construction.
    let (page, total) = d
        .list_slice(a, Kind::Subscription, 0, 3)
        .await
        .expect("list_slice");
    assert_eq!(total, ids.len(), "the total counts the set, not the page");
    assert_eq!(page.len(), 3, "limit is a maximum, and 3 were available");
    for (i, doc) in page.iter().enumerate() {
        assert_eq!(
            doc["id"].as_str(),
            Some(ids[i].as_str()),
            "the window is the id-ordered prefix: {page:?}"
        );
    }
    // Every single-element window lands on the element the walk has there.
    for (i, want) in ids.iter().enumerate() {
        let (page, _) = d
            .list_slice(a, Kind::Subscription, i, 1)
            .await
            .expect("list_slice");
        assert_eq!(
            page.first().and_then(|d| d["id"].as_str()),
            Some(want.as_str()),
            "offset {i} served the wrong element: {page:?}"
        );
    }
    // limit 0 is legal (6.3.10, with count): the count without the page.
    let (page, total) = d
        .list_slice(a, Kind::Subscription, 0, 0)
        .await
        .expect("list_slice");
    assert!(page.is_empty(), "limit 0 returns no elements: {page:?}");
    assert_eq!(total, ids.len(), "limit 0 still counts the whole set");
    // An offset past the end is an empty page, not an error, and the total
    // is unchanged by where the client asked to start.
    let (page, total) = d
        .list_slice(a, Kind::Subscription, ids.len() + 10, 5)
        .await
        .expect("list_slice");
    assert!(page.is_empty(), "past the end is empty: {page:?}");
    assert_eq!(total, ids.len(), "an offset does not change the match set");
    // 4.14 again: the window is inside one tenant.
    let (page, total) = d
        .list_slice(b, Kind::Subscription, 0, 100)
        .await
        .expect("slice");
    assert!(
        page.is_empty() && total == 0,
        "another tenant's rows reached this window: {page:?} / {total}"
    );

    for id in ids.iter().filter(|i| *i != &sub) {
        d.delete(a, Kind::Subscription, id).await.expect("delete");
    }

    // 5.2.14.2 delivery bookkeeping. A backend may write this as one
    // statement instead of a read-modify-write; the result must not depend on
    // which it chose, so the rule is pinned here rather than in one backend's
    // own tests.
    let t1 = "2020-01-01T00:00:00.000Z";
    let t2 = "2020-01-02T00:00:00.000Z";
    let first = d
        .record_delivery(a, Kind::Subscription, &sub, t1)
        .await
        .expect("record_delivery")
        .expect("the subscription is there");
    let n1 = &first.doc["notification"];
    assert_eq!(n1["timesSent"], 1, "the first attempt moves timesSent to 1");
    assert_eq!(
        n1["lastNotification"], t1,
        "lastNotification takes the stamp"
    );
    assert_eq!(n1["lastSuccess"], t1, "lastSuccess takes the stamp");
    assert_eq!(n1["status"], "ok", "the attempt leaves the notification ok");
    assert!(
        first.doc.get("status").is_none(),
        "the top-level status is a rendered member, not a stored one: {}",
        first.doc
    );
    assert!(
        first.prev_success.is_none(),
        "a subscription that never succeeded has no previous lastSuccess"
    );

    let second = d
        .record_delivery(a, Kind::Subscription, &sub, t2)
        .await
        .expect("record_delivery")
        .expect("the subscription is still there");
    assert_eq!(
        second.doc["notification"]["timesSent"], 2,
        "timesSent counts attempts, it does not reset"
    );
    assert_eq!(
        second.prev_success.as_ref().and_then(Value::as_str),
        Some(t1),
        "the overwritten lastSuccess comes back, or a failed attempt cannot \
         roll it back"
    );

    // 5.8.6: a subscription deleted between matching and delivery has no row
    // to book against, and nothing may be sent.
    assert!(
        d.record_delivery(a, Kind::Subscription, &gone, t1)
            .await
            .expect("record_delivery")
            .is_none(),
        "an absent subscription books nothing"
    );

    // T-5159: the batched stamp is the same rule, one answer per attempt in
    // input order, and a batch never books another tenant's row.
    let t3 = "2020-01-03T00:00:00.000Z";
    let batch = d
        .record_deliveries(
            a,
            Kind::Subscription,
            &[(gone.as_str(), t3, 1), (sub.as_str(), t3, 1)],
        )
        .await
        .expect("record_deliveries");
    assert_eq!(batch.len(), 2, "one answer per attempt: {batch:?}");
    assert!(
        batch.first().is_some_and(Option::is_none),
        "an absent subscription books nothing"
    );
    let third = batch
        .get(1)
        .and_then(Option::as_ref)
        .expect("the subscription is booked");
    assert_eq!(third.doc["notification"]["timesSent"], 3);
    assert_eq!(third.doc["notification"]["lastSuccess"], t3);
    assert_eq!(
        third.prev_success.as_ref().and_then(Value::as_str),
        Some(t2),
        "each attempt of a batch gets back the lastSuccess it overwrote"
    );
    // T-6371: a merged window books its notifications at once, at its last
    // instant.
    let t4 = "2020-01-04T00:00:00.000Z";
    let merged = d
        .record_deliveries(a, Kind::Subscription, &[(sub.as_str(), t4, 3)])
        .await
        .expect("record_deliveries");
    let merged = merged
        .first()
        .and_then(Option::as_ref)
        .expect("the subscription is booked");
    assert_eq!(merged.doc["notification"]["timesSent"], 6);
    assert_eq!(merged.doc["notification"]["lastNotification"], t4);
    let elsewhere = d
        .record_deliveries(b, Kind::Subscription, &[(sub.as_str(), t3, 1)])
        .await
        .expect("record_deliveries");
    assert!(
        elsewhere.first().is_some_and(Option::is_none),
        "a batch in another tenant books nothing: {elsewhere:?}"
    );

    assert!(
        d.delete(a, Kind::Subscription, &sub).await.expect("delete"),
        "the contract's own subscription must be removable"
    );

    // ADR-0021: a stored @context is a Tenant's document. "Hosted" and
    // "ImplicitlyCreated" rows hold term mappings authored through one
    // Tenant's requests, and 5.5.7 makes those mappings decide what that
    // Tenant's payloads mean, so a backend that answered every caller would
    // hand one Tenant's meaning of its own data to another. "Cached" is a
    // copy of a public document (5.13.1) and belongs to no Tenant.
    let hosted_id = format!("{prefix}-ctx-hosted");
    let cached_id = format!("{prefix}-ctx-cached");
    let hosted = json!({"localId": hosted_id, "kind": "Hosted", "owner": a.as_str(),
                        "body": {"@context": {"t": "https://example.org/t"}}});
    d.context_put(Some(a), &hosted_id, hosted.clone())
        .await
        .expect("the owning tenant stores its own @context");
    d.context_put(
        None,
        &cached_id,
        json!({"localId": cached_id, "kind": "Cached", "body": {"@context": {}}}),
    )
    .await
    .expect("a Cached copy belongs to no tenant");

    assert!(
        d.context_get(Some(a), &hosted_id)
            .await
            .expect("context_get")
            .is_some(),
        "the owning tenant must read its own @context"
    );
    assert!(
        d.context_get(Some(b), &hosted_id)
            .await
            .expect("context_get")
            .is_none(),
        "another tenant's Hosted @context must be as absent as one never stored"
    );
    assert!(
        d.context_get(None, &hosted_id)
            .await
            .expect("context_get")
            .is_none(),
        "no tenant in scope must reach a tenant's @context"
    );
    assert!(
        d.context_list_meta(Some(b))
            .await
            .expect("context_list_meta")
            .iter()
            .all(|r| r["localId"] != hosted_id.as_str()),
        "another tenant's @context must not be listed"
    );
    assert!(
        !d.context_delete(Some(b), &hosted_id)
            .await
            .expect("context_delete"),
        "another tenant must not delete it"
    );
    assert!(
        d.context_put(Some(b), &hosted_id, hosted).await.is_err(),
        "another tenant must not overwrite it either"
    );
    assert!(
        d.context_get(Some(a), &hosted_id)
            .await
            .expect("context_get")
            .is_some(),
        "the owner's @context must survive every foreign attempt"
    );
    for t in [Some(a), Some(b), None] {
        assert!(
            d.context_get(t, &cached_id)
                .await
                .expect("context_get")
                .is_some(),
            "a Cached copy is a public document every tenant reaches"
        );
    }
    assert!(
        d.context_delete(Some(a), &hosted_id)
            .await
            .expect("context_delete"),
        "the contract's own @context must be removable by its owner"
    );
    assert!(
        d.context_delete(None, &cached_id)
            .await
            .expect("context_delete"),
        "and the Cached copy by anyone"
    );

    context_rows_are_locked(d, a, b, prefix).await;

    for id in [&e1, &e2] {
        assert!(
            d.delete(a, Kind::Entity, id).await.expect("delete"),
            "the contract's own rows must be removable"
        );
    }
}

/// Drive two futures to completion together on the calling task, so their
/// store calls interleave (the crate carries no executor of its own).
async fn both<A: std::future::Future, B: std::future::Future>(
    x: A,
    y: B,
) -> (A::Output, B::Output) {
    let (mut x, mut y) = (Box::pin(x), Box::pin(y));
    let (mut xo, mut yo) = (None, None);
    std::future::poll_fn(|cx| {
        if xo.is_none() {
            if let std::task::Poll::Ready(v) = std::future::Future::poll(x.as_mut(), cx) {
                xo = Some(v);
            }
        }
        if yo.is_none() {
            if let std::task::Poll::Ready(v) = std::future::Future::poll(y.as_mut(), cx) {
                yo = Some(v);
            }
        }
        match (xo.take(), yo.take()) {
            (Some(a), Some(b)) => std::task::Poll::Ready((a, b)),
            (a, b) => {
                xo = a;
                yo = b;
                std::task::Poll::Pending
            }
        }
    })
    .await
}

/// 5.13.3.5 + ADR-0021 for the @context rows: a row has a kind, and a
/// read-modify-write of one holds its lock, so a usage bump racing a
/// delete leaves it deleted and two racing bumps both count.
async fn context_rows_are_locked(
    d: &dyn CurrentStateDriver,
    a: &TenantId,
    b: &TenantId,
    prefix: &str,
) {
    let id = format!("{prefix}-ctx-bumped");
    assert!(
        d.context_put(None, &id, json!({"localId": id, "body": {"@context": {}}}))
            .await
            .is_err(),
        "a row without a kind must be refused"
    );
    let bump = || -> crate::MutateFn<'static> {
        Box::new(|row| {
            let o = row.as_object_mut().ok_or(())?;
            let n = o.get("numberOfHits").and_then(Value::as_u64).unwrap_or(0);
            o.insert("numberOfHits".into(), json!(n.saturating_add(1)));
            Ok(())
        })
    };
    assert!(
        d.context_mutate(None, &id, bump())
            .await
            .expect("context_mutate")
            .is_none(),
        "context_mutate of an absent row must answer None"
    );
    assert!(
        d.context_get(None, &id)
            .await
            .expect("context_get")
            .is_none(),
        "context_mutate of an absent row must not insert it"
    );
    d.context_put(
        None,
        &id,
        json!({"localId": id, "kind": "Cached", "numberOfHits": 0, "body": {"@context": {}}}),
    )
    .await
    .expect("context_put");
    let (x, y) = both(
        d.context_mutate(Some(a), &id, bump()),
        d.context_mutate(Some(b), &id, bump()),
    )
    .await;
    assert!(
        matches!((x, y), (Ok(Some(Ok(()))), Ok(Some(Ok(()))))),
        "both bumps of a Cached row must apply"
    );
    let hits = d
        .context_get(None, &id)
        .await
        .expect("context_get")
        .map(|r| r["numberOfHits"].clone());
    assert_eq!(hits, Some(json!(2)), "two racing bumps must both count");

    let hosted = format!("{prefix}-ctx-owned");
    d.context_put(
        Some(a),
        &hosted,
        json!({"localId": hosted, "kind": "Hosted", "owner": a.as_str(), "body": {"@context": {}}}),
    )
    .await
    .expect("context_put");
    assert!(
        d.context_mutate(Some(b), &hosted, bump())
            .await
            .expect("context_mutate")
            .is_none(),
        "another tenant's @context must be as absent to a mutate as to a read"
    );
    assert!(
        d.context_delete(Some(a), &hosted)
            .await
            .expect("context_delete"),
        "the owner removes its @context"
    );
    assert!(
        d.context_mutate(Some(a), &hosted, bump())
            .await
            .expect("context_mutate")
            .is_none(),
        "a bump after the delete must find nothing"
    );
    assert!(
        d.context_get(Some(a), &hosted)
            .await
            .expect("context_get")
            .is_none(),
        "a bump racing a delete must leave the row deleted"
    );
    assert!(d.context_delete(None, &id).await.expect("context_delete"));
}

/// Hold a temporal driver to the same shape. A driver that answers
/// `supported() == false` declares it keeps no history and is held to
/// nothing else.
pub async fn run_temporal_contract(
    d: &dyn TemporalDriver,
    a: &TenantId,
    b: &TenantId,
    prefix: &str,
) {
    if !d.supported() {
        return;
    }
    let e1 = format!("urn:ngsi-ld:{prefix}:t1");
    let gone = format!("urn:ngsi-ld:{prefix}:tabsent");

    assert!(
        d.get(a, &gone).await.expect("get").is_none(),
        "get of an absent temporal document must answer None"
    );
    assert!(
        !d.delete(a, &gone).await.expect("delete"),
        "delete of an absent temporal document must answer false"
    );
    let missed = d
        .mutate::<(), ()>(a, &gone, |_| Ok(()))
        .await
        .expect("mutate");
    assert!(
        missed.is_none(),
        "mutate of an absent document must answer None"
    );
    assert!(
        d.get(a, &gone).await.expect("get").is_none(),
        "mutate of an absent document must not insert it (047_06)"
    );

    assert!(
        d.create(a, &e1, doc(&e1)).await.expect("create"),
        "the first create reports true"
    );
    assert!(
        !d.create(a, &e1, doc(&e1)).await.expect("create"),
        "a create over an existing id reports false"
    );
    let rejected = d
        .mutate::<(), &'static str>(a, &e1, |v| {
            v["marker"] = json!(2);
            Err("rejected")
        })
        .await
        .expect("mutate");
    assert_eq!(rejected, Some(Err("rejected")));
    assert!(
        d.get(a, &e1)
            .await
            .expect("get")
            .expect("row")
            .get("marker")
            .is_none(),
        "a rejected mutate must commit nothing"
    );

    assert!(
        d.get(b, &e1).await.expect("get").is_none(),
        "another tenant must not read the temporal document"
    );
    assert!(
        !d.delete(b, &e1).await.expect("delete"),
        "another tenant must not delete it"
    );
    assert!(
        d.list(b)
            .await
            .expect("list")
            .iter()
            .all(|r| r["id"] != e1.as_str()),
        "another tenant must not list it"
    );

    assert!(
        d.delete(a, &e1).await.expect("delete"),
        "the contract's own rows must be removable"
    );
}

/// Hold a driver to tenant isolation (T-3914): `a` and `b` each hold an
/// Entity, a Subscription, a Registration and a temporal document under the
/// SAME ids, and nothing done as `a` reads, changes or deletes `b`'s copy.
/// The walk covers every tenant-taking family the API reaches: one-row and
/// batch writes, the three list reads, the query, delivery bookkeeping, the
/// registration lookup, the temporal reads and the drain, and last a purge
/// of `a`, which must leave `b` whole.
///
/// The caller picks the pair that matters for its backend: two names a
/// LIKE pattern would confuse (`a_b`, `axb`), the default Tenant beside a
/// named one, a name that extends the other. `a` is purged, so it must not
/// be a Tenant other data lives in. `cs` and `tp` may be the same store.
pub async fn run_tenant_isolation_contract(
    cs: &dyn CurrentStateDriver,
    tp: &dyn TemporalDriver,
    a: &TenantId,
    b: &TenantId,
    prefix: &str,
) {
    use crate::filter::{EntityFilter, TemporalFilter};
    assert_ne!(a, b, "the walk needs two Tenants");
    let e = format!("urn:ngsi-ld:{prefix}:shared");
    let sub = format!("urn:ngsi-ld:Subscription:{prefix}:shared");
    let reg = format!("urn:ngsi-ld:ContextSourceRegistration:{prefix}:shared");
    let ty = "https://uri.etsi.org/ngsi-ld/default-context/T";
    let mine = |t: &TenantId| json!({"id": e, "type": ty, "who": t.as_str()});
    let ids = [e.clone()];
    let ids_ref = [e.as_str()];

    for t in [a, b] {
        assert!(
            cs.create(t, Kind::Entity, &e, mine(t))
                .await
                .expect("create"),
            "{}: the shared id is new in each Tenant",
            t.as_str()
        );
        cs.create(
            t,
            Kind::Subscription,
            &sub,
            json!({"id": sub, "type": "Subscription", "who": t.as_str()}),
        )
        .await
        .expect("subscription");
        cs.create(
            t,
            Kind::Registration,
            &reg,
            json!({"id": reg, "type": "ContextSourceRegistration", "who": t.as_str(),
                   "information": [{"entities": [{"type": ty}]}], "endpoint": "http://cs.example"}),
        )
        .await
        .expect("registration");
    }
    // `b`'s copies as they stand; every step below compares against these.
    let b_entity = cs.get(b, Kind::Entity, &e).await.expect("get");
    let b_sub = cs.get(b, Kind::Subscription, &sub).await.expect("get");
    // A macro, not a closure: the check awaits, and an async closure that
    // borrows the ids cannot be called twice.
    macro_rules! b_unchanged {
        ($step:literal) => {
            assert_eq!(
                cs.get(b, Kind::Entity, &e).await.expect("get"),
                b_entity,
                "{} as {} changed {}'s Entity",
                $step,
                a.as_str(),
                b.as_str()
            );
            assert_eq!(
                cs.get(b, Kind::Subscription, &sub).await.expect("get"),
                b_sub,
                "{} as {} changed {}'s Subscription",
                $step,
                a.as_str(),
                b.as_str()
            );
        };
    }
    let only_a = |rows: &[Value], what: &str| {
        let shared: Vec<&Value> = rows.iter().filter(|r| r["id"] == e.as_str()).collect();
        assert_eq!(shared.len(), 1, "{what}: the shared id once, {rows:?}");
        assert_eq!(
            shared[0]["who"],
            a.as_str(),
            "{what} served another Tenant's copy"
        );
    };

    // Reads.
    let got = cs
        .get(a, Kind::Entity, &e)
        .await
        .expect("get")
        .expect("row");
    assert_eq!(got["who"], a.as_str(), "get served another Tenant's copy");
    only_a(&cs.list(a, Kind::Entity).await.expect("list"), "list");
    only_a(
        &cs.list_page(a, Kind::Entity, None, 1000)
            .await
            .expect("page"),
        "list_page",
    );
    only_a(
        &cs.list_slice(a, Kind::Entity, 0, 1000)
            .await
            .expect("slice")
            .0,
        "list_slice",
    );
    let q = cs
        .query_entities(
            a,
            &EntityFilter {
                ids: Some(&ids_ref),
                ..Default::default()
            },
        )
        .await
        .expect("query");
    only_a(&q.rows, "query_entities");
    let subs = cs.list(a, Kind::Subscription).await.expect("list");
    assert!(
        subs.iter()
            .filter(|s| s["id"] == sub.as_str())
            .all(|s| s["who"] == a.as_str()),
        "the Subscription list served another Tenant's copy: {subs:?}"
    );
    let regs = cs
        .matching_registrations(a, None, None)
        .await
        .expect("registrations");
    let shared: Vec<&Value> = regs.iter().filter(|r| r["id"] == reg.as_str()).collect();
    assert!(
        shared.len() == 1 && shared[0]["who"] == a.as_str(),
        "matching_registrations served another Tenant's copy: {regs:?}"
    );

    // Writes, one by one and batched.
    cs.mutate::<(), ()>(a, Kind::Entity, &e, |d| {
        d["touched"] = json!(true);
        Ok(())
    })
    .await
    .expect("mutate");
    b_unchanged!("mutate");
    cs.upsert(
        a,
        Kind::Entity,
        &e,
        json!({"id": e, "type": ty, "who": a.as_str(), "n": 1}),
    )
    .await
    .expect("upsert");
    b_unchanged!("upsert");
    assert_eq!(
        cs.batch_create(a, vec![(e.clone(), mine(a))])
            .await
            .expect("batch_create"),
        [false],
        "a batch create sees only its own Tenant's row"
    );
    b_unchanged!("batch_create");
    assert_eq!(
        cs.batch_upsert(a, vec![(e.clone(), mine(a))])
            .await
            .expect("batch_upsert"),
        [false],
        "a batch upsert replaces its own Tenant's row"
    );
    b_unchanged!("batch_upsert");
    cs.batch_mutate::<()>(a, &ids, |_, d| {
        d["batched"] = json!(true);
        Ok(())
    })
    .await
    .expect("batch_mutate");
    b_unchanged!("batch_mutate");
    cs.record_delivery(a, Kind::Subscription, &sub, "2026-10-11T00:00:00Z")
        .await
        .expect("record_delivery")
        .expect("a's Subscription");
    b_unchanged!("record_delivery");
    assert!(
        cs.delete_entity_if(a, &e, &|_| true)
            .await
            .expect("delete_entity_if"),
        "a conditional delete removes its own Tenant's row"
    );
    b_unchanged!("delete_entity_if");
    cs.create(a, Kind::Entity, &e, mine(a))
        .await
        .expect("recreate");
    assert_eq!(
        cs.batch_delete(a, &ids).await.expect("batch_delete"),
        [true],
        "a batch delete removes its own Tenant's row"
    );
    b_unchanged!("batch_delete");
    cs.create(a, Kind::Entity, &e, mine(a))
        .await
        .expect("recreate");
    assert!(cs.delete(a, Kind::Entity, &e).await.expect("delete"));
    b_unchanged!("delete");
    cs.create(a, Kind::Entity, &e, mine(a))
        .await
        .expect("recreate");

    // History under the same id.
    if tp.supported() {
        let instance = |t: &TenantId| {
            json!({"n": [{"type": "Property", "value": t.as_str(),
                          "observedAt": "2026-10-11T00:00:00Z",
                          "instanceId": format!("urn:ngsi-ld:{prefix}:{}", t.as_str())}]})
        };
        for t in [a, b] {
            tp.create(t, &e, mine(t)).await.expect("temporal create");
            tp.temporal_append(t, &e, &mine(t), &instance(t))
                .await
                .expect("append");
        }
        let b_history = tp.get(b, &e).await.expect("get");
        macro_rules! history_unchanged {
            ($step:literal) => {
                assert_eq!(
                    tp.get(b, &e).await.expect("get"),
                    b_history,
                    "{} as {} changed {}'s history",
                    $step,
                    a.as_str(),
                    b.as_str()
                );
            };
        }
        let values = |doc: &Value| doc["n"].to_string();
        let one = tp
            .get_temporal(a, &e, &TemporalFilter::default())
            .await
            .expect("get_temporal")
            .expect("a's history");
        assert!(
            !values(&one).contains(&format!("\"{}\"", b.as_str())),
            "get_temporal served another Tenant's instances: {one}"
        );
        let rows = tp
            .query_temporal(
                a,
                &TemporalFilter {
                    ids: Some(&ids_ref),
                    ..Default::default()
                },
            )
            .await
            .expect("query_temporal")
            .rows;
        assert_eq!(
            rows.len(),
            1,
            "query_temporal: the shared id once, {rows:?}"
        );
        assert!(
            !values(&rows[0]).contains(&format!("\"{}\"", b.as_str())),
            "query_temporal served another Tenant's instances: {rows:?}"
        );
        assert!(
            tp.list(a)
                .await
                .expect("list")
                .iter()
                .filter(|r| r["id"] == e.as_str())
                .count()
                == 1,
            "the temporal list holds the shared id once"
        );
        tp.event_list(&[crate::TemporalEvent {
            op: crate::TemporalOp::AttrModified,
            tenant: a.clone(),
            entity_id: e.clone(),
            shell: mine(a),
            attr: "n".into(),
            instance: json!({"type": "Property", "value": "drained",
                             "observedAt": "2026-10-11T00:01:00Z",
                             "instanceId": format!("urn:ngsi-ld:{prefix}:drained")}),
        }])
        .await
        .expect("event_list");
        history_unchanged!("event_list");
        tp.mutate::<(), ()>(a, &e, |d| {
            d["touched"] = json!(true);
            Ok(())
        })
        .await
        .expect("mutate");
        history_unchanged!("temporal mutate");
        assert!(tp.delete(a, &e).await.expect("delete"));
        history_unchanged!("temporal delete");
        tp.create(a, &e, mine(a)).await.expect("recreate");
        tp.purge_tenant(a).await.expect("temporal purge");
        assert!(
            tp.get(a, &e).await.expect("get").is_none(),
            "the purge empties a's history"
        );
        history_unchanged!("temporal purge");
        assert!(tp.delete(b, &e).await.expect("cleanup"));
    }

    // A purge of `a` leaves `b` whole.
    match cs.purge_tenant(a).await {
        Ok(_) => {
            assert!(
                cs.get(a, Kind::Entity, &e).await.expect("get").is_none(),
                "the purge empties a"
            );
            b_unchanged!("purge_tenant");
            assert!(
                cs.get(b, Kind::Registration, &reg)
                    .await
                    .expect("get")
                    .is_some(),
                "the purge of {} took {}'s Registration",
                a.as_str(),
                b.as_str()
            );
        }
        Err(antares_model::NgsiError::OperationNotSupported(_)) => {
            for kind in [Kind::Entity, Kind::Subscription, Kind::Registration] {
                let id = match kind {
                    Kind::Entity => &e,
                    Kind::Subscription => &sub,
                    _ => &reg,
                };
                cs.delete(a, kind, id).await.expect("cleanup");
            }
        }
        Err(other) => panic!("purge_tenant: {other:?}"),
    }
    for (kind, id) in [
        (Kind::Entity, &e),
        (Kind::Subscription, &sub),
        (Kind::Registration, &reg),
    ] {
        assert!(
            cs.delete(b, kind, id).await.expect("cleanup"),
            "{}'s {kind:?} survived",
            b.as_str()
        );
    }
}
