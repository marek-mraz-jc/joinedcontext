#![forbid(unsafe_code)]
//! Authorizer decisions (R9), Problem Details / OData error envelopes (R10a, R12), the
//! `FrameSet` model (§3.2), identifiers, limits and quotas.

use daiya_model::{
    Action, Authorizer, ColumnValues, CrError, Decision, FieldColumn, FieldType, Frame, FrameMeta,
    FrameSet, GrafanaRole, InputLimits, InstallationId, InstanceId, Notice, NoticeSeverity,
    ObjectTarget, OrgId, OrgQuotas, PolicyAuthorizer, ProblemDetails, RequestCtx, RoleBinding,
    ShareType, Subject,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const ORG: OrgId = OrgId::from_u128(1);
const OTHER: OrgId = OrgId::from_u128(2);

fn ctx(subject: Subject) -> RequestCtx {
    RequestCtx::new(
        InstallationId::from_u128(9),
        ORG,
        InstanceId::from_u128(3),
        subject,
    )
}

fn dash(uid: &str, folder: Option<&str>) -> ObjectTarget {
    ObjectTarget::Dashboard {
        org_id: ORG,
        dashboard_uid: uid.into(),
        folder_uid: folder.map(str::to_owned),
    }
}

fn ds(uid: &str) -> ObjectTarget {
    ObjectTarget::Datasource {
        org_id: ORG,
        datasource_uid: uid.into(),
    }
}

#[test]
fn grafana_roles_map_onto_action_sets() {
    assert_eq!(GrafanaRole::Viewer.mapped_actions(), vec![Action::Read]);
    assert_eq!(
        GrafanaRole::Editor.mapped_actions(),
        vec![Action::Read, Action::Propose]
    );
    assert_eq!(
        GrafanaRole::Admin.mapped_actions(),
        vec![
            Action::Read,
            Action::Propose,
            Action::Approve,
            Action::Delete,
            Action::Admin
        ]
    );
}

#[test]
fn actions_parse_grafana_and_daiya_verbs_and_round_trip() {
    for a in [
        Action::Read,
        Action::Propose,
        Action::Approve,
        Action::Delete,
        Action::Admin,
    ] {
        assert_eq!(Action::parse(a.as_str()), Some(a));
        assert_eq!(a.is_write(), a != Action::Read);
    }
    assert_eq!(Action::parse("VIEW"), Some(Action::Read));
    assert_eq!(Action::parse("edit"), Some(Action::Propose));
    assert_eq!(Action::parse("create"), Some(Action::Propose));
    assert_eq!(Action::parse("update"), Some(Action::Propose));
    assert_eq!(Action::parse("apply"), Some(Action::Approve));
    assert_eq!(Action::parse("drop table"), None);
    assert_eq!(serde_json::to_value(Action::Approve).unwrap(), "approve");
}

#[test]
fn subjects_expose_principals_and_membership() {
    let anon = Subject::anonymous();
    assert!(anon.is_anonymous());
    assert!(anon.principals().is_empty());
    assert!(!anon.is_member_of(ORG));
    assert_eq!(ctx(anon).actor_name(), "anonymous");

    let s = Subject::authenticated("alice", vec![ORG], vec!["/ops".into(), "/sre".into()])
        .with_bearer_token("tok");
    assert!(!s.is_anonymous());
    assert!(s.is_member_of(ORG));
    assert!(!s.is_member_of(OTHER));
    assert_eq!(s.principals(), vec!["alice", "/ops", "/sre"]);
    assert_eq!(s.bearer_token.as_deref(), Some("tok"));
    // The raw bearer token never leaves the process in serialized form.
    let json = serde_json::to_value(&s).unwrap();
    assert!(json.get("bearerToken").is_none(), "{json}");
    assert_eq!(ctx(s.clone()).actor_name(), "alice");
    let back: Subject = serde_json::from_value(json).unwrap();
    assert_eq!(back.bearer_token, None);
    assert_eq!(back.groups, s.groups);

    let a = ctx(Subject::anonymous());
    let b = ctx(Subject::anonymous());
    assert_ne!(a.request_id, b.request_id, "each request gets its own id");
}

#[test]
fn object_targets_report_their_org_and_folder() {
    let targets = [
        (ObjectTarget::Organization { org_id: ORG }, None),
        (
            ObjectTarget::Folder {
                org_id: ORG,
                folder_uid: "f".into(),
            },
            Some("f"),
        ),
        (dash("d", Some("f2")), Some("f2")),
        (dash("d", None), None),
        (ds("p"), None),
        (
            ObjectTarget::LibraryPanel {
                org_id: ORG,
                panel_uid: "l".into(),
                folder_uid: Some("lf".into()),
            },
            Some("lf"),
        ),
        (
            ObjectTarget::PublicDashboard {
                org_id: ORG,
                dashboard_uid: "d".into(),
            },
            None,
        ),
    ];
    for (t, folder) in targets {
        assert_eq!(t.org_id(), ORG);
        assert_eq!(t.folder_uid(), folder, "{t:?}");
        let round: ObjectTarget =
            serde_json::from_value(serde_json::to_value(&t).unwrap()).unwrap();
        assert_eq!(round, t);
    }
}

#[test]
fn decisions_map_hidden_denials_to_404_and_visible_ones_to_403() {
    assert_eq!(Decision::allow("ok").into_result("nf"), Ok(()));
    assert_eq!(
        Decision::deny_hidden("other org").into_result("Dashboard not found"),
        Err(CrError::NotFound("Dashboard not found".into()))
    );
    assert_eq!(
        Decision::deny_visible("read-only").into_result("nf"),
        Err(CrError::Forbidden("read-only".into()))
    );
}

#[test]
fn policy_authorizer_hides_other_organizations_and_non_members() {
    let authz = PolicyAuthorizer::allow_org_members(ORG, "alice");
    let alice = ctx(Subject::authenticated("alice", vec![ORG], vec![]));
    assert!(authz.check(&alice, Action::Admin, &dash("d", None)).allowed);

    let foreign = ObjectTarget::Dashboard {
        org_id: OTHER,
        dashboard_uid: "d".into(),
        folder_uid: None,
    };
    let d = authz.check(&alice, Action::Read, &foreign);
    assert!(!d.allowed && !d.visible);

    // Same principal name, but not a member of the org: hidden as well.
    let outsider = ctx(Subject::authenticated("alice", vec![OTHER], vec![]));
    let d = authz.check(&outsider, Action::Read, &dash("d", None));
    assert!(!d.allowed && !d.visible);
}

#[test]
fn policy_authorizer_applies_roles_folders_datasources_and_read_only() {
    let authz = PolicyAuthorizer::new(vec![
        RoleBinding::from_grafana_role(ORG, "viewer", GrafanaRole::Viewer, None),
        RoleBinding::from_grafana_role(ORG, "/editors", GrafanaRole::Editor, Some("ops".into())),
        RoleBinding {
            org_id: ORG,
            principal: "approver".into(),
            actions: vec![Action::Read, Action::Approve],
            folder_uid: None,
            datasource_uid: None,
        },
        RoleBinding {
            org_id: ORG,
            principal: "dsuser".into(),
            actions: vec![Action::Read],
            folder_uid: None,
            datasource_uid: Some("prom".into()),
        },
        RoleBinding {
            org_id: ORG,
            principal: "proposer-no-read".into(),
            actions: vec![Action::Propose],
            folder_uid: None,
            datasource_uid: None,
        },
        // A binding of another organization never applies here.
        RoleBinding::from_grafana_role(OTHER, "viewer", GrafanaRole::Admin, None),
    ]);

    let viewer = ctx(Subject::authenticated("viewer", vec![ORG], vec![]));
    assert!(authz.check(&viewer, Action::Read, &dash("d", None)).allowed);
    let d = authz.check(&viewer, Action::Propose, &dash("d", None));
    assert!(!d.allowed && d.visible);
    assert_eq!(d.reason, "missing propose permission");

    // Group binding scoped to folder "ops".
    let editor = ctx(Subject::authenticated(
        "bob",
        vec![ORG],
        vec!["/editors".into()],
    ));
    let ok = authz.check(&editor, Action::Propose, &dash("d", Some("ops")));
    assert!(ok.allowed);
    assert_eq!(ok.reason, "granted propose");
    let other_folder = authz.check(&editor, Action::Read, &dash("d", Some("finance")));
    assert!(!other_folder.allowed && !other_folder.visible);
    let no_folder = authz.check(&editor, Action::Read, &dash("d", None));
    assert!(!no_folder.allowed && !no_folder.visible);
    assert!(
        authz
            .check(
                &editor,
                Action::Read,
                &ObjectTarget::Folder {
                    org_id: ORG,
                    folder_uid: "ops".into()
                }
            )
            .allowed
    );

    // Approve implies propose.
    let approver = ctx(Subject::authenticated("approver", vec![ORG], vec![]));
    assert!(
        authz
            .check(&approver, Action::Propose, &dash("d", None))
            .allowed
    );
    assert!(
        authz
            .check(&approver, Action::Approve, &dash("d", None))
            .allowed
    );
    assert!(
        !authz
            .check(&approver, Action::Delete, &dash("d", None))
            .allowed
    );

    // Datasource-scoped binding.
    let dsuser = ctx(Subject::authenticated("dsuser", vec![ORG], vec![]));
    assert!(authz.check(&dsuser, Action::Read, &ds("prom")).allowed);
    assert!(!authz.check(&dsuser, Action::Read, &ds("loki")).allowed);
    assert!(!authz.check(&dsuser, Action::Read, &dash("d", None)).allowed);

    // Write rights without read rights reveal nothing.
    let blind = ctx(Subject::authenticated(
        "proposer-no-read",
        vec![ORG],
        vec![],
    ));
    let d = authz.check(&blind, Action::Propose, &dash("d", None));
    assert!(!d.allowed && !d.visible);

    // A read-only connector cannot write, whatever its bindings.
    let mut ro = Subject::authenticated("approver", vec![ORG], vec![]);
    ro.read_only = true;
    let d = authz.check(&ctx(ro.clone()), Action::Propose, &dash("d", None));
    assert!(!d.allowed && d.visible);
    assert_eq!(d.reason, "caller is read-only");
    assert!(
        authz
            .check(&ctx(ro), Action::Read, &dash("d", None))
            .allowed
    );

    // The binding set round-trips through its JSON form.
    let json = serde_json::to_string(&authz).unwrap();
    let back: PolicyAuthorizer = serde_json::from_str(&json).unwrap();
    assert_eq!(back.bindings, authz.bindings);
    assert!(PolicyAuthorizer::default().bindings.is_empty());
}

#[test]
fn every_error_has_slug_status_and_title_and_hides_internal_detail() {
    let cases: Vec<(CrError, &str, u16, &str)> = vec![
        (
            CrError::NotFound("x".into()),
            "cr.not_found",
            404,
            "Not Found",
        ),
        (
            CrError::Unauthorized("x".into()),
            "cr.unauthorized",
            401,
            "Unauthorized",
        ),
        (
            CrError::Forbidden("x".into()),
            "cr.forbidden",
            403,
            "Forbidden",
        ),
        (
            CrError::BadRequest("x".into()),
            "cr.bad_request",
            400,
            "Bad Request",
        ),
        (
            CrError::Unprocessable {
                detail: "x".into(),
                errors: vec!["/a".into()],
            },
            "cr.unprocessable_entity",
            422,
            "Unprocessable Entity",
        ),
        (
            CrError::Conflict("x".into()),
            "cr.conflict",
            409,
            "Conflict",
        ),
        (
            CrError::PreconditionFailed("x".into()),
            "cr.precondition_failed",
            412,
            "Precondition Failed",
        ),
        (
            CrError::StalePlan("x".into()),
            "cr.stale_plan",
            409,
            "Stale Plan",
        ),
        (
            CrError::QuotaExceeded {
                resource: "dashboards".into(),
                limit: 5,
            },
            "cr.quota_exceeded",
            403,
            "Quota Exceeded",
        ),
        (
            CrError::RateLimited("x".into()),
            "cr.rate_limited",
            429,
            "Too Many Requests",
        ),
        (
            CrError::SsrfBlocked("x".into()),
            "cr.ssrf_blocked",
            403,
            "Outbound Request Blocked",
        ),
        (
            CrError::OutboundFailed("x".into()),
            "cr.outbound_failed",
            502,
            "Bad Gateway",
        ),
        (
            CrError::MigrationFailed("x".into()),
            "cr.migration_failed",
            400,
            "Schema Migration Failed",
        ),
        (
            CrError::ExpressionError("x".into()),
            "cr.expression_error",
            400,
            "Expression Error",
        ),
        (
            CrError::WasmError("x".into()),
            "cr.wasm_error",
            400,
            "Plugin Execution Error",
        ),
        (
            CrError::RlsNotEnforced("table t".into()),
            "cr.rls_not_enforced",
            500,
            "Tenant Isolation Check Failed",
        ),
        (
            CrError::NotImplemented("x".into()),
            "cr.not_implemented",
            501,
            "Not Implemented",
        ),
        (
            CrError::Internal("password=hunter2".into()),
            "cr.internal_error",
            500,
            "Internal Server Error",
        ),
    ];
    for (err, slug, status, title) in cases {
        assert_eq!(err.slug(), slug);
        assert_eq!(err.status_code(), status, "{slug}");
        assert_eq!(err.title(), title, "{slug}");
        let problem = err.to_problem(Some("/api/x".into()));
        assert_eq!(
            problem.type_uri,
            format!("urn:joinedcontext:problem:{slug}")
        );
        assert_eq!(problem.status, status);
        assert_eq!(problem.instance.as_deref(), Some("/api/x"));
        if status == 500 {
            assert_eq!(problem.detail, "an internal server error occurred");
            assert!(!serde_json::to_string(&problem).unwrap().contains("hunter2"));
            assert!(!err.to_odata_error().to_string().contains("table t"));
        } else {
            assert_eq!(problem.detail, err.to_string());
        }
    }
    assert_eq!(
        CrError::QuotaExceeded {
            resource: "dashboards".into(),
            limit: 5
        }
        .to_string(),
        "quota exceeded for dashboards: limit is 5"
    );
}

#[test]
fn unprocessable_errors_carry_their_pointers_into_both_envelopes() {
    let err = CrError::Unprocessable {
        detail: "bad dashboard".into(),
        errors: vec!["/title: required".into(), "/uid: invalid".into()],
    };
    let problem = err.to_problem(None);
    assert_eq!(problem.errors.len(), 2);
    let json = serde_json::to_value(&problem).unwrap();
    assert_eq!(
        json["type"],
        "urn:joinedcontext:problem:cr.unprocessable_entity"
    );
    assert!(json.get("instance").is_none());
    let back: ProblemDetails = serde_json::from_value(json).unwrap();
    assert_eq!(back, problem);

    let odata = err.to_odata_error();
    assert_eq!(odata["error"]["code"], "cr.unprocessable_entity");
    assert_eq!(odata["error"]["message"], "bad dashboard");
    assert_eq!(odata["error"]["status"], 422);
    assert_eq!(
        odata["error"]["details"],
        json!([
            {"code": "cr.unprocessable_entity", "message": "/title: required"},
            {"code": "cr.unprocessable_entity", "message": "/uid: invalid"}
        ])
    );
    // Other errors have no details and no `errors` member.
    let plain = CrError::NotFound("gone".into());
    assert_eq!(plain.to_odata_error()["error"]["details"], json!([]));
    assert!(
        serde_json::to_value(plain.to_problem(None))
            .unwrap()
            .get("errors")
            .is_none()
    );
}

#[test]
fn column_values_read_every_type_as_json_f64_and_i64() {
    let time = ColumnValues::Time(vec![Some(1_700_000_000_000), None]);
    let num = ColumnValues::Number(vec![Some(2.5), None, Some(f64::NAN)]);
    let s = ColumnValues::String(vec![Some("42".into()), Some("x".into()), None]);
    let b = ColumnValues::Bool(vec![Some(true), Some(false), None]);
    let j = ColumnValues::Json(vec![json!(7), json!("s")]);

    assert_eq!(time.len(), 2);
    assert_eq!(num.len(), 3);
    assert_eq!(s.len(), 3);
    assert_eq!(b.len(), 3);
    assert_eq!(j.len(), 2);
    assert!(!j.is_empty());
    assert!(ColumnValues::Json(vec![]).is_empty());

    assert_eq!(time.get_json(0), json!(1_700_000_000_000_i64));
    assert_eq!(time.get_json(1), Value::Null);
    assert_eq!(num.get_json(0), json!(2.5));
    assert_eq!(num.get_json(2), Value::Null, "NaN is not JSON");
    assert_eq!(num.get_json(9), Value::Null);
    assert_eq!(s.get_json(0), json!("42"));
    assert_eq!(s.get_json(2), Value::Null);
    assert_eq!(b.get_json(0), json!(true));
    assert_eq!(b.get_json(2), Value::Null);
    assert_eq!(j.get_json(1), json!("s"));
    assert_eq!(j.get_json(5), Value::Null);

    assert_eq!(num.get_f64(0), Some(2.5));
    assert_eq!(num.get_f64(1), None);
    assert_eq!(time.get_f64(0), Some(1_700_000_000_000.0));
    assert_eq!(b.get_f64(0), Some(1.0));
    assert_eq!(b.get_f64(1), Some(0.0));
    assert_eq!(b.get_f64(2), None);
    assert_eq!(j.get_f64(0), Some(7.0));
    assert_eq!(j.get_f64(1), None);
    assert_eq!(s.get_f64(0), Some(42.0));
    assert_eq!(s.get_f64(1), None);
    assert_eq!(s.get_f64(2), None);

    assert_eq!(time.get_i64(0), Some(1_700_000_000_000));
    assert_eq!(num.get_i64(0), Some(2));
    assert_eq!(j.get_i64(0), Some(7));
    assert_eq!(j.get_i64(1), None);
    assert_eq!(s.get_i64(0), Some(42));
    assert_eq!(s.get_i64(1), None);
    assert_eq!(b.get_i64(0), Some(1));
    assert_eq!(b.get_i64(1), Some(0));
    assert_eq!(b.get_i64(7), None);
}

#[test]
fn field_columns_build_and_read_values_like_grafana_frames() {
    let t = FieldColumn::time("Time", vec![1000, 2000]);
    assert_eq!(t.field_type, FieldType::Time);
    assert_eq!(t.len(), 2);
    assert!(!t.is_empty());
    assert_eq!(t.time_ms_at(1), Some(2000));
    assert_eq!(t.time_ms_at(5), None);

    let mut labels = BTreeMap::new();
    labels.insert("job".to_owned(), "api".to_owned());
    let n = FieldColumn::numbers(
        "Value",
        labels.clone(),
        vec![Some(1.5), None, Some(f64::INFINITY)],
    );
    assert_eq!(n.field_type, FieldType::Number);
    assert_eq!(n.values, vec![json!(1.5), Value::Null, Value::Null]);
    assert_eq!(n.f64_at(0), Some(1.5));
    assert_eq!(n.f64_at(1), None);
    assert_eq!(n.f64_at(10), None);
    assert_eq!(n.effective_name(), "Value");

    let s = FieldColumn::strings("name", vec![Some("a".into()), None]);
    assert_eq!(s.field_type, FieldType::String);
    assert_eq!(s.values, vec![json!("a"), Value::Null]);
    assert!(FieldColumn::strings("e", vec![]).is_empty());

    // Special float strings and booleans as Prometheus and Grafana encode them.
    let special = FieldColumn {
        name: "v".into(),
        field_type: FieldType::Number,
        unit: Some("ms".into()),
        display_name: Some("Latency".into()),
        labels,
        values: vec![
            json!("NaN"),
            json!("+Inf"),
            json!("Inf"),
            json!("-Inf"),
            json!(true),
            json!(false),
            json!("abc"),
        ],
    };
    assert!(special.f64_at(0).unwrap().is_nan());
    assert_eq!(special.f64_at(1), Some(f64::INFINITY));
    assert_eq!(special.f64_at(2), Some(f64::INFINITY));
    assert_eq!(special.f64_at(3), Some(f64::NEG_INFINITY));
    assert_eq!(special.f64_at(4), Some(1.0));
    assert_eq!(special.f64_at(5), Some(0.0));
    assert_eq!(special.f64_at(6), None);
    assert_eq!(special.effective_name(), "Latency");

    // Time values as integers, floats or RFC 3339 strings.
    let times = FieldColumn {
        name: "t".into(),
        field_type: FieldType::Time,
        unit: None,
        display_name: None,
        labels: BTreeMap::new(),
        values: vec![
            json!(5),
            json!(1500.7),
            json!("2023-11-14T22:13:20Z"),
            json!("yesterday"),
            json!(true),
        ],
    };
    assert_eq!(times.time_ms_at(0), Some(5));
    assert_eq!(times.time_ms_at(1), Some(1500));
    assert_eq!(times.time_ms_at(2), Some(1_700_000_000_000));
    assert_eq!(times.time_ms_at(3), None);
    assert_eq!(times.time_ms_at(4), None);

    // JSON shape: `type`, camelCase, empty optionals omitted.
    let json = serde_json::to_value(&special).unwrap();
    assert_eq!(json["type"], "number");
    assert_eq!(json["displayName"], "Latency");
    assert_eq!(json["labels"]["job"], "api");
    let plain = serde_json::to_value(FieldColumn::time("Time", vec![])).unwrap();
    assert_eq!(plain, json!({"name": "Time", "type": "time", "values": []}));
}

#[test]
fn public_sanitizing_strips_query_text_and_custom_meta() {
    let mut custom = BTreeMap::new();
    custom.insert("executedSql".to_owned(), json!("SELECT secret"));
    let frame = Frame {
        name: "A".into(),
        ref_id: Some("A".into()),
        fields: vec![FieldColumn::time("Time", vec![1, 2, 3])],
        meta: FrameMeta {
            executed_query_string: Some("up{token=\"x\"}".into()),
            notices: vec![Notice {
                severity: NoticeSeverity::Warning,
                text: "partial".into(),
            }],
            preferred_visualisation_type: Some("graph".into()),
            custom,
        },
    };
    assert_eq!(frame.row_count(), 3);
    assert_eq!(Frame::default().row_count(), 0);

    let mut hidden = FrameSet::new("A", vec![frame.clone()]);
    hidden.sanitize_for_public(false);
    assert_eq!(hidden.frames[0].meta.executed_query_string, None);
    assert!(hidden.frames[0].meta.custom.is_empty());
    assert_eq!(hidden.frames[0].meta.notices.len(), 1);

    let mut shown = FrameSet::new("A", vec![frame]);
    shown.sanitize_for_public(true);
    assert!(shown.frames[0].meta.executed_query_string.is_some());
    assert!(shown.frames[0].meta.custom.is_empty());

    let json = serde_json::to_value(&shown).unwrap();
    assert_eq!(json["refId"], "A");
    assert_eq!(
        json["frames"][0]["meta"]["notices"][0]["severity"],
        "warning"
    );
    assert_eq!(
        json["frames"][0]["meta"]["preferredVisualisationType"],
        "graph"
    );
    let back: FrameSet = serde_json::from_value(json).unwrap();
    assert_eq!(back, shown);
    // A minimal frame deserializes with defaults.
    let minimal: Frame = serde_json::from_value(json!({})).unwrap();
    assert_eq!(minimal, Frame::default());
}

#[test]
fn identifiers_limits_quotas_and_share_types() {
    let id = InstanceId::from_u128(0x1234);
    assert_eq!(id.as_uuid(), uuid::Uuid::from_u128(0x1234));
    assert_eq!(InstanceId::from(id.as_uuid()), id);
    assert_eq!(
        "00000000-0000-0000-0000-000000001234"
            .parse::<InstanceId>()
            .unwrap(),
        id
    );
    assert!("not-a-uuid".parse::<InstallationId>().is_err());
    assert_ne!(InstallationId::default(), InstallationId::default());
    assert_eq!(
        serde_json::to_value(OrgId::from_u128(1)).unwrap(),
        json!("00000000-0000-0000-0000-000000000001")
    );
    assert_eq!(
        InstallationId::from_u128(5).to_string(),
        "00000000-0000-0000-0000-000000000005"
    );
    let a = InstallationId::new();
    let b: InstallationId = a.as_uuid().into();
    assert_eq!(a, b);
    assert_eq!(OrgId::from(a.as_uuid()).as_uuid(), a.as_uuid());

    let limits = InputLimits::default();
    assert_eq!(limits.max_dashboard_json_bytes, 4 * 1024 * 1024);
    assert_eq!(limits.max_folder_depth, 4);
    let q = OrgQuotas::default();
    assert_eq!(q.max_public_range_seconds, 31 * 86_400);
    assert_eq!(q.min_public_interval_ms, 15_000);
    assert_eq!(q.max_dashboards, 500);
    let json = serde_json::to_value(&q).unwrap();
    assert_eq!(json["maxQueriesPerMinute"], 1200);
    let back: OrgQuotas = serde_json::from_value(json).unwrap();
    assert_eq!(back, q);

    assert_eq!(ShareType::default(), ShareType::Public);
    assert_eq!(ShareType::Public.as_str(), "public");
    assert_eq!(ShareType::Email.as_str(), "email");
    assert_eq!(serde_json::to_value(ShareType::Email).unwrap(), "email");
}

#[test]
fn public_dashboard_subjects_read_only_their_dashboard_and_saved_datasources() {
    use daiya_model::{PUBLIC_DASHBOARD_ACTOR_PREFIX, PublicDashboardAuthorizer};
    let subject = Subject::public_dashboard(ORG, "pub");
    assert!(subject.read_only);
    assert_eq!(subject.organizations, vec![ORG]);
    assert!(subject.groups.is_empty() && subject.bearer_token.is_none());
    assert_eq!(
        subject.actor_id.as_deref(),
        Some(format!("{PUBLIC_DASHBOARD_ACTOR_PREFIX}pub").as_str())
    );

    let authz = PublicDashboardAuthorizer::new(ORG, "pub", ["prom".to_owned()]);
    let c = ctx(subject);
    assert!(authz.check(&c, Action::Read, &dash("pub", None)).allowed);
    assert!(
        authz
            .check(
                &c,
                Action::Read,
                &ObjectTarget::PublicDashboard {
                    org_id: ORG,
                    dashboard_uid: "pub".into()
                }
            )
            .allowed
    );
    assert!(authz.check(&c, Action::Read, &ds("prom")).allowed);
    for (action, object) in [
        (Action::Propose, dash("pub", None)),
        (Action::Read, dash("other", None)),
        (Action::Read, ds("loki")),
        (Action::Read, ObjectTarget::Organization { org_id: ORG }),
        (
            Action::Read,
            ObjectTarget::Folder {
                org_id: ORG,
                folder_uid: "f".into(),
            },
        ),
        (
            Action::Read,
            ObjectTarget::LibraryPanel {
                org_id: ORG,
                panel_uid: "l".into(),
                folder_uid: None,
            },
        ),
        (
            Action::Read,
            ObjectTarget::Datasource {
                org_id: OTHER,
                datasource_uid: "prom".into(),
            },
        ),
    ] {
        let d = authz.check(&c, action, &object);
        assert!(!d.allowed && !d.visible, "{action:?} {object:?}");
    }
    // Another dashboard's public subject, a human, or the right subject in another org.
    for subject in [
        Subject::public_dashboard(ORG, "private"),
        Subject::authenticated("alice", vec![ORG], vec![]),
    ] {
        assert!(
            !authz
                .check(&ctx(subject), Action::Read, &ds("prom"))
                .allowed
        );
    }
    let foreign = RequestCtx::new(
        InstallationId::from_u128(9),
        OTHER,
        InstanceId::from_u128(3),
        Subject::public_dashboard(OTHER, "pub"),
    );
    assert!(
        !authz
            .check(&foreign, Action::Read, &dash("pub", None))
            .allowed
    );
}

#[test]
fn scoped_write_bindings_only_grant_writes_inside_their_scope() {
    let authz = PolicyAuthorizer::new(vec![
        RoleBinding::from_grafana_role(ORG, "carol", GrafanaRole::Viewer, None),
        RoleBinding {
            org_id: ORG,
            principal: "carol".into(),
            actions: vec![Action::Delete],
            folder_uid: Some("ops".into()),
            datasource_uid: None,
        },
        RoleBinding {
            org_id: ORG,
            principal: "carol".into(),
            actions: vec![Action::Admin],
            folder_uid: None,
            datasource_uid: Some("prom".into()),
        },
    ]);
    let carol = ctx(Subject::authenticated("carol", vec![ORG], vec![]));
    assert!(
        authz
            .check(&carol, Action::Delete, &dash("d", Some("ops")))
            .allowed
    );
    let d = authz.check(&carol, Action::Delete, &dash("d", None));
    assert!(!d.allowed && d.visible);
    assert!(
        !authz
            .check(&carol, Action::Delete, &dash("d", Some("fin")))
            .allowed
    );
    assert!(authz.check(&carol, Action::Admin, &ds("prom")).allowed);
    assert!(!authz.check(&carol, Action::Admin, &ds("loki")).allowed);
}
