// SPDX-License-Identifier: EUPL-1.2
//! Temporal writes: upsert (5.6.11), attrs (5.6.12, 5.6.13), instances (5.6.14, 5.6.15), delete (5.6.16).

use super::*;

/// 5.6.11 input: the pushed Temporal Evolution may carry the 4.5.7
/// deleted-instance representation (value = NGSI-LD Null), which 5.5.4
/// explicitly excepts for "the temporal evolution" — hence allow_null.
pub(super) const TEMPORAL_OPTS: ExpandOpts = ExpandOpts {
    fragment: false,
    allow_null: true,
    merge: false,
    temporal: true,
    sys: false,
};

/// 4.5.7 and 4.5.8: each recorded instance of a Property or Relationship
/// carries an instanceId and the 4.8 timestamps. The id is minted here and
/// only when the pushed instance brought none, because the temporal API is
/// add-only (5.6.11.4, 5.6.12.1) and 5.6.14/5.6.15 address an instance by
/// that id. Sub-Attributes are part of the instance and are not stamped
/// separately -- the current-state path stamps them one level deeper
/// (`crate::stamp::stamp_instances`).
pub(super) fn stamp_temporal_instances(doc: &mut Value, ts: &str) {
    if let Some(obj) = doc.as_object_mut() {
        for (k, v) in obj.iter_mut() {
            if is_meta(k) {
                continue;
            }
            if let Some(arr) = v.as_array_mut() {
                for inst in arr {
                    if let Some(o) = inst.as_object_mut() {
                        o.entry("instanceId".to_owned()).or_insert_with(|| {
                            Value::String(format!("urn:ngsi-ld:Instance:{}", uuid::Uuid::new_v4()))
                        });
                        o.insert("createdAt".into(), Value::String(ts.to_owned()));
                        o.insert("modifiedAt".into(), Value::String(ts.to_owned()));
                    }
                }
            }
        }
    }
}

// ---------- POST /temporal/entities/ — Upsert temporal (5.6.11) ----------

