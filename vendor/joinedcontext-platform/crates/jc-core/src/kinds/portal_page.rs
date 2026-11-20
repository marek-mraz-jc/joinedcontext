//! `spec.pages` of a [`Portal`][crate::kinds::Portal] (T-7418): the open data portal's home and
//! its further pages, each an ordered list of rows of typed blocks.
//!
//! A block is data, never markup: a heading and Markdown per language (rendered by
//! [`crate::opendata_pages::render`], the one safe renderer), dataset names, a gallery query of
//! fixed fields, links that pass [`is_link_url`], and files of the portal's own store named by
//! their address `/files/{sha256}/{name}`. No block carries HTML, CSS or script. A portal
//! without pages shows the built-in home; a page with the slug [`HOME_PAGE`] replaces it.

use crate::error::{Error, Result};
use crate::kinds::appearance::{
    is_link_url, DatasetListLayout, MAX_APPEARANCE_TEXT, MAX_HERO_INTRO,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The slug of the page that replaces the portal's built-in home.
pub const HOME_PAGE: &str = "home";
/// The most pages one portal holds, its home included.
pub const MAX_PORTAL_PAGES: usize = 12;
/// The most rows one page holds.
pub const MAX_PAGE_ROWS: usize = 20;
/// The most blocks one row shows side by side.
pub const MAX_ROW_BLOCKS: usize = 3;
/// The most datasets, cards or files one block lists.
pub const MAX_BLOCK_ITEMS: usize = 12;
/// The most datasets a gallery shows.
pub const MAX_GALLERY_DATASETS: u8 = 24;
/// The longest Markdown of a text block in one language, in bytes.
pub const MAX_BLOCK_MARKDOWN: usize = 8 * 1024;

/// A text per language, the key a two-letter code, e.g. `{sk: Doprava, en: Mobility}`.
pub type Texts = BTreeMap<String, String>;

/// One page of the portal (T-7418).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PortalPage {
    /// `home` for the home page, else a DNS label such as `doprava`: the page answers at
    /// `/page/doprava`.
    pub slug: String,
    /// The page's title and menu entry per language, e.g. `{sk: Doprava, en: Mobility}`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub title: Texts,
    /// Whether the header menu lists the page; the home page is the logo's link.
    #[serde(default = "yes", skip_serializing_if = "is_yes")]
    pub menu: bool,
    /// The rows from top to bottom.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<PortalRow>,
}

/// One row of a page: one to three blocks side by side, stacked on a phone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PortalRow {
    /// The blocks from left to right.
    pub blocks: Vec<PortalBlock>,
}

/// One block of a row, told apart by `type`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum PortalBlock {
    /// A heading, an introduction, an image and the dataset search box.
    Hero(HeroBlock),
    /// Markdown.
    Text(TextBlock),
    /// The datasets a query of this portal's public datasets finds, newest first.
    DatasetGallery(DatasetGalleryBlock),
    /// Datasets picked by name, in that order.
    FeaturedDatasets(FeaturedDatasetsBlock),
    /// The map of one dataset with a location, on the deployment's basemap.
    Map(MapBlock),
    /// Counts of the portal's public datasets.
    Numbers(NumbersBlock),
    /// A chart of one dataset's Endpoint, the chart its own page shows.
    Chart(ChartBlock),
    /// Cards that each link somewhere.
    LinkCards(LinkCardsBlock),
    /// Files of the portal's store to download.
    FileList(FileListBlock),
}

/// `type: hero`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HeroBlock {
    /// The heading per language; absent: the portal's title.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub title: Texts,
    /// The introduction per language in Markdown of at most 2 KiB, e.g. `Dáta mesta na
    /// **jednom mieste**.`
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub intro: Texts,
    /// A raster image of the portal's files, e.g. `/files/{sha256}/hero.png`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Whether the dataset search box shows.
    #[serde(default = "yes", skip_serializing_if = "is_yes")]
    pub search: bool,
}

/// `type: text`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TextBlock {
    /// Markdown per language, at most 8 KiB each, e.g. `## O portáli\n\nDáta mesta...`; raw HTML
    /// shows as text.
    pub markdown: Texts,
}

