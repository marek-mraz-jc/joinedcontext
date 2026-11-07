//! What a pipeline may write: a record checked against its target space's one model (PL-59,
//! PL-60, ADR-N-034).
//!
//! Model Tools renders one LinkML source into a JSON Schema of each class in key-value form and
//! into SHACL shapes (DM-43). A pipeline writes normalized NGSI-LD, so this module compiles the
//! key-value schema of each class into a schema of the normalized entity: `id` and `type`
//! required, `type` the class name, each attribute an object of the NGSI-LD kind the slot
//! declares with its value checked against the slot's schema, and no other attribute unless the
//! model is open. Those are the constraints the model's SHACL shapes carry, so a refusal names
//! the SHACL component it broke and the path it broke it at.
//!
//! The compilation and the check live in `jcctl::model_schema`, which the gateway checks every
//! write through an Endpoint with too (T-4011, T-4014, DM-86).
//!
//! The same compiled schema is checked here (the workbench's validation step, the rejected
//! list's reason) and rendered into the runner's stream (the stage before the write), so a
//! person and the runner reach the same verdict on the same record.

pub use jcctl::model_schema::{
    Attribute, ClassAttributes, ModelSchema, Problem, RelationshipTarget, Unit,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};

/// The labels the stage's processors carry: their error counter is the pipeline's rejected
/// count, not its errors (PL-61).
pub const STAGE_LABELS: [&str; 4] = [
    "validation",
    "validation_id",
    "validation_quality",
    "rejected",
];

/// The compiled model of every space that names one, by `(project, space)`: what the reconciler
/// renders the stage from and what the rejected route names a record's rule by. Replaced whole on
/// every sync, so both read the version the repository pins now (PL-60).
#[derive(Debug, Default)]
pub struct ModelSchemas {
    by_space: RwLock<HashMap<(String, String), Arc<ModelSchema>>>,
}

impl ModelSchemas {
    /// The compiled model of one space, when the space names one.
    pub fn get(&self, project: &str, space: &str) -> Option<Arc<ModelSchema>> {
        self.by_space
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(project.to_owned(), space.to_owned()))
            .cloned()
    }

    /// Every `(project, space)` that names a model, in order: what the data-quality run reads
    /// (DM-74).
    pub fn spaces(&self) -> Vec<(String, String)> {
        let mut spaces: Vec<_> = self
            .by_space
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect();
        spaces.sort();
        spaces
    }

    /// Swaps in what one sync compiled.
    pub fn replace(&self, compiled: HashMap<(String, String), Arc<ModelSchema>>) {
        *self.by_space.write().unwrap_or_else(|e| e.into_inner()) = compiled;
    }
}

/// Compiles the model every space names (`spec.dataModelRef`, DM-61) from a staged repository:
/// the model's manifest in the same project and space, and its JSON Schema artifact read from
/// beside it. A draft model has no artifact yet; its classes are then held to their id and type.
/// A space that names no model, or a model that is not there, compiles to nothing and its
/// pipelines are not narrowed (`jcctl validate` refuses the second before a commit).
pub fn load(
    repo: &jcctl::loader::Repository,
    root: &Path,
) -> HashMap<(String, String), Arc<ModelSchema>> {
    use jc_core::kinds::{ContextSpaceSpec, DataModelSpec};
    let mut compiled = HashMap::new();
    for (id, resource) in repo.iter() {
        if id.kind != "ContextSpace" {
            continue;
        }
        let Some(space) =
            serde_json::from_value::<ContextSpaceSpec>(resource.manifest.spec.clone()).ok()
        else {
            continue;
        };
        let Some(named) = space.data_model_ref else {
            continue;
        };
        let project = id.namespace.clone().unwrap_or_default();
        let model = repo.iter().find_map(|(model, found)| {
            let spec = serde_json::from_value::<DataModelSpec>(found.manifest.spec.clone()).ok()?;
            (model.kind == "DataModel"
                && model.namespace.as_deref() == Some(project.as_str())
                && model.name == named.name()
                && spec.context_space_ref.as_deref() == Some(id.name.as_str()))
            .then_some((found.path.clone(), spec))
        });
        let Some((manifest, spec)) = model else {
            tracing::warn!(project = %project, space = %id.name, model = %named.name(), "the space names a model it does not hold; its pipelines are not validated");
            continue;
        };
        let json_schema = spec
            .artifacts
            .json_schema
            .as_deref()
            .and_then(|relative| beside(root, &manifest, relative))
            .unwrap_or(Value::Null);
        let mut schema = ModelSchema::compile_for_space(
            named.name(),
            spec.version.as_str(),
            &json_schema,
            &spec.classes,
            spec.open_world,
            space.missing_unit_code,
        );
        schema.quality = space.quality;
        compiled.insert((project, id.name.clone()), Arc::new(schema));
    }
    compiled
}

