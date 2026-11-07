//! Every app variant (T-2705, SDK-13, AP-14, AP-26, AP-28, AP-93, AP-120): kind × access ×
//! endpoints × visibility, every combination, each App manifest as a run publishes it.
//!
//! For each variant the Portal renders the App's Endpoint and Policies exactly as it commits them
//! (`apps::reconciler::grants`), and the gateway's own code decides: the roles the Endpoint gives
//! a caller (`EndpointRoles::of_app(..).held_by`, as `with_endpoint_roles` adds them) and the
//! policy decision (`pdp::evaluator::evaluate`). Each variant asserts the refusals as well as the
//! successes: a read works, a write the access allows works, a write it forbids is denied, a
//! role-gated write is the role's alone, an anonymous caller reaches only a public app, a person
//! without a role of a `roles` app gets its `403` page, and the edge lets an anonymous request
//! through to a public app and to no other.
//!
//! The guard at the end fails when jc-core gains a class or a visibility, or the builder a preset,
//! that has no variant here.

use std::collections::{BTreeMap, BTreeSet};

use axum::http::{HeaderMap, StatusCode};
use chrono::Utc;
use context_gateway::auth::subject::Subject;
use context_gateway::pdp::evaluator::{evaluate, Request};
use context_gateway::resolver::EndpointRoles;
use jc_core::kinds::{AppSpec, EndpointSpec, Operation, PolicySpec};
use jcctl::loader::RawManifest;
use joinedcontext_portal::apps::reconciler::{
    generate_slug, grants, render, RenderError, Settings,
};
use joinedcontext_portal::apps::roles::{admits, refusal, AppPerson, Standing};
use joinedcontext_portal::auth::Identity;
use joinedcontext_portal::reconciler::app_clients::ClientSecret;
use joinedcontext_portal::reconciler::edge_file::edge_apps;
use joinedcontext_portal::resource::ResourceEnvelope;
use joinedcontext_portal::store::Mirror;
use serde_json::json;

const PROJECT: &str = "liptov";
const SPACE: &str = "dopravne-hlasenia";
const FURTHER: &str = "cesty";
const TYPE: &str = "TrafficAlert";
const APP: &str = "desk";
const IMAGE: &str =
    "ghcr.io/liptov/apps/desk@sha256:2222222222222222222222222222222222222222222222222222222222222222";

/// The kinds of T-2705, as the manifest spells each, and the server WASM App (AP-148, T-3415).
const KINDS: [&str; 5] = ["react", "html", "react-functions", "fullstack", "wasm"];
/// The builder's presets (AP-132), the operations each adds as `ACCESS_PRESETS` in AppGenerator.
const PRESETS: [(&str, &[&str]); 3] = [
    (
        "read",
        &[
            "queryEntity",
            "retrieveEntity",
            "queryTemporal",
            "retrieveTemporal",
        ],
    ),
    (
        "update",
        &[
            "queryEntity",
            "retrieveEntity",
            "queryTemporal",
            "retrieveTemporal",
            "updateAttrs",
            "appendAttrs",
        ],
    ),
    (
        "full",
        &[
            "queryEntity",
            "retrieveEntity",
            "queryTemporal",
            "retrieveTemporal",
            "updateAttrs",
            "appendAttrs",
            "createEntity",
            "deleteEntity",
        ],
    ),
];
const ENDPOINTS: [usize; 2] = [1, 2];
const VISIBILITIES: [&str; 5] = ["private", "project", "organization", "public", "roles"];

#[derive(Debug, Clone, Copy)]
struct Variant {
    kind: &'static str,
    access: &'static str,
    endpoints: usize,
    visibility: &'static str,
}

impl Variant {
    fn writes(&self) -> bool {
        self.access != "read"
    }

    fn label(&self) -> String {
        format!(
            "{} / {} / {} endpoint(s) / {}",
            self.kind, self.access, self.endpoints, self.visibility
        )
    }
}

/// Every combination: 150 variants, each rendered and decided in-process.
fn variants() -> Vec<Variant> {
    let mut all = Vec::new();
    for kind in KINDS {
        for (access, _) in PRESETS {
            for endpoints in ENDPOINTS {
                for visibility in VISIBILITIES {
                    all.push(Variant {
                        kind,
                        access,
                        endpoints,
                        visibility,
                    });
                }
            }
        }
    }
    all
}

