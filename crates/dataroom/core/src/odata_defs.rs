#![forbid(unsafe_code)]
//! Shared OData entity-set definitions (`odata-table`, C14).
//!
//! Declares the 8 entity sets (`Catalogues`, `Datasets`, `Distributions`, `Publishers`,
//! `HarvestSources`, `HarvestRuns`, `ActivityRows`, `ShaclResults`) once and derives CSDL,
//! OpenAPI 3.1, `$export` and `$allowed` from them.

use crate::auth::{Action, Authorizer, ObjectTarget, RequestCtx};
use crate::model::{
    text_in_lang, ActivityRow, Catalogue, Dataset, Distribution, HarvestRun, HarvestSource,
    Publisher, ShaclValidationReport, Visibility,
};
use odata_table::{EntityDef, Field, FieldKind, Operation, Rule, Scope, Service};
use serde_json::{json, Value};

fn req_field(name: &str, kind: FieldKind, description: &str) -> Field {
    Field {
        name: name.to_owned(),
        kind,
        description: description.to_owned(),
        nullable: false,
    }
}

fn opt_field(name: &str, kind: FieldKind, description: &str) -> Field {
    Field::new(name, kind, description)
}

fn set_rules(write_perm: &str) -> Vec<Rule> {
    vec![
        Rule {
            operation: Operation::Read,
            permission: None,
        },
        Rule {
            operation: Operation::Export,
            permission: None,
        },
        Rule {
            operation: Operation::Create,
            permission: Some(write_perm.to_owned()),
        },
        Rule {
            operation: Operation::Update,
            permission: Some(write_perm.to_owned()),
        },
        Rule {
            operation: Operation::Delete,
            permission: Some(write_perm.to_owned()),
        },
    ]
}

