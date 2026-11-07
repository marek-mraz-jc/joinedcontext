// SPDX-License-Identifier: EUPL-1.2
//! PgStore slice three: the temporal store over
//! `attr_instances` ROWS. The 0002
//! bridge doc is gone — `temporal_entities` holds only the small `meta`
//! document; every instance lives as a row, reads RECONSTRUCT the doc shape
//! the API layer consumes (so window()/aggregation/presentation are
//! untouched), and writes are deltas, never a full-history rewrite.
//!
//! What this buys: the hypertable/partition
//! machinery acts on the data queries actually read; retention shortens
//! query results; instance pruning and entity paging run in SQL with the
//! `(tenant_id, entity_id, attr_id, observed_at, instance_id)` index (0017).

use antares_model::TenantId;
use serde_json::Value;
use sqlx::postgres::PgPool;
use sqlx::Row;

pub struct PgTemporalStore {
    pool: PgPool,
    /// Set when this instance serves only the temporal seam: it never holds
    /// the entities, so the append guard must not look for them here.
    pub temporal_only: bool,
    keys: Keys,
}

/// The integer history keys' process state (T-5243, docs/src/storage.md
/// "Integer history keys").
#[derive(Default)]
struct Keys {
    /// `history_cutover`, cached once set: only a migration sets it, but a
    /// pod that started before a rolling deploy's migration sees it unset, so
    /// an unset value is read again on every write until it is set
    /// (F-ANT1-013). A test store pins it with `PgTemporalStore::with_cutover`.
    cutover: tokio::sync::OnceCell<Option<chrono::DateTime<chrono::Utc>>>,
    /// (Tenant, IRI) pairs a committed `iri_ids` row holds. A write sends the
    /// dictionary insert only for an IRI missing here, so a warm process
    /// sends no extra statement. An IRI this process inserted is added only
    /// when a later write finds it stored: the inserting transaction may still
    /// roll back. The ids themselves come from `iri_no` inside each
    /// statement, so a stale entry fails a write (23502) instead of keying a
    /// row by an id that names nothing.
    known: std::sync::Mutex<Known>,
    /// Which writes keep short history (T-5249): their rows go to the
    /// unlogged child of `attr_rows`, whatever the cut-over.
    short: std::sync::RwLock<std::sync::Arc<antares_store::ShortHistory>>,
}

/// Pairs one generation of [`Known`] holds before it ages. Two generations
/// hold at most twice this: about 150 B per pair (two `String`s with a
/// 60-byte IRI, the table slot), so ~30 MB per process at the bound.
const KNOWN_GEN: usize = 100_000;

/// The known (Tenant, IRI) set, bounded as a two-generation LRU: a pair
/// found in `old` moves to `new`; when `new` is full it becomes `old` and
/// the previous `old` is dropped. A working set under [`KNOWN_GEN`] pairs
/// never misses; a pair aged out costs its next write one
/// `INSERT ... ON CONFLICT DO NOTHING` round trip, after which it is known
/// again.
#[derive(Default)]
struct Known {
    new: std::collections::HashSet<(String, String)>,
    old: std::collections::HashSet<(String, String)>,
}

impl Known {
    fn contains(&mut self, k: &(String, String)) -> bool {
        if self.new.contains(k) {
            return true;
        }
        match self.old.take(k) {
            Some(k) => {
                self.insert(k);
                true
            }
            None => false,
        }
    }

    fn insert(&mut self, k: (String, String)) {
        if self.new.len() >= KNOWN_GEN {
            self.old = std::mem::take(&mut self.new);
        }
        self.new.insert(k);
    }

    fn retain(&mut self, mut keep: impl FnMut(&(String, String)) -> bool) {
        self.new.retain(&mut keep);
        self.old.retain(keep);
    }
}

impl Keys {
    fn known(&self) -> std::sync::MutexGuard<'_, Known> {
        self.known
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    async fn cutover(
        &self,
        tx: &mut sqlx::PgConnection,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>, sqlx::Error> {
        if let Some(at) = self.cutover.get() {
            return Ok(*at);
        }
        let at = sqlx::query_scalar::<_, Option<chrono::DateTime<chrono::Utc>>>(
            "SELECT at FROM history_cutover",
        )
        .fetch_optional(&mut *tx)
        .await
        .map(Option::flatten)?;
        if at.is_some() {
            // a racing writer may have set it first: both read the same row
            let _ = self.cutover.set(at);
        }
        Ok(at)
    }

    /// Writes the `iri_ids` rows of `iris` the dictionary lacks, inside the
    /// caller's transaction (RLS checks its Tenant), in IRI order so two
    /// writers wait on each other's first sights in one order. `ensured`
    /// holds what this transaction already wrote or found.
    async fn ensure(
        &self,
        tx: &mut sqlx::PgConnection,
        tenant: &TenantId,
        mut iris: Vec<String>,
        ensured: &mut std::collections::HashSet<(String, String)>,
    ) -> Result<(), sqlx::Error> {
        {
            let mut known = self.known();
            iris.retain(|i| {
                let k = (tenant.as_str().to_owned(), i.clone());
                !known.contains(&k) && !ensured.contains(&k)
            });
        }
        if iris.is_empty() {
            return Ok(());
        }
        iris.sort_unstable();
        iris.dedup();
        let fresh: std::collections::HashSet<String> = sqlx::query_scalar(
            "INSERT INTO iri_ids (tenant_id, iri) SELECT $1, unnest($2::text[])
             ON CONFLICT DO NOTHING RETURNING iri",
        )
        .bind(tenant.as_str())
        .bind(&iris)
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .collect();
        let mut known = self.known();
        for i in iris {
            let k = (tenant.as_str().to_owned(), i);
            if !fresh.contains(&k.1) {
                known.insert(k.clone());
            }
            ensured.insert(k);
        }
        Ok(())
    }

    /// Drops what this process knows of `tenant`'s dictionary: its purge, or
    /// a keyed write that found an IRI gone (another pod's purge).
    fn forget(&self, tenant: &TenantId) {
        self.known().retain(|(t, _)| t != tenant.as_str());
    }

    /// Whether a write of an Entity of `types` whose current state is
    /// `shell` keeps short history.
    fn is_short(&self, tenant: &TenantId, types: &[String], shell: &Value) -> bool {
        self.short
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_short(
                tenant.as_str(),
                types,
                shell.get(antares_store::HISTORY_RETENTION).is_some(),
            )
    }
}

/// The dictionary IRIs a write of `doc` for Entity `id` keys its rows by:
/// the Entity id and its Attribute names.
fn iris_of(id: &str, doc: &Value) -> Vec<String> {
    let mut out = vec![id.to_owned()];
    if let Some(o) = doc.as_object() {
        out.extend(
            o.iter()
                .filter(|(k, v)| v.is_array() && !DOC_META.contains(&k.as_str()))
                .map(|(k, _)| k.clone()),
        );
    }
    out
}

/// 4.22: "expiresAt is defined as the system temporal Property at which a
/// certain Entity, Property or Relationship shall become invalid" — an
/// expired temporal entity is absent from every read, ahead of the retention
/// sweep. One shared literal so `query`, its count fallback and `get_range`
/// can never disagree.
///
/// The stamp is jsonb TEXT, so it goes through `try_timestamptz`
/// (0001_init.sql): a bare cast RAISES on anything it cannot parse, and
/// these reads are
/// tenant-wide — one bad stamp would take down the whole tenant's temporal
/// API rather than hide one entity. An unusable stamp reads as no expiry, the
/// same direction the memory arm takes in `filter::expired_at`.
const NOT_EXPIRED: &str =
    "(try_timestamptz(m.meta->>'expiresAt') IS NULL OR try_timestamptz(m.meta->>'expiresAt') > now())";

/// The temporal tables carry the same four extracted columns as `entities`,
/// so they are read by the same function (`super::entity::types_scopes_stamps`)
/// rather than a second copy of it.
use super::entity::types_scopes_stamps as extract;

/// Entity-doc members that are NOT temporal attributes. `scope` may itself be
/// instance-shaped (deletion instances, 020_19/20) — it lives in `meta`
/// verbatim either way, exactly as the bridge kept it unpruned.
const DOC_META: &[&str] = antares_model::ENTITY_META_KEYS;

/// The meta-only document stored in `temporal_entities.meta`.
fn meta_of(doc: &Value) -> Value {
    let mut m = serde_json::Map::new();
    if let Some(obj) = doc.as_object() {
        for k in DOC_META {
            if let Some(v) = obj.get(*k) {
                m.insert((*k).to_owned(), v.clone());
            }
        }
    }
    Value::Object(m)
}

/// Decompose a doc's attribute arrays into row JSON for the multi-row
/// INSERT. The `observed_at` string is derived ONLY from the instance
/// document, so decomposing a reconstructed doc reproduces byte-identical
/// keys (what the mutate diff relies on).
fn decompose(doc: &Value) -> Vec<Value> {
    let mut rows: Vec<Value> = Vec::new();
    if let Some(obj) = doc.as_object() {
        for (attr, instances) in obj {
            if DOC_META.contains(&attr.as_str()) {
                continue;
            }
            let Some(arr) = instances.as_array() else {
                continue;
            };
            for i in arr {
                let s = |k: &str| i.get(k).and_then(Value::as_str);
                // 4.6.3: a comma seconds-fraction is legal in a request and
                // these stamps go straight into `::timestamptz` casts, which
                // refuse it — and the raise lands in the temporal drain,
                // which absorbs it, so the whole request's history would be
                // lost with a 2xx already returned. Timestamps only:
                // `datasetId` is a URI, where a comma is an ordinary
                // character and rewriting it would corrupt the id.
                let ts = |k: &str| s(k).map(antares_store::filter::canonical_datetime);
                let Some(instance_id) = s("instanceId") else {
                    continue; // stamped by the API layer; belt only
                };
                // observed_at falls back through the instance's own
                // timestamps — deletion instances carry only deletedAt and
                // must NOT collapse onto the epoch (retention would reap them)
                let observed = ts("observedAt")
                    .or_else(|| ts("modifiedAt"))
                    .or_else(|| ts("deletedAt"))
                    .or_else(|| ts("createdAt"))
                    .unwrap_or(std::borrow::Cow::Borrowed(crate::store::mem::EPOCH));
                // Provenance is a column, never data (T-4665).
                let mut data = i.clone();
                let provenance = antares_store::take_provenance(&mut data);
                // the `instance_id` column holds it (T-5131): ~70 B a row
                if let Some(o) = data.as_object_mut() {
                    o.remove("instanceId");
                }
                rows.push(serde_json::json!({
                    "attr_id": attr,
                    "instance_id": instance_id,
                    "dataset_id": s("datasetId"),
                    "observed_at": observed,
                    "created_at": ts("createdAt").unwrap_or_else(|| observed.clone()),
                    "modified_at": ts("modifiedAt").unwrap_or(observed),
                    "deleted_at": ts("deletedAt"),
                    "data": data,
                    "provenance": provenance,
                }));
            }
        }
    }
    rows
}

/// One attribute's instances of history rows `alias`, as the reads group
/// them (T-5131): a row stores its instanceId in the `instance_id` column
/// only, so an attribute comes back as `[instances, instance ids]` in one
/// order, which [`unpack`] zips into the instances. A row written before
/// T-5131 still holds `instanceId` in `data`, equal to the column. Building
/// each row in SQL instead (`jsonb_build_array` or `||` per row) cost
/// Postgres 10-35 % more than reading the old documents (240k rows).
fn insts_and_ids(alias: &str) -> String {
    let key = format!("{alias}.created_at, {alias}.observed_at, {alias}.instance_id");
    format!(
        "jsonb_build_array(jsonb_agg({alias}.data ORDER BY {key}), \
         to_jsonb(array_agg({alias}.instance_id ORDER BY {key})))"
    )
}

/// The whole instance document of row `alias`, built in SQL, where a row is
/// copied (attr_versions keeps whole documents) or read rarely. The column
/// equals `data`'s own instanceId on a row written before T-5131.
fn restamp(alias: &str) -> String {
    format!("({alias}.data || jsonb_build_object('instanceId', {alias}.instance_id))")
}

/// The history rows of one Entity over both parents (T-5200, docs/src/storage.md
/// "Integer history keys"): `attr_instances` and, in its row type, `attr_rows`
/// (`attr_rows_of`, migration 0020), which stays empty until the cut-over. The
/// caller's predicates reach each arm's btree; an ordered read with a LIMIT
/// writes its arms out instead, each limited itself.
fn history(tenant: &str, entity: &str) -> String {
    format!(
        "(SELECT * FROM attr_instances WHERE tenant_id = {tenant} AND entity_id = {entity} \
         UNION ALL SELECT * FROM attr_rows_of({tenant}, {entity}))"
    )
}

/// The Entity `m` has a history row passing `preds` (on alias `ai`): the
/// page qualification. Each parent is probed on its own along its btree,
/// `dir` from the end the window opens towards, so the first row read is
/// most likely in the window. An EXISTS over `history` read each Entity's
/// rows oldest first from the widened column bound and fetched every row the
/// text predicate then refused: ~115 rows an Entity on the helsinki-bikes
/// history, past the 30 s statement timeout for 577 Entities (T-5616).
/// `attrs`, the bind of the Attribute IRIs a policy shows, keeps the probe to
/// their rows, one probe per Attribute along the full (tenant, entity, attr,
/// observed_at) prefix: an `attr_id = ANY(..)` filter is no index condition
/// beside the window's column bound, so Postgres sorted every row of the
/// Entity within the widened bound for its LIMIT 1 (dev: 148 rows and a heap
/// fetch each per station, 29k buffers and 5.9 s for 577 stations; T-5616).
/// A `bare` Entity (migration 0030) holds no row and is not probed: each
/// probe reads every weekly partition of its parent, and a type of 19 782
/// Entities without history passed the 30 s statement timeout (T-5835).
pub fn has_history(preds: &str, dir: &str, attrs: Option<usize>) -> String {
    let Some(n) = attrs else {
        return format!(
            "NOT m.bare AND ((SELECT true FROM attr_instances ai \
               WHERE ai.tenant_id = m.tenant_id AND ai.entity_id = m.id{preds} \
               ORDER BY ai.attr_id {dir}, ai.observed_at {dir} LIMIT 1) IS NOT NULL \
             OR (SELECT true FROM attr_rows ai \
               WHERE ai.tenant_id = m.tenant_id AND ai.entity_no = iri_no(m.tenant_id, m.id){preds} \
               ORDER BY ai.attr_no {dir}, ai.observed_at {dir} LIMIT 1) IS NOT NULL)"
        );
    };
    format!(
        "NOT m.bare AND (EXISTS (SELECT 1 FROM unnest(${n}::text[]) s(attr) CROSS JOIN LATERAL \
           (SELECT true FROM attr_instances ai \
             WHERE ai.tenant_id = m.tenant_id AND ai.entity_id = m.id AND ai.attr_id = s.attr{preds} \
             ORDER BY ai.observed_at {dir} LIMIT 1) h) \
         OR EXISTS (SELECT 1 FROM iri_ids i CROSS JOIN LATERAL \
           (SELECT true FROM attr_rows ai \
             WHERE ai.tenant_id = m.tenant_id AND ai.entity_no = iri_no(m.tenant_id, m.id) \
               AND ai.attr_no = i.id{preds} \
             ORDER BY ai.observed_at {dir} LIMIT 1) h \
           WHERE i.tenant_id = m.tenant_id AND i.iri = ANY(${n})))"
    )
}

/// Turns each attribute's `[instances, instance ids]` that
/// [`insts_and_ids`] returned back into its instances.
fn unpack(doc: &mut Value) {
    let Some(o) = doc.as_object_mut() else {
        return;
    };
    for attr in o.values_mut() {
        let Some([insts, ids]) = attr.as_array_mut().map(Vec::as_mut_slice) else {
            continue;
        };
        let (Some(list), Some(ids)) = (insts.as_array_mut(), ids.as_array_mut()) else {
            continue;
        };
        if list.len() != ids.len() || !ids.iter().all(Value::is_string) {
            continue;
        }
        for (inst, id) in list.iter_mut().zip(ids.iter_mut()) {
            if let Some(i) = inst.as_object_mut() {
                i.insert("instanceId".to_owned(), id.take());
            }
        }
        let list = insts.take();
        *attr = list;
    }
}

/// `ANTARES_TEMPORAL_DEDUP=off` records a row per write again; anything else,
/// or unset, records a new instance only (T-4507, see [`unseen`]).
fn temporal_dedup() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("ANTARES_TEMPORAL_DEDUP").as_deref() != Ok("off"))
}

