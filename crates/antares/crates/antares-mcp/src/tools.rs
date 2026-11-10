// SPDX-License-Identifier: EUPL-1.2
//! The tools (MCP 3.5, 34 rows): each is one row of the operation matrix,
//! and a call is one NGSI-LD request of that operation, built from checked
//! arguments only. A write tool runs only after the person confirmed the
//! call (MCP 3.4, `crate::confirm`).

use crate::schema::{self, opt, req, Arg, Args, Checked, Kind};
use antares_ops::OperationId;
use axum::http::Method;
use serde_json::{json, Value};

/// What a tool answers, and whether it pages.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ToolDef {
    pub name: &'static str,
    pub op: OperationId,
    pub description: &'static str,
    pub args: &'static [Arg],
    /// The member of the structured result the answer goes under.
    pub key: &'static str,
    /// Whether the answer is a list paged with `limit` and `cursor`.
    pub paged: bool,
    /// Whether the tool writes, and so asks the person first (MCP 3.4).
    pub write: bool,
}

const GEOMETRIES: &[&str] = &[
    "Point",
    "MultiPoint",
    "LineString",
    "MultiLineString",
    "Polygon",
    "MultiPolygon",
];
const JOINS: &[&str] = &["flat", "inline"];
const TIMERELS: &[&str] = &["before", "after", "between"];
/// The instant a history is ordered by (TS 104 175 4.11); `observedAt`
/// when absent, so data without it needs another.
/// The 4.8 set (TS 104 175 5.2.4 NOTE 2), as antares-api's `TIMEPROPERTIES`.
const TIMEPROPERTIES: &[&str] = &[
    "observedAt",
    "createdAt",
    "modifiedAt",
    "deletedAt",
    "expiresAt",
];
/// The entity representations (6.3.7); `options=sysAttrs` rides as the
/// `sysAttrs` flag, which [`query`] turns into the option.
const FORMATS: &[&str] = &["normalized", "concise", "keyValues"];
/// The temporal representations (6.3.7, 6.3.12).
const TEMPORAL_FORMATS: &[&str] = &["normalized", "temporalValues", "aggregatedValues"];

const GET_ENTITY: &[Arg] = &[
    req("id", Kind::Id),
    opt("attrs", Kind::Attrs),
    opt("pick", Kind::Members),
    opt("omit", Kind::Members),
    opt("lang", Kind::Text(64)),
    opt("join", Kind::Word(JOINS)),
    opt("joinLevel", Kind::Int(1, 3)),
    opt("format", Kind::Word(FORMATS)),
    opt("sysAttrs", Kind::Flag),
];

const QUERY: &[Arg] = &[
    opt("type", Kind::Types),
    opt("id", Kind::Ids),
    opt("idPattern", Kind::Text(256)),
    opt("attrs", Kind::Attrs),
    opt("pick", Kind::Members),
    opt("omit", Kind::Members),
    opt("q", Kind::Text(4096)),
    opt("scopeQ", Kind::Text(1024)),
    opt("georel", Kind::Text(256)),
    opt("geometry", Kind::Word(GEOMETRIES)),
    opt("coordinates", Kind::Coordinates),
    opt("geoproperty", Kind::Attr),
    opt("local", Kind::Flag),
    opt("lang", Kind::Text(64)),
    opt("join", Kind::Word(JOINS)),
    opt("joinLevel", Kind::Int(1, 3)),
    opt("format", Kind::Word(FORMATS)),
    opt("sysAttrs", Kind::Flag),
    // 4.23 Entity Ordering, local scope only
    opt("orderBy", Kind::Text(1024)),
    opt("count", Kind::Flag),
    opt("limit", Kind::Limit),
    opt("cursor", Kind::Text(crate::cursor::MAX_LEN)),
];

const BATCH_QUERY: &[Arg] = &[
    req("type", Kind::Types),
    opt("id", Kind::Ids),
    opt("attrs", Kind::Attrs),
    opt("pick", Kind::Members),
    opt("omit", Kind::Members),
    opt("q", Kind::Text(4096)),
    opt("scopeQ", Kind::Text(1024)),
    opt("georel", Kind::Text(256)),
    opt("geometry", Kind::Word(GEOMETRIES)),
    opt("coordinates", Kind::Coordinates),
    opt("geoproperty", Kind::Attr),
    opt("local", Kind::Flag),
    opt("lang", Kind::Text(64)),
    opt("count", Kind::Flag),
    opt("limit", Kind::Limit),
    opt("cursor", Kind::Text(crate::cursor::MAX_LEN)),
];

const TEMPORAL_ONE: &[Arg] = &[
    req("id", Kind::Id),
    opt("attrs", Kind::Attrs),
    req("timerel", Kind::Word(TIMERELS)),
    req("timeAt", Kind::Text(64)),
    opt("endTimeAt", Kind::Text(64)),
    opt("timeproperty", Kind::Word(TIMEPROPERTIES)),
    opt("lastN", Kind::LastN),
    opt("aggrMethods", Kind::Text(256)),
    opt("aggrPeriodDuration", Kind::Text(64)),
    opt("format", Kind::Word(TEMPORAL_FORMATS)),
    opt("sysAttrs", Kind::Flag),
];

