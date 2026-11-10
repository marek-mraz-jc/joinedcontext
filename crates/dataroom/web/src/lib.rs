#![forbid(unsafe_code)]
//! Server-rendered HTML UI (`askama`, C12, D1-D7) and SDK DataGrid configurations (`C15`) for
//! `dataroom`, localized in Slovak (`sk`), Czech (`cs`), English (`en`), Hungarian (`hu`) and
//! German (`de`).

pub mod groups;
pub mod helpers;
pub mod i18n;
pub mod listing;

use askama::Template;
use chrono::Datelike;
use dataroom_core::branding::Address;
use dataroom_core::vocab::{find_licence, theme_code};
use dataroom_core::{
    localized_text, text_in_lang, BrandingBlock, Catalogue, Dataset, Distribution, FormFieldLayout,
    Publisher, RecordState, ResourceView, ShaclValidationReport, ThemeTokens, Visibility,
};
use dataroom_search::SearchFacets;
use helpers::{
    html_escape, jc_about, jc_assistant_guide, jc_developer_page, jc_examples, jc_explorer,
    jc_format_chips, jc_is_source, jc_kind, jc_live, jc_map_config, jc_mcp_install, jc_model,
    jc_more_rows, jc_page, jc_plain_glyphs, jc_preview, jc_resource_format_chip, jc_resource_label,
    jc_resource_sections, jc_schema_org_script, jc_standard, jc_subscriptions, jc_try_prompt,
    jc_visualizations, jc_what, publisher_card_info, render_breakable_html, AboutRow,
    AssistantGuideView, ChartView, ExplorerView, LiveEndpointBox, McpInstallView, ResourceExample,
    ResourcePreviewConfig, ResourceSectionView, StaticPageView, SubscriptionsView, CONNECTOR_NAME,
    STATIC_PAGE_NAMES,
};
use i18n::{
    available_languages, format_date_localized, jc_number, jc_rows, jc_t, language_display_name,
    normalize_lang_code, url_in_lang,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Complete stylesheet: the base layout CKAN's Bootstrap and main.css gave (`assets/base.css`), then
/// `ours/ckan-theme/public__jc-theme.css`, plus dark-mode support (`prefers-color-scheme: dark`, B2).
pub const JC_THEME_CSS: &str = concat!(
    include_str!("../assets/base.css"),
    include_str!("../assets/public__jc-theme.css"),
    "\n@media (prefers-color-scheme: dark) {\n  :root[data-jc-color-scheme=\"auto\"] {\n    --jc-paper: #0f172a;\n    --jc-ink: #f8fafc;\n  }\n}\n"
);

/// Built-in enum labels JSON from `ours/ckan-theme/public__jc-enums.json`.
pub const JC_ENUMS_JSON: &str = include_str!("../assets/public__jc-enums.json");

/// The theme's scripts every page names by `{mount}/{name}` (`ours/ckan-theme/public__*.js`),
/// each loaded with the page's CSP nonce; none runs inline.
pub const JC_SCRIPTS: &[(&str, &str)] = &[
    (
        "jc-assistant.js",
        include_str!("../assets/public__jc-assistant.js"),
    ),
    ("jc-copy.js", include_str!("../assets/public__jc-copy.js")),
    ("jc-dock.js", include_str!("../assets/public__jc-dock.js")),
    ("jc-forms.js", include_str!("../assets/public__jc-forms.js")),
    ("jc-frame.js", include_str!("../assets/public__jc-frame.js")),
    ("jc-map.js", include_str!("../assets/public__jc-map.js")),
    (
        "jc-preview.js",
        include_str!("../assets/public__jc-preview.js"),
    ),
    ("jc-viz.js", include_str!("../assets/public__jc-viz.js")),
    (
        "pmtiles-4.5.0.js",
        include_str!("../assets/public__pmtiles-4.5.0.js"),
    ),
];

/// The joinedcontext lockup (`/jc-logo.svg`, old prod's `public__jc-logo.svg`: paths only, no script,
/// no external href), the header logo and the `/logo.svg` an installation without `logoSvg` serves.
pub const JC_LOGO_SVG: &str = include_str!("../assets/public__jc-logo.svg");

///Localized UI strings struct for templates (C3).
#[derive(Debug, Clone)]
pub struct UiLabels {
    pub skip_to_content: String,
    pub login: String,
    pub home: String,
    pub welcome: String,
    pub datasets: String,
    pub publishers: String,
    pub distributions: String,
    pub nav_datasets: String,
    pub nav_publishers: String,
    pub nav_topics: String,
    pub nav_about: String,
    pub search_label: String,
    pub search_button: String,
    pub search_datasets_placeholder: String,
    pub search_publishers: String,
    pub hero_search: String,
    pub hero_placeholder: String,
    pub by_topic: String,
    pub recent: String,
    pub all_datasets: String,
    pub by_publisher: String,
    pub by_keyword: String,
    pub updated: String,
    pub created: String,
    pub last_update: String,
    pub no_datasets: String,
    pub empty_datasets: String,
    pub empty_publishers: String,
    pub empty_distributions: String,
    pub col_title: String,
    pub col_publisher: String,
    pub col_formats: String,
    pub col_modified: String,
    pub publisher: String,
    pub topic: String,
    pub keywords: String,
    pub format: String,
    pub licence: String,
    pub order_by: String,
    pub sort_relevance: String,
    pub sort_name_asc: String,
    pub sort_name_desc: String,
    pub sort_modified: String,
    pub go: String,
    pub filter_results: String,
    /// The close button of the phone filter modal.
    pub close: String,
    /// The API footnote under a dataset list (CKAN's `api_info`).
    pub api_access: String,
    pub api_docs: String,
    pub api_docs_link: String,
    pub show_more: String,
    pub about: String,
    pub about_site: String,
    pub catalogue_api: String,
    pub portal: String,
    pub language: String,
    pub assistants: String,
    pub assistants_label: String,
    pub assistant: String,
    pub live: String,
    pub live_lead: String,
    pub example: String,
    pub copy: String,
    pub copied: String,
    pub copy_select: String,
    pub copy_address: String,
    pub full_address: String,
    pub address: String,
    pub part_of: String,
    pub entity_type: String,
    pub resources: String,
    pub no_resources: String,
    pub more: String,
    pub visualizations: String,
    pub viz_embed: String,
    pub viz_embed_lead: String,
    pub viz_table: String,
    pub map: String,
    pub map_title: String,
    pub map_as_table: String,
    pub map_in_catalogue: String,
    pub map_embed: String,
    pub map_embed_lead: String,
    pub map_keys: String,
    pub subscribe: String,
    pub subscribe_calendar: String,
    pub subscribe_news: String,
    pub subscribe_hint: String,
    pub mcp: String,
    pub mcp_lead: String,
    pub mcp_url: String,
    pub mcp_claude_ai: String,
    pub mcp_claude_ai_1: String,
    pub mcp_claude_ai_2: String,
    pub mcp_claude_ai_3: String,
    pub mcp_editors: String,
    pub mcp_claude: String,
    pub mcp_json: String,
    pub mcp_vscode: String,
    pub mcp_cursor: String,
    pub any_client: String,
    pub for_developers: String,
    pub connector_status: String,
    pub connector_note: String,
    pub connector_why: String,
    pub store_address: String,
    pub guide_open: String,
    pub guide_who: String,
    pub guide_steps: String,
    pub guide_try: String,
    pub guide_trouble: String,
    pub guide_back: String,
    pub publishers_help: String,
    pub read_more: String,
    pub followers: String,
    pub members: String,
    pub about_project: String,
    pub tags_label: String,
    pub formats_label: String,
    pub licences_label: String,
    pub dataset_singular: String,
    pub info: String,
    pub usage: String,
    pub snippet: String,
    pub download: String,
    pub open: String,
    pub file_lead: String,
    pub source: String,
    pub source_lead: String,
    pub section_schema: String,
    pub conforms_to: String,
    pub records: String,
    pub preview_loading: String,
    pub preview_failed: String,
    pub preview_empty: String,
    pub preview_shown: String,
    pub shacl_validation: String,
    pub about_data: String,
    pub about_machines: String,
    pub about_api: String,
    pub about_dcat: String,
    pub about_dcat_what: String,
    pub about_contact: String,
    pub contact: String,
    pub sources_all: String,
}

impl UiLabels {
    pub fn for_lang(lang: &str) -> Self {
        let t = |k: &str| jc_t(k, lang);
        Self {
            skip_to_content: t("skip_to_content"),
            login: t("login"),
            home: t("home"),
            welcome: t("welcome"),
            datasets: t("datasets"),
            publishers: t("publishers"),
            distributions: t("resources"),
            nav_datasets: t("nav_datasets"),
            nav_publishers: t("nav_publishers"),
            nav_topics: t("nav_topics"),
            nav_about: t("nav_about"),
            search_label: t("hero_search"),
            search_button: t("search"),
            search_datasets_placeholder: t("search_datasets_placeholder"),
            search_publishers: t("search_publishers"),
            hero_search: t("hero_search"),
            hero_placeholder: t("hero_placeholder"),
            by_topic: t("by_topic"),
            recent: t("recent"),
            all_datasets: t("all_datasets"),
            by_publisher: t("by_publisher"),
            by_keyword: t("by_keyword"),
            updated: t("updated"),
            created: t("created"),
            last_update: t("last_update"),
            no_datasets: t("no_datasets"),
            empty_datasets: t("empty"),
            empty_publishers: t("no_publishers"),
            empty_distributions: t("no_resources"),
            col_title: t("dataset_singular"),
            col_publisher: t("publisher"),
            col_formats: t("format"),
            col_modified: t("updated"),
            publisher: t("publisher"),
            topic: t("topic"),
            keywords: t("keywords"),
            format: t("format"),
            licence: t("licence"),
            order_by: t("order_by"),
            sort_relevance: t("sort_relevance"),
            sort_name_asc: t("sort_name_asc"),
            sort_name_desc: t("sort_name_desc"),
            sort_modified: t("sort_modified"),
            go: t("go"),
            filter_results: t("filter_results"),
            close: t("map_close"),
            api_access: t("api_access"),
            api_docs: t("api_docs"),
            api_docs_link: t("api_docs_link"),
            show_more: t("show_more"),
            about: t("about"),
            about_site: t("about_site"),
            catalogue_api: t("catalogue_api"),
            portal: t("portal"),
            language: t("language"),
            assistants: t("assistants"),
            assistants_label: t("assistants_label"),
            assistant: t("assistant"),
            live: t("live"),
            live_lead: t("live_lead"),
            example: t("example"),
            copy: t("copy"),
            copied: t("copied"),
            copy_select: t("copy_select"),
            copy_address: t("copy_address"),
            full_address: t("full_address"),
            address: t("address"),
            part_of: t("part_of"),
            entity_type: t("entity_type"),
            resources: t("resources"),
            no_resources: t("no_resources"),
            more: t("more"),
            visualizations: t("visualizations"),
            viz_embed: t("viz_embed"),
            viz_embed_lead: t("viz_embed_lead"),
            viz_table: t("viz_table"),
            map: t("map"),
            map_title: t("map_title"),
            map_as_table: t("map_as_table"),
            map_in_catalogue: t("map_in_catalogue"),
            map_embed: t("map_embed"),
            map_embed_lead: t("map_embed_lead"),
            map_keys: t("map_keys"),
            subscribe: t("subscribe"),
            subscribe_calendar: t("subscribe_calendar"),
            subscribe_news: t("subscribe_news"),
            subscribe_hint: t("subscribe_hint"),
            mcp: t("mcp"),
            mcp_lead: t("mcp_lead"),
            mcp_url: t("mcp_url"),
            mcp_claude_ai: t("mcp_claude_ai"),
            mcp_claude_ai_1: t("mcp_claude_ai_1"),
            mcp_claude_ai_2: t("mcp_claude_ai_2"),
            mcp_claude_ai_3: t("mcp_claude_ai_3"),
            mcp_editors: t("mcp_editors"),
            mcp_claude: t("mcp_claude"),
            mcp_json: t("mcp_json"),
            mcp_vscode: t("mcp_vscode"),
            mcp_cursor: t("mcp_cursor"),
            any_client: t("any_client"),
            for_developers: t("for_developers"),
            connector_status: t("connector_status"),
            connector_note: t("connector_note"),
            connector_why: t("connector_why"),
            store_address: t("store_address"),
            guide_open: t("guide_open"),
            guide_who: t("guide_who"),
            guide_steps: t("guide_steps"),
            guide_try: t("guide_try"),
            guide_trouble: t("guide_trouble"),
            guide_back: t("guide_back"),
            publishers_help: t("publishers_help"),
            read_more: t("read_more"),
            followers: t("followers"),
            members: t("members"),
            about_project: t("about_project"),
            tags_label: t("tags_label"),
            formats_label: t("formats_label"),
            licences_label: t("licences_label"),
            dataset_singular: t("dataset_singular"),
            info: t("info"),
            usage: t("usage"),
            snippet: t("snippet"),
            download: t("download"),
            open: t("open"),
            file_lead: t("file_lead"),
            source: t("source"),
            source_lead: t("source_lead"),
            section_schema: t("section_schema"),
            conforms_to: t("conforms_to"),
            records: t("records"),
            preview_loading: t("preview_loading"),
            preview_failed: t("preview_failed"),
            preview_empty: t("preview_empty"),
            preview_shown: t("preview_shown"),
            shacl_validation: t("shacl_validation"),
            about_data: t("about_data"),
            about_machines: t("about_machines"),
            about_api: t("about_api"),
            about_dcat: t("about_dcat"),
            about_dcat_what: t("about_dcat_what"),
            about_contact: t("about_contact"),
            contact: t("contact"),
            sources_all: t("sources_all"),
        }
    }
}

/// Content Security Policy header with per-request nonce (`_csp` / `jc_csp_header` in
/// `ours/ckan-theme/plugin.py` lines 1581-1606, D7).
pub fn csp_header_for_domain(nonce: &str, domain: Option<&str>, framable_map: bool) -> String {
    let ancestors = if framable_map { "*" } else { "'self'" };
    let own = match domain.filter(|d| {
        !d.is_empty()
            && d.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    }) {
        Some(d) => format!(" https://{d} https://*.{d}"),
        None => String::new(),
    };
    format!(
        "default-src 'self'; script-src 'self' 'nonce-{nonce}'; style-src 'self' 'nonce-{nonce}'; img-src 'self' data:{own}; connect-src 'self'{own}; frame-src 'self'{own}; font-src 'self'; worker-src 'self'; manifest-src 'self'; form-action 'self'; frame-ancestors {ancestors}; object-src 'none'; base-uri 'self'"
    )
}

/// The page CSP of `branding`: the domain's hosts as [`csp_header_for_domain`] names them, plus
/// the origin of each address named on its own (`addresses`), so an installation without a domain
/// (jc) lets a page frame and read exactly the viz, gateway, tiles and assistant it names, and the
/// origin of the host's assistant widget page. With nothing named the header is [`csp_header_for_domain`]'s, byte for byte.
pub fn csp_header_for_branding(
    nonce: &str,
    branding: &BrandingBlock,
    framable_map: bool,
) -> String {
    let domain = (!branding.domain.is_empty()).then_some(branding.domain.as_str());
    let mut csp = csp_header_for_domain(nonce, domain, framable_map);
    let mut extra = String::new();
    let named = dataroom_core::branding::Address::ALL
        .into_iter()
        .filter_map(|which| {
            let base = branding.address(which)?;
            let derived = !branding.domain.is_empty()
                && base == format!("https://{}.{}", which.label(), branding.domain);
            (!derived).then_some(base)
        });
    // The host's own assistant page (`assistant_widget`), framed like a named assistant.
    let widget = branding
        .assistant_widget
        .clone()
        .filter(|page| !page.is_empty());
    for base in named.chain(widget) {
        let origin = dataroom_core::branding::origin_of(&base);
        if dataroom_core::branding::is_base_url(origin) && !extra.split(' ').any(|o| o == origin) {
            extra.push(' ');
            extra.push_str(origin);
        }
    }
    if !extra.is_empty() {
        for directive in [
            "img-src 'self' data:",
            "connect-src 'self'",
            "frame-src 'self'",
        ] {
            let at = csp.find(directive).map(|i| i + directive.len());
            if let Some(at) = at {
                let end = at + csp[at..].find(';').unwrap_or(csp.len() - at);
                csp.insert_str(end, &extra);
            }
        }
    }
    csp
}

pub fn csp_header(nonce: &str, framable_map: bool) -> String {
    csp_header_for_domain(nonce, None, framable_map)
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

/// Language switcher option + `<link rel="alternate" hreflang="...">` entry (C2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LangOption {
    pub code: String,
    pub label: String,
    pub url: String,
    pub selected: bool,
}

/// Common shell context shared across all Askama page templates (header, footer, branding,
/// languages, CSP nonce, mount prefix).
#[derive(Debug, Clone)]
pub struct PageChrome {
    pub lang: String,
    pub mount_prefix: String,
    pub lang_prefix: String,
    pub site_root: String,
    pub locale_root: String,
    pub site_title: String,
    pub site_tagline: String,
    pub site_logo: String,
    pub favicon: String,
    pub footer_logo: String,
    pub branding: BrandingBlock,
    pub portal_url: String,
    pub assistant_widget_url: String,
    pub has_mcp: bool,
    pub footer_pages: Vec<(String, String)>,
    pub lang_options: Vec<LangOption>,
    pub nav_active: String,
    pub csp_nonce: String,
}

impl PageChrome {
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        cat: &Catalogue,
        installation_branding: Option<&BrandingBlock>,
        lang: &str,
        had_url_prefix: bool,
        stripped_path: &str,
        query: Option<&str>,
        mount_prefix: &str,
        nav_active: &str,
        csp_nonce: &str,
    ) -> Self {
        let norm_lang = normalize_lang_code(lang);
        let branding = cat.effective_branding(installation_branding);
        let pfx = mount_prefix.trim_end_matches('/').to_owned();
        let lang_prefix = if had_url_prefix {
            format!("{pfx}/{norm_lang}")
        } else {
            pfx.clone()
        };
        let base = cat.base_url();
        let site_root = format!("{base}{pfx}/");
        let locale_root = if had_url_prefix {
            format!("{base}{pfx}/{norm_lang}/")
        } else {
            site_root.clone()
        };

        let site_title = branding.site_title_in(&norm_lang);
        let site_tagline = branding
            .site_tagline_in(&norm_lang)
            .unwrap_or_else(|| jc_t("tagline", &norm_lang));
        let site_logo = branding.site_logo_path(&pfx);
        let favicon = if !branding.favicon.is_empty() {
            format!("{pfx}/{}", branding.favicon.trim_start_matches('/'))
        } else {
            format!("{pfx}/logo.svg")
        };
        let footer_logo = if !branding.logo.is_empty() {
            format!("{pfx}/{}", branding.logo.trim_start_matches('/'))
        } else {
            String::new()
        };
        let portal_url = branding
            .address(Address::Portal)
            .map(|p| format!("{p}/"))
            .unwrap_or_default();
        let widget_query = format!("embed=1&dock=1&lang={norm_lang}&scheme=light");
        // The host's own assistant page first (jc serves the module: `Some(page)`), else the
        // assistant address (named or `assistant.{domain}`); `Some("")` shows no launcher.
        let assistant_widget_url = match branding.assistant_widget.as_deref() {
            Some("") => String::new(),
            Some(page) => format!("{page}?{widget_query}"),
            None => branding
                .address(Address::Assistant)
                .map(|a| format!("{a}/d/data/widget?{widget_query}"))
                .unwrap_or_default(),
        };
        let has_mcp = jc_mcp_install(&branding).is_some();

        let footer_pages = STATIC_PAGE_NAMES
            .iter()
            .filter_map(|&name| {
                let p = jc_page(name, &branding, &norm_lang)?;
                Some((format!("{lang_prefix}/{name}"), p.title))
            })
            .collect();

        let offered = available_languages(&branding.languages.offered);
        let lang_options = offered
            .into_iter()
            .map(|code| LangOption {
                label: language_display_name(&code).to_owned(),
                url: url_in_lang(&pfx, &code, stripped_path, query),
                selected: code == norm_lang,
                code,
            })
            .collect();

        Self {
            lang: norm_lang,
            mount_prefix: pfx,
            lang_prefix,
            site_root,
            locale_root,
            site_title,
            site_tagline,
            site_logo,
            favicon,
            footer_logo,
            branding,
            portal_url,
            assistant_widget_url,
            has_mcp,
            footer_pages,
            lang_options,
            nav_active: nav_active.to_owned(),
            csp_nonce: csp_nonce.to_owned(),
        }
    }
}