/// Builds the complete `odata-table::Service` for dataroom (C14).
#[must_use]
pub fn dataroom_service() -> Service {
    Service {
        namespace: "JoinedContext.Dataroom".into(),
        title: "dataroom OData & REST Catalogue API".into(),
        sets: vec![
            EntityDef {
                set: "Catalogues".into(),
                type_name: "Catalogue".into(),
                description: "Dataset catalogues hosted by dataroom.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Catalogue UUID."),
                    req_field("org_id", FieldKind::String, "Owning organization UUID."),
                    req_field("name", FieldKind::String, "Catalogue slug."),
                    req_field("host", FieldKind::String, "Catalogue public host name."),
                    req_field("title", FieldKind::String, "Default-language title."),
                    opt_field(
                        "description",
                        FieldKind::String,
                        "Default-language description.",
                    ),
                    req_field(
                        "default_language",
                        FieldKind::String,
                        "Default ISO 639-1 language.",
                    ),
                    req_field(
                        "main_profile",
                        FieldKind::String,
                        "Main DCAT-AP profile ID.",
                    ),
                    req_field(
                        "modified",
                        FieldKind::DateTime,
                        "Last modification timestamp.",
                    ),
                ],
                rules: set_rules("propose:Catalogue"),
            },
            EntityDef {
                set: "Datasets".into(),
                type_name: "Dataset".into(),
                description: "Datasets and dataset series in the catalogue.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Dataset ID."),
                    req_field("org_id", FieldKind::String, "Organization UUID."),
                    req_field("catalogue_id", FieldKind::String, "Catalogue UUID."),
                    req_field("name", FieldKind::String, "Dataset URL slug."),
                    req_field("title", FieldKind::String, "Dataset title."),
                    opt_field("notes", FieldKind::String, "Dataset description."),
                    req_field("publisher_id", FieldKind::String, "Publisher slug or ID."),
                    req_field(
                        "visibility",
                        FieldKind::String,
                        "Visibility: public, internal, restricted.",
                    ),
                    req_field(
                        "state",
                        FieldKind::String,
                        "State: draft, active, withdrawn.",
                    ),
                    opt_field("licence_id", FieldKind::String, "Licence identifier."),
                    opt_field("frequency", FieldKind::String, "Update frequency code."),
                    opt_field(
                        "endpoint_url",
                        FieldKind::String,
                        "Linked NGSI-LD Endpoint URL.",
                    ),
                    req_field("issued", FieldKind::DateTime, "Release timestamp."),
                    req_field(
                        "modified",
                        FieldKind::DateTime,
                        "Last modification timestamp.",
                    ),
                ],
                rules: set_rules("propose:Dataset"),
            },
            EntityDef {
                set: "Distributions".into(),
                type_name: "Distribution".into(),
                description: "Distributions (files and APIs) of datasets.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Distribution ID."),
                    req_field("org_id", FieldKind::String, "Organization UUID."),
                    req_field("catalogue_id", FieldKind::String, "Catalogue UUID."),
                    req_field("dataset_id", FieldKind::String, "Parent dataset ID."),
                    req_field("name", FieldKind::String, "Distribution title."),
                    req_field("access_url", FieldKind::String, "Access URL."),
                    opt_field("download_url", FieldKind::String, "Direct download URL."),
                    opt_field("format", FieldKind::String, "File or API format code."),
                    opt_field("media_type", FieldKind::String, "IANA media type."),
                    opt_field("byte_size", FieldKind::Integer, "Size in bytes."),
                ],
                rules: set_rules("propose:Dataset"),
            },
            EntityDef {
                set: "Publishers".into(),
                type_name: "Publisher".into(),
                description: "Publishing organizations and bodies in the catalogue.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Publisher ID."),
                    req_field("org_id", FieldKind::String, "Organization UUID."),
                    req_field("catalogue_id", FieldKind::String, "Catalogue UUID."),
                    req_field("name", FieldKind::String, "Publisher slug."),
                    req_field("title", FieldKind::String, "Publisher display title."),
                    req_field("uri", FieldKind::String, "Publisher registry IRI."),
                    req_field("state", FieldKind::String, "State: active, withdrawn."),
                    opt_field(
                        "merged_into",
                        FieldKind::String,
                        "Target publisher ID if merged.",
                    ),
                    req_field(
                        "modified",
                        FieldKind::DateTime,
                        "Last modification timestamp.",
                    ),
                ],
                rules: set_rules("propose:Publisher"),
            },
            EntityDef {
                set: "HarvestSources".into(),
                type_name: "HarvestSource".into(),
                description: "Configured external harvest sources.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Harvest source ID."),
                    req_field("org_id", FieldKind::String, "Organization UUID."),
                    req_field("catalogue_id", FieldKind::String, "Catalogue UUID."),
                    req_field("name", FieldKind::String, "Harvest source slug."),
                    req_field("kind", FieldKind::String, "Source kind."),
                    req_field("url", FieldKind::String, "Source endpoint URL."),
                    req_field("publisher_id", FieldKind::String, "Target publisher ID."),
                    req_field(
                        "enabled",
                        FieldKind::Boolean,
                        "Whether scheduled runs are enabled.",
                    ),
                    req_field(
                        "modified",
                        FieldKind::DateTime,
                        "Last modification timestamp.",
                    ),
                ],
                rules: set_rules("propose:HarvestSource"),
            },
            EntityDef {
                set: "HarvestRuns".into(),
                type_name: "HarvestRun".into(),
                description: "Execution records of harvest sources.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Harvest run ID."),
                    req_field("org_id", FieldKind::String, "Organization UUID."),
                    req_field("catalogue_id", FieldKind::String, "Catalogue UUID."),
                    req_field("source_id", FieldKind::String, "Harvest source ID."),
                    req_field("state", FieldKind::String, "Run state."),
                    req_field("started_at", FieldKind::DateTime, "Run start timestamp."),
                    opt_field(
                        "finished_at",
                        FieldKind::DateTime,
                        "Run completion timestamp.",
                    ),
                    req_field("created_count", FieldKind::Integer, "Datasets created."),
                    req_field("updated_count", FieldKind::Integer, "Datasets updated."),
                    req_field("unchanged_count", FieldKind::Integer, "Datasets unchanged."),
                    req_field("withdrawn_count", FieldKind::Integer, "Datasets withdrawn."),
                ],
                rules: set_rules("harvest:HarvestSource"),
            },
            EntityDef {
                set: "ActivityRows".into(),
                type_name: "ActivityRow".into(),
                description: "Audit activity log rows for the catalogue.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Activity row ID."),
                    req_field("org_id", FieldKind::String, "Organization UUID."),
                    req_field("catalogue_id", FieldKind::String, "Catalogue UUID."),
                    req_field("at", FieldKind::DateTime, "Timestamp."),
                    req_field("actor", FieldKind::String, "Actor identifier."),
                    req_field("operation", FieldKind::String, "Operation name."),
                    req_field("object_type", FieldKind::String, "Target object type."),
                    req_field("object_id", FieldKind::String, "Target object ID."),
                    req_field(
                        "summary",
                        FieldKind::String,
                        "Human-readable change summary.",
                    ),
                ],
                rules: odata_table::open_rules(),
            },
            EntityDef {
                set: "ShaclResults".into(),
                type_name: "ShaclResult".into(),
                description: "Per-dataset, per-profile SHACL validation results.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Report ID."),
                    req_field("org_id", FieldKind::String, "Organization UUID."),
                    req_field("catalogue_id", FieldKind::String, "Catalogue UUID."),
                    req_field("dataset_id", FieldKind::String, "Dataset ID."),
                    req_field("profile_id", FieldKind::String, "Profile ID."),
                    req_field("profile_version", FieldKind::String, "Profile version."),
                    req_field(
                        "conforms",
                        FieldKind::Boolean,
                        "Whether the dataset conforms.",
                    ),
                    req_field(
                        "violation_count",
                        FieldKind::Integer,
                        "Number of violations.",
                    ),
                    req_field("warning_count", FieldKind::Integer, "Number of warnings."),
                    req_field("checked_at", FieldKind::DateTime, "Validation timestamp."),
                ],
                rules: odata_table::open_rules(),
            },
        ],
    }
}