const TEMPORAL_QUERY: &[Arg] = &[
    opt("type", Kind::Types),
    opt("id", Kind::Ids),
    opt("attrs", Kind::Attrs),
    opt("q", Kind::Text(4096)),
    opt("scopeQ", Kind::Text(1024)),
    opt("georel", Kind::Text(256)),
    opt("geometry", Kind::Word(GEOMETRIES)),
    opt("coordinates", Kind::Coordinates),
    opt("geoproperty", Kind::Attr),
    req("timerel", Kind::Word(TIMERELS)),
    req("timeAt", Kind::Text(64)),
    opt("endTimeAt", Kind::Text(64)),
    opt("timeproperty", Kind::Word(TIMEPROPERTIES)),
    opt("lastN", Kind::LastN),
    opt("aggrMethods", Kind::Text(256)),
    opt("aggrPeriodDuration", Kind::Text(64)),
    opt("format", Kind::Word(TEMPORAL_FORMATS)),
    opt("sysAttrs", Kind::Flag),
    opt("count", Kind::Flag),
    opt("limit", Kind::Limit),
    opt("cursor", Kind::Text(crate::cursor::MAX_LEN)),
];

/// The POST form: its conditions go into the Query document.
const BATCH_TEMPORAL_QUERY: &[Arg] = &[
    opt("type", Kind::Types),
    opt("id", Kind::Ids),
    opt("attrs", Kind::Attrs),
    opt("q", Kind::Text(4096)),
    opt("scopeQ", Kind::Text(1024)),
    opt("georel", Kind::Text(256)),
    opt("geometry", Kind::Word(GEOMETRIES)),
    opt("coordinates", Kind::Coordinates),
    opt("geoproperty", Kind::Attr),
    req("timerel", Kind::Word(TIMERELS)),
    req("timeAt", Kind::Text(64)),
    opt("endTimeAt", Kind::Text(64)),
    opt("timeproperty", Kind::Word(TIMEPROPERTIES)),
    opt("lastN", Kind::LastN),
    opt("aggrMethods", Kind::Text(256)),
    opt("aggrPeriodDuration", Kind::Text(64)),
    opt("count", Kind::Flag),
    opt("limit", Kind::Limit),
    opt("cursor", Kind::Text(crate::cursor::MAX_LEN)),
];

const ID_ONLY: &[Arg] = &[req("id", Kind::Text(512))];
const PAGE_ONLY: &[Arg] = &[
    opt("count", Kind::Flag),
    opt("limit", Kind::Limit),
    opt("cursor", Kind::Text(crate::cursor::MAX_LEN)),
];
/// `summarize` (T-4680): one type and the filters of `query_entities`; it
/// pages by itself, so no `limit` or `cursor`.
const SUMMARIZE: &[Arg] = &[
    req("type", Kind::Types),
    opt("q", Kind::Text(4096)),
    opt("scopeQ", Kind::Text(1024)),
    opt("georel", Kind::Text(256)),
    opt("geometry", Kind::Word(GEOMETRIES)),
    opt("coordinates", Kind::Coordinates),
    opt("geoproperty", Kind::Attr),
    opt("lang", Kind::Text(64)),
];
/// `area_overview` (T-5279): one area, the types to read (every type the view
/// shows when absent) and how many Entities of each; it reads by itself.
const AREA: &[Arg] = &[
    req(
        "geometry",
        Kind::Word(&["Point", "LineString", "Polygon", "MultiPolygon"]),
    ),
    req("coordinates", Kind::Coordinates),
    opt("maxDistance", Kind::Int(1, crate::area::MAX_RADIUS_M)),
    opt("type", Kind::Types),
    opt("limit", Kind::Int(1, crate::area::MAX_K)),
    opt("lang", Kind::Text(64)),
];
const NONE: &[Arg] = &[];
const ATTR_VALUE: &[Arg] = &[req("id", Kind::Id), req("attrId", Kind::Attr)];
const TYPE_ONLY: &[Arg] = &[req("type", Kind::Type)];
const ATTR_ONLY: &[Arg] = &[req("attrId", Kind::Attr)];

const ENTITY_ID: &[Arg] = &[req("id", Kind::Id)];
const ENTITY: &[Arg] = &[req("entity", Kind::Entity)];
const REPLACE: &[Arg] = &[req("id", Kind::Id), req("entity", Kind::Entity)];
const ATTRIBUTES: &[Arg] = &[req("id", Kind::Id), req("attributes", Kind::Fragment)];
const APPEND: &[Arg] = &[
    req("id", Kind::Id),
    req("attributes", Kind::Fragment),
    opt("noOverwrite", Kind::Flag),
];
const MERGE: &[Arg] = &[req("id", Kind::Id), req("fragment", Kind::Fragment)];
const ONE_ATTRIBUTE: &[Arg] = &[
    req("id", Kind::Id),
    req("attrId", Kind::Attr),
    req("attribute", Kind::Attribute),
];
const DELETE_ATTRIBUTE: &[Arg] = &[
    req("id", Kind::Id),
    req("attrId", Kind::Attr),
    opt("datasetId", Kind::Text(512)),
    opt("deleteAll", Kind::Flag),
];
const BATCH: &[Arg] = &[req("entities", Kind::Entities)];
const BATCH_UPSERT: &[Arg] = &[
    req("entities", Kind::Entities),
    opt("options", Kind::Word(&["update", "replace"])),
];
const BATCH_UPDATE: &[Arg] = &[
    req("entities", Kind::Entities),
    opt("noOverwrite", Kind::Flag),
];
const BATCH_DELETE: &[Arg] = &[req("ids", Kind::Ids)];

