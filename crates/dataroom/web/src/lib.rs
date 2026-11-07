#![forbid(unsafe_code)]
//! Server-rendered HTML UI (`askama`, C12) and SDK DataGrid configurations (`C15`) for
//! `dataroom`, localized in Slovak (`sk`), Czech (`cs`) and English (`en`).

use askama::Template;
use dataroom_core::vocab::find_licence;
use dataroom_core::{
    text_in_lang, Catalogue, Dataset, FormFieldLayout, Publisher, ShaclValidationReport,
    ThemeTokens,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Localized UI strings for `sk`, `cs`, and `en` (C12).
#[derive(Debug, Clone)]
pub struct UiLabels {
    pub skip_to_content: &'static str,
    pub datasets: &'static str,
    pub publishers: &'static str,
    pub distributions: &'static str,
    pub search_label: &'static str,
    pub search_button: &'static str,
    pub col_title: &'static str,
    pub col_publisher: &'static str,
    pub col_formats: &'static str,
    pub col_modified: &'static str,
    pub licence: &'static str,
    pub live_preview: &'static str,
    pub shacl_validation: &'static str,
    pub empty_datasets: &'static str,
    pub empty_publishers: &'static str,
    pub empty_distributions: &'static str,
}

impl UiLabels {
    pub fn for_lang(lang: &str) -> Self {
        match lang {
            "sk" => Self {
                skip_to_content: "Preskočiť na hlavný obsah",
                datasets: "Datasety",
                publishers: "Poskytovatelia",
                distributions: "Distribúcie",
                search_label: "Vyhľadať datasety",
                search_button: "Hľadať",
                col_title: "Názov",
                col_publisher: "Poskytovateľ",
                col_formats: "Formáty",
                col_modified: "Upravené",
                licence: "Licencia",
                live_preview: "Živý náhľad (NGSI-LD Endpoint)",
                shacl_validation: "SHACL validácia profilov",
                empty_datasets: "Žiadne datasety nezodpovedajú filtrom.",
                empty_publishers: "Žiadni poskytovatelia.",
                empty_distributions: "Dataset nemá žiadne distribúcie.",
            },
            "cs" => Self {
                skip_to_content: "Přeskočit na hlavní obsah",
                datasets: "Datové sady",
                publishers: "Poskytovatelé",
                distributions: "Distribuce",
                search_label: "Vyhledat datové sady",
                search_button: "Hledat",
                col_title: "Název",
                col_publisher: "Poskytovatel",
                col_formats: "Formáty",
                col_modified: "Upraveno",
                licence: "Licence",
                live_preview: "Živý náhled (NGSI-LD Endpoint)",
                shacl_validation: "SHACL validace profilů",
                empty_datasets: "Žádné datové sady neodpovídají filtrům.",
                empty_publishers: "Žádní poskytovatelé.",
                empty_distributions: "Datová sada nemá žádné distribuce.",
            },
            _ => Self {
                skip_to_content: "Skip to main content",
                datasets: "Datasets",
                publishers: "Publishers",
                distributions: "Distributions",
                search_label: "Search datasets",
                search_button: "Search",
                col_title: "Title",
                col_publisher: "Publisher",
                col_formats: "Formats",
                col_modified: "Modified",
                licence: "Licence",
                live_preview: "Live Preview (NGSI-LD Endpoint)",
                shacl_validation: "SHACL Profile Validation",
                empty_datasets: "No datasets match the current filters.",
                empty_publishers: "No publishers registered.",
                empty_distributions: "No distributions listed for this dataset.",
            },
        }
    }
}

/// Content Security Policy header with per-request nonce (`research-tenancy.md` §2.6, `ours/deployment/README.md`).
pub fn csp_header(nonce: &str, framable_map: bool) -> String {
    let ancestors = if framable_map { "*" } else { "'self'" };
    format!(
        "default-src 'self'; script-src 'self' 'nonce-{nonce}'; style-src 'self' 'nonce-{nonce}'; frame-ancestors {ancestors}; object-src 'none'; base-uri 'self'; form-action 'self'"
    )
}

/// Emits the platform SDK grid configuration (`EntityGridConfig` from `shared/sdk-grid/config.ts`)
/// pointed at one of dataroom's OData v4 entity sets (`C15`: `{ kind: "odata", url, entitySet }`).
pub fn odata_grid_config(
    odata_base_url: &str,
    entity_set: &str,
    entity_type: &str,
    columns: &[(&str, &str, &str)],
) -> Value {
    let cols: Vec<Value> = columns
        .iter()
        .map(|(attr, label, fmt)| {
            json!({
                "attr": attr,
                "label": label,
                "format": fmt,
            })
        })
        .collect();
    json!({
        "source": {
            "kind": "odata",
            "url": odata_base_url,
            "entitySet": entity_set
        },
        "type": entity_type,
        "columns": cols,
        "pageSize": 50,
        "mode": "view",
        "density": "comfortable",
        "map": { "enabled": false }
    })
}

/// Row view model for a dataset in the HTML table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetRowView {
    pub name: String,
    pub title: String,
    pub publisher: String,
    pub formats: String,
    pub modified: String,
}

