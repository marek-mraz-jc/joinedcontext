//! go-syslog v3.0.0 `rfc5424/machine.go.rl`, `parser.go` and `syslog_message.go`: the RFC 5424
//! parser Bento's `parse_log` runs for `syslog_rfc5424`. The state machine itself is
//! `rfc5424/machine.rs`, translated from the Ragel output by scripts/ragel-go-to-rust.py.

use std::collections::BTreeMap;

pub(super) use super::common::{col, decimal, go_string, parse_rfc3339, remove_bytes, GoTime};

mod machine;

pub(super) const ERR_PRIVAL: &str = "expecting a priority value in the range 1-191 or equal to 0";
pub(super) const ERR_PRI: &str = "expecting a priority value within angle brackets";
pub(super) const ERR_VERSION: &str = "expecting a version value in the range 1-999";
pub(super) const ERR_TIMESTAMP: &str = "expecting a RFC3339MICRO timestamp or a nil value";
pub(super) const ERR_HOSTNAME: &str =
    "expecting an hostname (from 1 to max 255 US-ASCII characters) or a nil value";
pub(super) const ERR_APPNAME: &str =
    "expecting an app-name (from 1 to max 48 US-ASCII characters) or a nil value";
pub(super) const ERR_PROC_ID: &str =
    "expecting a procid (from 1 to max 128 US-ASCII characters) or a nil value";
pub(super) const ERR_MSG_ID: &str =
    "expecting a msgid (from 1 to max 32 US-ASCII characters) or a nil value";
pub(super) const ERR_STRUCTURED_DATA: &str = "expecting a structured data section containing one or more elements (`[id( key=\"value\")*]+`) or a nil value";
pub(super) const ERR_SD_ID: &str = "expecting a structured data element id (from 1 to max 32 US-ASCII characters; except `=`, ` `, `]`, and `\"`";
pub(super) const ERR_SD_ID_DUPLICATED: &str = "duplicate structured data element id";
pub(super) const ERR_SD_PARAM: &str = "expecting a structured data parameter (`key=\"value\"`, both part from 1 to max 32 US-ASCII characters; key cannot contain `=`, ` `, `]`, and `\"`, while value cannot contain `]`, backslash, and `\"` unless escaped)";
pub(super) const ERR_MSG: &str =
    "expecting a free-form optional message in UTF-8 (starting with or without BOM)";
pub(super) const ERR_ESCAPE: &str =
    "expecting chars `]`, `\"`, and `\\` to be escaped within param value";
pub(super) const ERR_PARSE: &str = "parsing error";

pub(super) const START: i32 = 1;
const FIRST_FINAL: i32 = 603;
const EN_FAIL: i32 = 607;

/// `syslogMessage`.
#[derive(Default)]
pub(super) struct Output {
    pub(super) priority_set: bool,
    pub(super) timestamp_set: bool,
    pub(super) has_elements: bool,
    pub(super) priority: u8,
    pub(super) version: u16,
    pub(super) timestamp: Option<GoTime>,
    pub(super) hostname: String,
    pub(super) appname: String,
    pub(super) proc_id: String,
    pub(super) msg_id: String,
    pub(super) structured_data: BTreeMap<String, BTreeMap<String, String>>,
    pub(super) message: String,
}

/// `machine`.
pub(super) struct Machine<'a> {
    pub(super) data: &'a [u8],
    pub(super) cs: i32,
    pub(super) p: isize,
    pub(super) pe: isize,
    pub(super) eof: isize,
    pub(super) pb: isize,
    pub(super) err: Option<String>,
    pub(super) currentelem: String,
    pub(super) currentparam: String,
    pub(super) msgat: isize,
    pub(super) backslashat: Vec<isize>,
}

impl Machine<'_> {
    /// `(m.data)[(m.p)]`.
    pub(super) fn c(&self) -> u8 {
        self.data
            .get(usize::try_from(self.p).unwrap_or(usize::MAX))
            .copied()
            .unwrap_or(0)
    }

    pub(super) fn slice(&self, a: isize, b: isize) -> &[u8] {
        let a = usize::try_from(a).unwrap_or(0);
        let b = usize::try_from(b).unwrap_or(0);
        self.data.get(a..b).unwrap_or_default()
    }

    /// `m.text()`.
    pub(super) fn text(&self) -> &[u8] {
        self.slice(self.pb, self.p)
    }
}

/// The exported `SyslogMessage` (`syslog.Base`), as Bento reads it.
#[derive(Debug, Default, PartialEq)]
pub struct SyslogMessage {
    pub priority: Option<u8>,
    pub facility: Option<u8>,
    pub severity: Option<u8>,
    pub version: u16,
    pub timestamp: Option<GoTime>,
    pub hostname: Option<String>,
    pub appname: Option<String>,
    pub proc_id: Option<String>,
    pub msg_id: Option<String>,
    pub structured_data: Option<BTreeMap<String, BTreeMap<String, String>>>,
    pub message: Option<String>,
}

fn set(s: String) -> Option<String> {
    (s != "-" && !s.is_empty()).then_some(s)
}

impl Output {
    fn minimal(&self) -> bool {
        self.priority_set && self.priority <= 191 && (1..=999).contains(&self.version)
    }

    fn export(self) -> SyslogMessage {
        SyslogMessage {
            priority: Some(self.priority),
            facility: Some(self.priority / 8),
            severity: Some(self.priority % 8),
            version: self.version,
            timestamp: if self.timestamp_set {
                self.timestamp
            } else {
                None
            },
            hostname: set(self.hostname),
            appname: set(self.appname),
            proc_id: set(self.proc_id),
            msg_id: set(self.msg_id),
            structured_data: self.has_elements.then_some(self.structured_data),
            message: (!self.message.is_empty()).then_some(self.message),
        }
    }
}

/// `parser.Parse`: the message, or the error with the partial message when `best_effort` holds and
/// the partial message is minimally valid.
pub fn parse(input: &[u8], best_effort: bool) -> (Option<SyslogMessage>, Option<String>) {
    let len = isize::try_from(input.len()).unwrap_or(isize::MAX);
    let mut m = Machine {
        data: input,
        cs: START,
        p: 0,
        pe: len,
        eof: len,
        pb: 0,
        err: None,
        currentelem: String::new(),
        currentparam: String::new(),
        msgat: 0,
        backslashat: Vec::new(),
    };
    let mut out = Output::default();
    machine::exec(&mut m, &mut out);
    if m.cs < FIRST_FINAL || m.cs == EN_FAIL {
        if best_effort && out.minimal() {
            return (Some(out.export()), m.err);
        }
        return (None, m.err);
    }
    (Some(out.export()), None)
}
