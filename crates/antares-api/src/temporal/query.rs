// SPDX-License-Identifier: EUPL-1.2
//! Query temporal evolution (5.7.4) and the batch temporal query (6.24).

use super::*;

// ---------- GET /temporal/entities/ (5.7.4) ----------

pub async fn query_temporal(
    State(st): State<AppState>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
) -> Response {
    match query_temporal_outer(&st, params, &headers).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

/// 5.7.4.4 EntityMap usage on the temporal query: a live map referenced by
/// the NGSILD-EntityMap header fixes the result set to the map's Entities
/// (5.5.14) and its location is echoed; an unknown or expired reference
/// means "a new EntityMap shall be created" (the entityMap=true branch,
/// answering 201 + the fresh location).
pub(super) async fn query_temporal_outer(
    st: &AppState,
    mut params: HashMap<String, String>,
    headers: &HeaderMap,
) -> ApiResult<Response> {
    let tenant = tenant_from(headers)?;
    let named = crate::entities::ReadMembers::of(st, headers, &params).await;
    let filter = gate!(
        st, &tenant, headers, "5.7.4",
        scope_q: params.get("scopeQ").map(String::as_str),
        q: named.q.as_ref(),
        types: &named.types,
        attrs: &named.attrs,
        geo: named.geo.as_ref(),
    )
    .await?;
    let Some(map_ref) = single_header(headers, "NGSILD-EntityMap")? else {
        return query_temporal_inner(st, &params, headers, &filter).await;
    };
    let map_id = map_ref.rsplit('/').next().unwrap_or(&map_ref).to_owned();
    let Some(mut map) = crate::entity_map::map_if_accessible(st, &tenant, headers, &map_id).await
    else {
        params.insert("entityMap".into(), "true".into());
        return query_temporal_inner(st, &params, headers, &filter).await;
    };
    params.remove("entityMap");
    // 5.5.9.3: the map fixes the candidate set and the request's own filters
    // narrow it, so `id=` on this request selects from the map rather than
    // replacing it.
    let candidates = crate::entity_map::candidate_ids(&map, &params);
    // "filters shall be rechecked before returning results" and "Entities not
    // or no longer fitting the query shall be removed from the Entity map
    // during pagination" — so the recheck asks about the map's OWN Entities,
    // in bounded chunks. Asking the whole Tenant instead judged, and then
    // deleted, entries this request never asked about, and lost every
    // candidate past the first page of the recheck. Pruning is judgeable only
    // for "@none" (local) entries: a remote-backed id may merely have an
    // unreachable source right now (5.5.14). Known cost: this recheck is a
    // second temporal query per map-using request, same shape as the entity
    // query's filter re-run.
    let mut matching: std::collections::HashSet<String> = std::collections::HashSet::new();
    for chunk in candidates.chunks(st.max_limit.max(1)) {
        let mut eff = params.clone();
        for k in ["limit", "offset", "count"] {
            eff.remove(k);
        }
        eff.insert("limit".into(), st.max_limit.to_string());
        eff.insert("id".into(), chunk.join(","));
        let resp = query_temporal_inner(st, &eff, headers, &filter).await?;
        let bytes = st.read_answer(resp.into_body(), "temporal").await?;
        matching.extend(
            serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default()
                .iter()
                .filter_map(|d| d.get("id").and_then(Value::as_str).map(str::to_owned)),
        );
    }
    let mut stale: Vec<String> = Vec::new();
    if let Some(emap) = map.get_mut("entityMap").and_then(Value::as_object_mut) {
        stale = candidates
            .iter()
            .filter(|eid| {
                emap.get(eid.as_str())
                    .and_then(Value::as_array)
                    .is_some_and(|a| matches!(a.as_slice(), [only] if only == "@none"))
                    && !matching.contains(eid.as_str())
            })
            .cloned()
            .collect();
        for k in &stale {
            emap.remove(k);
        }
    }
    crate::entity_map::map_prune(st, &tenant, &map_id, &stale).await?;
    // fix the query to the candidates that survived the recheck (5.5.14)
    let ids: Vec<&str> = candidates
        .iter()
        .filter(|id| {
            map.get("entityMap")
                .and_then(|m| m.get(id.as_str()))
                .is_some()
        })
        .map(String::as_str)
        .collect();
    params.insert(
        "id".into(),
        if ids.is_empty() {
            "urn:ngsi-ld:entitymap:empty".to_owned()
        } else {
            ids.join(",")
        },
    );
    let mut resp = query_temporal_inner(st, &params, headers, &filter).await?;
    if let Ok(v) = format!("/ngsi-ld/v1/entityMaps/{map_id}").parse() {
        resp.headers_mut().insert("NGSILD-EntityMap", v);
    }
    Ok(resp)
}

/// The 5.7.4.4 preconditions of a temporal query, before any store is
/// touched: the filter must qualify, Linked Entity conditions are not
/// defined for it, a context source filter must parse, and ordering may
/// name only `id` and only where the execution stays local (4.23.1).
pub(super) async fn check_temporal_query(
    st: &AppState,
    params: &HashMap<String, String>,
    headers: &HeaderMap,
    tenant: &TenantId,
    ctx: &Context,
    q_ast: Option<&antares_ql::QNode>,
) -> ApiResult<()> {
    let attrs_qualify = params.get("attrs").is_some_and(|a| {
        a.split(',')
            .any(|n| antares_ql::is_non_system_attr(n.trim()))
    });
    let q_qualifies = q_ast.is_some_and(|ast| {
        ast.attribute_paths()
            .iter()
            .any(|h| antares_ql::is_non_system_attr(h))
    });
    let has_filter = params.contains_key("type")
        || attrs_qualify
        || q_qualifies
        || params.contains_key("georel")
        || params.get("local").map(String::as_str) == Some("true");
    if !has_filter {
        return Err(NgsiError::BadRequestData(
            "temporal query needs at least one of type, attrs, q, georel (5.7.4)".into(),
        )
        .into());
    }
    // 5.7.4.4: Linked Entity retrieval is not defined for temporal queries —
    // linked filter conditions are an unconditional BadRequestData
    if q_ast.map(antares_ql::QNode::max_link_depth).unwrap_or(0) > 0 {
        return Err(NgsiError::BadRequestData(
            "temporal q must not reference Linked Entity attributes (5.7.4.4)".into(),
        )
        .into());
    }
    // 5.7.4.4: a syntactically invalid context source filter is 400; the
    // filter itself gates registrations in federation::reg_matches.
    if let Some(csf) = params.get("csf") {
        parse_q(csf)?;
    }
    crate::paging::check_collation(params)?;
    // 5.7.4.4: temporal ordering may only refer to the "id" entity member,
    // and only where the execution "is limited to the local scope (see
    // clause 5.5.13)" — 4.23.1 gives the reason: "Sort ordering is never
    // applied to distributed operations." The subject is the EXECUTION, so a
    // query nothing would federate to orders without `local=true`.
    if let Some(spec) = params.get("orderBy") {
        if crate::federation::would_federate(st, tenant, ctx, params, headers).await? {
            return Err(NgsiError::BadRequestData(
                "orderBy requires local scope — ordering is never applied to \
                 distributed operations (5.7.4.4, 4.23.1)"
                    .into(),
            )
            .into());
        }
        let non_id = spec.split(',').any(|part| {
            let m = part.trim().split(';').next().unwrap_or("").trim();
            m.split('[').next().unwrap_or(m) != "id"
        });
        if non_id {
            return Err(NgsiError::BadRequestData(
                "temporal orderBy may only name \"id\" (5.7.4.4)".into(),
            )
            .into());
        }
    }
    Ok(())
}

/// 4.5.19: the aggregated answer of a temporal query. The store computed
/// the bucket matrix, so the page is presented with the core members the
/// instance path renders and the aggregated attribute objects copied
/// under their compacted names.
#[allow(clippy::too_many_arguments)]
pub(super) fn respond_aggregated(
    st: &AppState,
    params: &HashMap<String, String>,
    tenant: &TenantId,
    ctx: &Context,
    accept: Accept,
    trepr: &TRepr,
    tq: Option<&TemporalQ>,
    attrs_filter: Option<&Vec<String>>,
    outcome: antares_store::filter::TemporalOutcome,
) -> ApiResult<Response> {
    let total = outcome
        .total
        .map(|t| t as usize)
        .unwrap_or(outcome.rows.len());
    let (page, count_hdr, links) = crate::paging::paginate_pre(
        st,
        params,
        outcome.rows,
        "/ngsi-ld/v1/temporal/entities",
        total,
    )?;
    let timeprop = tq.map_or("observedAt", |t| t.timeproperty.as_str());
    let none = Windowed {
        attrs: Default::default(),
        max_per_attr: 0,
        ts_min: None,
        ts_max: None,
        truncated: false,
        cut_served: None,
        more: false,
        held: Default::default(),
    };
    let mut payload: Vec<Value> = Vec::new();
    for d in &page {
        // core members exactly as the instance path presents them; the
        // aggregated attribute objects are copied under compacted names
        let mut presented = present_temporal(d, &none, ctx, trepr, tq, timeprop)?;
        let Some(out) = presented.as_object_mut() else {
            continue;
        };
        let mut any = false;
        for (k, v) in d.as_object().into_iter().flatten() {
            if is_meta(k) || attrs_filter.is_some_and(|a| !a.contains(k)) {
                continue;
            }
            out.insert(ctx.compact_iri(k), v.clone());
            any = true;
        }
        if any {
            payload.push(presented);
        }
    }
    let mut resp = crate::negotiate::respond_list(StatusCode::OK, payload, ctx, accept, tenant);
    attach_paging(&mut resp, count_hdr, &links);
    Ok(resp)
}

/// The two Table 5.2.33-1 selectors that need no compiled pattern: the
/// type set, and the `attrs` list, which excludes an entity carrying
/// none of the named Attributes. Each drops an entity the store could
/// not narrow away itself.
pub(super) fn type_and_attrs_match(
    doc: &Value,
    types: Option<&Vec<String>>,
    entity_attr_filter: Option<&Vec<String>>,
) -> bool {
    if let Some(types) = types {
        let etypes = doc["type"].as_array().cloned().unwrap_or_default();
        if !etypes
            .iter()
            .any(|t| types.iter().any(|w| Some(w.as_str()) == t.as_str()))
        {
            return false;
        }
    }
    if let Some(want) = entity_attr_filter {
        if !want.iter().any(|a| doc.get(a).is_some()) {
            return false;
        }
    }
    true
}

/// Whether [`member_now`] may judge the Entity on its held history: a policy
/// narrowing by `q`, `scopeQ` or an area reads Attributes the request may not
/// name, so a read for it keeps every Attribute (T-6346).
pub(super) fn reach_reads_state(filter: &crate::policy::Filter) -> bool {
    crate::repr::narrowing(filter)
        .policy_reach
        .is_some_and(|r| r.q.is_some() || r.scope_q.is_some() || !r.areas.is_empty())
}
/// ADR-0023 rule 8, for every narrowing: a history is served while the
/// narrowing's own predicates (its `q`, `scopeQ` and areas) hold for the
/// Entity as it is now, so an Entity that left the narrowing takes its
/// history with it. Its rules' conditions are not read here: rule 11 reads
/// them per instance. An Entity held only temporally (11.2.2) has no state
/// now, and its history `held` is the only state it has: the predicates
/// hold when some instance satisfies them, as the query's window check
/// reads them; `held` absent leaves that to the caller's own check.
pub(super) async fn member_now(
    st: &AppState,
    tenant: &TenantId,
    ctx: &Context,
    filter: &crate::policy::Filter,
    id: &str,
    held: Option<&Value>,
) -> ApiResult<bool> {
    if !reach_reads_state(filter) {
        return Ok(true);
    }
    let Some(reach) = crate::repr::narrowing(filter).policy_reach else {
        return Ok(true);
    };
    let reach = crate::repr::PolicyReach {
        rules: None,
        ..(*reach).clone()
    };
    match st
        .store
        .get(tenant, antares_store::Kind::Entity, id)
        .await?
    {
        Some(doc) => Ok(crate::repr::policy_admits(st, tenant, ctx, Some(&reach), &doc).await),
        None => Ok(held.is_none_or(|h| {
            let areas: Vec<&antares_ql::geo::GeoQuery> = reach.areas.iter().collect();
            window_conditions_match(h, None, reach.q.as_ref(), &areas, ctx)
                && reach
                    .scope_q
                    .as_deref()
                    .is_none_or(|sq| scope_window_retain(&mut h.clone(), sq, None, "observedAt"))
        })),
    }
}

pub(crate) async fn query_temporal_inner(
    st: &AppState,
    params: &HashMap<String, String>,
    headers: &HeaderMap,
    filter: &crate::policy::Filter,
) -> ApiResult<Response> {
    query_temporal_collected(st, params, headers, filter, None).await
}

/// The same query, with the merged documents of each page handed to `merged`
/// as well as rendered into the response. 5.16.1.4 fills a Snapshot from this
/// query and has to STORE what it matched: for an Evolution held by a
/// registered Context Source there is nothing else in this broker to store —
/// the fan-out's answer is the only copy, and the response body is a
/// presentation of it that compaction does not undo.
pub(crate) async fn query_temporal_collected(
    st: &AppState,
    params: &HashMap<String, String>,
    headers: &HeaderMap,
    filter: &crate::policy::Filter,
    merged: Option<&mut Vec<Value>>,
) -> ApiResult<Response> {
    let tenant = tenant_from(headers)?;
    check_params(params, QUERY_PARAMS)?;
    let accept = parse_accept(headers)?;
    let ctx = request_context(&st.loader, headers).await?;
    // 5.7.4.4 a-e: id/idPattern alone are NOT sufficient, and the attrs
    // list / q must include at least one non-system Attribute to qualify.
    let q_ast = params.get("q").map(|q| parse_q(q)).transpose()?;
    let q_ast = q_ast.map(|ast| expand_q_values(ast, params, &ctx));
    check_temporal_query(st, params, headers, &tenant, &ctx, q_ast.as_ref()).await?;
    // ADR-0020: the engine's narrowing joins the request's own filters after
    // 5.7.4.4's "too wide" judgement, which is about what the client asked
    // for. Everything below reads the narrowed query, the forward included.
    // ADR-0023 rule 11: the rules, and the client's own q apart from the
    // engine's, read on each Entity as its rules show it
    let types_rule = crate::repr::type_rules(filter);
    let client_q = if types_rule.is_some() {
        crate::repr::client_q(params, &ctx)?
    } else {
        None
    };
    let narrowed = filter.narrow_window(&filter.narrow_params(params, &ctx)?, true)?;
    let req = QueryReq {
        st,
        tenant: &tenant,
        headers,
        ctx: &ctx,
        params: &narrowed,
        filter,
    };
    let qry = Parsed::new(&req, types_rule, client_q)?;
    let (geos, shown_geos) = qry.geos(filter);
    let plan = Pushdown::plan(&req, &qry, geos.is_empty()).await?;
    let outcome = fetch_temporal(&req, &qry, &geos, &plan).await?;
    if outcome.aggregated {
        return respond_aggregated(
            st,
            req.params,
            &tenant,
            &ctx,
            accept,
            &qry.trepr,
            qry.tq.as_ref(),
            qry.attrs_filter.as_ref(),
            outcome,
        );
    }
    let (all, pre_paged, pre_total) = (outcome.rows, outcome.paged, outcome.total);
    let mut warnings: Vec<String> = Vec::new();
    let all = federate_temporal(&req, &qry.timeprop, all, &mut warnings).await?;
    let mut matches = Vec::new();
    for doc in all {
        let Some(doc) = qualifies(doc, &qry, &geos, &shown_geos, &ctx) else {
            continue;
        };
        if plan.narrowed_now {
            let id = doc
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if !member_now(st, &tenant, &ctx, filter, &id, None).await? {
                continue;
            }
        }
        matches.push(doc);
    }
    if let Some(spec) = req.params.get("orderBy") {
        crate::paging::order_entities(&mut matches, spec, req.params, &ctx)?;
    }
    let (page, count_hdr, links) = if pre_paged {
        let total = pre_total.map(|t| t as usize).unwrap_or(matches.len());
        crate::paging::paginate_pre(
            st,
            req.params,
            matches,
            "/ngsi-ld/v1/temporal/entities",
            total,
        )?
    } else {
        crate::paging::paginate(st, req.params, matches, "/ngsi-ld/v1/temporal/entities")?
    };
    if let Some(sink) = merged {
        sink.extend(page.iter().cloned());
    }
    let (payload, g) = assemble_page(&req, &qry, &page)?;
    let mut resp = respond_temporal_page(&req, accept, &qry, payload, &g, &warnings);
    attach_paging(&mut resp, count_hdr, &links);
    attach_paging(&mut resp, None, &g.links(&req, accept, &qry));
    // 6.18.3.2: entityMap=true — the temporal EntityMap for this query is
    // (re)created; the response carries NGSILD-EntityMap and 201 Created.
    if req.params.get("entityMap").map(String::as_str) == Some("true") {
        let map = build_temporal_map(st, &tenant, headers, &ctx, req.params, filter).await?;
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

/// The parameters 5.7.4 accepts (6.18.3.2 Table 6.18.3.2-1).
const QUERY_PARAMS: &[&str] = &[
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
    "timerel",
    "timeAt",
    "endTimeAt",
    "timeproperty",
    "aggrMethods",
    "aggrPeriodDuration",
    "lastN",
    "firstN",
    "offsetN",
    "limit",
    "offset",
    "count",
    "options",
    "format",
    "lang",
    "local",
    "entityMap",
    "pick",
    "omit",
    "datasetId",
    "orderBy",
    "orderFrom",
    "orderGeometry",
    "collation",
    "entityMapLifetime",
    "splitEntities",
    "expandValues",
    "jsonKeys",
];

/// 5.7.4.3 expandValues: the same 4.9 EXAMPLE 12 coercion as the entity
/// query — term values expanded against the @context before executing,
/// less the Attributes jsonKeys declares uninterpretable.
fn expand_q_values(
    ast: antares_ql::QNode,
    params: &HashMap<String, String>,
    ctx: &Context,
) -> antares_ql::QNode {
    antares_ql::eval::apply_expand_values(
        ast,
        antares_ql::eval::expansion_list(
            params.get("expandValues").map(String::as_str),
            params.get("jsonKeys").map(String::as_str),
        )
        .as_deref(),
        ctx,
    )
}

/// One 5.7.4 request as every stage after the narrowing reads it; `params`
/// is the narrowed query.
struct QueryReq<'a> {
    st: &'a AppState,
    tenant: &'a TenantId,
    headers: &'a HeaderMap,
    ctx: &'a Context,
    params: &'a HashMap<String, String>,
    filter: &'a crate::policy::Filter,
}

/// The narrowed 5.7.4 query, parsed and validated up front.
struct Parsed<'p> {
    /// ADR-0023 rule 11: the type rules each Entity is shown by
    types_rule: Option<std::sync::Arc<crate::policy::Filter>>,
    /// the client's own q under rules, judged on the shown history
    client_q: Option<antares_ql::QNode>,
    /// the q the store and the stored history read
    q_ast: Option<antares_ql::QNode>,
    scope_q: Option<&'p str>,
    tq: Option<TemporalQ>,
    trepr: TRepr,
    ids: Option<Vec<&'p str>>,
    id_pattern: Option<IdMatcher>,
    types: Option<Vec<String>>,
    /// what the window keeps per Entity
    attrs_filter: Option<Vec<String>>,
    /// only the attrs= param excludes entities; pick is projection-only
    entity_attr_filter: Option<Vec<String>>,
    /// under rules: the attrs= the shown history must hold
    shown_attrs: Option<Vec<String>>,
    own_geo: Option<antares_ql::geo::GeoQuery>,
    timeprop: String,
}

impl<'p> Parsed<'p> {
    fn new(
        req: &QueryReq<'p>,
        types_rule: Option<std::sync::Arc<crate::policy::Filter>>,
        client_q: Option<antares_ql::QNode>,
    ) -> ApiResult<Self> {
        let (params, ctx) = (req.params, req.ctx);
        let ruled = types_rule.is_some();
        let q_ast = params
            .get("q")
            .map(|q| antares_ql::parse_narrowed_q(q))
            .transpose()?
            .map(|ast| expand_q_values(ast, params, ctx));
        // under rules only the engine's own q reaches the store and the stored
        // history; the client's waits for the shown one
        let q_ast = if ruled {
            req.filter.q.as_deref().cloned()
        } else {
            q_ast
        };
        let tq = TemporalQ::from_params(params, true)?;
        let mut trepr = parse_trepr(params, ctx)?;
        crate::repr::narrow_projection(&mut trepr.pick, &mut trepr.omit, req.filter, ctx)?;
        // 5.7.4.4: {…} projection is Linked Entity retrieval — unconditional 400
        reject_linked_projection(&trepr, "5.7.4.4")?;
        let ids: Option<Vec<&str>> = params.get("id").map(|s| s.split(',').collect());
        // 5.7.4.4: an invalid URI in the id list is BadRequestData
        for id in ids.iter().flatten() {
            antares_model::EntityId::new(id)?;
        }
        let id_pattern = id_matcher(params.get("idPattern"))?;
        let types: Option<Vec<String>> = params.get("type").map(|s| {
            s.split([',', '|'])
                .map(|t| ctx.expand_key(t.trim()))
                .collect()
        });
        let attrs_filter = selection(&trepr);
        let (entity_attr_filter, shown_attrs) = if ruled {
            (None, trepr.attrs.clone())
        } else {
            (trepr.attrs.clone(), None)
        };
        let own_geo = antares_ql::geo::GeoQuery::from_params(params)?;
        let timeprop = tq
            .as_ref()
            .map_or("observedAt", |t| t.timeproperty.as_str())
            .to_owned();
        Ok(Self {
            types_rule,
            client_q,
            q_ast,
            scope_q: params.get("scopeQ").map(String::as_str),
            tq,
            trepr,
            ids,
            id_pattern,
            types,
            attrs_filter,
            entity_attr_filter,
            shown_attrs,
            own_geo,
            timeprop,
        })
    }

    /// The request's own geoquery and the narrowing's area, all of which
    /// must hold (ADR-0023 rule 1); under rules the request's reads the
    /// shown history.
    fn geos<'g>(&'g self, filter: &'g crate::policy::Filter) -> (Geos<'g>, Geos<'g>) {
        if self.types_rule.is_some() {
            (filter.areas().collect(), self.own_geo.iter().collect())
        } else {
            (
                self.own_geo.iter().chain(filter.areas()).collect(),
                Vec::new(),
            )
        }
    }

    fn instance_range(&self) -> Option<antares_store::filter::InstanceRange<'_>> {
        self.tq
            .as_ref()
            .map(|t| antares_store::filter::InstanceRange {
                timerel: &t.timerel,
                time_at: &t.time_at,
                end_time_at: t.end_time_at.as_deref(),
                timeproperty: &t.timeproperty,
            })
    }
}

