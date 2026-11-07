use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::reconciler::Syncer;
use joinedcontext_portal::resource::{ObjectMeta, Phase, ResourceEnvelope, Status, API_VERSION};
use joinedcontext_portal::store::Mirror;
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// UI-08: every read of configuration state is served from a mirror the sync fills from the
/// repository's default branch, never from Git per request.
#[tokio::test]
async fn sync_fills_mirror_from_tree_with_live_status_and_observed_revision() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client =
        Arc::new(GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").unwrap());
    let mirror = Arc::new(Mirror::new());
    let syncer = Syncer::new(Arc::clone(&client), Arc::clone(&mirror));

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "default_branch": "main"
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches/main"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "main",
            "commit": { "id": "c0ffee123456" }
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/git/trees/c0ffee123456",
        ))
        .and(query_param("recursive", "true"))
        .and(query_param("per_page", "1000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "tree-sha-1",
            "truncated": false,
            "tree": [
                {
                    "path": "projects/ovzdusie/spaces/mobility/space.yaml",
                    "type": "blob"
                },
                {
                    "path": "projects/ovzdusie/spaces/mobility/endpoints/air.yaml",
                    "type": "blob"
                }
            ]
        })))
        .mount(&server)
        .await;

    let space_yaml = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: mobility\n  namespace: ovzdusie\nspec:\n  isSandbox: true\n";
    let space_b64 = STANDARD.encode(space_yaml.as_bytes());

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .and(query_param("ref", "c0ffee123456"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-1",
            "content": space_b64
        })))
        .mount(&server)
        .await;

    let endpoint_yaml = "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: public-air\n  namespace: ovzdusie\nspec:\n  audience: public\n";
    let endpoint_b64 = STANDARD.encode(endpoint_yaml.as_bytes());

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/endpoints/air.yaml",
        ))
        .and(query_param("ref", "c0ffee123456"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-2",
            "content": endpoint_b64
        })))
        .mount(&server)
        .await;

    let count = syncer.sync_once().await.expect("sync should succeed");
    assert_eq!(count, 2);

    let status = syncer.status();
    assert_eq!(status.manifests, 2);
    assert_eq!(status.revision.as_deref(), Some("c0ffee123456"));
    assert!(status.last_sync.is_some());
    assert!(status.last_error.is_none());

    assert_eq!(mirror.len(), 2);

    let space = mirror
        .get("ovzdusie", "ContextSpace", "mobility")
        .expect("space should be in mirror");
    let space_status = space.status.as_ref().expect("status should be populated");
    assert_eq!(space_status.phase, Phase::Live);
    assert_eq!(
        space_status.observed_revision.as_deref(),
        Some("c0ffee123456")
    );
    assert!(space_status.conditions.is_empty());
    // The Source link points at the branch page of the very file the manifest came from, behind
    // the forge's own sign-in: a Portal session is not a forge session, and the configuration
    // repository is private (PF-81, T-0702).
    assert_eq!(
        space_status.source_url.as_deref(),
        Some(
            format!(
                "{}/user/oauth2/keycloak?redirect_to=%2Ftest-owner%2Ftest-repo%2Fsrc%2Fbranch%2Fmain%2Fprojects%2Fovzdusie%2Fspaces%2Fmobility%2Fspace.yaml",
                server.uri()
            )
            .as_str()
        )
    );

    let endpoint = mirror
        .get("ovzdusie", "Endpoint", "public-air")
        .expect("endpoint should be in mirror");
    let ep_status = endpoint
        .status
        .as_ref()
        .expect("status should be populated");
    assert_eq!(ep_status.phase, Phase::Live);
    assert_eq!(ep_status.observed_revision.as_deref(), Some("c0ffee123456"));
    assert!(ep_status.conditions.is_empty());
}

