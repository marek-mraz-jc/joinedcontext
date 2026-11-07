// SPDX-License-Identifier: EUPL-1.2
//! Candidate selection and full evaluation of a change against subscriptions: attribute diff, linked-entity and view projection, the per-subscription @context.

use super::*;

/// Where the matcher reads candidates from: the indexed mirror (both bus
/// modes wire one), with the store scan only as the never-wired
/// fallback so a missing mirror degrades to correct-but-slow.
pub(super) async fn subs_for(
    st: &AppState,
    tenant: &TenantId,
    types: &[&str],
    changed: &[&str],
    queued: u64,
) -> Vec<Arc<Value>> {
    match &st.sub_mirror {
        Some(m) => m.candidates_before(tenant.as_str(), types, changed, queued),
        // ponytail: the scan reads subscriptions as they are now, so a change
        // drained late may reach one made after it; only the never-wired path.
        // The scan is the fallback, so its own failure may not be read as
        // "this tenant has no subscriptions" either: that is the silence
        // the mirror seed refuses to install.
        None => match st.store.list(tenant, Kind::Subscription).await {
            Ok(subs) => subs.into_iter().map(Arc::new).collect(),
            Err(e) => {
                tracing::error!(
                    "subscription scan failed for tenant {}: {e}; no candidate matched this change",
                    tenant.as_str()
                );
                Vec::new()
            }
        },
    }
}

/// Whether two attribute instances are the same once the volatile members
/// — the timestamps the broker itself stamps, and the instance identity —
/// are set aside. Compared in place: this runs per changed attribute per
/// matching subscription per event, and materializing two stripped copies of
/// every JSON tree to throw them away one comparison later is the cost that
/// multiplies by.
pub(super) fn stable_eq(a: &Value, b: &Value) -> bool {
    fn volatile(k: &str) -> bool {
        matches!(k, "createdAt" | "modifiedAt" | "instanceId")
            || k == antares_jsonld::compact::MODIFIED_BY_KEY
    }
    match (a, b) {
        (Value::Object(oa), Value::Object(ob)) => {
            let count = |o: &Map<String, Value>| o.keys().filter(|k| !volatile(k)).count();
            // by lookup rather than by zipped iteration: which order a
            // serde_json Map yields is a cargo feature away from changing,
            // and features are additive
            count(oa) == count(ob)
                && oa
                    .iter()
                    .filter(|(k, _)| !volatile(k))
                    .all(|(k, va)| ob.get(k).is_some_and(|vb| stable_eq(va, vb)))
        }
        (Value::Array(aa), Value::Array(ab)) => {
            aa.len() == ab.len() && aa.iter().zip(ab).all(|(x, y)| stable_eq(x, y))
        }
        _ => a == b,
    }
}

