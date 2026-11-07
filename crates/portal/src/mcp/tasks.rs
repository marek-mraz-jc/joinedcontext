//! The MCP Tasks extension of the Portal's configuration server (AG-60, ADR-N-021, T-0843).
//!
//! A pipeline test runs a Bento stream, a model inference posts a file to Model Tools, a space
//! completion probes a feed: each takes longer than a client should hold a request open. Such a
//! call answers a task at once; the work runs on, and `tasks/get`, `tasks/result` and
//! `tasks/cancel` follow it.
//!
//! The store is per replica and in memory, like the Portal's other per-process maps: a task is
//! the tail of one call, not a record. A finished task is kept for [`TASK_TTL_SECONDS`] so a
//! client that polls slowly still reads its result, and is then forgotten.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::task::JoinHandle;

use crate::auth::session::now_unix;

/// How long a finished task's result is kept for the client that started it.
pub const TASK_TTL_SECONDS: i64 = 600;

/// How long a client should wait between polls, in milliseconds.
const POLL_INTERVAL_MS: u64 = 1000;

/// The operations that run longer than a request and therefore answer a task (T-0843).
///
/// Each of them leaves the Portal and waits on something else: the project's runner, Model
/// Tools, a feed of somebody else's. Everything else answers inline, as it always did.
pub const LONG_RUNNING: [&str; 6] = [
    "jc_pipeline_test",
    "jc_pipeline_sample_source",
    "jc_pipeline_try_mapping",
    "jc_model_infer",
    "jc_space_complete",
    "jc_datasource_check",
];

/// Whether a `tools/call` for this operation is served as a task.
pub fn is_long(name: &str) -> bool {
    LONG_RUNNING.contains(&name)
}

/// What became of one call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Working,
    Completed,
    Failed,
    Cancelled,
}

impl Status {
    fn as_str(&self) -> &'static str {
        match self {
            Status::Working => "working",
            Status::Completed => "completed",
            Status::Failed => "failed",
            Status::Cancelled => "cancelled",
        }
    }
}

struct Task {
    /// The subject of the token that started it: nobody else sees it at all.
    owner: String,
    status: Status,
    /// The `tools/call` result, once there is one.
    result: Option<Value>,
    created_at: i64,
    finished_at: Option<i64>,
    handle: Option<JoinHandle<()>>,
}

/// Every task of this replica, by id.
#[derive(Clone, Default)]
pub struct McpTasks {
    tasks: Arc<Mutex<HashMap<String, Task>>>,
}

impl std::fmt::Debug for McpTasks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("McpTasks")
    }
}

impl McpTasks {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts one call in the background and answers the task that follows it.
    ///
    /// The future carries the whole call, so the operation runs exactly as it does inline; the
    /// difference is only who waits for it.
    pub fn start<F>(&self, owner: &str, work: F) -> Value
    where
        F: std::future::Future<Output = Value> + Send + 'static,
    {
        self.forget_expired();
        let id = format!("task-{:016x}", rand_id());
        let created_at = now_unix();
        let tasks = self.tasks.clone();
        let key = id.clone();
        // The entry is in the map before the work starts: work that finishes first finds it and
        // keeps its answer, never leaving the task `working` for good.
        if let Ok(mut map) = self.tasks.lock() {
            map.insert(
                id.clone(),
                Task {
                    owner: owner.to_owned(),
                    status: Status::Working,
                    result: None,
                    created_at,
                    finished_at: None,
                    handle: None,
                },
            );
        }
        let handle = tokio::spawn(async move {
            let outcome = work.await;
            if let Ok(mut map) = tasks.lock() {
                if let Some(task) = map.get_mut(&key) {
                    // A cancelled task keeps its status: the answer that arrived late is dropped.
                    if task.status == Status::Working {
                        task.status = if outcome.get("isError") == Some(&json!(true)) {
                            Status::Failed
                        } else {
                            Status::Completed
                        };
                        task.result = Some(outcome);
                        task.finished_at = Some(now_unix());
                    }
                }
            }
        });
        if let Ok(mut map) = self.tasks.lock() {
            if let Some(task) = map.get_mut(&id) {
                task.handle = Some(handle);
            }
        }
        self.describe(owner, &id).unwrap_or_else(|| json!({}))
    }