/// `type: dataset-gallery`: every field narrows; none set lists the newest datasets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DatasetGalleryBlock {
    /// The heading per language, e.g. `{sk: Doprava}`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub title: Texts,
    /// Only datasets with this keyword, e.g. `doprava`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Only datasets of this publisher by name, e.g. `mesto-bb`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    /// Only datasets of this theme by its EU code, e.g. `TRAN`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// Only datasets with a distribution of this format, e.g. `CSV`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// How many datasets show, 1 to 24.
    #[serde(default = "gallery_limit")]
    pub limit: u8,
    /// Cards (the default) or a table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<DatasetListLayout>,
}

/// `type: featured-datasets`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FeaturedDatasetsBlock {
    /// The heading per language.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub title: Texts,
    /// One to 12 dataset names, e.g. `[ovzdusie-bb, mhd-zastavky]`.
    pub datasets: Vec<String>,
}

/// `type: map`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MapBlock {
    /// The heading per language.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub title: Texts,
    /// The dataset by name, one with a GeoJSON or OGC API Features distribution.
    pub dataset: String,
}

/// One count of a [`NumbersBlock`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PortalNumber {
    /// Public datasets.
    Datasets,
    /// Publishers with a public dataset.
    Publishers,
    /// Public datasets modified in the last 30 days.
    UpdatedThisMonth,
}

/// `type: numbers`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NumbersBlock {
    /// The heading per language.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub title: Texts,
    /// One to three counts, each once, e.g. `[datasets, publishers]`.
    pub items: Vec<PortalNumber>,
}

/// `type: chart`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ChartBlock {
    /// The heading per language.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub title: Texts,
    /// The dataset by name whose Endpoint the chart reads, e.g. `ovzdusie-bb`.
    pub dataset: String,
}

/// `type: link-cards`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LinkCardsBlock {
    /// The heading per language.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub title: Texts,
    /// One to 12 cards.
    pub cards: Vec<LinkCard>,
}

/// One card of a [`LinkCardsBlock`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LinkCard {
    /// The card's heading per language, e.g. `{sk: Rozpočet}`.
    pub title: Texts,
    /// One line under it per language.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub text: Texts,
    /// An `https://` address or a path on the portal, e.g. `/page/rozpocet`.
    pub url: String,
}

/// `type: file-list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FileListBlock {
    /// The heading per language.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub title: Texts,
    /// One to 12 files of the portal, each `/files/{sha256}/{name}`.
    pub files: Vec<String>,
}

const fn yes() -> bool {
    true
}

// serde's skip_serializing_if passes a reference.
const fn is_yes(on: &bool) -> bool {
    *on
}

const fn gallery_limit() -> u8 {
    6
}

fn invalid(field: impl Into<String>, reason: impl Into<String>) -> Error {
    Error::Invalid {
        field: field.into(),
        reason: reason.into(),
    }
}

/// Whether `path` names a file of a portal's store: `/files/{64 hex}/{name}`, the name without
/// `/`, `\`, quotes, angle brackets or control characters.
pub fn is_file_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/files/") else {
        return false;
    };
    let Some((hash, name)) = rest.split_once('/') else {
        return false;
    };
    hash.len() == 64
        && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        && !name.is_empty()
        && name.len() <= 240
        && name
            .chars()
            .all(|c| !c.is_control() && !"/\\\"'<>`".contains(c))
}

/// The SHA-256 a file path names, `None` for anything [`is_file_path`] refuses.
pub fn file_sha256(path: &str) -> Option<&str> {
    is_file_path(path).then(|| &path["/files/".len().."/files/".len() + 64])
}

fn languages(field: &str, texts: &Texts) -> Result<()> {
    for lang in texts.keys() {
        if lang.len() != 2 || !lang.bytes().all(|b| b.is_ascii_lowercase()) {
            return Err(invalid(
                format!("{field}.{lang}"),
                "a two-letter language code such as `sk`",
            ));
        }
    }
    Ok(())
}

/// One line per language of 1 to 120 characters without `<` or `>`.
fn lines(field: &str, texts: &Texts) -> Result<()> {
    languages(field, texts)?;
    for (lang, text) in texts {
        if text.trim().is_empty()
            || text.chars().count() > MAX_APPEARANCE_TEXT
            || text.chars().any(|c| c.is_control() || c == '<' || c == '>')
        {
            return Err(invalid(
                format!("{field}.{lang}"),
                "one line of 1 to 120 characters without < or >",
            ));
        }
    }
    Ok(())
}

