// SPDX-License-Identifier: EUPL-1.2
//! The distributed arm of the batch operations (5.6.7.4, 5.6.8.4, 5.6.9.4,
//! 5.6.10.4, 5.6.20.4): what a forward sends, and how a peer's answer merges
//! into the client's S and E arrays.

use super::*;

/// 5.2.17 BatchEntityError for a failed forwarded part — the remote status
/// and detail travel inside the ProblemDetails (5.6.7.4/5.6.10.4: remote
/// error results merge into E).
/// The status a Context Source put in a batch error entry, when it is an
/// HTTP error status; anything else (absent, not a number, outside 400..=599,
/// past u16) is the status its response came back with. A plain `as u16`
/// wrapped 65 736 to 200 and reported the failure as a success.
pub(super) fn remote_status(entry: &Value, fallback: u16) -> u16 {
    entry
        .get("error")
        .and_then(|p| p.get("status"))
        .and_then(Value::as_u64)
        .and_then(|s| u16::try_from(s).ok())
        .filter(|s| (400..=599).contains(s))
        .unwrap_or(fallback)
}

pub(super) fn err_remote(id: Option<&str>, status: u16, detail: &str) -> Value {
    let etype = match status {
        400 => "BadRequestData",
        404 => "ResourceNotFound",
        409 => "AlreadyExists",
        422 => "OperationNotSupported",
        _ => "InternalError",
    };
    json!({
        "entityId": id.unwrap_or("unknown"),
        "error": {
            "type": format!("https://uri.etsi.org/ngsi-ld/errors/{etype}"),
            "title": "distributed operation failed",
            "status": status,
            "detail": detail,
        }
    })
}

/// Merge one forwarded BATCH operation's outcome into the remote S/E sets
/// (5.6.7.4: "Merge the returned list of Entities successfully created with
/// S. Merge the returned list of Entities in Error with E.").
pub(super) fn merge_remote_batch(
    status: u16,
    body: &Value,
    sent_ids: &[String],
    created: bool,
    ok: &mut Vec<(String, bool)>,
    err: &mut Vec<Value>,
) {
    // Only the Entities this broker forwarded may appear in the client's
    // S/E arrays: an id the Context Source names but we never sent is not
    // part of this request's outcome, and the source's own error text is
    // rebuilt from its status rather than relayed verbatim.
    let mine = |id: &str| sent_ids.iter().any(|s| s == id);
    match (status, body) {
        (200..=206, Value::Array(a)) => {
            for id in a.iter().filter_map(Value::as_str).filter(|i| mine(i)) {
                ok.push((id.to_owned(), created));
            }
        }
        (200..=206, _) => {
            for id in sent_ids {
                ok.push((id.clone(), created));
            }
        }
        (207, Value::Object(o)) => {
            if let Some(Value::Array(a)) = o.get("success") {
                for id in a.iter().filter_map(Value::as_str).filter(|i| mine(i)) {
                    ok.push((id.to_owned(), created));
                }
            }
            if let Some(Value::Array(a)) = o.get("errors") {
                for e in a {
                    let Some(id) = e
                        .get("entityId")
                        .and_then(Value::as_str)
                        .filter(|i| mine(i))
                    else {
                        continue;
                    };
                    let remote = remote_status(e, status);
                    err.push(err_remote(
                        Some(id),
                        remote,
                        "forwarded batch operation reported an error for this entity",
                    ));
                }
            }
        }
        _ => {
            for id in sent_ids {
                err.push(err_remote(
                    Some(id),
                    status,
                    &format!("forwarded batch operation returned {status}"),
                ));
            }
        }
    }
}