/// The geoqueries an Evolution's window is judged by.
type Geos<'g> = Vec<&'g antares_ql::geo::GeoQuery>;

/// The compiled idPattern, as the one test the query asks of an id.
type IdMatcher = Box<dyn Fn(&str) -> bool + Send + Sync>;

/// 5.7.4.4: an idPattern that does not compile is BadRequestData.
fn id_matcher(pattern: Option<&String>) -> Result<Option<IdMatcher>, NgsiError> {
    let Some(p) = pattern else {
        return Ok(None);
    };
    let re = antares_ql::regex::compile(p)
        .map_err(|_| NgsiError::BadRequestData(format!("invalid idPattern {p:?}")))?;
    Ok(Some(Box::new(move |id: &str| re.is_match(id))))
}

/// What the store decides itself for one query.
struct Pushdown {
    /// q, geo and scopeQ absent: the store's instance cap is exact
    exact: bool,
    /// the store pages the Entities
    page: bool,
    /// the store answers the 4.5.19 aggregation
    agg: bool,
    /// a narrowing reads the state now: each Entity is checked after the store
    narrowed_now: bool,
    offset: usize,
    limit: usize,
    /// with `page` under rules: the Attributes an Entity qualifies through
    page_attrs: Option<Vec<String>>,
}

impl Pushdown {
    // Push entity narrowing (ids/types/attrs) and instance-window
    // pruning (range + RANK()-capped lastN) into the store. The loop
    // and window() stay the arbiters — pruning is byte-exact against
    // instance_matches (compile::temporal), so it cannot change an answer.
    // 5.7.4.4 S2/S3: q and geo are judged on the instances WITHIN the
    // temporal interval (the eval_doc retain in `qualifies`), so RANGE pruning is
    // verdict-safe to push even with q=/geo present. Only the lastN cap
    // (its ordering vs the values filter is unspecified) and entity paging
    // (q/geo still drop entities after SQL) wait for exactness.
    // scopeQ joins q/geo here: the 4.18 validity filter drops entities and
    // instances AFTER SQL, so a pushed page/lastN cap would under-return.
    async fn plan(req: &QueryReq<'_>, qry: &Parsed<'_>, no_geo: bool) -> ApiResult<Self> {
        let (st, params, filter) = (req.st, req.params, req.filter);
        let exact = qry.q_ast.is_none() && no_geo && qry.scope_q.is_none();
        // Entity-page pushdown: a temporal query used to materialize the
        // tenant's ENTIRE history. Pushed only when every filter the store
        // cannot see is absent — same gate family as the entity-query pushdown.
        // A values filter no longer blocks paging when its prefilter compiles
        // EXACTLY (every leaf a Cmp with the byte-exact text window): the SQL
        // entity verdict then equals the evaluator's. datasetId/pick still
        // block: their entity drops happen at presentation, after the page.
        let q_page_exact = qry.q_ast.as_ref().is_none_or(|ast| {
            let r = qry.instance_range();
            st.temporal
                .q_pushdown_exact(ast, r.as_ref(), &|t| req.ctx.expand_key(t))
        });
        let (offset, limit, _) = crate::paging::page_params(st, params)?;
        // rule 8 drops Entities after the store answered, so the store may not
        // page under a narrowing that reads the state now (`ponytail:` one read
        // per Entity; push it into the store query when it shows up)
        let narrowed_now =
            filter.q.is_some() || filter.scope_q.is_some() || filter.areas().next().is_some();
        // 5.7.4.4 + 5.5.9: pagination applies to the MERGED federated union, so
        // the store may only pre-page when nothing will federate — otherwise
        // page 1 is local-page + every remote row (matrix-9 IOP_EXT_TMP_03_04).
        // under rules only where their verdict on an Entity is the store's,
        // and the request's own q and geoquery, read on the shown history,
        // are absent
        let ruled = match qry.types_rule.as_deref() {
            None => Some(None),
            Some(_) if qry.client_q.is_some() || qry.own_geo.is_some() => None,
            Some(rules) => page_attrs(
                rules,
                qry.types.as_deref(),
                qry.tq.as_ref(),
                qry.shown_attrs.as_deref(),
            ),
        };
        let page = (exact || (no_geo && qry.scope_q.is_none() && q_page_exact))
            && ruled.is_some()
            && qry.id_pattern.is_none()
            && params.get("orderBy").is_none()
            && params.get("datasetId").is_none()
            && params.get("pick").is_none()
            && limit > 0
            && !narrowed_now
            && !crate::federation::would_federate(st, req.tenant, req.ctx, params, req.headers)
                .await?;
        // the store aggregates every Attribute it reads: never one the rules hide
        let agg = exact && page && qry.types_rule.is_none() && store_aggregates(req, &qry.trepr);
        Ok(Self {
            exact,
            page,
            agg,
            narrowed_now,
            offset,
            limit,
            page_attrs: ruled.flatten(),
        })
    }
}

