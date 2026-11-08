//! Which catalogue datasets a repository publishes (ADR-N-068, EP-174…EP-178).
//!
//! One function, [`datasets`], decides it for the publisher (`jcctl publish ckan`), for the
//! Portal reconciler and for `jcctl validate`, so a run from a shell, a reconcile and a plan
//! agree on every name. Every public Endpoint is one dataset; `spec.catalog.datasets` makes it
//! several; `harvest: false` and an App's own Endpoint make it none.
//!
//! The withdrawal is the difference of two calls: what the repository published before a change
//! and what it publishes after, keyed by `(catalogue, dataset name)` (EP-178), so removing one
//! entry withdraws that dataset alone and a name moving between Endpoints is never withdrawn.

use crate::loader::Repository;
use jc_core::envelope::Ref;
use jc_core::kinds::catalog_datasets::CatalogDataset;
use jc_core::kinds::ckan::{CkanInstanceSpec, CkanPublication};
use jc_core::kinds::{Audience, EndpointSpec};
use std::collections::{BTreeMap, BTreeSet};

/// The name the installation's own catalogue goes by in a [`Planned`] dataset: no `CkanInstance`
/// of a project carries it, since a DNS-1123 name holds no `:`.
pub const INSTALLATION: &str = "installation:catalogue";

/// One Endpoint as the planner reads it.
#[derive(Debug, Clone, Copy)]
pub struct Door<'a> {
    /// Its project.
    pub project: &'a str,
    /// Its `metadata.name`.
    pub name: &'a str,
    /// Its spec.
    pub spec: &'a EndpointSpec,
}

/// What the planner knows besides the Endpoints.
#[derive(Debug, Clone, Default)]
pub struct Catalogues {
    /// Every project's `CkanInstance`s, by project, then name.
    pub instances: BTreeMap<String, BTreeMap<String, CkanInstanceSpec>>,
    /// The installation's own catalogue, when the Portal is configured with one (EP-174).
    pub installation: Option<CkanInstanceSpec>,
    /// The `App`s of each project: `app-{name}` is that App's own door and no dataset.
    pub apps: BTreeSet<(String, String)>,
}

/// One dataset the repository publishes.
#[derive(Debug, Clone, PartialEq)]
pub struct Planned {
    /// The Endpoint's project.
    pub project: String,
    /// The Endpoint's name.
    pub endpoint: String,
    /// The `CkanInstance` it lands on, [`INSTALLATION`] for the installation's catalogue.
    pub instance_name: String,
    /// That instance.
    pub instance: CkanInstanceSpec,
    /// The publication block: the Endpoint's own, or the one a public Endpoint reads as.
    pub publication: CkanPublication,
    /// The CKAN dataset name.
    pub name: String,
    /// The slice of `spec.catalog.datasets` it is; `None` for the Endpoint's one dataset.
    pub slice: Option<CatalogDataset>,
}

impl Planned {
    /// The catalogue it lands in: the instance's URL, lowercased, so two `CkanInstance`s on one
    /// CKAN are one catalogue (T-6079).
    pub fn catalogue(&self) -> String {
        self.instance.base_url().to_ascii_lowercase()
    }

    /// `(catalogue, name)`: what one catalogue holds once (EP-178).
    pub fn key(&self) -> (String, String) {
        (self.catalogue(), self.name.clone())
    }
}

/// An Endpoint that would be a dataset and is not, with why (EP-174): one line of a report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unplanned {
    /// The Endpoint's project.
    pub project: String,
    /// The Endpoint's name.
    pub endpoint: String,
    /// Why, with the fix.
    pub reason: String,
}

