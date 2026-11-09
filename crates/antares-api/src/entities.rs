// SPDX-License-Identifier: EUPL-1.2
//! /entities resource (CIM 009 6.4–6.7; operations 5.6.1–5.6.6, 5.6.17,
//! 5.6.18, 5.6.19, 5.6.21, 5.7.1, 5.7.2).

use crate::history::mirror_delete_entity;
use crate::negotiate::*;
use crate::paging::{attach_warnings, order_entities, page_params, paginate, paginate_pre};
use crate::repr::{apply, parse_repr};
use crate::repr::{
    collect_flat_beyond, compact_for, inline_join_beyond, to_geojson_collection,
    to_geojson_feature, MAX_JOIN_LOOKUPS,
};
use crate::stamp::stamp_new;
use crate::state::{now_iso, AppState};
use antares_jsonld::{expand_entity, is_ngsi_null, ExpandOpts};
use antares_model::{NgsiError, TenantId};
use antares_ql::eval::eval_q;
use antares_ql::parse_q;
use antares_store::CurrentStateDriverExt;
use antares_store::Kind;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

use crate::negotiate::CleanParams;

use antares_model::is_meta;

// ---------- temporal mirroring (auto-recording; Scorpio ENTITY-topic parity) ----------
//
// Append-side auto-recording (create/update/partial/merge/replace/batch) is
// driven centrally off the store's change hook — see
// `notify::record_temporal_change`. Only the DELETION mirrors below stay as
// explicit handler calls (their typed-null deletion shape is not derivable
// from a plain before/after append).

// ---------- POST /entities/ (5.6.1) ----------

/// 5.6.1.5: the output of a successful Create Entity is "the URI of the
/// created Entity" — the resource URL carried in the Location header. The id
/// is one path segment (RFC 3986 clause 3.3), so it is percent-encoded:
/// spliced raw, a `#` in the id turns the rest of it into a fragment and the
/// client is handed a URL addressing a different resource.
fn entity_location(id: &str) -> String {
    format!(
        "/ngsi-ld/v1/entities/{}",
        crate::federation::path_segment(id)
    )
}

/// The selector of Entity types is input data of Delete (5.6.6.3), Merge
/// (5.6.17.3) and Replace Entity (5.6.18.3), and each of those clauses
/// forwards "matching input data ... to the Registration endpoint". A
/// registration may cover several Entity types, so a forward that drops the
/// selector lets the peer act on an entity the client's selector excluded.
fn type_selector_query(params: &HashMap<String, String>) -> Vec<(String, String)> {
    params
        .get("type")
        .map(|v| ("type".to_owned(), v.clone()))
        .into_iter()
        .collect()
}

