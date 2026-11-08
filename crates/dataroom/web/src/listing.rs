//! What CKAN's list pages carry besides the rows: links that keep the other filters, the pager
//! and the active-filter pills (parity gaps G4, G5).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Datasets per list page, as CKAN's `ckan.datasets_per_page` default.
pub const PER_PAGE: usize = 20;

/// Query parameters a list link never carries over: the page (a new filter starts at 1) and the
/// facet "show more" switches.
fn carried(key: &str) -> bool {
    key != "page" && !(key.starts_with('_') && key.ends_with("_limit"))
}

/// `base` with the request's `params`, every `(key, value)` of `set` applied (`None` removes the
/// key) and the page and empty values dropped: CKAN's `h.remove_url_param` / `h.add_url_param`.
pub fn link_with(
    base: &str,
    params: &BTreeMap<String, String>,
    set: &[(&str, Option<&str>)],
) -> String {
    let mut kept: BTreeMap<&str, &str> = params
        .iter()
        .filter(|(k, v)| carried(k) && !v.is_empty())
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    for (k, v) in set {
        match v {
            Some(v) => kept.insert(k, v),
            None => kept.remove(k),
        };
    }
    if kept.is_empty() {
        return base.to_owned();
    }
    let mut ser = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in kept {
        ser.append_pair(k, v);
    }
    format!("{base}?{}", ser.finish())
}

/// One pager link (`.pagination` in CKAN's `snippets/pagination`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageLinkView {
    pub label: String,
    pub href: String,
    pub active: bool,
}

/// The pager of a list of `total` rows on `page`: up to five numbers around the page, `«` and `»`.
/// Empty when everything fits on one page.
pub fn pager(
    base: &str,
    params: &BTreeMap<String, String>,
    total: usize,
    page: usize,
) -> Vec<PageLinkView> {
    let last = total.div_ceil(PER_PAGE).max(1);
    if last <= 1 {
        return Vec::new();
    }
    let page = page.clamp(1, last);
    let href = |n: usize| {
        let n = n.to_string();
        let mut url = link_with(base, params, &[]);
        url.push(if url.contains('?') { '&' } else { '?' });
        url.push_str("page=");
        url.push_str(&n);
        url
    };
    let first = page
        .saturating_sub(2)
        .max(1)
        .min(last.saturating_sub(4).max(1));
    let mut out = Vec::new();
    if page > 1 {
        out.push(PageLinkView {
            label: "«".into(),
            href: href(page - 1),
            active: false,
        });
    }
    for n in first..=(first + 4).min(last) {
        out.push(PageLinkView {
            label: n.to_string(),
            href: href(n),
            active: n == page,
        });
    }
    if page < last {
        out.push(PageLinkView {
            label: "»".into(),
            href: href(page + 1),
            active: false,
        });
    }
    out
}

/// One active filter shown under the result count (`p.filter-list` in CKAN's `search_form`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterPillView {
    pub facet: String,
    pub label: String,
    pub remove_href: String,
}

/// The pills of the active filters among `facets` (`(param, facet title)`, in display order);
/// `label_of(param, value)` names a value as its facet does.
pub fn filter_pills(
    base: &str,
    params: &BTreeMap<String, String>,
    facets: &[(&str, String)],
    label_of: impl Fn(&str, &str) -> String,
) -> Vec<FilterPillView> {
    facets
        .iter()
        .filter_map(|(param, title)| {
            let value = params.get(*param).filter(|v| !v.is_empty())?;
            Some(FilterPillView {
                facet: title.clone(),
                label: label_of(param, value),
                remove_href: link_with(base, params, &[(param, None)]),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(p: &[(&str, &str)]) -> BTreeMap<String, String> {
        p.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn a_link_keeps_the_other_filters_and_drops_the_page() {
        let p = params(&[
            ("organization", "bbsk"),
            ("res_format", "CSV"),
            ("page", "2"),
        ]);
        assert_eq!(
            link_with("/dataset/", &p, &[("tags", Some("a b"))]),
            "/dataset/?organization=bbsk&res_format=CSV&tags=a+b"
        );
        assert_eq!(
            link_with("/dataset/", &p, &[("res_format", None)]),
            "/dataset/?organization=bbsk"
        );
        assert_eq!(
            link_with("/dataset/", &params(&[("page", "3")]), &[]),
            "/dataset/"
        );
    }

    #[test]
    fn the_pager_shows_numbers_around_the_page_and_arrows() {
        let p = params(&[("q", "x")]);
        assert!(pager("/d", &p, 20, 1).is_empty());
        let labels = |v: Vec<PageLinkView>| v.into_iter().map(|l| l.label).collect::<Vec<_>>();
        assert_eq!(labels(pager("/d", &p, 22, 1)), ["1", "2", "»"]);
        assert_eq!(labels(pager("/d", &p, 22, 2)), ["«", "1", "2"]);
        assert_eq!(
            labels(pager("/d", &p, 200, 6)),
            ["«", "4", "5", "6", "7", "8", "»"]
        );
        assert_eq!(
            labels(pager("/d", &p, 200, 10)),
            ["«", "6", "7", "8", "9", "10"]
        );
        let two = pager("/d", &p, 22, 1);
        assert_eq!(two[1].href, "/d?q=x&page=2");
        assert!(two[0].active);
    }

    #[test]
    fn a_pill_removes_only_its_own_filter() {
        let p = params(&[("organization", "bbsk"), ("res_format", "CSV"), ("q", "")]);
        let pills = filter_pills(
            "/dataset/",
            &p,
            &[
                ("organization", "Vydavateľ".into()),
                ("res_format", "Formát".into()),
                ("q", "Q".into()),
            ],
            |_, v| v.to_uppercase(),
        );
        assert_eq!(pills.len(), 2);
        assert_eq!(pills[0].label, "BBSK");
        assert_eq!(pills[0].remove_href, "/dataset/?res_format=CSV");
        assert_eq!(pills[1].remove_href, "/dataset/?organization=bbsk");
    }
}
