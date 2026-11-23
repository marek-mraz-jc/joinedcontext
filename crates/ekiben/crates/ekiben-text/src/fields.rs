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

#[cfg(test)]
mod tests {
    use super::Fields;
    use serde_json::json;

    fn err(r: Result<impl std::fmt::Debug, ekiben_core::CoreError>) -> String {
        r.map_or_else(|e| e.to_string(), |v| format!("ok {v:?}"))
    }

    #[test]
    fn a_config_is_an_object_of_known_fields() {
        let known = ["a", "b"];
        assert!(Fields::new("grok", &json!(null), &known).is_ok());
        assert!(Fields::new("grok", &json!({"a": 1}), &known).is_ok());
        assert_eq!(
            err(Fields::new("grok", &json!([1]), &known).map(|_| ())),
            "configuration error: grok config must be an object"
        );
        assert_eq!(
            err(Fields::new("grok", &json!({"a": 1, "c": 2}), &known).map(|_| ())),
            "configuration error: grok: field c not recognised"
        );
    }

    #[test]
    fn each_kind_reads_its_values_and_refuses_the_others() -> Result<(), ekiben_core::CoreError> {
        let conf = json!({
            "s": "x", "n": 7, "t": true, "o": {},
            "l": ["x", 2, false], "bl": [[]],
            "m": {"a": "x", "b": 2, "c": true}, "bm": {"a": null},
        });
        let keys = ["s", "n", "t", "o", "l", "bl", "m", "bm"];
        let f = Fields::new("jq", &conf, &keys)?;
        assert_eq!(f.string("s")?, Some("x".into()));
        assert_eq!(f.string("n")?, Some("7".into()));
        assert_eq!(f.string("t")?, Some("true".into()));
        assert_eq!(f.string("absent")?, None);
        assert_eq!(
            err(f.string("o")),
            "configuration error: jq field o: expected string value"
        );
        assert_eq!(f.bool("t")?, Some(true));
        assert_eq!(f.bool("absent")?, None);
        assert_eq!(
            err(f.bool("s")),
            "configuration error: jq field s: expected bool value"
        );
        assert_eq!(
            f.string_list("l")?,
            Some(vec!["x".into(), "2".into(), "false".into()])
        );
        assert_eq!(f.string_list("absent")?, None);
        let list = "configuration error: jq field bl: expected string list value";
        assert_eq!(err(f.string_list("bl")), list);
        assert_eq!(
            err(f.string_list("s")),
            "configuration error: jq field s: expected string list value"
        );
        let map = f.string_map("m")?.unwrap_or_default();
        assert_eq!(
            map.into_iter().collect::<Vec<_>>(),
            [("a", "x"), ("b", "2"), ("c", "true")].map(|(k, v)| (k.to_string(), v.to_string()))
        );
        assert_eq!(f.string_map("absent")?, None);
        let bad_map = "configuration error: jq field bm: expected string map value";
        assert_eq!(err(f.string_map("bm")), bad_map);
        assert_eq!(
            err(f.string_map("l")),
            "configuration error: jq field l: expected string map value"
        );
        assert_eq!(
            f.missing("s").to_string(),
            "configuration error: jq: field 's' is required"
        );
        Ok(())
    }
}
