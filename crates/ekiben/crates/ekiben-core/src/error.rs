use std::collections::HashMap;
use std::fmt;

use thiserror::Error;

use crate::message::{Batch, Part, SortGroup};

/// Detailed processor error attached to a failed `Part` (`internal/value/detailed_error.go`).
/// Exposes `message`, `label` (`error_source_label()`), `component_type` (`error_source_type()`),
/// and `path` (`error_source_path()`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetailedError {
    pub message: String,
    pub label: String,
    pub component_type: String,
    pub path: String,
}

impl DetailedError {
    pub fn new(
        message: impl Into<String>,
        component_type: impl Into<String>,
        label: impl Into<String>,
        path: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            label: label.into(),
            component_type: component_type.into(),
            path: path.into(),
        }
    }

    pub fn plain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            label: String::new(),
            component_type: String::new(),
            path: String::new(),
        }
    }
}

impl fmt::Display for DetailedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// Batch-wide error that can also store granular per-part errors (`internal/batch/error.go`).
#[derive(Clone, Debug)]
pub struct BatchError {
    pub message: String,
    pub errored_batch: Batch,
    pub part_errors: HashMap<usize, String>,
}

impl BatchError {
    pub fn new(errored_batch: Batch, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            errored_batch,
            part_errors: HashMap::new(),
        }
    }

    pub fn failed(&mut self, index: usize, err: impl Into<String>) -> &mut Self {
        if index < self.errored_batch.len() {
            self.part_errors.insert(index, err.into());
        }
        self
    }

    pub fn indexed_errors(&self) -> usize {
        self.part_errors.len()
    }

    /// Walks parts of `errored_batch` mapped back to their indices in `source_batch` via `source_group`
    /// (`BatchError.WalkPartsBySource` in `internal/batch/error.go`).
    pub fn walk_parts_by_source<F>(&self, source_group: &SortGroup, source_batch: &[Part], mut f: F)
    where
        F: FnMut(usize, &Part, Option<&str>) -> bool,
    {
        for (i, part) in self.errored_batch.iter().enumerate() {
            let Some(src_idx) = source_group.get_index(part) else {
                continue;
            };
            if src_idx >= source_batch.len() {
                continue;
            }
            let err_opt = if self.part_errors.is_empty() {
                Some(self.message.as_str())
            } else {
                self.part_errors.get(&i).map(String::as_str)
            };
            if !f(src_idx, &source_batch[src_idx], err_opt) {
                break;
            }
        }
    }
}

impl fmt::Display for BatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for BatchError {}

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("end of input")]
    EndOfInput,

    #[error("component not connected")]
    NotConnected,

    #[error("component closed")]
    ComponentClosed,

    #[error("timeout")]
    Timeout,

    #[error("key not found: {0}")]
    KeyNotFound(String),

    #[error("key already exists: {0}")]
    KeyAlreadyExists(String),

    #[error("quota exceeded for organization '{org}': {reason}")]
    QuotaExceeded { org: String, reason: String },

    #[error("access denied for organization '{org}': {reason}")]
    AccessDenied { org: String, reason: String },

    #[error("configuration error: {0}")]
    Config(String),

    #[error(transparent)]
    Bloblang(#[from] ekiben_bloblang::BloblangError),

    #[error("mapping does not parse: {0}")]
    MappingParse(#[from] ekiben_bloblang::ParseError),

    #[error(transparent)]
    Batch(#[from] BatchError),

    #[error("{0}")]
    Other(String),
}

impl CoreError {
    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }

    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }
}