/// Topic tile on the Home page (`jc_topic_tiles` in `plugin.py` line 2060, D1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicTileView {
    pub code: String,
    pub label: String,
    pub count_label: String,
    pub url: String,
}

/// Dataset card on Home / Dataset list / Publisher page (D1, D2, D5).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetRowView {
    pub name: String,
    pub title: String,
    pub notes: String,
    pub publisher: String,
    pub publisher_title: String,
    pub formats: String,
    pub format_chips: Vec<String>,
    pub modified: String,
    pub modified_iso: String,
    pub modified_localized: String,
}

/// Publisher row/card view model (D1, D5).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublisherRowView {
    pub name: String,
    pub title: String,
    pub uri: String,
    pub image: String,
    pub description: String,
    pub description_short: String,
    pub count: usize,
}

/// Distribution row view model (D3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributionRowView {
    pub title: String,
    pub format: String,
    pub url: String,
}

/// Facet item view model for Dataset list / Publisher page (D2, D5).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FacetItemView {
    pub name: String,
    pub label: String,
    pub count: usize,
    pub href: String,
    pub active: bool,
}

/// Facet group view model (`Vydavateľ`, `Téma`, `Kľúčové slová`, `Formát`, `Licencia`, D2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FacetGroupView {
    pub param: String,
    pub title: String,
    pub items: Vec<FacetItemView>,
    pub more_href: Option<String>,
}

