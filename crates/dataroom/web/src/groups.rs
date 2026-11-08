//! The topics page (`/group/`, CKAN's group list "Témy", parity gap G3): one card per EU data
//! theme (`EU_DATA_THEMES`) and per other theme the datasets name, with its dataset count.

use crate::i18n::{jc_rows, jc_t};
use crate::{PageChrome, UiLabels};
use askama::Template;
use dataroom_core::vocab::{theme_code, theme_label, EU_DATA_THEMES};
use dataroom_core::{BrandingBlock, Catalogue, Dataset, RecordState, Visibility};
use std::collections::BTreeMap;

/// One topic card.
#[derive(Debug, Clone)]
pub struct TopicCardView {
    pub code: String,
    pub label: String,
    pub count_label: String,
    pub url: String,
}

#[derive(Template)]
#[template(path = "group_list.html")]
pub struct GroupListPageTemplate {
    pub chrome: PageChrome,
    pub labels: UiLabels,
    pub query: String,
    pub sort: String,
    pub found_heading: String,
    pub search_placeholder: String,
    pub topics_help: String,
    pub empty_topics: String,
    pub topics: Vec<TopicCardView>,
}

/// The topic cards of `datasets` (public and active only), filtered by `query` on label or code
/// and sorted by label (`sort` "title desc" reverses).
pub fn topic_cards(
    catalogue: &Catalogue,
    datasets: &[Dataset],
    query: &str,
    sort: &str,
    lang: &str,
    lang_prefix: &str,
) -> Vec<TopicCardView> {
    let mut counts: BTreeMap<String, usize> = EU_DATA_THEMES
        .iter()
        .map(|(c, ..)| ((*c).to_owned(), 0))
        .collect();
    for ds in datasets.iter().filter(|d| {
        d.state == RecordState::Active
            && d.visibility == Visibility::Public
            && !catalogue.hidden_datasets.contains_key(&d.name)
    }) {
        let codes: std::collections::BTreeSet<String> = ds
            .themes
            .iter()
            .map(|t| theme_code(t).to_ascii_uppercase())
            .filter(|c| !c.is_empty())
            .collect();
        for c in codes {
            *counts.entry(c).or_insert(0) += 1;
        }
    }
    let q = query.trim().to_lowercase();
    let mut cards: Vec<TopicCardView> = counts
        .into_iter()
        .map(|(code, n)| {
            let enc: String = url::form_urlencoded::byte_serialize(code.as_bytes()).collect();
            TopicCardView {
                label: theme_label(&code, lang),
                count_label: jc_rows(n, "datasets_n", lang),
                url: format!("{lang_prefix}/dataset/?vocab_jc_topic={enc}"),
                code,
            }
        })
        .filter(|c| {
            q.is_empty()
                || c.label.to_lowercase().contains(&q)
                || c.code.to_lowercase().contains(&q)
        })
        .collect();
    cards.sort_by(|a, b| a.label.cmp(&b.label));
    if sort == "title desc" {
        cards.reverse();
    }
    cards
}

/// Renders `/group/` (G3).
#[allow(clippy::too_many_arguments)]
pub fn render_group_list_page(
    catalogue: &Catalogue,
    installation_branding: Option<&BrandingBlock>,
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
        "/group/",
        None,
        mount_prefix,
        "group",
        csp_nonce,
    );
    let labels = UiLabels::for_lang(&chrome.lang);
    let topics = topic_cards(
        catalogue,
        datasets,
        query,
        sort,
        &chrome.lang,
        &chrome.lang_prefix,
    );
    let found_heading = if topics.is_empty() {
        jc_t("no_topics_found", &chrome.lang)
    } else {
        jc_rows(topics.len(), "topics_found", &chrome.lang)
    };
    GroupListPageTemplate {
        search_placeholder: jc_t("search_topics", &chrome.lang),
        topics_help: jc_t("topics_help", &chrome.lang),
        empty_topics: jc_t("empty_topics", &chrome.lang),
        chrome,
        labels,
        query: query.to_owned(),
        sort: sort.to_owned(),
        found_heading,
        topics,
    }
    .render()
    .unwrap_or_default()
}
