// SPDX-License-Identifier: EUPL-1.2
//! The NGSI-LD names a caller writes beside a standard's own query (ADR-0028 rule 6, T-3758):
//! read under the core @context with the view's own merged in, and written back as the terms
//! the surface's request defines ([`crate::OpsRequest::context`]), so the store answers in the
//! names the surface reads and the caller's names never redefine the surface's.

use std::sync::LazyLock;

use antares_jsonld::Context;
use antares_ql::{QNode, QPath};
use serde_json::{Map, Value};

use crate::{Refusal, ViewOps};

static CORE: LazyLock<Context> = LazyLock::new(antares_jsonld::core_context);

const BAD_Q: &str = "the NGSI-LD query is not one";
const BAD_NAMES: &str = "the NGSI-LD names are not a list of names";

/// The core @context with `local` merged in; `None` when it does not merge, which the loader
/// already refused the view for.
pub(crate) fn of_view(local: Option<&Map<String, Value>>) -> Option<Context> {
    let mut names = CORE.clone();
    if let Some(local) = local {
        names.merge_object(local).ok()?;
    }
    Some(names)
}

impl ViewOps {
    /// The IRI `name` stands for in this view: a term of the view's `@context` or of the core
    /// one, a compact IRI, an absolute IRI, or a name under the default vocabulary.
    #[must_use]
    pub fn expand_name(&self, name: &str) -> String {
        self.names.expand_key(name)
    }

    /// An NGSI-LD `q` as a caller wrote it in a URL (percent-encoded, in the view's names),
    /// with each Attribute and type name it holds replaced by `term` of its IRI, rendered as
    /// plain text. A name the core @context defines and the view does not redefine (a member
    /// such as `observedAt`) stays as written; a bracket is a key path or a language, never a
    /// name, and stays too.
    ///
    /// # Errors
    /// [`Refusal::Malformed`] when the text is no `q` (TS 104 175 7.2.3.2).
    pub fn rename_q(
        &self,
        written: &str,
        term: &mut dyn FnMut(&str) -> String,
    ) -> Result<String, Refusal> {
        rename_q(&self.names, written, term)
    }

    /// A comma-separated list of Attribute names as a caller wrote it in a URL (`pick`, `omit`,
    /// `geoproperty`), each as its IRI.
    ///
    /// # Errors
    /// [`Refusal::Malformed`] for an empty name or one that is not a plain name (a linked
    /// projection `rel{a,b}` included).
    pub fn expand_names(&self, written: &str) -> Result<Vec<String>, Refusal> {
        expand_names(&self.names, written)
    }
}

fn rename_q(
    names: &Context,
    written: &str,
    term: &mut dyn FnMut(&str) -> String,
) -> Result<String, Refusal> {
    let text = antares_ql::percent_decode(written.as_bytes());
    let node = antares_ql::parse_q(&text).map_err(|_| Refusal::Malformed(BAD_Q))?;
    Ok(rename_node(names, node, term).to_string())
}

fn expand_names(names: &Context, written: &str) -> Result<Vec<String>, Refusal> {
    antares_ql::percent_decode(written.as_bytes())
        .split(',')
        .map(|name| {
            let name = name.trim();
            let plain = !name.is_empty()
                && name
                    .chars()
                    .all(|c| !c.is_whitespace() && !matches!(c, '{' | '}' | '[' | ']' | ';' | '|'));
            plain
                .then(|| names.expand_key(name))
                .ok_or(Refusal::Malformed(BAD_NAMES))
        })
        .collect()
}

fn rename_node(names: &Context, node: QNode, term: &mut dyn FnMut(&str) -> String) -> QNode {
    match node {
        QNode::And(ns) => QNode::And(
            ns.into_iter()
                .map(|n| rename_node(names, n, term))
                .collect(),
        ),
        QNode::Or(ns) => QNode::Or(
            ns.into_iter()
                .map(|n| rename_node(names, n, term))
                .collect(),
        ),
        QNode::Cmp { path, op, value } => QNode::Cmp {
            path: rename_path(names, path, term),
            op,
            value,
        },
        QNode::Exists { path, negated } => QNode::Exists {
            path: rename_path(names, path, term),
            negated,
        },
    }
}

