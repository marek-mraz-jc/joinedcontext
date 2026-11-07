// SPDX-License-Identifier: EUPL-1.2
//! Unit tests of the temporal API, by clause.

use super::*;

#[cfg(test)]
mod clause_4_11 {
    use super::*;
    use serde_json::json;

    /// 5.7.3.4 / 4.5.5.3: instances with the same datasetId (or both default)
    /// AND the same timeproperty value are CONFLICTING instances of one slot
    /// — the merge resolves them to one (most recent modifiedAt wins), never
    /// serves both. Regression: the same instance held by two federated
    /// brokers came back twice (IOP_EXT_TMP_02_05).
    #[tokio::test]
    async fn merge_resolves_same_slot_instances_to_one() {
        let mut base = json!({"id": "urn:e", "type": "T", "speed": [
            {"type": "Property", "value": 10, "observedAt": "2026-05-01T00:00:00Z",
             "modifiedAt": "2026-05-01T00:00:00Z"},
        ]});
        let add = json!({"id": "urn:e", "type": "T", "speed": [
            {"type": "Property", "value": 11, "observedAt": "2026-05-01T00:00:00Z",
             "modifiedAt": "2026-06-01T00:00:00Z"},
            {"type": "Property", "value": 20, "observedAt": "2026-05-02T00:00:00Z",
             "modifiedAt": "2026-05-02T00:00:00Z"},
            {"type": "Property", "value": 30, "observedAt": "2026-05-01T00:00:00Z",
             "datasetId": "urn:d:1", "modifiedAt": "2026-05-01T00:00:00Z"},
        ]});
        merge_temporal_docs(&mut base, &add, false, "observedAt");
        let speed = base["speed"].as_array().expect("array");
        // default-instance duplicate collapsed (newer modifiedAt won),
        // the other timestamp appended, the datasetId instance is its own slot
        assert_eq!(speed.len(), 3, "{speed:?}");
        let default_slot: Vec<&Value> = speed
            .iter()
            .filter(|i| i.get("datasetId").is_none() && i["observedAt"] == "2026-05-01T00:00:00Z")
            .collect();
        assert_eq!(default_slot.len(), 1);
        assert_eq!(default_slot[0]["value"], 11, "newer modifiedAt wins");
    }

    /// 4.3.6.2: "An auxiliary Context Source Registration never overrides
    /// data held directly within a Context Broker. […] Context data from
    /// auxiliary context sources is only included if it is supplementary to
    /// the context data otherwise available to the Context Broker." On a
    /// Temporal Evolution the unit is the instance, so an auxiliary instance
    /// enters only where no other source supplied that timeproperty value.
    #[tokio::test]
    async fn an_auxiliary_instance_supplements_but_never_overrides() {
        let mut base = json!({"id": "urn:e", "type": "T", "speed": [
            {"type": "Property", "value": 10, "observedAt": "2026-05-01T00:00:00Z",
             "modifiedAt": "2026-05-01T00:00:00Z"},
        ]});
        let add = json!({"id": "urn:e", "type": "T", "speed": [
            // same slot as the local instance, and newer — still refused
            {"type": "Property", "value": 99, "observedAt": "2026-05-01T00:00:00Z",
             "modifiedAt": "2026-07-01T00:00:00Z"},
            // a timestamp nobody else supplied: supplementary, so included
            {"type": "Property", "value": 20, "observedAt": "2026-05-02T00:00:00Z"},
        ]});
        merge_temporal_docs(&mut base, &add, true, "observedAt");
        let speed = base["speed"].as_array().expect("array");
        assert_eq!(speed.len(), 2, "{speed:?}");
        assert!(
            !Value::Array(speed.clone()).to_string().contains("99"),
            "an auxiliary instance may not override an occupied slot: {speed:?}"
        );
        assert_eq!(speed[0]["value"], 10);
        assert_eq!(speed[1]["value"], 20);
    }

    /// 4.6.3 leaves the seconds fraction optional, so two Context Sources
    /// holding one instance may spell its timeproperty differently. The slot
    /// of 4.5.5.3 is the INSTANT, not the spelling: a byte comparison treats
    /// the two as separate slots and serves the same instance twice — the
    /// IOP_EXT_TMP_02_05 duplicate, reached through a different door.
    #[tokio::test]
    async fn one_instant_spelled_two_ways_is_still_one_slot() {
        let mut base = json!({"id": "urn:e", "type": "T", "speed": [
            {"type": "Property", "value": 10, "observedAt": "2026-05-01T00:00:00Z",
             "modifiedAt": "2026-05-01T00:00:00Z"},
        ]});
        let add = json!({"id": "urn:e", "type": "T", "speed": [
            {"type": "Property", "value": 11, "observedAt": "2026-05-01T00:00:00.000Z",
             "modifiedAt": "2026-06-01T00:00:00Z"},
        ]});
        merge_temporal_docs(&mut base, &add, false, "observedAt");
        let speed = base["speed"].as_array().expect("array");
        assert_eq!(speed.len(), 1, "one instant is one slot: {speed:?}");
        assert_eq!(speed[0]["value"], 11, "newer modifiedAt wins");
    }

    /// The winner of a conflicting slot is "the most recent modifiedAt", and
    /// which of two `modifiedAt` values is more recent is a comparison of
    /// instants for the same reason. A remote instance stamped
    /// `…:00.500Z` is LATER than a local one stamped `…:00Z`, though its
    /// bytes sort earlier.
    #[tokio::test]
    async fn the_more_recent_modified_at_wins_across_fraction_spellings() {
        let mut base = json!({"id": "urn:e", "type": "T", "speed": [
            {"type": "Property", "value": 10, "observedAt": "2026-05-01T00:00:00Z",
             "modifiedAt": "2026-05-01T09:00:00Z"},
        ]});
        let add = json!({"id": "urn:e", "type": "T", "speed": [
            {"type": "Property", "value": 11, "observedAt": "2026-05-01T00:00:00Z",
             "modifiedAt": "2026-05-01T09:00:00.500Z"},
        ]});
        merge_temporal_docs(&mut base, &add, false, "observedAt");
        let speed = base["speed"].as_array().expect("array");
        assert_eq!(speed.len(), 1, "{speed:?}");
        assert_eq!(speed[0]["value"], 11, "the later instant wins: {speed:?}");
    }

    /// 4.3.6.2 auxiliary supplementation is decided on the same slot, so an
    /// auxiliary instance that respells an occupied instant is still refused.
    #[tokio::test]
    async fn an_auxiliary_respelling_of_an_occupied_slot_is_refused() {
        let mut base = json!({"id": "urn:e", "type": "T", "speed": [
            {"type": "Property", "value": 10, "observedAt": "2026-05-01T00:00:00Z"},
        ]});
        let add = json!({"id": "urn:e", "type": "T", "speed": [
            {"type": "Property", "value": 99, "observedAt": "2026-05-01T00:00:00.000Z"},
        ]});
        merge_temporal_docs(&mut base, &add, true, "observedAt");
        let speed = base["speed"].as_array().expect("array");
        assert_eq!(speed.len(), 1, "{speed:?}");
        assert_eq!(speed[0]["value"], 10);
    }
}

#[cfg(test)]
mod clause_6_3_10 {
    use super::*;
    use serde_json::json;

    /// The ceiling the ETSI suite's brokers run with (ANTARES_TEMPORAL_INSTANCE_LIMIT=9):
    /// these cases cut 20 instances as its 206 checks do.
    const ETSI_LIMIT: usize = 9;