/// Row view model for a publisher in the HTML table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublisherRowView {
    pub name: String,
    pub title: String,
    pub uri: String,
}

/// Row view model for a distribution in the HTML table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributionRowView {
    pub title: String,
    pub format: String,
    pub url: String,
}

/// Home and search page Askama template (`templates/home.html`).
#[derive(Template)]
#[template(path = "home.html")]
pub struct HomePageTemplate {
    pub lang: String,
    pub title: String,
    pub catalogue_title: String,
    pub catalogue_description: String,
    pub query: String,
    pub total_datasets: usize,
    pub datasets: Vec<DatasetRowView>,
    pub publishers: Vec<PublisherRowView>,
    pub dataset_grid_config: String,
    pub publisher_grid_config: String,
    pub theme: ThemeTokens,
    pub labels: UiLabels,
    pub csp_nonce: String,
}

/// Dataset detail page Askama template (`templates/dataset.html`).
#[derive(Template)]
#[template(path = "dataset.html")]
pub struct DatasetPageTemplate {
    pub lang: String,
    pub catalogue_title: String,
    pub dataset_name: String,
    pub dataset_title: String,
    pub dataset_description: String,
    pub publisher_slug: String,
    pub publisher_title: String,
    pub licence_title: String,
    pub endpoint_url: String,
    pub distributions: Vec<DistributionRowView>,
    pub distributions_grid_config: String,
    pub show_editor_panel: bool,
    pub shacl_reports: Vec<ShaclValidationReport>,
    pub shacl_grid_config: String,
    pub schema_org_jsonld: String,
    pub theme: ThemeTokens,
    pub labels: UiLabels,
    pub csp_nonce: String,
}

/// Renders the catalogue home / search page.
pub fn render_home_page(
    catalogue: &Catalogue,
    datasets: &[Dataset],
    publishers: &[Publisher],
    query: &str,
    lang: &str,
    csp_nonce: &str,
) -> String {
    let labels = UiLabels::for_lang(lang);
    let catalogue_title = text_in_lang(&catalogue.title, Some(lang))
        .unwrap_or(&catalogue.name)
        .to_owned();
    let catalogue_description = text_in_lang(&catalogue.description, Some(lang))
        .unwrap_or("")
        .to_owned();

    let ds_rows: Vec<DatasetRowView> = datasets
        .iter()
        .take(50)
        .map(|d| {
            let mut fmts: Vec<String> = d
                .distributions
                .iter()
                .filter_map(|r| r.format.clone())
                .collect();
            fmts.sort();
            fmts.dedup();
            DatasetRowView {
                name: d.name.clone(),
                title: text_in_lang(&d.title, Some(lang))
                    .unwrap_or(&d.name)
                    .to_owned(),
                publisher: d.publisher_id.clone(),
                formats: fmts.join(", "),
                modified: d.modified.format("%Y-%m-%d").to_string(),
            }
        })
        .collect();

    let pub_rows: Vec<PublisherRowView> = publishers
        .iter()
        .map(|p| PublisherRowView {
            name: p.name.clone(),
            title: text_in_lang(&p.title, Some(lang))
                .unwrap_or(&p.name)
                .to_owned(),
            uri: p.uri.clone(),
        })
        .collect();

    let ds_grid = odata_grid_config(
        "/odata/v4",
        "Datasets",
        "Dataset",
        &[
            ("title", labels.col_title, "text"),
            ("publisher_id", labels.col_publisher, "text"),
            ("modified", labels.col_modified, "date"),
        ],
    );
    let pub_grid = odata_grid_config(
        "/odata/v4",
        "Publishers",
        "Publisher",
        &[
            ("title", labels.col_title, "text"),
            ("uri", "IRI", "link"),
        ],
    );

    let tpl = HomePageTemplate {
        lang: lang.to_owned(),
        title: labels.datasets.to_owned(),
        catalogue_title,
        catalogue_description,
        query: query.to_owned(),
        total_datasets: datasets.len(),
        datasets: ds_rows,
        publishers: pub_rows,
        dataset_grid_config: ds_grid.to_string(),
        publisher_grid_config: pub_grid.to_string(),
        theme: catalogue.theme.clone(),
        labels,
        csp_nonce: csp_nonce.to_owned(),
    };
    tpl.render().unwrap_or_default()
}

