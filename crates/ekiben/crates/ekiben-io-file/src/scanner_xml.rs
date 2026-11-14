//! `xml_documents` scanner (`internal/impl/pure/scanner_xml.go`): one message per top-level XML
//! element of the stream, converted to a structured value as mxj's `NewMapXmlReader` does. Text,
//! comments and processing instructions between documents are skipped, as mxj's decoder skips them.

use ekiben_bloblang::methods_strings::parse_xml_to_value;
use ekiben_bloblang::Value;
use ekiben_core::CoreError;
use quick_xml::events::Event;
use serde_json::Value as JsonValue;

/// Bento's `operator` (only `to_json`) and `cast`. Bento 1.22.0 reads `cast` and never hands it to
/// mxj, so values stay strings either way; ekiben does the same for the same output.
#[derive(Clone, Debug)]
pub struct XmlDocumentScanner;

impl XmlDocumentScanner {
    pub fn from_config(obj: &serde_json::Map<String, JsonValue>) -> Result<Self, CoreError> {
        let operator = obj
            .get("operator")
            .and_then(JsonValue::as_str)
            .unwrap_or("");
        if operator != "to_json" {
            return Err(CoreError::config(format!(
                "operator not recognised: {operator}"
            )));
        }
        if obj.get("cast").is_some_and(|c| !c.is_boolean()) {
            return Err(CoreError::config("field cast must be a bool"));
        }
        Ok(Self)
    }

    /// Every document of the stream, in order.
    pub fn documents(&self, raw: &[u8]) -> Result<Vec<Value>, CoreError> {
        let syntax = |e: &dyn std::fmt::Display| CoreError::other(format!("XML syntax error: {e}"));
        let mut reader = quick_xml::Reader::from_reader(raw);
        reader.config_mut().trim_text(false);
        let mut docs = Vec::new();
        let mut depth = 0usize;
        let mut start = 0usize;
        loop {
            let before = to_index(reader.buffer_position());
            let event = reader.read_event().map_err(|e| syntax(&e))?;
            let closed = match event {
                Event::Start(_) => {
                    if depth == 0 {
                        start = before;
                    }
                    depth += 1;
                    false
                }
                Event::End(_) => {
                    depth = depth.saturating_sub(1);
                    depth == 0
                }
                Event::Empty(_) if depth == 0 => {
                    start = before;
                    true
                }
                Event::DocType(_) => return Err(syntax(&"DOCTYPE is forbidden")),
                Event::Eof if depth > 0 => return Err(syntax(&"unexpected EOF")),
                Event::Eof => break,
                _ => false,
            };
            if closed {
                let end = to_index(reader.buffer_position());
                let doc = parse_xml_to_value(&raw[start..end], false).map_err(CoreError::other)?;
                docs.push(doc);
            }
        }
        Ok(docs)
    }
}

fn to_index(pos: u64) -> usize {
    usize::try_from(pos).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(input: &str, expected: &[&str]) {
        let conf = serde_json::json!({"operator": "to_json"});
        let got: Vec<String> = XmlDocumentScanner::from_config(conf.as_object().expect("object"))
            .expect("scanner")
            .documents(input.as_bytes())
            .expect("documents")
            .iter()
            .map(|v| v.to_json_value().to_string())
            .collect();
        assert_eq!(got, expected);
    }

    #[test]
    fn test_xml_scanner_default() {
        check(
            "<a>a0</a>\n<a>a1</a>\n<a>a2</a>\n<a>a3</a>\n<a>a4</a>\n",
            &[
                r#"{"a":"a0"}"#,
                r#"{"a":"a1"}"#,
                r#"{"a":"a2"}"#,
                r#"{"a":"a3"}"#,
                r#"{"a":"a4"}"#,
            ],
        );
    }

    #[test]
    fn test_xml_scanner_bad_data() {
        check(
            "<a>a0</a>invalid\n<a>a1</a>\n",
            &[r#"{"a":"a0"}"#, r#"{"a":"a1"}"#],
        );
    }

    #[test]
    fn test_xml_scanner_formatted() {
        check(
            "<a>\n\t\ta0\n\t</a>\n<a>\n\ta1\n</a>\n<a>\n\ta2\n</a>\n<a>\n\ta3\n</a>\n<a>\n\ta4\n</a>\n",
            &[r#"{"a":"a0"}"#, r#"{"a":"a1"}"#, r#"{"a":"a2"}"#, r#"{"a":"a3"}"#, r#"{"a":"a4"}"#],
        );
    }

    #[test]
    fn test_xml_scanner_nested() {
        check(
            "<a><b>ab0</b></a>\n<a><b>ab1</b></a>\n<a><b>ab2</b></a>\n<a><b>ab3</b></a>\n<a><b>ab4</b></a>\n",
            &[
                r#"{"a":{"b":"ab0"}}"#,
                r#"{"a":{"b":"ab1"}}"#,
                r#"{"a":{"b":"ab2"}}"#,
                r#"{"a":{"b":"ab3"}}"#,
                r#"{"a":{"b":"ab4"}}"#,
            ],
        );
    }

    #[test]
    fn test_xml_scanner_nested_and_formatted() {
        let doc = |i: usize| format!("<a>\n\t<b>\n\t\tab{i}\n\t</b>\n</a>\n");
        let input: String = (0..5).map(doc).collect();
        let want: Vec<String> = (0..5)
            .map(|i| format!(r#"{{"a":{{"b":"ab{i}"}}}}"#))
            .collect();
        check(&input, &want.iter().map(String::as_str).collect::<Vec<_>>());
    }

    #[test]
    fn test_xml_scanner_multiple_values_and_formatted() {
        let doc =
            |i: usize| format!("<a>\n\t<b>\n\t\tab{i}\n\t</b>\n\t<c>\n\t\tac{i}\n\t</c>\n</a>\n");
        let input: String = (0..3).map(doc).collect();
        let want: Vec<String> = (0..3)
            .map(|i| format!(r#"{{"a":{{"b":"ab{i}","c":"ac{i}"}}}}"#))
            .collect();
        check(&input, &want.iter().map(String::as_str).collect::<Vec<_>>());
    }

    #[test]
    fn an_unknown_operator_is_refused() {
        let conf = serde_json::json!({"operator": "to_yaml"});
        let err = XmlDocumentScanner::from_config(conf.as_object().expect("object"))
            .expect_err("refused");
        assert!(
            err.to_string().contains("operator not recognised: to_yaml"),
            "{err}"
        );
    }

    #[test]
    fn a_doctype_is_refused() {
        let conf = serde_json::json!({"operator": "to_json"});
        let scanner =
            XmlDocumentScanner::from_config(conf.as_object().expect("object")).expect("ok");
        let xxe = "<!DOCTYPE a [<!ENTITY x SYSTEM \"file:///etc/passwd\">]><a>&x;</a>";
        assert!(scanner.documents(xxe.as_bytes()).is_err());
    }
}
