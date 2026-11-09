#![forbid(unsafe_code)]
//! Nightly harvests: each enabled source runs when its cron `schedule` matches, as prod's CKAN
//! CronJobs did (`concurrencyPolicy: Forbid`, `backoffLimit: 3`, `activeDeadlineSeconds: 3600`).
//! At most one run per source holds its Running row ([`Store::claim_harvest_run`]); a failed run
//! is tried again up to [`RETRIES`] times; a source that still fails is logged and the next
//! cycle runs as usual. Every run acts as the catalogue's harvest principal
//! ([`harvest_principal`]), which holds `harvest` on that catalogue and nothing else.

use crate::runner::HarvestEngine;
use chrono::{DateTime, Datelike, Timelike, Utc};
use dataroom_core::ops::HarvestRunner;
use dataroom_core::{
    Action, Authorizer, CatalogueId, DataroomError, GrantRule, HarvestRun, ObjectTarget, OrgId,
    RequestCtx, Result, Store, Subject,
};
use std::sync::Arc;
use std::time::Duration;

/// Runs after the first failed one (prod's `backoffLimit`).
pub const RETRIES: u32 = 3;

/// The harvest principal of one catalogue, and its one grant: `harvest` there.
#[must_use]
pub fn harvest_principal(org_id: OrgId, catalogue_id: CatalogueId) -> (Subject, GrantRule) {
    let principal = format!("harvest:{catalogue_id}");
    let subject = Subject::authenticated(principal.clone(), vec![org_id], Vec::new());
    let grant = GrantRule {
        org_id,
        catalogue_id: Some(catalogue_id),
        principal,
        actions: vec![Action::Harvest],
        publisher_id: None,
    };
    (subject, grant)
}

/// One cron field: `*`, `n`, `a-b`, each with an optional `/step`, comma-separated.
fn field_matches(field: &str, value: u32, min: u32, max: u32) -> Result<bool> {
    let bad = || DataroomError::field_error("schedule", format!("bad cron field '{field}'"));
    for part in field.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((r, s)) => (r, s.parse::<u32>().ok().filter(|s| *s > 0).ok_or_else(bad)?),
            None => (part, 1),
        };
        let (lo, hi) = if range == "*" {
            (min, max)
        } else if let Some((a, b)) = range.split_once('-') {
            (a.parse().map_err(|_| bad())?, b.parse().map_err(|_| bad())?)
        } else {
            let n: u32 = range.parse().map_err(|_| bad())?;
            (n, if step > 1 { max } else { n })
        };
        if lo < min || hi > max || lo > hi {
            return Err(bad());
        }
        if (lo..=hi).contains(&value) && (value - lo).is_multiple_of(step) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether the five-field cron `expr` (minute hour day-of-month month day-of-week, UTC) fires
/// in the minute of `at`. Day of month and day of week, both restricted, match either (cron).
pub fn cron_matches(expr: &str, at: DateTime<Utc>) -> Result<bool> {
    let f: Vec<&str> = expr.split_whitespace().collect();
    let [min, hour, dom, mon, dow] = f.as_slice() else {
        return Err(DataroomError::field_error(
            "schedule",
            format!("'{expr}' is not a five-field cron expression"),
        ));
    };
    let weekday = at.weekday().num_days_from_sunday();
    // 7 is Sunday as well as 0.
    let dow_hit =
        field_matches(dow, weekday, 0, 7)? || (weekday == 0 && field_matches(dow, 7, 0, 7)?);
    let dom_hit = field_matches(dom, at.day(), 1, 31)?;
    let day = match (*dom == "*", *dow == "*") {
        (true, _) => dow_hit,
        (false, true) => dom_hit,
        (false, false) => dom_hit || dow_hit,
    };
    Ok(field_matches(min, at.minute(), 0, 59)?
        && field_matches(hour, at.hour(), 0, 23)?
        && field_matches(mon, at.month(), 1, 12)?
        && day)
}

/// The harvest schedule of one store: the catalogues it covers and how a run is tried.
pub struct HarvestScheduler {
    engine: Arc<HarvestEngine>,
    store: Arc<dyn Store>,
    authorizer: Arc<dyn Authorizer>,
    catalogues: Vec<(OrgId, CatalogueId)>,
    retry_delay: Duration,
}

impl HarvestScheduler {
    pub fn new(
        engine: Arc<HarvestEngine>,
        store: Arc<dyn Store>,
        authorizer: Arc<dyn Authorizer>,
        catalogues: Vec<(OrgId, CatalogueId)>,
    ) -> Self {
        Self {
            engine,
            store,
            authorizer,
            catalogues,
            retry_delay: Duration::from_secs(10),
        }
    }