pub async fn upsert_temporal(
    State(st): State<AppState>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let go = async {
        let tenant = tenant_from(&headers)?;
        check_params(&params, &["options", "local"])?;
        let parsed = parse_body(&st, &headers, &body, BodyKind::Standard).await?;
        let obj = parsed.object(NgsiError::BadRequestData(
            "temporal entity must be a JSON object".into(),
        ))?;
        let expanded = expand_entity(obj, &parsed.ctx, TEMPORAL_OPTS)?;
        let id = antares_jsonld::expanded_id(&expanded)?.to_owned();
        // the engine is asked with what 5.6.1 gives it (T-4855): types, Attributes, body
        let types: Vec<String> = expanded
            .get("type")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let attrs: Vec<String> = expanded
            .as_object()
            .map(|o| o.keys().filter(|k| !is_meta(k)).cloned().collect())
            .unwrap_or_default();
        gate!(
            st, &tenant, &headers, "5.6.11",
            ids: &[&id],
            types: &types,
            attrs: &attrs,
            body: Some(&expanded),
        )
        .await?;
        #[cfg(feature = "views")]
        crate::views::schema_admits_typed(&st, &tenant, &expanded, false)?;
        // the create rule on a comma id (F-ANT2-013) holds for a new Entity,
        // judged after the gate so a refused caller learns nothing stored;
        // a stored legacy one is still upserted
        if id.contains(',') && st.temporal.get(&tenant, &id).await?.is_none() {
            antares_jsonld::new_entity_id(&expanded)?;
        }
        // 5.6.11.4: exclusive/redirect registrations matching the input are
        // forwarded when "Create or Update Temporal" is supported; proxy
        // modes without it are an error of type Conflict; inclusive ones
        // forward when supported. Matching attributes are removed from the
        // local fragment.
        let spec = crate::registry::CsrSpec {
            ids: Some(vec![id.clone()]),
            ..Default::default()
        };
        let regs = match crate::federation::write_plan(
            &st,
            &tenant,
            &spec,
            &parsed.ctx,
            &params,
            &headers,
        )
        .await?
        {
            crate::federation::WritePlan::Answered(r) => return Ok(*r),
            crate::federation::WritePlan::Forward(regs) => regs,
        };
        if !regs.is_empty() {
            let mut parts = Vec::new();
            let mut fwd = Vec::new();
            for reg in &regs {
                if !reg.supports("upsertTemporal") {
                    if reg.is_proxy() {
                        parts.push(crate::federation::conflict_part("upsertTemporal", reg));
                    }
                    continue;
                }
                if let Some(frag) = crate::federation::reduce_to_scope(obj, reg, &parsed.ctx) {
                    fwd.push((reg.clone(), frag));
                }
            }
            let proxies: Vec<&crate::federation::FedReg> =
                regs.iter().filter(|r| r.is_proxy()).collect();
            let (rest, has_attrs) = crate::federation::strip_proxied(obj, &proxies, &parsed.ctx);
            if has_attrs || proxies.is_empty() {
                let local = expand_entity(&rest, &parsed.ctx, TEMPORAL_OPTS)?;
                let status = upsert_temporal_local(&st, &tenant, &id, local).await?;
                parts.push(crate::federation::Part::local(
                    status.as_u16(),
                    "local temporal upsert",
                ));
            }
            let ctx_url = crate::federation::ctx_link_url(&headers, &parsed.ctx.source);
            for (reg, frag) in fwd {
                parts.push(
                    crate::federation::forward_part(
                        &st,
                        reqwest::Method::POST,
                        format!("{}/ngsi-ld/v1/temporal/entities", reg.endpoint),
                        &[],
                        &headers,
                        &tenant,
                        &reg,
                        &ctx_url,
                        Some(frag),
                    )
                    .await,
                );
            }
            return Ok(crate::federation::combine(
                st.profile,
                Some(&id),
                parts,
                created(
                    format!(
                        "/ngsi-ld/v1/temporal/entities/{}",
                        crate::federation::path_segment(&id)
                    ),
                    &tenant,
                ),
                &tenant,
            ));
        }
        let status = upsert_temporal_local(&st, &tenant, &id, expanded).await?;
        Ok::<_, ApiError>(if status == StatusCode::CREATED {
            created(
                format!(
                    "/ngsi-ld/v1/temporal/entities/{}",
                    crate::federation::path_segment(&id)
                ),
                &tenant,
            )
        } else {
            no_content(&tenant)
        })
    };
    go.await.unwrap_or_else(|e| e.into_response())
}

/// 5.6.11.4 local half: create the Temporal Evolution, or add the provided
/// instances to the existing one per 5.6.12 (merge key = datasetId +
/// observedAt) with Entity Type names unioned. Returns 201 vs 204.
pub(super) async fn upsert_temporal_local(
    st: &AppState,
    tenant: &antares_model::TenantId,
    id: &str,
    mut expanded: Value,
) -> ApiResult<StatusCode> {
    let ts = now_iso();
    stamp_temporal_instances(&mut expanded, &ts);
    // get->create/mutate is a TOCTOU pair: two concurrent first-upserts
    // both see "absent", and the loser's create must NOT be silently
    // dropped (201 with a discarded payload). Loop: a lost create retries
    // as a merge, a mutate on a just-deleted doc retries as a create.
    let mut attempts: u8 = 0;
    loop {
        attempts = attempts.saturating_add(1);
        if attempts > 16 {
            return Err(NgsiError::InternalError("upsert retry storm".into()).into());
        }
        let existed = st.temporal.get(tenant, id).await?.is_some();
        if existed {
            let res = st
                .temporal
                .mutate(tenant, id, |doc| {
                    let target = antares_store::stored_object(doc)?;
                    // 5.6.11.4: new Entity Type names are added to the target
                    if let Some(new_types) = expanded.get("type").and_then(Value::as_array) {
                        let mut cur: Vec<Value> = target
                            .get("type")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        for t in new_types {
                            if !cur.contains(t) {
                                cur.push(t.clone());
                            }
                        }
                        target.insert("type".into(), Value::Array(cur));
                    }
                    // 5.6.11.4: the instances "shall be added" as 5.6.12
                    // mandates; identity is the instanceId, never observedAt
                    antares_jsonld::expanded_object(&expanded)?;
                    add_temporal_instances(doc, &expanded, &ts);
                    Ok::<(), NgsiError>(())
                })
                .await?;
            match res {
                Some(Err(e)) => return Err(ApiError::from(e)),
                Some(Ok(())) => return Ok(StatusCode::NO_CONTENT),
                None => continue, // deleted between get and mutate - retry as create
            }
        } else {
            let mut doc = expanded.clone();
            if let Some(o) = doc.as_object_mut() {
                o.insert("createdAt".into(), Value::String(ts.clone()));
                o.insert("modifiedAt".into(), Value::String(ts.clone()));
            }
            if st.temporal.create(tenant, id, doc).await? {
                return Ok(StatusCode::CREATED);
            }
            // lost the create race - the doc exists now; retry as a merge
        }
    }
}