/// Converts a `Catalogue` into an OData row `Value`.
pub fn catalogue_to_row(c: &Catalogue) -> Value {
    json!({
        "id": c.id.to_string(),
        "org_id": c.org_id.to_string(),
        "name": c.name,
        "host": c.host,
        "title": text_in_lang(&c.title, Some(&c.default_language)).unwrap_or(&c.name),
        "description": text_in_lang(&c.description, Some(&c.default_language)),
        "default_language": c.default_language,
        "main_profile": c.main_profile,
        "modified": c.modified.to_rfc3339(),
    })
}

/// Converts a `Dataset` into an OData row `Value`.
pub fn dataset_to_row(d: &Dataset, default_lang: Option<&str>) -> Value {
    json!({
        "id": d.id,
        "org_id": d.org_id.to_string(),
        "catalogue_id": d.catalogue_id.to_string(),
        "name": d.name,
        "title": text_in_lang(&d.title, default_lang).unwrap_or(&d.name),
        "notes": text_in_lang(&d.description, default_lang),
        "publisher_id": d.publisher_id,
        "visibility": d.visibility.as_str(),
        "state": d.state.as_str(),
        "licence_id": d.licence_id,
        "frequency": d.frequency,
        "endpoint_url": d.endpoint_url,
        "issued": d.issued.to_rfc3339(),
        "modified": d.modified.to_rfc3339(),
    })
}

/// Converts a `Distribution` into an OData row `Value`.
pub fn distribution_to_row(
    org_id: crate::model::OrgId,
    catalogue_id: crate::model::CatalogueId,
    d: &Distribution,
    default_lang: Option<&str>,
) -> Value {
    json!({
        "id": d.id,
        "org_id": org_id.to_string(),
        "catalogue_id": catalogue_id.to_string(),
        "dataset_id": d.dataset_id,
        "name": text_in_lang(&d.title, default_lang).unwrap_or(&d.access_url),
        "access_url": d.access_url,
        "download_url": d.download_url,
        "format": d.format,
        "media_type": d.media_type,
        "byte_size": d.byte_size.and_then(|b| i64::try_from(b).ok()),
    })
}

/// Converts a `Publisher` into an OData row `Value`.
pub fn publisher_to_row(p: &Publisher, default_lang: Option<&str>) -> Value {
    json!({
        "id": p.id,
        "org_id": p.org_id.to_string(),
        "catalogue_id": p.catalogue_id.to_string(),
        "name": p.name,
        "title": text_in_lang(&p.title, default_lang).unwrap_or(&p.name),
        "uri": p.uri,
        "state": p.state.as_str(),
        "merged_into": p.merged_into,
        "modified": p.modified.to_rfc3339(),
    })
}

/// Converts a `HarvestSource` into an OData row `Value`.
pub fn harvest_source_to_row(s: &HarvestSource) -> Value {
    json!({
        "id": s.id,
        "org_id": s.org_id.to_string(),
        "catalogue_id": s.catalogue_id.to_string(),
        "name": s.name,
        "kind": s.kind.as_str(),
        "url": s.url,
        "publisher_id": s.publisher_id,
        "enabled": s.enabled,
        "modified": s.modified.to_rfc3339(),
    })
}