/// The rule that refuses a variant's manifest, for the cells no App may be.
fn refused_by(variant: &Variant) -> Option<&'static str> {
    match (variant.kind, variant.visibility) {
        (_, "private") => Some("AP-18"),
        ("fullstack" | "wasm", "roles") => Some("AP-94"),
        _ => None,
    }
}

fn operations(access: &str) -> Vec<&'static str> {
    PRESETS
        .iter()
        .find(|(name, _)| *name == access)
        .map(|(_, ops)| ops.to_vec())
        .unwrap_or_default()
}

/// The App manifest a run of this variant publishes: the builder's needs (AP-22, AP-132), a
/// `roles` app's write held by its `editor` role (AP-96) and its two roles held by their default
/// groups (AP-118), and a second endpoint read beside it in a further space (AP-44, AP-04).
fn manifest(variant: &Variant) -> RawManifest {
    let (class, build) = match variant.kind {
        "fullstack" => ("ui-rust", json!({ "rust": "1.90", "node": "22" })),
        "wasm" => ("wasm", json!({ "rust": "1.90", "node": "22" })),
        "html" => ("ui", json!({})),
        _ => ("ui", json!({ "node": "22" })),
    };
    let reads: Vec<&str> = operations("read");
    let need = |ops: Vec<&str>, roles: Option<&str>| {
        let mut need = json!({
            "contextSpaceRef": { "kind": "ContextSpace", "name": SPACE },
            "types": [TYPE],
            "attrs": ["title", "severity", "location"],
            "operations": ops,
            "representations": ["ngsi-ld"],
        });
        if let Some(role) = roles {
            need["roles"] = json!([role]);
        }
        need
    };
    let mut needs = if !variant.writes() {
        vec![need(reads, None)]
    } else if variant.visibility == "roles" {
        // Everyone the app admits reads; only the editors write (the builder's write role).
        vec![
            need(reads, None),
            need(operations(variant.access), Some("editor")),
        ]
    } else {
        vec![need(operations(variant.access), None)]
    };
    if variant.endpoints == 2 {
        needs.push(json!({
            "contextSpaceRef": { "kind": "ContextSpace", "name": FURTHER },
            "types": ["RoadSegment"],
            "attrs": ["name", "location"],
            "operations": ["queryEntity", "retrieveEntity"],
            "representations": ["ngsi-ld"],
        }));
    }
    let mut spec = json!({
        "kind": class,
        "source": { "path": "./src" },
        "build": build,
        "visibility": variant.visibility,
        "lifecycle": "published",
        "dataNeeds": needs,
    });
    if variant.kind == "wasm" {
        spec["storage"] = json!({ "sql": {}, "blob": {} });
    }
    if variant.visibility == "roles" {
        spec["roles"] = json!([
            { "name": "viewer", "title": { "en": "Viewer" } },
            { "name": "editor", "title": { "en": "Editor" } },
        ]);
        spec["access"] = json!([
            { "role": "viewer", "subjects": [{ "group": format!("{APP}-viewer") }] },
            { "role": "editor", "subjects": [{ "group": format!("{APP}-editor") }] },
        ]);
    }
    serde_json::from_value(json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "App",
        "metadata": { "name": APP, "namespace": PROJECT },
        "spec": spec,
    }))
    .expect("a variant is a manifest")
}

fn settings() -> Settings {
    Settings {
        host: "portal.liptov.example".into(),
        apex: "liptov.example".into(),
        gateway_url: Some("http://context-gateway.jc.svc.cluster.local:8080".into()),
        service_url: Some("http://portal.jc.svc.cluster.local:8080".into()),
        namespace: "jc".into(),
        org_domain: "liptov.sk".into(),
        apisix_namespace: "apisix".into(),
        image_repository: None,
        pull_secret: None,
        basemap_base: None,
        release: Some("dev".into()),
        service_account: Some("portal".into()),
    }
}

/// Who calls the App's endpoint: nobody signed in, or a person whose token carries `roles` for
/// the App's own client (ADR-N-030).
#[derive(Debug, Clone, Copy)]
enum Persona {
    Anonymous,
    Member,
    Viewer,
    Editor,
}

