#![forbid(unsafe_code)]
//! The operation registry (C11, F1-F3): every operation through `execute_operation`, with the
//! authorization decision each caller gets, the activity rows writes leave (C13), and `allowed`
//! answering exactly what the call would do without revealing hidden records.

use dataroom_core::ops::{
    all_operations, execute_operation, find_operation, DatasetValidator, HarvestRunner, OpContext,
};
use dataroom_core::store::BoxFuture;
use dataroom_core::{
    Action, Catalogue, CatalogueId, DataroomError, Dataset, GrantRule, HarvestRun, HarvestSource,
    OrgId, PolicyAuthorizer, Publisher, RecordState, RequestCtx, Result, ShaclValidationReport,
    Store, Subject,
};
use dataroom_store_memory::MemoryStore;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

struct World {
    store: Arc<MemoryStore>,
    authz: Arc<PolicyAuthorizer>,
    org: OrgId,
    cat: CatalogueId,
}

impl World {
    fn ctx(&self, subject: Subject) -> RequestCtx {
        RequestCtx::new(self.org, self.cat, subject)
    }
    fn admin(&self) -> RequestCtx {
        self.ctx(Subject::authenticated("admin", vec![self.org], Vec::new()))
    }
    fn member(&self) -> RequestCtx {
        self.ctx(Subject::authenticated("member", vec![self.org], Vec::new()))
    }
    fn dept_editor(&self) -> RequestCtx {
        self.ctx(Subject::authenticated(
            "dept-editor",
            vec![self.org],
            Vec::new(),
        ))
    }
    fn auditor(&self) -> RequestCtx {
        self.ctx(Subject::authenticated(
            "aud",
            vec![self.org],
            vec!["auditors".into()],
        ))
    }
    fn outsider(&self) -> RequestCtx {
        let other = OrgId::from_uuid(Uuid::now_v7());
        self.ctx(Subject::authenticated("outsider", vec![other], Vec::new()))
    }
    fn anonymous(&self) -> RequestCtx {
        RequestCtx::anonymous(self.org, self.cat)
    }
    fn ops(&self) -> OpContext {
        OpContext {
            store: self.store.clone(),
            authorizer: self.authz.clone(),
            validator: None,
            harvester: None,
            sources_by_portal: false,
        }
    }
    async fn run(&self, ctx: &RequestCtx, op: &str, input: Value) -> Result<Value> {
        execute_operation(&self.ops(), ctx, op, input).await
    }
    async fn activity_ops(&self) -> Vec<String> {
        self.store
            .list_activity(&self.admin(), None, 1000)
            .await
            .unwrap()
            .into_iter()
            .map(|a| a.operation)
            .collect()
    }
}

fn publisher(name: &str, editors: &[&str]) -> Publisher {
    serde_json::from_value(json!({
        "orgId": Uuid::nil(), "catalogueId": Uuid::nil(), "id": "", "name": name,
        "title": {"sk": format!("Vydavateľ {name}")}, "description": {},
        "uri": format!("https://example.org/{name}"), "state": "active",
        "editorGrants": editors,
        "created": "2026-11-01T10:00:00Z", "modified": "2026-11-01T10:00:00Z"
    }))
    .unwrap()
}

fn dataset(name: &str, publisher: &str, visibility: &str, extra: Value) -> Dataset {
    let mut v = json!({
        "orgId": Uuid::nil(), "catalogueId": Uuid::nil(), "id": "", "name": name,
        "title": {"sk": format!("Dataset {name}")}, "description": {"sk": "ovzdušie mesto"},
        "publisherId": publisher, "visibility": visibility, "state": "active",
        "themes": ["http://publications.europa.eu/resource/authority/data-theme/ENVI"],
        "licenceId": "cc-by", "tags": ["ovzdusie"],
        "distributions": [{"id": "", "datasetId": "", "title": {"sk": "CSV"},
            "accessUrl": format!("https://files.example.org/{name}.csv"), "format": "CSV"}],
        "issued": "2026-11-01T10:00:00Z", "modified": "2026-11-02T10:00:00Z"
    });
    if let (Some(obj), Some(more)) = (v.as_object_mut(), extra.as_object()) {
        for (k, val) in more {
            obj.insert(k.clone(), val.clone());
        }
    }
    serde_json::from_value(v).unwrap()
}