fn rename_path(names: &Context, mut path: QPath, term: &mut dyn FnMut(&str) -> String) -> QPath {
    for link in &mut path.links {
        link.attr = rename(names, &link.attr, term);
        for ty in &mut link.types {
            *ty = rename(names, ty, term);
        }
    }
    for name in &mut path.path {
        *name = rename(names, name, term);
    }
    path
}

/// `name` as written when the core @context defines it and means the same here, else the
/// surface's term for its IRI: a minted term never meets a caller's name of the same spelling,
/// since only a core term is kept.
fn rename(names: &Context, name: &str, term: &mut dyn FnMut(&str) -> String) -> String {
    let iri = names.expand_key(name);
    if CORE.term(name).is_some() && CORE.expand_key(name) == iri {
        name.to_owned()
    } else {
        term(&iri)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn view() -> Context {
        let local = json!({
            "speed": "https://example.org/speed",
            "Car": "https://example.org/Car",
            "ex": "https://example.org/",
            "location": "https://example.org/where",
        });
        of_view(local.as_object()).unwrap_or_default()
    }

    /// Mints `t{n}` per IRI, as a surface does, and remembers them.
    fn minted(q: &str) -> Result<(String, Vec<String>), Refusal> {
        let mut seen: Vec<String> = Vec::new();
        let out = rename_q(&view(), q, &mut |iri| {
            let n = seen.iter().position(|s| s == iri).unwrap_or_else(|| {
                seen.push(iri.to_owned());
                seen.len().saturating_sub(1)
            });
            format!("t{n}")
        })?;
        Ok((out, seen))
    }

    #[test]
    fn a_q_is_renamed_into_the_surfaces_terms_and_core_members_stay() -> Result<(), Refusal> {
        let (q, iris) = minted("speed%3E5;rpm==2|speed.observedAt%3C2026-01-01T00:00:00Z")?;
        // the renderer quotes a date-time; the parser reads it back as one
        assert_eq!(q, "t0>5;t1==2|t0.observedAt<\"2026-01-01T00:00:00Z\"");
        assert_eq!(
            iris,
            [
                "https://example.org/speed",
                "https://uri.etsi.org/ngsi-ld/default-context/rpm"
            ]
        );
        // a core term the view redefines is the view's
        let (q, iris) = minted("location")?;
        assert_eq!(q, "t0");
        assert_eq!(iris, ["https://example.org/where"]);
        // a caller's `t0` is a name under the vocabulary, never the surface's `t0`
        let (q, iris) = minted("t0==1")?;
        assert_eq!(q, "t0==1");
        assert_eq!(iris, ["https://uri.etsi.org/ngsi-ld/default-context/t0"]);
        Ok(())
    }

    #[test]
    fn a_linked_path_renames_its_relationship_and_types_and_keeps_its_bracket(
    ) -> Result<(), Refusal> {
        let (q, iris) = minted("owner%7BCar:speed%7D==1;address[city]==%22B%22")?;
        assert_eq!(q, "t0{t1:t2}==1;t3[city]==\"B\"");
        assert_eq!(
            iris.get(1).map(String::as_str),
            Some("https://example.org/Car")
        );
        Ok(())
    }

    #[test]
    fn a_broken_q_and_a_broken_name_list_are_malformed() {
        assert!(matches!(minted("%28speed"), Err(Refusal::Malformed(_))));
        assert!(matches!(minted("speed=="), Err(Refusal::Malformed(_))));
        assert!(matches!(minted(""), Err(Refusal::Malformed(_))));
        let names = view();
        for bad in ["", "a,,b", "rel%7Ba%7D", "a%20b", "a;b"] {
            assert!(
                matches!(expand_names(&names, bad), Err(Refusal::Malformed(_))),
                "{bad}"
            );
        }
        assert_eq!(
            expand_names(&names, "speed%2Cex:rpm").ok(),
            Some(vec![
                "https://example.org/speed".to_owned(),
                "https://example.org/rpm".to_owned()
            ])
        );
    }

    #[test]
    fn a_view_context_that_does_not_merge_is_no_view() {
        let local = json!({"speed": {"@id": "https://example.org/speed", "@context": {}}});
        assert!(of_view(local.as_object()).is_none());
        assert!(of_view(None).is_some());
    }
}
