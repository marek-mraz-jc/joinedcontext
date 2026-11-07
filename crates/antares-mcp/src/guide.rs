// SPDX-License-Identifier: EUPL-1.2
//! The query guide of a view's types (T-4486): for each type the view names, which attribute is
//! its time axis and whether it is an instant or a range, where its place is, the few attributes
//! worth filtering on, the usual sort, and two example calls. It rides on the read tools'
//! descriptions, so an agent opening the view sees how to ask before its first call.
//!
//! The roles come from the type's Smart Data Model (the same names the platform's SDM table
//! classifies by, T-4498), never typed per view; a role names only an attribute the view shows,
//! so the guide never names one it hides. A type the table does not know gets no guide line.

use crate::schema::{Args, Checked, Names};
use antares_ops::guide::{long_text, Guide, Time, GUIDES};

/// Whether the view shows `attribute`: its leading name, before a `.` or `[`.
fn shows(names: &Names, attribute: &str) -> bool {
    let head = attribute.split(['.', '[']).next().unwrap_or(attribute);
    names.every || names.attributes.iter().any(|a| a == head)
}

/// Monday 00:00 UTC of the week holding `now`, and the Monday after, as `YYYY-MM-DDT00:00:00Z`.
fn this_week(now: chrono::DateTime<chrono::Utc>) -> (String, String) {
    use chrono::Datelike as _;
    let today = now.date_naive();
    let monday = today
        .checked_sub_days(chrono::Days::new(u64::from(
            today.weekday().num_days_from_monday(),
        )))
        .unwrap_or(today);
    let next = monday
        .checked_add_days(chrono::Days::new(7))
        .unwrap_or(monday);
    let stamp = |day: chrono::NaiveDate| format!("{}T00:00:00Z", day.format("%Y-%m-%d"));
    (stamp(monday), stamp(next))
}

/// The time axis, the places and the filters of `guide` the view shows.
fn picks(guide: &Guide, names: &Names) -> (Option<Time>, Vec<&'static str>, Vec<&'static str>) {
    let shown = |list: &'static [&'static str]| -> Vec<&'static str> {
        list.iter().copied().filter(|a| shows(names, a)).collect()
    };
    let mut filters = shown(guide.filters);
    filters.truncate(5);
    (
        guide.time_in(|a| shows(names, a)),
        shown(guide.place),
        filters,
    )
}

/// One type's guide line, or `None` for a type the table does not know or the view hides.
fn line(guide: &Guide, names: &Names, week: &(String, String)) -> Option<String> {
    if !names.has_type(guide.entity_type) {
        return None;
    }
    let t = guide.entity_type;
    let (from, to) = week;
    let (time, place, filters) = picks(guide, names);
    let mut parts = vec![match time {
        Some(Time::Range(a, b)) => {
            format!("{t}: time {a}..{b} (a range); this week: type={t}, q={a}<{to};{b}>={from}.")
        }
        Some(Time::Instant(a)) => format!(
            "{t}: time {a} (an instant), sort orderBy={a};desc; this week: type={t}, q={a}>={from};{a}<{to}."
        ),
        None => format!("{t}: no time axis."),
    }];
    match place.first() {
        Some(&"location") => parts.push(format!(
            "Place {}; near a point: type={t}, georel=near;maxDistance==2000, geometry=Point, coordinates=[longitude,latitude].",
            place.join(", ")
        )),
        Some(first) => parts.push(format!(
            "Place {}; by name: type={t}, q={first}~=\"(?i)bystric\".",
            place.join(", ")
        )),
        None => {}
    }
    if !filters.is_empty() {
        parts.push(format!("Filter on {}.", filters.join(", ")));
    }
    let long: Vec<&str> = long_text(t)
        .iter()
        .copied()
        .filter(|a| shows(names, a))
        .collect();
    if !long.is_empty() {
        parts.push(format!(
            "{} is left out unless asked for in attrs or pick.",
            long.join(", ")
        ));
    }
    Some(parts.join(" "))
}

/// The guide of every type the view names, one line each, or `None` when it names none the
/// table knows; the example weeks are the one holding `now`.
pub(crate) fn text(names: &Names, now: chrono::DateTime<chrono::Utc>) -> Option<String> {
    if names.every {
        // ponytail: a `*` view names no type; its schema-less guide is the primer's q grammar
        return None;
    }
    let week = this_week(now);
    let lines: Vec<String> = GUIDES
        .iter()
        .filter_map(|g| line(g, names, &week))
        .collect();
    (!lines.is_empty()).then(|| format!("Query guide, per type:\n{}", lines.join("\n")))
}

