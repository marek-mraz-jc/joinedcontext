//! The catalogue's dashboards (T-6790): `/dashboards` lists the packages of `type: dashboard`
//! the Portal publishes for each public Dashboard, `/dashboards/{name}` draws one. Its charts are
//! the package's `jc_visualization` views in the viz host's frames, exactly as a dataset's charts;
//! its tables are the viz host's explorer of the widget's Endpoint. `jc_dashboard` (the layout) and
//! `jc_endpoints` come from the catalogue, so both are read as untrusted: a slug or a type that
//! does not match its rule is dropped, never put into a URL.

use crate::helpers::{jc_visualizations, viz_frame, ChartView};
use crate::i18n::{jc_t, normalize_lang_code};
use crate::{PageChrome, UiLabels};
use askama::Template;
use dataroom_core::branding::Address;
use dataroom_core::{localized_text, BrandingBlock, Catalogue, Dataset, Publisher, ResourceView};
use serde::Deserialize;

/// The CKAN package type of a dashboard.
pub const DASHBOARD_TYPE: &str = "dashboard";
/// Most pages and widgets per page a dashboard page draws.
const MAX_ITEMS: usize = 50;

/// Whether a package is a dashboard rather than a dataset.
pub fn is_dashboard(ds: &Dataset) -> bool {
    ds.dataset_type == DASHBOARD_TYPE
}

/// The Endpoint address a dataset is published from, without a trailing slash.
fn endpoint_of(ds: &Dataset) -> Option<&str> {
    ds.endpoint_url
        .as_deref()
        .or_else(|| ds.extras.get("endpoint").map(String::as_str))
        .map(|e| e.trim_end_matches('/'))
        .filter(|e| !e.is_empty())
}

/// Whether `dashboard` reads the Endpoint `dataset` is published from.
pub fn reads(dashboard: &Dataset, dataset: &Dataset) -> bool {
    let Some(endpoint) = endpoint_of(dataset) else {
        return false;
    };
    dashboard.extras.get("jc_endpoints").is_some_and(|all| {
        all.split_whitespace()
            .any(|e| e.trim_end_matches('/') == endpoint)
    })
}