/// Markdown per language of 1 to `max` bytes, no control character but line breaks and tabs.
fn markdown(field: &str, texts: &Texts, max: usize) -> Result<()> {
    languages(field, texts)?;
    for (lang, text) in texts {
        if text.trim().is_empty()
            || text.len() > max
            || text
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        {
            return Err(invalid(
                format!("{field}.{lang}"),
                format!("Markdown of 1 to {} KiB", max / 1024),
            ));
        }
    }
    Ok(())
}

fn word(field: &str, value: &Option<String>) -> Result<()> {
    match value {
        Some(v) if v.trim().is_empty() || v.len() > 100 || v.chars().any(char::is_control) => Err(
            invalid(field, "a value of 1 to 100 characters, e.g. `doprava`"),
        ),
        _ => Ok(()),
    }
}

fn dataset(field: &str, name: &str) -> Result<()> {
    let plain = !name.is_empty()
        && name.len() <= 100
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_'));
    if plain {
        Ok(())
    } else {
        Err(invalid(
            field,
            "a dataset name: lower-case letters, digits, - and _, e.g. `ovzdusie-bb`",
        ))
    }
}

fn items(field: &str, len: usize, max: usize) -> Result<()> {
    if len == 0 || len > max {
        return Err(invalid(field, format!("1 to {max} items")));
    }
    Ok(())
}

/// Every page of `spec.pages`: unique slugs, at most one home, each block within its limits.
pub fn validate_pages(pages: &[PortalPage]) -> Result<()> {
    if pages.len() > MAX_PORTAL_PAGES {
        return Err(invalid(
            "spec.pages",
            format!("at most {MAX_PORTAL_PAGES} pages"),
        ));
    }
    let mut seen: Vec<&str> = Vec::new();
    for (p, page) in pages.iter().enumerate() {
        let at = format!("spec.pages[{p}]");
        if page.slug != HOME_PAGE && crate::names::validate_dns1123_label(&page.slug).is_err() {
            return Err(invalid(
                format!("{at}.slug"),
                format!(
                    "`{}` is not `home` or a DNS label such as `doprava`",
                    page.slug
                ),
            ));
        }
        if seen.contains(&page.slug.as_str()) {
            return Err(invalid(
                format!("{at}.slug"),
                format!("`{}` names two pages of this portal", page.slug),
            ));
        }
        seen.push(&page.slug);
        lines(&format!("{at}.title"), &page.title)?;
        if page.slug != HOME_PAGE && page.title.is_empty() {
            return Err(invalid(
                format!("{at}.title"),
                "a page other than home has a title, e.g. `{sk: Doprava}`",
            ));
        }
        if page.rows.len() > MAX_PAGE_ROWS {
            return Err(invalid(
                format!("{at}.rows"),
                format!("at most {MAX_PAGE_ROWS} rows"),
            ));
        }
        for (r, row) in page.rows.iter().enumerate() {
            let at = format!("{at}.rows[{r}]");
            items(&format!("{at}.blocks"), row.blocks.len(), MAX_ROW_BLOCKS)?;
            for (b, block) in row.blocks.iter().enumerate() {
                block.validate(&format!("{at}.blocks[{b}]"))?;
            }
        }
    }
    Ok(())
}

