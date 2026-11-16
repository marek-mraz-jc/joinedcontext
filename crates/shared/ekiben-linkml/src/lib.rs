//! `linkml_validate`: checks each message, a normalized NGSI-LD Entity in the compacted form a
//! pipeline writes, against the target space's LinkML model and fails the message with the first
//! rule it breaks (PL-59, PL-60). The Portal renders it as the validation stage before every write
//! in place of one `json_schema` per class (crates/portal `pipeline_validation::stage`), so the
//! runner, the Portal's rejected list and the gateway's `linkml_model::check` name one rule with
//! one SHACL component.
//!
//! The model arrives inline in the stream (`model`, resolved LinkML JSON as Model Tools renders
//! it for the broker), never from disk, and is read once when the stream is built under the
//! surfaces' limits (`linkml_model::SURFACE_LIMITS`). `revision` names the model's digest, so a
//! new version changes the stream's text and the stream is built again (PL-60). `closed` refuses
//! an attribute no class of the entity lists (`sh:closed`, a model that is not `openWorld`).
//!
//! An error names the attribute, the class and the rule, never the value (personal data); the
//! rule's SHACL component is in the message's `jc_rule` metadata.

use std::sync::Arc;

use async_trait::async_trait;
use ekiben_core::{Batch, ComponentConfig, CoreError, ProcContext, Processor, Registry};
use linkml_model::Model;
use linkml_model::check::{self, Rules};
use serde_json::Value;

/// Registers `linkml_validate`; the jc binary calls it beside `ekiben_server::build_full_registry`.
pub fn register(registry: &Registry) {
    registry.register_processor("linkml_validate", |conf, _ctx| {
        Ok(Arc::new(LinkmlValidateProcessor::from_config(conf)?))
    });
}

pub struct LinkmlValidateProcessor {
    rules: Rules,
}

impl LinkmlValidateProcessor {
    pub fn from_config(conf: &ComponentConfig) -> Result<Self, CoreError> {
        let text = conf
            .config
            .get("model")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .ok_or_else(|| CoreError::config("linkml_validate: a model is required"))?;
        let closed = match conf.config.get("closed") {
            None => false,
            Some(Value::Bool(b)) => *b,
            Some(_) => {
                return Err(CoreError::config(
                    "linkml_validate: closed is true or false",
                ));
            }
        };
        let model = Model::read(text.as_bytes(), &linkml_model::SURFACE_LIMITS).map_err(|e| {
            CoreError::config(format!(
                "linkml_validate: the model is not one the surfaces read: {e}"
            ))
        })?;
        Ok(Self {
            rules: Rules::compact(&model, closed),
        })
    }

    /// The first rule `doc` breaks, as the processor fails it.
    pub fn check(&self, doc: &Value) -> Result<(), check::Violation> {
        // A pipeline writes whole entities (an upsert), so the required slots hold.
        check::entity(&self.rules, doc, true)
    }
}

#[async_trait]
impl Processor for LinkmlValidateProcessor {
    fn processor_type(&self) -> &'static str {
        "linkml_validate"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        mut batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        for part in batch.iter_mut() {
            let verdict = serde_json::from_slice::<Value>(&part.as_bytes_ref())
                .map_err(|e| (None, format!("failed to parse message as JSON: {e}")))
                .and_then(|doc| {
                    self.check(&doc)
                        .map_err(|v| (Some(v.component), v.to_string()))
                });
            if let Err((component, message)) = verdict {
                if let Some(component) = component {
                    part.meta_set_str("jc_rule", component);
                }
                ctx.on_error(part, message);
            }
        }
        Ok(vec![batch])
    }
}
