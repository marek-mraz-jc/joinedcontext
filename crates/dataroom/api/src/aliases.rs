//! Old dataset names kept as read aliases of renamed datasets (evidence/jc-dev/RENAMES.md): a
//! dataset an installation renamed still answers at its old addresses, so clients holding the
//! old name keep reading it. Only reads (GET, HEAD) are aliased; a write names the dataset it
//! changes by its own name.

use axum::body::Body;
use axum::http::{Method, Request, Uri};
use axum::Router;
use std::collections::BTreeMap;
use std::sync::Arc;

/// The CKAN action reads that take a dataset by `id` or `name_or_id`.
const ACTIONS: [&str; 2] = ["/api/3/action/package_show", "/api/action/package_show"];

/// Whether `name` is a CKAN dataset name: 2 to 100 of `a-z`, `0-9`, `-`, `_`.
fn is_name(name: &str) -> bool {
    (2..=100).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// Refuses an alias table a reader could not trust: a name that is no CKAN dataset name, an old
/// name equal to its new one, or a new name that is itself an old one (no chains).
pub fn check_aliases(aliases: &BTreeMap<String, String>) -> Result<(), String> {
    for (old, new) in aliases {
        if !is_name(old) || !is_name(new) {
            return Err(format!(
                "{old} = {new}: a dataset name is 2-100 of a-z, 0-9, - and _"
            ));
        }
        if old == new {
            return Err(format!("{old}: an alias of itself"));
        }
        if aliases.contains_key(new) {
            return Err(format!("{old} = {new}: {new} is an alias too"));
        }
    }
    Ok(())
}

/// The address `uri` names with an aliased dataset name turned to its new one, or `None` when it
/// names no alias. Covers `package_show`, the dataset page with its resource pages and DCAT
/// files (`/dataset/<name>[.ttl|...][/...]`, with or without a `/<lang>` prefix) and the
/// Portal's catalogue read (`/api/v1/catalogue/datasets/<name>`).
pub fn aliased(uri: &Uri, aliases: &BTreeMap<String, String>) -> Option<Uri> {
    let path = uri.path();
    let query = uri.query();
    if ACTIONS.contains(&path) {
        let q = query?;
        let mut hit = false;
        let pairs: Vec<(String, String)> = url::form_urlencoded::parse(q.as_bytes())
            .map(|(k, v)| {
                let new = match k.as_ref() {
                    "id" | "name_or_id" => aliases.get(v.as_ref()),
                    _ => None,
                };
                hit |= new.is_some();
                (
                    k.into_owned(),
                    new.cloned().unwrap_or_else(|| v.into_owned()),
                )
            })
            .collect();
        if !hit {
            return None;
        }
        let q = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(pairs)
            .finish();
        return format!("{path}?{q}").parse().ok();
    }
    let mut segs: Vec<&str> = path.split('/').collect();
    // ["", "dataset", name, ..] or ["", lang, "dataset", name, ..] or the catalogue read.
    let at = match segs.as_slice() {
        ["", "dataset", _, ..] => 2,
        ["", lang, "dataset", _, ..] if lang.len() == 2 => 3,
        ["", "api", "v1", "catalogue", "datasets", _] => 5,
        _ => return None,
    };
    let (stem, ext) = match segs[at].split_once('.') {
        Some((s, e)) if at != 5 => (s, Some(e)),
        _ => (segs[at], None),
    };
    let new = aliases.get(stem)?;
    let seg = match ext {
        Some(e) => format!("{new}.{e}"),
        None => new.clone(),
    };
    segs[at] = &seg;
    let path = segs.join("/");
    match query {
        Some(q) => format!("{path}?{q}"),
        None => path,
    }
    .parse()
    .ok()
}

/// `app` with every read of an old dataset name answered as its new one (see [`aliased`]). The
/// rewrite runs before routing, so each route sees the new name as if it had been asked.
pub fn with_dataset_aliases(app: Router, aliases: BTreeMap<String, String>) -> Router {
    if aliases.is_empty() {
        return app;
    }
    let aliases = Arc::new(aliases);
    Router::new().fallback_service(tower::util::MapRequest::new(
        app,
        move |mut req: Request<Body>| {
            if matches!(*req.method(), Method::GET | Method::HEAD) {
                if let Some(uri) = aliased(req.uri(), &aliases) {
                    *req.uri_mut() = uri;
                }
            }
            req
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> BTreeMap<String, String> {
        BTreeMap::from([("catalog".to_owned(), "zilina-catalog".to_owned())])
    }

    fn at(uri: &str) -> Option<String> {
        aliased(&uri.parse().unwrap(), &table()).map(|u| u.to_string())
    }

    #[test]
    fn every_read_address_of_the_old_name_names_the_new_one() {
        for (old, new) in [
            (
                "/api/3/action/package_show?id=catalog",
                "/api/3/action/package_show?id=zilina-catalog",
            ),
            (
                "/api/action/package_show?name_or_id=catalog&x=1",
                "/api/action/package_show?name_or_id=zilina-catalog&x=1",
            ),
            ("/dataset/catalog", "/dataset/zilina-catalog"),
            ("/sk/dataset/catalog", "/sk/dataset/zilina-catalog"),
            ("/dataset/catalog.ttl", "/dataset/zilina-catalog.ttl"),
            (
                "/dataset/catalog.rdf?_profile=dcat-ap-sk-2.0",
                "/dataset/zilina-catalog.rdf?_profile=dcat-ap-sk-2.0",
            ),
            (
                "/dataset/catalog/resource/r1",
                "/dataset/zilina-catalog/resource/r1",
            ),
            (
                "/api/v1/catalogue/datasets/catalog",
                "/api/v1/catalogue/datasets/zilina-catalog",
            ),
        ] {
            assert_eq!(at(old).as_deref(), Some(new), "{old}");
        }
    }

    #[test]
    fn other_names_and_other_paths_stay_as_asked() {
        for uri in [
            "/api/3/action/package_show?id=catalogx",
            "/api/3/action/package_show",
            "/api/3/action/package_search?q=catalog",
            "/api/3/action/package_show?q=catalog",
            "/dataset/",
            "/dataset/zilina-catalog",
            "/catalog",
            "/catalog.ttl",
            "/organization/catalog",
            "/xyz/dataset/catalog",
        ] {
            assert_eq!(at(uri), None, "{uri}");
        }
    }

    #[test]
    fn a_table_with_a_bad_name_a_self_alias_or_a_chain_is_refused() {
        let t = |pairs: &[(&str, &str)]| {
            check_aliases(
                &pairs
                    .iter()
                    .map(|(a, b)| ((*a).into(), (*b).into()))
                    .collect(),
            )
        };
        assert!(t(&[("catalog", "zilina-catalog")]).is_ok());
        assert!(t(&[("Catalog", "zilina-catalog")]).is_err());
        assert!(t(&[("catalog", "../x")]).is_err());
        assert!(t(&[("catalog", "catalog")]).is_err());
        assert!(t(&[("a1", "b1"), ("b1", "c1")]).is_err());
    }
}
