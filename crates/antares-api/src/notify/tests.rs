// SPDX-License-Identifier: EUPL-1.2
//! Unit tests of the notification pipeline, one module per clause or seam.

use super::*;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod deleted_subscription_delivery {
    use super::*;

    /// 5.8.6: notifications are sent for the subscriptions a Context Broker
    /// holds — a subscription deleted between matching and delivery no
    /// longer exists, so its endpoint must receive nothing.
    #[tokio::test(flavor = "multi_thread")]
    async fn deleted_subscription_receives_no_notification() {
        crate::allow_private();
        let st = AppState::new("antares-deleted-sub-test".into());
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = count.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let app = axum::Router::new().route(
            "/notify",
            axum::routing::post(move || {
                let c = c.clone();
                async move {
                    c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    axum::http::StatusCode::OK
                }
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let tenant = TenantId::new("default").expect("tenant");
        let ctx = antares_jsonld::Loader::new().core();
        // the sub doc is a snapshot whose row is NOT in the store — the
        // deleted-concurrently case
        let sub = json!({
            "id": "urn:ngsi-ld:Subscription:ghost",
            "type": "Subscription",
            "entities": [{"type": "Vehicle"}],
            "notification": {"endpoint": {"uri": format!("http://{addr}/notify")}},
        });
        let data = vec![json!({"id": "urn:ngsi-ld:Vehicle:1", "type": "Vehicle"})];
        deliver_as(
            &st,
            &tenant,
            Kind::Subscription,
            &sub,
            "Notification",
            data,
            &ctx,
            None,
        )
        .await;
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a deleted subscription's endpoint must receive NO notification"
        );
    }
}

#[cfg(test)]
mod interval_tests {
    use super::*;
    use serde_json::json;

    /// 5.2.12 Table 5.2.12-1 bounds `Subscription.timeInterval` only as
    /// "greater than 0", so a client may name an interval whose milliseconds
    /// do not fit in the epoch arithmetic. A firing instant the broker cannot
    /// represent must read as far in the future, never as far in the past: a
    /// wrapped anchor is a NEGATIVE instant, which reports the subscription
    /// as permanently due and fires it on every tick, each firing running the
    /// subscription's whole query.
    #[test]
    fn an_interval_too_large_to_schedule_is_never_due_rather_than_always() {
        let sub = json!({
            "id": "urn:ngsi-ld:Subscription:huge",
            "type": "Subscription",
            "createdAt": "2026-01-01T00:00:00Z",
        });
        let now = chrono::Utc::now().timestamp_millis();
        for interval in [1e18, 1e30, f64::MAX] {
            let due = due_at_ms(&sub, interval);
            assert!(
                due > now,
                "timeInterval {interval} is due at {due}, which is not after {now}"
            );
        }
        // and an interval that DOES fit still schedules exactly one interval
        // past the anchor, so the guard costs the ordinary case nothing
        let anchor = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .expect("anchor")
            .timestamp_millis();
        assert_eq!(due_at_ms(&sub, 30.0), anchor + 30_000);
    }

    /// A subscriber cannot put its own value of a marker the broker sets on
    /// the wire: the pair is dropped before the broker appends its own, and
    /// the drop is by ASCII case-insensitive name, since a header name is
    /// case-insensitive (IETF RFC 9110 clause 5.1) and the pair would
    /// otherwise slip past under a different spelling.
    #[test]
    fn a_subscriber_cannot_add_its_own_notification_markers() {
        let mut info = vec![
            ("Authorization".to_owned(), "Bearer t".to_owned()),
            ("ngsild-tenant".to_owned(), "victim".to_owned()),
            ("NGSILD-Tenant".to_owned(), "victim".to_owned()),
            (
                "NGSILD-SNAPSHOT".to_owned(),
                "urn:ngsi-ld:Snapshot:x".to_owned(),
            ),
            ("X-NGSILD-Tenant".to_owned(), "kept".to_owned()),
        ];
        strip_reserved_markers(&mut info);
        assert_eq!(
            info,
            [
                ("Authorization".to_owned(), "Bearer t".to_owned()),
                ("X-NGSILD-Tenant".to_owned(), "kept".to_owned()),
            ]
        );
    }

    /// The sweep's own clock takes the same offset, so the same overflow
    /// would park (or un-park) every tenant's sweep, not just this
    /// subscription: `next_sub`/`next_csub` are process-wide minima.
    #[test]
    fn the_sweep_clock_offset_survives_an_unschedulable_interval() {
        let now_ms = chrono::Utc::now().timestamp_millis();
        for interval in [1e18, f64::MAX] {
            let next = now_ms.saturating_add(interval_offset_ms(interval));
            assert!(next > now_ms, "sweep clock went backwards for {interval}");
        }
        assert_eq!(interval_offset_ms(1.5), 1500);
    }
}

#[cfg(test)]
mod endpoint_tests {
    use super::*;
    use serde_json::json;

    fn ep(v: Value) -> serde_json::Map<String, Value> {
        v.as_object().expect("map").clone()
    }

    /// Clause 4.21 + Table 5.2.14.1-1: notification `pick`/`omit` are
    /// "a valid attribute projection language string as per clause 4.21", so a
    /// LinkedEntityTerm (`ProjectionTerm = AttrName *1(LinkedEntityTerm)`) must
    /// constrain the Linked Entity, exactly as it does on the query path.
    #[test]
    fn notification_projection_parses_linked_entity_terms() {
        let ctx = antares_jsonld::Loader::new().core();
        let sub = json!({
            "notification": { "pick": ["id", "type", "refDevice{type}"] }
        });
        let shape = notif_shape(&sub, &ctx);
        let pick = shape.repr.pick.expect("pick parsed");

        let linked = pick.iter().find(|n| n.raw == "refDevice").expect(
            "refDevice must survive as its own term, not as the literal \"refDevice{type}\"",
        );
        let children = linked
            .children
            .as_ref()
            .expect("the {…} term must become children so the Linked Entity is constrained");
        assert!(
            children.iter().any(|c| c.raw == "type"),
            "refDevice{{type}} must select `type` inside the Linked Entity"
        );
    }

    /// Table 5.2.12-1 `lang`: "Language filter to be applied to the query
    /// (clause 4.15)", the query being what a Notification carries; a
    /// LanguageProperty arrives as a Property in the chosen language
    /// (Coraine langprop_get_none, subscription_notify_langprop_array).
    #[test]
    fn the_subscription_lang_filters_the_notified_entity() {
        let ctx = antares_jsonld::Loader::new().core();
        let shape = notif_shape(&json!({"lang": "es", "notification": {}}), &ctx);
        let doc = json!({"id": "urn:E1", "type": "T",
            "https://example.org/L": [{"type": "LanguageProperty",
                "languageMap": {"en": "one", "es": "uno"}}]});
        let out = crate::repr::apply(&doc, &shape.repr);
        let l = &out["https://example.org/L"][0];
        assert_eq!(l["type"], "Property", "{out}");
        assert_eq!(l["value"], "uno");
        assert_eq!(l["lang"], "es");
    }

    /// Table 5.2.15-1 `timeout`: honored, clamped, defaulted.
    #[test]
    fn endpoint_timeout_is_honored_clamped_and_defaulted() {
        assert_eq!(endpoint_timeout_ms(&ep(json!({"timeout": 1500}))), 1500);
        assert_eq!(endpoint_timeout_ms(&ep(json!({}))), 5_000, "default");
        // "The NGSI-LD system can override this value" — the clamp is that
        // override, keeping delivery tasks bounded
        assert_eq!(endpoint_timeout_ms(&ep(json!({"timeout": 600000}))), 30_000);
        assert_eq!(endpoint_timeout_ms(&ep(json!({"timeout": 1}))), 100);
        // creation rejects <=0, but a hand-edited row must not panic
        assert_eq!(endpoint_timeout_ms(&ep(json!({"timeout": -5}))), 5_000);
    }

    /// Table 5.2.15-1 `cooldown`: gate opens only after a
    /// failure and closes once the window elapses or a success lands.
    #[test]
    fn cooldown_gates_only_failed_subscriptions_within_the_window() {
        let now = chrono::Utc::now();
        let recent = (now - chrono::Duration::milliseconds(500)).to_rfc3339();
        let old = (now - chrono::Duration::milliseconds(5_000)).to_rfc3339();
        let sub = |status: &str, last_failure: &str| {
            json!({
                "notification": {
                    "status": status,
                    "lastFailure": last_failure,
                    "endpoint": {"uri": "http://x/n", "cooldown": 2000}
                }
            })
        };
        assert!(
            in_cooldown(&sub("failed", &recent), now),
            "failed 0.5s ago, 2s cooldown ⇒ suppressed"
        );
        assert!(
            !in_cooldown(&sub("failed", &old), now),
            "failure outside the window ⇒ delivered"
        );
        assert!(
            !in_cooldown(&sub("ok", &recent), now),
            "a success clears the gate — status is not \"failed\""
        );
        let no_cooldown = json!({
            "notification": {
                "status": "failed", "lastFailure": recent,
                "endpoint": {"uri": "http://x/n"}
            }
        });
        assert!(
            !in_cooldown(&no_cooldown, now),
            "no cooldown member ⇒ no gate"
        );
    }
}

#[cfg(test)]
mod clause_5_8_1 {
    use super::*;

    /// 5.8.1.4: "the status of the Subscription changes automatically to
    /// \"expired\", so that notifications will no longer be sent" — an
    /// expiresAt spelled without a seconds fraction must count as expired
    /// the moment now (spelled with milliseconds) passes it. A raw
    /// lexicographic compare ranks 'Z' above '.' and keeps the
    /// subscription alive for the whole boundary second.
    #[test]
    fn expiry_compare_survives_fraction_spellings() {
        let now = crate::state::now_iso();
        let secs = &now[..19];
        std::thread::sleep(std::time::Duration::from_millis(5));
        let sub = serde_json::json!({ "expiresAt": format!("{secs}Z") });
        assert!(
            !is_active(&sub),
            "expiresAt {secs}Z lies in the past and must expire the subscription"
        );
        // a genuinely future expiry stays active
        let sub = serde_json::json!({ "expiresAt": "2999-01-01T00:00:00Z" });
        assert!(is_active(&sub));
    }
}

#[cfg(test)]
mod clause_5_3_3 {
    use super::*;
    use serde_json::json;

    /// 5.3.2 triggerReason + 5.3.3 TriggerReasonEnumeration: newlyMatching
    /// (did not match -> matches), updated (matched -> still matches),
    /// noLongerMatching (matched -> no longer / deleted); no notification
    /// when neither side matches.
    #[test]
    fn trigger_reason_enumeration() {
        let ctx = antares_jsonld::Loader::new().core();
        let sub = json!({"entities": [
            {"type": "https://uri.etsi.org/ngsi-ld/default-context/Building"}]});
        let reg = |t: &str| {
            json!({"id": "urn:csr:1", "type": "ContextSourceRegistration",
                "endpoint": "http://peer:9090",
                "information": [{"entities": [
                    {"type": format!("https://uri.etsi.org/ngsi-ld/default-context/{t}")}]}]})
        };
        let hit = reg("Building");
        let miss = reg("Vehicle");
        assert_eq!(
            csource_trigger(&sub, None, Some(&hit), &ctx),
            Some("newlyMatching")
        );
        assert_eq!(
            csource_trigger(&sub, Some(&miss), Some(&hit), &ctx),
            Some("newlyMatching"),
            "an update that STARTS matching is newlyMatching"
        );
        assert_eq!(
            csource_trigger(&sub, Some(&hit), Some(&hit), &ctx),
            Some("updated")
        );
        assert_eq!(
            csource_trigger(&sub, Some(&hit), None, &ctx),
            Some("noLongerMatching"),
            "deletion of a matching registration"
        );
        assert_eq!(
            csource_trigger(&sub, Some(&hit), Some(&miss), &ctx),
            Some("noLongerMatching"),
            "an update that STOPS matching"
        );
        assert_eq!(
            csource_trigger(&sub, Some(&miss), Some(&miss), &ctx),
            None,
            "never-matching changes produce no notification"
        );
    }
}

#[cfg(test)]
mod clause_5_2_33 {
    use super::*;
    use serde_json::json;

    /// Table 5.2.33-1: id is String or String[]; "id takes precedence over
    /// idPattern" when a selector carries both.
    #[test]
    fn selector_id_array_and_precedence() {
        let ctx = antares_jsonld::Loader::new().core();
        let doc = json!({"id": "urn:x:A", "type": ["T"]});
        let sub = |e: serde_json::Value| json!({"entities": [e]});
        assert!(!selector_match(
            &sub(json!({"type": "T", "idPattern": "^urn:x:B"})),
            &doc,
            &ctx
        ));
        assert!(
            selector_match(
                &sub(json!({"type": "T", "id": "urn:x:A", "idPattern": "^urn:x:B"})),
                &doc,
                &ctx
            ),
            "id takes precedence over idPattern"
        );
        assert!(selector_match(
            &sub(json!({"type": "T", "id": ["urn:x:A", "urn:x:C"]})),
            &doc,
            &ctx
        ));
        assert!(
            !selector_match(&sub(json!({"type": "T", "id": ["urn:x:B"]})), &doc, &ctx),
            "an id array not containing the entity id must not match"
        );
    }
}

/// Availability of the change→notification pipeline itself: the consumer
/// task must survive a panic, and its queue must stay bounded.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod change_pipeline {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tower::ServiceExt;

    async fn post(st: &AppState, uri: &str, body: Value) -> u16 {
        let body = body.to_string();
        crate::router(st.clone())
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("Content-Type", "application/json")
                    .header("Content-Length", body.len())
                    .body(axum::body::Body::from(body))
                    .expect("request"),
            )
            .await
            .expect("response")
            .status()
            .as_u16()
    }

    /// An endpoint that answers 200 and counts the notifications it got.
    async fn counting_endpoint() -> (String, Arc<AtomicUsize>) {
        let hits: Arc<AtomicUsize> = Arc::default();
        let seen = hits.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let app = axum::Router::new().route(
            "/notify",
            axum::routing::post(move || {
                let seen = seen.clone();
                async move {
                    seen.fetch_add(1, Ordering::SeqCst);
                    axum::http::StatusCode::OK
                }
            }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        (format!("http://{addr}/notify"), hits)
    }

    async fn subscribe(st: &AppState, id: &str, uri: &str, timeout_ms: u32) {
        let status = post(
            st,
            "/ngsi-ld/v1/subscriptions",
            json!({
                "id": format!("urn:ngsi-ld:Subscription:{id}"),
                "type": "Subscription",
                "entities": [{"type": "Vehicle"}],
                "notification": {"endpoint": {"uri": uri, "timeout": timeout_ms}},
            }),
        )
        .await;
        assert_eq!(status, 201, "subscription created");
    }

    async fn create_vehicle(st: &AppState, n: usize) -> u16 {
        post(
            st,
            "/ngsi-ld/v1/entities",
            json!({
                "id": format!("urn:ngsi-ld:Vehicle:pipe{n}"),
                "type": "Vehicle",
                "speed": {"type": "Property", "value": n},
            }),
        )
        .await
    }

    /// Table 5.2.14.2-1 timesSent / lastNotification: the mirror the matcher
    /// reads carries the counters the delivery writeback committed, and the
    /// store row agrees — one writeback, no second read.
    #[tokio::test(flavor = "multi_thread")]
    async fn mirror_carries_the_booked_counters() {
        crate::allow_private();
        let (uri, hits) = counting_endpoint().await;
        let mut st = AppState::new("antares-mirror-booked".into());
        crate::wire(&mut st).await;
        subscribe(&st, "booked", &uri, 30_000).await;
        assert_eq!(create_vehicle(&st, 7).await, 201);
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_secs(10 * crate::state::slow_factor());
        while hits.load(Ordering::SeqCst) < 1 && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1, "one notification delivered");
        let id = "urn:ngsi-ld:Subscription:booked";
        let mirrored = st
            .sub_mirror
            .as_ref()
            .expect("mirror wired")
            .docs("default")
            .into_iter()
            .find(|d| d["id"] == id)
            .expect("subscription mirrored");
        assert_eq!(mirrored["notification"]["timesSent"], json!(1));
        assert!(mirrored["notification"]["lastNotification"].is_string());
        let tenant = TenantId::new("default").expect("tenant");
        let stored = st
            .store
            .get(&tenant, Kind::Subscription, id)
            .await
            .expect("store")
            .expect("row");
        assert_eq!(stored["notification"]["timesSent"], json!(1));
        assert_eq!(
            stored["notification"]["lastNotification"],
            mirrored["notification"]["lastNotification"]
        );
    }

    /// T-3531: a stored deadline past u32 caps instead of wrapping.
    #[test]
    fn a_dead_letter_deadline_caps_instead_of_wrapping() {
        use serde_json::json;
        assert_eq!(letter_timeout_ms(&json!({})), 5_000);
        assert_eq!(letter_timeout_ms(&json!({"timeoutMs": 2_000})), 2_000);
        assert_eq!(
            letter_timeout_ms(&json!({"timeoutMs": 4_294_967_396u64})),
            30_000
        );
    }

    /// 5.8.6: a matching change notifies, and a panic around ONE change must
    /// not end notification delivery for the process — the NEXT change still
    /// notifies. The lock poisoning here used to make the matcher itself
    /// panic; since the mirrors recover from poisoning, the first change may
    /// legitimately deliver too. The contract under test is therefore that
    /// the SECOND change's notification arrives — not a count, which only
    /// measured the race between the two deliveries and failed either way on
    /// slow machines.
    #[tokio::test(flavor = "multi_thread")]
    async fn panicking_change_does_not_stop_the_next_notification() {
        crate::allow_private();
        let (uri, hits) = counting_endpoint().await;
        let mut st = AppState::new("antares-panic-guard".into());
        crate::wire(&mut st).await;
        subscribe(&st, "guard", &uri, 2_000).await;
        // Poison the mirror lock while a change is in flight: whatever the
        // matcher does with that (recover, or panic into the supervision
        // boundary), the pipeline must keep delivering afterwards.
        let mirror = st.sub_mirror.clone().expect("mirror");
        mirror.poison();
        assert_eq!(create_vehicle(&st, 1).await, 201);
        assert_eq!(create_vehicle(&st, 2).await, 201);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while hits.load(Ordering::SeqCst) < 1 && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let got = hits.load(Ordering::SeqCst);
        assert!(
            (1..=2).contains(&got),
            "delivery must survive the poisoned change: expected 1 or 2 notifications, got {got}"
        );
        // and the pipeline is still alive for a THIRD change after the dust
        // settles — the actual supervision contract
        let before = hits.load(Ordering::SeqCst);
        assert_eq!(create_vehicle(&st, 3).await, 201);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while hits.load(Ordering::SeqCst) <= before && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            hits.load(Ordering::SeqCst) > before,
            "a change created after the poisoned one must still notify"
        );
    }

    /// A binding that records the losses it is told of (T-4921).
    struct Losses(Arc<std::sync::Mutex<Vec<String>>>);

    impl antares_notifier::NotificationSink for Losses {
        fn schemes(&self) -> &'static [&'static str] {
            &["x-loss"]
        }
        fn parse_endpoint(
            &self,
            _uri: &str,
            _notifier_info: &[(&str, &str)],
        ) -> Result<(), antares_model::NgsiError> {
            Ok(())
        }
        fn deliver<'a>(
            &'a self,
            _uri: &'a str,
            _out: &'a antares_notifier::Outbound,
            _timeout: std::time::Duration,
        ) -> antares_notifier::DeliveryFuture<'a> {
            Box::pin(async { Ok(()) })
        }
        fn lost(&self, uri: &str, tenant: &str, subscription: &str) {
            if let Ok(mut l) = self.0.lock() {
                l.push(format!("{uri} {tenant} {subscription}"));
            }
        }
    }

    /// T-4921: a group a full lane drops is told to the subscription's
    /// binding, so a WebSocket consumer gets an `x-gap` instead of silence.
    /// The lane task does not run before this test yields, so the lane
    /// takes DELIVERY_LANE groups and drops the next two.
    #[tokio::test]
    async fn a_full_lane_tells_the_binding_what_it_lost() {
        let seen: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let st =
            AppState::new("antares-lane-loss".into()).with_sink(Box::new(Losses(seen.clone())));
        let pending: Arc<AtomicUsize> = Arc::default();
        let done = Arc::new(BatchDone {
            pending: pending.clone(),
            taken: 1,
        });
        let sub = Arc::new(json!({
            "id": "urn:ngsi-ld:Subscription:loss",
            "isActive": false,
            "notification": {"endpoint": {"uri": "x-loss:a"}}
        }));
        for _ in 0..DELIVERY_LANE + 2 {
            let group = Matched {
                tenant: TenantId::new("default").expect("tenant"),
                sub: sub.clone(),
                ctx: antares_jsonld::Loader::new().core(),
                data: vec![json!({})],
            };
            to_lane(&st, group, &done);
        }
        let seen = seen.lock().map(|l| l.clone()).unwrap_or_default();
        assert_eq!(
            seen,
            vec!["x-loss:a default urn:ngsi-ld:Subscription:loss".to_owned(); 2]
        );
    }

    /// T-4086: a backed-up lane folds into notifications of at most
    /// CHANGE_BATCH entities; a group that does not fit leads the next one,
    /// and a single group larger than that still leaves whole.
    #[tokio::test]
    async fn lane_fold_stops_at_change_batch() {
        let st = AppState::new("antares-lane-fold".into());
        let pending: Arc<AtomicUsize> = Arc::default();
        let job = |n: usize| {
            pending.fetch_add(1, Ordering::SeqCst);
            LaneJob {
                st: st.clone(),
                group: Matched {
                    tenant: TenantId::new("default").expect("tenant"),
                    sub: Arc::new(json!({"id": "urn:ngsi-ld:Subscription:fold"})),
                    ctx: antares_jsonld::Loader::new().core(),
                    data: vec![json!({}); n],
                },
                _batch: Arc::new(BatchDone {
                    pending: pending.clone(),
                    taken: 1,
                }),
                _held: LANE_LEDGER.take(0, usize::MAX).expect("no budget"),
            }
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel(DELIVERY_LANE);
        for n in [100, 100, 100, CHANGE_BATCH + 1, 1] {
            assert!(tx.try_send(job(n)).is_ok(), "lane has room");
        }
        let mut sizes = Vec::new();
        let mut next = rx.try_recv().ok();
        while let Some(first) = next {
            let (_, group, batches, rest) = fold(first, &mut rx);
            sizes.push((group.data.len(), batches.len()));
            next = rest.or_else(|| rx.try_recv().ok());
        }
        assert_eq!(
            sizes,
            [(200, 2), (100, 1), (CHANGE_BATCH + 1, 1), (1, 1)],
            "folds stop before CHANGE_BATCH; an oversized group leaves alone"
        );
        assert_eq!(pending.load(Ordering::SeqCst), 0, "every batch released");
    }

    /// T-5245: one process may serve several states (tests run a broker
    /// each, with the same tenants and subscription ids): a fold keeps to
    /// the state of its first group, so no state's change is delivered
    /// through another's sinks, and the lanes are keyed by broker.
    #[tokio::test]
    async fn lane_fold_keeps_to_one_state() {
        let (a, b) = (
            AppState::new("antares-lane-a".into()),
            AppState::new("antares-lane-b".into()),
        );
        let pending: Arc<AtomicUsize> = Arc::default();
        let job = |st: &AppState| {
            pending.fetch_add(1, Ordering::SeqCst);
            LaneJob {
                st: st.clone(),
                group: Matched {
                    tenant: TenantId::new("a").expect("tenant"),
                    sub: Arc::new(json!({"id": "urn:ngsi-ld:Subscription:s1"})),
                    ctx: antares_jsonld::Loader::new().core(),
                    data: vec![json!({})],
                },
                _batch: Arc::new(BatchDone {
                    pending: pending.clone(),
                    taken: 1,
                }),
                _held: LANE_LEDGER.take(0, usize::MAX).expect("no budget"),
            }
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel(DELIVERY_LANE);
        for st in [&a, &a, &b, &b, &a] {
            assert!(tx.try_send(job(st)).is_ok(), "lane has room");
        }
        let mut folds = Vec::new();
        let mut next = rx.try_recv().ok();
        while let Some(first) = next {
            let (st, group, batches, rest) = fold(first, &mut rx);
            folds.push((st.same(&a), group.data.len(), batches.len()));
            next = rest.or_else(|| rx.try_recv().ok());
        }
        assert_eq!(folds, [(true, 2, 2), (false, 2, 2), (true, 1, 1)]);
        assert!(!a.same(&b) && a.same(&a.clone()));
        // and each broker's groups wait on lanes of their own
        assert_ne!(a.broker(), b.broker());
        assert_eq!(a.broker(), a.clone().broker());
        assert_eq!(pending.load(Ordering::SeqCst), 0, "every batch released");
    }

    /// T-4549: the lanes together hold at most their byte budget; a group
    /// past it is refused, a group larger than the budget still leaves when
    /// nothing else waits, and what a group held is given back when it goes.
    #[test]
    fn the_lanes_hold_at_most_their_byte_budget() {
        let ledger: &'static Ledger = Box::leak(Box::new(Ledger(Default::default())));
        let first = ledger.take(600, 1000).expect("room");
        assert!(ledger.take(500, 1000).is_none(), "past the budget");
        let second = ledger.take(400, 1000).expect("exactly the budget");
        assert_eq!(ledger.used(), 1000);
        drop(first);
        assert_eq!(ledger.used(), 400, "a delivered group gives its bytes back");
        drop(second);
        let alone = ledger
            .take(5000, 1000)
            .expect("one oversized group on an empty ledger");
        assert!(ledger.take(1, 1000).is_none(), "nothing beside it");
        drop(alone);
        assert_eq!(ledger.used(), 0);
        let entity = json!({"id": "urn:ngsi-ld:CityWork:1", "status": {"type": "Property", "value": "ongoing"}});
        let estimate = heap_estimate(&entity);
        let text = serde_json::to_vec(&entity).map(|b| b.len()).unwrap_or(0);
        assert!(
            estimate > text,
            "the estimate counts the nodes, not only the text: {estimate} <= {text}"
        );
        // T-4562: two objects are two map nodes of about 640 bytes each, however
        // few members they hold
        assert!(estimate >= 2 * 640, "{estimate}");
        let wide: serde_json::Map<String, Value> =
            (0..24).map(|k| (format!("a{k}"), Value::Null)).collect();
        assert!(
            heap_estimate(&Value::Object(wide)) >= 3 * 640,
            "24 members need three nodes"
        );
    }

    /// T-4086: behind a subscriber that never answers, a burst of a whole
    /// matcher queue reaches a healthy subscriber in full. The stalled one
    /// backs up its own lane, which drops and counts; the matcher queue,
    /// shared by every subscription, drops nothing.
    #[tokio::test(flavor = "multi_thread")]
    async fn stalled_subscriber_costs_only_its_own_lane() {
        // the file store's fsync per create is slower than a 5 s stall frees
        // lane room, so its lane never fills; the healthy half still runs
        let file_store = std::env::var("ANTARES_TEST_STORE").is_ok_and(|s| s == "file");
        crate::allow_private();
        // accepts, reads nothing, never answers: every delivery waits out the
        // outbound client's timeout
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for s in listener.incoming().flatten() {
                held.push(s);
            }
        });
        // an endpoint that answers at once and keeps every entity id it got
        let seen: Arc<std::sync::Mutex<std::collections::HashSet<String>>> = Arc::default();
        let got = seen.clone();
        let healthy = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let healthy_addr = healthy.local_addr().expect("addr");
        let app = axum::Router::new().route(
            "/notify",
            axum::routing::post(move |body: axum::Json<Value>| {
                let got = got.clone();
                async move {
                    let mut got = got.lock().expect("ids");
                    for e in body.0["data"].as_array().into_iter().flatten() {
                        got.insert(e["id"].as_str().unwrap_or_default().to_owned());
                    }
                    axum::http::StatusCode::OK
                }
            }),
        );
        tokio::spawn(async move { axum::serve(healthy, app).await.expect("serve") });
        let mut st = AppState::new("antares-stalled-lane".into());
        crate::wire(&mut st).await;
        subscribe(&st, "staller", &format!("http://{addr}/notify"), 30_000).await;
        subscribe(
            &st,
            "healthy",
            &format!("http://{healthy_addr}/notify"),
            30_000,
        )
        .await;
        let (queue_before, lane_before) = (changes_dropped(), lane_drops());
        for n in 0..CHANGE_QUEUE {
            assert_eq!(create_vehicle(&st, 1_000 + n).await, 201);
        }
        let received = || seen.lock().expect("ids").len();
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_secs(30 * crate::state::slow_factor());
        while received() < CHANGE_QUEUE && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(
            received(),
            CHANGE_QUEUE,
            "the healthy subscriber must get every change of the burst (matcher queue dropped {})",
            changes_dropped() - queue_before
        );
        assert_eq!(changes_dropped(), queue_before, "the matcher queue dropped");
        if !file_store {
            assert!(
                lane_drops() > lane_before,
                "the stalled subscriber's lane must drop and count, not grow"
            );
        }
    }

    /// Every accepted change is counted until its pass has run, so a drain
    /// that waits for zero closes the pool only after the last delivery.
    #[tokio::test(flavor = "multi_thread")]
    async fn pending_changes_return_to_zero_once_delivered() {
        crate::allow_private();
        let (uri, hits) = counting_endpoint().await;
        let mut st = AppState::new("antares-pending-changes".into());
        crate::wire(&mut st).await;
        subscribe(&st, "counter", &uri, 30_000).await;
        for n in 0..20 {
            assert_eq!(create_vehicle(&st, 5_000 + n).await, 201);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        // one pass folds queued changes into one notification, so the hit
        // count is at least one, not twenty
        let pending = || st.pending_changes.load(Ordering::SeqCst);
        while (hits.load(Ordering::SeqCst) == 0 || pending() > 0)
            && std::time::Instant::now() < deadline
        {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(hits.load(Ordering::SeqCst) > 0, "nothing delivered");
        assert_eq!(
            pending(),
            0,
            "pending must fall back to zero after delivery"
        );
    }

    /// Table 5.2.12-1: "\"entityUpdated\" is equivalent to the combination
    /// \"attributeCreated\", \"attributeUpdated\" and \"attributeDeleted\"",
    /// so such a subscription notifies on a creation exactly like the
    /// spelled-out list does — while "entityDeleted" alone does not.
    #[tokio::test(flavor = "multi_thread")]
    async fn entity_updated_trigger_notifies_on_entity_creation() {
        // under a sanitizer 370 concurrent tests starve the endpoint past the
        // outbound client's 5 s cap (seen twice in strict); triggers, not latency
        if std::env::var_os("ANTARES_TEST_SANITIZER").is_some() {
            return;
        }
        crate::allow_private();
        let (uri, hits) = counting_endpoint().await;
        let (quiet_uri, quiet_hits) = counting_endpoint().await;
        let mut st = AppState::new("antares-trigger-equivalence".into());
        crate::wire(&mut st).await;
        let sub = |id: &str, trigger: &str, uri: &str| {
            json!({
                "id": format!("urn:ngsi-ld:Subscription:{id}"),
                "type": "Subscription",
                "entities": [{"type": "Vehicle"}],
                "notificationTrigger": [trigger],
                // 30 s: a sanitizer runner with 370 concurrent tests took
                // 8 s to deliver once; this test is about triggers, not latency
                "notification": {"endpoint": {"uri": uri, "timeout": 30_000}},
            })
        };
        for body in [
            sub("eu", "entityUpdated", &uri),
            sub("ed", "entityDeleted", &quiet_uri),
        ] {
            assert_eq!(post(&st, "/ngsi-ld/v1/subscriptions", body).await, 201);
        }
        assert_eq!(create_vehicle(&st, 7).await, 201);
        // a wall-clock bound, not an iteration count: a sanitizer build
        // delivers the same notification an order of magnitude slower
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while hits.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        // the receiver's count alone cannot say WHY nothing arrived: the
        // subscription's own bookkeeping and the pipeline counters can
        let sub = st
            .store
            .get(
                &TenantId::default(),
                Kind::Subscription,
                "urn:ngsi-ld:Subscription:eu",
            )
            .await
            .ok()
            .flatten()
            .map(|s| s["notification"].to_string())
            .unwrap_or_else(|| "subscription missing".into());
        let diagnosis = || {
            format!(
                "notification={sub} changes_dropped={} task_panics={} dead_letters={}",
                changes_dropped(),
                task_panics(),
                DEAD_LETTERS.load(Ordering::Relaxed)
            )
        };
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "entityUpdated implies attributeCreated, so a creation notifies; {}",
            diagnosis()
        );
        assert_eq!(
            quiet_hits.load(Ordering::SeqCst),
            0,
            "entityDeleted carries no equivalence and must not fire on a creation"
        );
    }
}