    /// One attribute with `n` instances, one per minute from 00:00.
    fn evolution(n: usize) -> Value {
        let speed: Vec<Value> = (0..n)
            .map(|i| json!({"type": "Property", "value": i, "observedAt": at(i)}))
            .collect();
        json!({"id": "urn:ngsi-ld:Vehicle:1", "type": "Vehicle", "speed": speed})
    }

    fn at(i: usize) -> String {
        format!("2020-01-01T00:{i:02}:00Z")
    }

    fn tq(timerel: &str, time_at: &str) -> TemporalQ {
        let mut p = HashMap::new();
        p.insert("timerel".to_owned(), timerel.to_owned());
        p.insert("timeAt".to_owned(), time_at.to_owned());
        TemporalQ::from_params(&p, true).unwrap().unwrap()
    }

    async fn windowed(doc: &Value, tq: Option<&TemporalQ>, last_n: Option<usize>) -> Windowed {
        let mut w = window(
            doc,
            tq,
            page(last_n),
            None,
            None,
            None,
            "observedAt",
            Default::default(),
        );
        truncate(&mut w, "observedAt", last_n.is_some(), ETSI_LIMIT);
        w
    }

    /// The page of a request naming lastN, or neither firstN nor lastN.
    fn page(last_n: Option<usize>) -> InstancePage {
        InstancePage {
            desc: last_n.is_some(),
            n: last_n,
            offset: 0,
        }
    }

    fn observed(w: &Windowed) -> Vec<&str> {
        w.attrs["speed"]
            .iter()
            .map(|i| i["observedAt"].as_str().unwrap())
            .collect()
    }

    /// F-ANT1-017: with no firstN/lastN the store may cap each Attribute at
    /// the default page plus one, oldest first: the trim over the capped
    /// history serves what it serves over the whole one, still cut (206).
    #[tokio::test]
    async fn the_default_trim_cap_keeps_the_answer() {
        assert_eq!(page(None).store_cap(Some(ETSI_LIMIT)), Some(10));
        assert_eq!(page(None).store_cap(None), None);
        assert_eq!(page(Some(3)).store_cap(Some(ETSI_LIMIT)), Some(4));
        let q = tq("after", "2019-01-01T00:00:00Z");
        let whole = windowed(&evolution(59), Some(&q), None).await;
        let capped = windowed(&evolution(10), Some(&q), None).await;
        assert_eq!(observed(&capped), observed(&whole));
        assert_eq!(observed(&whole).len(), ETSI_LIMIT);
        assert_eq!(
            (capped.truncated, capped.more),
            (whole.truncated, whole.more)
        );
        assert_eq!(
            (&capped.ts_min, &capped.ts_max, capped.cut_served),
            (&whole.ts_min, &whole.ts_max, whole.cut_served)
        );
        assert!(capped.truncated);
    }

    /// 6.3.10: a temporal retrieval the broker cannot serve in full is
    /// answered with 206 and a Content-Range. The body must then BE the
    /// partial representation the header describes — a window wide enough to
    /// select more instances than the broker serves at once is cut to the
    /// ceiling, oldest first, and the advertised range ends at the last
    /// instance returned.
    #[tokio::test]
    async fn wide_window_is_cut_to_the_ceiling_and_the_range_matches_the_body() {
        let doc = evolution(20);
        let q = tq("after", "2019-01-01T00:00:00Z");
        let w = windowed(&doc, Some(&q), None).await;
        assert_eq!(w.attrs["speed"].len(), ETSI_LIMIT);
        assert!(w.truncated);
        let got = observed(&w);
        assert_eq!(got[0], at(0));
        assert_eq!(got[ETSI_LIMIT - 1], at(ETSI_LIMIT - 1));
        assert!(
            !got.contains(&at(19).as_str()),
            "instances beyond the ceiling must not be served: {got:?}"
        );
        // 5.7.3.4 + 6.3.10: start is the requested lower bound, end the last
        // instance in the body, so the header cannot promise more than it sent
        assert_eq!(
            content_range(
                w.truncated,
                w.ts_min.as_deref(),
                w.ts_max.as_deref(),
                Some(&q),
                page(None)
            ),
            Some(format!(
                "date-time 2019-01-01T00:00:00Z-{}/*",
                at(ETSI_LIMIT - 1)
            ))
        );
    }

    /// 5-minute readings: the i-th instant from 2026-10-12T00:00Z on.
    fn five_minute(i: usize) -> String {
        let t0 = chrono::DateTime::parse_from_rfc3339("2026-10-12T00:00:00Z").expect("instant");
        (t0 + chrono::Duration::minutes(5 * i as i64))
            .to_utc()
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string()
    }

    fn windowed_at(n: usize) -> Windowed {
        let speed: Vec<Value> = (0..n)
            .map(|i| json!({"type": "Property", "value": i, "observedAt": five_minute(i)}))
            .collect();
        let doc = json!({"id": "urn:ngsi-ld:WeatherObserved:1", "type": "WeatherObserved", "speed": speed});
        let q = tq("after", "2026-10-01T00:00:00Z");
        let mut w = window(
            &doc,
            Some(&q),
            page(None),
            None,
            None,
            None,
            "observedAt",
            Default::default(),
        );
        truncate(
            &mut w,
            "observedAt",
            false,
            crate::bounds::temporal_instance_limit(),
        );
        w
    }

    /// T-4800: with no setting the ceiling is 1,000: 1,000 instances of an
    /// Attribute answer whole (200), 1,001 are cut at the 1,000th (206).
    #[test]
    fn the_default_ceiling_serves_1000_whole_and_cuts_1001() {
        const CEILING: usize = 1000;
        assert_eq!(
            crate::bounds::temporal_instance_limit(),
            CEILING,
            "ANTARES_TEMPORAL_INSTANCE_LIMIT is unset in tests"
        );
        let q = tq("after", "2026-10-01T00:00:00Z");
        let w = windowed_at(CEILING);
        assert!(!w.truncated && !w.more);
        assert_eq!(w.attrs["speed"].len(), CEILING);

        let w = windowed_at(CEILING + 1);
        assert!(w.truncated && w.more);
        assert_eq!(w.attrs["speed"].len(), CEILING);
        assert_eq!(
            content_range(
                w.truncated,
                w.ts_min.as_deref(),
                w.ts_max.as_deref(),
                Some(&q),
                page(None)
            ),
            Some(format!(
                "date-time 2026-10-01T00:00:00Z-{}/*",
                five_minute(CEILING - 1)
            ))
        );
    }

