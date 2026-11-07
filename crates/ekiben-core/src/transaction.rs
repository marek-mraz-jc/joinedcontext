use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;

use crate::error::BatchError;
use crate::message::{Batch, Part, SortGroup};

/// Callback or oneshot channel used to acknowledge or nack a transaction.
pub struct AckResponder {
    inner: Option<Box<dyn FnOnce(Option<BatchError>) + Send + Sync>>,
}

impl AckResponder {
    pub fn noop() -> Self {
        Self { inner: None }
    }

    pub fn from_fn<F>(f: F) -> Self
    where
        F: FnOnce(Option<BatchError>) + Send + Sync + 'static,
    {
        Self {
            inner: Some(Box::new(f)),
        }
    }

    pub fn channel() -> (Self, oneshot::Receiver<Option<BatchError>>) {
        let (tx, rx) = oneshot::channel();
        let responder = Self::from_fn(move |res| {
            let _ = tx.send(res);
        });
        (responder, rx)
    }

    pub fn ack(mut self) {
        if let Some(f) = self.inner.take() {
            f(None);
        }
    }

    pub fn nack(mut self, err: BatchError) {
        if let Some(f) = self.inner.take() {
            f(Some(err));
        }
    }

    pub fn respond(mut self, res: Option<BatchError>) {
        if let Some(f) = self.inner.take() {
            f(res);
        }
    }
}

/// Associates a `Batch` with an `AckResponder` (`internal/message/transaction.go`).
pub struct Transaction {
    pub payload: Batch,
    pub ack: AckResponder,
}

impl Transaction {
    pub fn new(payload: Batch, ack: AckResponder) -> Self {
        Self { payload, ack }
    }
}

/// Aggregates multiple derived `AckResponder`s into a single upstream `AckResponder`
/// (`internal/batch/combined_ack_func.go`).
pub struct CombinedAcker {
    state: Arc<Mutex<CombinedState>>,
}

struct CombinedState {
    remaining: usize,
    first_err: Option<BatchError>,
    root: Option<AckResponder>,
}

impl CombinedAcker {
    pub fn new(root: AckResponder) -> Self {
        Self {
            state: Arc::new(Mutex::new(CombinedState {
                remaining: 0,
                first_err: None,
                root: Some(root),
            })),
        }
    }

    pub fn derive(&self) -> AckResponder {
        let state = self.state.clone();
        {
            let mut guard = state.lock().unwrap();
            guard.remaining += 1;
        }
        AckResponder::from_fn(move |err| {
            let mut root_to_call = None;
            let mut final_err = None;
            {
                let mut guard = state.lock().unwrap();
                if err.is_some() && guard.first_err.is_none() {
                    guard.first_err = err;
                }
                guard.remaining = guard.remaining.saturating_sub(1);
                if guard.remaining == 0 {
                    root_to_call = guard.root.take();
                    final_err = guard.first_err.clone();
                }
            }
            if let Some(root) = root_to_call {
                root.respond(final_err);
            }
        })
    }
}

/// Tags a transaction's messages with a `SortGroup` so downstream `BatchError`s from combined batches
/// can be narrowed down to only the transactions whose parts actually failed (`internal/transaction/tracked.go`).
pub struct TrackedTransaction {
    msg: Batch,
    group: SortGroup,
    ack: AckResponder,
}

impl TrackedTransaction {
    pub fn new(msg: Batch, ack: AckResponder) -> Self {
        let (group, tracked_msg) = SortGroup::new(msg);
        Self {
            msg: tracked_msg,
            group,
            ack,
        }
    }

    pub fn message(&self) -> &Batch {
        &self.msg
    }

    pub fn into_parts(self) -> (Batch, SortGroup, AckResponder) {
        (self.msg, self.group, self.ack)
    }

    pub fn resolve(self, err: Option<BatchError>) {
        let Some(batch_err) = err else {
            self.ack.ack();
            return;
        };
        if batch_err.indexed_errors() == 0 {
            self.ack.nack(batch_err);
            return;
        }
        let mut remaining_indexes: HashSet<usize> = (0..self.msg.len()).collect();
        let mut found_err: Option<String> = None;

        batch_err.walk_parts_by_source(&self.group, &self.msg, |index, _part, part_err| {
            if let Some(e) = part_err {
                found_err = Some(e.to_string());
                return false;
            }
            remaining_indexes.remove(&index);
            !remaining_indexes.is_empty()
        });

        if let Some(e) = found_err {
            self.ack.nack(BatchError::new(self.msg, e));
        } else if !remaining_indexes.is_empty() {
            self.ack.nack(batch_err);
        } else {
            self.ack.ack();
        }
    }
}

/// Propagates synchronous responses back to inputs such as `http_server` or `nats` request-reply
/// (`internal/transaction/result_store.go`).
#[derive(Clone, Debug, Default)]
pub struct SyncResponseStore {
    batches: Arc<Mutex<Vec<Batch>>>,
}

impl SyncResponseStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&self, batch: &[Part]) {
        let copied: Batch = batch.iter().map(Part::deep_copy).collect();
        self.batches.lock().unwrap().push(copied);
    }

    pub fn read(&self) -> Vec<Batch> {
        self.batches.lock().unwrap().clone()
    }

    pub fn clear(&self) {
        self.batches.lock().unwrap().clear();
    }
}