/// An instance's stamps: two instances that differ only in these are the same
/// observation (CIM 009 4.5: a static Property is one instance).
const STAMPS: &str = "ARRAY['instanceId','createdAt','modifiedAt']";

/// The provenance of a row `e` that corrects the stored one (ADR-N-054 2.3).
const CORRECTED: &str = "jsonb_set(COALESCE(NULLIF(e->'provenance', 'null'::jsonb), '{}'::jsonb), '{operation}', '\"correct\"')";

/// The geometry column of an instance row `e` (see `insert_rows`).
const GEO_VALUE: &str = "CASE WHEN jsonb_typeof(e->'data'->'value') = 'object'
                      AND e->'data'->'value'->>'type' IN
                          ('Point','MultiPoint','LineString','MultiLineString',
                           'Polygon','MultiPolygon')
                     THEN try_geomfromgeojson((e->'data'->'value')::text) END";

/// Whether the newest stored instance of row `e`'s (attribute, datasetId) is
/// another instance with the same value (T-4507): one row read by the index
/// (tenant_id, entity_id, attr_id, observed_at, …) read backwards, false when none is stored.
/// The current and the last week first (T-4906): unbounded, the newest row is
/// a merge over every weekly partition, one cold index descent each. A row
/// found there is the newest of all; only an attribute silent for longer
/// reads the whole history, and COALESCE runs that probe for it alone.
/// Each parent (T-5200) is its own ordered arm with its own LIMIT: Postgres
/// merges UNION ALL arms in index order only when each arm is limited itself.
const NEWEST_REPEATS: &str = "COALESCE((
             SELECT x.instance_id <> e->>'instance_id'
                    AND x.data - ARRAY['instanceId','createdAt','modifiedAt']
                        = (e->'data') - ARRAY['instanceId','createdAt','modifiedAt']
             FROM ((SELECT x.instance_id, x.data, x.observed_at FROM attr_instances x
                    WHERE x.tenant_id = $1 AND x.entity_id = $2 AND x.attr_id = e->>'attr_id'
                      AND x.dataset_id IS NOT DISTINCT FROM e->>'dataset_id'
                      AND x.observed_at >= date_trunc('week', now()) - interval '1 week'
                    ORDER BY x.observed_at DESC LIMIT 1)
                   UNION ALL
                   (SELECT x.instance_id, x.data, x.observed_at FROM attr_rows_at($1, $2, e->>'attr_id') x
                    WHERE x.dataset_id IS NOT DISTINCT FROM e->>'dataset_id'
                      AND x.observed_at >= date_trunc('week', now()) - interval '1 week'
                    ORDER BY x.observed_at DESC LIMIT 1)) x
             ORDER BY x.observed_at DESC LIMIT 1), (
             SELECT x.instance_id <> e->>'instance_id'
                    AND x.data - ARRAY['instanceId','createdAt','modifiedAt']
                        = (e->'data') - ARRAY['instanceId','createdAt','modifiedAt']
             FROM ((SELECT x.instance_id, x.data, x.observed_at FROM attr_instances x
                    WHERE x.tenant_id = $1 AND x.entity_id = $2 AND x.attr_id = e->>'attr_id'
                      AND x.dataset_id IS NOT DISTINCT FROM e->>'dataset_id'
                    ORDER BY x.observed_at DESC LIMIT 1)
                   UNION ALL
                   (SELECT x.instance_id, x.data, x.observed_at FROM attr_rows_at($1, $2, e->>'attr_id') x
                    WHERE x.dataset_id IS NOT DISTINCT FROM e->>'dataset_id'
                    ORDER BY x.observed_at DESC LIMIT 1)) x
             ORDER BY x.observed_at DESC LIMIT 1), false)";

/// The earliest and latest `observed_at` of `rows`, bound beside every probe
/// that matches a row by its `observed_at` (T-4906). The equality reaches
/// the partition key only through a join, so a generic plan reads every
/// weekly partition of the Entity; a bound parameter prunes them to the weeks
/// the batch writes before the statement runs. A stamp chrono cannot read
/// leaves the range open, as the cast in the statement judges it.
fn observed_range(rows: &[Value]) -> (String, String) {
    let mut range: Option<(
        chrono::DateTime<chrono::FixedOffset>,
        chrono::DateTime<chrono::FixedOffset>,
    )> = None;
    for r in rows {
        let Some(t) = r["observed_at"]
            .as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        else {
            return ("-infinity".into(), "infinity".into());
        };
        range = Some(match range {
            None => (t, t),
            Some((lo, hi)) => (lo.min(t), hi.max(t)),
        });
    }
    match range {
        Some((lo, hi)) => (lo.to_rfc3339(), hi.to_rfc3339()),
        None => ("-infinity".into(), "infinity".into()),
    }
}

/// The rows of one append that are new observations (T-4507): of rows equal
/// but for their stamps, the first, and of rows with an `observedAt`, one per
/// (attribute, datasetId, observedAt). What the table already holds is
/// `insert_rows`'s to compare.
fn unseen(rows: Vec<Value>) -> Vec<Value> {
    let mut seen = std::collections::HashSet::new();
    rows.into_iter()
        .filter(|r| {
            let data = &r["data"];
            let what = if data.get("observedAt").is_some() {
                r["observed_at"].clone()
            } else {
                let mut d = data.clone();
                if let Some(o) = d.as_object_mut() {
                    for k in ["instanceId", "createdAt", "modifiedAt"] {
                        o.remove(k);
                    }
                }
                d
            };
            seen.insert((
                r["attr_id"].to_string(),
                r["dataset_id"].to_string(),
                what.to_string(),
            ))
        })
        .collect()
}

/// How [`insert_rows`] writes one batch: `dedup` records new observations
/// only, `short` writes every row to the unlogged child (T-5249).
#[derive(Clone, Copy)]
struct Route {
    dedup: bool,
    short: bool,
}