async fn world() -> World {
    let org = OrgId::from_uuid(Uuid::now_v7());
    let cat = CatalogueId::from_uuid(Uuid::now_v7());
    let store = Arc::new(MemoryStore::new());
    let catalogue: Catalogue = serde_json::from_value(json!({
        "orgId": org, "id": cat, "name": "city", "host": "data.city.example",
        "title": {"sk": "Mesto"}, "description": {}, "defaultLanguage": "sk",
        "languages": ["sk", "en"], "mainProfile": "dcat-ap-3.0.1", "activeProfiles": [],
        "modified": "2026-11-01T10:00:00Z"
    }))
    .unwrap();
    store.upsert_catalogue(catalogue).await.unwrap();
    let authz = Arc::new(
        PolicyAuthorizer::new(vec![GrantRule {
            org_id: org,
            catalogue_id: Some(cat),
            principal: "admin".into(),
            actions: vec![Action::Admin],
            publisher_id: None,
        }])
        .with_rule(GrantRule {
            org_id: org,
            catalogue_id: Some(cat),
            principal: "dept-editor".into(),
            actions: vec![Action::Propose, Action::Delete],
            publisher_id: Some("dept".into()),
        }),
    );
    let w = World {
        store,
        authz,
        org,
        cat,
    };
    let admin = w.admin();
    w.store
        .upsert_publisher(&admin, publisher("city", &[]))
        .await
        .unwrap();
    w.store
        .upsert_publisher(&admin, publisher("dept", &[]))
        .await
        .unwrap();
    w.store
        .upsert_publisher(&admin, publisher("gone", &[]))
        .await
        .unwrap();
    w.store.withdraw_publisher(&admin, "gone").await.unwrap();
    for ds in [
        dataset("open", "city", "public", json!({})),
        dataset(
            "dept-air",
            "dept",
            "public",
            json!({"themes": ["TRAN"], "licenceId": "cc-zero",
                   "distributions": [{"id": "", "datasetId": "", "title": {"sk": "JSON"},
                     "accessUrl": "https://files.example.org/d.json", "format": "JSON"}]}),
        ),
        dataset("internal", "city", "internal", json!({})),
        dataset(
            "restricted",
            "city",
            "restricted",
            json!({"restrictedGrants": ["auditors"]}),
        ),
        dataset("orphaned", "gone", "public", json!({})),
        dataset("retired", "city", "public", json!({"state": "withdrawn"})),
    ] {
        w.store.upsert_dataset(&admin, ds).await.unwrap();
    }
    w
}

fn names(v: &Value, key: &str) -> Vec<String> {
    let mut out: Vec<String> = v[key]
        .as_array()
        .unwrap_or_else(|| panic!("{key} in {v}"))
        .iter()
        .map(|d| d["name"].as_str().unwrap_or_default().to_owned())
        .collect();
    out.sort();
    out
}

fn is_not_found(r: &Result<Value>) -> bool {
    matches!(r, Err(DataroomError::NotFound(_)))
}

fn is_forbidden(r: &Result<Value>) -> bool {
    matches!(r, Err(DataroomError::Forbidden(_)))
}

fn field(r: &Result<Value>, name: &str) -> bool {
    matches!(r, Err(DataroomError::Validation { fields, .. }) if fields.contains_key(name))
}

#[tokio::test]
async fn registry_lookup_by_name_tool_and_command() {
    let ops = all_operations();
    assert_eq!(ops.len(), 25);
    for op in &ops {
        for key in [op.name, op.mcp_tool, op.cli_command] {
            assert_eq!(find_operation(key).map(|d| d.name), Some(op.name), "{key}");
        }
        assert!(op.input_schema.is_object(), "{}", op.name);
        assert!(op.output_schema.is_object(), "{}", op.name);
    }
    assert!(find_operation("nope").is_none());
    let w = world().await;
    let r = w.run(&w.admin(), "nope", json!({})).await;
    assert!(is_not_found(&r), "{r:?}");
}

#[tokio::test]
async fn catalogue_operations() {
    let w = world().await;
    let listed = w
        .run(&w.anonymous(), "catalogue.list", json!({}))
        .await
        .unwrap();
    assert_eq!(listed["catalogues"].as_array().unwrap().len(), 1);
    let got = w
        .run(&w.anonymous(), "jc_catalogue_get", json!({}))
        .await
        .unwrap();
    assert_eq!(got["name"], "city");

    let mut cat = got.clone();
    cat["title"] = json!({"sk": "Nové mesto"});
    // The tenant always comes from the request, never from the body.
    cat["orgId"] = json!(Uuid::now_v7());
    let r = w
        .run(
            &w.member(),
            "catalogue.propose",
            json!({"catalogue": cat.clone()}),
        )
        .await;
    assert!(is_forbidden(&r), "{r:?}");
    let r = w
        .run(
            &w.anonymous(),
            "catalogue.propose",
            json!({"catalogue": cat.clone()}),
        )
        .await;
    assert!(is_forbidden(&r), "{r:?}");
    let saved = w
        .run(&w.admin(), "catalogue.propose", json!({"catalogue": cat}))
        .await
        .unwrap();
    assert_eq!(saved["orgId"], json!(w.org));
    assert_eq!(saved["title"]["sk"], "Nové mesto");
    assert!(w.activity_ops().await.contains(&"catalogue.propose".into()));
    let r = w
        .run(
            &w.admin(),
            "catalogue.propose",
            json!({"catalogue": {"name": 3}}),
        )
        .await;
    assert!(field(&r, "catalogue"), "{r:?}");

    // A catalogue that does not exist answers not found.
    let lost = RequestCtx::new(
        w.org,
        CatalogueId::from_uuid(Uuid::now_v7()),
        Subject::anonymous(),
    );
    assert!(is_not_found(
        &w.run(&lost, "catalogue.get", json!({})).await
    ));
    assert!(is_not_found(&w.run(&lost, "branding.get", json!({})).await));
    assert!(is_not_found(
        &w.run(&lost, "catalogue.feed_check", json!({})).await
    ));
}