pub(super) fn attr_keys(doc: &Value) -> Vec<String> {
    doc.as_object()
        .map(|o| {
            o.keys()
                .filter(|k| !crate::repr::ENTITY_META.contains(&k.as_str()))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Per-attribute change classification between two internal docs.
pub(super) fn diff(before: Option<&Value>, after: Option<&Value>) -> Vec<(String, ChangeClass)> {
    let empty = Value::Object(Map::new());
    let b = before.unwrap_or(&empty);
    let a = after.unwrap_or(&empty);
    let mut keys = attr_keys(b);
    for k in attr_keys(a) {
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    let mut out = Vec::new();
    for k in keys {
        let bv = b.get(&k);
        let av = a.get(&k);
        match (bv, av) {
            (None, Some(_)) => out.push((k, ChangeClass::Created)),
            (Some(_), None) => out.push((k, ChangeClass::Deleted)),
            (Some(x), Some(y)) => {
                // instance-level deletion (by datasetId) counts as
                // attributeDeleted even when other instances survive (5.8.6)
                let bx: Vec<&Value> = x.as_array().map(|a| a.iter().collect()).unwrap_or_default();
                let by: Vec<&Value> = y.as_array().map(|a| a.iter().collect()).unwrap_or_default();
                let removed = bx
                    .iter()
                    .any(|bi| !by.iter().any(|ai| instance_ds(ai) == instance_ds(bi)));
                if removed {
                    out.push((k.clone(), ChangeClass::Deleted));
                    let survivors_changed = by.iter().any(|ai| {
                        match bx.iter().find(|bi| instance_ds(bi) == instance_ds(ai)) {
                            Some(bi) => !stable_eq(bi, ai),
                            None => true,
                        }
                    });
                    if survivors_changed {
                        out.push((k, ChangeClass::Updated));
                    }
                } else if !stable_eq(x, y) {
                    out.push((k, ChangeClass::Updated));
                }
            }
            _ => {}
        }
    }
    out
}

/// One string-valued member of a stored Subscription, absent or of another
/// JSON type reading the same as absent.
pub(super) fn sub_str<'a>(sub: &'a Value, key: &str) -> Option<&'a str> {
    sub.get(key).and_then(Value::as_str)
}

/// 4.9 EXAMPLE 13/14: linked-entity q terms (`attr{path}`) resolve through
/// the local store, same tenant. The evaluator is synchronous and the store
/// is not, so `eval` runs against a cache: a URI the cache misses is
/// recorded, reads as absent for that pass, and is fetched before the next
/// one. The first pass that misses nothing is the answer. Every miss of a
/// pass is fetched together, so the pass count is the depth of the link
/// chain rather than the number of entities it touches.
///
/// `narrow` is the caller's narrowing (ADR-0020) with the @context its
/// policy q is read against: a hop into an Entity it does not admit reads
/// as a hop into a missing one, and a resolved target loses the Attributes
/// its projection hides before the evaluator sees it (ADR-0023 rule 2).
pub(crate) async fn linked_eval<F>(
    st: &AppState,
    tenant: &TenantId,
    narrow: Option<(&crate::repr::Repr, &Context)>,
    mut eval: F,
) -> bool
where
    F: for<'a> FnMut(antares_ql::eval::EntityLookup<'a>) -> bool,
{
    let mut cache: HashMap<String, Option<Value>> = HashMap::new();
    // ponytail: the evaluator's own lookup budget bounds how many distinct
    // URIs one expression can name, so it bounds the passes too; a chain
    // deeper than that reads as unresolved, which is what exhausting the
    // budget inside the evaluator already does.
    for _ in 0..antares_ql::eval::MAX_Q_LINK_LOOKUPS {
        let missed = std::cell::RefCell::new(Vec::new());
        let out = eval(&|uri: &str| match cache.get(uri) {
            Some(doc) => doc.clone(),
            None => {
                missed.borrow_mut().push(uri.to_owned());
                None
            }
        });
        let mut missed = missed.into_inner();
        if missed.is_empty() {
            return out;
        }
        missed.sort();
        missed.dedup();
        // one read for every miss of the pass (F-R2ANT1-012); a failed read leaves them absent
        let refs: Vec<&str> = missed.iter().map(String::as_str).collect();
        let mut found = st
            .store
            .get_entities(tenant, &refs)
            .await
            .unwrap_or_default();
        for uri in missed {
            let mut doc = found.remove(&uri);
            if let (Some(d), Some((r, ctx))) = (&doc, narrow) {
                // boxed: the admission check evaluates the policy's own q,
                // which comes back here
                let admitted = Box::pin(crate::repr::policy_admits(
                    st,
                    tenant,
                    ctx,
                    r.policy_reach.as_deref(),
                    d,
                ))
                .await;
                doc = admitted.then(|| crate::repr::policy_view(d, r));
            }
            cache.insert(uri, doc);
        }
    }
    eval(&|uri: &str| cache.get(uri).cloned().flatten())
}

/// What each Entity of a notification's `data` (and each one it embeds)
/// shows under the expanded `rules` (every Attribute without rules), read
/// on its stored state now, names compacted the way `data` is. An Entity the store no longer holds, that
/// `reach` does not admit, or that no rule holds for, maps to `None`.
pub(super) async fn shown_now(
    st: &AppState,
    tenant: &TenantId,
    rules: Option<&crate::policy::Filter>,
    reach: Option<&crate::repr::PolicyReach>,
    data: &Value,
    ctx: &Context,
) -> HashMap<String, Option<crate::policy::Shown<'static>>> {
    fn ids<'a>(v: &'a Value, out: &mut std::collections::BTreeSet<&'a str>) {
        match v {
            Value::Object(o) => {
                if let (Some(id), Some(_)) = (o.get("id").and_then(Value::as_str), o.get("type")) {
                    out.insert(id);
                }
                o.values().for_each(|v| ids(v, out));
            }
            Value::Array(a) => a.iter().for_each(|v| ids(v, out)),
            _ => {}
        }
    }
    let mut wanted = std::collections::BTreeSet::new();
    ids(data, &mut wanted);
    // one read for every Entity of the notification, shared with every
    // other delivery of the tenant waiting for a read (T-5159): one by one, a
    // folded notification of 256 Entities was 256 round trips under a
    // delivery slot (T-4921), and a fan-out to 1 000 subscriptions of one
    // view read the same Entities 1 000 times. One failed read hides every
    // Entity of the notification; the failure is logged, not silent.
    let ids: Vec<String> = wanted.iter().map(|id| (*id).to_owned()).collect();
    let stored_now = super::coalesce::read_entities(st, tenant, ids).await;
    let mut out = HashMap::new();
    for id in wanted {
        let mut stored = stored_now.as_ref().and_then(|m| m.get(id));
        if let (Some(d), Some(_)) = (stored, reach) {
            let ctx = crate::repr::policy_context();
            if !crate::repr::policy_admits(st, tenant, ctx, reach, d).await {
                stored = None;
            }
        }
        let shown = stored
            .and_then(|d| crate::repr::shown(rules, d))
            .map(|s| s.map_names(|n| ctx.compact_iri(n)));
        out.insert(id.to_owned(), shown);
    }
    out
}