/// The registration-matching input of a batch (4.3.6.1): the ids, the
/// expanded types and the expanded Attribute names the whole array carries,
/// as one `CsrSpec`, together with the items in the shape a forward sends
/// them. An entry whose body is not an object, or whose `@context` did not
/// resolve, contributes nothing and is left to the local arm to report.
pub(super) fn batch_spec(items: &[BatchItem]) -> (crate::registry::CsrSpec, Vec<FwdItem>) {
    let mut fwd_items = Vec::new();
    let mut spec = crate::registry::CsrSpec::default();
    let (mut types, mut ids, mut attrs) = (Vec::new(), Vec::new(), Vec::new());
    for (item, ctx) in items {
        let (Some(o), Ok(c)) = (item.as_object(), ctx.as_ref()) else {
            continue;
        };
        if let Some(id) = o.get("id").and_then(Value::as_str) {
            ids.push(id.to_owned());
        }
        match o.get("type") {
            Some(Value::String(t)) => types.push(c.expand_key(t)),
            Some(Value::Array(a)) => {
                types.extend(a.iter().filter_map(Value::as_str).map(|t| c.expand_key(t)))
            }
            _ => {}
        }
        for k in o.keys() {
            if !matches!(k.as_str(), "id" | "type" | "scope" | "@context") {
                attrs.push(c.expand_key(k));
            }
        }
        fwd_items.push((o.clone(), c.clone()));
    }
    if !types.is_empty() {
        spec.types = Some(types);
    }
    if !ids.is_empty() {
        spec.ids = Some(ids);
    }
    if !attrs.is_empty() {
        spec.attrs = Some(attrs);
    }
    (spec, fwd_items)
}

/// What the forwards of one batch brought back: the ids a peer applied, each
/// with whether it created the Entity, and the BatchEntityErrors for E.
#[derive(Default)]
pub(super) struct Remote {
    pub(super) ok: Vec<(String, bool)>,
    pub(super) err: Vec<Value>,
}

impl Remote {
    /// The outcome of one forwarded single-Entity request.
    fn one(&mut self, id: &str, status: u16, created: bool) {
        if (200..300).contains(&status) && status != 207 {
            self.ok.push((id.to_owned(), created && status == 201));
        } else {
            self.err.push(err_remote(
                Some(id),
                status,
                &format!("forwarded operation returned {status}"),
            ));
        }
    }
}

/// The distributed arm of a batch write (5.6.7.4, 5.6.8.4, 5.6.9.4,
/// 5.6.20.4): one forwarded request per matching registration, whose
/// outcomes merge into the same S and E arrays the local arm fills, as
/// Entity ids and BatchEntityErrors (5.2.16, 5.2.17) rather than opaque
/// part descriptors.
pub(super) async fn forward_batch(
    st: &AppState,
    params: &HashMap<String, String>,
    headers: &HeaderMap,
    tenant: &antares_model::TenantId,
    opts: WriteOpts,
    fed_regs: &[crate::federation::FedReg],
    fwd_items: &[FwdItem],
) -> Remote {
    let src = fwd_items
        .first()
        .map(|(_, c)| c.source.clone())
        .unwrap_or(Value::Null);
    let fwd = Fwd {
        st,
        headers,
        tenant,
        ctx_url: crate::federation::ctx_link_url(headers, &src),
    };
    let mut query: Vec<(String, String)> = Vec::new();
    if let Some(o) = params.get("options") {
        query.push(("options".into(), o.clone()));
    }
    let mut remote = Remote::default();
    for reg in fed_regs {
        let arr: Vec<Value> = fwd_items
            .iter()
            .filter_map(|(o, c)| crate::federation::reduce_to_scope(o, reg, c))
            .collect();
        if arr.is_empty() {
            continue;
        }
        let sent_ids: Vec<String> = arr
            .iter()
            .filter_map(|e| e.get("id").and_then(Value::as_str).map(str::to_owned))
            .collect();
        let op = opts.mode.batch_op();
        if reg.supports(op) {
            fwd.batch(reg, opts.mode, &query, arr, &sent_ids, &mut remote)
                .await;
        } else if !fwd.singly(reg, opts, arr, &mut remote).await && reg.is_proxy() {
            // 5.6.7.4/5.6.8.4/5.6.9.4/5.6.20.4 last rung: "In case CSR is
            // an exclusive or redirect Context Source Registration, add an
            // Error of type Conflict for each Entity in IN to E."
            for id in &sent_ids {
                remote.err.push(err_entry(
                    Some(id),
                    &NgsiError::Conflict(format!(
                        "registration does not accept the operation {op}"
                    )),
                ));
            }
        }
    }
    remote
}

/// The request context every forward of one batch shares.
struct Fwd<'a> {
    st: &'a AppState,
    headers: &'a HeaderMap,
    tenant: &'a antares_model::TenantId,
    ctx_url: String,
}