// ---------- DELETE /temporal/entities/{id} (5.6.16) ----------

pub async fn delete_temporal(
    State(st): State<AppState>,
    Path(id): Path<String>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
) -> Response {
    let go = async {
        let tenant = tenant_from(&headers)?;
        antares_model::EntityId::new(&id)?;
        check_params(&params, &["local"])?;
        // 5.6.16.4: forward to registrations supporting the operation;
        // unsupported proxy modes are Conflict.
        let ctx = st.loader.core();
        let pre = crate::policy::temporal_pre_image(&st, &tenant, &id).await?;
        gate!(
            st, &tenant, &headers, "5.6.16",
            ids: &[&id],
            types: &pre.types,
            stored: pre.stored.as_ref(),
        )
        .await?;
        let regs = match temporal_write_regs(&st, &tenant, &headers, &ctx, &params, &id).await {
            Ok(regs) => regs,
            Err(refused) => return Ok(*refused),
        };
        let deleted = st.temporal.delete(&tenant, &id).await?;
        answer_temporal_attr_write(
            &st,
            &tenant,
            &headers,
            &ctx,
            &id,
            "deleteTemporal",
            reqwest::Method::DELETE,
            "",
            None,
            regs,
            LocalWrite {
                res: deleted.then_some(Ok(())),
                found: deleted,
                missing: format!("temporal entity {id}"),
                applied: "deleted locally",
            },
        )
        .await
    };
    go.await.unwrap_or_else(|e| e.into_response())
}

// ---------- POST /temporal/entities/{id}/attrs/ (5.6.12) ----------

