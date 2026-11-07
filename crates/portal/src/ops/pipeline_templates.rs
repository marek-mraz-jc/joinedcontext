//! Pipeline templates of a project (API/01 §42, T-3976): a pipeline's spec saved without its
//! credentials and its references into the project, to start the next pipeline from. The
//! Portal's own record, not a kind in Git: a template runs and configures nothing.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::Row;
use tokio::sync::RwLock;
use utoipa::ToSchema;

use crate::activity::{chrono_to_odt, odt_to_chrono};
use crate::ops::data_views::new_id;

/// Templates one project may hold.
pub const MAX_PER_PROJECT: usize = 50;
/// The largest spec a template keeps, as JSON.
pub const MAX_SPEC_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PipelineTemplate {
    pub id: String,
    pub title: String,
    /// The pipeline's spec without `secretRefs`, source and target references or a space.
    #[schema(value_type = Object)]
    pub spec: Value,
    pub created_by: String,
    #[schema(value_type = String, format = DateTime)]
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateTemplate {
    /// What the gallery shows, 1 to 120 characters.
    pub title: String,
    /// A Pipeline manifest; only its `spec` is kept, without what belongs to the project.
    #[schema(value_type = Object)]
    pub manifest: Value,
}

#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("{0}")]
    Invalid(String),
    #[error("the template is larger than {MAX_SPEC_BYTES} bytes of JSON")]
    TooLarge,
    #[error("a project keeps at most {MAX_PER_PROJECT} pipeline templates; delete one first")]
    Limit,
    #[error("no such template in this project")]
    NotFound,
    #[error("database: {0}")]
    Db(String),
}

/// The spec a template keeps: the manifest's `spec` without the credentials and the references
/// that name the project's own resources (API/01 §42).
pub fn template_spec(manifest: &Value) -> Result<Value, TemplateError> {
    if manifest.get("kind").and_then(Value::as_str) != Some("Pipeline") {
        return Err(TemplateError::Invalid(
            "a template is saved from a Pipeline manifest".into(),
        ));
    }
    if let Some(key) = crate::api::mutate::find_literal_secret(manifest) {
        return Err(TemplateError::Invalid(format!(
            "the manifest holds a credential written out in `{key}`; reference it with a secretRef"
        )));
    }
    let Some(Value::Object(spec)) = manifest.get("spec") else {
        return Err(TemplateError::Invalid("the manifest has no spec".into()));
    };
    let mut spec: Map<String, Value> = spec.clone();
    spec.remove("secretRefs");
    spec.remove("contextSpaceRef");
    // The single-source and single-target forms of a spec carry the same references.
    spec.remove("dataSourceRef");
    spec.remove("endpointRef");
    spec.remove("targetEndpoint");
    // `source`/`output` is the one-object form of `sources`/`outputs` (T-3995: dev's seeded
    // pipelines kept their `source.dataSourceRef` in a template).
    for (field, refs) in [
        ("sources", &["dataSourceRef", "endpointRef"][..]),
        ("source", &["dataSourceRef", "endpointRef"][..]),
        ("outputs", &["targetEndpoint"][..]),
        ("output", &["targetEndpoint"][..]),
    ] {
        let items: Vec<&mut Value> = match spec.get_mut(field) {
            Some(Value::Array(items)) => items.iter_mut().collect(),
            Some(item) => vec![item],
            None => Vec::new(),
        };
        for item in items.into_iter().filter_map(Value::as_object_mut) {
            for key in refs {
                item.remove(*key);
            }
        }
    }
    let spec = Value::Object(spec);
    if serde_json::to_vec(&spec).map_or(0, |bytes| bytes.len()) > MAX_SPEC_BYTES {
        return Err(TemplateError::TooLarge);
    }
    Ok(spec)
}

enum Inner {
    Db(sqlx::PgPool),
    Memory(RwLock<BTreeMap<String, Vec<PipelineTemplate>>>),
}

#[derive(Clone)]
pub struct TemplateStore {
    inner: Arc<Inner>,
}

fn db(err: sqlx::Error) -> TemplateError {
    TemplateError::Db(err.to_string())
}