/// Home page Askama template (`templates/home.html`, D1).
#[derive(Template)]
#[template(path = "home.html")]
pub struct HomePageTemplate {
    pub chrome: PageChrome,
    pub lang: String,
    pub title: String,
    pub catalogue_title: String,
    pub catalogue_description: String,
    pub query: String,
    pub total_datasets: usize,
    pub total_datasets_formatted: String,
    pub total_publishers_formatted: String,
    pub topic_tiles: Vec<TopicTileView>,
    pub recent_datasets: Vec<DatasetRowView>,
    pub datasets: Vec<DatasetRowView>,
    pub publishers: Vec<PublisherRowView>,
    pub popular_keywords: Vec<(String, String)>,
    pub dataset_grid_config: String,
    pub publisher_grid_config: String,
    pub theme: ThemeTokens,
    pub labels: UiLabels,
    pub csp_nonce: String,
}

/// Dataset search / list page Askama template (`templates/dataset_list.html`, D2).
#[derive(Template)]
#[template(path = "dataset_list.html")]
pub struct DatasetListPageTemplate {
    pub chrome: PageChrome,
    pub labels: UiLabels,
    pub query: String,
    pub sort: String,
    pub total: usize,
    pub found_heading: String,
    pub facet_groups: Vec<FacetGroupView>,
    pub datasets: Vec<DatasetRowView>,
    pub dataset_grid_config: String,
    pub page: usize,
    pub pager: Vec<listing::PageLinkView>,
    pub filter_pills: Vec<listing::FilterPillView>,
    pub remove_label: String,
    /// The active filters, carried as hidden fields so a new search or sort keeps them.
    pub hidden_params: Vec<(String, String)>,
}

