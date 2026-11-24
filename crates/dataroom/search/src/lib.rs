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

use dataroom_core::vocab::{find_licence, theme_code, theme_label};
use dataroom_core::{
    text_in_lang, Authorizer, DataroomError, Dataset, DatasetFilter, Publisher, RecordState,
    RequestCtx, Result, SearchCandidate, SearchQuery, Store, Visibility,
};
use jc_time::TimestampTz;
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
        if lower.ends_with(ending) && char_count > 5 {
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
                    // At least four letters, shorter than the word: never empty, never a repeat.
                    out.push(fold_unaccent(&stem));
                    break;
                }
            }
        }
        for (ending, rest) in FLEETING_ENDINGS {
            if lower.ends_with(ending) && char_count > 5 {
                let end_chars = ending.chars().count();
                let base: String = word.chars().take(char_count - end_chars).collect();
                // A fleeting ending is no Slavic ending (k, c, n against vowels, h, m, j, v), so
                // the stem above was not taken: this one is new.
                out.push(fold_unaccent(&format!("{base}{rest}")));
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
    // "", "*" and "*:*" (match everything) hold no word, so they come out as no term.
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
/// The grammar is `key:value` or `key:"quoted value"` clauses joined by whitespace, `AND` or a
/// leading `+`. Solr local parameters (`{!...}`), negation (`-key:`, `NOT`), `OR` and words
/// outside a clause are refused: ignoring them would widen or change the result silently.
/// Unknown keys (`dataset_type:dataset`, which CKAN clients send) are accepted and ignored.
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
        if let Some(after_and) = rest
            .strip_prefix("AND")
            .filter(|s| s.starts_with(char::is_whitespace))
        {
            rest = after_and.trim_start();
            continue;
        }
        if let Some(after_plus) = rest.strip_prefix('+') {
            rest = after_plus;
        }
        let refuse = |what: &str| {
            Err(DataroomError::field_error(
                "fq",
                format!("{what} is not supported in fq; use key:value clauses joined by AND"),
            ))
        };
        let Some((key, after_colon)) = rest.split_once(':') else {
            return refuse(&format!("'{}'", rest.trim()));
        };
        let key = key.trim().to_owned();
        if key.starts_with('-') || key.starts_with('!') {
            return refuse("negation");
        }
        if key.contains(char::is_whitespace) || key.is_empty() {
            return refuse(&format!("'{key}'"));
        }
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

/// The longest word a Postgres `tsvector` holds (2046 bytes): index words and query prefixes are
/// both cut there, at a character boundary, so the store and [`SearchLexemes::rank`] agree.
pub const MAX_LEXEME_BYTES: usize = 2046;

fn cut_lexeme(mut word: String) -> String {
    if word.len() > MAX_LEXEME_BYTES {
        let mut end = MAX_LEXEME_BYTES;
        while !word.is_char_boundary(end) {
            end -= 1;
        }
        word.truncate(end);
    }
    word
}

/// The folded words of `text` as search splits them: letters, digits, `-` and `_`.
fn folded_words(text: &str) -> Vec<String> {
    let mut words: Vec<String> = fold_unaccent(text)
        .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
        .filter(|w| !w.is_empty())
        .map(|w| cut_lexeme(w.to_owned()))
        .collect();
    words.sort();
    words.dedup();
    words
}

/// Per query word of `q`, the folded prefixes any of which matches it ([`word_stem_candidates`]),
/// cut like the index words; refuses what [`parse_search_terms`] refuses.
pub fn term_candidates(q: &str) -> Result<Vec<Vec<String>>> {
    Ok(parse_search_terms(q)?
        .iter()
        .map(|term| {
            let mut cands: Vec<String> = word_stem_candidates(term)
                .into_iter()
                .map(cut_lexeme)
                .collect();
            cands.dedup();
            cands
        })
        .collect())
}

/// The folded words a dataset is found by (C2, S11-38): every title (`sk`, `cs`, `en`, `fi`,
/// `ca` and any other language), every description, its tags and keywords, and its name. The
/// Postgres store keeps them as `tsvector` columns, so SQL and [`SearchLexemes::rank`] match
/// the same words.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchLexemes {
    /// The folded name; a prefix matches anywhere in it.
    pub name: String,
    pub title: Vec<String>,
    pub keywords: Vec<String>,
    pub description: Vec<String>,
}

impl SearchLexemes {
    #[must_use]
    pub fn of(dataset: &Dataset) -> Self {
        let join = |texts: &mut dyn Iterator<Item = &String>| {
            texts.fold(String::new(), |mut bag, t| {
                bag.push(' ');
                bag.push_str(t);
                bag
            })
        };
        let keywords = join(
            &mut dataset
                .tags
                .iter()
                .chain(dataset.keywords.values().flatten()),
        );
        Self {
            name: fold_unaccent(&dataset.name),
            title: folded_words(&join(&mut dataset.title.values())),
            keywords: folded_words(&keywords),
            description: folded_words(&join(&mut dataset.description.values())),
        }
    }

    /// Every suffix of the folded name, cut to [`MAX_LEXEME_BYTES`]: a prefix starts one of
    /// them exactly when the name contains it, which a `tsvector` prefix query can test.
    #[must_use]
    pub fn name_suffixes(&self) -> Vec<String> {
        self.name
            .char_indices()
            .map(|(i, _)| cut_lexeme(self.name[i..].to_owned()))
            .collect()
    }

    /// `None` unless every term matches the name, a title, a keyword or a description word;
    /// else the rank: per term 8 for the name, 4 for a title, 2 for a keyword, 1 for the
    /// description. The Postgres store computes the same sum in SQL.
    #[must_use]
    pub fn rank(&self, terms: &[Vec<String>]) -> Option<usize> {
        let mut score = 0usize;
        for candidates in terms {
            let starts = |words: &[String]| {
                words
                    .iter()
                    .any(|w| candidates.iter().any(|cand| w.starts_with(cand.as_str())))
            };
            let in_name = candidates
                .iter()
                .any(|cand| self.name.contains(cand.as_str()));
            let hits = [
                (in_name, 8),
                (starts(&self.title), 4),
                (starts(&self.keywords), 2),
                (starts(&self.description), 1),
            ];
            if !hits.iter().any(|(hit, _)| *hit) {
                return None;
            }
            score += hits
                .iter()
                .filter(|(hit, _)| *hit)
                .map(|(_, w)| w)
                .sum::<usize>();
        }
        Some(score)
    }
}

/// Checks if a dataset matches the folded prefix terms across its titles, descriptions, name,
/// tags and keywords ([`SearchLexemes::rank`]); the rank is 0 when there is no term.
pub fn matches_full_text(dataset: &Dataset, terms: &[String]) -> (bool, usize) {
    let candidates: Vec<Vec<String>> = terms
        .iter()
        .map(|t| {
            word_stem_candidates(t)
                .into_iter()
                .map(cut_lexeme)
                .collect()
        })
        .collect();
    match SearchLexemes::of(dataset).rank(&candidates) {
        Some(rank) => (true, rank),
        None => (false, 0),
    }
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
            Some(temp) => match (
                temp.start.map(|d| i32::from(d.0.year())),
                temp.end.map(|d| i32::from(d.0.year())),
            ) {
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
    let candidates = datasets
        .iter()
        .map(|d| SearchCandidate {
            dataset: d.clone(),
            rank: None,
        })
        .collect();
    search_candidates(ctx, authorizer, candidates, publishers, filter, lang)
}

/// One search page read from `store` in one transaction (S11-38): the store matches and ranks
/// the text, `keep` drops what the route never lists (dashboards, hidden datasets), and
/// [`search_candidates`] authorizes, counts facets and pages; the page's datasets are full
/// records. Facets count over `facet_publishers` when given, else over every publisher. Also
/// returns the catalogue's publishers.
pub async fn search_store(
    store: &dyn Store,
    ctx: &RequestCtx,
    authorizer: &dyn Authorizer,
    filter: &DatasetFilter,
    lang: &str,
    keep: &(dyn Fn(&Dataset, &[Publisher]) -> bool + Sync),
    facet_publishers: Option<&[Publisher]>,
) -> Result<(SearchOutcome, Vec<Publisher>)> {
    let query = SearchQuery {
        terms: term_candidates(&filter.q)?,
        include_drafts: filter.include_drafts,
        include_withdrawn: filter.include_withdrawn,
        public_only: ctx.subject.is_anonymous() && !filter.include_private,
    };
    let mut outcome = None;
    let mut pick = |publishers: &[Publisher], mut candidates: Vec<SearchCandidate>| {
        candidates.retain(|c| keep(&c.dataset, publishers));
        let found = search_candidates(
            ctx,
            authorizer,
            candidates,
            facet_publishers.unwrap_or(publishers),
            filter,
            lang,
        )?;
        let ids = found.datasets.iter().map(|d| d.id.clone()).collect();
        outcome = Some(found);
        Ok(ids)
    };
    let page = store.search_page(ctx, &query, &mut pick).await?;
    let mut outcome = outcome.ok_or_else(|| {
        DataroomError::Storage("the store answered a search without choosing a page".into())
    })?;
    outcome.datasets = page.datasets;
    Ok((outcome, page.publishers))
}

/// [`search_datasets`] over store candidates: a candidate with a rank was matched by the store,
/// one without is matched here against its full record.
pub fn search_candidates(
    ctx: &RequestCtx,
    authorizer: &dyn Authorizer,
    candidates: Vec<SearchCandidate>,
    publishers: &[Publisher],
    filter: &DatasetFilter,
    lang: &str,
) -> Result<SearchOutcome> {
    let terms = term_candidates(&filter.q)?;
    let this_year = i32::from(TimestampTz::now().utc().year());

    let mut pub_map = BTreeMap::new();
    for p in publishers {
        pub_map.insert(p.id.clone(), p.clone());
        pub_map.insert(p.name.clone(), p.clone());
    }

    let mut filed = Vec::new();
    for SearchCandidate { dataset: ds, rank } in candidates {
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
        // The flags widen an editor's search only: a caller who may not propose the dataset never
        // sees a draft, a withdrawn record or a withdrawn publisher's dataset (as `package_show`).
        if !dataroom_core::dataset_visible(authorizer, ctx, &ds, pub_rec, false) {
            continue;
        }
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
        if let Some(ref hs) = filter.harvest_source {
            let ds_hs = ds
                .harvest_source_id
                .as_deref()
                .or_else(|| ds.extras.get("harvest_source").map(String::as_str));
            if ds_hs != Some(hs.as_str()) {
                continue;
            }
        }
        let Some(rank) = rank.or_else(|| SearchLexemes::of(&ds).rank(&terms)) else {
            continue;
        };
        filed.push(FiledDataset::from_dataset(
            ds, rank, &pub_map, lang, this_year,
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
            if !list.iter().any(|f| {
                f.value.eq_ignore_ascii_case(wanted) || f.label.eq_ignore_ascii_case(wanted)
            }) {
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
        Action, CatalogueId, Distribution, GrantRule, OrgId, PolicyAuthorizer, Subject,
        TemporalPeriod,
    };
    use jc_time::Date;
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
                start: Date::new(2024, 3, 1),
                end: Date::new(2025, 6, 30),
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
            issued: TimestampTz::now(),
            modified: TimestampTz::now(),
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
                created: TimestampTz::now(),
                modified: TimestampTz::now(),
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
                created: TimestampTz::now(),
                modified: TimestampTz::now(),
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

    fn publisher(id: &str, name: &str, title: &str, state: RecordState) -> Publisher {
        let (org_id, catalogue_id) = test_ids();
        serde_json::from_value(json!({
            "orgId": org_id, "catalogueId": catalogue_id, "id": id, "name": name,
            "title": {"sk": title}, "description": {}, "uri": format!("https://e.org/{name}"),
            "state": state, "editorGrants": [],
            "created": "2026-11-01T10:00:00Z", "modified": "2026-11-01T10:00:00Z"
        }))
        .expect("publisher fixture")
    }

    fn fq(text: &str) -> Result<DatasetFilter> {
        let mut f = DatasetFilter::default();
        apply_fq_to_filter(text, &mut f).map(|()| f)
    }

    #[test]
    fn fq_closed_grammar_maps_every_key_and_refuses_the_rest() {
        let f = fq(r#"organization:a +owner_org:"b c" AND tags:t1 keyword:t2 res_format:CSV license_id:cc-by theme:ENVI spatial:SK032 visibility:internal visibility:restricted visibility:other harvest_source:h1 dataset_type:dataset publisher:"""#).unwrap();
        assert_eq!(f.publishers, ["a", "b c"]);
        assert_eq!(f.tags, ["t1", "t2"]);
        assert_eq!(f.formats, ["CSV"]);
        assert_eq!(f.licences, ["cc-by"]);
        assert_eq!(f.themes, ["ENVI"]);
        assert_eq!(f.spatial, ["SK032"]);
        assert_eq!(
            f.visibilities,
            [
                Visibility::Internal,
                Visibility::Restricted,
                Visibility::Public
            ]
        );
        assert_eq!(f.harvest_source.as_deref(), Some("h1"));
        assert_eq!(fq("tags:\"unterminated").unwrap().tags, ["unterminated"]);
        assert_eq!(fq("   ").unwrap(), DatasetFilter::default());

        // Each of these used to be dropped without a word, widening or changing the result.
        for bad in [
            "-organization:a",
            "!tags:x",
            "organization:a OR organization:b",
            "NOT organization:a",
            "organization:a stray",
            "justwords",
            ":empty-key",
            "_query_:x",
            "_val_:x",
        ] {
            let e = fq(bad).expect_err(bad);
            assert!(
                matches!(&e, DataroomError::Validation { fields, .. } if fields.contains_key("fq")),
                "{bad}: {e:?}"
            );
        }
    }

    #[test]
    fn lexemes_rank_and_cut_like_the_store_index() {
        let mut ds = make_dataset(
            "bike_Šharing-2",
            "p",
            "Kvalita ovzdušia",
            &[],
            &[],
            Visibility::Public,
        );
        ds.description = BTreeMap::from([("en".into(), "Hourly PM10 data".into())]);
        ds.tags = vec!["doprava".into()];
        ds.keywords.insert("en".into(), vec!["mobility".into()]);
        let lex = SearchLexemes::of(&ds);
        assert_eq!(lex.name, "bike_sharing-2");
        assert_eq!(lex.title, ["kvalita", "ovzdusia"]);
        assert_eq!(lex.keywords, ["doprava", "mobility"]);
        assert_eq!(lex.description, ["data", "hourly", "pm10"]);
        let suffixes = lex.name_suffixes();
        assert_eq!(suffixes.len(), 14);
        assert_eq!(suffixes.last().map(String::as_str), Some("2"));

        let rank = |q: &str| lex.rank(&term_candidates(q).unwrap());
        assert_eq!(rank(""), Some(0));
        assert_eq!(rank("sharing"), Some(8), "inside the name");
        assert_eq!(rank("ovzdušie"), Some(4), "a declined title word");
        assert_eq!(rank("mobil"), Some(2));
        assert_eq!(rank("hour"), Some(1));
        assert_eq!(rank("kvalita hour"), Some(5), "every term adds");
        assert_eq!(rank("kvalita nikde"), None, "every term must match");
        assert_eq!(
            lex.rank(&[Vec::new()]),
            None,
            "a term with no prefix matches nothing"
        );

        // A word past a tsvector's 2046 bytes is cut at a character boundary, in the index and
        // in the query alike.
        let long = format!("{}ž", "a".repeat(MAX_LEXEME_BYTES - 1));
        assert_eq!(cut_lexeme(long.clone()).len(), MAX_LEXEME_BYTES - 1);
        ds.description.insert("sk".into(), long.clone());
        let cut = SearchLexemes::of(&ds);
        assert!(cut.description.iter().all(|w| w.len() <= MAX_LEXEME_BYTES));
        assert!(term_candidates(&long).unwrap()[0]
            .iter()
            .all(|c| c.len() <= MAX_LEXEME_BYTES));
        assert_eq!(cut.rank(&term_candidates(&long).unwrap()), Some(1));
    }

    #[test]
    fn query_terms_tsquery_and_search_words() {
        assert!(parse_search_terms("a _query_:x").is_err());
        assert!(parse_search_terms("_val_:x").is_err());
        for empty in ["", "  ", "*", "*:*"] {
            assert!(parse_search_terms(empty).unwrap().is_empty());
        }
        assert_eq!(
            parse_search_terms("Ovzdušie, bike-sharing!").unwrap(),
            ["ovzdusie", "bike-sharing"]
        );
        let many = "slovo ".repeat(40);
        assert_eq!(
            parse_search_terms(&many).unwrap().len(),
            32,
            "terms are capped"
        );

        assert_eq!(jc_search_words(""), "");
        assert_eq!(jc_search_words("\"exact\" x"), "\"exact\" x");
        assert_eq!(jc_search_words("tags:x"), "tags:x");
        assert_eq!(
            jc_search_words("a- -b"),
            "a- -b",
            "short or broken words stay"
        );
        assert_eq!(jc_stem("dom2x"), "dom2x");
        assert_eq!(word_stem_candidates("ovzdušie"), ["ovzdusie", "ovzdus"]);
        assert_eq!(word_stem_candidates("dôchodok"), ["dochodok", "dochodk"]);
        assert_eq!(word_stem_candidates("most"), ["most"]);
        assert!(word_stem_candidates("").is_empty());
    }

    /// The bounds of each rule: a stem keeps four letters, a fleeting vowel drops only past five,
    /// a quote or a field name alone keeps the query as typed, `_` joins a hyphenated word, a
    /// local parameter alone is refused, and a name matches inside a word.
    #[test]
    fn stemming_and_query_rules_hold_at_their_bounds() {
        assert_eq!(
            jc_stem("kava"),
            "(kava OR kava*)",
            "a three-letter stem is no stem"
        );
        assert_eq!(word_stem_candidates("kava"), ["kava"]);
        assert_eq!(
            jc_stem("zámok"),
            "(zámok OR zámok*)",
            "five letters keep their vowel"
        );
        assert_eq!(word_stem_candidates("zámok"), ["zamok"]);
        assert_eq!(
            word_stem_candidates("a1ovzdušie"),
            ["a1ovzdusie"],
            "a digit stops stemming"
        );
        assert_eq!(jc_search_words("\"exact\" ovzdušie"), "\"exact\" ovzdušie");
        assert_eq!(jc_search_words("tags:x ovzdušie"), "tags:x ovzdušie");
        assert_eq!(jc_search_words("bike_a-sharing"), "\"bike_a-sharing\"");
        assert!(parse_search_terms("{!lucene}x").is_err());

        let ds = make_dataset("airquality", "p", "Státie", &[], &[], Visibility::Public);
        let terms = parse_search_terms("quality").expect("terms");
        assert!(
            matches_full_text(&ds, &terms).0,
            "a name matches inside a word"
        );
    }

    /// With a query, "name desc" and "metadata_modified desc" are their own orders, not the rank's;
    /// a dataset of another catalogue of the same organization is never searched.
    #[test]
    fn named_sorts_override_the_rank_and_other_catalogues_stay_out() {
        let (org_id, catalogue_id) = test_ids();
        let ctx = RequestCtx::anonymous(org_id, catalogue_id);
        let authz = PolicyAuthorizer::default();
        let mut ranked = make_dataset("aa-ovzdusie", "p", "Ovzdušie", &[], &[], Visibility::Public);
        ranked.modified = TimestampTz::from_second(TimestampTz::now().as_second() - 3 * 86_400)
            .expect("in range");
        let newer = make_dataset("zz", "p", "Ovzdušie", &[], &[], Visibility::Public);
        let mut elsewhere =
            make_dataset("elsewhere", "p", "Ovzdušie", &[], &[], Visibility::Public);
        elsewhere.catalogue_id = CatalogueId::from_uuid(Uuid::from_u128(77));
        let all = [ranked, newer, elsewhere];
        let sorted = |sort: Option<&str>| {
            let filter = DatasetFilter {
                q: "ovzdusie".into(),
                sort: sort.map(str::to_owned),
                limit: 20,
                ..DatasetFilter::default()
            };
            search_datasets(&ctx, &authz, &all, &[], &filter, "sk")
                .expect("search")
                .datasets
                .into_iter()
                .map(|d| d.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            sorted(None),
            ["aa-ovzdusie", "zz"],
            "the name match ranks first"
        );
        assert_eq!(sorted(Some("name desc")), ["zz", "aa-ovzdusie"]);
        assert_eq!(
            sorted(Some("metadata_modified desc")),
            ["zz", "aa-ovzdusie"]
        );
        assert_eq!(sorted(Some("modified desc")), ["zz", "aa-ovzdusie"]);
    }

    /// The search keeps another catalogue's dataset out, and a withdrawn publisher's one and a
    /// non-public one from an anonymous caller behind include_private, by itself, whatever the
    /// Authorizer (the Portal's live one) allows.
    #[test]
    fn an_editor_search_keeps_its_catalogue_and_withdrawn_publishers_behind_the_flag() {
        struct AllowAll;
        impl Authorizer for AllowAll {
            fn check(
                &self,
                _: &RequestCtx,
                _: Action,
                _: &dataroom_core::ObjectTarget,
            ) -> dataroom_core::Decision {
                dataroom_core::Decision::allow("test")
            }
        }
        let (org_id, catalogue_id) = test_ids();
        let editor = Subject::authenticated("ed", vec![org_id], Vec::new());
        let ctx = RequestCtx::new(org_id, catalogue_id, editor);
        let authz = AllowAll;
        let mut elsewhere =
            make_dataset("elsewhere", "pub-a", "Inde", &[], &[], Visibility::Public);
        elsewhere.catalogue_id = CatalogueId::from_uuid(Uuid::from_u128(77));
        let all = [
            make_dataset("mine", "pub-a", "Moje", &[], &[], Visibility::Public),
            make_dataset("orphan", "pub-w", "Sirota", &[], &[], Visibility::Public),
            elsewhere,
        ];
        let pubs = [
            publisher("pub-a", "a-slug", "A", RecordState::Active),
            publisher("pub-w", "w-slug", "W", RecordState::Withdrawn),
        ];
        let names = |include_private: bool| {
            let filter = DatasetFilter {
                include_private,
                sort: Some("name".into()),
                limit: 20,
                ..DatasetFilter::default()
            };
            search_datasets(&ctx, &authz, &all, &pubs, &filter, "sk")
                .expect("search")
                .datasets
                .into_iter()
                .map(|d| d.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(false), ["mine"]);
        assert_eq!(names(true), ["mine", "orphan"]);

        // Without include_private an anonymous caller gets no non-public dataset; with it the
        // Authorizer decides.
        let anonymous = RequestCtx::anonymous(org_id, catalogue_id);
        let internal = [make_dataset(
            "inner",
            "pub-a",
            "Vnútorný",
            &[],
            &[],
            Visibility::Internal,
        )];
        let public_only = DatasetFilter {
            limit: 20,
            ..DatasetFilter::default()
        };
        let found = search_datasets(&anonymous, &authz, &internal, &pubs, &public_only, "sk");
        assert_eq!(found.expect("search").total, 0);
    }

    /// A licence id left empty is no licence facet, and an open period spans at most 100 years.
    #[test]
    fn facets_skip_an_empty_licence_and_bound_an_open_period() {
        let mut ds = make_dataset("old", "p", "Starý", &[], &[], Visibility::Public);
        ds.licence_id = Some(String::new());
        ds.temporal = Some(TemporalPeriod {
            start: Date::new(1900, 1, 1),
            end: None,
        });
        let filed = FiledDataset::from_dataset(ds, 0, &BTreeMap::new(), "sk", 2026);
        assert!(filed.licences.is_empty());
        assert_eq!(filed.years.len(), 100);
        assert_eq!(filed.years.last().map(|y| y.0.as_str()), Some("1999"));
    }

    #[test]
    fn full_text_ranks_name_title_keywords_and_description() {
        let mut ds = make_dataset("parking-zones", "p", "Státie", &[], &[], Visibility::Public);
        ds.description.insert("sk".into(), "Text".into());
        ds.title.insert("fi".into(), "Pysäköinti".into());
        ds.title.insert("de".into(), "Parkzonen".into());
        ds.description.insert("de".into(), "Beschreibung".into());
        ds.keywords.insert("en".into(), vec!["mobility".into()]);
        let score = |q: &str| matches_full_text(&ds, &parse_search_terms(q).unwrap());
        assert_eq!(score(""), (true, 0));
        assert_eq!(score("parking"), (true, 8), "name");
        assert_eq!(score("statie"), (true, 4), "title, folded");
        assert_eq!(score("pysakointi"), (true, 4), "Finnish title");
        assert_eq!(score("parkzonen"), (true, 4), "any other title language");
        assert_eq!(score("mobil"), (true, 2), "keyword prefix");
        assert_eq!(score("ovzdusie"), (true, 2), "tag");
        assert_eq!(score("beschreib"), (true, 1), "description");
        assert_eq!(score("zones-x"), (false, 0));
        assert_eq!(score("parking statie").1, 12, "every term adds");
    }

    #[test]
    fn search_filters_sorts_pages_and_keeps_active_facet_values() {
        let (org_id, catalogue_id) = test_ids();
        let ctx = RequestCtx::anonymous(org_id, catalogue_id);
        let authz = PolicyAuthorizer::default();
        let mut a = make_dataset(
            "alpha",
            "pub-a",
            "Zeta",
            &["ENVI"],
            &["CSV"],
            Visibility::Public,
        );
        a.modified = TimestampTz::from_second(TimestampTz::now().as_second() - 2 * 86_400)
            .expect("in range");
        a.harvest_source_id = Some("h1".into());
        a.keywords
            .insert("sk".into(), vec!["voda".into(), "ovzdusie".into()]);
        let mut b = make_dataset(
            "beta",
            "b-slug",
            "Alfa",
            &["TRAN", "TRAN"],
            &["json", "JSON", " "],
            Visibility::Public,
        );
        b.licence_id = Some("unknown-licence".into());
        b.extras.insert("harvest_source".into(), "h2".into());
        b.spatial.push("{\"type\":\"Point\"}".into());
        b.temporal = Some(TemporalPeriod {
            start: None,
            end: Date::new(2020, 1, 1),
        });
        let mut c = make_dataset("gamma", "pub-a", "Mid", &[], &[], Visibility::Public);
        c.temporal = Some(TemporalPeriod {
            start: Date::new(2026, 1, 1),
            end: Date::new(2020, 1, 1),
        });
        c.licence_id = None;
        let mut draft = make_dataset("draft", "pub-a", "Draft", &[], &[], Visibility::Public);
        draft.state = RecordState::Draft;
        let mut gone = make_dataset("gone", "pub-a", "Gone", &[], &[], Visibility::Public);
        gone.state = RecordState::Withdrawn;
        let orphan = make_dataset("orphan", "pub-w", "Orphan", &[], &[], Visibility::Public);
        let mut foreign = make_dataset("foreign", "pub-a", "Foreign", &[], &[], Visibility::Public);
        foreign.org_id = OrgId::from_uuid(Uuid::from_u128(99));
        let mut empty_temporal =
            make_dataset("none", "pub-a", "None", &[], &[], Visibility::Public);
        empty_temporal.temporal = Some(TemporalPeriod {
            start: None,
            end: None,
        });
        let all = vec![a, b, c, draft, gone, orphan, foreign, empty_temporal];
        let pubs = vec![
            publisher("pub-a", "a-slug", "Vydavateľ A", RecordState::Active),
            publisher("pub-w", "w-slug", "Zrušený", RecordState::Withdrawn),
            publisher("pub-z", "z-slug", "Iný", RecordState::Active),
        ];
        let run = |f: DatasetFilter| {
            search_datasets(&ctx, &authz, &all, &pubs, &f, "sk").expect("search")
        };
        let names = |o: &SearchOutcome| {
            o.datasets
                .iter()
                .map(|d| d.name.clone())
                .collect::<Vec<_>>()
        };
        let base = DatasetFilter {
            limit: 20,
            ..DatasetFilter::default()
        };

        // Drafts, withdrawn records, a withdrawn publisher's datasets and other organizations' are out.
        let res = run(base.clone());
        assert_eq!(res.total, 4, "{:?}", names(&res));
        // The flags widen a member's search; an anonymous caller asking for them gets the same 4.
        let member = RequestCtx::new(
            org_id,
            catalogue_id,
            Subject::authenticated("alice", vec![org_id], Vec::new()),
        );
        let run_member = |f: DatasetFilter| {
            search_datasets(&member, &authz, &all, &pubs, &f, "sk").expect("search")
        };
        for (flags, widened) in [
            (
                DatasetFilter {
                    include_drafts: true,
                    ..base.clone()
                },
                5,
            ),
            (
                DatasetFilter {
                    include_withdrawn: true,
                    ..base.clone()
                },
                5,
            ),
            (
                DatasetFilter {
                    include_private: true,
                    ..base.clone()
                },
                5,
            ),
        ] {
            assert_eq!(run_member(flags.clone()).total, widened, "{flags:?}");
            assert!(
                !names(&run_member(flags.clone())).contains(&"orphan".to_owned())
                    || flags.include_private,
                "a withdrawn publisher's dataset needs include_private: {flags:?}"
            );
            assert_eq!(run(flags.clone()).total, 4, "anonymous {flags:?}");
        }

        // Facets: themes deduplicated, formats case-folded and sorted, blank ones dropped,
        // unknown licences labelled by id, GeoJSON spatial skipped, years spanned and bounded.
        let fmt: Vec<_> = res
            .facets
            .format
            .iter()
            .map(|f| (f.value.as_str(), f.count))
            .collect();
        assert_eq!(fmt, [("CSV", 1), ("json", 1)]);
        assert!(res
            .facets
            .theme
            .iter()
            .any(|f| f.value == "TRAN" && f.count == 1));
        assert!(res
            .facets
            .licence
            .iter()
            .any(|f| f.value == "unknown-licence" && f.label == "unknown-licence"));
        assert!(res.facets.spatial.iter().all(|f| !f.value.starts_with('{')));
        assert_eq!(res.facets.spatial[0].label, "SK032");
        let years: Vec<_> = res.facets.year.iter().map(|f| f.value.as_str()).collect();
        assert!(
            years.contains(&"2020") && years.contains(&"2024") && years.contains(&"2026"),
            "{years:?}"
        );
        assert!(res.facets.keyword.iter().any(|f| f.value == "voda"));
        assert_eq!(
            res.facets
                .publisher
                .iter()
                .find(|f| f.value == "a-slug")
                .map(|f| f.label.as_str()),
            Some("Vydavateľ A")
        );
        assert_eq!(res.facets.visibility[0].value, "public");

        // Sorting.
        let sorted = |s: &str| {
            names(&run(DatasetFilter {
                sort: Some(s.into()),
                ..base.clone()
            }))
        };
        assert_eq!(sorted("name asc"), ["alpha", "beta", "gamma", "none"]);
        assert_eq!(sorted("name"), ["alpha", "beta", "gamma", "none"]);
        assert_eq!(sorted("name desc"), ["none", "gamma", "beta", "alpha"]);
        assert_eq!(
            sorted("title_string asc"),
            ["beta", "gamma", "none", "alpha"]
        );
        assert_eq!(sorted("title desc"), ["alpha", "none", "gamma", "beta"]);
        assert_eq!(
            sorted("metadata_modified desc").last().map(String::as_str),
            Some("alpha")
        );
        let ranked = run(DatasetFilter {
            q: "alpha zeta".into(),
            ..base.clone()
        });
        assert_eq!(names(&ranked), ["alpha"]);

        // Paging: rows=0 counts only, rows over 1000 are capped, offset skips.
        let counted = run(DatasetFilter {
            limit: 0,
            ..base.clone()
        });
        assert_eq!(
            (counted.total, counted.datasets.len(), counted.limit),
            (4, 0, 0)
        );
        assert_eq!(
            run(DatasetFilter {
                limit: 5000,
                ..base.clone()
            })
            .limit,
            1000
        );
        let paged = run(DatasetFilter {
            offset: 3,
            sort: Some("name".into()),
            ..base.clone()
        });
        assert_eq!(names(&paged), ["none"]);

        // Filters per facet, harvest source from the column or the extras.
        let only = |f: DatasetFilter| names(&run(f));
        assert_eq!(
            only(DatasetFilter {
                harvest_source: Some("h1".into()),
                ..base.clone()
            }),
            ["alpha"]
        );
        assert_eq!(
            only(DatasetFilter {
                harvest_source: Some("h2".into()),
                ..base.clone()
            }),
            ["beta"]
        );
        assert_eq!(
            only(DatasetFilter {
                formats: vec!["JSON".into()],
                ..base.clone()
            }),
            ["beta"]
        );
        assert_eq!(
            only(DatasetFilter {
                tags: vec!["voda".into()],
                ..base.clone()
            }),
            ["alpha"]
        );
        assert_eq!(
            only(DatasetFilter {
                licences: vec!["unknown-licence".into()],
                ..base.clone()
            }),
            ["beta"]
        );
        assert_eq!(
            only(DatasetFilter {
                spatial: vec!["SK032".into()],
                ..base.clone()
            })
            .len(),
            4,
            "label matches"
        );
        assert_eq!(
            only(DatasetFilter {
                years: vec![2026],
                sort: Some("name".into()),
                ..base.clone()
            }),
            ["gamma"]
        );
        assert_eq!(
            only(DatasetFilter {
                visibilities: vec![Visibility::Internal],
                ..base.clone()
            }),
            Vec::<String>::new()
        );
        assert_eq!(
            only(DatasetFilter {
                publishers: vec!["pub-a".into()],
                sort: Some("name".into()),
                ..base.clone()
            }),
            ["alpha", "gamma", "none"],
            "by id too"
        );
        assert_eq!(
            only(DatasetFilter {
                themes: vec![
                    "http://publications.europa.eu/resource/authority/data-theme/TRAN".into()
                ],
                ..base.clone()
            }),
            ["beta"]
        );

        // An active filter value nothing matches stays in its facet with count 0 and a label.
        let res = run(DatasetFilter {
            publishers: vec!["z-slug".into(), "nobody".into()],
            themes: vec!["AGRI".into()],
            licences: vec!["cc-zero".into(), "own".into()],
            formats: vec!["XLSX".into()],
            ..base.clone()
        });
        assert_eq!(res.total, 0);
        let label = |list: &[FacetCount], v: &str| {
            list.iter()
                .find(|f| f.value == v)
                .map(|f| (f.label.clone(), f.count))
        };
        assert_eq!(
            label(&res.facets.publisher, "z-slug"),
            Some(("Iný".into(), 0))
        );
        assert_eq!(
            label(&res.facets.publisher, "nobody"),
            Some(("nobody".into(), 0))
        );
        assert_eq!(
            label(&res.facets.publisher, "a-slug").map(|l| l.1),
            Some(0),
            "the other facets narrow it to 0, it stays selectable"
        );
        assert!(label(&res.facets.theme, "AGRI").is_some_and(|(l, c)| c == 0 && l != "AGRI"));
        assert!(
            label(&res.facets.licence, "cc-zero").is_some_and(|(l, c)| c == 0 && l != "cc-zero")
        );
        assert_eq!(label(&res.facets.licence, "own"), Some(("own".into(), 0)));
        assert_eq!(label(&res.facets.format, "XLSX"), Some(("XLSX".into(), 0)));

        // A query refused by the term parser refuses the search.
        assert!(search_datasets(
            &ctx,
            &authz,
            &all,
            &pubs,
            &DatasetFilter {
                q: "{!x}".into(),
                ..base
            },
            "sk"
        )
        .is_err());
    }

    #[test]
    fn ckan_facet_dictionaries() {
        let facets = SearchFacets {
            publisher: vec![FacetCount {
                value: "a".into(),
                label: "A".into(),
                count: 2,
            }],
            spatial: vec![FacetCount {
                value: "s".into(),
                label: "S".into(),
                count: 1,
            }],
            ..SearchFacets::default()
        };
        let (simple, detailed) = facets.to_ckan_facets(&[]);
        assert_eq!(simple["organization"], json!({"a": 2}));
        assert_eq!(simple.as_object().unwrap().len(), 5);
        assert_eq!(detailed["organization"]["items"][0]["display_name"], "A");
        let asked: Vec<String> = [
            "spatial",
            "visibility",
            "owner_org",
            "groups",
            "license_id",
            "format",
            "keyword",
            "nonsense",
        ]
        .map(str::to_owned)
        .to_vec();
        let (simple, detailed) = facets.to_ckan_facets(&asked);
        assert_eq!(simple["spatial"], json!({"s": 1}));
        assert_eq!(simple["visibility"], json!({}));
        assert!(simple.get("nonsense").is_none());
        assert_eq!(detailed["owner_org"]["title"], "owner_org");
    }
    #[test]
    fn equal_sort_keys_fall_back_to_the_name_and_tags_count_once() {
        let (org_id, catalogue_id) = test_ids();
        let ctx = RequestCtx::anonymous(org_id, catalogue_id);
        let authz = PolicyAuthorizer::default();
        let when =
            TimestampTz::from_second(TimestampTz::now().as_second() - 86_400).expect("in range");
        let twin = |name: &str| {
            let mut d = make_dataset(name, "pub-a", "Rovnaký", &[], &[], Visibility::Public);
            d.modified = when;
            d
        };
        let mut b = twin("b-twin");
        b.tags = vec!["voda".into(), "voda".into(), "les".into()];
        b.keywords.insert("sk".into(), vec!["voda".into()]);
        let all = vec![b, twin("c-twin"), twin("a-twin")];
        let pubs = vec![publisher(
            "pub-a",
            "a-slug",
            "Vydavateľ A",
            RecordState::Active,
        )];
        let names = |sort: Option<&str>| {
            let mut f = DatasetFilter::public_default();
            f.sort = sort.map(str::to_owned);
            search_datasets(&ctx, &authz, &all, &pubs, &f, "sk")
                .expect("search")
                .datasets
                .into_iter()
                .map(|d| d.name)
                .collect::<Vec<_>>()
        };
        let by_name = ["a-twin", "b-twin", "c-twin"];
        for sort in [
            Some("title asc"),
            Some("title_string desc"),
            Some("metadata_modified desc"),
            None,
        ] {
            assert_eq!(names(sort), by_name, "ties under {sort:?} order by name");
        }

        let mut f = DatasetFilter::public_default();
        f.tags = vec!["voda".into()];
        let res = search_datasets(&ctx, &authz, &all, &pubs, &f, "sk").expect("search");
        assert_eq!(res.total, 1);
        let count = |v: &str| {
            res.facets
                .keyword
                .iter()
                .find(|k| k.value == v)
                .map(|k| k.count)
        };
        assert_eq!(
            count("voda"),
            Some(1),
            "a repeated tag counts its dataset once"
        );
        assert_eq!(count("les"), Some(1));
    }
}
