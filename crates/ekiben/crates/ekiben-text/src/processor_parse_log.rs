//! The `parse_log` processor (`internal/impl/pure/processor_parse_log.go`, Bento 1.22.0): parses a
//! syslog line with go-syslog v3.0.0 (`crate::go_syslog`) into a structured document. A message the
//! parser rejects fails with the parser's error, `best_effort` or not: Bento drops the partial
//! message go-syslog returns beside an error.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Datelike;
use ekiben_bloblang::Value;
use ekiben_core::{Batch, ComponentConfig, CoreError, Part, ProcContext, Processor, Registry};

use crate::fields::Fields;
use crate::go_syslog::common::{format_rfc3339_nano, load_location};
use crate::go_syslog::{rfc3164, rfc5424};

pub fn register(registry: &Registry) {
    registry.register_processor("parse_log", |conf, _ctx| {
        Ok(Arc::new(ParseLogProcessor::from_config(conf)?))
    });
}

/// `parseLogConfig`.
#[derive(Clone, Debug)]
pub struct ParseLogConfig {
    pub format: String,
    pub best_effort: bool,
    pub with_rfc3339: bool,
    pub with_year: String,
    pub with_timezone: String,
}

impl Default for ParseLogConfig {
    fn default() -> Self {
        Self {
            format: String::new(),
            best_effort: true,
            with_rfc3339: true,
            with_year: "current".to_string(),
            with_timezone: "UTC".to_string(),
        }
    }
}

enum Format {
    Rfc5424 { best_effort: bool },
    Rfc3164(rfc3164::Options),
}

/// Implements `parse_log`.
pub struct ParseLogProcessor {
    format: Format,
}

type Doc = BTreeMap<String, Value>;

fn put_str(doc: &mut Doc, k: &str, v: Option<String>) {
    if let Some(v) = v {
        doc.insert(k.to_string(), Value::String(v));
    }
}

fn put_num(doc: &mut Doc, k: &str, v: Option<u8>) {
    if let Some(v) = v {
        doc.insert(k.to_string(), Value::Int(i64::from(v)));
    }
}

impl ParseLogProcessor {
    pub fn from_config(conf: &ComponentConfig) -> Result<Self, CoreError> {
        let f = Fields::new(
            "parse_log",
            &conf.config,
            &[
                "format",
                "codec",
                "best_effort",
                "allow_rfc3339",
                "default_year",
                "default_timezone",
            ],
        )?;
        let d = ParseLogConfig::default();
        let c = ParseLogConfig {
            format: f.string("format")?.ok_or_else(|| f.missing("format"))?,
            best_effort: f.bool("best_effort")?.unwrap_or(d.best_effort),
            with_rfc3339: f.bool("allow_rfc3339")?.unwrap_or(d.with_rfc3339),
            with_year: f.string("default_year")?.unwrap_or(d.with_year),
            with_timezone: f.string("default_timezone")?.unwrap_or(d.with_timezone),
        };
        // `codec` is deprecated and read by nothing, in Bento as here.
        f.string("codec")?;
        Self::new(&c).map_err(CoreError::config)
    }

    /// `newParseLog` / `getParseFormat`.
    pub fn new(c: &ParseLogConfig) -> Result<Self, String> {
        let format = match c.format.as_str() {
            "syslog_rfc5424" => Format::Rfc5424 {
                best_effort: c.best_effort,
            },
            "syslog_rfc3164" => {
                let year = match c.with_year.as_str() {
                    "current" => i64::from(chrono::Utc::now().year()),
                    "" => 0,
                    y => go_atoi(y)
                        .map_err(|e| format!("failed to convert year {y} into integer:  {e}"))?,
                };
                let location = if c.with_timezone.is_empty() {
                    None
                } else {
                    // Bento prints the nil location it failed to load, which reads "UTC".
                    Some(
                        load_location(&c.with_timezone)
                            .map_err(|e| format!("failed to lookup timezone UTC - {e}"))?,
                    )
                };
                Format::Rfc3164(rfc3164::Options {
                    best_effort: c.best_effort,
                    year,
                    rfc3339: c.with_rfc3339,
                    location,
                })
            }
            other => return Err(format!("format not recognised: {other}")),
        };
        Ok(Self { format })
    }

    /// `parserRFC5424` / `parserRFC3164`: the document Bento writes, keys sorted as Go's JSON does.
    pub fn parse(&self, body: &[u8]) -> Result<Value, String> {
        let mut doc = Doc::new();
        match &self.format {
            Format::Rfc5424 { best_effort } => {
                let res = match rfc5424::parse(body, *best_effort) {
                    (_, Some(e)) => return Err(e),
                    (None, None) => return Err(no_message()),
                    (Some(r), None) => r,
                };
                put_str(&mut doc, "message", res.message);
                put_str(
                    &mut doc,
                    "timestamp",
                    res.timestamp.as_ref().map(format_rfc3339_nano),
                );
                put_num(&mut doc, "facility", res.facility);
                put_num(&mut doc, "severity", res.severity);
                put_num(&mut doc, "priority", res.priority);
                if res.version != 0 {
                    doc.insert("version".into(), Value::Int(i64::from(res.version)));
                }
                put_str(&mut doc, "hostname", res.hostname);
                put_str(&mut doc, "procid", res.proc_id);
                put_str(&mut doc, "appname", res.appname);
                put_str(&mut doc, "msgid", res.msg_id);
                if let Some(sd) = res.structured_data {
                    let sd: Doc = sd
                        .into_iter()
                        .map(|(k, el)| {
                            let el: Doc =
                                el.into_iter().map(|(k, v)| (k, Value::String(v))).collect();
                            (k, Value::Object(el.into()))
                        })
                        .collect();
                    doc.insert("structureddata".into(), Value::Object(sd.into()));
                }
            }
            Format::Rfc3164(opts) => {
                let res = match rfc3164::parse(body, opts) {
                    (_, Some(e)) => return Err(e),
                    (None, None) => return Err(no_message()),
                    (Some(r), None) => r,
                };
                put_str(&mut doc, "message", res.message);
                put_str(
                    &mut doc,
                    "timestamp",
                    res.timestamp.as_ref().map(format_rfc3339_nano),
                );
                put_num(&mut doc, "facility", res.facility);
                put_num(&mut doc, "severity", res.severity);
                put_num(&mut doc, "priority", res.priority);
                put_str(&mut doc, "hostname", res.hostname);
                put_str(&mut doc, "procid", res.proc_id);
                put_str(&mut doc, "appname", res.appname);
            }
        }
        Ok(Value::Object(doc.into()))
    }

    pub fn process(&self, part: &mut Part) -> Result<(), String> {
        let v = self.parse(&part.as_bytes())?;
        part.set_structured(v);
        Ok(())
    }
}

/// go-syslog returns neither message nor error when the input stops inside a field that has no
/// end-of-input action and best effort cannot export it; Bento then dereferences a nil message and
/// panics. ekiben fails the message instead.
fn no_message() -> String {
    "parsing error: incomplete syslog message".to_string()
}

/// `strconv.Atoi`'s accepted syntax and error text.
fn go_atoi(s: &str) -> Result<i64, String> {
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("strconv.Atoi: parsing {s:?}: invalid syntax"));
    }
    s.parse::<i64>()
        .map_err(|_| format!("strconv.Atoi: parsing {s:?}: value out of range"))
}

#[async_trait]
impl Processor for ParseLogProcessor {
    fn processor_type(&self) -> &'static str {
        "parse_log"
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
