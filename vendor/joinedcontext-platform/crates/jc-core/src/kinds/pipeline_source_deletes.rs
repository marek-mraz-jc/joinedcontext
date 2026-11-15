//! `spec.sourceDeletes` of a Pipeline (PL-96…PL-100): how the pipeline follows a record its source
//! removed. `never` keeps today's behaviour; `snapshot` deletes what complete runs no longer read;
//! `events` deletes what the mapped record marks as deleted; `lease` relies on a rolling
//! `spec.lifetime`. Only entities in the pipeline's own source ledger are ever candidates; the
//! ledger, the guards and the deletes are the runner's (T-4891).
//!
//! The checks here read the Pipeline alone; the one that needs its DataSource (a snapshot of a
//! source that never ends) is in `data_source::check_class`; the
//! author-Bloblang rules on `deletedWhen` (PL-50) are applied by `PipelineSpec::validate`.

use crate::error::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Consecutive complete runs an id must be missing from before it is deleted, by default (PL-97).
pub const MISSES_DEFAULT: u8 = 2;
/// The most consecutive misses a snapshot may ask for (PL-96).
pub const MISSES_MAX: u8 = 10;
/// The share of the ledger, in percent, one run may delete before it is held, by default (PL-97).
pub const MAX_SHARE_DEFAULT: u8 = 10;

/// How a pipeline follows deletions at its source (PL-96).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SourceDeleteMode {
    /// Nothing is deleted when the source drops a record; said so in the editor.
    Never,
    /// An id missing from `misses` consecutive complete runs is deleted, within `maxShare`.
    Snapshot,
    /// A mapped record for which `deletedWhen` holds deletes the entity of its id.
    Events,
    /// The entity ends by a rolling `spec.lifetime`, renewed by each write.
    Lease,
}

/// `spec.sourceDeletes` (PL-96).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SourceDeletes {
    /// `never`, `snapshot`, `events` or `lease`.
    pub mode: SourceDeleteMode,
    /// `snapshot`: consecutive complete runs an id is missing from before it goes, 1 to 10
    /// (default 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub misses: Option<u8>,
    /// `snapshot`: the most of the ledger, in percent (1 to 100, default 10), one run deletes;
    /// a run that would delete more deletes nothing and is held.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_share: Option<u8>,
    /// `events`: a Bloblang query on the mapped record, true when the source says the record
    /// is deleted (for example `this.status.value == "withdrawn"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_when: Option<String>,
}

/// What the Pipeline knows that a source-delete rule depends on.
pub struct Context {
    /// The pipeline names a `period` or a `schedule`: it runs, so a run can end complete.
    pub has_clock: bool,
    /// The pipeline carries `spec.expiry`, the age sweep (PL-64).
    pub has_expiry: bool,
    /// A `spec.lifetime` entry ends the whole entity by `rolling` (PL-80).
    pub has_rolling_entity_lifetime: bool,
}

impl SourceDeletes {
    /// The checked number of misses (PL-96, PL-97).
    pub fn misses(&self) -> u8 {
        self.misses.unwrap_or(MISSES_DEFAULT)
    }

    /// The checked share in percent (PL-96, PL-97).
    pub fn max_share(&self) -> u8 {
        self.max_share.unwrap_or(MAX_SHARE_DEFAULT)
    }

    /// Admission of the rule (PL-96, PL-99), each refusal naming its field.
    pub fn validate(&self, context: &Context) -> Result<()> {
        let refused = |field: &str, reason: &str| Error::Invalid {
            field: format!("spec.sourceDeletes.{field}"),
            reason: reason.to_owned(),
        };
        let snapshot = self.mode == SourceDeleteMode::Snapshot;
        let events = self.mode == SourceDeleteMode::Events;
        if context.has_expiry {
            return Err(refused(
                "mode",
                "spec.expiry is the age sweep of a pipeline with no source-delete rule; keep one \
                 of the two (PL-99)",
            ));
        }
        if !snapshot && (self.misses.is_some() || self.max_share.is_some()) {
            let field = if self.misses.is_some() {
                "misses"
            } else {
                "maxShare"
            };
            return Err(refused(
                field,
                "only a snapshot counts misses and shares (PL-96)",
            ));
        }
        if !events && self.deleted_when.is_some() {
            return Err(refused(
                "deletedWhen",
                "only mode events reads a delete marker (PL-96)",
            ));
        }
        match self.mode {
            SourceDeleteMode::Never => {}
            SourceDeleteMode::Snapshot => {
                if !(1..=MISSES_MAX).contains(&self.misses()) {
                    return Err(refused("misses", "misses is 1 to 10 complete runs (PL-96)"));
                }
                if !(1..=100).contains(&self.max_share()) {
                    return Err(refused("maxShare", "maxShare is 1 to 100 percent (PL-96)"));
                }
                if !context.has_clock {
                    return Err(refused(
                        "mode",
                        "a snapshot compares complete runs, and a pipeline with no period or \
                         schedule waits for pushes and has none; use events or lease (PL-99)",
                    ));
                }
            }
            SourceDeleteMode::Events => {
                let marker = self.deleted_when.as_deref().unwrap_or_default();
                if marker.trim().is_empty() {
                    return Err(refused(
                        "deletedWhen",
                        "mode events needs deletedWhen, the query that marks a mapped record as \
                         deleted at the source (PL-96)",
                    ));
                }
            }
            SourceDeleteMode::Lease => {
                if !context.has_rolling_entity_lifetime {
                    return Err(refused(
                        "mode",
                        "mode lease needs a spec.lifetime entry whose entity is rolling (PL-80, \
                         PL-96)",
                    ));
                }
            }
        }
        Ok(())
    }

