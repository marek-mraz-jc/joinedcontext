//! A Policy's `temporalQ` window as the engine reads it (Antares
//! `addons/joinedcontext-policy/src/document.rs`; T-5340 split it out of `views`): the ends of a
//! window, a duration before now, and the intersection of a projection's window with a Policy's own.
//!
//! Pure functions of the text: no I/O, no clock.

/// One end of a `temporalQ` window as the engine reads it (Antares
/// `addons/joinedcontext-policy/src/document.rs` `bound`): an instant, or a duration before now.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Bound {
    At(chrono::DateTime<chrono::Utc>),
    Ago(chrono::Duration),
}

/// The window `[from, to)` of a `temporalQ`, each end with the text it was written as.
type Window<'a> = (Option<(Bound, &'a str)>, Option<(Bound, &'a str)>);

fn window(raw: &str) -> Result<Window<'_>, String> {
    let (mut rel, mut at, mut end) = (None, None, None);
    for part in raw.split([';', '&']) {
        match part.split_once('=').map(|(k, v)| (k.trim(), v.trim())) {
            Some(("timerel", v)) => rel = Some(v),
            Some(("timeAt", v)) => at = Some((bound(v)?, v)),
            Some(("endTimeAt", v)) => end = Some((bound(v)?, v)),
            _ => return Err(format!("{part:?} is not timerel, timeAt or endTimeAt")),
        }
    }
    match rel {
        Some("after") => Ok((Some(at.ok_or("no timeAt")?), None)),
        Some("before") => Ok((None, Some(at.ok_or("no timeAt")?))),
        Some("between") => Ok((
            Some(at.ok_or("no timeAt")?),
            Some(end.ok_or("no endTimeAt")?),
        )),
        other => Err(format!("timerel {other:?} is not before, after or between")),
    }
}

fn bound(raw: &str) -> Result<Bound, String> {
    if let Ok(at) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Ok(Bound::At(at.with_timezone(&chrono::Utc)));
    }
    ago(raw)
        .map(Bound::Ago)
        .ok_or_else(|| format!("{raw:?} is neither an instant nor a duration before now"))
}

/// `P-1D`, `-P1D`, `PT-2H30M`: how long before now, in weeks, days, hours, minutes and seconds
/// (months and years have no fixed length, the engine refuses them too).
fn ago(raw: &str) -> Option<chrono::Duration> {
    let (mut negative, rest) = match raw.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, raw),
    };
    let rest = rest.strip_prefix('P')?;
    let (date, time) = rest.split_once('T').unwrap_or((rest, ""));
    let mut total = chrono::Duration::zero();
    let mut digits = String::new();
    let mut units = 0;
    for (part, allowed) in [(date, "WD"), (time, "HMS")] {
        for c in part.chars() {
            match c {
                '-' if digits.is_empty() => negative = !negative,
                '0'..='9' => digits.push(c),
                u if allowed.contains(u) => {
                    let n: i64 = digits.parse().ok()?;
                    digits.clear();
                    units += 1;
                    total = total.checked_add(&match u {
                        'W' => chrono::Duration::try_weeks(n)?,
                        'D' => chrono::Duration::try_days(n)?,
                        'H' => chrono::Duration::try_hours(n)?,
                        'M' => chrono::Duration::try_minutes(n)?,
                        _ => chrono::Duration::try_seconds(n)?,
                    })?;
                }
                _ => return None,
            }
        }
    }
    (negative && units > 0 && digits.is_empty() && total > chrono::Duration::zero())
        .then_some(total)
}

/// The later (`later`) or earlier of two ends, or why they cannot be told apart: an instant and
/// a duration before now order differently as time passes.
fn pick<'a>(
    a: Option<(Bound, &'a str)>,
    b: Option<(Bound, &'a str)>,
    later: bool,
) -> Result<Option<(Bound, &'a str)>, String> {
    let (Some(x), Some(y)) = (a, b) else {
        return Ok(a.or(b));
    };
    let x_first = match (x.0, y.0) {
        (Bound::At(p), Bound::At(q)) => p <= q,
        // further back is earlier
        (Bound::Ago(p), Bound::Ago(q)) => p >= q,
        _ => {
            return Err(format!(
                "{} and {} cannot be compared, one is an instant and one a time before now",
                x.1, y.1
            ))
        }
    };
    Ok(Some(if x_first == later { y } else { x }))
}

/// The `temporalQ` that holds both `projection` and `own` (a Policy's), as the engine reads one:
/// the later start and the earlier end.
pub(super) fn intersect(projection: &str, own: Option<&str>) -> Result<String, String> {
    let (from_a, to_a) = window(projection)?;
    let (from_b, to_b) = match own {
        Some(own) => window(own)?,
        None => (None, None),
    };
    let from = pick(from_a, from_b, true)?;
    let to = pick(to_a, to_b, false)?;
    Ok(match (from, to) {
        (Some((_, f)), Some((_, t))) => format!("timerel=between;timeAt={f};endTimeAt={t}"),
        (Some((_, f)), None) => format!("timerel=after;timeAt={f}"),
        (None, Some((_, t))) => format!("timerel=before;timeAt={t}"),
        (None, None) => return Err("no timeAt".into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-3812: the later start and the earlier end, as the engine reads them.
    #[test]
    fn two_windows_intersect_to_the_later_start_and_the_earlier_end() {
        let between = "timerel=between;timeAt=2025-01-01T00:00:00Z;endTimeAt=2026-01-01T00:00:00Z";
        for (projection, own, want) in [
            (
                "timerel=after;timeAt=P-7D",
                None,
                "timerel=after;timeAt=P-7D",
            ),
            (
                "timerel=after;timeAt=-P7D",
                Some("timerel=after;timeAt=P-1W1D"),
                "timerel=after;timeAt=-P7D",
            ),
            (
                "timerel=after;timeAt=P-7D",
                Some("timerel=before;timeAt=P-1D"),
                "timerel=between;timeAt=P-7D;endTimeAt=P-1D",
            ),
            (
                between,
                Some("timerel=after;timeAt=2025-06-01T00:00:00Z"),
                "timerel=between;timeAt=2025-06-01T00:00:00Z;endTimeAt=2026-01-01T00:00:00Z",
            ),
            (
                between,
                Some("timerel=before;timeAt=2025-03-01T00:00:00+01:00"),
                "timerel=between;timeAt=2025-01-01T00:00:00Z;endTimeAt=2025-03-01T00:00:00+01:00",
            ),
        ] {
            assert_eq!(
                intersect(projection, own).as_deref(),
                Ok(want),
                "{projection} {own:?}"
            );
        }
        for (projection, own) in [
            (
                "timerel=after;timeAt=P-7D",
                Some("timerel=after;timeAt=2025-01-01T00:00:00Z"),
            ),
            ("timerel=after;timeAt=P7D", None),
            ("timerel=after;timeAt=P-1Y", None),
            ("timerel=after;timeAt=P-", None),
            ("timerel=during;timeAt=P-1D", None),
            ("timerel=between;timeAt=P-1D", None),
            ("timerel=after;timeAt=P-1D;geometry=Point", None),
        ] {
            assert!(intersect(projection, own).is_err(), "{projection} {own:?}");
        }
    }
}