#[cfg(test)]
mod clause_5_2_12_triggers {
    use super::*;
    use serde_json::json;

    /// Table 5.2.12-1 notificationTrigger: "If not present, the default is
    /// the combination \"attributeCreated\" and \"attributeUpdated\".
    /// \"entityUpdated\" is equivalent to the combination
    /// \"attributeCreated\", \"attributeUpdated\" and \"attributeDeleted\"."
    #[test]
    fn entity_updated_expands_to_its_equivalent_attribute_triggers() {
        let has = |v: &[&str], t: &str| v.contains(&t);

        let empty = json!({});
        let default = triggers_of(&empty);
        assert_eq!(default, vec!["attributeCreated", "attributeUpdated"]);
        assert!(
            !has(&default, "attributeDeleted"),
            "the default combination is two triggers, not three"
        );

        let updated = json!({"notificationTrigger": ["entityUpdated"]});
        let expanded = triggers_of(&updated);
        for t in ["attributeCreated", "attributeUpdated", "attributeDeleted"] {
            assert!(has(&expanded, t), "entityUpdated must imply {t}");
        }
        assert!(
            has(&expanded, "entityUpdated"),
            "the declared trigger itself survives the expansion"
        );

        // Only entityUpdated carries the equivalence: the other two entity
        // triggers must NOT gain attribute triggers.
        for t in ["entityCreated", "entityDeleted"] {
            let one = json!({ "notificationTrigger": [t] });
            let only = triggers_of(&one);
            assert_eq!(only, vec![t], "{t} is not an equivalence");
        }

        // Idempotent: an explicit list that already spells the combination
        // out gains nothing, and the equivalent forms agree.
        let spelled = json!({"notificationTrigger":
            ["entityUpdated", "attributeCreated", "attributeUpdated", "attributeDeleted"]});
        let literal = triggers_of(&spelled);
        assert_eq!(literal.len(), 4, "no duplicates are appended");
        let mut a = expanded.clone();
        let mut b = literal.clone();
        a.sort();
        b.sort();
        assert_eq!(a, b, "[\"entityUpdated\"] == the spelled-out combination");
    }