impl Persona {
    const ALL: [Persona; 4] = [
        Persona::Anonymous,
        Persona::Member,
        Persona::Viewer,
        Persona::Editor,
    ];

    fn client_roles(self) -> Vec<String> {
        match self {
            Persona::Viewer => vec!["viewer".into()],
            Persona::Editor => vec!["editor".into()],
            Persona::Anonymous | Persona::Member => Vec::new(),
        }
    }

    fn person(self) -> Option<AppPerson> {
        let roles = self.client_roles();
        (!matches!(self, Persona::Anonymous)).then(|| AppPerson {
            identity: Identity {
                subject: "sub-jana".into(),
                username: "jana".into(),
                email: Some("jana@liptov.sk".into()),
                name: Some("Jana".into()),
                roles: Vec::new(),
                groups: Vec::new(),
                client: None,
                second_factor: false,
            },
            roles,
        })
    }
}

/// The subject the gateway decides for on the App's endpoint, or `None` when the endpoint does
/// not admit the caller at all: an anonymous request reaches only a public endpoint (EP-16, GW22).
fn subject(persona: Persona, endpoint: &EndpointSpec, roles: &EndpointRoles) -> Option<Subject> {
    let client_roles = persona.client_roles();
    let mut subject = match persona {
        Persona::Anonymous => {
            if !matches!(endpoint.audience, jc_core::kinds::Audience::Public) {
                return None;
            }
            Subject::anonymous()
        }
        _ => Subject {
            user: Some("jana@liptov.sk".into()),
            ..Subject::default()
        },
    };
    let held: Vec<String> = roles
        .held_by(subject.user.as_deref(), &subject.groups, &client_roles)
        .map(str::to_owned)
        .collect();
    subject.roles.extend(held);
    Some(subject)
}

fn allowed(subject: &Subject, operation: Operation, policies: &[PolicySpec]) -> bool {
    let request = Request {
        types: BTreeSet::from([TYPE.to_owned()]),
        ..Request::default()
    };
    !evaluate(subject, operation, &request, SPACE, policies, Utc::now()).is_deny()
}

/// What the variant's access and roles say a persona may do; the gateway is held to this.
fn expected(variant: &Variant, persona: Persona, operation: Operation) -> bool {
    let reads = matches!(
        operation,
        Operation::QueryEntity | Operation::RetrieveEntity
    );
    if reads {
        return true;
    }
    let preset = operations(variant.access);
    let name = match operation {
        Operation::UpdateAttrs => "updateAttrs",
        Operation::CreateEntity => "createEntity",
        Operation::DeleteEntity => "deleteEntity",
        _ => return false,
    };
    preset.contains(&name) && (variant.visibility != "roles" || matches!(persona, Persona::Editor))
}