/// Multi-row instance upsert inside the caller's transaction. With `dedup`
/// (the auto-recording append) a row is written only when it is a new
/// observation (T-4507): an instance with an `observedAt` already stored for
/// its (attribute, datasetId) is updated in place, keeping its instanceId,
/// and one without is skipped when the newest stored instance holds the same
/// value. tenant_id is part of every comparison.
///
/// A row observed from the cut-over on goes to `attr_rows` (T-5243), any
/// earlier one to `attr_instances`, a late write included: one observation
/// always meets its stored instance in the same parent. Every row of a
/// `short` write goes to `attr_rows` as `short`, its unlogged child (T-5249).
async fn insert_rows(
    tx: &mut sqlx::PgConnection,
    keys: &Keys,
    ensured: &mut std::collections::HashSet<(String, String)>,
    tenant: &TenantId,
    entity_id: &str,
    rows: Vec<Value>,
    Route { dedup, short }: Route,
) -> Result<(), sqlx::Error> {
    let dedup = dedup && temporal_dedup();
    let rows = if dedup { unseen(rows) } else { rows };
    if rows.is_empty() {
        return Ok(());
    }
    // The row stays locked until this write commits, so the bare sweep skips
    // it, or this waits for the sweep to commit and then sees its mark
    // (migration 0030). Every caller writes the Entity's temporal row first.
    let bare: Option<bool> = sqlx::query_scalar(
        "SELECT bare FROM temporal_entities WHERE tenant_id = $1 AND id = $2 FOR SHARE",
    )
    .bind(tenant.as_str())
    .bind(entity_id)
    .fetch_optional(&mut *tx)
    .await?;
    if bare == Some(true) {
        sqlx::query("UPDATE temporal_entities SET bare = false WHERE tenant_id = $1 AND id = $2")
            .bind(tenant.as_str())
            .bind(entity_id)
            .execute(&mut *tx)
            .await?;
    }
    // The Attributes given an instance in a dataset (migration 0009): lastN
    // keeps every row of these, so the read needs to know them.
    let mut in_dataset: Vec<String> = rows
        .iter()
        .filter(|r| r["dataset_id"].is_string())
        .filter_map(|r| r["attr_id"].as_str().map(str::to_owned))
        .collect();
    in_dataset.sort_unstable();
    in_dataset.dedup();
    // The newest instance whose observedAt the rank may order unlike
    // observed_at (migration 0028): lastN reads past its N newest only there.
    let offset_until = rows
        .iter()
        .filter(|r| !utc_observed(&r["data"]))
        .filter_map(|r| r["observed_at"].as_str())
        .filter_map(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .max();
    let (keyed, text): (Vec<Value>, Vec<Value>) = if short {
        (rows, Vec::new())
    } else {
        let cutover = keys.cutover(tx).await?;
        rows.into_iter().partition(|r| is_keyed(r, cutover))
    };
    if !text.is_empty() {
        insert_text(tx, tenant, entity_id, text, dedup).await?;
    }
    if !keyed.is_empty() {
        let mut iris = vec![entity_id.to_owned()];
        iris.extend(
            keyed
                .iter()
                .filter_map(|r| r["attr_id"].as_str().map(str::to_owned)),
        );
        keys.ensure(tx, tenant, iris, ensured).await?;
        if let Err(e) = insert_keyed(tx, tenant, entity_id, keyed, dedup, short).await {
            // an IRI the known set holds is gone: another pod purged the Tenant
            if matches!(&e, sqlx::Error::Database(d) if d.code().as_deref() == Some("23502")) {
                keys.forget(tenant);
            }
            return Err(e);
        }
    }
    if let Some(t) = offset_until {
        // Every caller writes the Entity's temporal row before its instances.
        sqlx::query(
            "UPDATE temporal_entities SET offset_until = $3
             WHERE tenant_id = $1 AND id = $2 AND offset_until < $3",
        )
        .bind(tenant.as_str())
        .bind(entity_id)
        .bind(t)
        .execute(&mut *tx)
        .await?;
    }
    if !in_dataset.is_empty() {
        // Every caller writes the Entity's temporal row before its instances.
        sqlx::query(
            "UPDATE temporal_entities
             SET dataset_attrs = ARRAY(SELECT DISTINCT x FROM unnest(dataset_attrs || $3) AS x ORDER BY x)
             WHERE tenant_id = $1 AND id = $2 AND NOT dataset_attrs @> $3",
        )
        .bind(tenant.as_str())
        .bind(entity_id)
        .bind(&in_dataset)
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

/// Whether an instance's `observedAt` is a UTC RFC 3339 stamp, whose text
/// key (`dt_key`) orders exactly like its instant. Any other instance, one
/// without observedAt included, may rank unlike `observed_at` in lastN.
fn utc_observed(data: &Value) -> bool {
    data.get("observedAt")
        .and_then(Value::as_str)
        // `dt_key` reads the first 19 bytes as the wall time: only the `T`
        // spelling of the separator orders with the rest
        .filter(|t| t.ends_with('Z') && t.as_bytes().get(10) == Some(&b'T'))
        .is_some_and(|t| {
            chrono::DateTime::parse_from_rfc3339(&antares_store::filter::canonical_datetime(t))
                .is_ok()
        })
}

/// Whether row `r` is observed at or after `cutover`. A stamp chrono cannot
/// read stays in the text shape, where its cast judges it as before.
fn is_keyed(r: &Value, cutover: Option<chrono::DateTime<chrono::Utc>>) -> bool {
    let Some(cut) = cutover else {
        return false;
    };
    r["observed_at"]
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .is_some_and(|t| t >= cut)
}

/// The `attr_rows` key of an instance id and whether it is the broker's own:
/// the uuid of `urn:ngsi-ld:Instance:<uuid>` in its canonical lowercase form,
/// which the read side prints back as is; any other id keys by its uuid v5
/// and stays in `data` as `instanceId` (docs/src/storage.md).
fn instance_no(id: &str) -> (uuid::Uuid, bool) {
    let own = id.strip_prefix("urn:ngsi-ld:Instance:").and_then(|s| {
        uuid::Uuid::try_parse(s)
            .ok()
            .filter(|u| u.hyphenated().to_string() == s)
    });
    match own {
        Some(u) => (u, true),
        None => (
            uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, id.as_bytes()),
            false,
        ),
    }
}

/// [`insert_rows`] into `attr_instances`, the text shape.
async fn insert_text(
    tx: &mut sqlx::PgConnection,
    tenant: &TenantId,
    entity_id: &str,
    rows: Vec<Value>,
    dedup: bool,
) -> Result<(), sqlx::Error> {
    let (lo, hi) = observed_range(&rows);
    // geo_value: extracted per instance when the value LOOKS like a GeoJSON
    // geometry; try_geomfromgeojson (0001_init.sql) maps anything PostGIS rejects to
    // NULL, which the S3 prefilter treats as "reaches the evaluator".
    let rows = Value::Array(rows);
    // The stored instance of the same (attribute, datasetId) a row of `e` repeats.
    // Its tenant and entity predicates stay written in each statement, where the tenant check
    // (tests/attr_instances_tenant_predicate.rs) reads them.
    let same = "x.attr_id = e->>'attr_id'
                AND x.dataset_id IS NOT DISTINCT FROM e->>'dataset_id'
                AND x.instance_id <> e->>'instance_id'";
    // A row a write replaces with another value is a correction: the stored
    // version is kept in attr_versions before the row below takes the next
    // version number in place (T-4665, ADR-N-054 2.1, 2.4). Its two targets
    // are the two below: the same instanceId (the upsert's conflict), and with
    // `dedup` the same (attribute, datasetId, observedAt).
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO attr_versions
           (tenant_id, entity_id, attr_id, instance_id, dataset_id, observed_at, version, data, provenance)
         SELECT x.tenant_id, x.entity_id, x.attr_id, x.instance_id, x.dataset_id, x.observed_at,
                x.version, {x_data}, x.provenance
         FROM jsonb_array_elements($3::jsonb) AS e
         JOIN attr_instances x ON x.tenant_id = $1 AND x.entity_id = $2
           AND x.attr_id = e->>'attr_id' AND x.observed_at = (e->>'observed_at')::timestamptz
           AND x.observed_at BETWEEN $5::timestamptz AND $6::timestamptz
         WHERE (x.instance_id = e->>'instance_id'
                OR ($4 AND e->'data' ? 'observedAt'
                    AND x.dataset_id IS NOT DISTINCT FROM e->>'dataset_id'))
           AND x.data - {STAMPS} IS DISTINCT FROM (e->'data') - {STAMPS}
         ON CONFLICT DO NOTHING",
        x_data = restamp("x")
    )))
    .bind(tenant.as_str())
    .bind(entity_id)
    .bind(&rows)
    .bind(dedup)
    .bind(&lo)
    .bind(&hi)
    .execute(&mut *tx)
    .await?;
    if dedup {
        // Same observedAt, another value: that observation was corrected.
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE attr_instances x
             SET data = (e->'data') - 'instanceId',
                 version = x.version + 1, provenance = {CORRECTED},
                 modified_at = (e->>'modified_at')::timestamptz,
                 deleted_at = (e->>'deleted_at')::timestamptz, geo_value = {GEO_VALUE}
             FROM jsonb_array_elements($3::jsonb) AS e
             WHERE x.tenant_id = $1 AND x.entity_id = $2 AND e->'data' ? 'observedAt' AND {same}
               AND x.observed_at = (e->>'observed_at')::timestamptz
               AND x.observed_at BETWEEN $4::timestamptz AND $5::timestamptz
               AND x.data - {STAMPS} IS DISTINCT FROM (e->'data') - {STAMPS}"
        )))
        .bind(tenant.as_str())
        .bind(entity_id)
        .bind(&rows)
        .bind(&lo)
        .bind(&hi)
        .execute(&mut *tx)
        .await?;
    }
    // `$4` off: today's row per write. On: a row with an observedAt already
    // stored, or without one equal to the newest stored instance, is no new
    // observation. Both probes ride (tenant_id, entity_id, attr_id, observed_at, …)
    // and read one row: the second is NEWEST_REPEATS, never a scan of the
    // attribute's whole history (T-4669: that scan cost ~90 ms a station at a
    // month of history, and a minute's batch of 2,063 stations recorded nothing).
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO attr_instances
           (tenant_id, entity_id, attr_id, instance_id, dataset_id, observed_at,
            created_at, modified_at, deleted_at, data, geo_value, provenance)
         SELECT $1, $2, e->>'attr_id', e->>'instance_id', e->>'dataset_id',
                (e->>'observed_at')::timestamptz, (e->>'created_at')::timestamptz,
                (e->>'modified_at')::timestamptz, (e->>'deleted_at')::timestamptz,
                e->'data', {GEO_VALUE}, NULLIF(e->'provenance', 'null'::jsonb)
         FROM jsonb_array_elements($3::jsonb) AS e
         WHERE NOT $4 OR NOT CASE
           WHEN e->'data' ? 'observedAt' THEN EXISTS (
             SELECT 1 FROM attr_instances x
             WHERE x.tenant_id = $1 AND x.entity_id = $2 AND {same}
               AND x.observed_at = (e->>'observed_at')::timestamptz
               AND x.observed_at BETWEEN $5::timestamptz AND $6::timestamptz)
           ELSE {NEWEST_REPEATS}
         END
         ON CONFLICT (tenant_id, entity_id, attr_id, instance_id, observed_at)
           DO UPDATE SET data = EXCLUDED.data, modified_at = EXCLUDED.modified_at,
                         dataset_id = EXCLUDED.dataset_id,
                         deleted_at = EXCLUDED.deleted_at,
                         geo_value = EXCLUDED.geo_value,
                         version = attr_instances.version + 1,
                         provenance = jsonb_set(COALESCE(EXCLUDED.provenance, '{{}}'::jsonb),
                                                '{{operation}}', '\"correct\"')
           WHERE attr_instances.data - {STAMPS} IS DISTINCT FROM EXCLUDED.data - {STAMPS}"
    )))
    .bind(tenant.as_str())
    .bind(entity_id)
    .bind(&rows)
    .bind(dedup)
    .bind(&lo)
    .bind(&hi)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// [`insert_rows`] into `attr_rows`: [`insert_text`]'s three statements over
/// the integer keys. `iri_no` binds the dictionary ids [`Keys::ensure`] wrote;
/// `instance_no` keys the instance, and a client's own id rides in `data`.
async fn insert_keyed(
    tx: &mut sqlx::PgConnection,
    tenant: &TenantId,
    entity_id: &str,
    mut rows: Vec<Value>,
    dedup: bool,
    short: bool,
) -> Result<(), sqlx::Error> {
    for r in &mut rows {
        let id = r["instance_id"].as_str().unwrap_or_default().to_owned();
        let (no, own) = instance_no(&id);
        r["instance_no"] = Value::String(no.to_string());
        if !own {
            if let Some(d) = r["data"].as_object_mut() {
                d.insert("instanceId".to_owned(), Value::String(id));
            }
        }
    }
    let (lo, hi) = observed_range(&rows);
    let rows = Value::Array(rows);
    // the stored instance of the same (attribute, datasetId) a row of `e` repeats
    let same = "x.attr_no = iri_no($1, e->>'attr_id')
                AND x.dataset_id IS NOT DISTINCT FROM e->>'dataset_id'
                AND x.instance_no <> (e->>'instance_no')::uuid";
    // attr_versions keeps whole documents in the text shape (ADR-N-054 2.4)
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO attr_versions
           (tenant_id, entity_id, attr_id, instance_id, dataset_id, observed_at, version, data, provenance)
         SELECT x.tenant_id, x.entity_id, x.attr_id, x.instance_id, x.dataset_id, x.observed_at,
                x.version, {x_data}, x.provenance
         FROM jsonb_array_elements($3::jsonb) AS e
         CROSS JOIN LATERAL attr_rows_at($1, $2, e->>'attr_id') x
         WHERE x.observed_at = (e->>'observed_at')::timestamptz
           AND x.observed_at BETWEEN $5::timestamptz AND $6::timestamptz
           AND (x.instance_id = e->>'instance_id'
                OR ($4 AND e->'data' ? 'observedAt'
                    AND x.dataset_id IS NOT DISTINCT FROM e->>'dataset_id'))
           AND x.data - {STAMPS} IS DISTINCT FROM (e->'data') - {STAMPS}
         ON CONFLICT DO NOTHING",
        x_data = restamp("x")
    )))
    .bind(tenant.as_str())
    .bind(entity_id)
    .bind(&rows)
    .bind(dedup)
    .bind(&lo)
    .bind(&hi)
    .execute(&mut *tx)
    .await?;
    if dedup {
        // Same observedAt, another value: corrected in place, keeping the
        // stored instance's id (a client's one in `data`).
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE attr_rows x
             SET data = ((e->'data') - 'instanceId')
                        || CASE WHEN x.data ? 'instanceId'
                                THEN jsonb_build_object('instanceId', x.data->'instanceId')
                                ELSE '{{}}'::jsonb END,
                 version = x.version + 1, provenance = {CORRECTED},
                 modified_at = (e->>'modified_at')::timestamptz,
                 deleted_at = (e->>'deleted_at')::timestamptz, geo_value = {GEO_VALUE}
             FROM jsonb_array_elements($3::jsonb) AS e
             WHERE x.tenant_id = $1 AND x.entity_no = iri_no($1, $2)
               AND e->'data' ? 'observedAt' AND {same}
               AND x.observed_at = (e->>'observed_at')::timestamptz
               AND x.observed_at BETWEEN $4::timestamptz AND $5::timestamptz
               AND x.data - {STAMPS} IS DISTINCT FROM (e->'data') - {STAMPS}"
        )))
        .bind(tenant.as_str())
        .bind(entity_id)
        .bind(&rows)
        .bind(&lo)
        .bind(&hi)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO attr_rows
           (tenant_id, entity_no, attr_no, instance_no, dataset_id, observed_at,
            created_at, modified_at, deleted_at, data, geo_value, provenance, short)
         SELECT $1, iri_no($1, $2), iri_no($1, e->>'attr_id'), (e->>'instance_no')::uuid,
                e->>'dataset_id',
                (e->>'observed_at')::timestamptz, (e->>'created_at')::timestamptz,
                (e->>'modified_at')::timestamptz, (e->>'deleted_at')::timestamptz,
                e->'data', {GEO_VALUE}, NULLIF(e->'provenance', 'null'::jsonb), $7
         FROM jsonb_array_elements($3::jsonb) AS e
         WHERE NOT $4 OR NOT CASE
           WHEN e->'data' ? 'observedAt' THEN EXISTS (
             SELECT 1 FROM attr_rows x
             WHERE x.tenant_id = $1 AND x.entity_no = iri_no($1, $2) AND {same}
               AND x.observed_at = (e->>'observed_at')::timestamptz
               AND x.observed_at BETWEEN $5::timestamptz AND $6::timestamptz)
           ELSE {NEWEST_REPEATS}
         END
         ON CONFLICT (tenant_id, entity_no, attr_no, observed_at, instance_no, short)
           DO UPDATE SET data = EXCLUDED.data, modified_at = EXCLUDED.modified_at,
                         dataset_id = EXCLUDED.dataset_id,
                         deleted_at = EXCLUDED.deleted_at,
                         geo_value = EXCLUDED.geo_value,
                         version = attr_rows.version + 1,
                         provenance = jsonb_set(COALESCE(EXCLUDED.provenance, '{{}}'::jsonb),
                                                '{{operation}}', '\"correct\"')
           WHERE attr_rows.data - {STAMPS} IS DISTINCT FROM EXCLUDED.data - {STAMPS}"
    )))
    .bind(tenant.as_str())
    .bind(entity_id)
    .bind(&rows)
    .bind(dedup)
    .bind(&lo)
    .bind(&hi)
    .bind(short)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// Removes every instance and every kept version of one Entity: what 5.6.16
/// and an expired Entity's reap take, versions included (ADR-N-054 2.4).
async fn wipe_instances(
    tx: &mut sqlx::PgConnection,
    tenant: &TenantId,
    id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM attr_instances WHERE tenant_id = $1 AND entity_id = $2")
        .bind(tenant.as_str())
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM attr_rows WHERE tenant_id = $1 AND entity_no = iri_no($1, $2)")
        .bind(tenant.as_str())
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM attr_versions WHERE tenant_id = $1 AND entity_id = $2")
        .bind(tenant.as_str())
        .bind(id)
        .execute(&mut *tx)
        .await?;
    Ok(())
}

// TemporalFilter lives in `store::filter`; re-exported for path compat.
pub use crate::store::filter::{TemporalFilter, TemporalOutcome};