/// 4.5.19 computed by the store: the numeric bucket matrix per attribute
/// comes back aggregated when nothing after the store call could change
/// the answer — every filter exact in SQL, the page pushed, no
/// projection/lastN/month periods — otherwise the instances are
/// aggregated here as before. A store that cannot (memory) or a
/// non-numeric value class leaves `outcome.aggregated` false.
fn store_aggregates(req: &QueryReq<'_>, trepr: &TRepr) -> bool {
    trepr.aggregated
        && !crate::history::records_on_change(req.st, req.tenant)
        && trepr.omit.is_none()
        && trepr.page == InstancePage::default()
        && !matches!(trepr.aggr_period, AggrPeriod::Months(..))
        && req.params.get("entityMap").map(String::as_str) != Some("true")
        && trepr
            .aggr_methods
            .iter()
            .all(|m| antares_store::filter::AGGREGATE_METHODS.contains(&m.as_str()))
}

/// The store call of one query, with what `plan` pushed down.
async fn fetch_temporal(
    req: &QueryReq<'_>,
    qry: &Parsed<'_>,
    geos: &[&antares_ql::geo::GeoQuery],
    plan: &Pushdown,
) -> ApiResult<antares_store::filter::TemporalOutcome> {
    let (ctx, trepr, page_n) = (req.ctx, &qry.trepr, qry.trepr.page);
    // scoped: the &dyn expander must not live across an await (handler
    // futures are Send; the store call itself is synchronous)
    let expand = |t: &str| ctx.expand_key(t);
    let geo_pre: Vec<_> = geos.iter().map(|g| g.to_instance_spec(ctx)).collect();
    let tf = antares_store::filter::TemporalFilter {
        ids: qry.ids.as_deref(),
        types: qry.types.as_deref(),
        // the rules admit only ids these match: the store skips the rest
        // instead of walking the type into its 10 000-row ceiling (T-6052)
        id_patterns: qry
            .types_rule
            .as_deref()
            .and_then(|r| crate::policy::shared_id_patterns(&r.rules)),
        attrs: qry.entity_attr_filter.as_deref(),
        // a condition rebuilds each instance's state from the whole
        // history; rules without one read only the window (T-4246)
        range: qry
            .instance_range()
            .filter(|_| !replays_state(qry.types_rule.as_deref())),
        last_n: page_n
            .store_cap(default_page(req.st, req.tenant, trepr))
            .filter(|_| {
                plan.exact
                    && cap_keeps_shown(
                        qry.types_rule.as_deref(),
                        &page_n,
                        &qry.timeprop,
                        range_start(qry.tq.as_ref()),
                    )
            }),
        oldest_first: !page_n.desc,
        timeproperty: &qry.timeprop,
        page: plan.page.then_some(antares_store::filter::Page {
            offset: plan.offset as i64,
            limit: plan.limit as i64,
            count: true,
        }),
        page_attrs: plan.page_attrs.as_deref().filter(|_| plan.page),
        q: qry.q_ast.as_ref(),
        expand: &expand,
        geo: &geo_pre,
        aggregate: plan.agg.then_some(antares_store::filter::Aggregate {
            methods: &trepr.aggr_methods,
            period_secs: match trepr.aggr_period {
                AggrPeriod::Seconds(sc) => Some(sc),
                _ => None,
            },
            anchor: qry.tq.as_ref().map(|t| t.time_at.as_str()),
        }),
    };
    Ok(req.st.temporal.query_temporal(req.tenant, &tf).await?)
}