/// A read that names no `attrs` or `pick` leaves out the long text attributes (T-4582) of the
/// types it asks for, or of every type the view shows when it names none, by adding them to `omit`.
pub(crate) fn lean(args: &mut Args, names: &Names) {
    if args.contains_key("attrs") || args.contains_key("pick") {
        return;
    }
    let types: Vec<&str> = match args.get("type") {
        Some(Checked::Many(types)) => types.iter().map(String::as_str).collect(),
        _ => GUIDES
            .iter()
            .map(|g| g.entity_type)
            .filter(|t| names.has_type(t))
            .collect(),
    };
    let mut long: Vec<String> = Vec::new();
    for attribute in types.into_iter().flat_map(long_text) {
        if shows(names, attribute) && !long.iter().any(|a| a == attribute) {
            long.push((*attribute).to_owned());
        }
    }
    if long.is_empty() {
        return;
    }
    match args.get_mut("omit") {
        Some(Checked::Many(omit)) => {
            for attribute in long {
                if !omit.contains(&attribute) {
                    omit.push(attribute);
                }
            }
        }
        _ => {
            args.insert("omit", Checked::Many(long));
        }
    }
}

/// The guide of one type the view shows (`describe_type`, T-4537): its line, then the same as
/// JSON; `None` for a type the view hides or the table does not know.
pub(crate) fn describe(
    names: &Names,
    entity_type: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<(String, serde_json::Value)> {
    let guide = GUIDES.iter().find(|g| g.entity_type == entity_type)?;
    let text = line(guide, names, &this_week(now))?;
    let (time, place, filters) = picks(guide, names);
    let time = match time {
        Some(Time::Instant(a)) => serde_json::json!({"instant": a}),
        Some(Time::Range(a, b)) => serde_json::json!({"start": a, "end": b}),
        None => serde_json::Value::Null,
    };
    let json = serde_json::json!({
        "type": entity_type, "time": time, "place": place, "filters": filters,
    });
    Some((text, json))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;

    /// Saturday 2026-10-17 12:00 UTC.
    fn saturday() -> chrono::DateTime<chrono::Utc> {
        chrono::Utc
            .with_ymd_and_hms(2026, 10, 17, 12, 0, 0)
            .single()
            .unwrap_or_default()
    }

    fn names(types: &[&str], attributes: &[&str]) -> Names {
        Names::new(
            types.iter().map(|t| (*t).to_owned()).collect(),
            attributes.iter().map(|a| (*a).to_owned()).collect(),
        )
    }

    fn omit_of(args: &Args) -> Option<Vec<String>> {
        match args.get("omit") {
            Some(Checked::Many(v)) => Some(v.clone()),
            _ => None,
        }
    }

    fn typed(types: &[&str]) -> Args {
        let mut args = Args::new();
        args.insert(
            "type",
            Checked::Many(types.iter().map(|t| (*t).to_owned()).collect()),
        );
        args
    }

    #[test]
    fn an_article_read_leaves_out_its_body_unless_named() {
        let view = names(&["Article", "Event"], &["name", "content", "dateGmt"]);
        let mut query = typed(&["Article"]);
        lean(&mut query, &view);
        assert_eq!(omit_of(&query), Some(vec!["content".to_owned()]));
        // retrieve_entity names no type: every type the view shows counts
        let mut one = Args::new();
        lean(&mut one, &view);
        assert_eq!(omit_of(&one), Some(vec!["content".to_owned()]));
        for asked in ["attrs", "pick"] {
            let mut named = typed(&["Article"]);
            named.insert(asked, Checked::Many(vec!["id".into(), "content".into()]));
            lean(&mut named, &view);
            assert_eq!(omit_of(&named), None, "{asked}");
        }
        let mut more = typed(&["Article"]);
        more.insert("omit", Checked::Many(vec!["url".into(), "content".into()]));
        lean(&mut more, &view);
        assert_eq!(
            omit_of(&more),
            Some(vec!["url".to_owned(), "content".to_owned()])
        );
        let mut other = typed(&["Article", "Article"]);
        other.insert("omit", Checked::Many(vec!["url".into()]));
        lean(&mut other, &view);
        assert_eq!(
            omit_of(&other),
            Some(vec!["url".to_owned(), "content".to_owned()])
        );
    }

    #[test]
    fn a_type_without_long_text_or_a_view_hiding_it_reads_as_before() {
        let view = names(&["Article", "Event"], &["name", "content"]);
        let mut events = typed(&["Event"]);
        lean(&mut events, &view);
        assert_eq!(omit_of(&events), None);
        let hidden = names(&["Article"], &["name"]);
        let mut articles = typed(&["Article"]);
        lean(&mut articles, &hidden);
        assert_eq!(omit_of(&articles), None);
        let mut any = Args::new();
        lean(&mut any, &names(&["Event"], &["content"]));
        assert_eq!(omit_of(&any), None);
    }

    #[test]
    fn the_article_guide_says_its_body_is_left_out() -> Result<(), String> {
        let shown = text(
            &names(&["Article"], &["name", "content", "dateGmt"]),
            saturday(),
        )
        .ok_or("a guide")?;
        assert!(
            shown.contains("content is left out unless asked for in attrs or pick."),
            "{shown}"
        );
        assert!(shown.contains("time dateGmt (an instant)"), "{shown}");
        let hidden = text(&names(&["Article"], &["name"]), saturday()).ok_or("a guide")?;
        assert!(!hidden.contains("content"), "{hidden}");
        Ok(())
    }

    #[test]
    fn the_week_is_monday_to_monday() {
        assert_eq!(
            this_week(saturday()),
            (
                "2026-10-12T00:00:00Z".to_owned(),
                "2026-10-19T00:00:00Z".to_owned()
            )
        );
    }

    #[test]
    fn an_event_is_a_range_filtered_by_the_week() -> Result<(), String> {
        let guide = text(
            &names(
                &["Event"],
                &["startDate", "endDate", "location", "category", "name"],
            ),
            saturday(),
        )
        .ok_or("a guide")?;
        assert!(
            guide.contains("Event: time startDate..endDate (a range)"),
            "{guide}"
        );
        assert!(
            guide.contains("q=startDate<2026-10-19T00:00:00Z;endDate>=2026-10-12T00:00:00Z"),
            "{guide}"
        );
        assert!(guide.contains("georel=near"), "{guide}");
        assert!(guide.contains("Filter on category, name."), "{guide}");
        Ok(())
    }

    #[test]
    fn a_vehicle_is_located_and_observed() -> Result<(), String> {
        let guide =
            text(&names(&["Vehicle"], &["location", "speed"]), saturday()).ok_or("a guide")?;
        assert!(
            guide.contains("time location.observedAt (an instant)"),
            "{guide}"
        );
        assert!(guide.contains("Place location"), "{guide}");
        Ok(())
    }

    #[test]
    fn a_kpi_reads_its_period_and_region() -> Result<(), String> {
        let guide = text(
            &names(
                &["KeyPerformanceIndicator"],
                &["refPeriod", "refArea", "indicator", "kpiValue"],
            ),
            saturday(),
        )
        .ok_or("a guide")?;
        assert!(guide.contains("time refPeriod (an instant)"), "{guide}");
        assert!(guide.contains("Place refArea; by name"), "{guide}");
        assert!(
            guide.contains("Filter on indicator, refArea, kpiValue."),
            "{guide}"
        );
        Ok(())
    }

    #[test]
    fn half_a_range_is_an_instant_and_a_type_without_filters_says_none() -> Result<(), String> {
        let event =
            text(&names(&["Event"], &["startDate", "location"]), saturday()).ok_or("a guide")?;
        assert!(event.contains("time startDate (an instant)"), "{event}");
        let poi = text(&names(&["PointOfInterest"], &["location"]), saturday()).ok_or("a guide")?;
        assert!(poi.contains("PointOfInterest: no time axis."), "{poi}");
        assert!(!poi.contains("Filter on"), "{poi}");
        assert!(shows(&Names::every(), "anything"));
        Ok(())
    }

    #[test]
    fn a_hidden_attribute_is_never_named_and_an_unknown_type_has_no_line() -> Result<(), String> {
        let guide = text(&names(&["Event", "Thing"], &["name"]), saturday()).ok_or("a guide")?;
        assert!(!guide.contains("startDate"), "{guide}");
        assert!(!guide.contains("location"), "{guide}");
        assert!(guide.contains("Event: no time axis."), "{guide}");
        assert!(!guide.contains("Thing"), "{guide}");
        assert_eq!(text(&names(&["Thing"], &["name"]), saturday()), None);
        assert_eq!(text(&Names::every(), saturday()), None);
        Ok(())
    }

    #[test]
    fn describe_answers_a_shown_type_and_nothing_for_a_hidden_one() -> Result<(), String> {
        let view = names(&["Event"], &["startDate", "endDate", "location", "name"]);
        let (text, json) = describe(&view, "Event", saturday()).ok_or("a guide")?;
        assert!(text.starts_with("Event: time startDate..endDate"), "{text}");
        assert_eq!(
            json,
            serde_json::json!({"type": "Event", "time": {"start": "startDate", "end": "endDate"},
                               "place": ["location"], "filters": ["name"]})
        );
        assert_eq!(describe(&view, "Vehicle", saturday()), None);
        let (_, vehicle) = describe(&names(&["Vehicle"], &["location"]), "Vehicle", saturday())
            .ok_or("a vehicle guide")?;
        assert_eq!(
            vehicle.get("time"),
            Some(&serde_json::json!({"instant": "location.observedAt"}))
        );
        assert_eq!(describe(&view, "Thing", saturday()), None);
        let bare = describe(&names(&["Event"], &["name"]), "Event", saturday());
        let shown = format!("{bare:?}");
        assert!(!shown.contains("startDate"), "{shown}");
        assert!(!shown.contains("location"), "{shown}");
        Ok(())
    }
}
