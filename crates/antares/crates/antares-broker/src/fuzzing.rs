// SPDX-License-Identifier: EUPL-1.2
//! The broker's parsers of outside input that the fuzz targets under `fuzz/`
//! drive (T-7829). Built only with the `fuzzing` feature, which no broker
//! build turns on: each entry point hands one input to the parser the broker
//! hands it to and answers whether it was accepted. A panic or a failed
//! invariant is the finding; a refusal is the parser doing its job.

/// One subscription-mirror delta as a peer broker writes it to the NATS KV
/// bucket: any bytes decode or are dropped, and a row it accepts names a
/// tenant inside the grammar the mirror's lookups use.
pub fn mirror_delta(bytes: &[u8]) -> bool {
    let Ok(delta) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return false;
    };
    match crate::wiring::decode_delta(&delta) {
        None => false,
        Some(crate::wiring::Delta::Csub) => true,
        Some(crate::wiring::Delta::Row { tenant, id, .. }) => {
            assert!(
                antares_model::TenantId::new_internal(tenant).is_ok(),
                "a delta outside the tenant grammar reached the mirror: {tenant:?}"
            );
            assert_eq!(delta.get("id").and_then(|v| v.as_str()), Some(id));
            true
        }
    }
}

/// One operator-supplied configuration value through every parser that
/// reads one at startup: none may panic, and each answer keeps its contract
/// (a port is never 0 nor the API port, a cadence never 0, a userinfo never
/// survives the log redaction). Whether any parser accepted it.
pub fn config_value(text: &str) -> bool {
    let mut accepted = false;
    let _ = crate::unknown_config_key(text);
    let _ = crate::is_off(text);
    if let Ok(secs) = crate::parse_sweep_secs(Some(text)) {
        assert!(secs > 0, "a zero sweep cadence was accepted from {text:?}");
        accepted = true;
    }
    match crate::parse_metrics_port(Some(text), 9090) {
        Ok(Some(port)) => {
            assert!(port != 0 && port != 9090, "{text:?} gave port {port}");
            accepted = true;
        }
        Ok(None) => assert!(text.is_empty(), "{text:?} read as no metrics port"),
        Err(_) => {}
    }
    if let Ok(from) = crate::parse_subject_from(text) {
        assert!(!from.is_empty() && from.iter().all(|s| !s.is_empty() && s.trim() == s));
        accepted = true;
    }
    accepted |= crate::wiring::Roles::parse(text).is_ok();
    let redacted = crate::telemetry::redact_url(text);
    if let Some((_, rest)) = text.split_once("://") {
        let authority = rest.split('/').next().unwrap_or_default();
        if let Some((userinfo, _)) = authority.rsplit_once('@') {
            let kept = redacted.split_once("://").map_or("", |(_, r)| r);
            let kept = kept.split('/').next().unwrap_or_default();
            assert!(
                !kept.contains('@'),
                "userinfo {userinfo:?} survived the redaction of {text:?}: {redacted:?}"
            );
        }
    } else {
        assert_eq!(redacted, text, "a URL without an authority was rewritten");
    }
    let _ = crate::telemetry::logs_endpoint(text);
    let _ = crate::telemetry::no_userinfo(format!("invalid URI {text}"), text);
    accepted
}
