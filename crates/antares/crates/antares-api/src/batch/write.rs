// SPDX-License-Identifier: EUPL-1.2
//! Batch Entity Create, Upsert, Update and Merge (5.6.7, 5.6.8, 5.6.9,
//! 5.6.20): one [`BatchRun`] takes the array through its phases in order,
//! the per-member decision, the expansion of each member, the chronological
//! rounds, the one multi-row create, the distributed arm and the S/E outcome.

use super::*;

// ---------- POST /entityOperations/create (5.6.7) ----------

pub async fn batch_create(
    State(st): State<AppState>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match batch_write(&st, &params, &headers, &body, BatchMode::Create).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

pub async fn batch_upsert(
    State(st): State<AppState>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match batch_write(&st, &params, &headers, &body, BatchMode::Upsert).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

pub async fn batch_update(
    State(st): State<AppState>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match batch_write(&st, &params, &headers, &body, BatchMode::Update).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

/// 5.6.20 Batch Entity Merge: each entity merged per 5.6.17 locally;
/// 204 when all succeed, 207 with S/E arrays otherwise (5.6.20.5).
pub async fn batch_merge(
    State(st): State<AppState>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match batch_write(&st, &params, &headers, &body, BatchMode::Merge).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

/// The 6.3.7 `options` of a batch write: a comma separated list of strings,
/// so a mode is selected when it appears as ONE MEMBER of the list, never by
/// whole-string equality. 6.15.3.1 replace (default) | update for upsert;
/// 6.16.3.1 noOverwrite for update.
fn write_opts(params: &HashMap<String, String>, mode: BatchMode) -> WriteOpts {
    let has_option = |name: &str| {
        params
            .get("options")
            .is_some_and(|o| o.split(',').any(|s| s.trim() == name))
    };
    WriteOpts {
        mode,
        update: has_option("update"),
        no_overwrite: has_option("noOverwrite"),
    }
}

async fn batch_write(
    st: &AppState,
    params: &HashMap<String, String>,
    headers: &HeaderMap,
    body: &[u8],
    mode: BatchMode,
) -> ApiResult<Response> {
    let tenant = tenant_from(headers)?;
    check_params(params, &["options", "local"])?;
    // T-4562: a batch runs only inside the in-flight budget, held until it answers.
    let _admitted = crate::bounds::admit_batch(body.len())
        .await
        .ok_or(ApiError::Overloaded(crate::negotiate::RETRY_AFTER_SECONDS))?;
    let opts = write_opts(params, mode);
    let mut items = parse_batch(st, &tenant, headers, body).await?;
    let mut run = BatchRun {
        st,
        tenant,
        opts,
        out: BatchOutcome::new(),
        conds: Conditions::default(),
        #[cfg(feature = "views")]
        known: HashMap::new(),
        created_ids: Vec::new(),
        any_created: false,
    };
    // ADR-0023 rule 7 under an endpoint view: every batch write is decided
    // per member on the stored Entities below; an engine narrowing still
    // refuses it whole, as `gate!` refuses a narrowed write.
    #[cfg(feature = "views")]
    crate::views::with_view(
        st.view().as_deref(),
        mode.clause(),
        Ok(crate::policy::unnarrowed()),
    )?;
    run.decide(headers, &mut items).await?;
    let (spec, fwd_items) = batch_spec(&items);
    let fed_regs = match crate::federation::write_plan(
        st,
        &run.tenant,
        &spec,
        &st.loader.core(),
        params,
        headers,
    )
    .await?
    {
        crate::federation::WritePlan::Answered(r) => return Ok(*r),
        crate::federation::WritePlan::Forward(regs) => regs,
    };
    run.conds.unforwarded(!fed_regs.is_empty())?;
    let proxies: Vec<&crate::federation::FedReg> =
        fed_regs.iter().filter(|r| r.is_proxy()).collect();
    let (creates, prepped) = run.prep(items, &proxies);
    let prepped = run.new_comma_ids(prepped).await?;
    for round in rounds(prepped) {
        run.round(round).await?;
    }
    run.create(creates).await?;
    // Distributed arm (5.6.7.4/5.6.8.4/5.6.9.4/5.6.20.4): remote outcomes
    // merge into the same S/E arrays as local ones — the response body
    // carries Entity IDs and BatchEntityErrors (5.2.16/5.2.17), never
    // opaque part descriptors.
    if !fed_regs.is_empty() {
        let remote = forward_batch(
            st,
            params,
            headers,
            &run.tenant,
            opts,
            &fed_regs,
            &fwd_items,
        )
        .await;
        run.merge_remote(remote);
    }
    Ok(run.finish())
}

/// A member as the engine is shown it: its id, its pre-image and its
/// expanded body.
type Shown = (Option<String>, crate::policy::PreImage, Option<Value>);

/// One member of a batch, as the store writes it: the id and the expanded
/// document.
type Member = (String, Value);

/// One batch write in flight: what its phases read and fill.
struct BatchRun<'a> {
    st: &'a AppState,
    tenant: antares_model::TenantId,
    opts: WriteOpts,
    out: BatchOutcome,
    /// The conditions the engine set on the allowed members (gap G12).
    conds: Conditions,
    /// Entities read before the first write, which a view's Relationship check
    /// takes instead of reading them again (T-4085).
    #[cfg(feature = "views")]
    known: HashMap<String, Value>,
    /// The ids the batch created, local and forwarded, each once (5.6.8.5).
    created_ids: Vec<String>,
    any_created: bool,
}

impl BatchRun<'_> {
    /// ADR-0020 per member (T-3480): before any item is written and before
    /// any forward, the engine decides each member as the single-Entity write
    /// would — its id, its types (stored and written), its Attributes and its
    /// expanded body. A refused member lands in E with the AccessDenied
    /// problem and the rest are applied (207); a batch refused whole is the
    /// 403 it always was. An entry that does not expand is shown by its id
    /// alone and left to the local arm to report.
    async fn decide(&mut self, headers: &HeaderMap, items: &mut Vec<BatchItem>) -> ApiResult<()> {
        let st = self.st;
        let Some(engine) = &st.policy else {
            return Ok(());
        };
        let shown = self.pre_images(members(items)).await?;
        let attrs: Vec<Vec<String>> = shown
            .iter()
            .map(|(_, _, b)| written_attrs(b.as_ref()))
            .collect();
        let ids: Vec<Vec<&str>> = shown
            .iter()
            .map(|(id, _, _)| id.as_deref().into_iter().collect())
            .collect();
        let ops: Vec<crate::policy::Operation<'_>> = shown
            .iter()
            .zip(&ids)
            .zip(&attrs)
            .map(|(((_, pre, body), ids), attrs)| crate::policy::Operation {
                ids,
                types: &pre.types,
                attrs,
                body: body.as_ref(),
                stored: pre.stored.as_ref(),
                ..crate::policy::Operation::new(self.opts.mode.clause())
            })
            .collect();
        let asking = crate::snapshots::asking_tenant(st, &self.tenant).await;
        let refused = crate::policy::gate_each(engine.as_ref(), &asking, headers, &ops).await;
        drop(ops);
        if let Some(why) = whole_refusal(&refused) {
            return Err(crate::policy::Denied(why).into());
        }
        let mut verdicts = refused.into_iter().zip(shown);
        let (out, conds) = (&mut self.out, &mut self.conds);
        items.retain(|_| match verdicts.next() {
            Some((Err(why), (id, _, _))) => {
                out.errors.push(denied_entry(id.as_deref(), &why));
                false
            }
            Some((Ok(granted), (id, _, _))) => {
                conds.add(id.as_deref(), &granted);
                true
            }
            None => true,
        });
        Ok(())
    }

    /// Each member with its pre-image, from one read for every member's
    /// stored Entity and, under a view, every Relationship target the batch
    /// names (T-4085); a store failure fails the batch (fail-closed,
    /// F-ANT2-009), never reads as absent.
    async fn pre_images(
        &mut self,
        members: Vec<(Option<String>, Option<Value>)>,
    ) -> Result<Vec<Shown>, NgsiError> {
        let mut wanted: Vec<&str> = members.iter().filter_map(|(id, _)| id.as_deref()).collect();
        #[cfg(feature = "views")]
        if self.st.view().is_some() {
            wanted.extend(
                members
                    .iter()
                    .filter_map(|(_, body)| body.as_ref())
                    .flat_map(crate::views::relationship_targets),
            );
        }
        wanted.sort_unstable();
        wanted.dedup();
        let stored = if wanted.is_empty() {
            HashMap::new()
        } else {
            self.st.store.get_entities(&self.tenant, &wanted).await?
        };
        let shown: Vec<Shown> = members
            .into_iter()
            .map(|(id, body)| {
                let pre = match &id {
                    Some(id) => crate::policy::image(stored.get(id).cloned(), body.as_ref()),
                    None => crate::policy::PreImage::default(),
                };
                (id, pre, body)
            })
            .collect();
        // What the batch writes itself is read again by the round that needs it.
        #[cfg(feature = "views")]
        {
            let written: std::collections::HashSet<&str> = shown
                .iter()
                .filter_map(|(id, _, _)| id.as_deref())
                .collect();
            self.known = stored
                .into_iter()
                .filter(|(id, _)| !written.contains(id.as_str()))
                .collect();
        }
        Ok(shown)
    }

    /// Expansion and validation per member, ahead of every store operation:
    /// the creates, and the members the rounds write. A member that fails
    /// lands in E; one wholly proxied has no local part.
    fn prep(
        &mut self,
        items: Vec<BatchItem>,
        proxies: &[&crate::federation::FedReg],
    ) -> (Vec<Member>, Vec<Member>) {
        // Creates are collected and written as ONE multi-row statement;
        // upsert/update/merge run batched per round.
        // 5.5.11.0: "All Entities and Attributes in the batch will get the same
        // modifiedAt timestamp" — the clock is read once for the whole array, not
        // per document. Stamping per document spreads a large create over several
        // milliseconds, and a Context Consumer filtering or paging on modifiedAt
        // then sees one batch as two. The per-round stamp is the separate
        // case: repeated instances of one id are sequential operations (5.5.11.2,
        // 5.5.11.5) and do carry their own instants.
        let create_ts = now_iso();
        let (mut creates, mut prepped) = (Vec::new(), Vec::new());
        for (item, ctx) in items {
            let id_hint = item.get("id").and_then(Value::as_str).map(str::to_owned);
            let ctx = match ctx {
                Ok(c) => c,
                Err(e) => {
                    self.out
                        .errors
                        .push(err_entry(id_hint.as_deref(), &ngsi_of(e)));
                    continue;
                }
            };
            let Some(item) = local_part(item, proxies, &ctx) else {
                continue; // wholly proxied: no local part for this item
            };
            let (prep, into) = if self.opts.mode == BatchMode::Create {
                (prep_create(&item, &ctx, &create_ts), &mut creates)
            } else {
                (prep_write(&item, &ctx, self.opts.mode), &mut prepped)
            };
            // T-5912: each member against the Tenant's schema, refused alone
            // as any other invalid member (5.6.7.4: the rest are applied).
            #[cfg(feature = "views")]
            let prep = prep.and_then(|(id, doc)| {
                let whole = match self.opts.mode {
                    BatchMode::Create => true,
                    BatchMode::Upsert => !self.opts.update,
                    BatchMode::Update | BatchMode::Merge => false,
                };
                crate::views::schema_admits_typed(self.st, &self.tenant, &doc, whole)?;
                Ok((id, doc))
            });
            match prep {
                Ok(pair) => into.push(pair),
                Err(e) => self.out.errors.push(err_entry(id_hint.as_deref(), &e)),
            }
        }
        (creates, prepped)
    }

    /// An upsert creates what is missing, so the create rule on a comma id
    /// (F-ANT2-013) holds for a member whose Entity is not stored; one
    /// already stored under such an id is still updated, so a source that
    /// keeps upserting a legacy comma id does not turn red.
    async fn new_comma_ids(&mut self, prepped: Vec<Member>) -> Result<Vec<Member>, NgsiError> {
        let comma: Vec<&str> = prepped
            .iter()
            .map(|(id, _)| id.as_str())
            .filter(|id| id.contains(','))
            .collect();
        if self.opts.mode != BatchMode::Upsert || comma.is_empty() {
            return Ok(prepped);
        }
        let stored = self.st.store.get_entities(&self.tenant, &comma).await?;
        let mut kept = Vec::with_capacity(prepped.len());
        for (id, doc) in prepped {
            match antares_jsonld::new_entity_id(&doc) {
                Err(e) if !stored.contains_key(&id) => {
                    self.out.errors.push(err_entry(Some(&id), &e));
                }
                _ => kept.push((id, doc)),
            }
        }
        Ok(kept)
    }

    /// One round of an upsert, update or merge, every id in it unique, its
    /// store operations batched: one transaction per round instead of one
    /// per item.
    async fn round(&mut self, round: Vec<Member>) -> Result<(), NgsiError> {
        let ts = now_iso();
        match self.opts.mode {
            // Creates are collected apart and never reach a round; a request
            // handler answers, it does not panic.
            BatchMode::Create => {
                for (id, _) in round {
                    self.out.errors.push(err_entry(
                        Some(&id),
                        &NgsiError::InternalError(
                            "batch create is handled before the rounds".into(),
                        ),
                    ));
                }
                Ok(())
            }
            BatchMode::Upsert => self.upsert(round, &ts).await,
            BatchMode::Update | BatchMode::Merge => self.update(round, &ts).await,
        }
    }

    async fn upsert(&mut self, round: Vec<Member>, ts: &str) -> Result<(), NgsiError> {
        #[cfg(feature = "views")]
        if self.st.view().is_some() {
            let created = upsert_in_view(
                self.st,
                &self.tenant,
                round,
                self.opts.update,
                ts,
                &self.conds,
                &self.known,
                &mut self.out,
            )
            .await?;
            for id in created {
                self.created(id);
            }
            return Ok(());
        }
        let replaces = if self.opts.update {
            self.upsert_merge(round, ts).await?
        } else {
            self.replace_conditioned(round, ts).await?
        };
        if !replaces.is_empty() {
            self.replace(replaces, ts).await?;
        }
        Ok(())
    }

    /// options=update: update existing rows first (Update Attributes); ids that turn out
    /// absent (or vanish mid-flight) fall through to the replace batch —
    /// never a silent success (TOCTOU fix). The members left to replace.
    async fn upsert_merge(
        &mut self,
        round: Vec<Member>,
        ts: &str,
    ) -> Result<Vec<Member>, NgsiError> {
        let ids: Vec<String> = round.iter().map(|(id, _)| id.clone()).collect();
        let docs: HashMap<&str, &Value> = round.iter().map(|(id, d)| (id.as_str(), d)).collect();
        let conds = &self.conds;
        let res = self
            .st
            .store
            .batch_mutate(&self.tenant, &ids, |id, doc| {
                crate::history::restage(id);
                conds.pre(id, doc)?;
                let src = member(&docs, id)?;
                let before = doc.clone();
                // 10.3.3.4: update mode runs Update Attributes (10.2.3.4),
                // which replaces each instance it names whole (T-5657)
                crate::entities::update_into(doc, src, ts);
                // a member that only restates the stored state
                // (a parked vehicle, a poll that saw nothing
                // new) is no change (4.8, 5.8.3): handing the
                // store the document unchanged writes no row,
                // version, history point or notification
                if same_but_stamps(&before, doc) {
                    *doc = before;
                }
                Ok::<(), NgsiError>(())
            })
            .await?;
        crate::history::settle_all(&ids, &res);
        let mut replaces = Vec::new();
        for ((id, expanded), r) in round.iter().zip(res) {
            match r {
                Some(Err(e)) => self.out.errors.push(err_entry(Some(id), &e)),
                Some(Ok(())) => self.out.succeed(id),
                None if self.conds.has(id) => self
                    .out
                    .errors
                    .push(err_entry(Some(id), &Conditions::not_found(id))),
                None => {
                    // F-R2ANT1-009: the member was checked as a fragment; the Entity it now
                    // creates is held to the whole-Entity rules
                    #[cfg(feature = "views")]
                    if let Err(e) =
                        crate::views::schema_admits_typed(self.st, &self.tenant, expanded, true)
                    {
                        self.out.errors.push(err_entry(Some(id), &e));
                        continue;
                    }
                    replaces.push((id.clone(), expanded.clone()));
                }
            }
        }
        Ok(replaces)
    }

    /// A replace-upsert: a conditioned member is replaced only inside the
    /// store's read of it, where its condition is judged. The free members,
    /// left to replace.
    async fn replace_conditioned(
        &mut self,
        round: Vec<Member>,
        ts: &str,
    ) -> Result<Vec<Member>, NgsiError> {
        let (conditioned, free): (Vec<_>, Vec<_>) =
            round.into_iter().partition(|(id, _)| self.conds.has(id));
        if conditioned.is_empty() {
            return Ok(free);
        }
        let ids: Vec<String> = conditioned.iter().map(|(id, _)| id.clone()).collect();
        let docs: HashMap<&str, &Value> =
            conditioned.iter().map(|(id, d)| (id.as_str(), d)).collect();
        let conds = &self.conds;
        let res = self
            .st
            .store
            .batch_mutate(&self.tenant, &ids, |id, doc| {
                conds.pre(id, doc)?;
                let src = member(&docs, id)?;
                *doc = replacement(src, doc, ts);
                Ok::<(), NgsiError>(())
            })
            .await?;
        for (id, r) in ids.iter().zip(res) {
            match r {
                Some(Ok(())) => self.out.succeed(id),
                Some(Err(e)) => self.out.errors.push(err_entry(Some(id), &e)),
                None => self
                    .out
                    .errors
                    .push(err_entry(Some(id), &Conditions::not_found(id))),
            }
        }
        Ok(free)
    }

    /// The replace batch of an upsert round: absent ids are created.
    async fn replace(&mut self, mut replaces: Vec<Member>, ts: &str) -> Result<(), NgsiError> {
        for (_, doc) in replaces.iter_mut() {
            stamp_new(doc, ts);
        }
        // ids first, then MOVE the documents into the store —
        // the loop below only needs the ids, so the batch payload
        // is never deep-cloned on the ingest hot path.
        let ids: Vec<String> = replaces.iter().map(|(id, _)| id.clone()).collect();
        let flags = self.st.store.batch_upsert(&self.tenant, replaces).await?;
        for (id, created) in ids.into_iter().zip(flags) {
            self.out.succeed(&id);
            if created {
                self.created(id);
            }
        }
        Ok(())
    }

    /// One round of a Batch Update or Merge.
    async fn update(&mut self, round: Vec<Member>, ts: &str) -> Result<(), NgsiError> {
        #[cfg(feature = "views")]
        let (round, guards) =
            view_guards(self.st, &self.tenant, round, &self.known, &mut self.out).await;
        let ids: Vec<String> = round.iter().map(|(id, _)| id.clone()).collect();
        let docs: HashMap<&str, &Value> = round.iter().map(|(id, d)| (id.as_str(), d)).collect();
        // batch update with noOverwrite: existing attribute instances
        // are left alone; if any existed, the entity is a partial
        // failure (005_02 ⇒ 207)
        let mut skipped: std::collections::HashSet<String> = Default::default();
        let (conds, opts) = (&self.conds, self.opts);
        let res = self
            .st
            .store
            .batch_mutate(&self.tenant, &ids, |id, doc| {
                crate::history::restage(id);
                conds.pre(id, doc)?;
                #[cfg(feature = "views")]
                let guard = guards.get(id);
                #[cfg(feature = "views")]
                if let Some(g) = guard {
                    g.pre(id, doc)?;
                }
                let expanded = member(&docs, id)?;
                if opts.mode == BatchMode::Update && opts.no_overwrite {
                    if append_new_instances(doc, expanded, ts)? {
                        skipped.insert(id.to_owned());
                    }
                } else if opts.mode == BatchMode::Update {
                    crate::entities::update_into(doc, expanded, ts);
                } else {
                    merge_into(doc, expanded, ts);
                }
                #[cfg(feature = "views")]
                if let Some(g) = guard {
                    g.post(doc)?;
                }
                Ok::<(), NgsiError>(())
            })
            .await?;
        crate::history::settle_all(&ids, &res);
        for ((id, _), r) in round.iter().zip(res) {
            #[cfg(feature = "views")]
            if let Some(crate::policy::Denied(why)) = guards.get(id).and_then(|g| g.verdict().err())
            {
                self.out.errors.push(denied_entry(Some(id), &why));
                continue;
            }
            match r {
                None => self.out.errors.push(err_entry(
                    Some(id),
                    &NgsiError::ResourceNotFound(format!("entity {id} not found")),
                )),
                Some(Err(e)) => self.out.errors.push(err_entry(Some(id), &e)),
                Some(Ok(())) if skipped.contains(id) => self.out.errors.push(err_entry(
                    Some(id),
                    &NgsiError::BadRequestData(
                        "some attributes already existed (noOverwrite)".into(),
                    ),
                )),
                Some(Ok(())) => self.out.succeed(id),
            }
        }
        Ok(())
    }

    /// The collected creates, one multi-row statement, one transaction.
    async fn create(&mut self, creates: Vec<Member>) -> Result<(), NgsiError> {
        // ADR-0023 rule 7 per member: a create through a view lands in it.
        #[cfg(feature = "views")]
        let creates = if self.st.view().is_some() {
            let mut kept = Vec::with_capacity(creates.len());
            for (id, doc) in creates {
                match crate::views::admits_new(self.st, &self.tenant, &doc).await {
                    Ok(()) => kept.push((id, doc)),
                    Err(crate::policy::Denied(why)) => {
                        self.out.errors.push(denied_entry(Some(&id), &why))
                    }
                }
            }
            kept
        } else {
            creates
        };
        if creates.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = creates.iter().map(|(id, _)| id.clone()).collect();
        let flags = self.st.store.batch_create(&self.tenant, creates).await?;
        for (id, created) in ids.into_iter().zip(flags) {
            if created {
                self.out.succeed(&id);
                self.created(id);
            } else {
                self.out.errors.push(err_entry(
                    Some(&id),
                    &NgsiError::AlreadyExists(format!("entity {id} already exists")),
                ));
            }
        }
        Ok(())
    }

    /// What the forwards brought back joins S and E after the local outcome.
    fn merge_remote(&mut self, remote: Remote) {
        for (id, was_created) in remote.ok {
            self.out.succeed(&id);
            if was_created {
                // 5.6.8.4: one Entity ID per Entity, the local and the forwarded
                // part of one Entity included
                self.created(id);
            }
        }
        self.out.errors.extend(remote.err);
    }

    fn created(&mut self, id: String) {
        self.any_created = true;
        if !self.created_ids.contains(&id) {
            self.created_ids.push(id);
        }
    }

    /// 5.6.7.5, 5.6.8.5, 5.6.9.5, 5.6.20.5: the all-ok status, else 207.
    fn finish(mut self) -> Response {
        // 5.6.8.5: the created-only S array is the ALL-SUCCEEDED reading ("if all
        // Entities not existing prior to this request have been successfully
        // created and the others have been successfully updated"). Once E is
        // non-empty the third bullet applies and S is "the list of Entities
        // successfully created or updated".
        if self.opts.mode == BatchMode::Upsert && self.any_created && self.out.errors.is_empty() {
            self.out.success = self.created_ids.into_iter().map(Value::String).collect();
        }
        let (status, body_on_ok) = match self.opts.mode {
            BatchMode::Create => (StatusCode::CREATED, true),
            BatchMode::Upsert if self.any_created => (StatusCode::CREATED, true),
            BatchMode::Upsert | BatchMode::Update | BatchMode::Merge => {
                (StatusCode::NO_CONTENT, false)
            }
        };
        self.out.respond(&self.tenant, status, body_on_ok)
    }
}

/// Each entry's id and, when it expands, its expanded body: what the engine
/// is shown of it.
fn members(items: &[BatchItem]) -> Vec<(Option<String>, Option<Value>)> {
    items
        .iter()
        .map(|(item, ctx)| {
            let id = item.get("id").and_then(Value::as_str).map(str::to_owned);
            let body = match (item.as_object(), ctx.as_ref()) {
                (Some(o), Ok(c)) => expand_entity(o, c, ExpandOpts::default()).ok(),
                _ => None,
            };
            (id, body)
        })
        .collect()
}

/// The Attribute names an expanded member writes.
fn written_attrs(body: Option<&Value>) -> Vec<String> {
    body.and_then(Value::as_object)
        .map(|o| {
            o.keys()
                .filter(|k| !matches!(k.as_str(), "id" | "type" | "scope" | "@context"))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// The part of a member stored here: proxied (exclusive/redirect)
/// attributes are never stored locally. None when every attribute is.
fn local_part(
    item: Value,
    proxies: &[&crate::federation::FedReg],
    ctx: &antares_jsonld::Context,
) -> Option<Value> {
    if proxies.is_empty() {
        return Some(item);
    }
    let Some(o) = item.as_object() else {
        return Some(item);
    };
    let (rest, has_attrs) = crate::federation::strip_proxied(o, proxies, ctx);
    has_attrs.then_some(Value::Object(rest))
}

/// A create member, expanded and stamped with the batch's one instant.
fn prep_create(item: &Value, ctx: &antares_jsonld::Context, ts: &str) -> Result<Member, NgsiError> {
    let obj = item
        .as_object()
        .ok_or_else(|| NgsiError::BadRequestData("entity must be an object".into()))?;
    let mut expanded = expand_entity(obj, ctx, ExpandOpts::default())?;
    let id = antares_jsonld::new_entity_id(&expanded)?.to_owned();
    stamp_new(&mut expanded, ts);
    Ok((id, expanded))
}

/// An upsert, update or merge member, expanded; a merge allows the null
/// that deletes an Attribute.
fn prep_write(
    item: &Value,
    ctx: &antares_jsonld::Context,
    mode: BatchMode,
) -> Result<Member, NgsiError> {
    let obj = item
        .as_object()
        .ok_or_else(|| NgsiError::BadRequestData("entity must be an object".into()))?;
    let fragment_ok = mode == BatchMode::Merge;
    let expanded = expand_entity(
        obj,
        ctx,
        ExpandOpts {
            fragment: false,
            allow_null: fragment_ok,
            merge: fragment_ok,
            temporal: false,
            ..Default::default()
        },
    )?;
    let id = antares_jsonld::expanded_id(&expanded)?.to_owned();
    Ok((id, expanded))
}

/// 4.6.6: duplicate instances of one Entity in a batch array "shall come
/// in chronological order" — first oldest. Sequential semantics are kept
/// by splitting into rounds: the Nth occurrence of an id lands in round
/// N, rounds execute in order, and within a round every id is unique.
fn rounds(prepped: Vec<Member>) -> Vec<Vec<Member>> {
    let mut rounds: Vec<Vec<Member>> = Vec::new();
    let mut occurrence: HashMap<String, usize> = HashMap::new();
    for (id, doc) in prepped {
        let n = occurrence.entry(id.clone()).or_insert(0);
        // `n` never passes `rounds.len()`: each id's count grows by one
        // and a round is added the first time a count reaches it
        match rounds.get_mut(*n) {
            Some(round) => round.push((id, doc)),
            None => rounds.push(vec![(id, doc)]),
        }
        *n = n.saturating_add(1);
    }
    rounds
}

/// The document a store closure writes for `id`; every id the store hands
/// it is one the round names.
fn member<'d>(docs: &HashMap<&str, &'d Value>, id: &str) -> Result<&'d Value, NgsiError> {
    docs.get(id)
        .copied()
        .ok_or_else(|| NgsiError::InternalError(format!("batch member {id} lost its document")))
}

/// `src` stamped as the new state of the stored `doc`. 4.8: a replaced
/// Entity keeps the instant it was created.
fn replacement(src: &Value, doc: &Value, ts: &str) -> Value {
    let mut new = src.clone();
    stamp_new(&mut new, ts);
    if let (Some(o), Some(created)) = (new.as_object_mut(), doc.get("createdAt")) {
        o.insert("createdAt".into(), created.clone());
    }
    new
}

/// 5.6.9 with noOverwrite, which is instance-level: an incoming instance
/// whose datasetId the stored Attribute already holds is skipped, every
/// other one appended. Whether any instance was skipped.
fn append_new_instances(doc: &mut Value, expanded: &Value, ts: &str) -> Result<bool, NgsiError> {
    let mut skipped = false;
    let target = antares_store::stored_object(doc)?;
    for (k, v) in antares_jsonld::expanded_object(expanded)? {
        if matches!(
            k.as_str(),
            "id" | "type" | "scope" | "createdAt" | "modifiedAt"
        ) {
            continue;
        }
        // stamped as every other write path stamps a new instance
        let mut incoming = v.clone();
        crate::stamp::stamp_instances(&mut incoming, ts);
        let incoming: Vec<Value> = match incoming {
            Value::Array(a) => a,
            _ => Vec::new(),
        };
        match target.get_mut(k).and_then(Value::as_array_mut) {
            None => {
                target.insert(k.clone(), Value::Array(incoming));
            }
            Some(cur) => {
                for ni in incoming {
                    let ds = ni.get("datasetId").and_then(Value::as_str);
                    if cur
                        .iter()
                        .any(|ci| ci.get("datasetId").and_then(Value::as_str) == ds)
                    {
                        skipped = true;
                    } else {
                        cur.push(ni);
                    }
                }
            }
        }
    }
    target.insert("modifiedAt".into(), Value::String(ts.to_owned()));
    Ok(skipped)
}

/// The members of a batch round a view lets through its Relationship check,
/// each with the guard its store closure applies (ADR-0023 rule 7); the
/// others land in E. Without a view every member goes, with no guard. The
/// round's targets are read together (T-4085), apart from those `known`
/// already holds: Entities the batch read before its first write and does not
/// itself write, so no round has changed them.
#[cfg(feature = "views")]
async fn view_guards(
    st: &AppState,
    tenant: &antares_model::TenantId,
    round: Vec<(String, Value)>,
    known: &HashMap<String, Value>,
    out: &mut BatchOutcome,
) -> (
    Vec<(String, Value)>,
    HashMap<String, crate::views::WriteGuard>,
) {
    let members: Vec<(String, Value, Option<crate::views::WriteGuard>)> = round
        .into_iter()
        .map(|(id, doc)| {
            let names = crate::attrs::attr_iris_of(&doc);
            let g = crate::views::WriteGuard::of(st, names.iter().map(String::as_str));
            (id, doc, g)
        })
        .collect();
    let mut missing: Vec<&str> = members
        .iter()
        .filter(|(_, _, g)| g.is_some())
        .flat_map(|(_, doc, _)| crate::views::relationship_targets(doc))
        .filter(|t| !known.contains_key(*t))
        .collect();
    missing.sort_unstable();
    missing.dedup();
    let mut found = if missing.is_empty() {
        HashMap::new()
    } else {
        // A store failure reads as absent, as a failed single read did.
        st.store
            .get_entities(tenant, &missing)
            .await
            .unwrap_or_default()
    };
    for t in members
        .iter()
        .filter(|(_, _, g)| g.is_some())
        .flat_map(|(_, doc, _)| crate::views::relationship_targets(doc))
    {
        if let Some(d) = known.get(t) {
            found.entry(t.to_owned()).or_insert_with(|| d.clone());
        }
    }
    let mut guards = HashMap::new();
    let mut kept = Vec::with_capacity(members.len());
    for (id, doc, g) in members {
        let Some(g) = g else {
            kept.push((id, doc));
            continue;
        };
        match g.targets_in(&doc, &found) {
            Ok(()) => {
                guards.insert(id.clone(), g);
                kept.push((id, doc));
            }
            Err(crate::policy::Denied(why)) => out.errors.push(denied_entry(Some(&id), &why)),
        }
    }
    (kept, guards)
}

/// One round of a Batch Upsert under a view, member by member (ADR-0023
/// rule 7, T-3594): a stored Entity the view shows is replaced, or with
/// `options=update` updated, under the store's lock between the pre-image
/// and post-image checks; a replace counts every Attribute the Entity holds
/// as written, so one holding a hidden Attribute is refused (D6). An absent
/// id, and one out of the view, take the create path: the Entity is created
/// in the view or refused, and an out-of-view one is never overwritten,
/// answering as Create Entity through the view does for a taken id. The ids
/// created, in order.
#[cfg(feature = "views")]
#[allow(clippy::too_many_arguments)]
async fn upsert_in_view(
    st: &AppState,
    tenant: &antares_model::TenantId,
    round: Vec<(String, Value)>,
    update_mode: bool,
    ts: &str,
    conds: &Conditions,
    known: &HashMap<String, Value>,
    out: &mut BatchOutcome,
) -> Result<Vec<String>, NgsiError> {
    let (round, guards) = view_guards(st, tenant, round, known, out).await;
    let ids: Vec<String> = round.iter().map(|(id, _)| id.clone()).collect();
    let docs: HashMap<&str, &Value> = round.iter().map(|(id, d)| (id.as_str(), d)).collect();
    let res = st
        .store
        .batch_mutate(tenant, &ids, |id, doc| {
            crate::history::restage(id);
            conds.pre(id, doc)?;
            let guard = guards.get(id);
            if let Some(g) = guard {
                if update_mode {
                    g.pre(id, doc)?;
                } else {
                    g.pre_whole(id, doc)?;
                }
            }
            let Some(&src) = docs.get(id) else {
                return Err(NgsiError::InternalError(format!(
                    "batch member {id} lost its document"
                )));
            };
            if update_mode {
                crate::entities::update_into(doc, src, ts);
            } else {
                let mut new = src.clone();
                stamp_new(&mut new, ts);
                // 4.8: a replaced Entity keeps the instant it was created.
                if let (Some(o), Some(created)) = (new.as_object_mut(), doc.get("createdAt")) {
                    o.insert("createdAt".into(), created.clone());
                }
                *doc = new;
            }
            if let Some(g) = guard {
                g.post(doc)?;
            }
            Ok::<(), NgsiError>(())
        })
        .await?;
    crate::history::settle_all(&ids, &res);
    let mut creates = Vec::new();
    for ((id, doc), r) in round.into_iter().zip(res) {
        if let Some(crate::policy::Denied(why)) = guards.get(&id).and_then(|g| g.verdict().err()) {
            out.errors.push(denied_entry(Some(&id), &why));
            continue;
        }
        match r {
            Some(Ok(())) => {
                if !out.success.contains(&Value::String(id.clone())) {
                    out.success.push(Value::String(id));
                }
            }
            // a conditioned member never creates: absent or not selected,
            // it is not found
            None | Some(Err(NgsiError::ResourceNotFound(_))) if conds.has(&id) => {
                out.errors
                    .push(err_entry(Some(&id), &Conditions::not_found(&id)));
            }
            // absent, or out of the view: the pre-image answers not found
            None | Some(Err(NgsiError::ResourceNotFound(_))) => creates.push((id, doc)),
            Some(Err(e)) => out.errors.push(err_entry(Some(&id), &e)),
        }
    }
    let mut kept = Vec::with_capacity(creates.len());
    for (id, mut doc) in creates {
        match crate::views::admits_new(st, tenant, &doc).await {
            Ok(()) => {
                stamp_new(&mut doc, ts);
                kept.push((id, doc));
            }
            Err(crate::policy::Denied(why)) => out.errors.push(denied_entry(Some(&id), &why)),
        }
    }
    let mut created = Vec::new();
    if kept.is_empty() {
        return Ok(created);
    }
    let ids: Vec<String> = kept.iter().map(|(id, _)| id.clone()).collect();
    let flags = st.store.batch_create(tenant, kept).await?;
    for (id, ok) in ids.into_iter().zip(flags) {
        if !ok {
            let e = NgsiError::AlreadyExists(format!("entity {id} already exists"));
            out.errors.push(err_entry(Some(&id), &e));
        } else if !out.success.contains(&Value::String(id.clone())) {
            out.success.push(Value::String(id.clone()));
            created.push(id);
        }
    }
    Ok(created)
}

fn ngsi_of(e: ApiError) -> NgsiError {
    match e {
        ApiError::Ngsi(n) => n,
        ApiError::Bare(code) => NgsiError::BadRequestData(format!("HTTP {code}")),
        ApiError::NotAcceptable(_) => NgsiError::BadRequestData("HTTP 406".into()),
        // One item of a batch cannot answer 503: the array's other entries
        // may have succeeded, and 5.6.7.4 gives each failure its own
        // ProblemDetails. The item carries the same detail the whole-request
        // answer would, so the cause is not lost in the collapse.
        ApiError::Overloaded(_) => {
            NgsiError::InternalError(antares_model::error::DB_OVERLOADED.into())
        }
        // The policy gate judges each member in `decide` (`gate_each`), which
        // answers a refused member with its own entry (`denied_entry`); only a
        // member's @context reaches here, and it is never refused by policy.
        // 422 is the nearest thing Table 6.3.2-1 has should one ever arrive.
        ApiError::Denied(why) => NgsiError::OperationNotSupported(why),
    }
}

/// `a` and `b` hold the same Entity apart from the `modifiedAt` stamps a
/// merge leaves on the Entity and on each Attribute instance it touched,
/// the writer beside it included (DM-138). Only those two levels are
/// skipped: a `modifiedAt` key inside a value is data and compares like any
/// other.
pub(super) fn same_but_stamps(a: &Value, b: &Value) -> bool {
    fn without(o: &serde_json::Map<String, Value>) -> impl Iterator<Item = (&String, &Value)> {
        o.iter().filter(|(k, _)| {
            !matches!(
                k.as_str(),
                "modifiedAt" | antares_jsonld::compact::MODIFIED_BY_KEY
            )
        })
    }
    fn same_obj(
        a: &serde_json::Map<String, Value>,
        b: &serde_json::Map<String, Value>,
        inner: fn(&Value, &Value) -> bool,
    ) -> bool {
        without(a).count() == without(b).count()
            && without(a).all(|(k, v)| b.get(k).is_some_and(|w| inner(v, w)))
    }
    fn instances(a: &Value, b: &Value) -> bool {
        match (a.as_array(), b.as_array()) {
            (Some(x), Some(y)) => {
                x.len() == y.len()
                    && x.iter()
                        .zip(y)
                        .all(|(i, j)| match (i.as_object(), j.as_object()) {
                            (Some(i), Some(j)) => same_obj(i, j, |v, w| v == w),
                            _ => i == j,
                        })
            }
            _ => a == b,
        }
    }
    match (a.as_object(), b.as_object()) {
        (Some(a), Some(b)) => same_obj(a, b, instances),
        _ => a == b,
    }
}