pub async fn add_temporal_attrs(
    State(st): State<AppState>,
    Path(id): Path<String>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let go = async {
        let tenant = tenant_from(&headers)?;
        antares_model::EntityId::new(&id)?;
        check_params(&params, &["local"])?;
        let parsed = parse_body(&st, &headers, &body, BodyKind::Standard).await?;
        let obj = parsed.object(NgsiError::BadRequestData(
            "fragment must be a JSON object".into(),
        ))?;
        let pre = crate::policy::temporal_pre_image(&st, &tenant, &id).await?;
        let granted = gate!(
            st, &tenant, &headers, "5.6.12",
            ids: &[&id],
            types: &pre.types,
            stored: pre.stored.as_ref(),
        )
        .await?;
        let cond = crate::policy::Condition::of(&granted);
        // 5.6.12 input is pushed history — the 4.5.7 deleted-instance
        // representation is legal (5.5.4 temporal exception), hence
        // allow_null (mirrors 5.6.11).
        let mut expanded = expand_entity(
            obj,
            &parsed.ctx,
            ExpandOpts {
                fragment: true,
                allow_null: true,
                temporal: true,
                ..Default::default()
            },
        )?;
        #[cfg(feature = "views")]
        crate::views::schema_admits(&st, &tenant, &id, &expanded, &pre.types).await?;
        // 5.6.12.4: forwarding — proxy modes without appendAttrsTemporal
        // are Conflict; supporting registrations receive the fragment and
        // the matching attributes are stripped from the local half.
        let spec = crate::registry::CsrSpec {
            ids: Some(vec![id.clone()]),
            ..Default::default()
        };
        let regs = match crate::federation::write_plan(
            &st,
            &tenant,
            &spec,
            &parsed.ctx,
            &params,
            &headers,
        )
        .await?
        {
            crate::federation::WritePlan::Answered(r) => return Ok(*r),
            crate::federation::WritePlan::Forward(regs) => regs,
        };
        crate::policy::Condition::unforwarded(cond.as_ref(), !regs.is_empty())?;
        if !regs.is_empty() {
            let mut parts = Vec::new();
            let mut fwd = Vec::new();
            for reg in &regs {
                if !reg.supports("appendAttrsTemporal") {
                    if reg.is_proxy() {
                        parts.push(crate::federation::conflict_part("appendAttrsTemporal", reg));
                    }
                    continue;
                }
                if let Some(frag) = crate::federation::reduce_to_scope(obj, reg, &parsed.ctx) {
                    fwd.push((reg.clone(), frag));
                }
            }
            let proxies: Vec<&crate::federation::FedReg> =
                regs.iter().filter(|r| r.is_proxy()).collect();
            let (rest, has_attrs) = crate::federation::strip_proxied(obj, &proxies, &parsed.ctx);
            if has_attrs || proxies.is_empty() {
                let mut local = expand_entity(
                    &rest,
                    &parsed.ctx,
                    ExpandOpts {
                        fragment: true,
                        allow_null: true,
                        temporal: true,
                        ..Default::default()
                    },
                )?;
                let ts = now_iso();
                stamp_temporal_instances(&mut local, &ts);
                let res = st
                    .temporal
                    .mutate(&tenant, &id, |doc| {
                        add_temporal_instances(doc, &local, &ts);
                        Ok::<(), NgsiError>(())
                    })
                    .await?;
                parts.push(match res {
                    Some(Ok(())) => crate::federation::Part::local(204, "added locally"),
                    _ => crate::federation::Part::local(
                        404,
                        format!("temporal entity {id} not found locally"),
                    ),
                });
            }
            let ctx_url = crate::federation::ctx_link_url(&headers, &parsed.ctx.source);
            for (reg, frag) in fwd {
                parts.push(
                    crate::federation::forward_part(
                        &st,
                        reqwest::Method::POST,
                        format!(
                            "{}/ngsi-ld/v1/temporal/entities/{}/attrs",
                            reg.endpoint,
                            crate::federation::path_segment(&id)
                        ),
                        &[],
                        &headers,
                        &tenant,
                        &reg,
                        &ctx_url,
                        Some(frag),
                    )
                    .await,
                );
            }
            return Ok(crate::federation::combine(
                st.profile,
                Some(&id),
                parts,
                no_content(&tenant),
                &tenant,
            ));
        }
        let ts = now_iso();
        stamp_temporal_instances(&mut expanded, &ts);
        let res = st
            .temporal
            .mutate(&tenant, &id, |doc| {
                crate::policy::Condition::pre_evolution(cond.as_ref(), &id, doc)?;
                add_temporal_instances(doc, &expanded, &ts);
                Ok::<(), NgsiError>(())
            })
            .await?;
        match res {
            None => {
                Err(NgsiError::ResourceNotFound(format!("temporal entity {id} not found")).into())
            }
            Some(Err(e)) => Err(ApiError::from(e)),
            Some(Ok(())) => Ok(no_content(&tenant)),
        }
    };
    go.await.unwrap_or_else(|e| e.into_response())
}

/// 5.6.12.4: append the fragment's Attribute instances to the Temporal
/// Evolution (instances accumulate — history is never overwritten here).
pub(super) fn add_temporal_instances(doc: &mut Value, expanded: &Value, ts: &str) {
    let Some(target) = doc.as_object_mut() else {
        return;
    };
    // 5.6.12.4: "If type is included in the EntityTemporal Fragment and it
    // includes Entity Type names that are not yet in the target ..., add
    // them to the list of Entity Type names"
    if let Some(Value::Array(add)) = expanded.get("type") {
        let mut cur = match target.remove("type") {
            Some(Value::Array(a)) => a,
            Some(t) => vec![t],
            None => Vec::new(),
        };
        for t in add {
            if !cur.contains(t) {
                cur.push(t.clone());
            }
        }
        target.insert("type".into(), Value::Array(cur));
    }
    for (k, v) in expanded.as_object().into_iter().flatten() {
        if is_meta(k) {
            continue;
        }
        let incoming = v.as_array().cloned().unwrap_or_default();
        match target.get_mut(k).and_then(Value::as_array_mut) {
            Some(cur) => cur.extend(incoming),
            None => {
                target.insert(k.clone(), Value::Array(incoming));
            }
        }
    }
    target.insert("modifiedAt".into(), Value::String(ts.to_owned()));
}

