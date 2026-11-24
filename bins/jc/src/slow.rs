//! sqlx's slow-statement WARN, folded into one line per statement fingerprint per minute.
//!
//! sqlx writes every statement over its threshold (1 s) as a WARN holding the whole SQL text:
//! 242 lines and 17.6 % of jc-prod's log bytes in five minutes (T-7479). [`layer`] takes those
//! events off the log line (`runtime::init_tracing` keeps them from the fmt layer with
//! [`is_slow_statement`]) and counts them by fingerprint: the statement's first
//! [`FINGERPRINT_CHARS`] characters, whitespace collapsed and every quoted or numeric literal
//! replaced by `?`, so a statement built with a value inline carries no user data into the log.
//! A ticker writes one WARN per fingerprint each minute with the count and the slowest time.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};
use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;

/// How long one line counts.
const WINDOW: Duration = Duration::from_secs(60);

/// How much of a statement its fingerprint keeps.
const FINGERPRINT_CHARS: usize = 120;

/// The target sqlx logs statements under.
const SQLX: &str = "sqlx::query";

/// Whether `meta` is sqlx's slow-statement event: only that one carries `slow_threshold`.
pub fn is_slow_statement(meta: &tracing::Metadata<'_>) -> bool {
    meta.is_event() && meta.target() == SQLX && meta.fields().field("slow_threshold").is_some()
}

/// The count and the slowest time of one fingerprint in the window.
#[derive(Default)]
struct Seen {
    statements: u64,
    slowest: f64,
}

/// When the window began, and what it counted by fingerprint.
type Window = (Instant, HashMap<String, Seen>);

static WINDOW_COUNTS: Mutex<Option<Window>> = Mutex::new(None);

/// The layer counting slow statements, filtered to them alone so it widens no other callsite;
/// starts the minute ticker.
pub fn layer<S>() -> impl Layer<S>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    tick();
    SlowStatements.with_filter(tracing_subscriber::filter::filter_fn(is_slow_statement))
}

struct SlowStatements;

impl<S: tracing::Subscriber> Layer<S> for SlowStatements {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        let text = if fields.statement.trim().is_empty() {
            fields.summary.trim_end_matches('…')
        } else {
            &fields.statement
        };
        count(fingerprint(text), fields.seconds, Instant::now());
    }
}

/// The fields of sqlx's event the line needs.
#[derive(Default)]
struct Fields {
    summary: String,
    statement: String,
    seconds: f64,
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "summary" => value.clone_into(&mut self.summary),
            "db.statement" => value.clone_into(&mut self.statement),
            _ => {}
        }
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        if field.name() == "elapsed_secs" {
            self.seconds = value;
        }
    }

    fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
}

/// `sql`'s first [`FINGERPRINT_CHARS`] characters with whitespace collapsed and each literal
/// (`'…'`, `E'…'`, `$tag$…$tag$` is left to its tag, a number not part of a name) as `?`.
pub fn fingerprint(sql: &str) -> String {
    let mut out = String::new();
    let mut chars = sql.chars().peekable();
    let mut prev = ' ';
    while let Some(c) = chars.next() {
        if out.chars().count() >= FINGERPRINT_CHARS {
            break;
        }
        if c == '\'' {
            // A quoted literal, `''` inside it included.
            while let Some(q) = chars.next() {
                if q == '\'' && chars.next_if_eq(&'\'').is_none() {
                    break;
                }
            }
            if prev == 'E' || prev == 'e' {
                out.pop();
            }
            out.push('?');
            prev = '?';
        } else if c.is_ascii_digit() && !(prev.is_alphanumeric() || prev == '_' || prev == '$') {
            while chars
                .next_if(|d| d.is_ascii_alphanumeric() || *d == '.')
                .is_some()
            {}
            out.push('?');
            prev = '?';
        } else if c.is_whitespace() {
            if prev != ' ' {
                out.push(' ');
            }
            prev = ' ';
        } else {
            out.push(c);
            prev = c;
        }
    }
    out.trim_end().to_owned()
}

/// One slow statement of `fingerprint` at `now`.
fn count(fingerprint: String, seconds: f64, now: Instant) {
    let mut held = WINDOW_COUNTS.lock().unwrap_or_else(PoisonError::into_inner);
    let (_, seen) = held.get_or_insert_with(|| (now, HashMap::new()));
    let entry = seen.entry(fingerprint).or_default();
    entry.statements += 1;
    entry.slowest = entry.slowest.max(seconds);
}

/// Writes one WARN per fingerprint counted when the window is [`WINDOW`] old, and a new window.
/// Never from inside `on_event`: the ticker's call (and a test's).
fn flush(now: Instant) {
    let mut lines: Vec<(String, Seen)> = {
        let mut held = WINDOW_COUNTS.lock().unwrap_or_else(PoisonError::into_inner);
        match held.as_ref() {
            Some((began, _)) if now.saturating_duration_since(*began) >= WINDOW => held
                .take()
                .map(|(_, seen)| seen.into_iter().collect())
                .unwrap_or_default(),
            _ => return,
        }
    };
    lines.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    for (fingerprint, seen) in lines {
        tracing::warn!(
            target: "jc::slow_statement",
            fingerprint,
            statements = seen.statements,
            slowest_seconds = seen.slowest,
            "slow statements in the last minute: execution time exceeded alert threshold"
        );
    }
}

