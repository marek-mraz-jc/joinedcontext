//! Public dashboards in the open-data catalogue (T-6790): each `visibility: public` Dashboard of a
//! project whose Endpoints are published goes to their catalogue as one package of
//! `type: dashboard`, as ckanext-showcase files a `showcase`.
//!
//! - The package carries jcctl's `generated_by` mark, so the organization sweep withdraws it with
//!   the dashboard ([`super::ckan::sweep_with`]), and is written only over a package of that mark.
//! - It has one resource per Endpoint its widgets read, and one `jc_visualization` view per
//!   `vega-lite` widget on its Endpoint's resource: the viz host draws it exactly as a dataset
//!   chart, reading the rows from the Endpoint itself (ADR-N-053 §2.2).
//! - `jc_dashboard` holds the layout the catalogue page draws (pages, chart views, grids) and
//!   `jc_endpoints` the Endpoint addresses, by which a dataset page finds the dashboards that read
//!   it. A widget whose Endpoint is not published this pass is left out: UI-19 keeps a public
//!   dashboard on public Endpoints, so that is an Endpoint held back, never a private one.

use jc_core::kinds::dashboard::{
    DashboardSpec, DashboardVisibility, GRID_WIDGET, VEGA_LITE_WIDGET,
};
use jcctl::commands::publish_ckan::Target;
use jcctl::loader::Repository;
use jcctl::publish::ckan::{CkanApi, Outcome, Settings, GENERATOR, VISUALIZATION_VIEW};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The CKAN package type a dashboard is published as.
pub const DASHBOARD_TYPE: &str = "dashboard";
/// The extra holding the layout the catalogue page draws.
pub const LAYOUT_EXTRA: &str = "jc_dashboard";
/// The extra holding the addresses of the Endpoints the dashboard reads, space-separated.
pub const ENDPOINTS_EXTRA: &str = "jc_endpoints";
/// The longest package name CKAN takes.
const MAX_NAME: usize = 100;

/// One public dashboard and the publication of its project it is published beside.
#[derive(Debug, Clone)]
pub struct DashboardTarget {
    pub project: String,
    pub name: String,
    pub spec: DashboardSpec,
    /// A published Endpoint the dashboard reads: its catalogue, organization and token.
    pub anchor: Target,
    /// The published Endpoints the dashboard reads, by name, with their slugs.
    pub endpoints: BTreeMap<String, String>,
}

impl DashboardTarget {
    /// The package name: `{project}-{name}`, so two projects of one organization never meet.
    pub fn package_name(&self) -> String {
        format!("{}-{}", self.project, self.name)
    }

    /// The CKAN organization it is filed under: its anchor's.
    pub fn organization(&self) -> Option<String> {
        self.anchor
            .publication
            .organization
            .clone()
            .or_else(|| self.anchor.instance.organization_default.clone())
    }
}

/// Every public dashboard of the repository that reads at least one Endpoint `wanted` publishes
/// (after routing). A dashboard that reads none has nothing a catalogue could draw.
pub fn targets(repo: &Repository, wanted: &[Target]) -> Vec<DashboardTarget> {
    let mut out = Vec::new();
    for (id, resource) in repo.iter() {
        if id.kind != "Dashboard" {
            continue;
        }
        let Some(project) = id.namespace.clone() else {
            continue;
        };
        // The repository validated it on write; one that does not read is not published.
        let Ok(spec) = serde_json::from_value::<DashboardSpec>(resource.manifest.spec.clone())
        else {
            continue;
        };
        if spec.visibility != DashboardVisibility::Public {
            continue;
        }
        let published = |endpoint: &str| {
            wanted.iter().find(|t| {
                t.id.namespace.as_deref() == Some(project.as_str())
                    && t.id.name == endpoint
                    && t.slice.is_none()
            })
        };
        let mut endpoints = BTreeMap::new();
        let mut anchor = None;
        for widget in spec.pages.iter().flat_map(|p| &p.widgets) {
            let Some(target) = widget.endpoint_ref.as_deref().and_then(published) else {
                continue;
            };
            endpoints.insert(target.id.name.clone(), target.slug.clone());
            anchor.get_or_insert_with(|| target.clone());
        }
        if let Some(anchor) = anchor {
            out.push(DashboardTarget {
                project,
                name: id.name.clone(),
                spec,
                anchor,
                endpoints,
            });
        }
    }
    out
}

/// What the catalogue should hold for a dashboard: the package, and each chart view with the
/// slug of the Endpoint whose resource it hangs off.
#[derive(Debug, Clone, PartialEq)]
pub struct Desired {
    pub package: Value,
    pub views: Vec<(String, Value)>,
}