    /// T-4804 (owner: default 100, maximum 1,000): with no lastN a request
    /// gets the broker's default of 100 instances per Attribute, cut with a
    /// 206 range ending at the 100th; lastN=1000 gets all 1,000.
    #[test]
    fn no_last_n_serves_the_default_100_and_last_n_1000_serves_1000() {
        assert_eq!(
            crate::bounds::temporal_default_instances(),
            100,
            "ANTARES_TEMPORAL_DEFAULT_INSTANCES is unset in tests"
        );
        let served = |n: usize, last_n: Option<usize>| {
            let speed: Vec<Value> = (0..n)
                .map(|i| json!({"type": "Property", "value": i, "observedAt": five_minute(i)}))
                .collect();
            let doc = json!({"id": "urn:ngsi-ld:WeatherObserved:1", "type": "WeatherObserved", "speed": speed});
            let q = tq("after", "2026-10-01T00:00:00Z");
            let mut w = window(
                &doc,
                Some(&q),
                page(last_n),
                None,
                None,
                None,
                "observedAt",
                Default::default(),
            );
            let cut = page(last_n).size(
                crate::bounds::temporal_default_instances(),
                crate::bounds::temporal_instance_limit(),
            );
            truncate(&mut w, "observedAt", last_n.is_some(), cut);
            (w, q)
        };
        let (w, q) = served(101, None);
        assert!(w.truncated && w.more);
        assert_eq!(w.attrs["speed"].len(), 100);
        assert_eq!(
            content_range(
                w.truncated,
                w.ts_min.as_deref(),
                w.ts_max.as_deref(),
                Some(&q),
                page(None)
            ),
            Some(format!(
                "date-time 2026-10-01T00:00:00Z-{}/*",
                five_minute(99)
            ))
        );
        let (w, _) = served(1000, Some(1000));
        assert!(!w.truncated);
        assert_eq!(w.attrs["speed"].len(), 1000);
        // the ETSI brokers' ceiling of 9 stays below the default
        assert_eq!(page(None).size(100, ETSI_LIMIT), ETSI_LIMIT);
        assert_eq!(page(Some(1000)).size(100, 1000), 1000);
    }

    /// Two attributes, `speed` minutes 0..n and `heading` minutes 5..m.
    fn evolution2(n: usize, m: usize) -> Value {
        let mut doc = evolution(n);
        doc["heading"] = (5..m)
            .map(|i| json!({"type": "Property", "value": i, "observedAt": at(i)}))
            .collect();
        doc
    }

    fn last_observed(w: &Windowed, attr: &str) -> Option<String> {
        w.attrs[attr]
            .iter()
            .filter_map(|i| i["observedAt"].as_str().map(str::to_owned))
            .max()
    }

    /// T-3613: the time continuation keeps the window's other end, and
    /// moves the end the page reached in its order.
    #[test]
    fn a_page_cut_to_nothing_resumes_past_its_boundary() {
        let p = |kv: &[(&str, &str)]| -> HashMap<String, String> {
            kv.iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        let (a, b, e) = (
            "2020-01-01T00:00:00Z",
            "2020-01-01T00:08:00Z",
            "2020-01-02T00:00:00Z",
        );
        let between = |from: &str, to: &str| {
            vec![
                ("timerel", "between".to_owned()),
                ("timeAt", from.to_owned()),
                ("endTimeAt", to.to_owned()),
            ]
        };
        let one =
            |rel: &str, at: &str| vec![("timerel", rel.to_owned()), ("timeAt", at.to_owned())];
        let window = p(&[("timerel", "between"), ("timeAt", a), ("endTimeAt", e)]);
        assert_eq!(resume_after(&window, false, b), between(b, e));
        assert_eq!(resume_after(&window, true, b), between(a, b));
        let before = p(&[("timerel", "before"), ("timeAt", e)]);
        assert_eq!(resume_after(&before, false, b), between(b, e));
        assert_eq!(resume_after(&before, true, b), one("before", b));
        let after = p(&[("timerel", "after"), ("timeAt", a)]);
        assert_eq!(resume_after(&after, false, b), one("after", b));
        assert_eq!(resume_after(&after, true, b), between(a, b));
        assert_eq!(resume_after(&p(&[]), false, b), one("after", b));
        assert_eq!(resume_after(&p(&[]), true, b), one("before", b));
    }

    /// 6.3.10: the partial content IS the representation the Content-Range
    /// describes — so the cut is one time boundary for the whole entity.
    /// With `speed` over-full first, `heading` is trimmed to the same last
    /// instant, and nothing of either attribute lies past the advertised
    /// range-end (a client continuing from it misses no instance).
    #[tokio::test]
    async fn the_cut_is_one_time_boundary_across_attributes() {
        let w = windowed(&evolution2(21, 31), None, None).await;
        assert!(w.truncated);
        let end = at(ETSI_LIMIT - 1);
        assert_eq!(w.attrs["speed"].len(), ETSI_LIMIT);
        assert_eq!(last_observed(&w, "speed").as_deref(), Some(end.as_str()));
        assert_eq!(
            last_observed(&w, "heading").as_deref(),
            Some(end.as_str()),
            "heading must stop at speed's boundary, not at its own ninth instance"
        );
        assert_eq!(w.attrs["heading"].len(), ETSI_LIMIT - 5);
        assert_eq!(w.ts_max.as_deref(), Some(end.as_str()));
        for inst in w.attrs["heading"].iter().chain(w.attrs["speed"].iter()) {
            assert!(
                inst["observedAt"].as_str().expect("t") <= end.as_str(),
                "no instance may lie past the advertised range-end: {inst}"
            );
        }
        // backwards (lastN): the boundary is the LATEST ninth instant, so the
        // page covers [heading's ninth-newest, newest] and `speed`, which ends
        // before that, is empty on this page rather than incoherently present
        let w = windowed(&evolution2(21, 31), None, Some(20)).await;
        assert!(w.truncated);
        assert_eq!(w.attrs["heading"].len(), ETSI_LIMIT);
        assert_eq!(w.attrs["heading"][0]["observedAt"], json!(at(30)));
        assert_eq!(w.ts_min.as_deref(), Some(at(30 - ETSI_LIMIT + 1).as_str()));
        assert!(
            w.attrs["speed"].is_empty(),
            "speed lies entirely before the page boundary: {:?}",
            w.attrs["speed"]
        );
    }

    /// 5.7.3.4/5.7.4.4 lastN "shall be limited to the specified number of
    /// instances" — an upper limit, not an entitlement: a lastN above the
    /// broker ceiling is served up to the ceiling (newest first) and the
    /// answer is the partial one. The Content-Range size stays the requested
    /// lastN, its start-end pair the instants actually returned.
    #[tokio::test]
    async fn last_n_above_the_ceiling_is_clamped_to_it() {
        let doc = evolution(20);
        let w = windowed(&doc, None, Some(20)).await;
        assert_eq!(w.attrs["speed"].len(), ETSI_LIMIT);
        assert!(w.truncated);
        let got = observed(&w);
        assert_eq!(got[0], at(19), "lastN delivers newest first");
        assert_eq!(got[ETSI_LIMIT - 1], at(20 - ETSI_LIMIT));
        assert_eq!(
            content_range(
                w.truncated,
                w.ts_min.as_deref(),
                w.ts_max.as_deref(),
                None,
                page(Some(20))
            ),
            Some(format!("date-time {}-{}/20", at(19), at(20 - ETSI_LIMIT)))
        );
    }

    /// 5.7.3.4: temporalQ is optional on retrieval, so a request naming no
    /// window at all asks for the whole Temporal Evolution. It is still
    /// bounded by the same ceiling, and still answered as partial.
    #[tokio::test]
    async fn a_request_with_no_window_is_capped_by_default() {
        let doc = evolution(20);
        let w = windowed(&doc, None, None).await;
        assert_eq!(w.attrs["speed"].len(), ETSI_LIMIT);
        assert!(w.truncated);
        assert_eq!(w.ts_max.as_deref(), Some(at(ETSI_LIMIT - 1).as_str()));
        assert_eq!(
            content_range(
                w.truncated,
                w.ts_min.as_deref(),
                w.ts_max.as_deref(),
                None,
                page(None)
            ),
            Some(format!("date-time {}-{}/*", at(0), at(ETSI_LIMIT - 1)))
        );
    }

    /// 6.3.10: partial content is conditional on truncation — a result the
    /// broker serves in full is a plain 200 with no Content-Range.
    #[tokio::test]
    async fn a_complete_result_is_not_partial_content() {
        let doc = evolution(ETSI_LIMIT);
        let w = windowed(&doc, None, None).await;
        assert_eq!(w.attrs["speed"].len(), ETSI_LIMIT);
        assert!(!w.truncated);
        assert_eq!(
            content_range(
                w.truncated,
                w.ts_min.as_deref(),
                w.ts_max.as_deref(),
                None,
                page(None)
            ),
            None
        );
    }

    /// 4.6.3 allows a DateTime to carry a seconds fraction or leave it out,
    /// and both spellings of one instant are the same instant. The window's
    /// own bounds are compared as raw strings nowhere: `.` sorts before `Z`,
    /// so `…:00.500Z` reads as EARLIER than `…:00Z` on a byte compare, and a
    /// Content-Range built from those bounds would name a range the body
    /// contradicts. The instances are sorted on the canonical key already
    /// (`dt_key`); the bounds are on the same key or they disagree with the
    /// order they summarize.
    #[tokio::test]
    async fn the_window_bounds_are_the_true_extremes_across_fraction_spellings() {
        let doc = json!({
            "id": "urn:ngsi-ld:Vehicle:1",
            "type": "Vehicle",
            "speed": [
                {"type": "Property", "value": 1, "observedAt": "2020-01-01T00:09:00Z"},
                {"type": "Property", "value": 2, "observedAt": "2020-01-01T00:09:00.500Z"},
            ],
        });
        let w = windowed(&doc, None, None).await;
        assert_eq!(w.attrs["speed"].len(), 2);
        assert_eq!(
            observed(&w),
            vec!["2020-01-01T00:09:00Z", "2020-01-01T00:09:00.500Z"],
            "the instances themselves sort on the canonical key"
        );
        assert_eq!(w.ts_min.as_deref(), Some("2020-01-01T00:09:00Z"));
        assert_eq!(w.ts_max.as_deref(), Some("2020-01-01T00:09:00.500Z"));
    }
}

#[cfg(test)]
mod forwarded_path_encoding {
    use crate::AppState;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