#[tokio::test]
async fn sync_skips_unparseable_yaml_and_unknown_kinds_while_loading_valid_manifests() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client =
        Arc::new(GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").unwrap());
    let mirror = Arc::new(Mirror::new());
    let syncer = Syncer::new(Arc::clone(&client), Arc::clone(&mirror));

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "default_branch": "main"
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches/main"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "main",
            "commit": { "id": "rev-test-skip" }
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/git/trees/rev-test-skip",
        ))
        .and(query_param("recursive", "true"))
        .and(query_param("per_page", "1000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "tree-sha-2",
            "truncated": false,
            "tree": [
                {
                    "path": "projects/ovzdusie/spaces/mobility/space.yaml",
                    "type": "blob"
                },
                {
                    "path": "projects/ovzdusie/spaces/mobility/broken.yaml",
                    "type": "blob"
                },
                {
                    "path": "projects/ovzdusie/spaces/mobility/unknown.yaml",
                    "type": "blob"
                }
            ]
        })))
        .mount(&server)
        .await;

    let valid_space = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: mobility\n  namespace: ovzdusie\nspec:\n  isSandbox: true\n";
    let valid_b64 = STANDARD.encode(valid_space.as_bytes());

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .and(query_param("ref", "rev-test-skip"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-valid",
            "content": valid_b64
        })))
        .mount(&server)
        .await;

    let broken_yaml = "::: invalid [yaml { syntax :::\n\t[bad\n";
    let broken_b64 = STANDARD.encode(broken_yaml.as_bytes());

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/broken.yaml",
        ))
        .and(query_param("ref", "rev-test-skip"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-broken",
            "content": broken_b64
        })))
        .mount(&server)
        .await;

    let unknown_kind_yaml = "apiVersion: joinedcontext.com/v1alpha1\nkind: UnknownDeviceWidget\nmetadata:\n  name: widget-one\n  namespace: ovzdusie\nspec: {}\n";
    let unknown_b64 = STANDARD.encode(unknown_kind_yaml.as_bytes());

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/unknown.yaml",
        ))
        .and(query_param("ref", "rev-test-skip"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-unknown",
            "content": unknown_b64
        })))
        .mount(&server)
        .await;

    let count = syncer
        .sync_once()
        .await
        .expect("sync should succeed despite skipped manifests");
    assert_eq!(count, 1);
    assert_eq!(mirror.len(), 1);
    assert!(mirror.get("ovzdusie", "ContextSpace", "mobility").is_some());
}

#[tokio::test]
async fn sync_replaces_untrusted_status_block_from_manifest_file() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client =
        Arc::new(GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").unwrap());
    let mirror = Arc::new(Mirror::new());
    let syncer = Syncer::new(Arc::clone(&client), Arc::clone(&mirror));

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "default_branch": "main"
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches/main"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "main",
            "commit": { "id": "commit-head-rev-777" }
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/git/trees/commit-head-rev-777",
        ))
        .and(query_param("recursive", "true"))
        .and(query_param("per_page", "1000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "tree-sha-3",
            "truncated": false,
            "tree": [
                {
                    "path": "projects/ovzdusie/spaces/mobility/space.yaml",
                    "type": "blob"
                }
            ]
        })))
        .mount(&server)
        .await;

    let manifest_with_untrusted_status = r#"apiVersion: joinedcontext.com/v1alpha1
kind: ContextSpace
metadata:
  name: mobility
  namespace: ovzdusie
spec:
  isSandbox: false
status:
  phase: Error
  observedRevision: old-untrusted-rev-999
  conditions:
    - type: Reconciled
      status: "False"
      reason: Broken
      lastTransitionTime: "2020-01-01T00:00:00Z"
"#;
    let b64 = STANDARD.encode(manifest_with_untrusted_status.as_bytes());

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .and(query_param("ref", "commit-head-rev-777"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-status-fake",
            "content": b64
        })))
        .mount(&server)
        .await;

    let count = syncer.sync_once().await.expect("sync should succeed");
    assert_eq!(count, 1);

    let space = mirror
        .get("ovzdusie", "ContextSpace", "mobility")
        .expect("mobility should exist in mirror");
    let status = space.status.expect("status must be present");
    assert_eq!(status.phase, Phase::Live);
    assert_eq!(
        status.observed_revision.as_deref(),
        Some("commit-head-rev-777")
    );
    assert!(status.conditions.is_empty());
}