impl Fwd<'_> {
    /// The whole reduced array as the peer's own batch operation.
    async fn batch(
        &self,
        reg: &crate::federation::FedReg,
        mode: BatchMode,
        query: &[(String, String)],
        arr: Vec<Value>,
        sent_ids: &[String],
        remote: &mut Remote,
    ) {
        let (status, body, _) = crate::federation::forward(
            self.st,
            reqwest::Method::POST,
            format!(
                "{}/ngsi-ld/v1/entityOperations/{}",
                reg.endpoint,
                mode.batch_path()
            ),
            query,
            self.headers,
            self.tenant,
            reg,
            &self.ctx_url,
            Some(Value::Array(arr)),
        )
        .await;
        // 5.6.8.5: an upsert forward only CREATED entities when the
        // remote said so — 201 (body lists the created ids); a 204
        // means every forwarded entity was updated.
        merge_remote_batch(
            status,
            &body,
            sent_ids,
            mode == BatchMode::Create || (mode == BatchMode::Upsert && status == 201),
            &mut remote.ok,
            &mut remote.err,
        );
    }

    /// 5.6.7.4/5.6.8.4/5.6.9.4/5.6.20.4 single-op fallbacks, one request per
    /// Entity; false when the peer supports none of them. These never
    /// inherit the batch `options` parameter — e.g. options=update is no
    /// Create Entity parameter (5.6.1.3) and would 400 the forward; the
    /// append fallback re-adds noOverwrite explicitly (5.6.9.4).
    async fn singly(
        &self,
        reg: &crate::federation::FedReg,
        opts: WriteOpts,
        arr: Vec<Value>,
        remote: &mut Remote,
    ) -> bool {
        // F-R2ANT1-014: one request per Entity, as many at once as a fan-out sends
        if opts.mode == BatchMode::Upsert && reg.supports("createEntity") {
            let answers =
                crate::federation::fan_out(arr, |ent| self.upsert_one(reg, opts, ent)).await;
            for answer in answers {
                match answer {
                    Ok((id, status, created)) => remote.one(&id, status, created),
                    Err(entry) => remote.err.push(entry),
                }
            }
            return true;
        }
        let Some(call) = BatchFwd::single(reg, opts) else {
            return false;
        };
        let call = &call;
        for (id, status) in crate::federation::fan_out(arr, |ent| self.one(reg, ent, call)).await {
            remote.one(&id, status, call.created);
        }
        true
    }

    /// 5.6.8.4: an upsert forwarded as Create Entity; an Entity the peer
    /// already holds is not a failed upsert — it falls through to the
    /// operation that updates the one that is there.
    /// The id, status and whether it created, or the error entry when no path upserts it.
    async fn upsert_one(
        &self,
        reg: &crate::federation::FedReg,
        opts: WriteOpts,
        ent: Value,
    ) -> Result<(String, u16, bool), Value> {
        let (id, status) = self.one(reg, ent.clone(), &BatchFwd::create()).await;
        if status != 409 {
            return Ok((id, status, true));
        }
        let call = if !opts.update && reg.supports("replaceEntity") {
            BatchFwd::replace()
        } else if opts.update && reg.supports("updateEntity") {
            BatchFwd::update()
        } else {
            // 5.6.8.4: neither replace nor update available
            return Err(err_remote(
                Some(&id),
                422,
                &format!("OperationNotSupported: no upsert path for {id}"),
            ));
        };
        let (id, status) = self.one(reg, ent, &call).await;
        Ok((id, status, call.created))
    }

    async fn one(
        &self,
        reg: &crate::federation::FedReg,
        ent: Value,
        call: &BatchFwd,
    ) -> (String, u16) {
        forward_one(
            self.st,
            reg,
            self.headers,
            self.tenant,
            &self.ctx_url,
            ent,
            call,
        )
        .await
    }
}

/// Where a forwarded batch item lands on the peer.
#[derive(Clone, Copy)]
enum FwdPath {
    Collection,
    Entity,
    Attrs,
}

