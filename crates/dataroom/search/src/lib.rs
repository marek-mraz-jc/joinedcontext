#![forbid(unsafe_code)]
//! Full-text search and faceted filtering for `dataroom` (C2, C14).
//!
//! Supports:
//! - Multilingual full-text search (`sk`, `cs`, `en`, plus `fi` and `ca` titles) with
//!   diacritic folding (`unaccent`) and prefix matching.
//! - Closed-grammar parser for CKAN `fq` filter queries (`organization:...`, `tags:...`,
//!   `res_format:...`, `license_id:...`, `extras_...:...`), refusing Solr local params (`{!...}`).
//! - Facet counts over `publisher`, `theme`, `format`, `keyword`, `licence`, `spatial`, `year`,
//!   and `visibility`, where each facet counts over datasets kept by every *other* facet filter.

use chrono::{Datelike, Utc};
use dataroom_core::vocab::{find_licence, theme_code, theme_label};
use dataroom_core::{
    text_in_lang, Action, Authorizer, DataroomError, Dataset, DatasetFilter, ObjectTarget,
    Publisher, RecordState, RequestCtx, Result, Visibility,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

/// One counted value in a search facet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FacetCount {
    pub value: String,
    pub label: String,
    pub count: usize,
}

/// All facets computed for a search result (C2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchFacets {
    pub publisher: Vec<FacetCount>,
    pub theme: Vec<FacetCount>,
    pub format: Vec<FacetCount>,
    pub keyword: Vec<FacetCount>,
    pub licence: Vec<FacetCount>,
    pub spatial: Vec<FacetCount>,
    pub year: Vec<FacetCount>,
    pub visibility: Vec<FacetCount>,
}

/// Result of a dataset search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchOutcome {
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub datasets: Vec<Dataset>,
    pub facets: SearchFacets,
}

/// Strips combining diacritical marks and lowercases text (`unaccent` + `simple`).
pub fn fold_unaccent(text: &str) -> String {
    text.nfd()
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase()
}

/// Endings Slovak and Czech nouns and adjectives decline through, longest first (`SLAVIC_ENDINGS`
/// in `ours/ckan-theme/plugin.py` line 2109, D2).
pub const SLAVIC_ENDINGS: [&str; 29] = [
    "iach", "ách", "ami", "ých", "ými", "ého", "ému", "ia", "ie", "ii", "iu", "ej", "om", "ov",
    "ou", "ím", "ým", "a", "á", "e", "é", "ě", "i", "í", "o", "u", "ú", "y", "ý",
];

/// Endings whose vowel drops when the word declines (`FLEETING` in `plugin.py` line 2116, D2):
/// "dôchodok" -> "dôchodky", "lekáreň" -> "lekárne", "Lučenec" -> "Lučenca".
pub const FLEETING_ENDINGS: [(&str, &str); 4] =
    [("ok", "k"), ("ek", "k"), ("ec", "c"), ("eň", "n")];

/// Port of `jc_stem(word)` from `ours/ckan-theme/plugin.py` lines 2119-2135 (D2).
pub fn jc_stem(word: &str) -> String {
    let char_count = word.chars().count();
    if !word.chars().all(char::is_alphabetic) || char_count < 4 {
        return word.to_owned();
    }
    let lower = word.to_lowercase();
    for ending in SLAVIC_ENDINGS {
        if lower.ends_with(ending) {
            let end_chars = ending.chars().count();
            if char_count - end_chars >= 4 {
                let stem: String = word.chars().take(char_count - end_chars).collect();
                return format!("({word} OR {stem}*)");
            }
        }
    }
    for (ending, rest) in FLEETING_ENDINGS {
        if lower.ends_with(ending) && char_count - 1 >= 5 {
            let end_chars = ending.chars().count();
            let base: String = word.chars().take(char_count - end_chars).collect();
            return format!("({word} OR {word}* OR {base}{rest}*)");
        }
    }
    format!("({word} OR {word}*)")
}