/// 5.7.4.4: fan the query out to matching queryTemporal registrations
/// and merge the remote Temporal Evolutions with the local set (4.5.5;
/// auxiliary data never introduces new entities).
async fn federate_temporal(
    req: &QueryReq<'_>,
    timeprop: &str,
    all: Vec<Value>,
    warnings: &mut Vec<String>,
) -> ApiResult<Vec<Value>> {
    let looped = crate::federation::via_loop(
        req.headers,
        &crate::federation::alias_for(&req.st.host_alias, req.tenant),
    );
    if !crate::federation::active(req.params) || looped {
        return Ok(all);
    }
    let mut proxies = Vec::new();
    let fed = crate::federation::fed_query_temporal(
        req.st,
        req.tenant,
        req.headers,
        req.ctx,
        req.params,
        warnings,
        &mut proxies,
    )
    .await?;
    let mut all = all;
    for doc in &mut all {
        crate::federation::drop_proxied_local(doc, &proxies, req.ctx);
    }
    Ok(merge_federated(all, &fed, timeprop))
}

/// The local Evolutions in their order, each merged with its remote
/// copies, then the Entities only an exclusive registration holds.
fn merge_federated(all: Vec<Value>, fed: &[(bool, Value)], timeprop: &str) -> Vec<Value> {
    let mut order: Vec<String> = Vec::new();
    let mut by_id: std::collections::HashMap<String, Value> = Default::default();
    for doc in all {
        if let Some(id) = doc.get("id").and_then(Value::as_str) {
            order.push(id.to_owned());
            by_id.insert(id.to_owned(), doc);
        }
    }
    for aux_pass in [false, true] {
        for (aux, d) in fed {
            if *aux != aux_pass {
                continue;
            }
            let Some(id) = d.get("id").and_then(Value::as_str) else {
                continue;
            };
            match by_id.get_mut(id) {
                Some(base) => merge_temporal_docs(base, d, *aux, timeprop),
                None if !aux => {
                    order.push(id.to_owned());
                    by_id.insert(id.to_owned(), d.clone());
                }
                None => {}
            }
        }
    }
    order
        .into_iter()
        .filter_map(|id| by_id.remove(&id))
        .collect()
}