    /// An entity id is a URI (4.6.2), and `#` is legal in one. It also ends a
    /// path in RFC 3986 clause 3.3, so the id has to be percent-encoded
    /// wherever it becomes a path segment.
    const ENTITY: &str = "urn:ngsi-ld:Vehicle:temporal-enc#frag";
    const ENCODED: &str = "urn:ngsi-ld:Vehicle:temporal-enc%23frag";

    /// A Context Source answering 204 to everything, recording request lines.
    fn mock_source() -> (u16, Arc<Mutex<Vec<String>>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                let mut buf = [0u8; 8192];
                let n = s.read(&mut buf).unwrap_or(0);
                if let Some(line) = String::from_utf8_lossy(&buf[..n]).lines().next() {
                    log.lock().expect("lock").push(line.to_owned());
                }
                let _ = s.write_all(
                    b"HTTP/1.1 204 No Content\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
                );
            }
        });
        (port, seen)
    }

    fn state() -> AppState {
        // the mock source is loopback, denied by the egress policy by default
        crate::allow_private();
        AppState::new("antares-temporal-enc".into())
    }

    async fn send(st: &AppState, req: Request<Body>) -> axum::http::Response<Body> {
        crate::router(st.clone())
            .oneshot(req)
            .await
            .expect("response")
    }

    async fn post(st: &AppState, uri: &str, body: String) -> axum::http::Response<Body> {
        let req = Request::builder()
            .method("POST")
            .uri(uri)
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request");
        send(st, req).await
    }

    async fn register(st: &AppState, port: u16, id: &str, entity: &str) {
        let doc = serde_json::json!({
            "id": format!("urn:ngsi-ld:ContextSourceRegistration:{id}"),
            "type": "ContextSourceRegistration",
            "mode": "redirect",
            "operations": ["deleteAttrsTemporal", "deleteAttrInstanceTemporal"],
            "information": [{"entities": [{"type": "Vehicle", "id": entity}]}],
            "endpoint": format!("http://127.0.0.1:{port}"),
        });
        assert_eq!(
            post(st, "/ngsi-ld/v1/csourceRegistrations", doc.to_string())
                .await
                .status(),
            StatusCode::CREATED,
            "registration create"
        );
    }

    /// 5.6.13.4/5.6.15.4: the operation is forwarded to the registration
    /// endpoint with the target resource named in the request path. The id,
    /// the Attribute name and the instanceId arrive percent-decoded from this
    /// broker's own path, so splicing them raw would let a `#` end the
    /// forwarded path (RFC 3986 clause 3.3) and turn Delete Attribute into
    /// Delete Temporal Evolution of an Entity (5.6.16) on the peer.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_hash_in_the_id_reaches_the_peer_encoded_not_truncated() {
        let st = state();
        let (port, seen) = mock_source();
        register(&st, port, "csr-temporal-enc", ENTITY).await;

        // each suffix is already in its encoded spelling, so the forwarded
        // path must repeat it verbatim
        for suffix in ["/attrs/speed", "/attrs/speed/urn:ngsi-ld:Instance:1%23x"] {
            let req = Request::builder()
                .method("DELETE")
                .uri(format!("/ngsi-ld/v1/temporal/entities/{ENCODED}{suffix}"))
                .body(Body::empty())
                .expect("request");
            let status = send(&st, req).await.status();
            assert_ne!(status, StatusCode::BAD_REQUEST, "{suffix}");
            let lines = seen.lock().expect("lock").clone();
            let last = lines.last().cloned().unwrap_or_default();
            assert!(
                last.contains(&format!("/ngsi-ld/v1/temporal/entities/{ENCODED}{suffix}")),
                "forwarded request line {last:?} for suffix {suffix}"
            );
            // the negative assertion: the peer must never see a path that
            // stops at the entity resource
            assert!(
                !last.contains("temporal-enc HTTP/"),
                "forwarded path truncated at the `#`: {last:?}"
            );
        }
    }

    /// 5.6.13.4: "If the target Attribute name is not a valid name, then an
    /// error of type BadRequestData shall be raised." A name begins with a
    /// letter (4.6.2), so no valid name is a relative-path dot segment (RFC
    /// 3986 clause 5.2.4) — and such a name in a forwarded path would address
    /// the peer's Temporal Evolution resource instead of its Attribute, so it
    /// is refused before anything leaves this broker.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_dot_segment_attribute_name_is_refused_and_never_forwarded() {
        const TARGET: &str = "urn:ngsi-ld:Vehicle:temporal-dots";
        let st = state();
        let (port, seen) = mock_source();
        register(&st, port, "csr-temporal-dots", TARGET).await;
        // raw, decoded once by this broker, and decoded once more by the peer
        for attr in ["..", "%2e%2e", "%252e%252e", "."] {
            for suffix in ["", "/urn:ngsi-ld:Instance:1"] {
                let req = Request::builder()
                    .method("DELETE")
                    .uri(format!(
                        "/ngsi-ld/v1/temporal/entities/{TARGET}/attrs/{attr}{suffix}"
                    ))
                    .body(Body::empty())
                    .expect("request");
                assert_eq!(
                    send(&st, req).await.status(),
                    StatusCode::BAD_REQUEST,
                    "attribute name {attr:?} with suffix {suffix:?}"
                );
            }
        }
        assert!(
            seen.lock().expect("lock").is_empty(),
            "a rejected attribute name must never reach a registration endpoint"
        );
    }

    /// 5.6.11.4: on creation the response carries a Location header holding
    /// the resource URI of the created Temporal Representation. A URI has its
    /// reserved characters percent-encoded (RFC 3986 clause 3.3), so a `#` in
    /// the id may not be spliced raw — there it would read as the start of a
    /// fragment identifier and address the entity collection.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_location_header_percent_encodes_the_id() {
        let st = state();
        let doc = serde_json::json!({
            "id": ENTITY, "type": "Vehicle",
            "speed": [{"type": "Property", "value": 1,
                       "observedAt": "2026-03-01T12:05:00Z"}],
        });
        let res = post(&st, "/ngsi-ld/v1/temporal/entities", doc.to_string()).await;
        assert_eq!(res.status(), StatusCode::CREATED);
        assert_eq!(
            res.headers()
                .get("Location")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default(),
            format!("/ngsi-ld/v1/temporal/entities/{ENCODED}")
        );
    }
}