/// The entry of `shown` for the Entity `e`; `None` when `e` is no Entity.
pub(super) fn entity_shown<'a>(
    shown: &'a HashMap<String, Option<crate::policy::Shown<'static>>>,
    e: &Value,
) -> Option<Option<&'a crate::policy::Shown<'static>>> {
    let id = e.get("id").and_then(Value::as_str)?;
    Some(shown.get(id).and_then(Option::as_ref))
}

/// Strip a compacted notified Entity to what [`shown_now`] says it shows,
/// and every Entity it embeds the same way: an embedded one no rule holds
/// for is not embedded (ADR-0023 rule 6), its Relationship stays.
pub(super) fn project_shown(
    shown: &HashMap<String, Option<crate::policy::Shown<'static>>>,
    e: &mut Value,
    key_values: bool,
) {
    let allowed = entity_shown(shown, e).flatten().cloned();
    let Some(obj) = e.as_object_mut() else {
        return;
    };
    let allowed = allowed.unwrap_or_else(crate::policy::Shown::nothing);
    obj.retain(|k, _| k == "@context" || antares_model::is_meta(k) || allowed.allows(k));
    for v in obj.values_mut() {
        let instances: Vec<&mut Value> = match v {
            Value::Array(a) => a.iter_mut().collect(),
            other => vec![other],
        };
        for inst in instances {
            if key_values {
                if inst.get("id").is_some() && inst.get("type").is_some() {
                    match entity_shown(shown, inst) {
                        Some(Some(_)) => project_shown(shown, inst, key_values),
                        _ => {
                            let id = inst.get("id").cloned().unwrap_or(Value::Null);
                            *inst = id;
                        }
                    }
                }
                continue;
            }
            let Some(o) = inst.as_object_mut() else {
                continue;
            };
            if let Some(j) = o.get("entity") {
                if matches!(entity_shown(shown, j), Some(Some(_))) {
                    if let Some(j) = o.get_mut("entity") {
                        project_shown(shown, j, key_values);
                    }
                } else {
                    o.remove("entity");
                }
            }
            if let Some(Value::Array(a)) = o.get_mut("entityList") {
                a.retain(|j| matches!(entity_shown(shown, j), Some(Some(_))));
                for j in a.iter_mut() {
                    project_shown(shown, j, key_values);
                }
            }
        }
    }
}