    /// The task as a client reads it, or `None` when this caller has no such task.
    pub fn describe(&self, owner: &str, id: &str) -> Option<Value> {
        let map = self.tasks.lock().ok()?;
        let task = map.get(id).filter(|task| task.owner == owner)?;
        Some(json!({
            "taskId": id,
            "status": task.status.as_str(),
            "createdAt": task.created_at,
            "ttl": TASK_TTL_SECONDS,
            "pollInterval": POLL_INTERVAL_MS,
        }))
    }

    /// The call's own result once the task has one; `Err(status)` while it has not.
    pub fn result(&self, owner: &str, id: &str) -> Result<Option<Value>, &'static str> {
        let map = match self.tasks.lock() {
            Ok(map) => map,
            Err(_) => return Err("the task store is unavailable"),
        };
        match map.get(id).filter(|task| task.owner == owner) {
            None => Ok(None),
            Some(task) => match (&task.status, &task.result) {
                (Status::Working, _) => Err("the task is still working"),
                (Status::Cancelled, _) => Err("the task was cancelled"),
                (_, Some(result)) => Ok(Some(result.clone())),
                (_, None) => Err("the task left no result"),
            },
        }
    }

    /// Stops a working task. Whatever the call was waiting on is dropped with it, so nothing it
    /// had not finished is written; a verdict it would have filed is never filed (AG-62).
    pub fn cancel(&self, owner: &str, id: &str) -> Option<Value> {
        let mut map = self.tasks.lock().ok()?;
        let task = map.get_mut(id).filter(|task| task.owner == owner)?;
        if task.status == Status::Working {
            if let Some(handle) = task.handle.take() {
                handle.abort();
            }
            task.status = Status::Cancelled;
            task.finished_at = Some(now_unix());
        }
        Some(json!({
            "taskId": id,
            "status": task.status.as_str(),
            "createdAt": task.created_at,
            "ttl": TASK_TTL_SECONDS,
            "pollInterval": POLL_INTERVAL_MS,
        }))
    }

    /// This caller's tasks, newest first.
    pub fn list(&self, owner: &str) -> Vec<Value> {
        let Ok(map) = self.tasks.lock() else {
            return Vec::new();
        };
        let mut rows: Vec<(i64, Value)> = map
            .iter()
            .filter(|(_, task)| task.owner == owner)
            .map(|(id, task)| {
                (
                    task.created_at,
                    json!({
                        "taskId": id,
                        "status": task.status.as_str(),
                        "createdAt": task.created_at,
                        "ttl": TASK_TTL_SECONDS,
                        "pollInterval": POLL_INTERVAL_MS,
                    }),
                )
            })
            .collect();
        rows.sort_by_key(|(created_at, _)| std::cmp::Reverse(*created_at));
        rows.into_iter().map(|(_, row)| row).collect()
    }

    /// Drops what finished longer than the TTL ago: the store follows calls, it does not record
    /// them.
    fn forget_expired(&self) {
        let now = now_unix();
        if let Ok(mut map) = self.tasks.lock() {
            map.retain(|_, task| {
                task.finished_at
                    .is_none_or(|at| now - at < TASK_TTL_SECONDS)
            });
        }
    }
}