/// The datasets `doors` publish, in their order, and the Endpoints that would and cannot.
pub fn datasets<'a>(
    doors: impl IntoIterator<Item = Door<'a>>,
    catalogues: &Catalogues,
) -> (Vec<Planned>, Vec<Unplanned>) {
    let mut planned = Vec::new();
    let mut unplanned = Vec::new();
    for door in doors {
        let refuse = |reason: String| Unplanned {
            project: door.project.to_owned(),
            endpoint: door.name.to_owned(),
            reason,
        };
        let spec = door.spec;
        let catalog = spec.catalog.as_ref();
        if catalog.is_some_and(|c| !c.harvested())
            || catalogues
                .apps
                .contains(&(door.project.to_owned(), app_of(door.name)))
        {
            continue;
        }
        let declared = spec.publish.as_ref().and_then(|p| p.ckan.clone());
        // A closed Endpoint is published only when it says so; then as a private dataset (EP-69).
        if declared.is_none() && spec.audience != Audience::Public {
            continue;
        }
        let instances = catalogues.instances.get(door.project);
        let (instance_name, instance) = match &declared {
            Some(publication) => {
                // T-6292: never another project's instance, whose token and catalogue are not ours.
                if let Ref::Typed(typed) = &publication.instance_ref {
                    if typed
                        .namespace
                        .as_deref()
                        .is_some_and(|n| n != door.project)
                    {
                        unplanned.push(refuse(
                            "publish.ckan.instanceRef names another project's CkanInstance: an \
                             Endpoint publishes only through one of its own project"
                                .to_owned(),
                        ));
                        continue;
                    }
                }
                let name = publication.instance_ref.name();
                match instances.and_then(|all| all.get(name)) {
                    Some(instance) => (name.to_owned(), instance.clone()),
                    None => {
                        unplanned.push(refuse(format!(
                            "publish.ckan.instanceRef names CkanInstance {name}, which project {} \
                             does not declare",
                            door.project
                        )));
                        continue;
                    }
                }
            }
            None => match instances.map(BTreeMap::len).unwrap_or_default() {
                0 => match &catalogues.installation {
                    Some(instance) => (INSTALLATION.to_owned(), instance.clone()),
                    // An installation without a catalogue publishes nothing, and says nothing.
                    None => continue,
                },
                1 => {
                    let Some((name, instance)) = instances.and_then(|all| all.iter().next()) else {
                        continue;
                    };
                    (name.clone(), instance.clone())
                }
                _ => {
                    let names: Vec<&str> = instances
                        .into_iter()
                        .flat_map(|all| all.keys().map(String::as_str))
                        .collect();
                    unplanned.push(refuse(format!(
                        "not published: project {} holds the CkanInstances {}, so name the one \
                         this Endpoint publishes to in spec.publish.ckan.instanceRef (EP-174)",
                        door.project,
                        names.join(", ")
                    )));
                    continue;
                }
            },
        };
        let publication = declared.unwrap_or_else(|| CkanPublication {
            instance_ref: Ref::Name(instance_name.clone()),
            organization: None,
            name: None,
            datastore: None,
            license: None,
        });
        let slices = catalog.map(|c| c.datasets.as_slice()).unwrap_or_default();
        let one = |name: String, slice: Option<CatalogDataset>| Planned {
            project: door.project.to_owned(),
            endpoint: door.name.to_owned(),
            instance_name: instance_name.clone(),
            instance: instance.clone(),
            publication: publication.clone(),
            name,
            slice,
        };
        if slices.is_empty() {
            planned.push(one(publication.dataset_name(door.name).to_owned(), None));
        } else {
            planned.extend(
                slices
                    .iter()
                    .map(|slice| one(slice.name.clone(), Some(slice.clone()))),
            );
        }
    }
    (planned, unplanned)
}

/// The App an Endpoint name would be the own door of: `app-{name}` reads `{name}`.
fn app_of(endpoint: &str) -> String {
    endpoint.strip_prefix("app-").unwrap_or_default().to_owned()
}

/// Every planned dataset whose `(catalogue, name)` another Endpoint's dataset also claims, by
/// its index in `planned`, with the reason it is refused: the other Endpoints and the fix
/// (T-6079, EP-177). Two entries of one Endpoint are refused by the Endpoint's own validation.
pub fn claimed_twice(planned: &[Planned]) -> BTreeMap<usize, String> {
    let mut claimants: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    for (index, dataset) in planned.iter().enumerate() {
        claimants.entry(dataset.key()).or_default().push(index);
    }
    let mut refused = BTreeMap::new();
    for ((catalogue, name), indexes) in claimants {
        for &index in &indexes {
            let mine = &planned[index];
            let others: BTreeSet<String> = indexes
                .iter()
                .map(|&other| &planned[other])
                .filter(|other| {
                    (&other.project, &other.endpoint) != (&mine.project, &mine.endpoint)
                })
                .map(|other| format!("{}/{}", other.project, other.endpoint))
                .collect();
            if others.is_empty() {
                continue;
            }
            let field = if mine.slice.is_some() {
                "the spec.catalog.datasets entry"
            } else {
                "spec.publish.ckan.name"
            };
            refused.insert(
                index,
                format!(
                    "not published: the CKAN dataset `{name}` on {catalogue} is also what \
                     Endpoint {} publishes, and one catalogue holds one dataset of a name, so \
                     neither is written; give {field} a name of its own, such as `{}-{name}`",
                    others.into_iter().collect::<Vec<_>>().join(", "),
                    mine.project,
                ),
            );
        }
    }
    refused
}