/// ADR-0020: a narrowing reaches every document the answer carries, so the
/// projection runs on each Entity `join=inline` embedded (4.5.23.2): under
/// `entity`/`entityList` normalized, in place of the object URI simplified.
pub(super) fn project_joined(f: &crate::policy::Filter, e: &mut Value, key_values: bool) {
    f.project(e);
    let Some(obj) = e.as_object_mut() else {
        return;
    };
    for v in obj.values_mut() {
        let instances: Vec<&mut Value> = match v {
            Value::Array(a) => a.iter_mut().collect(),
            other => vec![other],
        };
        for inst in instances {
            if key_values {
                if inst.get("id").is_some() && inst.get("type").is_some() {
                    project_joined(f, inst, key_values);
                }
                continue;
            }
            let Some(o) = inst.as_object_mut() else {
                continue;
            };
            for m in ["entity", "entityList"] {
                match o.get_mut(m) {
                    Some(Value::Array(a)) => {
                        for j in a {
                            project_joined(f, j, key_values);
                        }
                    }
                    Some(j @ Value::Object(_)) => project_joined(f, j, key_values),
                    _ => {}
                }
            }
        }
    }
}

/// What [`view_sees`] answers: the shown set, the narrowed document, the
/// view's representation.
#[cfg(feature = "views")]
pub(super) type ViewSeen = (crate::policy::Shown<'static>, Value, crate::repr::Repr);

/// One change's [`ViewSeen`] per view, by the compiled view's address.
#[cfg(feature = "views")]
type SeenMemo = HashMap<usize, Option<Arc<ViewSeen>>>;

/// ADR-0023 rule 9: what a Subscription made through `view` sees of the
/// changed Entity `doc` (internal form): the Attributes the view shows of it,
/// the document with only those, and the view's narrowing for a linked
/// condition to read targets through; `None` when the view does not select
/// the Entity.
#[cfg(feature = "views")]
pub(super) async fn view_sees(
    st: &AppState,
    tenant: &TenantId,
    view: &crate::views::CompiledView,
    doc: &Value,
) -> Option<ViewSeen> {
    let repr = crate::repr::narrowing(view.narrowing());
    let ctx = crate::repr::policy_context();
    if !crate::repr::policy_admits(st, tenant, ctx, repr.policy_reach.as_deref(), doc).await {
        return None;
    }
    let shown = crate::repr::shown(repr.policy_types.as_deref(), doc)?.into_owned();
    let seen = crate::repr::policy_view(doc, &repr);
    Some((shown, seen, repr))
}

/// ADR-0023 rule 9 on a notification about to leave: each notified Entity,
/// and each one it embeds, read under `view` on its stored state now; the
/// ones the view does not select are dropped (embedded ones unembedded,
/// rule 6), the rest keep the Attributes the view shows, `previousValue`
/// included. `false` when no Entity is left to notify.
#[cfg(feature = "views")]
pub(super) async fn view_project(
    st: &AppState,
    tenant: &TenantId,
    sub: &Value,
    view: &crate::views::CompiledView,
    body: &mut Value,
    ctx: &Context,
) -> bool {
    let repr = crate::repr::narrowing(view.narrowing());
    let (rules, reach) = (repr.policy_types.as_deref(), repr.policy_reach.as_deref());
    // the `*` view without a condition (rule 12) shows everything; one with
    // a q, scopeQ or geoQ reads every Entity it delivers, joined ones too,
    // under that condition as a GET through it does
    if rules.is_none() && reach.is_none() {
        return true;
    }
    let key_values = matches!(
        sub.get("notification")
            .and_then(|n| n.get("format"))
            .and_then(Value::as_str),
        Some("keyValues" | "simplified")
    );
    let shown = shown_now(st, tenant, rules, reach, &body["data"], ctx).await;
    let Some(arr) = body.get_mut("data").and_then(Value::as_array_mut) else {
        return false;
    };
    arr.retain(|e| matches!(entity_shown(&shown, e), Some(Some(_))));
    for e in arr.iter_mut() {
        project_shown(&shown, e, key_values);
    }
    !arr.is_empty()
}

/// Whether the Subscription's q follows a Relationship (4.9
/// LinkedEntityRelation).
pub(super) fn linked_condition(sub: &Value) -> bool {
    sub_str(sub, "q")
        .is_some_and(|q| antares_ql::regex::q_node(q).is_some_and(|n| n.has_linked_paths()))
}

/// The @context governing a subscription's notifications (5.8.6): the
/// jsonldContext member if set, else the @context of the creating request.
pub(crate) async fn sub_context(st: &AppState, tenant: &TenantId, sub: &Value) -> Arc<Context> {
    // Borrowed: the resolver only reads it, and this is on the per-candidate
    // path, where cloning an inline @context copied the whole document.
    let source = sub.get("jsonldContext").or_else(|| sub.get("__context"));
    match source {
        // 5.5.10: the Subscription belongs to one Tenant, so the @context it
        // names resolves within that Tenant — a Hosted @context another Tenant
        // stored (5.13.1) is not in scope here and falls back to the core
        // context rather than compacting this Notification against it.
        Some(v) if !v.is_null() => st
            .loader
            .resolve_quiet_for(tenant, v)
            .await
            .unwrap_or_else(|_| st.loader.core()),
        _ => st.loader.core(),
    }
}

/// A resolved `@context` per Tenant, then per the `@context` URL the
/// Subscription names. Keyed by Tenant first: 5.5.10 scopes a resolution to
/// one Tenant, so one Tenant's resolved context is never reachable under
/// another's identifier even where both name the same URL.
pub(super) type CtxMemo = HashMap<String, HashMap<String, Arc<Context>>>;

/// `sub_context` over one change's memo. Resolving is a cache lookup per
/// call, and this runs once per candidate subscription per change: one change
/// over 1 000 candidates resolves the same handful of `@context` URLs a
/// thousand times. The memo lives for the one change that owns it (the
/// changes of a drain are matched apart, T-6371), so an `@context` a request
/// replaces or deletes meanwhile is picked up by the next change. A
/// Subscription naming an inline `@context` rather than a URL resolves as
/// before — it has no cheap identity to key on.
pub(super) async fn sub_context_memo(
    st: &AppState,
    tenant: &TenantId,
    tenant_str: &str,
    sub: &Value,
    memo: &mut CtxMemo,
) -> Arc<Context> {
    let url = sub
        .get("jsonldContext")
        .or_else(|| sub.get("__context"))
        .and_then(Value::as_str);
    let Some(url) = url else {
        return sub_context(st, tenant, sub).await;
    };
    if let Some(hit) = memo.get(tenant_str).and_then(|by_url| by_url.get(url)) {
        return Arc::clone(hit);
    }
    let ctx = sub_context(st, tenant, sub).await;
    memo.entry(tenant_str.to_owned())
        .or_default()
        .insert(url.to_owned(), Arc::clone(&ctx));
    ctx
}

/// The notification triggers of one subscription, in the form the matcher
/// compares against (Table 5.2.12-1): absent means the default combination
/// `"attributeCreated"` + `"attributeUpdated"`, and `"entityUpdated"` "is
/// equivalent to the combination `"attributeCreated"`, `"attributeUpdated"`
/// and `"attributeDeleted"`" — so it is expanded here, at the single point
/// the list is read, rather than at each comparison.
pub(super) fn triggers_of(sub: &Value) -> Vec<&str> {
    // Borrowed from the Subscription, not copied out of it: this runs once
    // per candidate per change, and the default arm used to allocate a
    // String for each of DEFAULT_TRIGGERS, which are already 'static.
    let mut triggers: Vec<&str> = sub
        .get("notificationTrigger")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_else(|| DEFAULT_TRIGGERS.to_vec());
    if triggers.contains(&"entityUpdated") {
        for t in DEFAULT_TRIGGERS.iter().copied().chain(["attributeDeleted"]) {
            if !triggers.contains(&t) {
                triggers.push(t);
            }
        }
    }
    triggers
}

pub(super) async fn matches_for(
    st: &AppState,
    tenant_str: &str,
    before: Option<Value>,
    after: Option<Value>,
    queued: u64,
    ctx_memo: &mut CtxMemo,
) -> Vec<Matched> {
    let mut out = Vec::new();
    // The tenant of the write that fired the change hook — this broker's own
    // value, and a write inside a Snapshot (5.5.15) carries the synthetic one.
    let Ok(tenant) = TenantId::new_internal(tenant_str) else {
        return out;
    };
    let changes = diff(before.as_ref(), after.as_ref());
    let entity_trigger = match (&before, &after) {
        (None, Some(_)) => "entityCreated",
        (Some(_), None) => "entityDeleted",
        _ => "entityUpdated",
    };
    let eval_doc = after.as_ref().or(before.as_ref());
    let Some(eval_doc) = eval_doc else { return out };
    // Candidate lookup by the entity's types and the changed attribute
    // IRIs — no linear scan over all subscriptions.
    let types: Vec<&str> = eval_doc
        .get("type")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let changed_keys: Vec<&str> = changes.iter().map(|(k, _)| k.as_str()).collect();
    let subs = subs_for(st, &tenant, &types, &changed_keys, queued).await;
    // one instant for the change, so equal shapes render one payload
    let now = now_iso();
    let mut built: HashMap<String, Vec<Value>> = HashMap::new();
    // What a view sees of this change, once per view: every Subscription made
    // through one view reads the same narrowing of the same document, and a
    // fan-out of 1 000 Subscriptions on one view judged and copied it 1 000
    // times per change on the serial matcher (T-6371). Keyed by the compiled
    // view's address in one snapshot of the table, held for this whole
    // call, so no view can free its address for another to take.
    #[cfg(feature = "views")]
    let views = st.views.snapshot();
    #[cfg(feature = "views")]
    let mut seen_memo: SeenMemo = HashMap::new();
    for sub in subs {
        if !is_active(&sub) || sub.get("timeInterval").is_some() {
            continue;
        }
        // ADR-0023 rule 9: a Subscription made through a view matches what
        // the view shows of the Entity now: its conditions read only the
        // shown Attributes (rule 2), and a change to hidden ones alone is no
        // change at all, so its timing does not leak.
        #[cfg(feature = "views")]
        let seen = match crate::views::of_subscription_in(&views, &tenant, &sub) {
            None => None,
            Some(None) => continue,
            Some(Some(view)) => {
                let key = Arc::as_ptr(view) as usize;
                let seen = match seen_memo.get(&key) {
                    Some(seen) => seen.clone(),
                    None => {
                        let seen = view_sees(st, &tenant, view, eval_doc).await.map(Arc::new);
                        seen_memo.insert(key, seen.clone());
                        seen
                    }
                };
                match seen {
                    Some(seen) => Some(seen),
                    None => continue,
                }
            }
        };
        #[cfg(feature = "views")]
        let (eval_doc, narrow) = match seen.as_deref() {
            Some((_, doc, repr)) => (doc, Some((repr, crate::repr::policy_context()))),
            None => (eval_doc, None),
        };
        #[cfg(not(feature = "views"))]
        let narrow = None;
        let triggers = triggers_of(&sub);
        // which attribute-level changes this sub cares about
        let watched: Option<Vec<&str>> = sub
            .get("watchedAttributes")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect());
        #[cfg_attr(not(feature = "views"), expect(unused_mut))]
        let mut relevant: Vec<&(String, ChangeClass)> = changes
            .iter()
            .filter(|(k, _)| watched.as_ref().is_none_or(|w| w.contains(&k.as_str())))
            .collect();
        #[cfg(feature = "views")]
        if let Some((shown, _, _)) = seen.as_deref() {
            relevant.retain(|(k, _)| shown.allows(k));
            if relevant.is_empty() {
                continue;
            }
        }
        let attr_trigger_fired = relevant.iter().any(|(_, c)| {
            let t = match c {
                ChangeClass::Created => "attributeCreated",
                ChangeClass::Updated => "attributeUpdated",
                ChangeClass::Deleted => "attributeDeleted",
            };
            triggers.contains(&t)
        });
        let entity_trigger_fired =
            triggers.contains(&entity_trigger) && (watched.is_none() || !relevant.is_empty());
        if !attr_trigger_fired && !entity_trigger_fired {
            continue;
        }
        let ctx = sub_context_memo(st, &tenant, tenant_str, &sub, ctx_memo).await;
        if !selector_match(&sub, eval_doc, &ctx) {
            continue;
        }
        if !linked_eval(st, &tenant, narrow, |l| {
            conditions_match(&sub, eval_doc, &ctx, l)
        })
        .await
        {
            continue;
        }
        if throttled(&sub) {
            continue;
        }
        let deleted: Vec<String> = if triggers.contains(&"attributeDeleted") {
            relevant
                .iter()
                .filter(|(_, c)| *c == ChangeClass::Deleted)
                .map(|(k, _)| k.clone())
                .collect()
        } else {
            Vec::new()
        };
        let entity_deleted_fired = after.is_none() && triggers.contains(&"entityDeleted");
        let key = payload_key(&sub, &ctx, &deleted, entity_deleted_fired);
        let data = match built.get(&key) {
            Some(data) => data.clone(),
            None => {
                let data = build_data(
                    st,
                    &tenant,
                    &sub,
                    &ctx,
                    before.as_ref(),
                    after.as_ref(),
                    &deleted,
                    entity_deleted_fired,
                    &now,
                )
                .await;
                built.insert(key, data.clone());
                data
            }
        };
        out.push(Matched {
            tenant: tenant.clone(),
            sub,
            ctx,
            data,
        });
    }
    out
}