pub async fn create_entity(
    State(st): State<AppState>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match create_entity_inner(&st, &params, &headers, &body).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

async fn create_entity_inner(
    st: &AppState,
    params: &HashMap<String, String>,
    headers: &HeaderMap,
    body: &[u8],
) -> ApiResult<Response> {
    let tenant = tenant_from(headers)?;
    check_params(params, &["local"])?;
    let parsed = parse_body(st, headers, body, BodyKind::Standard).await?;
    let obj = parsed.object(NgsiError::InvalidRequest(
        "entity document must be a JSON object".into(),
    ))?;
    let mut expanded = expand_entity(obj, &parsed.ctx, ExpandOpts::default())?;
    let id = antares_jsonld::new_entity_id(&expanded)?.to_owned();

    // distributed create (4.3.6, 6.4.3.1)
    let types: Option<Vec<String>> = expanded.get("type").and_then(Value::as_array).map(|a| {
        a.iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    });
    let attr_iris: Vec<String> = expanded
        .as_object()
        .map(|o| o.keys().filter(|k| !is_meta(k)).cloned().collect())
        .unwrap_or_default();
    let spec = crate::registry::CsrSpec {
        ids: Some(vec![id.clone()]),
        types,
        attrs: (!attr_iris.is_empty()).then_some(attr_iris),
        ..Default::default()
    };
    // ADR-0020: the policy seam, once per request, after expansion and
    // before the operation or any fan-out. Everything the engine is given
    // is the expanded form the store would see.
    gate!(
        st, &tenant, headers, "5.6.1",
        ids: &[&id],
        types: spec.types.as_deref().unwrap_or(&[]),
        attrs: spec.attrs.as_deref().unwrap_or(&[]),
        body: Some(&expanded),
    )
    .await?;
    // ADR-0023 rule 7: a create through a view writes an Entity the view
    // selects, with Attributes it shows and Relationships to Entities it
    // shows, or nothing at all.
    #[cfg(feature = "views")]
    crate::views::admits_new(st, &tenant, &expanded).await?;
    #[cfg(feature = "views")]
    crate::views::schema_admits_typed(st, &tenant, &expanded, true)?;
    let regs = match crate::federation::write_plan(st, &tenant, &spec, &parsed.ctx, params, headers)
        .await?
    {
        crate::federation::WritePlan::Answered(r) => return Ok(*r),
        crate::federation::WritePlan::Forward(regs) => regs,
    };
    if !regs.is_empty() {
        let mut conflicts = Vec::new();
        let mut fwd = Vec::new();
        for reg in &regs {
            // 5.6.1.4: exclusive/redirect registrations not supporting the
            // Create Entity operation yield an error of type Conflict (and
            // are never contacted); an inclusive one is simply not forwarded.
            if !reg.supports("createEntity") {
                if reg.is_proxy() {
                    conflicts.push(crate::federation::conflict_part("createEntity", reg));
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
        let mut parts = Vec::new();
        // local part only when something non-proxied remains (4.3.6.3)
        if has_attrs || proxies.is_empty() {
            let mut local_exp = expand_entity(&rest, &parsed.ctx, ExpandOpts::default())?;
            stamp_new(&mut local_exp, &now_iso());
            if st
                .store
                .create(&tenant, Kind::Entity, &id, local_exp.clone())
                .await?
            {
                parts.push(crate::federation::Part::local(201, "created locally"));
            } else {
                parts.push(crate::federation::Part::local(
                    409,
                    format!("entity {id} already exists"),
                ));
            }
        }
        parts.extend(conflicts);
        let ctx_url = crate::federation::ctx_link_url(headers, &parsed.ctx.source);
        for (reg, frag) in fwd {
            parts.push(
                crate::federation::forward_part(
                    st,
                    reqwest::Method::POST,
                    format!("{}/ngsi-ld/v1/entities", reg.endpoint),
                    &[],
                    headers,
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
            created(entity_location(&id), &tenant),
            &tenant,
        ));
    }

    stamp_new(&mut expanded, &now_iso());
    if !st
        .store
        .create(&tenant, Kind::Entity, &id, expanded.clone())
        .await?
    {
        return Err(NgsiError::AlreadyExists(format!("entity {id} already exists")).into());
    }
    Ok(created(entity_location(&id), &tenant))
}

// ---------- GET /entities/{id} (5.7.1) ----------

pub async fn retrieve_entity(
    State(st): State<AppState>,
    Path(id): Path<String>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
) -> Response {
    match retrieve_entity_outer(&st, &id, &params, &headers).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

/// 5.7.1.4 Retrieve Entity: the EntityMap half of the clause is the shared
/// rule (`entity_maps::retrieve_with_map`); this is the retrieve it wraps.
async fn retrieve_entity_outer(
    st: &AppState,
    id: &str,
    params: &HashMap<String, String>,
    headers: &HeaderMap,
) -> ApiResult<Response> {
    crate::entity_map::retrieve_with_map(st, id, params, headers, false, |map| async move {
        retrieve_entity_inner(st, id, params, headers, map.as_ref()).await
    })
    .await
}

async fn retrieve_entity_inner(
    st: &AppState,
    id: &str,
    params: &HashMap<String, String>,
    headers: &HeaderMap,
    map: Option<&Value>,
) -> ApiResult<Response> {
    let tenant = tenant_from(headers)?;
    check_params(
        params,
        &[
            "attrs",
            "pick",
            "omit",
            "options",
            "format",
            "lang",
            "type",
            "geometryProperty",
            "datasetId",
            "containedBy",
            "join",
            "joinLevel",
            "local",
            "entityMap",
        ],
    )?;
    let accept = parse_accept_geo(headers)?;
    // 5.7.1.4: geometryProperty is only meaningful for the GeoJSON
    // representation — any other Accept is BadRequestData
    if params.contains_key("geometryProperty") && accept != Accept::GeoJson {
        return Err(NgsiError::BadRequestData(
            "geometryProperty requires Accept: application/geo+json (5.7.1.4)".into(),
        )
        .into());
    }
    let ctx = request_context(&st.loader, headers).await?;
    let named = ReadMembers::with_ctx(st, params, &ctx);
    let filter = gate!(
        st, &tenant, headers, "5.7.1",
        ids: &[id],
        types: &named.types,
        attrs: &named.attrs,
    )
    .await?;
    let mut repr = parse_repr(params, &ctx)?;
    repr.modified_by = crate::provenance::shows_modified_by(headers, st.provenance_from.as_deref());
    crate::repr::narrow_repr(&mut repr, &filter);
    let join = parse_join(params)?;
    check_linked_projection(&repr, &join)?;
    antares_model::EntityId::new(id)?;
    let local_doc = st.store.get(&tenant, Kind::Entity, id).await?;
    // T-3789: through a view, the tag a conditional write checks: of the
    // stored document, before any federated merge; sent only with a 200
    #[cfg(feature = "views")]
    let etag = crate::views::WriteGuard::of(st, std::iter::empty())
        .zip(local_doc.as_ref())
        .map(|(g, d)| g.tag(d));
    let looped = crate::federation::via_loop(
        headers,
        &crate::federation::alias_for(&st.host_alias, &tenant),
    );
    let fed_on = crate::federation::active(params) && !looped;
    // 6.3.17: abnormal distributed-GET outcomes surface as NGSILD-Warning
    let mut warnings: Vec<String> = Vec::new();
    if crate::federation::active(params) && looped {
        let spec = crate::registry::CsrSpec {
            ids: Some(vec![id.to_owned()]),
            ..Default::default()
        };
        // only a loop that suppressed a real forward is abnormal behaviour
        if !crate::federation::matching_regs(st, &tenant, &spec, &ctx, headers)
            .await?
            .is_empty()
        {
            warnings.push(crate::federation::warning(
                199,
                &crate::federation::alias_for(&st.host_alias, &tenant),
                "a registration loop has been detected",
            ));
        }
    }
    let doc = if fed_on {
        let mut sole_failure = None;
        let fed = crate::federation::fed_retrieve(
            st,
            &tenant,
            headers,
            &ctx,
            id,
            map,
            None,
            &mut warnings,
            &mut sole_failure,
        )
        .await?;
        match local_doc {
            Some(mut base) => {
                for aux_pass in [false, true] {
                    for (aux, d) in &fed {
                        if *aux == aux_pass {
                            crate::federation::merge_docs(&mut base, d, *aux);
                        }
                    }
                }
                base
            }
            None => {
                let first = fed
                    .iter()
                    .find(|(aux, _)| !aux)
                    .map(|(_, d)| d.clone())
                    .or_else(|| fed.first().map(|(_, d)| d.clone()));
                let Some(mut base) = first else {
                    // 6.3.17 p.278: "In the case of an exclusive or redirect
                    // registration, where all of the data is held outside of
                    // the Context Broker and held in a single registered
                    // source, the following errors shall be returned": 504
                    // when it "fails to respond in time", 502 when "the
                    // single forwarded request fails for any other reason".
                    // Its 404 stays the plain 404 below.
                    if let Some(p) = sole_failure.filter(|p| p.status != 404) {
                        let mut resp = crate::federation::source_error(&p, &tenant);
                        attach_warnings(&mut resp, &warnings);
                        return Ok(resp);
                    }
                    // 6.3.17: abnormal distributed outcomes surface as
                    // NGSILD-Warning even when the retrieve ends 404
                    let mut resp = ApiError::from(NgsiError::ResourceNotFound(format!(
                        "entity {id} not found"
                    )))
                    .into_response();
                    attach_warnings(&mut resp, &warnings);
                    echo_tenant(&tenant, &mut resp);
                    return Ok(resp);
                };
                for aux_pass in [false, true] {
                    for (aux, d) in &fed {
                        if *aux == aux_pass {
                            crate::federation::merge_docs(&mut base, d, *aux);
                        }
                    }
                }
                base
            }
        }
    } else {
        local_doc.ok_or_else(|| NgsiError::ResourceNotFound(format!("entity {id} not found")))?
    };
    // 5.7.1.4: no entity "whose id (URI), and where specified type, is
    // equivalent" — the optional ?type selector (4.17) narrows the target
    if !crate::negotiate::matches_type_param(&doc, params, &ctx) {
        return Err(NgsiError::ResourceNotFound(format!(
            "entity {id} does not match the type selector"
        ))
        .into());
    }
    // ADR-0020: an Entity outside the engine's narrowing answers the way
    // 5.7.1.4 answers an absent one — "If the NGSI-LD Entity does not
    // exist, an error of type ResourceNotFound shall be raised" — because a
    // refusal here would tell the caller the Entity is there.
    if !crate::repr::policy_admits(st, &tenant, &ctx, repr.policy_reach.as_deref(), &doc).await {
        return Err(NgsiError::ResourceNotFound(format!("entity {id} not found")).into());
    }
    // 5.7.1: attrs projection with no matching attribute ⇒ 404, read on the
    // Entity as the narrowing shows it: a hidden Attribute is absent, or
    // the 404 would tell the caller it is there (ADR-0023 rule 2)
    if let Some(want) = &repr.attrs {
        let shown = crate::repr::policy_view(&doc, &repr);
        if !want.iter().any(|a| shown.get(a).is_some()) {
            return Err(NgsiError::ResourceNotFound(format!(
                "entity {id} has none of the requested attributes"
            ))
            .into());
        }
    }
    let shaped = apply(&doc, &repr);
    if (repr.pick.is_some() || repr.omit.is_some())
        && shaped.as_object().is_some_and(|o| o.is_empty())
    {
        return Err(NgsiError::ResourceNotFound(format!(
            "projection matches nothing on entity {id}"
        ))
        .into());
    }
    let mut payload = compact_for(&repr, &shaped, &ctx);
    if let Some((mode, level)) = &join {
        let held = contained_by(params);
        let complete = match mode.as_str() {
            "inline" => {
                inline_join_beyond(
                    st,
                    &tenant,
                    &ctx,
                    &repr,
                    &mut payload,
                    *level,
                    &held,
                    &mut { MAX_JOIN_LOOKUPS },
                    fed_on.then_some(headers),
                )
                .await
            }
            "flat" => {
                let mut linked = std::collections::BTreeMap::new();
                let complete = collect_flat_beyond(
                    st,
                    &tenant,
                    &ctx,
                    &repr,
                    &doc,
                    *level,
                    &mut linked,
                    &held,
                    &mut { MAX_JOIN_LOOKUPS },
                )
                .await;
                if !linked.is_empty() {
                    let mut arr = vec![payload];
                    for (_, (ldoc, lrepr)) in linked {
                        arr.push(compact_for(&lrepr, &apply(&ldoc, &lrepr), &ctx));
                    }
                    payload = Value::Array(arr);
                }
                complete
            }
            _ => true,
        };
        if !complete {
            warnings.push(crate::federation::warning(
                199,
                &crate::federation::alias_for(&st.host_alias, &tenant),
                "the linked entity retrieval was truncated",
            ));
        }
    }
    let payload = if accept == Accept::GeoJson {
        to_geojson_feature(payload, params.get("geometryProperty"))
    } else {
        payload
    };
    let mut resp = respond_prefer(StatusCode::OK, payload, &ctx, accept, &tenant, headers);
    attach_warnings(&mut resp, &warnings);
    filter.mark(&st.host_alias, resp.headers_mut());
    #[cfg(feature = "views")]
    if let Some(tag) = etag.and_then(|t| axum::http::HeaderValue::from_str(&t).ok()) {
        resp.headers_mut().insert(axum::http::header::ETAG, tag);
    }
    Ok(resp)
}

/// 5.7.1.4 / 5.7.2.4: a `{…}` projection selects into Linked Entities —
/// it must be requested via join, and may not select deeper than joinLevel.
pub(crate) fn check_linked_projection(
    repr: &crate::repr::Repr,
    join: &Option<(String, usize)>,
) -> ApiResult<()> {
    let depth = repr
        .pick
        .as_deref()
        .map(crate::repr::proj_depth)
        .unwrap_or(0)
        .max(
            repr.omit
                .as_deref()
                .map(crate::repr::proj_depth)
                .unwrap_or(0),
        );
    if depth == 0 {
        return Ok(());
    }
    match join {
        Some((mode, level)) if mode != "@none" => {
            if depth > *level {
                return Err(NgsiError::BadRequestData(format!(
                    "projected attribute depth {depth} exceeds joinLevel {level} (5.7.1.4/5.7.2.4)"
                ))
                .into());
            }
            Ok(())
        }
        _ => Err(NgsiError::BadRequestData(
            "projection uses Linked Entity selection but join is not specified (5.7.1.4/5.7.2.4)"
                .into(),
        )
        .into()),
    }
}

/// join/joinLevel params (4.5.23). Returns (mode, level).
pub fn parse_join(params: &HashMap<String, String>) -> ApiResult<Option<(String, usize)>> {
    let Some(mode) = params.get("join") else {
        return Ok(None);
    };
    if !["inline", "flat", "@none"].contains(&mode.as_str()) {
        return Err(NgsiError::BadRequestData(format!("invalid join {mode:?}")).into());
    }
    let level = match params.get("joinLevel") {
        Some(l) => l
            .parse::<usize>()
            .ok()
            // Bounded traversal depth
            .filter(|l| *l >= 1 && *l <= crate::bounds::MAX_JOIN_LEVEL)
            .ok_or_else(|| {
                NgsiError::BadRequestData(format!(
                    "invalid joinLevel {l:?} (1..={})",
                    crate::bounds::MAX_JOIN_LEVEL
                ))
            })?,
        None => 1,
    };
    if mode == "@none" {
        return Ok(None);
    }
    Ok(Some((mode.clone(), level)))
}

/// Table 6.4.3.2-1 `containedBy`: "List of entity ids which have previously
/// been encountered whilst retrieving the Entity Graph. Only applicable if
/// joinLevel is present." They are already in the graph the client is
/// assembling, so 4.5.23.1's "avoid ... duplicates or loops" counts them as
/// resolved and the walk does not follow them again.
pub fn contained_by(params: &HashMap<String, String>) -> Vec<String> {
    params
        .get("containedBy")
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

// ---------- GET /entities/ (5.7.2) ----------

pub async fn query_entities(
    State(st): State<AppState>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
) -> Response {
    match query_entities_outer(&st, params, &headers).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

/// 5.5.14 / 5.5.9.3: a query referencing an EntityMap (NGSILD-EntityMap
/// request header, 6.4.3.2-2) is fixed to the map's Entities; the filters
/// are re-checked at processing time and local entries that no longer match
/// are removed from the map by its creator. An expired or unknown map means
/// "no inference can be made … a new one shall be created".
async fn query_entities_outer(
    st: &AppState,
    mut params: HashMap<String, String>,
    headers: &HeaderMap,
) -> ApiResult<Response> {
    let tenant = tenant_from(headers)?;
    let q_ast = params.get("q").map(|q| parse_q(q)).transpose()?;
    let named = ReadMembers::of(st, headers, &params).await;
    let filter = gate!(
        st, &tenant, headers, "5.7.2",
        q: q_ast.as_ref(),
        scope_q: params.get("scopeQ").map(String::as_str),
        types: &named.types,
        attrs: &named.attrs,
        geo: named.geo.as_ref(),
    )
    .await?;
    let Some(map_ref) = single_header(headers, "NGSILD-EntityMap")? else {
        return query_entities_inner(st, &params, headers, &filter).await;
    };
    // 5.7.2.4: an unknown parameter and a too-wide query are BadRequestData
    // for this request, whether or not it carries an EntityMap reference —
    // and the paged fetch below walks the whole map, locally and forwarded,
    // before the inner call would reach these same two checks.
    check_params(&params, crate::negotiate::QUERY_PARAMS)?;
    if !qualifies_non_wide(&params, q_ast.as_ref()) {
        return Err(NgsiError::BadRequestData(
            "query needs at least one of type, attrs, q, georel (5.7.2)".into(),
        )
        .into());
    }
    let map_id = map_ref.rsplit('/').next().unwrap_or(&map_ref).to_owned();
    let Some(mut map) = crate::entity_map::map_if_accessible(st, &tenant, headers, &map_id).await
    else {
        // 5.5.14: expired or inaccessible → a new EntityMap is created
        params.insert("entityMap".into(), "true".into());
        return query_entities_inner(st, &params, headers, &filter).await;
    };
    let ctx = request_context(&st.loader, headers).await?;
    // a request that references a live map does not create a new one
    params.remove("entityMap");
    let (offset, limit, count) = page_params(st, &params)?;
    // pagination links carry the ORIGINAL query, never the page's id list
    let link_params = params.clone();
    let accept = parse_accept_geo(headers)?;

    // 5.5.9.3 paged fetch: the map fixes the candidate id set; candidates
    // are fetched (locally + forwarded) chunk by chunk, "filters shall be
    // rechecked before returning results" per chunk, and visited entries
    // that no longer match are removed from the map — "Entities not or no
    // longer fitting the query shall be removed from the Entity map during
    // pagination". Pruning is judgeable only for "@none" (local) entries: a
    // remote-backed id may merely have an unreachable source right now
    // (5.5.14). Memory per request is O(chunk), never O(map) — the reason
    // EntityMaps exist for the distributed case. count=true walks every
    // candidate (the total needs each id checked), still chunk-bounded.
    let ids: Vec<String> = crate::entity_map::candidate_ids(&map, &params);
    let narrow = crate::repr::narrowing_reading(&filter, &params, &ctx)?;
    let looped = crate::federation::via_loop(
        headers,
        &crate::federation::alias_for(&st.host_alias, &tenant),
    );
    let chunk_size = limit.max(20);
    let mut page_ids: Vec<String> = Vec::new();
    let mut stale: Vec<String> = Vec::new();
    let (mut skipped, mut total, mut more, mut visited) = (0usize, 0usize, false, 0usize);
    for chunk in ids.chunks(chunk_size) {
        let mut p = params.clone();
        p.remove("limit");
        p.remove("offset");
        p.remove("count");
        p.insert("id".into(), chunk.join(","));
        // the final page fetch below re-surfaces the same peers' warnings
        let mut chunk_warnings = Vec::new();
        let fed = if crate::federation::active(&p) && !looped {
            crate::federation::fed_query(st, &tenant, headers, &ctx, &p, &mut chunk_warnings)
                .await?
        } else {
            Vec::new()
        };
        let docs = filter_entities_fed(st, &tenant, &p, &ctx, fed, Some(&narrow)).await?;
        let matched: std::collections::HashSet<&str> = docs
            .iter()
            .filter_map(|d| d.get("id").and_then(Value::as_str))
            .collect();
        if let Some(emap) = map.get_mut("entityMap").and_then(Value::as_object_mut) {
            for id in chunk {
                let local_only = emap
                    .get(id)
                    .and_then(Value::as_array)
                    .is_some_and(|a| matches!(a.as_slice(), [only] if only == "@none"));
                if local_only && !matched.contains(id.as_str()) {
                    emap.remove(id);
                    stale.push(id.clone());
                }
            }
        }
        visited = visited.saturating_add(chunk.len());
        for id in chunk {
            if !matched.contains(id.as_str()) {
                continue;
            }
            total = total.saturating_add(1);
            if skipped < offset {
                skipped = skipped.saturating_add(1);
            } else if page_ids.len() < limit {
                page_ids.push(id.clone());
            } else {
                more = true;
            }
        }
        if !count && page_ids.len() == limit {
            // page full — next exists if an extra match was seen or
            // unvisited candidates remain ("pages shall always be filled to
            // the maximum, as long as Entities are available")
            more = more || visited < ids.len();
            break;
        }
    }
    if count {
        more = total > offset.saturating_add(limit);
    }
    crate::entity_map::map_prune(st, &tenant, &map_id, &stale).await?;
    // fix the final fetch to exactly the page's survivors (5.5.14: an empty
    // set is fixed to nothing); one extra page-sized fetch keeps the whole
    // repr pipeline shared instead of forked
    params.insert(
        "id".into(),
        if page_ids.is_empty() {
            "urn:ngsi-ld:entitymap:empty".to_owned()
        } else {
            page_ids.join(",")
        },
    );
    params.remove("offset");
    params.remove("count");
    if limit == 0 {
        // count-only shape: the inner default limit is irrelevant against
        // the empty id sentinel
        params.remove("limit");
    }
    // The narrowing reaches the answer here; the map's own contents are
    // still the ones the query built, which is what P5's per-subject map is
    // for.
    let mut resp = query_entities_inner(st, &params, headers, &filter).await?;
    // "The location of the EntityMap used in the query operation is
    // returned in the response" (6.4.3.2-2)
    if let Ok(v) = format!("/ngsi-ld/v1/entityMaps/{map_id}").parse() {
        resp.headers_mut().insert("NGSILD-EntityMap", v);
    }
    if count {
        if let Ok(v) = total.to_string().parse() {
            resp.headers_mut().insert("NGSILD-Results-Count", v);
        }
    }
    // 6.3.10 links from the original query — the inner call saw offset 0
    // over exactly one page, so it emitted none
    for (off, rel, cond) in [
        (offset.saturating_add(limit), "next", more && limit > 0),
        (offset.saturating_sub(limit.max(1)), "prev", offset > 0),
    ] {
        if !cond {
            continue;
        }
        let mut qp: Vec<String> = link_params
            .iter()
            .filter(|(k, _)| k.as_str() != "offset")
            .map(|(k, v)| format!("{k}={}", crate::paging::query_value(v)))
            .collect();
        qp.push(format!("offset={off}"));
        qp.sort(); // deterministic order — the suite string-compares links
        let ty = match accept {
            Accept::LdJson => ";type=\"application/ld+json\"",
            Accept::Json => ";type=\"application/json\"",
            Accept::GeoJson => ";type=\"application/geo+json\"",
        };
        if let Ok(v) = format!("</ngsi-ld/v1/entities?{}>; rel=\"{rel}\"{ty}", qp.join("&")).parse()
        {
            resp.headers_mut().append(axum::http::header::LINK, v);
        }
    }
    Ok(resp)
}

async fn query_entities_inner(
    st: &AppState,
    params: &HashMap<String, String>,
    headers: &HeaderMap,
    filter: &std::sync::Arc<crate::policy::Filter>,
) -> ApiResult<Response> {
    let tenant = tenant_from(headers)?;
    check_params(params, crate::negotiate::QUERY_PARAMS)?;
    let accept = parse_accept_geo(headers)?;
    let ctx = request_context(&st.loader, headers).await?;

    // 5.7.2.4 a-e: id/idPattern alone are NOT sufficient, and the attrs
    // list / q must include "at least one non-system Attribute" to qualify.
    // The judgement is about what the CLIENT asked for, so it reads the
    // request's own `q` — a policy condition is not the client's filter and
    // does not make a wide query narrow (ADR-0020).
    let has_filter = qualifies_non_wide(
        params,
        params.get("q").map(|q| parse_q(q)).transpose()?.as_ref(),
    );
    // what the client's own query reads, before the engine's q joins it
    // (ADR-0023 rule 11)
    let client_params = params;
    // Everything below reads the narrowed query: the store push-down, the
    // local re-check 5.7.2.4 runs over merged results, and the query the
    // request is forwarded with.
    let narrowed = filter.narrow_params(params, &ctx)?;
    let params = &narrowed;
    let q_ast = params
        .get("q")
        .map(|q| antares_ql::parse_narrowed_q(q))
        .transpose()?;
    // 5.7.2.4 validation bullets (p.201), in the spec's own order.
    if params.get("type").map(String::as_str) == Some("*")
        && params.get("local").map(String::as_str) == Some("false")
    {
        return Err(NgsiError::BadRequestData(
            "type=* implies local and shall not be combined with local=false \
             (Table 6.4.3.2-1)"
                .into(),
        )
        .into());
    }
    if params.contains_key("geometryProperty") && accept != Accept::GeoJson {
        return Err(NgsiError::BadRequestData(
            "geometryProperty requires Accept: application/geo+json (5.7.2.4)".into(),
        )
        .into());
    }
    // "If the ordering parameter is present and the execution of the operation
    // is not limited to the local scope then BadRequestData" — reinforced by
    // 4.23.1: "Sort ordering is never applied to distributed operations."
    // The subject is the EXECUTION: a query nothing federates to runs locally
    // regardless of `local=true`, which is why this asks would_federate rather
    // than active (ETSI 019_19 orders without local).
    crate::paging::check_collation(params)?;
    if params.contains_key("orderBy")
        && crate::federation::would_federate(st, &tenant, &ctx, params, headers).await?
    {
        return Err(NgsiError::BadRequestData(
            "orderBy requires local scope — ordering is never applied to \
             distributed operations (5.7.2.4, 4.23.1)"
                .into(),
        )
        .into());
    }
    if !has_filter {
        return Err(NgsiError::BadRequestData(
            "query needs at least one of type, attrs, q, georel (5.7.2)".into(),
        )
        .into());
    }

    let mut repr = parse_repr(params, &ctx)?;
    repr.modified_by = crate::provenance::shows_modified_by(headers, st.provenance_from.as_deref());
    crate::repr::narrow_repr(&mut repr, filter);
    crate::repr::note_reads(&mut repr, client_params, &ctx)?;
    let join = parse_join(params)?;
    check_linked_projection(&repr, &join)?;
    // 5.7.2.4: filter conditions using Linked Entity attributes need join,
    // and their hop depth may not exceed joinLevel ("too deep query")
    let link_depth = q_ast
        .as_ref()
        .map(antares_ql::QNode::max_link_depth)
        .unwrap_or(0);
    if link_depth > 0 {
        match &join {
            Some((mode, level)) if mode != "@none" => {
                if link_depth > *level {
                    return Err(NgsiError::BadRequestData(format!(
                        "linked attribute query depth {link_depth} exceeds joinLevel {level} \
                         (5.7.2.4 — too deep query)"
                    ))
                    .into());
                }
            }
            _ => {
                return Err(NgsiError::BadRequestData(
                    "q references Linked Entity attributes but join is not specified \
                     (5.7.2.4 — too deep query)"
                        .into(),
                )
                .into());
            }
        }
    }
    // 5.7.2.4: a syntactically invalid context source filter is 400.
    if let Some(csf) = params.get("csf") {
        parse_q(csf)?;
    }
    // 6.3.17: abnormal distributed-GET outcomes surface as NGSILD-Warning
    let mut warnings: Vec<String> = Vec::new();
    let looped = crate::federation::via_loop(
        headers,
        &crate::federation::alias_for(&st.host_alias, &tenant),
    );
    let fed = if crate::federation::active(params) && !looped {
        crate::federation::fed_query(st, &tenant, headers, &ctx, params, &mut warnings).await?
    } else {
        if crate::federation::active(params)
            && looped
            && crate::federation::would_federate(st, &tenant, &ctx, params, headers).await?
        {
            warnings.push(crate::federation::warning(
                199,
                &crate::federation::alias_for(&st.host_alias, &tenant),
                "a registration loop has been detected",
            ));
        }
        Vec::new()
    };
    // Pushdown gates: pagination per page_pushdown_allowed (no federation
    // candidates, no idPattern, no orderBy). Projection additionally excludes
    // join (linked-entity walks read page docs) and GeoJSON output.
    let (p_offset, p_limit, _) = page_params(st, params)?;
    let push_page = page_pushdown_allowed(fed.is_empty(), params);
    let push_proj = join.is_none() && accept != Accept::GeoJson;
    // an ordered read sorts on what its keys read and reads its page whole
    // afterwards (T-4919)
    let order_keys = params
        .get("orderBy")
        .map(|spec| crate::paging::order_heads(spec, &ctx));
    let pushdown = Pushdown {
        page: push_page.then_some((p_offset, p_limit)),
        proj: push_proj.then_some(&repr),
        order_keys: order_keys.as_deref(),
        q: q_ast.as_ref(),
    };
    let shape = {
        let (repr, ctx) = (repr.clone(), ctx.clone());
        move |doc: &Value| {
            let shaped = apply(doc, &repr);
            // pick projections that match nothing drop the entity entirely
            if repr.pick.is_some() && shaped.as_object().is_some_and(|o| o.is_empty()) {
                return None;
            }
            Some(compact_for(&repr, &shaped, &ctx))
        }
    };
    // T-6055: a plain list writes each Entity as it is admitted and holds the
    // page as its answer (~0.25 KB an Entity), where the stored `Value` tree
    // takes ~5 KB
    // an ordered read sorts trimmed documents and writes its page as read
    // whole by id ([`whole_page_written`])
    let as_frames = join.is_none() && accept != Accept::GeoJson;
    let ld_ctx = crate::negotiate::list_context(&ctx, accept);
    let write = |doc: &Value| {
        shape(doc).map(|shaped| crate::negotiate::list_frame(shaped, ld_ctx.as_ref()))
    };
    let mut filtered = match as_frames && order_keys.is_none() && fed.is_empty() {
        true => {
            filter_entities_written(st, &tenant, params, &ctx, pushdown, Some(&repr), &write)
                .await?
        }
        false => {
            filter_entities_paged(st, &tenant, params, &ctx, fed, pushdown, Some(&repr)).await?
        }
    };
    // the page is read back by `id`, a comma-separated list
    let listable = |d: &Value| {
        d.get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.contains(','))
    };
    if filtered.trimmed && !filtered.docs.iter().all(listable) {
        filtered = filter_entities_paged(
            st,
            &tenant,
            params,
            &ctx,
            Vec::new(),
            Pushdown {
                order_keys: None,
                ..pushdown
            },
            Some(&repr),
        )
        .await?;
    }
    let trimmed = filtered.trimmed;
    let mut matches = filtered.docs;
    if let Some(spec) = params.get("orderBy") {
        crate::repr::order_as_shown(&mut matches, Some(&repr), |m| {
            order_entities(m, spec, params, &ctx)
        })?;
    }
    let keyset = push_page && crate::paging::keyset_allowed(params);
    let cut = PageCut {
        keyset: keyset && params.contains_key(crate::paging::CURSOR),
        paged: filtered.paged,
        total: filtered.total,
        offset: p_offset,
        limit: p_limit,
    };
    let (page, count_hdr, links) = match filtered.written.is_empty() {
        true => {
            let (page, count, links) = cut.page(st, params, matches)?;
            let page = match (trimmed, as_frames) {
                (true, true) => Page::Written(
                    whole_page_written(st, &tenant, params, &ctx, &repr, page, &write).await?,
                ),
                (true, false) => {
                    Page::Docs(whole_page(st, &tenant, params, &ctx, &repr, page).await?)
                }
                (false, _) => Page::Docs(page),
            };
            (page, count, links)
        }
        false => {
            let (page, count, links) = cut.page(st, params, filtered.written)?;
            (Page::Written(page), count, links)
        }
    };
    let links = match keyset {
        true => crate::paging::with_next_cursor(
            links,
            &crate::paging::view_of(st),
            tenant.as_str(),
            params,
            page.last_id().as_deref(),
            p_offset.saturating_add(p_limit),
        ),
        false => links,
    };

    let mut resp = match page {
        Page::Written(rows) => {
            let frames = rows.into_iter().filter_map(|w| w.frame);
            crate::negotiate::respond_frames(StatusCode::OK, frames, &ctx, accept, &tenant)
        }
        Page::Docs(page) if join.is_none() && accept != Accept::GeoJson => {
            // T-6049: each Entity is shaped as it is written, and the page holds no shaped
            // copy of itself beside the read one
            let docs = page.into_iter().filter_map(move |doc| shape(&doc));
            crate::negotiate::respond_list(StatusCode::OK, docs, &ctx, accept, &tenant)
        }
        Page::Docs(page) => {
            let mut payload: Vec<Value> = page.iter().filter_map(&shape).collect();
            if let Some((mode, level)) = &join {
                let mut complete = true;
                let held = contained_by(params);
                // 4.5.23.1 bounds the WIDTH of the retrieval per request, so one
                // allowance is spent across the whole page: minting a fresh budget per
                // payload Entity multiplied it by the page size, and a page of
                // max_limit densely linked Entities bought MAX_JOIN_LOOKUPS lookups
                // each.
                let mut budget = MAX_JOIN_LOOKUPS;
                match mode.as_str() {
                    "inline" => {
                        for p in &mut payload {
                            complete &= inline_join_beyond(
                                st,
                                &tenant,
                                &ctx,
                                &repr,
                                p,
                                *level,
                                &held,
                                &mut budget,
                                crate::federation::active(params).then_some(headers),
                            )
                            .await;
                        }
                    }
                    "flat" => {
                        let mut linked = std::collections::BTreeMap::new();
                        for doc in &page {
                            complete &= collect_flat_beyond(
                                st,
                                &tenant,
                                &ctx,
                                &repr,
                                doc,
                                *level,
                                &mut linked,
                                &held,
                                &mut budget,
                            )
                            .await;
                        }
                        let page_ids: Vec<&str> =
                            page.iter().filter_map(|d| d["id"].as_str()).collect();
                        for (id, (ldoc, lrepr)) in linked {
                            if !page_ids.contains(&id.as_str()) {
                                payload.push(compact_for(&lrepr, &apply(&ldoc, &lrepr), &ctx));
                            }
                        }
                    }
                    _ => {}
                }
                if !complete {
                    warnings.push(crate::federation::warning(
                        199,
                        &crate::federation::alias_for(&st.host_alias, &tenant),
                        "the linked entity retrieval was truncated",
                    ));
                }
            }
            if accept == Accept::GeoJson {
                let fc = to_geojson_collection(payload, params.get("geometryProperty"));
                respond_prefer(StatusCode::OK, fc, &ctx, accept, &tenant, headers)
            } else {
                crate::negotiate::respond_list(StatusCode::OK, payload, &ctx, accept, &tenant)
            }
        }
    };
    attach_paging(&mut resp, count_hdr, &links);
    attach_warnings(&mut resp, &warnings);
    // 6.4.3.2: entityMap=true — the EntityMap for this query is (re)created;
    // the response carries NGSILD-EntityMap and 201 Created.
    if params.get("entityMap").map(String::as_str) == Some("true") {
        let map = build_query_map(st, &tenant, headers, &ctx, params, filter).await?;
        *resp.status_mut() = StatusCode::CREATED;
        if let Some(id) = map.get("id").and_then(Value::as_str) {
            if let Ok(v) = format!("/ngsi-ld/v1/entityMaps/{id}").parse() {
                resp.headers_mut().insert("NGSILD-EntityMap", v);
            }
        }
    }
    filter.mark(&st.host_alias, resp.headers_mut());
    Ok(resp)
}

/// A Query Entities page: documents, or each Entity as written
/// ([`filter_entities_written`]).
enum Page {
    Docs(Vec<Value>),
    Written(Vec<Written>),
}

impl Page {
    /// The id of the page's last Entity, which a keyset cursor seeks past.
    fn last_id(&self) -> Option<String> {
        let last = match self {
            Page::Docs(p) => p.last()?.get("id")?.as_str()?.to_owned(),
            Page::Written(p) => p.last()?.id.clone(),
        };
        Some(last)
    }
}

/// How Query Entities cuts its matches to the client's page (6.3.10).
struct PageCut {
    /// A keyset cursor: the rows start after its id, at the client's offset.
    keyset: bool,
    /// The store paged ([`Filtered::paged`]) and counted (`total`).
    paged: bool,
    total: Option<usize>,
    offset: usize,
    limit: usize,
}

impl PageCut {
    fn page<T>(
        &self,
        st: &AppState,
        params: &HashMap<String, String>,
        mut matches: Vec<T>,
    ) -> ApiResult<(Vec<T>, Option<usize>, Vec<String>)> {
        const PATH: &str = "/ngsi-ld/v1/entities";
        if self.keyset {
            let total = match self.paged {
                true => self.total.unwrap_or(matches.len()),
                false => matches.len(),
            };
            matches.truncate(self.limit);
            let total = self.offset.saturating_add(total);
            paginate_pre(st, params, matches, PATH, total)
        } else if self.paged {
            let total = self.total.unwrap_or(matches.len());
            paginate_pre(st, params, matches, PATH, total)
        } else {
            paginate(st, params, matches, PATH)
        }
    }
}

/// 5.7.2.4 / 5.7.4.4 / 5.14.4.4 a-e: a query qualifies (is not "too wide")
/// only with a type selector, an attrs list or q naming at least one
/// non-system Attribute, a geoquery, or local scope.
pub(crate) fn qualifies_non_wide(
    params: &HashMap<String, String>,
    q_ast: Option<&antares_ql::QNode>,
) -> bool {
    let attrs_qualify = params.get("attrs").is_some_and(|a| {
        a.split(',')
            .any(|n| antares_ql::is_non_system_attr(n.trim()))
    });
    let q_qualifies = q_ast.is_some_and(|ast| {
        ast.attribute_paths()
            .iter()
            .any(|h| antares_ql::is_non_system_attr(h))
    });
    params.contains_key("type")
        || attrs_qualify
        || q_qualifies
        || params.contains_key("georel")
        || params.get("local").map(String::as_str) == Some("true")
}

/// Same, with federated candidate docs merged in before filtering (4.3.6.7).
pub async fn filter_entities_fed(
    st: &AppState,
    tenant: &TenantId,
    params: &HashMap<String, String>,
    ctx: &antares_jsonld::Context,
    fed: Vec<(bool, Value)>,
    narrow: Option<&crate::repr::Repr>,
) -> ApiResult<Vec<Value>> {
    Ok(
        filter_entities_paged(st, tenant, params, ctx, fed, Pushdown::default(), narrow)
            .await?
            .docs,
    )
}

/// What the paged variant produced. `paged` = the store already applied
/// ORDER BY id + LIMIT/OFFSET (and `total` is the pre-LIMIT match count), so
/// the caller must NOT slice again.
pub struct Filtered {
    pub docs: Vec<Value>,
    pub paged: bool,
    pub total: Option<usize>,
    /// How many documents the store returned, before the evaluator below
    /// dropped any. With a pushed page that is the size of the SQL page —
    /// which is what a chunked walk has to step over, since `docs` undercounts
    /// it whenever idPattern (invisible to the store filter) removed rows.
    pub rows: usize,
    /// The store decided the match set and kept only the core members and
    /// [`Pushdown::order_keys`] of each document: enough to order and page
    /// it, and the page is then read whole by id (T-4919).
    pub trimmed: bool,
    /// The matches as written, in place of `docs`, from
    /// [`filter_entities_written`].
    pub(crate) written: Vec<Written>,
}

/// A match [`filter_entities_written`] wrote as it admitted it.
pub(crate) struct Written {
    pub(crate) id: String,
    /// `None`: the Entity shapes to nothing and is left out of the answer,
    /// though it still counts in the page.
    pub(crate) frame: Option<axum::body::Bytes>,
}

/// What [`filter_entities_written`] writes each admitted Entity with.
pub(crate) type Write<'a> = &'a (dyn Fn(&Value) -> Option<axum::body::Bytes> + Sync);

/// Whether the store may be asked to project members away. Only when its
/// answer is the final answer: 5.7.2.4 applies the query, geoquery, Scope
/// query and Attribute filters after remote parts have been aggregated, so
/// with federated candidates present those filters still run over documents
/// the store would have stripped; and 4.23 orders Entities by the value of
/// the ordering member, which a projection may have removed — leaving the
/// comparator nothing to compare and the client id order.
fn proj_pushdown_allowed(fed_is_empty: bool, params: &HashMap<String, String>) -> bool {
    fed_is_empty && !params.contains_key("orderBy")
}

/// Whether `q` reads only Attributes that every rule of a type in `groups`
/// shows and the narrowing's own projection keeps, in the form the store
/// compiles exactly: one Attribute per term, no Linked Entity hop, no system
/// member. An Entity of those types shows at least what one such rule shows,
/// so on those Attributes it answers `q` as stored and as shown alike.
fn reads_only_shown(
    q: &antares_ql::QNode,
    rules: &crate::policy::Filter,
    groups: &[Vec<String>],
    narrow: Option<&crate::repr::Repr>,
    expand: &dyn Fn(&str) -> String,
) -> bool {
    antares_ql::sql::compile_q(q, "entity", 1, expand).is_some()
        && shows_alike(q.attribute_paths(), rules, groups, narrow, expand)
}

/// Whether every rule of a type in `groups` shows each of `names` (terms) and
/// the narrowing's own projection keeps it: on those Attributes the Entity as
/// stored and as shown are alike.
fn shows_alike<'n>(
    names: impl IntoIterator<Item = &'n str>,
    rules: &crate::policy::Filter,
    groups: &[Vec<String>],
    narrow: Option<&crate::repr::Repr>,
    expand: &dyn Fn(&str) -> String,
) -> bool {
    if !rules.omit_by_type.is_empty() {
        return false;
    }
    // every rule naming a type the request selects
    let selected: Vec<&crate::policy::Rule> = rules
        .rules
        .iter()
        .filter(|r| {
            r.types
                .iter()
                .any(|t| groups.iter().flatten().any(|g| g == t))
        })
        .collect();
    let heads = |nodes: &Option<Vec<crate::repr::ProjNode>>| {
        nodes
            .as_ref()
            .map(|n| n.iter().map(|n| n.iri.clone()).collect::<Vec<_>>())
    };
    let pick = narrow.and_then(|r| heads(&r.policy_pick));
    let omit = narrow
        .and_then(|r| heads(&r.policy_omit))
        .unwrap_or_default();
    !selected.is_empty()
        && names.into_iter().all(|name| {
            let iri = expand(name);
            antares_ql::is_non_system_attr(name)
                && !omit.contains(&iri)
                && pick.as_ref().is_none_or(|p| p.contains(&iri))
                && selected
                    .iter()
                    .all(|r| r.every_attribute || r.attributes.contains(&iri))
        })
}

/// The client's type selection (OR of AND groups, `None` = any type) with
/// one of the rules' types required in every group: `(a;b)|c` under rules
/// of `T` and `U` selects `(a;b;T)|(a;b;U)|(c;T)|(c;U)`, and a group that
/// already names one of them stays as it is (`(a;T)` selects no more than
/// `(a;T;U)`). `None` when the rules name no type, so a selection is never
/// dropped into "any".
fn judged_types<'a>(
    client: Option<&'a [Vec<String>]>,
    rules: &crate::policy::Filter,
) -> Option<std::borrow::Cow<'a, [Vec<String>]>> {
    let mut union: Vec<&String> = rules.rules.iter().flat_map(|r| &r.types).collect();
    union.sort();
    union.dedup();
    if union.is_empty() {
        return None;
    }
    let named = |g: &Vec<String>| g.iter().any(|t| union.contains(&t));
    if let Some(groups) = client.filter(|c| c.iter().all(named)) {
        return Some(std::borrow::Cow::Borrowed(groups));
    }
    let any = [Vec::new()];
    Some(std::borrow::Cow::Owned(
        client
            .unwrap_or(&any)
            .iter()
            .flat_map(|g| -> Vec<Vec<String>> {
                if named(g) {
                    return vec![g.clone()];
                }
                union
                    .iter()
                    .map(|t| {
                        let mut g = g.clone();
                        g.push((*t).clone());
                        g
                    })
                    .collect()
            })
            .collect(),
    ))
}

/// The documents of a page ordered over trimmed ones ([`Filtered::trimmed`]),
/// read whole by id under the same filter, in the page's order. An Entity
/// deleted between the two reads drops out of the page.
async fn whole_page(
    st: &AppState,
    tenant: &TenantId,
    params: &HashMap<String, String>,
    ctx: &antares_jsonld::Context,
    repr: &crate::repr::Repr,
    page: Vec<Value>,
) -> ApiResult<Vec<Value>> {
    let ids: Vec<&str> = page.iter().filter_map(|d| d.get("id")?.as_str()).collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut by_id = params.clone();
    by_id.insert("id".to_owned(), ids.join(","));
    let whole = filter_entities_paged(
        st,
        tenant,
        &by_id,
        ctx,
        Vec::new(),
        Pushdown::default(),
        Some(repr),
    )
    .await?;
    let mut found: HashMap<String, Value> = whole
        .docs
        .into_iter()
        .filter_map(|d| Some((d.get("id")?.as_str()?.to_owned(), d)))
        .collect();
    Ok(ids.iter().filter_map(|id| found.remove(*id)).collect())
}

/// [`whole_page`] with each Entity written as it is read
/// ([`filter_entities_written`]), in the order of the trimmed `page`.
async fn whole_page_written(
    st: &AppState,
    tenant: &TenantId,
    params: &HashMap<String, String>,
    ctx: &antares_jsonld::Context,
    repr: &crate::repr::Repr,
    page: Vec<Value>,
    write: Write<'_>,
) -> ApiResult<Vec<Written>> {
    let ids: Vec<String> = page
        .iter()
        .filter_map(|d| Some(d.get("id")?.as_str()?.to_owned()))
        .collect();
    drop(page);
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut by_id = params.clone();
    by_id.insert("id".to_owned(), ids.join(","));
    let whole = filter_entities_written(
        st,
        tenant,
        &by_id,
        ctx,
        Pushdown::default(),
        Some(repr),
        write,
    )
    .await?;
    // a read of documents (a walk in store pages) is written here
    let docs = whole.docs.iter().filter_map(|d| {
        let id = d.get("id")?.as_str()?.to_owned();
        Some(Written {
            id,
            frame: write(d),
        })
    });
    let mut found: HashMap<String, Written> = whole
        .written
        .into_iter()
        .chain(docs)
        .map(|w| (w.id.clone(), w))
        .collect();
    Ok(ids.iter().filter_map(|id| found.remove(id)).collect())
}

/// Whether the store may be asked to cut the page (ORDER BY id +
/// LIMIT/OFFSET). Only when nothing outside it still narrows or reorders the
/// match set:
///
/// * federated candidates are merged in afterwards, and 5.7.2.4 applies the
///   query, geoquery, Scope query and Attribute filters only after that
///   aggregation — so a SQL page would be cut from the wrong set;
/// * `idPattern` is not part of the store filter, so it drops rows the SQL
///   page already counted;
/// * `orderBy` orders the whole match set by the value of the ordering member
///   (4.23) before the page is cut, and that comparison order is the
///   evaluator's.
///
/// `limit=0` — the count-only shape of 6.3.10, where `count=true` is
/// mandatory — IS pushed: the store returns no rows and counts the match set,
/// which is the same count the scan derives from a materialized one, without
/// materializing 100 million documents to throw them away.
fn page_pushdown_allowed(fed_is_empty: bool, params: &HashMap<String, String>) -> bool {
    fed_is_empty && !params.contains_key("idPattern") && !params.contains_key("orderBy")
}

/// Entity Type Selection Language (4.17): `,`/`|` = OR, `(a;b)` = AND.
/// Table 6.4.3.2-1: `"*"` selects every Entity Type, i.e. no type predicate
/// at all. Expanding it as a term yields an IRI nothing matches, which is
/// how `type=*` silently returned an empty array.
fn type_selection(
    params: &HashMap<String, String>,
    ctx: &antares_jsonld::Context,
) -> Option<Vec<Vec<String>>> {
    params.get("type").filter(|s| *s != "*").map(|s| {
        s.split([',', '|'])
            .map(|alt| {
                alt.trim()
                    .trim_start_matches('(')
                    .trim_end_matches(')')
                    .split(';')
                    .map(|t| ctx.expand_key(t.trim()))
                    .collect()
            })
            .collect()
    })
}

/// The expanded `attrs` list of a query.
fn attr_list(
    params: &HashMap<String, String>,
    ctx: &antares_jsonld::Context,
) -> Option<Vec<String>> {
    params
        .get("attrs")
        .map(|s| s.split(',').map(|t| ctx.expand_key(t.trim())).collect())
}

/// What a read names, for the policy gate (ADR-0020: "`Operation` is the
/// expanded request"): the types of its type selector, its `attrs`, its
/// `q` and its GeoQuery, expanded with the request @context exactly as the
/// store query uses them. Built only with an engine attached. A member
/// that does not parse is left out, and the read refuses it after the gate
/// as before.
#[derive(Default)]
pub(crate) struct ReadMembers {
    pub types: Vec<String>,
    pub attrs: Vec<String>,
    pub q: Option<antares_ql::QNode>,
    pub geo: Option<antares_ql::geo::GeoQuery>,
}

impl ReadMembers {
    pub(crate) fn with_ctx(
        st: &AppState,
        params: &HashMap<String, String>,
        ctx: &antares_jsonld::Context,
    ) -> Self {
        if st.policy.is_none() {
            return Self::default();
        }
        let mut types: Vec<String> = Vec::new();
        for t in type_selection(params, ctx).into_iter().flatten().flatten() {
            if !types.contains(&t) {
                types.push(t);
            }
        }
        Self {
            types,
            attrs: attr_list(params, ctx).unwrap_or_default(),
            q: params
                .get("q")
                .and_then(|q| antares_ql::parse_narrowed_q(q).ok()),
            geo: antares_ql::geo::GeoQuery::from_params(params)
                .ok()
                .flatten(),
        }
    }

    /// Same, for a gate taken before the request @context is resolved.
    pub(crate) async fn of(
        st: &AppState,
        headers: &HeaderMap,
        params: &HashMap<String, String>,
    ) -> Self {
        if st.policy.is_none() {
            return Self::default();
        }
        match crate::negotiate::request_context_quiet(&st.loader, headers).await {
            Ok(ctx) => Self::with_ctx(st, params, &ctx),
            Err(_) => Self::default(),
        }
    }
}

/// What the store may be asked to do beyond filtering; see
/// [`filter_entities_paged`].
#[derive(Debug, Default, Clone, Copy)]
pub struct Pushdown<'a> {
    pub page: Option<(usize, usize)>,
    pub proj: Option<&'a crate::repr::Repr>,
    /// The expanded Attributes an `orderBy` reads: an ordered read sorts
    /// the whole match set, and these alone are what the sort needs
    /// ([`Filtered::trimmed`]).
    pub order_keys: Option<&'a [String]>,
    /// `params`' own `q`, parsed already by the caller; `None` parses it
    /// again (T-6610 R1-017: a query parsed its narrowed `q` twice).
    pub q: Option<&'a antares_ql::QNode>,
}

/// The full filtering path (5.7.2). `push.page` = (offset, limit) to push into the
/// store — pass it ONLY when every filter the store cannot see is absent
/// (idPattern, federation, orderBy); the store still refuses unless its own
/// predicates compiled exactly. `push.proj` = the parsed representation,
/// offered for projection pushdown (pick/omit/attrs top-level heads) under
/// the same exactness gate. `narrow` = the caller's policy narrowing, which
/// a linked q term reads its targets through (ADR-0020, T-3474).
pub async fn filter_entities_paged(
    st: &AppState,
    tenant: &TenantId,
    params: &HashMap<String, String>,
    ctx: &antares_jsonld::Context,
    fed: Vec<(bool, Value)>,
    push: Pushdown<'_>,
    narrow: Option<&crate::repr::Repr>,
) -> ApiResult<Filtered> {
    filter_rows(st, tenant, params, ctx, Rows::Docs(fed), push, narrow).await
}

/// [`filter_entities_paged`] with each match written by `write` as it is
/// admitted (`Filtered::written`) wherever the store answers in one read:
/// the store hands its rows over as JSON text, each is parsed while it is
/// decided and written at once, so the page is held as its answer instead of
/// ~5 KB `Value` trees an Entity (T-6055). A walk in store pages keeps
/// `docs`.
pub(crate) async fn filter_entities_written(
    st: &AppState,
    tenant: &TenantId,
    params: &HashMap<String, String>,
    ctx: &antares_jsonld::Context,
    push: Pushdown<'_>,
    narrow: Option<&crate::repr::Repr>,
    write: Write<'_>,
) -> ApiResult<Filtered> {
    filter_rows(st, tenant, params, ctx, Rows::Written(write), push, narrow).await
}

/// How [`filter_rows`] holds its matches: documents, beside the federated
/// candidates merged into them, or the local rows as written.
enum Rows<'a> {
    Docs(Vec<(bool, Value)>),
    Written(Write<'a>),
}

async fn filter_rows(
    st: &AppState,
    tenant: &TenantId,
    params: &HashMap<String, String>,
    ctx: &antares_jsonld::Context,
    source: Rows<'_>,
    push: Pushdown<'_>,
    narrow: Option<&crate::repr::Repr>,
) -> ApiResult<Filtered> {
    let (fed, write) = match source {
        Rows::Docs(fed) => (fed, None),
        Rows::Written(write) => (Vec::new(), Some(write)),
    };
    let Pushdown {
        page,
        proj,
        order_keys,
        q: parsed_q,
    } = push;
    // a keyset cursor (T-4907): the store seeks past its id, so the rows
    // already start at the client's offset and every page below is counted
    // from there
    let after = crate::paging::cursor_after(&crate::paging::view_of(st), tenant.as_str(), params)?;
    let page = page.map(|(offset, limit)| (if after.is_some() { 0 } else { offset }, limit));
    // a pushed page over local rows cannot be merged with federated
    // candidates — refuse here so no caller can create that page
    let page = if fed.is_empty() { page } else { None };
    // and neither can a pushed projection: the store's answer is only the
    // final answer when nothing downstream still needs the stripped members
    let proj = proj.filter(|_| proj_pushdown_allowed(fed.is_empty(), params));
    // ADR-0023 rule 11: under rules the store answers the policy's own
    // predicates over the stored Entity, and the loop below decides which
    // rules hold and evaluates the client's predicates on the Entity as it
    // is shown, so the client's page is cut in the store only where the
    // store can judge the rules itself (`judged` below).
    let rules = narrow.and_then(|r| r.policy_types.as_deref());
    let client_page = page;
    let page = if rules.is_some() { None } else { page };
    let proj = if rules.is_some() { None } else { proj };
    let ids: Option<Vec<&str>> = params.get("id").map(|s| s.split(',').collect());
    if let Some(ids) = &ids {
        for id in ids {
            antares_model::EntityId::new(id)?;
        }
    }
    let id_pattern = match params.get("idPattern") {
        Some(p) => {
            if ["**", "++", "*+", "+*"].iter().any(|q| p.contains(q)) {
                return Err(NgsiError::BadRequestData(format!("invalid idPattern {p:?}")).into());
            }
            Some(
                antares_ql::regex::compile(p)
                    .map_err(|_| NgsiError::BadRequestData(format!("invalid idPattern {p:?}")))?,
            )
        }
        None => None,
    };
    let type_sel = type_selection(params, ctx);
    let attr_filter = attr_list(params, ctx);
    let q_ast = match params.get("q") {
        // 4.9 expandValues: "attributes whose values should be expanded
        // against the supplied @context using JSON-LD type coercion prior to
        // executing the query" (EXAMPLE 12), less the Attributes jsonKeys
        // declares uninterpretable as JSON-LD.
        // the query may be narrowed: every caller parsed its own `q` with
        // `parse_q` before (threat V23)
        Some(q) => Some(antares_ql::eval::apply_expand_values(
            match parsed_q {
                Some(node) => node.clone(),
                None => antares_ql::parse_narrowed_q(q)?,
            },
            antares_ql::eval::expansion_list(
                params.get("expandValues").map(String::as_str),
                params.get("jsonKeys").map(String::as_str),
            )
            .as_deref(),
            ctx,
        )),
        None => None,
    };
    let scope_q = params.get("scopeQ");
    // The request's own geoquery (7.2.5 allows one) and the narrowing's
    // area: every one must hold (ADR-0023 rule 1), so a caller's area
    // cannot widen the policy's.
    let own_geo = antares_ql::geo::GeoQuery::from_params(params)?;
    let policy_geo = narrow
        .and_then(|r| r.policy_reach.as_deref())
        .map(|reach| reach.areas.as_slice())
        .unwrap_or_default();
    let geos: Vec<&antares_ql::geo::GeoQuery> = own_geo.iter().chain(policy_geo).collect();
    // under rules the client's q, attrs and geoQ read the shown Entity, and
    // only the engine's own q and area reach the store and the stored one
    let client_q = narrow.and_then(|r| r.policy_client_q.as_ref());
    let engine_q = narrow
        .and_then(|r| r.policy_reach.as_deref())
        .and_then(|reach| reach.q.as_ref());
    let client_and_engine_q = q_ast.clone();
    let (q_ast, shown_q) = if rules.is_some() {
        (engine_q.cloned(), client_q)
    } else {
        (q_ast, None)
    };
    let (geos, shown_geos): (Vec<_>, Vec<_>) = if rules.is_some() {
        (policy_geo.iter().collect(), own_geo.iter().collect())
    } else {
        (geos, Vec::new())
    };
    let (attr_filter, shown_attrs) = if rules.is_some() {
        (None, attr_filter)
    } else {
        (attr_filter, None)
    };
    let reads_shown = shown_attrs.is_some() || !shown_geos.is_empty() || shown_q.is_some();
    // Rules that select by type alone admit exactly the Entities of their
    // types. When the client reads nothing on the shown Entity either, the
    // store takes those types as one more selection, and pages and counts
    // as at the root (T-3667: a page of one read every Entity the policy
    // admits).
    // So too rules that narrow their types only by one set of `idPattern`s
    // the store applies exactly (T-5826): it takes them as a selection too.
    let rule_patterns = rules.and_then(|f| crate::policy::shared_id_patterns(&f.rules));
    let by_type = rules
        .filter(|f| {
            rule_patterns.is_some()
                || f.rules
                    .iter()
                    .all(crate::policy::Rule::admits_by_type_alone)
        })
        .and_then(|f| Some((f, judged_types(type_sel.as_deref(), f)?)));
    // So too when the client's `q` reads only Attributes that every rule of
    // the types it selects shows: on those, the Entity as stored and as shown
    // answer it alike (T-4289: `count=true` with a `q`, which every OGC and
    // OData read sends, read every Entity the policy admits and hit the
    // store's TooManyResults ceiling).
    // So too the client's geoquery on the default GeoProperty when every rule
    // of those types shows it: the store's spatial pushdown answers it as the
    // shown Entity would (T-5062: an OGC bbox, an OData or MCP geo read under
    // a view read every Entity of the type in doubling pages).
    let geos_alike = shown_attrs.is_none()
        && by_type.as_ref().is_some_and(|(f, groups)| {
            shown_geos.iter().all(|g| {
                let name = Some(g.geoproperty.as_str()).filter(|n| !n.is_empty());
                g.to_sql_spec(ctx).is_some()
                    && shows_alike(
                        [name.unwrap_or("location")],
                        f,
                        groups,
                        narrow,
                        &|t: &str| ctx.expand_key(t),
                    )
            })
        });
    let q_alike = geos_alike
        && by_type
            .as_ref()
            .zip(shown_q)
            .is_some_and(|((f, groups), q)| {
                reads_only_shown(q, f, groups, narrow, &|t: &str| ctx.expand_key(t))
            });
    let judged = by_type
        .filter(|_| !reads_shown || (geos_alike && (shown_q.is_none() || q_alike)))
        .map(|(_, groups)| groups);
    let (geos, shown_geos) = if judged.is_some() && geos_alike {
        (geos.into_iter().chain(shown_geos).collect(), Vec::new())
    } else {
        (geos, shown_geos)
    };
    let reads_shown = shown_attrs.is_some() || !shown_geos.is_empty() || shown_q.is_some();
    // the client's own `q` (with the engine's, as the parameters carry them)
    // reaches the store only where it reads the shown Entity alike
    let q_ast = if judged.is_some() && q_alike {
        client_and_engine_q
    } else {
        q_ast
    };
    let page = if judged.is_some() { client_page } else { page };
    // An ordered read sorts every match, so the store keeps only what the
    // sort reads (T-4919): only where nothing below reads more of the stored
    // Entity, that is no rules or rules that select by type alone (a client
    // `q` they leave to the store is decided there), and no federated rows.
    let order_keys = order_keys.filter(|_| fed.is_empty() && (rules.is_none() || judged.is_some()));
    // Otherwise, without `count` the answer needs the matches up to the
    // page's end and one more (whether a next page exists): the store is
    // read in pages of its own, doubling, until that many held.
    // `count=true` under conditional rules needs every match counted: the
    // store is read in pages of its own to the end, only the client's window
    // is kept, and the scan stops at MAX_FOLD_DOCS (T-4516). One flat read
    // refused a dataset's count once its type held MAX_UNDECIDED_ROWS.
    // ponytail: still reads every Entity the policy admits; compile the
    // rules' OR and the shown-set predicates to SQL if that shows up
    let counting = client_page.filter(|_| {
        rules.is_some()
            && judged.is_none()
            && params.get("count").map(String::as_str) == Some("true")
    });
    // So too a geoquery or a scopeQ the store narrows but cannot decide: it
    // pages the candidates (`query_candidates`) and the loop below decides
    // them, where one flat read took up to MAX_UNDECIDED_ROWS for a page of
    // 20 (T-5088: ~1 000 rows, 0.6-1.1 s a statement).
    let undecidable = !geos.is_empty() || scope_q.is_some();
    let enough = match client_page {
        Some((offset, limit))
            if ((rules.is_some() && judged.is_none()) || undecidable)
                && params.get("count").map(String::as_str) != Some("true") =>
        {
            offset.checked_add(limit).and_then(|n| n.checked_add(1))
        }
        _ => None,
    };

    // Hand the store what it can filter on. A backend that can push
    // the predicate down (postgres/timescale) returns fewer rows — and says
    // via `decided` whether it applied EVERY present predicate exactly, which
    // is what licenses pagination/projection pushdown and lets the loop below
    // skip re-deciding. A backend that cannot (memory/file) returns the
    // snapshot and the loop stays the arbiter.
    let expand = |t: &str| ctx.expand_key(t);
    let geo_specs: Vec<_> = geos.iter().filter_map(|g| g.to_sql_spec(ctx)).collect();
    // A geo query whose spec declined to compile (non-default geoproperty) is
    // INVISIBLE to the store — the store would truthfully claim `decided`
    // about what it saw, projection would strip the very member the evaluator
    // still needs, and a pushed page would page over the wrong set. Forfeit
    // every pushdown up front and mask `decided` after. The specs that did
    // compile still narrow the rows.
    let geo_uncompiled = geo_specs.len() < geos.len();
    let page = if geo_uncompiled { None } else { page };
    let enough = enough.filter(|_| !geo_uncompiled);
    let counting = counting.filter(|_| !geo_uncompiled);
    let proj = proj.filter(|_| !geo_uncompiled);
    let order_keys = order_keys.filter(|_| !geo_uncompiled);
    // pick (or attrs) heads to keep / whole-attr omit heads to drop; core
    // members are never SQL-dropped (only `://` IRIs qualify) — repr::apply
    // stays the decider for those.
    let keep_attrs: Option<Vec<String>> = proj
        .and_then(|r| {
            r.pick
                .as_ref()
                .map(|nodes| nodes.iter().map(|n| n.iri.clone()).collect())
                .or_else(|| r.attrs.clone())
        })
        .or_else(|| order_keys.map(<[String]>::to_vec));
    let drop_attrs: Option<Vec<String>> = proj
        .and_then(|r| {
            r.omit.as_ref().map(|nodes| {
                nodes
                    .iter()
                    .filter(|n| n.children.is_none() && n.iri.contains("://"))
                    .map(|n| n.iri.clone())
                    .collect::<Vec<_>>()
            })
        })
        .filter(|v| !v.is_empty());
    // 5.7.2.4 split entities (p.202): the filters (q, geoquery, Scope query,
    // Attributes) apply only AFTER remote parts and local information have
    // been aggregated — so with federated candidates present the store must
    // not drop (or pre-project) the LOCAL half of a split entity. The
    // post-merge loop below applies them instead (`decided` is already
    // false whenever `fed` is non-empty).
    let split_agg = crate::federation::split_entities(params) && !fed.is_empty();
    // the id list is client-sized (a POST query body carries an array with no
    // count of its own) and the candidate set is store-sized, so the two are
    // never multiplied together
    let id_set: Option<std::collections::HashSet<&str>> =
        ids.as_ref().map(|v| v.iter().copied().collect());
    let mut out = Vec::new();
    let mut out_written = Vec::new();
    let (mut paged, mut total): (bool, Option<usize>);
    let mut trimmed: bool;
    let mut rows = 0usize;
    let mut fed_once = Some(fed);
    // the store page under `enough` or `counting`: (offset, limit), the limit
    // doubling up to COUNT_CHUNK. Under `enough` only the client's window is
    // kept, as under `counting`: holding every match up to the page's end made
    // a deep page of a large dataset hold the whole dataset, read in one
    // statement, and four of them took the broker past its limit (T-4562).
    const COUNT_CHUNK: usize = 2048;
    let window = client_page.filter(|_| enough.is_some());
    let mut read = (
        0usize,
        enough
            .map(|n| n.min(COUNT_CHUNK))
            .unwrap_or(if counting.is_some() { 1024 } else { 0 }),
    );
    let mut matched = 0usize;
    // the walk's keyset (T-4683): each store page starts after the last id
    // the one before read, where a growing OFFSET re-read every row it
    // skipped (T-5826: 80 pages over 164 000 Trees)
    let mut walk_after: Option<String> = None;
    // Rules the loop below decides row by row, each narrowing by one
    // `idPattern`, hand the store the id prefixes those patterns force: a
    // dataset view of 70 KPIs walked the Tenant's 128 678 and timed out at
    // the gateway, even for `limit=1` (T-5787). The rules still decide.
    // Rules the store decides by their patterns keep the ranges too: the
    // regex filters only the rows the prefixes' index range found.
    let rule_prefixes =
        match rules.filter(|_| (judged.is_none() || rule_patterns.is_some()) && ids.is_none()) {
            Some(f) => crate::policy::rules_id_prefixes(st, tenant, &f.rules).await?,
            None => None,
        };
    'scan: loop {
        let f = antares_store::filter::EntityFilter {
            ids: ids.as_deref(),
            // 5.2.33: id takes precedence over idPattern, so the literal
            // narrows only when no id selector was given
            id_literal: if ids.is_none() {
                params
                    .get("idPattern")
                    .and_then(|p| antares_store::filter::id_pattern_literal(p))
            } else {
                None
            },
            id_prefixes: rule_prefixes.as_deref(),
            id_patterns: rule_patterns,
            types: judged.as_deref().or(type_sel.as_deref()),
            attrs: if split_agg {
                None
            } else {
                attr_filter.as_deref()
            },
            q: if split_agg { None } else { q_ast.as_ref() },
            scope_q: if split_agg {
                None
            } else {
                scope_q.map(String::as_str)
            },
            geo: if split_agg { &[] } else { &geo_specs },
            expand: &expand,
            ctx: Some(ctx),
            page: match enough.or(counting.map(|_| 0)) {
                Some(_) => Some(store_page(read)),
                None => page.map(|(offset, limit)| antares_store::filter::Page {
                    offset: offset as i64,
                    limit: limit as i64,
                    count: params.get("count").map(String::as_str) == Some("true"),
                }),
            },
            keep_attrs: keep_attrs.as_deref(),
            drop_attrs: drop_attrs.as_deref(),
            after_id: walk_after.as_deref().or(after.as_deref()),
        };
        // a walk in pages decides each row itself, so the store may page
        // what it cannot decide
        let (outcome, texts) = match enough.or(counting.map(|_| 0)) {
            Some(_) => (st.store.query_candidates(tenant, &f).await?, Vec::new()),
            None if write.is_some() => {
                let o = st.store.query_entities_text(tenant, &f).await?;
                let outcome = antares_store::filter::QueryOutcome {
                    rows: Vec::new(),
                    decided: o.decided,
                    paged: o.paged,
                    total: o.total,
                };
                (outcome, o.rows)
            }
            None => (st.store.query_entities(tenant, &f).await?, Vec::new()),
        };
        let fed = fed_once.take().unwrap_or_default();
        let store_paged = outcome.paged;
        let decided = outcome.decided && fed.is_empty() && !geo_uncompiled;
        trimmed = order_keys.is_some() && decided;
        paged = store_paged && fed.is_empty() && enough.is_none();
        total = outcome.total.map(|t| t as usize);
        let got = outcome.rows.len().saturating_add(texts.len());
        rows = rows.saturating_add(got);
        walk_after = outcome
            .rows
            .last()
            .and_then(|d| d.get("id")?.as_str())
            .map(str::to_owned);
        // a row the store hands over as text is parsed once, to be decided and written
        let all = crate::federation::merge_candidates(outcome.rows, fed)
            .into_iter()
            .map(|d| Ok::<_, serde_json::Error>((d, None)))
            .chain(
                texts
                    .into_iter()
                    .map(|r| serde_json::from_str::<Value>(r.get()).map(|d| (d, write))),
            );
        for item in all {
            let (doc, write) = item.map_err(|e| NgsiError::InternalError(e.to_string()))?;
            let id = doc.get("id").and_then(Value::as_str).unwrap_or("");
            if let Some(ids) = &id_set {
                if !decided && !ids.contains(id) {
                    continue;
                }
            }
            // 5.2.33: "id takes precedence over idPattern" — the pattern only
            // filters when no id selector was given.
            if ids.is_none() {
                if let Some(re) = &id_pattern {
                    // idPattern is invisible to the store — applied even when
                    // decided
                    if !re.is_match(id) {
                        continue;
                    }
                }
            }
            if !decided {
                if let Some(sel) = &type_sel {
                    let etypes: Vec<&str> = doc
                        .get("type")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();
                    let matched = sel
                        .iter()
                        .any(|and_group| and_group.iter().all(|w| etypes.contains(&w.as_str())));
                    if !matched {
                        continue;
                    }
                }
                if let Some(attrs) = &attr_filter {
                    if !attrs.iter().any(|a| doc.get(a).is_some()) {
                        continue;
                    }
                }
                if let Some(ast) = &q_ast {
                    // 4.9 linked-entity subqueries (attr{path}) resolve through
                    // the local store, same tenant, under the caller's narrowing
                    let narrow = narrow.map(|r| (r, ctx));
                    if !crate::notify::linked_eval(st, tenant, narrow, |l| {
                        eval_q(ast, &doc, ctx, l)
                    })
                    .await
                    {
                        continue;
                    }
                }
                if let Some(sq) = scope_q {
                    if !crate::scope_matches(sq, &doc) {
                        continue;
                    }
                }
                if !geos.iter().all(|g| g.matches(&doc, ctx)) {
                    continue;
                }
            }
            if let (Some(rules), Some(r)) = (rules, narrow) {
                if !rules.admits(&doc, crate::repr::policy_context()) {
                    continue;
                }
                // the Entity as shown is built only for a client predicate
                // that reads it
                if reads_shown {
                    let view = crate::repr::policy_view(&doc, r);
                    if let Some(attrs) = &shown_attrs {
                        if !attrs.iter().any(|a| view.get(a).is_some()) {
                            continue;
                        }
                    }
                    if !shown_geos.iter().all(|g| g.matches(&view, ctx)) {
                        continue;
                    }
                    // a `q` the store took because it reads the shown Entity
                    // alike is decided there
                    if let Some(ast) = shown_q.filter(|_| !(q_alike && decided)) {
                        let narrow = narrow.map(|r| (r, ctx));
                        if !crate::notify::linked_eval(st, tenant, narrow, |l| {
                            eval_q(ast, &view, ctx, l)
                        })
                        .await
                        {
                            continue;
                        }
                    }
                }
            }
            if let Some((offset, limit)) = counting {
                // ORDER BY id is the store's order, so the k-th match is the
                // k-th Entity of the client's page
                if matched.checked_sub(offset).is_some_and(|k| k < limit) {
                    out.push(doc);
                }
                matched = matched.saturating_add(1);
                continue;
            }
            if let Some((offset, limit)) = window {
                // the window and one match past it, which says a next page exists
                if matched >= offset {
                    out.push(doc);
                }
                matched = matched.saturating_add(1);
                if out.len() > limit {
                    break 'scan;
                }
                continue;
            }
            match write {
                Some(write) => out_written.push(Written {
                    id: id.to_owned(),
                    frame: write(&doc),
                }),
                None => out.push(doc),
            }
        }
        // a store that could not page answered with every row it holds
        if (enough.is_none() && counting.is_none()) || !store_paged || got < read.1 {
            break;
        }
        // a walk over candidates is bounded as the flat read it replaces was,
        // a window that never fills (rules admitting nothing) too (F-ANT2-010)
        if rows >= *crate::bounds::MAX_FOLD_DOCS {
            return Err(NgsiError::TooManyResults(format!(
                "this query read more than {} entities; narrow it",
                *crate::bounds::MAX_FOLD_DOCS
            ))
            .into());
        }
        read = (0, read.1.saturating_mul(2).min(COUNT_CHUNK));
    }
    if let Some((offset, limit)) = window {
        // the smallest total consistent with "a next page exists"
        let more = out.len() > limit;
        out.truncate(limit);
        return Ok(Filtered {
            total: Some(
                offset
                    .saturating_add(out.len())
                    .saturating_add(usize::from(more)),
            ),
            docs: out,
            paged: true,
            rows,
            trimmed,
            written: Vec::new(),
        });
    }
    if counting.is_some() {
        // the store paged to the end, or answered every row it holds at once
        return Ok(Filtered {
            docs: out,
            paged: true,
            total: Some(matched),
            rows,
            trimmed,
            written: Vec::new(),
        });
    }
    Ok(Filtered {
        docs: out,
        paged,
        total,
        rows,
        trimmed,
        written: out_written,
    })
}

// ---------- DELETE /entities/{id} (5.6.6) ----------

pub async fn delete_entity(
    State(st): State<AppState>,
    Path(id): Path<String>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
) -> Response {
    let go = async {
        let tenant = tenant_from(&headers)?;
        antares_model::EntityId::new(&id)?;
        check_params(&params, &["local", "type"])?;
        // 5.5.7: `type` is a term, so the request's own @context expands it.
        // Under the core context the same word names a different type than
        // the client meant, and the delete then removes an Entity the
        // client's selector excluded.
        let ctx = request_context(&st.loader, &headers).await?;
        let pre = crate::policy::pre_image(&st, &tenant, &id, None).await?;
        let granted = gate!(
            st, &tenant, &headers, "5.6.6",
            ids: &[&id],
            types: &pre.types,
            stored: pre.stored.as_ref(),
        )
        .await?;
        let cond = crate::policy::Condition::of(&granted);
        // 4.17/5.6.6.4: the type selector gates the target — a registration
        // for a different type must not receive the forwarded delete.
        let spec = crate::registry::CsrSpec {
            ids: Some(vec![id.clone()]),
            types: params
                .get("type")
                .map(|s| s.split(',').map(|t| ctx.expand_key(t.trim())).collect()),
            ..Default::default()
        };
        let regs = match crate::federation::write_plan(&st, &tenant, &spec, &ctx, &params, &headers)
            .await?
        {
            crate::federation::WritePlan::Answered(r) => return Ok(*r),
            crate::federation::WritePlan::Forward(regs) => regs,
        };
        crate::policy::Condition::unforwarded(cond.as_ref(), !regs.is_empty())?;
        // 5.6.6.4: the ?type selector narrows the target — an entity of a
        // non-matching type is "not known" for this delete. The selector is
        // tested inside the delete, under the row lock: read first and delete
        // after, and the document that answered the test is not necessarily
        // the one the delete removes.
        #[cfg(feature = "views")]
        let guard =
            crate::views::WriteGuard::of(&st, std::iter::empty()).map(|g| g.matching(&headers));
        let keep = |d: &Value| {
            // gap G12: an Entity the engine's condition does not select is
            // not found
            if cond.as_ref().is_some_and(|c| !c.holds(d)) {
                return false;
            }
            // ADR-0023 rule 7: an Entity out of the view is not found
            #[cfg(feature = "views")]
            if guard.as_ref().is_some_and(|g| !g.keeps(d)) {
                return false;
            }
            crate::negotiate::matches_type_param(d, &params, &ctx)
        };
        if !regs.is_empty() {
            let proxy_match = regs.iter().any(|r| r.is_proxy());
            let mut parts = Vec::new();
            if st.store.delete_entity_if(&tenant, &id, &keep).await? {
                mirror_delete_entity(&st, &tenant, &id).await;
                parts.push(crate::federation::Part::local(204, "deleted locally"));
            } else if !proxy_match {
                // nothing local to delete, and no proxy that owns it: the
                // local half of this operation is the 404
                parts.push(crate::federation::Part::local(
                    404,
                    format!("entity {id} not found locally"),
                ));
            }
            let ctx_url = crate::federation::ctx_link_url(&headers, &ctx.source);
            let seg = crate::federation::path_segment(&id);
            let fwd_q = type_selector_query(&params);
            for reg in &regs {
                // 5.6.6.4: proxy modes not supporting Delete Entity are an
                // error of type Conflict; inclusive ones are not forwarded.
                if !reg.supports("deleteEntity") {
                    if reg.is_proxy() {
                        parts.push(crate::federation::conflict_part("deleteEntity", reg));
                    }
                    continue;
                }
                parts.push(
                    crate::federation::forward_part(
                        &st,
                        reqwest::Method::DELETE,
                        format!("{}/ngsi-ld/v1/entities/{seg}", reg.endpoint),
                        &fwd_q,
                        &headers,
                        &tenant,
                        reg,
                        &ctx_url,
                        None,
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
        if st.store.delete_entity_if(&tenant, &id, &keep).await? {
            mirror_delete_entity(&st, &tenant, &id).await;
            Ok::<_, ApiError>(no_content(&tenant))
        } else {
            #[cfg(feature = "views")]
            if let Some(g) = &guard {
                g.settled()?;
            }
            Err(NgsiError::ResourceNotFound(format!("entity {id} not found")).into())
        }
    };
    go.await.unwrap_or_else(|e| e.into_response())
}

// ---------- DELETE /entities/ — Purge (5.6.21) ----------

/// 5.6.21.3 Input data of a Purge: the Entity type selector, the identifier
/// list and id pattern, the restrictive and exclusionary Attribute-name
/// lists, the NGSI-LD Query, the GeoQuery, the Scope query and the context
/// source filter. 5.6.21.4 forwards "matching input data ... to the
/// Registration endpoint", so every one of them travels: a forward that
/// carries fewer restrictions than the client issued makes the peer execute
/// a strictly wider purge than the one requested. `local` is absent by
/// design — it selects local scope, which is what stops the forward
/// happening at all (5.5.13).
const PURGE_FORWARD_PARAMS: &[&str] = &[
    "type",
    "id",
    "idPattern",
    "attrs",
    "q",
    "georel",
    "geometry",
    "coordinates",
    "geoproperty",
    "scopeQ",
    "csf",
    "keep",
    "drop",
];

fn forwarded_purge_query(params: &HashMap<String, String>) -> Vec<(String, String)> {
    PURGE_FORWARD_PARAMS
        .iter()
        .filter_map(|k| params.get(*k).map(|v| ((*k).to_owned(), v.clone())))
        .collect()
}

/// How many matched Entities one round of a Purge fetches and applies. The
/// match set of 5.6.21.4 is deliberately unbounded — `DELETE /entities?type=T`
/// matches every Entity of that type — so it is walked page by page and
/// applied in batches rather than materialized whole.
const PURGE_CHUNK: usize = 500;

/// Where the next chunk of a walked match set starts, or None when the set is
/// exhausted. `rows` = documents the store returned for this chunk (not the
/// subset that survived the evaluator: idPattern is applied after the store,
/// so a full chunk can arrive narrowed or even empty and the walk must still
/// go on — 5.6.21.4 deletes ALL matched Entities, not the first page of
/// them). `left_the_set` = how many of those rows no longer match the query
/// afterwards: a Purge deletes or prunes them, a read leaves every row where
/// it was. The rows still in the set have to be stepped over, or the walk
/// re-reads them forever.
///
/// Termination: every round either removes rows from the match set or
/// advances the offset by a full chunk, and both are bounded by the number of
/// stored Entities.
pub(crate) fn next_scan_offset(
    offset: usize,
    rows: usize,
    left_the_set: usize,
    paged: bool,
    chunk: usize,
) -> Option<usize> {
    // an unpaged answer IS the whole match set, which this round just applied
    if !paged || rows < chunk {
        return None;
    }
    Some(offset.saturating_add(rows.saturating_sub(left_the_set)))
}

/// 5.6.21.4 "And thereafter": with no Attribute-name list, delete every
/// matched Entity found locally; with a restrictive list, delete those
/// Attributes from them; with an exclusionary list, delete all but those.
async fn purge_locally(
    st: &AppState,
    tenant: &TenantId,
    params: &HashMap<String, String>,
    ctx: &antares_jsonld::Context,
    keep: &Option<Vec<String>>,
    drop: &Option<Vec<String>>,
) -> ApiResult<()> {
    let prune = keep.is_some() || drop.is_some();
    let mut offset = 0usize;
    loop {
        let batch = filter_entities_paged(
            st,
            tenant,
            params,
            ctx,
            Vec::new(),
            Pushdown {
                page: Some((offset, PURGE_CHUNK)),
                proj: None,
                order_keys: None,
                q: None,
            },
            // 5.6.21.4 refuses linked q terms before a purge runs
            None,
        )
        .await?;
        let rows = batch.rows;
        let ids: Vec<String> = batch
            .docs
            .iter()
            .filter_map(|d| d["id"].as_str().map(str::to_owned))
            .collect();
        let left_the_set = if ids.is_empty() {
            0
        } else if prune {
            let mut changed = 0usize;
            st.store
                .batch_mutate(tenant, &ids, |_, doc| {
                    let target = antares_store::stored_object(doc)?;
                    let attrs: Vec<String> =
                        target.keys().filter(|k| !is_meta(k)).cloned().collect();
                    let before = target.len();
                    for a in attrs {
                        let purge = match (keep, drop) {
                            (Some(keep), _) => !keep.contains(&a),
                            (_, Some(drop)) => drop.contains(&a),
                            _ => true,
                        };
                        if purge {
                            target.remove(&a);
                        }
                    }
                    if target.len() != before {
                        changed = changed.saturating_add(1);
                    }
                    Ok::<(), NgsiError>(())
                })
                .await?;
            // A prune keeps the Entity, but it may have removed the very
            // Attribute the query matched on (`attrs=speed&drop=speed`), which
            // takes it out of the match set and shifts the rest down. Whether
            // it did is only observable by reading the window again, so a round
            // that changed something re-reads it; the re-read finds those
            // Entities unchanged and steps over them.
            if changed > 0 {
                rows
            } else {
                0
            }
        } else {
            let mut gone = 0usize;
            for (id, deleted) in ids.iter().zip(st.store.batch_delete(tenant, &ids).await?) {
                if deleted {
                    gone = gone.saturating_add(1);
                    mirror_delete_entity(st, tenant, id).await;
                }
            }
            gone
        };
        match next_scan_offset(offset, rows, left_the_set, batch.paged, PURGE_CHUNK) {
            Some(next) => offset = next,
            None => return Ok(()),
        }
    }
}

/// 5.6.21 Purge Entities: delete (or keep=/drop=-prune) all entities matched
/// by the query; output data is none — 204 (5.6.21.5). Too-wide queries,
/// Linked Entity paths and invalid id/q/geo/csf are BadRequestData
/// (5.6.21.4); matched registrations forward only with purgeEntity support.
pub async fn purge_entities(
    State(st): State<AppState>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
) -> Response {
    match purge_inner(&st, &params, &headers).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

async fn purge_inner(
    st: &AppState,
    params: &HashMap<String, String>,
    headers: &HeaderMap,
) -> ApiResult<Response> {
    let tenant = tenant_from(headers)?;
    check_params(
        params,
        &[
            "id",
            "idPattern",
            "type",
            "attrs",
            "q",
            "georel",
            "geometry",
            "coordinates",
            "geoproperty",
            "scopeQ",
            "csf",
            "keep",
            "drop",
            "local",
        ],
    )?;
    let ctx = request_context(&st.loader, headers).await?;
    // The engine sees the whole request (F-ANT2-023); 5.6.21 stays one
    // tenant-wide grant (ADR-0020), so nothing narrows the purge itself.
    let named = ReadMembers::with_ctx(st, params, &ctx);
    let ids: Vec<&str> = params
        .get("id")
        .map(|s| s.split(',').map(str::trim).collect())
        .unwrap_or_default();
    gate!(
        st, &tenant, headers, "5.6.21",
        ids: &ids,
        q: named.q.as_ref(),
        scope_q: params.get("scopeQ").map(String::as_str),
        types: &named.types,
        attrs: &named.attrs,
        geo: named.geo.as_ref(),
    )
    .await?;
    // 5.6.21.4: exactly five qualifying conditions —
    //   a) selector of Entity Types
    //   b) list of Attribute names, including at least one non-system Attribute
    //   c) NGSI-LD Query, including at least one non-system Attribute
    //   d) NGSI-LD GeoQuery
    //   e) local scope (5.5.13)
    // "If none of the above is provided, then an error of type BadRequestData
    // shall be raised (too wide query)."
    //
    // id/idPattern are legal input data (5.6.21.3) and DO filter, but they are
    // never sufficient on their own: "it is not possible to purge a set of
    // entities by only specifying desired Entity identifiers". Listing them
    // here is how `DELETE /entities?idPattern=.*` became a tenant wipe.
    let attrs_qualify = params.get("attrs").is_some_and(|a| {
        a.split(',')
            .any(|n| antares_ql::is_non_system_attr(n.trim()))
    });
    let q_ast = params.get("q").map(|q| parse_q(q)).transpose()?;
    let q_qualifies = q_ast.as_ref().is_some_and(|ast| {
        ast.attribute_paths()
            .iter()
            .any(|h| antares_ql::is_non_system_attr(h))
    });
    // 5.6.21.4: Linked Entity retrieval in the projection attributes, or
    // Linked Entity attributes in the filter conditions → BadRequestData.
    if q_ast
        .as_ref()
        .is_some_and(antares_ql::QNode::has_linked_paths)
    {
        return Err(NgsiError::BadRequestData(
            "purge q must not reference Linked Entity attributes (5.6.21.4)".into(),
        )
        .into());
    }
    if params.get("attrs").is_some_and(|a| a.contains('{')) {
        return Err(NgsiError::BadRequestData(
            "purge attrs must not use Linked Entity retrieval (5.6.21.4)".into(),
        )
        .into());
    }
    // 5.6.21.4: a syntactically invalid context source filter is
    // BadRequestData.
    if let Some(csf) = params.get("csf") {
        parse_q(csf)?;
    }
    let has_filter = params.contains_key("type")
        || attrs_qualify
        || q_qualifies
        || params.contains_key("georel")
        || params.get("local").map(String::as_str) == Some("true");
    if !has_filter {
        return Err(NgsiError::BadRequestData(
            "purge needs at least one of: type, attrs or q naming a non-system \
             Attribute, georel, or local=true (5.6.21.4 — too wide query)"
                .into(),
        )
        .into());
    }
    if params.contains_key("keep") && params.contains_key("drop") {
        return Err(NgsiError::BadRequestData(
            "keep and drop are mutually exclusive (5.6.21)".into(),
        )
        .into());
    }
    let keep: Option<Vec<String>> = params
        .get("keep")
        .map(|s| s.split(',').map(|t| ctx.expand_key(t.trim())).collect());
    let drop: Option<Vec<String>> = params
        .get("drop")
        .map(|s| s.split(',').map(|t| ctx.expand_key(t.trim())).collect());
    // distributed purge (5.6.21 / 6.4.3.3). Matching and the 6.3.17/6.3.18
    // loop check come first: 508 Loop Detected is an error status, so the
    // request it answers must not have deleted a page of Entities already.
    let spec = crate::registry::CsrSpec {
        types: params
            .get("type")
            .map(|s| s.split(',').map(|t| ctx.expand_key(t.trim())).collect()),
        ids: params
            .get("id")
            .map(|s| s.split(',').map(str::to_owned).collect()),
        // 5.12: the purge's idPattern is part of the Entity specification too
        id_pattern: params.get("idPattern").cloned(),
        csf: params.get("csf").and_then(|c| antares_ql::parse_q(c).ok()),
        ..Default::default()
    };
    let regs =
        match crate::federation::write_plan(st, &tenant, &spec, &ctx, params, headers).await? {
            crate::federation::WritePlan::Answered(r) => return Ok(*r),
            crate::federation::WritePlan::Forward(regs) => regs,
        };
    purge_locally(st, &tenant, params, &ctx, &keep, &drop).await?;
    if !regs.is_empty() {
        let mut parts = vec![crate::federation::Part::local(204, "purged locally")];
        let ctx_url = crate::federation::ctx_link_url(headers, &ctx.source);
        let query = forwarded_purge_query(params);
        for reg in &regs {
            // 5.6.21.4: matching input data is forwarded only when the
            // registration supports Purge Entity; an unsupported matched
            // registration — any mode — is an error of type Conflict
            // (partial success when other parts succeeded).
            if !reg.supports("purgeEntity") {
                parts.push(crate::federation::conflict_part("purgeEntity", reg));
                continue;
            }
            parts.push(
                crate::federation::forward_part(
                    st,
                    reqwest::Method::DELETE,
                    format!("{}/ngsi-ld/v1/entities", reg.endpoint),
                    &query,
                    headers,
                    &tenant,
                    reg,
                    &ctx_url,
                    None,
                )
                .await,
            );
        }
        return Ok(crate::federation::combine(
            st.profile,
            None,
            parts,
            no_content(&tenant),
            &tenant,
        ));
    }
    Ok(no_content(&tenant))
}

// ---------- PATCH /entities/{id} — Merge (5.6.17 / 5.5.12) ----------

pub async fn merge_entity(
    State(st): State<AppState>,
    Path(id): Path<String>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match merge_entity_inner(&st, &id, &params, &headers, &body).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

async fn merge_entity_inner(
    st: &AppState,
    id: &str,
    params: &HashMap<String, String>,
    headers: &HeaderMap,
    body: &[u8],
) -> ApiResult<Response> {
    let tenant = tenant_from(headers)?;
    antares_model::EntityId::new(id)?;
    check_params(
        params,
        &["options", "format", "observedAt", "lang", "local", "type"],
    )?;
    let parsed = parse_body(st, headers, body, BodyKind::MergePatch).await?;
    let obj = parsed.object(NgsiError::BadRequestData(
        "fragment must be a JSON object".into(),
    ))?;
    if let Some(bid) = obj.get("id").and_then(Value::as_str) {
        if bid != id {
            return Err(NgsiError::BadRequestData("fragment id mismatch".into()).into());
        }
    }
    let mut fragment = expand_entity(
        obj,
        &parsed.ctx,
        ExpandOpts {
            fragment: true,
            allow_null: true,
            merge: true,
            temporal: false,
            ..Default::default()
        },
    )?;
    let pre = crate::policy::pre_image(st, &tenant, id, Some(&fragment)).await?;
    let granted = gate!(
        st, &tenant, headers, "5.6.17",
        ids: &[id],
        types: &pre.types,
        stored: pre.stored.as_ref(),
        attrs: &crate::attrs::attr_iris_of(&fragment),
        body: Some(&fragment),
    )
    .await?;
    #[cfg(feature = "views")]
    crate::views::schema_admits(st, &tenant, id, &fragment, &pre.types).await?;
    let cond = crate::policy::Condition::of(&granted);
    #[cfg(feature = "views")]
    let guard = crate::views::WriteGuard::of(
        st,
        crate::attrs::attr_iris_of(&fragment)
            .iter()
            .map(String::as_str),
    )
    .map(|g| g.matching(headers));
    #[cfg(feature = "views")]
    if let Some(g) = &guard {
        g.targets(st, &tenant, &fragment).await?;
    }
    // 5.6.17.3: a common observedAt timestamp to use across merged
    // Attributes, and a common language tag for merged LanguageMaps.
    let observed_at = params.get("observedAt").map(String::as_str);
    if let Some(t) = observed_at {
        if !antares_jsonld::parse_datetime(t) {
            return Err(
                NgsiError::BadRequestData("observedAt must be a DateTime (4.8)".into()).into(),
            );
        }
    }
    let lang = params.get("lang").map(String::as_str);
    let simplified = simplified_input(params)?;
    apply_common_observed_at(&mut fragment, observed_at);
    let ts = now_iso();

    let spec = crate::registry::CsrSpec {
        ids: Some(vec![id.to_owned()]),
        ..Default::default()
    };
    let regs = match crate::federation::write_plan(st, &tenant, &spec, &parsed.ctx, params, headers)
        .await?
    {
        crate::federation::WritePlan::Answered(r) => return Ok(*r),
        crate::federation::WritePlan::Forward(regs) => regs,
    };
    crate::policy::Condition::unforwarded(cond.as_ref(), !regs.is_empty())?;
    if !regs.is_empty() {
        let proxies: Vec<&crate::federation::FedReg> =
            regs.iter().filter(|r| r.is_proxy()).collect();
        let mut parts = Vec::new();
        let (rest, has_attrs) = crate::federation::strip_proxied(obj, &proxies, &parsed.ctx);
        // 5.6.17.4: the target is "an existing Entity whose id (URI), and
        // where specified type, is equivalent held locally" — the ?type
        // selector narrows it on this path exactly as on the local-only one.
        let local_exists = st
            .store
            .get(&tenant, Kind::Entity, id)
            .await?
            .is_some_and(|d| crate::negotiate::matches_type_param(&d, params, &parsed.ctx));
        if (local_exists || proxies.is_empty()) && has_attrs {
            let mut local_frag = expand_entity(
                &rest,
                &parsed.ctx,
                ExpandOpts {
                    fragment: true,
                    allow_null: true,
                    merge: true,
                    temporal: false,
                    ..Default::default()
                },
            )?;
            apply_common_observed_at(&mut local_frag, observed_at);
            let res = st
                .store
                .mutate(&tenant, Kind::Entity, id, |doc| {
                    crate::history::restage(id);
                    if !crate::negotiate::matches_type_param(doc, params, &parsed.ctx) {
                        return Err(NgsiError::ResourceNotFound(format!(
                            "entity {id} does not match the type selector"
                        )));
                    }
                    let mut frag = local_frag.clone();
                    let raw = KeptTypes {
                        raw: &rest,
                        ctx: &parsed.ctx,
                        simplified,
                        lang,
                        observed_at,
                    };
                    raw.apply(doc, &mut frag)?;
                    apply_common_lang(doc, &mut frag, lang);
                    merge_into(doc, &frag, &ts);
                    Ok::<(), NgsiError>(())
                })
                .await?;
            crate::history::settle(id, matches!(res, Some(Ok(()))));
            parts.push(match res {
                Some(Ok(())) => crate::federation::Part::local(204, "merged locally"),
                _ => crate::federation::Part::local(404, format!("entity {id} not found locally")),
            });
        }
        let ctx_url = crate::federation::ctx_link_url(headers, &parsed.ctx.source);
        let seg = crate::federation::path_segment(id);
        let fwd_q = type_selector_query(params);
        for reg in &regs {
            // 5.6.17.4: proxy modes without Merge Entity support are an
            // error of type Conflict; inclusive ones are not forwarded.
            if !reg.supports("mergeEntity") {
                if reg.is_proxy() {
                    parts.push(crate::federation::conflict_part("mergeEntity", reg));
                }
                continue;
            }
            let Some(frag) = crate::federation::reduce_to_scope(obj, reg, &parsed.ctx) else {
                continue;
            };
            parts.push(
                crate::federation::forward_part(
                    st,
                    reqwest::Method::PATCH,
                    format!("{}/ngsi-ld/v1/entities/{seg}", reg.endpoint),
                    &fwd_q,
                    headers,
                    &tenant,
                    reg,
                    &ctx_url,
                    Some(frag),
                )
                .await,
            );
        }
        return Ok(crate::federation::combine(
            st.profile,
            Some(id),
            parts,
            no_content(&tenant),
            &tenant,
        ));
    }

    let res = st
        .store
        .mutate(&tenant, Kind::Entity, id, |doc| {
            crate::history::restage(id);
            crate::policy::Condition::pre(cond.as_ref(), id, doc)?;
            #[cfg(feature = "views")]
            if let Some(g) = &guard {
                g.pre(id, doc)?;
            }
            // 5.6.17.4: the ?type selector narrows the merge target
            if !crate::negotiate::matches_type_param(doc, params, &parsed.ctx) {
                return Err(NgsiError::ResourceNotFound(format!(
                    "entity {id} does not match the type selector"
                )));
            }
            let mut frag = fragment.clone();
            let raw = KeptTypes {
                raw: obj,
                ctx: &parsed.ctx,
                simplified,
                lang,
                observed_at,
            };
            raw.apply(doc, &mut frag)?;
            apply_common_lang(doc, &mut frag, lang);
            merge_into(doc, &frag, &ts);
            crate::policy::Condition::post(cond.as_ref(), doc)?;
            #[cfg(feature = "views")]
            if let Some(g) = &guard {
                g.post(doc)?;
            }
            Ok::<(), NgsiError>(())
        })
        .await?;
    crate::history::settle(id, matches!(res, Some(Ok(()))));
    crate::policy::Condition::verdict(cond.as_ref())?;
    #[cfg(feature = "views")]
    if let Some(g) = &guard {
        g.settled()?;
    }
    match res {
        None => Err(NgsiError::ResourceNotFound(format!("entity {id} not found")).into()),
        Some(Err(e)) => Err(e.into()),
        Some(Ok(())) => Ok(no_content(&tenant)),
    }
}

/// 5.6.17.3: "An optional parameter indicating a common observedAt timestamp
/// to use across merged Attributes." It applies to the Attribute instances of
/// the Fragment that do not carry an observedAt of their own; a deletion
/// instance (5.5.12 NGSI-LD Null) removes the Attribute and takes none.
fn apply_common_observed_at(fragment: &mut Value, observed_at: Option<&str>) {
    let (Some(ts), Some(obj)) = (observed_at, fragment.as_object_mut()) else {
        return;
    };
    for (k, v) in obj.iter_mut() {
        if is_meta(k) {
            continue;
        }
        let Some(instances) = v.as_array_mut() else {
            continue;
        };
        for inst in instances {
            if antares_jsonld::is_deletion_instance(inst) {
                continue;
            }
            if let Some(o) = inst.as_object_mut() {
                o.entry("observedAt".to_owned())
                    .or_insert_with(|| Value::String(ts.to_owned()));
            }
        }
    }
}

/// 5.6.17.4: "If a common language tag is defined and a LanguageProperty
/// Attribute to be merged is represented as a string, the pre-existing
/// languageMap JSON object shall be preserved. The string value shall only
/// replace the value associated to the language tag key found within the
/// languageMap." The string instance is rewritten into a one-key languageMap
/// patch, which 5.5.12 then merges into the stored map key by key.
fn apply_common_lang(target: &Value, fragment: &mut Value, lang: Option<&str>) {
    let (Some(lang), Some(frag)) = (lang, fragment.as_object_mut()) else {
        return;
    };
    for (k, v) in frag.iter_mut() {
        if is_meta(k) {
            continue;
        }
        let pre_existing_langmap = target
            .get(k)
            .and_then(Value::as_array)
            .is_some_and(|insts| insts.iter().any(|i| i.get("languageMap").is_some()));
        if !pre_existing_langmap {
            continue;
        }
        let Some(instances) = v.as_array_mut() else {
            continue;
        };
        for inst in instances {
            if antares_jsonld::is_deletion_instance(inst) {
                continue;
            }
            let Some(o) = inst.as_object_mut() else {
                continue;
            };
            let Some(s) = o.get("value").and_then(Value::as_str).map(str::to_owned) else {
                continue;
            };
            o.remove("value");
            o.insert("type".into(), Value::String("LanguageProperty".into()));
            let mut map = Map::new();
            map.insert(lang.to_owned(), Value::String(s));
            o.insert("languageMap".into(), Value::Object(map));
        }
    }
}

/// TS 104 176 7.3.3.2 Table 7.3.3.2-1: `format` "shall be one of:
/// "simplified" (or its synonym "keyValues")", and so is the deprecated
/// `options=keyValues` spelling (6.3.7). Either says the Fragment holds the
/// simplified representation of ETSI TS 104 175 clause 5.3.2.4.
fn simplified_input(params: &HashMap<String, String>) -> Result<bool, NgsiError> {
    let by_options = params.get("options").is_some_and(|o| {
        o.split(',')
            .any(|o| matches!(o.trim(), "keyValues" | "simplified"))
    });
    match params.get("format").map(String::as_str) {
        None => Ok(by_options),
        Some("simplified" | "keyValues") => Ok(true),
        Some("normalized" | "concise") => Ok(by_options),
        Some(f) => Err(NgsiError::BadRequestData(format!(
            "format {f:?}: Merge Entity takes only \"simplified\" or \"keyValues\" \
             (TS 104 176 7.3.3.2)"
        ))),
    }
}

/// The Fragment members a Merge Entity reads by the type of the Attribute it
/// lands on, re-expanded inside the store transaction against the stored
/// Entity:
/// - 10.2.9.4: "If the Attribute to be merged is represented in a simplified
///   representation, the type of any pre-existing Attribute in the target
///   entity shall be preserved" (TS 104 176 7.3.3.2: "any attempt to modify
///   the type of an Attribute shall result in a BadRequest error"). A bare
///   value goes into the member the stored type holds it in, and a value
///   that type cannot hold is BadRequestData from the same expansion a
///   normalized write runs.
/// - 4.7.1: a GeoProperty's Value "shall be a GeoJSON Geometry". A type-less
///   `{"value": ...}` onto a stored GeoProperty is validated as one; read as
///   a plain Property it stored `{"type":"Polygon"}` with no coordinates.
struct KeptTypes<'a> {
    raw: &'a Map<String, Value>,
    ctx: &'a antares_jsonld::Context,
    simplified: bool,
    lang: Option<&'a str>,
    observed_at: Option<&'a str>,
}

impl KeptTypes<'_> {
    fn apply(&self, doc: &Value, frag: &mut Value) -> Result<(), NgsiError> {
        let mut kept = Map::new();
        let mut iris = Vec::new();
        for (k, v) in self.raw {
            if k == "@context" || is_meta(k) || is_ngsi_null(v) {
                continue;
            }
            let iri = antares_jsonld::expand_attr_name(k, self.ctx)?;
            let Some(stored) = doc
                .get(&iri)
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(|i| i.get("type"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            let typeless = v
                .as_object()
                .filter(|o| !o.contains_key("type") && !o.contains_key("@type"));
            let normalized = if self.simplified {
                self.simplified_as(&iri, stored, v)?
            } else if let Some(o) =
                typeless.filter(|o| stored == "GeoProperty" && o.contains_key("value"))
            {
                let mut o = o.clone();
                o.insert("type".into(), Value::String(stored.to_owned()));
                Value::Object(o)
            } else {
                continue;
            };
            kept.insert(k.clone(), normalized);
            iris.push(iri);
        }
        if kept.is_empty() {
            return Ok(());
        }
        let mut expanded = expand_entity(
            &kept,
            self.ctx,
            ExpandOpts {
                fragment: true,
                allow_null: true,
                merge: true,
                temporal: false,
                ..Default::default()
            },
        )?;
        apply_common_observed_at(&mut expanded, self.observed_at);
        if let (Some(f), Some(e)) = (frag.as_object_mut(), expanded.as_object_mut()) {
            for iri in iris {
                if let Some(v) = e.remove(&iri) {
                    f.insert(iri, v);
                }
            }
        }
        Ok(())
    }

    /// The normalized Attribute a simplified value of a stored `stored`
    /// Attribute stands for (TS 104 175 5.3.2.4: a LanguageProperty,
    /// JsonProperty or VocabProperty may arrive as `{"languageMap": ...}`,
    /// `{"json": ...}` or `{"vocab": ...}`; every other type as its bare
    /// value). 10.2.9.4: a LanguageProperty sent "as a string" updates the
    /// key of the common language tag, so without `lang` there is no key.
    fn simplified_as(&self, iri: &str, stored: &str, v: &Value) -> Result<Value, NgsiError> {
        let member = match stored {
            "Relationship" => "object",
            "ListProperty" => "valueList",
            "ListRelationship" => "objectList",
            "LanguageProperty" => "languageMap",
            "VocabProperty" => "vocab",
            "JsonProperty" => "json",
            _ => "value",
        };
        let self_described = matches!(
            stored,
            "LanguageProperty" | "VocabProperty" | "JsonProperty"
        ) && v
            .as_object()
            .is_some_and(|o| o.len() == 1 && o.contains_key(member));
        let mut out = if self_described {
            v.as_object().cloned().unwrap_or_default()
        } else if stored == "LanguageProperty" {
            let Some(tag) = self.lang else {
                return Err(NgsiError::BadRequestData(format!(
                    "attribute {iri}: a simplified value for a LanguageProperty needs the \
                     lang URL parameter to name its languageMap key (TS 104 176 7.3.3.2)"
                )));
            };
            let mut m = Map::new();
            m.insert(member.into(), json!({ tag: v }));
            m
        } else {
            let mut m = Map::new();
            m.insert(member.into(), v.clone());
            m
        };
        out.insert("type".into(), Value::String(stored.to_owned()));
        Ok(Value::Object(out))
    }
}

/// JSON Merge-Patch over internal docs (5.5.12).
pub fn merge_into(doc: &mut Value, fragment: &Value, ts: &str) {
    merge_with(doc, fragment, ts, false);
}

/// 5.6.9 Update (5.6.3 per Entity): as [`merge_into`], but an instance the
/// fragment names replaces the stored one whole (its `createdAt` kept), so a
/// member the update leaves out (`unitCode`) is gone, never merged over.
pub fn update_into(doc: &mut Value, fragment: &Value, ts: &str) {
    merge_with(doc, fragment, ts, true);
}

fn merge_with(doc: &mut Value, fragment: &Value, ts: &str, replace: bool) {
    // The Entity as a backfill names it (T-4665): id and type, before the merge.
    let shell = serde_json::json!({"id": doc.get("id"), "type": doc.get("type")});
    let (Some(target), Some(frag)) = (doc.as_object_mut(), fragment.as_object()) else {
        return;
    };
    for (k, v) in frag {
        match k.as_str() {
            "id" | "createdAt" | "modifiedAt" => continue,
            "type" => {
                // union of types
                let mut cur: Vec<Value> = target
                    .get("type")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for t in v.as_array().cloned().unwrap_or_default() {
                    if !cur.contains(&t) {
                        cur.push(t);
                    }
                }
                target.insert("type".into(), Value::Array(cur));
            }
            // 5.5.12: "For each member of the Fragment, whose value is an
            // NGSI-LD Null, contained by the target, the target member is
            // removed." 4.18 admits the sentinel as a scope for exactly this
            // reason, and expansion hands scopes over as an array, so the
            // deletion arrives as its single entry. Storing it instead left
            // the Entity scoped to a string no 4.18 grammar accepts.
            "scope" => {
                if antares_jsonld::is_ngsi_null_list(v) {
                    target.remove("scope");
                } else {
                    target.insert("scope".into(), v.clone());
                }
            }
            // 4.22: expiresAt is a settable Entity member (5.2.4, not in the
            // read-only Table 5.2.2-1) — merge updates the storage expiry;
            // an NGSI-LD Null removes it (5.5.12). Without this arm it fell
            // through to the attribute path, where a bare string has no
            // instances and the member was silently dropped.
            "expiresAt" => {
                if is_ngsi_null(v) {
                    target.remove("expiresAt");
                } else {
                    target.insert("expiresAt".into(), v.clone());
                }
            }
            _ => {
                let frag_instances = v.as_array().cloned().unwrap_or_default();
                let mut cur: Vec<Value> = target
                    .get(k)
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for fi in frag_instances {
                    let is_delete = antares_jsonld::is_deletion_instance(&fi);
                    let want_ds = fi.get("datasetId").and_then(Value::as_str);
                    let pos = cur
                        .iter()
                        .position(|ci| ci.get("datasetId").and_then(Value::as_str) == want_ds);
                    match (is_delete, pos) {
                        (true, Some(p)) => {
                            cur.remove(p);
                        }
                        (true, None) => {}
                        (false, Some(p)) => {
                            if let Some(ci) = cur.get_mut(p) {
                                if crate::history::backfilled(&shell, k, ci, &fi) {
                                    // older than the stored one: history only
                                } else if replace {
                                    let created = ci.get("createdAt").cloned();
                                    let mut ni = Value::Array(vec![fi.clone()]);
                                    crate::stamp::stamp_instances(&mut ni, ts);
                                    if let Some(Value::Object(mut o)) =
                                        ni.as_array_mut().and_then(Vec::pop)
                                    {
                                        if let Some(c) = created {
                                            o.insert("createdAt".into(), c);
                                        }
                                        *ci = Value::Object(o);
                                    }
                                } else {
                                    merge_instance(ci, &fi, ts);
                                }
                            }
                        }
                        (false, None) => {
                            let mut ni = fi.clone();
                            if let Some(o) = ni.as_object_mut() {
                                o.insert("createdAt".into(), Value::String(ts.to_owned()));
                                o.insert("modifiedAt".into(), Value::String(ts.to_owned()));
                                crate::stamp::stamp_by(o);
                            }
                            cur.push(ni);
                        }
                    }
                }
                if cur.is_empty() {
                    target.remove(k);
                } else {
                    target.insert(k.clone(), Value::Array(cur));
                }
            }
        }
    }
    target.insert("modifiedAt".into(), Value::String(ts.to_owned()));
}

fn merge_instance(target: &mut Value, frag: &Value, ts: &str) {
    let (Some(t), Some(f)) = (target.as_object_mut(), frag.as_object()) else {
        return;
    };
    // An instance of another attribute type (a LanguageProperty sent over a
    // stored Property) is a new attribute, not a patch of the old one: a key
    // by key merge kept the Property's value beside the new languageMap,
    // a shape no NGSI-LD type allows (T-5167). It replaces the instance and
    // keeps only the instant the attribute was created (4.8).
    let changes_type = f
        .get("type")
        .is_some_and(|ft| t.get("type").is_some_and(|tt| tt != ft));
    if changes_type {
        let created = t.get("createdAt").cloned();
        t.clear();
        for (k, v) in f {
            if k != "createdAt" && k != "modifiedAt" && !antares_jsonld::removes_member(v) {
                t.insert(k.clone(), v.clone());
            }
        }
        if let Some(c) = created {
            t.insert("createdAt".into(), c);
        }
        t.insert("modifiedAt".into(), Value::String(ts.to_owned()));
        crate::stamp::stamp_by(t);
        return;
    }
    for (k, v) in f {
        if k == "createdAt" || k == "modifiedAt" {
            continue;
        }
        // a sub-Attribute whose instances are all deletions leaves too (10.2.9.4, T-3677)
        if antares_jsonld::removes_member(v) {
            t.remove(k);
        } else if let (Some(cur), Some(patch)) =
            (t.get_mut(k).and_then(Value::as_object_mut), v.as_object())
        {
            // 5.5.12: the merge goes "into JSON objects representing a
            // Property value" — RFC 7396 with the NGSI-LD Null as removal.
            merge_value_object(cur, patch);
        } else {
            t.insert(k.clone(), v.clone());
        }
    }
    t.insert("modifiedAt".into(), Value::String(ts.to_owned()));
    crate::stamp::stamp_by(t);
}

/// RFC 7396 merge patch over a compound (JSON object) member value, with
/// "urn:ngsi-ld:null" / JSON null as the key-removal marker (5.5.12); the
/// sentinel itself is never stored (5.5.4).
fn merge_value_object(target: &mut Map<String, Value>, patch: &Map<String, Value>) {
    for (k, v) in patch {
        if v.is_null() || is_ngsi_null(v) {
            target.remove(k);
        } else if let (Some(cur), Some(po)) = (
            target.get_mut(k).and_then(Value::as_object_mut),
            v.as_object(),
        ) {
            merge_value_object(cur, po);
        } else {
            target.insert(k.clone(), v.clone());
        }
    }
}

// ---------- PUT /entities/{id} — Replace (5.6.18) ----------

pub async fn replace_entity(
    State(st): State<AppState>,
    Path(id): Path<String>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let go = async {
        let tenant = tenant_from(&headers)?;
        antares_model::EntityId::new(&id)?;
        check_params(&params, &["local", "type"])?;
        // 5.5.7 again: the selector is expanded with the request's @context,
        // not the core one. The target is judged before the body is parsed
        // (5.6.18: an unknown target is 404 before body validation), so the
        // header context is resolved on its own here.
        let ctx0 = request_context(&st.loader, &headers).await?;
        // The engine is shown the replacement. The body is read for it
        // leniently: a body that does not parse is not the engine's to
        // judge, and the replace below refuses it as before, after the
        // 404 an unknown target answers first (057_03).
        let shown = match &st.policy {
            Some(_) => parse_body(&st, &headers, &body, BodyKind::Standard)
                .await
                .ok()
                .and_then(|p| {
                    let obj = p.object(NgsiError::BadRequestData(String::new())).ok()?;
                    expand_entity(obj, &p.ctx, ExpandOpts::default()).ok()
                }),
            None => None,
        };
        let pre = crate::policy::pre_image(&st, &tenant, &id, shown.as_ref()).await?;
        let granted = gate!(
            st, &tenant, &headers, "5.6.18",
            ids: &[&id],
            types: &pre.types,
            stored: pre.stored.as_ref(),
            attrs: &shown.as_ref().map(crate::attrs::attr_iris_of).unwrap_or_default(),
            body: shown.as_ref(),
        )
        .await?;
        let cond = crate::policy::Condition::of(&granted);
        // 5.6.18.4: the ?type selector narrows the target — a non-matching
        // entity is "not known" for this replace.
        let local_doc = st
            .store
            .get(&tenant, Kind::Entity, &id)
            .await?
            .filter(|d| crate::negotiate::matches_type_param(d, &params, &ctx0))
            // gap G12: an Entity the engine's condition does not select is
            // not found, before the body is read, as an absent target is
            .filter(|d| cond.as_ref().is_none_or(|c| c.holds(d)));
        // ADR-0023 rule 3: out of the view is not found, before the body is
        // read, as an absent target is (057_03)
        #[cfg(feature = "views")]
        let local_doc = local_doc.filter(|d| {
            crate::views::WriteGuard::of(&st, std::iter::empty()).is_none_or(|g| g.sees(d))
        });
        let spec = crate::registry::CsrSpec {
            ids: Some(vec![id.clone()]),
            ..Default::default()
        };
        let regs =
            match crate::federation::write_plan(&st, &tenant, &spec, &ctx0, &params, &headers)
                .await?
            {
                crate::federation::WritePlan::Answered(r) => return Ok(*r),
                crate::federation::WritePlan::Forward(regs) => regs,
            };
        crate::policy::Condition::unforwarded(cond.as_ref(), !regs.is_empty())?;
        if regs.is_empty() {
            // 5.6.18: an unknown target is 404 before body validation (057_03).
            // The read above answers that; the write below decides again
            // under the row lock, because between the two the target can be
            // deleted and a replace that writes anyway puts it back.
            if local_doc.is_none() {
                // T-3789: under `If-Match`, out of the view and absent are both 412
                #[cfg(feature = "views")]
                if let Some(g) = crate::views::WriteGuard::of(&st, std::iter::empty()) {
                    g.matching(&headers).settled()?;
                }
                return Err(NgsiError::ResourceNotFound(format!("entity {id} not found")).into());
            }
            let parsed = parse_body(&st, &headers, &body, BodyKind::Standard).await?;
            let obj = parsed.object(NgsiError::BadRequestData(
                "entity must be a JSON object".into(),
            ))?;
            let mut expanded = expand_entity(obj, &parsed.ctx, ExpandOpts::default())?;
            if expanded.get("id").and_then(Value::as_str) != Some(id.as_str()) {
                return Err(NgsiError::BadRequestData("entity id mismatch".into()).into());
            }
            #[cfg(feature = "views")]
            crate::views::schema_admits_typed(&st, &tenant, &expanded, true)?;
            let ts = now_iso();
            stamp_new(&mut expanded, &ts);
            #[cfg(feature = "views")]
            let guard = crate::views::WriteGuard::of(
                &st,
                crate::attrs::attr_iris_of(&expanded)
                    .iter()
                    .map(String::as_str),
            )
            .map(|g| g.matching(&headers));
            #[cfg(feature = "views")]
            if let Some(g) = &guard {
                g.targets(&st, &tenant, &expanded).await?;
            }
            let res = st
                .store
                .mutate(&tenant, Kind::Entity, &id, |doc| {
                    crate::policy::Condition::pre(cond.as_ref(), &id, doc)?;
                    #[cfg(feature = "views")]
                    if let Some(g) = &guard {
                        g.pre_whole(&id, doc)?;
                    }
                    // 5.6.18.4: the ?type selector narrows the target here too —
                    // the type of the row being written is the one that counts
                    if !crate::negotiate::matches_type_param(doc, &params, &ctx0) {
                        return Err(NgsiError::ResourceNotFound(format!(
                            "entity {id} does not match the type selector"
                        )));
                    }
                    // 4.8: "createdAt ... shall be the date and time at which the
                    // Entity was created" — the target's own stamp, read under
                    // the lock rather than from a snapshot another write may
                    // already have replaced.
                    if let (Some(o), Some(created)) =
                        (expanded.as_object_mut(), doc.get("createdAt").cloned())
                    {
                        o.insert("createdAt".into(), created);
                    }
                    *doc = expanded.clone();
                    crate::policy::Condition::post(cond.as_ref(), doc)?;
                    #[cfg(feature = "views")]
                    if let Some(g) = &guard {
                        g.post(doc)?;
                    }
                    Ok::<(), NgsiError>(())
                })
                .await?;
            crate::policy::Condition::verdict(cond.as_ref())?;
            #[cfg(feature = "views")]
            if let Some(g) = &guard {
                g.settled()?;
            }
            return match res {
                None => Err(NgsiError::ResourceNotFound(format!("entity {id} not found")).into()),
                Some(Err(e)) => Err(e.into()),
                Some(Ok(())) => Ok::<_, ApiError>(no_content(&tenant)),
            };
        }
        let parsed = parse_body(&st, &headers, &body, BodyKind::Standard).await?;
        let obj = parsed.object(NgsiError::BadRequestData(
            "entity must be a JSON object".into(),
        ))?;
        let expanded = expand_entity(obj, &parsed.ctx, ExpandOpts::default())?;
        if expanded.get("id").and_then(Value::as_str) != Some(id.as_str()) {
            return Err(NgsiError::BadRequestData("entity id mismatch".into()).into());
        }
        #[cfg(feature = "views")]
        crate::views::schema_admits_typed(&st, &tenant, &expanded, true)?;
        let mut parts = Vec::new();
        let proxies: Vec<&crate::federation::FedReg> =
            regs.iter().filter(|r| r.is_proxy()).collect();
        let proxy_match = !proxies.is_empty();
        if local_doc.is_some() || !proxy_match {
            let gone =
                crate::federation::Part::local(404, format!("entity {id} not found locally"));
            if local_doc.is_none() {
                parts.push(gone);
            } else {
                let (rest, _) = crate::federation::strip_proxied(obj, &proxies, &parsed.ctx);
                let mut local_exp = expand_entity(&rest, &parsed.ctx, ExpandOpts::default())?;
                let ts = now_iso();
                stamp_new(&mut local_exp, &ts);
                // the same row lock as the local-only path above: the read
                // that found the target is not the write that replaces it
                let res = st
                    .store
                    .mutate(&tenant, Kind::Entity, &id, |doc| {
                        if !crate::negotiate::matches_type_param(doc, &params, &ctx0) {
                            return Err(NgsiError::ResourceNotFound(format!(
                                "entity {id} does not match the type selector"
                            )));
                        }
                        if let (Some(o), Some(created)) =
                            (local_exp.as_object_mut(), doc.get("createdAt").cloned())
                        {
                            o.insert("createdAt".into(), created);
                        }
                        *doc = local_exp.clone();
                        Ok::<(), NgsiError>(())
                    })
                    .await?;
                match res {
                    Some(Ok(())) => {
                        parts.push(crate::federation::Part::local(204, "replaced locally"))
                    }
                    _ => parts.push(gone),
                }
            }
        }
        let ctx_url = crate::federation::ctx_link_url(&headers, &parsed.ctx.source);
        let seg = crate::federation::path_segment(&id);
        let fwd_q = type_selector_query(&params);
        for reg in &regs {
            // 5.6.18.4: proxy modes without Replace Entity support are an
            // error of type Conflict; inclusive ones are not forwarded.
            if !reg.supports("replaceEntity") {
                if reg.is_proxy() {
                    parts.push(crate::federation::conflict_part("replaceEntity", reg));
                }
                continue;
            }
            let Some(frag) = crate::federation::reduce_to_scope(obj, reg, &parsed.ctx) else {
                continue;
            };
            parts.push(
                crate::federation::forward_part(
                    &st,
                    reqwest::Method::PUT,
                    format!("{}/ngsi-ld/v1/entities/{seg}", reg.endpoint),
                    &fwd_q,
                    &headers,
                    &tenant,
                    reg,
                    &ctx_url,
                    Some(frag),
                )
                .await,
            );
        }
        Ok(crate::federation::combine(
            st.profile,
            Some(&id),
            parts,
            no_content(&tenant),
            &tenant,
        ))
    };
    go.await.unwrap_or_else(|e| e.into_response())
}

// ---------- Entity Ordering (4.23) ----------

// ---------- GeoJSON output (6.3.15) ----------

// ---------- GET /entities/{id}/attrs/{attrId} [+ /value] ----------
// NGSI-LD 2.0 pre-adoptions #14/#15: retrieve a single
// attribute of an entity, and its bare value. Additive-only: 2.0 defines the
// resources, 1.9.1 clients never see them unless asked.

pub async fn retrieve_entity_attr(
    State(st): State<AppState>,
    Path((id, attr)): Path<(String, String)>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
) -> Response {
    match retrieve_attr_inner(&st, &id, &attr, false, &params, &headers).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

pub async fn retrieve_entity_attr_value(
    State(st): State<AppState>,
    Path((id, attr)): Path<(String, String)>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
) -> Response {
    match retrieve_attr_inner(&st, &id, &attr, true, &params, &headers).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

async fn retrieve_attr_inner(
    st: &AppState,
    id: &str,
    attr: &str,
    value_only: bool,
    params: &HashMap<String, String>,
    headers: &HeaderMap,
) -> ApiResult<Response> {
    let tenant = tenant_from(headers)?;
    check_params(
        params,
        &["options", "format", "lang", "datasetId", "local", "type"],
    )?;
    let ctx = request_context(&st.loader, headers).await?;
    let filter = gate!(
        st, &tenant, headers, "5.7.1",
        ids: &[id],
        attrs: &antares_jsonld::expand_attr_name(attr, &ctx).ok().into_iter().collect::<Vec<_>>(),
    )
    .await?;
    let mut repr = parse_repr(params, &ctx)?;
    repr.modified_by = crate::provenance::shows_modified_by(headers, st.provenance_from.as_deref());
    crate::repr::narrow_repr(&mut repr, &filter);
    antares_model::EntityId::new(id)?;
    antares_model::check_attr_name(attr)?;
    let doc = st
        .store
        .get(&tenant, Kind::Entity, id)
        .await?
        .ok_or_else(|| NgsiError::ResourceNotFound(format!("entity {id} not found")))?;
    // ADR-0020, ADR-0023 rule 3: the Attribute of an Entity outside the
    // narrowing is the Attribute of an absent one, as Retrieve Entity
    // answers it, whatever the Attribute's own visibility.
    if !crate::repr::policy_admits(st, &tenant, &ctx, repr.policy_reach.as_deref(), &doc).await {
        return Err(NgsiError::ResourceNotFound(format!("entity {id} not found")).into());
    }
    // TS 104 175 10.4.4.4: no Entity "whose id (URI), and where specified
    // type, is equivalent" — the ?type selector (7.2.2) narrows the target.
    if !crate::negotiate::matches_type_param(&doc, params, &ctx) {
        return Err(NgsiError::ResourceNotFound(format!(
            "entity {id} does not match the type selector"
        ))
        .into());
    }
    let attr_iri = antares_jsonld::expand_attr_name(attr, &ctx)?;
    // One answer for an Attribute the Entity lacks and one the narrowing
    // hides (ADR-0023 rule 2).
    let lacks = || NgsiError::ResourceNotFound(format!("entity {id} has no attribute {attr}"));
    let node = doc.get(&attr_iri).ok_or_else(lacks)?;
    // 10.4.4.4: without ?datasetId the Attribute is found only by its
    // default instance, the one carrying no datasetId.
    let default_instance = |i: &Value| i.get("datasetId").is_none();
    let has_default = match node {
        Value::Array(a) => a.iter().any(default_instance),
        one => default_instance(one),
    };
    if !params.contains_key("datasetId") && !has_default {
        return Err(NgsiError::ResourceNotFound(format!(
            "entity {id} has no default instance of attribute {attr}; ask one by datasetId"
        ))
        .into());
    }
    // Compact through the entity pipeline so the attribute serializes exactly
    // as it would inside a full retrieve.
    let mini = serde_json::json!({
        "id": doc.get("id").cloned().unwrap_or_default(),
        "type": doc.get("type").cloned().unwrap_or_default(),
        attr_iri.clone(): node.clone(),
    });
    let shaped = crate::repr::apply(&mini, &repr);
    let compacted = compact_for(&repr, &shaped, &ctx);
    let key = ctx.compact_iri(&attr_iri);
    let member = compacted.get(&key).cloned().ok_or_else(lacks)?;
    let body = if value_only {
        // #15: the bare value — value / object / languageMap, whichever the
        // attribute type carries; multi-instance attributes yield an array.
        fn bare(v: &Value) -> Value {
            match v {
                Value::Array(a) => Value::Array(a.iter().map(bare).collect()),
                Value::Object(o) => o
                    .get("value")
                    .or_else(|| o.get("object"))
                    .or_else(|| o.get("languageMap"))
                    .or_else(|| o.get("json"))
                    .or_else(|| o.get("vocab"))
                    .or_else(|| o.get("valueList"))
                    .or_else(|| o.get("objectList"))
                    .cloned()
                    .unwrap_or(Value::Null),
                other => other.clone(),
            }
        }
        bare(&member)
    } else {
        member
    };
    let accept = parse_accept(headers)?;
    let mut resp = respond(StatusCode::OK, body, &ctx, accept, &tenant);
    filter.mark(&st.host_alias, resp.headers_mut());
    Ok(resp)
}

/// 5.14.4.4: run the (split-reduced when applicable) local query and record
/// each matching id under the "@none" local marker; forward to matching
/// registrations supporting createEntityMapQueryEntity and merge each
/// returned EntityMap (ids → registration id, linkedMaps → remote map id);
/// store the local EntityMap and return it.
/// Known ceiling: the local candidate ids are the first max_limit matches —
/// the query is paged into the store instead of materializing every matching
/// Entity document, so one request cannot pull a whole tenant into memory.
/// Raise the cap if local candidate sets outgrow it.
pub(crate) async fn build_query_map(
    st: &AppState,
    tenant: &TenantId,
    headers: &HeaderMap,
    ctx: &antares_jsonld::Context,
    params: &HashMap<String, String>,
    filter: &crate::policy::Filter,
) -> ApiResult<Value> {
    let q_ast = params
        .get("q")
        .map(|q| antares_ql::parse_q(q))
        .transpose()?;
    // 5.14.4.4 a-e: too wide query
    if !qualifies_non_wide(params, q_ast.as_ref()) {
        return Err(NgsiError::BadRequestData(
            "EntityMap query needs at least one of type, attrs, q, georel, or local=true \
             (5.14.4.4 — too wide query)"
                .into(),
        )
        .into());
    }
    // the candidate set is the NARROWED query's (ADR-0020)
    let narrowed = filter.narrow_params(params, ctx)?;
    let narrow = crate::repr::narrowing_reading(filter, params, ctx)?;
    let params = &narrowed;
    // 5.14.4.4: invalid entity ids / csf are BadRequestData
    if let Some(ids) = params.get("id") {
        for id in ids.split(',') {
            antares_model::EntityId::new(id.trim())?;
        }
    }
    if let Some(csf) = params.get("csf") {
        antares_ql::parse_q(csf)?;
    }
    let local_scope = params.get("local").map(String::as_str) == Some("true");
    let split = params.get("splitEntities").map(String::as_str) == Some("true");
    // Split entities: only id/type/idPattern narrow the local candidate set —
    // value/geo/scope filters cannot be judged on a fragment (5.14.4.4).
    let eff: HashMap<String, String> = if split && !local_scope {
        params
            .iter()
            .filter(|(k, _)| ["id", "idPattern", "type", "local"].contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    } else {
        params.clone()
    };
    // idPattern is invisible to the store, so a pushed page cannot BE the
    // candidate set's page — it is still the window a walk steps through, and
    // the literal the pattern forces has already narrowed the rows (5.2.33).
    // Without the walk a tenant-wide pattern arrives as one allocation of
    // every Entity the rest of the filter matched.
    let mut emap = Map::new();
    let mut offset = 0usize;
    while emap.len() < st.max_limit {
        let batch = filter_entities_paged(
            st,
            tenant,
            &eff,
            ctx,
            Vec::new(),
            Pushdown {
                page: Some((offset, st.max_limit)),
                proj: None,
                order_keys: None,
                q: None,
            },
            Some(&narrow),
        )
        .await?;
        for d in &batch.docs {
            if emap.len() == st.max_limit {
                break;
            }
            if let Some(id) = d.get("id").and_then(Value::as_str) {
                // "@none" refers to an Entity held locally (5.2.39)
                emap.insert(id.to_owned(), json!(["@none"]));
            }
        }
        match next_scan_offset(offset, batch.rows, 0, batch.paged, st.max_limit) {
            Some(next) => offset = next,
            None => break,
        }
    }
    crate::entity_map::merge_and_store_map(st, tenant, headers, ctx, params, false, emap).await
}

/// The store page of the conditional-rules scan as the store's `i64` binds,
/// saturating: `enough` is the client's offset plus limit, so an offset near
/// `i64::MAX` (which `page_params` admits) and the doubling limit pass it,
/// and an `as` cast wraps negative, a LIMIT postgres refuses (500).
fn store_page((offset, limit): (usize, usize)) -> antares_store::filter::Page {
    antares_store::filter::Page {
        offset: i64::try_from(offset).unwrap_or(i64::MAX),
        limit: i64::try_from(limit).unwrap_or(i64::MAX),
        count: false,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_scan_page_past_i64_saturates_and_never_wraps_negative() {
        let past = usize::try_from(i64::MAX).unwrap_or(usize::MAX) + 11;
        let p = super::store_page((past, usize::MAX));
        assert_eq!((p.offset, p.limit), (i64::MAX, i64::MAX));
        let p = super::store_page((20, 41));
        assert_eq!((p.offset, p.limit, p.count), (20, 41, false));
    }

    use super::merge_into;
    use serde_json::json;

    /// 5.5.12 EXAMPLE 1 + the datasetId/type bullets: a merge updates the
    /// named sub-attributes and leaves the others untouched; a fragment
    /// instance with an unknown datasetId is ADDED (not replacing the
    /// default); entity types are unioned.
    #[test]
    fn clause_5_5_12_merge_algorithm() {
        let mut doc = json!({"id": "urn:x", "type": ["https://uri.etsi.org/ngsi-ld/default-context/T"],
            "https://uri.etsi.org/ngsi-ld/default-context/temperature": [{
                "type": "Property", "value": 25, "unitCode": "CEL",
                "observedAt": "2022-03-14T01:59:26.535Z"}]});
        merge_into(
            &mut doc,
            &json!({
            "type": ["https://uri.etsi.org/ngsi-ld/default-context/T",
                     "https://uri.etsi.org/ngsi-ld/default-context/U"],
            "https://uri.etsi.org/ngsi-ld/default-context/temperature": [
                {"type": "Property", "value": 100,
                 "observedAt": "2022-03-14T13:00:00.000Z"},
                {"type": "Property", "value": 7,
                 "datasetId": "urn:ngsi-ld:Dataset:extra"}
            ]}),
            "2026-08-11T00:00:00Z",
        );
        // EXAMPLE 1: value/observedAt updated, unitCode untouched
        let t = &doc["https://uri.etsi.org/ngsi-ld/default-context/temperature"];
        let default = t
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i.get("datasetId").is_none())
            .expect("default instance");
        assert_eq!(default["value"], 100);
        assert_eq!(default["observedAt"], "2022-03-14T13:00:00.000Z");
        assert_eq!(
            default["unitCode"], "CEL",
            "unmentioned sub-attribute survives"
        );
        // unknown datasetId is added as a NEW instance
        assert_eq!(t.as_array().unwrap().len(), 2);
        // entity types are unioned, no duplicates
        assert_eq!(
            doc["type"],
            json!([
                "https://uri.etsi.org/ngsi-ld/default-context/T",
                "https://uri.etsi.org/ngsi-ld/default-context/U"
            ])
        );
    }

    /// 5.5.12: merge "merges the provided information with the existing
    /// information up to an arbitrary depth, e.g. including going into JSON
    /// objects representing a Property value" (RFC 7396 with the NGSI-LD
    /// Null) — untouched keys survive, null-valued keys are removed, and the
    /// null sentinel never lands in the stored document (5.5.4).
    #[test]
    fn merge_goes_into_compound_property_values() {
        let mut doc = json!({"id": "urn:x", "type": ["T"],
            "https://uri.etsi.org/ngsi-ld/default-context/address": [{
                "type": "Property",
                "value": {"street": "Straße des 17. Juni", "city": "Berlin",
                          "country": "Germany"}}]});
        merge_into(
            &mut doc,
            &json!({"https://uri.etsi.org/ngsi-ld/default-context/address": [{
                "type": "Property",
                "value": {"street": "Pariser Platz",
                          "country": "urn:ngsi-ld:null"}}]}),
            "2026-08-11T00:00:00Z",
        );
        let v = &doc["https://uri.etsi.org/ngsi-ld/default-context/address"][0]["value"];
        assert_eq!(v["street"], "Pariser Platz");
        assert_eq!(v["city"], "Berlin", "untouched keys survive the merge");
        assert!(v.get("country").is_none(), "null removes the key");
        assert!(
            !doc.to_string().contains("urn:ngsi-ld:null"),
            "the null sentinel must never be stored"
        );
    }

    /// T-5167: dev's Bridge M1020 kept `value` beside a `languageMap` after a
    /// writer changed `name` from one attribute type to the other.
    #[test]
    fn merge_replaces_an_instance_whose_attribute_type_changes() {
        let name = "https://schema.org/name";
        let mut doc = json!({"id": "urn:x", "type": ["T"],
            name: [{"type": "Property", "value": "M1020",
                    "createdAt": "2026-10-01T00:00:00Z", "modifiedAt": "2026-10-01T00:00:00Z"}]});
        merge_into(
            &mut doc,
            &json!({name: [{"type": "LanguageProperty", "languageMap": {"sk": "Most"}}]}),
            "2026-10-22T00:02:58Z",
        );
        assert_eq!(
            doc[name],
            json!([{"type": "LanguageProperty", "languageMap": {"sk": "Most"},
                    "createdAt": "2026-10-01T00:00:00Z", "modifiedAt": "2026-10-22T00:02:58Z"}])
        );
        merge_into(
            &mut doc,
            &json!({name: [{"type": "Property", "value": "M1020"}]}),
            "2026-10-22T01:00:00Z",
        );
        assert_eq!(
            doc[name],
            json!([{"type": "Property", "value": "M1020",
                    "createdAt": "2026-10-01T00:00:00Z", "modifiedAt": "2026-10-22T01:00:00Z"}])
        );
    }

    /// The same attribute type still merges key by key (5.5.12).
    #[test]
    fn merge_keeps_the_keys_of_an_instance_of_the_same_type() {
        let name = "https://schema.org/name";
        let mut doc = json!({"id": "urn:x", "type": ["T"],
            name: [{"type": "LanguageProperty", "languageMap": {"sk": "Most", "en": "Bridge"},
                    "observedAt": "2026-10-01T00:00:00Z"}]});
        merge_into(
            &mut doc,
            &json!({name: [{"type": "LanguageProperty", "languageMap": {"sk": "Most M1020"}}]}),
            "2026-10-22T00:00:00Z",
        );
        assert_eq!(
            doc[name][0]["languageMap"],
            json!({"sk": "Most M1020", "en": "Bridge"})
        );
        assert_eq!(doc[name][0]["observedAt"], "2026-10-01T00:00:00Z");
    }

    #[test]
    fn merge_sets_and_null_removes_expires_at() {
        let mut doc = json!({"id": "urn:x", "type": ["T"]});
        merge_into(
            &mut doc,
            &json!({"expiresAt": "2030-01-01T00:00:00Z"}),
            "2026-08-08T00:00:00Z",
        );
        assert_eq!(doc["expiresAt"], "2030-01-01T00:00:00Z");
        merge_into(
            &mut doc,
            &json!({"expiresAt": "urn:ngsi-ld:null"}),
            "2026-08-08T00:00:01Z",
        );
        assert!(doc.get("expiresAt").is_none(), "NGSI-LD Null removes it");
    }
}

#[cfg(test)]
mod clause_4_16 {
    use super::*;
    use serde_json::json;

    /// 4.16: "Entity Types can be implicitly added by all operations that
    /// update or append attributes. There is no operation to remove Entity
    /// Types from an Entity."
    #[test]
    fn merge_unions_types_and_never_removes() {
        let mut doc = json!({"id": "urn:x", "type": ["A"],
            "https://ex/p": [{"type": "Property", "value": 1}]});
        merge_into(
            &mut doc,
            &json!({"type": ["B"]}),
            "2026-08-11T00:00:00.000Z",
        );
        assert_eq!(doc["type"], json!(["A", "B"]), "types union");
        // a fragment naming FEWER types must not shrink the set
        merge_into(
            &mut doc,
            &json!({"type": ["A"]}),
            "2026-08-11T00:00:00.000Z",
        );
        assert_eq!(
            doc["type"],
            json!(["A", "B"]),
            "no operation removes Entity Types"
        );
        // duplicates are not accumulated
        merge_into(
            &mut doc,
            &json!({"type": ["B"]}),
            "2026-08-11T00:00:00.000Z",
        );
        assert_eq!(doc["type"], json!(["A", "B"]));
    }
}

#[cfg(test)]
mod clause_4_17 {
    use antares_jsonld::Loader;
    use antares_ql::type_selection_matches;

    /// 4.17: disjunction via `|` or `,`, conjunction via `(a;b)`; short
    /// names expand against the @context.
    #[test]
    fn selection_language_semantics() {
        let ctx = Loader::new().core();
        const D: &str = "https://uri.etsi.org/ngsi-ld/default-context/";
        let home = format!("{D}Home");
        let vehicle = format!("{D}Vehicle");
        let motorhome = format!("{D}Motorhome");
        let both: Vec<&str> = vec![&home, &vehicle];
        let only_home: Vec<&str> = vec![&home];
        let only_motor: Vec<&str> = vec![&motorhome];
        // EXAMPLE 1: OR, both spellings
        assert!(type_selection_matches("Building|Home", &only_home, &ctx));
        assert!(type_selection_matches("Building,Home", &only_home, &ctx));
        assert!(!type_selection_matches("Building|House", &only_home, &ctx));
        // EXAMPLE 2: conjunction — ALL listed types required
        assert!(type_selection_matches("(Home;Vehicle)", &both, &ctx));
        assert!(
            !type_selection_matches("(Home;Vehicle)", &only_home, &ctx),
            "an entity with only Home must NOT match the conjunction"
        );
        // EXAMPLE 3: (Home;Vehicle)|Motorhome in both alternative spellings
        for sel in ["(Home;Vehicle)|Motorhome", "(Home;Vehicle),Motorhome"] {
            assert!(type_selection_matches(sel, &both, &ctx), "{sel}");
            assert!(type_selection_matches(sel, &only_motor, &ctx), "{sel}");
            assert!(!type_selection_matches(sel, &only_home, &ctx), "{sel}");
        }
    }
}

#[cfg(test)]
mod clause_5_2_2 {
    use super::*;
    use antares_jsonld::{ExpandOpts, Loader};
    use serde_json::json;

    /// 5.2.2: createdAt/modifiedAt/deletedAt "shall not be provided by
    /// Context Producers. In the event that they are provided ... NGSI-LD
    /// implementations shall ignore them" — server stamps win, no error.
    #[test]
    fn client_provided_system_timestamps_are_ignored() {
        let ctx = Loader::new().core();
        let doc = json!({"id": "urn:x", "type": "T",
            "createdAt": "1999-01-01T00:00:00Z",
            "modifiedAt": "1999-01-01T00:00:00Z",
            "deletedAt": "1999-01-01T00:00:00Z",
            "p": {"type": "Property", "value": 1,
                  "createdAt": "1999-01-01T00:00:00Z"}});
        let mut out = antares_jsonld::expand_entity(
            doc.as_object().expect("obj"),
            &ctx,
            ExpandOpts::default(),
        )
        .expect("providing common members is not an error");
        stamp_new(&mut out, "2026-08-11T00:00:00.000Z");
        assert_eq!(out["createdAt"], "2026-08-11T00:00:00.000Z");
        assert_eq!(out["modifiedAt"], "2026-08-11T00:00:00.000Z");
        assert!(out.get("deletedAt").is_none(), "deletedAt never creatable");
        let inst = &out["https://uri.etsi.org/ngsi-ld/default-context/p"][0];
        assert_eq!(
            inst["createdAt"], "2026-08-11T00:00:00.000Z",
            "instance-level client timestamp ignored too"
        );
    }

    /// 5.2.2: common members are only generated "when the Context Consumer
    /// explicitly asks for their inclusion" (sysAttrs, 6.3.11).
    #[test]
    fn common_members_appear_only_on_request() {
        let doc = json!({"id": "urn:x", "type": ["T"],
            "createdAt": "2026-08-11T00:00:00.000Z",
            "modifiedAt": "2026-08-11T00:00:00.000Z",
            "https://uri.etsi.org/ngsi-ld/default-context/p": [
                {"type": "Property", "value": 1,
                 "createdAt": "2026-08-11T00:00:00.000Z",
                 "modifiedAt": "2026-08-11T00:00:00.000Z"}]});
        let plain = crate::repr::apply(&doc, &crate::repr::Repr::default());
        assert!(plain.get("createdAt").is_none());
        assert!(plain["https://uri.etsi.org/ngsi-ld/default-context/p"][0]
            .get("modifiedAt")
            .is_none());
        let sys = crate::repr::apply(
            &doc,
            &crate::repr::Repr {
                sys_attrs: true,
                ..Default::default()
            },
        );
        assert!(sys.get("createdAt").is_some());
        assert!(sys["https://uri.etsi.org/ngsi-ld/default-context/p"][0]
            .get("modifiedAt")
            .is_some());
    }
}

/// 4.5.23.1: "When retrieving Linked Entities, it is necessary to limit
/// retrieval to avoid cascades of an excessive length, duplicates or loops."
#[cfg(test)]
mod clause_4_5_23_bounds {
    use super::*;
    use crate::repr::inline_join;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use serde_json::json;
    use tower::ServiceExt;

    fn app() -> axum::Router {
        crate::router(AppState::new("antares-test".into()))
    }

    async fn create(app: &axum::Router, body: Value) {
        let payload = body.to_string();
        let resp = app
            .clone()
            .oneshot(
                Request::post("/ngsi-ld/v1/entities")
                    .header("Content-Type", "application/json")
                    .header("Content-Length", payload.len().to_string())
                    .body(Body::from(payload))
                    .expect("req"),
            )
            .await
            .expect("resp");
        assert_eq!(resp.status(), StatusCode::CREATED, "create failed");
    }

    async fn get(app: &axum::Router, uri: &str) -> (axum::http::response::Parts, Value) {
        let resp = app
            .clone()
            .oneshot(Request::get(uri).body(Body::empty()).expect("req"))
            .await
            .expect("resp");
        let (parts, body) = resp.into_parts();
        let bytes = body.collect().await.expect("body").to_bytes();
        let json: Value = serde_json::from_slice(&bytes).expect("json");
        (parts, json)
    }

    /// 4.5.23.1/4.5.23.3: the flattened array carries the Linking Entity and
    /// its Linked Entities — a Relationship pointing back at the root is a
    /// loop, so the root shall appear exactly ONCE, not once as the Linking
    /// Entity and again as its own Linked Entity.
    #[tokio::test]
    async fn flat_join_never_repeats_the_root_entity() {
        let app = app();
        let root = "urn:ngsi-ld:Loop:root";
        let leaf = "urn:ngsi-ld:Loop:leaf";
        create(&app, json!({"id": leaf, "type": "Loop"})).await;
        create(
            &app,
            json!({"id": root, "type": "Loop",
                   "self": {"type": "Relationship", "object": root},
                   "other": {"type": "Relationship", "object": leaf}}),
        )
        .await;

        let (_, body) = get(
            &app,
            &format!("/ngsi-ld/v1/entities/{root}?join=flat&joinLevel=3"),
        )
        .await;
        let arr = match body {
            Value::Array(a) => a,
            other => vec![other],
        };
        let (mut roots, mut leaves) = (0usize, 0usize);
        for e in &arr {
            match e["id"].as_str() {
                Some(id) if id == root => roots += 1,
                Some(id) if id == leaf => leaves += 1,
                _ => {}
            }
        }
        assert_eq!(
            roots, 1,
            "the root must appear exactly once in the flattened array: {arr:?}"
        );
        assert_eq!(
            leaves, 1,
            "the genuine Linked Entity is still there once: {arr:?}"
        );
        assert_eq!(arr.len(), 2, "no other entity is in the array: {arr:?}");
    }

    /// 4.5.23.1: a cyclic graph at a high joinLevel shall not cascade — the
    /// walk stops at entities it already resolved and says so with an
    /// NGSILD-Warning (6.3.17) instead of expanding fan-out^joinLevel.
    #[tokio::test]
    async fn cyclic_inline_join_stops_instead_of_cascading() {
        let app = app();
        let ids = [
            "urn:ngsi-ld:Cyc:a",
            "urn:ngsi-ld:Cyc:b",
            "urn:ngsi-ld:Cyc:c",
        ];
        // complete graph: every entity links to every entity, itself included
        for id in ids {
            create(
                &app,
                json!({"id": id, "type": "Cyc",
                       "toA": {"type": "Relationship", "object": ids[0]},
                       "toB": {"type": "Relationship", "object": ids[1]},
                       "toC": {"type": "Relationship", "object": ids[2]}}),
            )
            .await;
        }

        let (parts, body) = get(
            &app,
            &format!("/ngsi-ld/v1/entities/{}?join=inline&joinLevel=9", ids[0]),
        )
        .await;
        assert_eq!(parts.status, StatusCode::OK);
        // 3^9 embeddings if the walk is unbounded; a handful if it is not
        let embedded = body.to_string().matches("urn:ngsi-ld:Cyc:").count();
        assert!(
            embedded < 64,
            "the cyclic walk cascaded: {embedded} entity references embedded"
        );
        assert!(
            parts.headers.get("NGSILD-Warning").is_some(),
            "a truncated Linked Entity Retrieval must be reported (6.3.17)"
        );
    }

    /// 4.5.23.1: joinLevel bounds the depth, not the width — one retrieval
    /// may only buy MAX_JOIN_LOOKUPS Linked Entity reads, and the truncation
    /// is reported back to the caller.
    #[tokio::test]
    async fn wide_inline_join_stops_at_the_lookup_budget() {
        let st = AppState::new("antares-test".into());
        let tenant = TenantId::default();
        let ctx = antares_jsonld::Loader::new().core();
        let mut targets: Vec<Value> = Vec::new();
        for n in 0..MAX_JOIN_LOOKUPS + 100 {
            let id = format!("urn:ngsi-ld:Wide:{n}");
            let doc = json!({"id": &id, "type":
                ["https://uri.etsi.org/ngsi-ld/default-context/Wide"]});
            st.store
                .upsert(&tenant, Kind::Entity, &id, doc)
                .await
                .expect("seed");
            targets.push(Value::String(id));
        }
        let mut compacted = json!({"id": "urn:ngsi-ld:Wide:root", "type": "Wide",
            "links": {"type": "Relationship", "object": Value::Array(targets)}});
        let complete = inline_join(
            &st,
            &tenant,
            &ctx,
            &crate::repr::Repr::default(),
            &mut compacted,
            1,
        )
        .await;
        assert!(!complete, "the budget was hit, so the walk is incomplete");
        assert_eq!(
            compacted["links"]["entity"]
                .as_array()
                .expect("entity array")
                .len(),
            MAX_JOIN_LOOKUPS,
            "no more Linked Entities than the budget are resolved"
        );
    }
}

/// 5.6.6.4 / 5.6.17.4 / 5.6.18.4 / 5.6.21.4: "matching input data is
/// forwarded to the Registration endpoint" — the forward may narrow what the
/// peer does, never widen it.
#[cfg(test)]
mod forwarded_input_data {
    use super::*;

    fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    /// 5.6.21.3 lists the type selector, the id list, the id pattern, the
    /// restrictive and exclusionary Attribute-name lists, the query, the
    /// geoquery, the Scope query and the context source filter as Purge
    /// input data. Every one of them restricts the purge, so every one of
    /// them travels.
    #[test]
    fn purge_forward_carries_every_restriction_the_client_issued() {
        let p = params(&[
            ("type", "Vehicle"),
            ("id", "urn:ngsi-ld:Vehicle:A1"),
            ("idPattern", "^urn:ngsi-ld:Vehicle:"),
            ("attrs", "speed"),
            ("q", "speed>5"),
            ("georel", "near;maxDistance==100"),
            ("geometry", "Point"),
            ("coordinates", "[0,0]"),
            ("geoproperty", "location"),
            ("scopeQ", "/x"),
            ("csf", "name==p"),
            ("keep", "name"),
            ("local", "false"),
        ]);
        let q = forwarded_purge_query(&p);
        for k in [
            "type",
            "id",
            "idPattern",
            "attrs",
            "q",
            "georel",
            "geometry",
            "coordinates",
            "geoproperty",
            "scopeQ",
            "csf",
            "keep",
        ] {
            assert!(
                q.iter().any(|(a, _)| a == k),
                "{k} was dropped, so the peer executes a wider purge than the \
                 client issued: {q:?}"
            );
        }
        assert!(
            !q.iter().any(|(k, _)| k == "local"),
            "local scope is the reason a forward happens at all — it is not \
             itself forwarded (5.5.13): {q:?}"
        );
        assert_eq!(q.len(), 12, "nothing else is invented: {q:?}");
    }

    /// The exclusionary list travels on its own too — `drop=` alone must not
    /// reach the peer as a bare, entity-deleting purge.
    #[test]
    fn purge_forward_carries_drop_and_omits_absent_members() {
        let q = forwarded_purge_query(&params(&[("type", "Vehicle"), ("drop", "speed")]));
        assert!(
            q.contains(&("drop".to_owned(), "speed".to_owned())),
            "{q:?}"
        );
        assert_eq!(q.len(), 2, "absent parameters are not forwarded: {q:?}");
    }

    /// 5.6.6.3 / 5.6.17.3 / 5.6.18.3: the selector of Entity types is input
    /// data of Delete, Merge and Replace Entity. A registration may cover
    /// several types, so the peer needs the selector to reach the same
    /// verdict this broker reached locally.
    #[test]
    fn write_forwards_carry_the_type_selector() {
        assert_eq!(
            type_selector_query(&params(&[("type", "Vehicle"), ("local", "false")])),
            vec![("type".to_owned(), "Vehicle".to_owned())]
        );
        assert!(
            type_selector_query(&params(&[("local", "false")])).is_empty(),
            "no selector, nothing to forward"
        );
    }

    /// 5.7.2.4 applies q/geoquery/Scope query/Attributes only after remote
    /// parts have been aggregated, and 4.23 orders by the value of a member —
    /// neither survives a store-side projection of that member.
    #[test]
    fn projection_is_pushed_down_only_when_the_store_answer_is_final() {
        let plain = params(&[("type", "T"), ("pick", "name")]);
        assert!(
            proj_pushdown_allowed(true, &plain),
            "a purely local query keeps the pushdown"
        );
        assert!(
            !proj_pushdown_allowed(false, &plain),
            "federated candidates mean the filters run again after the merge"
        );
        assert!(
            !proj_pushdown_allowed(true, &params(&[("attrs", "name"), ("orderBy", "age")])),
            "the ordering member must survive to be compared"
        );
    }

    /// 6.3.10 makes `limit=0` legal only with `count=true` — an answer that is
    /// a count and no rows. That shape is pushed to the store; the filters the
    /// store cannot see (idPattern, 4.23 ordering, federated candidates merged
    /// per 5.7.2.4) still forfeit the page.
    #[test]
    fn the_count_only_page_is_pushed_down_but_store_blind_filters_are_not() {
        assert!(
            page_pushdown_allowed(
                true,
                &params(&[("type", "T"), ("limit", "0"), ("count", "true")])
            ),
            "a count is answered by counting, not by materializing the match set"
        );
        assert!(
            page_pushdown_allowed(true, &params(&[("type", "T"), ("limit", "10")])),
            "a plain local query keeps the pushdown"
        );
        assert!(
            !page_pushdown_allowed(true, &params(&[("type", "T"), ("idPattern", "^urn:")])),
            "idPattern is applied after the store, so it drops rows the SQL \
             page already counted"
        );
        assert!(
            !page_pushdown_allowed(true, &params(&[("type", "T"), ("orderBy", "speed")])),
            "4.23 orders the whole match set before the page is cut"
        );
        assert!(
            !page_pushdown_allowed(false, &params(&[("type", "T")])),
            "federated candidates are merged after the store answered"
        );
    }
}

/// 5.6.1 Create Entity and 5.6.21 Purge Entities, end to end over the store.
#[cfg(test)]
mod clause_5_6_1_and_5_6_21 {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use serde_json::json;
    use tower::ServiceExt;

    /// 5.6.1.5: the output is "the URI of the created Entity", returned in
    /// the Location header. An id is one path segment (RFC 3986 clause 3.3),
    /// so a `#` in it must not be able to end the segment.
    #[tokio::test]
    async fn location_header_percent_encodes_the_entity_id() {
        let app = crate::router(AppState::new("antares-test".into()));
        let id = "urn:ngsi-ld:Vehicle:A#4567";
        let payload = json!({"id": id, "type": "Vehicle"}).to_string();
        let resp = app
            .oneshot(
                Request::post("/ngsi-ld/v1/entities")
                    .header("Content-Type", "application/json")
                    .header("Content-Length", payload.len().to_string())
                    .body(Body::from(payload))
                    .expect("req"),
            )
            .await
            .expect("resp");
        assert_eq!(resp.status(), StatusCode::CREATED);
        let loc = resp
            .headers()
            .get("Location")
            .expect("Location header")
            .to_str()
            .expect("ascii");
        assert_eq!(loc, "/ngsi-ld/v1/entities/urn:ngsi-ld:Vehicle:A%234567");
        assert!(
            !loc.contains('#'),
            "a raw # truncates the URL at the fragment and addresses another \
             resource: {loc}"
        );
    }

    /// F-ANT2-013: 5.7.2 names Entities by a comma-separated `id` list, and
    /// the EntityMap, temporal and federation pages rebuild that list, so an
    /// id holding a comma is refused on every create door and nothing is
    /// stored.
    #[tokio::test]
    async fn an_entity_id_holding_a_comma_is_refused_at_create() {
        let app = crate::router(AppState::new("antares-test".into()));
        let id = "urn:ngsi-ld:Vehicle:a,b";
        let one = json!({"id": id, "type": "Vehicle"});
        let temporal = json!({"id": id, "type": "Vehicle",
            "speed": [{"type": "Property", "value": 1, "observedAt": "2026-10-26T00:00:00Z"}]});
        // a batch answers 207 with the member's 400 in `errors` (5.6.7)
        for (path, body, want) in [
            ("/ngsi-ld/v1/entities", one.clone(), StatusCode::BAD_REQUEST),
            (
                "/ngsi-ld/v1/entityOperations/create",
                json!([one.clone()]),
                StatusCode::MULTI_STATUS,
            ),
            (
                "/ngsi-ld/v1/entityOperations/upsert",
                json!([one.clone()]),
                StatusCode::MULTI_STATUS,
            ),
            (
                "/ngsi-ld/v1/temporal/entities",
                temporal,
                StatusCode::BAD_REQUEST,
            ),
        ] {
            let payload = body.to_string();
            let resp = app
                .clone()
                .oneshot(
                    Request::post(path)
                        .header("Content-Type", "application/json")
                        .header("Content-Length", payload.len().to_string())
                        .body(Body::from(payload))
                        .expect("req"),
                )
                .await
                .expect("resp");
            let status = resp.status();
            let bytes = resp.into_body().collect().await.expect("body").to_bytes();
            let text = String::from_utf8_lossy(&bytes);
            assert_eq!(status, want, "{path}: {text}");
            assert!(text.contains("holds a comma"), "{path}: {text}");
        }
        let resp = app
            .oneshot(
                Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:Vehicle:a,b")
                    .body(Body::empty())
                    .expect("req"),
            )
            .await
            .expect("resp");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    /// F-ANT2-013: an upsert refuses a comma only for an id it would create;
    /// an Entity stored under one before the rule is still upserted, so a
    /// source that keeps writing it does not turn red.
    #[tokio::test]
    async fn an_upsert_still_reaches_a_stored_comma_id() {
        let st = AppState::new("antares-test".into());
        let tenant = TenantId::default();
        let id = "urn:ngsi-ld:Vehicle:a,b";
        let vehicle = "https://uri.etsi.org/ngsi-ld/default-context/Vehicle";
        let doc = json!({"id": id, "type": [vehicle]});
        st.store
            .upsert(&tenant, Kind::Entity, id, doc.clone())
            .await
            .expect("seed entity");
        assert!(st
            .temporal
            .create(&tenant, id, doc)
            .await
            .expect("seed temporal"));
        let app = crate::router(st);
        let one = json!({"id": id, "type": "Vehicle",
            "speed": {"type": "Property", "value": 2}});
        let temporal = json!({"id": id, "type": "Vehicle",
            "speed": [{"type": "Property", "value": 2, "observedAt": "2026-10-26T00:00:00Z"}]});
        for (path, body) in [
            ("/ngsi-ld/v1/entityOperations/upsert", json!([one])),
            ("/ngsi-ld/v1/temporal/entities", temporal),
        ] {
            let payload = body.to_string();
            let resp = app
                .clone()
                .oneshot(
                    Request::post(path)
                        .header("Content-Type", "application/json")
                        .header("Content-Length", payload.len().to_string())
                        .body(Body::from(payload))
                        .expect("req"),
                )
                .await
                .expect("resp");
            let status = resp.status();
            let bytes = resp.into_body().collect().await.expect("body").to_bytes();
            assert_eq!(
                status,
                StatusCode::NO_CONTENT,
                "{path}: {}",
                String::from_utf8_lossy(&bytes)
            );
        }
    }

    async fn seed(st: &AppState, tenant: &TenantId, n: usize) {
        for i in 0..n {
            let id = format!("urn:ngsi-ld:Purge:{i:05}");
            let doc = json!({"id": &id,
                "type": ["https://uri.etsi.org/ngsi-ld/default-context/Purge"],
                "https://uri.etsi.org/ngsi-ld/default-context/name":
                    [{"type": "Property", "value": "n"}],
                "https://uri.etsi.org/ngsi-ld/default-context/speed":
                    [{"type": "Property", "value": 1}]});
            st.store
                .upsert(tenant, Kind::Entity, &id, doc)
                .await
                .expect("seed");
        }
    }

    fn purge_params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    /// 5.6.21.4: the implementation "shall delete all Entities that can be
    /// found locally using retrieved list of Entity ids" — all of them, not
    /// one page of them, however many rounds that takes.
    #[tokio::test]
    async fn purge_deletes_every_match_across_page_boundaries() {
        let st = AppState::new("antares-test".into());
        let tenant = TenantId::default();
        let n = PURGE_CHUNK * 2 + 7;
        seed(&st, &tenant, n).await;
        let resp = purge_inner(&st, &purge_params(&[("type", "Purge")]), &HeaderMap::new())
            .await
            .expect("purge");
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(
            st.store
                .list(&tenant, Kind::Entity)
                .await
                .expect("list")
                .is_empty(),
            "entities survived the purge"
        );
    }

    /// 5.6.21.4: with an exclusionary list the implementation "shall delete
    /// all but the given set of Attributes" — the Entities themselves
    /// survive, again for the whole match set and not just its first page.
    #[tokio::test]
    async fn purge_with_keep_prunes_every_match_and_deletes_no_entity() {
        let st = AppState::new("antares-test".into());
        let tenant = TenantId::default();
        let n = PURGE_CHUNK * 2 + 7;
        seed(&st, &tenant, n).await;
        let resp = purge_inner(
            &st,
            &purge_params(&[("type", "Purge"), ("keep", "name")]),
            &HeaderMap::new(),
        )
        .await
        .expect("purge");
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let left = st.store.list(&tenant, Kind::Entity).await.expect("list");
        assert_eq!(left.len(), n, "keep= prunes attributes, not entities");
        for doc in &left {
            assert!(
                doc.get("https://uri.etsi.org/ngsi-ld/default-context/name")
                    .is_some(),
                "the kept attribute is still there: {doc}"
            );
            assert!(
                doc.get("https://uri.etsi.org/ngsi-ld/default-context/speed")
                    .is_none(),
                "every other attribute is gone: {doc}"
            );
        }
    }

    /// 5.6.21.4: "id matches the id pattern passed as a parameter" — the
    /// pattern narrows the match set, and only that set is deleted.
    #[tokio::test]
    async fn purge_deletes_the_id_pattern_matches_and_nothing_else() {
        let st = AppState::new("antares-test".into());
        let tenant = TenantId::default();
        seed(&st, &tenant, 30).await;
        let resp = purge_inner(
            &st,
            &purge_params(&[("type", "Purge"), ("idPattern", "^urn:ngsi-ld:Purge:0000")]),
            &HeaderMap::new(),
        )
        .await
        .expect("purge");
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let left: Vec<String> = st
            .store
            .list(&tenant, Kind::Entity)
            .await
            .expect("list")
            .iter()
            .filter_map(|d| d["id"].as_str().map(str::to_owned))
            .collect();
        assert_eq!(
            left.len(),
            20,
            "only the ten pattern matches went: {left:?}"
        );
        assert!(
            !left
                .iter()
                .any(|id| id.starts_with("urn:ngsi-ld:Purge:0000")),
            "a pattern match survived: {left:?}"
        );
        assert!(
            left.contains(&"urn:ngsi-ld:Purge:00010".to_owned()),
            "an Entity the pattern does not match must not be purged: {left:?}"
        );
    }

    /// 5.6.21.4 deletes "all Entities that can be found locally using
    /// retrieved list of Entity ids". The retrieval is chunked, and idPattern
    /// is applied after the store — so a chunk can come back narrowed, or
    /// empty, while matches still wait behind it. The walk continues on the
    /// store's row count, never on the surviving subset.
    #[test]
    fn purge_walks_on_the_rows_the_store_returned_not_the_narrowed_subset() {
        assert_eq!(
            next_scan_offset(0, 500, 0, true, 500),
            Some(500),
            "a full chunk the pattern narrowed to nothing still has a successor"
        );
        assert_eq!(
            next_scan_offset(500, 500, 120, true, 500),
            Some(880),
            "the 380 rows the round did not remove are stepped over"
        );
        assert_ne!(
            next_scan_offset(0, 500, 0, true, 500),
            None,
            "stopping here leaves every match behind the first chunk alive"
        );
    }

    /// 5.6.21.4 against a store that really pages (memory answers every query
    /// with the whole match set, so it cannot exercise the walk): with an
    /// idPattern spread across the match set, every chunk arrives narrowed —
    /// and the purge still has to delete "all Entities that can be found
    /// locally using retrieved list of Entity ids", not just the first chunk's
    /// share. Skips without ANTARES_TEST_DATABASE_URL.
    #[cfg(feature = "postgres")]
    #[tokio::test(flavor = "multi_thread")]
    async fn purge_over_a_paging_store_deletes_every_id_pattern_match() {
        let url = match std::env::var("ANTARES_TEST_DATABASE_URL") {
            Ok(u) => u,
            Err(_) => {
                eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
                return;
            }
        };
        let pool = antares_sql::store::pg::connect(&url, 5)
            .await
            .expect("connect");
        let tenant = TenantId::new("purgepaging").expect("tenant");
        antares_sql::store::pg::ensure_tenant(&pool, &tenant)
            .await
            .expect("tenant row");
        let st = AppState::with_store(
            "antares-test".into(),
            std::sync::Arc::new(antares_sql::store::any::AnyStore::Pg(
                antares_sql::store::any::PgBackend::new(pool),
            )),
            "postgres",
        );
        for doc in st.store.list(&tenant, Kind::Entity).await.expect("list") {
            if let Some(id) = doc["id"].as_str() {
                st.store
                    .delete(&tenant, Kind::Entity, id)
                    .await
                    .expect("clean");
            }
        }
        // more than two chunks, and the pattern matches every second id — so
        // no chunk the store pages is full once the pattern filtered it
        let n = PURGE_CHUNK * 2 + 200;
        seed(&st, &tenant, n).await;
        // the purge must run AS the seeded tenant — with no NGSILD-Tenant
        // header it correctly purges the default tenant's (empty) match set
        // and every seeded row survives (5.5.10)
        let mut headers = HeaderMap::new();
        headers.insert("NGSILD-Tenant", "purgepaging".parse().expect("header"));
        let resp = purge_inner(
            &st,
            &purge_params(&[("type", "Purge"), ("idPattern", "[02468]$")]),
            &headers,
        )
        .await
        .expect("purge");
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let left: Vec<String> = st
            .store
            .list(&tenant, Kind::Entity)
            .await
            .expect("list")
            .iter()
            .filter_map(|d| d["id"].as_str().map(str::to_owned))
            .collect();
        assert!(
            !left
                .iter()
                .any(|id| id.ends_with(['0', '2', '4', '6', '8'])),
            "{} of {} pattern matches survived the chunked walk",
            left.iter()
                .filter(|id| id.ends_with(['0', '2', '4', '6', '8']))
                .count(),
            n / 2
        );
        assert_eq!(
            left.len(),
            n / 2,
            "an Entity the pattern does not match must not be purged"
        );
        for id in &left {
            st.store
                .delete(&tenant, Kind::Entity, id)
                .await
                .expect("clean");
        }
    }

    /// The same walk terminates: a chunk the store did not page IS the whole
    /// match set, a short chunk is the last one, and a round that deleted its
    /// whole chunk re-reads the window the deletions shifted down.
    #[test]
    fn purge_stops_when_the_match_set_is_exhausted() {
        assert_eq!(
            next_scan_offset(0, 500, 500, false, 500),
            None,
            "an unpaged answer is the whole match set"
        );
        assert_eq!(
            next_scan_offset(0, 7, 7, true, 500),
            None,
            "a chunk shorter than the page size is the last one"
        );
        assert_eq!(
            next_scan_offset(0, 500, 500, true, 500),
            Some(0),
            "everything deleted — the rest of the match set shifted to the front"
        );
    }
}

/// 5.6.17 Merge Entity and the Linked Entity Retrieval parameters of
/// Table 6.4.3.2-1, over the HTTP surface.
#[cfg(test)]
mod clause_5_6_17_and_6_4_3_2 {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use serde_json::json;
    use tower::ServiceExt;

    fn app() -> axum::Router {
        crate::router(AppState::new("antares-test".into()))
    }

    async fn create(app: &axum::Router, body: Value) {
        let payload = body.to_string();
        let resp = app
            .clone()
            .oneshot(
                Request::post("/ngsi-ld/v1/entities")
                    .header("Content-Type", "application/json")
                    .header("Content-Length", payload.len().to_string())
                    .body(Body::from(payload))
                    .expect("req"),
            )
            .await
            .expect("resp");
        assert_eq!(resp.status(), StatusCode::CREATED, "create failed");
    }

    async fn patch(app: &axum::Router, uri: &str, body: Value) -> StatusCode {
        let payload = body.to_string();
        app.clone()
            .oneshot(
                Request::patch(uri)
                    .header("Content-Type", "application/json")
                    .header("Content-Length", payload.len().to_string())
                    .body(Body::from(payload))
                    .expect("req"),
            )
            .await
            .expect("resp")
            .status()
    }

    async fn get(app: &axum::Router, uri: &str) -> Value {
        let resp = app
            .clone()
            .oneshot(Request::get(uri).body(Body::empty()).expect("req"))
            .await
            .expect("resp");
        let bytes = resp.into_body().collect().await.expect("body").to_bytes();
        serde_json::from_slice(&bytes).expect("json")
    }

    /// 5.6.17.3: "An optional parameter indicating a common observedAt
    /// timestamp to use across merged Attributes." It is the timestamp of
    /// every Attribute this merge touches; an Attribute the Fragment gives an
    /// observedAt of its own keeps that one, and an Attribute the Fragment
    /// does not mention is not a merged Attribute and is left alone.
    #[tokio::test]
    async fn merge_applies_the_common_observed_at() {
        let app = app();
        let id = "urn:ngsi-ld:Obs:1";
        create(
            &app,
            json!({"id": id, "type": "T",
                   "a": {"type": "Property", "value": 1},
                   "b": {"type": "Property", "value": 1},
                   "untouched": {"type": "Property", "value": 1,
                                 "observedAt": "2020-01-01T00:00:00Z"}}),
        )
        .await;
        let uri = format!("/ngsi-ld/v1/entities/{id}?observedAt=2026-08-17T10:00:00Z");
        assert_eq!(
            patch(
                &app,
                &uri,
                json!({"a": {"value": 2},
                       "b": {"value": 2, "observedAt": "2021-01-01T00:00:00Z"}}),
            )
            .await,
            StatusCode::NO_CONTENT
        );
        let body = get(&app, &format!("/ngsi-ld/v1/entities/{id}")).await;
        assert_eq!(
            body["a"]["observedAt"], "2026-08-17T10:00:00Z",
            "the merged Attribute takes the common timestamp: {body}"
        );
        assert_eq!(
            body["b"]["observedAt"], "2021-01-01T00:00:00Z",
            "a Fragment instance carrying its own observedAt keeps it: {body}"
        );
        assert_eq!(
            body["untouched"]["observedAt"], "2020-01-01T00:00:00Z",
            "an Attribute this merge does not touch is not restamped: {body}"
        );
    }

    /// 4.8 makes observedAt a DateTime, so a value that is not one cannot be
    /// stamped across the merged Attributes.
    #[tokio::test]
    async fn merge_rejects_an_observed_at_that_is_not_a_datetime() {
        let app = app();
        let id = "urn:ngsi-ld:Obs:2";
        create(&app, json!({"id": id, "type": "T"})).await;
        assert_eq!(
            patch(
                &app,
                &format!("/ngsi-ld/v1/entities/{id}?observedAt=yesterday"),
                json!({"a": {"type": "Property", "value": 1}}),
            )
            .await,
            StatusCode::BAD_REQUEST
        );
    }

    /// 5.6.17.4: "If a common language tag is defined and a LanguageProperty
    /// Attribute to be merged is represented as a string, the pre-existing
    /// languageMap JSON object shall be preserved. The string value shall
    /// only replace the value associated to the language tag key found
    /// within the languageMap."
    #[tokio::test]
    async fn merge_with_a_common_lang_replaces_only_that_language_key() {
        let app = app();
        let id = "urn:ngsi-ld:Lang:1";
        create(
            &app,
            json!({"id": id, "type": "T",
                   "greeting": {"type": "LanguageProperty",
                                "languageMap": {"en": "hello", "es": "adios"}}}),
        )
        .await;
        assert_eq!(
            patch(
                &app,
                &format!("/ngsi-ld/v1/entities/{id}?lang=es"),
                json!({"greeting": "hola"}),
            )
            .await,
            StatusCode::NO_CONTENT
        );
        let body = get(&app, &format!("/ngsi-ld/v1/entities/{id}")).await;
        assert_eq!(
            body["greeting"]["languageMap"],
            json!({"en": "hello", "es": "hola"}),
            "the pre-existing languageMap survives, only the tagged key moves: {body}"
        );
        assert!(
            body["greeting"].get("value").is_none(),
            "the string never lands as a plain Property value: {body}"
        );
        assert_eq!(body["greeting"]["type"], "LanguageProperty", "{body}");
    }

    /// Table 6.4.3.2-1 containedBy: "List of entity ids which have previously
    /// been encountered whilst retrieving the Entity Graph" — 4.5.23.1 keeps
    /// the walk from retrieving them a second time.
    #[tokio::test]
    async fn contained_by_ids_are_not_retrieved_again() {
        let app = app();
        let (root, held, fresh) = (
            "urn:ngsi-ld:Graph:root",
            "urn:ngsi-ld:Graph:held",
            "urn:ngsi-ld:Graph:fresh",
        );
        create(&app, json!({"id": held, "type": "G"})).await;
        create(&app, json!({"id": fresh, "type": "G"})).await;
        create(
            &app,
            json!({"id": root, "type": "G",
                   "toHeld": {"type": "Relationship", "object": held},
                   "toFresh": {"type": "Relationship", "object": fresh}}),
        )
        .await;

        let body = get(
            &app,
            &format!("/ngsi-ld/v1/entities/{root}?join=flat&joinLevel=2&containedBy={held}"),
        )
        .await;
        let arr = match body {
            Value::Array(a) => a,
            other => vec![other],
        };
        let ids: Vec<&str> = arr.iter().filter_map(|e| e["id"].as_str()).collect();
        assert!(
            !ids.contains(&held),
            "an id the client already holds is not retrieved again: {ids:?}"
        );
        assert!(
            ids.contains(&fresh),
            "the Linked Entity it does not hold is still returned: {ids:?}"
        );
        assert!(ids.contains(&root), "the Linking Entity is there: {ids:?}");
    }
}

#[cfg(test)]
mod clause_4_8_system_attributes {
    use super::*;
    use crate::stamp::stamp_instances;
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::json;
    use tower::ServiceExt;

    /// 4.8 stamps a sub-Attribute because "a sub-Property is a Property".
    /// The members an Attribute instance carries under 4.5 are NOT
    /// sub-Attributes, so none of them may be descended into and stamped —
    /// several of them (`previousJson` and `previousVocab` hold uninterpreted
    /// JSON per 4.5.20/4.5.21, `entityList` holds Linked Entities) can be an
    /// array of objects, which is exactly the shape the walk mistakes for an
    /// instance array. The list the expander keeps verbatim is the one list;
    /// a second copy is what drifts.
    #[test]
    fn no_reserved_instance_member_is_walked_as_a_sub_attribute() {
        let carrier = json!([{"probe": "untouched"}]);
        for member in antares_jsonld::RESERVED_MEMBERS {
            // the two the stamp itself writes; every other member is the
            // instance's own and is left exactly as it was found
            if matches!(*member, "createdAt" | "modifiedAt") {
                continue;
            }
            let mut inst = json!({"type": "Property", "value": 1});
            inst[*member] = carrier.clone();
            let mut attr = json!([inst]);
            stamp_instances(&mut attr, "2020-01-01T00:00:00Z");
            let held = attr[0].get(*member).expect("the member survives");
            assert_eq!(
                held, &carrier,
                "{member} was walked as a sub-Attribute and stamped"
            );
        }
    }

    /// 4.8: createdAt is "the temporal Property at which the Entity,
    /// Property or Relationship was entered into an NGSI-LD system" and
    /// modifiedAt the one at which it "was last modified". Both are
    /// generated by the system, at every level: an Entity, an Attribute
    /// instance and a sub-Attribute (a sub-Property is a Property). A client
    /// that could set them would rewrite the provenance of its own data, and
    /// a subscriber filtering on modifiedAt would never see the write.
    #[tokio::test]
    async fn the_client_cannot_write_its_own_created_and_modified_stamps() {
        let st = AppState::new("antares-sysattrs".into());
        let forged = "1970-01-01T00:00:00Z";
        let payload = json!({
            "id": "urn:ngsi-ld:Vehicle:stamped",
            "type": "Vehicle",
            "createdAt": forged,
            "modifiedAt": forged,
            "speed": {
                "type": "Property",
                "value": 10,
                "createdAt": forged,
                "modifiedAt": forged,
                "accuracy": {"type": "Property", "value": 1,
                             "createdAt": forged, "modifiedAt": forged},
            },
        })
        .to_string();
        let resp = crate::router(st.clone())
            .oneshot(
                Request::post("/ngsi-ld/v1/entities")
                    .header("Content-Type", "application/json")
                    .header("Content-Length", payload.len().to_string())
                    .body(Body::from(payload))
                    .expect("req"),
            )
            .await
            .expect("resp");
        assert_eq!(resp.status(), StatusCode::CREATED);

        let resp = crate::router(st)
            .oneshot(
                Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:Vehicle:stamped?options=sysAttrs")
                    .body(Body::empty())
                    .expect("req"),
            )
            .await
            .expect("resp");
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body");
        let served = String::from_utf8_lossy(&body);
        assert!(
            served.contains("createdAt") && served.contains("modifiedAt"),
            "sysAttrs serves the stamps: {served}"
        );
        assert!(
            !served.contains(forged),
            "no level of the document may carry the stamp the client sent: {served}"
        );
    }

    /// 4.8 again, on every route that writes an Attribute. Create is the
    /// obvious way in; a client that is refused there and accepted on Append,
    /// Merge, Partial Update or Replace has the same forged provenance one
    /// request later. Each of the four carries the stamps at Entity,
    /// Attribute and sub-Attribute level.
    #[tokio::test]
    async fn no_write_route_lets_the_client_stamp_its_own_attributes() {
        let st = AppState::new("antares-sysattrs-writes".into());
        let forged = "1970-01-01T00:00:00Z";
        let id = "urn:ngsi-ld:Vehicle:writes";
        let stamped = |v: Value| {
            let mut o = v;
            if let Some(m) = o.as_object_mut() {
                m.insert("createdAt".into(), json!(forged));
                m.insert("modifiedAt".into(), json!(forged));
            }
            o
        };
        let send = |req: Request<Body>| {
            let st = st.clone();
            async move { crate::router(st).oneshot(req).await.expect("resp") }
        };

        let seed = json!({"id": id, "type": "Vehicle"}).to_string();
        let resp = send(
            Request::post("/ngsi-ld/v1/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", seed.len().to_string())
                .body(Body::from(seed))
                .expect("req"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        let attr = || {
            stamped(json!({
                "type": "Property",
                "value": 10,
                "accuracy": stamped(json!({"type": "Property", "value": 1})),
            }))
        };
        // Append (5.6.3), Partial Update (5.6.4), Merge (5.6.17), Replace
        // (5.6.16) — the whole write surface that reaches expand_entity.
        let calls: Vec<(&str, String, Value)> = vec![
            (
                "POST",
                format!("/ngsi-ld/v1/entities/{id}/attrs"),
                stamped(json!({"speed": attr()})),
            ),
            (
                "PATCH",
                format!("/ngsi-ld/v1/entities/{id}/attrs/speed"),
                attr(),
            ),
            (
                "PATCH",
                format!("/ngsi-ld/v1/entities/{id}"),
                stamped(json!({"speed": attr()})),
            ),
            (
                "PUT",
                format!("/ngsi-ld/v1/entities/{id}"),
                stamped(json!({"id": id, "type": "Vehicle", "speed": attr()})),
            ),
        ];
        for (method, path, payload) in calls {
            let body = payload.to_string();
            let resp = send(
                Request::builder()
                    .method(method)
                    .uri(&path)
                    .header("Content-Type", "application/json")
                    .header("Content-Length", body.len().to_string())
                    .body(Body::from(body))
                    .expect("req"),
            )
            .await;
            assert!(
                resp.status().is_success(),
                "{method} {path} answered {}",
                resp.status()
            );
            let resp = send(
                Request::get(format!("/ngsi-ld/v1/entities/{id}?options=sysAttrs"))
                    .body(Body::empty())
                    .expect("req"),
            )
            .await;
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .expect("body");
            let served = String::from_utf8_lossy(&bytes);
            assert!(
                !served.contains(forged),
                "{method} {path} let the client stamp the document: {served}"
            );
        }
    }
}

#[cfg(test)]
mod judged_types_tests {
    use super::judged_types;
    use crate::policy::{Filter, Rule};
    use std::borrow::Cow;

    fn rules(types: &[&[&str]]) -> Filter {
        Filter {
            rules: types
                .iter()
                .map(|t| Rule {
                    types: t.iter().map(|s| (*s).to_owned()).collect(),
                    ..Rule::default()
                })
                .collect(),
            ..Filter::default()
        }
    }

    fn sel(groups: &[&[&str]]) -> Vec<Vec<String>> {
        groups
            .iter()
            .map(|g| g.iter().map(|s| (*s).to_owned()).collect())
            .collect()
    }

    #[test]
    fn a_selection_naming_a_rule_type_in_every_group_is_kept_as_is() {
        let client = sel(&[&["a", "T"], &["U"]]);
        let got = judged_types(Some(&client), &rules(&[&["T"], &["U"]]));
        assert!(matches!(got, Some(Cow::Borrowed(g)) if g == client.as_slice()));
    }

    #[test]
    fn a_group_naming_no_rule_type_takes_each_in_turn() {
        let client = sel(&[&["a", "b"], &["T"]]);
        let got = judged_types(Some(&client), &rules(&[&["U", "T"], &["T"]]));
        assert_eq!(
            got.as_deref(),
            Some(sel(&[&["a", "b", "T"], &["a", "b", "U"], &["T"]]).as_slice())
        );
    }

    #[test]
    fn no_client_selection_selects_the_rule_types() {
        let got = judged_types(None, &rules(&[&["T"], &["U"]]));
        assert_eq!(got.as_deref(), Some(sel(&[&["T"], &["U"]]).as_slice()));
    }

    #[test]
    fn rules_without_types_judge_nothing() {
        assert!(judged_types(None, &rules(&[&[]])).is_none());
        assert!(judged_types(Some(&sel(&[&["a"]])), &Filter::default()).is_none());
    }
}