/// What a pipeline's spec alone says against the models of the spaces it writes (T-3223): every
/// output whose entity type is not a class of the model its Endpoint's space pins, named at that
/// output, before any sample is run. An output that names no type, an Endpoint not in the mirror
/// and a space with no compiled model say nothing here; the runner's stage and the gateway still
/// hold those (PL-59, DM-61).
pub fn output_type_problems(
    schemas: &ModelSchemas,
    mirror: &crate::store::Mirror,
    project: &str,
    spec: &jc_core::kinds::PipelineSpec,
) -> Vec<Problem> {
    let second_shape = !spec.outputs.is_empty();
    spec.outputs()
        .into_iter()
        .enumerate()
        .filter_map(|(index, output)| {
            let entity_type = output.entity_type?;
            let endpoint = mirror.get(project, "Endpoint", output.target_endpoint.local_id())?;
            let space = crate::api::assistant::ref_name(endpoint.spec.get("contextSpaceRef")?)?;
            let model = schemas.get(project, &space)?;
            if model.classes.contains_key(&entity_type) {
                return None;
            }
            let classes: Vec<&str> = model.classes.keys().map(String::as_str).collect();
            Some(Problem {
                rule: "type".to_owned(),
                path: if second_shape {
                    format!("spec.outputs[{index}].type")
                } else {
                    "spec.output.type".to_owned()
                },
                message: format!(
                    "{entity_type} is not a class of the data model {} of space {space}; its classes are {} (DM-61)",
                    model.model,
                    classes.join(", ")
                ),
            })
        })
        .collect()
}