/// Every type a slice names that its Endpoint does not serve, as `(slice name, type)`, given the
/// types the Endpoint serves (EP-177).
pub fn unserved_types<'a>(
    spec: &'a EndpointSpec,
    served: &BTreeSet<String>,
) -> Vec<(&'a str, &'a str)> {
    spec.catalog
        .iter()
        .flat_map(|c| &c.datasets)
        .flat_map(|slice| {
            slice
                .types
                .iter()
                .filter(|kind| !served.contains(kind.as_str()))
                .map(|kind| (slice.name.as_str(), kind.as_str()))
        })
        .collect()
}

/// [`datasets`] over a loaded repository; `installation` is the installation's catalogue, when
/// the caller has one. An Endpoint or instance whose spec does not parse is left to the rest of
/// validation, which names it.
pub fn of_repository(
    repo: &Repository,
    installation: Option<CkanInstanceSpec>,
) -> (Vec<Planned>, Vec<Unplanned>) {
    let mut catalogues = Catalogues {
        installation,
        ..Catalogues::default()
    };
    let mut endpoints = Vec::new();
    for (id, resource) in repo.iter() {
        let project = id.namespace.clone().unwrap_or_default();
        match id.kind.as_str() {
            "CkanInstance" => {
                if let Ok(spec) = serde_json::from_value(resource.manifest.spec.clone()) {
                    catalogues
                        .instances
                        .entry(project)
                        .or_default()
                        .insert(id.name.clone(), spec);
                }
            }
            "App" => {
                catalogues.apps.insert((project, id.name.clone()));
            }
            "Endpoint" => {
                if let Ok(spec) =
                    serde_json::from_value::<EndpointSpec>(resource.manifest.spec.clone())
                {
                    endpoints.push((project, id.name.clone(), spec));
                }
            }
            _ => {}
        }
    }
    datasets(
        endpoints.iter().map(|(project, name, spec)| Door {
            project,
            name,
            spec,
        }),
        &catalogues,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn endpoint(
        audience: &str,
        catalog: serde_json::Value,
        publish: Option<serde_json::Value>,
    ) -> EndpointSpec {
        let mut spec = json!({
            "contextSpaceRef": {"kind": "ContextSpace", "name": "schools"},
            "slug": "zt4qm7ge2xdv6ksb3ncf5arw2y",
            "audience": audience,
            "catalog": catalog,
        });
        if let Some(publish) = publish {
            spec["publish"] = publish;
        }
        serde_json::from_value(spec).expect("an Endpoint spec")
    }

    fn instance(url: &str) -> CkanInstanceSpec {
        serde_json::from_value(json!({
            "url": url,
            "organizationDefault": "bbsk",
            "apiTokenRef": {"name": "ckan", "key": "apiToken"},
        }))
        .expect("an instance")
    }

    fn slice(name: &str, kind: &str) -> serde_json::Value {
        json!({"name": name, "title": {"sk": name}, "description": {"sk": name}, "types": [kind]})
    }

    fn catalogues(instances: &[(&str, &str, &str)]) -> Catalogues {
        let mut out = Catalogues::default();
        for (project, name, url) in instances {
            out.instances
                .entry((*project).to_owned())
                .or_default()
                .insert((*name).to_owned(), instance(url));
        }
        out
    }

    fn plan(
        doors: &[(&str, &str, &EndpointSpec)],
        catalogues: &Catalogues,
    ) -> (Vec<Planned>, Vec<Unplanned>) {
        datasets(
            doors.iter().map(|(project, name, spec)| Door {
                project,
                name,
                spec,
            }),
            catalogues,
        )
    }

    fn names(planned: &[Planned]) -> Vec<&str> {
        planned.iter().map(|p| p.name.as_str()).collect()
    }

    #[test]
    fn a_public_endpoint_is_one_dataset_without_a_block() {
        let spec = endpoint("public", json!({}), None);
        let (planned, unplanned) = plan(
            &[("bbsk", "skoly", &spec)],
            &catalogues(&[("bbsk", "ckan", "https://opendata.example.org")]),
        );
        assert_eq!(names(&planned), ["skoly"]);
        assert_eq!(planned[0].instance_name, "ckan");
        assert!(unplanned.is_empty());
    }

    #[test]
    fn a_closed_endpoint_or_one_out_of_the_feeds_is_no_dataset() {
        let closed = endpoint("organization", json!({}), None);
        let out = endpoint("public", json!({"harvest": false}), None);
        let app = endpoint("public", json!({}), None);
        let mut known = catalogues(&[("bbsk", "ckan", "https://opendata.example.org")]);
        known.apps.insert(("bbsk".to_owned(), "map".to_owned()));
        let (planned, unplanned) = plan(
            &[
                ("bbsk", "closed", &closed),
                ("bbsk", "out", &out),
                ("bbsk", "app-map", &app),
            ],
            &known,
        );
        assert!(planned.is_empty(), "{planned:?}");
        assert!(unplanned.is_empty());
    }

    #[test]
    fn a_closed_endpoint_that_declares_its_publication_keeps_it() {
        let spec = endpoint(
            "organization",
            json!({}),
            Some(json!({"ckan": {"instanceRef": {"kind": "CkanInstance", "name": "ckan"}}})),
        );
        let (planned, _) = plan(
            &[("bbsk", "interne", &spec)],
            &catalogues(&[("bbsk", "ckan", "https://opendata.example.org")]),
        );
        assert_eq!(names(&planned), ["interne"]);
    }

    #[test]
    fn datasets_replace_the_one_dataset() {
        let spec = endpoint(
            "public",
            json!({"datasets": [slice("bbsk-skoly", "School"), slice("bbsk-lekarne", "Pharmacy")]}),
            None,
        );
        let (planned, _) = plan(
            &[("bbsk", "regions", &spec)],
            &catalogues(&[("bbsk", "ckan", "https://opendata.example.org")]),
        );
        assert_eq!(names(&planned), ["bbsk-skoly", "bbsk-lekarne"]);
        assert!(planned.iter().all(|p| p.slice.is_some()));
    }

    #[test]
    fn the_installation_catalogue_takes_a_project_without_one() {
        let spec = endpoint("public", json!({}), None);
        let known = Catalogues {
            installation: Some(instance("https://opendata.example.org")),
            ..Catalogues::default()
        };
        let (planned, _) = plan(&[("bbsk", "skoly", &spec)], &known);
        assert_eq!(planned[0].instance_name, INSTALLATION);
        let (none, unplanned) = plan(&[("bbsk", "skoly", &spec)], &Catalogues::default());
        assert!(none.is_empty() && unplanned.is_empty());
    }

    #[test]
    fn two_instances_and_no_choice_publish_nothing_and_say_why() {
        let spec = endpoint("public", json!({}), None);
        let (planned, unplanned) = plan(
            &[("bbsk", "skoly", &spec)],
            &catalogues(&[
                ("bbsk", "a", "https://a.example.org"),
                ("bbsk", "b", "https://b.example.org"),
            ]),
        );
        assert!(planned.is_empty());
        assert!(
            unplanned[0]
                .reason
                .contains("spec.publish.ckan.instanceRef"),
            "{unplanned:?}"
        );
    }

    #[test]
    fn a_name_two_endpoints_claim_in_one_catalogue_is_refused_naming_the_other() {
        let whole = endpoint(
            "public",
            json!({"datasets": [slice("skoly", "School")]}),
            None,
        );
        let split = endpoint("public", json!({}), None);
        let known = catalogues(&[
            ("bbsk", "ckan", "https://opendata.example.org"),
            ("mesto", "ckan", "https://OPENDATA.example.org"),
        ]);
        let (planned, _) = plan(
            &[("bbsk", "regions", &whole), ("mesto", "skoly", &split)],
            &known,
        );
        let refused = claimed_twice(&planned);
        assert_eq!(refused.len(), 2);
        assert!(refused[&0].contains("mesto/skoly"), "{refused:?}");
        assert!(refused[&1].contains("bbsk/regions"), "{refused:?}");
        // Another catalogue holds its own names.
        let apart = catalogues(&[
            ("bbsk", "ckan", "https://opendata.example.org"),
            ("mesto", "ckan", "https://data.mesto.example"),
        ]);
        let (planned, _) = plan(
            &[("bbsk", "regions", &whole), ("mesto", "skoly", &split)],
            &apart,
        );
        assert!(claimed_twice(&planned).is_empty());
    }

    #[test]
    fn a_type_the_endpoint_does_not_serve_is_named() {
        let spec = endpoint(
            "public",
            json!({"datasets": [slice("skoly", "School"), slice("tajne", "Secret")]}),
            None,
        );
        let served = BTreeSet::from(["School".to_owned()]);
        assert_eq!(unserved_types(&spec, &served), [("tajne", "Secret")]);
    }
}
