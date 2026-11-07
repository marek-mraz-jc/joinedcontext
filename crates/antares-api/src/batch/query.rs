// SPDX-License-Identifier: EUPL-1.2
//! Query Entities over POST (5.7.2, resource 6.23): the Query body read as
//! the GET twin reads its URI, then paged, shaped and joined.

use super::*;

// ---------- POST /entityOperations/query (6.23) ----------

pub async fn batch_query(
    State(st): State<AppState>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match batch_query_inner(&st, &params, &headers, &body).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

async fn batch_query_inner(
    st: &AppState,
    params: &HashMap<String, String>,
    headers: &HeaderMap,
    body: &[u8],
) -> ApiResult<Response> {
    let tenant = tenant_from(headers)?;
    check_params(
        params,
        &["limit", "offset", "count", "options", "format", "local"],
    )?;
    // POST query IS Query Entities: geo+json is a valid Accept here (6.3.15)
    let accept = parse_accept_geo(headers)?;
    let parsed = parse_body(st, headers, body, BodyKind::Standard).await?;
    let q = parsed.object(NgsiError::BadRequestData(
        "query body must be an object".into(),
    ))?;
    if q.get("type").and_then(Value::as_str) != Some("Query") {
        return Err(NgsiError::BadRequestData("body type must be Query (5.2.23)".into()).into());
    }
    // Convert Query members into virtual params reusing the GET filter path.
    let mut vp: HashMap<String, String> = HashMap::new();
    query_doc_params(q, false, &mut vp)?;
    if let Some(l) = params.get("local") {
        vp.insert("local".into(), l.clone());
    }
    // ADR-0020: the engine is shown every member of the Query body
    let named = crate::entities::ReadMembers::with_ctx(st, &vp, &parsed.ctx);
    let filter = gate!(
        st, &tenant, headers, "5.7.2",
        q: named.q.as_ref(),
        scope_q: vp.get("scopeQ").map(String::as_str),
        types: &named.types,
        attrs: &named.attrs,
        geo: named.geo.as_ref(),
    )
    .await?;
    refuse_too_wide(&vp)?;
    // ADR-0020: the engine's narrowing joins the Query's own filters here,
    // after the "too wide" judgement — which is about what the client asked
    // for. Everything below reads the narrowed query, the forwarded one
    // included.
    // what the client's own Query reads, before the engine's q joins it
    // (ADR-0023 rule 11)
    let narrow = crate::repr::narrowing_reading(&filter, &vp, &parsed.ctx)?;
    let vp = filter.narrow_params(&vp, &parsed.ctx)?;
    let fed = fed_matches(st, &tenant, headers, &parsed.ctx, &vp).await?;
    let mut matches =
        crate::entities::filter_entities_fed(st, &tenant, &vp, &parsed.ctx, fed, Some(&narrow))
            .await?;
    let mut page_params = params.clone();
    page_params.extend(vp.clone());
    // 5.2.43 ordering: same 4.23 keys as the GET twin, applied pre-pagination
    if let Some(spec) = page_params.get("orderBy") {
        crate::repr::order_as_shown(&mut matches, Some(&narrow), |m| {
            crate::paging::order_entities(m, spec, &page_params, &parsed.ctx)
        })?;
    }
    let (page, count_hdr, _links) = paginate(
        st,
        &page_params,
        matches,
        "/ngsi-ld/v1/entityOperations/query",
    )?;
    // body members (pick/omit/attrs/lang/datasetId) shape the representation
    // exactly like their 6.3.7 query-parameter twins
    let mut repr = parse_repr(&page_params, &parsed.ctx)?;
    repr.modified_by = crate::provenance::shows_modified_by(headers, st.provenance_from.as_deref());
    crate::repr::narrow_repr(&mut repr, &filter);
    let join = crate::entities::parse_join(&vp)?;
    crate::entities::check_linked_projection(&repr, &join)?;
    let mut payload = shaped_page(&page, &repr, &parsed.ctx);
    if let Some((mode, level)) = &join {
        // 4.5.23.1 bounds the WIDTH of the retrieval per REQUEST, so one
        // allowance is spent across the whole page, exactly as the GET twin
        // spends it. Minting a fresh allowance per payload Entity multiplies
        // the ceiling by the page size, which is the request the ceiling
        // exists to bound: a page of densely linked Entities.
        let scope = JoinScope {
            st,
            tenant: &tenant,
            ctx: &parsed.ctx,
            repr: &repr,
            level: *level,
            held: crate::entities::contained_by(&page_params),
        };
        match mode.as_str() {
            "inline" => {
                let remote = crate::federation::active(&page_params).then_some(headers);
                scope.inline(&mut payload, remote).await;
            }
            "flat" => payload.extend(scope.flat(&page).await),
            _ => {}
        }
    }
    let out = if accept == Accept::GeoJson {
        crate::repr::to_geojson_collection(payload, None)
    } else {
        Value::Array(payload)
    };
    // 6.3.6: the json answer's Link names the @context it was compacted in
    let served = crate::contexts::linkable(st, &tenant, headers, &parsed.ctx, accept).await?;
    let mut resp = respond_prefer(StatusCode::OK, out, &served, accept, &tenant, headers);
    attach_paging(&mut resp, count_hdr, &[]);
    filter.mark(&st.host_alias, resp.headers_mut());
    Ok(resp)
}

/// Each page Entity in the requested representation; one a `pick` leaves
/// empty is dropped.
fn shaped_page(
    page: &[Value],
    repr: &crate::repr::Repr,
    ctx: &antares_jsonld::Context,
) -> Vec<Value> {
    page.iter()
        .filter_map(|doc| {
            let shaped = apply(doc, repr);
            if repr.pick.is_some() && shaped.as_object().is_some_and(|o| o.is_empty()) {
                return None;
            }
            Some(crate::repr::compact_for(repr, &shaped, ctx))
        })
        .collect()
}

/// 5.7.2.4: the Query must name what it reads, and every filter in it must
/// parse.
fn refuse_too_wide(vp: &HashMap<String, String>) -> ApiResult<()> {
    // 5.7.2.4 (p. 201): "At least one of the following input data shall be
    // provided: a) selector of Entity Types; b) list of Attribute names,
    // including at least one non-system Attribute; c) NGSI-LD Query,
    // including at least one non-system Attribute; d) NGSI-LD GeoQuery;
    // e) local scope. If none of the above is provided, then an error of type
    // BadRequestData shall be raised (too wide query)." Query Entities is ONE
    // operation: the resource that carries the Query in a body answers to the
    // same behaviour clause as the one that carries it in the URI, and
    // without this a bare `{"type":"Query"}` reads the whole tenant and fans
    // the filterless query out to every matching registration.
    let q_ast = vp.get("q").map(|q| antares_ql::parse_q(q)).transpose()?;
    // 5.7.2.4: "if ... the query, geoquery or context source filter are not
    // syntactically valid (as per the referred clauses 4.9 and 4.10) an error
    // of type BadRequestData shall be raised." Registration matching parses
    // the csf again and drops what it cannot parse, which widens the fan-out
    // to the Context Sources the filter was there to exclude — so it is
    // refused here, the way the twin carrying the Query in the URI refuses it.
    if let Some(csf) = vp.get("csf") {
        antares_ql::parse_q(csf)?;
    }
    if !crate::entities::qualifies_non_wide(vp, q_ast.as_ref()) {
        return Err(NgsiError::BadRequestData(
            "query needs at least one of type, attrs, q, georel (5.7.2.4)".into(),
        )
        .into());
    }
    Ok(())
}

/// The matches the registrations add, unless this request came round the
/// federation loop.
async fn fed_matches(
    st: &AppState,
    tenant: &antares_model::TenantId,
    headers: &HeaderMap,
    ctx: &antares_jsonld::Context,
    vp: &HashMap<String, String>,
) -> ApiResult<Vec<(bool, Value)>> {
    Ok(
        if crate::federation::active(vp)
            && !crate::federation::via_loop(
                headers,
                &crate::federation::alias_for(&st.host_alias, tenant),
            )
        {
            // 6.3.17 scopes NGSILD-Warning to GET /entities(/{id}) — collected
            // here for the log only, never emitted on entityOperations/query
            let mut warnings = Vec::new();
            let fed =
                crate::federation::fed_query(st, tenant, headers, ctx, vp, &mut warnings).await?;
            for w in &warnings {
                tracing::debug!("distributed query warning (batch query): {w}");
            }
            fed
        } else {
            Vec::new()
        },
    )
}

/// The 4.5.23 join of one page: what both join modes walk with.
struct JoinScope<'a> {
    st: &'a AppState,
    tenant: &'a antares_model::TenantId,
    ctx: &'a antares_jsonld::Context,
    repr: &'a crate::repr::Repr,
    level: usize,
    held: Vec<String>,
}

