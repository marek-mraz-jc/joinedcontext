//! `system_window` buffer (`internal/impl/pure/buffer_system_window.go`): holds messages and flushes
//! them as tumbling or sliding windows of the system clock, each message stamped with
//! `window_end_timestamp`. Times are nanoseconds since the Unix epoch; windows round on Go's zero
//! time (0001-01-01) as `time.Time.Round` does, so a 7m window starts where Bento's does.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use ekiben_bloblang::{Executor, Value};
use ekiben_core::{
    AckResponder, Batch, BatchError, Buffer, CombinedAcker, ComponentConfig, CoreError, Part,
    PartsView, StreamContext, Transaction,
};
use serde_json::Value as JsonValue;
use tokio::sync::watch;

/// Nanoseconds since 1970-01-01T00:00:00Z.
type Nanos = i128;
type Clock = Arc<dyn Fn() -> Nanos + Send + Sync>;
/// An input's ack, answered the first time a window carrying the message is acked or nacked.
type SharedAck = Arc<Mutex<Option<AckResponder>>>;

const SECOND: Nanos = 1_000_000_000;
/// Go's zero `time.Time`, 0001-01-01T00:00:00Z.
const GO_ZERO: Nanos = -62_135_596_800 * SECOND;
/// Bytes one window buffer may hold before a write is nacked: the pipeline limit class bounds a
/// run's memory (PIP-11).
pub const MAX_PENDING_BYTES: usize = 256 * 1024 * 1024;
/// The longest window: a longer one is refused when the stream is built (PIP-11).
pub const MAX_SIZE: Nanos = 24 * 3600 * SECOND;

struct TsMessage {
    ts: Nanos,
    part: Part,
    ack: SharedAck,
}

struct State {
    latest_flushed_window_end: Nanos,
    oldest_ts: Nanos,
    pending: Vec<TsMessage>,
    pending_bytes: usize,
}

pub struct SystemWindowBuffer {
    ts_mapping: Executor,
    clock: Clock,
    size: Nanos,
    slide: Nanos,
    offset: Nanos,
    allowed_lateness: Nanos,
    state: Mutex<State>,
    end_of_input: watch::Sender<bool>,
}

fn fire(ack: &SharedAck, err: Option<BatchError>) {
    let taken = ack.lock().unwrap_or_else(|p| p.into_inner()).take();
    if let Some(a) = taken {
        a.respond(err);
    }
}

fn system_now() -> Nanos {
    let now = chrono::Utc::now();
    Nanos::from(now.timestamp()) * SECOND + Nanos::from(now.timestamp_subsec_nanos())
}

/// Go's `time.Time.Round(d)`: the nearest multiple of `d` since the zero time, halves rounded up.
fn go_round(t: Nanos, d: Nanos) -> Nanos {
    if d <= 0 {
        return t;
    }
    let r = (t - GO_ZERO).rem_euclid(d);
    if r + r < d {
        t - r
    } else {
        t + (d - r)
    }
}

/// Go's `time.RFC3339Nano` in UTC: trailing zeros of the fraction dropped.
pub(crate) fn format_rfc3339_nano(t: Nanos) -> String {
    let secs = t.div_euclid(SECOND);
    let nanos = t.rem_euclid(SECOND);
    let dt = i64::try_from(secs)
        .ok()
        .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
        .unwrap_or_default();
    let mut out = dt.format("%Y-%m-%dT%H:%M:%S").to_string();
    if nanos > 0 {
        let frac = format!(".{nanos:09}");
        out.push_str(frac.trim_end_matches('0'));
    }
    out.push('Z');
    out
}

