//! Reading a processor's config object the way Bento's `service.ParsedConfig` does: unknown fields
//! refused, each field's kind checked, an absent field left to its spec default.

use std::collections::BTreeMap;

use ekiben_core::CoreError;
use serde_json::{Map, Value as JsonValue};

static EMPTY: std::sync::LazyLock<Map<String, JsonValue>> = std::sync::LazyLock::new(Map::new);

pub struct Fields<'a> {
    component: &'static str,
    obj: &'a Map<String, JsonValue>,
}

impl<'a> Fields<'a> {
    pub fn new(
        component: &'static str,
        conf: &'a JsonValue,
        known: &[&str],
    ) -> Result<Self, CoreError> {
        let obj = match conf {
            JsonValue::Null => &*EMPTY,
            JsonValue::Object(o) => o,
            _ => {
                return Err(CoreError::config(format!(
                    "{component} config must be an object"
                )))
            }
        };
        if let Some(k) = obj.keys().find(|k| !known.contains(&k.as_str())) {
            return Err(CoreError::config(format!(
                "{component}: field {k} not recognised"
            )));
        }
        Ok(Self { component, obj })
    }

    fn kind_err(&self, name: &str, kind: &str) -> CoreError {
        CoreError::config(format!(
            "{} field {name}: expected {kind} value",
            self.component
        ))
    }

    pub fn missing(&self, name: &str) -> CoreError {
        CoreError::config(format!("{}: field '{name}' is required", self.component))
    }

    pub fn string(&self, name: &str) -> Result<Option<String>, CoreError> {
        match self.obj.get(name) {
            None => Ok(None),
            Some(JsonValue::String(s)) => Ok(Some(s.clone())),
            // YAML scalars Bento reads as strings.
            Some(JsonValue::Number(n)) => Ok(Some(n.to_string())),
            Some(JsonValue::Bool(b)) => Ok(Some(b.to_string())),
            Some(_) => Err(self.kind_err(name, "string")),
        }
    }

    pub fn bool(&self, name: &str) -> Result<Option<bool>, CoreError> {
        match self.obj.get(name) {
            None => Ok(None),
            Some(JsonValue::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(self.kind_err(name, "bool")),
        }
    }

    pub fn string_list(&self, name: &str) -> Result<Option<Vec<String>>, CoreError> {
        match self.obj.get(name) {
            None => Ok(None),
            Some(JsonValue::Array(a)) => a
                .iter()
                .map(|v| match v {
                    JsonValue::String(s) => Ok(s.clone()),
                    JsonValue::Number(n) => Ok(n.to_string()),
                    JsonValue::Bool(b) => Ok(b.to_string()),
                    _ => Err(self.kind_err(name, "string list")),
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Some),
            Some(_) => Err(self.kind_err(name, "string list")),
        }
    }

    pub fn string_map(&self, name: &str) -> Result<Option<BTreeMap<String, String>>, CoreError> {
        match self.obj.get(name) {
            None => Ok(None),
            Some(JsonValue::Object(o)) => o
                .iter()
                .map(|(k, v)| match v {
                    JsonValue::String(s) => Ok((k.clone(), s.clone())),
                    JsonValue::Number(n) => Ok((k.clone(), n.to_string())),
                    JsonValue::Bool(b) => Ok((k.clone(), b.to_string())),
                    _ => Err(self.kind_err(name, "string map")),
                })
                .collect::<Result<BTreeMap<_, _>, _>>()
                .map(Some),
            Some(_) => Err(self.kind_err(name, "string map")),
        }
    }
}
