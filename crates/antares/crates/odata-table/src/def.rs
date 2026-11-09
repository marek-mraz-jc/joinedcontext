// SPDX-License-Identifier: MIT
//! One definition per entity set, written once (owner 2026-11-07): its fields, the operations it
//! offers and the permission each needs. From it come the CSDL model (`$metadata` and URL
//! parsing), the OpenAPI 3.1 document, the [`TableInfo`] a source serves, and [`allowed`], the
//! "am I allowed?" answer that runs the same decision as the real call. The definition holds
//! plain names and enums only, no OData type, so MCP tool lists and CLI commands can read it too.

use std::sync::Arc;

use odata_csdl::{
    ComplexType, CsdlVersion, EntityContainer, EntitySet, EntityType, Model, Property, Schema,
    TypeRef,
};
use serde_json::{json, Map, Value};

use crate::{key_options, query, Scope, Table, TableError, TableInfo};

/// The value type of one field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldKind {
    /// Text.
    String,
    /// A 64-bit integer.
    Integer,
    /// A double-precision number.
    Number,
    /// `true` or `false`.
    Boolean,
    /// An RFC 3339 timestamp with offset.
    DateTime,
    /// A nested object of its own named type.
    Object {
        /// The type's name, unique in the service.
        name: String,
        /// Its fields.
        fields: Vec<Field>,
    },
}

/// One field of an entity set or nested object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// The name as rows hold it.
    pub name: String,
    /// The value type.
    pub kind: FieldKind,
    /// One sentence for people and models reading the documents.
    pub description: String,
    /// Whether a row may hold `null` (or nothing) here. A key never may.
    pub nullable: bool,
}

impl Field {
    /// A nullable field.
    #[must_use]
    pub fn new(name: &str, kind: FieldKind, description: &str) -> Self {
        Self {
            name: name.to_owned(),
            kind,
            description: description.to_owned(),
            nullable: true,
        }
    }
}

/// What a caller does to an entity set or one entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// Query the set or read one entity.
    Read,
    /// Export the set as plain JSON rows.
    Export,
    /// Create an entity.
    Create,
    /// Change an entity.
    Update,
    /// Delete an entity.
    Delete,
}

impl Operation {
    /// Every operation.
    pub const ALL: [Self; 5] = [
        Self::Read,
        Self::Export,
        Self::Create,
        Self::Update,
        Self::Delete,
    ];

    /// Its name in a URL (`op=update`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Export => "export",
            Self::Create => "create",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }

    /// The operation named `name`.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|op| op.as_str() == name)
    }

    fn writes(self) -> bool {
        matches!(self, Self::Create | Self::Update | Self::Delete)
    }
}

/// One operation a set offers and the permission a caller needs for it; `None` needs none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// The operation.
    pub operation: Operation,
    /// The permission ([`Scope::permissions`]) it needs.
    pub permission: Option<String>,
}

/// Read and export offered to every scoped caller: what a set built from a bare model offers.
#[must_use]
pub fn open_rules() -> Vec<Rule> {
    [Operation::Read, Operation::Export]
        .into_iter()
        .map(|operation| Rule {
            operation,
            permission: None,
        })
        .collect()
}

/// One entity set: its name, entity type, key, fields and operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityDef {
    /// The set's name (`Endpoints`).
    pub set: String,
    /// The entity type's name, unqualified (`Endpoint`).
    pub type_name: String,
    /// One sentence on what an entity is.
    pub description: String,
    /// The key field: a string or integer field.
    pub key: String,
    /// The fields, the key among them.
    pub fields: Vec<Field>,
    /// The operations offered; an operation not listed is refused.
    pub rules: Vec<Rule>,
}

/// Entity sets served together under one namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    /// The CSDL namespace (`JoinedContext.Portal`).
    pub namespace: String,
    /// The OpenAPI title.
    pub title: String,
    /// The sets.
    pub sets: Vec<EntityDef>,
}