/// Go's `time.Duration.String`, for the configuration errors Bento prints.
pub(crate) fn go_duration_string(d: Nanos) -> String {
    fn frac(v: u128, div: u128) -> String {
        let (whole, rest) = (v / div, v % div);
        if rest == 0 {
            return whole.to_string();
        }
        let width = div.ilog10() as usize;
        let s = format!("{whole}.{rest:0width$}");
        s.trim_end_matches('0').to_string()
    }
    if d == 0 {
        return "0s".to_string();
    }
    let u = d.unsigned_abs();
    let sec = SECOND.unsigned_abs();
    let body = if u < sec {
        match u {
            0..=999 => format!("{u}ns"),
            1_000..=999_999 => format!("{}µs", frac(u, 1_000)),
            _ => format!("{}ms", frac(u, 1_000_000)),
        }
    } else {
        let (h, rest) = (u / (3600 * sec), u % (3600 * sec));
        let (m, s) = (rest / (60 * sec), rest % (60 * sec));
        let mut out = String::new();
        if h > 0 {
            out.push_str(&format!("{h}h"));
        }
        if h > 0 || m > 0 {
            out.push_str(&format!("{m}m"));
        }
        out.push_str(&format!("{}s", frac(s, sec)));
        out
    };
    if d < 0 {
        format!("-{body}")
    } else {
        body
    }
}

/// Bento's `getDuration`: an empty optional field is zero.
fn duration_field(
    obj: &serde_json::Map<String, JsonValue>,
    name: &str,
    required: bool,
) -> Result<Nanos, CoreError> {
    let raw = match obj.get(name) {
        Some(v) => v
            .as_str()
            .ok_or_else(|| CoreError::config(format!("field {name} must be a string")))?,
        None if required => return Err(CoreError::config(format!("field {name} is required"))),
        None => "",
    };
    if !required && raw.is_empty() {
        return Ok(0);
    }
    ekiben_bloblang::time_format::parse_go_duration_nanos(raw)
        .map(Nanos::from)
        .map_err(|e| {
            CoreError::config(format!(
                "failed to parse field '{name}' as duration: {}",
                crate::processor_cached::go_duration_error(raw, &e)
            ))
        })
}