/// One candidate Evolution judged by the query: `None` drops it, `Some`
/// is the Evolution as the query keeps it (q instances retained, scopeQ
/// windows cut, the rules' shown history).
fn qualifies(
    mut doc: Value,
    qry: &Parsed<'_>,
    geos: &[&antares_ql::geo::GeoQuery],
    shown_geos: &[&antares_ql::geo::GeoQuery],
    ctx: &Context,
) -> Option<Value> {
    let tq = qry.tq.as_ref();
    let id = doc.get("id").and_then(Value::as_str).unwrap_or("");
    match (&qry.ids, &qry.id_pattern) {
        (Some(ids), _) if !ids.contains(&id) => return None,
        // 5.2.33: "id takes precedence over idPattern"
        (None, Some(matches)) if !matches(id) => return None,
        _ => {}
    }
    if !type_and_attrs_match(&doc, qry.types.as_ref(), qry.entity_attr_filter.as_ref()) {
        return None;
    }
    if !conditions_hold(&mut doc, tq, qry.q_ast.as_ref(), geos, ctx) {
        return None;
    }
    if let Some(sq) = qry.scope_q {
        if !scope_window_retain(&mut doc, sq, tq, &qry.timeprop) {
            return None;
        }
    }
    if let Some(rules) = qry.types_rule.as_deref() {
        doc = rules_per_instance(&doc, rules)?;
        if !type_and_attrs_match(&doc, None, qry.shown_attrs.as_ref()) {
            return None;
        }
        if !conditions_hold(&mut doc, tq, qry.client_q.as_ref(), shown_geos, ctx) {
            return None;
        }
    }
    // entity qualifies only if some instance falls in the window
    any_in_window(&doc, tq).then_some(doc)
}