fn edm(kind: &FieldKind, namespace: &str) -> String {
    match kind {
        FieldKind::String => "Edm.String".into(),
        FieldKind::Integer => "Edm.Int64".into(),
        FieldKind::Number => "Edm.Double".into(),
        FieldKind::Boolean => "Edm.Boolean".into(),
        FieldKind::DateTime => "Edm.DateTimeOffset".into(),
        FieldKind::Object { name, .. } => format!("{namespace}.{name}"),
    }
}

fn property(field: &Field, namespace: &str, key: bool) -> Property {
    let mut p = Property::new(&field.name, TypeRef::single(edm(&field.kind, namespace)));
    p.nullable = field.nullable && !key;
    p
}

/// Every nested object type of `fields`, depth first, each once.
fn objects<'f>(fields: &'f [Field], out: &mut Vec<(&'f str, &'f [Field])>) {
    for field in fields {
        if let FieldKind::Object { name, fields } = &field.kind {
            if !out.iter().any(|(n, _)| n == name) {
                out.push((name, fields));
                objects(fields, out);
            }
        }
    }
}

impl Service {
    fn objects(&self) -> Vec<(&str, &[Field])> {
        let mut out = Vec::new();
        for set in &self.sets {
            objects(&set.fields, &mut out);
        }
        out
    }

    /// The CSDL model: one entity type and set per definition, one complex type per object.
    #[must_use]
    pub fn model(&self) -> Model {
        let ns = &self.namespace;
        let mut schema = Schema::new(ns.clone());
        let mut container = EntityContainer::new("Service");
        for set in &self.sets {
            let mut ty = EntityType::new(&set.type_name);
            ty.key = vec![set.key.clone()];
            ty.properties = set
                .fields
                .iter()
                .map(|f| property(f, ns, f.name == set.key))
                .collect();
            schema.entity_types.push(ty);
            container
                .entity_sets
                .push(EntitySet::new(&set.set, format!("{ns}.{}", set.type_name)));
        }
        for (name, fields) in self.objects() {
            let mut complex = ComplexType::new(name);
            complex.properties = fields.iter().map(|f| property(f, ns, false)).collect();
            schema.complex_types.push(complex);
        }
        schema.entity_container = Some(container);
        Model {
            version: CsdlVersion::V4_01,
            references: vec![],
            schemas: vec![schema],
        }
    }

    /// What a source serving `set` knows, its operations from the definition; `model` is
    /// [`Service::model`]'s, shared by every set.
    #[must_use]
    pub fn info(&self, model: Arc<Model>, set: &str) -> Option<TableInfo> {
        let def = self.sets.iter().find(|d| d.set == set)?;
        let mut info = TableInfo::new(model, set)?;
        info.rules.clone_from(&def.rules);
        Some(info)
    }