/// The package and views of `dashboard`, filed under `organization`.
pub fn desired(
    dashboard: &DashboardTarget,
    organization: &str,
    settings: &Settings,
) -> Result<Desired, String> {
    let name = dashboard.package_name();
    if name.len() > MAX_NAME {
        return Err(format!(
            "the catalogue name {name} is longer than {MAX_NAME} characters: shorten the project or dashboard name"
        ));
    }
    let preferred: Vec<String> = settings.language.iter().cloned().collect();
    let title = dashboard.spec.title.resolve(&preferred, "en").to_owned();
    let text = |value: &Value| serde_json::to_string(value).map_err(|e| e.to_string());
    let mut pages = Vec::new();
    let mut views = Vec::new();
    for (p, page) in dashboard.spec.pages.iter().enumerate() {
        let mut items = Vec::new();
        for (w, widget) in page.widgets.iter().enumerate() {
            let Some(slug) = widget
                .endpoint_ref
                .as_deref()
                .and_then(|e| dashboard.endpoints.get(e))
            else {
                continue;
            };
            match widget.widget_type.as_str() {
                VEGA_LITE_WIDGET => {
                    let (Some(data), Some(spec)) = (&widget.data, &widget.spec) else {
                        continue;
                    };
                    let jc_name = format!("p{p}w{w}");
                    let chart_title = spec
                        .get("title")
                        .and_then(|t| t.as_str().or_else(|| t.get("text")?.as_str()))
                        .map_or_else(|| format!("{title} {}", views.len() + 1), str::to_owned);
                    let description = spec
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    views.push((
                        slug.clone(),
                        json!({
                            "view_type": VISUALIZATION_VIEW,
                            "title": chart_title,
                            "description": description,
                            "jc_name": jc_name,
                            "spec": text(spec)?,
                            "source": text(&serde_json::to_value(data).map_err(|e| e.to_string())?)?,
                            "endpoint": slug,
                        }),
                    ));
                    items.push(json!({ "view": jc_name }));
                }
                GRID_WIDGET => {
                    let Some(kind) = widget.entity_type.as_deref() else {
                        continue;
                    };
                    items.push(json!({ "grid": slug, "type": kind }));
                }
                // The Portal's own charts read with the reader's session; the catalogue draws
                // what the viz host can (Vega-Lite and the grid).
                _ => {}
            }
        }
        if !items.is_empty() {
            pages
                .push(json!({ "title": page.title.clone().unwrap_or_default(), "widgets": items }));
        }
    }
    let urls: Vec<String> = dashboard
        .endpoints
        .values()
        .map(|slug| settings.endpoint_url(slug))
        .collect();
    let resources: Vec<Value> = dashboard
        .endpoints
        .iter()
        .map(|(endpoint, slug)| {
            json!({ "name": endpoint, "url": settings.endpoint_url(slug), "format": "NGSI-LD" })
        })
        .collect();
    // DCAT-AP asks every dataset for a description; the catalogue refuses one without (SHACL).
    let read: Vec<&str> = dashboard.endpoints.keys().map(String::as_str).collect();
    let notes = format!(
        "{title}: charts and tables over the live data of {}.",
        read.join(", ")
    );
    let package = json!({
        "name": name,
        "title": title,
        "notes": notes,
        "type": DASHBOARD_TYPE,
        "owner_org": organization,
        "extras": [
            { "key": "generated_by", "value": GENERATOR },
            { "key": ENDPOINTS_EXTRA, "value": urls.join(" ") },
            { "key": LAYOUT_EXTRA, "value": text(&json!({ "pages": pages }))? },
        ],
        "resources": resources,
    });
    Ok(Desired { package, views })
}