/// 5.7.4.4 S2/S3: q and the geoqueries hold within the window, and the
/// instances q does not hold for leave the Evolution.
fn conditions_hold(
    doc: &mut Value,
    tq: Option<&TemporalQ>,
    q: Option<&antares_ql::QNode>,
    geos: &[&antares_ql::geo::GeoQuery],
    ctx: &Context,
) -> bool {
    if (q.is_some() || !geos.is_empty()) && !window_conditions_match(doc, tq, q, geos, ctx) {
        return false;
    }
    if let Some(ast) = q {
        retain_q_instances(doc, tq, ast, ctx);
    }
    true
}

fn any_in_window(doc: &Value, tq: Option<&TemporalQ>) -> bool {
    doc.as_object().is_some_and(|o| {
        o.iter().any(|(k, v)| {
            !is_meta(k)
                && v.as_array().is_some_and(|arr| {
                    arr.iter()
                        .any(|inst| tq.is_none_or(|tq| tq.instance_matches(inst)))
                })
        })
    })
}

/// What the page's windows add up to: the 6.3.10 Content-Range and the
/// 6.4.7.3 next-page links read it.
#[derive(Default)]
struct PageCut {
    truncated: bool,
    ts_min: Option<String>,
    ts_max: Option<String>,
    more: bool,
    /// the next page's move: the fewest instances any Entity's cut Attribute
    /// served, and the boundary every Entity's cut reached (`interval_links`)
    cut: Option<usize>,
    bound: Option<String>,
}

