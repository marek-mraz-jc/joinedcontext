// SPDX-License-Identifier: EUPL-1.2
//! Notification body construction (5.3.1, 5.8.6): the per-subscription shape, tombstones of deleted attributes and the data members.

use super::*;

/// Per-type NGSI-LD-null member and its showChanges previous-member (5.8.6).
pub(super) fn null_members(atype: &str) -> (&'static str, Value, &'static str) {
    let null = Value::String("urn:ngsi-ld:null".into());
    match atype {
        "Relationship" => ("object", null, "previousObject"),
        "LanguageProperty" => (
            "languageMap",
            json!({"@none": "urn:ngsi-ld:null"}),
            "previousLanguageMap",
        ),
        "JsonProperty" => ("json", null, "previousJson"),
        "VocabProperty" => ("vocab", null, "previousVocab"),
        _ => ("value", null, "previousValue"),
    }
}

pub(super) fn current_member(atype: &str) -> &'static str {
    null_members(atype).0
}

/// The deletion tombstone for one former attribute instance (5.8.6 payload
/// forms: typed null + optional datasetId / sysAttrs stamps / previous value).
pub(super) fn tombstone(before_inst: &Value, sys: bool, show: bool, now: &str) -> Value {
    let atype = before_inst
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("Property");
    let (member, null_val, prev_member) = null_members(atype);
    let mut m = Map::new();
    m.insert("type".into(), Value::String(atype.to_owned()));
    m.insert(member.into(), null_val);
    if let Some(ds) = before_inst.get("datasetId") {
        m.insert("datasetId".into(), ds.clone());
    }
    if sys {
        for k in ["createdAt", "modifiedAt"] {
            if let Some(v) = before_inst.get(k) {
                m.insert(k.into(), v.clone());
            }
        }
        m.insert("deletedAt".into(), Value::String(now.to_owned()));
    }
    if show {
        if let Some(prev) = before_inst.get(member) {
            m.insert(prev_member.into(), prev.clone());
        }
    }
    Value::Object(m)
}

pub(super) fn instance_ds(inst: &Value) -> Option<&str> {
    inst.get("datasetId").and_then(Value::as_str)
}

/// Instances of `before[attr]` that no longer exist in `after[attr]`
/// (matched by datasetId — instance-level deletions count, 046_22_06).
pub(super) fn deleted_instances<'a>(
    before: &'a Value,
    after: Option<&Value>,
    attr: &str,
) -> Vec<&'a Value> {
    let b: Vec<&Value> = before
        .get(attr)
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let a: Vec<&Value> = after
        .and_then(|d| d.get(attr))
        .and_then(Value::as_array)
        .map(|x| x.iter().collect())
        .unwrap_or_default();
    b.into_iter()
        .filter(|bi| !a.iter().any(|ai| instance_ds(ai) == instance_ds(bi)))
        .collect()
}

pub(crate) struct NotifShape {
    pub(crate) repr: crate::repr::Repr,
    pub(crate) show_changes: bool,
    pub(crate) join: Option<(String, usize)>,
}

