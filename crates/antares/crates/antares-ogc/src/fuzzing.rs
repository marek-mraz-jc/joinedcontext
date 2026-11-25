// SPDX-License-Identifier: EUPL-1.2
//! The query readers a request reaches, for the fuzz targets (`feature = "fuzzing"`, T-7803):
//! the `items` query and the `next` page token, read against one fixed collection.

use crate::model::{Collection, Prop, LOCATION};
use serde_json::json;

fn prop(name: &str, sortable: bool) -> Prop {
    Prop {
        name: name.into(),
        iri: format!("https://e.org/{name}"),
        schema: json!({"type": "number"}),
        role: None,
        unit: None,
        description: None,
        sortable,
        kind: "Property".into(),
        reference: false,
        target: None,
    }
}

fn car() -> Collection {
    Collection {
        history: false,
        output: None,
        id: "Car".into(),
        description: None,
        type_iri: "https://e.org/Car".into(),
        geometry: Some(LOCATION.into()),
        properties: vec![prop("speed", true), prop("tags", false)],
        time: None,
    }
}

/// Reads `pairs` as an `items` and as a single-item query; `true` when either accepts it.
#[must_use]
pub fn items_query(pairs: &[(String, String)]) -> bool {
    let c = car();
    let items = crate::items::params(&c, pairs).is_ok();
    let item = crate::items::item_params(&c, pairs).is_ok();
    items || item
}

/// The part, the offset and the store cursor a keyset token names.
pub type Keyset = (usize, usize, Option<String>);

/// A page token read plain and read with its store cursor.
pub type Tokens = (Option<(usize, usize)>, Option<Keyset>);

/// Reads `text` as the `next` token of a request whose digest is `digest` over `parts` parts,
/// plain and with its store cursor.
#[must_use]
pub fn next_token(text: &str, digest: u64, parts: usize) -> Tokens {
    (
        crate::items::read_token(text, digest, parts),
        crate::items::read_keyset_token(text, digest, parts),
    )
}