/// Renders the dataset detail page with distributions, live preview link, Schema.org JSON-LD,
/// and the SHACL validation panel for editors.
pub fn render_dataset_page(
    catalogue: &Catalogue,
    dataset: &Dataset,
    publisher: Option<&Publisher>,
    shacl_reports: &[ShaclValidationReport],
    show_editor_panel: bool,
    lang: &str,
    csp_nonce: &str,
) -> String {
    let labels = UiLabels::for_lang(lang);
    let catalogue_title = text_in_lang(&catalogue.title, Some(lang))
        .unwrap_or(&catalogue.name)
        .to_owned();
    let dataset_title = text_in_lang(&dataset.title, Some(lang))
        .unwrap_or(&dataset.name)
        .to_owned();
    let dataset_description = text_in_lang(&dataset.description, Some(lang))
        .unwrap_or("")
        .to_owned();
    let publisher_slug = publisher
        .map(|p| p.name.clone())
        .unwrap_or_else(|| dataset.publisher_id.clone());
    let publisher_title = publisher
        .and_then(|p| text_in_lang(&p.title, Some(lang)).map(str::to_owned))
        .unwrap_or_else(|| publisher_slug.clone());

    let licence_title = dataset
        .licence_id
        .as_deref()
        .and_then(find_licence)
        .map(|l| l.title)
        .or_else(|| dataset.licence_id.clone())
        .unwrap_or_default();

    let dist_rows: Vec<DistributionRowView> = dataset
        .distributions
        .iter()
        .map(|d| DistributionRowView {
            title: text_in_lang(&d.title, Some(lang))
                .unwrap_or(&d.access_url)
                .to_owned(),
            format: d.format.clone().unwrap_or_default(),
            url: d
                .download_url
                .as_ref()
                .unwrap_or(&d.access_url)
                .clone(),
        })
        .collect();

    let dist_grid = odata_grid_config(
        "/odata/v4",
        "Distributions",
        "Distribution",
        &[
            ("name", labels.col_title, "text"),
            ("format", labels.col_formats, "text"),
            ("access_url", "URL", "link"),
        ],
    );
    let shacl_grid = odata_grid_config(
        "/odata/v4",
        "ShaclResults",
        "ShaclResult",
        &[
            ("profile_id", "Profile", "text"),
            ("profile_version", "Version", "text"),
            ("violation_count", "Violations", "number"),
        ],
    );

    let schema_org = json!({
        "@context": "https://schema.org/",
        "@type": "Dataset",
        "name": dataset_title,
        "description": dataset_description,
        "identifier": dataset.id,
        "license": dataset.licence_url.as_deref().unwrap_or(""),
        "publisher": {
            "@type": "Organization",
            "name": publisher_title
        }
    });

    let tpl = DatasetPageTemplate {
        lang: lang.to_owned(),
        catalogue_title,
        dataset_name: dataset.name.clone(),
        dataset_title,
        dataset_description,
        publisher_slug,
        publisher_title,
        licence_title,
        endpoint_url: dataset.endpoint_url.clone().unwrap_or_default(),
        distributions: dist_rows,
        distributions_grid_config: dist_grid.to_string(),
        show_editor_panel,
        shacl_reports: shacl_reports.to_vec(),
        shacl_grid_config: shacl_grid.to_string(),
        schema_org_jsonld: schema_org.to_string(),
        theme: catalogue.theme.clone(),
        labels,
        csp_nonce: csp_nonce.to_owned(),
    };
    tpl.render().unwrap_or_default()
}

/// Renders an HTML editor form from the [`FormFieldLayout`] derived from a profile's SHACL shapes (C17).
pub fn render_profile_editor_form(fields: &[FormFieldLayout], lang: &str) -> String {
    let mut out = String::from("<form method=\"post\" action=\"/api/v1/datasets\" class=\"dr-profile-form\">\n");
    for f in fields {
        let label = text_in_lang(&f.name, Some(lang)).unwrap_or(&f.field_name);
        let desc = text_in_lang(&f.description, Some(lang)).unwrap_or("");
        let req_attr = if f.required { " required" } else { "" };
        out.push_str(&format!(
            "  <div class=\"form-field\" data-path=\"{}\" data-order=\"{}\">\n",
            f.path, f.order
        ));
        out.push_str(&format!(
            "    <label for=\"field-{}\">{}{}</label>\n",
            f.field_name,
            label,
            if f.required { " *" } else { "" }
        ));
        out.push_str(&format!(
            "    <input id=\"field-{}\" name=\"{}\" type=\"text\"{req_attr} aria-describedby=\"hint-{}\"/>\n",
            f.field_name, f.field_name, f.field_name
        ));
        if !desc.is_empty() {
            out.push_str(&format!(
                "    <small id=\"hint-{}\">{}</small>\n",
                f.field_name, desc
            ));
        }
        out.push_str("  </div>\n");
    }
    out.push_str("</form>\n");
    out
}