/// Converts a `HarvestRun` into an OData row `Value`.
pub fn harvest_run_to_row(r: &HarvestRun) -> Value {
    json!({
        "id": r.id,
        "org_id": r.org_id.to_string(),
        "catalogue_id": r.catalogue_id.to_string(),
        "source_id": r.source_id,
        "state": r.state.as_str(),
        "started_at": r.started_at.to_rfc3339(),
        "finished_at": r.finished_at.map(|t| t.to_rfc3339()),
        "created_count": r.created_count as i64,
        "updated_count": r.updated_count as i64,
        "unchanged_count": r.unchanged_count as i64,
        "withdrawn_count": r.withdrawn_count as i64,
    })
}

/// Converts an `ActivityRow` into an OData row `Value`.
pub fn activity_to_row(a: &ActivityRow) -> Value {
    json!({
        "id": a.id,
        "org_id": a.org_id.to_string(),
        "catalogue_id": a.catalogue_id.to_string(),
        "at": a.at.to_rfc3339(),
        "actor": a.actor,
        "operation": a.operation,
        "object_type": a.object_type,
        "object_id": a.object_id,
        "summary": a.summary,
    })
}

/// Converts a `ShaclValidationReport` into an OData row `Value`.
pub fn shacl_report_to_row(r: &ShaclValidationReport) -> Value {
    json!({
        "id": r.id,
        "org_id": r.org_id.to_string(),
        "catalogue_id": r.catalogue_id.to_string(),
        "dataset_id": r.dataset_id,
        "profile_id": r.profile_id,
        "profile_version": r.profile_version,
        "conforms": r.conforms,
        "violation_count": r.violation_count as i64,
        "warning_count": r.warning_count as i64,
        "checked_at": r.checked_at.to_rfc3339(),
    })
}

/// Builds the `odata_table::Scope` for `ctx` and `set_name`, enforcing tenant isolation
/// (`org_id`, `catalogue_id`), dataset visibility, and held permissions before `$filter`.
pub fn scope_for_context(
    ctx: &RequestCtx,
    authorizer: &dyn Authorizer,
    set_name: &str,
    visible_dataset_ids: Option<Vec<String>>,
) -> Scope {
    let mut perms = Vec::new();
    let cat_target = ObjectTarget::Catalogue {
        org_id: ctx.org_id,
        catalogue_id: ctx.catalogue_id,
    };
    if authorizer.check(ctx, Action::Propose, &cat_target).allowed {
        perms.push("propose:Catalogue");
        perms.push("propose:Publisher");
        perms.push("propose:Dataset");
    }
    let hs_target = ObjectTarget::HarvestSource {
        org_id: ctx.org_id,
        catalogue_id: ctx.catalogue_id,
        source_id: "*".into(),
    };
    if authorizer.check(ctx, Action::Propose, &hs_target).allowed {
        perms.push("propose:HarvestSource");
    }
    if authorizer.check(ctx, Action::Harvest, &hs_target).allowed {
        perms.push("harvest:HarvestSource");
    }

    let mut scope = Scope::default()
        .holding(&perms)
        .and(&["org_id"], vec![Value::String(ctx.org_id.to_string())]);

    if set_name == "Catalogues" {
        scope = scope.and(&["id"], vec![Value::String(ctx.catalogue_id.to_string())]);
    } else {
        scope = scope.and(
            &["catalogue_id"],
            vec![Value::String(ctx.catalogue_id.to_string())],
        );
    }

    if set_name == "Datasets" {
        if let Some(ids) = visible_dataset_ids {
            scope = scope.and(&["id"], ids.into_iter().map(Value::String).collect());
        } else if !ctx.subject.is_member_of(ctx.org_id) {
            scope = scope
                .and(
                    &["visibility"],
                    vec![Value::String(Visibility::Public.as_str().into())],
                )
                .and(&["state"], vec![Value::String("active".into())]);
        }
    }

    if matches!(set_name, "HarvestSources" | "HarvestRuns") && !ctx.subject.is_member_of(ctx.org_id)
    {
        scope = scope.and(&["id"], Vec::new());
    }

    scope
}