impl SystemWindowBuffer {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let empty = serde_json::Map::new();
        let obj = conf.config.as_object().unwrap_or(&empty);
        let (ts_mapping, durations) = Self::parse(obj, &ctx.blobl_env)?;
        Ok(Self::new(ts_mapping, Arc::new(system_now), durations))
    }

    /// The checks Bento's constructor runs, in its order and with its texts.
    fn parse(
        obj: &serde_json::Map<String, JsonValue>,
        blobl_env: &ekiben_bloblang::Environment,
    ) -> Result<(Executor, [Nanos; 4]), CoreError> {
        let size = duration_field(obj, "size", true)?;
        if size > MAX_SIZE {
            return Err(CoreError::config(format!(
                "system_window: size '{}' is over the {} a pipeline window may hold",
                go_duration_string(size),
                go_duration_string(MAX_SIZE)
            )));
        }
        let slide = duration_field(obj, "slide", false)?;
        let d = go_duration_string;
        if slide >= size {
            return Err(CoreError::config(format!(
                "invalid window slide '{}' must be lower than the size '{}'",
                d(slide),
                d(size)
            )));
        }
        let offset = duration_field(obj, "offset", false)?;
        if offset >= size {
            return Err(CoreError::config(format!(
                "invalid offset '{}' must be lower than the size '{}'",
                d(offset),
                d(size)
            )));
        }
        if slide > 0 && offset >= slide {
            return Err(CoreError::config(format!(
                "invalid offset '{}' must be lower than the slide '{}'",
                d(offset),
                d(slide)
            )));
        }
        let allowed_lateness = duration_field(obj, "allowed_lateness", false)?;
        if allowed_lateness >= size {
            return Err(CoreError::config(format!(
                "invalid allowed_lateness '{}' must be lower than the size '{}'",
                d(allowed_lateness),
                d(size)
            )));
        }
        let mapping = match obj.get("timestamp_mapping") {
            None => "root = now()",
            Some(v) => v
                .as_str()
                .ok_or_else(|| CoreError::config("field timestamp_mapping must be a string"))?,
        };
        let ts_mapping = blobl_env
            .compile_mapping(mapping)
            .map_err(|e| CoreError::config(e.to_string()))?;
        Ok((ts_mapping, [size, slide, offset, allowed_lateness]))
    }

    fn new(ts_mapping: Executor, clock: Clock, durations: [Nanos; 4]) -> Self {
        let [size, slide, offset, allowed_lateness] = durations;
        let oldest_ts = clock();
        Self {
            ts_mapping,
            clock,
            size,
            slide,
            offset,
            allowed_lateness,
            state: Mutex::new(State {
                latest_flushed_window_end: GO_ZERO,
                oldest_ts,
                pending: Vec::new(),
                pending_bytes: 0,
            }),
            end_of_input: watch::channel(false).0,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// `(prevStart, prevEnd, start, end)` of the window the clock is in, as Bento's `nextSystemWindow`.
    fn next_system_window(&self) -> (Nanos, Nanos, Nanos, Nanos) {
        let now = (self.clock)();
        let epoch = if self.slide > 0 {
            self.slide
        } else {
            self.size
        };
        let mut start = go_round(now, epoch) + 1 + self.offset;
        if start > now {
            start -= epoch;
        }
        if self.slide > 0 {
            start += self.slide - self.size;
        }
        let end = start + self.size - 1;
        (start - epoch, end - epoch, start, end)
    }

    fn timestamp(&self, i: usize, batch: &[Part]) -> Result<Nanos, String> {
        let mut meta = std::collections::BTreeMap::new();
        let value = self
            .ts_mapping
            .map_part(i, &PartsView(batch), &mut meta, None)
            .map_err(|e| format!("timestamp mapping failed: {e}"))?;
        let value = match value {
            Some(Value::Bytes(b)) => Value::String(String::from_utf8_lossy(&b).into_owned()),
            Some(v) => v,
            None => {
                return Err("timestamp mapping failed: the mapping deleted the message".to_string())
            }
        };
        let ts = value.to_timestamp_coerced().map_err(|e| {
            format!("unable to parse result of timestamp mapping as timestamp: {e}")
        })?;
        Ok(Nanos::from(ts.timestamp()) * SECOND + Nanos::from(ts.timestamp_subsec_nanos()))
    }

    /// Bento's `WriteBatch`. On a mapping error the input's ack is answered with it.
    pub fn write_batch(&self, batch: Batch, ack: AckResponder) -> Result<(), CoreError> {
        let mut st = self.lock();
        let (prev_start, ..) = self.next_system_window();
        if st.latest_flushed_window_end < prev_start && st.oldest_ts < prev_start {
            let mut new_oldest = (self.clock)();
            let mut kept = Vec::with_capacity(st.pending.len());
            for p in st.pending.drain(..) {
                if p.ts < prev_start {
                    fire(&p.ack, None);
                    continue;
                }
                new_oldest = new_oldest.min(p.ts);
                kept.push(p);
            }
            st.pending_bytes = kept.iter().map(|p| p.part.as_bytes_ref().len()).sum();
            st.pending = kept;
            st.oldest_ts = new_oldest;
        }
        let root: SharedAck = Arc::new(Mutex::new(Some(ack)));
        let root_fire = root.clone();
        let combined = CombinedAcker::new(AckResponder::from_fn(move |e| fire(&root_fire, e)));
        let mut added = false;
        for i in 0..batch.len() {
            let ts = match self.timestamp(i, &batch) {
                Ok(ts) => ts,
                Err(e) => {
                    tracing::error!("Timestamp mapping failed for message: {e}");
                    fire(&root, Some(BatchError::new(batch.clone(), e.clone())));
                    return Err(CoreError::other(e));
                }
            };
            if ts <= st.latest_flushed_window_end {
                continue;
            }
            let bytes = batch[i].as_bytes_ref().len();
            if st.pending_bytes + bytes > MAX_PENDING_BYTES {
                let e = format!(
                    "system_window holds {} bytes, the most one buffer may hold is {MAX_PENDING_BYTES}",
                    st.pending_bytes
                );
                fire(&root, Some(BatchError::new(batch.clone(), e.clone())));
                return Err(CoreError::other(e));
            }
            added = true;
            st.pending_bytes += bytes;
            st.pending.push(TsMessage {
                ts,
                part: batch[i].clone(),
                ack: Arc::new(Mutex::new(Some(combined.derive()))),
            });
            st.oldest_ts = st.oldest_ts.min(ts);
        }
        if !added {
            fire(&root, None);
        }
        Ok(())
    }

    fn flush_window(&self, start: Nanos, end: Nanos) -> (Batch, AckResponder) {
        let mut st = self.lock();
        let next_start = start
            + if self.slide > 0 {
                self.slide
            } else {
                self.size
            };
        let stamp = format_rfc3339_nano(end);
        let mut out = Vec::new();
        let mut acks = Vec::new();
        let mut kept = Vec::with_capacity(st.pending.len());
        let mut new_oldest = (self.clock)();
        for p in st.pending.drain(..) {
            let flush = p.ts >= start && p.ts <= end;
            let preserve = p.ts >= next_start;
            if flush {
                let mut m = p.part.clone();
                m.meta_set_str("window_end_timestamp", stamp.clone());
                out.push(m);
                acks.push(p.ack.clone());
            }
            if preserve {
                new_oldest = new_oldest.min(p.ts);
                kept.push(p);
            } else if !flush {
                fire(&p.ack, None);
            }
        }
        st.pending_bytes = kept.iter().map(|p| p.part.as_bytes_ref().len()).sum();
        st.pending = kept;
        st.latest_flushed_window_end = end;
        st.oldest_ts = new_oldest;
        let ack = AckResponder::from_fn(move |err| {
            for a in &acks {
                fire(a, err.clone());
            }
        });
        (out, ack)
    }

    /// Nacks every held message so the input delivers it again on the next start.
    fn nack_pending(&self) {
        let pending = {
            let mut st = self.lock();
            st.pending_bytes = 0;
            std::mem::take(&mut st.pending)
        };
        for p in pending {
            let err = BatchError::new(vec![p.part], "message rejected as window did not complete");
            fire(&p.ack, Some(err));
        }
    }

    /// Bento's `ReadBatch`: the next window with messages once the clock has passed its end plus
    /// `allowed_lateness`; at the end of the input or a stop the held messages are nacked.
    pub async fn read_batch(
        &self,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(Batch, AckResponder), CoreError> {
        let (prev_start, prev_end, mut start, mut end) = self.next_system_window();
        if self.lock().latest_flushed_window_end < prev_start {
            let (batch, ack) = self.flush_window(prev_start, prev_end);
            if !batch.is_empty() {
                return Ok((batch, ack));
            }
        }
        let mut eoi = self.end_of_input.subscribe();
        loop {
            let wait = end - (self.clock)() + self.allowed_lateness;
            if wait > 0 {
                let nanos = u64::try_from(wait).unwrap_or(u64::MAX);
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_nanos(nanos)) => {}
                    _ = eoi.wait_for(|v| *v) => {
                        self.nack_pending();
                        return Err(CoreError::EndOfInput);
                    }
                    _ = shutdown.wait_for(|v| *v) => {
                        self.nack_pending();
                        return Err(CoreError::EndOfInput);
                    }
                }
            } else {
                tokio::task::yield_now().await;
            }
            let (batch, ack) = self.flush_window(start, end);
            if !batch.is_empty() {
                return Ok((batch, ack));
            }
            (_, _, start, end) = self.next_system_window();
        }
    }
}