#[cfg(test)]
mod clause_4_5_19 {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_fractional_period_is_refused_never_truncated() {
        // F-R2ANT2-015: PT0.5S summed to 0 s and read as one bucket
        assert_eq!(parse_aggr_period("PT0.5S"), None);
        assert_eq!(parse_aggr_period("P1.5M"), None);
    }

    /// 4.5.19.1: "The duration shall be a string in the format
    /// `P[n]Y[n]M[n]DT[n]H[n]M[n]S` or `P[n]W` … For example,
    /// `"P3Y6M4DT12H30M5S"` represents a duration of "three years, six
    /// months, four days, twelve hours, thirty minutes, and five seconds"."
    /// A period mixing date and time elements is therefore valid, and
    /// "PT0S" spans the whole time range of the query.
    #[test]
    fn a_mixed_date_and_time_duration_is_a_valid_period() {
        const DAY: i64 = 86_400;
        assert_eq!(
            parse_aggr_period("P3Y6M4DT12H30M5S"),
            Some(AggrPeriod::Months(42, 4 * DAY + 12 * 3600 + 30 * 60 + 5))
        );
        assert_eq!(
            parse_aggr_period("P1Y1D"),
            Some(AggrPeriod::Months(12, DAY))
        );
        assert_eq!(
            parse_aggr_period("P1MT1H"),
            Some(AggrPeriod::Months(1, 3600))
        );
        // the pure forms are unchanged
        assert_eq!(parse_aggr_period("PT0S"), Some(AggrPeriod::Whole));
        assert_eq!(parse_aggr_period("P0D"), Some(AggrPeriod::Whole));
        assert_eq!(parse_aggr_period("P1M"), Some(AggrPeriod::Months(1, 0)));
        assert_eq!(parse_aggr_period("PT90M"), Some(AggrPeriod::Seconds(5400)));
        assert_eq!(parse_aggr_period("P1W"), Some(AggrPeriod::Seconds(7 * DAY)));
        // and the grammar still rejects what is not a duration
        assert_eq!(parse_aggr_period("P1X"), None);
        assert_eq!(parse_aggr_period("1Y"), None);
        assert_eq!(parse_aggr_period("P1"), None);
    }

    fn windowed(times: &[&str]) -> Windowed {
        let instances: Vec<Value> = times
            .iter()
            .map(|t| json!({"type": "Property", "value": 1, "observedAt": t}))
            .collect();
        let mut attrs = std::collections::BTreeMap::new();
        attrs.insert("speed".to_owned(), instances);
        Windowed {
            attrs,
            max_per_attr: times.len(),
            ts_min: times.first().map(|s| (*s).to_owned()),
            ts_max: times.last().map(|s| (*s).to_owned()),
            truncated: false,
            cut_served: None,
            more: false,
            held: Default::default(),
        }
    }

    fn repr(duration: &str) -> TRepr {
        TRepr {
            aggregated: true,
            aggr_methods: vec!["totalCount".to_owned()],
            aggr_period: parse_aggr_period(duration).expect("duration"),
            ..Default::default()
        }
    }

    /// The periods of an aggregated response are the periods "in the time
    /// range of the query" (4.5.19.0), so with `timerel=before` they run
    /// backwards from `timeAt` and every returned period contains the
    /// instances aggregated into it.
    #[tokio::test]
    async fn month_periods_before_the_anchor_contain_their_instances() {
        let w = windowed(&[
            "2020-01-15T00:00:00Z",
            "2020-02-15T00:00:00Z",
            "2020-03-15T00:00:00Z",
        ]);
        let tq = TemporalQ {
            timerel: "before".to_owned(),
            time_at: "2020-04-01T00:00:00Z".to_owned(),
            end_time_at: None,
            timeproperty: "observedAt".to_owned(),
        };
        let out = render_aggregated(
            &w,
            Some(&tq),
            &repr("P1M"),
            &antares_jsonld::Context::default(),
            "observedAt",
        )
        .expect("aggregated");
        let rows = out["speed"]["totalCount"].as_array().expect("rows").clone();
        assert_eq!(
            rows,
            vec![
                json!([1, "2020-01-01T00:00:00Z", "2020-02-01T00:00:00Z"]),
                json!([1, "2020-02-01T00:00:00Z", "2020-03-01T00:00:00Z"]),
                json!([1, "2020-03-01T00:00:00Z", "2020-04-01T00:00:00Z"]),
            ]
        );
        // the negative assertion: no period may start at the anchor, since
        // such a period holds none of the instances of a `before` query
        assert!(
            !Value::Array(rows)
                .to_string()
                .contains("2020-04-01T00:00:00Z\",\""),
            "a period starting at timeAt contains no instance of a before query"
        );
    }

    /// 4.5.19.1: "A duration of 0 second (e.g. expressed as "PT0S" or
    /// "P0D") is valid and is interpreted as a duration spanning the whole
    /// time range specified by the temporal query." The query names one
    /// edge of that range and 4.11 leaves the other open, so the period
    /// runs from `timeAt` only when `timeAt` is where the range starts.
    #[tokio::test]
    async fn the_zero_duration_period_spans_the_time_range_the_query_asked_for() {
        let w = windowed(&["2020-09-01T12:03:00Z", "2020-09-01T12:05:00Z"]);
        let one = |tq: &TemporalQ| {
            let out = render_aggregated(
                &w,
                Some(tq),
                &repr("PT0S"),
                &antares_jsonld::Context::default(),
                "observedAt",
            )
            .expect("aggregated");
            out["speed"]["totalCount"].as_array().expect("rows").clone()
        };

        // before: timeAt ENDS the range (4.11 makes the start open, so the
        // data supplies it). The period must not run backwards.
        assert_eq!(
            one(&TemporalQ {
                timerel: "before".to_owned(),
                time_at: "2030-01-01T00:00:00Z".to_owned(),
                end_time_at: None,
                timeproperty: "observedAt".to_owned(),
            }),
            vec![json!([2, "2020-09-01T12:03:00Z", "2030-01-01T00:00:00Z"])]
        );

        // between: both edges are named, and the period is exactly them —
        // not the last instant the data happens to hold.
        assert_eq!(
            one(&TemporalQ {
                timerel: "between".to_owned(),
                time_at: "2020-09-01T12:00:00Z".to_owned(),
                end_time_at: Some("2020-09-01T13:00:00Z".to_owned()),
                timeproperty: "observedAt".to_owned(),
            }),
            vec![json!([2, "2020-09-01T12:00:00Z", "2020-09-01T13:00:00Z"])]
        );

        // after: timeAt STARTS the range, the data closes it.
        assert_eq!(
            one(&TemporalQ {
                timerel: "after".to_owned(),
                time_at: "2020-01-01T00:00:00Z".to_owned(),
                end_time_at: None,
                timeproperty: "observedAt".to_owned(),
            }),
            vec![json!([2, "2020-01-01T00:00:00Z", "2020-09-01T12:05:01Z"])]
        );
    }