impl JoinScope<'_> {
    /// 4.5.23 inline join: the linked Entities inside each payload Entity;
    /// `remote` carries the request headers when the query is federated.
    async fn inline(&self, payload: &mut [Value], remote: Option<&HeaderMap>) {
        let mut budget = crate::repr::MAX_JOIN_LOOKUPS;
        for p in payload {
            crate::repr::inline_join_beyond(
                self.st,
                self.tenant,
                self.ctx,
                self.repr,
                p,
                self.level,
                &self.held,
                &mut budget,
                remote,
            )
            .await;
        }
    }

    /// 4.5.23 flat join: the linked Entities the page does not hold, to follow it.
    async fn flat(&self, page: &[Value]) -> Vec<Value> {
        let (st, tenant, ctx, repr, level, held) = (
            self.st,
            self.tenant,
            self.ctx,
            self.repr,
            self.level,
            &self.held[..],
        );
        let mut budget = crate::repr::MAX_JOIN_LOOKUPS;
        let mut linked = std::collections::BTreeMap::new();
        for doc in page {
            crate::repr::collect_flat_beyond(
                st,
                tenant,
                ctx,
                repr,
                doc,
                level,
                &mut linked,
                held,
                &mut budget,
            )
            .await;
        }
        let page_ids: Vec<&str> = page.iter().filter_map(|d| d["id"].as_str()).collect();
        linked
            .into_iter()
            .filter(|(id, _)| !page_ids.contains(&id.as_str()))
            .map(|(_, (ldoc, lrepr))| crate::repr::compact_for(&lrepr, &apply(&ldoc, &lrepr), ctx))
            .collect()
    }
}