// ---------- DELETE /temporal/entities/{id}/attrs/{attrId} (5.6.13) ----------

pub async fn delete_temporal_attr(
    State(st): State<AppState>,
    Path((id, attr)): Path<(String, String)>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
) -> Response {
    let go = async {
        let tenant = tenant_from(&headers)?;
        antares_model::EntityId::new(&id)?;
        // 5.6.13.4: "If the target Attribute name is not a valid name, then an
        // error of type BadRequestData shall be raised." The shared guard is
        // the one that also refuses dot segments — the name is interpolated
        // into the forwarded request path, where `..` addresses the peer's
        // Temporal Evolution resource instead of its attribute.
        antares_model::check_attr_name(&attr)?;
        check_params(&params, &["datasetId", "deleteAll", "local"])?;
        let ctx = request_context(&st.loader, &headers).await?;
        let pre = crate::policy::temporal_pre_image(&st, &tenant, &id).await?;
        let granted = gate!(
            st, &tenant, &headers, "5.6.13",
            ids: &[&id],
            types: &pre.types,
            stored: pre.stored.as_ref(),
        )
        .await?;
        let cond = crate::policy::Condition::of(&granted);
        let attr_iri = antares_jsonld::expand_attr_name(&attr, &ctx)?;
        let delete_all = params.get("deleteAll").map(String::as_str) == Some("true");
        let want_ds = crate::repr::target_dataset_id(&params).map(String::from);
        let regs = match temporal_write_regs(&st, &tenant, &headers, &ctx, &params, &id).await {
            Ok(regs) => regs,
            Err(refused) => return Ok(*refused),
        };
        crate::policy::Condition::unforwarded(cond.as_ref(), !regs.is_empty())?;
        let mut found = false;
        let ts = now_iso();
        let res = st
            .temporal
            .mutate(&tenant, &id, |doc| {
                crate::policy::Condition::pre_evolution(cond.as_ref(), &id, doc)?;
                let target = antares_store::stored_object(doc)?;
                if delete_all
                    || (want_ds.is_none()
                        && !target
                            .get(&attr_iri)
                            .and_then(Value::as_array)
                            .is_some_and(|a| a.iter().any(|i| i.get("datasetId").is_some())))
                {
                    // deleteAll, or single-instance-set attribute: drop it whole
                    if target.remove(&attr_iri).is_some() {
                        found = true;
                    }
                } else if let Some(arr) = target.get_mut(&attr_iri).and_then(Value::as_array_mut) {
                    // 5.6.13: only the matching datasetId instance set is deleted
                    let before = arr.len();
                    arr.retain(|i| {
                        i.get("datasetId").and_then(Value::as_str) != want_ds.as_deref()
                    });
                    found = arr.len() != before;
                    if arr.is_empty() {
                        target.remove(&attr_iri);
                    }
                }
                if found {
                    target.insert("modifiedAt".into(), Value::String(ts.clone()));
                }
                Ok::<(), NgsiError>(())
            })
            .await?;
        answer_temporal_attr_write(
            &st,
            &tenant,
            &headers,
            &ctx,
            &id,
            "deleteAttrsTemporal",
            reqwest::Method::DELETE,
            &format!("/attrs/{}", crate::federation::path_segment(&attr)),
            None,
            regs,
            LocalWrite {
                res,
                found,
                missing: format!("attribute {attr}"),
                applied: "deleted locally",
            },
        )
        .await
    };
    go.await.unwrap_or_else(|e| e.into_response())
}