impl PageCut {
    fn add(&mut self, w: &Windowed, desc: bool) {
        self.truncated |= w.truncated;
        self.more |= w.more;
        if let Some(c) = w.cut_served {
            self.cut = Some(self.cut.map_or(c, |g| g.min(c)));
        }
        if let Some(b) = w.truncated.then(|| cut_boundary(w, desc)).flatten() {
            // the earliest boundary forwards, the latest backwards: no
            // Entity's instance past its own boundary is left behind
            if self
                .bound
                .as_deref()
                .is_none_or(|g| (dt_key(b) < dt_key(g)) != desc)
            {
                self.bound = Some(b.to_owned());
            }
        }
        if let Some(m) = &w.ts_min {
            if self.ts_min.as_deref().is_none_or(|c| dt_key(m) < dt_key(c)) {
                self.ts_min = Some(m.clone());
            }
        }
        if let Some(m) = &w.ts_max {
            if self.ts_max.as_deref().is_none_or(|c| dt_key(m) > dt_key(c)) {
                self.ts_max = Some(m.clone());
            }
        }
    }

    /// aggregated responses are complete by construction — never 206 (6.3.10)
    fn content_range(&self, qry: &Parsed<'_>) -> Option<String> {
        if qry.trepr.aggregated {
            return None;
        }
        content_range(
            self.truncated,
            self.ts_min.as_deref(),
            self.ts_max.as_deref(),
            qry.tq.as_ref(),
            qry.trepr.page,
        )
    }

    fn links(&self, req: &QueryReq<'_>, accept: Accept, qry: &Parsed<'_>) -> Vec<String> {
        let page_n = qry.trepr.page;
        interval_links(
            req.params,
            "/ngsi-ld/v1/temporal/entities",
            accept,
            page_n,
            self.more,
            page_n.step(
                qry.trepr.aggregated,
                req.st.temporal_default_instances,
                req.st.temporal_instance_limit,
            ),
            self.cut.zip(self.bound.as_deref()),
        )
    }
}

/// Each Entity of the page windowed, cut and presented. ADR-0023 rule 11:
/// each Entity's history was narrowed by its rules in `qualifies`, before
/// the window, so lastN counts only what may be seen (rule 8).
fn assemble_page(
    req: &QueryReq<'_>,
    qry: &Parsed<'_>,
    page: &[Value],
) -> ApiResult<(Vec<Value>, PageCut)> {
    let (st, trepr, page_n) = (req.st, &qry.trepr, qry.trepr.page);
    let core_only_pick = qry.attrs_filter.as_ref().is_some_and(Vec::is_empty);
    let mut payload: Vec<Value> = Vec::new();
    let mut g = PageCut::default();
    for d in page {
        let d = &*crate::history::as_recorded(st, req.tenant, d, false);
        let mut w = window(
            d,
            qry.tq.as_ref(),
            page_n,
            qry.attrs_filter.as_ref(),
            trepr.omit.as_ref(),
            trepr.dataset_id.as_ref(),
            &qry.timeprop,
            crate::history::held_attrs(st, req.tenant, d),
        );
        if !trepr.aggregated {
            truncate(
                &mut w,
                &qry.timeprop,
                page_n.desc,
                page_n.size(st.temporal_default_instances, st.temporal_instance_limit),
            );
        }
        g.add(&w, page_n.desc);
        // no instance survived the window/dataset filters ⇒ the entity is
        // not part of the temporal result (unless the projection is
        // deliberately core-only, e.g. pick=id)
        if w.attrs.is_empty() && !core_only_pick {
            continue;
        }
        let presented = present_temporal(d, &w, req.ctx, trepr, qry.tq.as_ref(), &qry.timeprop)?;
        if trepr.pick.is_some() && presented.as_object().is_some_and(|o| o.is_empty()) {
            continue;
        }
        payload.push(presented);
    }
    Ok((payload, g))
}