#[test]
fn every_variant_renders_and_the_gateway_decides_what_its_access_says() {
    let all = variants();
    assert_eq!(all.len(), 150);
    let mut decided = 0;
    for variant in &all {
        let label = variant.label();
        let app = manifest(variant);
        let spec: AppSpec = serde_json::from_value(app.spec.clone()).expect("the App parses");
        // The cells the platform refuses, each with its rule: a private App has no audience, so
        // it is never published (AP-18); `roles` is the static host's to enforce, which a
        // ui-rust App's requests never pass (AP-94).
        if let Some(rule) = refused_by(variant) {
            let refused = spec.validate().expect_err("a refused variant");
            assert!(refused.to_string().contains(rule), "{label}: {refused}");
            continue;
        }
        spec.validate()
            .unwrap_or_else(|err| panic!("{label}: the App validates: {err}"));

        // AP-04: only a `ui` App reads a further space, through its public endpoints; a pod
        // App naming one is refused before anything is rendered.
        let rendered = render(&app, Some(IMAGE), &generate_slug(), &settings());
        if variant.kind == "fullstack" && variant.endpoints == 2 {
            assert!(
                matches!(rendered, Err(RenderError::SeveralSpaces { .. })),
                "{label}: a pod App reads one space: {rendered:?}"
            );
            continue;
        }
        let rendered = rendered.unwrap_or_else(|err| panic!("{label}: renders: {err}"));
        assert_eq!(
            rendered.workload.is_some(),
            variant.kind == "fullstack",
            "{label}: only a ui-rust App runs a pod"
        );

        let (endpoint, policies) = grants(&app, &generate_slug(), "liptov.sk")
            .unwrap_or_else(|err| panic!("{label}: grants: {err}"));
        let endpoint_spec: EndpointSpec =
            serde_json::from_value(endpoint.spec.clone()).expect("the Endpoint parses");
        assert_eq!(
            matches!(endpoint_spec.audience, jc_core::kinds::Audience::Public),
            variant.visibility == "public",
            "{label}: only a public App's endpoint admits an anonymous caller"
        );
        // The further space's needs compile into nothing: its public endpoints answer the page.
        for policy in &policies {
            assert_eq!(
                policy.spec["contextSpaceRef"]["name"], SPACE,
                "{label}: a policy of the further space"
            );
        }
        let policy_specs: Vec<PolicySpec> = policies
            .iter()
            .map(|policy| {
                serde_json::from_value(policy.spec.clone())
                    .unwrap_or_else(|err| panic!("{label}: a Policy parses: {err}"))
            })
            .collect();
        let roles = EndpointRoles::of_app(
            PROJECT,
            &endpoint.metadata.name,
            &endpoint_spec,
            APP.to_owned(),
        );

        for persona in Persona::ALL {
            let Some(subject) = subject(persona, &endpoint_spec, &roles) else {
                assert_ne!(
                    variant.visibility, "public",
                    "{label}: {persona:?} is admitted"
                );
                continue;
            };
            for operation in [
                Operation::QueryEntity,
                Operation::RetrieveEntity,
                Operation::UpdateAttrs,
                Operation::CreateEntity,
                Operation::DeleteEntity,
            ] {
                assert_eq!(
                    allowed(&subject, operation, &policy_specs),
                    expected(variant, persona, operation),
                    "{label}: {persona:?} {operation:?}"
                );
                decided += 1;
            }
            // Nobody reaches another type through the App's grants: the gateway answers that
            // read with nothing, never with the type (T-0381, GW10).
            let elsewhere = Request {
                types: BTreeSet::from(["Person".to_owned()]),
                ..Request::default()
            };
            let verdict = evaluate(
                &subject,
                Operation::QueryEntity,
                &elsewhere,
                SPACE,
                &policy_specs,
                Utc::now(),
            );
            assert!(
                verdict
                    .constraints()
                    .is_none_or(|constraints| constraints.empty),
                "{label}: {persona:?} reads a type the App never named: {verdict:?}"
            );
        }
        // A caller the endpoint gives no role, however it got there, holds nothing.
        let stranger = Subject {
            user: Some("someone@elsewhere.sk".into()),
            ..Subject::default()
        };
        for operation in [Operation::QueryEntity, Operation::UpdateAttrs] {
            assert!(
                !allowed(&stranger, operation, &policy_specs),
                "{label}: a stranger {operation:?}"
            );
        }
    }
    assert!(decided > 1000, "{decided} decisions");
}

#[test]
fn who_opens_each_visibility_and_the_page_a_roles_app_refuses_with() {
    for visibility in VISIBILITIES {
        let variant = Variant {
            kind: "react",
            access: "update",
            endpoints: 1,
            visibility,
        };
        let spec: AppSpec =
            serde_json::from_value(manifest(&variant).spec).expect("the App parses");
        // Every persona is a member of the App's project; none is its owner (T-4910).
        let member = Standing {
            project_member: true,
            organization_member: true,
            ..Standing::default()
        };
        for persona in Persona::ALL {
            let opens = match visibility {
                "public" => true,
                "roles" => matches!(persona, Persona::Viewer | Persona::Editor),
                "private" => false,
                _ => !matches!(persona, Persona::Anonymous),
            };
            assert_eq!(
                admits(spec.visibility, persona.person().as_ref(), member),
                opens,
                "{visibility}: {persona:?} opens"
            );
        }
        if visibility == "roles" {
            let page = refusal(
                &spec,
                APP,
                None,
                PROJECT,
                &HeaderMap::new(),
                Some("https://portal.liptov.example"),
            );
            assert_eq!(
                page.status(),
                StatusCode::FORBIDDEN,
                "AP-93: a 403 page, not a 404"
            );
        }
    }
}