/// The exact Entity ids a subscription's `entities` selector pins down, or
/// `None` when it leaves any of them open. Table 5.2.33-1: `id` is a String
/// or a String[] and "id takes precedence over idPattern", so an entry
/// carrying an id constrains the read exactly — while one entry without an id
/// (a bare type, or an idPattern no store column can answer) admits every id
/// and forfeits the narrowing for the whole OR-ed selector.
pub(super) fn selector_ids(sub: &Value) -> Option<Vec<String>> {
    let sel = sub.get("entities").and_then(Value::as_array)?;
    let mut ids = Vec::new();
    for e in sel {
        match e.get("id") {
            Some(Value::String(i)) => ids.push(i.clone()),
            Some(Value::Array(a)) => {
                for v in a {
                    ids.push(v.as_str()?.to_owned());
                }
            }
            _ => return None,
        }
    }
    (!ids.is_empty()).then_some(ids)
}

/// A store read on a delivery path has no caller to fail: the sweep is a
/// timer, the fan-out is spawned, and both answer `()`. Silence is what makes
/// a failure dangerous here — a subscription that stops firing because the
/// store could not be read looks exactly like one with nothing to send — so
/// the failure is named and the path continues on the empty set it would
/// have continued on anyway.
/// Every document of `kind` in `tenant` that `keep` accepts, read a page
/// at a time (F-ANT3-019): the whole-tenant `list` carries the client-query
/// row ceiling (5.5.6), and past it a notification path would read nothing
/// and send an empty or no notification. Only the kept documents are held.
pub(crate) async fn docs_where(
    st: &AppState,
    tenant: &TenantId,
    kind: Kind,
    keep: impl Fn(&Value) -> bool,
) -> Result<Vec<Value>, antares_model::NgsiError> {
    let mut out = Vec::new();
    crate::csource::walk_docs(st, tenant, kind, |doc| {
        if keep(&doc) {
            out.push(doc);
        }
        Ok(())
    })
    .await?;
    Ok(out)
}

pub(super) fn read_or_warn<T>(res: Result<Vec<T>, antares_model::NgsiError>, what: &str) -> Vec<T> {
    res.unwrap_or_else(|e| {
        tracing::warn!("notification path: reading {what} failed: {e}");
        Vec::new()
    })
}
