//! A public dashboard reads only through public Endpoints (UI-19), checked where the three
//! manifests meet: the mirror. The view refuses to ask for such a layer's data too, so this is
//! what keeps a repository from ever holding the combination.

use serde_json::Value;

use crate::error::ApiError;
use crate::store::{ListOptions, Mirror};

fn is_public_dashboard(spec: &Value) -> bool {
    spec.get("visibility").and_then(Value::as_str) == Some("public")
}

/// The layers a dashboard's pages draw, by name.
pub fn layer_names(spec: &Value) -> Vec<String> {
    spec.get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|page| page.get("layers").and_then(Value::as_array))
        .flatten()
        .filter_map(|name| name.as_str().map(str::to_owned))
        .collect()
}

fn endpoint_of(layer: &Value) -> Option<&str> {
    layer.get("sourceEndpointRef").and_then(Value::as_str)
}

fn audience_of(endpoint: &Value) -> Option<&str> {
    endpoint.get("audience").and_then(Value::as_str)
}

/// Refuses a write that would leave a public dashboard reading through a non-public Endpoint:
/// the dashboard being published, a layer being pointed at another Endpoint, or an Endpoint
/// leaving the public audience while a public dashboard still draws from it.
pub fn check(
    mirror: &Mirror,
    project: &str,
    kind: &str,
    name: &str,
    spec: &Value,
) -> Result<(), ApiError> {
    let refuse = |detail: String| Err(ApiError::BadRequest(format!("{detail} (UI-19)")));
    match kind {
        "Dashboard" if is_public_dashboard(spec) => {
            for layer_name in layer_names(spec) {
                let Some(layer) = mirror.get(project, "Layer", &layer_name) else {
                    return refuse(format!("a public dashboard may only read public endpoints: layer {layer_name} does not exist"));
                };
                let endpoint = endpoint_of(&layer.spec).unwrap_or_default();
                let audience = mirror
                    .get(project, "Endpoint", endpoint)
                    .and_then(|e| audience_of(&e.spec).map(str::to_owned));
                if audience.as_deref() != Some("public") {
                    return refuse(format!(
                        "a public dashboard may only read public endpoints: layer {layer_name} reads {endpoint} whose audience is {}",
                        audience.as_deref().unwrap_or("unknown")
                    ));
                }
            }
            Ok(())
        }
        "Layer" => {
            let endpoint = endpoint_of(spec).unwrap_or_default();
            let public = mirror
                .get(project, "Endpoint", endpoint)
                .is_some_and(|e| audience_of(&e.spec) == Some("public"));
            if public {
                return Ok(());
            }
            match public_dashboards(mirror, project).into_iter().find(|(_, layers)| layers.iter().any(|l| l == name)) {
                Some((dashboard, _)) => refuse(format!(
                    "layer {name} is drawn by the public dashboard {dashboard}, so it may only read a public endpoint; {endpoint} is not"
                )),
                None => Ok(()),
            }
        }
        "Endpoint" if audience_of(spec) != Some("public") => {
            for (dashboard, layers) in public_dashboards(mirror, project) {
                for layer_name in layers {
                    let reads_it = mirror
                        .get(project, "Layer", &layer_name)
                        .is_some_and(|l| endpoint_of(&l.spec) == Some(name));
                    if reads_it {
                        return refuse(format!(
                            "endpoint {name} is read by layer {layer_name} of the public dashboard {dashboard}, so its audience stays public"
                        ));
                    }
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Every public dashboard of the project with the layers it draws.
fn public_dashboards(mirror: &Mirror, project: &str) -> Vec<(String, Vec<String>)> {
    mirror
        .list(project, "Dashboard", &ListOptions::default())
        .items
        .into_iter()
        .filter(|d| is_public_dashboard(&d.spec))
        .map(|d| (d.metadata.name.clone(), layer_names(&d.spec)))
        .collect()
}