    /// Table 5.2.12-1 scopes watchedAttributes to "Watched Attributes
    /// (Properties or Relationships)", so a write that only moves the
    /// entity's own system members is no attribute-level change and must
    /// raise no attributeCreated/attributeUpdated trigger.
    #[test]
    fn entity_system_members_are_not_attribute_changes() {
        let before = json!({
            "id": "urn:ngsi-ld:Vehicle:1",
            "type": ["Vehicle"],
            "expiresAt": "2030-01-01T00:00:00Z",
        });
        let after = json!({
            "id": "urn:ngsi-ld:Vehicle:1",
            "type": ["Vehicle"],
            "expiresAt": "2031-01-01T00:00:00Z",
            "deletedAt": "2031-01-01T00:00:00Z",
            "modifiedAt": "2026-01-01T00:00:00Z",
            "scope": "/a",
        });
        assert!(
            diff(Some(&before), Some(&after)).is_empty(),
            "entity-level system members are not Attributes"
        );
        // Positive control: a real Property change is still reported.
        let mut with_attr = after.clone();
        with_attr["speed"] = json!([{"type": "Property", "value": 1}]);
        assert_eq!(
            diff(Some(&before), Some(&with_attr)),
            vec![("speed".to_owned(), ChangeClass::Created)]
        );
    }
}

#[cfg(test)]
mod clause_4_5_7_instance_identity {
    use super::*;
    use serde_json::json;

    fn id_of(observed: Option<&str>, dataset: Option<&str>) -> String {
        let mut inst = Map::new();
        if let Some(o) = observed {
            inst.insert("observedAt".into(), json!(o));
        }
        if let Some(d) = dataset {
            inst.insert("datasetId".into(), json!(d));
        }
        instance_id("urn:ngsi-ld:Vehicle:1", "https://a/speed", &inst)
    }

    /// 4.5.7: an instance is the Property "at a particular point in time,
    /// which is recorded as a Temporal Property of the instance (typically
    /// observedAt)" — so the instant, not the way a client spelled it,
    /// decides which instance a record belongs to. 4.6.3 leaves the seconds
    /// fraction optional and accepts a comma separator in requests, and the
    /// broker stores the stamp as written, so one instant reaches this
    /// function under several spellings.
    ///
    /// The consequence is the one the clause names: "Without such an
    /// instanceId, it is not possible to selectively modify or delete
    /// temporal information via the NGSI-LD API. The consequences of this
    /// may be severe in the case of modification or deletion requests for
    /// legal reasons". A correction re-sent with a different spelling landed
    /// on a second row and left the value it was correcting in place.
    #[test]
    fn one_instant_is_one_instance_however_it_is_spelled() {
        let spellings = [
            "2020-01-01T00:00:00Z",
            "2020-01-01T00:00:00.0Z",
            "2020-01-01T00:00:00.000Z",
            "2020-01-01T00:00:00.000000Z",
            "2020-01-01T00:00:00,000Z",
        ];
        let first = id_of(Some(spellings[0]), None);
        for s in &spellings[1..] {
            assert_eq!(id_of(Some(s), None), first, "{s} is the same instant");
        }
        // and with a datasetId, which is part of the same identity
        let first = id_of(Some(spellings[0]), Some("urn:ds:1"));
        for s in &spellings[1..] {
            assert_eq!(
                id_of(Some(s), Some("urn:ds:1")),
                first,
                "{s} is the same instant"
            );
        }
    }

    /// The identity still separates what the clause separates: a different
    /// instant, a different dataset and a different fractional value are
    /// different instances.
    #[test]
    fn different_instants_and_datasets_stay_different_instances() {
        let base = id_of(Some("2020-01-01T00:00:00Z"), None);
        for other in [
            id_of(Some("2020-01-01T00:00:00.500Z"), None),
            id_of(Some("2020-01-01T00:00:01Z"), None),
            id_of(Some("2020-01-02T00:00:00Z"), None),
            id_of(Some("2020-01-01T00:00:00Z"), Some("urn:ds:1")),
        ] {
            assert_ne!(other, base);
        }
        assert_ne!(
            id_of(Some("2020-01-01T00:00:00.500Z"), Some("urn:ds:1")),
            id_of(Some("2020-01-01T00:00:00.500Z"), Some("urn:ds:2"))
        );
        // "Without observedAt there is no instant to key on": a fresh id
        // every time, so two unobserved records never collide.
        assert_ne!(id_of(None, None), id_of(None, None));
    }
}

#[cfg(test)]
mod clause_5_8_6_deletion_payload {
    use super::*;
    use serde_json::json;

    const DC: &str = "https://uri.etsi.org/ngsi-ld/default-context";

    fn ctx() -> Arc<Context> {
        antares_jsonld::Loader::new().core()
    }

    /// Instances of one attribute in a notification data entry, whether the
    /// representation collapsed a single instance to an object or kept an
    /// array.
    fn insts<'a>(entry: &'a Value, name: &str) -> Vec<&'a Value> {
        match entry.get(name) {
            Some(Value::Array(a)) => a.iter().collect(),
            Some(v) => vec![v],
            None => Vec::new(),
        }
    }

    async fn build(
        sub: &Value,
        before: &Value,
        after: Option<&Value>,
        deleted: &[String],
        entity_deleted: bool,
    ) -> Value {
        let st = AppState::new("antares-tombstone-test".into());
        let tenant = TenantId::new("default").expect("tenant");
        let ctx = ctx();
        let data = build_data(
            &st,
            &tenant,
            sub,
            &ctx,
            Some(before),
            after,
            deleted,
            entity_deleted,
            "2026-01-01T00:00:00Z",
        )
        .await;
        assert_eq!(data.len(), 1, "one changed entity ⇒ one data entry");
        data.into_iter().next().expect("entry")
    }

    /// 5.8.6: a deleted Attribute is notified as the NGSI-LD null value of
    /// its own type — `object` for a Relationship, the `{"@none": …}`
    /// languageMap form for a LanguageProperty, `json`, `vocab`, `value`.
    #[tokio::test]
    async fn typed_null_member_per_attribute_type() {
        let cases = [
            ("Property", "value", json!("urn:ngsi-ld:null")),
            ("Relationship", "object", json!("urn:ngsi-ld:null")),
            (
                "LanguageProperty",
                "languageMap",
                json!({"@none": "urn:ngsi-ld:null"}),
            ),
            ("JsonProperty", "json", json!("urn:ngsi-ld:null")),
            ("VocabProperty", "vocab", json!("urn:ngsi-ld:null")),
        ];
        for (atype, member, null_value) in cases {
            let before = json!({
                "id": "urn:ngsi-ld:Vehicle:tomb",
                "type": [format!("{DC}/Vehicle")],
                format!("{DC}/gone"): [{"type": atype, member: json!("previous")}],
            });
            let after = json!({
                "id": "urn:ngsi-ld:Vehicle:tomb",
                "type": [format!("{DC}/Vehicle")],
            });
            let entry = build(
                &json!({"notification": {}}),
                &before,
                Some(&after),
                &[format!("{DC}/gone")],
                false,
            )
            .await;
            let got = insts(&entry, "gone");
            assert_eq!(got.len(), 1, "{atype}: one tombstone");
            assert_eq!(got[0].get("type"), Some(&json!(atype)));
            assert_eq!(
                got[0].get(member),
                Some(&null_value),
                "{atype} tombstones through its own {member} member"
            );
            // showChanges false and sysAttrs false ⇒ neither stamp appears
            assert!(
                got[0].get("deletedAt").is_none(),
                "{atype}: no deletedAt without sysAttrs"
            );
            assert!(
                got[0]
                    .as_object()
                    .is_some_and(|o| !o.keys().any(|k| k.starts_with("previous"))),
                "{atype}: no previous* member without showChanges"
            );
        }
    }

    /// 5.8.6: every deleted instance is its own tombstone carrying its
    /// datasetId, whether the whole Attribute went at once (deleteAll, forge
    /// 75ba471d, 046_22_08) or single instances did, and survivors stay
    /// untouched. A multi-instance Attribute is an Array, deleted or not.
    #[tokio::test]
    async fn whole_attribute_versus_per_instance_deletion() {
        let three = json!([
            {"type": "Property", "value": 1},
            {"type": "Property", "value": 2, "datasetId": "urn:ds:a"},
            {"type": "Property", "value": 3, "datasetId": "urn:ds:b"},
        ]);
        let before = json!({
            "id": "urn:ngsi-ld:Vehicle:multi",
            "type": [format!("{DC}/Vehicle")],
            format!("{DC}/speed"): three,
        });
        let attr = vec![format!("{DC}/speed")];

        // whole attribute gone
        let after = json!({
            "id": "urn:ngsi-ld:Vehicle:multi",
            "type": [format!("{DC}/Vehicle")],
        });
        let entry = build(
            &json!({"notification": {}}),
            &before,
            Some(&after),
            &attr,
            false,
        )
        .await;
        let got = insts(&entry, "speed");
        assert_eq!(got.len(), 3, "one tombstone per deleted instance: {got:?}");
        let mut ds: Vec<Option<&Value>> = got.iter().map(|i| i.get("datasetId")).collect();
        ds.sort_by_key(|d| d.and_then(Value::as_str));
        assert_eq!(
            ds,
            [None, Some(&json!("urn:ds:a")), Some(&json!("urn:ds:b"))],
            "each tombstone keeps its datasetId"
        );
        assert!(
            got.iter()
                .all(|i| i.get("value") == Some(&json!("urn:ngsi-ld:null"))),
            "{got:?}"
        );

        // one instance gone, two survive
        let after = json!({
            "id": "urn:ngsi-ld:Vehicle:multi",
            "type": [format!("{DC}/Vehicle")],
            format!("{DC}/speed"): [
                {"type": "Property", "value": 1},
                {"type": "Property", "value": 3, "datasetId": "urn:ds:b"},
            ],
        });
        let entry = build(
            &json!({"notification": {}}),
            &before,
            Some(&after),
            &attr,
            false,
        )
        .await;
        let got = insts(&entry, "speed");
        assert_eq!(got.len(), 3, "two survivors plus one tombstone");
        let tombs: Vec<&&Value> = got
            .iter()
            .filter(|i| i.get("value") == Some(&json!("urn:ngsi-ld:null")))
            .collect();
        assert_eq!(tombs.len(), 1, "exactly the lost instance is tombstoned");
        assert_eq!(tombs[0].get("datasetId"), Some(&json!("urn:ds:a")));
        for surviving in got
            .iter()
            .filter(|i| i.get("value") != Some(&json!("urn:ngsi-ld:null")))
        {
            assert!(
                surviving.get("deletedAt").is_none(),
                "a surviving instance is not marked deleted"
            );
        }
    }

    /// 5.8.6: "If an Attribute has been deleted, only the name of the
    /// attribute as key and the URI `urn:ngsi-ld:null` as value shall be
    /// provided, unless more information is required. The latter is the case,
    /// if: a datasetId needs to be provided; the notification.sysAttrs is set
    /// to true …; notification.showChanges is set to true …. In all such
    /// cases, a JSON object with all the required information is provided,
    /// where the value or the object is set to the URI `urn:ngsi-ld:null`
    /// respectively or, in case of a LanguageProperty, the languageMap is set
    /// to `{"@none": "urn:ngsi-ld:null"}`."
    ///
    /// The bare-key form is the one an interoperability campaign reads, and
    /// it is reached only through the concise collapse — the tombstone itself
    /// is always built as an object, so nothing below this level can tell the
    /// two apart. 5.5.4 confines the bare form to concise: normalized keeps
    /// the object, and a first-level `urn:ngsi-ld:null` is BadRequestData
    /// everywhere else.
    #[tokio::test]
    async fn a_deleted_attribute_is_bare_unless_it_has_more_to_say() {
        let speed = format!("{DC}/speed");
        let label = format!("{DC}/label");
        let before = json!({
            "id": "urn:ngsi-ld:Vehicle:c",
            "type": [format!("{DC}/Vehicle")],
            speed.clone(): [{"type": "Property", "value": 1}],
            label.clone(): [{"type": "LanguageProperty", "languageMap": {"en": "hi"}}],
        });
        let after = json!({
            "id": "urn:ngsi-ld:Vehicle:c",
            "type": [format!("{DC}/Vehicle")],
        });
        let deleted = [speed.clone(), label.clone()];
        // a nested fn, not a closure: the future borrows the arguments and
        // the `sub` it builds, and a closure cannot return a value borrowing
        // its own captures
        async fn entry(before: &Value, after: &Value, deleted: &[String], n: Value) -> Value {
            build(
                &json!({"notification": n}),
                before,
                Some(after),
                deleted,
                false,
            )
            .await
        }

        // Nothing more is required: the attribute IS the URI.
        for fmt in ["concise", "simplified"] {
            let e = entry(&before, &after, &deleted, json!({"format": fmt})).await;
            assert_eq!(
                e["speed"],
                json!("urn:ngsi-ld:null"),
                "{fmt}: a plain deletion is the bare URI, not an object"
            );
            assert_eq!(
                e["label"],
                json!({"languageMap": {"@none": "urn:ngsi-ld:null"}}),
                "{fmt}: a LanguageProperty deletion is the @none map"
            );
        }

        // Normalized is not the bare form (5.5.4 confines that to concise).
        let e = entry(&before, &after, &deleted, json!({"format": "normalized"})).await;
        assert_eq!(
            e["speed"],
            json!({"type": "Property", "value": "urn:ngsi-ld:null"}),
            "normalized keeps the typed object"
        );

        // sysAttrs: the system-generated sub-attributes have to be provided,
        // so the deletion becomes an object carrying them.
        let e = entry(
            &before,
            &after,
            &deleted,
            json!({"format": "concise", "sysAttrs": true}),
        )
        .await;
        assert_eq!(e["speed"]["value"], json!("urn:ngsi-ld:null"));
        assert_eq!(e["speed"]["deletedAt"], json!("2026-01-01T00:00:00Z"));

        // showChanges: a previous value has to be provided.
        let e = entry(
            &before,
            &after,
            &deleted,
            json!({"format": "concise", "showChanges": true}),
        )
        .await;
        assert_eq!(e["speed"]["value"], json!("urn:ngsi-ld:null"));
        assert_eq!(e["speed"]["previousValue"], json!(1));
        assert_eq!(
            e["label"]["previousLanguageMap"],
            json!({"en": "hi"}),
            "a LanguageProperty reports previousLanguageMap"
        );

        // A datasetId needs to be provided: one instance of two goes, so the
        // tombstone must name which — and stays an object to do it.
        let before_ds = json!({
            "id": "urn:ngsi-ld:Vehicle:c",
            "type": [format!("{DC}/Vehicle")],
            speed.clone(): [{"type": "Property", "value": 1, "datasetId": "urn:ds:a"},
                    {"type": "Property", "value": 2, "datasetId": "urn:ds:b"}],
        });
        let after_ds = json!({
            "id": "urn:ngsi-ld:Vehicle:c",
            "type": [format!("{DC}/Vehicle")],
            speed.clone(): [{"type": "Property", "value": 2, "datasetId": "urn:ds:b"}],
        });
        let e = build(
            &json!({"notification": {"format": "concise"}}),
            &before_ds,
            Some(&after_ds),
            &[speed],
            false,
        )
        .await;
        let gone = e["speed"]
            .as_array()
            .and_then(|a| a.iter().find(|i| i["value"] == json!("urn:ngsi-ld:null")))
            .expect("the lost instance is tombstoned");
        assert_eq!(gone["datasetId"], json!("urn:ds:a"));
        assert!(
            e["speed"]
                .as_array()
                .is_some_and(|a| a.iter().any(|i| i["value"] == json!(2))),
            "the surviving instance is still reported: {e}"
        );
    }

    /// 5.8.6: "If the notification was triggered by the deletion of an Entity
    /// and the notification.showChanges member is not set to true, only the
    /// deletedAt system property shall be provided", even when the same
    /// deletion also fires attributeDeleted (Coraine subscription_notify_update_delete).
    #[tokio::test]
    async fn an_entity_deletion_notifies_deleted_at_only() {
        let before = json!({
            "id": "urn:ngsi-ld:Vehicle:gone",
            "type": [format!("{DC}/Vehicle")],
            format!("{DC}/speed"): [{"type": "Property", "value": 1}],
        });
        let entry = build(
            &json!({"notification": {}}),
            &before,
            None,
            &[format!("{DC}/speed")],
            true,
        )
        .await;
        assert!(entry.get("speed").is_none(), "{entry}");
        assert!(
            entry.get(format!("{DC}/speed").as_str()).is_none(),
            "{entry}"
        );
        assert_eq!(entry["deletedAt"], json!("2026-01-01T00:00:00Z"));
    }

    /// 5.8.6 with sysAttrs and showChanges: the tombstone carries deletedAt
    /// and the previous value of its own typed member.
    #[tokio::test]
    async fn sys_attrs_and_show_changes_stamp_the_tombstone() {
        let before = json!({
            "id": "urn:ngsi-ld:Vehicle:stamp",
            "type": [format!("{DC}/Vehicle")],
            format!("{DC}/where"): [{"type": "Relationship", "object": "urn:ngsi-ld:P:1",
                                     "createdAt": "2025-01-01T00:00:00Z"}],
        });
        let after = json!({
            "id": "urn:ngsi-ld:Vehicle:stamp",
            "type": [format!("{DC}/Vehicle")],
        });
        let sub = json!({"notification": {"sysAttrs": true, "showChanges": true}});
        let entry = build(&sub, &before, Some(&after), &[format!("{DC}/where")], false).await;
        let got = insts(&entry, "where");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].get("object"), Some(&json!("urn:ngsi-ld:null")));
        assert_eq!(
            got[0].get("deletedAt"),
            Some(&json!("2026-01-01T00:00:00Z"))
        );
        assert_eq!(
            got[0].get("previousObject"),
            Some(&json!("urn:ngsi-ld:P:1")),
            "showChanges reports the previous object, not previousValue"
        );
        assert!(
            got[0].get("previousValue").is_none(),
            "a Relationship never reports previousValue"
        );
    }
}

