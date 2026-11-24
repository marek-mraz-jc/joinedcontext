//! PL-19, PL-20 (T-7794): each Pipeline of a repository derives its own account and Policies on
//! the Endpoints its manifest names, and nothing for a spec that does not parse.

use jc_manifest::loader::{RawManifest, Repository};
use jc_manifest::pipeline_identity::{derived, endpoint_grants, RUNNER_NAMESPACE};

fn manifest(yaml: &str) -> RawManifest {
    serde_norway::from_str(yaml).expect("manifest parses")
}

const ENDPOINT: &str = "apiVersion: joinedcontext.com/v1alpha1
kind: Endpoint
metadata: { name: ep-write, namespace: city }
spec:
  contextSpaceRef: air
  slug: zt4qm7ge2xdv6ksb3ncf5arw2y
  audience: public
  enabledRepresentations: [ngsi-ld]
";

const PIPELINE: &str = "apiVersion: joinedcontext.com/v1alpha2
kind: Pipeline
metadata: { name: sensors, namespace: city }
spec:
  class: resident
  sources:
    - dataSourceRef: { kind: DataSource, name: feed }
  outputs:
    - targetEndpoint: urn:ngsi-ld:Endpoint:city.example:air:ep-write
";

fn repo(extra: &[&str]) -> Repository {
    let mut manifests = vec![manifest(ENDPOINT), manifest(PIPELINE)];
    manifests.extend(extra.iter().map(|yaml| manifest(yaml)));
    Repository::from_manifests(manifests, Some("city.example")).expect("the repository assembles")
}

#[test]
fn a_pipeline_derives_its_account_and_a_write_grant_on_its_target() {
    let all = derived(&repo(&[]));
    assert_eq!(all.len(), 1);
    let (project, identity) = &all[0];
    assert_eq!(project, "city");
    assert!(!identity.policies.is_empty(), "{identity:?}");
    assert!(identity
        .endpoints
        .values()
        .all(|endpoint| endpoint == "ep-write"));
    let grants = endpoint_grants(&all);
    let on_target = grants
        .get(&("city".to_owned(), "ep-write".to_owned()))
        .expect("a grant on the target Endpoint");
    assert_eq!(on_target.len(), identity.endpoints.len());
    assert_eq!(RUNNER_NAMESPACE, "pipeline-runner");
}

#[test]
fn a_pipeline_whose_spec_does_not_parse_derives_nothing_and_a_broken_endpoint_is_no_space() {
    let broken_pipeline = "apiVersion: joinedcontext.com/v1alpha2
kind: Pipeline
metadata: { name: broken, namespace: city }
spec: { class: 42 }
";
    let broken_endpoint = "apiVersion: joinedcontext.com/v1alpha1
kind: Endpoint
metadata: { name: ep-broken, namespace: city }
spec: { audience: nobody }
";
    let all = derived(&repo(&[broken_pipeline, broken_endpoint]));
    assert_eq!(all.len(), 1, "only the Pipeline that parses: {all:?}");
    assert_eq!(all[0].1.name, derived(&repo(&[]))[0].1.name);
}

#[test]
fn a_target_in_another_project_grants_nothing_here() {
    let elsewhere = "apiVersion: joinedcontext.com/v1alpha2
kind: Pipeline
metadata: { name: other, namespace: transport }
spec:
  class: resident
  sources:
    - dataSourceRef: { kind: DataSource, name: feed }
  outputs:
    - targetEndpoint: urn:ngsi-ld:Endpoint:city.example:air:ep-write
";
    let all = derived(&repo(&[elsewhere]));
    let other = all
        .iter()
        .find(|(project, _)| project == "transport")
        .expect("the other Pipeline derives an account");
    assert!(other.1.endpoints.is_empty(), "{other:?}");
    let grants = endpoint_grants(&all);
    assert!(!grants.contains_key(&("transport".to_owned(), "ep-write".to_owned())));
}

#[test]
fn no_pipelines_derive_nothing() {
    let repo = Repository::from_manifests(vec![manifest(ENDPOINT)], None).expect("assembles");
    assert!(derived(&repo).is_empty());
    assert!(endpoint_grants(&[]).is_empty());
}