/// 5.6.13 / 5.6.14 / 5.6.15 / 5.6.16 answer: the local result becomes one
/// 4.3.6 Part, every registration supporting `op` (Table 4.20-1) contributes
/// another, and 4.3.6.4 combines them into the one status the client sees.
/// With no registration to forward to — the common case — the local result
/// is the whole answer. The four operations differ in what they change and
/// in what they forward, never in how the two halves are answered together.
/// The registrations a 5.6.13/5.6.14/5.6.15/5.6.16 write forwards to, with
/// 6.3.17/6.3.18 loop handling already applied. The check belongs here,
/// ahead of the local write: 508 Loop Detected is an error status, so the
/// request it answers has to leave the Temporal Evolution as it found it.
/// The refusal travels boxed — a whole `Response` in an `Err` makes every
/// `Ok` of this function carry its width.
pub(super) async fn temporal_write_regs(
    st: &AppState,
    tenant: &antares_model::TenantId,
    headers: &HeaderMap,
    ctx: &antares_jsonld::Context,
    params: &HashMap<String, String>,
    id: &str,
) -> Result<Vec<crate::federation::FedReg>, Box<Response>> {
    let spec = crate::registry::CsrSpec {
        ids: Some(vec![id.to_owned()]),
        ..Default::default()
    };
    match crate::federation::write_plan(st, tenant, &spec, ctx, params, headers)
        .await
        .map_err(|e| Box::new(crate::negotiate::ApiError::from(e).into_response()))?
    {
        crate::federation::WritePlan::Answered(refused) => Err(refused),
        crate::federation::WritePlan::Forward(regs) => Ok(regs),
    }
}

#[allow(clippy::too_many_arguments)] // mirrors the wire: one param per forwarded request part
pub(super) async fn answer_temporal_attr_write(
    st: &AppState,
    tenant: &antares_model::TenantId,
    headers: &HeaderMap,
    ctx: &antares_jsonld::Context,
    id: &str,
    op: &str,
    method: reqwest::Method,
    path_suffix: &str,
    body: Option<Value>,
    regs: Vec<crate::federation::FedReg>,
    local: LocalWrite,
) -> ApiResult<Response> {
    if regs.is_empty() {
        return match local.res {
            None => {
                Err(NgsiError::ResourceNotFound(format!("temporal entity {id} not found")).into())
            }
            Some(Err(e)) => Err(ApiError::from(e)),
            Some(Ok(())) if local.found => Ok(no_content(tenant)),
            Some(Ok(())) => {
                Err(NgsiError::ResourceNotFound(format!("{} not found", local.missing)).into())
            }
        };
    }
    let local_part = match &local.res {
        None => {
            crate::federation::Part::local(404, format!("temporal entity {id} not found locally"))
        }
        Some(_) if local.found => crate::federation::Part::local(204, local.applied),
        Some(_) => {
            crate::federation::Part::local(404, format!("{} not found locally", local.missing))
        }
    };
    let mut parts = vec![local_part];
    let ctx_url = crate::federation::ctx_link_url(headers, &ctx.source);
    for reg in &regs {
        if !reg.supports(op) {
            if reg.is_proxy() {
                parts.push(crate::federation::conflict_part(op, reg));
            }
            continue;
        }
        parts.push(
            crate::federation::forward_part(
                st,
                method.clone(),
                format!(
                    "{}/ngsi-ld/v1/temporal/entities/{}{path_suffix}",
                    reg.endpoint,
                    crate::federation::path_segment(id)
                ),
                &[],
                headers,
                tenant,
                reg,
                &ctx_url,
                body.clone(),
            )
            .await,
        );
    }
    Ok(crate::federation::combine(
        st.profile,
        Some(id),
        parts,
        no_content(tenant),
        tenant,
    ))
}

// ---------- PATCH/DELETE .../attrs/{attrId}/{instanceId} (5.6.14/5.6.15) ----------