/// Dataset detail page Askama template (`templates/dataset.html`, D3).
#[derive(Template)]
#[template(path = "dataset.html")]
pub struct DatasetPageTemplate {
    pub chrome: PageChrome,
    pub lang: String,
    pub catalogue_title: String,
    pub dataset_id: String,
    pub dataset_name: String,
    pub dataset_title: String,
    pub dataset_description: String,
    pub publisher_slug: String,
    pub publisher_title: String,
    pub publisher_image: String,
    pub publisher_description: String,
    pub licence_title: String,
    pub endpoint_url: String,
    pub explorer: Option<ExplorerView>,
    pub charts: Vec<ChartView>,
    pub map_config_json: Option<String>,
    pub map_embed_url: String,
    pub subscriptions: SubscriptionsView,
    pub about_rows: Vec<AboutRow>,
    pub source_rows: Vec<AboutRow>,
    pub fold_sources: bool,
    pub sources_all_label: String,
    pub live: Option<LiveEndpointBox>,
    pub mcp: Option<McpInstallView>,
    pub resource_sections: Vec<ResourceSectionView>,
    pub more_rows: Vec<AboutRow>,
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

/// Resource detail page Askama template (`templates/resource.html`, D4).
#[derive(Template)]
#[template(path = "resource.html")]
pub struct ResourcePageTemplate {
    pub chrome: PageChrome,
    pub labels: UiLabels,
    /// The heading of the resource list card ("Zdroje", CKAN's `package/snippets/resources.html`).
    pub resources_heading: String,
    pub dataset_name: String,
    pub dataset_title: String,
    pub publisher_slug: String,
    pub publisher_title: String,
    pub resource_id: String,
    pub resource_label: String,
    pub format_chip: String,
    pub entity_type: String,
    pub what: String,
    pub description: String,
    pub is_api: bool,
    pub is_source: bool,
    pub url: String,
    /// `url` when it is an http(s) address, else empty (no link).
    pub link_url: String,
    pub short_segments: Vec<String>,
    pub full_breakable: String,
    pub explorer: Option<ExplorerView>,
    pub preview: Option<ResourcePreviewConfig>,
    pub examples: Vec<ResourceExample>,
    pub guide_url: Option<String>,
    pub developer_page_url: Option<String>,
    pub dates: Vec<(String, String)>,
    pub licence_title: String,
    pub licence_url: String,
    pub standard: Option<(String, String)>,
    pub model: Option<(String, String)>,
    pub resource_sections: Vec<ResourceSectionView>,
}

/// Publisher list page (`/organization/`, `templates/organization_list.html`, D5).
#[derive(Template)]
#[template(path = "organization_list.html")]
pub struct OrganizationListPageTemplate {
    pub chrome: PageChrome,
    pub labels: UiLabels,
    pub query: String,
    pub sort: String,
    pub found_heading: String,
    pub publishers: Vec<PublisherRowView>,
    pub publisher_grid_config: String,
}

/// Publisher detail page (`/organization/{name}`, `templates/organization_read.html`, D5).
#[derive(Template)]
#[template(path = "organization_read.html")]
pub struct OrganizationReadPageTemplate {
    pub chrome: PageChrome,
    pub labels: UiLabels,
    pub publisher: PublisherRowView,
    pub query: String,
    pub sort: String,
    pub total: usize,
    pub found_heading: String,
    pub facet_groups: Vec<FacetGroupView>,
    pub datasets: Vec<DatasetRowView>,
    pub dataset_grid_config: String,
    pub pager: Vec<listing::PageLinkView>,
    pub filter_pills: Vec<listing::FilterPillView>,
    pub remove_label: String,
    pub hidden_params: Vec<(String, String)>,
}

/// About page (`/about`, `templates/about.html`, D6).
#[derive(Template)]
#[template(path = "about.html")]
pub struct AboutPageTemplate {
    pub chrome: PageChrome,
    pub labels: UiLabels,
    pub intro: String,
    pub api_url: String,
    pub dcat_url: String,
    pub email: String,
}

/// Static page (`/connector`, `/privacy`, `/terms`, `/support`, `/accessibility`, `templates/static_page.html`, D6).
#[derive(Template)]
#[template(path = "static_page.html")]
pub struct StaticPageTemplate {
    pub chrome: PageChrome,
    pub labels: UiLabels,
    pub page: StaticPageView,
    pub mcp: Option<McpInstallView>,
}

/// Assistants overview page (`/assistants`, `templates/assistants.html`, D6).
#[derive(Template)]
#[template(path = "assistants.html")]
pub struct AssistantsPageTemplate {
    pub chrome: PageChrome,
    pub labels: UiLabels,
    pub lead: String,
    pub prompt: String,
    pub cards: Vec<(String, String)>,
    pub mcp: Option<McpInstallView>,
}

/// Assistant step-by-step guide page (`/assistants/{id}`, `templates/guide.html`, D6).
#[derive(Template)]
#[template(path = "guide.html")]
pub struct GuidePageTemplate {
    pub chrome: PageChrome,
    pub labels: UiLabels,
    pub guide: AssistantGuideView,
    pub mcp: McpInstallView,
}

/// Framable map page (`/map/{id}`, `templates/map.html`, D6).
#[derive(Template)]
#[template(path = "map.html")]
pub struct MapPageTemplate {
    pub chrome: PageChrome,
    pub labels: UiLabels,
    pub dataset_title: String,
    pub dataset_url: String,
    pub map_config_json: String,
    pub has_pmtiles: bool,
}

fn to_dataset_row_view(
    ds: &Dataset,
    pub_map: &BTreeMap<String, Publisher>,
    lang: &str,
    default_lang: &str,
) -> DatasetRowView {
    let pub_rec = pub_map.get(&ds.publisher_id);
    let pub_title = pub_rec
        .and_then(|p| localized_text(&p.title, lang, default_lang).map(str::to_owned))
        .unwrap_or_else(|| ds.publisher_id.clone());
    let raw_title = localized_text(&ds.title, lang, default_lang).unwrap_or(&ds.name);
    let raw_notes = localized_text(&ds.description, lang, default_lang).unwrap_or("");
    let clean_notes = jc_plain_glyphs(raw_notes);
    let notes_excerpt: String = if clean_notes.chars().count() > 180 {
        format!(
            "{}...",
            clean_notes.chars().take(180).collect::<String>().trim_end()
        )
    } else {
        clean_notes
    };
    let chips = jc_format_chips(ds);
    DatasetRowView {
        name: ds.name.clone(),
        title: jc_plain_glyphs(raw_title),
        notes: notes_excerpt,
        publisher: ds.publisher_id.clone(),
        publisher_title: pub_title,
        formats: chips.join(", "),
        format_chips: chips,
        modified: ds.modified.format("%Y-%m-%d").to_string(),
        modified_iso: ds.modified.to_rfc3339(),
        modified_localized: format_date_localized(
            ds.modified.year(),
            ds.modified.month(),
            ds.modified.day(),
            lang,
        ),
    }
}

/// Renders the catalogue home page (D1).
pub fn render_home_page(
    catalogue: &Catalogue,
    datasets: &[Dataset],
    publishers: &[Publisher],
    query: &str,
    lang: &str,
    csp_nonce: &str,
) -> String {
    render_home_page_mounted(
        catalogue, None, datasets, publishers, query, lang, false, "", csp_nonce,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn render_home_page_mounted(
    catalogue: &Catalogue,
    installation_branding: Option<&BrandingBlock>,
    datasets: &[Dataset],
    publishers: &[Publisher],
    query: &str,
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
        "/",
        None,
        mount_prefix,
        "home",
        csp_nonce,
    );
    let labels = UiLabels::for_lang(&chrome.lang);
    let pub_map: BTreeMap<String, Publisher> = publishers
        .iter()
        .flat_map(|p| [(p.id.clone(), p.clone()), (p.name.clone(), p.clone())])
        .collect();

    // Filter out hidden datasets for public home page (`jc_hide`, D1, D2)
    let public_ds: Vec<&Dataset> = datasets
        .iter()
        .filter(|d| {
            d.state == RecordState::Active
                && d.visibility == Visibility::Public
                && !catalogue.hidden_datasets.contains_key(&d.name)
        })
        .collect();

    // Topic tiles (`jc_topic_tiles` in `plugin.py` line 2060, D1)
    let mut theme_counts: BTreeMap<String, usize> = BTreeMap::new();
    for ds in &public_ds {
        let mut ds_themes = std::collections::BTreeSet::new();
        for t in &ds.themes {
            let code = theme_code(t).to_ascii_uppercase();
            ds_themes.insert(code);
        }
        for c in ds_themes {
            *theme_counts.entry(c).or_insert(0) += 1;
        }
    }
    let mut topic_tiles: Vec<(usize, TopicTileView)> = theme_counts
        .into_iter()
        .filter(|(_, c)| *c > 0)
        .map(|(code, count)| {
            let label = dataroom_core::vocab::theme_label(&code, &chrome.lang);
            let url = format!("{}/dataset/?vocab_jc_topic={code}", chrome.lang_prefix);
            (
                count,
                TopicTileView {
                    code,
                    label,
                    count_label: jc_rows(count, "datasets_n", &chrome.lang),
                    url,
                },
            )
        })
        .collect();
    topic_tiles.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.label.cmp(&b.1.label)));
    let topic_tiles: Vec<TopicTileView> = topic_tiles.into_iter().map(|(_, t)| t).collect();

    // Recent 6 datasets (`jc_recent(6)` in `plugin.py` line 2092, D1)
    let mut recent_sorted = public_ds.clone();
    recent_sorted.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| a.name.cmp(&b.name))
    });
    let recent_datasets: Vec<DatasetRowView> = recent_sorted
        .iter()
        .take(6)
        .map(|d| to_dataset_row_view(d, &pub_map, &chrome.lang, &catalogue.default_language))
        .collect();

    let ds_rows: Vec<DatasetRowView> = public_ds
        .iter()
        .take(50)
        .map(|d| to_dataset_row_view(d, &pub_map, &chrome.lang, &catalogue.default_language))
        .collect();

    // Publisher strip with dataset counts, sorted by count desc (D1)
    let mut pub_rows: Vec<PublisherRowView> = publishers
        .iter()
        .filter(|p| p.state == RecordState::Active)
        .map(|p| {
            let count = public_ds
                .iter()
                .filter(|d| d.publisher_id == p.name || d.publisher_id == p.id)
                .count();
            let (title, image, desc) =
                publisher_card_info(
                    p,
                    chrome.branding.publishers.get(&p.name),
                    &chrome.lang,
                    &catalogue.default_language,
                );
            let d_str = desc.unwrap_or_default();
            let d_short = if d_str.chars().count() > 80 {
                format!(
                    "{}...",
                    d_str.chars().take(80).collect::<String>().trim_end()
                )
            } else {
                d_str.clone()
            };
            PublisherRowView {
                name: p.name.clone(),
                title,
                uri: p.uri.clone(),
                image,
                description: d_str,
                description_short: d_short,
                count,
            }
        })
        .collect();
    pub_rows.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.title.cmp(&b.title)));

    // Popular keywords (top 12, D1)
    let mut tag_counts: BTreeMap<String, usize> = BTreeMap::new();
    for ds in &public_ds {
        for t in &ds.tags {
            *tag_counts.entry(t.clone()).or_insert(0) += 1;
        }
    }
    let mut tag_vec: Vec<(String, usize)> = tag_counts.into_iter().collect();
    tag_vec.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let popular_keywords: Vec<(String, String)> = tag_vec
        .into_iter()
        .take(12)
        .map(|(tag, _)| {
            let enc: String = url::form_urlencoded::byte_serialize(tag.as_bytes()).collect();
            (tag, format!("{}/dataset/?tags={enc}", chrome.lang_prefix))
        })
        .collect();

    let odata_base = format!("{}/odata/v4", chrome.mount_prefix);
    let ds_grid = odata_grid_config(
        &odata_base,
        "Datasets",
        "Dataset",
        &[
            ("title", &labels.col_title, "text"),
            ("publisher_id", &labels.col_publisher, "text"),
            ("modified", &labels.col_modified, "date"),
        ],
    );
    let pub_grid = odata_grid_config(
        &odata_base,
        "Publishers",
        "Publisher",
        &[("title", &labels.col_title, "text"), ("uri", "IRI", "link")],
    );

    let tpl = HomePageTemplate {
        lang: chrome.lang.clone(),
        title: labels.welcome.clone(),
        catalogue_title: chrome.site_title.clone(),
        catalogue_description: chrome.site_tagline.clone(),
        query: query.to_owned(),
        total_datasets: public_ds.len(),
        total_datasets_formatted: jc_number(public_ds.len()),
        total_publishers_formatted: jc_number(pub_rows.len()),
        topic_tiles,
        recent_datasets,
        datasets: ds_rows,
        publishers: pub_rows,
        popular_keywords,
        dataset_grid_config: ds_grid.to_string(),
        publisher_grid_config: pub_grid.to_string(),
        theme: catalogue.theme.clone(),
        labels,
        csp_nonce: csp_nonce.to_owned(),
        chrome,
    };
    tpl.render().unwrap_or_default()
}