#[tokio::test]
async fn dataset_search_shows_each_caller_what_it_may_read() {
    let w = world().await;
    let anon = w
        .run(&w.anonymous(), "dataset.search", json!({}))
        .await
        .unwrap();
    assert_eq!(names(&anon, "datasets"), ["dept-air", "open"]);
    assert_eq!(anon["total"], 2);
    let admin = w
        .run(&w.admin(), "dataset.search", json!({}))
        .await
        .unwrap();
    assert_eq!(
        names(&admin, "datasets"),
        ["dept-air", "internal", "open", "restricted"]
    );
    let aud = w
        .run(&w.auditor(), "dataset.search", json!({}))
        .await
        .unwrap();
    assert_eq!(names(&aud, "datasets"), ["dept-air", "open", "restricted"]);
    let outsider = w
        .run(&w.outsider(), "dataset.search", json!({}))
        .await
        .unwrap();
    assert_eq!(names(&outsider, "datasets"), ["dept-air", "open"]);

    for (input, want) in [
        (json!({"q": "OPEN"}), vec!["open"]),
        (json!({"q": "dataset ovzdušie"}), vec!["dept-air", "open"]),
        (json!({"q": "nothing-like-this"}), vec![]),
        (json!({"publisher": "dept"}), vec!["dept-air"]),
        (json!({"publisher": "pub-city"}), vec!["open"]),
        (json!({"theme": "TRAN"}), vec!["dept-air"]),
        (json!({"theme": "envi"}), vec!["open"]),
        (json!({"format": "json"}), vec!["dept-air"]),
        (json!({"licence": "cc-by"}), vec!["open"]),
    ] {
        let got = w
            .run(&w.anonymous(), "jc_dataset_search", input.clone())
            .await
            .unwrap();
        assert_eq!(names(&got, "datasets"), want, "{input}");
    }
    let page2 = w
        .run(&w.anonymous(), "dataset.search", json!({"page": 2}))
        .await
        .unwrap();
    assert_eq!(page2["page"], 2);
    assert!(page2["datasets"].as_array().unwrap().is_empty());
    assert_eq!(page2["total"], 2);
}

#[tokio::test]
async fn dataset_get_hides_what_the_caller_may_not_read() {
    let w = world().await;
    let r = w.run(&w.anonymous(), "dataset.get", json!({})).await;
    assert!(field(&r, "name"), "{r:?}");
    assert!(is_not_found(
        &w.run(&w.anonymous(), "dataset.get", json!({"name": "none"}))
            .await
    ));
    let open = w
        .run(&w.anonymous(), "dataset.get", json!({"id": "pkg-open"}))
        .await
        .unwrap();
    assert_eq!(open["name"], "open");
    for name in ["internal", "restricted"] {
        let r = w
            .run(&w.anonymous(), "dataset.get", json!({"name": name}))
            .await;
        assert!(is_not_found(&r), "{name}: {r:?}");
        let r = w
            .run(&w.member(), "dataset.get", json!({"name": name}))
            .await;
        assert!(is_not_found(&r), "member without read on {name}: {r:?}");
    }
    assert_eq!(
        w.run(&w.auditor(), "dataset.get", json!({"name": "restricted"}))
            .await
            .unwrap()["name"],
        "restricted"
    );
    // Withdrawn datasets and datasets of a withdrawn publisher left the public catalogue: an
    // anonymous caller gets what package_show gives (not found); an editor still sees them.
    for name in ["retired", "orphaned"] {
        let r = w
            .run(&w.anonymous(), "dataset.get", json!({"name": name}))
            .await;
        assert!(is_not_found(&r), "anonymous {name}: {r:?}");
        let r = w
            .run(&w.admin(), "dataset.get", json!({"name": name}))
            .await;
        assert!(r.is_ok(), "admin {name}: {r:?}");
    }
    // A resource with views says so, as CKAN's has_views does.
    let admin = w.admin();
    w.store
        .upsert_resource_view(
            &admin,
            serde_json::from_value(json!({"id": "v1", "resourceId": "res-open-0",
                "viewType": "jc_visualization", "title": "Graf"}))
            .unwrap(),
        )
        .await
        .unwrap();
    let open = w
        .run(&w.anonymous(), "dataset.get", json!({"name": "open"}))
        .await
        .unwrap();
    assert_eq!(open["resources"][0]["has_views"], true, "{open}");

    // A dataset hidden by the publish gate (jc_hide) is shown to its editors only.
    let mut cat = w.store.get_catalogue(w.org, w.cat).await.unwrap().unwrap();
    cat.hidden_datasets
        .insert("open".into(), "quality check pending".into());
    w.store.upsert_catalogue(cat).await.unwrap();
    for ctx in [w.anonymous(), w.member()] {
        let r = w.run(&ctx, "dataset.get", json!({"name": "open"})).await;
        assert!(is_not_found(&r), "{r:?}");
    }
    assert!(w
        .run(&w.admin(), "dataset.get", json!({"name": "open"}))
        .await
        .is_ok());
}

struct Validator {
    conforms: bool,
}

impl DatasetValidator for Validator {
    fn validate_dataset(
        &self,
        catalogue: &Catalogue,
        dataset: &Dataset,
        _publisher: Option<&Publisher>,
    ) -> Result<Vec<ShaclValidationReport>> {
        Ok(vec![serde_json::from_value(json!({
            "orgId": catalogue.org_id, "catalogueId": catalogue.id, "id": "",
            "datasetId": dataset.id, "profileId": catalogue.main_profile,
            "profileVersion": "3.0.1", "conforms": self.conforms,
            "violationCount": if self.conforms { 0 } else { 1 }, "warningCount": 0,
            "findings": if self.conforms { json!([]) } else { json!([{
                "focusNode": "_:d", "resultPath": "http://purl.org/dc/terms/description",
                "sourceConstraintComponent": "sh:MinCountConstraintComponent",
                "sourceShape": "_:s", "severity": "Violation",
                "message": {"en": "description is required"}
            }]) },
            "checkedAt": "2026-11-01T10:00:00Z"
        }))
        .unwrap()])
    }