/// A notification endpoint: one the deployment allows (MC12).
const ENDPOINT: &[Arg] = &[
    req("uri", Kind::Endpoint),
    opt(
        "accept",
        Kind::Word(&["application/json", "application/ld+json"]),
    ),
];
const NOTIFICATION: &[Arg] = &[
    opt("attributes", Kind::Attrs),
    opt(
        "format",
        Kind::Word(&["normalized", "concise", "keyValues"]),
    ),
    req("endpoint", Kind::Object(ENDPOINT)),
];
const SELECTOR: &[Arg] = &[
    req("type", Kind::Type),
    opt("id", Kind::Id),
    opt("idPattern", Kind::Text(256)),
];
/// The members a Subscription made through a tool may carry (5.2.12).
const SUBSCRIPTION: &[Arg] = &[
    req("type", Kind::Word(&["Subscription"])),
    opt("subscriptionName", Kind::Text(256)),
    opt("description", Kind::Text(1024)),
    opt("entities", Kind::ListOf(SELECTOR, 50)),
    opt("watchedAttributes", Kind::Attrs),
    opt("q", Kind::Text(4096)),
    req("notification", Kind::Object(NOTIFICATION)),
    opt("expiresAt", Kind::Text(64)),
    opt("throttling", Kind::Int(0, 86_400)),
    opt("isActive", Kind::Flag),
];
/// [`SUBSCRIPTION`], each member optional.
const SUBSCRIPTION_FRAGMENT: &[Arg] = &[
    opt("subscriptionName", Kind::Text(256)),
    opt("description", Kind::Text(1024)),
    opt("entities", Kind::ListOf(SELECTOR, 50)),
    opt("watchedAttributes", Kind::Attrs),
    opt("q", Kind::Text(4096)),
    opt("notification", Kind::Object(NOTIFICATION)),
    opt("expiresAt", Kind::Text(64)),
    opt("throttling", Kind::Int(0, 86_400)),
    opt("isActive", Kind::Flag),
];
const CREATE_SUBSCRIPTION: &[Arg] = &[req("subscription", Kind::Object(SUBSCRIPTION))];
const UPDATE_SUBSCRIPTION: &[Arg] = &[
    req("id", Kind::Text(512)),
    req("fragment", Kind::Object(SUBSCRIPTION_FRAGMENT)),
];

/// A write tool: one call changes data, after the person confirmed it.
const fn write(
    name: &'static str,
    op: OperationId,
    description: &'static str,
    args: &'static [Arg],
    key: &'static str,
) -> ToolDef {
    ToolDef {
        name,
        op,
        description,
        args,
        key,
        paged: false,
        write: true,
    }
}