#[cfg(test)]
mod candidate_index {
    use super::*;
    use serde_json::json;

    const DC: &str = "https://uri.etsi.org/ngsi-ld/default-context";

    /// The index may over-select, never under-select: every subscription
    /// `selector_match` accepts for a change must come back from
    /// `candidates()` for that change's types and changed attributes —
    /// otherwise it silently stops firing (5.8.6).
    #[test]
    fn candidates_never_under_select_for_any_selector_shape() {
        let ctx = antares_jsonld::Loader::new().core();
        let doc = json!({
            "id": "urn:ngsi-ld:Vehicle:1",
            "type": [format!("{DC}/Vehicle")],
            format!("{DC}/speed"): [{"type": "Property", "value": 1}],
        });
        let shapes: Vec<(&str, Value)> = vec![
            (
                "plain type",
                json!({"entities": [{"type": format!("{DC}/Vehicle")}]}),
            ),
            (
                "multiple types",
                json!({"entities": [{"type": format!("{DC}/Vehicle")},
                                    {"type": format!("{DC}/Building")}]}),
            ),
            (
                "id only",
                json!({"entities": [{"id": "urn:ngsi-ld:Vehicle:1"}]}),
            ),
            (
                "idPattern only",
                json!({"entities": [{"idPattern": "^urn:ngsi-ld:Vehicle:"}]}),
            ),
            (
                "type selection expression",
                json!({"entities": [{"type": format!("{DC}/Vehicle|{DC}/Building")}]}),
            ),
            (
                "watchedAttributes only",
                json!({"watchedAttributes": [format!("{DC}/speed")]}),
            ),
            (
                "watchedAttributes with entities",
                json!({"entities": [{"type": format!("{DC}/Vehicle")}],
                       "watchedAttributes": [format!("{DC}/speed")]}),
            ),
            (
                "type with idPattern",
                json!({"entities": [{"type": format!("{DC}/Vehicle"),
                                     "idPattern": "^urn:ngsi-ld:Vehicle:"}]}),
            ),
        ];
        let mirror = SubMirror::default();
        for (i, (_, doc)) in shapes.iter().enumerate() {
            let mut sub = doc.clone();
            sub["id"] = json!(format!("urn:ngsi-ld:Subscription:{i}"));
            mirror.apply(
                "default",
                &format!("urn:ngsi-ld:Subscription:{i}"),
                Some(sub),
            );
        }
        let types = [format!("{DC}/Vehicle")];
        let changed = [format!("{DC}/speed")];
        let got = mirror.candidates(
            "default",
            &types.iter().map(String::as_str).collect::<Vec<_>>(),
            &changed.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        for (i, (name, shape)) in shapes.iter().enumerate() {
            if !selector_match(shape, &doc, &ctx) {
                continue;
            }
            let id = format!("urn:ngsi-ld:Subscription:{i}");
            assert!(
                got.iter()
                    .any(|c| c.get("id").and_then(Value::as_str) == Some(id.as_str())),
                "{name}: selector_match accepts it, so candidates() must return it"
            );
        }
        // A change touching neither the type nor the watched attribute still
        // yields the shapes the index cannot classify — over-selection is
        // allowed — but never the exactly-classified plain-type subscription.
        let other = mirror.candidates("default", &[&format!("{DC}/Device")], &[]);
        assert!(
            !other
                .iter()
                .any(|c| c.get("id").and_then(Value::as_str) == Some("urn:ngsi-ld:Subscription:0")),
            "an exactly classified type subscription is not evaluated for other types"
        );
    }

    /// The two classification sites must stay in the superset relation:
    /// every character `selector_match` reads as a 4.17 type-selection
    /// expression has to send the subscription to the broad bucket, or the
    /// index would under-select.
    #[test]
    fn type_selection_expressions_are_always_broad() {
        for c in ['|', ',', ';', '('] {
            let sub = json!({"entities": [{"type": format!("{DC}/A{c}{DC}/B")}]});
            assert!(
                matches!(index_keys(&sub), Keys::Broad),
                "a type containing {c:?} is a selection expression for selector_match, \
                 so the index must not classify it as a plain type"
            );
        }
        // Table 5.2.33-1's "*" is the other member of that relation: it is
        // stored raw and `selector_match` reads it as every type, so an index
        // that classified it as the literal type "*" would look up a key no
        // change carries and the subscription would never be a candidate.
        assert!(
            matches!(
                index_keys(&json!({"entities": [{"type": "*"}]})),
                Keys::Broad
            ),
            "a \"*\" selector matches every type, so the index must go broad"
        );
        assert!(matches!(
            index_keys(&json!({"entities": [{"type": format!("{DC}/Vehicle")}]})),
            Keys::Types(_)
        ));
    }
}

/// Table 5.2.14.2-1 bookkeeping around a delivery that fails or is never
/// attempted at all.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod clause_5_2_14_2_bookkeeping {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const SUB_ID: &str = "urn:ngsi-ld:Subscription:book";