/// One thread for the process: a minute's lines are written even when no statement follows.
fn tick() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("jc-slow-statements".to_owned())
            .spawn(|| {
                loop {
                    std::thread::sleep(WINDOW / 4);
                    flush(Instant::now());
                }
            });
        if let Err(error) = spawned {
            tracing::error!(%error, "the slow-statement ticker did not start: those lines stay unwritten");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// One test at a time holds the process's window.
    static SERIAL: Mutex<()> = Mutex::new(());

    /// What `f` logs through the same layers `init_tracing` installs, flushed a window later.
    fn logged(f: impl FnOnce()) -> String {
        use tracing_subscriber::layer::SubscriberExt;
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        use std::io::{Read as _, Seek as _};
        let file = Arc::new(tempfile::tempfile().expect("a temporary log file"));
        let fmt = tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(Arc::clone(&file))
            .with_filter(tracing_subscriber::filter::filter_fn(|m| {
                !is_slow_statement(m)
            }));
        let subscriber = tracing_subscriber::registry().with(fmt).with(
            SlowStatements.with_filter(tracing_subscriber::filter::filter_fn(is_slow_statement)),
        );
        tracing::subscriber::with_default(subscriber, || {
            f();
            flush(Instant::now() + WINDOW);
        });
        let mut log = String::new();
        let mut reader = &*file;
        reader.rewind().expect("rewind the log");
        reader.read_to_string(&mut log).expect("utf8 log");
        log
    }

    /// sqlx's slow event, field for field (sqlx-core 0.9 logger.rs).
    fn slow(sql: &str, seconds: f64) {
        let threshold = Duration::from_secs(1);
        let elapsed = Duration::from_secs_f64(seconds);
        tracing::warn!(
            target: "sqlx::query",
            summary = "select id from …",
            db.statement = format!("\n\n{sql}\n"),
            rows_affected = 0u64,
            rows_returned = 1u64,
            ?elapsed,
            elapsed_secs = seconds,
            slow_threshold = ?threshold,
            "slow statement: execution time exceeded alert threshold"
        );
    }

    #[test]
    fn two_slow_statements_of_one_fingerprint_write_one_line_with_count_2_and_no_values() {
        let log = logged(|| {
            slow(
                "SELECT id FROM people WHERE email = 'ana@example.org' AND age > 41",
                1.5,
            );
            slow(
                "SELECT id FROM people WHERE email = 'bo@example.org' AND age > 7",
                2.25,
            );
            // A slow-free sqlx line and any other WARN pass as before.
            tracing::warn!(target: "sqlx::query", summary = "begin", "statement");
        });
        let folded: Vec<_> = log
            .lines()
            .filter(|l| l.contains("jc::slow_statement") && l.contains("people"))
            .collect();
        assert_eq!(folded.len(), 1, "{log}");
        assert!(folded[0].contains("statements=2"), "{log}");
        assert!(folded[0].contains("slowest_seconds=2.25"), "{log}");
        assert!(
            folded[0].contains("SELECT id FROM people WHERE email = ? AND age > ?"),
            "{log}"
        );
        for leak in ["ana@", "bo@", "> 41", "> 7", "sqlx::query: slow statement"] {
            assert!(!log.contains(leak), "{leak}: {log}");
        }
        assert!(log.contains("sqlx::query: statement"), "{log}");
    }

    #[test]
    fn a_statement_without_its_text_is_counted_by_its_summary() {
        let log = logged(|| {
            tracing::warn!(
                target: "sqlx::query",
                summary = "update ledger set n = 5 where k = 'x' …",
                db.statement = "  ",
                db.system = "postgresql",
                elapsed_secs = 3.0,
                rows_secs = 0.5,
                slow_threshold = ?Duration::from_secs(1),
                "slow statement: execution time exceeded alert threshold"
            );
            slow("select a from aardvark", 1.0);
        });
        // One line per fingerprint, in fingerprint order.
        let order: Vec<usize> = ["select a from aardvark", "update ledger"]
            .iter()
            .map(|f| log.find(f).expect(f))
            .collect();
        assert!(order[0] < order[1], "{log}");
        let line = log
            .lines()
            .find(|l| l.contains("jc::slow_statement") && l.contains("ledger"))
            .expect("a folded ledger line");
        assert!(
            line.contains("update ledger set n = ? where k = ?"),
            "{log}"
        );
        assert!(
            line.contains("statements=1") && line.contains("slowest_seconds=3"),
            "{log}"
        );
        assert!(!line.contains('…'), "{log}");
    }

    #[test]
    fn a_window_younger_than_a_minute_is_kept_and_the_layer_counts_into_it() {
        use tracing_subscriber::layer::SubscriberExt;
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        // The layer `init_tracing` installs (it starts the ticker once).
        let subscriber = tracing_subscriber::registry().with(layer());
        tracing::subscriber::with_default(subscriber, || slow("select 1 from young_window", 1.25));
        flush(now);
        let held = WINDOW_COUNTS.lock().unwrap_or_else(PoisonError::into_inner);
        let seen = &held.as_ref().expect("the window stays").1["select ? from young_window"];
        assert_eq!((seen.statements, seen.slowest), (1, 1.25));
        drop(held);
        flush(Instant::now() + WINDOW);
        assert!(
            WINDOW_COUNTS.lock().unwrap().is_none(),
            "an old window is written and taken"
        );
        // No window at all: nothing to write.
        flush(Instant::now() + WINDOW);
    }

    #[test]
    fn a_fingerprint_keeps_names_cuts_long_statements_and_hides_literals() {
        assert_eq!(
            fingerprint("select t1.a_2, $1 from t1"),
            "select t1.a_2, $1 from t1"
        );
        assert_eq!(
            fingerprint("insert  into x\n values (E'it''s', 3.5e2)"),
            "insert into x values (?, ?)"
        );
        let long = format!("select {} from t", "a, ".repeat(100));
        assert_eq!(fingerprint(&long).chars().count(), FINGERPRINT_CHARS);
    }
}