    /// The OpenAPI 3.1 document of the sets as `table_router` serves them under `service_root`.
    #[must_use]
    pub fn openapi(&self, service_root: &str) -> Value {
        let mut paths = Map::new();
        let mut schemas = Map::new();
        for set in &self.sets {
            set_paths(set, &mut paths);
            schemas.insert(
                set.type_name.clone(),
                object_schema(&set.fields, &set.description, Some(&set.key)),
            );
        }
        for (name, fields) in self.objects() {
            schemas.insert(name.to_owned(), object_schema(fields, "", None));
        }
        schemas.insert(
            "Decision".into(),
            json!({"type": "object", "properties": {
            "allowed": {"type": "boolean"},
            "reason": {"type": "string"},
            "permission": {"type": ["string", "null"]}
        }, "required": ["allowed", "reason", "permission"]}),
        );
        schemas.insert(
            "Error".into(),
            json!({"type": "object", "properties": {"error": {"type": "object", "properties": {
            "code": {"type": "string"}, "message": {"type": "string"}
        }, "required": ["code", "message"]}}, "required": ["error"]}),
        );
        let param = |name: &str, schema: Value, text: &str| json!({"name": format!("${name}"), "in": "query", "required": false, "schema": schema, "description": text});
        json!({
            "openapi": "3.1.0",
            "info": {"title": self.title, "version": "1"},
            "servers": [{"url": service_root}],
            "paths": paths,
            "components": {
                "schemas": schemas,
                "responses": {"Error": {"description": "An OData error", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Error"}}}}},
                "parameters": {
                    "filter": param("filter", json!({"type": "string"}), "Rows to keep, e.g. size gt 10 and contains(name,'a')"),
                    "select": param("select", json!({"type": "string"}), "Fields to answer, e.g. id,name"),
                    "orderby": param("orderby", json!({"type": "string"}), "Sort order, e.g. size desc,id"),
                    "top": param("top", json!({"type": "integer", "minimum": 0}), "Rows at most, e.g. 50"),
                    "skip": param("skip", json!({"type": "integer", "minimum": 0}), "Rows to skip, e.g. 100"),
                    "count": param("count", json!({"type": "boolean"}), "Answer the number of matching rows too"),
                    "skiptoken": param("skiptoken", json!({"type": "string"}), "The next page, as the previous nextLink names it")
                }
            }
        })
    }
}

/// The five paths `table_router` serves for `set`.
fn set_paths(set: &EntityDef, paths: &mut Map<String, Value>) {
    let s = &set.set;
    let entity = json!({"$ref": format!("#/components/schemas/{}", set.type_name)});
    let key_kind = set
        .fields
        .iter()
        .find(|f| f.name == set.key)
        .map(|f| &f.kind);
    let key_schema = match key_kind {
        Some(FieldKind::Integer) => json!({"type": "integer", "format": "int64"}),
        _ => json!({"type": "string"}),
    };
    let responses = |ok: Value| {
        let e = json!({"$ref": "#/components/responses/Error"});
        json!({"200": ok, "400": e, "403": e, "404": e, "501": e})
    };
    let query = |names: &[&str]| -> Vec<Value> {
        names
            .iter()
            .map(|n| json!({"$ref": format!("#/components/parameters/{n}")}))
            .collect()
    };
    let list = responses(
        json!({"description": format!("A page of {s}"), "content": {"application/json": {"schema": {
            "type": "object",
            "properties": {
                "value": {"type": "array", "items": entity},
                "@odata.count": {"type": "integer"},
                "@odata.nextLink": {"type": "string", "format": "uri"}
            },
            "required": ["value"]
        }}}}),
    );
    paths.insert(format!("/{s}"), json!({"get": {
        "operationId": format!("{s}.list"), "summary": format!("Query {s}"), "description": set.description,
        "parameters": query(&["filter", "select", "orderby", "top", "skip", "count", "skiptoken"]),
        "responses": list
    }}));
    let one = responses(
        json!({"description": format!("One of {s}"), "content": {"application/json": {"schema": entity}}}),
    );
    let key = json!({"name": "key", "in": "path", "required": true, "schema": key_schema});
    paths.insert(format!("/{s}({{key}})"), json!({"get": {
        "operationId": format!("{s}.get"), "summary": format!("Read one of {s} by its {}", set.key),
        "parameters": [key, json!({"$ref": "#/components/parameters/select"})],
        "responses": one
    }}));
    let count = responses(
        json!({"description": "The number of matching rows", "content": {"text/plain": {"schema": {"type": "integer"}}}}),
    );
    paths.insert(
        format!("/{s}/$count"),
        json!({"get": {
            "operationId": format!("{s}.count"), "summary": format!("Count {s}"),
            "parameters": query(&["filter"]), "responses": count
        }}),
    );
    let export = responses(
        json!({"description": format!("Every matching row of {s} as plain JSON"), "content": {"application/json": {"schema": {"type": "array", "items": entity}}}}),
    );
    paths.insert(
        format!("/{s}/$export"),
        json!({"get": {
            "operationId": format!("{s}.export"), "summary": format!("Export {s}"),
            "parameters": query(&["filter", "select", "orderby"]), "responses": export
        }}),
    );
    let allowed = responses(
        json!({"description": "Whether the caller may run the operation", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Decision"}}}}),
    );
    let ops: Vec<&str> = Operation::ALL.iter().map(|o| o.as_str()).collect();
    paths.insert(format!("/{s}/$allowed"), json!({"get": {
        "operationId": format!("{s}.allowed"), "summary": format!("Ask whether an operation on {s} is allowed, without running it"),
        "parameters": [
            {"name": "op", "in": "query", "required": true, "schema": {"type": "string", "enum": ops}, "description": "The operation, e.g. update"},
            {"name": "key", "in": "query", "required": false, "schema": key_schema, "description": "The entity, e.g. t1; absent for the whole set"}
        ],
        "responses": allowed
    }}));
}

fn object_schema(fields: &[Field], description: &str, key: Option<&str>) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for f in fields {
        let mut schema = match &f.kind {
            FieldKind::String => json!({"type": "string"}),
            FieldKind::Integer => json!({"type": "integer", "format": "int64"}),
            FieldKind::Number => json!({"type": "number", "format": "double"}),
            FieldKind::Boolean => json!({"type": "boolean"}),
            FieldKind::DateTime => json!({"type": "string", "format": "date-time"}),
            FieldKind::Object { name, .. } => {
                json!({"$ref": format!("#/components/schemas/{name}")})
            }
        };
        let is_key = key == Some(f.name.as_str());
        if f.nullable && !is_key {
            schema = match schema.get("type").and_then(Value::as_str) {
                Some(ty) => {
                    let mut s = schema.clone();
                    if let Some(o) = s.as_object_mut() {
                        o.insert("type".into(), json!([ty, "null"]));
                    }
                    s
                }
                None => json!({"anyOf": [schema, {"type": "null"}]}),
            };
        }
        if !f.description.is_empty() {
            if let Some(o) = schema.as_object_mut() {
                o.insert("description".into(), json!(f.description));
            }
        }
        if is_key {
            required.push(json!(f.name));
        }
        properties.insert(f.name.clone(), schema);
    }
    let mut out = Map::new();
    out.insert("type".into(), json!("object"));
    out.insert("properties".into(), Value::Object(properties));
    out.insert("required".into(), Value::Array(required));
    if !description.is_empty() {
        out.insert("description".into(), json!(description));
    }
    Value::Object(out)
}

/// The answer to "may this caller run this operation?".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// Whether the call would be served.
    pub allowed: bool,
    /// Why, in words a person acts on.
    pub reason: String,
    /// The permission the operation needs, when it needs one.
    pub permission: Option<String>,
    /// Refused because the entity is not there for this caller: missing, or outside its scope
    /// (the two answer alike, so the answer never reveals a row the caller cannot read).
    pub not_found: bool,
}

impl Decision {
    /// The decision as `{"allowed", "reason", "permission"}`, the `$allowed` answer.
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({"allowed": self.allowed, "reason": self.reason, "permission": self.permission})
    }
}

/// Whether `scope` may run `operation` on `table` (the whole set, or the entity `key`): the
/// decision `table_router` runs before serving the call, run without serving it.
///
/// # Errors
///
/// The source's failure while it looks the entity up.
pub async fn allowed<T: Table + ?Sized>(
    table: &T,
    scope: &Scope,
    operation: Operation,
    key: Option<&Value>,
) -> Result<Decision, TableError> {
    let info = table.info();
    let refuse = |reason: String, permission: Option<String>, not_found: bool| Decision {
        allowed: false,
        reason,
        permission,
        not_found,
    };
    let Some(rule) = info.rules.iter().find(|r| r.operation == operation) else {
        return Ok(refuse(
            format!("{} does not offer {}", info.set, operation.as_str()),
            None,
            false,
        ));
    };
    let permission = rule.permission.clone();
    if let Some(p) = &permission {
        if !scope.holds(p) {
            return Ok(refuse(
                format!("{} needs the permission {p}", operation.as_str()),
                permission,
                false,
            ));
        }
    }
    if operation.writes() {
        return Ok(refuse(
            format!("{} is served read only", info.set),
            permission,
            false,
        ));
    }
    if let Some(key) = key {
        let options = key_options(info, key, None)?;
        let found = query(table, &options, scope, 1).await?;
        if found.rows.is_empty() {
            return Ok(refuse(
                format!("{} has no such entity", info.set),
                permission,
                true,
            ));
        }
    }
    Ok(Decision {
        allowed: true,
        reason: "allowed".into(),
        permission,
        not_found: false,
    })
}