    /// A mixed period steps by its months AND its seconds: "P1MT12H" from
    /// the anchor ends one month and twelve hours later (4.5.19.1).
    #[tokio::test]
    async fn a_mixed_period_steps_by_both_components() {
        let w = windowed(&["2020-01-01T06:00:00Z", "2020-02-02T06:00:00Z"]);
        let tq = TemporalQ {
            timerel: "after".to_owned(),
            time_at: "2020-01-01T00:00:00Z".to_owned(),
            end_time_at: None,
            timeproperty: "observedAt".to_owned(),
        };
        let out = render_aggregated(
            &w,
            Some(&tq),
            &repr("P1MT12H"),
            &antares_jsonld::Context::default(),
            "observedAt",
        )
        .expect("aggregated");
        assert_eq!(
            out["speed"]["totalCount"],
            json!([
                [1, "2020-01-01T00:00:00Z", "2020-02-01T12:00:00Z"],
                [1, "2020-02-01T12:00:00Z", "2020-03-02T00:00:00Z"],
            ])
        );
    }

    /// One attribute whose instances carry the given members, an hour apart.
    fn windowed_props(members: &[Value]) -> Windowed {
        let instances: Vec<Value> = members
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let mut inst = json!({
                    "type": "Property",
                    "observedAt": format!("2020-01-01T{i:02}:00:00Z"),
                });
                for (k, v) in m.as_object().expect("instance members") {
                    inst[k] = v.clone();
                }
                inst
            })
            .collect();
        let times: Vec<String> = instances
            .iter()
            .map(|i| i["observedAt"].as_str().unwrap_or_default().to_owned())
            .collect();
        let mut attrs = std::collections::BTreeMap::new();
        attrs.insert("speed".to_owned(), instances);
        Windowed {
            attrs,
            max_per_attr: members.len(),
            ts_min: times.first().cloned(),
            ts_max: times.last().cloned(),
            truncated: false,
            cut_served: None,
            more: false,
            held: Default::default(),
        }
    }

    fn aggregate(w: &Windowed, methods: &[&str]) -> Result<Map<String, Value>, NgsiError> {
        let r = TRepr {
            aggregated: true,
            aggr_methods: methods.iter().map(|m| (*m).to_string()).collect(),
            ..Default::default()
        };
        render_aggregated(
            w,
            None,
            &r,
            &antares_jsonld::Context::default(),
            "observedAt",
        )
    }

    /// Table 4.5.19.1-2: on a DateTime and on a Date, `min` "calculates the
    /// minimum value inside the period" and `max` the maximum. Both
    /// datatypes reach the broker as a JSON-LD typed value — C.6's
    /// `{"@type": "DateTime", "@value": "2018-12-04T12:00:00Z"}` — so the
    /// aggregation reads the value through its type instead of treating the
    /// wrapper as an opaque object.
    #[test]
    fn a_date_time_and_a_date_have_a_minimum_and_a_maximum() {
        let dt = |v: &str| json!({"value": {"@type": "DateTime", "@value": v}});
        let out = aggregate(
            &windowed_props(&[
                dt("2020-03-01T00:00:00Z"),
                dt("2020-01-01T00:00:00Z"),
                dt("2020-02-01T00:00:00Z"),
            ]),
            &["min", "max"],
        )
        .expect("DateTime is eligible for min and max");
        assert_eq!(
            out["speed"]["min"][0][0],
            json!({"@type": "DateTime", "@value": "2020-01-01T00:00:00Z"})
        );
        assert_eq!(
            out["speed"]["max"][0][0],
            json!({"@type": "DateTime", "@value": "2020-03-01T00:00:00Z"})
        );

        let d = |v: &str| json!({"value": {"@type": "Date", "@value": v}});
        let out = aggregate(
            &windowed_props(&[d("2020-03-01"), d("2020-01-01")]),
            &["min", "max"],
        )
        .expect("Date is eligible for min and max");
        assert_eq!(
            out["speed"]["min"][0][0],
            json!({"@type": "Date", "@value": "2020-01-01"})
        );
        assert_eq!(
            out["speed"]["max"][0][0],
            json!({"@type": "Date", "@value": "2020-03-01"})
        );
    }

    /// C.6 gives a second representation of the same datatypes: the value
    /// stays a string and `valueType` (4.5.2.2) carries the type, coerced to
    /// its datatype URI on the way in. Table 4.5.19.1-2 applies to the
    /// datatype, not to the spelling, so this form aggregates identically.
    #[test]
    fn a_value_type_carries_the_datatype_as_far_as_the_typed_value_does() {
        let dt =
            |v: &str| json!({"value": v, "valueType": "https://uri.etsi.org/ngsi-ld/DateTime"});
        let out = aggregate(
            &windowed_props(&[dt("2020-03-01T00:00:00Z"), dt("2020-01-01T00:00:00Z")]),
            &["min", "max"],
        )
        .expect("a valueType-coerced DateTime is eligible for min and max");
        assert_eq!(out["speed"]["min"][0][0], json!("2020-01-01T00:00:00Z"));
        assert_eq!(out["speed"]["max"][0][0], json!("2020-03-01T00:00:00Z"));
    }

    /// Table 4.5.19.1-2, Time column: `avg` "calculates the average time
    /// inside the period", and min/max apply as well. 4.6.3 mandates
    /// `hh:mm:ssZ` for a Time, so the computed average is one — carrying its
    /// type, since a bare string would read back as a JSON String, whose own
    /// column in Table 4.5.19.1-1 has no average at all.
    #[test]
    fn a_time_has_an_average_a_minimum_and_a_maximum() {
        let t = |v: &str| json!({"value": {"@type": "Time", "@value": v}});
        let out = aggregate(
            &windowed_props(&[t("09:30:00Z"), t("08:30:00Z")]),
            &["avg", "min", "max"],
        )
        .expect("Time is eligible for avg, min and max");
        assert_eq!(
            out["speed"]["avg"][0][0],
            json!({"@type": "Time", "@value": "09:00:00Z"})
        );
        assert_eq!(
            out["speed"]["min"][0][0],
            json!({"@type": "Time", "@value": "08:30:00Z"})
        );
        assert_eq!(
            out["speed"]["max"][0][0],
            json!({"@type": "Time", "@value": "09:30:00Z"})
        );
    }

    /// The N/A cells of Table 4.5.19.1-2 are refused, not computed: 5.7.4.4
    /// p.211 raises InvalidRequest when an Attribute "is not eligible for at
    /// least one of the aggregation methods specified in the request".
    /// DateTime and Date have no avg, sum, stddev or sumsq; Time has no sum,
    /// stddev or sumsq.
    #[test]
    fn the_methods_a_temporal_datatype_does_not_support_are_refused() {
        let w = windowed_props(&[
            json!({"value": {"@type": "DateTime", "@value": "2020-01-01T00:00:00Z"}}),
        ]);
        for method in ["avg", "sum", "stddev", "sumsq"] {
            assert!(
                matches!(aggregate(&w, &[method]), Err(NgsiError::InvalidRequest(_))),
                "DateTime must not be eligible for {method}"
            );
        }
        let w = windowed_props(&[json!({"value": {"@type": "Time", "@value": "08:30:00Z"}})]);
        for method in ["sum", "stddev", "sumsq"] {
            assert!(
                matches!(aggregate(&w, &[method]), Err(NgsiError::InvalidRequest(_))),
                "Time must not be eligible for {method}"
            );
        }
    }

    /// Table 4.5.19.1-1, JSON String column: `avg` is N/A. A string is a
    /// JSON String whatever it spells, so a value that reads like a
    /// time-of-day is averaged only when its datatype says it is a Time.
    #[test]
    fn a_json_string_has_no_average_however_it_reads() {
        let w = windowed_props(&[json!({"value": "08:30:00Z"}), json!({"value": "09:30:00Z"})]);
        assert!(matches!(
            aggregate(&w, &["avg"]),
            Err(NgsiError::InvalidRequest(_))
        ));
        // its own row of the table is unchanged: lexicographic min and max
        let out = aggregate(&w, &["min", "max"]).expect("a string has min and max");
        assert_eq!(out["speed"]["min"][0][0], json!("08:30:00Z"));
        assert_eq!(out["speed"]["max"][0][0], json!("09:30:00Z"));
    }

    /// The counting methods have no N/A cell in any of the three tables:
    /// `totalCount` "the number of times the value has been updated" and
    /// `distinctCount` "the count of distinct values", for every datatype
    /// including the ones with no other method at all.
    #[test]
    fn every_datatype_is_counted() {
        for members in [
            json!({"value": {"@type": "DateTime", "@value": "2020-01-01T00:00:00Z"}}),
            json!({"value": {"@type": "Date", "@value": "2020-01-01"}}),
            json!({"value": {"@type": "Time", "@value": "08:30:00Z"}}),
            json!({"vocab": "urn:ngsi-ld:Colour:red"}),
            json!({"object": "urn:ngsi-ld:Car:1"}),
        ] {
            let w = windowed_props(&[members.clone(), members.clone()]);
            let out = aggregate(&w, &["totalCount", "distinctCount"])
                .unwrap_or_else(|e| panic!("{members} must be counted: {e:?}"));
            assert_eq!(out["speed"]["totalCount"][0][0], json!(2));
            assert_eq!(out["speed"]["distinctCount"][0][0], json!(1));
        }
    }
}

