// SPDX-License-Identifier: EUPL-1.2
//! The narrowed handles of the views a surface served (T-5896). A handle
//! carries the view's state and, after its first in-process call, that
//! state's router: about 250 KiB each, measured over 690 views. A handle no
//! request used for [`VIEW_IDLE_ENV`] (default one hour) is dropped, and the
//! next request builds it again from the table as it is then (~2 ms).
//!
//! The handle is a cache of the table, never its source: the narrowing,
//! operations and policy live in the view table and the engine, which
//! eviction never touches. A build runs under its view's own lock, so
//! concurrent first requests for a view share one build and none answers
//! before it is complete, while requests for other views go on (T-6677); a
//! failed build caches nothing.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock};
use std::time::{Duration, Instant};

/// How long a view's handle is kept after the last request through it,
/// as `antares_api::history::parse_duration` reads it (`1h`, `PT30M`).
/// Default one hour.
pub const VIEW_IDLE_ENV: &str = "ANTARES_VIEW_IDLE";

/// [`VIEW_IDLE_ENV`] unset.
pub const DEFAULT_IDLE: Duration = Duration::from_secs(3600);

/// The longest a cache goes between sweeps while every request hits.
const SWEEP_EVERY: Duration = Duration::from_secs(60);

static IDLE: OnceLock<Duration> = OnceLock::new();

/// The idle bound `raw` names, [`DEFAULT_IDLE`] when unset.
///
/// # Errors
/// A value that is not a duration of one minute to ten years.
pub(crate) fn parse_idle(raw: Option<&str>) -> Result<Duration, String> {
    raw.map_or(Ok(DEFAULT_IDLE), |raw| {
        antares_api::history::parse_duration(raw.trim())
            .map_err(|e| format!("{VIEW_IDLE_ENV}: {e}"))
    })
}

/// Read [`VIEW_IDLE_ENV`] once, at startup: a bad value stops the broker.
///
/// # Errors
/// [`parse_idle`]'s.
pub(crate) fn configure() -> Result<(), String> {
    let idle = parse_idle(std::env::var(VIEW_IDLE_ENV).ok().as_deref())?;
    let _ = IDLE.set(idle);
    Ok(())
}

/// The idle bound in force.
pub(crate) fn idle() -> Duration {
    IDLE.get().copied().unwrap_or(DEFAULT_IDLE)
}

/// Built handles by view id, each with when a request last used it.
///
/// A hit takes the map's read lock only, so warm requests never queue
/// behind a build (T-6677). A miss builds outside the map lock, under the
/// key's own lock: concurrent first requests for one view share one build,
/// and a cold build of one view delays no request for another.
pub(crate) struct ViewCache<T> {
    held: RwLock<HashMap<String, Arc<Slot<T>>>>,
    /// When the last sweep ran, in nanoseconds after `base`.
    swept: AtomicU64,
    base: Instant,
}

/// One view's handle, built at most once.
struct Slot<T> {
    made: OnceLock<T>,
    /// Held while the handle is built, so a second request waits for it.
    building: Mutex<()>,
    /// When a request last used it, in nanoseconds after the cache's `base`.
    used: AtomicU64,
}

impl<T> Default for ViewCache<T> {
    fn default() -> Self {
        Self {
            held: RwLock::default(),
            swept: AtomicU64::new(u64::MAX),
            base: Instant::now(),
        }
    }
}

fn read<T>(l: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    l.read().unwrap_or_else(PoisonError::into_inner)
}

fn write<T>(l: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    l.write().unwrap_or_else(PoisonError::into_inner)
}

impl<T: Clone> ViewCache<T> {
    fn at(&self, now: Instant) -> u64 {
        u64::try_from(now.saturating_duration_since(self.base).as_nanos()).unwrap_or(u64::MAX - 1)
    }

    /// The handle of `id` when one is held and `current` says it was built
    /// from the view the table holds now; else `build`'s, held from `now`
    /// on. `loaded` says which ids the table still holds: every other one,
    /// and every handle idle for `idle`, is dropped at least once a minute;
    /// the dropped handles are returned, for the caller to free outside the
    /// lock (690 of them take ~140 ms). A failed build holds nothing, so the
    /// next request builds again. Builds are counted on
    /// `antares_view_builds_total`.
    pub(crate) fn get_or_build(
        &self,
        id: &str,
        current: impl Fn(&T) -> bool,
        loaded: impl Fn(&str) -> bool,
        now: Instant,
        idle: Duration,
        build: impl FnOnce() -> Option<T>,
    ) -> (Option<T>, Vec<T>) {
        let at = self.at(now);
        let gone = self.sweep_due(&loaded, at, idle);
        let slot = read(&self.held).get(id).cloned();
        if let Some(s) = &slot {
            if let Some(held) = s.made.get().filter(|h| current(h)) {
                s.used.fetch_max(at, Ordering::Relaxed);
                return (Some(held.clone()), gone);
            }
        }
        let slot = {
            let mut map = write(&self.held);
            if let Some(s) = map.get(id).filter(|s| s.made.get().is_none_or(&current)) {
                s.clone()
            } else {
                let s = Arc::new(Slot {
                    made: OnceLock::new(),
                    building: Mutex::new(()),
                    used: AtomicU64::new(at),
                });
                map.insert(id.to_owned(), s.clone());
                s
            }
        };
        let guard = slot.building.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(held) = slot.made.get().filter(|h| current(h)) {
            slot.used.fetch_max(at, Ordering::Relaxed);
            return (Some(held.clone()), gone);
        }
        let Some(made) = build() else {
            drop(guard);
            let mut map = write(&self.held);
            if map.get(id).is_some_and(|s| Arc::ptr_eq(s, &slot)) {
                map.remove(id);
            }
            return (None, gone);
        };
        metrics::counter!("antares_view_builds_total").increment(1);
        let _ = slot.made.set(made.clone());
        slot.used.fetch_max(at, Ordering::Relaxed);
        (Some(made), gone)
    }

