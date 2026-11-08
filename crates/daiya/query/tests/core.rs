#![forbid(unsafe_code)]
//! The shared driver types: errors map to the platform's problem kinds without secret values,
//! secrets resolve per organization from the map or the secrets mount, token exchange refuses by
//! default, and the Loki slot keeps targets while refusing to run them.

mod common;

use common::{ORG, ctx, q};
use daiya_model::{CrError, InstanceId, OrgId, Subject};
use daiya_query::{
    Datasource, DenyTokenExchanger, DsError, FileSecretResolver, Health, HealthState,
    LokiDatasource, MapSecretResolver, PassthroughAudienceExchanger, Query, QueryCtx, ResourceReq,
    SecretResolver, TokenExchanger,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;

#[test]
fn driver_errors_map_to_problem_kinds_without_the_secret() {
    let cases: Vec<(DsError, fn(&CrError) -> bool)> = vec![
        (
            DsError::InvalidQuery {
                ref_id: "A".into(),
                reason: "bad".into(),
            },
            |e| matches!(e, CrError::BadRequest(m) if m == "query [A]: bad"),
        ),
        (
            DsError::GuardRefused("no".into()),
            |e| matches!(e, CrError::SsrfBlocked(m) if m == "no"),
        ),
        (
            DsError::Upstream {
                status: 503,
                message: "down".into(),
            },
            |e| matches!(e, CrError::OutboundFailed(m) if m == "upstream status 503: down"),
        ),
        (
            DsError::SecretResolution {
                secret_ref: "ref".into(),
                reason: "value=hunter2".into(),
            },
            |e| matches!(e, CrError::OutboundFailed(m) if m == "failed to resolve secretRef 'ref'"),
        ),
        (
            DsError::TokenExchange {
                audience: "aud".into(),
                reason: "no".into(),
            },
            |e| matches!(e, CrError::Forbidden(m) if m.contains("'aud'")),
        ),
        (
            DsError::Wasm("trap".into()),
            |e| matches!(e, CrError::WasmError(m) if m == "trap"),
        ),
        (
            DsError::NotImplemented("x".into()),
            |e| matches!(e, CrError::NotImplemented(m) if m == "x"),
        ),
    ];
    for (err, check) in cases {
        let text = err.to_string();
        let mapped = CrError::from(err);
        assert!(check(&mapped), "{text} -> {mapped:?}");
        assert!(!format!("{mapped:?}").contains("hunter2"));
    }
}

#[test]
fn map_secrets_are_per_organization() {
    let mut m = MapSecretResolver::new();
    m.insert(ORG, "token", "v1");
    assert_eq!(m.resolve_secret(ORG, "token").unwrap().as_str(), "v1");
    let err = m.resolve_secret(OrgId::from_u128(99), "token").unwrap_err();
    assert!(
        matches!(err, DsError::SecretResolution { ref secret_ref, .. } if secret_ref == "token")
    );
}

#[test]
fn file_secrets_are_one_name_in_the_organization_directory() {
    let dir = std::env::temp_dir().join(format!("daiya-secrets-{}", uuid::Uuid::now_v7()));
    let own = dir.join(ORG.to_string());
    let other = dir.join(OrgId::from_u128(99).to_string());
    std::fs::create_dir_all(&own).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(own.join("db.password"), "s3cret\r\n").unwrap();
    std::fs::write(own.join("empty"), "\n").unwrap();
    std::fs::write(other.join("theirs"), "nope").unwrap();
    std::fs::write(dir.join("top-level"), "nope").unwrap();
    let r = FileSecretResolver::new(&dir);

    assert_eq!(
        r.resolve_secret(ORG, "db.password").unwrap().as_str(),
        "s3cret"
    );
    let refused = |name: &str| match r.resolve_secret(ORG, name) {
        Err(DsError::SecretResolution { reason, .. }) => reason,
        other => panic!("{name}: {other:?}"),
    };
    assert_eq!(refused("empty"), "secret is empty");
    assert!(refused("missing").contains("not found"));
    assert!(
        refused("theirs").contains("not found"),
        "another organization's secret"
    );
    for bad in [
        "",
        ".hidden",
        "..",
        "../top-level",
        "a/b",
        "a b",
        &format!("../{}/theirs", OrgId::from_u128(99)),
        &"x".repeat(201),
    ] {
        assert!(refused(bad).contains("one name"), "{bad}");
    }
    assert!(
        r.resolve_secret(ORG, &"y".repeat(200)).is_err(),
        "long but valid name, absent"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn token_exchange_refuses_by_default() {
    let err = DenyTokenExchanger
        .exchange_for_audience("jwt", "aud")
        .unwrap_err();
    assert!(matches!(err, DsError::TokenExchange { ref audience, .. } if audience == "aud"));
    let p = PassthroughAudienceExchanger;
    assert_eq!(
        p.exchange_for_audience("jwt", "aud").unwrap().as_str(),
        "exchanged:aud:jwt"
    );
    assert!(p.exchange_for_audience("  ", "aud").is_err());
}

#[test]
fn context_query_and_health_values() {
    let c = QueryCtx::new(ORG, InstanceId::from_u128(1), Subject::anonymous(), 1, 2);
    assert_eq!((c.max_data_points, c.interval_ms), (1000, 15_000));
    assert_eq!(c.timeout, Duration::from_secs(10));
    assert!(c.variables.is_empty());
    let parsed: Query = serde_json::from_value(json!({ "refId": "A", "model": {} })).unwrap();
    assert!(!parsed.hide);
    assert_eq!(Health::ok("x").status, HealthState::Ok);
    assert_eq!(Health::error("y").status, HealthState::Error);
    assert_eq!(
        serde_json::to_value(Health::error("y")).unwrap(),
        json!({ "status": "error", "message": "y" })
    );
}

#[tokio::test]
async fn loki_is_a_slot_that_keeps_targets() {
    let l = LokiDatasource::new("logs");
    assert_eq!(l.kind(), "loki");
    assert_eq!(l.uid, "logs");
    let c = ctx(Subject::anonymous());
    assert!(matches!(
        l.query(&c, &q(json!({ "expr": "{job=\"a\"}" }))).await,
        Err(DsError::NotImplemented(_))
    ));
    assert_eq!(l.health(&c).await.status, HealthState::Error);
    assert!(matches!(
        l.resource(
            &c,
            ResourceReq {
                path: "labels".into(),
                params: BTreeMap::new()
            }
        )
        .await,
        Err(DsError::NotImplemented(_))
    ));
    let target = json!({ "refId": "L", "expr": "{job=\"a\"}", "maxLines": 10, "hide": true });
    let imported = l.import(&target).unwrap();
    assert_eq!(imported.warnings.len(), 1);
    assert!(imported.query.hide);
    assert_eq!(l.export(&imported.query), target);
    let plain = l.import(&json!({ "expr": "x" })).unwrap();
    assert_eq!(l.export(&plain.query), json!({ "expr": "x", "refId": "A" }));
    assert_eq!(
        l.export(&Query {
            ref_id: "Z".into(),
            hide: false,
            model: json!([])
        }),
        json!({ "refId": "Z" })
    );
    assert!(l.import(&json!(1)).is_err());
    assert_eq!(l.editor().json_schema["required"], json!(["expr"]));
}
