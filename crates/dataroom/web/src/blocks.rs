//! A portal's pages built from blocks (T-7433): the home page that replaces the built-in one
//! and each further page at `/page/{slug}`, rendered from `spec.pages` of its Portal.
//!
//! Every block renders from data the host already checked (jc-core's `validate_pages`) and is
//! checked again here where it names something: a dataset shows only when it is one of this
//! catalogue's public, active and listed datasets (an internal or restricted one never), a file
//! only when it is in this catalogue's or its Organization's store, a link only when it passes
//! [`is_link_url`]. Markdown goes through jc-core's one safe renderer; every other text is
//! escaped by the template. A gallery reads at most [`MAX_GALLERY_DATASETS`] of the datasets the
//! page was handed, in memory: no query leaves the process.

use crate::i18n::{jc_number, jc_t};
use crate::{to_dataset_row_view, DatasetRowView, PageChrome, UiLabels};
use askama::Template;
use dataroom_core::files::FileEntry;
use dataroom_core::vocab::theme_code;
use dataroom_core::{
    BrandingBlock, Catalogue, Dataset, Publisher, RecordState, ResourceView, Visibility,
};
use jc_core::kinds::appearance::is_link_url;
use jc_core::kinds::portal_page::{
    file_sha256, text_in, PortalBlock, PortalNumber, PortalPage, Texts, HOME_PAGE,
    MAX_GALLERY_DATASETS,
};
use jc_core::kinds::DatasetListLayout;
use jc_time::TimestampTz;
use std::collections::BTreeMap;

/// What a page's blocks read besides the manifest.
#[derive(Debug, Clone, Copy)]
pub struct PageInputs<'a> {
    /// Every dataset of the catalogue; the blocks show only the public ones.
    pub datasets: &'a [Dataset],
    pub publishers: &'a [Publisher],
    /// The files of this catalogue's store and its Organization's.
    pub files: &'a [FileEntry],
    /// The resource views of each dataset a chart block names.
    pub views: &'a BTreeMap<String, Vec<ResourceView>>,
    /// The moment "updated in the last 30 days" counts back from.
    pub now: TimestampTz,
}

