//! The `msgpack` processor (`internal/impl/msgpack/processor.go`, Bento 1.22.0): `to_json` reads a
//! MessagePack document into the structured message, `from_json` writes the structured message as
//! MessagePack. The codec is vmihailenco/msgpack v5's, shared with Bloblang's `parse_msgpack` and
//! `format_msgpack` (`internal/impl/msgpack/bloblang.go`, ported in `ekiben_bloblang::msgpack`).
//! ponytail: `from_json` writes object keys sorted where Go writes them in random map order.

use std::sync::Arc;

use async_trait::async_trait;
use ekiben_bloblang::msgpack;
use ekiben_core::{Batch, ComponentConfig, CoreError, Part, ProcContext, Processor, Registry};

use crate::fields::Fields;

pub fn register(registry: &Registry) {
    registry.register_processor("msgpack", |conf, _ctx| {
        Ok(Arc::new(MsgpackProcessor::from_config(conf)?))
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operator {
    ToJson,
    FromJson,
}

/// Implements `msgpack`.
pub struct MsgpackProcessor {
    operator: Operator,
}

impl MsgpackProcessor {
    pub fn from_config(conf: &ComponentConfig) -> Result<Self, CoreError> {
        let f = Fields::new("msgpack", &conf.config, &["operator"])?;
        let op = f.string("operator")?.ok_or_else(|| f.missing("operator"))?;
        Self::new(&op).map_err(CoreError::config)
    }

    /// `strToMsgPackOperator`.
    pub fn new(op: &str) -> Result<Self, String> {
        let operator = match op {
            "to_json" => Operator::ToJson,
            "from_json" => Operator::FromJson,
            other => return Err(format!("operator not recognised: {other}")),
        };
        Ok(Self { operator })
    }

    pub fn process(&self, part: &mut Part) -> Result<(), String> {
        match self.operator {
            Operator::ToJson => {
                let v = msgpack::decode(&part.as_bytes()).map_err(|e| {
                    format!("failed to convert MsgPack document to JSON: {e}")
                })?;
                part.set_structured(v);
            }
            Operator::FromJson => {
                let v = part
                    .as_structured()
                    .map_err(|e| format!("failed to parse message as JSON: {e}"))?;
                part.set_bytes(msgpack::encode(&v));
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Processor for MsgpackProcessor {
    fn processor_type(&self) -> &'static str {
        "msgpack"
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