/// The correlated subquery reconstructing the attribute object for the meta
/// row aliased `m`, with the 4.11 range and the lastN RANK() cap applied over
/// the rows (byte-exact against the API window: predicates and ordering run
/// on the instance JSON with COLLATE "C", never on the partition column).
/// Returns the SQL fragment + its text binds, numbered from `first_bind`;
/// `None` when a range is present but outside the compiler's exact subset —
/// the caller then reconstructs unpruned and the window stays the arbiter.
/// `only`, the bind of a `text[]` of Attribute IRIs, keeps the read to their
/// rows (T-6346: a one-Attribute week of one station read every Attribute's
/// rows, ~1 heap page each, 3.4 s on dev).
fn attr_object_expr(
    f: &TemporalFilter<'_>,
    first_bind: usize,
    only: Option<usize>,
) -> Option<(String, Vec<String>)> {
    let (mut range_and, mut binds) = window_sql(f, first_bind)?;
    if let Some(k) = only {
        range_and.push_str(&format!(" AND ai.attr_id = ANY(${k}::text[])"));
    }
    let tp = first_bind;
    let expr = match f.last_n {
        Some(n) => {
            let n_bind = first_bind.saturating_add(binds.len());
            binds.push(n.to_string());
            // 4.11 lastN keeps the N most recent INSTANTS, so the rank orders
            // on the same canonical key the window compares on — raw bytes
            // put "…00.000Z" after "…00Z" and kept the wrong N.
            let order_key = crate::compile::temporal::dt_key_sql(&format!("(ai.data ->> ${tp})"));
            // firstN keeps the N oldest (TS 104 176 6.4.7.3), the exact
            // reverse: the API window sorts an instance without the
            // timeproperty before every instant, so it ranks first ascending
            let dir = if f.oldest_first {
                "ASC NULLS FIRST"
            } else {
                "DESC NULLS LAST"
            };
            let rows = newest_rows(f, first_bind, n_bind, only)
                .unwrap_or_else(|| format!("{} ai WHERE true", history("m.tenant_id", "m.id")));
            format!(
                "COALESCE((SELECT jsonb_object_agg(g.attr_id, g.insts) FROM (\
                   SELECT s.attr_id, {s_insts} AS insts \
                   FROM (SELECT ai.*, rank() OVER (PARTITION BY ai.attr_id, ai.data ->> 'datasetId' \
                             ORDER BY {order_key} {dir}) AS rk \
                         FROM {rows}{range_and}) s \
                   WHERE s.rk <= ${n_bind}::bigint GROUP BY s.attr_id) g), '{{}}'::jsonb)",
                s_insts = insts_and_ids("s")
            )
        }
        None => {
            // No range and no lastN: nothing names the timeproperty, so the
            // bind `window_sql` reserved for it is not a parameter of the
            // statement the caller assembles. Handing it back anyway would
            // make every plain Retrieve Temporal Evolution bind one argument
            // more than its SQL declares.
            if f.range.is_none() {
                binds.clear();
            }
            format!(
                "COALESCE((SELECT jsonb_object_agg(g.attr_id, g.insts) FROM (\
                   SELECT ai.attr_id, {ai_insts} AS insts \
                   FROM {rows} ai \
                   WHERE true{range_and} \
                   GROUP BY ai.attr_id) g), '{{}}'::jsonb)",
                ai_insts = insts_and_ids("ai"),
                rows = history("m.tenant_id", "m.id")
            )
        }
    };
    Some((expr, binds))
}

/// [`newest_rows`]' statement: `{window}` is its range bound on `t`
/// (`{xwindow}`, `{rwindow}` the same on `x` and `r`), `${n}` the bind of N.
///
/// Distinct Attributes by a skip scan of the same btree, in each parent
/// (T-5200), inside the window's column bound: an Attribute with no row
/// there contributes no row anyway, and the bound keeps the scan to the
/// partitions the window reaches instead of every week of the retention
/// (T-5616); `OFFSET 0` keeps the instance read a parameterised index
/// range per Attribute, which a hash join over the whole Entity would
/// otherwise replace. The N-th newest takes the newest N of each parent,
/// each arm limited itself so both read their btree backwards.
const NEWEST_ROWS: &str = "(WITH RECURSIVE a(attr) AS (\
            SELECT min(x.attr_id) FROM attr_instances x \
            WHERE x.tenant_id = m.tenant_id AND x.entity_id = m.id{xwindow} \
          UNION ALL \
            SELECT (SELECT min(x.attr_id) FROM attr_instances x \
                    WHERE x.tenant_id = m.tenant_id AND x.entity_id = m.id AND x.attr_id > a.attr{xwindow}) \
            FROM a WHERE a.attr IS NOT NULL), \
          b(no) AS (\
            SELECT min(r.attr_no) FROM attr_rows r \
            WHERE r.tenant_id = m.tenant_id AND r.entity_no = iri_no(m.tenant_id, m.id){rwindow} \
          UNION ALL \
            SELECT (SELECT min(r.attr_no) FROM attr_rows r \
                    WHERE r.tenant_id = m.tenant_id AND r.entity_no = iri_no(m.tenant_id, m.id) \
                      AND r.attr_no > b.no{rwindow}) \
            FROM b WHERE b.no IS NOT NULL), \
          n(attr) AS ({names}) \
          SELECT ai.* FROM n a \
          CROSS JOIN LATERAL (SELECT CASE WHEN a.attr = ANY(m.dataset_attrs) THEN '-infinity'::timestamptz \
              ELSE (SELECT CASE WHEN k.nth - interval '28 hours' > m.offset_until THEN k.nth \
                ELSE k.nth - interval '28 hours' END FROM (SELECT COALESCE((SELECT t.observed_at FROM (\
                (SELECT t.observed_at FROM attr_instances t \
                  WHERE t.tenant_id = m.tenant_id AND t.entity_id = m.id AND t.attr_id = a.attr{window} \
                  ORDER BY t.observed_at DESC LIMIT greatest(${n}::bigint, 1)) \
                UNION ALL \
                (SELECT t.observed_at FROM attr_rows_at(m.tenant_id, m.id, a.attr) t \
                  WHERE true{window} \
                  ORDER BY t.observed_at DESC LIMIT greatest(${n}::bigint, 1))) t \
                ORDER BY t.observed_at DESC OFFSET greatest(${n}::bigint - 1, 0) LIMIT 1), \
                '-infinity'::timestamptz) AS nth) k) END AS th) c \
          CROSS JOIN LATERAL (SELECT * FROM attr_instances ai \
            WHERE ai.tenant_id = m.tenant_id AND ai.entity_id = m.id AND ai.attr_id = a.attr \
            AND ai.observed_at >= c.th{awindow} \
            UNION ALL SELECT * FROM attr_rows_at(m.tenant_id, m.id, a.attr) ai \
            WHERE ai.observed_at >= c.th{awindow} OFFSET 0) ai) ai WHERE true";

/// [`NEWEST_ROWS`]' Attributes when no `only` names them: the skip scans'.
const SKIPPED_ATTRS: &str = "SELECT a.attr FROM a WHERE a.attr IS NOT NULL \
          UNION \
            SELECT i.iri FROM b JOIN iri_ids i ON i.tenant_id = m.tenant_id AND i.id = b.no";

/// lastN's candidate rows, read through the (tenant, entity, attr,
/// observed_at DESC) btree instead of ranking every instance of the window:
/// per Attribute, the rows at most 28 hours older than its N-th newest
/// `observed_at`. A SUPERSET of what the rank keeps: the parsed column and
/// the text key the rank orders on diverge by at most the two RFC 3339
/// offsets (±14 h each), so a row more than 28 h older than the N-th newest
/// has N rows strictly above it under either order. KPI histories of 22k
/// instances per Entity passed the gateway's 30 s ranking all of them
/// (T-4246); this reads about N plus 28 hours of rows per Attribute.
/// The rows themselves keep to the window's column bound as well: a window
/// holding fewer than N rows has no N-th newest (`-infinity`), and without
/// the bound that read every week of the Attribute's retention (T-6346:
/// 20 546 rows from disk to keep 288, past the 30 s statement timeout).
/// `only` (see [`attr_object_expr`]) replaces the skip scans by its list.
/// Where the Entity's `offset_until` (migration 0028) lies more than 28 h
/// before the N-th newest, every row within those 28 h is a UTC stamp,
/// whose text key orders like `observed_at`: the N newest and their ties are
/// then exactly what the rank keeps, and the read stops there (T-5616: the
/// helsinki-bikes stations write ~1200 rows a day each).
///
/// `None` (rank the whole window, as before) where the superset argument
/// does not hold or no index serves it:
/// - a timeproperty other than observedAt (no btree on its column);
/// - firstN, and a window closed towards now (`before`, `between`): rows the
///   window drops could raise the N-th newest above what it keeps;
/// - an Attribute with any instance in a dataset: the rank partitions by
///   datasetId, and a sparse dataset's newest may sit below the Attribute's
///   N-th; such an Attribute keeps every row (`-infinity`), found through the
///   Entity's `dataset_attrs` (migration 0009), which the write path keeps.
fn newest_rows(
    f: &TemporalFilter<'_>,
    first_bind: usize,
    n_bind: usize,
    only: Option<usize>,
) -> Option<String> {
    if f.oldest_first || f.timeproperty != "observedAt" {
        return None;
    }
    let bound = |alias| match &f.range {
        None => Some(String::new()),
        Some(r) if r.timerel == "any" => Some(String::new()),
        Some(r) if r.timerel == "after" => {
            crate::compile::temporal::column_range_bound(r, alias, first_bind.saturating_add(1))
                .map(|b| format!(" AND {b}"))
        }
        Some(_) => None,
    };
    let names = match only {
        Some(k) => format!("SELECT DISTINCT unnest(${k}::text[])"),
        None => SKIPPED_ATTRS.to_owned(),
    };
    Some(
        NEWEST_ROWS
            .replace("{names}", &names)
            .replace("{awindow}", &bound("ai")?)
            .replace("{window}", &bound("t")?)
            .replace("{xwindow}", &bound("x")?)
            .replace("{rwindow}", &bound("r")?)
            .replace("{n}", &n_bind.to_string()),
    )
}

/// The 4.11 window over the instance rows `ai` of the meta row `m`: the
/// ` AND …` fragment plus its text binds — $first_bind is always the
/// timeproperty (predicate member / order key), the range binds follow.
/// `None` when the range is outside the compiler's exact subset.
fn window_sql(f: &TemporalFilter<'_>, first_bind: usize) -> Option<(String, Vec<String>)> {
    let mut binds = vec![f.timeproperty.to_owned()];
    let mut range_and = String::new();
    if let Some(r) = &f.range {
        let c = crate::compile::temporal::compile_instance_range(r, "ai.data", first_bind)?;
        range_and = format!(" AND {}", c.sql);
        debug_assert_eq!(c.binds.first().map(String::as_str), Some(f.timeproperty));
        binds.extend(c.binds.into_iter().skip(1));
        // widened COLUMN bound on the SAME binds ($first_bind+1 = timeAt):
        // lets the (tenant, entity, attr, observed_at) btree serve the range;
        // the byte-exact text predicate above still decides membership
        if let Some(cb) =
            crate::compile::temporal::column_range_bound(r, "ai", first_bind.saturating_add(1))
        {
            range_and.push_str(&format!(" AND {cb}"));
        }
    }
    Some((range_and, binds))
}

/// One Entity's Retrieve Temporal Evolution statement
/// ([`PgTemporalStore::get_range`]): `$1` the Tenant, `$2` the Entity id,
/// `$3` the `only` Attribute IRIs when there are any, then `binds` as text.
/// Public so a test can EXPLAIN exactly what the read runs (T-6346).
pub struct RangeStatement {
    pub sql: String,
    pub only: Option<Vec<String>>,
    pub binds: Vec<String>,
}

/// Builds [`RangeStatement`] for `f`. Same unpruned fallback as
/// `query_inner`, and compiling for the same reason: no range and no lastN
/// is the shape that never refuses. A request reaches this: a refusal is an
/// error, never a panic (T-3915). `attrs` keeps the read to their rows.
pub fn range_statement(f: &TemporalFilter<'_>) -> Result<RangeStatement, sqlx::Error> {
    let only = f.attrs.map(super::storable).filter(|a| !a.is_empty());
    let (first, only_bind) = match only {
        Some(_) => (4, Some(3)),
        None => (3, None),
    };
    let (attr_expr, binds) = match attr_object_expr(f, first, only_bind) {
        Some(v) => v,
        None => {
            attr_object_expr(&TemporalFilter::default(), first, only_bind).ok_or_else(|| {
                sqlx::Error::Protocol("the unpruned temporal shape did not compile".into())
            })?
        }
    };
    // 4.22: an expired entity is invalid → None (404), same as get().
    let sql = format!(
        "SELECT m.meta || {attr_expr} FROM temporal_entities m \
         WHERE m.tenant_id = $1 AND m.id = $2 AND {NOT_EXPIRED}"
    );
    Ok(RangeStatement { sql, only, binds })
}

/// Timestamp text in the shape the API's aggregated rows carry.
const BUCKET_TS: &str = r#"to_char({} AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"')"#;