#[test]
fn the_edge_lets_an_anonymous_request_through_to_a_public_app_alone() {
    for visibility in VISIBILITIES {
        for kind in ["react", "fullstack", "wasm"] {
            let variant = Variant {
                kind,
                access: "read",
                endpoints: 1,
                visibility,
            };
            let app = manifest(&variant);
            let mirror = Mirror::new();
            mirror.upsert(ResourceEnvelope {
                api_version: app.api_version.clone(),
                kind: app.kind.clone(),
                metadata: serde_json::from_value(json!({ "name": APP, "namespace": PROJECT }))
                    .expect("metadata"),
                spec: app.spec.clone(),
                status: None,
            });
            let secrets =
                BTreeMap::from([(APP.to_owned(), ClientSecret::from("s-desk".to_owned()))]);
            let (apps, skipped) = edge_apps(&mirror, &secrets, &settings(), None);
            assert!(skipped.is_empty(), "{visibility} {kind}: {skipped:?}");
            assert_eq!(apps.len(), 1, "{visibility} {kind}");
            assert_eq!(
                apps[0].public,
                visibility == "public",
                "{visibility} {kind}: anonymous passes the edge"
            );
        }
    }
}

/// The guard: a class, a visibility or a preset with no variant fails here, so the matrix grows
/// with the platform (T-2705).
#[test]
fn every_class_visibility_and_preset_has_a_variant() {
    let schema = serde_json::to_value(schemars::schema_for!(AppSpec)).expect("the App schema");
    let enum_of = |name: &str| -> BTreeSet<String> {
        let definition = &schema["definitions"][name];
        let mut values = BTreeSet::new();
        for value in definition["enum"].as_array().into_iter().flatten() {
            values.extend(value.as_str().map(str::to_owned));
        }
        for one in definition["oneOf"].as_array().into_iter().flatten() {
            for value in one["enum"].as_array().into_iter().flatten() {
                values.extend(value.as_str().map(str::to_owned));
            }
            values.extend(one["const"].as_str().map(str::to_owned));
        }
        values
    };
    let visibilities = enum_of("AppVisibility");
    assert!(
        !visibilities.is_empty(),
        "the schema names the visibilities: {}",
        schema["definitions"]["AppVisibility"]
    );
    assert_eq!(
        visibilities,
        VISIBILITIES
            .iter()
            .map(|v| (*v).to_owned())
            .collect::<BTreeSet<_>>(),
        "a visibility without a variant, or a variant of none"
    );
    let classes = enum_of("AppClass");
    let covered: BTreeSet<String> = KINDS
        .iter()
        .map(|kind| {
            match *kind {
                "fullstack" => "ui-rust",
                "wasm" => "wasm",
                _ => "ui",
            }
            .to_owned()
        })
        .collect();
    for class in &classes {
        // `ui-node` is declared and not built yet (AP-125): no run starts one.
        if class == "ui-node" {
            continue;
        }
        assert!(covered.contains(class), "the class {class} has no variant");
    }
    let builder = include_str!("../ui/src/pages/apps/AppGenerator.tsx");
    let start = builder
        .find("export const ACCESS_PRESETS = {")
        .expect("the builder's presets");
    let block = &builder[start..start + builder[start..].find("} as const;").expect("their end")];
    let reads = builder
        .split("const OPERATIONS = [")
        .nth(1)
        .and_then(|rest| rest.split(']').next())
        .expect("the builder's OPERATIONS");
    for (preset, ops) in PRESETS {
        assert!(
            block.contains(&format!("{preset}:")),
            "the preset {preset} is no longer the builder's"
        );
        for op in ops {
            let quoted = format!("\"{op}\"");
            // Every preset starts from `...OPERATIONS`, the two reads every app makes.
            assert!(
                block.contains(&quoted)
                    || (block.contains("...OPERATIONS") && reads.contains(&quoted)),
                "the preset {preset} no longer carries {op}"
            );
        }
    }
    let presets_in_builder = block
        .lines()
        .filter(|line| line.trim_end().ends_with(": [") || line.contains(": [...OPERATIONS"))
        .count();
    assert_eq!(
        presets_in_builder,
        PRESETS.len(),
        "a preset of the builder has no variant:\n{block}"
    );
}