/// An Endpoint slug as the gateway mints it: 16 to 64 lowercase letters and digits.
fn is_slug(s: &str) -> bool {
    (16..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// An entity type name (`[A-Za-z][A-Za-z0-9_]{0,127}`).
fn is_type_name(s: &str) -> bool {
    s.len() <= 128
        && s.bytes().next().is_some_and(|b| b.is_ascii_alphabetic())
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

#[derive(Debug, Default, Deserialize)]
struct Layout {
    #[serde(default)]
    pages: Vec<LayoutPage>,
}

#[derive(Debug, Default, Deserialize)]
struct LayoutPage {
    #[serde(default)]
    title: String,
    #[serde(default)]
    widgets: Vec<LayoutWidget>,
}

#[derive(Debug, Default, Deserialize)]
struct LayoutWidget {
    view: Option<String>,
    grid: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

/// A table of a dashboard: the explorer of one Endpoint's type.
#[derive(Debug, Clone)]
pub struct GridFrame {
    pub id: String,
    pub title: String,
    pub src: String,
    pub origin: String,
    pub sandbox: String,
}

/// One widget: a chart or a table.
#[derive(Debug, Clone)]
pub struct DashboardItem {
    pub chart: Option<ChartView>,
    pub grid: Option<GridFrame>,
}

/// One page of a dashboard, drawn as one section.
#[derive(Debug, Clone)]
pub struct DashboardSection {
    pub id: String,
    pub title: String,
    pub items: Vec<DashboardItem>,
}

/// A dataset a dashboard reads, linked from its page.
#[derive(Debug, Clone)]
pub struct DatasetLink {
    pub title: String,
    pub url: String,
}

/// The sections of a dashboard page, from its layout and its views. A chart's "as a table" link
/// goes to the dataset of its Endpoint, when the catalogue holds it.
pub fn dashboard_sections(
    dashboard: &Dataset,
    views: &[ResourceView],
    datasets: &[Dataset],
    branding: &BrandingBlock,
    lang: &str,
    lang_prefix: &str,
) -> Vec<DashboardSection> {
    let layout: Layout = dashboard
        .extras
        .get("jc_dashboard")
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or_default();
    let charts = jc_visualizations(dashboard, views, branding, lang, lang_prefix);
    let dataset_of = |slug: &str| {
        let tail = format!("/api/endpoint/{slug}");
        datasets
            .iter()
            .find(|d| !is_dashboard(d) && endpoint_of(d).is_some_and(|e| e.ends_with(&tail)))
    };
    let viz = branding.address(Address::Viz);
    let code = normalize_lang_code(lang);
    let mut sections = Vec::new();
    for (p, page) in layout.pages.iter().take(MAX_ITEMS).enumerate() {
        let mut items = Vec::new();
        for (w, widget) in page.widgets.iter().take(MAX_ITEMS).enumerate() {
            if let Some(jc_name) = &widget.view {
                let view = views.iter().find(|v| {
                    v.config.get("jc_name").and_then(|n| n.as_str()) == Some(jc_name.as_str())
                });
                let chart = view.and_then(|v| charts.iter().find(|c| c.id == v.id));
                if let (Some(view), Some(chart)) = (view, chart) {
                    let mut chart = chart.clone();
                    chart.package_link = view
                        .config
                        .get("endpoint")
                        .and_then(|s| s.as_str())
                        .filter(|s| is_slug(s))
                        .and_then(dataset_of)
                        .map(|d| format!("{lang_prefix}/dataset/{}#jc-preview-title", d.name))
                        .unwrap_or_default();
                    items.push(DashboardItem {
                        chart: Some(chart),
                        grid: None,
                    });
                }
            } else if let (Some(slug), Some(kind), Some(viz)) = (&widget.grid, &widget.kind, &viz) {
                if !is_slug(slug) || !is_type_name(kind) {
                    continue;
                }
                let (sandbox, origin) = viz_frame(
                    viz,
                    branding,
                    "allow-scripts allow-same-origin allow-downloads allow-popups allow-popups-to-escape-sandbox",
                );
                let mut src =
                    format!("{viz}/explore.html#{slug}&inline&lang={code}&theme=light&type={kind}");
                if let Some(d) = dataset_of(slug) {
                    src.push_str("&dataset=");
                    src.push_str(&d.name);
                }
                items.push(DashboardItem {
                    chart: None,
                    grid: Some(GridFrame {
                        id: format!("p{p}w{w}"),
                        title: format!("{}: {kind}", jc_t("dashboard_grid", lang)),
                        src,
                        origin,
                        sandbox,
                    }),
                });
            }
        }
        if !items.is_empty() {
            sections.push(DashboardSection {
                id: format!("jc-dashboard-page-{p}"),
                title: page.title.chars().take(200).collect(),
                items,
            });
        }
    }
    sections
}

/// A dashboard in the list.
#[derive(Debug, Clone)]
pub struct DashboardCard {
    pub title: String,
    pub url: String,
    pub publisher: String,
}

/// `/dashboards` (`templates/dashboard_list.html`).
#[derive(Template)]
#[template(path = "dashboard_list.html")]
pub struct DashboardListTemplate {
    pub chrome: PageChrome,
    pub labels: UiLabels,
    pub heading: String,
    pub lead: String,
    pub empty: String,
    pub cards: Vec<DashboardCard>,
}

/// `/dashboards/{name}` (`templates/dashboard.html`).
#[derive(Template)]
#[template(path = "dashboard.html")]
pub struct DashboardTemplate {
    pub chrome: PageChrome,
    pub labels: UiLabels,
    pub heading: String,
    pub title: String,
    pub publisher: String,
    pub sections: Vec<DashboardSection>,
    pub data_heading: String,
    pub data: Vec<DatasetLink>,
    pub empty: String,
}

fn publisher_title(ds: &Dataset, publishers: &[Publisher], lang: &str, default: &str) -> String {
    publishers
        .iter()
        .find(|p| p.id == ds.publisher_id || p.name == ds.publisher_id)
        .and_then(|p| localized_text(&p.title, lang, default).map(str::to_owned))
        .unwrap_or_default()
}

/// Renders `/dashboards`: the dashboards the reader may see, by title.
#[allow(clippy::too_many_arguments)]
pub fn render_dashboard_list_page(
    catalogue: &Catalogue,
    installation_branding: Option<&BrandingBlock>,
    dashboards: &[Dataset],
    publishers: &[Publisher],
    lang: &str,
    had_url_prefix: bool,
    mount_prefix: &str,
    csp_nonce: &str,
) -> String {
    let chrome = PageChrome::build(
        catalogue,
        installation_branding,
        lang,
        had_url_prefix,
        "/dashboards",
        None,
        mount_prefix,
        "dashboards",
        csp_nonce,
    );
    let default = &catalogue.default_language;
    let mut cards: Vec<DashboardCard> = dashboards
        .iter()
        .map(|d| DashboardCard {
            title: localized_text(&d.title, &chrome.lang, default)
                .unwrap_or(&d.name)
                .to_owned(),
            url: format!("{}/dashboards/{}", chrome.lang_prefix, d.name),
            publisher: publisher_title(d, publishers, &chrome.lang, default),
        })
        .collect();
    cards.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    let tpl = DashboardListTemplate {
        labels: UiLabels::for_lang(&chrome.lang),
        heading: jc_t("dashboards", &chrome.lang),
        lead: jc_t("dashboards_lead", &chrome.lang),
        empty: jc_t("dashboards_empty", &chrome.lang),
        cards,
        chrome,
    };
    tpl.render().unwrap_or_default()
}

/// Renders `/dashboards/{name}`.
#[allow(clippy::too_many_arguments)]
pub fn render_dashboard_page(
    catalogue: &Catalogue,
    installation_branding: Option<&BrandingBlock>,
    dashboard: &Dataset,
    views: &[ResourceView],
    datasets: &[Dataset],
    publishers: &[Publisher],
    lang: &str,
    had_url_prefix: bool,
    mount_prefix: &str,
    csp_nonce: &str,
) -> String {
    let chrome = PageChrome::build(
        catalogue,
        installation_branding,
        lang,
        had_url_prefix,
        &format!("/dashboards/{}", dashboard.name),
        None,
        mount_prefix,
        "dashboards",
        csp_nonce,
    );
    let default = &catalogue.default_language;
    let sections = dashboard_sections(
        dashboard,
        views,
        datasets,
        &chrome.branding,
        &chrome.lang,
        &chrome.lang_prefix,
    );
    let data = datasets
        .iter()
        .filter(|d| !is_dashboard(d) && reads(dashboard, d))
        .map(|d| DatasetLink {
            title: localized_text(&d.title, &chrome.lang, default)
                .unwrap_or(&d.name)
                .to_owned(),
            url: format!("{}/dataset/{}", chrome.lang_prefix, d.name),
        })
        .collect();
    let tpl = DashboardTemplate {
        labels: UiLabels::for_lang(&chrome.lang),
        heading: jc_t("dashboards", &chrome.lang),
        title: localized_text(&dashboard.title, &chrome.lang, default)
            .unwrap_or(&dashboard.name)
            .to_owned(),
        publisher: publisher_title(dashboard, publishers, &chrome.lang, default),
        sections,
        data_heading: jc_t("dashboard_data", &chrome.lang),
        data,
        empty: jc_t("dashboard_nothing", &chrome.lang),
        chrome,
    };
    tpl.render().unwrap_or_default()
}

/// The dashboards a dataset page links: those of `dashboards` that read its Endpoint.
pub fn dashboard_links(
    dataset: &Dataset,
    dashboards: &[Dataset],
    lang: &str,
    default_lang: &str,
    lang_prefix: &str,
) -> Vec<DatasetLink> {
    dashboards
        .iter()
        .filter(|d| is_dashboard(d) && reads(d, dataset))
        .map(|d| DatasetLink {
            title: localized_text(&d.title, lang, default_lang)
                .unwrap_or(&d.name)
                .to_owned(),
            url: format!("{lang_prefix}/dashboards/{}", d.name),
        })
        .collect()
}
