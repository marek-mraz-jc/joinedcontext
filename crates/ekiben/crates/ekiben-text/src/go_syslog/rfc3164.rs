//! go-syslog v3.0.0 `rfc3164/machine.go.rl`, `parser.go`, `options.go`, `year.go` and
//! `syslog_message.go`: the RFC 3164 parser Bento's `parse_log` runs for `syslog_rfc3164`. The state
//! machine itself is `rfc3164/machine.rs`, translated from the Ragel output by
//! scripts/ragel-go-to-rust.py.

pub(super) use super::common::{
    add_years, col, decimal, go_string, in_location, parse_rfc3339, parse_stamp, GoTime, Location,
};

mod machine;

pub(super) const ERR_PRIVAL: &str = "expecting a priority value in the range 1-191 or equal to 0";
pub(super) const ERR_PRI: &str = "expecting a priority value within angle brackets";
pub(super) const ERR_TIMESTAMP: &str = "expecting a Stamp timestamp";
pub(super) const ERR_RFC3339: &str = "expecting a Stamp or a RFC3339 timestamp";
pub(super) const ERR_HOSTNAME: &str =
    "expecting an hostname (from 1 to max 255 US-ASCII characters)";
pub(super) const ERR_TAG: &str = "expecting an alphanumeric tag (max 32 characters)";

pub(super) const START: i32 = 1;
const FIRST_FINAL: i32 = 333;
const EN_FAIL: i32 = 373;

/// `syslogMessage`.
#[derive(Default)]
pub(super) struct Output {
    pub(super) priority_set: bool,
    pub(super) timestamp_set: bool,
    pub(super) priority: u8,
    pub(super) timestamp: Option<GoTime>,
    pub(super) hostname: String,
    pub(super) tag: String,
    pub(super) content: String,
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
    pub(super) yyyy: i64,
    pub(super) rfc3339: bool,
    pub(super) loc: Option<Location>,
}

impl Machine<'_> {
    /// `(m.data)[(m.p)]`.
    pub(super) fn c(&self) -> u8 {
        self.data
            .get(usize::try_from(self.p).unwrap_or(usize::MAX))
            .copied()
            .unwrap_or(0)
    }

    /// `m.text()`.
    pub(super) fn text(&self) -> &[u8] {
        let a = usize::try_from(self.pb).unwrap_or(0);
        let b = usize::try_from(self.p).unwrap_or(0);
        self.data.get(a..b).unwrap_or_default()
    }
}

/// The exported `SyslogMessage` (`syslog.Base`), as Bento reads it.
#[derive(Debug, Default, PartialEq)]
pub struct SyslogMessage {
    pub priority: Option<u8>,
    pub facility: Option<u8>,
    pub severity: Option<u8>,
    pub timestamp: Option<GoTime>,
    pub hostname: Option<String>,
    pub appname: Option<String>,
    pub proc_id: Option<String>,
    pub message: Option<String>,
}

fn set(s: String) -> Option<String> {
    (s != "-" && !s.is_empty()).then_some(s)
}

impl Output {
    fn minimal(&self) -> bool {
        self.priority_set && self.priority <= 191
    }

    fn export(self) -> SyslogMessage {
        SyslogMessage {
            priority: Some(self.priority),
            facility: Some(self.priority / 8),
            severity: Some(self.priority % 8),
            timestamp: if self.timestamp_set {
                self.timestamp
            } else {
                None
            },
            hostname: set(self.hostname),
            appname: set(self.tag),
            proc_id: set(self.content),
            message: (!self.message.is_empty()).then_some(self.message),
        }
    }
}

/// The machine options (`WithBestEffort`, `WithYear`, `WithTimezone`, `WithRFC3339`).
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub best_effort: bool,
    /// The year `WithYear` applied; 0 when unset.
    pub year: i64,
    pub rfc3339: bool,
    pub location: Option<Location>,
}

/// `parser.Parse`.
pub fn parse(input: &[u8], opts: &Options) -> (Option<SyslogMessage>, Option<String>) {
    let len = isize::try_from(input.len()).unwrap_or(isize::MAX);
    let mut m = Machine {
        data: input,
        cs: START,
        p: 0,
        pe: len,
        eof: len,
        pb: 0,
        err: None,
        yyyy: opts.year,
        rfc3339: opts.rfc3339,
        loc: opts.location.clone(),
    };
    let mut out = Output::default();
    machine::exec(&mut m, &mut out);
    if m.cs < FIRST_FINAL || m.cs == EN_FAIL {
        if opts.best_effort && out.minimal() {
            return (Some(out.export()), m.err);
        }
        return (None, m.err);
    }
    (Some(out.export()), None)
}