pub async fn modify_temporal_instance(
    State(st): State<AppState>,
    Path((id, attr, instance_id)): Path<(String, String, String)>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let go = async {
        let tenant = tenant_from(&headers)?;
        // Empty attr segment ⇒ the URI names no resource with a PATCH method
        // (suite 016_02_06 asserts 405 here, vs 400 on the DELETE sibling).
        if attr.is_empty() {
            return Err(ApiError::Bare(StatusCode::METHOD_NOT_ALLOWED));
        }
        antares_model::EntityId::new(&id)?;
        antares_model::check_attr_name(&attr)?;
        antares_model::EntityId::new(&instance_id)
            .map_err(|_| NgsiError::BadRequestData("invalid instance id".into()))?;
        check_params(&params, &["local"])?;
        let pre = crate::policy::temporal_pre_image(&st, &tenant, &id).await?;
        let granted = gate!(
            st, &tenant, &headers, "5.6.14",
            ids: &[&id],
            types: &pre.types,
            stored: pre.stored.as_ref(),
        )
        .await?;
        let cond = crate::policy::Condition::of(&granted);
        let parsed = parse_body(&st, &headers, &body, BodyKind::MergePatch).await?;
        let obj = parsed.object(NgsiError::BadRequestData(
            "fragment must be a JSON object".into(),
        ))?;
        let attr_iri = antares_jsonld::expand_attr_name(&attr, &parsed.ctx)?;
        let frag_inst = instance_fragment(obj, &attr, &attr_iri, &parsed.ctx)?;
        #[cfg(feature = "views")]
        crate::views::schema_admits(
            &st,
            &tenant,
            &id,
            &serde_json::json!({ attr_iri.clone(): [frag_inst.clone()] }),
            &pre.types,
        )
        .await?;
        let regs =
            match temporal_write_regs(&st, &tenant, &headers, &parsed.ctx, &params, &id).await {
                Ok(regs) => regs,
                Err(refused) => return Ok(*refused),
            };
        crate::policy::Condition::unforwarded(cond.as_ref(), !regs.is_empty())?;
        let ts = now_iso();
        let mut found = false;
        let res = st
            .temporal
            .mutate(&tenant, &id, |doc| {
                crate::policy::Condition::pre_evolution(cond.as_ref(), &id, doc)?;
                let target = antares_store::stored_object(doc)?;
                if let Some(arr) = target.get_mut(&attr_iri).and_then(Value::as_array_mut) {
                    if let Some(inst) = arr.iter_mut().find(|i| {
                        i.get("instanceId").and_then(Value::as_str) == Some(instance_id.as_str())
                    }) {
                        found = true;
                        replace_instance(inst, &frag_inst, &ts)?;
                    }
                }
                Ok::<(), NgsiError>(())
            })
            .await?;
        answer_temporal_attr_write(
            &st,
            &tenant,
            &headers,
            &parsed.ctx,
            &id,
            "updateAttrInstanceTemporal",
            reqwest::Method::PATCH,
            &format!(
                "/attrs/{}/{}",
                crate::federation::path_segment(&attr),
                crate::federation::path_segment(&instance_id)
            ),
            Some(parsed.value.clone()),
            regs,
            LocalWrite {
                res,
                found,
                missing: format!("instance {instance_id}"),
                applied: "applied locally",
            },
        )
        .await
    };
    go.await.unwrap_or_else(|e| e.into_response())
}

/// 5.6.14.4 takes "the Attribute instance in the EntityTemporal
/// Fragment": one keyed by the Attribute name. A bare instance (no
/// such key, ETSI 016_01) is taken as that instance itself. The instance
/// comes back expanded.
fn instance_fragment(
    frag: &Map<String, Value>,
    attr: &str,
    attr_iri: &str,
    ctx: &Context,
) -> ApiResult<Value> {
    let mut frag = frag.clone();
    frag.remove("@context");
    let named = (!frag.contains_key("type")).then_some(()).and_then(|()| {
        frag.iter().find_map(|(k, v)| {
            let is_attr = k == attr
                || antares_jsonld::expand_attr_name(k, ctx).ok().as_deref() == Some(attr_iri);
            is_attr.then(|| v.clone())
        })
    });
    let frag = match named {
        Some(Value::Array(mut one)) if one.len() == 1 => one.remove(0),
        Some(Value::Object(o)) => Value::Object(o),
        Some(_) => {
            return Err(NgsiError::BadRequestData(
                "the EntityTemporal Fragment holds one instance of the Attribute (5.6.14.4)".into(),
            )
            .into())
        }
        None => Value::Object(frag),
    };
    let mut wrapper = Map::new();
    wrapper.insert(attr.to_owned(), frag);
    let expanded = expand_entity(
        &wrapper,
        ctx,
        ExpandOpts {
            fragment: true,
            allow_null: false,
            temporal: true,
            ..Default::default()
        },
    )?;
    expanded
        .get(attr_iri)
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .cloned()
        .ok_or_else(|| NgsiError::BadRequestData("invalid instance fragment".into()).into())
}