    /// One run of a `snapshot` rule over the pipeline's ledger, id -> consecutive misses
    /// (PL-97). A failed or empty run counts nothing and deletes nothing; a run that would delete
    /// more than `maxShare` of the ledger deletes nothing and leaves the ledger as it was, so it
    /// is held until a person looks. Otherwise each id missing `misses` complete runs in a row
    /// leaves the ledger and is returned for deletion. Another mode deletes nothing here.
    pub fn snapshot_run(
        &self,
        ledger: &mut BTreeMap<String, u8>,
        seen: &BTreeSet<String>,
        complete: bool,
    ) -> SnapshotVerdict {
        if self.mode != SourceDeleteMode::Snapshot {
            return SnapshotVerdict::Delete(Vec::new());
        }
        if !complete || seen.is_empty() {
            return SnapshotVerdict::Incomplete;
        }
        let mut next = ledger.clone();
        for (id, misses) in next.iter_mut() {
            *misses = if seen.contains(id) {
                0
            } else {
                misses.saturating_add(1)
            };
        }
        for id in seen {
            next.entry(id.clone()).or_insert(0);
        }
        let gone: Vec<String> = next
            .iter()
            .filter(|(_, misses)| **misses >= self.misses())
            .map(|(id, _)| id.clone())
            .collect();
        let total = next.len();
        if gone.len().saturating_mul(100) > total.saturating_mul(usize::from(self.max_share())) {
            return SnapshotVerdict::Held {
                would_delete: gone.len(),
                ledger: total,
            };
        }
        for id in &gone {
            next.remove(id);
        }
        *ledger = next;
        SnapshotVerdict::Delete(gone)
    }
}

/// What one snapshot run decides (PL-97).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotVerdict {
    /// The ids to delete now; each is gone from the ledger.
    Delete(Vec<String>),
    /// The run failed or read nothing: nothing is counted and nothing deleted.
    Incomplete,
    /// More than `maxShare` would go: nothing deleted, the ledger left as it was.
    Held {
        /// How many ids the run would have deleted.
        would_delete: usize,
        /// How many ids the ledger holds with this run's.
        ledger: usize,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(misses: Option<u8>, max_share: Option<u8>) -> SourceDeletes {
        SourceDeletes {
            mode: SourceDeleteMode::Snapshot,
            misses,
            max_share,
            deleted_when: None,
        }
    }

    fn ids(range: std::ops::Range<u32>) -> BTreeSet<String> {
        range.map(|n| format!("urn:ngsi-ld:T:{n}")).collect()
    }

    #[test]
    fn a_run_listing_nine_of_ten_removes_the_tenth() {
        let rule = snapshot(Some(1), None);
        let mut ledger = BTreeMap::new();
        assert_eq!(
            rule.snapshot_run(&mut ledger, &ids(0..10), true),
            SnapshotVerdict::Delete(Vec::new())
        );
        assert_eq!(
            rule.snapshot_run(&mut ledger, &ids(0..9), true),
            SnapshotVerdict::Delete(vec!["urn:ngsi-ld:T:9".to_owned()])
        );
        assert_eq!(ledger.len(), 9);
    }

    #[test]
    fn the_default_waits_two_complete_runs() {
        let rule = snapshot(None, None);
        let mut ledger = BTreeMap::new();
        rule.snapshot_run(&mut ledger, &ids(0..10), true);
        assert_eq!(
            rule.snapshot_run(&mut ledger, &ids(0..9), true),
            SnapshotVerdict::Delete(Vec::new())
        );
        assert_eq!(
            rule.snapshot_run(&mut ledger, &ids(0..9), true),
            SnapshotVerdict::Delete(vec!["urn:ngsi-ld:T:9".to_owned()])
        );
    }

    #[test]
    fn an_empty_or_failed_run_removes_nothing() {
        let rule = snapshot(Some(1), Some(100));
        let mut ledger = BTreeMap::new();
        rule.snapshot_run(&mut ledger, &ids(0..10), true);
        let before = ledger.clone();
        assert_eq!(
            rule.snapshot_run(&mut ledger, &BTreeSet::new(), true),
            SnapshotVerdict::Incomplete
        );
        assert_eq!(
            rule.snapshot_run(&mut ledger, &ids(0..3), false),
            SnapshotVerdict::Incomplete
        );
        assert_eq!(ledger, before);
    }

    #[test]
    fn a_run_over_the_share_is_held_whole() {
        let rule = snapshot(Some(1), Some(10));
        let mut ledger = BTreeMap::new();
        rule.snapshot_run(&mut ledger, &ids(0..10), true);
        let before = ledger.clone();
        assert_eq!(
            rule.snapshot_run(&mut ledger, &ids(0..8), true),
            SnapshotVerdict::Held {
                would_delete: 2,
                ledger: 10
            }
        );
        assert_eq!(ledger, before, "a held run counts no miss");
    }

    #[test]
    fn another_mode_deletes_nothing() {
        let rule = SourceDeletes {
            mode: SourceDeleteMode::Never,
            misses: None,
            max_share: None,
            deleted_when: None,
        };
        let mut ledger = BTreeMap::from([("urn:a".to_owned(), 5)]);
        assert_eq!(
            rule.snapshot_run(&mut ledger, &BTreeSet::new(), true),
            SnapshotVerdict::Delete(Vec::new())
        );
        assert_eq!(ledger.len(), 1);
    }
}