#[cfg(test)]
mod clause_4_21 {
    use super::*;
    use antares_jsonld::Loader;

    fn params(kv: &[(&str, &str)]) -> HashMap<String, String> {
        kv.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    /// 5.6.12.4: Append Attributes adds the fragment's new Entity Types
    /// once, in order (Coraine troe_timescale_multi_type 04, 05).
    #[test]
    fn temporal_append_adds_new_entity_types() {
        let mut doc = json!({"id": "urn:A1", "type": ["V", "C"]});
        add_temporal_instances(
            &mut doc,
            &json!({"type": ["T", "V"]}),
            "2026-01-01T00:00:00Z",
        );
        assert_eq!(doc["type"], json!(["V", "C", "T"]));
        let mut doc = json!({"id": "urn:A1", "type": "V"});
        add_temporal_instances(&mut doc, &json!({"p": []}), "2026-01-01T00:00:00Z");
        assert_eq!(doc["type"], json!("V"));
    }

    /// 5.7.3.4/5.7.4.4: a Linked Entity projection in attrs is
    /// BadRequestData like one in pick (Coraine
    /// troe_timescale_temporal_linked_projection 05).
    #[test]
    fn linked_entity_attrs_refused_on_temporal_reads() {
        let ctx = Loader::new().core();
        let linked = parse_trepr(&params(&[("attrs", "owner{object}")]), &ctx).expect("parses");
        assert!(matches!(
            reject_linked_projection(&linked, "5.7.3.4"),
            Err(NgsiError::BadRequestData(_))
        ));
        let plain = parse_trepr(&params(&[("attrs", "owner,speed")]), &ctx).expect("parses");
        assert!(reject_linked_projection(&plain, "5.7.3.4").is_ok());
    }

    /// 4.21 Projections: the pick/omit/attrs refusal (repr::check_projection_exclusive,
    /// ETSI TP 020_22) holds on the temporal representation exactly as it does
    /// on the current-state one — the temporal operations define no exception to it, so any pair
    /// is BadRequestData and each one alone parses.
    #[test]
    fn pick_omit_and_attrs_cannot_be_combined_on_a_temporal_query() {
        let ctx = Loader::new().core();
        for p in [
            params(&[("pick", "a"), ("omit", "b")]),
            params(&[("pick", "a"), ("attrs", "b")]),
            params(&[("omit", "a"), ("attrs", "b")]),
            params(&[("pick", "a"), ("omit", "b"), ("attrs", "c")]),
        ] {
            match parse_trepr(&p, &ctx) {
                Err(NgsiError::BadRequestData(_)) => {}
                other => panic!("must be BadRequestData, got {:?}", other.err()),
            }
        }
        for p in [
            params(&[("pick", "a")]),
            params(&[("omit", "a")]),
            params(&[("attrs", "a")]),
        ] {
            assert!(parse_trepr(&p, &ctx).is_ok());
        }
    }

    /// 4.21 on the core members of a temporal Entity: `pick` constrains them
    /// strictly (only what is named survives) and `omit` drops a named member
    /// only when the node carries no children — the same reading the
    /// current-state representation applies, so the two never disagree about
    /// whether `id` or `type` is in the answer.
    #[test]
    fn core_members_follow_the_same_projection_rule() {
        let pick = crate::repr::parse_projection("id", &Loader::new().core()).expect("pick");
        assert!(crate::repr::meta_projected(Some(&pick), None, "id"));
        assert!(!crate::repr::meta_projected(Some(&pick), None, "type"));
        let omit = crate::repr::parse_projection("type", &Loader::new().core()).expect("omit");
        assert!(!crate::repr::meta_projected(None, Some(&omit), "type"));
        assert!(crate::repr::meta_projected(None, Some(&omit), "id"));
        assert!(crate::repr::meta_projected(None, None, "type"));
    }
}

#[cfg(test)]
mod t3932_time_of_day {
    use super::seconds_of_day;

    /// T-3932: a Time read with checked byte positions; a short or
    /// multi-byte value is not a Time, never a panic.
    #[test]
    fn a_short_or_multibyte_time_is_none() {
        assert_eq!(seconds_of_day("12:30:00Z"), Some(45_000.0));
        assert_eq!(seconds_of_day("12:30:00.5"), Some(45_000.5));
        for bad in ["", "12:3", "12:30", "1é:00:00", "12-30-00", "aa:bb:cc"] {
            assert_eq!(seconds_of_day(bad), None, "{bad}");
        }
    }
}

#[cfg(test)]
mod t4794_held_values_scopes_and_versions {
    use super::{
        aggregate_bucket, scope_match_intervals, time_weighted_avg, with_versions, AggrClass,
        Windowed,
    };
    use serde_json::{json, Map, Value};

    fn at(s: &str) -> chrono::DateTime<chrono::FixedOffset> {
        chrono::DateTime::parse_from_rfc3339(s).expect("instant")
    }