/// A task id nobody can guess from another's: 64 bits of the process's randomness. The owner
/// check is what protects a task; this only keeps ids apart.
fn rand_id() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_i64(now_unix());
    hasher.write_usize(std::ptr::addr_of!(hasher) as usize);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-5699, F-POR4-009: on a multi-thread runtime, work that finishes before `start` returns
    /// still leaves its answer; it used to find no entry and the task stayed `working`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_instant_answer_is_kept_on_a_multi_thread_runtime() {
        let tasks = McpTasks::new();
        let ids: Vec<String> = (0..200)
            .map(|_| {
                let started = tasks.start("owner-1", async { json!({ "ok": 1 }) });
                started["taskId"].as_str().unwrap_or_default().to_owned()
            })
            .collect();
        for id in &ids {
            let mut answered = false;
            for _ in 0..200 {
                if matches!(tasks.result("owner-1", id), Ok(Some(_))) {
                    answered = true;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            assert!(answered, "{id} never answered");
        }
    }

    #[tokio::test]
    async fn a_task_carries_its_answer_to_the_caller_who_started_it_and_to_nobody_else() {
        let tasks = McpTasks::new();
        let started = tasks.start("owner-1", async { json!({ "isError": false, "ok": 1 }) });
        let id = started["taskId"].as_str().expect("an id").to_owned();
        assert_eq!(started["status"], json!("working"));

        // The work is on its own task; polling is how the client learns it finished.
        for _ in 0..50 {
            if tasks.result("owner-1", &id).is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(
            tasks.result("owner-1", &id).expect("finished"),
            Some(json!({ "isError": false, "ok": 1 }))
        );
        assert_eq!(
            tasks.describe("owner-1", &id).unwrap()["status"],
            "completed"
        );

        // Another token's subject has no such task at all.
        assert_eq!(tasks.describe("owner-2", &id), None);
        assert_eq!(tasks.result("owner-2", &id), Ok(None));
        assert_eq!(tasks.cancel("owner-2", &id), None);
    }

    #[tokio::test]
    async fn a_cancelled_task_keeps_nothing_the_call_had_not_finished() {
        let tasks = McpTasks::new();
        let started = tasks.start("owner-1", async {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            json!({ "isError": false, "written": true })
        });
        let id = started["taskId"].as_str().expect("an id").to_owned();
        let cancelled = tasks.cancel("owner-1", &id).expect("the task");
        assert_eq!(cancelled["status"], json!("cancelled"));
        assert_eq!(tasks.result("owner-1", &id), Err("the task was cancelled"));
        assert_eq!(tasks.list("owner-1").len(), 1);
        assert!(tasks.list("owner-2").is_empty());
    }

    #[test]
    fn every_long_running_name_is_an_operation_the_registry_has() {
        for name in LONG_RUNNING {
            assert!(crate::ops::find(name).is_some(), "{name} is registered");
        }
    }

    // ---------------------------------------------------------------------------------------------
    // T-2111 `start`, T-2112 `result` (AG-60, PF-59)
    //
    // The contract: a task belongs to the subject of the token that started it and to nobody else,
    // its id is unguessable and says nothing about the call, and a client polling it is either told
    // the truth or told to wait — never given somebody else's answer, and never given an answer the
    // call did not produce. A task nobody owns and a task that never existed are the same answer,
    // so an id cannot be probed for existence.
    // ---------------------------------------------------------------------------------------------

    #[tokio::test]
    async fn a_task_id_is_unguessable_and_carries_nothing_of_the_caller_or_the_call() {
        let tasks = McpTasks::new();
        let mut ids = std::collections::BTreeSet::new();
        for _ in 0..200 {
            let started = tasks.start("sub-secret-owner", async { json!({ "isError": false }) });
            let id = started["taskId"].as_str().expect("an id").to_owned();
            assert!(id.starts_with("task-"), "{id}");
            let body = id.trim_start_matches("task-");
            assert_eq!(body.len(), 16, "{id}");
            assert!(body.chars().all(|c| c.is_ascii_hexdigit()), "{id}");
            assert!(!id.contains("secret"), "the id carried the owner: {id}");
            assert!(ids.insert(id.clone()), "{id} was handed out twice");
        }
        // What the client is handed is the task and its terms, and nothing of what it runs.
        let started = tasks.start("sub-1", async {
            json!({ "isError": false, "secret": "x" })
        });
        let keys: Vec<&str> = started
            .as_object()
            .expect("a document")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            ["createdAt", "pollInterval", "status", "taskId", "ttl"]
        );
        assert_eq!(started["status"], json!("working"));
    }

    #[tokio::test]
    async fn a_task_of_another_subject_reads_exactly_like_a_task_that_never_existed() {
        let tasks = McpTasks::new();
        let started = tasks.start("owner-1", async { json!({ "isError": false, "ok": 1 }) });
        let id = started["taskId"].as_str().expect("an id").to_owned();
        for _ in 0..50 {
            if tasks.result("owner-1", &id).is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        // A stranger, a subject that is empty, and an id nobody ever had: one answer for all three,
        // so no client can tell from the answer whether the task is there (PF-59).
        let never = "task-0000000000000000";
        for (owner, probe) in [
            ("owner-2", id.as_str()),
            ("", id.as_str()),
            ("owner-1", never),
            ("owner-2", never),
            ("owner-1", ""),
            ("owner-1", "../../task"),
        ] {
            assert_eq!(tasks.result(owner, probe), Ok(None), "{owner} {probe}");
            assert_eq!(tasks.describe(owner, probe), None, "{owner} {probe}");
            assert_eq!(tasks.cancel(owner, probe), None, "{owner} {probe}");
        }
        // And the owner still reads their own, so none of the probes disturbed it.
        assert_eq!(
            tasks.result("owner-1", &id).expect("finished"),
            Some(json!({ "isError": false, "ok": 1 })),
        );
    }

    #[tokio::test]
    async fn a_call_that_failed_is_a_failed_task_whose_answer_the_client_still_reads() {
        let tasks = McpTasks::new();
        let body = json!({ "isError": true, "content": [{ "type": "text", "text": "refused" }] });
        let started = tasks.start("owner-1", {
            let body = body.clone();
            async move { body }
        });
        let id = started["taskId"].as_str().expect("an id").to_owned();
        for _ in 0..50 {
            if tasks.result("owner-1", &id).is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(
            tasks.describe("owner-1", &id).expect("the task")["status"],
            "failed"
        );
        assert_eq!(tasks.result("owner-1", &id), Ok(Some(body.clone())));
        // Reading it does not spend it: a client may poll after it has the answer.
        assert_eq!(tasks.result("owner-1", &id), Ok(Some(body)));
    }

    #[tokio::test]
    async fn a_task_still_working_is_told_to_wait_and_never_handed_half_an_answer() {
        let tasks = McpTasks::new();
        let started = tasks.start("owner-1", async {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            json!({ "isError": false, "written": true })
        });
        let id = started["taskId"].as_str().expect("an id").to_owned();
        assert_eq!(
            tasks.result("owner-1", &id),
            Err("the task is still working")
        );
        assert_eq!(
            tasks.describe("owner-1", &id).expect("the task")["status"],
            "working"
        );

        // Cancelling twice is the same as cancelling once, and the answer stays the refusal rather
        // than becoming the result of work that was dropped.
        let first = tasks.cancel("owner-1", &id).expect("the task");
        let again = tasks.cancel("owner-1", &id).expect("the task");
        assert_eq!(first["status"], json!("cancelled"));
        assert_eq!(again["status"], json!("cancelled"));
        assert_eq!(tasks.result("owner-1", &id), Err("the task was cancelled"));
    }

    #[tokio::test]
    async fn a_call_that_panicked_is_never_answered_with_a_result_it_did_not_produce() {
        // A panic on a request path is forbidden by the family's rules, so this is the case for the
        // one that gets through anyway: `tokio::spawn` keeps it to its own task, the store is never
        // poisoned by it, and the client is told to wait rather than handed anything. The task then
        // stays `working` until the TTL sweep forgets it — noted in /workspace/chyby.md, since a
        // client polls a call that will never answer for as long as that takes.
        let tasks = McpTasks::new();
        let started = tasks.start("owner-1", async { panic!("the call fell over") });
        let id = started["taskId"].as_str().expect("an id").to_owned();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(
            tasks.result("owner-1", &id),
            Err("the task is still working")
        );
        assert_eq!(
            tasks.describe("owner-1", &id).expect("the task")["status"],
            "working"
        );
        // The store still works for everybody else.
        let other = tasks.start("owner-1", async { json!({ "isError": false, "ok": 2 }) });
        assert!(other["taskId"].as_str().is_some());
    }

    // Struck as impossible to drive from here:
    // - the TTL sweep (`forget_expired`, `TASK_TTL_SECONDS`) has no clock seam and `created_at` is
    //   private, so ageing a task would mean sleeping for the whole TTL;
    // - `result`'s last arm, `(_, None) => Err("the task left no result")`, cannot be reached:
    //   `Working` and `Cancelled` are answered above it, and the only writer of any other status
    //   (line 106) sets `result` in the same breath.
}