impl PortalBlock {
    /// The block alone, `at` naming it in a refusal, e.g. `spec.pages[0].rows[1].blocks[0]`.
    pub fn validate(&self, at: &str) -> Result<()> {
        let f = |name: &str| format!("{at}.{name}");
        let title = |t: &Texts| lines(&f("title"), t);
        match self {
            Self::Hero(b) => {
                title(&b.title)?;
                markdown(&f("intro"), &b.intro, MAX_HERO_INTRO)?;
                if b.image.as_deref().is_some_and(|i| !is_file_path(i)) {
                    return Err(invalid(
                        f("image"),
                        "a file of this portal, /files/{sha256}/{name}",
                    ));
                }
            }
            Self::Text(b) => {
                if b.markdown.is_empty() {
                    return Err(invalid(f("markdown"), "Markdown in at least one language"));
                }
                markdown(&f("markdown"), &b.markdown, MAX_BLOCK_MARKDOWN)?;
            }
            Self::DatasetGallery(b) => {
                title(&b.title)?;
                word(&f("tag"), &b.tag)?;
                word(&f("publisher"), &b.publisher)?;
                word(&f("theme"), &b.theme)?;
                word(&f("format"), &b.format)?;
                if b.limit == 0 || b.limit > MAX_GALLERY_DATASETS {
                    return Err(invalid(
                        f("limit"),
                        format!("1 to {MAX_GALLERY_DATASETS} datasets"),
                    ));
                }
            }
            Self::FeaturedDatasets(b) => {
                title(&b.title)?;
                items(&f("datasets"), b.datasets.len(), MAX_BLOCK_ITEMS)?;
                for (i, name) in b.datasets.iter().enumerate() {
                    dataset(&format!("{at}.datasets[{i}]"), name)?;
                }
            }
            Self::Map(b) => {
                title(&b.title)?;
                dataset(&f("dataset"), &b.dataset)?;
            }
            Self::Chart(b) => {
                title(&b.title)?;
                dataset(&f("dataset"), &b.dataset)?;
            }
            Self::Numbers(b) => {
                title(&b.title)?;
                items(&f("items"), b.items.len(), 3)?;
                for (i, n) in b.items.iter().enumerate() {
                    if b.items[..i].contains(n) {
                        return Err(invalid(format!("{at}.items[{i}]"), "each count once"));
                    }
                }
            }
            Self::LinkCards(b) => {
                title(&b.title)?;
                items(&f("cards"), b.cards.len(), MAX_BLOCK_ITEMS)?;
                for (i, card) in b.cards.iter().enumerate() {
                    let at = format!("{at}.cards[{i}]");
                    if card.title.is_empty() {
                        return Err(invalid(format!("{at}.title"), "a title in one language"));
                    }
                    lines(&format!("{at}.title"), &card.title)?;
                    lines(&format!("{at}.text"), &card.text)?;
                    if !is_link_url(&card.url) {
                        return Err(invalid(
                            format!("{at}.url"),
                            "an https:// address or a path starting / such as /page/rozpocet",
                        ));
                    }
                }
            }
            Self::FileList(b) => {
                title(&b.title)?;
                items(&f("files"), b.files.len(), MAX_BLOCK_ITEMS)?;
                for (i, file) in b.files.iter().enumerate() {
                    if !is_file_path(file) {
                        return Err(invalid(
                            format!("{at}.files[{i}]"),
                            "a file of this portal, /files/{sha256}/{name}",
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

/// The text of `texts` in `lang`, else in `fallback`, else the first one.
pub fn text_in<'a>(texts: &'a Texts, lang: &str, fallback: &str) -> Option<&'a str> {
    texts
        .get(lang)
        .or_else(|| texts.get(fallback))
        .or_else(|| texts.values().next())
        .map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SHA: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

    fn page(blocks: serde_json::Value) -> Result<Vec<PortalPage>> {
        let pages: Vec<PortalPage> = serde_json::from_value(json!([
            { "slug": "home", "rows": [{ "blocks": blocks }] }
        ]))
        .map_err(|e| Error::Parse(e.to_string()))?;
        validate_pages(&pages)?;
        Ok(pages)
    }

    fn one_of_each() -> serde_json::Value {
        json!([
            { "type": "hero", "title": { "sk": "Dáta mesta" }, "intro": { "sk": "**Vitajte**" },
              "image": format!("/files/{SHA}/hero.png"), "search": false },
            { "type": "text", "markdown": { "sk": "## O nás", "en": "## About" } },
            { "type": "dataset-gallery", "tag": "doprava", "publisher": "mesto", "theme": "TRAN",
              "format": "CSV", "limit": 24, "layout": "table" },
            { "type": "featured-datasets", "datasets": ["ovzdusie-bb", "mhd_zastavky"] },
            { "type": "map", "dataset": "zastavky" },
            { "type": "numbers", "items": ["datasets", "publishers", "updated-this-month"] },
            { "type": "chart", "dataset": "ovzdusie-bb" },
            { "type": "link-cards", "cards": [
                { "title": { "sk": "Rozpočet" }, "url": "/page/rozpocet" },
                { "title": { "en": "City" }, "text": { "en": "Home" }, "url": "https://mesto.sk" }] },
            { "type": "file-list", "files": [format!("/files/{SHA}/plan.pdf")] },
        ])
    }

    #[test]
    fn each_of_the_nine_blocks_parses_and_round_trips() {
        let blocks = one_of_each();
        let rows: Vec<serde_json::Value> = blocks
            .as_array()
            .unwrap()
            .chunks(MAX_ROW_BLOCKS)
            .map(|c| json!({ "blocks": c }))
            .collect();
        let pages: Vec<PortalPage> = serde_json::from_value(json!([
            { "slug": "home", "rows": rows },
            { "slug": "doprava", "title": { "sk": "Doprava" }, "menu": false, "rows": [] },
        ]))
        .unwrap();
        validate_pages(&pages).unwrap();
        assert_eq!(
            pages[0].rows.iter().map(|r| r.blocks.len()).sum::<usize>(),
            9
        );
        assert!(
            pages[0].menu,
            "a page is in the menu unless it says otherwise"
        );
        let back = serde_json::to_value(&pages).unwrap();
        assert_eq!(back[0].get("menu"), None, "{back}");
        assert_eq!(back[1]["menu"], json!(false));
        assert_eq!(
            back[0]["rows"][0]["blocks"][2]["type"],
            json!("dataset-gallery")
        );
        let again: Vec<PortalPage> = serde_json::from_value(back).unwrap();
        assert_eq!(again, pages);
        let PortalBlock::DatasetGallery(g) =
            page(json!([{ "type": "dataset-gallery" }])).unwrap()[0].rows[0].blocks[0].clone()
        else {
            panic!("a gallery");
        };
        assert_eq!(g.limit, 6, "a gallery shows six unless it says otherwise");
    }

    #[test]
    fn an_unknown_field_is_refused_in_every_block_and_an_unknown_type_too() {
        for block in one_of_each().as_array().unwrap() {
            let mut bad = block.clone();
            bad["style"] = json!("color:red");
            let err = page(json!([bad])).expect_err(&block.to_string());
            assert!(err.to_string().contains("style"), "{block}: {err}");
        }
        for extra in [json!({ "html": "<b>x</b>" }), json!({ "script": "x" })] {
            let mut bad = json!({ "type": "text", "markdown": { "sk": "x" } });
            bad.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            assert!(page(json!([bad])).is_err());
        }
        let err = page(json!([{ "type": "iframe", "src": "https://x.sk" }])).expect_err("iframe");
        assert!(err.to_string().contains("iframe"), "{err}");
        let err: std::result::Result<Vec<PortalPage>, _> =
            serde_json::from_value(json!([{ "slug": "home", "css": "body{}", "rows": [] }]));
        assert!(err.unwrap_err().to_string().contains("css"));
        let err: std::result::Result<Vec<PortalPage>, _> = serde_json::from_value(
            json!([{ "slug": "home", "rows": [{ "blocks": [], "width": 2 }] }]),
        );
        assert!(err.unwrap_err().to_string().contains("width"));
    }

    #[test]
    fn each_field_a_block_gets_wrong_is_named() {
        let at = "spec.pages[0].rows[0].blocks[0]";
        for (block, field) in [
            (
                json!({ "type": "hero", "image": "https://x.sk/a.png" }),
                "image",
            ),
            (
                json!({ "type": "hero", "image": format!("/files/{SHA}/../x") }),
                "image",
            ),
            (
                json!({ "type": "hero", "title": { "sk": "<script>" } }),
                "title.sk",
            ),
            (
                json!({ "type": "hero", "title": { "SK": "x" } }),
                "title.SK",
            ),
            (
                json!({ "type": "hero", "intro": { "sk": "x".repeat(MAX_HERO_INTRO + 1) } }),
                "intro.sk",
            ),
            (json!({ "type": "text", "markdown": {} }), "markdown"),
            (
                json!({ "type": "text", "markdown": { "sk": "x".repeat(MAX_BLOCK_MARKDOWN + 1) } }),
                "markdown.sk",
            ),
            (
                json!({ "type": "text", "markdown": { "sk": "a\u{0}b" } }),
                "markdown.sk",
            ),
            (json!({ "type": "dataset-gallery", "limit": 0 }), "limit"),
            (json!({ "type": "dataset-gallery", "limit": 25 }), "limit"),
            (json!({ "type": "dataset-gallery", "tag": "" }), "tag"),
            (
                json!({ "type": "featured-datasets", "datasets": [] }),
                "datasets",
            ),
            (
                json!({ "type": "featured-datasets", "datasets": ["A b"] }),
                "datasets[0]",
            ),
            (
                json!({ "type": "featured-datasets", "datasets": vec!["a"; 13] }),
                "datasets",
            ),
            (json!({ "type": "map", "dataset": "../x" }), "dataset"),
            (json!({ "type": "chart", "dataset": "" }), "dataset"),
            (json!({ "type": "numbers", "items": [] }), "items"),
            (
                json!({ "type": "numbers", "items": ["datasets", "datasets"] }),
                "items[1]",
            ),
            (
                json!({ "type": "link-cards", "cards": [{ "title": { "sk": "x" }, "url": "javascript:alert(1)" }] }),
                "cards[0].url",
            ),
            (
                json!({ "type": "link-cards", "cards": [{ "title": { "sk": "x" }, "url": "//evil.sk" }] }),
                "cards[0].url",
            ),
            (
                json!({ "type": "link-cards", "cards": [{ "title": {}, "url": "/x" }] }),
                "cards[0].title",
            ),
            (
                json!({ "type": "file-list", "files": ["/files/abc/x.pdf"] }),
                "files[0]",
            ),
            (json!({ "type": "file-list", "files": [] }), "files"),
        ] {
            let err = page(json!([block])).expect_err(&block.to_string());
            let want = format!("{at}.{field}");
            assert!(
                err.to_string().contains(&want),
                "{block}: {err} (want {want})"
            );
        }
    }

    #[test]
    fn pages_rows_and_slugs_keep_their_limits() {
        let pages = |v: serde_json::Value| -> Result<()> {
            let pages: Vec<PortalPage> =
                serde_json::from_value(v).map_err(|e| Error::Parse(e.to_string()))?;
            validate_pages(&pages)
        };
        let text = json!({ "type": "text", "markdown": { "sk": "x" } });
        for (v, field) in [
            (
                json!([{ "slug": "Doprava", "title": { "sk": "D" } }]),
                "spec.pages[0].slug",
            ),
            (
                json!([{ "slug": "a/b", "title": { "sk": "D" } }]),
                "spec.pages[0].slug",
            ),
            (
                json!([{ "slug": "home" }, { "slug": "home" }]),
                "spec.pages[1].slug",
            ),
            (json!([{ "slug": "doprava" }]), "spec.pages[0].title"),
            (
                json!([{ "slug": "home", "rows": [{ "blocks": [] }] }]),
                "spec.pages[0].rows[0].blocks",
            ),
            (
                json!([{ "slug": "home", "rows": [{ "blocks": vec![text.clone(); 4] }] }]),
                "spec.pages[0].rows[0].blocks",
            ),
            (
                json!([{ "slug": "home", "rows": vec![json!({ "blocks": [text.clone()] }); MAX_PAGE_ROWS + 1] }]),
                "spec.pages[0].rows",
            ),
        ] {
            let err = pages(v.clone()).expect_err(&v.to_string());
            assert!(err.to_string().contains(field), "{v}: {err}");
        }
        let many: Vec<_> = (0..=MAX_PORTAL_PAGES)
            .map(|i| json!({ "slug": format!("p{i}"), "title": { "sk": "P" } }))
            .collect();
        assert!(pages(json!(many))
            .unwrap_err()
            .to_string()
            .contains("spec.pages"));
    }

    #[test]
    fn only_a_file_address_of_the_store_is_a_file_path() {
        assert_eq!(
            file_sha256(&format!("/files/{SHA}/plan mesta.pdf")),
            Some(SHA)
        );
        for bad in [
            format!("/files/{}/x.pdf", SHA.to_uppercase()),
            format!("/files/{SHA}/"),
            format!("/files/{SHA}"),
            format!("/files/{SHA}/a/b.pdf"),
            format!("/files/{SHA}/a\"onerror=x.png"),
            format!("https://x.sk/files/{SHA}/a.png"),
            format!("/files/{}/a.png", &SHA[1..]),
        ] {
            assert!(!is_file_path(&bad), "{bad}");
        }
    }

    #[test]
    fn a_text_falls_back_to_the_default_language_then_any() {
        let t: Texts = [
            ("en".to_owned(), "Data".to_owned()),
            ("sk".to_owned(), "Dáta".to_owned()),
        ]
        .into();
        assert_eq!(text_in(&t, "sk", "en"), Some("Dáta"));
        assert_eq!(text_in(&t, "cs", "sk"), Some("Dáta"));
        assert_eq!(text_in(&t, "cs", "de"), Some("Data"));
        assert_eq!(text_in(&Texts::new(), "sk", "sk"), None);
    }
}