/// One JSON artifact beside its manifest, never outside the staged tree.
pub(crate) fn beside(root: &Path, manifest: &Path, relative: &str) -> Option<Value> {
    if relative.starts_with('/') || relative.split('/').any(|part| part == "..") {
        return None;
    }
    let path = root.join(manifest).parent()?.join(relative);
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Puts the stage and the rejected sink into a rendered stream, right before the split into
/// gateway batches, so each record is checked alone and a refused one never reaches a batch
/// (PL-60, PL-61). Answers whether the stream had the place to put them.
pub fn insert_stage(stream: &mut Value, schema: &ModelSchema, sink_url: &str) -> bool {
    let Some(processors) = stream
        .pointer_mut("/pipeline/processors")
        .and_then(Value::as_array_mut)
    else {
        return false;
    };
    let Some(split) = processors.iter().rposition(|p| p.get("split").is_some()) else {
        return false;
    };
    let mut checks = stage(schema);
    checks.push(rejected_sink(sink_url));
    // The quality mapping is there only when a rule rejects on write (DM-88).
    let whole = checks.len() == STAGE_LABELS.len();
    let labels = STAGE_LABELS
        .iter()
        .filter(|label| whole || **label != "validation_quality");
    for (processor, label) in checks.iter_mut().zip(labels) {
        if let Some(fields) = processor.as_object_mut() {
            fields.insert("label".into(), json!(label));
        }
    }
    processors.splice(split..split, checks);
    true
}

/// The runner's stage for this model (PL-60): Bento processors that fail a record whose type
/// is not a class, whose id breaks PF-42, or that breaks its class's schema. A failed record
/// is caught by the processors [`rejected_sink`] renders and is never written.
pub fn stage(schema: &ModelSchema) -> Vec<Value> {
    let mut cases: Vec<Value> = schema
        .classes
        .iter()
        .map(|(class, schema)| {
            json!({
                "check": format!("this.type == {}", bloblang_string(class)),
                "processors": [{ "json_schema": { "schema": schema.to_string() } }]
            })
        })
        .collect();
    cases.push(json!({
        "processors": [{ "mapping": format!(
            "root = throw(\"the type is not a class of the space's data model {} (DM-61)\")",
            schema.model
        )}]
    }));
    let mut stage = vec![
        json!({ "switch": cases }),
        json!({ "mapping": concat!(
            "root = this\n",
            "let prefix = \"urn:ngsi-ld:\" + this.type.string() + \":\"\n",
            "if !this.id.or(\"\").string().has_prefix($prefix) || this.id.or(\"\").string().length() <= $prefix.length() {\n",
            "  root = throw(\"the id is not an NGSI-LD URN of its type, urn:ngsi-ld:{type}:{id} (PF-43)\")\n",
            "}"
        )}),
    ];
    stage.extend(quality_mapping(schema));
    stage
}

/// The stage's mapping for the space's rejecting rules (DM-88): it fails a record that
/// breaks one, with the rule's name, so the rejected sink lists it. `None` when no rule
/// rejects on write. `ModelSchema::check` stays the verdict the rejected route records.
fn quality_mapping(schema: &ModelSchema) -> Option<Value> {
    use jc_core::kinds::{duration_seconds, QualityKind, QualityOutcome};
    let tests: Vec<String> = schema
        .quality
        .iter()
        .filter(|rule| rule.write_outcome() == QualityOutcome::Reject)
        .filter_map(|rule| {
            let instance = format!("this.get({}).or(null)", bloblang_string(&rule.attribute));
            let value = format!(
                "{instance}.(a -> if a.type() == \"object\" {{ a.value.or(null) }} else {{ a }})"
            );
            let broken = match rule.kind {
                QualityKind::Range => {
                    let mut out = Vec::new();
                    if let Some(min) = &rule.min {
                        out.push(format!("$v < {min}"));
                    }
                    if let Some(max) = &rule.max {
                        out.push(format!("$v > {max}"));
                    }
                    format!(
                        "let v = {value}\nlet broken = $v.type() == \"number\" && ({})",
                        out.join(" || ")
                    )
                }
                QualityKind::Area => {
                    let [w, s, e, n] = rule.bbox.as_ref()?;
                    format!(
                        "let g = {value}\n\
                         let c = $g.coordinates.or([])\n\
                         let points = match $g.type.or(\"\") {{\n  \"Point\" => [$c]\n  \"MultiPoint\" => $c\n  \"LineString\" => $c\n  \"MultiLineString\" => $c.flatten()\n  \"Polygon\" => $c.flatten()\n  \"MultiPolygon\" => $c.flatten().flatten()\n  _ => []\n}}\n\
                         let broken = $points.any(p -> p.index(0).number().catch(0) < {w} || p.index(0).number().catch(0) > {e} || p.index(1).number().catch(0) < {s} || p.index(1).number().catch(0) > {n})"
                    )
                }
                QualityKind::Freshness => {
                    let ahead = duration_seconds(rule.max_ahead.as_deref()?)?;
                    format!(
                        "let at = {instance}.(a -> if a.type() == \"object\" {{ a.observedAt.or(null) }} else {{ null }})\n\
                         let broken = $at.type() == \"string\" && $at.ts_unix().catch(0) > now().ts_unix() + {ahead}"
                    )
                }
            };
            Some(format!(
                "if this.type == {} {{\n{broken}\nif $broken {{ root = throw({}) }}\n}}",
                bloblang_string(&rule.entity_type),
                bloblang_string(&format!(
                    "{} breaks the space's quality rule {} (DM-88)",
                    rule.attribute, rule.name
                ))
            ))
        })
        .collect();
    (!tests.is_empty()).then(|| json!({ "mapping": format!("root = this\n{}", tests.join("\n")) }))
}

/// What catches a record the stage refused (PL-61): it is posted once, with the reason, to the
/// Portal's internal rejected route for this pipeline and then dropped, so it is never written.
/// A Portal that does not answer loses the rejection and never holds back the stream.
pub fn rejected_sink(url: &str) -> Value {
    json!({ "catch": [
        { "mapping": format!(
            "root = {{ \"record\": this, \"error\": error(), \"step\": meta(\"jc_step\").or(null), \"run\": {} }}",
            crate::pipeline_log::RUN
        ) },
        { "http": {
            "url": url,
            "verb": "POST",
            "headers": { "Content-Type": "application/json" },
            "timeout": "5s",
            "retries": 0,
            "oauth2": {
                "enabled": true,
                "client_key": "${JC_CLIENT_ID}",
                "client_secret": "${JC_CLIENT_SECRET}",
                "token_url": "${JC_TOKEN_URL}",
            },
        }},
        { "mapping": "root = deleted()" }
    ]})
}

fn bloblang_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Value {
        json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "definitions": {
                "Window": { "enum": ["day", "month"], "type": "string" },
                "AirQualityObserved": {
                    "additionalProperties": false,
                    "properties": {
                        "id": { "type": "string" },
                        "dateObserved": { "type": "string", "format": "date-time", "x-ngsi-ld-kind": "Property" },
                        "pm10": { "type": ["number", "null"], "minimum": 0, "x-ngsi-ld-kind": "Property" },
                        "window": { "$ref": "#/definitions/Window", "x-ngsi-ld-kind": "Property" },
                        "name": { "x-ngsi-ld-kind": "LanguageProperty" },
                        "refDevice": { "type": "string", "x-ngsi-ld-kind": "Relationship" },
                        "location": { "type": ["object", "null"], "required": ["type", "coordinates"], "x-ngsi-ld-kind": "GeoProperty" }
                    },
                    "required": ["id", "dateObserved"]
                }
            }
        })
    }

    fn model(open: bool) -> ModelSchema {
        ModelSchema::compile(
            "bb-air-quality",
            "1.0.0",
            &schema(),
            &["AirQualityObserved".into()],
            open,
        )
    }

    fn valid() -> Value {
        json!({
            "id": "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:station-1",
            "type": "AirQualityObserved",
            "dateObserved": { "type": "Property", "value": "2026-09-01T06:00:00Z" },
            "pm10": { "type": "Property", "value": 18.4, "unitCode": "GQ" },
            "window": { "type": "Property", "value": "day" },
            "name": { "type": "LanguageProperty", "languageMap": { "sk": "Stanica 1" } },
            "refDevice": { "type": "Relationship", "object": "urn:ngsi-ld:Device:banskabystrica.sk:ovzdusie:d-1" },
            "location": { "type": "GeoProperty", "value": { "type": "Point", "coordinates": [19.1, 48.7] } }
        })
    }

    /// A model with every shape an attribute can take: a required quantity with its unit, a coded
    /// slot by `$ref`, a relationship with its target, a language map and a geometry.
    fn attributed() -> ModelSchema {
        ModelSchema::compile(
            "stations",
            "2.1.0",
            &json!({ "definitions": {
                "Status": { "enum": ["working", "closed"], "type": "string" },
                "Station": {
                    "description": "One docking station.",
                    "required": ["id", "type", "bikes"],
                    "properties": {
                        "id": { "type": "string" },
                        "type": { "type": "string" },
                        "bikes": { "type": ["integer", "null"], "minimum": 0, "description": "Free bikes.",
                                   "x-ngsi-ld-kind": "Property",
                                   "x-unit": { "exactMappings": ["ucefact:C62"], "ucumCode": "1" } },
                        "status": { "$ref": "#/definitions/Status", "x-ngsi-ld-kind": "Property" },
                        "refArea": { "type": ["string", "null"], "x-ngsi-ld-kind": "Relationship",
                                     "x-ngsi-ld-relationship": { "target": "AdministrativeArea" } },
                        "name": { "type": ["object", "null"], "x-ngsi-ld-kind": "LanguageProperty" },
                        "location": { "type": ["object", "null"], "x-ngsi-ld-kind": "GeoProperty" },
                        "seen": { "type": ["string", "null"], "format": "date-time", "x-ngsi-ld-kind": "Property" }
                    }
                }
            }}),
            &["Station".into(), "Draft".into()],
            false,
        )
    }

    /// T-3223: an output whose type the target space's model lacks is named before any sample
    /// runs, at that output, with the classes the model does have.
    #[test]
    fn an_output_type_the_target_spaces_model_lacks_is_named_at_that_output() {
        use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
        let mirror = crate::store::Mirror::new();
        for (name, space) in [("bikes-in", "bikes"), ("plain-in", "plain")] {
            mirror.upsert(ResourceEnvelope {
                api_version: API_VERSION.to_owned(),
                kind: "Endpoint".to_owned(),
                metadata: ObjectMeta {
                    name: name.to_owned(),
                    namespace: Some("helsinki".to_owned()),
                    ..Default::default()
                },
                spec: json!({ "contextSpaceRef": space, "slug": format!("{name}slug") }),
                status: None,
            });
        }
        let schemas = ModelSchemas::default();
        schemas.replace(HashMap::from([(
            ("helsinki".to_owned(), "bikes".to_owned()),
            Arc::new(attributed()),
        )]));
        let spec = |value: Value| -> jc_core::kinds::PipelineSpec {
            serde_json::from_value(value).expect("a pipeline spec")
        };
        let first = |entity_type: &str, endpoint: &str| {
            spec(json!({
                "class": "resident",
                "source": { "dataSourceRef": { "kind": "DataSource", "name": "feed" } },
                "compute": { "kind": "bloblang", "bloblang": "root = this" },
                "targetEndpoint": format!("urn:ngsi-ld:Endpoint:hel.fi:helsinki:{endpoint}"),
                "output": { "type": entity_type, "mode": "upsert" }
            }))
        };
        let problems =
            output_type_problems(&schemas, &mirror, "helsinki", &first("Bus", "bikes-in"));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(
            (problems[0].rule.as_str(), problems[0].path.as_str()),
            ("type", "spec.output.type")
        );
        assert!(
            problems[0]
                .message
                .contains("Bus is not a class of the data model stations of space bikes"),
            "{}",
            problems[0].message
        );
        assert!(
            problems[0].message.contains("Draft, Station"),
            "{}",
            problems[0].message
        );
        // A class of the model, a space with no compiled model and an Endpoint nobody holds say
        // nothing; the runner's stage and the gateway still hold those.
        assert!(
            output_type_problems(&schemas, &mirror, "helsinki", &first("Station", "bikes-in"))
                .is_empty()
        );
        assert!(
            output_type_problems(&schemas, &mirror, "helsinki", &first("Bus", "plain-in"))
                .is_empty()
        );
        assert!(
            output_type_problems(&schemas, &mirror, "helsinki", &first("Bus", "gone")).is_empty()
        );
        // In the second shape the problem names the output it is about.
        let second = spec(json!({
            "class": "resident",
            "sources": [{ "dataSourceRef": { "kind": "DataSource", "name": "feed" } }],
            "steps": [{ "kind": "bloblang", "bloblang": "root = this" }],
            "outputs": [
                { "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:bikes-in", "type": "Station" },
                { "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:bikes-in", "type": "Tram" }
            ]
        }));
        let problems = output_type_problems(&schemas, &mirror, "helsinki", &second);
        assert_eq!(
            problems.iter().map(|p| p.path.as_str()).collect::<Vec<_>>(),
            ["spec.outputs[1].type"]
        );
    }

    #[test]
    fn the_stage_switches_on_each_class_and_refuses_the_rest() {
        let stage = stage(&model(false));
        let cases = stage[0]["switch"].as_array().expect("a switch");
        assert_eq!(cases.len(), 2);
        assert_eq!(cases[0]["check"], "this.type == \"AirQualityObserved\"");
        let rendered: Value = serde_json::from_str(
            cases[0]["processors"][0]["json_schema"]["schema"]
                .as_str()
                .expect("the schema as text"),
        )
        .expect("the rendered schema is JSON");
        assert_eq!(rendered, model(false).classes["AirQualityObserved"]);
        assert!(cases[1]["processors"][0]["mapping"]
            .as_str()
            .is_some_and(|m| m.contains("throw") && m.contains("bb-air-quality")));
        let id_rule = stage[1]["mapping"].as_str().expect("the id rule");
        // The id rule asks for an NGSI-LD URN of the record's type and reads no space or
        // organization from it (ADR-N-041, PF-42).
        assert!(
            id_rule.contains("\"urn:ngsi-ld:\" + this.type.string() + \":\""),
            "{id_rule}"
        );
        assert!(
            !id_rule.contains("JC_ORG_DOMAIN") && !id_rule.contains("JC_SPACE"),
            "{id_rule}"
        );
    }

    /// T-6548: every pipeline's stream carries its space's stage, so the stage must grow with
    /// the model, not with its classes times the model. Each class here reaches one of 30 large
    /// definitions; a class schema holding all of them rendered 30 copies of the model into each
    /// stream, and Portal 1d71dc7 climbed past its 1536Mi limit at start once the seeded models'
    /// JSON Schemas were staged.
    #[test]
    fn the_stage_text_grows_with_the_model_not_with_its_classes_times_the_model() {
        let classes: Vec<String> = (0..30).map(|i| format!("Class{i}")).collect();
        let mut definitions = serde_json::Map::new();
        for (i, class) in classes.iter().enumerate() {
            let values: Vec<String> = (0..200).map(|v| format!("code-{i}-{v}")).collect();
            definitions.insert(
                format!("Codes{i}"),
                json!({ "type": "string", "enum": values }),
            );
            definitions.insert(
                class.clone(),
                json!({ "properties": { "code": {
                    "$ref": format!("#/definitions/Codes{i}"), "x-ngsi-ld-kind": "Property"
                } } }),
            );
        }
        let schema = json!({ "definitions": definitions });
        let model = ModelSchema::compile("codes", "1.0.0", &schema, &classes, false);
        let staged = stage(&model)
            .iter()
            .map(|p| p.to_string().len())
            .sum::<usize>();
        let whole = schema.to_string().len();
        assert!(
            staged < 3 * whole,
            "the stage is {staged} bytes for a model of {whole}: each class carries the whole model"
        );
    }

    fn write(dir: &Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, body).expect("write");
    }

    /// PL-60: the model a space names, with its JSON Schema read from beside the manifest; a
    /// space that names none compiles to nothing, and a draft without an artifact to its types.
    #[test]
    fn load_compiles_the_model_each_space_names() {
        let dir = std::env::temp_dir().join(format!(
            "portal-validation-load-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        let space = |name: &str, model: Option<&str>| {
            let named = model
                .map(|m| format!("\n  dataModelRef: {{ kind: DataModel, name: {m} }}"))
                .unwrap_or_default();
            format!("apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: {name}\n  namespace: ovzdusie\nspec:\n  isSandbox: false{named}\n")
        };
        write(
            &dir,
            "projects/ovzdusie/spaces/ovzdusie/space.yaml",
            &space("ovzdusie", Some("air")),
        );
        write(
            &dir,
            "projects/ovzdusie/spaces/raw/space.yaml",
            &space("raw", None),
        );
        write(
            &dir,
            "projects/ovzdusie/spaces/draft/space.yaml",
            &space("draft", Some("drafted")),
        );
        let model = |name: &str, space: &str, artifact: bool| {
            format!(
                "apiVersion: joinedcontext.com/v1alpha1\nkind: DataModel\nmetadata:\n  name: {name}\n  namespace: ovzdusie\nspec:\n  contextSpaceRef: {space}\n  linkml: ./{name}.linkml.yaml\n  version: 1.0.0\n  lifecycle: draft\n  classes: [AirQualityObserved]\n{}",
                if artifact { format!("  artifacts:\n    jsonSchema: ./{name}.v1.schema.json\n") } else { String::new() }
            )
        };
        write(
            &dir,
            "projects/ovzdusie/spaces/ovzdusie/datamodels/air.yaml",
            &model("air", "ovzdusie", true),
        );
        write(
            &dir,
            "projects/ovzdusie/spaces/ovzdusie/datamodels/air.v1.schema.json",
            &schema().to_string(),
        );
        write(
            &dir,
            "projects/ovzdusie/spaces/draft/datamodels/drafted.yaml",
            &model("drafted", "draft", false),
        );
        let repo = jcctl::loader::Repository::load(&dir).expect("the repository loads");

        let compiled = load(&repo, &dir);
        assert_eq!(compiled.len(), 2, "{:?}", compiled.keys());
        let air = &compiled[&("ovzdusie".to_owned(), "ovzdusie".to_owned())];
        assert_eq!(air.model, "air");
        let mut record = valid();
        record["pm10"]["value"] = json!("n/a");
        assert_eq!(
            air.check(&record)[0].rule,
            "sh:datatype",
            "the artifact was read"
        );

        let drafted = &compiled[&("ovzdusie".to_owned(), "draft".to_owned())];
        let loose = json!({ "id": "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:draft:1", "type": "AirQualityObserved", "anything": { "type": "Property", "value": 1 } });
        assert!(
            drafted.check(&loose).is_empty(),
            "a draft holds its types and ids only"
        );
        let mut wrong = loose.clone();
        wrong["type"] = json!("Device");
        assert_eq!(drafted.check(&wrong)[0].rule, "type");
    }

    #[test]
    fn the_stage_goes_right_before_the_split_and_a_stream_without_one_is_left_alone() {
        let mut stream = json!({ "pipeline": { "processors": [
            { "mapping": "root = this" }, { "unarchive": {} }, { "split": { "size": 1000 } }, { "archive": {} }
        ]}});
        assert!(insert_stage(
            &mut stream,
            &model(false),
            "http://p/internal/pipelines/a/b/rejected"
        ));
        let labels: Vec<Value> = stream["pipeline"]["processors"]
            .as_array()
            .expect("processors")
            .iter()
            .map(|p| p.get("label").cloned().unwrap_or(Value::Null))
            .collect();
        assert_eq!(
            labels[2..5],
            [
                json!("validation"),
                json!("validation_id"),
                json!("rejected")
            ]
        );
        assert!(stream["pipeline"]["processors"][5].get("split").is_some());

        let mut no_split = json!({ "pipeline": { "processors": [{ "mapping": "root = this" }] } });
        assert!(!insert_stage(&mut no_split, &model(false), "http://p/x"));
        assert_eq!(
            no_split["pipeline"]["processors"].as_array().map(Vec::len),
            Some(1)
        );
    }

    /// T-4014: the record the gateway refuses with `sh:pattern` at `stationCode` (T-4011,
    /// `model_constraint_tests.rs`) is refused by the workbench's step with the same rule and path.
    #[test]
    fn the_gateways_refusal_is_the_steps_refusal() {
        let mut schema = schema();
        schema["definitions"]["AirQualityObserved"]["properties"]["stationCode"] = json!({ "type": "string", "pattern": "^[A-Z]{2}[0-9]{3}$", "x-ngsi-ld-kind": "Property" });
        let model = ModelSchema::compile(
            "bb-air-quality",
            "1.0.0",
            &schema,
            &["AirQualityObserved".into()],
            false,
        );
        let mut record = valid();
        record["stationCode"] = json!({ "type": "Property", "value": "BB001" });
        assert!(
            model.check(&record).is_empty(),
            "{:?}",
            model.check(&record)
        );
        record["stationCode"]["value"] = json!("x");
        let named: Vec<(String, String)> = model
            .check(&record)
            .into_iter()
            .map(|p| (p.rule, p.path))
            .collect();
        assert_eq!(named, [("sh:pattern".into(), "stationCode".into())]);
    }

    #[test]
    fn the_sink_posts_with_the_runners_own_credential_then_drops() {
        let sink = rejected_sink("http://portal-internal:8081/internal/pipelines/p/n/rejected");
        let steps = sink["catch"].as_array().expect("a catch");
        assert_eq!(steps[1]["http"]["oauth2"]["client_key"], "${JC_CLIENT_ID}");
        assert_eq!(steps[1]["http"]["retries"], 0);
        assert_eq!(steps[2]["mapping"], "root = deleted()");
    }

    fn ruled() -> ModelSchema {
        let mut ruled = model(false);
        ruled.quality = serde_json::from_value(json!([
            {"name": "pm10-plausible", "entityType": "AirQualityObserved", "kind": "range", "attribute": "pm10", "max": 500},
            {"name": "pm10-positive", "entityType": "AirQualityObserved", "kind": "range", "attribute": "pm10", "min": 0, "outcome": "reject"},
            {"name": "in-region", "entityType": "AirQualityObserved", "kind": "area", "attribute": "location", "bbox": [16.8, 47.7, 22.6, 49.7], "outcome": "reject"},
            {"name": "not-ahead", "entityType": "AirQualityObserved", "kind": "freshness", "attribute": "pm10", "maxAhead": "PT30S"}
        ]))
        .expect("rules");
        ruled
    }

    /// DM-88: the stage carries one mapping for the rejecting rules, labelled so its failures
    /// count as rejected; a space without one keeps the stage it had.
    #[test]
    fn the_stage_fails_a_record_a_rule_rejects_and_counts_it_rejected() {
        let mut stream = json!({ "pipeline": { "processors": [{ "split": { "size": 1000 } }] } });
        assert!(insert_stage(&mut stream, &ruled(), "http://p/x"));
        let processors = stream["pipeline"]["processors"]
            .as_array()
            .expect("processors");
        let labels: Vec<&str> = processors
            .iter()
            .filter_map(|p| p["label"].as_str())
            .collect();
        assert_eq!(labels, STAGE_LABELS);
        let mapping = processors[2]["mapping"]
            .as_str()
            .expect("the quality mapping");
        assert!(
            mapping.contains("pm10-positive")
                && mapping.contains("in-region")
                && mapping.contains("not-ahead")
        );
        assert!(
            !mapping.contains("pm10-plausible"),
            "a flag is the gateway's: {mapping}"
        );
        if let Ok(out) = std::env::var("JC_QUALITY_MAPPING_OUT") {
            std::fs::write(out, mapping).expect("write the mapping");
        }
        assert_eq!(
            stage(&model(false)).len(),
            2,
            "no rejecting rule, no mapping"
        );
    }
}