#[async_trait]
impl Buffer for SystemWindowBuffer {
    async fn push(&self, tx: Transaction) -> Result<(), CoreError> {
        self.write_batch(tx.payload, tx.ack)
    }

    async fn pop(&self, ctx: &StreamContext) -> Result<Transaction, CoreError> {
        let (batch, ack) = self.read_batch(ctx.shutdown_rx.clone()).await?;
        Ok(Transaction::new(batch, ack))
    }

    fn end_of_input(&self) {
        self.end_of_input.send_replace(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Nanos = 60 * SECOND;
    const HOUR: Nanos = 60 * MIN;

    fn at(rfc3339: &str) -> Nanos {
        let t = chrono::DateTime::parse_from_rfc3339(rfc3339).expect("a time");
        Nanos::from(t.timestamp()) * SECOND + Nanos::from(t.timestamp_subsec_nanos())
    }

    fn unix(secs: i64, nanos: i64) -> Nanos {
        Nanos::from(secs) * SECOND + Nanos::from(nanos)
    }

    struct Fixture {
        now: Arc<Mutex<Nanos>>,
        w: SystemWindowBuffer,
    }

    impl Fixture {
        fn new(now: Nanos, durations: [Nanos; 4]) -> Self {
            let now = Arc::new(Mutex::new(now));
            let clock_now = now.clone();
            let clock: Clock = Arc::new(move || *clock_now.lock().expect("clock"));
            let mapping = ekiben_bloblang::Environment::new()
                .compile_mapping("root = this.ts")
                .expect("mapping");
            Self {
                now,
                w: SystemWindowBuffer::new(mapping, clock, durations),
            }
        }

        fn set(&self, t: Nanos) {
            *self.now.lock().expect("clock") = t;
        }

        fn write(&self, msgs: &[&str], ack: AckResponder) {
            let batch = msgs.iter().map(|m| Part::from_str(m)).collect();
            self.w.write_batch(batch, ack).expect("written");
        }

        fn pending(&self) -> Vec<String> {
            self.w
                .lock()
                .pending
                .iter()
                .map(|p| body(&p.part))
                .collect()
        }

        async fn read(&self) -> (Vec<String>, AckResponder) {
            let (_tx, rx) = watch::channel(false);
            let (batch, ack) = self.w.read_batch(rx).await.expect("a window");
            (batch.iter().map(body).collect(), ack)
        }
    }

    fn body(p: &Part) -> String {
        String::from_utf8_lossy(&p.as_bytes_ref()).into_owned()
    }

    fn counting_ack(calls: &Arc<Mutex<Vec<Option<String>>>>) -> AckResponder {
        let calls = calls.clone();
        AckResponder::from_fn(move |e| {
            calls.lock().expect("calls").push(e.map(|e| e.message));
        })
    }

    #[test]
    fn test_system_window_buffer_configs() {
        let env = ekiben_bloblang::Environment::new();
        let cases: [(&str, Option<&str>); 10] = [
            (r#"{"size":"60m"}"#, None),
            (r#"{"size":"24h"}"#, None),
            (
                r#"{"size":"25h"}"#,
                Some("size '25h0m0s' is over the 24h0m0s"),
            ),
            (r#"{}"#, Some("field size is required")),
            (r#"{"timestamp_mapping":"root =","size":"60m"}"#, Some("")),
            (
                r#"{"size":"60m","slide":"5m","offset":"1m","allowed_lateness":"2m"}"#,
                None,
            ),
            (
                r#"{"size":"60m","slide":"120m","offset":"1m","allowed_lateness":"2m"}"#,
                Some("invalid window slide '2h0m0s' must be lower than the size '1h0m0s'"),
            ),
            (
                r#"{"size":"60m","offset":"60m","allowed_lateness":"2m"}"#,
                Some("invalid offset '1h0m0s' must be lower than the size '1h0m0s'"),
            ),
            (
                r#"{"size":"60m","slide":"10m","offset":"10m","allowed_lateness":"2m"}"#,
                Some("invalid offset '10m0s' must be lower than the slide '10m0s'"),
            ),
            (
                r#"{"size":"60m","slide":"10m","allowed_lateness":"200m"}"#,
                Some("invalid allowed_lateness '3h20m0s' must be lower than the size '1h0m0s'"),
            ),
        ];
        for (conf, want) in cases {
            let obj: serde_json::Map<String, JsonValue> = serde_json::from_str(conf).expect("json");
            match (SystemWindowBuffer::parse(&obj, &env), want) {
                (Ok(_), None) => {}
                (Err(e), Some(text)) => assert!(e.to_string().contains(text), "{conf}: {e}"),
                (Ok(_), Some(text)) => panic!("{conf}: accepted, want {text}"),
                (Err(e), None) => panic!("{conf}: {e}"),
            }
        }
    }

    #[test]
    fn test_system_current_window_calc() {
        let cases = [
            (
                "2006-01-02T15:00:00Z",
                HOUR,
                0,
                0,
                "2006-01-02T13:00:00.000000001Z",
                "2006-01-02T14:00:00Z",
                "2006-01-02T14:00:00.000000001Z",
                "2006-01-02T15:00:00Z",
            ),
            (
                "2006-01-02T15:00:00.000000001Z",
                HOUR,
                0,
                0,
                "2006-01-02T14:00:00.000000001Z",
                "2006-01-02T15:00:00Z",
                "2006-01-02T15:00:00.000000001Z",
                "2006-01-02T16:00:00Z",
            ),
            (
                "2006-01-02T15:04:05.123456789Z",
                HOUR,
                0,
                0,
                "2006-01-02T14:00:00.000000001Z",
                "2006-01-02T15:00:00Z",
                "2006-01-02T15:00:00.000000001Z",
                "2006-01-02T16:00:00Z",
            ),
            (
                "2006-01-02T15:34:05.123456789Z",
                HOUR,
                0,
                0,
                "2006-01-02T14:00:00.000000001Z",
                "2006-01-02T15:00:00Z",
                "2006-01-02T15:00:00.000000001Z",
                "2006-01-02T16:00:00Z",
            ),
            (
                "2006-01-02T00:04:05.123456789Z",
                HOUR,
                0,
                0,
                "2006-01-01T23:00:00.000000001Z",
                "2006-01-02T00:00:00Z",
                "2006-01-02T00:00:00.000000001Z",
                "2006-01-02T01:00:00Z",
            ),
            (
                "2006-01-02T15:04:05.123456789Z",
                HOUR,
                10 * MIN,
                0,
                "2006-01-02T14:00:00.000000001Z",
                "2006-01-02T15:00:00Z",
                "2006-01-02T14:10:00.000000001Z",
                "2006-01-02T15:10:00Z",
            ),
            (
                "2006-01-02T15:04:05.123456789Z",
                HOUR,
                0,
                30 * MIN,
                "2006-01-02T13:30:00.000000001Z",
                "2006-01-02T14:30:00Z",
                "2006-01-02T14:30:00.000000001Z",
                "2006-01-02T15:30:00Z",
            ),
            (
                "2006-01-02T15:04:05.123456789Z",
                HOUR,
                10 * MIN,
                5 * MIN,
                "2006-01-02T13:55:00.000000001Z",
                "2006-01-02T14:55:00Z",
                "2006-01-02T14:05:00.000000001Z",
                "2006-01-02T15:05:00Z",
            ),
            (
                "2006-01-02T15:09:59.123456789Z",
                HOUR,
                10 * MIN,
                5 * MIN,
                "2006-01-02T14:05:00.000000001Z",
                "2006-01-02T15:05:00Z",
                "2006-01-02T14:15:00.000000001Z",
                "2006-01-02T15:15:00Z",
            ),
        ];
        for (now, size, slide, offset, prev_start, prev_end, start, end) in cases {
            let f = Fixture::new(at(now), [size, slide, offset, 0]);
            let (ps, pe, s, e) = f.w.next_system_window();
            let got = [ps, pe, s, e].map(format_rfc3339_nano);
            assert_eq!(got, [prev_start, prev_end, start, end], "now {now}");
        }
    }

    #[test]
    fn test_system_window_write_purge() {
        let f = Fixture::new(unix(10, 1), [SECOND, 0, 0, 0]);
        f.write(
            &[
                r#"{"id":"1","ts":7.999999999}"#,
                r#"{"id":"2","ts":8.5}"#,
                r#"{"id":"3","ts":9.5}"#,
                r#"{"id":"4","ts":10.5}"#,
            ],
            AckResponder::noop(),
        );
        assert_eq!(f.pending().len(), 4);
        f.write(
            &[
                r#"{"id":"5","ts":10.6}"#,
                r#"{"id":"6","ts":10.7}"#,
                r#"{"id":"7","ts":10.8}"#,
                r#"{"id":"8","ts":10.9}"#,
            ],
            AckResponder::noop(),
        );
        assert_eq!(f.pending().len(), 6);
    }

    #[tokio::test]
    async fn test_system_window_creation() {
        let f = Fixture::new(unix(10, 1), [SECOND, 0, 0, 0]);
        f.write(
            &[
                r#"{"id":"1","ts":7.999999999}"#,
                r#"{"id":"2","ts":8.5}"#,
                r#"{"id":"3","ts":9.5}"#,
                r#"{"id":"4","ts":10.5}"#,
            ],
            AckResponder::noop(),
        );
        assert_eq!(f.pending().len(), 4);
        assert_eq!(f.read().await.0, [r#"{"id":"3","ts":9.5}"#]);
        assert_eq!(f.pending().len(), 1);
        assert_eq!(
            format_rfc3339_nano(f.w.lock().latest_flushed_window_end),
            "1970-01-01T00:00:10Z"
        );

        f.set(unix(10, 999_999_100));
        assert_eq!(f.read().await.0, [r#"{"id":"4","ts":10.5}"#]);
        assert!(f.pending().is_empty());
        assert_eq!(
            format_rfc3339_nano(f.w.lock().latest_flushed_window_end),
            "1970-01-01T00:00:11Z"
        );

        f.set(unix(11, 999_999_100));
        let (_tx, rx) = watch::channel(false);
        let small_wait = tokio::time::timeout(Duration::from_millis(50), f.w.read_batch(rx)).await;
        assert!(small_wait.is_err(), "no window holds a message");
        assert_eq!(
            format_rfc3339_nano(f.w.lock().latest_flushed_window_end),
            "1970-01-01T00:00:12Z"
        );

        f.write(
            &[
                r#"{"id":"5","ts":8.1}"#,
                r#"{"id":"6","ts":9.999999999}"#,
                r#"{"id":"7","ts":10}"#,
                r#"{"id":"8","ts":11.999999999}"#,
                r#"{"id":"9","ts":12.1}"#,
                r#"{"id":"10","ts":13}"#,
            ],
            AckResponder::noop(),
        );
        assert_eq!(
            f.pending(),
            [r#"{"id":"9","ts":12.1}"#, r#"{"id":"10","ts":13}"#]
        );
    }

    #[tokio::test]
    async fn test_system_window_creation_sliding() {
        let f = Fixture::new(unix(10, 0), [SECOND, 500_000_000, 0, 0]);
        f.w.lock().latest_flushed_window_end = unix(9, 500_000_000);
        let ids = [
            "9.85", "9.9", "10.15", "10.3", "10.5", "10.7", "10.9", "11.1", "11.35", "11.52",
            "11.8",
        ];
        let msgs: Vec<String> = ids
            .iter()
            .enumerate()
            .map(|(i, ts)| format!(r#"{{"id":"{}","ts":{ts}}}"#, i + 1))
            .collect();
        f.write(
            &msgs.iter().map(String::as_str).collect::<Vec<_>>(),
            AckResponder::noop(),
        );
        assert_eq!(f.pending().len(), 11);
        let window = |from: usize, to: usize| msgs[from - 1..to].to_vec();
        assert_eq!(f.read().await.0, window(1, 2));
        f.set(unix(10, 500_000_000));
        assert_eq!(f.read().await.0, window(1, 5));
        f.set(unix(11, 0));
        assert_eq!(f.read().await.0, window(3, 7));
        f.set(unix(11, 500_000_000));
        assert_eq!(f.read().await.0, window(6, 9));
        f.set(unix(12, 0));
        assert_eq!(f.read().await.0, window(8, 11));
    }

    #[tokio::test]
    async fn test_system_window_ack_one_to_many() {
        let f = Fixture::new(unix(10, 1), [SECOND, 0, 0, 0]);
        let calls = Arc::new(Mutex::new(Vec::new()));
        f.write(
            &[
                r#"{"id":"1","ts":9.5}"#,
                r#"{"id":"2","ts":10.5}"#,
                r#"{"id":"3","ts":11.5}"#,
            ],
            counting_ack(&calls),
        );
        let (b1, a1) = f.read().await;
        assert_eq!(b1, [r#"{"id":"1","ts":9.5}"#]);
        f.set(unix(11, 0));
        let (b2, a2) = f.read().await;
        assert_eq!(b2, [r#"{"id":"2","ts":10.5}"#]);
        f.set(unix(12, 0));
        let (b3, a3) = f.read().await;
        assert_eq!(b3, [r#"{"id":"3","ts":11.5}"#]);
        assert!(calls.lock().expect("calls").is_empty());
        a1.ack();
        assert!(calls.lock().expect("calls").is_empty());
        a2.nack(BatchError::new(Vec::new(), "custom error"));
        assert!(calls.lock().expect("calls").is_empty());
        a3.ack();
        assert_eq!(
            *calls.lock().expect("calls"),
            [Some("custom error".to_string())]
        );
    }

    #[tokio::test]
    async fn test_system_window_ack_many_to_one() {
        let f = Fixture::new(unix(10, 1), [SECOND, 0, 0, 0]);
        let calls = Arc::new(Mutex::new(Vec::new()));
        for msg in [
            r#"{"id":"1","ts":9.5}"#,
            r#"{"id":"2","ts":9.6}"#,
            r#"{"id":"3","ts":9.7}"#,
        ] {
            f.write(&[msg], counting_ack(&calls));
        }
        let (batch, ack) = f.read().await;
        assert_eq!(
            batch,
            [
                r#"{"id":"1","ts":9.5}"#,
                r#"{"id":"2","ts":9.6}"#,
                r#"{"id":"3","ts":9.7}"#
            ]
        );
        assert!(calls.lock().expect("calls").is_empty());
        ack.nack(BatchError::new(Vec::new(), "custom error"));
        assert_eq!(
            *calls.lock().expect("calls"),
            vec![Some("custom error".to_string()); 3]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_system_window_parallel_read_and_writes() {
        let f = Arc::new(Fixture::new(unix(10, 500_000_000), [SECOND, 0, 0, 0]));
        let writer = f.clone();
        let writes = tokio::spawn(async move {
            for i in 0..1000 {
                writer.write(
                    &[&format!(r#"{{"id":"{i}","ts":10.5}}"#)],
                    AckResponder::noop(),
                );
            }
        });
        let reader = f.clone();
        let reads = tokio::spawn(async move {
            reader.set(unix(11, 0));
            reader.read().await.0.len()
        });
        writes.await.expect("writes");
        assert!(reads.await.expect("reads") > 0);
    }

    #[tokio::test]
    async fn test_system_window_ownership() {
        let f = Fixture::new(unix(10, 1), [SECOND, 0, 0, 0]);
        let original = Part::from_str(r#"{"hello":"world","ts":10}"#);
        f.w.write_batch(vec![original.clone()], AckResponder::noop())
            .expect("written");
        let (_tx, rx) = watch::channel(false);
        let (mut out, ack) = f.w.read_batch(rx).await.expect("a window");
        ack.ack();
        out[0].set_bytes(bytes::Bytes::from_static(b"changed"));
        assert_eq!(body(&original), r#"{"hello":"world","ts":10}"#);
        assert_eq!(
            out[0].meta_get_str("window_end_timestamp"),
            "1970-01-01T00:00:10Z"
        );
    }

    #[tokio::test]
    async fn end_of_input_nacks_what_the_open_window_holds() {
        let f = Fixture::new(unix(10, 1), [HOUR, 0, 0, 0]);
        let calls = Arc::new(Mutex::new(Vec::new()));
        f.write(&[r#"{"id":"1","ts":10.5}"#], counting_ack(&calls));
        f.w.end_of_input();
        let (_tx, rx) = watch::channel(false);
        assert!(matches!(
            f.w.read_batch(rx).await,
            Err(CoreError::EndOfInput)
        ));
        assert_eq!(
            *calls.lock().expect("calls"),
            [Some(
                "message rejected as window did not complete".to_string()
            )]
        );
    }

    #[test]
    fn a_failed_timestamp_mapping_nacks_the_input() {
        let f = Fixture::new(unix(10, 1), [SECOND, 0, 0, 0]);
        let calls = Arc::new(Mutex::new(Vec::new()));
        let err =
            f.w.write_batch(
                vec![Part::from_str(r#"{"ts":"yesterday"}"#)],
                counting_ack(&calls),
            )
            .expect_err("refused");
        assert!(
            err.to_string()
                .contains("unable to parse result of timestamp mapping"),
            "{err}"
        );
        assert_eq!(calls.lock().expect("calls").len(), 1);
    }

    #[test]
    fn a_write_past_the_byte_cap_is_nacked() {
        let f = Fixture::new(unix(10, 1), [SECOND, 0, 0, 0]);
        f.w.lock().pending_bytes = MAX_PENDING_BYTES - 5;
        let calls = Arc::new(Mutex::new(Vec::new()));
        let err =
            f.w.write_batch(vec![Part::from_str(r#"{"ts":10.5}"#)], counting_ack(&calls))
                .expect_err("over the cap");
        assert!(
            err.to_string().contains("the most one buffer may hold"),
            "{err}"
        );
        assert_eq!(calls.lock().expect("calls").len(), 1);
        assert!(f.pending().is_empty());
    }

    #[test]
    fn go_duration_strings() {
        assert_eq!(go_duration_string(HOUR), "1h0m0s");
        assert_eq!(go_duration_string(90 * SECOND + 500_000_000), "1m30.5s");
        assert_eq!(go_duration_string(1_500_000), "1.5ms");
        assert_eq!(go_duration_string(0), "0s");
    }
}