/// 4.5.19 / 5.7.4.4 aggregated temporal representation computed in SQL for
/// the meta row `m`: per attribute the bucket matrix of every requested
/// method as `[value, start, end]` rows, in one object
/// `{"bad": <any windowed value non-numeric>, "attrs": {iri: {"type":
/// "Property", method: rows…}}}`. Only the numeric class (numbers and
/// booleans, Table 4.5.19.1-1) is computed here; `bad` tells the caller to
/// reconstruct instead so the API keeps every other class and the
/// eligibility errors. Buckets: `period_secs` wide from the anchor (the
/// request's timeAt, else the attribute's first instant); no period = one
/// bucket spanning the query's whole time range (4.5.19.1 PT0S), with the
/// edge 4.11 leaves open closed by the attribute's own first or last
/// instant — the API's own rule.
fn aggregate_expr(
    f: &TemporalFilter<'_>,
    agg: &crate::store::filter::Aggregate<'_>,
    first_bind: usize,
) -> Option<(String, Vec<String>)> {
    let (range_and, mut binds) = window_sql(f, first_bind)?;
    let tp = first_bind;
    let col = match f.timeproperty {
        "observedAt" => "observed_at",
        "createdAt" => "created_at",
        "modifiedAt" => "modified_at",
        _ => return None,
    };
    let anchor = match agg.anchor {
        Some(a) => {
            binds.push(antares_store::filter::canonical_datetime(a).into_owned());
            format!(
                "${}::timestamptz",
                first_bind.saturating_add(binds.len()).saturating_sub(1)
            )
        }
        None => format!("min(ai.{col}) OVER (PARTITION BY ai.attr_id)"),
    };
    let (bs, be) = match agg.period_secs {
        None => {
            // 4.5.19.1: a zero duration "is interpreted as a duration
            // spanning the whole time range specified by the temporal
            // query". `before` names only the range's end and `after` only
            // its start (4.11), so the data closes the other edge.
            let (qs, qe) = match &f.range {
                Some(r) if r.timerel == "before" => (None, Some(r.time_at)),
                Some(r) if r.timerel == "between" => (Some(r.time_at), r.end_time_at),
                Some(r) if r.timerel == "after" => (Some(r.time_at), None),
                _ => (None, None),
            };
            let start = match qs {
                Some(v) => {
                    binds.push(antares_store::filter::canonical_datetime(v).into_owned());
                    format!(
                        "${}::timestamptz",
                        first_bind.saturating_add(binds.len()).saturating_sub(1)
                    )
                }
                None => "s0.first_ts".to_owned(),
            };
            let end = match qe {
                Some(v) => {
                    binds.push(antares_store::filter::canonical_datetime(v).into_owned());
                    format!(
                        "${}::timestamptz",
                        first_bind.saturating_add(binds.len()).saturating_sub(1)
                    )
                }
                None => "s0.last + interval '1 second'".to_owned(),
            };
            (start, end)
        }
        Some(sc) => {
            binds.push(sc.to_string());
            let n = first_bind.saturating_add(binds.len()).saturating_sub(1);
            let start = format!("date_bin(make_interval(secs => ${n}::bigint), s0.ts, s0.anchor)");
            (
                start.clone(),
                format!("{start} + make_interval(secs => ${n}::bigint)"),
            )
        }
    };
    let mut aggs = String::new();
    let mut rows = String::new();
    let mut pairs = String::new();
    for (i, m) in agg.methods.iter().enumerate() {
        // The SQL and the member name both come out of this table, so the
        // request's own string never reaches the statement (T-3464).
        let (name, sql) = match m.as_str() {
            "totalCount" => ("totalCount", "count(*)"),
            "distinctCount" => ("distinctCount", "count(DISTINCT s.v)"),
            "min" => ("min", "min(s.v)"),
            "max" => ("max", "max(s.v)"),
            "sum" => ("sum", "sum(s.v)"),
            "avg" => ("avg", "avg(s.v)"),
            "stddev" => ("stddev", "stddev_pop(s.v)"),
            "sumsq" => ("sumsq", "sum(s.v * s.v)"),
            _ => return None,
        };
        aggs.push_str(&format!(", {sql} AS m{i}"));
        rows.push_str(&format!(
            ", jsonb_agg(jsonb_build_array(g.m{i}, {}, {}) ORDER BY g.bs) AS r{i}",
            BUCKET_TS.replace("{}", "g.bs"),
            BUCKET_TS.replace("{}", "g.be")
        ));
        pairs.push_str(&format!(", '{name}', b.r{i}"));
    }
    let hist = history("m.tenant_id", "m.id");
    let expr = format!(
        "(SELECT jsonb_build_object('bad', bool_or(b.bad), 'attrs', \
            jsonb_object_agg(b.attr_id, jsonb_build_object('type', 'Property'{pairs}))) \
          FROM (SELECT g.attr_id, bool_or(g.bad) AS bad{rows} \
                FROM (SELECT s.attr_id, s.bs, s.be, bool_or(s.bad) AS bad{aggs} \
                      FROM (SELECT s0.attr_id, s0.v, s0.bad, {bs} AS bs, {be} AS be \
                            FROM (SELECT ai.attr_id, \
                                   CASE WHEN jsonb_typeof(ai.data -> 'value') = 'boolean' \
                                          THEN (CASE WHEN (ai.data ->> 'value')::boolean THEN 1 ELSE 0 END)::float8 \
                                        WHEN jsonb_typeof(ai.data -> 'value') = 'number' \
                                          THEN (ai.data ->> 'value')::float8 END AS v, \
                                   jsonb_typeof(ai.data -> 'value') IS DISTINCT FROM 'number' \
                                     AND jsonb_typeof(ai.data -> 'value') IS DISTINCT FROM 'boolean' AS bad, \
                                   ai.{col} AS ts, {anchor} AS anchor, \
                                   min(ai.{col}) OVER (PARTITION BY ai.attr_id) AS first_ts, \
                                   max(ai.{col}) OVER (PARTITION BY ai.attr_id) AS last \
                                  FROM {hist} ai \
                                  WHERE jsonb_typeof(ai.data -> ${tp}) = 'string'{range_and}) s0) s \
                      GROUP BY s.attr_id, s.bs, s.be) g \
                GROUP BY g.attr_id) b)"
    );
    Some((expr, binds))
}