/// Every tool, sorted by name (2026-07-28: a deterministic order).
pub(crate) const TOOLS: [ToolDef; 34] = [
    write(
        "append_attributes",
        OperationId::AppendAttributes,
        "Append Attributes to one Entity (TS 104 175 10.2.4); noOverwrite keeps those it has. The tool asks the person to confirm before it writes.",
        APPEND,
        "result",
    ),
    ToolDef {
        name: "area_overview",
        op: OperationId::QueryEntities,
        description: "Everything in one area, every Entity type at once: per type its count and the limit nearest (id, name, distance in metres, key Attributes). A Point or a LineString with maxDistance in metres, or a Polygon; at most 5000 m, 1000 positions, 100 km². Use it for \"what is here\".",
        args: AREA,
        key: "overview",
        paged: false,
        write: false,
    },
    write(
        "batch_create",
        OperationId::BatchCreate,
        "Create several Entities (TS 104 175 10.3.2). The tool asks the person to confirm before it writes.",
        BATCH,
        "result",
    ),
    write(
        "batch_delete",
        OperationId::BatchDelete,
        "Delete several Entities by id (TS 104 175 10.3.6). The tool asks the person to confirm before it writes.",
        BATCH_DELETE,
        "result",
    ),
    write(
        "batch_merge",
        OperationId::BatchMerge,
        "Merge into several Entities (TS 104 175 10.3.5). The tool asks the person to confirm before it writes.",
        BATCH,
        "result",
    ),
    ToolDef {
        name: "batch_query",
        op: OperationId::QueryEntitiesPost,
        description: "Query Entities with a Query document (TS 104 175 10.4.3, POST form): the Entities of the given types that match the conditions, a page at a time.",
        args: BATCH_QUERY,
        key: "entities",
        paged: true,
        write: false,
    },
    ToolDef {
        name: "batch_query_temporal",
        op: OperationId::TemporalQueryPost,
        description: "Query the history of Entities with a Query document (TS 104 175 11.3.3, POST form), a page of Entities at a time.",
        args: BATCH_TEMPORAL_QUERY,
        key: "entities",
        paged: true,
        write: false,
    },
    write(
        "batch_update",
        OperationId::BatchUpdate,
        "Update Attributes of several Entities (TS 104 175 10.3.4). The tool asks the person to confirm before it writes.",
        BATCH_UPDATE,
        "result",
    ),
    write(
        "batch_upsert",
        OperationId::BatchUpsert,
        "Create several Entities or update those that exist (TS 104 175 10.3.3). The tool asks the person to confirm before it writes.",
        BATCH_UPSERT,
        "result",
    ),
    write(
        "create_entity",
        OperationId::CreateEntity,
        "Create one Entity (TS 104 175 10.2.2). The tool asks the person to confirm before it writes.",
        ENTITY,
        "created",
    ),
    write(
        "create_subscription",
        OperationId::CreateSubscription,
        "Subscribe an endpoint the deployment allows to changes of Entities (TS 104 175 10.5.2). The tool asks the person to confirm before it writes.",
        CREATE_SUBSCRIPTION,
        "created",
    ),
    write(
        "delete_attribute",
        OperationId::DeleteAttribute,
        "Delete one Attribute of one Entity (TS 104 175 10.2.7). The tool asks the person to confirm before it writes.",
        DELETE_ATTRIBUTE,
        "result",
    ),
    write(
        "delete_entity",
        OperationId::DeleteEntity,
        "Delete one Entity (TS 104 175 10.2.8). The tool asks the person to confirm before it writes.",
        ENTITY_ID,
        "result",
    ),
    write(
        "delete_subscription",
        OperationId::DeleteSubscription,
        "Delete one Subscription (TS 104 175 10.5.6). The tool asks the person to confirm before it writes.",
        ID_ONLY,
        "result",
    ),
    ToolDef {
        name: "get_attribute",
        op: OperationId::RetrieveAttributeInfo,
        description: "Describe one Attribute: the types that carry it and how many instances exist (TS 104 175 10.6.8).",
        args: ATTR_ONLY,
        key: "attribute",
        paged: false,
        write: false,
    },
    ToolDef {
        name: "get_attribute_value",
        op: OperationId::RetrieveAttributeValue,
        description: "Read one Attribute of one Entity (TS 104 175 10.4.4).",
        args: ATTR_VALUE,
        key: "attribute",
        paged: false,
        write: false,
    },
    ToolDef {
        name: "get_entity",
        op: OperationId::RetrieveEntity,
        description: "Read one Entity by its id (TS 104 175 10.4.2); attrs, pick and omit narrow what it returns.",
        args: GET_ENTITY,
        key: "entity",
        paged: false,
        write: false,
    },
    ToolDef {
        name: "get_subscription",
        op: OperationId::RetrieveSubscription,
        description: "Read one Subscription made through this endpoint (TS 104 175 10.5.4).",
        args: ID_ONLY,
        key: "subscription",
        paged: false,
        write: false,
    },
    ToolDef {
        name: "get_type",
        op: OperationId::RetrieveEntityTypeInfo,
        description: "Describe one Entity type: its Attributes and how many Entities it has (TS 104 175 10.6.5).",
        args: TYPE_ONLY,
        key: "entityType",
        paged: false,
        write: false,
    },
    ToolDef {
        name: "list_attribute_details",
        op: OperationId::RetrieveAttributeDetails,
        description: "List the Attributes with the types that carry each (TS 104 175 10.6.7).",
        args: NONE,
        key: "attributes",
        paged: false,
        write: false,
    },
    ToolDef {
        name: "list_attributes",
        op: OperationId::RetrieveAttributes,
        description: "List the names of the Attributes this endpoint serves (TS 104 175 10.6.6).",
        args: NONE,
        key: "attributes",
        paged: false,
        write: false,
    },
    ToolDef {
        name: "list_subscriptions",
        op: OperationId::QuerySubscriptions,
        description: "List the Subscriptions made through this endpoint (TS 104 175 10.5.5), a page at a time.",
        args: PAGE_ONLY,
        key: "subscriptions",
        paged: true,
        write: false,
    },
    ToolDef {
        name: "list_type_details",
        op: OperationId::RetrieveEntityTypeDetails,
        description: "List the Entity types with the Attributes of each (TS 104 175 10.6.4).",
        args: NONE,
        key: "entityTypes",
        paged: false,
        write: false,
    },
    ToolDef {
        name: "list_types",
        op: OperationId::RetrieveEntityTypes,
        description: "List the names of the Entity types this endpoint serves (TS 104 175 10.6.3).",
        args: NONE,
        key: "entityTypes",
        paged: false,
        write: false,
    },
    write(
        "merge_entity",
        OperationId::MergeEntity,
        "Merge a fragment into one Entity (TS 104 175 10.2.9). The tool asks the person to confirm before it writes.",
        MERGE,
        "result",
    ),
    ToolDef {
        name: "query_entities",
        op: OperationId::QueryEntities,
        description: "Query Entities (TS 104 175 10.4.3): by type, id, q, scopeQ or area, a page at a time. Give at least one of type, attrs, q, georel or local.",
        args: QUERY,
        key: "entities",
        paged: true,
        write: false,
    },
    ToolDef {
        name: "query_temporal",
        op: OperationId::TemporalQuery,
        description: "Query the history of Entities (TS 104 175 11.3.3) between timeAt and endTimeAt, a page of Entities at a time; lastN bounds each Attribute's instances.",
        args: TEMPORAL_QUERY,
        key: "entities",
        paged: true,
        write: false,
    },
    write(
        "replace_attribute",
        OperationId::ReplaceAttribute,
        "Replace one Attribute of one Entity (TS 104 175 10.2.11). The tool asks the person to confirm before it writes.",
        ONE_ATTRIBUTE,
        "result",
    ),
    write(
        "replace_entity",
        OperationId::ReplaceEntity,
        "Replace one Entity whole (TS 104 175 10.2.10). The tool asks the person to confirm before it writes.",
        REPLACE,
        "result",
    ),
    ToolDef {
        name: "retrieve_temporal",
        op: OperationId::TemporalRetrieve,
        description: "Read the history of one Entity (TS 104 175 11.3.2) between timeAt and endTimeAt; lastN bounds each Attribute's instances.",
        args: TEMPORAL_ONE,
        key: "entity",
        paged: false,
        write: false,
    },
    ToolDef {
        name: "summarize",
        op: OperationId::QueryEntities,
        description: "Profile one Entity type: its count and, per Attribute, statistics (min, max, mean, median, the most common values, the area) over at most 1,000 of the Entities the caller reads, and five of them. Use it for a broad question; use query_entities for specific Entities.",
        args: SUMMARIZE,
        key: "summary",
        paged: false,
        write: false,
    },
    write(
        "update_attribute",
        OperationId::PartialAttributeUpdate,
        "Update part of one Attribute of one Entity (TS 104 175 10.2.5). The tool asks the person to confirm before it writes.",
        ONE_ATTRIBUTE,
        "result",
    ),
    write(
        "update_attributes",
        OperationId::UpdateAttributes,
        "Update Attributes one Entity has (TS 104 175 10.2.3). The tool asks the person to confirm before it writes.",
        ATTRIBUTES,
        "result",
    ),
    write(
        "update_subscription",
        OperationId::UpdateSubscription,
        "Update one Subscription (TS 104 175 10.5.3). The tool asks the person to confirm before it writes.",
        UPDATE_SUBSCRIPTION,
        "result",
    ),
];