/// 5.6.14.4: "Replace the target Attribute instance identified by the
/// instanceId with the Attribute instance in the EntityTemporal Fragment.
/// The createdAt property of the concerned instance shall remain
/// unchanged, but the modifiedAt property shall be set to the timestamp
/// corresponding to this modification." A replace, so a member only the
/// stored instance carries does not survive it; the instance keeps the
/// identity it is addressed by.
fn replace_instance(inst: &mut Value, frag_inst: &Value, ts: &str) -> Result<(), NgsiError> {
    let t = antares_store::stored_object(inst)?;
    let kept: Vec<(String, Value)> = ["createdAt", "instanceId"]
        .iter()
        .filter_map(|k| t.get(*k).map(|v| ((*k).to_owned(), v.clone())))
        .collect();
    t.clear();
    t.extend(kept);
    for (k, v) in antares_jsonld::expanded_object(frag_inst)? {
        if matches!(k.as_str(), "createdAt" | "instanceId") {
            continue;
        }
        t.insert(k.clone(), v.clone());
    }
    t.insert("modifiedAt".into(), Value::String(ts.to_owned()));
    Ok(())
}

pub async fn delete_temporal_instance(
    State(st): State<AppState>,
    Path((id, attr, instance_id)): Path<(String, String, String)>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
) -> Response {
    let go = async {
        let tenant = tenant_from(&headers)?;
        antares_model::EntityId::new(&id)?;
        antares_model::check_attr_name(&attr)?;
        antares_model::EntityId::new(&instance_id)
            .map_err(|_| NgsiError::BadRequestData("invalid instance id".into()))?;
        check_params(&params, &["local"])?;
        let ctx = request_context(&st.loader, &headers).await?;
        let pre = crate::policy::temporal_pre_image(&st, &tenant, &id).await?;
        let granted = gate!(
            st, &tenant, &headers, "5.6.15",
            ids: &[&id],
            types: &pre.types,
            stored: pre.stored.as_ref(),
        )
        .await?;
        let cond = crate::policy::Condition::of(&granted);
        let attr_iri = antares_jsonld::expand_attr_name(&attr, &ctx)?;
        let regs = match temporal_write_regs(&st, &tenant, &headers, &ctx, &params, &id).await {
            Ok(regs) => regs,
            Err(refused) => return Ok(*refused),
        };
        crate::policy::Condition::unforwarded(cond.as_ref(), !regs.is_empty())?;
        let mut found = false;
        let ts = now_iso();
        let res = st
            .temporal
            .mutate(&tenant, &id, |doc| {
                crate::policy::Condition::pre_evolution(cond.as_ref(), &id, doc)?;
                let target = antares_store::stored_object(doc)?;
                if let Some(arr) = target.get_mut(&attr_iri).and_then(Value::as_array_mut) {
                    let before = arr.len();
                    arr.retain(|i| {
                        i.get("instanceId").and_then(Value::as_str) != Some(instance_id.as_str())
                    });
                    found = arr.len() != before;
                    if arr.is_empty() {
                        target.remove(&attr_iri);
                    }
                }
                if found {
                    target.insert("modifiedAt".into(), Value::String(ts.clone()));
                }
                Ok::<(), NgsiError>(())
            })
            .await?;
        answer_temporal_attr_write(
            &st,
            &tenant,
            &headers,
            &ctx,
            &id,
            "deleteAttrInstanceTemporal",
            reqwest::Method::DELETE,
            &format!(
                "/attrs/{}/{}",
                crate::federation::path_segment(&attr),
                crate::federation::path_segment(&instance_id)
            ),
            None,
            regs,
            LocalWrite {
                res,
                found,
                missing: format!("instance {instance_id}"),
                applied: "applied locally",
            },
        )
        .await
    };
    go.await.unwrap_or_else(|e| e.into_response())
}