/// UI-08: a sync that cannot read the repository keeps serving the last mirror it had.
#[tokio::test]
async fn failed_tree_listing_preserves_mirror_and_records_error_in_sync_status() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client =
        Arc::new(GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").unwrap());
    let mirror = Arc::new(Mirror::new());

    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "ContextSpace".to_string(),
        metadata: ObjectMeta {
            name: "existing-space".to_string(),
            namespace: Some("ovzdusie".to_string()),
            ..Default::default()
        },
        spec: json!({ "isSandbox": true }),
        status: Some(Status {
            phase: Phase::Live,
            observed_revision: Some("initial-rev".to_string()),
            source_url: None,
            conditions: Vec::new(),
            build: None,
            shard: None,
            domain_verification: None,
            jobs: Vec::new(),
            notifications: None,
        }),
    });
    assert_eq!(mirror.len(), 1);

    let syncer = Arc::new(Syncer::new(Arc::clone(&client), Arc::clone(&mirror)));

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "default_branch": "main"
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches/main"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "main",
            "commit": { "id": "failing-rev" }
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/git/trees/failing-rev",
        ))
        .respond_with(ResponseTemplate::new(500).set_body_string("tree database corrupted"))
        .mount(&server)
        .await;

    let err = syncer.sync_once().await.unwrap_err();
    let err_str = err.to_string();
    assert!(err_str.contains("tree database corrupted") || err_str.contains("500"));

    // Previous resources are still preserved
    assert_eq!(mirror.len(), 1);
    assert!(mirror
        .get("ovzdusie", "ContextSpace", "existing-space")
        .is_some());

    // The sync status records the error
    let recorded_error = syncer
        .status()
        .last_error
        .expect("last_error must be present");
    assert!(recorded_error.contains("tree database corrupted") || recorded_error.contains("500"));
}

/// PF-62, PF-63, AP-118 (T-3031): a `Group` lives at `users/groups/{name}.yaml`, and a sync
/// loads it into the mirror. The staging filter once left that directory out, so no Group ever
/// reached the mirror: the realm's groups were never written and an App's default groups gave
/// nobody its roles (helsinki-alerts' demo stewards and viewers were refused).
#[tokio::test]
async fn sync_loads_the_groups_of_users_groups_into_the_mirror() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client =
        Arc::new(GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").unwrap());
    let mirror = Arc::new(Mirror::new());
    let syncer = Syncer::new(Arc::clone(&client), Arc::clone(&mirror));

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "default_branch": "main"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches/main"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "main",
            "commit": { "id": "rev-groups" }
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/git/trees/rev-groups",
        ))
        .and(query_param("recursive", "true"))
        .and(query_param("per_page", "1000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "tree-groups",
            "truncated": false,
            "tree": [
                { "path": "users/groups/helsinki-alerts-steward.yaml", "type": "blob" },
                { "path": "users/groups/README.md", "type": "blob" },
            ]
        })))
        .mount(&server)
        .await;

    // The seed's own file (deployment components/gitea/apps/helsinki-alerts/grants/).
    let group = "apiVersion: joinedcontext.com/v1alpha1\nkind: Group\nmetadata:\n  name: helsinki-alerts-steward\n  namespace: org\n  annotations:\n    joinedcontext.com/app: helsinki/helsinki-alerts\nspec:\n  description: The stewards of helsinki-alerts, its default group (AP-118).\n  members:\n  - user: demo.steward@hel.fi\n";
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/users/groups/helsinki-alerts-steward.yaml",
        ))
        .and(query_param("ref", "rev-groups"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-group",
            "content": STANDARD.encode(group.as_bytes())
        })))
        .mount(&server)
        .await;

    let count = syncer.sync_once().await.expect("the sync loads the group");

    assert_eq!(
        count, 1,
        "the Group and nothing else; the README is no manifest"
    );
    let held = mirror
        .get("org", "Group", "helsinki-alerts-steward")
        .expect("the Group is in the mirror");
    assert_eq!(
        held.spec["members"],
        json!([{ "user": "demo.steward@hel.fi" }])
    );
}