impl PgTemporalStore {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            temporal_only: false,
            keys: Keys::default(),
        }
    }

    /// A store that routes by `cutover` instead of `history_cutover`, so a
    /// test sets its own cut-over without moving every other test's rows.
    #[cfg(any(test, feature = "test-kit"))]
    pub fn with_cutover(pool: PgPool, cutover: Option<chrono::DateTime<chrono::Utc>>) -> Self {
        let mut s = Self::new(pool);
        s.keys.cutover = tokio::sync::OnceCell::new_with(Some(cutover));
        s
    }

    /// Replaces which writes keep short history (T-5249) with `short`.
    pub fn set_short_history(&self, short: antares_store::ShortHistory) {
        *self
            .keys
            .short
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = std::sync::Arc::new(short);
    }

    /// Forgets the dictionary entries of `tenant` after its purge.
    pub fn forget_tenant(&self, tenant: &TenantId) {
        self.keys.forget(tenant);
    }

    /// `false` when the id already exists (create semantics, like the memory
    /// store's `create`).
    ///
    /// 4.22: an expired one does NOT exist, so it is dropped here — history
    /// included — and the insert below is then an ordinary create. Leaving it
    /// in place would make `create` report a conflict for an id every read
    /// calls absent, and `AnyStore::upsert` reads that `false` as "already
    /// there" and falls through to `mutate`, which refuses an expired entity
    /// too: the upsert would write nothing and report success.
    pub async fn create(
        &self,
        tenant: &TenantId,
        id: &str,
        doc: &Value,
    ) -> Result<bool, sqlx::Error> {
        let (types, scopes, created, modified) = extract(doc);
        let meta = meta_of(doc);
        let rows = decompose(doc);
        let mut tx = super::begin(&self.pool).await?;
        crate::store::pg::set_tenant(&mut tx, tenant).await?;
        crate::store::pg::claim_tenant(&mut tx, tenant).await?;
        let reaped = sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM temporal_entities m \
             WHERE m.tenant_id = $1 AND m.id = $2 AND NOT {NOT_EXPIRED}"
        )))
        .bind(tenant.as_str())
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if reaped == 1 {
            wipe_instances(&mut tx, tenant, id).await?;
        }
        let n = sqlx::query(
            "INSERT INTO temporal_entities
               (tenant_id, id, types, scopes, meta, created_at, modified_at)
             VALUES ($1, $2, $3, $4, $5, $6::timestamptz, $7::timestamptz)
             ON CONFLICT (tenant_id, id) DO NOTHING",
        )
        .bind(tenant.as_str())
        .bind(id)
        .bind(&types)
        .bind(&scopes)
        .bind(&meta)
        .bind(&created)
        .bind(&modified)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if n == 1 {
            let mut ensured = std::collections::HashSet::new();
            let short = self.keys.is_short(tenant, &types, doc);
            insert_rows(
                &mut tx,
                &self.keys,
                &mut ensured,
                tenant,
                id,
                rows,
                Route {
                    dedup: false,
                    short,
                },
            )
            .await?;
        }
        tx.commit().await?;
        Ok(n == 1)
    }

    /// Append-only fast path (auto-recording, 5.6.12 adds): NO reconstruction,
    /// no history read — a shell meta insert (first touch) plus one multi-row
    /// instance upsert. This is the write the old full-resync made O(history).
    ///
    /// Conditional on the entity still existing. 5.6.6 Delete Entity removes
    /// the entity and the temporal evolution recorded for it, in that order and
    /// in two transactions; an auto-recording append that overlaps the delete
    /// would otherwise commit history for an entity that is gone, and no later
    /// delete would ever clean it. The `FOR KEY SHARE` lock is what makes the
    /// check hold: it lets concurrent updates of the same entity through and
    /// makes a concurrent DELETE wait until this append has committed.
    pub async fn append(
        &self,
        tenant: &TenantId,
        id: &str,
        shell: &Value,
        additions: &Value,
    ) -> Result<(), sqlx::Error> {
        let mut tx = super::begin(&self.pool).await?;
        let mut ensured = std::collections::HashSet::new();
        self.append_in(&mut tx, &mut ensured, tenant, id, shell, additions)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// [`Self::append`] of a whole drained request in one transaction
    /// (T-4085): a batch of N Entities costs one BEGIN, not N. Each append
    /// keeps its own liveness check and tenant.
    pub async fn append_many(
        &self,
        appends: &[(&TenantId, &str, &Value, Value)],
    ) -> Result<(), sqlx::Error> {
        if appends.is_empty() {
            return Ok(());
        }
        let mut tx = super::begin(&self.pool).await?;
        let mut ensured = std::collections::HashSet::new();
        // After the cut-over or for a short write, the batch's first sights go
        // in first, in one order: an Entity at a time, two batches could each
        // wait on an IRI the other wrote first. Only live Entities count:
        // append_in skips the others, and their IRIs would stay in iri_ids
        // naming nothing. The FOR KEY SHARE taken here holds until commit, so
        // append_in's own check finds the same Entities.
        let short = appends
            .iter()
            .any(|(tenant, _, shell, _)| self.keys.is_short(tenant, &extract(shell).0, shell));
        if short || self.keys.cutover(&mut tx).await?.is_some() {
            // the batch's Entities per Tenant, as (id, additions)
            type Docs<'a> = Vec<(&'a str, &'a Value)>;
            let mut by_tenant: std::collections::BTreeMap<&str, (&TenantId, Docs)> =
                std::collections::BTreeMap::new();
            for (tenant, id, _, additions) in appends {
                by_tenant
                    .entry(tenant.as_str())
                    .or_insert_with(|| (tenant, Vec::new()))
                    .1
                    .push((id, additions));
            }
            for (tenant, docs) in by_tenant.into_values() {
                crate::store::pg::set_tenant(&mut tx, tenant).await?;
                let live = self.live_of(&mut tx, tenant, &docs).await?;
                let iris = docs
                    .into_iter()
                    .filter(|(id, _)| live.contains(*id))
                    .flat_map(|(id, additions)| iris_of(id, additions))
                    .collect();
                self.keys
                    .ensure(&mut tx, tenant, iris, &mut ensured)
                    .await?;
            }
        }
        for (tenant, id, shell, additions) in appends {
            self.append_in(&mut tx, &mut ensured, tenant, id, shell, additions)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// The ids of `docs` stored in `entities` (all of them when this store
    /// serves only the temporal seam), each locked FOR KEY SHARE until the
    /// caller's commit.
    async fn live_of(
        &self,
        tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
        tenant: &TenantId,
        docs: &[(&str, &Value)],
    ) -> Result<std::collections::HashSet<String>, sqlx::Error> {
        let mut ids: Vec<&str> = docs.iter().map(|(id, _)| *id).collect();
        if self.temporal_only {
            return Ok(ids.into_iter().map(str::to_owned).collect());
        }
        ids.sort_unstable();
        ids.dedup();
        let live = sqlx::query_scalar(
            "SELECT id FROM entities WHERE tenant_id = $1 AND id = ANY($2)
             ORDER BY id FOR KEY SHARE",
        )
        .bind(tenant.as_str())
        .bind(&ids)
        .fetch_all(&mut **tx)
        .await?;
        Ok(live.into_iter().collect())
    }

    async fn append_in(
        &self,
        tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
        ensured: &mut std::collections::HashSet<(String, String)>,
        tenant: &TenantId,
        id: &str,
        shell: &Value,
        additions: &Value,
    ) -> Result<(), sqlx::Error> {
        let (types, scopes, created, modified) = extract(shell);
        let meta = meta_of(shell);
        let rows = decompose(additions);
        crate::store::pg::set_tenant(tx, tenant).await?;
        if !self.temporal_only {
            let live = sqlx::query(
                "SELECT 1 FROM entities WHERE tenant_id = $1 AND id = $2 FOR KEY SHARE",
            )
            .bind(tenant.as_str())
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
            if live.is_none() {
                return Ok(());
            }
        }
        // DO NOTHING froze types/scopes at first touch — an
        // entity gaining a type stayed invisible to type-filtered
        // temporal queries forever. The shell carries the CURRENT
        // entity, so refresh on change; the IS DISTINCT FROM guard keeps
        // the common no-change append from churning the meta row.
        sqlx::query(
            "INSERT INTO temporal_entities
               (tenant_id, id, types, scopes, meta, created_at, modified_at)
             VALUES ($1, $2, $3, $4, $5, $6::timestamptz, $7::timestamptz)
             ON CONFLICT (tenant_id, id) DO UPDATE SET
               types = EXCLUDED.types,
               scopes = EXCLUDED.scopes,
               meta = EXCLUDED.meta,
               modified_at = EXCLUDED.modified_at
             WHERE temporal_entities.types IS DISTINCT FROM EXCLUDED.types
                OR temporal_entities.scopes IS DISTINCT FROM EXCLUDED.scopes
                OR temporal_entities.meta IS DISTINCT FROM EXCLUDED.meta",
        )
        .bind(tenant.as_str())
        .bind(id)
        .bind(&types)
        .bind(&scopes)
        .bind(&meta)
        .bind(&created)
        .bind(&modified)
        .execute(&mut **tx)
        .await?;
        let short = self.keys.is_short(tenant, &types, shell);
        let route = Route { dedup: true, short };
        insert_rows(tx, &self.keys, ensured, tenant, id, rows, route).await
    }

    pub async fn get(&self, tenant: &TenantId, id: &str) -> Result<Option<Value>, sqlx::Error> {
        self.get_range(tenant, id, &TemporalFilter::default()).await
    }

    /// 5.6.16 Delete Temporal Evolution: `true` when it was there, `false`
    /// for "no existing Entity whose id (URI) is equivalent held locally",
    /// which the caller answers as ResourceNotFound. 4.22 decides what "held
    /// locally" means, the same way `get_range` and `query` decide it.
    pub async fn delete(&self, tenant: &TenantId, id: &str) -> Result<bool, sqlx::Error> {
        let mut tx = super::begin(&self.pool).await?;
        crate::store::pg::set_tenant(&mut tx, tenant).await?;
        let n = sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM temporal_entities m \
             WHERE m.tenant_id = $1 AND m.id = $2 AND {NOT_EXPIRED}"
        )))
        .bind(tenant.as_str())
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        // no FK: a partitioned table cannot be the referencing side of a
        // cascade from temporal_entities — clean the instances explicitly,
        // and only for the row this call actually removed. An expired
        // entity is refused above and keeps its history until the 4.22
        // reap or a create replaces it; wiping it here would destroy the
        // history behind a 404.
        if n == 1 {
            wipe_instances(&mut tx, tenant, id).await?;
        }
        tx.commit().await?;
        Ok(n == 1)
    }

    /// Erasure (T-4665, ADR-N-054 2.6): the rows of `attrs` of one Entity,
    /// current and superseded, observed at or before `before` (`None`: all);
    /// `attrs` `None` takes every Attribute, and the Entity's history itself
    /// once it holds no row. How many rows went.
    pub async fn erase(
        &self,
        tenant: &TenantId,
        id: &str,
        attrs: Option<&[String]>,
        before: Option<&str>,
    ) -> Result<u64, sqlx::Error> {
        let mut tx = super::begin(&self.pool).await?;
        crate::store::pg::set_tenant(&mut tx, tenant).await?;
        let names = attrs.map(<[String]>::to_vec);
        let mut n = 0u64;
        for sql in [
            "DELETE FROM attr_instances WHERE tenant_id = $1 AND entity_id = $2 \
             AND ($3::text[] IS NULL OR attr_id = ANY($3)) \
             AND ($4::timestamptz IS NULL OR observed_at <= $4::timestamptz)",
            "DELETE FROM attr_rows WHERE tenant_id = $1 AND entity_no = iri_no($1, $2) \
             AND ($3::text[] IS NULL OR attr_no IN \
                  (SELECT i.id FROM iri_ids i WHERE i.tenant_id = $1 AND i.iri = ANY($3))) \
             AND ($4::timestamptz IS NULL OR observed_at <= $4::timestamptz)",
            "DELETE FROM attr_versions WHERE tenant_id = $1 AND entity_id = $2 \
             AND ($3::text[] IS NULL OR attr_id = ANY($3)) \
             AND ($4::timestamptz IS NULL OR observed_at <= $4::timestamptz)",
        ] {
            let gone = sqlx::query(sql)
                .bind(tenant.as_str())
                .bind(id)
                .bind(&names)
                .bind(before)
                .execute(&mut *tx)
                .await?
                .rows_affected();
            n = n.saturating_add(gone);
        }
        if attrs.is_none() {
            sqlx::query(
                "DELETE FROM temporal_entities t WHERE t.tenant_id = $1 AND t.id = $2 \
                 AND NOT EXISTS (SELECT 1 FROM attr_instances a \
                                 WHERE a.tenant_id = t.tenant_id AND a.entity_id = t.id) \
                 AND NOT EXISTS (SELECT 1 FROM attr_rows r WHERE r.tenant_id = t.tenant_id \
                                 AND r.entity_no = iri_no(t.tenant_id, t.id))",
            )
            .bind(tenant.as_str())
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(n)
    }

    /// Every version of the rows of one Entity's history (T-4665, ADR-N-054
    /// 2.4) by Attribute: each instance with its `version` and `provenance`,
    /// a superseded one with `supersededAt`, oldest first per measurement.
    pub async fn versions(
        &self,
        tenant: &TenantId,
        id: &str,
    ) -> Result<serde_json::Map<String, Value>, sqlx::Error> {
        let mut tx = super::begin(&self.pool).await?;
        crate::store::pg::set_tenant(&mut tx, tenant).await?;
        let rows: Vec<(String, Value)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT attr_id, data || jsonb_build_object('version', version) || jsonb_strip_nulls( \
                 jsonb_build_object('provenance', provenance, 'supersededAt', \
                   to_char(superseded_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'))) \
             FROM (SELECT attr_id, observed_at, dataset_id, version, instance_id, data, provenance, superseded_at \
                     FROM attr_versions WHERE tenant_id = $1 AND entity_id = $2 \
                   UNION ALL \
                   SELECT ai.attr_id, ai.observed_at, ai.dataset_id, ai.version, ai.instance_id, {}, ai.provenance, NULL \
                     FROM {} ai) v \
             ORDER BY attr_id, observed_at, dataset_id NULLS FIRST, version, instance_id",
            restamp("ai"),
            history("$1", "$2")
        )))
        .bind(tenant.as_str())
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        let mut out = serde_json::Map::new();
        for (attr, instance) in rows {
            if let Some(list) = out
                .entry(attr)
                .or_insert_with(|| Value::Array(Vec::new()))
                .as_array_mut()
            {
                list.push(instance);
            }
        }
        Ok(out)
    }

    /// Temporal query: entity narrowing (ids/types/attrs) in the WHERE,
    /// instance pruning (range + lastN cap) in the reconstruction, and —
    /// when the caller passes a page — entity qualification + LIMIT/OFFSET
    /// in SQL, so a temporal query no longer materializes the whole tenant.
    pub async fn query(
        &self,
        tenant: &TenantId,
        f: &TemporalFilter<'_>,
    ) -> Result<TemporalOutcome, sqlx::Error> {
        self.query_inner(tenant, f, f.aggregate.is_some()).await
    }

    /// `push_agg`: compute the filter's 4.5.19 aggregation in SQL; a row
    /// whose windowed values are not all numeric makes the whole query fall
    /// back to instance reconstruction (one extra round trip, only then).
    async fn query_inner(
        &self,
        tenant: &TenantId,
        f: &TemporalFilter<'_>,
        push_agg: bool,
    ) -> Result<TemporalOutcome, sqlx::Error> {
        enum B {
            Text(String),
            Arr(Vec<String>),
            Num(i64),
            Float(f64),
        }
        let mut binds: Vec<B> = vec![B::Text(tenant.as_str().to_owned())];
        // 4.22: an expired ENTITY is invalid on temporal reads too — filter it
        // in SQL (no bind) so paging/totals stay exact. Expired instances are
        // stripped at the read boundary (any.rs). Literal, applies to the
        // fallback count query too.
        let mut wheres = vec!["m.tenant_id = $1".to_owned(), NOT_EXPIRED.to_owned()];
        if let Some(ids) = f.ids {
            binds.push(B::Arr(super::storable(ids)));
            wheres.push(format!("m.id = ANY(${})", binds.len()));
        }
        if let Some(types) = f.types {
            // overlap: entity has ANY of the wanted types (flat OR list).
            // `&&` alone is no index condition under RLS (not leakproof):
            // the first-type arm and the multi-type arm are (migration 0015),
            // and they BitmapOr instead of reading the Tenant (T-4852).
            binds.push(B::Arr(super::storable(types)));
            let n = binds.len();
            wheres.push(format!(
                "(m.types[1] = ANY(${n}) OR (cardinality(m.types) > 1 AND m.types && ${n}))"
            ));
        }
        // a policy's idPatterns, as the entity query takes them (T-5826,
        // T-6052): every one written or none, so the rows stay a superset
        if let Some(res) = f.id_patterns.and_then(|ps| {
            ps.iter()
                .map(|p| antares_store::filter::id_pattern_sql(p))
                .collect::<Option<Vec<_>>>()
        }) {
            let any: Vec<String> = res
                .into_iter()
                .map(|re| {
                    binds.push(B::Text(re));
                    format!("regexp_like(m.id COLLATE \"C\", ${})", binds.len())
                })
                .collect();
            wheres.push(match any.as_slice() {
                [] => "false".to_owned(),
                _ => format!("({})", any.join(" OR ")),
            });
        }
        if let Some(attrs) = f.attrs {
            binds.push(B::Arr(super::storable(attrs)));
            wheres.push(format!(
                "(EXISTS (SELECT 1 FROM attr_instances x WHERE x.tenant_id = m.tenant_id \
                 AND x.entity_id = m.id AND x.attr_id = ANY(${n})) \
                 OR EXISTS (SELECT 1 FROM attr_rows r WHERE r.tenant_id = m.tenant_id \
                 AND r.entity_no = iri_no(m.tenant_id, m.id) AND r.attr_no IN \
                   (SELECT i.id FROM iri_ids i WHERE i.tenant_id = m.tenant_id AND i.iri = ANY(${n}))))",
                n = binds.len()
            ));
        }
        // page pushdown: only when the caller passed one AND the range (if
        // any) compiles — SQL then also applies the evaluator's entity-
        // qualification rule (≥1 instance, in-window when ranged). WHERE
        // binds come FIRST so the fallback count query (offset past the end)
        // can reuse them with identical numbering.
        // None: no range to compile. Some(None): a range that does not
        // compile, which is the case page pushdown must not take — the SQL
        // would qualify entities on a window it cannot express.
        let compiled_range = f.range.as_ref().map(|r| {
            crate::compile::temporal::compile_instance_range(
                r,
                "ai.data",
                binds.len().saturating_add(1),
            )
        });
        let mut paged = false;
        if f.page.is_some() && !matches!(compiled_range, Some(None)) {
            let mut preds = String::new();
            if let (Some(r), Some(Some(c))) = (&f.range, compiled_range) {
                let n = binds.len().saturating_add(1);
                for b in c.binds {
                    binds.push(B::Text(b));
                }
                preds.push_str(&format!(" AND {}", c.sql));
                // index-serving widened bound on the same binds ($n+1 = timeAt)
                if let Some(cb) =
                    crate::compile::temporal::column_range_bound(r, "ai", n.saturating_add(1))
                {
                    preds.push_str(&format!(" AND {cb}"));
                }
            }
            let dir = match &f.range {
                Some(r) if r.timerel == "before" => "ASC",
                _ => "DESC",
            };
            let shown = f.page_attrs.map(|attrs| {
                binds.push(B::Arr(super::storable(attrs)));
                binds.len()
            });
            wheres.push(has_history(&preds, dir, shown));
            paged = true;
        }
        // 5.7.4.4 S2 superset prefilter: entities with no windowed instance
        // satisfying the compilable part of q= are never reconstructed. The
        // API arbiter re-evaluates q on every row that comes back, so this
        // can only narrow (compile::qprefilter invariant), never decide.
        if let Some(qn) = f.q {
            if let Some(c) = crate::compile::qprefilter::compile_prefilter(
                qn,
                f.range.as_ref(),
                "m",
                binds.len().saturating_add(1),
                f.expand,
            ) {
                for b in c.binds {
                    binds.push(B::Text(b));
                }
                wheres.push(c.sql);
            }
        }
        // 5.7.4.4 S3 superset prefilter: entities with no windowed instance
        // of the geoproperty possibly satisfying the geoquery are never
        // reconstructed. NULL geo_value (rows with no extracted geometry) always
        // survives; GeoQuery::matches stays the arbiter.
        for (spec, iri) in f.geo {
            let attr_bind = binds.len().saturating_add(1);
            let mut win_binds: Vec<String> = Vec::new();
            let mut window = String::new();
            if let Some(r) = &f.range {
                if let Some(cb) = crate::compile::temporal::column_range_bound(
                    r,
                    "gi",
                    attr_bind.saturating_add(1),
                ) {
                    // 4.6.3 lets a request spell the seconds fraction with a
                    // comma; the bound lands in a `::timestamptz` cast, which
                    // takes only the point form. Every other range bind is
                    // canonicalized for exactly this reason.
                    win_binds
                        .push(antares_store::filter::canonical_datetime(r.time_at).into_owned());
                    if r.timerel == "between" {
                        if let Some(e) = r.end_time_at {
                            win_binds
                                .push(antares_store::filter::canonical_datetime(e).into_owned());
                        }
                    }
                    window = format!(" AND {cb}");
                }
            }
            if let Some(c) = crate::compile::geo::compile_geo_instance(
                spec,
                "gi.geo_value",
                attr_bind.saturating_add(1).saturating_add(win_binds.len()),
            ) {
                binds.push(B::Text(iri.to_owned()));
                for b in win_binds {
                    binds.push(B::Text(b));
                }
                for b in c.geo_binds {
                    binds.push(B::Text(b));
                }
                for n in c.num_binds {
                    binds.push(B::Float(n));
                }
                wheres.push(format!(
                    "EXISTS (SELECT 1 FROM (SELECT * FROM attr_instances \
                     WHERE tenant_id = m.tenant_id AND entity_id = m.id AND attr_id = ${attr_bind} \
                     UNION ALL SELECT * FROM attr_rows_at(m.tenant_id, m.id, ${attr_bind})) gi \
                     WHERE true{window} AND {})",
                    c.sql
                ));
            }
        }
        let n_where = binds.len();
        let where_sql = wheres.join(" AND ");
        let agg = if push_agg {
            f.aggregate
                .as_ref()
                .and_then(|a| aggregate_expr(f, a, n_where.saturating_add(1)))
        } else {
            None
        };
        let aggregated = agg.is_some();
        let (attr_expr, extra) = match agg {
            Some((e, b)) => (format!("jsonb_build_object('$agg', {e})"), b),
            None => match attr_object_expr(f, n_where.saturating_add(1), None) {
                Some(v) => v,
                // refused range shape: reconstruct unpruned, window arbitrates.
                // The default filter has neither range nor lastN, the two
                // shapes the compiler can refuse: pinned by
                // `the_unpruned_fallback_always_compiles`. A request reaches
                // this line: a refusal is an error, never a panic (T-3915).
                None => {
                    attr_object_expr(&TemporalFilter::default(), n_where.saturating_add(1), None)
                        .ok_or_else(|| {
                            sqlx::Error::Protocol(
                                "the unpruned temporal shape did not compile".into(),
                            )
                        })?
                }
            },
        };
        binds.extend(extra.into_iter().map(B::Text));
        let mut windowed = false;
        let mut tail = " ORDER BY m.id".to_owned();
        // `paged` is set only inside `if f.page.is_some()`, so binding the
        // two together says that in the type system instead of in a comment.
        if let (true, Some(page)) = (paged, f.page.as_ref()) {
            windowed = true;
            binds.push(B::Num(page.limit));
            tail.push_str(&format!(" LIMIT ${}", binds.len()));
            binds.push(B::Num(page.offset));
            tail.push_str(&format!(" OFFSET ${}", binds.len()));
        } else {
            // No page pushed down: the caller still has to filter, so the only
            // bound on this statement is the safety ceiling — without it a
            // bare `?timerel=…` reconstructs every temporal entity of the
            // tenant into one Vec.
            binds.push(B::Num(super::entity::MAX_UNDECIDED_ROWS));
            tail.push_str(&format!(" LIMIT ${}", binds.len()));
        }
        // A page is cut over ids first and only its rows rebuild their
        // history: in one SELECT the window count makes every OFFSET row pass
        // through the projection, so a deep page rebuilt each skipped
        // Entity's history (F-R2ANT4-009; entities do the same, T-4907).
        let sql = if windowed {
            format!(
                "SELECT m.meta || {attr_expr}, p.total FROM (SELECT m.tenant_id, m.id, \
                 count(*) OVER () AS total FROM temporal_entities m WHERE {where_sql}{tail}) AS p \
                 JOIN temporal_entities m ON m.tenant_id = p.tenant_id AND m.id = p.id \
                 ORDER BY m.id"
            )
        } else {
            format!("SELECT m.meta || {attr_expr} FROM temporal_entities m WHERE {where_sql}{tail}")
        };
        let first: Result<Option<TemporalOutcome>, sqlx::Error> = async {
            let mut tx = super::begin(&self.pool).await?;
            crate::store::pg::set_tenant(&mut tx, tenant).await?;
            // `sql` is compiler literals + $n placeholders only.
            let mut qy = sqlx::query(sqlx::AssertSqlSafe(sql.clone()));
            for b in &binds {
                qy = match b {
                    B::Text(s) => qy.bind(s),
                    B::Arr(v) => qy.bind(v),
                    B::Num(n) => qy.bind(n),
                    B::Float(x) => qy.bind(x),
                };
            }
            let rows = qy.fetch_all(&mut *tx).await?;
            let mut total = if paged {
                rows.first().map(|r| r.get::<i64, _>(1))
            } else {
                None
            };
            if paged && total.is_none() {
                // offset past the end: the window function came back with the
                // page, which is empty — count separately with the WHERE binds
                let count_sql =
                    format!("SELECT count(*) FROM temporal_entities m WHERE {where_sql}");
                let mut cq = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(count_sql));
                for b in binds.iter().take(n_where) {
                    cq = match b {
                        B::Text(s) => cq.bind(s),
                        B::Arr(v) => cq.bind(v),
                        B::Num(n) => cq.bind(n),
                        B::Float(x) => cq.bind(x),
                    };
                }
                total = Some(cq.fetch_one(&mut *tx).await?);
            }
            tx.commit().await?;
            super::entity::check_ceiling(paged, rows.len(), super::entity::MAX_UNDECIDED_ROWS)?;
            let mut docs: Vec<Value> = rows.into_iter().map(|r| r.get::<Value, _>(0)).collect();
            docs.iter_mut().for_each(unpack);
            if aggregated {
                for d in &mut docs {
                    let Some(o) = d.as_object_mut() else { continue };
                    let Some(agg) = o.remove("$agg") else {
                        continue;
                    };
                    if agg.get("bad").and_then(Value::as_bool) == Some(true) {
                        return Ok(None);
                    }
                    if let Some(attrs) = agg.get("attrs").and_then(Value::as_object) {
                        for (k, v) in attrs {
                            o.insert(k.clone(), v.clone());
                        }
                    }
                }
            }
            Ok(Some(TemporalOutcome {
                rows: docs,
                paged,
                total,
                aggregated,
            }))
        }
        .await;
        match first? {
            Some(out) => Ok(out),
            // the fallback re-runs the same statement without the pushed
            // aggregation; boxed because an async fn cannot recurse inline
            None => Box::pin(self.query_inner(tenant, f, false)).await,
        }
    }

    /// Single-entity fetch with the same instance pruning (Retrieve
    /// Temporal Evolution, 5.7.3). `None` = entity absent.
    pub async fn get_range(
        &self,
        tenant: &TenantId,
        id: &str,
        f: &TemporalFilter<'_>,
    ) -> Result<Option<Value>, sqlx::Error> {
        let st = range_statement(f)?;
        let mut tx = super::begin(&self.pool).await?;
        crate::store::pg::set_tenant(&mut tx, tenant).await?;
        let mut qy = sqlx::query(sqlx::AssertSqlSafe(st.sql))
            .bind(tenant.as_str())
            .bind(id);
        if let Some(a) = st.only {
            qy = qy.bind(a);
        }
        for b in st.binds {
            qy = qy.bind(b);
        }
        let row = qy.fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        Ok(row.map(|r| {
            let mut doc = r.get::<Value, _>(0);
            unpack(&mut doc);
            doc
        }))
    }

    pub async fn list(&self, tenant: &TenantId) -> Result<Vec<Value>, sqlx::Error> {
        Ok(self.query(tenant, &TemporalFilter::default()).await?.rows)
    }

    /// Row-locked read-modify-write over the RECONSTRUCTED doc, written back
    /// as a DELTA: rows are diffed by (attr, instanceId) and only moved,
    /// changed or removed instances touch the table — never a full-history
    /// rewrite (the old resync's O(history) write amplification, and the
    /// thing that fought hypertable compression).
    pub async fn mutate<T, E>(
        &self,
        tenant: &TenantId,
        id: &str,
        f: impl FnOnce(&mut Value) -> Result<T, E>,
    ) -> Result<Option<Result<T, E>>, sqlx::Error> {
        let mut tx = super::begin(&self.pool).await?;
        crate::store::pg::set_tenant(&mut tx, tenant).await?;
        // the meta row is the serialization point (FOR UPDATE). 4.22
        // qualifies it: 5.6.12 to 5.6.15 reach the history through here,
        // and none of them may modify an entity every read calls absent.
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT m.meta FROM temporal_entities m \
             WHERE m.tenant_id = $1 AND m.id = $2 AND {NOT_EXPIRED} FOR UPDATE"
        )))
        .bind(tenant.as_str())
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            tx.commit().await?;
            return Ok(None);
        };
        let meta: Value = row.get(0);
        // reconstruct the full doc inside the same transaction
        let attrs_row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT COALESCE((SELECT jsonb_object_agg(g.attr_id, g.insts) FROM (
                SELECT ai.attr_id,
                       {} AS insts
                FROM {} ai
                GROUP BY ai.attr_id) g), '{{}}'::jsonb)",
            insts_and_ids("ai"),
            history("$1", "$2")
        )))
        .bind(tenant.as_str())
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        let mut attrs: Value = attrs_row.get(0);
        unpack(&mut attrs);
        let mut doc = meta;
        if let (Some(d), Some(a)) = (doc.as_object_mut(), attrs.as_object()) {
            for (k, v) in a {
                d.insert(k.clone(), v.clone());
            }
        }
        let before_rows = decompose(&doc);
        match f(&mut doc) {
            Ok(t) => {
                let (types, scopes, _created, modified) = extract(&doc);
                let after_rows = decompose(&doc);
                // diff by logical identity (attr, instanceId); a changed
                // observed_at moves the physical row → delete + insert
                let key = |r: &Value| -> (String, String) {
                    (
                        r["attr_id"].as_str().unwrap_or("").to_owned(),
                        r["instance_id"].as_str().unwrap_or("").to_owned(),
                    )
                };
                let old: std::collections::HashMap<(String, String), &Value> =
                    before_rows.iter().map(|r| (key(r), r)).collect();
                let new: std::collections::HashMap<(String, String), &Value> =
                    after_rows.iter().map(|r| (key(r), r)).collect();
                let mut deletes: Vec<Value> = Vec::new();
                for (k, o) in &old {
                    match new.get(k) {
                        None => deletes.push(serde_json::json!({"a": k.0, "i": k.1})),
                        Some(n) if n["observed_at"] != o["observed_at"] => {
                            deletes.push(serde_json::json!({"a": k.0, "i": k.1}))
                        }
                        _ => {}
                    }
                }
                let upserts: Vec<Value> = after_rows
                    .iter()
                    .filter(|r| old.get(&key(r)).is_none_or(|o| *o != *r))
                    .cloned()
                    .collect();
                if !deletes.is_empty() {
                    // 5.6.15 deletes an instance: its last version is kept,
                    // so a deletion is never without a record (T-4665).
                    sqlx::query(sqlx::AssertSqlSafe(format!(
                        "INSERT INTO attr_versions
                           (tenant_id, entity_id, attr_id, instance_id, dataset_id, observed_at,
                            version, data, provenance)
                         SELECT t.tenant_id, t.entity_id, t.attr_id, t.instance_id, t.dataset_id,
                                t.observed_at, t.version, {}, t.provenance
                         FROM {} t JOIN jsonb_array_elements($3::jsonb) AS e
                           ON t.attr_id = e->>'a' AND t.instance_id = e->>'i'
                         ON CONFLICT DO NOTHING",
                        restamp("t"),
                        history("$1", "$2")
                    )))
                    .bind(tenant.as_str())
                    .bind(id)
                    .bind(Value::Array(deletes.clone()))
                    .execute(&mut *tx)
                    .await?;
                    for sql in [
                        "DELETE FROM attr_instances t
                         USING jsonb_array_elements($3::jsonb) AS e
                         WHERE t.tenant_id = $1 AND t.entity_id = $2
                           AND t.attr_id = e->>'a' AND t.instance_id = e->>'i'",
                        "DELETE FROM attr_rows t
                         USING jsonb_array_elements($3::jsonb) AS e
                         WHERE t.tenant_id = $1 AND t.entity_no = iri_no($1, $2)
                           AND t.attr_no = iri_no($1, e->>'a')
                           AND COALESCE(t.data->>'instanceId',
                                        'urn:ngsi-ld:Instance:' || t.instance_no::text) = e->>'i'",
                    ] {
                        sqlx::query(sql)
                            .bind(tenant.as_str())
                            .bind(id)
                            .bind(Value::Array(deletes.clone()))
                            .execute(&mut *tx)
                            .await?;
                    }
                }
                let mut ensured = std::collections::HashSet::new();
                insert_rows(
                    &mut tx,
                    &self.keys,
                    &mut ensured,
                    tenant,
                    id,
                    upserts,
                    Route {
                        dedup: false,
                        short: self.keys.is_short(tenant, &types, &doc),
                    },
                )
                .await?;
                sqlx::query(
                    "UPDATE temporal_entities SET meta = $3, types = $4, scopes = $5,
                       modified_at = $6::timestamptz
                     WHERE tenant_id = $1 AND id = $2",
                )
                .bind(tenant.as_str())
                .bind(id)
                .bind(meta_of(&doc))
                .bind(&types)
                .bind(&scopes)
                .bind(&modified)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(Some(Ok(t)))
            }
            Err(e) => {
                tx.rollback().await?;
                Ok(Some(Err(e)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::temporal::InstanceRange;

    fn pair(n: usize) -> (String, String) {
        ("t".to_owned(), format!("urn:ngsi-ld:E:{n}"))
    }

    /// T-5243: the known set holds at most two generations; a pair read
    /// while old survives the next ageing, one never read again is dropped.
    #[test]
    fn known_ages_by_generation_and_keeps_what_it_reads() {
        let mut k = Known::default();
        for n in 0..KNOWN_GEN {
            k.insert(pair(n));
        }
        k.insert(pair(KNOWN_GEN));
        assert_eq!(k.old.len(), KNOWN_GEN);
        assert_eq!(k.new.len(), 1);
        assert!(k.contains(&pair(0)));
        assert!(!k.old.contains(&pair(0)));
        for n in 1..KNOWN_GEN {
            k.insert(pair(KNOWN_GEN.saturating_add(n)));
        }
        k.insert(pair(KNOWN_GEN.saturating_mul(3)));
        assert!(k.contains(&pair(0)));
        assert!(!k.contains(&pair(1)));
        assert!(!k.contains(&pair(KNOWN_GEN.saturating_mul(4))));
        k.retain(|(t, _)| t != "t");
        assert!(k.new.is_empty());
        assert!(k.old.is_empty());
    }

    /// T-5243: the broker's own canonical id keys by its uuid; any other id,
    /// an upper-case uuid included, keys by its v5 and stays in data.
    #[test]
    fn instance_no_keys_own_ids_by_their_uuid_and_others_by_v5() {
        let own = "urn:ngsi-ld:Instance:0f8fad5b-d9cb-469f-a165-70867728950e";
        let (no, is_own) = instance_no(own);
        assert_eq!(
            (no.to_string(), is_own),
            ("0f8fad5b-d9cb-469f-a165-70867728950e".to_owned(), true)
        );
        let upper = "urn:ngsi-ld:Instance:0F8FAD5B-D9CB-469F-A165-70867728950E";
        for other in [upper, "urn:client:speed:thirty", "urn:ngsi-ld:Instance:x"] {
            let (no, is_own) = instance_no(other);
            assert_eq!((no.get_version_num(), is_own), (5, false), "{other}");
            assert_eq!(no, instance_no(other).0, "{other}: stable");
        }
        assert_ne!(
            instance_no(upper).0,
            no,
            "a v5 never equals the uuid it spells"
        );
    }

    /// T-5243: a row from the cut-over on is keyed; an earlier one, an
    /// unreadable stamp and no cut-over stay in the text shape.
    #[test]
    fn is_keyed_routes_by_observed_at_against_the_cut_over() {
        let cut = chrono::DateTime::parse_from_rfc3339("2026-10-26T00:00:00Z")
            .ok()
            .map(|t| t.to_utc());
        let row = |at: &str| serde_json::json!({"observed_at": at});
        let got = [
            is_keyed(&row("2026-10-26T00:00:00Z"), cut),
            is_keyed(&row("2026-10-26T01:00:00+02:00"), cut),
            is_keyed(&row("2026-10-25T23:59:59Z"), cut),
            is_keyed(&row("not a stamp"), cut),
            is_keyed(&row("2027-01-01T00:00:00Z"), None),
        ];
        assert_eq!(got, [true, false, false, false, false]);
    }

    /// T-5243: the IRIs a batch append writes first are the Entity id and its
    /// Attribute names, never a meta member.
    #[test]
    fn iris_of_names_the_entity_and_its_attributes() {
        let doc = serde_json::json!({"id": "x", "type": "T", "createdAt": "2026-01-01T00:00:00Z",
                                     "https://a": [{}], "https://b": {"not": "instances"}});
        assert_eq!(
            iris_of("urn:e", &doc),
            vec!["urn:e".to_owned(), "https://a".to_owned()]
        );
    }

    /// T-5131: a row's instanceId lives in its column; `unpack` puts it back
    /// and leaves stored instances and meta members alone.
    #[test]
    fn instance_id_leaves_data_and_unpack_puts_it_back() {
        let speed = "https://uri.etsi.org/ngsi-ld/default-context/speed";
        let inst = serde_json::json!({"type": "Property", "value": 1,
                                      "instanceId": "urn:ngsi-ld:Instance:1"});
        let rows = decompose(&serde_json::json!({"id": "urn:ngsi-ld:V:1", speed: [inst.clone()]}));
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0]["data"],
            serde_json::json!({"type": "Property", "value": 1})
        );
        assert_eq!(rows[0]["instance_id"], "urn:ngsi-ld:Instance:1");
        // a row written before T-5131 keeps its instanceId in data
        let stored = serde_json::json!({"type": "Property", "value": 2, "instanceId": "urn:ngsi-ld:Instance:2"});
        let mut doc = serde_json::json!({
            "id": "urn:ngsi-ld:V:1", "type": "V", "scope": ["/a", "/b"],
            speed: [[rows[0]["data"].clone(), stored.clone()],
                    ["urn:ngsi-ld:Instance:1", "urn:ngsi-ld:Instance:2"]],
            "short": [[{"value": 1}], ["x", "y"]],
            "numbers": [[{"value": 1}], [1]]
        });
        unpack(&mut doc);
        assert_eq!(
            doc,
            serde_json::json!({"id": "urn:ngsi-ld:V:1", "type": "V", "scope": ["/a", "/b"],
                               speed: [inst, stored],
                               "short": [[{"value": 1}], ["x", "y"]],
                               "numbers": [[{"value": 1}], [1]]})
        );
        let mut scalar = serde_json::json!(1);
        unpack(&mut scalar);
        assert_eq!(scalar, serde_json::json!(1));
    }

    /// T-4669: the dedup of an instance without observedAt reads the newest
    /// stored row through the index and nothing older. A correlated max()
    /// compared row by row read the attribute's whole history per write
    /// (~90 ms a station at a month deep, measured in the task).
    #[test]
    fn the_newest_row_probe_reads_one_row() {
        assert!(NEWEST_REPEATS.contains("ORDER BY x.observed_at DESC LIMIT 1"));
        assert!(!NEWEST_REPEATS.to_lowercase().contains("max("));
        // Its stamps are the ones every other comparison here ignores.
        assert!(NEWEST_REPEATS.contains(&STAMPS["ARRAY".len()..]));
        assert!(NEWEST_REPEATS.contains("x.tenant_id = $1 AND x.entity_id = $2"));
    }

    /// Two callers fall back to `attr_object_expr(&TemporalFilter::default())`
    /// when a range is outside the compiler's exact subset, and unwrap it:
    /// the default filter has no range and no `lastN`, and `None` is returned
    /// only for a range the compiler refuses. That is the reconstruct-unpruned
    /// path, so if it ever stopped compiling the broker would panic on
    /// precisely the queries the fallback exists to serve. Pinned here rather
    /// than argued in a comment.
    #[test]
    fn the_unpruned_fallback_always_compiles() {
        let f = TemporalFilter::default();
        assert!(f.range.is_none(), "the default filter carries no range");
        assert!(f.last_n.is_none(), "the default filter carries no lastN");
        for first_bind in [1, 3, 4, 17] {
            let built = attr_object_expr(&f, first_bind, None);
            let (sql, binds) = built.unwrap_or_else(|| {
                panic!("the unpruned fallback failed to compile at bind {first_bind}")
            });
            assert!(!sql.is_empty(), "empty SQL at bind {first_bind}");
            assert!(
                !sql.contains('$'),
                "the unpruned fragment names no placeholder: {sql}"
            );
            assert!(
                binds.is_empty(),
                "a fragment that names no placeholder must hand back no bind: {binds:?}"
            );
        }
    }

    fn between<'a>() -> TemporalFilter<'a> {
        TemporalFilter {
            range: Some(InstanceRange {
                timerel: "between",
                time_at: "2026-01-01T00:00:00Z",
                end_time_at: Some("2026-02-01T00:00:00Z"),
                timeproperty: "observedAt",
            }),
            ..Default::default()
        }
    }

    /// The temporal tables extract their four columns with the same function
    /// the entity table uses, so what holds there holds here: `type` and
    /// `scope` read as arrays whichever shape the document spells them in,
    /// and 4.6.3's comma seconds-fraction is canonicalised. A stamp that
    /// reaches a `::timestamptz` bind with its comma intact is rejected by
    /// Postgres, and the append fails on a document the API accepted.
    #[test]
    fn extract_canonicalises_the_stamps_and_takes_type_in_either_shape() {
        let (types, scopes, created, modified) = extract(&serde_json::json!({
            "id": "urn:x", "type": "T", "scope": ["/a", "/b"],
            "createdAt": "2026-01-01T00:00:00,500Z",
            "modifiedAt": "2026-01-01T00:00:01.500Z"}));
        assert_eq!(types, ["T"]);
        assert_eq!(scopes.expect("scopes"), ["/a", "/b"]);
        assert_eq!(
            created, "2026-01-01T00:00:00.500Z",
            "the comma is the fraction"
        );
        assert_eq!(modified, "2026-01-01T00:00:01.500Z");
        let (types, scopes, created, _) = extract(&serde_json::json!({"id": "urn:x"}));
        assert!(types.is_empty(), "no type is no types, never a panic");
        assert!(
            scopes.is_none(),
            "an absent scope stays absent, not an empty list"
        );
        assert_eq!(
            created, "1970-01-01T00:00:00Z",
            "the belt stamp is usable as a bind"
        );
    }

    #[test]
    fn meta_of_keeps_only_meta_members() {
        let doc = serde_json::json!({
            "id": "urn:x", "type": ["T"], "createdAt": "c", "modifiedAt": "m",
            "https://a/attr": [{"instanceId": "i1"}]
        });
        let m = meta_of(&doc);
        assert!(m.get("id").is_some() && m.get("https://a/attr").is_none());
    }

    #[test]
    fn range_binds_are_numbered_from_first_bind() {
        let (expr, binds) = attr_object_expr(&between(), 2, None).expect("compiles");
        assert_eq!(
            binds,
            vec!["observedAt", "2026-01-01T00:00:00Z", "2026-02-01T00:00:00Z"]
        );
        assert!(
            expr.contains("$2") && expr.contains("$3") && expr.contains("$4"),
            "{expr}"
        );
        // predicates run on the instance JSON, never the partition column
        assert!(expr.contains("ai.data ->>"), "{expr}");
    }

    #[test]
    fn last_n_caps_with_rank_per_attr_and_dataset_never_row_number() {
        let f = TemporalFilter {
            last_n: Some(5),
            ..Default::default()
        };
        let (expr, binds) = attr_object_expr(&f, 1, None).expect("compiles");
        // RANK keeps timestamp ties; ROW_NUMBER would cut an instance the
        // API-side per-attr lastN still wants (the tie-break divergence bug)
        assert!(expr.contains("rank() OVER"), "{expr}");
        assert!(!expr.contains("row_number"), "{expr}");
        assert!(
            expr.contains("PARTITION BY ai.attr_id, ai.data ->> 'datasetId'"),
            "{expr}"
        );
        assert!(expr.contains("COLLATE \"C\" DESC NULLS LAST"), "{expr}");
        assert_eq!(binds, vec!["observedAt", "5"]);
    }

    /// T-5616: lastN's distinct-Attribute skip scan keeps to the window's
    /// column bound in both parents, so it reads the weeks the window
    /// reaches and not every week of the retention; no window, no bound.
    #[test]
    fn the_attribute_skip_scan_keeps_to_the_window() {
        let after = TemporalFilter {
            last_n: Some(10),
            range: Some(InstanceRange {
                timerel: "after",
                time_at: "2026-10-24T00:00:00Z",
                end_time_at: None,
                timeproperty: "observedAt",
            }),
            ..Default::default()
        };
        let sql = newest_rows(&after, 3, 7, None).expect("after reads the newest rows");
        assert!(!sql.contains("window}"), "{sql}");
        for alias in ["x", "r", "t"] {
            let bound = format!("{alias}.observed_at >= $4::timestamptz - interval '48 hours'");
            assert!(sql.contains(&bound), "{bound} missing: {sql}");
        }
        assert_eq!(sql.matches("x.observed_at >= $4").count(), 2, "{sql}");
        assert_eq!(sql.matches("r.observed_at >= $4").count(), 2, "{sql}");
        let unbounded = TemporalFilter {
            last_n: Some(10),
            ..Default::default()
        };
        let sql = newest_rows(&unbounded, 3, 4, None).expect("no range reads the newest rows");
        assert!(!sql.contains("window}"), "{sql}");
        assert!(!sql.contains("observed_at >= $"), "{sql}");
    }

    /// T-5616: the page qualification probes each parent newest first (oldest
    /// first for `before`), one row along its btree, never an EXISTS over the
    /// union that reads an Entity's rows in the btree's own order.
    #[test]
    fn the_page_qualification_probes_one_row_per_parent() {
        let sql = has_history(" AND ai.x", "DESC", None);
        assert!(
            sql.contains("ORDER BY ai.attr_id DESC, ai.observed_at DESC LIMIT 1"),
            "{sql}"
        );
        assert!(
            sql.contains("ORDER BY ai.attr_no DESC, ai.observed_at DESC LIMIT 1"),
            "{sql}"
        );
        assert_eq!(sql.matches(" AND ai.x").count(), 2, "{sql}");
        assert!(sql.starts_with("NOT m.bare AND ("), "{sql}");
        assert!(!sql.contains("EXISTS"), "{sql}");
        assert!(!sql.contains("ANY("), "{sql}");
    }

    /// T-5616: under a policy the page qualifies an Entity only through the
    /// Attributes its rules show, in both parents, on the one bind.
    #[test]
    fn the_page_qualification_keeps_to_the_shown_attributes() {
        let sql = has_history(" AND ai.x", "DESC", Some(9));
        assert!(sql.contains("unnest($9::text[]) s(attr)"), "{sql}");
        assert!(
            sql.contains("ai.attr_id = s.attr AND ai.x ORDER BY ai.observed_at DESC LIMIT 1"),
            "{sql}"
        );
        assert!(
            sql.contains("ai.attr_no = i.id AND ai.x ORDER BY ai.observed_at DESC LIMIT 1"),
            "{sql}"
        );
        assert!(
            sql.contains("i.tenant_id = m.tenant_id AND i.iri = ANY($9)"),
            "{sql}"
        );
        assert!(!sql.contains("attr_id = ANY"), "{sql}");
        assert!(sql.starts_with("NOT m.bare AND ("), "{sql}");
    }

    /// T-5616: the read takes the N newest alone only where the Entity's
    /// last non-UTC stamp lies more than 28 h before the N-th newest.
    #[test]
    fn the_newest_rows_widen_only_near_an_offset_stamp() {
        let f = TemporalFilter {
            last_n: Some(10),
            ..Default::default()
        };
        let sql = newest_rows(&f, 3, 4, None).expect("reads the newest rows");
        assert!(
            sql.contains(
                "CASE WHEN k.nth - interval '28 hours' > m.offset_until THEN k.nth \
                 ELSE k.nth - interval '28 hours' END"
            ),
            "{sql}"
        );
        assert!(sql.contains("ai.observed_at >= c.th"), "{sql}");
    }

    /// T-5616: a UTC RFC 3339 stamp keys like its instant; an offset, a space
    /// separator, a non-stamp or no observedAt does not.
    #[test]
    fn only_a_utc_t_stamp_orders_like_its_instant() {
        for (data, utc) in [
            (
                serde_json::json!({"observedAt": "2026-10-26T12:00:00Z"}),
                true,
            ),
            (
                serde_json::json!({"observedAt": "2026-10-26T12:00:00,5Z"}),
                true,
            ),
            (
                serde_json::json!({"observedAt": "2026-10-26T12:00:00.123456789Z"}),
                true,
            ),
            (
                serde_json::json!({"observedAt": "2026-10-26T12:00:00+00:00"}),
                false,
            ),
            (
                serde_json::json!({"observedAt": "2026-10-26T14:00:00+02:00"}),
                false,
            ),
            (
                serde_json::json!({"observedAt": "2026-10-26 12:00:00Z"}),
                false,
            ),
            (serde_json::json!({"observedAt": "yesterdayZ"}), false),
            (serde_json::json!({"observedAt": 5}), false),
            (serde_json::json!({"value": 1}), false),
        ] {
            assert_eq!(utc_observed(&data), utc, "{data}");
        }
    }

    #[test]
    fn first_n_ranks_the_oldest_first() {
        let f = TemporalFilter {
            last_n: Some(5),
            oldest_first: true,
            ..Default::default()
        };
        let (expr, binds) = attr_object_expr(&f, 1, None).expect("compiles");
        assert!(expr.contains("COLLATE \"C\" ASC NULLS FIRST"), "{expr}");
        assert!(!expr.contains("DESC"), "{expr}");
        assert_eq!(binds, vec!["observedAt", "5"]);
    }

    #[test]
    fn refused_range_shape_refuses_the_pruning() {
        let f = TemporalFilter {
            range: Some(InstanceRange {
                timerel: "since", // not a 4.11 relation
                time_at: "t",
                end_time_at: None,
                timeproperty: "observedAt",
            }),
            last_n: Some(3),
            ..Default::default()
        };
        // half-pruning would silently skip the range: refuse instead
        assert!(attr_object_expr(&f, 1, None).is_none());
    }

    #[test]
    fn decompose_never_parks_deletion_instances_on_the_epoch() {
        let doc = serde_json::json!({
            "id": "urn:x",
            "https://a/attr": [{
                "type": "Property", "value": "urn:ngsi-ld:null",
                "instanceId": "urn:ngsi-ld:Instance:d1",
                "deletedAt": "2026-08-08T10:00:00Z"
            }]
        });
        let rows = decompose(&doc);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["observed_at"], "2026-08-08T10:00:00Z");
    }
}