/// Brings the catalogue to `desired`: the package, then its chart views by `jc_name`. Idempotent:
/// an unchanged dashboard makes no writing call. A package of that name the platform did not
/// publish is never written over.
pub fn publish(api: &mut impl CkanApi, desired: &Desired) -> Result<Outcome, String> {
    let name = desired.package["name"].as_str().unwrap_or_default();
    let err = |e: jcctl::publish::ckan::CkanError| e.to_string();
    let mut outcome = match api.show("package_show", name).map_err(err)? {
        None => {
            api.action("package_create", &desired.package)
                .map_err(err)?;
            Outcome::Created
        }
        Some(live) if extra(&live, "generated_by") != Some(GENERATOR) => {
            return Err(format!(
                "the catalogue already holds a dataset named {name} that the platform did not publish: rename the dashboard"
            ));
        }
        Some(live) if !deleted(&live) && same(&live, &desired.package) => Outcome::Unchanged,
        Some(live) => {
            let mut payload = desired.package.clone();
            payload["id"] = live["id"].clone();
            payload["state"] = json!("active");
            // A resource keeps its id, and with it its views, while its Endpoint stays.
            for resource in payload["resources"].as_array_mut().into_iter().flatten() {
                if let Some(id) = resources(&live)
                    .find(|r| r["url"] == resource["url"])
                    .and_then(|r| r.get("id"))
                {
                    resource["id"] = id.clone();
                }
            }
            api.action("package_update", &payload).map_err(err)?;
            Outcome::Updated
        }
    };
    let Some(live) = api.show("package_show", name).map_err(err)? else {
        return Ok(outcome);
    };
    if sync_views(api, &live, &desired.views).map_err(err)? && outcome == Outcome::Unchanged {
        outcome = Outcome::Updated;
    }
    Ok(outcome)
}

/// The views on the package's resources made what `views` says; whether anything was written.
fn sync_views(
    api: &mut impl CkanApi,
    live: &Value,
    views: &[(String, Value)],
) -> Result<bool, jcctl::publish::ckan::CkanError> {
    let url_of = |slug: &str| format!("/api/endpoint/{slug}");
    let mut wanted: BTreeMap<(String, String), Value> = BTreeMap::new();
    for (slug, view) in views {
        let Some(resource) = resources(live).find(|r| {
            r["url"]
                .as_str()
                .is_some_and(|u| u.ends_with(&url_of(slug)))
        }) else {
            continue;
        };
        let id = resource["id"].as_str().unwrap_or_default().to_owned();
        let mut view = view.clone();
        view["resource_id"] = json!(id);
        let jc_name = view["jc_name"].as_str().unwrap_or_default().to_owned();
        wanted.insert((id, jc_name), view);
    }
    let mut wrote = false;
    for resource in resources(live).filter(|r| r.get("has_views") != Some(&Value::Bool(false))) {
        let id = resource["id"].as_str().unwrap_or_default().to_owned();
        let held = api.show("resource_view_list", &id)?.unwrap_or_default();
        for view in held.as_array().into_iter().flatten() {
            if view["view_type"] != VISUALIZATION_VIEW {
                continue;
            }
            let key = (
                id.clone(),
                view["jc_name"].as_str().unwrap_or_default().to_owned(),
            );
            match wanted.remove(&key) {
                Some(want) if VIEW_FIELDS.iter().all(|f| view.get(*f) == want.get(*f)) => {}
                Some(mut want) => {
                    want["id"] = view["id"].clone();
                    api.action("resource_view_update", &want)?;
                    wrote = true;
                }
                None => {
                    api.action("resource_view_delete", &json!({ "id": view["id"] }))?;
                    wrote = true;
                }
            }
        }
    }
    for want in wanted.into_values() {
        api.action("resource_view_create", &want)?;
        wrote = true;
    }
    Ok(wrote)
}

/// What a chart view holds; an update is sent when one of them differs.
const VIEW_FIELDS: [&str; 5] = ["title", "description", "spec", "source", "endpoint"];

fn resources(package: &Value) -> impl Iterator<Item = &Value> {
    package["resources"].as_array().into_iter().flatten()
}

fn extra<'a>(package: &'a Value, key: &str) -> Option<&'a str> {
    package["extras"]
        .as_array()?
        .iter()
        .find(|e| e["key"] == key)?["value"]
        .as_str()
}

fn deleted(package: &Value) -> bool {
    package["state"] == "deleted"
}

/// Whether the live package already is what `desired` says: its own fields, each desired extra,
/// and its resources by name and address.
fn same(live: &Value, desired: &Value) -> bool {
    let pairs = |p: &Value| -> BTreeSet<(String, String)> {
        resources(p)
            .map(|r| {
                (
                    r["name"].as_str().unwrap_or_default().to_owned(),
                    r["url"].as_str().unwrap_or_default().to_owned(),
                )
            })
            .collect()
    };
    let owner = |p: &Value| {
        p["organization"]["name"]
            .as_str()
            .or_else(|| p["owner_org"].as_str())
            .map(str::to_owned)
    };
    ["name", "title", "notes", "type"]
        .iter()
        .all(|f| live[*f] == desired[*f])
        && owner(live) == owner(desired)
        && desired["extras"]
            .as_array()
            .into_iter()
            .flatten()
            .all(|e| extra(live, e["key"].as_str().unwrap_or_default()) == e["value"].as_str())
        && pairs(live) == pairs(desired)
}