    /// First wait before a retry; it doubles each time (Kubernetes Job back-off).
    #[must_use]
    pub fn with_retry_delay(mut self, delay: Duration) -> Self {
        self.retry_delay = delay;
        self
    }

    /// Runs every enabled source of every catalogue whose schedule fires in the minute of `now`,
    /// sources side by side, and returns each source's last outcome.
    pub async fn run_due(&self, now: DateTime<Utc>) -> Vec<(String, Result<HarvestRun>)> {
        let mut runs = tokio::task::JoinSet::new();
        for &(org_id, catalogue_id) in &self.catalogues {
            let (subject, _) = harvest_principal(org_id, catalogue_id);
            let ctx = RequestCtx::new(org_id, catalogue_id, subject);
            let sources = match self.store.list_harvest_sources(&ctx).await {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("dataroom harvest: catalogue {catalogue_id}: list sources: {e}");
                    continue;
                }
            };
            for source in sources.into_iter().filter(|s| s.enabled) {
                match cron_matches(&source.schedule, now) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(e) => {
                        eprintln!("dataroom harvest: source {}: {e}", source.name);
                        continue;
                    }
                }
                let (engine, store, authorizer, ctx, delay) = (
                    self.engine.clone(),
                    self.store.clone(),
                    self.authorizer.clone(),
                    ctx.clone(),
                    self.retry_delay,
                );
                runs.spawn(async move {
                    let target = ObjectTarget::HarvestSource {
                        org_id: ctx.org_id,
                        catalogue_id: ctx.catalogue_id,
                        source_id: source.id.clone(),
                    };
                    let decision = authorizer.check(&ctx, Action::Harvest, &target);
                    let res = if decision.allowed {
                        run_with_retries(&engine, store.as_ref(), &ctx, &source, delay).await
                    } else {
                        Err(DataroomError::Forbidden(decision.reason))
                    };
                    if let Err(e) = &res {
                        eprintln!("dataroom harvest: source {} failed: {e}", source.name);
                    }
                    (source.name, res)
                });
            }
        }
        let mut out = Vec::new();
        while let Some(done) = runs.join_next().await {
            if let Ok(pair) = done {
                out.push(pair);
            }
        }
        out
    }

    /// Checks the schedules once a minute, for the life of the process; a run takes minutes,
    /// so each minute's runs go on beside the next check.
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        let me = Arc::new(self);
        tokio::spawn(async move {
            loop {
                let into_minute = u64::from(Utc::now().second());
                tokio::time::sleep(Duration::from_secs(60 - into_minute)).await;
                let (me, minute) = (me.clone(), Utc::now());
                tokio::spawn(async move {
                    me.run_due(minute).await;
                });
            }
        })
    }
}

/// One run of `source` and up to [`RETRIES`] more after a failure; a run refused because
/// another run holds the source is not tried again.
async fn run_with_retries(
    engine: &HarvestEngine,
    store: &dyn Store,
    ctx: &RequestCtx,
    source: &dataroom_core::HarvestSource,
    first_delay: Duration,
) -> Result<HarvestRun> {
    let mut delay = first_delay;
    let mut attempt = 0;
    loop {
        match engine.run_source(ctx, store, source).await {
            Ok(run) => return Ok(run),
            Err(e @ DataroomError::Conflict(_)) => return Err(e),
            Err(e) if attempt >= RETRIES => return Err(e),
            Err(e) => {
                eprintln!(
                    "dataroom harvest: source {} attempt {} failed, trying again: {e}",
                    source.name,
                    attempt + 1
                );
                attempt += 1;
                tokio::time::sleep(delay).await;
                delay *= 2;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn prod_schedule_fires_once_a_day_at_03_23() {
        let at = |h, m| Utc.with_ymd_and_hms(2026, 11, 9, h, m, 0).unwrap();
        assert!(cron_matches("23 3 * * *", at(3, 23)).unwrap());
        assert!(!cron_matches("23 3 * * *", at(3, 24)).unwrap());
        assert!(!cron_matches("23 3 * * *", at(4, 23)).unwrap());
        assert!(cron_matches("*/15 * * * *", at(7, 45)).unwrap());
        assert!(!cron_matches("*/15 * * * *", at(7, 46)).unwrap());
        assert!(cron_matches("0 1-5/2 * * 1", at(3, 0)).unwrap()); // 2026-11-09 is a Monday
        assert!(!cron_matches("0 3 * * 0,7", at(3, 0)).unwrap());
        for bad in [
            "23 3 * *",
            "61 3 * * *",
            "x 3 * * *",
            "*/0 * * * *",
            "5-1 * * * *",
        ] {
            assert!(cron_matches(bad, at(3, 23)).is_err(), "{bad}");
        }
    }
}