/// Renders the `/dataset/` search and facet page (D2).
#[allow(clippy::too_many_arguments)]
pub fn render_dataset_list_page(
    catalogue: &Catalogue,
    installation_branding: Option<&BrandingBlock>,
    datasets: &[Dataset],
    publishers: &[Publisher],
    facets: &SearchFacets,
    total: usize,
    page: usize,
    query: &str,
    sort: &str,
    active_params: &BTreeMap<String, String>,
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
        "/dataset/",
        None,
        mount_prefix,
        "dataset",
        csp_nonce,
    );
    let labels = UiLabels::for_lang(&chrome.lang);
    let pub_map: BTreeMap<String, Publisher> = publishers
        .iter()
        .flat_map(|p| [(p.id.clone(), p.clone()), (p.name.clone(), p.clone())])
        .collect();

    let ds_rows: Vec<DatasetRowView> = datasets
        .iter()
        .map(|d| to_dataset_row_view(d, &pub_map, &chrome.lang, &catalogue.default_language))
        .collect();

    let base_path = format!("{}/dataset/", chrome.lang_prefix);
    let facet_groups =
        build_dataset_facet_groups(facets, total, active_params, &base_path, &chrome.lang);

    let filter_pills = dataset_filter_pills(
        facets,
        &pub_map,
        active_params,
        &base_path,
        &chrome.lang,
        &catalogue.default_language,
    );
    let pager = listing::pager(&base_path, active_params, total, page);
    let odata_base = format!("{}/odata/v4", chrome.mount_prefix);
    let ds_grid = odata_grid_config(
        &odata_base,
        "Datasets",
        "Dataset",
        &[
            ("title", &labels.col_title, "text"),
            ("publisher_id", &labels.col_publisher, "text"),
            ("modified", &labels.col_modified, "date"),
        ],
    );

    let hidden_params = hidden_filter_params(active_params);
    let tpl = DatasetListPageTemplate {
        found_heading: found_heading(total, query, &chrome.lang),
        remove_label: jc_t("remove", &chrome.lang),
        chrome,
        labels,
        query: query.to_owned(),
        sort: sort.to_owned(),
        total,
        facet_groups,
        filter_pills,
        hidden_params,
        datasets: ds_rows,
        dataset_grid_config: ds_grid.to_string(),
        page,
        pager,
    };
    tpl.render().unwrap_or_default()
}

/// The active-filter pills of a dataset list (CKAN's `p.filter-list`): each names its facet and
/// the value as the facet shows it, with a link that drops only that filter (G5).
fn dataset_filter_pills(
    facets: &SearchFacets,
    publishers: &BTreeMap<String, Publisher>,
    active_params: &BTreeMap<String, String>,
    base_path: &str,
    lang: &str,
    default_lang: &str,
) -> Vec<listing::FilterPillView> {
    let format = jc_t("format", lang);
    let defs = [
        ("organization", jc_t("publisher", lang)),
        ("publisher", jc_t("publisher", lang)),
        ("vocab_jc_topic", jc_t("topic", lang)),
        ("theme", jc_t("topic", lang)),
        ("tags", jc_t("keywords", lang)),
        ("keyword", jc_t("keywords", lang)),
        ("vocab_jc_format", format.clone()),
        ("res_format", format),
        ("license_id", jc_t("licence", lang)),
        ("licence", jc_t("licence", lang)),
        ("spatial", jc_t("coverage", lang)),
    ];
    listing::filter_pills(base_path, active_params, &defs, |param, value| {
        let listed = |list: &[dataroom_search::FacetCount]| {
            list.iter()
                .find(|f| f.value == value)
                .map(|f| f.label.clone())
        };
        match param {
            "organization" | "publisher" => publishers
                .get(value)
                .map(|p| publisher_card_info(p, None, lang, default_lang).0)
                .or_else(|| listed(&facets.publisher)),
            "vocab_jc_topic" | "theme" => {
                Some(dataroom_core::vocab::theme_label(value, lang)).filter(|l| !l.is_empty())
            }
            "vocab_jc_format" | "res_format" => Some(helpers::data_format_label(value)),
            "license_id" | "licence" => listed(&facets.licence),
            "spatial" => Some(helpers::nuts_label(value, lang)),
            _ => listed(&facets.keyword),
        }
        .unwrap_or_else(|| value.to_owned())
    })
}

