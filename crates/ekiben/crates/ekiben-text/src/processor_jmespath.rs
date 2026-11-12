//! The `jmespath` processor (`internal/impl/pure/processor_jmespath.go`, Bento 1.22.0): the message
//! replaced by the query's result. As Bento's `clearNumbers` does, every number is a float64 before
//! the search, so integers and floats compare and print alike.

use std::sync::Arc;

use async_trait::async_trait;
use ekiben_bloblang::Value;
use ekiben_core::{Batch, ComponentConfig, CoreError, Part, ProcContext, Processor, Registry};
use jmespath::{Expression, Variable};
use serde_json::Value as JsonValue;

use crate::fields::Fields;

pub fn register(registry: &Registry) {
    registry.register_processor("jmespath", |conf, _ctx| {
        Ok(Arc::new(JmespathProcessor::from_config(conf)?))
    });
}

/// Implements `jmespath`.
pub struct JmespathProcessor {
    query: Expression<'static>,
}

impl JmespathProcessor {
    pub fn from_config(conf: &ComponentConfig) -> Result<Self, CoreError> {
        let f = Fields::new("jmespath", &conf.config, &["query"])?;
        let q = f.string("query")?.ok_or_else(|| f.missing("query"))?;
        let query = jmespath::compile(&q)
            .map_err(|e| CoreError::config(format!("failed to compile JMESPath query: {e}")))?;
        Ok(Self { query })
    }

    pub fn process(&self, part: &mut Part) -> Result<(), String> {
        let doc = clear_numbers(part.as_structured()?.to_json_value());
        let found = self.query.search(doc).map_err(|e| e.to_string())?;
        part.set_structured(from_var(&found));
        Ok(())
    }
}

#[async_trait]
impl Processor for JmespathProcessor {
    fn processor_type(&self) -> &'static str {
        "jmespath"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        let mut out = Batch::new();
        for mut part in batch {
            if let Err(e) = self.process(&mut part) {
                ctx.on_error(&mut part, e);
            }
            out.push(part);
        }
        Ok(vec![out])
    }
}

/// Bento's `clearNumbers`: each `json.Number` becomes a float64.
fn clear_numbers(v: JsonValue) -> JsonValue {
    match v {
        JsonValue::Number(n) => n
            .as_f64()
            .and_then(serde_json::Number::from_f64)
            .map_or(JsonValue::Null, JsonValue::Number),
        JsonValue::Array(a) => JsonValue::Array(a.into_iter().map(clear_numbers).collect()),
        JsonValue::Object(o) => {
            JsonValue::Object(o.into_iter().map(|(k, v)| (k, clear_numbers(v))).collect())
        }
        other => other,
    }
}

/// A go-jmespath result: numbers are float64 (its functions return float64 too).
fn from_var(v: &Variable) -> Value {
    match v {
        Variable::Null | Variable::Expref(_) => Value::Null,
        Variable::Bool(b) => Value::Bool(*b),
        Variable::Number(n) => Value::Float(n.as_f64().unwrap_or(f64::NAN)),
        Variable::String(s) => Value::String(s.clone()),
        Variable::Array(a) => {
            Value::Array(a.iter().map(|v| from_var(v)).collect::<Vec<_>>().into())
        }
        Variable::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), from_var(v)))
                .collect::<std::collections::BTreeMap<_, _>>()
                .into(),
        ),
    }
}