    /// ADR-N-054 2.5: an onChange value weighs by the time it held, the
    /// level carried in holding from the bucket's start.
    #[test]
    fn an_on_change_mean_weighs_each_value_by_the_time_it_held() {
        let (start, end) = (at("2026-01-01T00:00:00Z"), at("2026-01-01T01:00:00Z"));
        let (ten, twenty, text) = (json!(10), json!(20), json!("x"));
        let rows = [(at("2026-01-01T00:15:00Z"), &twenty)];
        // 10 for 15 minutes, then 20 for 45: 17.5
        assert_eq!(
            time_weighted_avg(AggrClass::Number, &rows, Some(&ten), start, end),
            Some(json!(17.5))
        );
        // nothing carried in: the first row holds to the end
        assert_eq!(
            time_weighted_avg(AggrClass::Number, &rows, None, start, end),
            Some(json!(20.0))
        );
        // no number, or no moment held, is no mean
        let words = [(at("2026-01-01T00:15:00Z"), &text)];
        assert_eq!(
            time_weighted_avg(AggrClass::Number, &words, None, start, end),
            None
        );
        assert_eq!(
            time_weighted_avg(AggrClass::Number, &[], None, start, end),
            None
        );
        assert_eq!(
            time_weighted_avg(AggrClass::TimeOfDay, &rows, Some(&ten), start, end),
            None
        );
    }

    /// 4.18 over a Temporal Evolution: a Scope holds from the time it was
    /// set until the next one; a plain scope holds for all time.
    #[test]
    fn a_scope_matches_from_the_time_it_was_set_until_the_next() {
        let doc = json!({"scope": [
            {"type": "Property", "value": "/b", "observedAt": "2026-02-01T00:00:00Z"},
            {"type": "Property", "value": "/a", "modifiedAt": "2026-01-01T00:00:00Z"},
            {"type": "Property", "value": "urn:ngsi-ld:null", "createdAt": "2026-03-01T00:00:00Z"},
            {"type": "Property", "value": ["/a", "/c"], "observedAt": "2026-04-01T00:00:00Z"},
            {"type": "Property", "value": "/a"}
        ]});
        assert_eq!(
            scope_match_intervals(&doc, "/a"),
            vec![
                (
                    "2026-01-01T00:00:00Z".to_owned(),
                    Some("2026-02-01T00:00:00Z".to_owned())
                ),
                ("2026-04-01T00:00:00Z".to_owned(), None),
            ],
            "the deleted scope matches nothing, an unstamped instance is skipped"
        );
        assert!(scope_match_intervals(&doc, "/z").is_empty());
        assert_eq!(
            scope_match_intervals(&json!({"scope": "/a"}), "/a"),
            vec![(String::new(), None)]
        );
        assert!(scope_match_intervals(&json!({"scope": "/a"}), "/b").is_empty());
        assert!(scope_match_intervals(&json!({}), "/a").is_empty());
    }

    /// ADR-N-054 2.4: `versions=all` replaces an instance by every version
    /// of the same measurement; one with no version stays.
    #[test]
    fn versions_replace_the_same_measurement_only() {
        let speed = "https://x/speed";
        let mut w = Windowed {
            attrs: [
                (
                    speed.to_owned(),
                    vec![
                        json!({"value": 2, "observedAt": "2026-01-01T00:00:00Z"}),
                        json!({"value": 5, "observedAt": "2026-01-02T00:00:00Z"}),
                    ],
                ),
                ("https://x/other".to_owned(), vec![json!({"value": 1})]),
            ]
            .into_iter()
            .collect(),
            max_per_attr: 100,
            ts_min: None,
            ts_max: None,
            truncated: false,
            more: false,
            cut_served: None,
            held: Default::default(),
        };
        let mut chains = Map::new();
        chains.insert(
            speed.to_owned(),
            json!([
                {"value": 1, "observedAt": "2026-01-01T00:00:00.000Z"},
                {"value": 2, "observedAt": "2026-01-01T00:00:00Z"},
                {"value": 9, "observedAt": "2026-01-01T00:00:00Z", "datasetId": "urn:d"},
            ]),
        );
        with_versions(&mut w, &chains);
        let values: Vec<&Value> = w.attrs[speed].iter().map(|i| &i["value"]).collect();
        assert_eq!(values, [&json!(1), &json!(2), &json!(5)]);
        assert_eq!(w.attrs["https://x/other"], [json!({"value": 1})]);
    }

    /// Tables 4.5.19.1-1/2/3: each method over numbers, a List counted by
    /// its length, text ordered as written; an overflow is null, never an
    /// out-of-range float.
    #[test]
    fn each_method_folds_a_bucket_by_its_class() {
        let (two, four, six) = (json!(2), json!(4), json!(6));
        let nums = [&two, &four, &six];
        let agg = |m: &str| aggregate_bucket(m, AggrClass::Number, &nums);
        assert_eq!(agg("min"), json!(2.0));
        assert_eq!(agg("max"), json!(6.0));
        assert_eq!(agg("sum"), json!(12.0));
        assert_eq!(agg("avg"), json!(4.0));
        assert_eq!(agg("sumsq"), json!(56.0));
        let sd = agg("stddev").as_f64().expect("stddev");
        assert!((sd - (8.0f64 / 3.0).sqrt()).abs() < 1e-12, "{sd}");
        assert_eq!(agg("totalCount"), json!(3));
        assert_eq!(agg("unknown"), Value::Null);
        for m in ["min", "max", "avg", "stddev"] {
            assert_eq!(
                aggregate_bucket(m, AggrClass::Number, &[]),
                Value::Null,
                "{m}"
            );
        }
        let huge = json!(f64::MAX);
        assert_eq!(
            aggregate_bucket("sum", AggrClass::Number, &[&huge, &huge]),
            Value::Null
        );

        let (short, long) = (json!(["a"]), json!(["a", "b", "c"]));
        assert_eq!(
            aggregate_bucket("max", AggrClass::List, &[&short, &long]),
            json!(3.0)
        );
        let (b, a) = (json!("b"), json!("a"));
        assert_eq!(
            aggregate_bucket("min", AggrClass::Text, &[&b, &a]),
            json!("a")
        );
        assert_eq!(
            aggregate_bucket("max", AggrClass::Text, &[&b, &a]),
            json!("b")
        );
        let (r1, r2) = (json!(["urn:a", "urn:b"]), json!("urn:a"));
        assert_eq!(
            aggregate_bucket("distinctCount", AggrClass::Relationship, &[&r1, &r2]),
            json!(2),
            "distinct targets, an array flattened"
        );
    }
}

#[cfg(test)]
mod f_ant2_014_scope_window {
    use super::*;
    use serde_json::json;

    /// An instance observed half a second after its scope was set lies
    /// inside the scope's validity: the comparison is temporal, not byte
    /// order (`…00.500Z` sorts below `…00Z` as a raw string).
    #[test]
    fn a_fractional_instance_after_the_scope_is_kept() {
        let mut doc = json!({"id": "urn:e", "type": "T",
            "scope": [{"type": "Property", "value": "/a", "observedAt": "2026-01-01T00:00:00Z"}],
            "speed": [{"type": "Property", "value": 1, "observedAt": "2026-01-01T00:00:00.500Z"}]});
        assert!(scope_window_retain(&mut doc, "/a", None, "observedAt"));
        assert_eq!(doc["speed"].as_array().map(Vec::len), Some(1));
    }
}