pub(crate) fn notif_shape(sub: &Value, ctx: &Context) -> NotifShape {
    let n = sub.get("notification").and_then(Value::as_object);
    let format = n
        .and_then(|n| n.get("format"))
        .and_then(Value::as_str)
        .unwrap_or("normalized");
    let mut repr = crate::repr::Repr {
        sys_attrs: n
            .and_then(|n| n.get("sysAttrs"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        key_values: matches!(format, "keyValues" | "simplified"),
        concise: format == "concise",
        // Table 5.2.12-1: "Language filter to be applied to the query (clause 4.15)"
        lang: sub.get("lang").and_then(Value::as_str).map(str::to_owned),
        ..Default::default()
    };
    let names = |key: &str| -> Option<Vec<String>> {
        n.and_then(|n| n.get(key))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
    };
    if let Some(attrs) = names("attributes") {
        repr.attrs = Some(attrs.iter().map(|a| ctx.expand_key(a)).collect());
    }
    // 4.21 NGSI-LD Attribute Projection Language: pick/omit values are
    // projection language strings, which Table 5.2.14.1-1 requires for the
    // notification members too ("a valid attribute projection language string
    // as per clause 4.21"). Each term may carry a LinkedEntityTerm
    // (`ProjectionTerm = AttrName *1(LinkedEntityTerm)`), which is what
    // constrains an Attribute inside a Linked Entity retrieved by join. Building
    // the nodes by hand here degraded `refDevice{type}` to a literal Attribute
    // name matching nothing, so the term was dropped instead of applied.
    // A term that fails to parse is dropped rather than kept flat: for pick that
    // withholds the Attribute, which is the safe direction.
    let nodes = |list: Vec<String>| -> Vec<crate::repr::ProjNode> {
        list.iter()
            .filter_map(|term| crate::repr::parse_projection(term, ctx).ok())
            .flatten()
            .collect()
    };
    if let Some(pick) = names("pick") {
        repr.pick = Some(nodes(pick));
    }
    if let Some(omit) = names("omit") {
        repr.omit = Some(nodes(omit));
    }
    if let Some(ds) = sub.get("datasetId").and_then(Value::as_array) {
        repr.dataset_id = Some(
            ds.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
        );
    }
    let join = n
        .and_then(|n| n.get("join"))
        .and_then(Value::as_str)
        .filter(|j| *j == "inline" || *j == "flat")
        .map(|j| {
            // the stored member is bounded at creation, but a Subscription
            // written before that bound existed is still on disk, so the
            // traversal takes the ceiling from here too
            let level = (n
                .and_then(|n| n.get("joinLevel"))
                .and_then(Value::as_u64)
                .map_or(1, |n| usize::try_from(n).unwrap_or(usize::MAX)))
            .min(crate::bounds::MAX_JOIN_LEVEL);
            (j.to_owned(), level)
        });
    NotifShape {
        repr,
        show_changes: n
            .and_then(|n| n.get("showChanges"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        join,
    }
}

/// Everything [`build_data`] reads of one Subscription for one change, as a
/// memo key: the members [`notif_shape`] reads, the deleted Attributes its
/// triggers name and the `@context` it compacts with. Subscriptions with
/// equal keys get equal payloads, so 1 000 subscriptions of one view render
/// one (T-5159). The `@context` counts by identity: the drain's memo hands
/// every Subscription naming one URL the same `Arc`, and a key holds an
/// address only while its `Arc` lives in a matched group, so no other
/// `@context` can take that address in the meantime.
pub(super) fn payload_key(
    sub: &Value,
    ctx: &Arc<Context>,
    deleted: &[String],
    entity_deleted: bool,
) -> String {
    let n = sub.get("notification");
    let member = |k: &str| n.and_then(|n| n.get(k));
    json!([
        Arc::as_ptr(ctx).addr(),
        member("format"),
        member("sysAttrs"),
        member("attributes"),
        member("pick"),
        member("omit"),
        member("showChanges"),
        member("join"),
        member("joinLevel"),
        sub.get("lang"),
        sub.get("datasetId"),
        deleted,
        entity_deleted,
    ])
    .to_string()
}

/// Build the notification `data` array for one change and one subscription.
#[allow(clippy::too_many_arguments)] // one param per 5.8.6 payload input
pub(super) async fn build_data(
    st: &AppState,
    tenant: &TenantId,
    sub: &Value,
    ctx: &Context,
    before: Option<&Value>,
    after: Option<&Value>,
    relevant_deleted_attrs: &[String],
    entity_deleted_fired: bool,
    now: &str,
) -> Vec<Value> {
    let shape = notif_shape(sub, ctx);
    let sys = shape.repr.sys_attrs;
    let show = shape.show_changes;
    let internal = match after {
        Some(a) => {
            let mut doc = a.clone();
            if show {
                // previous* on changed instances (046_31..33). An entity off
                // the change feed is an object; one that is not carries
                // nothing to decorate and travels on unchanged.
                if let (Some(b), Some(obj)) = (before, doc.as_object_mut()) {
                    for (k, v) in obj.iter_mut() {
                        if crate::repr::ENTITY_META.contains(&k.as_str()) {
                            continue;
                        }
                        let Some(arr) = v.as_array_mut() else {
                            continue;
                        };
                        for inst in arr {
                            let Some(bi) = b.get(k).and_then(Value::as_array).and_then(|ba| {
                                ba.iter().find(|x| instance_ds(x) == instance_ds(inst))
                            }) else {
                                continue;
                            };
                            let atype = inst
                                .get("type")
                                .and_then(Value::as_str)
                                .unwrap_or("Property");
                            let member = current_member(atype);
                            let (_, _, prev_member) = null_members(atype);
                            if bi.get(member) != inst.get(member) {
                                if let (Some(pv), Some(io)) =
                                    (bi.get(member).cloned(), inst.as_object_mut())
                                {
                                    io.insert(prev_member.into(), pv);
                                }
                            }
                        }
                    }
                }
            }
            // deletion tombstones appended beside surviving instances
            if let Some(b) = before {
                for attr in relevant_deleted_attrs {
                    let gone = deleted_instances(b, Some(&doc), attr);
                    if gone.is_empty() {
                        continue;
                    }
                    // one tombstone per deleted instance, each with its
                    // datasetId, deleteAll included (046_22_08)
                    let tss: Vec<Value> = gone
                        .iter()
                        .map(|di| tombstone(di, sys, show, now))
                        .collect();
                    let Some(target) = doc.as_object_mut() else {
                        continue;
                    };
                    if let Some(arr) = target
                        .entry(attr.clone())
                        .or_insert_with(|| Value::Array(vec![]))
                        .as_array_mut()
                    {
                        arr.extend(tss);
                    }
                }
            }
            doc
        }
        None => {
            // entity deleted: tombstone entity (046_21) + per-trigger attrs.
            // The caller returns early unless one of before/after is there,
            // so this arm has a before; without one there is nothing to
            // describe and nothing to notify about.
            let Some(b) = before else {
                return Vec::new();
            };
            let mut m = Map::new();
            for k in ["id", "type"] {
                if let Some(v) = b.get(k) {
                    m.insert(k.into(), v.clone());
                }
            }
            if sys {
                for k in ["createdAt", "modifiedAt"] {
                    if let Some(v) = b.get(k) {
                        m.insert(k.into(), v.clone());
                    }
                }
            }
            m.insert("deletedAt".into(), Value::String(now.to_owned()));
            // 5.8.6: an Entity deletion notifies "only the deletedAt system
            // property" unless showChanges, then every attribute, tombstoned
            // (046_37); attributeDeleted alone names the attributes it fired for
            let attrs: Vec<String> = match (entity_deleted_fired, show) {
                (true, true) => attr_keys(b),
                (true, false) => Vec::new(),
                (false, _) => relevant_deleted_attrs.to_vec(),
            };
            for attr in attrs {
                let insts: Vec<Value> = b
                    .get(&attr)
                    .and_then(Value::as_array)
                    .map(|a| a.iter().map(|i| tombstone(i, sys, show, now)).collect())
                    .unwrap_or_default();
                if !insts.is_empty() {
                    m.insert(attr, Value::Array(insts));
                }
            }
            Value::Object(m)
        }
    };
    let shaped = crate::repr::apply(&internal, &shape.repr);
    let mut main = crate::repr::compact_for(&shape.repr, &shaped, ctx);
    let mut data = Vec::new();
    match &shape.join {
        Some((mode, level)) if mode == "inline" => {
            crate::repr::inline_join(st, tenant, ctx, &shape.repr, &mut main, *level).await;
            data.push(main);
        }
        Some((mode, level)) if mode == "flat" => {
            let main_id = internal.get("id").and_then(Value::as_str).unwrap_or("");
            let mut linked = std::collections::BTreeMap::new();
            crate::repr::collect_flat(st, tenant, ctx, &shape.repr, &internal, *level, &mut linked)
                .await;
            data.push(main);
            for (id, (ldoc, lrepr)) in linked {
                if id != main_id {
                    data.push(crate::repr::compact_for(
                        &lrepr,
                        &crate::repr::apply(&ldoc, &lrepr),
                        ctx,
                    ));
                }
            }
        }
        _ => data.push(main),
    }
    data
}