/// The datasets the chart blocks of page `slug` name, so the host loads only their views.
#[must_use]
pub fn chart_datasets(branding: &BrandingBlock, slug: &str) -> Vec<String> {
    let mut names: Vec<String> = branding
        .layout
        .page(slug)
        .into_iter()
        .flat_map(|p| &p.rows)
        .flat_map(|r| &r.blocks)
        .filter_map(|b| match b {
            PortalBlock::Chart(c) => Some(c.dataset.clone()),
            _ => None,
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

/// The header menu entries of the portal's pages in `lang`: `(href, title)`.
#[must_use]
pub fn menu_links(
    pages: &[PortalPage],
    lang: &str,
    fallback: &str,
    lang_prefix: &str,
) -> Vec<(String, String)> {
    pages
        .iter()
        .filter(|p| p.menu && p.slug != HOME_PAGE)
        .filter_map(|p| {
            let title = text_in(&p.title, lang, fallback)?;
            Some((format!("{lang_prefix}/page/{}", p.slug), title.to_owned()))
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct HeroView {
    /// Whether the hero opens the page and its title is the page's `<h1>`.
    pub h1: bool,
    /// Numbers the hero's search field, unique on the page.
    pub n: usize,
    pub title: String,
    pub intro_html: String,
    pub image: Option<String>,
    pub search: bool,
}

#[derive(Debug, Clone)]
pub struct DatasetsView {
    pub title: String,
    pub rows: Vec<DatasetRowView>,
    pub table: bool,
    pub more_href: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FrameView {
    pub title: String,
    /// The frame's address; `None` when the dataset has nothing to draw (`empty` says so).
    pub src: Option<String>,
    pub sandbox: String,
    pub origin: String,
    /// Behind the frame until it paints, and what stays when it fails to load.
    pub waiting: String,
    /// What the waiting line turns into when the frame fails (jc-viz.js).
    pub failed: String,
    pub empty: String,
    /// The dataset's page: the way on when the frame shows nothing.
    pub href: String,
}

#[derive(Debug, Clone)]
pub struct NumberView {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct LinkCardView {
    pub title: String,
    pub text: String,
    pub href: String,
    pub external: bool,
}

#[derive(Debug, Clone)]
pub struct FileView {
    pub name: String,
    pub href: String,
    pub size: String,
    pub kind: String,
}

/// One rendered block.
#[derive(Debug, Clone)]
pub enum BlockView {
    Hero(HeroView),
    Text(String),
    Datasets(DatasetsView),
    Map(FrameView),
    Chart(FrameView),
    Numbers(String, Vec<NumberView>),
    Links(String, Vec<LinkCardView>),
    Files(String, Vec<FileView>),
}

/// One row of rendered blocks; a block with nothing to show is left out.
#[derive(Debug, Clone)]
pub struct RowView {
    pub blocks: Vec<BlockView>,
}

/// A page built from blocks (`templates/portal_page.html`).
#[derive(Template)]
#[template(path = "portal_page.html")]
pub struct PortalPageTemplate {
    pub chrome: PageChrome,
    pub labels: UiLabels,
    pub title: String,
    pub slug: String,
    pub home: bool,
    /// Whether the page heading shows above the rows: not when a hero opens the page.
    pub heading: bool,
    pub query: String,
    pub rows: Vec<RowView>,
    pub frames: bool,
    pub updated_label: String,
    pub download_label: String,
    pub gallery_empty: String,
    pub open_dataset: String,
}

/// The public, active, listed datasets of the catalogue: the only ones a block shows.
fn public<'a>(catalogue: &Catalogue, datasets: &'a [Dataset]) -> Vec<&'a Dataset> {
    datasets
        .iter()
        .filter(|d| {
            d.state == RecordState::Active
                && d.visibility == Visibility::Public
                && !catalogue.hidden_datasets.contains_key(&d.name)
        })
        .collect()
}

/// `markdown` as safe HTML through jc-core's renderer; a leading `# heading` stays as `<h2>`.
fn markdown_html(markdown: &str) -> String {
    let page = jc_core::opendata_pages::render(markdown, &[]);
    match page.title {
        Some(title) => format!(
            "<h2>{}</h2>{}",
            crate::helpers::html_escape(&title),
            page.html
        ),
        None => page.html,
    }
}

fn human_size(bytes: u64) -> String {
    match bytes {
        b if b >= 1024 * 1024 => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
        b if b >= 1024 => format!("{} kB", b.div_ceil(1024)),
        b => format!("{b} B"),
    }
}

fn file_kind(content_type: &str) -> &'static str {
    match content_type {
        "application/pdf" => "PDF",
        "text/csv" => "CSV",
        "image/png" => "PNG",
        "image/jpeg" => "JPEG",
        "image/webp" => "WebP",
        "image/svg+xml" => "SVG",
        _ if content_type.contains("spreadsheet") => "XLSX",
        _ => "",
    }
}

struct Ctx<'a> {
    catalogue: &'a Catalogue,
    chrome: &'a PageChrome,
    labels: &'a UiLabels,
    inputs: PageInputs<'a>,
    public: Vec<&'a Dataset>,
    pub_map: BTreeMap<String, Publisher>,
}

impl Ctx<'_> {
    fn text(&self, texts: &Texts) -> String {
        text_in(texts, &self.chrome.lang, &self.catalogue.default_language)
            .unwrap_or_default()
            .to_owned()
    }

    fn row(&self, d: &Dataset) -> DatasetRowView {
        to_dataset_row_view(
            d,
            &self.pub_map,
            &self.chrome.lang,
            &self.catalogue.default_language,
        )
    }

    fn dataset(&self, name: &str) -> Option<&Dataset> {
        self.public.iter().copied().find(|d| d.name == name)
    }

    fn dataset_href(&self, d: &Dataset) -> String {
        format!("{}/dataset/{}", self.chrome.lang_prefix, d.name)
    }

    /// The store's file a block names, by the hash in its address; `None` for a file this
    /// catalogue and its Organization do not hold.
    fn file(&self, path: &str) -> Option<&FileEntry> {
        let sha = file_sha256(path)?;
        self.inputs.files.iter().find(|f| f.sha256 == sha)
    }

    fn file_href(&self, f: &FileEntry) -> String {
        format!("{}{}", self.chrome.mount_prefix, f.path())
    }

    fn block(&self, block: &PortalBlock) -> Option<BlockView> {
        Some(match block {
            PortalBlock::Hero(b) => BlockView::Hero(HeroView {
                h1: false,
                n: 0,
                title: Some(self.text(&b.title))
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| self.chrome.site_title.clone()),
                intro_html: text_in(
                    &b.intro,
                    &self.chrome.lang,
                    &self.catalogue.default_language,
                )
                .map(markdown_html)
                .unwrap_or_default(),
                // Only a raster image of this portal's store shows inline.
                image: b
                    .image
                    .as_deref()
                    .and_then(|i| self.file(i))
                    .filter(|f| f.inline())
                    .map(|f| self.file_href(f)),
                search: b.search,
            }),
            PortalBlock::Text(b) => BlockView::Text(markdown_html(text_in(
                &b.markdown,
                &self.chrome.lang,
                &self.catalogue.default_language,
            )?)),
            PortalBlock::DatasetGallery(b) => {
                let limit = usize::from(b.limit.clamp(1, MAX_GALLERY_DATASETS));
                let mut found: Vec<&Dataset> = self
                    .public
                    .iter()
                    .copied()
                    .filter(|d| b.tag.as_ref().is_none_or(|t| d.tags.contains(t)))
                    .filter(|d| {
                        b.publisher.as_ref().is_none_or(|p| {
                            d.publisher_id == *p
                                || self
                                    .pub_map
                                    .get(&d.publisher_id)
                                    .is_some_and(|x| x.name == *p)
                        })
                    })
                    .filter(|d| {
                        b.theme.as_ref().is_none_or(|t| {
                            d.themes
                                .iter()
                                .any(|x| theme_code(x).eq_ignore_ascii_case(t))
                        })
                    })
                    .filter(|d| {
                        b.format.as_ref().is_none_or(|f| {
                            d.distributions.iter().any(|x| {
                                x.format
                                    .as_deref()
                                    .is_some_and(|y| y.eq_ignore_ascii_case(f))
                            })
                        })
                    })
                    .collect();
                found.sort_by(|a, b| {
                    b.modified
                        .cmp(&a.modified)
                        .then_with(|| a.name.cmp(&b.name))
                });
                let mut query = Vec::new();
                if let Some(t) = &b.tag {
                    query.push(("tags", t.as_str()));
                }
                if let Some(p) = &b.publisher {
                    query.push(("organization", p.as_str()));
                }
                if let Some(t) = &b.theme {
                    query.push(("vocab_jc_topic", t.as_str()));
                }
                if let Some(f) = &b.format {
                    query.push(("res_format", f.as_str()));
                }
                let more = found.len() > limit;
                let encoded: String = url::form_urlencoded::Serializer::new(String::new())
                    .extend_pairs(query)
                    .finish();
                BlockView::Datasets(DatasetsView {
                    title: self.text(&b.title),
                    rows: found.into_iter().take(limit).map(|d| self.row(d)).collect(),
                    table: b.layout == Some(DatasetListLayout::Table),
                    more_href: more
                        .then(|| format!("{}/dataset/?{encoded}", self.chrome.lang_prefix)),
                })
            }
            PortalBlock::FeaturedDatasets(b) => BlockView::Datasets(DatasetsView {
                title: self.text(&b.title),
                rows: b
                    .datasets
                    .iter()
                    .filter_map(|n| self.dataset(n))
                    .map(|d| self.row(d))
                    .collect(),
                table: false,
                more_href: None,
            }),
            PortalBlock::Map(b) => {
                let d = self.dataset(&b.dataset)?;
                let drawable = crate::helpers::jc_map_config(
                    self.catalogue,
                    d,
                    None,
                    &self.chrome.branding,
                    &self.chrome.lang,
                )
                .is_some();
                BlockView::Map(FrameView {
                    title: Some(self.text(&b.title))
                        .filter(|t| !t.is_empty())
                        .unwrap_or_else(|| self.row(d).title),
                    src: drawable.then(|| format!("{}/map/{}", self.chrome.lang_prefix, d.name)),
                    sandbox: "allow-scripts allow-same-origin".into(),
                    origin: String::new(),
                    waiting: jc_t("block_map_waiting", &self.chrome.lang),
                    failed: jc_t("block_map_failed", &self.chrome.lang),
                    empty: jc_t("map_empty", &self.chrome.lang),
                    href: self.dataset_href(d),
                })
            }
            PortalBlock::Chart(b) => {
                let d = self.dataset(&b.dataset)?;
                let chart = self.inputs.views.get(&d.name).and_then(|views| {
                    crate::helpers::jc_visualizations(
                        d,
                        views,
                        &self.chrome.branding,
                        &self.chrome.lang,
                        &self.chrome.lang_prefix,
                    )
                    .into_iter()
                    .next()
                });
                let title = Some(self.text(&b.title)).filter(|t| !t.is_empty());
                let waiting = jc_t("block_chart_waiting", &self.chrome.lang);
                let failed = jc_t("block_chart_failed", &self.chrome.lang);
                let empty = jc_t("block_chart_empty", &self.chrome.lang);
                let href = self.dataset_href(d);
                match chart {
                    Some(chart) => BlockView::Chart(FrameView {
                        title: title.unwrap_or(chart.title),
                        src: Some(chart.src),
                        sandbox: chart.sandbox,
                        origin: chart.origin,
                        waiting,
                        failed: failed.clone(),
                        empty,
                        href,
                    }),
                    None => BlockView::Chart(FrameView {
                        title: title.unwrap_or_else(|| self.row(d).title),
                        src: None,
                        sandbox: String::new(),
                        origin: String::new(),
                        waiting,
                        failed,
                        empty,
                        href,
                    }),
                }
            }
            PortalBlock::Numbers(b) => {
                let month_ago = self.inputs.now.as_second() - 30 * 24 * 3600;
                let items = b
                    .items
                    .iter()
                    .map(|n| match n {
                        PortalNumber::Datasets => NumberView {
                            value: jc_number(self.public.len()),
                            label: self.labels.datasets.clone(),
                        },
                        PortalNumber::Publishers => {
                            let mut names: Vec<&str> = self
                                .public
                                .iter()
                                .map(|d| d.publisher_id.as_str())
                                .collect();
                            names.sort_unstable();
                            names.dedup();
                            NumberView {
                                value: jc_number(names.len()),
                                label: self.labels.publishers.clone(),
                            }
                        }
                        PortalNumber::UpdatedThisMonth => NumberView {
                            value: jc_number(
                                self.public
                                    .iter()
                                    .filter(|d| d.modified.as_second() >= month_ago)
                                    .count(),
                            ),
                            label: jc_t("updated_month", &self.chrome.lang),
                        },
                    })
                    .collect();
                BlockView::Numbers(self.text(&b.title), items)
            }
            PortalBlock::LinkCards(b) => BlockView::Links(
                self.text(&b.title),
                b.cards
                    .iter()
                    .filter(|c| is_link_url(&c.url))
                    .map(|c| {
                        let external = !c.url.starts_with('/');
                        LinkCardView {
                            title: self.text(&c.title),
                            text: self.text(&c.text),
                            href: if external {
                                c.url.clone()
                            } else {
                                format!("{}{}", self.chrome.lang_prefix, c.url)
                            },
                            external,
                        }
                    })
                    .collect(),
            ),
            PortalBlock::FileList(b) => {
                let files: Vec<FileView> = b
                    .files
                    .iter()
                    .filter_map(|p| self.file(p))
                    .map(|f| FileView {
                        name: f.name.clone(),
                        href: self.file_href(f),
                        size: human_size(f.size),
                        kind: file_kind(&f.content_type).to_owned(),
                    })
                    .collect();
                if files.is_empty() {
                    return None;
                }
                BlockView::Files(self.text(&b.title), files)
            }
        })
    }
}

/// Renders page `slug` of the catalogue's portal; `None` when the portal has no such page.
#[allow(clippy::too_many_arguments)]
pub fn render_portal_page(
    catalogue: &Catalogue,
    installation_branding: Option<&BrandingBlock>,
    slug: &str,
    inputs: PageInputs<'_>,
    query: &str,
    lang: &str,
    had_url_prefix: bool,
    mount_prefix: &str,
    csp_nonce: &str,
) -> Option<String> {
    let home = slug == HOME_PAGE;
    let path = if home {
        "/".to_owned()
    } else {
        format!("/page/{slug}")
    };
    let chrome = PageChrome::build(
        catalogue,
        installation_branding,
        lang,
        had_url_prefix,
        &path,
        None,
        mount_prefix,
        if home { "home" } else { "" },
        csp_nonce,
    );
    let page = chrome.branding.layout.page(slug)?.clone();
    let labels = UiLabels::for_lang(&chrome.lang);
    let ctx = Ctx {
        catalogue,
        chrome: &chrome,
        labels: &labels,
        inputs,
        public: public(catalogue, inputs.datasets),
        pub_map: inputs
            .publishers
            .iter()
            .flat_map(|p| [(p.id.clone(), p.clone()), (p.name.clone(), p.clone())])
            .collect(),
    };
    let mut rows: Vec<RowView> = page
        .rows
        .iter()
        .map(|r| RowView {
            blocks: r.blocks.iter().filter_map(|b| ctx.block(b)).collect(),
        })
        .filter(|r| !r.blocks.is_empty())
        .collect();
    let opens_with_hero = match rows.first_mut().and_then(|r| r.blocks.first_mut()) {
        Some(BlockView::Hero(h)) => {
            h.h1 = true;
            true
        }
        _ => false,
    };
    for (n, h) in rows
        .iter_mut()
        .flat_map(|r| r.blocks.iter_mut())
        .filter_map(|b| match b {
            BlockView::Hero(h) => Some(h),
            _ => None,
        })
        .enumerate()
    {
        h.n = n;
    }
    let frames = rows
        .iter()
        .flat_map(|r| &r.blocks)
        .any(|b| matches!(b, BlockView::Map(f) | BlockView::Chart(f) if f.src.is_some()));
    let title = text_in(&page.title, &chrome.lang, &catalogue.default_language)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            if home {
                labels.welcome.clone()
            } else {
                slug.to_owned()
            }
        });
    let tpl = PortalPageTemplate {
        title,
        slug: slug.to_owned(),
        home,
        heading: !opens_with_hero,
        query: query.to_owned(),
        rows,
        frames,
        updated_label: labels.updated.clone(),
        download_label: jc_t("download", &chrome.lang),
        gallery_empty: jc_t("gallery_empty", &chrome.lang),
        open_dataset: jc_t("block_open_dataset", &chrome.lang),
        labels,
        chrome,
    };
    tpl.render().ok()
}