    fn check_catalogue_feed(
        &self,
        _catalogue: &Catalogue,
        datasets: &[Dataset],
        _publishers: &BTreeMap<String, Publisher>,
        profile_id: Option<&str>,
    ) -> Result<Value> {
        Ok(json!({"checked": datasets.len(), "profile": profile_id}))
    }
}

#[tokio::test]
async fn dataset_propose_authorizes_validates_and_records() {
    let w = world().await;
    let ckan = json!({"name": "new-ckan", "title": "Nový", "owner_org": "city", "private": true});
    let r = w.run(&w.member(), "dataset.propose", ckan.clone()).await;
    assert!(is_forbidden(&r), "member without a grant: {r:?}");
    let r = w.run(&w.outsider(), "dataset.propose", ckan.clone()).await;
    assert!(is_forbidden(&r), "outsider: {r:?}");
    let created = w
        .run(&w.admin(), "dataset.propose", json!({"dataset": ckan}))
        .await
        .unwrap();
    assert_eq!(created["name"], "new-ckan");
    assert!(w.activity_ops().await.contains(&"dataset.propose".into()));

    // The native shape (publisherId) and the publisher canonicalised to its name.
    let mut native =
        serde_json::to_value(dataset("native", "pub-dept", "internal", json!({}))).unwrap();
    native["orgId"] = json!(Uuid::now_v7());
    let saved = w
        .run(
            &w.dept_editor(),
            "dataset.propose",
            json!({"dataset": native}),
        )
        .await
        .unwrap();
    assert_eq!(saved["organization"]["name"], "dept", "{saved}");
    let stored = w
        .store
        .get_dataset(&w.admin(), "native")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (stored.org_id, stored.publisher_id.as_str()),
        (w.org, "dept")
    );

    let r = w
        .run(
            &w.admin(),
            "dataset.propose",
            json!({"publisherId": "x", "name": 5}),
        )
        .await;
    assert!(field(&r, "dataset"), "{r:?}");
    let r = w
        .run(
            &w.admin(),
            "dataset.propose",
            json!({"name": "x1", "title": "X", "owner_org": "nobody"}),
        )
        .await;
    assert!(field(&r, "owner_org"), "{r:?}");
    let r = w
        .run(
            &w.admin(),
            "dataset.propose",
            json!({"name": "Bad Name", "title": "X", "owner_org": "city"}),
        )
        .await;
    assert!(field(&r, "name"), "{r:?}");

    // SHACL: a violation in the main profile refuses public visibility, not internal.
    let mut ops = w.ops();
    ops.validator = Some(Arc::new(Validator { conforms: false }));
    let public = json!({"name": "shacl-pub", "title": "P", "owner_org": "city"});
    match execute_operation(&ops, &w.admin(), "dataset.propose", public).await {
        Err(DataroomError::ShaclViolation {
            violations,
            details,
            ..
        }) => {
            assert_eq!(violations, 1);
            assert_eq!(
                details,
                ["http://purl.org/dc/terms/description: description is required"]
            );
        }
        other => panic!("expected a SHACL refusal, got {other:?}"),
    }
    let internal = json!({"name": "shacl-int", "title": "I", "owner_org": "city", "private": true});
    execute_operation(&ops, &w.admin(), "dataset.propose", internal)
        .await
        .unwrap();
    let stored = w
        .store
        .get_dataset(&w.admin(), "shacl-int")
        .await
        .unwrap()
        .unwrap();
    let reports = w
        .store
        .list_shacl_reports(&w.admin(), Some(&stored.id), None)
        .await
        .unwrap();
    assert_eq!(reports.len(), 1, "the warning report is stored");
    assert!(!reports[0].conforms);
    ops.validator = Some(Arc::new(Validator { conforms: true }));
    execute_operation(
        &ops,
        &w.admin(),
        "dataset.propose",
        json!({"name": "shacl-ok", "title": "OK", "owner_org": "city"}),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn dataset_propose_cannot_take_over_another_publishers_dataset() {
    let w = world().await;
    // dept's editor names city's dataset and claims it for dept.
    for attempt in [
        json!({"name": "open", "title": "Taken", "owner_org": "dept"}),
        json!({"id": "pkg-open", "name": "open", "title": "Taken", "owner_org": "dept"}),
    ] {
        let r = w
            .run(&w.dept_editor(), "dataset.propose", attempt.clone())
            .await;
        assert!(
            is_forbidden(&r) || is_not_found(&r),
            "dept editor overwriting city's dataset: {r:?}"
        );
    }
    // A hidden record cannot be overwritten either, and the refusal does not reveal it.
    let r = w
        .run(
            &w.dept_editor(),
            "dataset.propose",
            json!({"name": "internal", "title": "Taken", "owner_org": "dept"}),
        )
        .await;
    assert!(is_not_found(&r), "{r:?}");
    let open = w
        .store
        .get_dataset(&w.admin(), "open")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(open.publisher_id, "city");
    assert_eq!(open.title["sk"], "Dataset open");
    let internal = w
        .store
        .get_dataset(&w.admin(), "internal")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(internal.publisher_id, "city");
    // Updating dept's own dataset still works.
    w.run(
        &w.dept_editor(),
        "dataset.propose",
        json!({"name": "dept-air", "title": "Ovzdušie dept", "owner_org": "dept"}),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn dataset_withdraw() {
    let w = world().await;
    assert!(field(
        &w.run(&w.admin(), "dataset.withdraw", json!({})).await,
        "name"
    ));
    assert!(is_not_found(
        &w.run(&w.admin(), "dataset.withdraw", json!({"name": "none"}))
            .await
    ));
    let r = w
        .run(&w.member(), "dataset.withdraw", json!({"name": "open"}))
        .await;
    assert!(is_forbidden(&r), "{r:?}");
    let r = w
        .run(
            &w.anonymous(),
            "dataset.withdraw",
            json!({"name": "internal"}),
        )
        .await;
    assert!(is_not_found(&r), "a hidden dataset stays hidden: {r:?}");
    let r = w
        .run(&w.dept_editor(), "dataset.withdraw", json!({"id": "open"}))
        .await;
    assert!(is_forbidden(&r), "{r:?}");
    let done = w
        .run(
            &w.dept_editor(),
            "dataset.withdraw",
            json!({"name": "dept-air"}),
        )
        .await
        .unwrap();
    assert_eq!(done["state"], "deleted");
    assert_eq!(
        w.store
            .get_dataset(&w.admin(), "dept-air")
            .await
            .unwrap()
            .unwrap()
            .state,
        RecordState::Withdrawn
    );
    assert!(w.activity_ops().await.contains(&"dataset.withdraw".into()));
}

#[tokio::test]
async fn publisher_operations_and_no_self_granted_editing() {
    let w = world().await;
    let listed = w
        .run(&w.anonymous(), "publisher.list", json!({}))
        .await
        .unwrap();
    assert_eq!(names(&listed, "publishers"), ["city", "dept", "gone"]);

    // A member without rights may not create a publisher by naming itself its editor...
    let selfgrant = json!({"publisher": {"name": "mine", "title": {"sk": "Môj"},
        "description": {}, "uri": "https://example.org/mine", "state": "active",
        "editorGrants": ["member"]}});
    let r = w.run(&w.member(), "publisher.propose", selfgrant).await;
    assert!(is_forbidden(&r), "self-granted creation: {r:?}");
    assert!(w
        .store
        .get_publisher(&w.admin(), "mine")
        .await
        .unwrap()
        .is_none());
    // ... nor take over an existing publisher the same way.
    let takeover = json!({"publisher": {"id": "pub-city", "name": "city",
        "title": {"sk": "Ukradnuté"}, "description": {}, "uri": "https://example.org/city",
        "state": "active", "editorGrants": ["member"]}});
    let r = w.run(&w.member(), "publisher.propose", takeover).await;
    assert!(is_forbidden(&r), "self-granted takeover: {r:?}");
    let city = w
        .store
        .get_publisher(&w.admin(), "city")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(city.title["sk"], "Vydavateľ city");
    assert!(city.editor_grants.is_empty());

    // dept's editor may edit dept's record but not hand out editing rights.
    let dept_edit = json!({"publisher": {"id": "pub-dept", "name": "dept",
        "title": {"sk": "Odbor"}, "description": {}, "uri": "https://example.org/dept",
        "state": "active"}});
    let saved = w
        .run(&w.dept_editor(), "publisher.propose", dept_edit)
        .await
        .unwrap();
    assert_eq!(saved["title"]["sk"], "Odbor");
    let dept_grant = json!({"publisher": {"id": "pub-dept", "name": "dept",
        "title": {"sk": "Odbor"}, "description": {}, "uri": "https://example.org/dept",
        "state": "active", "editorGrants": ["friend"]}});
    let r = w
        .run(&w.dept_editor(), "publisher.propose", dept_grant)
        .await;
    assert!(is_forbidden(&r), "editor changing editor grants: {r:?}");

    // The catalogue admin creates publishers and sets their editors.
    let created = w
        .run(
            &w.admin(),
            "jc_publisher_propose",
            json!({"name": "region", "title": {"sk": "Kraj"}, "description": {},
                   "uri": "https://example.org/region", "state": "active",
                   "editorGrants": ["member"]}),
        )
        .await
        .unwrap();
    assert_eq!(created["id"], "pub-region");
    assert!(w.activity_ops().await.contains(&"publisher.propose".into()));
    // Now the member is region's editor and may edit region's record.
    w.run(
        &w.member(),
        "publisher.propose",
        json!({"id": "pub-region", "name": "region", "title": {"sk": "Kraj 2"},
               "description": {}, "uri": "https://example.org/region", "state": "active",
               "editorGrants": ["member"]}),
    )
    .await
    .unwrap();
    let r = w
        .run(
            &w.admin(),
            "publisher.propose",
            json!({"publisher": "not an object"}),
        )
        .await;
    assert!(field(&r, "body"), "{r:?}");
    let r = w
        .run(
            &w.admin(),
            "publisher.propose",
            json!({"publisher": {"name": 1}}),
        )
        .await;
    assert!(field(&r, "publisher"), "{r:?}");
}

#[tokio::test]
async fn publisher_merge_and_withdraw() {
    let w = world().await;
    assert!(field(
        &w.run(&w.admin(), "publisher.merge", json!({})).await,
        "source"
    ));
    assert!(field(
        &w.run(&w.admin(), "publisher.merge", json!({"source": "dept"}))
            .await,
        "target"
    ));
    let r = w
        .run(
            &w.dept_editor(),
            "publisher.merge",
            json!({"source": "dept", "target": "city"}),
        )
        .await;
    assert!(is_forbidden(&r), "{r:?}");
    let merged = w
        .run(
            &w.admin(),
            "publisher.merge",
            json!({"source": "dept", "target": "city"}),
        )
        .await
        .unwrap();
    assert_eq!(merged["name"], "city");
    assert_eq!(
        w.store
            .get_dataset(&w.admin(), "dept-air")
            .await
            .unwrap()
            .unwrap()
            .publisher_id,
        "city"
    );
    assert!(w.activity_ops().await.contains(&"publisher.merge".into()));

    assert!(field(
        &w.run(&w.admin(), "publisher.withdraw", json!({})).await,
        "publisher"
    ));
    assert!(is_not_found(
        &w.run(&w.admin(), "publisher.withdraw", json!({"id": "none"}))
            .await
    ));
    let r = w
        .run(
            &w.member(),
            "publisher.withdraw",
            json!({"publisher": "city"}),
        )
        .await;
    assert!(is_forbidden(&r), "{r:?}");
    let done = w
        .run(
            &w.admin(),
            "publisher.withdraw",
            json!({"publisher": "city"}),
        )
        .await
        .unwrap();
    assert_eq!(done["state"], "withdrawn");
    assert!(w
        .activity_ops()
        .await
        .contains(&"publisher.withdraw".into()));
}

#[tokio::test]
async fn publisher_scoped_grants_apply_to_the_publisher_record() {
    let w = world().await;
    // dept's editor holds Delete on publisher "dept": the publisher record answers to the
    // same name the datasets carry.
    let done = w
        .run(
            &w.dept_editor(),
            "publisher.withdraw",
            json!({"publisher": "dept"}),
        )
        .await
        .unwrap();
    assert_eq!(done["state"], "withdrawn");
    let r = w
        .run(
            &w.dept_editor(),
            "publisher.withdraw",
            json!({"publisher": "city"}),
        )
        .await;
    assert!(is_forbidden(&r), "{r:?}");
}

struct Harvester;

impl HarvestRunner for Harvester {
    fn run_source<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        store: &'a dyn Store,
        source: &'a HarvestSource,
    ) -> BoxFuture<'a, Result<HarvestRun>> {
        Box::pin(async move {
            let run: HarvestRun = serde_json::from_value(json!({
                "orgId": ctx.org_id, "catalogueId": ctx.catalogue_id, "id": "run-1",
                "sourceId": source.id, "state": "succeeded",
                "startedAt": "2026-11-01T10:00:00Z", "createdCount": 1, "updatedCount": 0,
                "unchangedCount": 0, "withdrawnCount": 0,
                "logs": [
                    {"at": "2026-11-01T10:00:00Z", "level": "info", "fingerprint": "a", "message": "start"},
                    {"at": "2026-11-01T10:00:01Z", "level": "warn", "fingerprint": "b", "message": "skip"}
                ]
            }))
            .unwrap();
            store.upsert_harvest_run(ctx, run).await
        })
    }
}

#[tokio::test]
async fn harvest_source_propose_is_refused_where_the_portal_writes_the_sources() {
    let w = world().await;
    let ops = OpContext {
        sources_by_portal: true,
        ..w.ops()
    };
    let src = json!({"source": {"name": "nkod", "kind": "dcat-ap",
        "url": "https://nkod.example/feed.ttl", "publisherId": "city",
        "allowedHosts": ["nkod.example"]}});
    let r = execute_operation(&ops, &w.admin(), "harvest_source.propose", src).await;
    match r {
        Err(DataroomError::NotSupported(why)) => assert!(why.contains("ODP-11"), "{why}"),
        other => panic!("not refused: {other:?}"),
    }
    let listed = execute_operation(&ops, &w.admin(), "harvest_source.list", json!({}))
        .await
        .unwrap();
    assert_eq!(listed["sources"], json!([]), "nothing written");
}

#[tokio::test]
async fn harvest_operations_are_for_members_with_rights() {
    let w = world().await;
    let r = w
        .run(&w.anonymous(), "harvest_source.list", json!({}))
        .await;
    assert!(is_not_found(&r), "anonymous: {r:?}");
    let r = w.run(&w.member(), "harvest_source.list", json!({})).await;
    assert!(is_forbidden(&r), "member: {r:?}");

    let src = json!({"source": {"name": "nkod", "kind": "dcat-ap",
        "url": "https://nkod.example/feed.ttl", "publisherId": "city",
        "allowedHosts": ["nkod.example"]}});
    let r = w
        .run(&w.member(), "harvest_source.propose", src.clone())
        .await;
    assert!(is_forbidden(&r), "{r:?}");
    let saved = w
        .run(&w.admin(), "harvest_source.propose", src)
        .await
        .unwrap();
    assert_eq!(saved["id"], "hs-nkod");
    assert_eq!(saved["orgId"], json!(w.org));
    let r = w
        .run(
            &w.admin(),
            "harvest_source.propose",
            json!({"source": {"kind": "ftp"}}),
        )
        .await;
    assert!(field(&r, "source"), "{r:?}");
    let listed = w
        .run(&w.admin(), "harvest_source.list", json!({}))
        .await
        .unwrap();
    assert_eq!(listed["sources"].as_array().unwrap().len(), 1);

    assert!(field(
        &w.run(&w.admin(), "harvest_run.start", json!({})).await,
        "source"
    ));
    assert!(is_not_found(
        &w.run(&w.admin(), "harvest_run.start", json!({"source": "none"}))
            .await
    ));
    let r = w
        .run(&w.member(), "harvest_run.start", json!({"source": "nkod"}))
        .await;
    assert!(is_forbidden(&r), "{r:?}");
    let r = w
        .run(&w.admin(), "harvest_run.start", json!({"source": "nkod"}))
        .await;
    assert!(matches!(r, Err(DataroomError::NotSupported(_))), "{r:?}");
    let mut ops = w.ops();
    ops.harvester = Some(Arc::new(Harvester));
    let run = execute_operation(
        &ops,
        &w.admin(),
        "harvest_run.start",
        json!({"source": "hs-nkod"}),
    )
    .await
    .unwrap();
    assert_eq!(run["id"], "run-1");

    for op in ["harvest_run.get", "harvest_run.logs"] {
        assert!(
            field(&w.run(&w.admin(), op, json!({})).await, "run"),
            "{op}"
        );
        assert!(is_not_found(
            &w.run(&w.admin(), op, json!({"run": "run-9"})).await
        ));
        let r = w.run(&w.anonymous(), op, json!({"run": "run-1"})).await;
        assert!(is_not_found(&r), "{op} anonymous: {r:?}");
    }
    let got = w
        .run(&w.admin(), "harvest_run.get", json!({"run": "run-1"}))
        .await
        .unwrap();
    assert_eq!(got["state"], "succeeded");
    let logs = w
        .run(
            &w.admin(),
            "harvest_run.logs",
            json!({"run": "run-1", "level": "WARN"}),
        )
        .await
        .unwrap();
    assert_eq!(logs["logs"].as_array().unwrap().len(), 1);
    let all = w
        .run(&w.admin(), "harvest_run.logs", json!({"run": "run-1"}))
        .await
        .unwrap();
    assert_eq!(all["logs"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn plugins_profiles_branding_activity_and_feed_check() {
    let w = world().await;
    let plugin = json!({"plugin": {"name": "maps", "version": "1.0.0", "digest": "sha256:00",
        "enabled": true}});
    let r = w.run(&w.member(), "plugin.propose", plugin.clone()).await;
    assert!(is_forbidden(&r), "{r:?}");
    let saved = w.run(&w.admin(), "plugin.propose", plugin).await.unwrap();
    assert_eq!(saved["orgId"], json!(w.org));
    let r = w
        .run(
            &w.admin(),
            "plugin.propose",
            json!({"plugin": {"name": "x"}}),
        )
        .await;
    assert!(field(&r, "plugin"), "{r:?}");
    let listed = w
        .run(&w.anonymous(), "plugin.list", json!({}))
        .await
        .unwrap();
    assert_eq!(listed["plugins"].as_array().unwrap().len(), 1);

    let profile = json!({"profile": {"id": "city-ext", "version": "1",
        "uri": "https://example.org/p", "title": {"en": "City"}, "shapesTurtle": "",
        "builtIn": false}});
    let r = w.run(&w.member(), "profile.propose", profile.clone()).await;
    assert!(is_forbidden(&r), "{r:?}");
    w.run(&w.admin(), "profile.propose", profile).await.unwrap();
    let r = w
        .run(&w.admin(), "profile.propose", json!({"profile": {"id": 1}}))
        .await;
    assert!(field(&r, "profile"), "{r:?}");
    let listed = w
        .run(&w.anonymous(), "profile.list", json!({}))
        .await
        .unwrap();
    assert_eq!(listed["profiles"][0]["id"], "city-ext");

    let branding = w
        .run(&w.anonymous(), "branding.get", json!({}))
        .await
        .unwrap();
    assert_eq!(branding["languages"]["default"], "sk");
    let r = w
        .run(&w.member(), "branding.propose", json!({"branding": {}}))
        .await;
    assert!(is_forbidden(&r), "{r:?}");
    let eff = w
        .run(
            &w.admin(),
            "branding.propose",
            json!({"branding": {"colours": {"primary": "#123456"}}}),
        )
        .await
        .unwrap();
    assert_eq!(eff["colours"]["primary"], "#123456", "{eff}");

    let r = w.run(&w.anonymous(), "catalogue.activity", json!({})).await;
    assert!(is_not_found(&r), "{r:?}");
    let r = w.run(&w.outsider(), "catalogue.activity", json!({})).await;
    assert!(is_not_found(&r), "{r:?}");
    let act = w
        .run(&w.member(), "catalogue.activity", json!({}))
        .await
        .unwrap();
    let rows = act["activity"].as_array().unwrap().len();
    assert!(rows >= 3, "{act}");
    let one = w
        .run(&w.member(), "catalogue.activity", json!({"limit": 1}))
        .await
        .unwrap();
    assert_eq!(one["activity"].as_array().unwrap().len(), 1);
    let none = w
        .run(
            &w.member(),
            "catalogue.activity",
            json!({"object": "nothing"}),
        )
        .await
        .unwrap();
    assert!(none["activity"].as_array().unwrap().is_empty());

    let fc = w
        .run(&w.anonymous(), "catalogue.feed_check", json!({}))
        .await
        .unwrap();
    assert_eq!(fc["conforms"], true);
    assert_eq!(fc["datasetsChecked"], 3, "public active datasets: {fc}");
    let mut ops = w.ops();
    ops.validator = Some(Arc::new(Validator { conforms: true }));
    let fc = execute_operation(
        &ops,
        &w.anonymous(),
        "catalogue.feed_check",
        json!({"profile": "dcat-ap-sk-2.0"}),
    )
    .await
    .unwrap();
    assert_eq!(fc, json!({"checked": 3, "profile": "dcat-ap-sk-2.0"}));
}

/// `allowed` answers what the call does: allowed exactly when the call is not refused for
/// authorization, and "not found" wherever the call would not reveal the record.
#[tokio::test]
async fn allowed_answers_like_the_call() {
    let w = world().await;
    assert!(field(
        &w.run(&w.admin(), "allowed", json!({})).await,
        "operation"
    ));
    let unknown = w
        .run(&w.admin(), "allowed", json!({"operation": "nope"}))
        .await
        .unwrap();
    assert_eq!(unknown["allowed"], false);

    let ask = |ctx: RequestCtx, op: &'static str, object: Option<&'static str>| {
        let w = &w;
        async move {
            let mut input = json!({"operation": op});
            if let Some(o) = object {
                input["object"] = json!(o);
            }
            w.run(&ctx, "allowed", input).await.unwrap()
        }
    };

    // Hidden records are "not found", whether they exist or not.
    for object in ["internal", "no-such-dataset"] {
        let a = ask(w.anonymous(), "dataset.get", Some(object)).await;
        assert_eq!(a["allowed"], false);
        assert_eq!(a["reason"], "dataset not found", "{object}");
    }
    let a = ask(w.anonymous(), "catalogue.activity", None).await;
    assert_eq!(
        (a["allowed"].clone(), a["reason"].clone()),
        (json!(false), json!("not found"))
    );
    assert_eq!(
        ask(w.member(), "catalogue.activity", None).await["allowed"],
        true
    );
    let a = ask(w.anonymous(), "publisher.withdraw", Some("ghost")).await;
    assert_eq!(a["reason"], "publisher not found");
    let a = ask(w.admin(), "harvest_run.start", Some("ghost")).await;
    assert_eq!(a["reason"], "not found");
    let a = ask(w.anonymous(), "harvest_source.list", None).await;
    assert_eq!(
        a["reason"], "not found",
        "harvest is hidden from non-members"
    );

    // Decisions match the calls.
    assert_eq!(
        ask(w.anonymous(), "dataset.get", Some("open")).await["allowed"],
        true
    );
    assert_eq!(
        ask(w.auditor(), "dataset.get", Some("restricted")).await["allowed"],
        true
    );
    assert_eq!(
        ask(w.dept_editor(), "dataset.withdraw", Some("dept-air")).await["allowed"],
        true
    );
    assert_eq!(
        ask(w.dept_editor(), "dataset.withdraw", Some("open")).await["allowed"],
        false
    );
    assert_eq!(
        ask(w.member(), "dataset.propose", None).await["allowed"],
        false
    );
    assert_eq!(
        ask(w.admin(), "dataset.propose", None).await["allowed"],
        true
    );
    assert_eq!(
        ask(w.admin(), "publisher.propose", None).await["allowed"],
        true
    );
    assert_eq!(
        ask(w.admin(), "profile.propose", Some("p")).await["allowed"],
        true
    );
    assert_eq!(
        ask(w.member(), "plugin.propose", Some("maps")).await["allowed"],
        false
    );
    assert_eq!(
        ask(w.admin(), "branding.propose", None).await["allowed"],
        true
    );
    let a = ask(w.anonymous(), "dataset.get", Some("retired")).await;
    assert_eq!(
        a["reason"], "dataset not found",
        "a withdrawn dataset is as hidden in allowed as in dataset.get"
    );
    w.run(
        &w.admin(),
        "harvest_source.propose",
        json!({"name": "nkod", "kind": "dcat-ap", "url": "https://n.example/f.ttl",
               "publisherId": "city", "allowedHosts": ["n.example"]}),
    )
    .await
    .unwrap();
    assert_eq!(
        ask(w.admin(), "harvest_run.start", Some("nkod")).await["allowed"],
        true
    );
    assert_eq!(
        ask(w.member(), "harvest_run.start", Some("nkod")).await["allowed"],
        false
    );

    // publisher.merge needs Admin on the catalogue: a publisher-scoped admin is refused by
    // the call, so allowed must refuse it too.
    let scoped = PolicyAuthorizer::new(vec![GrantRule {
        org_id: w.org,
        catalogue_id: Some(w.cat),
        principal: "dept-admin".into(),
        actions: vec![Action::Admin],
        publisher_id: Some("dept".into()),
    }]);
    let ops = OpContext {
        store: w.store.clone(),
        authorizer: Arc::new(scoped),
        validator: None,
        harvester: None,
        sources_by_portal: false,
    };
    let dept_admin = w.ctx(Subject::authenticated(
        "dept-admin",
        vec![w.org],
        Vec::new(),
    ));
    let call = execute_operation(
        &ops,
        &dept_admin,
        "publisher.merge",
        json!({"source": "dept", "target": "city"}),
    )
    .await;
    let asked = execute_operation(
        &ops,
        &dept_admin,
        "allowed",
        json!({"operation": "publisher.merge", "object": "dept"}),
    )
    .await
    .unwrap();
    assert!(is_forbidden(&call), "{call:?}");
    assert_eq!(
        asked["allowed"], false,
        "allowed must match the call: {asked}"
    );
}

#[tokio::test]
async fn writes_into_a_catalogue_that_does_not_exist_are_not_found() {
    let w = world().await;
    // Same tenant and grants, but a store that holds no catalogue for it.
    let empty = World {
        store: Arc::new(MemoryStore::new()),
        authz: w.authz.clone(),
        org: w.org,
        cat: w.cat,
    };
    let admin = empty.admin();
    for (op, input) in [
        (
            "dataset.propose",
            json!({"name": "novy", "title": "Nový", "notes": "Popis", "owner_org": "mesto"}),
        ),
        (
            "branding.propose",
            json!({"branding": {"primary": "#112233"}}),
        ),
    ] {
        match empty.run(&admin, op, input).await {
            Err(DataroomError::NotFound(m)) => assert_eq!(m, "catalogue not found", "{op}"),
            other => panic!("{op}: {other:?}"),
        }
    }
    assert!(empty.activity_ops().await.is_empty());
}