/// The tool named `name`.
pub(crate) fn named(name: &str) -> Option<&'static ToolDef> {
    TOOLS.iter().find(|t| t.name == name)
}

/// One NGSI-LD request: method, path and query below the API root, body.
#[derive(Debug, PartialEq)]
pub(crate) struct Built {
    pub method: Method,
    pub target: String,
    pub body: Option<Value>,
}

/// `s` as one URI component: unreserved characters and `:` stay, the rest
/// is percent-encoded.
pub(crate) fn encode(s: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~:".contains(&b) {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

pub(crate) fn text(c: &Checked) -> String {
    match c {
        Checked::One(s) => s.clone(),
        Checked::Many(v) => v.join(","),
        Checked::Int(n) => n.to_string(),
        Checked::Flag(b) => b.to_string(),
        Checked::Json(v) => v.to_string(),
    }
}

fn one<'a>(args: &'a Args, name: &str) -> &'a str {
    match args.get(name) {
        Some(Checked::One(s)) => s,
        _ => "",
    }
}

/// The query string of `args` but those the tool handles itself; the
/// `sysAttrs` flag is the `options=sysAttrs` of 6.3.7.
fn query(args: &Args, skip: &[&str], extra: &[(&str, String)]) -> String {
    let mut pairs: Vec<String> = args
        .iter()
        .filter(|(k, _)| !skip.contains(k))
        .filter_map(|(k, v)| match (*k, v) {
            ("sysAttrs", Checked::Flag(true)) => Some("options=sysAttrs".to_owned()),
            ("sysAttrs", _) => None,
            _ => Some(format!("{k}={}", encode(&text(v)))),
        })
        .collect();
    pairs.extend(extra.iter().map(|(k, v)| format!("{k}={}", encode(v))));
    if pairs.is_empty() {
        String::new()
    } else {
        format!("?{}", pairs.join("&"))
    }
}

/// Members a page tool handles itself rather than passing on.
const PAGING: [&str; 3] = ["count", "limit", "cursor"];

/// Whether a page of `limit` asks one row past it instead of the count: a
/// call without `count` learns from that row whether rows are left, and the
/// store counts nothing (T-5062: a count per page held 80 % of PostgreSQL's
/// time at 1 000 distinct reads a second).
pub(crate) fn probes(args: &Args, limit: u64) -> bool {
    probes_under(args, limit, schema::limit_max())
}

/// [`probes`] under the broker's `limit` ceiling `max` (`ANTARES_MAX_LIMIT`):
/// a page this large cannot ask one row more.
fn probes_under(args: &Args, limit: u64, max: u64) -> bool {
    !matches!(args.get("count"), Some(Checked::Flag(true))) && limit < max
}