/// The request's filters a search form carries in hidden fields (CKAN's `form.hidden_from_list`):
/// everything but the query, the sort and the page.
fn hidden_filter_params(params: &BTreeMap<String, String>) -> Vec<(String, String)> {
    params
        .iter()
        .filter(|(k, v)| {
            !v.is_empty()
                && !matches!(k.as_str(), "q" | "sort" | "page" | "lang")
                && !k.starts_with('_')
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// "N datasets found", with the query when there is one (CKAN's `search_result_text`).
fn found_heading(total: usize, query: &str, lang: &str) -> String {
    let key = if query.trim().is_empty() {
        "datasets_found"
    } else {
        "datasets_found_query"
    };
    jc_t(key, lang)
        .replace("{n}", &total.to_string())
        .replace("{q}", query.trim())
}

fn build_dataset_facet_groups(
    facets: &SearchFacets,
    total: usize,
    active_params: &BTreeMap<String, String>,
    base_path: &str,
    lang: &str,
) -> Vec<FacetGroupView> {
    let mut groups = Vec::new();
    let defs = [
        ("organization", jc_t("publisher", lang), &facets.publisher),
        ("vocab_jc_topic", jc_t("topic", lang), &facets.theme),
        ("tags", jc_t("keywords", lang), &facets.keyword),
        ("vocab_jc_format", jc_t("format", lang), &facets.format),
        ("license_id", jc_t("licence", lang), &facets.licence),
    ];

    for (param, title, list) in defs {
        let active_val = active_params.get(param).map(String::as_str);
        let mut items = Vec::new();
        for fc in list.iter().take(10) {
            let is_active = active_val == Some(fc.value.as_str());
            // `jc_facet_labels` rule (`plugin.py` line 2049): Topic leaves out a value every result
            // shares unless the search already filters by it.
            if param == "vocab_jc_topic" && fc.count == total && total > 0 && !is_active {
                continue;
            }
            // A publisher filter narrows the publisher facet to the chosen one, as CKAN's
            // facets come from the result set (the search keeps every publisher, A3).
            if param == "organization" && active_val.is_some() && !is_active {
                continue;
            }
            let href = listing::link_with(
                base_path,
                active_params,
                &[(param, (!is_active).then_some(fc.value.as_str()))],
            );
            let display = if param == "vocab_jc_format" {
                helpers::data_format_label(&fc.value)
            } else {
                fc.label.clone()
            };
            items.push(FacetItemView {
                name: fc.value.clone(),
                label: display,
                count: fc.count,
                href,
                active: is_active,
            });
        }
        if !items.is_empty() {
            let more_href = (list.len() > 10).then(|| {
                let switch = format!("_{param}_limit");
                let mut url = listing::link_with(base_path, active_params, &[]);
                url.push(if url.contains('?') { '&' } else { '?' });
                url.push_str(&switch);
                url.push_str("=0");
                url
            });
            groups.push(FacetGroupView {
                param: param.to_owned(),
                title,
                items,
                more_href,
            });
        }
    }

    if !facets.spatial.is_empty() {
        let param = "spatial";
        let active_val = active_params.get(param).map(String::as_str);
        let items: Vec<FacetItemView> = facets
            .spatial
            .iter()
            .take(10)
            .map(|fc| {
                let is_active = active_val == Some(fc.value.as_str());
                FacetItemView {
                    name: fc.value.clone(),
                    label: helpers::nuts_label(&fc.value, lang),
                    count: fc.count,
                    href: listing::link_with(
                        base_path,
                        active_params,
                        &[(param, (!is_active).then_some(fc.value.as_str()))],
                    ),
                    active: is_active,
                }
            })
            .collect();
        groups.push(FacetGroupView {
            param: param.to_owned(),
            title: jc_t("coverage", lang),
            items,
            more_href: None,
        });
    }

    groups
}

/// Renders the dataset detail page (D3).
pub fn render_dataset_page(
    catalogue: &Catalogue,
    dataset: &Dataset,
    publisher: Option<&Publisher>,
    shacl_reports: &[ShaclValidationReport],
    show_editor_panel: bool,
    lang: &str,
    csp_nonce: &str,
) -> String {
    render_dataset_page_full(
        catalogue,
        None,
        dataset,
        publisher,
        &[],
        shacl_reports,
        show_editor_panel,
        lang,
        false,
        "",
        csp_nonce,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn render_dataset_page_full(
    catalogue: &Catalogue,
    installation_branding: Option<&BrandingBlock>,
    dataset: &Dataset,
    publisher: Option<&Publisher>,
    views: &[ResourceView],
    shacl_reports: &[ShaclValidationReport],
    show_editor_panel: bool,
    lang: &str,
    had_url_prefix: bool,
    mount_prefix: &str,
    csp_nonce: &str,
) -> String {
    let stripped = format!("/dataset/{}", dataset.name);
    let chrome = PageChrome::build(
        catalogue,
        installation_branding,
        lang,
        had_url_prefix,
        &stripped,
        None,
        mount_prefix,
        "dataset",
        csp_nonce,
    );
    let labels = UiLabels::for_lang(&chrome.lang);
    let dataset_title = jc_plain_glyphs(
        localized_text(&dataset.title, &chrome.lang, &catalogue.default_language)
            .unwrap_or(&dataset.name),
    );
    let dataset_description = jc_plain_glyphs(
        localized_text(
            &dataset.description,
            &chrome.lang,
            &catalogue.default_language,
        )
        .unwrap_or(""),
    );

    let publisher_slug = publisher
        .map(|p| p.name.clone())
        .unwrap_or_else(|| dataset.publisher_id.clone());
    let (publisher_title, publisher_image, pub_desc_opt) = match publisher {
        Some(p) => publisher_card_info(
                    p,
                    chrome.branding.publishers.get(&p.name),
                    &chrome.lang,
                    &catalogue.default_language,
                ),
        None => (
            publisher_slug.clone(),
            "/base/images/placeholder-organization.png".into(),
            None,
        ),
    };
    let publisher_description = pub_desc_opt.unwrap_or_default();

    let licence_title = dataset
        .licence_id
        .as_deref()
        .and_then(find_licence)
        .map(|l| l.title)
        .or_else(|| dataset.licence_id.clone())
        .unwrap_or_default();

    let odata_base = format!("{}/odata/v4", chrome.mount_prefix);
    let explorer = jc_explorer(dataset, None, &chrome.branding, &chrome.lang, &odata_base);
    let charts = jc_visualizations(
        dataset,
        views,
        &chrome.branding,
        &chrome.lang,
        &chrome.lang_prefix,
    );
    // The explorer draws its own map of the same Endpoint (as on CKAN, where the map card sits
    // only beside the DataStore tables): a second map card would ask the same reads again.
    let map_cfg = explorer
        .is_none()
        .then(|| jc_map_config(catalogue, dataset, None, &chrome.branding, &chrome.lang))
        .flatten();
    let map_embed_url = map_cfg
        .as_ref()
        .and_then(|m| m.get("embed").and_then(Value::as_str))
        .unwrap_or("")
        .to_owned();
    let map_config_json = map_cfg.map(|m| m.to_string());
    let subscriptions = jc_subscriptions(dataset, &chrome.lang, &catalogue.default_language);

    let all_about = jc_about(
        dataset,
        publisher,
        &chrome.lang,
        &catalogue.default_language,
    );
    let src_label = jc_t("source", &chrome.lang);
    let source_rows: Vec<AboutRow> = all_about
        .iter()
        .filter(|r| r.label == src_label)
        .cloned()
        .collect();
    let fold_sources = source_rows.len() > 3;
    let sources_all_label =
        jc_t("sources_all", &chrome.lang).replace("{n}", &source_rows.len().to_string());
    let about_rows: Vec<AboutRow> = if fold_sources {
        let mut seen_src = false;
        all_about
            .into_iter()
            .filter(|r| {
                if r.label == src_label {
                    if seen_src {
                        return false;
                    }
                    seen_src = true;
                }
                true
            })
            .collect()
    } else {
        all_about
    };

    let live = jc_live(dataset, &chrome.branding);
    let mcp = jc_mcp_install(&chrome.branding);
    let resource_sections = jc_resource_sections(
        dataset,
        &chrome.lang,
        &catalogue.default_language,
        &chrome.lang_prefix,
    );
    let more_rows = jc_more_rows(dataset, &chrome.lang);

    let dist_rows: Vec<DistributionRowView> = dataset
        .distributions
        .iter()
        .map(|d| DistributionRowView {
            title: jc_resource_label(d, dataset, &chrome.lang, &catalogue.default_language),
            format: jc_resource_format_chip(d),
            url: d.download_url.as_ref().unwrap_or(&d.access_url).clone(),
        })
        .collect();

    let dist_grid = odata_grid_config(
        &odata_base,
        "Distributions",
        "Distribution",
        &[
            ("name", &labels.col_title, "text"),
            ("format", &labels.col_formats, "text"),
            ("access_url", "URL", "link"),
        ],
    );
    let shacl_grid = odata_grid_config(
        &odata_base,
        "ShaclResults",
        "ShaclResult",
        &[
            ("profile_id", "Profile", "text"),
            ("profile_version", "Version", "text"),
            ("violation_count", "Violations", "number"),
        ],
    );

    let schema_org_jsonld = jc_schema_org_script(
        catalogue,
        dataset,
        publisher,
        &chrome.branding,
        &chrome.lang,
    );

    let tpl = DatasetPageTemplate {
        lang: chrome.lang.clone(),
        catalogue_title: chrome.site_title.clone(),
        dataset_id: dataset.id.clone(),
        dataset_name: dataset.name.clone(),
        dataset_title,
        dataset_description,
        publisher_slug,
        publisher_title,
        publisher_image,
        publisher_description,
        licence_title,
        endpoint_url: dataset.endpoint_url.clone().unwrap_or_default(),
        explorer,
        charts,
        map_config_json,
        map_embed_url,
        subscriptions,
        about_rows,
        source_rows,
        fold_sources,
        sources_all_label,
        live,
        mcp,
        resource_sections,
        more_rows,
        distributions: dist_rows,
        distributions_grid_config: dist_grid.to_string(),
        show_editor_panel,
        shacl_reports: shacl_reports.to_vec(),
        shacl_grid_config: shacl_grid.to_string(),
        schema_org_jsonld,
        theme: catalogue.theme.clone(),
        labels,
        csp_nonce: csp_nonce.to_owned(),
        chrome,
    };
    tpl.render().unwrap_or_default()
}

/// Renders the resource detail page (`/dataset/{name}/resource/{id}`, D4).
#[allow(clippy::too_many_arguments)]
pub fn render_resource_page(
    catalogue: &Catalogue,
    installation_branding: Option<&BrandingBlock>,
    dataset: &Dataset,
    dist: &Distribution,
    publisher: Option<&Publisher>,
    lang: &str,
    had_url_prefix: bool,
    mount_prefix: &str,
    csp_nonce: &str,
) -> String {
    let stripped = format!("/dataset/{}/resource/{}", dataset.name, dist.id);
    let chrome = PageChrome::build(
        catalogue,
        installation_branding,
        lang,
        had_url_prefix,
        &stripped,
        None,
        mount_prefix,
        "dataset",
        csp_nonce,
    );
    let labels = UiLabels::for_lang(&chrome.lang);
    let dataset_title = localized_text(&dataset.title, &chrome.lang, &catalogue.default_language)
        .unwrap_or(&dataset.name)
        .to_owned();
    let publisher_slug = publisher
        .map(|p| p.name.clone())
        .unwrap_or_else(|| dataset.publisher_id.clone());
    let publisher_title = publisher
        .and_then(|p| {
            localized_text(&p.title, &chrome.lang, &catalogue.default_language).map(str::to_owned)
        })
        .unwrap_or_else(|| publisher_slug.clone());

    let resource_label =
        jc_resource_label(dist, dataset, &chrome.lang, &catalogue.default_language);
    let format_chip = jc_resource_format_chip(dist);
    let entity_type = helpers::jc_entity_type(dist).unwrap_or_default();
    let what = jc_what(dist, &chrome.lang);
    let description = localized_text(&dist.description, &chrome.lang, &catalogue.default_language)
        .unwrap_or("")
        .to_owned();

    let sec = helpers::resource_section_key(dist, dataset);
    let is_api = sec == "api";
    let is_source = jc_is_source(dist, dataset);
    let url = dist
        .download_url
        .as_ref()
        .unwrap_or(&dist.access_url)
        .clone();
    let link_url = if helpers::is_url(&url) {
        url.clone()
    } else {
        String::new()
    };
    let short_segments: Vec<String> = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .map(str::to_owned)
        .collect();
    let full_breakable = render_breakable_html(&url);

    let odata_base = format!("{}/odata/v4", chrome.mount_prefix);
    let explorer = if matches!(jc_kind(dist), Some("NGSI-LD" | "OGCFEAT" | "ODATA" | "STA")) {
        jc_explorer(
            dataset,
            Some(dist),
            &chrome.branding,
            &chrome.lang,
            &odata_base,
        )
    } else {
        None
    };
    let preview = if explorer.is_none() {
        jc_preview(dist, &chrome.branding, &chrome.lang)
    } else {
        None
    };
    let examples = jc_examples(dist, &chrome.lang);
    let guide_url = (jc_kind(dist) == Some("WEBSOCKET")
        && chrome
            .branding
            .documentation_base_url
            .starts_with("https://"))
    .then(|| {
        format!(
            "{}/API/07-websocket",
            chrome.branding.documentation_base_url.trim_end_matches('/')
        )
    });
    let developer_page_url = jc_developer_page(dataset, &chrome.branding, &chrome.lang);

    let updated_dt = dist.modified.unwrap_or(dataset.modified);
    let mut dates = vec![(
        jc_t("updated", &chrome.lang),
        format_date_localized(
            updated_dt.year(),
            updated_dt.month(),
            updated_dt.day(),
            &chrome.lang,
        ),
    )];
    let created_dt = dist.issued.unwrap_or(dataset.issued);
    if created_dt.date_naive() != updated_dt.date_naive() {
        dates.push((
            jc_t("created", &chrome.lang),
            format_date_localized(
                created_dt.year(),
                created_dt.month(),
                created_dt.day(),
                &chrome.lang,
            ),
        ));
    }

    let lic_entry = dataset
        .licence_id
        .as_deref()
        .and_then(find_licence)
        .or_else(|| dataset.licence_url.as_deref().and_then(find_licence));
    let licence_title = lic_entry
        .as_ref()
        .map(|l| l.title.clone())
        .or_else(|| dataset.licence_id.clone())
        .unwrap_or_default();
    let licence_url = dataset
        .licence_url
        .clone()
        .or_else(|| lic_entry.map(|l| l.url))
        .filter(|u| helpers::is_url(u))
        .unwrap_or_default();

    let standard = jc_standard(dist);
    let model = jc_model(dataset, &chrome.lang, &catalogue.default_language);
    let resource_sections = jc_resource_sections(
        dataset,
        &chrome.lang,
        &catalogue.default_language,
        &chrome.lang_prefix,
    );

    let tpl = ResourcePageTemplate {
        resources_heading: jc_t("resources_nav", &chrome.lang),
        chrome,
        labels,
        dataset_name: dataset.name.clone(),
        dataset_title,
        publisher_slug,
        publisher_title,
        resource_id: dist.id.clone(),
        resource_label,
        format_chip,
        entity_type,
        what,
        description,
        is_api,
        is_source,
        url,
        link_url,
        short_segments,
        full_breakable,
        explorer,
        preview,
        examples,
        guide_url,
        developer_page_url,
        dates,
        licence_title,
        licence_url,
        standard,
        model,
        resource_sections,
    };
    tpl.render().unwrap_or_default()
}

/// Renders the publisher list page (`/organization/`, D5).
#[allow(clippy::too_many_arguments)]
pub fn render_organization_list_page(
    catalogue: &Catalogue,
    installation_branding: Option<&BrandingBlock>,
    publishers: &[Publisher],
    datasets: &[Dataset],
    query: &str,
    sort: &str,
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
        "/organization/",
        None,
        mount_prefix,
        "organization",
        csp_nonce,
    );
    let labels = UiLabels::for_lang(&chrome.lang);
    let public_ds: Vec<&Dataset> = datasets
        .iter()
        .filter(|d| {
            d.state == RecordState::Active
                && d.visibility == Visibility::Public
                && !catalogue.hidden_datasets.contains_key(&d.name)
        })
        .collect();

    let q_lower = query.trim().to_lowercase();
    let mut pub_rows: Vec<PublisherRowView> = publishers
        .iter()
        .filter(|p| p.state == RecordState::Active)
        .filter_map(|p| {
            let (title, image, desc) =
                publisher_card_info(
                    p,
                    chrome.branding.publishers.get(&p.name),
                    &chrome.lang,
                    &catalogue.default_language,
                );
            if !q_lower.is_empty()
                && !title.to_lowercase().contains(&q_lower)
                && !p.name.to_lowercase().contains(&q_lower)
            {
                return None;
            }
            let count = public_ds
                .iter()
                .filter(|d| d.publisher_id == p.name || d.publisher_id == p.id)
                .count();
            let d_str = desc.unwrap_or_default();
            let d_short = if d_str.chars().count() > 80 {
                format!(
                    "{}...",
                    d_str.chars().take(80).collect::<String>().trim_end()
                )
            } else {
                d_str.clone()
            };
            Some(PublisherRowView {
                name: p.name.clone(),
                title,
                uri: p.uri.clone(),
                image,
                description: d_str,
                description_short: d_short,
                count,
            })
        })
        .collect();

    if sort == "title desc" {
        pub_rows.sort_by(|a, b| b.title.cmp(&a.title));
    } else {
        pub_rows.sort_by(|a, b| a.title.cmp(&b.title));
    }

    let found_heading =
        jc_t("organizations_found", &chrome.lang).replace("{n}", &pub_rows.len().to_string());
    let odata_base = format!("{}/odata/v4", chrome.mount_prefix);
    let pub_grid = odata_grid_config(
        &odata_base,
        "Publishers",
        "Publisher",
        &[("title", &labels.col_title, "text"), ("uri", "IRI", "link")],
    );

    let tpl = OrganizationListPageTemplate {
        chrome,
        labels,
        query: query.to_owned(),
        sort: sort.to_owned(),
        found_heading,
        publishers: pub_rows,
        publisher_grid_config: pub_grid.to_string(),
    };
    tpl.render().unwrap_or_default()
}

/// Renders an individual publisher's page (`/organization/{name}` or `/publisher/{name}`, D5).
#[allow(clippy::too_many_arguments)]
pub fn render_organization_read_page(
    catalogue: &Catalogue,
    installation_branding: Option<&BrandingBlock>,
    publisher: &Publisher,
    datasets: &[Dataset],
    facets: &SearchFacets,
    total: usize,
    page: usize,
    query: &str,
    sort: &str,
    active_params: &BTreeMap<String, String>,
    lang: &str,
    had_url_prefix: bool,
    mount_prefix: &str,
    csp_nonce: &str,
) -> String {
    let stripped = format!("/organization/{}", publisher.name);
    let chrome = PageChrome::build(
        catalogue,
        installation_branding,
        lang,
        had_url_prefix,
        &stripped,
        None,
        mount_prefix,
        "organization",
        csp_nonce,
    );
    let labels = UiLabels::for_lang(&chrome.lang);
    let (title, image, desc) =
        publisher_card_info(
                    publisher,
                    chrome.branding.publishers.get(&publisher.name),
                    &chrome.lang,
                    &catalogue.default_language,
                );
    let d_str = desc.unwrap_or_default();
    let pub_view = PublisherRowView {
        name: publisher.name.clone(),
        title,
        uri: publisher.uri.clone(),
        image,
        description_short: d_str.clone(),
        description: d_str,
        count: total,
    };

    let pub_map = BTreeMap::from([
        (publisher.id.clone(), publisher.clone()),
        (publisher.name.clone(), publisher.clone()),
    ]);
    let ds_rows: Vec<DatasetRowView> = datasets
        .iter()
        .map(|d| to_dataset_row_view(d, &pub_map, &chrome.lang, &catalogue.default_language))
        .collect();

    let base_path = format!("{}/organization/{}", chrome.lang_prefix, publisher.name);
    let mut facet_groups = Vec::new();
    let org_defs = [
        (
            "organization",
            labels.nav_publishers.clone(),
            &facets.publisher,
            "",
        ),
        (
            "tags",
            labels.tags_label.clone(),
            &facets.keyword,
            "_tags_limit=0",
        ),
        (
            "res_format",
            labels.formats_label.clone(),
            &facets.format,
            "_res_format_limit=0",
        ),
        (
            "license_id",
            labels.licences_label.clone(),
            &facets.licence,
            "",
        ),
    ];
    for (param, gtitle, list, more_q) in org_defs {
        if list.is_empty() {
            continue;
        }
        let active_val = active_params.get(param).map(String::as_str);
        let items: Vec<FacetItemView> = list
            .iter()
            // The page is one publisher's: its publisher facet names only that publisher (G4).
            .filter(|fc| {
                param != "organization" || fc.value == publisher.name || fc.value == publisher.id
            })
            .take(10)
            .map(|fc| {
                let active = active_val == Some(fc.value.as_str());
                let href = if param == "organization" {
                    base_path.clone()
                } else {
                    listing::link_with(
                        &base_path,
                        active_params,
                        &[(param, (!active).then_some(fc.value.as_str()))],
                    )
                };
                FacetItemView {
                    name: fc.value.clone(),
                    label: fc.label.clone(),
                    count: fc.count,
                    href,
                    active,
                }
            })
            .collect();
        if items.is_empty() {
            continue;
        }
        let more_href = (!more_q.is_empty()).then(|| {
            let mut url = listing::link_with(&base_path, active_params, &[]);
            url.push(if url.contains('?') { '&' } else { '?' });
            url.push_str(more_q);
            url
        });
        facet_groups.push(FacetGroupView {
            param: param.to_owned(),
            title: gtitle,
            items,
            more_href,
        });
    }

    let odata_base = format!("{}/odata/v4", chrome.mount_prefix);
    let ds_grid = odata_grid_config(
        &odata_base,
        "Datasets",
        "Dataset",
        &[
            ("title", &labels.col_title, "text"),
            ("publisher_id", &labels.col_publisher, "text"),
            ("modified", &labels.col_modified, "date"),
        ],
    );
    let mut pill_params = active_params.clone();
    pill_params.remove("organization");
    let filter_pills = dataset_filter_pills(
        facets,
        &pub_map,
        &pill_params,
        &base_path,
        &chrome.lang,
        &catalogue.default_language,
    );
    let pager = listing::pager(&base_path, active_params, total, page);

    let tpl = OrganizationReadPageTemplate {
        found_heading: found_heading(total, query, &chrome.lang),
        remove_label: jc_t("remove", &chrome.lang),
        hidden_params: hidden_filter_params(&pill_params),
        filter_pills,
        pager,
        chrome,
        labels,
        publisher: pub_view,
        query: query.to_owned(),
        sort: sort.to_owned(),
        total,
        facet_groups,
        datasets: ds_rows,
        dataset_grid_config: ds_grid.to_string(),
    };
    tpl.render().unwrap_or_default()
}

/// Renders the `/about` page (D6).
pub fn render_about_page(
    catalogue: &Catalogue,
    installation_branding: Option<&BrandingBlock>,
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
        "/about",
        None,
        mount_prefix,
        "about",
        csp_nonce,
    );
    let labels = UiLabels::for_lang(&chrome.lang);
    let name = &chrome.branding.instance_name;
    let org = &chrome.branding.organisation;
    let intro = if !org.is_empty() {
        jc_t("about_intro_org", &chrome.lang)
            .replace("{name}", name)
            .replace("{org}", org)
    } else {
        jc_t("about_intro", &chrome.lang).replace("{name}", name)
    };
    let base = catalogue.base_url();
    let api_url = format!("{base}{}/api/3/action/package_list", chrome.mount_prefix);
    let dcat_url = format!("{base}{}/catalog.ttl", chrome.mount_prefix);
    let email = if chrome.branding.contact_email.contains('@') {
        chrome.branding.contact_email.clone()
    } else {
        String::new()
    };

    let tpl = AboutPageTemplate {
        chrome,
        labels,
        intro,
        api_url,
        dcat_url,
        email,
    };
    tpl.render().unwrap_or_default()
}

/// Renders one of the 5 static pages (`/connector`, `/privacy`, `/terms`, `/support`, `/accessibility`, D6).
pub fn render_static_page(
    catalogue: &Catalogue,
    installation_branding: Option<&BrandingBlock>,
    name: &str,
    lang: &str,
    had_url_prefix: bool,
    mount_prefix: &str,
    csp_nonce: &str,
) -> Option<String> {
    let stripped = format!("/{name}");
    let chrome = PageChrome::build(
        catalogue,
        installation_branding,
        lang,
        had_url_prefix,
        &stripped,
        None,
        mount_prefix,
        "",
        csp_nonce,
    );
    let page = jc_page(name, &chrome.branding, &chrome.lang)?;
    let labels = UiLabels::for_lang(&chrome.lang);
    let mcp = (name == "connector")
        .then(|| jc_mcp_install(&chrome.branding))
        .flatten();
    let tpl = StaticPageTemplate {
        chrome,
        labels,
        page,
        mcp,
    };
    tpl.render().ok()
}

/// Renders the `/assistants` page (D6).
pub fn render_assistants_page(
    catalogue: &Catalogue,
    installation_branding: Option<&BrandingBlock>,
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
        "/assistants",
        None,
        mount_prefix,
        "",
        csp_nonce,
    );
    let labels = UiLabels::for_lang(&chrome.lang);
    let lead = jc_t("assistants_lead", &chrome.lang).replace("{name}", CONNECTOR_NAME);
    let prompt = jc_try_prompt(&chrome.branding, &chrome.lang);
    let cards = vec![
        ("chatgpt".into(), "ChatGPT".into()),
        ("claude".into(), "Claude".into()),
        ("grok".into(), "Grok".into()),
    ];
    let mcp = jc_mcp_install(&chrome.branding);
    let tpl = AssistantsPageTemplate {
        chrome,
        labels,
        lead,
        prompt,
        cards,
        mcp,
    };
    tpl.render().unwrap_or_default()
}

/// Renders `/assistants/{id}` (`claude`, `chatgpt`, `grok`, D6).
pub fn render_assistant_guide_page(
    catalogue: &Catalogue,
    installation_branding: Option<&BrandingBlock>,
    id: &str,
    lang: &str,
    had_url_prefix: bool,
    mount_prefix: &str,
    csp_nonce: &str,
) -> Option<String> {
    let stripped = format!("/assistants/{id}");
    let chrome = PageChrome::build(
        catalogue,
        installation_branding,
        lang,
        had_url_prefix,
        &stripped,
        None,
        mount_prefix,
        "",
        csp_nonce,
    );
    let guide = jc_assistant_guide(id, &chrome.branding, &chrome.lang)?;
    let mcp = jc_mcp_install(&chrome.branding)?;
    let labels = UiLabels::for_lang(&chrome.lang);
    let tpl = GuidePageTemplate {
        chrome,
        labels,
        guide,
        mcp,
    };
    tpl.render().ok()
}

/// Renders `/map/{id}` or `/map/{id}/{resource_id}` (D6).
#[allow(clippy::too_many_arguments)]
pub fn render_map_page(
    catalogue: &Catalogue,
    installation_branding: Option<&BrandingBlock>,
    dataset: &Dataset,
    resource_id: Option<&str>,
    lang: &str,
    had_url_prefix: bool,
    mount_prefix: &str,
    csp_nonce: &str,
) -> Option<String> {
    let stripped = match resource_id {
        Some(rid) => format!("/map/{}/{rid}", dataset.name),
        None => format!("/map/{}", dataset.name),
    };
    let chrome = PageChrome::build(
        catalogue,
        installation_branding,
        lang,
        had_url_prefix,
        &stripped,
        None,
        mount_prefix,
        "",
        csp_nonce,
    );
    let map_cfg = jc_map_config(
        catalogue,
        dataset,
        resource_id,
        &chrome.branding,
        &chrome.lang,
    )?;
    let dataset_title = localized_text(&dataset.title, &chrome.lang, &catalogue.default_language)
        .unwrap_or(&dataset.name)
        .to_owned();
    let dataset_url = format!("{}/dataset/{}", catalogue.base_url(), dataset.name);
    let has_pmtiles = map_cfg
        .get("pmtiles")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty());
    let labels = UiLabels::for_lang(&chrome.lang);
    let tpl = MapPageTemplate {
        chrome,
        labels,
        dataset_title,
        dataset_url,
        map_config_json: map_cfg.to_string(),
        has_pmtiles,
    };
    tpl.render().ok()
}

/// Renders an HTML editor form from the [`FormFieldLayout`] derived from a profile's SHACL shapes (C17).
pub fn render_profile_editor_form(fields: &[FormFieldLayout], lang: &str) -> String {
    let mut out = String::from(
        "<form method=\"post\" action=\"/api/v1/datasets\" class=\"dr-profile-form\">\n",
    );
    for f in fields {
        let label = text_in_lang(&f.name, Some(lang)).unwrap_or(&f.field_name);
        let desc = text_in_lang(&f.description, Some(lang)).unwrap_or("");
        let req_attr = if f.required { " required" } else { "" };
        out.push_str(&format!(
            "  <div class=\"form-field\" data-path=\"{}\" data-order=\"{}\">\n",
            html_escape(&f.path),
            f.order
        ));
        out.push_str(&format!(
            "    <label for=\"field-{}\">{}{}</label>\n",
            html_escape(&f.field_name),
            html_escape(label),
            if f.required { " *" } else { "" }
        ));
        out.push_str(&format!(
            "    <input id=\"field-{}\" name=\"{}\" type=\"text\"{req_attr} aria-describedby=\"hint-{}\"/>\n",
            html_escape(&f.field_name),
            html_escape(&f.field_name),
            html_escape(&f.field_name)
        ));
        if !desc.is_empty() {
            out.push_str(&format!(
                "    <small id=\"hint-{}\">{}</small>\n",
                html_escape(&f.field_name),
                html_escape(desc)
            ));
        }
        out.push_str("  </div>\n");
    }
    out.push_str("</form>\n");
    out
}