    /// An endpoint answering `status`, counting the requests that reach it.
    async fn endpoint(status: axum::http::StatusCode) -> (String, Arc<AtomicUsize>) {
        let hits: Arc<AtomicUsize> = Arc::default();
        let seen = hits.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let app = axum::Router::new().route(
            "/notify",
            axum::routing::post(move || {
                let seen = seen.clone();
                async move {
                    seen.fetch_add(1, Ordering::SeqCst);
                    status
                }
            }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        (format!("http://{addr}/notify"), hits)
    }

    async fn stored_notification(st: &AppState, tenant: &TenantId) -> Value {
        st.store
            .get(tenant, Kind::Subscription, SUB_ID)
            .await
            .expect("store read")
            .expect("subscription row")
            .get("notification")
            .cloned()
            .expect("notification member")
    }

    async fn send(st: &AppState, tenant: &TenantId, sub: &Value) {
        let ctx = antares_jsonld::Loader::new().core();
        deliver_as(
            st,
            tenant,
            Kind::Subscription,
            sub,
            "Notification",
            vec![json!({"id": "urn:ngsi-ld:Vehicle:1", "type": "Vehicle"})],
            &ctx,
            None,
        )
        .await;
    }

    async fn subscribe(st: &AppState, tenant: &TenantId, uri: &str) -> Value {
        let sub = json!({
            "id": SUB_ID,
            "type": "Subscription",
            "entities": [{"type": "Vehicle"}],
            "notification": {"endpoint": {"uri": uri}},
        });
        st.store
            .create(tenant, Kind::Subscription, SUB_ID, sub.clone())
            .await
            .expect("subscription row");
        sub
    }

    /// Table 5.2.14.2-1 timesFailed: "Number of times an unsuccessful
    /// response (or timeout) has been received when delivering the
    /// notification" — an output-only member implementations shall generate.
    #[tokio::test(flavor = "multi_thread")]
    async fn failed_delivery_generates_and_increments_times_failed() {
        crate::allow_private();
        let (uri, hits) = endpoint(axum::http::StatusCode::INTERNAL_SERVER_ERROR).await;
        let st = AppState::new("antares-times-failed".into());
        let tenant = TenantId::new("default").expect("tenant");
        let sub = subscribe(&st, &tenant, &uri).await;

        send(&st, &tenant, &sub).await;
        let n = stored_notification(&st, &tenant).await;
        assert_eq!(hits.load(Ordering::SeqCst), 1, "the endpoint was tried");
        assert_eq!(n.get("timesFailed"), Some(&json!(1)));
        assert_eq!(n.get("timesSent"), Some(&json!(1)), "the attempt was sent");
        assert_eq!(n.get("status"), Some(&json!("failed")));
        assert!(
            n.get("lastSuccess").is_none(),
            "a failure rolls the optimistic lastSuccess back"
        );

        send(&st, &tenant, &sub).await;
        let n = stored_notification(&st, &tenant).await;
        assert_eq!(
            n.get("timesFailed"),
            Some(&json!(2)),
            "timesFailed counts every unsuccessful response"
        );
    }

    /// Table 5.2.14.2-1 timesSent = "Number of times that the notification
    /// has been sent" and lastNotification = "the instant when the last
    /// notification has been sent": a change suppressed by the open circuit
    /// never reaches the wire, so neither member may move.
    #[tokio::test(flavor = "multi_thread")]
    async fn breaker_suppressed_delivery_does_not_move_times_sent() {
        crate::allow_private();
        let (uri, hits) = endpoint(axum::http::StatusCode::OK).await;
        let st = AppState::new("antares-breaker-bookkeeping".into());
        let tenant = TenantId::new("default").expect("tenant");
        let sub = subscribe(&st, &tenant, &uri).await;
        for _ in 0..crate::egress::TRIP_AFTER {
            st.egress.record_failure(tenant.as_str(), &uri);
        }
        assert!(
            st.egress.is_open(tenant.as_str(), &uri),
            "the destination is open-circuit"
        );

        send(&st, &tenant, &sub).await;
        let n = stored_notification(&st, &tenant).await;
        assert_eq!(hits.load(Ordering::SeqCst), 0, "nothing left the process");
        assert!(
            n.get("timesSent").is_none(),
            "a suppressed notification was never sent"
        );
        assert!(
            n.get("lastNotification").is_none(),
            "lastNotification is the instant a notification was sent"
        );

        // Positive control: with the circuit closed the same call delivers,
        // so the assertions above cannot pass vacuously.
        st.egress.record_success(tenant.as_str(), &uri);
        send(&st, &tenant, &sub).await;
        let n = stored_notification(&st, &tenant).await;
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert_eq!(n.get("timesSent"), Some(&json!(1)));
        assert!(n.get("lastNotification").is_some());
    }
}

/// Delivery policy: retries are transport under one notification (5.8.6
/// books the notification once), a success by retry sets lastSuccess and
/// status ok, an exhausted policy leaves exactly one dead letter, and the
/// default policy is the single attempt the clause describes.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod delivery_policy_tests {
    use super::*;
    use antares_notifier::DeliveryPolicy;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    const SUB_ID: &str = "urn:ngsi-ld:Subscription:policy";

    fn policy(attempts: u32, backoff_ms: u64) -> DeliveryPolicy {
        DeliveryPolicy {
            attempts,
            backoff: Duration::from_millis(backoff_ms),
            jitter: 0.0,
            max_age: Duration::from_secs(60),
        }
    }

    /// Answers 500 to the first `fail_first` requests, 200 afterwards.
    async fn flaky_endpoint(fail_first: usize) -> (String, Arc<AtomicUsize>) {
        let hits: Arc<AtomicUsize> = Arc::default();
        let seen = hits.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let app = axum::Router::new().route(
            "/notify",
            axum::routing::post(move || {
                let seen = seen.clone();
                async move {
                    let n = seen.fetch_add(1, Ordering::SeqCst);
                    if n < fail_first {
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR
                    } else {
                        axum::http::StatusCode::OK
                    }
                }
            }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        (format!("http://{addr}/notify"), hits)
    }

    fn state(p: DeliveryPolicy) -> (AppState, TenantId) {
        crate::allow_private();
        let mut st = AppState::new("antares-policy".into());
        st.delivery = p;
        (st, TenantId::new("default").expect("tenant"))
    }

    async fn subscribe(st: &AppState, tenant: &TenantId, id: &str, uri: &str) -> Value {
        let sub = json!({
            "id": id,
            "type": "Subscription",
            "entities": [{"type": "Vehicle"}],
            "notification": {"endpoint": {"uri": uri}},
        });
        st.store
            .create(tenant, Kind::Subscription, id, sub.clone())
            .await
            .expect("subscription row");
        sub
    }

    async fn send(st: &AppState, tenant: &TenantId, sub: &Value) {
        let ctx = antares_jsonld::Loader::new().core();
        deliver_as(
            st,
            tenant,
            Kind::Subscription,
            sub,
            "Notification",
            vec![json!({"id": "urn:ngsi-ld:Vehicle:1", "type": "Vehicle"})],
            &ctx,
            None,
        )
        .await;
    }

    async fn notification(st: &AppState, tenant: &TenantId, id: &str) -> Value {
        st.store
            .get(tenant, Kind::Subscription, id)
            .await
            .expect("store read")
            .expect("subscription row")["notification"]
            .clone()
    }

    async fn letters(st: &AppState, tenant: &TenantId) -> Vec<Value> {
        st.store.list(tenant, Kind::DeadLetter).await.expect("list")
    }

    async fn wait_until<F, Fut>(mut cond: F, what: &str)
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        for _ in 0..100 {
            if cond().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("timed out waiting for {what}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_retry_that_succeeds_is_one_notification() {
        let (st, t) = state(policy(3, 50));
        let (uri, hits) = flaky_endpoint(2).await;
        let sub = subscribe(&st, &t, SUB_ID, &uri).await;
        send(&st, &t, &sub).await;
        // the first attempt is booked at once, as 5.8.6 says
        let n = notification(&st, &t, SUB_ID).await;
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert_eq!(n["timesSent"], json!(1));
        assert_eq!(n["status"], json!("failed"));
        assert!(n.get("lastSuccess").is_none());
        wait_until(
            || async { notification(&st, &t, SUB_ID).await["status"] == json!("ok") },
            "retry success",
        )
        .await;
        let n = notification(&st, &t, SUB_ID).await;
        assert_eq!(hits.load(Ordering::SeqCst), 3, "two retries were made");
        assert_eq!(
            n["timesSent"],
            json!(1),
            "retries never count as a second notification"
        );
        assert_eq!(
            n["timesFailed"],
            json!(1),
            "the failed first attempt was booked once"
        );
        assert!(n.get("lastSuccess").is_some());
        assert!(
            n.get("lastFailure").is_some(),
            "the earlier failure stays recorded"
        );
        assert!(
            letters(&st, &t).await.is_empty(),
            "a delivered notification is no dead letter"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_exhausted_policy_leaves_exactly_one_dead_letter() {
        let (st, t) = state(policy(2, 50));
        let (uri, hits) = flaky_endpoint(usize::MAX).await;
        let sub = subscribe(&st, &t, SUB_ID, &uri).await;
        let before = dead_letters_written();
        send(&st, &t, &sub).await;
        wait_until(
            || async { !letters(&st, &t).await.is_empty() },
            "dead letter",
        )
        .await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        let l = letters(&st, &t).await;
        assert_eq!(l.len(), 1, "{l:?}");
        assert_eq!(hits.load(Ordering::SeqCst), 2, "attempts = policy.attempts");
        let l = &l[0];
        assert_eq!(l["subscriptionId"], json!(SUB_ID));
        assert_eq!(l["attempts"], json!(2));
        assert_eq!(l["binding"], json!("http"));
        assert_eq!(l["uri"], json!(uri));
        assert_eq!(l["lastError"], json!("HTTP 500"));
        assert_eq!(l["payload"]["type"], json!("Notification"));
        assert_eq!(l["payload"]["subscriptionId"], json!(SUB_ID));
        assert!(l["id"]
            .as_str()
            .is_some_and(|i| i.starts_with("urn:ngsi-ld:DeadLetter:")));
        // the letter carries the endpoint members the binding renders from,
        // so a replay produces the identical request
        assert_eq!(l["accept"], json!("application/json"));
        assert!(l["link"].as_str().is_some_and(|v| v.contains("rel=")));
        assert!(l["receiverInfo"].is_array());
        assert!(dead_letters_written() > before);
        let n = notification(&st, &t, SUB_ID).await;
        assert_eq!(n["timesSent"], json!(1));
        assert_eq!(n["timesFailed"], json!(1));
        assert_eq!(n["status"], json!("failed"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_default_policy_never_retries_and_never_dead_letters() {
        let (st, t) = state(DeliveryPolicy::default());
        let (uri, hits) = flaky_endpoint(usize::MAX).await;
        let sub = subscribe(&st, &t, SUB_ID, &uri).await;
        send(&st, &t, &sub).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert!(letters(&st, &t).await.is_empty());
        assert_eq!(
            notification(&st, &t, SUB_ID).await["status"],
            json!("failed")
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_backoff_on_one_subscription_does_not_delay_another() {
        // Both stretch under a sanitizer (ThreadSanitizer took 2 s for the
        // two sends, T-4250); the bound stays a third of the backoff, so a
        // backoff on the delivery path still fails it.
        let slow = crate::state::slow_factor();
        let (st, t) = state(policy(2, 3_000 * slow));
        let (dead, _) = flaky_endpoint(usize::MAX).await;
        let (live, live_hits) = flaky_endpoint(0).await;
        let a = subscribe(&st, &t, "urn:ngsi-ld:Subscription:a", &dead).await;
        let b = subscribe(&st, &t, "urn:ngsi-ld:Subscription:b", &live).await;
        let started = std::time::Instant::now();
        send(&st, &t, &a).await;
        send(&st, &t, &b).await;
        assert_eq!(live_hits.load(Ordering::SeqCst), 1, "B delivered");
        assert!(
            started.elapsed() < Duration::from_secs(slow),
            "A's {} s backoff must not sit on the delivery path: {:?}",
            3 * slow,
            started.elapsed()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn retries_stop_when_the_subscription_is_deleted() {
        let (st, t) = state(policy(4, 100));
        let (uri, hits) = flaky_endpoint(usize::MAX).await;
        let sub = subscribe(&st, &t, SUB_ID, &uri).await;
        send(&st, &t, &sub).await;
        st.store
            .delete(&t, Kind::Subscription, SUB_ID)
            .await
            .expect("delete");
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "no retry for a gone subscription"
        );
        assert!(
            letters(&st, &t).await.is_empty(),
            "no dead letter for a gone subscription"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_egress_refusal_is_never_retried() {
        let (mut st, t) = state(policy(3, 50));
        // the deny policy is built directly: the environment is shared by
        // every test thread in this process, and a state constructed while
        // the variable read "false" would refuse its loopback endpoint for
        // the rest of its life
        st.egress = Arc::new(crate::egress::Egress::new(antares_jsonld::EgressPolicy {
            allow_private: false,
            private_hosts: &[],
        }));
        let (uri, hits) = flaky_endpoint(0).await;
        let sub = subscribe(&st, &t, SUB_ID, &uri).await;
        send(&st, &t, &sub).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(
            hits.load(Ordering::SeqCst),
            0,
            "policy refusal: nothing leaves"
        );
        assert!(
            letters(&st, &t).await.is_empty(),
            "a policy verdict is not a transport failure"
        );
        assert_eq!(
            notification(&st, &t, SUB_ID).await["status"],
            json!("failed")
        );
    }

    /// T-3908: listing a name in `private_hosts` never opens the broker's own
    /// host. A subscription naming `localhost` is refused at delivery like
    /// any private destination: nothing leaves, the status is failed.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_listed_name_on_the_local_host_is_refused_at_delivery() {
        let (mut st, t) = state(policy(3, 50));
        let hosts: &'static [String] = vec!["localhost".to_owned()].leak();
        st.egress = Arc::new(crate::egress::Egress::new(antares_jsonld::EgressPolicy {
            allow_private: false,
            private_hosts: hosts,
        }));
        let (uri, hits) = flaky_endpoint(0).await;
        let uri = uri.replace("127.0.0.1", "localhost");
        let sub = subscribe(&st, &t, SUB_ID, &uri).await;
        send(&st, &t, &sub).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(
            hits.load(Ordering::SeqCst),
            0,
            "nothing reaches the local host"
        );
        assert!(
            letters(&st, &t).await.is_empty(),
            "a policy verdict, not a transport failure"
        );
        assert_eq!(
            notification(&st, &t, SUB_ID).await["status"],
            json!("failed")
        );
    }
}

/// 5.8.6 periodic notifications: what a due `timeInterval` subscription
/// reads, what it sends, and which ticks it costs nothing at all.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod clause_5_8_6_periodic_sweep {
    use super::*;
    use serde_json::json;

    const DC: &str = "https://uri.etsi.org/ngsi-ld/default-context";

    fn entity(id: &str, ty: &str) -> Value {
        json!({"id": id, "type": [format!("{DC}/{ty}")]})
    }

    /// 5.8.6 narrows the periodic read to "all the subscribed Entities" — and
    /// a narrowing may only ever over-select: every Entity the arbiter
    /// (`selector_match`) accepts has to survive both predicates the sweep
    /// hands the store, or that subscription silently stops reporting it.
    #[test]
    fn periodic_read_narrowing_never_under_selects() {
        let ctx = antares_jsonld::Loader::new().core();
        let docs = [
            entity("urn:ngsi-ld:Vehicle:1", "Vehicle"),
            entity("urn:ngsi-ld:Vehicle:2", "Vehicle"),
            entity("urn:ngsi-ld:Building:1", "Building"),
        ];
        let subs: Vec<(&str, Value)> = vec![
            (
                "plain type",
                json!({"entities": [{"type": format!("{DC}/Vehicle")}]}),
            ),
            (
                "one id with its type",
                json!({"entities": [{"id": "urn:ngsi-ld:Vehicle:2",
                                     "type": format!("{DC}/Vehicle")}]}),
            ),
            (
                "id array",
                json!({"entities": [{"id": ["urn:ngsi-ld:Vehicle:1",
                                            "urn:ngsi-ld:Building:1"]}]}),
            ),
            (
                "one id plus a bare-type entry",
                json!({"entities": [{"id": "urn:ngsi-ld:Vehicle:1"},
                                    {"type": format!("{DC}/Building")}]}),
            ),
            (
                "idPattern only",
                json!({"entities": [{"idPattern": "^urn:ngsi-ld:Vehicle:"}]}),
            ),
            (
                "id overriding a contradicting idPattern",
                json!({"entities": [{"id": "urn:ngsi-ld:Vehicle:1",
                                     "idPattern": "^urn:ngsi-ld:Building:"}]}),
            ),
            (
                "type selection expression",
                json!({"entities": [{"type": format!("{DC}/Vehicle|{DC}/Building")}]}),
            ),
            (
                "no entities selector at all",
                json!({"watchedAttributes": [format!("{DC}/speed")]}),
            ),
        ];
        for (name, sub) in &subs {
            let ids = selector_ids(sub);
            let type_groups: Vec<Vec<String>> = match index_keys(sub) {
                Keys::Types(ts) => ts.into_iter().map(|t| vec![t]).collect(),
                _ => Vec::new(),
            };
            for doc in &docs {
                if !selector_match(sub, doc, &ctx) {
                    continue;
                }
                let id = doc["id"].as_str().expect("id");
                if let Some(ids) = &ids {
                    assert!(
                        ids.iter().any(|i| i == id),
                        "{name}: selector_match accepts {id}, so the id narrowing must keep it"
                    );
                }
                if !type_groups.is_empty() {
                    let types: Vec<&str> = doc["type"]
                        .as_array()
                        .expect("type array")
                        .iter()
                        .filter_map(Value::as_str)
                        .collect();
                    assert!(
                        type_groups
                            .iter()
                            .any(|g| g.iter().all(|t| types.contains(&t.as_str()))),
                        "{name}: selector_match accepts {id}, so the type narrowing must keep it"
                    );
                }
            }
        }
    }

    /// Table 5.2.33-1: `id` is a String or a String[] and takes precedence
    /// over `idPattern`, so it pins the read exactly — while any entry
    /// leaving the id open (a bare type, an idPattern, no selector at all)
    /// must yield NO id predicate, since the OR-ed selector then admits
    /// Entities no listed id names.
    #[test]
    fn periodic_read_narrows_by_id_only_when_every_selector_entry_pins_one() {
        assert_eq!(
            selector_ids(&json!({"entities": [{"id": "urn:x:A", "type": "T"}]})),
            Some(vec!["urn:x:A".to_owned()])
        );
        assert_eq!(
            selector_ids(&json!({"entities": [{"id": ["urn:x:A", "urn:x:B"]},
                                              {"id": "urn:x:C"}]})),
            Some(vec![
                "urn:x:A".to_owned(),
                "urn:x:B".to_owned(),
                "urn:x:C".to_owned()
            ])
        );
        for open in [
            json!({"entities": [{"id": "urn:x:A"}, {"type": "T"}]}),
            json!({"entities": [{"idPattern": "^urn:x:"}]}),
            json!({"entities": [{"type": "T"}]}),
            json!({"entities": []}),
            json!({"watchedAttributes": ["a"]}),
            json!({}),
            json!({"entities": [{"id": {"not": "a string"}}]}),
        ] {
            assert_eq!(
                selector_ids(&open),
                None,
                "{open} leaves the id open — narrowing by id would drop matching Entities"
            );
        }
    }

    /// An endpoint that keeps every notification body it receives.
    async fn recording_endpoint() -> (String, Arc<std::sync::Mutex<Vec<Value>>>) {
        let seen: Arc<std::sync::Mutex<Vec<Value>>> = Arc::default();
        let sink = seen.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let app = axum::Router::new().route(
            "/notify",
            axum::routing::post(move |body: String| {
                let sink = sink.clone();
                async move {
                    if let Ok(v) = serde_json::from_str::<Value>(&body) {
                        sink.lock().expect("recorded bodies").push(v);
                    }
                    axum::http::StatusCode::OK
                }
            }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        (format!("http://{addr}/notify"), seen)
    }

    /// A periodic subscription created `age_s` seconds ago with a one-second
    /// interval — due on the next sweep.
    fn periodic(id: &str, uri: &str, entities: Value, age_s: i64) -> Value {
        let created = chrono::Utc::now() - chrono::Duration::seconds(age_s);
        json!({
            "id": id,
            "type": "Subscription",
            "timeInterval": 1,
            "createdAt": created.to_rfc3339(),
            "entities": entities,
            "notification": {"endpoint": {"uri": uri, "accept": "application/json"}},
        })
    }

    async fn install(st: &AppState, tenant: &TenantId, sub: &Value) {
        let id = sub["id"].as_str().expect("sub id");
        st.store
            .create(tenant, Kind::Subscription, id, sub.clone())
            .await
            .expect("subscription row");
        if let Some(m) = &st.sub_mirror {
            m.apply(tenant.as_str(), id, Some(sub.clone()));
        }
    }

    fn state_with_mirror(alias: &str) -> (AppState, Arc<SubMirror>, TenantId) {
        let mut st = AppState::new(alias.into());
        let mirror = Arc::new(SubMirror::default());
        st.sub_mirror = Some(mirror.clone());
        let tenant = TenantId::new("default").expect("tenant");
        (st, mirror, tenant)
    }

    /// 5.8.6: the periodic Notification carries "all the subscribed Entities
    /// that match the query, geoquery and Scope query conditions" — the one
    /// Entity this selector names out of a populated tenant, and nothing
    /// else. And for the subscription whose selector matches nothing: "If
    /// there are no matching Entities, no Notification is sent."
    #[tokio::test(flavor = "multi_thread")]
    async fn periodic_sweep_notifies_only_the_subscribed_entities() {
        crate::allow_private();
        let (uri, seen) = recording_endpoint().await;
        let (st, _mirror, tenant) = state_with_mirror("antares-periodic-narrowing");
        for (id, ty) in [
            ("urn:ngsi-ld:Vehicle:1", "Vehicle"),
            ("urn:ngsi-ld:Vehicle:2", "Vehicle"),
            ("urn:ngsi-ld:Vehicle:3", "Vehicle"),
            ("urn:ngsi-ld:Building:1", "Building"),
        ] {
            st.store
                .create(&tenant, Kind::Entity, id, entity(id, ty))
                .await
                .expect("entity row");
        }
        install(
            &st,
            &tenant,
            &periodic(
                "urn:ngsi-ld:Subscription:one",
                &uri,
                json!([{"id": "urn:ngsi-ld:Vehicle:2", "type": format!("{DC}/Vehicle")}]),
                10,
            ),
        )
        .await;
        install(
            &st,
            &tenant,
            &periodic(
                "urn:ngsi-ld:Subscription:none",
                &uri,
                json!([{"id": "urn:ngsi-ld:Vehicle:404", "type": format!("{DC}/Vehicle")}]),
                10,
            ),
        )
        .await;

        interval_tick(&st).await;

        let bodies = seen.lock().expect("recorded bodies").clone();
        assert_eq!(bodies.len(), 1, "exactly one subscription had a match");
        let body = &bodies[0];
        assert_eq!(
            body["subscriptionId"], "urn:ngsi-ld:Subscription:one",
            "a subscription matching no Entity sends no Notification"
        );
        let data = body["data"].as_array().expect("data array");
        assert_eq!(data.len(), 1, "only the subscribed Entity is included");
        assert_eq!(data[0]["id"], "urn:ngsi-ld:Vehicle:2");
        for other in [
            "urn:ngsi-ld:Vehicle:1",
            "urn:ngsi-ld:Vehicle:3",
            "urn:ngsi-ld:Building:1",
        ] {
            assert!(
                !body.to_string().contains(other),
                "{other} is not subscribed and must not appear in the notification"
            );
        }
    }

    /// F-ANT3-020: a type-only selector reads its match set a page at a
    /// time and sends each page as its own Notification: three pages of
    /// Vehicles arrive whole, in more than one Notification.
    #[tokio::test(flavor = "multi_thread")]
    async fn periodic_sweep_pages_a_large_match_set() {
        crate::allow_private();
        let (uri, seen) = recording_endpoint().await;
        let (mut st, _mirror, tenant) = state_with_mirror("antares-periodic-paged");
        st.max_limit = 4;
        let n = 12;
        for i in 0..n {
            let id = format!("urn:ngsi-ld:Vehicle:p{i:02}");
            st.store
                .create(&tenant, Kind::Entity, &id, entity(&id, "Vehicle"))
                .await
                .expect("entity row");
        }
        install(
            &st,
            &tenant,
            &periodic(
                "urn:ngsi-ld:Subscription:paged",
                &uri,
                json!([{"type": format!("{DC}/Vehicle")}]),
                10,
            ),
        )
        .await;

        interval_tick(&st).await;

        let bodies = seen.lock().expect("recorded bodies").clone();
        assert!(
            bodies.len() > 1,
            "one Notification per page: {}",
            bodies.len()
        );
        let mut ids: Vec<String> = bodies
            .iter()
            .flat_map(|b| b["data"].as_array().cloned().unwrap_or_default())
            .filter_map(|d| d["id"].as_str().map(str::to_owned))
            .collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n, "every Vehicle is notified: {ids:?}");
    }

    /// The sweep clock: 5.8.6 sends the periodic Notification "when the time
    /// interval (in seconds) specified in such value field is reached", so a
    /// tick before the earliest such instant cannot fire and must not sweep —
    /// while writing a periodic subscription clears the clock, because a
    /// subscription the previous sweep never saw may be due sooner.
    #[tokio::test(flavor = "multi_thread")]
    async fn armed_sweep_clock_skips_the_tick_until_a_subscription_write_clears_it() {
        use std::sync::atomic::Ordering::Relaxed;
        crate::allow_private();
        let (uri, seen) = recording_endpoint().await;
        let (st, mirror, tenant) = state_with_mirror("antares-periodic-clock");
        st.store
            .create(
                &tenant,
                Kind::Entity,
                "urn:ngsi-ld:Vehicle:1",
                entity("urn:ngsi-ld:Vehicle:1", "Vehicle"),
            )
            .await
            .expect("entity row");
        let sub = periodic(
            "urn:ngsi-ld:Subscription:clock",
            &uri,
            json!([{"type": format!("{DC}/Vehicle")}]),
            10,
        );
        install(&st, &tenant, &sub).await;
        let armed = chrono::Utc::now().timestamp_millis() + 60_000;
        mirror.next_sub_sweep_ms.store(armed, Relaxed);

        interval_tick(&st).await;
        assert!(
            seen.lock().expect("recorded bodies").is_empty(),
            "a tick before the clock must not fire, due subscription or not"
        );
        assert_eq!(
            mirror.next_sub_sweep_ms.load(Relaxed),
            armed,
            "a skipped tick leaves the clock alone"
        );

        // what a subscription write does
        mirror.apply(
            tenant.as_str(),
            sub["id"].as_str().expect("sub id"),
            Some(sub.clone()),
        );
        assert_eq!(
            mirror.next_sub_sweep_ms.load(Relaxed),
            0,
            "writing a periodic subscription clears the sweep clock"
        );

        interval_tick(&st).await;
        assert_eq!(
            seen.lock().expect("recorded bodies").len(),
            1,
            "with the clock cleared the due subscription fires"
        );
        assert!(
            mirror.next_sub_sweep_ms.load(Relaxed) > chrono::Utc::now().timestamp_millis(),
            "after firing, the clock points at the next due instant"
        );
    }

    /// 5.11.7: a Context Source Registration Subscription with `timeInterval`
    /// fires periodically. Its writes reach the sweep as a signal rather than
    /// as a mirrored document, so a write must clear the clock the way a
    /// Subscription write does — otherwise a new one waits out a clock
    /// computed before it existed.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_written_csource_subscription_clears_the_csub_sweep_clock() {
        use std::sync::atomic::Ordering::Relaxed;
        let (_st, mirror, _tenant) = state_with_mirror("antares-csub-clock");
        let armed = chrono::Utc::now().timestamp_millis() + 600_000;
        mirror.next_csub_sweep_ms.store(armed, Relaxed);
        assert_eq!(
            mirror.next_csub_sweep_ms.load(Relaxed),
            armed,
            "the clock stands until something writes"
        );

        mirror.csub_written();

        assert_eq!(
            mirror.next_csub_sweep_ms.load(Relaxed),
            0,
            "writing a Context Source Registration Subscription clears the sweep clock"
        );
    }

    /// With nothing periodic to serve, the sweep must park past the next few
    /// ticks instead of re-listing every tenant's Context Source Registration
    /// Subscriptions every second: at the tenant target that poll is the
    /// broker's whole idle cost.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_sweep_that_finds_nothing_periodic_parks_the_csub_clock() {
        use std::sync::atomic::Ordering::Relaxed;
        let (st, mirror, _tenant) = state_with_mirror("antares-csub-park");
        mirror.next_csub_sweep_ms.store(0, Relaxed);

        interval_tick(&st).await;

        let parked = mirror.next_csub_sweep_ms.load(Relaxed);
        let now = chrono::Utc::now().timestamp_millis();
        assert!(
            parked > now + 1_000,
            "an idle sweep parked until {parked} ({} ms out) — the poll is still the fast path",
            parked - now
        );
    }
}

#[cfg(test)]
mod clause_5_8_6_grouped_delivery {
    use super::*;
    use serde_json::json;
    use tower::ServiceExt as _;

    async fn post(st: &AppState, uri: &str, body: Value) -> u16 {
        let body = body.to_string();
        crate::router(st.clone())
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("Content-Type", "application/json")
                    .header("Content-Length", body.len())
                    .body(axum::body::Body::from(body))
                    .expect("request"),
            )
            .await
            .expect("response")
            .status()
            .as_u16()
    }

    async fn recording_endpoint() -> (String, Arc<std::sync::Mutex<Vec<Value>>>) {
        let seen: Arc<std::sync::Mutex<Vec<Value>>> = Arc::default();
        let sink = seen.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let app = axum::Router::new().route(
            "/notify",
            axum::routing::post(move |body: String| {
                let sink = sink.clone();
                async move {
                    if let Ok(v) = serde_json::from_str::<Value>(&body) {
                        sink.lock().expect("recorded bodies").push(v);
                    }
                    axum::http::StatusCode::OK
                }
            }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        (format!("http://{addr}/notify"), seen)
    }

    /// 5.8.6: one batch request writing N matching entities is ONE
    /// notification whose `data` carries the N entities — not N
    /// notifications — and Table 5.2.14.2-1 `timesSent` moves by one.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_batch_of_matching_entities_is_one_notification() {
        crate::allow_private();
        let (uri, seen) = recording_endpoint().await;
        let mut st = AppState::new("antares-grouped-delivery".into());
        crate::wire(&mut st).await;
        assert_eq!(
            post(
                &st,
                "/ngsi-ld/v1/subscriptions",
                json!({
                    "id": "urn:ngsi-ld:Subscription:grouped",
                    "type": "Subscription",
                    "entities": [{"type": "Vehicle"}],
                    "notification": {"endpoint": {"uri": uri, "accept": "application/json"}},
                }),
            )
            .await,
            201
        );
        let batch: Vec<Value> = (1..=3)
            .map(|i| {
                json!({"id": format!("urn:ngsi-ld:Vehicle:{i}"), "type": "Vehicle",
                       "speed": {"type": "Property", "value": i}})
            })
            .collect();
        assert_eq!(
            post(&st, "/ngsi-ld/v1/entityOperations/create", json!(batch)).await,
            201
        );
        for _ in 0..600 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            if !seen.lock().expect("bodies").is_empty() {
                break;
            }
        }
        // settle: a second POST, if the broker were still splitting, lands here
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let bodies = seen.lock().expect("bodies").clone();
        assert_eq!(bodies.len(), 1, "one POST for the batch, got {bodies:?}");
        let data = bodies[0]["data"].as_array().expect("data array");
        let mut ids: Vec<&str> = data.iter().filter_map(|e| e["id"].as_str()).collect();
        ids.sort_unstable();
        assert_eq!(
            ids,
            [
                "urn:ngsi-ld:Vehicle:1",
                "urn:ngsi-ld:Vehicle:2",
                "urn:ngsi-ld:Vehicle:3"
            ]
        );
        let tenant = TenantId::new("default").expect("tenant");
        let sub = st
            .store
            .get(
                &tenant,
                Kind::Subscription,
                "urn:ngsi-ld:Subscription:grouped",
            )
            .await
            .expect("store")
            .expect("row");
        assert_eq!(sub["notification"]["timesSent"], json!(1));
    }
}

#[cfg(test)]
mod notification_body_bound {
    use super::*;
    use serde_json::json;

    /// A grouped notification is cut into whole-entity runs under the byte
    /// cap; nothing is split mid-entity and an oversize single entity still
    /// travels (alone) instead of being dropped.
    #[test]
    fn chunks_cut_at_whole_items_under_the_cap() {
        let item = |i: usize| json!({"id": format!("urn:ngsi-ld:V:{i}"), "v": "x".repeat(40)});
        let one = serde_json::to_vec(&item(0)).expect("json").len();
        let runs = chunk_by_bytes((0..5).map(item).collect(), one * 2);
        assert_eq!(runs.iter().map(Vec::len).collect::<Vec<_>>(), [2, 2, 1]);
        assert_eq!(runs[2][0]["id"], json!("urn:ngsi-ld:V:4"));
        let runs = chunk_by_bytes(vec![item(0), item(1)], one / 2);
        assert_eq!(runs.len(), 2, "an over-cap item is its own run, never lost");
        assert!(chunk_by_bytes(Vec::new(), 1).is_empty());
    }
}

#[cfg(test)]
mod interval_claim {
    use super::*;
    use serde_json::json;

    async fn seed(st: &AppState, tenant: &TenantId) {
        st.store
            .create(
                tenant,
                Kind::Subscription,
                "urn:ngsi-ld:Subscription:tick",
                json!({
                    "id": "urn:ngsi-ld:Subscription:tick",
                    "type": "Subscription",
                    "entities": [{"type": "https://uri.etsi.org/ngsi-ld/default-context/Vehicle"}],
                    "timeInterval": 1,
                    "status": "active",
                    "createdAt": "2020-01-01T00:00:00Z",
                    "notification": {"endpoint": {"uri": "http://127.0.0.1:9/notify"}},
                }),
            )
            .await
            .expect("seed");
    }

    async fn stamp(st: &AppState, tenant: &TenantId) -> Option<String> {
        st.store
            .get(tenant, Kind::Subscription, "urn:ngsi-ld:Subscription:tick")
            .await
            .expect("store")
            .expect("row")["notification"]["lastNotification"]
            .as_str()
            .map(str::to_owned)
    }

    /// 5.8.6: "If there are no matching Entities, no Notification is sent",
    /// and Table 5.2.14.2-1 makes `lastNotification` "the timestamp
    /// corresponding to the instant when the last notification was sent".
    /// The multi-pod claim stamps that member to win the firing, so a due
    /// subscription that then matches nothing must have the stamp put back:
    /// otherwise a client reads a notification instant for a notification
    /// that never happened, and the subscription — still owing its firing —
    /// waits out a whole interval that the single-process path does not.
    #[tokio::test]
    async fn a_claimed_firing_that_matches_nothing_leaves_no_notification_instant() {
        let tenant = TenantId::new("default").expect("tenant");
        for nats in [false, true] {
            let mut st = AppState::new("me".into());
            st.nats = nats;
            seed(&st, &tenant).await;
            interval_tick(&st).await;
            assert_eq!(
                stamp(&st, &tenant).await,
                None,
                "nats={nats}: nothing matched, so nothing was sent"
            );
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod change_grouping {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc as StdArc;

    /// 5.8.6: "the Notification … data … shall contain the Entities that
    /// match" — every change of one drain that matches the same Subscription
    /// travels in ONE notification. Grouping is what makes a batch of N
    /// writes one POST with N data entries instead of N POSTs, and it has to
    /// hold when many Subscriptions match the same change: each of them gets
    /// exactly one notification carrying every entity, and no entity lands on
    /// the wrong Subscription.
    #[tokio::test(flavor = "multi_thread")]
    async fn every_change_of_a_drain_reaches_each_matching_subscription_once() {
        crate::allow_private();
        let mut st = AppState::new("antares-grouping-test".into());
        crate::wire(&mut st).await;
        let tenant = TenantId::new("default").expect("tenant");
        let posts: StdArc<AtomicUsize> = StdArc::default();
        let entities: StdArc<std::sync::Mutex<Vec<usize>>> = StdArc::default();
        let (p, e) = (posts.clone(), entities.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let app = axum::Router::new().route(
            "/notify",
            axum::routing::post(move |body: String| {
                let (p, e) = (p.clone(), e.clone());
                async move {
                    p.fetch_add(1, Ordering::SeqCst);
                    let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                    e.lock()
                        .expect("seen")
                        .push(v["data"].as_array().map(Vec::len).unwrap_or(0));
                    axum::http::StatusCode::OK
                }
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });

        const SUBS: usize = 8;
        const CHANGES: usize = 5;
        for i in 0..SUBS {
            let sub = json!({
                "id": format!("urn:ngsi-ld:Subscription:group-{i}"),
                "type": "Subscription",
                "status": "active",
                "entities": [{"type": "https://uri.etsi.org/ngsi-ld/default-context/Vehicle"}],
                "notification": {"endpoint": {"uri": format!("http://{addr}/notify")}},
            });
            st.store
                .create(
                    &tenant,
                    Kind::Subscription,
                    &format!("urn:ngsi-ld:Subscription:group-{i}"),
                    sub.clone(),
                )
                .await
                .expect("seed subscription");
            if let Some(m) = &st.sub_mirror {
                m.apply(
                    tenant.as_str(),
                    &format!("urn:ngsi-ld:Subscription:group-{i}"),
                    Some(sub),
                );
            }
        }
        let changes: Vec<Change> = (0..CHANGES)
            .map(|i| {
                (
                    tenant.as_str().to_owned(),
                    None,
                    Some(json!({
                        "id": format!("urn:ngsi-ld:Vehicle:{i}"),
                        "type": ["https://uri.etsi.org/ngsi-ld/default-context/Vehicle"],
                        "https://uri.etsi.org/ngsi-ld/default-context/speed": [
                            {"type": "Property", "value": i}
                        ],
                    })),
                )
            })
            .collect();
        process_changes(&st, changes).await;
        assert_eq!(
            posts.load(Ordering::SeqCst),
            SUBS,
            "one notification per matching subscription, never one per change"
        );
        let sizes = entities.lock().expect("seen").clone();
        assert_eq!(
            sizes,
            vec![CHANGES; SUBS],
            "each notification carries every entity of the drain"
        );
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod interval_sweep_concurrency {
    use super::*;

    /// 5.8.6 sends a periodic Notification "when the time interval … is
    /// reached". One sweep visits every tenant and every due Subscription, so
    /// whatever it does per Subscription it does 10 000 tenants' worth of —
    /// and a notification endpoint may take its whole `endpoint.timeout` to
    /// answer (Table 5.2.15-1). Awaiting each delivery in turn makes one
    /// unresponsive endpoint the deadline of every other subscriber's
    /// periodic notification, on a broker whose targets are 10 000 tenants
    /// and 100 000 subscriptions.
    #[tokio::test(flavor = "multi_thread")]
    async fn one_unresponsive_endpoint_does_not_hold_up_the_other_subscriptions() {
        crate::allow_private();
        // a listener that accepts and never answers: every delivery to it
        // costs exactly its endpoint timeout
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((s, _)) = listener.accept().await {
                held.push(s);
            }
        });
        // no mirror: the sweep reads the subscriptions from the store, which
        // is where this test seeds them
        let st = AppState::new("antares-sweep-test".into());
        let tenant = TenantId::new("default").expect("tenant");
        st.store
            .create(
                &tenant,
                Kind::Entity,
                "urn:ngsi-ld:Vehicle:sweep",
                json!({
                    "id": "urn:ngsi-ld:Vehicle:sweep",
                    "type": ["https://uri.etsi.org/ngsi-ld/default-context/Vehicle"],
                }),
            )
            .await
            .expect("seed entity");
        const SUBS: u32 = 4;
        const TIMEOUT_MS: u64 = 300;
        for i in 0..SUBS {
            let id = format!("urn:ngsi-ld:Subscription:sweep-{i}");
            st.store
                .create(
                    &tenant,
                    Kind::Subscription,
                    &id,
                    json!({
                        "id": id,
                        "type": "Subscription",
                        "status": "active",
                        "timeInterval": 1,
                        "createdAt": "2020-01-01T00:00:00Z",
                        "entities": [{
                            "type": "https://uri.etsi.org/ngsi-ld/default-context/Vehicle"
                        }],
                        "notification": {"endpoint": {
                            "uri": format!("http://{addr}/notify"),
                            "timeout": TIMEOUT_MS,
                        }},
                    }),
                )
                .await
                .expect("seed subscription");
        }
        let started = std::time::Instant::now();
        interval_tick(&st).await;
        let elapsed = started.elapsed().as_millis() as u64;
        let serial = TIMEOUT_MS * u64::from(SUBS);
        assert!(
            elapsed < serial * crate::state::slow_factor(),
            "the sweep took {elapsed} ms — {SUBS} deliveries of {TIMEOUT_MS} ms ran one after \
             the other instead of together"
        );
    }
}

/// What `stable_eq` treats as the same attribute instance (5.8.6: a change
/// notification carries the attributes that changed, and the members the
/// broker stamps itself are not a change).
#[cfg(test)]
mod stable_comparison {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_the_volatile_members_may_differ() {
        let base = json!({"type": "Property", "value": 1,
                          "createdAt": "2020-01-01T00:00:00Z",
                          "modifiedAt": "2020-01-01T00:00:00Z",
                          "instanceId": "urn:ngsi-ld:Instance:1"});
        // every volatile member differs, or is missing outright
        assert!(stable_eq(&base, &json!({"type": "Property", "value": 1})));
        assert!(stable_eq(
            &base,
            &json!({"type": "Property", "value": 1,
                    "modifiedAt": "2021-06-06T00:00:00Z",
                    "instanceId": "urn:ngsi-ld:Instance:2"})
        ));
        // a member that is not volatile
        assert!(!stable_eq(&base, &json!({"type": "Property", "value": 2})));
        assert!(!stable_eq(
            &base,
            &json!({"type": "Property", "value": 1, "unitCode": "CEL"})
        ));
        // same count of non-volatile members, different names
        assert!(!stable_eq(
            &json!({"a": 1, "b": 2}),
            &json!({"a": 1, "c": 2})
        ));
        // volatile only counts as a member name, never as a value
        assert!(!stable_eq(
            &json!({"observedAt": "createdAt"}),
            &json!({"observedAt": "modifiedAt"})
        ));
    }

    #[test]
    fn nesting_and_arrays_compare_element_by_element() {
        assert!(stable_eq(
            &json!({"value": {"a": [1, {"b": 2, "createdAt": "x"}]}}),
            &json!({"value": {"a": [1, {"b": 2}]}})
        ));
        assert!(!stable_eq(
            &json!({"value": {"a": [1, {"b": 2}]}}),
            &json!({"value": {"a": [1, {"b": 3}]}})
        ));
        // order within an array is part of the value
        assert!(!stable_eq(&json!([1, 2]), &json!([2, 1])));
        assert!(!stable_eq(&json!([1, 2]), &json!([1, 2, 3])));
        // scalars and mismatched kinds
        assert!(stable_eq(&json!(null), &json!(null)));
        assert!(!stable_eq(&json!({}), &json!([])));
        assert!(!stable_eq(&json!(1), &json!("1")));
    }
}

/// How much of the notification width one tenant may hold.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod delivery_share {
    use super::*;

    /// Counts the deliveries the sweep holds open once its first round is
    /// out. A loaded runner can take seconds before the sweep opens its
    /// first connection, so a fixed sleep read 0 (full run 40695109842):
    /// wait for the first connection, bounded, then give the rest of the
    /// round the settle time.
    async fn in_flight_after_sweep(opened: &std::sync::atomic::AtomicUsize) -> usize {
        let slow = crate::state::slow_factor();
        let first = async {
            while opened.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        };
        let wait = std::time::Duration::from_secs(slow.saturating_mul(10));
        // a timeout leaves the count at 0 for the caller's assert to name
        let _ = tokio::time::timeout(wait, first).await;
        let settle = std::time::Duration::from_millis(slow.saturating_mul(400));
        tokio::time::sleep(settle).await;
        opened.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// A Subscription belongs to one tenant (5.2.12), and a delivery to an
    /// endpoint that accepts and never answers holds its slot until the
    /// endpoint's timeout expires — up to 30 s (Table 5.2.15-1). A tenant
    /// that points more subscriptions than the whole width at such an
    /// endpoint would, with one width shared broker-wide, hold every slot
    /// for that long and stop every other tenant's notifications. A tenant
    /// therefore takes a share of the width, never all of it.
    #[tokio::test(flavor = "multi_thread")]
    async fn one_tenant_takes_a_share_of_the_delivery_width_not_all_of_it() {
        crate::allow_private();
        // accepts and never answers: each connection is held for the whole
        // endpoint timeout, so the connections open at once ARE the
        // deliveries in flight
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let opened = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&opened);
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((s, _)) = listener.accept().await {
                held.push(s);
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        });
        let st = AppState::new("antares-share-test".into());
        let tenant = TenantId::new("hog").expect("tenant");
        st.store
            .create(
                &tenant,
                Kind::Entity,
                "urn:ngsi-ld:Vehicle:share",
                json!({
                    "id": "urn:ngsi-ld:Vehicle:share",
                    "type": ["https://uri.etsi.org/ngsi-ld/default-context/Vehicle"],
                }),
            )
            .await
            .expect("seed entity");
        // more subscriptions than the whole width, so an unbounded tenant
        // would hold every slot
        let subs = *DELIVERY_WIDTH + *DELIVERY_WIDTH_PER_TENANT;
        for i in 0..subs {
            let id = format!("urn:ngsi-ld:Subscription:share-{i}");
            st.store
                .create(
                    &tenant,
                    Kind::Subscription,
                    &id,
                    json!({
                        "id": id,
                        "type": "Subscription",
                        "status": "active",
                        "timeInterval": 1,
                        "createdAt": "2020-01-01T00:00:00Z",
                        "entities": [{
                            "type": "https://uri.etsi.org/ngsi-ld/default-context/Vehicle"
                        }],
                        // long enough that nothing in the first round has
                        // timed out and freed its slot while this is counted
                        "notification": {"endpoint": {
                            "uri": format!("http://{addr}/notify"),
                            "timeout": 30_000,
                        }},
                    }),
                )
                .await
                .expect("seed subscription");
        }
        let sweep = tokio::spawn(async move { interval_tick(&st).await });
        let in_flight = in_flight_after_sweep(&opened).await;
        sweep.abort();
        assert!(in_flight > 0, "the sweep delivered nothing at all");
        assert!(
            in_flight <= *DELIVERY_WIDTH_PER_TENANT,
            "one tenant held {in_flight} of the {} delivery slots",
            *DELIVERY_WIDTH
        );
    }

    /// F-ANT3-007: the periodic Context Source notifications take the
    /// Tenant's share of the delivery width too.
    #[tokio::test(flavor = "multi_thread")]
    async fn one_tenant_takes_a_share_of_the_csource_delivery_width() {
        crate::allow_private();
        // accepts and never answers: each connection is held for the whole
        // endpoint timeout, so the connections open at once ARE the
        // deliveries in flight
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let opened = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&opened);
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((s, _)) = listener.accept().await {
                held.push(s);
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        });
        let st = AppState::new("antares-share-csub-test".into());
        let tenant = TenantId::new("hog").expect("tenant");
        // a periodic Context Source notification carries the matching
        // registrations, and one with none sends nothing
        st.store
            .create(
                &tenant,
                Kind::Registration,
                "urn:ngsi-ld:ContextSourceRegistration:share",
                json!({
                    "id": "urn:ngsi-ld:ContextSourceRegistration:share",
                    "type": "ContextSourceRegistration",
                    "information": [{"entities": [{
                        "type": "https://uri.etsi.org/ngsi-ld/default-context/Vehicle"
                    }]}],
                    "endpoint": "http://127.0.0.1:9",
                }),
            )
            .await
            .expect("seed registration");
        // more subscriptions than the whole width, so an unbounded tenant
        // would hold every slot
        let subs = *DELIVERY_WIDTH + *DELIVERY_WIDTH_PER_TENANT;
        for i in 0..subs {
            let id = format!("urn:ngsi-ld:Subscription:csub-share-{i}");
            st.store
                .create(
                    &tenant,
                    Kind::CSourceSubscription,
                    &id,
                    json!({
                        "id": id,
                        "type": "ContextSourceRegistrationSubscription",
                        "status": "active",
                        "timeInterval": 1,
                        "createdAt": "2020-01-01T00:00:00Z",
                        "entities": [{
                            "type": "https://uri.etsi.org/ngsi-ld/default-context/Vehicle"
                        }],
                        // long enough that nothing in the first round has
                        // timed out and freed its slot while this is counted
                        "notification": {"endpoint": {
                            "uri": format!("http://{addr}/notify"),
                            "timeout": 30_000,
                        }},
                    }),
                )
                .await
                .expect("seed subscription");
        }
        let sweep = tokio::spawn(async move { interval_tick(&st).await });
        let in_flight = in_flight_after_sweep(&opened).await;
        sweep.abort();
        assert!(in_flight > 0, "the sweep delivered nothing at all");
        assert!(
            in_flight <= *DELIVERY_WIDTH_PER_TENANT,
            "one tenant held {in_flight} of the {} delivery slots",
            *DELIVERY_WIDTH
        );
    }
}

/// The drain's memo of resolved `@contexts`.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod context_memo {
    use super::*;

    /// 5.5.10 scopes an `@context` resolution to one Tenant, so the drain's
    /// memo is keyed by Tenant before URL: a resolution one Tenant reached is
    /// not reachable under another Tenant naming the same URL.
    #[tokio::test]
    async fn one_tenants_memoised_context_is_not_served_to_another() {
        let st = AppState::new("antares-ctx-memo-tenant".into());
        let url = "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.9.jsonld";
        let sub = json!({"id": "urn:ngsi-ld:Subscription:1", "__context": url});
        let planted = Arc::new(Context::default());
        let mut memo = CtxMemo::new();
        memo.entry("alpha".to_owned())
            .or_default()
            .insert(url.to_owned(), Arc::clone(&planted));

        let beta = TenantId::new_internal("beta").expect("tenant");
        let got = sub_context_memo(&st, &beta, "beta", &sub, &mut memo).await;

        assert!(
            !Arc::ptr_eq(&got, &planted),
            "Tenant beta was served Tenant alpha's resolved @context"
        );
        assert!(
            memo.get("alpha").and_then(|m| m.get(url)).is_some(),
            "alpha's entry was overwritten by another Tenant's resolution"
        );
    }

    /// The memo exists to spare the resolver a lookup per candidate: a second
    /// candidate naming the same `@context` in the same drain reuses the
    /// resolution the first one reached rather than resolving again.
    #[tokio::test]
    async fn a_second_candidate_reuses_the_resolution_of_the_first() {
        let st = AppState::new("antares-ctx-memo-reuse".into());
        let url = "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.9.jsonld";
        let tenant = TenantId::new_internal("alpha").expect("tenant");
        let mut memo = CtxMemo::new();

        let first = sub_context_memo(
            &st,
            &tenant,
            "alpha",
            &json!({"id": "urn:ngsi-ld:Subscription:1", "__context": url}),
            &mut memo,
        )
        .await;
        let second = sub_context_memo(
            &st,
            &tenant,
            "alpha",
            &json!({"id": "urn:ngsi-ld:Subscription:2", "__context": url}),
            &mut memo,
        )
        .await;

        assert!(
            Arc::ptr_eq(&first, &second),
            "the second candidate resolved its @context again"
        );
    }

    /// A Subscription carrying an inline `@context` rather than a URL has no
    /// cheap identity to key on: it resolves as it did before the memo, and
    /// leaves no entry behind that a later URL lookup could collide with.
    #[tokio::test]
    async fn an_inline_context_is_resolved_rather_than_memoised() {
        let st = AppState::new("antares-ctx-memo-inline".into());
        let tenant = TenantId::new_internal("alpha").expect("tenant");
        let mut memo = CtxMemo::new();
        let sub = json!({
            "id": "urn:ngsi-ld:Subscription:1",
            "__context": {"Vehicle": "https://example.org/Vehicle"},
        });

        let ctx = sub_context_memo(&st, &tenant, "alpha", &sub, &mut memo).await;

        assert_eq!(ctx.expand_key("Vehicle"), "https://example.org/Vehicle");
        assert!(memo.is_empty(), "an inline @context left a memo entry");
    }
}

/// T-5159: the store calls of in-process deliveries are shared per tenant.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod coalesced_delivery {
    use super::coalesce::{book, Coalescer};
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    async fn subscription(st: &AppState, tenant: &TenantId, id: &str) {
        st.store
            .create(
                tenant,
                Kind::Subscription,
                id,
                json!({"id": id, "type": "Subscription",
                       "notification": {"endpoint": {"uri": "x-inproc:a"}}}),
            )
            .await
            .expect("seed subscription");
    }

    /// Two attempts on one subscription in one batch book two notifications,
    /// the second getting back the stamp the first wrote; a subscription that
    /// is gone books nothing and is told so.
    #[tokio::test]
    async fn a_batch_books_every_attempt_once() {
        let st = AppState::new("antares-coalesced-book".into());
        let tenant = TenantId::new("default").expect("tenant");
        let (a, b) = ("urn:ngsi-ld:Subscription:a", "urn:ngsi-ld:Subscription:b");
        subscription(&st, &tenant, a).await;
        subscription(&st, &tenant, b).await;
        let (t1, t2) = ("2026-01-01T00:00:01.000Z", "2026-01-01T00:00:02.000Z");
        let attempts: Vec<(String, String, i64)> = [
            (a, t1),
            (b, t1),
            (a, t2),
            ("urn:ngsi-ld:Subscription:gone", t1),
        ]
        .iter()
        .map(|(id, t)| ((*id).to_owned(), (*t).to_owned(), 1))
        .collect();
        let booked = book(&st, &tenant, &attempts).await;
        let prev: Vec<Option<Option<Value>>> = booked
            .iter()
            .map(|d| d.as_ref().map(|d| d.prev_success.clone()))
            .collect();
        assert_eq!(
            prev,
            vec![Some(None), Some(None), Some(Some(json!(t1))), None]
        );
        let times = |doc: Option<Value>| doc.map(|d| d["notification"]["timesSent"].clone());
        let stored_a = st
            .store
            .get(&tenant, Kind::Subscription, a)
            .await
            .expect("read a");
        let stored_b = st
            .store
            .get(&tenant, Kind::Subscription, b)
            .await
            .expect("read b");
        assert_eq!(times(stored_a), Some(json!(2)));
        assert_eq!(times(stored_b), Some(json!(1)));
    }

    /// Questions arriving while a tenant's statement runs are answered by
    /// ONE next statement, each with its own answer, and a question never
    /// joins the statement already running.
    #[tokio::test]
    async fn waiting_questions_share_the_next_statement() {
        static UNDER_TEST: std::sync::LazyLock<Coalescer<usize, usize>> =
            std::sync::LazyLock::new(Coalescer::new);
        let st = AppState::new("antares-coalescer".into());
        let tenant = TenantId::new("default").expect("tenant");
        let rounds: Arc<std::sync::Mutex<Vec<Vec<usize>>>> = Arc::default();
        let started = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let run = {
            let (rounds, started, gate) = (rounds.clone(), started.clone(), gate.clone());
            move |_st: AppState, _t: TenantId, qs: Vec<usize>| {
                let (rounds, started, gate) = (rounds.clone(), started.clone(), gate.clone());
                async move {
                    started.fetch_add(1, Ordering::SeqCst);
                    // each statement waits for the test to let it finish
                    gate.acquire().await.map(|p| p.forget()).ok();
                    if let Ok(mut r) = rounds.lock() {
                        r.push(qs.clone());
                    }
                    qs.iter().map(|q| q.saturating_mul(10)).collect()
                }
            }
        };
        let first = {
            let (st, tenant, run) = (st.clone(), tenant.clone(), run.clone());
            tokio::spawn(async move { UNDER_TEST.call(&st, &tenant, 0, run).await })
        };
        while started.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        let rest: Vec<_> = (1..=5)
            .map(|q| {
                let (st, tenant, run) = (st.clone(), tenant.clone(), run.clone());
                tokio::spawn(async move { UNDER_TEST.call(&st, &tenant, q, run).await })
            })
            .collect();
        // every waiting question is queued behind the held statement
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        gate.add_permits(2);
        assert_eq!(first.await.expect("first"), Some(0));
        let mut answers = Vec::new();
        for r in rest {
            answers.push(r.await.expect("waiting"));
        }
        assert_eq!(answers, (1..=5).map(|q| Some(q * 10)).collect::<Vec<_>>());
        let rounds = rounds.lock().map(|r| r.clone()).unwrap_or_default();
        assert_eq!(rounds, vec![vec![0], vec![1, 2, 3, 4, 5]]);
    }

    /// A caller dropped while its own statement runs (its delivery's
    /// deadline) hands the questions queued behind it to a runner: none of
    /// them is lost with it.
    #[tokio::test]
    async fn a_dropped_first_caller_hands_the_queue_on() {
        static UNDER_TEST: std::sync::LazyLock<Coalescer<usize, usize>> =
            std::sync::LazyLock::new(Coalescer::new);
        let st = AppState::new("antares-coalescer-drop".into());
        let tenant = TenantId::new("default").expect("tenant");
        let started = Arc::new(AtomicUsize::new(0));
        let run = {
            let started = started.clone();
            move |_st: AppState, _t: TenantId, qs: Vec<usize>| {
                let started = started.clone();
                async move {
                    // the first statement never finishes on its own
                    if started.fetch_add(1, Ordering::SeqCst) == 0 {
                        std::future::pending::<()>().await;
                    }
                    qs.iter().map(|q| q.saturating_mul(10)).collect::<Vec<_>>()
                }
            }
        };
        let first = {
            let (st, tenant, run) = (st.clone(), tenant.clone(), run.clone());
            tokio::spawn(async move { UNDER_TEST.call(&st, &tenant, 0, run).await })
        };
        while started.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        let rest: Vec<_> = (1..=3)
            .map(|q| {
                let (st, tenant, run) = (st.clone(), tenant.clone(), run.clone());
                tokio::spawn(async move { UNDER_TEST.call(&st, &tenant, q, run).await })
            })
            .collect();
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        first.abort();
        let mut answers = Vec::new();
        for r in rest {
            let answer = tokio::time::timeout(std::time::Duration::from_secs(5), r).await;
            answers.push(answer.ok().and_then(Result::ok).flatten());
        }
        assert_eq!(answers, vec![Some(10), Some(20), Some(30)]);
    }

    /// A question of another store (a second broker in the process, same
    /// tenant) never joins a statement running on the first one's store: it
    /// runs its own, on its own store (T-5245).
    #[tokio::test]
    async fn another_store_never_joins_the_statement() {
        static UNDER_TEST: std::sync::LazyLock<Coalescer<usize, usize>> =
            std::sync::LazyLock::new(Coalescer::new);
        let a = AppState::new("antares-coalescer-a".into());
        let b = AppState::new("antares-coalescer-b".into());
        let store = |st: &AppState| Arc::as_ptr(&st.store).cast::<()>().addr();
        let (store_a, store_b) = (store(&a), store(&b));
        let tenant = TenantId::new("default").expect("tenant");
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let run = {
            let gate = gate.clone();
            move |st: AppState, _t: TenantId, qs: Vec<usize>| {
                let gate = gate.clone();
                async move {
                    // a's statement waits for the test to let it finish
                    if store(&st) == store_a {
                        gate.acquire().await.map(|p| p.forget()).ok();
                    }
                    vec![store(&st); qs.len()]
                }
            }
        };
        let first = {
            let (st, tenant, run) = (a.clone(), tenant.clone(), run.clone());
            tokio::spawn(async move { UNDER_TEST.call(&st, &tenant, 0, run).await })
        };
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        let other = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            UNDER_TEST.call(&b, &tenant, 1, run.clone()),
        )
        .await;
        gate.add_permits(1);
        assert_eq!(other.ok().flatten(), Some(store_b), "b ran on b's store");
        assert_eq!(first.await.ok().flatten(), Some(store_a));
    }

    /// Subscriptions that differ only in their endpoint and bookkeeping get
    /// one payload; a different shape or @context gets its own.
    #[test]
    fn equal_shapes_share_one_payload() {
        let loader = antares_jsonld::Loader::new();
        let ctx = loader.core();
        let sub = |uri: &str, extra: Value| {
            let mut s = json!({"id": "urn:x", "notification": {
                "endpoint": {"uri": uri}, "format": "keyValues", "timesSent": 3}});
            if let (Some(n), Value::Object(extra)) = (s["notification"].as_object_mut(), extra) {
                n.extend(extra);
            }
            s
        };
        let base = payload_key(&sub("x-inproc:a", json!({})), &ctx, &[], false);
        let other_socket = payload_key(
            &sub(
                "x-inproc:b",
                json!({"timesSent": 9, "lastNotification": "t"}),
            ),
            &ctx,
            &[],
            false,
        );
        assert_eq!(base, other_socket);
        let picked = payload_key(
            &sub("x-inproc:a", json!({"attributes": ["speed"]})),
            &ctx,
            &[],
            false,
        );
        assert_ne!(base, picked);
        let deleted = payload_key(
            &sub("x-inproc:a", json!({})),
            &ctx,
            &["speed".into()],
            false,
        );
        assert_ne!(base, deleted);
        let own_ctx = Arc::new((*ctx).clone());
        assert_ne!(
            base,
            payload_key(&sub("x-inproc:a", json!({})), &own_ctx, &[], false)
        );
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod csource_fanout_share {
    use super::*;

    /// F-ANT3-006: a tenant whose fan-outs hang holds at most its share of
    /// the fan-out slots, so another tenant's registration write still gets
    /// one at once.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_hung_tenant_leaves_slots_for_another() {
        let a = TenantId::new("fanshare-a").expect("tenant");
        let b = TenantId::new("fanshare-b").expect("tenant");
        let mut held = Vec::new();
        for _ in 0..CSOURCE_FANOUT_PER_TENANT {
            held.push(fanout_permits(&a).await);
        }
        let wait = std::time::Duration::from_millis(300);
        assert!(
            tokio::time::timeout(wait, fanout_permits(&a))
                .await
                .is_err(),
            "tenant a is at its share"
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(5), fanout_permits(&b))
                .await
                .is_ok(),
            "tenant b must not wait behind tenant a"
        );
        drop(held);
    }

    /// F-ANT3-019: the CSource notification paths read every registration a
    /// page at a time, past the whole-list ceiling that made them read
    /// nothing on Postgres.
    #[cfg(feature = "postgres")]
    #[tokio::test(flavor = "multi_thread")]
    async fn the_registration_read_passes_the_list_ceiling() {
        let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
            eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
            return;
        };
        let pool = antares_sql::store::pg::connect(&url, 5)
            .await
            .expect("connect");
        let tenant = TenantId::new("csrceiling").expect("tenant");
        antares_sql::store::pg::ensure_tenant(&pool, &tenant)
            .await
            .expect("tenant row");
        let st = AppState::with_store(
            "antares-csr-ceiling".into(),
            Arc::new(antares_sql::store::any::AnyStore::Pg(
                antares_sql::store::any::PgBackend::new(pool),
            )),
            "postgres",
        );
        let n = usize::try_from(antares_sql::store::pg::entity::MAX_UNDECIDED_ROWS)
            .expect("n")
            .saturating_add(1);
        for i in 0..n {
            let id = format!("urn:ngsi-ld:ContextSourceRegistration:c{i:06}");
            let doc = serde_json::json!({"id": id, "type": "ContextSourceRegistration",
                "endpoint": "http://localhost:1/", "keep": i % 2 == 0});
            let _ = st.store.create(&tenant, Kind::Registration, &id, doc).await;
        }
        let kept = docs_where(&st, &tenant, Kind::Registration, |d| d["keep"] == true)
            .await
            .expect("a paged read is never refused for volume");
        assert_eq!(kept.len(), n.div_ceil(2));
    }
}

/// T-6371: an in-process delivery leaves before its stamp, and its window
/// books what a reader of 5.2.14.2 must see.
mod booking_window {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[derive(Default)]
    struct Socket {
        sent: AtomicUsize,
        refuses: AtomicBool,
    }

    /// A binding that opens no socket, like the WebSocket one.
    struct InProcess(Arc<Socket>);

    impl antares_notifier::NotificationSink for InProcess {
        fn schemes(&self) -> &'static [&'static str] {
            &["x-window"]
        }
        fn parse_endpoint(
            &self,
            _uri: &str,
            _notifier_info: &[(&str, &str)],
        ) -> Result<(), antares_model::NgsiError> {
            Ok(())
        }
        fn network(&self) -> bool {
            false
        }
        fn deliver<'a>(
            &'a self,
            _uri: &'a str,
            _out: &'a antares_notifier::Outbound,
            _timeout: std::time::Duration,
        ) -> antares_notifier::DeliveryFuture<'a> {
            let refused = self.0.refuses.load(Ordering::SeqCst);
            self.0
                .sent
                .fetch_add(usize::from(!refused), Ordering::SeqCst);
            Box::pin(async move {
                if refused {
                    Err(antares_notifier::DeliveryError::failed("queue overflowed"))
                } else {
                    Ok(())
                }
            })
        }
    }

    const ID: &str = "urn:ngsi-ld:Subscription:window";

    fn window_state(alias: &str) -> (AppState, Arc<Socket>) {
        let socket = Arc::new(Socket::default());
        let mut st = AppState::new(alias.into()).with_sink(Box::new(InProcess(socket.clone())));
        st.sub_mirror = Some(Arc::default());
        (st, socket)
    }

    async fn subscribed(st: &AppState, tenant: &TenantId, extra: Value) -> Value {
        let mut sub = json!({
            "id": ID,
            "type": "Subscription",
            "status": "active",
            "createdAt": "2020-01-01T00:00:00Z",
            "notification": {"endpoint": {"uri": "x-window:1"}},
        });
        if let (Some(s), Some(e)) = (sub.as_object_mut(), extra.as_object()) {
            s.extend(e.clone());
        }
        st.store
            .create(tenant, Kind::Subscription, ID, sub.clone())
            .await
            .expect("subscription row");
        if let Some(m) = &st.sub_mirror {
            m.apply(tenant.as_str(), ID, Some(sub.clone()));
        }
        sub
    }

    async fn send(st: &AppState, tenant: &TenantId, sub: &Value) {
        let ctx = st.loader.core();
        deliver(
            st,
            tenant,
            sub,
            vec![json!({"id": "urn:ngsi-ld:V:1"})],
            &ctx,
        )
        .await;
    }

    async fn stored(st: &AppState, tenant: &TenantId) -> Value {
        st.store
            .get(tenant, Kind::Subscription, ID)
            .await
            .expect("read")
            .expect("subscription")["notification"]
            .clone()
    }

    fn tenant() -> TenantId {
        TenantId::new("default").expect("tenant")
    }

    #[tokio::test]
    async fn a_window_books_its_notifications_once_after_they_left() {
        let (st, socket) = window_state("antares-window-merge");
        let tenant = tenant();
        let sub = subscribed(&st, &tenant, json!({})).await;
        for _ in 0..3 {
            send(&st, &tenant, &sub).await;
        }
        assert_eq!(socket.sent.load(Ordering::SeqCst), 3, "none waited");
        assert_eq!(stored(&st, &tenant).await.get("timesSent"), None);
        settle_bookings(&st, &tenant).await;
        let booked = stored(&st, &tenant).await;
        assert_eq!(booked["timesSent"], json!(3));
        assert_eq!(booked["status"], json!("ok"));
        assert!(booked["lastNotification"].is_string());
        let mirrored = st
            .sub_mirror
            .as_ref()
            .expect("mirror")
            .docs(tenant.as_str());
        assert_eq!(mirrored[0]["notification"]["timesSent"], json!(3));
    }

    #[tokio::test]
    async fn the_window_writes_itself_and_holds_the_shutdown_drain_until_it_did() {
        let (st, _socket) = window_state("antares-window-timer");
        let tenant = tenant();
        let sub = subscribed(&st, &tenant, json!({})).await;
        send(&st, &tenant, &sub).await;
        assert_eq!(
            st.pending_changes.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while st.pending_changes.load(std::sync::atomic::Ordering::SeqCst) > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the window closes");
        assert_eq!(stored(&st, &tenant).await["timesSent"], json!(1));
    }

    /// Who holds a notification reads a `timesSent` that counts it.
    #[tokio::test]
    async fn retrieve_subscription_reads_what_already_left() {
        let (st, _socket) = window_state("antares-window-read");
        let tenant = tenant();
        let sub = subscribed(&st, &tenant, json!({})).await;
        send(&st, &tenant, &sub).await;
        let answer = st
            .call(
                &axum::http::HeaderMap::new(),
                axum::http::Request::builder()
                    .uri(format!("/ngsi-ld/v1/subscriptions/{ID}"))
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await;
        let body = axum::body::to_bytes(answer.into_body(), usize::MAX)
            .await
            .expect("body");
        let doc: Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(doc["notification"]["timesSent"], json!(1), "{doc}");
    }

    /// The failure lands on top of the window: a later statement of the
    /// window cannot turn `failed` back to `ok`, and `lastSuccess` falls back
    /// to the last notification that did leave.
    #[tokio::test]
    async fn a_failed_attempt_lands_on_top_of_its_window() {
        let (st, socket) = window_state("antares-window-failure");
        let tenant = tenant();
        let sub = subscribed(&st, &tenant, json!({})).await;
        send(&st, &tenant, &sub).await;
        send(&st, &tenant, &sub).await;
        socket.refuses.store(true, Ordering::SeqCst);
        send(&st, &tenant, &sub).await;
        let booked = stored(&st, &tenant).await;
        assert_eq!(
            booked["timesSent"],
            json!(3),
            "the attempt counts: {booked}"
        );
        assert_eq!(booked["timesFailed"], json!(1));
        assert_eq!(booked["status"], json!("failed"));
        assert!(booked["lastSuccess"].is_string(), "{booked}");
        settle_bookings(&st, &tenant).await;
        assert_eq!(stored(&st, &tenant).await["status"], json!("failed"));
    }

    /// `throttling` reads `lastNotification`, so its stamp cannot wait.
    #[tokio::test]
    async fn a_throttled_subscription_is_booked_before_it_leaves() {
        let (st, socket) = window_state("antares-window-throttled");
        let tenant = tenant();
        let sub = subscribed(&st, &tenant, json!({"throttling": 60})).await;
        send(&st, &tenant, &sub).await;
        assert_eq!(socket.sent.load(Ordering::SeqCst), 1);
        assert_eq!(stored(&st, &tenant).await["timesSent"], json!(1));
        let mirrored = st
            .sub_mirror
            .as_ref()
            .expect("mirror")
            .docs(tenant.as_str());
        assert!(matches!(mirrored.first(), Some(d) if throttled(d)));
    }

    /// 5.8.6: nothing is sent for a Subscription the broker no longer holds.
    #[tokio::test]
    async fn a_deleted_subscription_is_not_notified_and_its_window_books_nothing() {
        let (st, socket) = window_state("antares-window-deleted");
        let tenant = tenant();
        let sub = subscribed(&st, &tenant, json!({})).await;
        send(&st, &tenant, &sub).await;
        st.store
            .delete(&tenant, Kind::Subscription, ID)
            .await
            .expect("delete");
        st.sub_mirror
            .as_ref()
            .expect("mirror")
            .apply(tenant.as_str(), ID, None);
        send(&st, &tenant, &sub).await;
        assert_eq!(socket.sent.load(Ordering::SeqCst), 1);
        settle_bookings(&st, &tenant).await;
        assert!(
            st.sub_mirror
                .as_ref()
                .expect("mirror")
                .docs(tenant.as_str())
                .is_empty(),
            "a window never brings a deleted Subscription back"
        );
    }

    #[tokio::test]
    async fn a_window_is_its_tenants_alone() {
        let (st, _socket) = window_state("antares-window-tenants");
        let (a, b) = (tenant(), TenantId::new("window-b").expect("tenant"));
        let (sub_a, sub_b) = (
            subscribed(&st, &a, json!({})).await,
            subscribed(&st, &b, json!({})).await,
        );
        send(&st, &a, &sub_a).await;
        send(&st, &a, &sub_a).await;
        send(&st, &b, &sub_b).await;
        settle_bookings(&st, &a).await;
        assert_eq!(stored(&st, &a).await["timesSent"], json!(2));
        assert_eq!(stored(&st, &b).await.get("timesSent"), None);
        settle_bookings(&st, &b).await;
        assert_eq!(stored(&st, &b).await["timesSent"], json!(1));
    }
}