    /// Sweep when the last sweep is a minute old, else nothing.
    fn sweep_due(&self, loaded: &impl Fn(&str) -> bool, at: u64, idle: Duration) -> Vec<T> {
        let last = self.swept.load(Ordering::Relaxed);
        let every = u64::try_from(SWEEP_EVERY.as_nanos()).unwrap_or(u64::MAX);
        if last != u64::MAX && at.saturating_sub(last) < every {
            return Vec::new();
        }
        // One request sweeps; the others go on.
        if self
            .swept
            .compare_exchange(last, at, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return Vec::new();
        }
        let idle = u64::try_from(idle.as_nanos()).unwrap_or(u64::MAX);
        let mut gone = Vec::new();
        write(&self.held).retain(|id, s| {
            let keep = loaded(id) && at.saturating_sub(s.used.load(Ordering::Relaxed)) < idle;
            if !keep {
                gone.extend(s.made.get().cloned());
            }
            keep
        });
        gone
    }

    /// How many handles are held.
    #[cfg(test)]
    fn len(&self) -> usize {
        read(&self.held).len()
    }
}

/// Free `gone` off the request path: on a blocking thread when a tokio
/// runtime runs, else here.
pub(crate) fn release<T: Send + 'static>(gone: Vec<T>) {
    if gone.is_empty() {
        return;
    }
    match tokio::runtime::Handle::try_current() {
        Ok(rt) => {
            rt.spawn_blocking(move || drop(gone));
        }
        Err(_) => drop(gone),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Barrier;

    const HOUR: Duration = Duration::from_secs(3600);

    fn get(
        c: &ViewCache<u32>,
        id: &str,
        version: u32,
        now: Instant,
        builds: &AtomicUsize,
    ) -> Option<u32> {
        c.get_or_build(
            id,
            |held| *held == version,
            |_| true,
            now,
            HOUR,
            || {
                builds.fetch_add(1, Ordering::SeqCst);
                Some(version)
            },
        )
        .0
    }

    #[test]
    fn concurrent_first_requests_build_once_and_all_get_the_view() {
        let cache = Arc::new(ViewCache::<u32>::default());
        let builds = Arc::new(AtomicUsize::new(0));
        let start = Arc::new(Barrier::new(16));
        let now = Instant::now();
        let got: Vec<Option<u32>> = (0..16)
            .map(|_| {
                let (cache, builds, start) = (cache.clone(), builds.clone(), start.clone());
                std::thread::spawn(move || {
                    start.wait();
                    cache
                        .get_or_build(
                            "cars",
                            |held| *held == 7,
                            |_| true,
                            now,
                            HOUR,
                            || {
                                builds.fetch_add(1, Ordering::SeqCst);
                                // long enough that every thread arrives during the build
                                std::thread::sleep(Duration::from_millis(50));
                                Some(7)
                            },
                        )
                        .0
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|t| t.join().unwrap_or(None))
            .collect();
        assert_eq!(builds.load(Ordering::SeqCst), 1);
        assert_eq!(got, vec![Some(7); 16]);
    }

    /// T-6677: a flood of cold first requests must not serialise warm ones.
    /// The build of `cold` waits until a warm request for `cars` has been
    /// answered; under one cache lock around builds it never would be.
    #[test]
    fn a_cold_build_does_not_hold_up_a_warm_request() {
        let cache = Arc::new(ViewCache::<u32>::default());
        let builds = AtomicUsize::new(0);
        let now = Instant::now();
        assert_eq!(get(&cache, "cars", 1, now, &builds), Some(1));
        let (warm_tx, warm_rx) = std::sync::mpsc::channel::<()>();
        let (building_tx, building_rx) = std::sync::mpsc::channel::<()>();
        let cold = {
            let cache = cache.clone();
            std::thread::spawn(move || {
                cache
                    .get_or_build(
                        "cold",
                        |_| true,
                        |_| true,
                        now,
                        HOUR,
                        || {
                            let _ = building_tx.send(());
                            warm_rx
                                .recv_timeout(Duration::from_secs(5))
                                .ok()
                                .map(|()| 2)
                        },
                    )
                    .0
            })
        };
        building_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the cold build started");
        let warm = {
            let cache = cache.clone();
            std::thread::spawn(move || {
                let builds = AtomicUsize::new(0);
                let hit = get(&cache, "cars", 1, now, &builds);
                let _ = warm_tx.send(());
                hit
            })
        };
        assert_eq!(
            warm.join().ok().flatten(),
            Some(1),
            "the warm hit was answered"
        );
        assert_eq!(
            cold.join().ok().flatten(),
            Some(2),
            "and the cold build finished after it"
        );
    }

    #[test]
    fn an_idle_view_is_dropped_and_reloads_the_current_version() {
        let c = ViewCache::default();
        let builds = AtomicUsize::new(0);
        let t0 = Instant::now();
        assert_eq!(get(&c, "cars", 1, t0, &builds), Some(1));
        assert_eq!(get(&c, "cars", 1, t0 + HOUR / 2, &builds), Some(1));
        assert_eq!(builds.load(Ordering::SeqCst), 1);
        // a request on another view an hour after the last use of `cars` sweeps it
        assert_eq!(get(&c, "bikes", 1, t0 + HOUR / 2 + HOUR, &builds), Some(1));
        assert_eq!(c.len(), 1);
        // the view changed while evicted: the reload builds the table's version, not the old one
        assert_eq!(get(&c, "cars", 2, t0 + 2 * HOUR, &builds), Some(2));
        assert_eq!(builds.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn a_hit_sweeps_idle_views_once_a_minute() {
        let c = ViewCache::default();
        let builds = AtomicUsize::new(0);
        let t0 = Instant::now();
        get(&c, "cars", 1, t0, &builds);
        get(
            &c,
            "bikes",
            1,
            t0 + HOUR.saturating_sub(SWEEP_EVERY),
            &builds,
        );
        get(&c, "bikes", 1, t0 + HOUR, &builds);
        assert_eq!(c.len(), 1, "cars went on the hit's sweep");
        assert_eq!(builds.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_changed_view_is_rebuilt_and_a_request_never_gets_the_old_one() {
        let c = ViewCache::default();
        let builds = AtomicUsize::new(0);
        let t0 = Instant::now();
        assert_eq!(get(&c, "cars", 1, t0, &builds), Some(1));
        assert_eq!(get(&c, "cars", 2, t0, &builds), Some(2));
        assert_eq!(get(&c, "cars", 2, t0, &builds), Some(2));
        assert_eq!(builds.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_removed_view_goes_on_the_next_sweep() {
        let c = ViewCache::default();
        let t0 = Instant::now();
        for id in ["cars", "bikes"] {
            c.get_or_build(id, |_| true, |_| true, t0, HOUR, || Some(1_u32));
        }
        let (_, gone) = c.get_or_build(
            "buses",
            |_| true,
            |id| id != "cars",
            t0 + SWEEP_EVERY,
            HOUR,
            || Some(2),
        );
        assert_eq!(c.len(), 2);
        assert_eq!(gone, vec![1], "the removed handle is handed back to free");
    }

    #[test]
    fn a_failed_build_is_not_cached_and_is_retried() {
        let c = ViewCache::<u32>::default();
        let t0 = Instant::now();
        let (none, _) = c.get_or_build("cars", |_| true, |_| true, t0, HOUR, || None);
        assert_eq!(none, None);
        assert_eq!(c.len(), 0);
        let (some, _) = c.get_or_build("cars", |_| true, |_| true, t0, HOUR, || Some(3));
        assert_eq!(some, Some(3));
    }

    #[test]
    fn released_handles_are_freed_with_or_without_a_runtime() {
        let held = Arc::new(());
        release(vec![held.clone()]);
        assert_eq!(
            Arc::strong_count(&held),
            1,
            "freed at once outside a runtime"
        );
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a runtime");
        rt.block_on(async {
            release(Vec::<Arc<()>>::new());
            release(vec![held.clone()]);
        });
        drop(rt);
        assert_eq!(Arc::strong_count(&held), 1, "freed on the blocking pool");
    }

    #[test]
    fn the_idle_bound_defaults_to_an_hour_and_refuses_a_bad_value() {
        assert_eq!(parse_idle(None), Ok(DEFAULT_IDLE));
        assert_eq!(parse_idle(Some("30m")), Ok(Duration::from_mins(30)));
        assert_eq!(parse_idle(Some(" PT2H ")), Ok(Duration::from_hours(2)));
        let bad = parse_idle(Some("soon")).unwrap_err();
        assert!(bad.starts_with("ANTARES_VIEW_IDLE: "), "{bad}");
        assert!(parse_idle(Some("10s")).is_err(), "under a minute");
    }
}