impl TemplateStore {
    pub fn new(pool: Option<sqlx::PgPool>) -> Self {
        let inner = match pool {
            Some(pool) => Inner::Db(pool),
            None => Inner::Memory(RwLock::new(BTreeMap::new())),
        };
        Self {
            inner: Arc::new(inner),
        }
    }

    /// The project's templates, newest first.
    pub async fn list(&self, project: &str) -> Result<Vec<PipelineTemplate>, TemplateError> {
        match &*self.inner {
            Inner::Memory(map) => {
                let mut items = map.read().await.get(project).cloned().unwrap_or_default();
                items.reverse();
                Ok(items)
            }
            Inner::Db(pool) => sqlx::query(
                "SELECT id, title, spec, created_by, created_at FROM pipeline_templates \
                 WHERE project = $1 ORDER BY seq DESC",
            )
            .bind(project)
            .fetch_all(pool)
            .await
            .map_err(db)?
            .into_iter()
            .map(|row| {
                Ok(PipelineTemplate {
                    id: row.get("id"),
                    title: row.get("title"),
                    spec: row.get("spec"),
                    created_by: row.get("created_by"),
                    created_at: odt_to_chrono(row.get("created_at")),
                })
            })
            .collect(),
        }
    }

    /// Saves a template made of `request`, refused past [`MAX_PER_PROJECT`]. In the database the
    /// count and the insert run under a transaction lock on the project, so two saves at once
    /// cannot both pass the count.
    pub async fn create(
        &self,
        project: &str,
        created_by: &str,
        request: CreateTemplate,
    ) -> Result<PipelineTemplate, TemplateError> {
        let title = request.title.trim();
        if title.is_empty() || title.chars().count() > 120 {
            return Err(TemplateError::Invalid(
                "a title is 1 to 120 characters".into(),
            ));
        }
        let template = PipelineTemplate {
            id: new_id(),
            title: title.to_owned(),
            spec: template_spec(&request.manifest)?,
            created_by: created_by.to_owned(),
            created_at: Utc::now(),
        };
        match &*self.inner {
            Inner::Memory(map) => {
                let mut map = map.write().await;
                let items = map.entry(project.to_owned()).or_default();
                if items.len() >= MAX_PER_PROJECT {
                    return Err(TemplateError::Limit);
                }
                items.push(template.clone());
            }
            Inner::Db(pool) => {
                let mut tx = pool.begin().await.map_err(db)?;
                sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('pipeline_templates/' || $1, 0))")
                    .bind(project)
                    .execute(&mut *tx)
                    .await
                    .map_err(db)?;
                let count: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM pipeline_templates WHERE project = $1",
                )
                .bind(project)
                .fetch_one(&mut *tx)
                .await
                .map_err(db)?;
                if count as usize >= MAX_PER_PROJECT {
                    return Err(TemplateError::Limit);
                }
                sqlx::query(
                    "INSERT INTO pipeline_templates (id, project, title, spec, created_by, created_at) \
                     VALUES ($1, $2, $3, $4, $5, $6)",
                )
                .bind(&template.id)
                .bind(project)
                .bind(&template.title)
                .bind(&template.spec)
                .bind(&template.created_by)
                .bind(chrono_to_odt(template.created_at))
                .execute(&mut *tx)
                .await
                .map_err(db)?;
                tx.commit().await.map_err(db)?;
            }
        }
        Ok(template)
    }

    pub async fn delete(&self, project: &str, id: &str) -> Result<(), TemplateError> {
        match &*self.inner {
            Inner::Memory(map) => {
                let mut map = map.write().await;
                let items = map.get_mut(project).ok_or(TemplateError::NotFound)?;
                let before = items.len();
                items.retain(|t| t.id != id);
                if items.len() == before {
                    return Err(TemplateError::NotFound);
                }
                Ok(())
            }
            Inner::Db(pool) => {
                let done =
                    sqlx::query("DELETE FROM pipeline_templates WHERE project = $1 AND id = $2")
                        .bind(project)
                        .bind(id)
                        .execute(pool)
                        .await
                        .map_err(db)?;
                if done.rows_affected() == 0 {
                    return Err(TemplateError::NotFound);
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_template_keeps_the_shape_and_drops_what_belongs_to_the_project() {
        let spec = template_spec(&json!({
            "kind": "Pipeline",
            "metadata": { "name": "air", "namespace": "helsinki" },
            "spec": {
                "class": "resident",
                "contextSpaceRef": "air",
                "secretRefs": [{ "name": "mqtt", "key": "password" }],
                "sources": [{ "dataSourceRef": { "kind": "DataSource", "name": "feed" }, "query": { "type": "X" } }],
                "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:air:all", "type": "X", "mode": "upsert" }],
                "steps": [{ "kind": "bloblang", "bloblang": "root = this" }]
            }
        }))
        .expect("a template");
        assert_eq!(
            spec,
            json!({
                "class": "resident",
                "sources": [{ "query": { "type": "X" } }],
                "outputs": [{ "type": "X", "mode": "upsert" }],
                "steps": [{ "kind": "bloblang", "bloblang": "root = this" }]
            })
        );
    }

    #[test]
    fn the_one_source_form_drops_its_references_too() {
        let spec = template_spec(&json!({
            "kind": "Pipeline",
            "spec": {
                "class": "auto",
                "schedule": "*/30 * * * *",
                "source": { "dataSourceRef": { "kind": "DataSource", "name": "fmi-air-quality" } },
                "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:air:all",
                "output": { "type": "AirQualityObserved", "mode": "upsert", "targetEndpoint": "x" }
            }
        }))
        .expect("a template");
        assert_eq!(
            spec,
            json!({
                "class": "auto",
                "schedule": "*/30 * * * *",
                "source": {},
                "output": { "type": "AirQualityObserved", "mode": "upsert" }
            })
        );
        let by_endpoint = template_spec(&json!({
            "kind": "Pipeline",
            "spec": { "source": { "endpointRef": "kpi", "query": { "type": "X" } } }
        }))
        .expect("a template");
        assert_eq!(
            by_endpoint,
            json!({ "source": { "query": { "type": "X" } } })
        );
    }

    #[test]
    fn a_credential_written_out_or_another_kind_is_refused() {
        let leak = json!({ "kind": "Pipeline", "spec": { "sources": [{ "query": { "token": "s3cr3t" } }] } });
        assert!(
            matches!(template_spec(&leak), Err(TemplateError::Invalid(m)) if m.contains("token"))
        );
        assert!(matches!(
            template_spec(&json!({ "kind": "Endpoint", "spec": {} })),
            Err(TemplateError::Invalid(_))
        ));
        assert!(matches!(
            template_spec(&json!({ "kind": "Pipeline" })),
            Err(TemplateError::Invalid(_))
        ));
        let big = json!({ "kind": "Pipeline", "spec": { "steps": [{ "bloblang": "x".repeat(MAX_SPEC_BYTES) }] } });
        assert!(matches!(template_spec(&big), Err(TemplateError::TooLarge)));
    }

    #[tokio::test]
    async fn a_project_keeps_its_own_templates_and_no_more_than_the_limit() {
        let store = TemplateStore::new(None);
        let make = |title: &str| CreateTemplate {
            title: title.into(),
            manifest: json!({ "kind": "Pipeline", "spec": { "class": "batch" } }),
        };
        for i in 0..MAX_PER_PROJECT {
            store
                .create("helsinki", "jana", make(&format!("t{i}")))
                .await
                .expect("saved");
        }
        assert!(matches!(
            store.create("helsinki", "jana", make("one more")).await,
            Err(TemplateError::Limit)
        ));
        assert!(store.list("espoo").await.expect("list").is_empty());
        let newest = store.list("helsinki").await.expect("list");
        assert_eq!(newest[0].title, format!("t{}", MAX_PER_PROJECT - 1));
        assert!(matches!(
            store.delete("espoo", &newest[0].id).await,
            Err(TemplateError::NotFound)
        ));
        store
            .delete("helsinki", &newest[0].id)
            .await
            .expect("deleted");
        assert!(matches!(
            store.create("helsinki", "jana", make("  ")).await,
            Err(TemplateError::Invalid(_))
        ));
    }
}