/// The page's body, 200 or 206 with its Content-Range, and the
/// federation's warnings.
fn respond_temporal_page(
    req: &QueryReq<'_>,
    accept: Accept,
    qry: &Parsed<'_>,
    payload: Vec<Value>,
    g: &PageCut,
    warnings: &[String],
) -> Response {
    let cr = g.content_range(qry);
    let status = if cr.is_some() {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    let mut resp = crate::negotiate::respond_list(status, payload, req.ctx, accept, req.tenant);
    crate::paging::attach_warnings(&mut resp, warnings);
    if let Some(cr) = cr {
        if let Ok(v) = cr.parse() {
            resp.headers_mut().insert("Content-Range", v);
        }
    }
    resp
}

// ---------- POST /temporal/entityOperations/query (6.24) ----------

pub async fn batch_temporal_query(
    State(st): State<AppState>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let go = async {
        // 6.3.14 and 6.3.4: a Tenant outside the grammar and an Accept the
        // operation cannot serve are both refused here. Neither VALUE is
        // needed — the inner query reads the headers again — but the request
        // must not reach it having skipped either check.
        let tenant = tenant_from(&headers)?;
        check_params(
            &params,
            &["limit", "offset", "count", "options", "format", "local"],
        )?;
        parse_accept(&headers)?;
        let parsed = parse_body(&st, &headers, &body, BodyKind::Standard).await?;
        let q = parsed.object(NgsiError::BadRequestData(
            "query body must be an object".into(),
        ))?;
        if q.get("type").and_then(Value::as_str) != Some("Query") {
            return Err(NgsiError::BadRequestData("body type must be Query".into()).into());
        }
        // 5.2.23 Query (temporal reading): members flattened with their
        // Table 5.2.23-1 value spaces enforced, incl. temporalQ (5.2.21)
        // and aggrParams (5.2.44).
        let mut vp: HashMap<String, String> = params.clone();
        crate::paging::query_doc_params(q, true, &mut vp)?;
        // ADR-0020: the engine is shown every member of the Query body
        let named = crate::entities::ReadMembers::with_ctx(&st, &vp, &parsed.ctx);
        let filter = gate!(
            st, &tenant, &headers, "5.7.4",
            q: named.q.as_ref(),
            scope_q: vp.get("scopeQ").map(String::as_str),
            types: &named.types,
            attrs: &named.attrs,
            geo: named.geo.as_ref(),
        )
        .await?;
        crate::policy::Metadata::refuse_body(&filter, &vp, &parsed.ctx)?;
        query_temporal_inner(&st, &vp, &headers, &filter).await
    };
    go.await.unwrap_or_else(|e| e.into_response())
}

/// 5.14.5.4: temporal query required; the S1–S4 candidate selection is the
/// temporal query pipeline itself (5.7.4.4) run unpaged — the ids of its
/// result set form the EntityMap; the createEntityMapQueryTemporal
/// registrations are then merged like 5.14.4.
/// Known ceiling: candidate ids are read from the internal 5.7.4 response capped
/// at max_limit; raise the cap if temporal sets outgrow it.
pub(crate) async fn build_temporal_map(
    st: &AppState,
    tenant: &TenantId,
    headers: &HeaderMap,
    ctx: &antares_jsonld::Context,
    params: &HashMap<String, String>,
    filter: &crate::policy::Filter,
) -> ApiResult<Value> {
    if !params.contains_key("timerel") {
        return Err(NgsiError::BadRequestData(
            "a temporal query is required to create a temporal EntityMap (5.14.5.4)".into(),
        )
        .into());
    }
    let local_scope = params.get("local").map(String::as_str) == Some("true");
    let split = params.get("splitEntities").map(String::as_str) == Some("true");
    let mut eff: HashMap<String, String> = if split && !local_scope {
        params
            .iter()
            .filter(|(k, _)| {
                [
                    "id",
                    "idPattern",
                    "type",
                    "local",
                    "timerel",
                    "timeAt",
                    "endTimeAt",
                    "timeproperty",
                ]
                .contains(&k.as_str())
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    } else {
        params.clone()
    };
    for k in [
        "entityMap",
        "entityMapLifetime",
        "splitEntities",
        "offset",
        "count",
    ] {
        eff.remove(k);
    }
    eff.insert("limit".into(), st.max_limit.to_string());
    // Box::pin: build_temporal_map is reachable from query_temporal_inner
    // (entityMap=true), so this recursive edge needs indirection.
    let resp = Box::pin(query_temporal_inner(st, &eff, headers, filter)).await?;
    let bytes = st.read_answer(resp.into_body(), "temporal").await?;
    // 5.5.14: the map FIXES the Entities considered by every later request, so
    // an unreadable candidate set must fail rather than become "no candidates".
    let candidates: Value = serde_json::from_slice(&bytes)
        .map_err(|_| NgsiError::InternalError("temporal candidate parse".into()))?;
    let mut emap = Map::new();
    if let Some(arr) = candidates.as_array() {
        for d in arr {
            if let Some(id) = d.get("id").and_then(Value::as_str) {
                emap.insert(id.to_owned(), json!(["@none"]));
            }
        }
    }
    crate::entity_map::merge_and_store_map(st, tenant, headers, ctx, params, true, emap).await
}