/// The single-Entity request a batch operation forwards when the peer does
/// not support the batch operation itself — the 5.6.7.4 / 5.6.8.4 / 5.6.9.4 /
/// 5.6.20.4 fallback ladders. 5.6.7 posts to the collection, 5.6.8 replaces
/// the Entity or updates its Attributes, 5.6.9 updates them or appends with
/// overwrite disabled, 5.6.20 merges the Entity; the loop around them is the
/// same. None of them inherits the batch `options` parameter, which is no
/// parameter of the single-Entity operation being carried — 5.6.9.4's
/// noOverwrite is re-added here, explicitly, because that one is.
struct BatchFwd {
    path: FwdPath,
    method: reqwest::Method,
    query: Vec<(String, String)>,
    /// whether a 201 from the peer counts as a creation in the 5.2.16 S array
    created: bool,
}

impl BatchFwd {
    /// The single-Entity operation a peer without the batch operation takes
    /// for each Entity of a create, replace-upsert, update or merge, in the
    /// order of the support ladders; None when it supports none of them.
    /// The upsert that starts with Create Entity is [`Fwd::upsert_one`].
    fn single(reg: &crate::federation::FedReg, opts: WriteOpts) -> Option<Self> {
        match opts.mode {
            BatchMode::Create if reg.supports("createEntity") => Some(Self::create()),
            BatchMode::Upsert if !opts.update && reg.supports("replaceEntity") => {
                Some(Self::replace())
            }
            BatchMode::Upsert if opts.update && reg.supports("updateEntity") => {
                Some(Self::update())
            }
            BatchMode::Update if !opts.no_overwrite && reg.supports("updateEntity") => {
                Some(Self::update())
            }
            // 5.6.20.4 support ladder: no mergeBatch -> per-entity
            // Merge Entity (5.6.17) forwards.
            BatchMode::Merge if reg.supports("mergeEntity") => Some(Self::merge()),
            // 5.6.9.4: append with Attribute overwrite disabled.
            BatchMode::Update if opts.no_overwrite && reg.supports("appendAttrs") => {
                Some(Self::append_no_overwrite())
            }
            _ => None,
        }
    }

    /// 5.6.1 Create Entity.
    fn create() -> Self {
        Self {
            path: FwdPath::Collection,
            method: reqwest::Method::POST,
            query: Vec::new(),
            created: true,
        }
    }
    /// 5.6.4 Replace Entity.
    fn replace() -> Self {
        Self {
            path: FwdPath::Entity,
            method: reqwest::Method::PUT,
            query: Vec::new(),
            created: false,
        }
    }
    /// 5.6.2 Update Entity Attributes.
    fn update() -> Self {
        Self {
            path: FwdPath::Attrs,
            method: reqwest::Method::PATCH,
            query: Vec::new(),
            created: false,
        }
    }
    /// 5.6.17 Merge Entity.
    fn merge() -> Self {
        Self {
            path: FwdPath::Entity,
            method: reqwest::Method::PATCH,
            query: Vec::new(),
            created: false,
        }
    }
    /// 5.6.3 Append Entity Attributes with overwrite disabled (5.6.9.4).
    fn append_no_overwrite() -> Self {
        Self {
            path: FwdPath::Attrs,
            method: reqwest::Method::POST,
            query: vec![("options".into(), "noOverwrite".into())],
            created: false,
        }
    }
}

/// Forward one Entity of the batch to one registration (4.3.6.3) and report
/// the id it was sent under with the status the peer answered. The id is
/// reported back to the client verbatim, so it is kept raw here and
/// percent-encoded per RFC 3986 clause 3.3 only where it becomes a path
/// segment of the forwarded URL.
async fn forward_one(
    st: &AppState,
    reg: &crate::federation::FedReg,
    headers: &HeaderMap,
    tenant: &antares_model::TenantId,
    ctx_url: &str,
    ent: Value,
    call: &BatchFwd,
) -> (String, u16) {
    let id = ent
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let seg = crate::federation::path_segment(&id);
    let base = &reg.endpoint;
    let url = match call.path {
        FwdPath::Collection => format!("{base}/ngsi-ld/v1/entities"),
        FwdPath::Entity => format!("{base}/ngsi-ld/v1/entities/{seg}"),
        FwdPath::Attrs => format!("{base}/ngsi-ld/v1/entities/{seg}/attrs"),
    };
    let (status, _, _) = crate::federation::forward(
        st,
        call.method.clone(),
        url,
        &call.query,
        headers,
        tenant,
        reg,
        ctx_url,
        Some(ent),
    )
    .await;
    (id, status)
}
