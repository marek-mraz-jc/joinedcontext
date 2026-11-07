// SPDX-License-Identifier: EUPL-1.2
//! Adaptive summary budget (T-4680): monitors system resources (cgroup v2 memory,
//! CPU pressure, `/proc/meminfo`) to categorize system state into Normal, Busy, or
//! Tight, enforcing row and byte limits as well as concurrency bounds.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// The adaptive operating state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum State {
    Normal,
    Busy,
    Tight,
}

impl State {
    /// String representation for metrics and logging.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            State::Normal => "normal",
            State::Busy => "busy",
            State::Tight => "tight",
        }
    }
}

/// The resource budget allocated for a summary execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Budget {
    pub max_rows: usize,
    pub max_bytes: usize,
}

fn parse_env_f64(name: &str, default: f64) -> f64 {
    f64_or(std::env::var(name).ok().as_deref(), default)
}

fn parse_env_usize(name: &str, default: usize, min: usize, max: usize) -> usize {
    usize_or(std::env::var(name).ok().as_deref(), default, min, max)
}

/// `raw` as a number, or `default` when it is unset or no number.
fn f64_or(raw: Option<&str>, default: f64) -> f64 {
    raw.and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// `raw` as a count clamped to `min..=max`, or `default` when it is unset or no count.
fn usize_or(raw: Option<&str>, default: usize, min: usize, max: usize) -> usize {
    raw.and_then(|v| v.parse().ok())
        .map_or(default, |n: usize| n.clamp(min, max))
}

/// `ANTARES_AREA_MAX_KM2`: the largest area `area_overview` searches (T-5279).
pub(crate) static AREA_MAX_KM2: LazyLock<f64> =
    LazyLock::new(|| area_max(std::env::var(crate::AREA_MAX_KM2_ENV).ok().as_deref()));

/// A positive km² bound, or the default: `NaN` or `-1` would lift the bound.
fn area_max(raw: Option<&str>) -> f64 {
    Some(f64_or(raw, crate::area::DEFAULT_MAX_KM2))
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(crate::area::DEFAULT_MAX_KM2)
}
pub(crate) static BUSY_MEMORY_PCT: LazyLock<f64> =
    LazyLock::new(|| parse_env_f64(crate::SUMMARY_BUSY_MEMORY_PCT_ENV, 70.0));
pub(crate) static TIGHT_MEMORY_PCT: LazyLock<f64> =
    LazyLock::new(|| parse_env_f64(crate::SUMMARY_TIGHT_MEMORY_PCT_ENV, 85.0));
pub(crate) static BUSY_CPU_AVG10: LazyLock<f64> =
    LazyLock::new(|| parse_env_f64(crate::SUMMARY_BUSY_CPU_AVG10_ENV, 30.0));
pub(crate) static TIGHT_CPU_AVG10: LazyLock<f64> =
    LazyLock::new(|| parse_env_f64(crate::SUMMARY_TIGHT_CPU_AVG10_ENV, 60.0));
pub(crate) static BUSY_NODE_MIB: LazyLock<u64> = LazyLock::new(|| {
    u64::try_from(parse_env_usize(
        crate::SUMMARY_BUSY_NODE_MIB_ENV,
        1536,
        1,
        usize::MAX,
    ))
    .unwrap_or(u64::MAX)
});
pub(crate) static TIGHT_NODE_MIB: LazyLock<u64> = LazyLock::new(|| {
    u64::try_from(parse_env_usize(
        crate::SUMMARY_TIGHT_NODE_MIB_ENV,
        819,
        1,
        usize::MAX,
    ))
    .unwrap_or(u64::MAX)
});

pub(crate) static SUMMARY_ROWS: LazyLock<usize> =
    LazyLock::new(|| parse_env_usize(crate::SUMMARY_ROWS_ENV, 1000, 1, 5000));
pub(crate) static SUMMARY_MAX_BYTES: LazyLock<usize> = LazyLock::new(|| {
    parse_env_usize(
        crate::SUMMARY_MAX_BYTES_ENV,
        32 * 1024 * 1024,
        1024,
        1024 * 1024 * 1024,
    )
});
pub(crate) static SUMMARY_CONCURRENCY: LazyLock<usize> =
    LazyLock::new(|| parse_env_usize(crate::SUMMARY_CONCURRENCY_ENV, 4, 1, 256));

pub(crate) static CONCURRENCY: LazyLock<tokio::sync::Semaphore> =
    LazyLock::new(|| tokio::sync::Semaphore::new(*SUMMARY_CONCURRENCY));

/// Compute budget limits for a given state.
pub(crate) fn budget_for(state: State) -> Budget {
    match state {
        State::Normal => Budget {
            max_rows: *SUMMARY_ROWS,
            max_bytes: *SUMMARY_MAX_BYTES,
        },
        State::Busy => Budget {
            max_rows: 250,
            max_bytes: 8 * 1024 * 1024,
        },
        State::Tight => Budget {
            max_rows: 0,
            max_bytes: 0,
        },
    }
}

/// The pod's memory use in whole percent of its limit; `None` without a limit
/// (`memory.max` reads `max`, which is no number) or with a zero one.
fn read_memory_pct(cgroup_root: &Path) -> Option<f64> {
    let read = |file: &str| -> Option<u64> {
        std::fs::read_to_string(cgroup_root.join(file))
            .ok()?
            .trim()
            .parse()
            .ok()
    };
    let pct = read("memory.current")?
        .saturating_mul(100)
        .checked_div(read("memory.max")?)?;
    Some(f64::from(u32::try_from(pct).unwrap_or(u32::MAX)))
}

/// The `some avg10` value of the cgroup's `cpu.pressure`.
fn read_cpu_pressure(cgroup_root: &Path) -> Option<f64> {
    let content = std::fs::read_to_string(cgroup_root.join("cpu.pressure")).ok()?;
    let line = content.lines().find(|l| l.starts_with("some "))?;
    line.split_once("avg10=")?
        .1
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// `MemAvailable` of `/proc/meminfo`, in MiB.
fn read_mem_available(proc_root: &Path) -> Option<u64> {
    let content = std::fs::read_to_string(proc_root.join("meminfo")).ok()?;
    let line = content.lines().find(|l| l.starts_with("MemAvailable:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024)
}

static WARNED: AtomicBool = AtomicBool::new(false);

/// Evaluate state against directories.
pub(crate) fn evaluate_state(cgroup_root: &Path, proc_root: &Path) -> State {
    let mem_pct = read_memory_pct(cgroup_root);
    let cpu_avg10 = read_cpu_pressure(cgroup_root);
    let mem_avail_mib = read_mem_available(proc_root);

    if (mem_pct.is_none() || cpu_avg10.is_none() || mem_avail_mib.is_none())
        && !WARNED.swap(true, Ordering::Relaxed)
    {
        tracing::warn!("cgroup v2 or proc files not available or unparsable; unknown signals treated as normal");
    }

    let tight_mem = mem_pct.is_some_and(|p| p > *TIGHT_MEMORY_PCT);
    let tight_cpu = cpu_avg10.is_some_and(|c| c > *TIGHT_CPU_AVG10);
    let tight_node = mem_avail_mib.is_some_and(|m| m < *TIGHT_NODE_MIB);

    if tight_mem || tight_cpu || tight_node {
        return State::Tight;
    }

    let normal_mem = mem_pct.is_none_or(|p| p < *BUSY_MEMORY_PCT);
    let normal_cpu = cpu_avg10.is_none_or(|c| c < *BUSY_CPU_AVG10);
    let normal_node = mem_avail_mib.is_none_or(|m| m > *BUSY_NODE_MIB);

    if normal_mem && normal_cpu && normal_node {
        State::Normal
    } else {
        State::Busy
    }
}

static STATE_CACHE: Mutex<Option<(Instant, State)>> = Mutex::new(None);

/// Return the current system state, cached for 1 second.
pub(crate) fn current_state() -> State {
    let now = Instant::now();
    // a poisoned cache still holds a state or none: read past the poison
    let mut cache = STATE_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match *cache {
        Some((at, state)) if now.duration_since(at) < Duration::from_secs(1) => state,
        _ => {
            let state = evaluate_state(Path::new("/sys/fs/cgroup"), Path::new("/proc"));
            *cache = Some((now, state));
            state
        }
    }
}

/// Acquire semaphore permits based on state. Normal takes 1, Busy/Tight takes all.
pub(crate) async fn acquire_permit(
    semaphore: &tokio::sync::Semaphore,
    total_permits: usize,
    state: State,
    wait: Duration,
) -> Option<tokio::sync::SemaphorePermit<'_>> {
    let permits = match state {
        State::Normal => 1u32,
        State::Busy | State::Tight => u32::try_from(total_permits).unwrap_or(u32::MAX),
    };
    tokio::time::timeout(wait, semaphore.acquire_many(permits))
        .await
        .ok()
        .and_then(Result::ok)
}

/// How long a summary waits for its permits before it is answered "busy".
pub(crate) const PERMIT_WAIT: Duration = Duration::from_secs(2);

/// A single row larger than this is skipped and counted, never loaded.
pub(crate) const MAX_ROW_BYTES: usize = 1024 * 1024;

/// What the loader does with the next row under the byte budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Admit {
    /// Keep it; it costs this many bytes.
    Keep(usize),
    /// Over [`MAX_ROW_BYTES`] alone: skip and count it.
    Skip,
    /// It would pass the budget: stop loading.
    Full,
}

/// The byte budget: `used` bytes are kept, `max` may be.
pub(crate) fn admit(row: &serde_json::Value, used: usize, max: usize) -> Admit {
    let size = serde_json::to_vec(row).map_or(usize::MAX, |b| b.len());
    if size > MAX_ROW_BYTES {
        Admit::Skip
    } else if used.saturating_add(size) > max {
        Admit::Full
    } else {
        Admit::Keep(size)
    }
}

/// No permit came within [`PERMIT_WAIT`]: the summary is answered busy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Busy;

/// The rows a summary loaded and what stopped it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Loaded {
    pub rows: Vec<serde_json::Value>,
    pub bytes: usize,
    pub skipped: usize,
    /// The broker's count, from the first page.
    pub total: Option<u64>,
    pub reason: crate::summary::BasisReason,
}

/// A summary's rows, page by page within `allowed`, once `state`'s permits are
/// held: `fetch(limit, offset)` answers the broker's count and one page. A
/// Tight state asks one empty page, for the count alone.
pub(crate) async fn load<E, F, Fut>(
    semaphore: &tokio::sync::Semaphore,
    total_permits: usize,
    state: State,
    allowed: Budget,
    mut fetch: F,
) -> Result<Loaded, E>
where
    E: From<Busy>,
    F: FnMut(u64, u64) -> Fut,
    Fut: std::future::Future<Output = Result<(Option<u64>, serde_json::Value), E>>,
{
    use crate::summary::BasisReason;
    let Some(_permit) = acquire_permit(semaphore, total_permits, state, PERMIT_WAIT).await else {
        return Err(Busy.into());
    };
    let mut out = Loaded {
        rows: Vec::new(),
        bytes: 0,
        skipped: 0,
        total: None,
        reason: if state == State::Tight {
            BasisReason::Tight
        } else {
            BasisReason::All
        },
    };
    let mut offset = 0u64;
    loop {
        let want = allowed.max_rows.saturating_sub(out.rows.len()).min(1000);
        let (count, body) = fetch(u64::try_from(want).unwrap_or(1000), offset).await?;
        if offset == 0 {
            out.total = count;
        }
        let page = match body {
            serde_json::Value::Array(page) => page,
            _ => Vec::new(),
        };
        let got = page.len();
        for row in page.into_iter().take(want) {
            match admit(&row, out.bytes, allowed.max_bytes) {
                Admit::Keep(size) => {
                    out.bytes = out.bytes.saturating_add(size);
                    out.rows.push(row);
                }
                Admit::Skip => out.skipped = out.skipped.saturating_add(1),
                Admit::Full => {
                    out.reason = BasisReason::MemoryLimit;
                    return Ok(out);
                }
            }
        }
        if want == 0 || got < want {
            return Ok(out);
        }
        if out.rows.len() >= allowed.max_rows {
            let seen =
                u64::try_from(out.rows.len().saturating_add(out.skipped)).unwrap_or(u64::MAX);
            if out.total.is_none_or(|t| t > seen) {
                out.reason = if state == State::Busy {
                    BasisReason::Busy
                } else {
                    BasisReason::RowLimit
                };
            }
            return Ok(out);
        }
        offset = offset.saturating_add(u64::try_from(got).unwrap_or(u64::MAX));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_area_bound_is_a_positive_number_or_the_default() {
        assert_eq!(area_max(Some("25")), 25.0);
        for raw in [
            None,
            Some("NaN"),
            Some("-1"),
            Some("0"),
            Some("inf"),
            Some("x"),
        ] {
            assert_eq!(area_max(raw), crate::area::DEFAULT_MAX_KM2, "{raw:?}");
        }
    }

    fn temp_test_dir(name: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        let base =
            std::env::temp_dir().join(format!("antares_budget_test_{name}_{}", std::process::id()));
        let cgroup = base.join("cgroup");
        let proc = base.join("proc");
        let _ = std::fs::remove_dir_all(&base);
        assert!(std::fs::create_dir_all(&cgroup).is_ok());
        assert!(std::fs::create_dir_all(&proc).is_ok());
        (base, cgroup, proc)
    }

    #[test]
    fn missing_or_unparsable_files_default_to_normal() {
        let (base, cgroup, proc) = temp_test_dir("missing");
        let state = evaluate_state(&cgroup, &proc);
        assert_eq!(state, State::Normal);

        assert!(std::fs::write(cgroup.join("memory.current"), "bad").is_ok());
        assert!(std::fs::write(cgroup.join("memory.max"), "bad").is_ok());
        assert!(std::fs::write(cgroup.join("cpu.pressure"), "some bad").is_ok());
        assert!(std::fs::write(proc.join("meminfo"), "MemAvailable: bad kB").is_ok());

        let state2 = evaluate_state(&cgroup, &proc);
        assert_eq!(state2, State::Normal);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn threshold_edges_evaluate_accurately() {
        let (base, cgroup, proc) = temp_test_dir("edges");

        // Normal: memory < 70%, cpu < 30, mem_avail > 1536 MiB (1536 * 1024 kB)
        assert!(std::fs::write(cgroup.join("memory.current"), "6000").is_ok());
        assert!(std::fs::write(cgroup.join("memory.max"), "10000").is_ok()); // 60%
        assert!(std::fs::write(
            cgroup.join("cpu.pressure"),
            "some avg10=20.00 avg60=0.00 avg300=0.00 total=0\n",
        )
        .is_ok());
        assert!(std::fs::write(proc.join("meminfo"), "MemAvailable: 2097152 kB\n").is_ok()); // 2048 MiB
        assert_eq!(evaluate_state(&cgroup, &proc), State::Normal);

        // Busy via memory (75%)
        assert!(std::fs::write(cgroup.join("memory.current"), "7500").is_ok());
        assert_eq!(evaluate_state(&cgroup, &proc), State::Busy);

        // Tight via memory (90%)
        assert!(std::fs::write(cgroup.join("memory.current"), "9000").is_ok());
        assert_eq!(evaluate_state(&cgroup, &proc), State::Tight);

        // Reset memory to 60%, test CPU Busy (35)
        assert!(std::fs::write(cgroup.join("memory.current"), "6000").is_ok());
        assert!(std::fs::write(
            cgroup.join("cpu.pressure"),
            "some avg10=35.00 avg60=0.00 avg300=0.00 total=0\n",
        )
        .is_ok());
        assert_eq!(evaluate_state(&cgroup, &proc), State::Busy);

        // Tight via CPU (65)
        assert!(std::fs::write(
            cgroup.join("cpu.pressure"),
            "some avg10=65.00 avg60=0.00 avg300=0.00 total=0\n",
        )
        .is_ok());
        assert_eq!(evaluate_state(&cgroup, &proc), State::Tight);

        // Reset CPU to 10, test MemAvailable Busy (1000 MiB: 1024000 kB)
        assert!(std::fs::write(
            cgroup.join("cpu.pressure"),
            "some avg10=10.00 avg60=0.00 avg300=0.00 total=0\n",
        )
        .is_ok());
        assert!(std::fs::write(proc.join("meminfo"), "MemAvailable: 1024000 kB\n").is_ok());
        assert_eq!(evaluate_state(&cgroup, &proc), State::Busy);

        // Tight via MemAvailable (700 MiB: 716800 kB < 819 MiB)
        assert!(std::fs::write(proc.join("meminfo"), "MemAvailable: 716800 kB\n").is_ok());
        assert_eq!(evaluate_state(&cgroup, &proc), State::Tight);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn budget_for_each_state_is_correct() {
        let normal = budget_for(State::Normal);
        assert_eq!(normal.max_rows, *SUMMARY_ROWS);
        assert_eq!(normal.max_bytes, *SUMMARY_MAX_BYTES);

        let busy = budget_for(State::Busy);
        assert_eq!(busy.max_rows, 250);
        assert_eq!(busy.max_bytes, 8 * 1024 * 1024);

        let tight = budget_for(State::Tight);
        assert_eq!(tight.max_rows, 0);
        assert_eq!(tight.max_bytes, 0);
    }

    #[tokio::test]
    async fn concurrency_semaphore_busy_times_out_when_permits_held() {
        let sem = tokio::sync::Semaphore::new(4);
        let wait = Duration::from_millis(50);

        // Normal acquires 1 permit
        let p1 = acquire_permit(&sem, 4, State::Normal, wait).await;
        assert!(p1.is_some());

        // Busy wants 4 permits, only 3 available -> times out
        let busy_fail = acquire_permit(&sem, 4, State::Busy, wait).await;
        assert!(busy_fail.is_none());

        drop(p1);
        // All 4 permits free -> Busy succeeds
        let busy_ok = acquire_permit(&sem, 4, State::Busy, wait).await;
        assert!(busy_ok.is_some());
    }

    #[test]
    fn the_byte_budget_stops_at_its_limit_and_skips_a_huge_row() {
        let row = serde_json::json!({"id": "urn:ngsi-ld:Bridge:1", "type": "Bridge"});
        let size = serde_json::to_vec(&row).map_or(0, |b| b.len());
        assert_eq!(admit(&row, 0, size), Admit::Keep(size));
        assert_eq!(admit(&row, 1, size), Admit::Full);
        assert_eq!(admit(&row, 0, 0), Admit::Full);
        let huge = serde_json::json!({"text": "x".repeat(2 * 1024 * 1024)});
        assert_eq!(admit(&huge, 0, usize::MAX), Admit::Skip);
        let (mut used, mut kept, mut skipped) = (0usize, 0usize, 0usize);
        let twice_size = size.saturating_mul(2);
        for r in [row.clone(), huge, row.clone(), row] {
            match admit(&r, used, twice_size) {
                Admit::Keep(n) => {
                    used = used.saturating_add(n);
                    kept = kept.saturating_add(1);
                }
                Admit::Skip => skipped = skipped.saturating_add(1),
                Admit::Full => break,
            }
        }
        assert_eq!((kept, skipped, used), (2, 1, twice_size));
    }

    #[test]
    fn a_state_names_itself_for_the_metric() {
        let names = [State::Normal, State::Busy, State::Tight].map(State::as_str);
        assert_eq!(names, ["normal", "busy", "tight"]);
    }

    #[test]
    fn an_env_value_counts_only_when_it_is_a_number_and_is_clamped() {
        assert!((f64_or(Some("12.5"), 70.0) - 12.5).abs() < f64::EPSILON);
        assert!((f64_or(Some("lots"), 70.0) - 70.0).abs() < f64::EPSILON);
        assert!((f64_or(None, 70.0) - 70.0).abs() < f64::EPSILON);
        assert_eq!(usize_or(Some("9000"), 1000, 1, 5000), 5000);
        assert_eq!(usize_or(Some("0"), 1000, 1, 5000), 1);
        assert_eq!(usize_or(Some("-3"), 1000, 1, 5000), 1000);
        assert_eq!(usize_or(None, 4, 1, 256), 4);
    }

    #[test]
    fn a_pod_without_a_memory_limit_or_with_a_zero_one_reads_no_percentage() {
        let (base, cgroup, _) = temp_test_dir("limits");
        assert!(std::fs::write(cgroup.join("memory.current"), "6000\n").is_ok());
        assert!(std::fs::write(cgroup.join("memory.max"), "max\n").is_ok());
        assert_eq!(read_memory_pct(&cgroup), None);
        assert!(std::fs::write(cgroup.join("memory.max"), "0\n").is_ok());
        assert_eq!(read_memory_pct(&cgroup), None);
        assert!(std::fs::write(cgroup.join("memory.max"), "8000\n").is_ok());
        assert_eq!(read_memory_pct(&cgroup), Some(75.0));
        // a limit past u32 percent still reads whole
        assert!(std::fs::write(cgroup.join("memory.current"), u64::MAX.to_string()).is_ok());
        assert!(std::fs::write(cgroup.join("memory.max"), "1").is_ok());
        assert_eq!(read_memory_pct(&cgroup), Some(f64::from(u32::MAX)));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn each_signal_alone_missing_counts_as_normal_for_that_signal() {
        let (base, cgroup, proc) = temp_test_dir("partial");
        assert!(std::fs::write(cgroup.join("memory.current"), "9000").is_ok());
        assert!(std::fs::write(cgroup.join("memory.max"), "10000").is_ok());
        // memory alone, tight; no cpu.pressure and no meminfo
        assert_eq!(evaluate_state(&cgroup, &proc), State::Tight);
        assert!(std::fs::write(cgroup.join("memory.current"), "1000").is_ok());
        assert_eq!(evaluate_state(&cgroup, &proc), State::Normal);
        // a pressure file without a `some` line, or without avg10
        assert!(std::fs::write(cgroup.join("cpu.pressure"), "full avg10=99.00\n").is_ok());
        assert_eq!(read_cpu_pressure(&cgroup), None);
        assert!(std::fs::write(cgroup.join("cpu.pressure"), "some avg60=99.00\n").is_ok());
        assert_eq!(read_cpu_pressure(&cgroup), None);
        assert!(std::fs::write(cgroup.join("cpu.pressure"), "some avg10=\n").is_ok());
        assert_eq!(read_cpu_pressure(&cgroup), None);
        assert!(std::fs::write(cgroup.join("cpu.pressure"), "some avg10=70.00 avg60=1\n").is_ok());
        // memory and cpu, no meminfo
        assert_eq!(evaluate_state(&cgroup, &proc), State::Tight);
        assert!(std::fs::write(proc.join("meminfo"), "MemTotal: 1 kB\n").is_ok());
        assert_eq!(read_mem_available(&proc), None);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_state_is_read_once_a_second() {
        let first = current_state();
        assert_eq!(current_state(), first);
    }

    use crate::summary::BasisReason;
    use serde_json::{json, Value};

    /// One page as the fake broker answers it.
    type Page = std::future::Ready<Result<(Option<u64>, Value), Busy>>;

    /// A fake broker over `rows`: the count on every page, `limit` rows from `offset`.
    fn pages(rows: Vec<Value>, count: Option<u64>) -> impl FnMut(u64, u64) -> Page {
        move |limit, offset| {
            let from = usize::try_from(offset).unwrap_or(usize::MAX);
            let page: Vec<Value> = rows
                .iter()
                .skip(from)
                .take(usize::try_from(limit).unwrap_or(usize::MAX))
                .cloned()
                .collect();
            std::future::ready(Ok((count, Value::Array(page))))
        }
    }

    fn rows(n: usize) -> Vec<Value> {
        (0..n)
            .map(|i| json!({"id": format!("urn:ngsi-ld:Bridge:{i}")}))
            .collect()
    }

    /// Rows, count, reason and skipped rows of a load.
    fn got(out: &Result<Loaded, Busy>) -> Option<(usize, Option<u64>, BasisReason, usize)> {
        out.as_ref()
            .ok()
            .map(|o| (o.rows.len(), o.total, o.reason, o.skipped))
    }

    const WIDE: Budget = Budget {
        max_rows: 1000,
        max_bytes: 1 << 20,
    };

    async fn run(
        state: State,
        allowed: Budget,
        fetch: impl FnMut(u64, u64) -> Page,
    ) -> Result<Loaded, Busy> {
        let sem = tokio::sync::Semaphore::new(4);
        load(&sem, 4, state, allowed, fetch).await
    }

    #[tokio::test]
    async fn a_type_under_the_budget_loads_whole() {
        let out = run(State::Normal, WIDE, pages(rows(3), Some(3))).await;
        assert_eq!(got(&out), Some((3, Some(3), BasisReason::All, 0)));
    }

    #[tokio::test]
    async fn a_tight_state_asks_for_the_count_alone() {
        let mut asked = Vec::new();
        let fetch = |limit, offset| {
            asked.push((limit, offset));
            std::future::ready(Ok::<_, Busy>((Some(40), json!([{"id": "x"}]))))
        };
        let out = run(State::Tight, budget_for(State::Tight), fetch).await;
        assert_eq!(got(&out), Some((0, Some(40), BasisReason::Tight, 0)));
        assert_eq!(asked, [(0, 0)]);
    }

    #[tokio::test]
    async fn a_failed_page_is_the_answer() {
        let fetch = |_, _| std::future::ready(Err::<(Option<u64>, Value), _>(Busy));
        assert_eq!(run(State::Normal, WIDE, fetch).await, Err(Busy));
    }

    #[tokio::test]
    async fn a_body_that_is_no_list_loads_nothing() {
        let fetch = |_, _| std::future::ready(Ok::<_, Busy>((Some(1), json!({"id": "x"}))));
        let out = run(State::Normal, WIDE, fetch).await;
        assert_eq!(got(&out), Some((0, Some(1), BasisReason::All, 0)));
    }

    #[tokio::test]
    async fn the_byte_budget_stops_the_load_and_says_so() {
        let one =
            serde_json::to_vec(rows(1).first().unwrap_or(&Value::Null)).map_or(0, |b| b.len());
        let tight = Budget {
            max_rows: 1000,
            max_bytes: one * 2,
        };
        let out = run(State::Normal, tight, pages(rows(5), Some(5))).await;
        assert_eq!(got(&out), Some((2, Some(5), BasisReason::MemoryLimit, 0)));
    }

    #[tokio::test]
    async fn the_row_limit_is_named_unless_every_row_was_seen() {
        let two = Budget {
            max_rows: 2,
            max_bytes: 1 << 20,
        };
        let out = run(State::Normal, two, pages(rows(5), Some(5))).await;
        assert_eq!(got(&out), Some((2, Some(5), BasisReason::RowLimit, 0)));
        let out = run(State::Normal, two, pages(rows(5), None)).await;
        assert_eq!(got(&out), Some((2, None, BasisReason::RowLimit, 0)));
        let out = run(State::Busy, two, pages(rows(5), Some(5))).await;
        assert_eq!(got(&out), Some((2, Some(5), BasisReason::Busy, 0)));
        let out = run(State::Normal, two, pages(rows(2), Some(2))).await;
        assert_eq!(got(&out), Some((2, Some(2), BasisReason::All, 0)));
    }

    #[tokio::test]
    async fn a_skipped_row_makes_room_for_the_next_page() {
        let mut all = rows(4);
        all.insert(
            0,
            json!({"id": "urn:ngsi-ld:Bridge:huge", "text": "x".repeat(MAX_ROW_BYTES)}),
        );
        let three = Budget {
            max_rows: 3,
            max_bytes: 1 << 20,
        };
        let out = run(State::Normal, three, pages(all, Some(5))).await;
        // page 1: the huge row skipped and two kept; page 2 from offset 3: one more
        assert_eq!(got(&out), Some((3, Some(5), BasisReason::RowLimit, 1)));
        let ids: Vec<&str> = out.as_ref().map_or(Vec::new(), |o| {
            o.rows
                .iter()
                .filter_map(|r| r.get("id").and_then(Value::as_str))
                .collect()
        });
        assert_eq!(
            ids,
            [
                "urn:ngsi-ld:Bridge:0",
                "urn:ngsi-ld:Bridge:1",
                "urn:ngsi-ld:Bridge:2"
            ]
        );
    }

    #[tokio::test]
    async fn no_permit_in_time_is_busy() {
        let sem = tokio::sync::Semaphore::new(1);
        let held = sem.try_acquire();
        assert!(held.is_ok());
        let out = load(&sem, 1, State::Normal, WIDE, pages(rows(1), Some(1))).await;
        assert_eq!(out, Err(Busy));
    }
}