/// The paging members of an inner request: one row past the page when
/// [`probes`], else `count=true`, so the next cursor knows whether rows are
/// left.
fn page(args: &Args, limit: u64, offset: u64) -> [(&'static str, String); 3] {
    let (limit, count) = if probes(args, limit) {
        (limit.saturating_add(1), "false")
    } else {
        (limit, "true")
    };
    [
        ("limit", limit.to_string()),
        ("offset", offset.to_string()),
        ("count", count.into()),
    ]
}

/// `c` as the JSON it was given as.
fn json_of(c: &Checked) -> Value {
    match c {
        Checked::Many(list) => json!(list),
        Checked::Json(j) => j.clone(),
        Checked::Int(n) => json!(n),
        Checked::Flag(b) => json!(b),
        Checked::One(s) => json!(s),
    }
}

/// A Query document (5.2.23) of `args`.
fn query_document(args: &Args, temporal: bool) -> Value {
    let types = text(args.get("type").unwrap_or(&Checked::Many(Vec::new())));
    let entities: Vec<Value> = match args.get("id") {
        Some(Checked::Many(ids)) => ids
            .iter()
            .map(|id| json!({"id": id, "type": types}))
            .collect(),
        _ => vec![json!({"type": types})],
    };
    let mut doc = serde_json::Map::new();
    doc.insert("type".into(), json!("Query"));
    doc.insert("entities".into(), json!(entities));
    let (mut geo, mut time) = (serde_json::Map::new(), serde_json::Map::new());
    for (k, v) in args {
        let value = json_of(v);
        let into = match *k {
            "type" | "id" | "count" | "limit" | "cursor" => continue,
            "georel" | "geometry" | "coordinates" | "geoproperty" => &mut geo,
            "timerel" | "timeAt" | "endTimeAt" | "timeproperty" if temporal => &mut time,
            _ => &mut doc,
        };
        into.insert((*k).to_owned(), value);
    }
    for (key, part) in [("geoQ", geo), ("temporalQ", time)] {
        if !part.is_empty() {
            doc.insert(key.into(), Value::Object(part));
        }
    }
    Value::Object(doc)
}

/// The request `tool` makes of `args`, at `offset` with `limit` when it
/// pages.
pub(crate) fn build(tool: &ToolDef, args: &Args, limit: u64, offset: u64) -> Built {
    let get = |target: String| Built {
        method: Method::GET,
        target,
        body: None,
    };
    let root = "/ngsi-ld/v1";
    if let Some(built) = build_write(tool, args) {
        return built;
    }
    match tool.op {
        OperationId::RetrieveEntity => get(format!(
            "{root}/entities/{}{}",
            encode(one(args, "id")),
            query(args, &["id"], &[])
        )),
        OperationId::RetrieveAttributeValue => get(format!(
            "{root}/entities/{}/attrs/{}",
            encode(one(args, "id")),
            encode(one(args, "attrId"))
        )),
        OperationId::QueryEntities => get(format!(
            "{root}/entities{}",
            query(args, &PAGING, &page(args, limit, offset))
        )),
        OperationId::QueryEntitiesPost => Built {
            method: Method::POST,
            target: format!(
                "{root}/entityOperations/query{}",
                query(&Args::new(), &[], &page(args, limit, offset))
            ),
            body: Some(query_document(args, false)),
        },
        OperationId::RetrieveSubscription => {
            get(format!("{root}/subscriptions/{}", encode(one(args, "id"))))
        }
        OperationId::QuerySubscriptions => get(format!(
            "{root}/subscriptions{}",
            query(args, &PAGING, &page(args, limit, offset))
        )),
        OperationId::RetrieveEntityTypes => get(format!("{root}/types")),
        OperationId::RetrieveEntityTypeDetails => get(format!("{root}/types?details=true")),
        OperationId::RetrieveEntityTypeInfo => {
            get(format!("{root}/types/{}", encode(one(args, "type"))))
        }
        OperationId::RetrieveAttributes => get(format!("{root}/attributes")),
        OperationId::RetrieveAttributeDetails => get(format!("{root}/attributes?details=true")),
        OperationId::RetrieveAttributeInfo => {
            get(format!("{root}/attributes/{}", encode(one(args, "attrId"))))
        }
        OperationId::TemporalRetrieve => get(format!(
            "{root}/temporal/entities/{}{}",
            encode(one(args, "id")),
            query(args, &["id"], &[])
        )),
        OperationId::TemporalQueryPost => Built {
            method: Method::POST,
            target: format!(
                "{root}/temporal/entityOperations/query{}",
                query(&Args::new(), &[], &page(args, limit, offset))
            ),
            body: Some(query_document(args, true)),
        },
        // TemporalQuery, the only other operation of the table
        _ => get(format!(
            "{root}/temporal/entities{}",
            query(args, &PAGING, &page(args, limit, offset))
        )),
    }
}

/// The request of a write tool; `None` for a read tool.
fn build_write(tool: &ToolDef, args: &Args) -> Option<Built> {
    let root = "/ngsi-ld/v1";
    let send = |method: Method, target: String, member: &str| Built {
        method,
        target,
        body: args.get(member).map(json_of),
    };
    let entity = |rest: &str| format!("{root}/entities/{}{rest}", encode(one(args, "id")));
    let attrs = |q: &str| entity(&format!("/attrs{q}"));
    let attr = |q: String| entity(&format!("/attrs/{}{q}", encode(one(args, "attrId"))));
    let batch = |what: &str, q: &str| format!("{root}/entityOperations/{what}{q}");
    let subscription = || format!("{root}/subscriptions/{}", encode(one(args, "id")));
    let no_overwrite = if args.get("noOverwrite") == Some(&Checked::Flag(true)) {
        "?options=noOverwrite"
    } else {
        ""
    };
    Some(match tool.op {
        OperationId::CreateEntity => send(Method::POST, format!("{root}/entities"), "entity"),
        OperationId::UpdateAttributes => send(Method::PATCH, attrs(""), "attributes"),
        OperationId::AppendAttributes => send(Method::POST, attrs(no_overwrite), "attributes"),
        OperationId::PartialAttributeUpdate => {
            send(Method::PATCH, attr(String::new()), "attribute")
        }
        OperationId::ReplaceAttribute => send(Method::PUT, attr(String::new()), "attribute"),
        OperationId::DeleteAttribute => Built {
            method: Method::DELETE,
            target: attr(query(args, &["id", "attrId"], &[])),
            body: None,
        },
        OperationId::DeleteEntity => Built {
            method: Method::DELETE,
            target: entity(""),
            body: None,
        },
        OperationId::MergeEntity => send(Method::PATCH, entity(""), "fragment"),
        OperationId::ReplaceEntity => send(Method::PUT, entity(""), "entity"),
        OperationId::BatchCreate => send(Method::POST, batch("create", ""), "entities"),
        OperationId::BatchUpsert => {
            let options = query(args, &["entities"], &[]);
            send(Method::POST, batch("upsert", &options), "entities")
        }
        OperationId::BatchUpdate => send(Method::POST, batch("update", no_overwrite), "entities"),
        OperationId::BatchMerge => send(Method::POST, batch("merge", ""), "entities"),
        OperationId::BatchDelete => send(Method::POST, batch("delete", ""), "ids"),
        OperationId::CreateSubscription => send(
            Method::POST,
            format!("{root}/subscriptions"),
            "subscription",
        ),
        OperationId::UpdateSubscription => send(Method::PATCH, subscription(), "fragment"),
        OperationId::DeleteSubscription => Built {
            method: Method::DELETE,
            target: subscription(),
            body: None,
        },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {

    /// T-6612, R3-026: one String per call, the same text as before.
    #[test]
    fn a_component_is_encoded_with_colons_kept() {
        assert_eq!(
            super::encode("urn:ngsi-ld:Car:a b/ä%"),
            "urn:ngsi-ld:Car:a%20b%2F%C3%A4%25"
        );
        assert_eq!(super::encode(""), "");
    }
    use super::*;

    fn args(pairs: &[(&'static str, Checked)]) -> Args {
        pairs.iter().cloned().collect()
    }

    #[test]
    fn a_write_tool_writes_and_never_pages() {
        // `write` runs at compile time in `TOOLS`; once here at run time
        let t = write("x", OperationId::DeleteEntity, "d", ENTITY_ID, "result");
        assert_eq!((t.write, t.paged), (true, false));
        assert_eq!(TOOLS.iter().filter(|t| t.write).count(), 17);
    }

    #[test]
    fn the_table_is_sorted_and_each_tool_is_its_own_row() {
        let names: Vec<&str> = TOOLS.iter().map(|t| t.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
        // `summarize` (T-4680) reads through `query_entities`' operation and
        // answers a profile of it; every other tool is its operation's own row.
        let mut ops: Vec<&str> = TOOLS
            .iter()
            .filter(|t| t.name != "summarize" && t.name != "area_overview")
            .map(|t| t.op.id())
            .collect();
        ops.sort_unstable();
        ops.dedup();
        assert_eq!(ops.len(), TOOLS.len().saturating_sub(2));
    }

    #[test]
    fn every_tool_builds_a_request_of_its_own_operation() {
        let all = args(&[
            ("id", Checked::One("urn:ngsi-ld:Car:1".into())),
            ("attrId", Checked::One("speed".into())),
            ("type", Checked::One("Car".into())),
            ("timerel", Checked::One("after".into())),
            ("timeAt", Checked::One("2020-01-01T00:00:00Z".into())),
        ]);
        for tool in &TOOLS {
            let b = build(tool, &all, 20, 0);
            let (path, q) = match b.target.split_once('?') {
                Some((p, q)) => (p, Some(q)),
                None => (b.target.as_str(), None),
            };
            assert_eq!(
                OperationId::of(b.method.as_str(), path, q),
                Some(tool.op),
                "{} built {} {}",
                tool.name,
                b.method,
                b.target
            );
        }
    }

    #[test]
    fn values_are_encoded_and_paging_is_the_tools() {
        let a = args(&[
            ("q", Checked::One("speed>1;name==\"a b\"".into())),
            ("type", Checked::Many(vec!["Car".into(), "Bike".into()])),
            ("limit", Checked::Int(5)),
            ("cursor", Checked::One("ab".into())),
        ]);
        let target = |name: &str, a: &Args, limit, offset| {
            named(name).map(|t| build(t, a, limit, offset).target)
        };
        assert_eq!(
            target("query_entities", &a, 5, 40).unwrap_or_default(),
            "/ngsi-ld/v1/entities?q=speed%3E1%3Bname%3D%3D%22a%20b%22&type=Car%2CBike&limit=6&offset=40&count=false"
        );
        // T-5062: a page asks one row past it, unless the call counts or the
        // page is already at the broker's limit ceiling
        let mut counted = a.clone();
        counted.insert("count", Checked::Flag(true));
        assert_eq!(
            target("query_entities", &counted, 5, 40).unwrap_or_default(),
            "/ngsi-ld/v1/entities?q=speed%3E1%3Bname%3D%3D%22a%20b%22&type=Car%2CBike&limit=5&offset=40&count=true"
        );
        counted.insert("count", Checked::Flag(false));
        assert!(target("query_entities", &counted, 5, 40)
            .unwrap_or_default()
            .ends_with("&limit=6&offset=40&count=false"));
        assert!(target("query_entities", &a, schema::limit_max(), 0)
            .unwrap_or_default()
            .ends_with("&limit=1000&offset=0&count=true"));
        // F-ANT1-010: the ceiling is the broker's setting, so a lower
        // ANTARES_MAX_LIMIT never sees limit + 1 at its maximum
        assert!(!probes_under(&a, 500, 500));
        assert!(probes_under(&a, 499, 500));
        let one = args(&[("id", Checked::One("urn:ngsi-ld:Car:a/b?c".into()))]);
        assert_eq!(
            target("get_entity", &one, 0, 0).unwrap_or_default(),
            "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:a%2Fb%3Fc"
        );
    }

    #[test]
    fn a_query_document_carries_the_conditions() {
        let a = args(&[
            ("type", Checked::Many(vec!["Car".into()])),
            ("id", Checked::Many(vec!["urn:ngsi-ld:Car:1".into()])),
            ("q", Checked::One("speed>1".into())),
            ("georel", Checked::One("near;maxDistance==10".into())),
            ("coordinates", Checked::Json(json!([1, 2]))),
            ("timerel", Checked::One("after".into())),
            ("limit", Checked::Int(3)),
            ("local", Checked::Flag(true)),
        ]);
        assert_eq!(
            query_document(&a, true),
            json!({
                "type": "Query",
                "entities": [{"id": "urn:ngsi-ld:Car:1", "type": "Car"}],
                "q": "speed>1",
                "local": true,
                "geoQ": {"georel": "near;maxDistance==10", "coordinates": [1, 2]},
                "temporalQ": {"timerel": "after"}
            })
        );
        assert_eq!(
            query_document(&args(&[("type", Checked::Many(vec!["Car".into()]))]), false),
            json!({"type": "Query", "entities": [{"type": "Car"}]})
        );
        assert_eq!(
            query_document(&args(&[("timerel", Checked::One("after".into()))]), false),
            json!({"type": "Query", "entities": [{"type": ""}], "timerel": "after"}),
            "outside a temporal query a time member is an ordinary one"
        );
    }

    /// T-4258: a representation is the standard's: `format` as given,
    /// `sysAttrs` as `options=sysAttrs`, `orderBy` as given; the POST
    /// temporal form takes none of them into its document.
    #[test]
    fn format_sys_attrs_and_order_by_are_the_standard_parameters() {
        let a = args(&[
            ("type", Checked::Many(vec!["Car".into()])),
            ("format", Checked::One("keyValues".into())),
            ("sysAttrs", Checked::Flag(true)),
            ("orderBy", Checked::One("speed;desc".into())),
        ]);
        let q = named("query_entities").map(|t| build(t, &a, 5, 0).target);
        assert_eq!(
            q.as_deref(),
            Some("/ngsi-ld/v1/entities?format=keyValues&orderBy=speed%3Bdesc&options=sysAttrs&type=Car&limit=6&offset=0&count=false")
        );
        let off = args(&[
            ("id", Checked::One("urn:ngsi-ld:Car:1".into())),
            ("sysAttrs", Checked::Flag(false)),
        ]);
        let e = named("get_entity").map(|t| build(t, &off, 0, 0).target);
        assert_eq!(e.as_deref(), Some("/ngsi-ld/v1/entities/urn:ngsi-ld:Car:1"));
        let arg_names = |name: &str| -> Vec<&str> {
            named(name).map_or(Vec::new(), |t| t.args.iter().map(|a| a.name).collect())
        };
        for name in [
            "get_entity",
            "query_entities",
            "query_temporal",
            "retrieve_temporal",
        ] {
            let names = arg_names(name);
            assert_eq!(
                (names.contains(&"format"), names.contains(&"sysAttrs")),
                (true, true),
                "{name}"
            );
        }
        assert!(!arg_names("batch_query_temporal").contains(&"format"));
        let kinds = |name: &str| {
            named(name)
                .and_then(|t| t.args.iter().find(|a| a.name == "format"))
                .map(|a| a.kind)
        };
        assert_eq!(kinds("query_temporal"), Some(Kind::Word(TEMPORAL_FORMATS)));
        assert_eq!(kinds("get_entity"), Some(Kind::Word(FORMATS)));
    }

    #[test]
    fn every_kind_of_value_reaches_the_query_string() {
        let a = args(&[
            ("type", Checked::Many(vec!["Car".into()])),
            ("joinLevel", Checked::Int(2)),
            ("local", Checked::Flag(true)),
            ("coordinates", Checked::Json(json!([1, 2]))),
        ]);
        let q = named("query_entities").map(|t| build(t, &a, 5, 0).target);
        assert_eq!(
            q.as_deref(),
            Some("/ngsi-ld/v1/entities?coordinates=%5B1%2C2%5D&joinLevel=2&local=true&type=Car&limit=6&offset=0&count=false")
        );
        let e = named("get_entity").map(|t| build(t, &Args::new(), 0, 0).target);
        assert_eq!(
            e.as_deref(),
            Some("/ngsi-ld/v1/entities/"),
            "a missing path member is empty"
        );
    }

    /// Every argument name of `args`, those of nested objects included.
    fn argument_names(args: &[Arg], out: &mut Vec<&'static str>) {
        for arg in args {
            out.push(arg.name);
            if let Kind::Object(inner) | Kind::ListOf(inner, _) = arg.kind {
                argument_names(inner, out);
            }
        }
    }

    /// TS 104 175 5.2.4 NOTE 2: every temporal tool offers expiresAt as a
    /// timeproperty, the schema enum the client sees included.
    #[test]
    fn every_temporal_tool_offers_expires_at() {
        let offered: Vec<Kind> = TOOLS
            .iter()
            .flat_map(|t| t.args)
            .filter(|a| a.name == "timeproperty")
            .map(|a| a.kind)
            .collect();
        // query_temporal, retrieve_temporal, batch_query_temporal
        assert_eq!(offered, vec![Kind::Word(TIMEPROPERTIES); 3]);
        assert_eq!(TIMEPROPERTIES.last(), Some(&"expiresAt"));
    }

    /// T-4532: an argument a tool takes is explained in the primer, so a new one without its
    /// line fails here.
    #[test]
    fn every_tool_argument_is_explained_in_the_primer() {
        let words: std::collections::BTreeSet<&str> = crate::PRIMER
            .split(|c: char| !c.is_ascii_alphanumeric())
            .collect();
        let mut names = Vec::new();
        for tool in &TOOLS {
            argument_names(tool.args, &mut names);
        }
        let unexplained: Vec<&str> = names.into_iter().filter(|n| !words.contains(n)).collect();
        assert!(unexplained.is_empty(), "not in the primer: {unexplained:?}");
        assert!(crate::PRIMER.len() < 4_400, "{}", crate::PRIMER.len());
    }
}