/// Port of `jc_search_words(q)` from `ours/ckan-theme/plugin.py` lines 2138-2146 (D2).
pub fn jc_search_words(q: &str) -> String {
    if q.is_empty() || q.contains('"') || q.contains(':') {
        return q.to_owned();
    }
    let is_hyphenated = |w: &str| -> bool {
        let parts: Vec<&str> = w.split('-').collect();
        parts.len() >= 2
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.chars().all(|c| c.is_alphanumeric() || c == '_'))
    };
    q.split_whitespace()
        .map(|w| {
            if is_hyphenated(w) {
                format!("\"{w}\"")
            } else {
                jc_stem(w)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Returns the folded prefix stems for one search word (including Slavic ending stripping and
/// fleeting-vowel stems so "ovzdušie" matches "ovzdušia", "dôchodok" matches "dôchodky", and
/// "Lučenec" matches "Lučenca", D2).
pub fn word_stem_candidates(word: &str) -> Vec<String> {
    let mut out = Vec::new();
    let folded_full = fold_unaccent(word);
    if !folded_full.is_empty() {
        out.push(folded_full.clone());
    }
    let char_count = word.chars().count();
    if word.chars().all(char::is_alphabetic) && char_count >= 4 {
        let lower = word.to_lowercase();
        for ending in SLAVIC_ENDINGS {
            if lower.ends_with(ending) {
                let end_chars = ending.chars().count();
                if char_count - end_chars >= 4 {
                    let stem: String = word.chars().take(char_count - end_chars).collect();
                    let f = fold_unaccent(&stem);
                    if !f.is_empty() && !out.contains(&f) {
                        out.push(f);
                    }
                    break;
                }
            }
        }
        for (ending, rest) in FLEETING_ENDINGS {
            if lower.ends_with(ending) && char_count - 1 >= 5 {
                let end_chars = ending.chars().count();
                let base: String = word.chars().take(char_count - end_chars).collect();
                let alt = format!("{base}{rest}");
                let f = fold_unaccent(&alt);
                if !f.is_empty() && !out.contains(&f) {
                    out.push(f);
                }
                break;
            }
        }
    }
    out
}

/// Tokenizes a search query into folded prefix terms, rejecting Solr local-params (`{!...}`).
pub fn parse_search_terms(q: &str) -> Result<Vec<String>> {
    let trimmed = q.trim();
    if trimmed.contains("{!") || trimmed.contains("_query_") || trimmed.contains("_val_") {
        return Err(DataroomError::field_error(
            "q",
            "Solr local parameters and magic fields are not allowed",
        ));
    }
    if trimmed.is_empty() || trimmed == "*:*" || trimmed == "*" {
        return Ok(Vec::new());
    }
    let folded = fold_unaccent(trimmed);
    let terms = folded
        .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
        .filter(|w| !w.is_empty())
        .take(32)
        .map(str::to_owned)
        .collect();
    Ok(terms)
}

/// Parses a CKAN `fq` filter string using a closed grammar into `DatasetFilter` clauses.
///
/// Refuses Solr local parameters (`{!...}`) and unknown syntax.
pub fn apply_fq_to_filter(fq: &str, filter: &mut DatasetFilter) -> Result<()> {
    let trimmed = fq.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    if trimmed.contains("{!") || trimmed.contains("_query_") || trimmed.contains("_val_") {
        return Err(DataroomError::field_error(
            "fq",
            "Solr local parameters are not allowed in fq",
        ));
    }

    // Split on whitespace / AND while respecting double quotes.
    let mut clauses: Vec<(String, String)> = Vec::new();
    let mut rest = trimmed;
    while !rest.is_empty() {
        rest = rest.trim_start();
        if let Some(after_and) = rest.strip_prefix("AND").filter(|s| s.starts_with(char::is_whitespace)) {
            rest = after_and.trim_start();
            continue;
        }
        if let Some(after_plus) = rest.strip_prefix('+') {
            rest = after_plus;
        }
        let Some((key, after_colon)) = rest.split_once(':') else {
            break;
        };
        let key = key.trim().to_owned();
        let after_colon = after_colon.trim_start();
        let (val, tail) = if let Some(quoted) = after_colon.strip_prefix('"') {
            match quoted.split_once('"') {
                Some((inside, remaining)) => (inside.to_owned(), remaining),
                None => (quoted.to_owned(), ""),
            }
        } else {
            match after_colon.find(char::is_whitespace) {
                Some(idx) => (after_colon[..idx].to_owned(), &after_colon[idx..]),
                None => (after_colon.to_owned(), ""),
            }
        };
        clauses.push((key, val));
        rest = tail;
    }

    for (key, val) in clauses {
        if val.is_empty() {
            continue;
        }
        match key.as_str() {
            "organization" | "owner_org" | "publisher" => filter.publishers.push(val),
            "tags" | "vocab_Topics" | "keyword" => filter.tags.push(val),
            "res_format" | "vocab_jc_format" | "format" => filter.formats.push(val),
            "license_id" | "licence" => filter.licences.push(val),
            "theme" | "vocab_jc_topic" | "groups" => filter.themes.push(val),
            "spatial" | "spatial_uri" => filter.spatial.push(val),
            "visibility" => {
                let v = match val.as_str() {
                    "internal" => Visibility::Internal,
                    "restricted" => Visibility::Restricted,
                    _ => Visibility::Public,
                };
                filter.visibilities.push(v);
            }
            "extras_harvest_source" | "harvest_source" => {
                filter.harvest_source = Some(val);
            }
            _ => {}
        }
    }
    Ok(())
}

/// Builds a Postgres `tsquery` expression (`term1:* & term2:*`) from search terms.
pub fn to_pg_prefix_tsquery(terms: &[String]) -> Option<String> {
    let clean: Vec<String> = terms
        .iter()
        .map(|t| {
            t.chars()
                .filter(|c| c.is_alphanumeric() || *c == '_')
                .collect::<String>()
        })
        .filter(|t| !t.is_empty())
        .map(|t| format!("{t}:*"))
        .collect();
    if clean.is_empty() {
        None
    } else {
        Some(clean.join(" & "))
    }
}

/// Checks if a dataset matches the folded prefix terms across its `sk`, `cs`, `en` titles and
/// descriptions, its `fi` and `ca` titles (C2), its name, and its tags/keywords.
pub fn matches_full_text(dataset: &Dataset, terms: &[String]) -> (bool, usize) {
    if terms.is_empty() {
        return (true, 0);
    }
    let mut title_bag = String::new();
    for lang in ["sk", "cs", "en", "fi", "ca"] {
        if let Some(t) = dataset.title.get(lang) {
            title_bag.push(' ');
            title_bag.push_str(t);
        }
    }
    for (_, t) in &dataset.title {
        title_bag.push(' ');
        title_bag.push_str(t);
    }
    let mut desc_bag = String::new();
    for lang in ["sk", "cs", "en"] {
        if let Some(d) = dataset.description.get(lang) {
            desc_bag.push(' ');
            desc_bag.push_str(d);
        }
    }
    for (_, d) in &dataset.description {
        desc_bag.push(' ');
        desc_bag.push_str(d);
    }
    let mut kw_bag = dataset.tags.join(" ");
    for words in dataset.keywords.values() {
        for w in words {
            kw_bag.push(' ');
            kw_bag.push_str(w);
        }
    }

    let folded_name = fold_unaccent(&dataset.name);
    let folded_title = fold_unaccent(&title_bag);
    let folded_desc = fold_unaccent(&desc_bag);
    let folded_kw = fold_unaccent(&kw_bag);

    let words_of = |s: &str| -> Vec<String> {
        s.split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
            .filter(|w| !w.is_empty())
            .map(str::to_owned)
            .collect()
    };

    let name_words = words_of(&folded_name);
    let title_words = words_of(&folded_title);
    let desc_words = words_of(&folded_desc);
    let kw_words = words_of(&folded_kw);

    let mut score = 0usize;
    for term in terms {
        let candidates = word_stem_candidates(term);
        let matches_any = |words: &[String]| -> bool {
            words
                .iter()
                .any(|w| candidates.iter().any(|cand| w.starts_with(cand)))
        };
        let in_name = matches_any(&name_words)
            || candidates.iter().any(|cand| folded_name.contains(cand));
        let in_title = matches_any(&title_words);
        let in_kw = matches_any(&kw_words);
        let in_desc = matches_any(&desc_words);
        if !(in_name || in_title || in_kw || in_desc) {
            return (false, 0);
        }
        if in_name {
            score += 8;
        }
        if in_title {
            score += 4;
        }
        if in_kw {
            score += 2;
        }
        if in_desc {
            score += 1;
        }
    }
    (true, score)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FacetKind {
    Publisher,
    Theme,
    Format,
    Keyword,
    Licence,
    Spatial,
    Year,
    Visibility,
}

const ALL_FACETS: [FacetKind; 8] = [
    FacetKind::Publisher,
    FacetKind::Theme,
    FacetKind::Format,
    FacetKind::Keyword,
    FacetKind::Licence,
    FacetKind::Spatial,
    FacetKind::Year,
    FacetKind::Visibility,
];

struct FiledDataset {
    dataset: Dataset,
    rank: usize,
    publishers: Vec<(String, String)>,
    themes: Vec<(String, String)>,
    formats: Vec<(String, String)>,
    keywords: Vec<(String, String)>,
    licences: Vec<(String, String)>,
    spatial: Vec<(String, String)>,
    years: Vec<(String, String)>,
    visibilities: Vec<(String, String)>,
}

impl FiledDataset {
    fn from_dataset(
        dataset: Dataset,
        rank: usize,
        pub_map: &BTreeMap<String, Publisher>,
        lang: &str,
        this_year: i32,
    ) -> Self {
        let pub_rec = pub_map.get(&dataset.publisher_id);
        let pub_slug = pub_rec
            .map(|p| p.name.clone())
            .unwrap_or_else(|| dataset.publisher_id.clone());
        let pub_title = pub_rec
            .and_then(|p| text_in_lang(&p.title, Some(lang)).map(str::to_owned))
            .unwrap_or_else(|| pub_slug.clone());

        let mut themes = Vec::new();
        for t in &dataset.themes {
            let code = theme_code(t);
            let label = theme_label(&code, lang);
            if !themes.iter().any(|(c, _)| c == &code) {
                themes.push((code, label));
            }
        }

        let mut formats: Vec<(String, String)> = Vec::new();
        for dist in &dataset.distributions {
            if let Some(f) = &dist.format {
                let clean = f.trim().to_owned();
                if !clean.is_empty() && !formats.iter().any(|(c, _)| c.eq_ignore_ascii_case(&clean))
                {
                    formats.push((clean.clone(), clean));
                }
            }
        }
        formats.sort_by(|a, b| a.0.cmp(&b.0));

        let mut keywords: Vec<(String, String)> = Vec::new();
        for t in &dataset.tags {
            if !keywords.iter().any(|(k, _)| k == t) {
                keywords.push((t.clone(), t.clone()));
            }
        }
        for words in dataset.keywords.values() {
            for w in words {
                if !keywords.iter().any(|(k, _)| k == w) {
                    keywords.push((w.clone(), w.clone()));
                }
            }
        }

        let licences = match &dataset.licence_id {
            Some(lid) if !lid.is_empty() => {
                let title = find_licence(lid)
                    .map(|l| l.title)
                    .unwrap_or_else(|| lid.clone());
                vec![(lid.clone(), title)]
            }
            _ => Vec::new(),
        };

        let spatial: Vec<(String, String)> = dataset
            .spatial
            .iter()
            .filter(|s| !s.starts_with('{'))
            .map(|s| {
                let label = s
                    .trim_end_matches('/')
                    .rsplit(['/', '#'])
                    .next()
                    .unwrap_or(s)
                    .to_owned();
                (s.clone(), label)
            })
            .collect();

        let years: Vec<(String, String)> = match &dataset.temporal {
            Some(temp) => match (temp.start.map(|d| d.year()), temp.end.map(|d| d.year())) {
                (Some(s), e) => {
                    let end = e.unwrap_or(this_year).max(s).min(s + 99);
                    (s..=end).map(|y| (y.to_string(), y.to_string())).collect()
                }
                (None, Some(e)) => vec![(e.to_string(), e.to_string())],
                (None, None) => Vec::new(),
            },
            None => Vec::new(),
        };

        let vis = dataset.visibility.as_str().to_owned();
        let visibilities = vec![(vis.clone(), vis)];

        Self {
            dataset,
            rank,
            publishers: vec![(pub_slug, pub_title)],
            themes,
            formats,
            keywords,
            licences,
            spatial,
            years,
            visibilities,
        }
    }

    fn values(&self, kind: FacetKind) -> &[(String, String)] {
        match kind {
            FacetKind::Publisher => &self.publishers,
            FacetKind::Theme => &self.themes,
            FacetKind::Format => &self.formats,
            FacetKind::Keyword => &self.keywords,
            FacetKind::Licence => &self.licences,
            FacetKind::Spatial => &self.spatial,
            FacetKind::Year => &self.years,
            FacetKind::Visibility => &self.visibilities,
        }
    }

    fn kept_by(&self, kind: FacetKind, filter: &DatasetFilter) -> bool {
        match kind {
            FacetKind::Publisher => {
                filter.publishers.is_empty()
                    || self.publishers.iter().any(|(v, _)| {
                        filter.publishers.contains(v)
                            || filter.publishers.contains(&self.dataset.publisher_id)
                    })
            }
            FacetKind::Theme => {
                filter.themes.is_empty()
                    || self.themes.iter().any(|(code, _)| {
                        filter.themes.iter().any(|wanted| {
                            wanted.eq_ignore_ascii_case(code)
                                || theme_code(wanted).eq_ignore_ascii_case(code)
                        })
                    })
            }
            FacetKind::Format => {
                filter.formats.is_empty()
                    || self.formats.iter().any(|(fmt, _)| {
                        filter
                            .formats
                            .iter()
                            .any(|wanted| wanted.eq_ignore_ascii_case(fmt))
                    })
            }
            FacetKind::Keyword => {
                filter.tags.is_empty()
                    || self.keywords.iter().any(|(kw, _)| filter.tags.contains(kw))
            }
            FacetKind::Licence => {
                filter.licences.is_empty()
                    || self
                        .licences
                        .iter()
                        .any(|(l, _)| filter.licences.contains(l))
            }
            FacetKind::Spatial => {
                filter.spatial.is_empty()
                    || self.spatial.iter().any(|(v, label)| {
                        filter.spatial.contains(v) || filter.spatial.contains(label)
                    })
            }
            FacetKind::Year => {
                filter.years.is_empty()
                    || self.years.iter().any(|(y, _)| {
                        y.parse::<i32>()
                            .ok()
                            .is_some_and(|yr| filter.years.contains(&yr))
                    })
            }
            FacetKind::Visibility => {
                filter.visibilities.is_empty()
                    || filter.visibilities.contains(&self.dataset.visibility)
            }
        }
    }
}

/// Runs full-text search, visibility authorization, facet calculation, sorting and pagination.
pub fn search_datasets(
    ctx: &RequestCtx,
    authorizer: &dyn Authorizer,
    datasets: &[Dataset],
    publishers: &[Publisher],
    filter: &DatasetFilter,
    lang: &str,
) -> Result<SearchOutcome> {
    let terms = parse_search_terms(&filter.q)?;
    let this_year = Utc::now().year();

    let mut pub_map = BTreeMap::new();
    for p in publishers {
        pub_map.insert(p.id.clone(), p.clone());
        pub_map.insert(p.name.clone(), p.clone());
    }

    let mut filed = Vec::new();
    for ds in datasets {
        if ds.org_id != ctx.org_id || ds.catalogue_id != ctx.catalogue_id {
            continue;
        }
        match ds.state {
            RecordState::Active => {}
            RecordState::Draft if filter.include_drafts => {}
            RecordState::Withdrawn if filter.include_withdrawn => {}
            _ => continue,
        }
        let pub_rec = pub_map.get(&ds.publisher_id);
        // Withdrawn publisher's datasets leave public feeds/listings unless include_private is requested (C16).
        if pub_rec.is_some_and(|p| p.state == RecordState::Withdrawn) && !filter.include_private {
            continue;
        }
        if ds.visibility != Visibility::Public && !filter.include_private {
            // Anonymous or public-only search excludes non-public datasets.
            if ctx.subject.is_anonymous() {
                continue;
            }
        }
        let target = ObjectTarget::from_dataset(ds, pub_rec);
        if !authorizer.check(ctx, Action::Read, &target).allowed {
            continue;
        }
        if let Some(ref hs) = filter.harvest_source {
            let ds_hs = ds
                .harvest_source_id
                .as_deref()
                .or_else(|| ds.extras.get("harvest_source").map(String::as_str));
            if ds_hs != Some(hs.as_str()) {
                continue;
            }
        }
        let (matched, rank) = matches_full_text(ds, &terms);
        if !matched {
            continue;
        }
        filed.push(FiledDataset::from_dataset(
            ds.clone(),
            rank,
            &pub_map,
            lang,
            this_year,
        ));
    }

    let mut facets = SearchFacets::default();
    for facet_kind in ALL_FACETS {
        let mut counts: BTreeMap<String, (String, usize)> = BTreeMap::new();
        for item in filed.iter().filter(|d| {
            ALL_FACETS
                .iter()
                .all(|other| *other == facet_kind || d.kept_by(*other, filter))
        }) {
            for (val, label) in item.values(facet_kind) {
                counts
                    .entry(val.clone())
                    .or_insert_with(|| (label.clone(), 0))
                    .1 += 1;
            }
        }
        let mut list: Vec<FacetCount> = counts
            .into_iter()
            .map(|(value, (label, count))| FacetCount {
                value,
                label,
                count,
            })
            .collect();
        // Ensure every active filter value on this facet remains present in the facet list
        // (with count 0 when other filters narrow it out), matching CKAN and Portal facet rules (A3).
        let active_for_facet: &[String] = match facet_kind {
            FacetKind::Publisher => &filter.publishers,
            FacetKind::Theme => &filter.themes,
            FacetKind::Format => &filter.formats,
            FacetKind::Keyword => &filter.tags,
            FacetKind::Licence => &filter.licences,
            FacetKind::Spatial => &filter.spatial,
            FacetKind::Year => &[],
            FacetKind::Visibility => &[],
        };
        for wanted in active_for_facet {
            if !list
                .iter()
                .any(|f| f.value.eq_ignore_ascii_case(wanted) || f.label.eq_ignore_ascii_case(wanted))
            {
                let label = match facet_kind {
                    FacetKind::Publisher => pub_map
                        .get(wanted)
                        .and_then(|p| text_in_lang(&p.title, Some(lang)).map(str::to_owned))
                        .unwrap_or_else(|| wanted.clone()),
                    FacetKind::Theme => theme_label(wanted, lang),
                    FacetKind::Licence => find_licence(wanted)
                        .map(|l| l.title)
                        .unwrap_or_else(|| wanted.clone()),
                    _ => wanted.clone(),
                };
                list.push(FacetCount {
                    value: wanted.clone(),
                    label,
                    count: 0,
                });
            }
        }
        // Also include all active publishers from `publishers` when filtering by publisher so that
        // unselected publishers stay selectable even if another facet has narrowed them to 0 (A3).
        if facet_kind == FacetKind::Publisher && !filter.publishers.is_empty() {
            for p in publishers {
                if p.state == RecordState::Active
                    && !list.iter().any(|f| f.value == p.name || f.value == p.id)
                {
                    let label = text_in_lang(&p.title, Some(lang))
                        .unwrap_or(&p.name)
                        .to_owned();
                    list.push(FacetCount {
                        value: p.name.clone(),
                        label,
                        count: 0,
                    });
                }
            }
        }
        list.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.label.cmp(&b.label)));
        match facet_kind {
            FacetKind::Publisher => facets.publisher = list,
            FacetKind::Theme => facets.theme = list,
            FacetKind::Format => facets.format = list,
            FacetKind::Keyword => facets.keyword = list,
            FacetKind::Licence => facets.licence = list,
            FacetKind::Spatial => facets.spatial = list,
            FacetKind::Year => facets.year = list,
            FacetKind::Visibility => facets.visibility = list,
        }
    }

    let mut kept: Vec<FiledDataset> = filed
        .into_iter()
        .filter(|d| ALL_FACETS.iter().all(|f| d.kept_by(*f, filter)))
        .collect();

    match filter.sort.as_deref() {
        Some("name asc") | Some("name") => {
            kept.sort_by(|a, b| a.dataset.name.cmp(&b.dataset.name));
        }
        Some("name desc") => {
            kept.sort_by(|a, b| b.dataset.name.cmp(&a.dataset.name));
        }
        Some("title_string asc") | Some("title asc") => {
            kept.sort_by(|a, b| {
                let ta = text_in_lang(&a.dataset.title, Some(lang)).unwrap_or(&a.dataset.name);
                let tb = text_in_lang(&b.dataset.title, Some(lang)).unwrap_or(&b.dataset.name);
                ta.cmp(tb).then_with(|| a.dataset.name.cmp(&b.dataset.name))
            });
        }
        Some("title_string desc") | Some("title desc") => {
            kept.sort_by(|a, b| {
                let ta = text_in_lang(&a.dataset.title, Some(lang)).unwrap_or(&a.dataset.name);
                let tb = text_in_lang(&b.dataset.title, Some(lang)).unwrap_or(&b.dataset.name);
                tb.cmp(ta).then_with(|| a.dataset.name.cmp(&b.dataset.name))
            });
        }
        Some("metadata_modified desc") | Some("modified desc") => {
            kept.sort_by(|a, b| {
                b.dataset
                    .modified
                    .cmp(&a.dataset.modified)
                    .then_with(|| a.dataset.name.cmp(&b.dataset.name))
            });
        }
        _ => {
            kept.sort_by(|a, b| {
                b.rank
                    .cmp(&a.rank)
                    .then_with(|| b.dataset.modified.cmp(&a.dataset.modified))
                    .then_with(|| a.dataset.name.cmp(&b.dataset.name))
            });
        }
    }

    let total = kept.len();
    let limit = if filter.limit == 0 {
        0
    } else {
        filter.limit.min(1000)
    };
    let page: Vec<Dataset> = if limit == 0 {
        Vec::new()
    } else {
        kept.into_iter()
            .skip(filter.offset)
            .take(limit)
            .map(|f| f.dataset)
            .collect()
    };

    Ok(SearchOutcome {
        total,
        offset: filter.offset,
        limit,
        datasets: page,
        facets,
    })
}

impl SearchFacets {
    /// Formats facets into CKAN `search_facets` and `facets` dictionaries.
    pub fn to_ckan_facets(&self, requested_fields: &[String]) -> (Value, Value) {
        let mut simple = serde_json::Map::new();
        let mut detailed = serde_json::Map::new();

        let fields = if requested_fields.is_empty() {
            vec![
                "organization".to_owned(),
                "tags".to_owned(),
                "res_format".to_owned(),
                "license_id".to_owned(),
                "theme".to_owned(),
            ]
        } else {
            requested_fields.to_vec()
        };

        for field in fields {
            let source = match field.as_str() {
                "organization" | "owner_org" | "publisher" => &self.publisher,
                "tags" | "keyword" => &self.keyword,
                "res_format" | "vocab_jc_format" | "format" => &self.format,
                "license_id" | "licence" => &self.licence,
                "theme" | "vocab_jc_topic" | "groups" => &self.theme,
                "spatial" => &self.spatial,
                "visibility" => &self.visibility,
                _ => continue,
            };
            let mut map = serde_json::Map::new();
            let mut items = Vec::new();
            for item in source {
                map.insert(item.value.clone(), json!(item.count));
                items.push(json!({
                    "name": item.value,
                    "display_name": item.label,
                    "count": item.count,
                }));
            }
            simple.insert(field.clone(), Value::Object(map));
            detailed.insert(
                field.clone(),
                json!({
                    "title": field,
                    "items": items,
                }),
            );
        }

        (Value::Object(simple), Value::Object(detailed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dataroom_core::{
        CatalogueId, Distribution, GrantRule, OrgId, PolicyAuthorizer, Subject, TemporalPeriod,
    };
    use chrono::NaiveDate;
    use uuid::Uuid;

    fn test_ids() -> (OrgId, CatalogueId) {
        (
            OrgId::from_uuid(Uuid::from_u128(1)),
            CatalogueId::from_uuid(Uuid::from_u128(10)),
        )
    }

    fn make_dataset(
        name: &str,
        publisher: &str,
        title_sk: &str,
        themes: &[&str],
        formats: &[&str],
        visibility: Visibility,
    ) -> Dataset {
        let (org_id, catalogue_id) = test_ids();
        Dataset {
            org_id,
            catalogue_id,
            id: format!("pkg-{name}"),
            name: name.into(),
            uri: None,
            dataset_type: "dataset".into(),
            title: BTreeMap::from([("sk".into(), title_sk.into())]),
            description: BTreeMap::from([("sk".into(), format!("Popis {title_sk}"))]),
            publisher_id: publisher.into(),
            publisher: None,
            creator: None,
            contact_point: None,
            visibility,
            state: RecordState::Active,
            licence_id: Some("cc-by".into()),
            licence_url: None,
            themes: themes.iter().map(|s| (*s).to_owned()).collect(),
            eurovoc_themes: Vec::new(),
            keywords: BTreeMap::new(),
            tags: vec!["ovzdusie".into()],
            spatial: vec!["http://data.europa.eu/nuts/code/SK032".into()],
            spatial_resolution_in_meters: None,
            temporal: Some(TemporalPeriod {
                start: NaiveDate::from_ymd_opt(2024, 3, 1),
                end: NaiveDate::from_ymd_opt(2025, 6, 30),
            }),
            temporal_resolution: None,
            frequency: None,
            landing_page: None,
            documentation: Vec::new(),
            conforms_to: Vec::new(),
            applicable_legislation: Vec::new(),
            in_series: None,
            relations: Vec::new(),
            sources: Vec::new(),
            languages: Vec::new(),
            version: None,
            version_notes: BTreeMap::new(),
            access_rights: None,
            provenance: BTreeMap::new(),
            dcat_type: None,
            identifier: None,
            alternate_identifiers: Vec::new(),
            endpoint_url: None,
            generated_by: None,
            harvest_source_id: None,
            harvest_source_item_id: None,
            harvest_hash: None,
            custom_shacl_shapes: None,
            distributions: formats
                .iter()
                .enumerate()
                .map(|(i, fmt)| Distribution {
                    id: format!("res-{name}-{i}"),
                    dataset_id: format!("pkg-{name}"),
                    uri: None,
                    title: BTreeMap::from([("sk".into(), (*fmt).into())]),
                    description: BTreeMap::new(),
                    access_url: format!("https://data.example/{name}.{fmt}"),
                    download_url: None,
                    format: Some((*fmt).into()),
                    media_type: None,
                    compress_format: None,
                    package_format: None,
                    byte_size: None,
                    hash: None,
                    hash_algorithm: None,
                    licence: None,
                    rights: BTreeMap::new(),
                    status: None,
                    availability: None,
                    resource_type: None,
                    conforms_to: Vec::new(),
                    documentation: Vec::new(),
                    languages: Vec::new(),
                    applicable_legislation: Vec::new(),
                    temporal_resolution: None,
                    spatial_resolution_in_meters: None,
                    terms_of_use: None,
                    access_services: Vec::new(),
                    issued: None,
                    modified: None,
                    extras: BTreeMap::new(),
                })
                .collect(),
            extras: BTreeMap::new(),
            restricted_grants: Vec::new(),
            issued: Utc::now(),
            modified: Utc::now(),
        }
    }

    #[test]
    fn facet_counts_and_visibility_match_portal_rules() {
        let (org_id, catalogue_id) = test_ids();
        let datasets = vec![
            make_dataset(
                "air",
                "banskabystrica",
                "Kvalita ovzdušia v meste",
                &["ENVI"],
                &["CSV", "NGSI-LD"],
                Visibility::Public,
            ),
            make_dataset(
                "kpi",
                "bbsk",
                "Ekonomické ukazovatele",
                &["ECON", "ENVI"],
                &["CSV"],
                Visibility::Public,
            ),
            make_dataset(
                "secret",
                "bbsk",
                "Interný rozpočet",
                &["ECON"],
                &["CSV"],
                Visibility::Internal,
            ),
        ];
        let publishers = vec![
            Publisher {
                org_id,
badge_initials: None,
                badge_colour: None,
                declared_by_instance: None,
                catalogue_id,
                id: "pub-bb".into(),
                name: "banskabystrica".into(),
                title: BTreeMap::from([("sk".into(), "Mesto Banská Bystrica".into())]),
                description: BTreeMap::new(),
                uri: "https://data.gov.sk/id/legal-subject/00313271".into(),
                publisher_type: None,
                identifier: None,
                email: None,
                homepage: None,
                logo_url: None,
                contact_point: None,
                state: RecordState::Active,
                merged_into: None,
                editor_grants: Vec::new(),
                created: Utc::now(),
                modified: Utc::now(),
            },
            Publisher {
                org_id,
badge_initials: None,
                badge_colour: None,
                declared_by_instance: None,
                catalogue_id,
                id: "pub-bbsk".into(),
                name: "bbsk".into(),
                title: BTreeMap::from([("sk".into(), "Banskobystrický samosprávny kraj".into())]),
                description: BTreeMap::new(),
                uri: "https://data.gov.sk/id/legal-subject/37828100".into(),
                publisher_type: None,
                identifier: None,
                email: None,
                homepage: None,
                logo_url: None,
                contact_point: None,
                state: RecordState::Active,
                merged_into: None,
                editor_grants: Vec::new(),
                created: Utc::now(),
                modified: Utc::now(),
            },
        ];

        let anon_ctx = RequestCtx::anonymous(org_id, catalogue_id);
        let authz = PolicyAuthorizer::default();

        // 1. Anonymous search sees only the 2 public datasets; internal dataset is absent from counts.
        let mut filter = DatasetFilter {
            limit: 20,
            ..DatasetFilter::default()
        };
        let res = search_datasets(&anon_ctx, &authz, &datasets, &publishers, &filter, "en")
            .expect("search");
        assert_eq!(res.total, 2);
        assert!(res.datasets.iter().all(|d| d.name != "secret"));
        let bbsk_facet = res
            .facets
            .publisher
            .iter()
            .find(|f| f.value == "bbsk")
            .expect("bbsk facet");
        assert_eq!(bbsk_facet.count, 1);

        // 2. Filtering by publisher=bbsk and theme=ECON|ENVI keeps publisher facet showing both publishers.
        filter.publishers = vec!["bbsk".into()];
        filter.themes = vec!["ECON".into(), "ENVI".into()];
        let res2 = search_datasets(&anon_ctx, &authz, &datasets, &publishers, &filter, "en")
            .expect("filtered search");
        assert_eq!(res2.total, 1);
        assert_eq!(res2.datasets[0].name, "kpi");
        assert_eq!(res2.facets.publisher.len(), 2);
        let themes: Vec<(&str, usize)> = res2
            .facets
            .theme
            .iter()
            .map(|f| (f.value.as_str(), f.count))
            .collect();
        assert_eq!(themes, vec![("ECON", 1), ("ENVI", 1)]);

        // 3. Unaccent prefix search finds "ovzdušia" with "ovzdus".
        let q_filter = DatasetFilter {
            q: "ovzdus mesto".into(),
            limit: 20,
            ..DatasetFilter::default()
        };
        let res3 = search_datasets(&anon_ctx, &authz, &datasets, &publishers, &q_filter, "sk")
            .expect("unaccent search");
        assert_eq!(res3.total, 1);
        assert_eq!(res3.datasets[0].name, "air");

        // 4. Member with read permission and include_private=true sees all 3 datasets.
        let member_ctx = RequestCtx::new(
            org_id,
            catalogue_id,
            Subject::authenticated("alice", vec![org_id], Vec::new()),
        );
        let member_authz = PolicyAuthorizer::new(vec![GrantRule {
            org_id,
            catalogue_id: Some(catalogue_id),
            principal: "alice".into(),
            actions: vec![Action::Read],
            publisher_id: None,
        }]);
        let priv_filter = DatasetFilter {
            include_private: true,
            limit: 20,
            ..DatasetFilter::default()
        };
        let res4 = search_datasets(
            &member_ctx,
            &member_authz,
            &datasets,
            &publishers,
            &priv_filter,
            "sk",
        )
        .expect("member search");
        assert_eq!(res4.total, 3);
    }

    #[test]
    fn solr_local_params_are_refused() {
        let mut filter = DatasetFilter::default();
        assert!(apply_fq_to_filter("{!knn f=vector}[1,2]", &mut filter).is_err());
        assert!(parse_search_terms("{!func}ord(id)").is_err());
    }

    #[test]
    fn d2_slavic_stemming_and_search_words_match_plugin_py() {
        assert_eq!(jc_stem("ovzdušie"), "(ovzdušie OR ovzduš*)");
        assert_eq!(jc_stem("dôchodok"), "(dôchodok OR dôchodok* OR dôchodk*)");
        assert_eq!(jc_stem("lekáreň"), "(lekáreň OR lekáreň* OR lekárn*)");
        assert_eq!(jc_stem("Lučenec"), "(Lučenec OR Lučenec* OR Lučenc*)");
        assert_eq!(jc_stem("most"), "(most OR most*)");
        assert_eq!(jc_stem("sk"), "sk");
        assert_eq!(
            jc_search_words("bike-sharing ovzdušie"),
            "\"bike-sharing\" (ovzdušie OR ovzduš*)"
        );

        let (org_id, catalogue_id) = test_ids();
        let ds = make_dataset(
            "air",
            "banskabystrica",
            "Kvalita ovzdušia v Lučenci a dôchodky",
            &["ENVI"],
            &["CSV"],
            Visibility::Public,
        );
        // Searching "ovzdušie Lučenec dôchodok" finds "ovzdušia ... Lučenci ... dôchodky"!
        let terms = parse_search_terms("ovzdušie Lučenec dôchodok").unwrap();
        let (matched, score) = matches_full_text(&ds, &terms);
        assert!(matched && score > 0);
        let _ = (org_id, catalogue_id);
    }
}
