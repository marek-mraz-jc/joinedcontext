// SPDX-License-Identifier: EUPL-1.2
//! The view's EDM model (odata-linkml-mapping.md sections 3, 4 and 7.2):
//! the Tenant's `LinkML` schema intersected with what the view shows, built
//! once per request key and used for `$metadata` and for resolving every
//! name a request carries (M1 to M3). A type or an Attribute the view does
//! not show leaves no trace in it (S4).

use linkml_model::{edm_types, Builtin, Range, EDM_TYPE_TAG};
use odata_csdl::{
    Annotation, CsdlVersion, EntityContainer, EntitySet, EntityType, Expr, Include, Model,
    NavigationProperty, NavigationPropertyBinding, Property, Reference, ReferentialConstraint,
    Scale, Schema, Term, TypeRef,
};
use std::collections::{BTreeMap, BTreeSet};

use crate::names::mangle;

/// The surface's own vocabulary (section 3.2): `Iri`, `Kind`, and the
/// abstract base every EntityType derives from.
pub const VOCABULARY: &str = "Antares.NgsiLd";

/// The version of this surface's mapping (`docs/src/odata.md` section "Mapping version",
/// T-5207): bumped by every change to how a slot shapes, types or reads (`shape`,
/// `property`, the write and filter readers), so a client that caches `$metadata` sees it in
/// `Core.SchemaVersion` and `$schemaversion` before a member changes type under it.
pub const MAPPING_VERSION: u32 = 1;

/// The version of the [`VOCABULARY`] terms, stated as its `Core.SchemaVersion`: a term
/// removed or retyped takes a new namespace (`Antares.NgsiLd.V2`) beside this one.
pub const VOCABULARY_VERSION: &str = "1";
const VOCABULARY_ALIAS: &str = "NgsiLd";
const BASE: &str = "NgsiLd.Entity";

const VOCABULARIES: &str = "https://oasis-tcs.github.io/odata-vocabularies/vocabularies/";

/// The slot annotation that names the Property keying a navigation's
/// target (R6, T-3761).
pub const REFERENCE_TAG: &str = "odata_referential_constraint";

/// The Entity types the view shows, each with the Attributes it shows, as
/// expanded IRIs.
pub type Shown = BTreeMap<String, BTreeSet<String>>;

/// The writes the view offers (its `operations`), which the Capabilities
/// annotations state (C2 to C5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Writable {
    /// `create_entity`: `POST {Set}` (W1).
    pub insert: bool,
    /// `merge_entity` or `replace_entity`: `PATCH` or `PUT` of an Entity
    /// (W4, W6).
    pub update: bool,
    /// `delete_entity` (W9).
    pub delete: bool,
}

/// How the surface reads and writes one member of an EntityType.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    /// A Property's `value` (P1, P6 when `collection`), typed by its range.
    Value {
        /// The Edm type of the value, or of each item.
        edm: &'static str,
        /// A ListProperty or a multivalued slot.
        collection: bool,
    },
    /// A GeoProperty (P3): `Edm.Geography`, never filtered as a value.
    Geo,
    /// A LanguageProperty (P4): one entry of the `languageMap`.
    Language,
    /// A VocabProperty (P5).
    Vocab,
    /// A JsonProperty or a value no range types (P7, `Edm.Untyped`).
    Json,
    /// A Relationship to a type the model does not hold (R4, R5): the
    /// `object` URI as a string.
    Url {
        /// A multivalued Relationship.
        collection: bool,
    },
    /// A Relationship to a visible type (R1 to R3).
    Nav {
        /// The EntitySet of the target type.
        target: String,
        /// A multivalued Relationship or a ListRelationship.
        collection: bool,
    },
}

impl Shape {
    /// Whether `$filter` may name it (C11).
    #[must_use]
    pub fn filterable(&self) -> bool {
        matches!(
            self,
            Self::Value { .. } | Self::Vocab | Self::Language | Self::Url { .. }
        )
    }

    /// Whether this round's `$filter` and `$orderby` take it: a
    /// single-valued value, the one form a `q` term compares today.
    #[must_use]
    pub fn scalar(&self) -> bool {
        matches!(
            self,
            Self::Value {
                collection: false,
                ..
            }
        )
    }

    /// Whether a write may name it: not a Relationship to a type the view
    /// hides (W15, it would link to an Entity the caller cannot see), not
    /// one entry of a `languageMap` (writing it would drop the others).
    #[must_use]
    pub fn writable(&self) -> bool {
        !matches!(self, Self::Url { .. } | Self::Language)
    }

    /// Whether `$orderby` may name it (C12).
    #[must_use]
    pub fn sortable(&self) -> bool {
        matches!(
            self,
            Self::Value {
                collection: false,
                ..
            } | Self::Vocab
                | Self::Url { collection: false }
        )
    }
}

/// One member of an EntityType: its NGSI-LD Attribute and its shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// The Attribute's expanded IRI.
    pub iri: String,
    /// How it maps.
    pub shape: Shape,
    /// The slot is a `ListProperty` or a `ListRelationship`: a write
    /// keeps the kind (`valueList`, `objectList`).
    pub list: bool,
    /// The slot is a `JsonProperty`: a [`Shape::Json`] write keeps `json`. Any other
    /// [`Shape::Json`] (an inlined object, P2) is a `Property` (T-5206 finding 4).
    pub json: bool,
    /// A `Property` whose range is `linkml:Any` (`range: Any`): `$filter` compares it as
    /// `Edm.Untyped` with a null, boolean, number or string literal; a value of another JSON
    /// type matches only `ne` (T-5333, NGSI-LD 4.9). An inlined class of the view (a complex
    /// value) is never compared.
    pub untyped: bool,
    /// When a client may write it (T-3722).
    pub written: Written,
}

/// When a client may write a member: `Core.Computed` never,
/// `Core.Immutable` on create only (T-3722).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Written {
    /// On create and on update.
    #[default]
    Always,
    /// On create only (`odata_immutable`).
    OnCreate,
    /// Never (`odata_computed`).
    Never,
}

/// One EntitySet: its Entity type and the members the view shows of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Set {
    /// The Entity type's expanded IRI.
    pub type_iri: String,
    /// The members by their CSDL name.
    pub members: BTreeMap<String, Member>,
    /// The class's `identifier` slot, when the view shows it (E2a); the key
    /// is `id` without one (E2).
    pub identifier: Option<Identifier>,
    /// The writes the set takes: the view's, narrowed by the class (C10a).
    pub writable: Writable,
    /// Each Property that keys the target of a navigation (R6, T-3761), by
    /// its CSDL name: the target's EntitySet.
    pub references: BTreeMap<String, String>,
    /// Each navigation with such a Property, by CSDL name: the Property a bind
    /// of it sets to the target's key (CSDL XML 8.5, T-5906).
    pub bound: BTreeMap<String, String>,
    /// The filter of the named output this set serves (docs API/11 EP-159, T-5225), a conjunct
    /// of every read of it; `None` for a class's own set.
    pub output: Option<antares_ops::outputs::Filter>,
}

/// A key taken from the model (E2a): its value `v` names the Entity
/// `{prefix}{v}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identifier {
    /// The key member's CSDL name.
    pub member: String,
    /// Its Edm type: `Edm.String`, `Edm.Int64` or `Edm.Guid`.
    pub edm: &'static str,
    /// `urn:ngsi-ld:{Class}:`.
    pub prefix: String,
}

/// The view's model: the CSDL document and, for each EntitySet, how its
/// names map back to NGSI-LD.
#[derive(Debug, Clone, PartialEq)]
pub struct Edm {
    /// The CSDL model `$metadata` writes and requests resolve against.
    pub csdl: Model,
    /// The view's namespace.
    pub namespace: String,
    /// The EntitySets by name.
    pub sets: BTreeMap<String, Set>,
    /// `Core.SchemaVersion` of the view's schema: the LinkML schema's `version` (`0` without
    /// one) then `.m` and [`MAPPING_VERSION`], `1.2.0.m1` (a character outside the URL's unreserved
    /// set becomes `_`); `$schemaversion` takes it.
    pub schema_version: String,
    /// The `{Set}_history` sets by name (T-5908), apart from `sets` so no read, write, filter
    /// hop or `$expand` of the current state reaches them.
    pub histories: BTreeMap<String, Set>,
}

/// What a history set's name adds to its class set's: `-` is no `SimpleIdentifier` character
/// (CSDL XML 17.2), so the OGC `{type}-history` is `{Set}_history` here.
pub const HISTORY_SUFFIX: &str = "_history";

/// The IRI of a history row's `observedAt` member; no Entity has it.
pub const OBSERVED_AT_IRI: &str = "urn:antares:odata:history:observedAt";

/// Builds the model of view `view` from `schema` and what the view shows.
/// A class the view does not show, a slot it does not show of that class,
/// and a name with no CSDL form are left out; a Relationship whose target
/// is not visible becomes a URL string (R5).
///
/// # Errors
/// When the view id has no CSDL namespace form.
pub fn build(
    view: &str,
    schema: &linkml_model::Model,
    shown: &Shown,
    writable: Writable,
    version: CsdlVersion,
) -> Result<Edm, String> {
    let namespace = mangle(view).ok_or("the view id has no OData namespace form")?;
    let mut visible: BTreeMap<&str, (String, &BTreeSet<String>)> = BTreeMap::new();
    for class in schema.schema().classes.keys() {
        let (Some(attrs), Some(name)) = (shown.get(&schema.class_iri(class)), mangle(class)) else {
            continue;
        };
        visible.insert(class.as_str(), (name, attrs));
    }
    let mut types = Vec::new();
    let mut sets = BTreeMap::new();
    let mut container = EntityContainer::new("Container");
    for (class, (name, attrs)) in &visible {
        let mut ty = EntityType::new(name.clone());
        ty.base_type = Some(BASE.into());
        let (mut identifier, mut required) = (None, Vec::new());
        ty.annotations.push(iri(&schema.class_iri(class)));
        let own = schema.schema().classes.get(*class);
        ty.annotations
            .extend(own.and_then(|c| c.description.as_deref()).map(description));
        let writable = narrowed(writable, own);
        let mut set = EntitySet::new(name.clone(), format!("{namespace}.{name}"));
        let mut members = BTreeMap::new();
        for (slot_name, slot) in schema.class_slots(class) {
            let slot_iri = schema.slot_iri(slot_name);
            let Some(member) = attrs
                .contains(&slot_iri)
                .then(|| member_name(slot_name, &slot))
                .flatten()
            else {
                continue;
            };
            let shape = shape(schema, &slot, &visible);
            // CSDL XML 3.4: a 4.0 client gets no Untyped property and an open type; the
            // member is served, selected and filtered as a dynamic property (T-5206 finding 5).
            let dynamic = version == CsdlVersion::V4_0 && shape == Shape::Json;
            ty.open |= dynamic;
            if let (Some(true), Shape::Value { edm, .. }) = (slot.identifier, &shape) {
                identifier = Some(keyed(&mut ty, class, &member, edm));
            }
            match &shape {
                Shape::Nav { target, collection } => {
                    let ordered = schema.kind(&slot) == "ListRelationship";
                    let target_ty = format!("{namespace}.{target}");
                    ty.navigation_properties.push(navigation(
                        &member,
                        &slot_iri,
                        target_ty,
                        *collection,
                        ordered,
                    ));
                    set.bindings.push(NavigationPropertyBinding {
                        path: member.clone(),
                        target: target.clone(),
                    });
                }
                _ if dynamic => {}
                other => {
                    let p = property(&member, &slot_iri, other, schema, &slot);
                    ty.properties
                        .push(key_property(p, ty.key.contains(&member), other));
                    let named = slot.required == Some(true);
                    required.extend(named.then(|| Expr::PropertyPath(member.clone())));
                }
            }
            members.insert(member, member_of(schema, &slot, slot_iri, shape));
        }
        set.annotations = capabilities(&members, writable, required);
        container.entity_sets.push(set);
        let (references, bound) = constrain(schema, class, &visible, &mut ty, &members);
        let set = Set {
            type_iri: schema.class_iri(class),
            references,
            bound,
            members,
            identifier,
            writable,
            output: None,
        };
        types.push(ty);
        sets.insert(name.clone(), set);
    }
    container.annotations = container_capabilities();
    let (mut own, schema_version) = own_schema(namespace.clone(), schema);
    own.entity_types = types;
    own.entity_container = Some(container);
    Ok(Edm {
        csdl: Model {
            version,
            references: references(),
            schemas: vec![vocabulary(), own],
        },
        namespace,
        sets,
        schema_version,
        histories: BTreeMap::new(),
    })
}

/// `edm` with a read-only `{Set}_history` beside each class's set (T-5908), for a view that
/// offers the Temporal Query: one row per Entity and `observedAt`, keyed by both, its members
/// the class's (a navigation as its object URIs, R5) holding the values valid at that instant.
/// A class whose set would collide with a name the model holds, or whose members name `id` or
/// `observedAt`, gets none.
#[must_use]
pub fn histories(mut edm: Edm) -> Edm {
    let Some(own) = edm
        .csdl
        .schemas
        .iter_mut()
        .find(|s| s.entity_container.is_some())
    else {
        return edm;
    };
    let taken: BTreeSet<String> = own
        .entity_types
        .iter()
        .map(|t| t.name.clone())
        .chain(edm.sets.keys().cloned())
        .collect();
    let mut types = Vec::new();
    let mut sets = Vec::new();
    for (name, set) in edm.sets.iter().filter(|(_, s)| s.output.is_none()) {
        let history = format!("{name}{HISTORY_SUFFIX}");
        // a row's key takes `id` and `observedAt`; a set showing no member has no history
        let plain = set
            .members
            .keys()
            .filter(|k| !matches!(k.as_str(), "id" | "observedAt"))
            .count();
        let serves = plain == set.members.len() && plain > 0;
        let class = own.entity_types.iter().find(|t| t.name == *name);
        let (Some(class), true, false) = (class, serves, taken.contains(&history)) else {
            continue;
        };
        let (ty, members) = history_type(&history, name, class, set);
        let mut entity_set =
            EntitySet::new(history.clone(), format!("{}.{history}", edm.namespace));
        entity_set.annotations = history_capabilities(&members);
        types.push(ty);
        sets.push(entity_set);
        let set = Set {
            members,
            identifier: None,
            writable: Writable::default(),
            references: BTreeMap::new(),
            bound: BTreeMap::new(),
            output: None,
            type_iri: set.type_iri.clone(),
        };
        edm.histories.insert(history, set);
    }
    own.entity_types.extend(types);
    own.entity_container
        .iter_mut()
        .for_each(|c| c.entity_sets.append(&mut sets));
    edm
}

/// The EntityType of `class`'s history set `history` and its members: the key, then the
/// class's properties nullable, each navigation as its object URIs (R5).
fn history_type(
    history: &str,
    name: &str,
    class: &EntityType,
    set: &Set,
) -> (EntityType, BTreeMap<String, Member>) {
    let mut members: BTreeMap<String, Member> = set
        .members
        .iter()
        .map(|(n, m)| {
            let shape = match &m.shape {
                Shape::Nav { collection, .. } => Shape::Url {
                    collection: *collection,
                },
                other => other.clone(),
            };
            (
                n.clone(),
                Member {
                    shape,
                    written: Written::Never,
                    ..m.clone()
                },
            )
        })
        .collect();
    let mut ty = EntityType::new(history.to_owned());
    // a 4.0 model serves an Untyped member as a dynamic property, as the class does
    ty.open = class.open;
    ty.key = vec!["id".into(), "observedAt".into()];
    ty.annotations.push(iri(&set.type_iri));
    ty.annotations.push(description(&format!(
            "The stored history of {name}: one row per Entity and observedAt, its values those valid at that instant"
        )));
    ty.properties
        .push(Property::new("id", TypeRef::single("Edm.String")).required());
    ty.properties
        .push(Property::new("observedAt", TypeRef::single("Edm.DateTimeOffset")).required());
    ty.properties
        .extend(class.properties.iter().cloned().map(|mut p| {
            p.nullable = true;
            p
        }));
    let urls = class.navigation_properties.iter().filter_map(|nav| {
        let m = members.get(&nav.name)?;
        let many = m.shape == Shape::Url { collection: true };
        let ty_ref = if many {
            TypeRef::collection("Edm.String")
        } else {
            TypeRef::single("Edm.String")
        };
        let mut p = Property::new(nav.name.clone(), ty_ref);
        p.annotations.push(iri(&m.iri));
        Some(p)
    });
    ty.properties.extend(urls.collect::<Vec<_>>());
    members.insert(
        "observedAt".into(),
        Member {
            iri: OBSERVED_AT_IRI.into(),
            shape: Shape::Value {
                edm: "Edm.DateTimeOffset",
                collection: false,
            },
            list: false,
            json: false,
            untyped: false,
            written: Written::Never,
        },
    );
    (ty, members)
}

/// The Capabilities of a history set (T-5908): read only, countable, filtered and sorted by
/// its single values, no `$search` and no `$expand`.
fn history_capabilities(members: &BTreeMap<String, Member>) -> Vec<Annotation> {
    let record = |term: &str, members: Vec<(&str, Expr)>| {
        Annotation::new(
            format!("Capabilities.{term}"),
            Expr::Record(members.into_iter().map(|(m, v)| (m.into(), v)).collect()),
        )
    };
    let not_scalar = || {
        Expr::Collection(
            members
                .iter()
                .filter(|(_, m)| !m.shape.scalar())
                .map(|(name, _)| Expr::PropertyPath(name.clone()))
                .collect(),
        )
    };
    vec![
        record(
            "InsertRestrictions",
            vec![("Insertable", Expr::Bool(false))],
        ),
        record("UpdateRestrictions", vec![("Updatable", Expr::Bool(false))]),
        record("DeleteRestrictions", vec![("Deletable", Expr::Bool(false))]),
        record("CountRestrictions", vec![("Countable", Expr::Bool(true))]),
        record(
            "SearchRestrictions",
            vec![("Searchable", Expr::Bool(false))],
        ),
        record(
            "FilterRestrictions",
            vec![
                ("Filterable", Expr::Bool(true)),
                ("NonFilterableProperties", not_scalar()),
            ],
        ),
        record(
            "SortRestrictions",
            vec![
                ("Sortable", Expr::Bool(true)),
                ("NonSortableProperties", not_scalar()),
            ],
        ),
        record(
            "ExpandRestrictions",
            vec![("Expandable", Expr::Bool(false))],
        ),
    ]
}

/// `edm` with one EntitySet per output of the view's `representations.odata` (docs API/11
/// EP-159, T-5225): the set of its one source's class under the output's name (`-` written `_`),
/// read only, its filter a conjunct of every read. An output whose class the model does not
/// show, or whose name a class's set holds, serves nobody.
#[must_use]
pub fn outputs(mut edm: Edm, outputs: &[antares_ops::outputs::Output]) -> Edm {
    let Some(container) = edm
        .csdl
        .schemas
        .iter_mut()
        .find_map(|s| s.entity_container.as_mut())
    else {
        return edm;
    };
    for o in outputs {
        let name = mangle(&o.name.replace('-', "_"));
        let class = o.sources.first().and_then(|s| mangle(&s.entity_type));
        let (Some(name), Some(class)) = (name, class) else {
            continue;
        };
        let own = container.entity_sets.iter().find(|s| s.name == class);
        let (Some(own), Some(set), false) =
            (own, edm.sets.get(&class), edm.sets.contains_key(&name))
        else {
            continue;
        };
        let mut entity_set = own.clone();
        entity_set.name.clone_from(&name);
        entity_set.annotations = capabilities(&set.members, Writable::default(), Vec::new());
        entity_set.annotations.push(description(
            o.description_in("en").unwrap_or(o.title_in("en")),
        ));
        let set = Set {
            writable: Writable::default(),
            output: Some(o.filter.clone()),
            ..set.clone()
        };
        container.entity_sets.push(entity_set);
        edm.sets.insert(name, set);
    }
    edm
}

/// The view's own CSDL schema, annotated with its `Core.SchemaVersion` (T-5207), and that
/// version: the LinkML `version` (`0` without one), `.m` and [`MAPPING_VERSION`];
/// `$schemaversion` takes unreserved characters alone (OData URL ABNF), so any other becomes `_`.
fn own_schema(namespace: String, schema: &linkml_model::Model) -> (Schema, String) {
    let version: String = format!(
        "{}.m{MAPPING_VERSION}",
        schema.schema().version.as_deref().unwrap_or("0")
    )
    .chars()
    .map(|c| {
        if c.is_ascii_alphanumeric() || "-._~".contains(c) {
            c
        } else {
            '_'
        }
    })
    .collect();
    let mut own = Schema::new(namespace);
    own.annotations.push(Annotation::new(
        "Core.SchemaVersion",
        Expr::String(version.clone()),
    ));
    (own, version)
}

/// Row R6: each single-valued navigation of `class` whose slot names a
/// Property with `odata_referential_constraint` gets a `ReferentialConstraint`
/// to the target's key, when the view shows all three, the Property is
/// not required and has the key's Edm type (CSDL XML 8.5). The Properties
/// that hold, by CSDL name, with the target's EntitySet, and each such
/// navigation with its Property.
fn constrain(
    schema: &linkml_model::Model,
    class: &str,
    visible: &BTreeMap<&str, (String, &BTreeSet<String>)>,
    ty: &mut EntityType,
    members: &BTreeMap<String, Member>,
) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
    let slots = schema.class_slots(class);
    let mut references = BTreeMap::new();
    let mut bound = BTreeMap::new();
    for (name, slot) in &slots {
        let (Some(dependent), Range::Class(target)) =
            (slot.annotations.get(REFERENCE_TAG), schema.range(slot))
        else {
            continue;
        };
        // A navigation the view shows: its target is visible (R1).
        let Some(nav) = mangle(name).and_then(|n| {
            ty.navigation_properties
                .iter_mut()
                .find(|p| p.name == n && !p.ty.collection)
        }) else {
            continue;
        };
        let Some((set, key, key_edm)) = key_of(schema, target, visible) else {
            continue;
        };
        let value = Shape::Value {
            edm: key_edm,
            collection: false,
        };
        let Some(property) = slots
            .iter()
            .find(|(n, s)| *n == dependent.value && s.required != Some(true))
            .and_then(|(n, _)| mangle(n))
            .filter(|m| members.get(m).is_some_and(|m| m.shape == value))
        else {
            continue;
        };
        nav.referential_constraints
            .push(ReferentialConstraint::new(property.clone(), key));
        bound.insert(nav.name.clone(), property.clone());
        references.insert(property, set);
    }
    (references, bound)
}

/// The EntitySet of a visible class, its key's CSDL name and Edm type: the
/// E2a key when the view shows it, `id` for a class without one (E2).
fn key_of(
    schema: &linkml_model::Model,
    class: &str,
    visible: &BTreeMap<&str, (String, &BTreeSet<String>)>,
) -> Option<(String, String, &'static str)> {
    let (set, attrs) = visible.get(class)?;
    // As `build` keys a type (E2a): an identifier slot of a plain value.
    let key = schema
        .class_slots(class)
        .into_iter()
        .find_map(
            |(name, slot)| match (slot.identifier, shape(schema, &slot, visible)) {
                (Some(true), Shape::Value { edm, .. }) => Some((name, edm)),
                _ => None,
            },
        );
    let Some((name, edm)) = key else {
        return Some((set.clone(), "id".to_owned(), "Edm.String"));
    };
    if !attrs.contains(&schema.slot_iri(name)) {
        return None;
    }
    Some((set.clone(), mangle(name)?, edm))
}

/// A key member is required, and one a create may leave out is
/// `Core.ComputedDefaultValue`: the surface mints a uuid for a `Guid` or
/// `String` key (W1).
fn key_property(p: Property, key: bool, shape: &Shape) -> Property {
    if !key {
        return p;
    }
    let mut p = p.required();
    if matches!(
        shape,
        Shape::Value {
            edm: "Edm.Guid" | "Edm.String",
            ..
        }
    ) {
        p.annotations.push(core("ComputedDefaultValue"));
    }
    p
}

/// The member a shown slot is, `None` for none. T-5206 finding 10: a slot named like a base
/// member never declares it twice; an identifier slot `id` is the NGSI-LD `id` itself, any
/// other is served escaped ([`unreserved`]).
fn member_name(slot_name: &str, slot: &linkml_model::Slot) -> Option<String> {
    let member = mangle(slot_name)?;
    if slot.identifier == Some(true) && member == "id" {
        return None;
    }
    unreserved(&member)
}

/// The members of `NgsiLd.Entity` a slot cannot be named as.
const RESERVED: [&str; 5] = ["id", "_types", "scope", "createdAt", "modifiedAt"];

/// `member`, or for a base member's name that name with its first character escaped
/// (`_x0073_cope`, CSDL names.rs form); `None` when even that does not fit 128 characters.
fn unreserved(member: &str) -> Option<String> {
    if !RESERVED.contains(&member) {
        return Some(member.to_owned());
    }
    let mut chars = member.chars();
    let first = chars.next()?;
    let escaped = format!("_x{:04X}_{}", u32::from(first), chars.as_str());
    (escaped.len() <= 128).then_some(escaped)
}

/// E2a: `ty` declares the base members itself and is keyed by `member`.
fn keyed(ty: &mut EntityType, class: &str, member: &str, edm: &'static str) -> Identifier {
    ty.base_type = None;
    ty.key = vec![member.to_owned()];
    ty.properties.splice(0..0, base_properties());
    Identifier {
        member: member.to_owned(),
        edm,
        prefix: format!("urn:ngsi-ld:{class}:"),
    }
}

/// Rows R1 to R4: the navigation property of a Relationship slot.
fn navigation(
    member: &str,
    slot_iri: &str,
    target_ty: String,
    collection: bool,
    ordered: bool,
) -> NavigationProperty {
    let ty = if collection {
        TypeRef::collection(target_ty)
    } else {
        TypeRef::single(target_ty)
    };
    let mut nav = NavigationProperty::new(member, ty);
    // CSDL XML 8.1.4: a collection-valued navigation is never null.
    nav.nullable = !collection;
    nav.annotations.push(iri(slot_iri));
    if ordered {
        nav.annotations.push(core("Ordered"));
    }
    nav
}

/// Section 3.4 and rows P1 to P7, R1 to R5.
fn shape(
    schema: &linkml_model::Model,
    slot: &linkml_model::Slot,
    visible: &BTreeMap<&str, (String, &BTreeSet<String>)>,
) -> Shape {
    let many = slot.multivalued == Some(true);
    match (schema.kind(slot), schema.range(slot)) {
        // A kind this mapping does not know (a later NGSI-LD version's) is an untyped,
        // unfiltered, read-only value of that slot alone (T-5207, mapping-contract rule 4).
        (kind, _) if !linkml_model::known_kind(kind) => Shape::Json,
        // P2: an inlined class is a JSON object value, whatever kind is named.
        (_, Range::Class(_)) if slot.inlined == Some(true) => Shape::Json,
        ("Relationship" | "ListRelationship", Range::Class(c)) => {
            let collection = many || schema.kind(slot) == "ListRelationship";
            match visible.get(c) {
                Some((target, _)) => Shape::Nav {
                    target: target.clone(),
                    collection,
                },
                None => Shape::Url { collection },
            }
        }
        ("Relationship" | "ListRelationship", _) => Shape::Url {
            collection: many || schema.kind(slot) == "ListRelationship",
        },
        ("GeoProperty", _) => Shape::Geo,
        ("LanguageProperty", _) => Shape::Language,
        ("VocabProperty", _) => Shape::Vocab,
        ("JsonProperty", _) | (_, Range::Class(_)) => Shape::Json,
        (kind, Range::Enum(_)) => Shape::Value {
            edm: "Edm.String",
            collection: many || kind == "ListProperty",
        },
        (kind, Range::Type(b)) => Shape::Value {
            // The model admits only the narrowings of `edm_types`.
            edm: slot
                .annotations
                .get(EDM_TYPE_TAG)
                .and_then(|a| edm_types(b).iter().find(|t| **t == a.value))
                .copied()
                .unwrap_or_else(|| edm_type(b)),
            collection: many || kind == "ListProperty",
        },
    }
}

/// Section 3.4.
fn edm_type(b: Builtin) -> &'static str {
    match b {
        Builtin::String | Builtin::Uri | Builtin::Uriorcurie => "Edm.String",
        Builtin::Integer => "Edm.Int64",
        Builtin::Float | Builtin::Double => "Edm.Double",
        Builtin::Decimal => "Edm.Decimal",
        Builtin::Boolean => "Edm.Boolean",
        Builtin::Date => "Edm.Date",
        Builtin::Datetime => "Edm.DateTimeOffset",
        Builtin::Time => "Edm.TimeOfDay",
    }
}

fn property(
    name: &str,
    slot_iri: &str,
    shape: &Shape,
    schema: &linkml_model::Model,
    slot: &linkml_model::Slot,
) -> Property {
    let (ty, collection) = match shape {
        Shape::Value { edm, collection } => (*edm, *collection),
        Shape::Geo => ("Edm.Geography", false),
        Shape::Language | Shape::Vocab => ("Edm.String", false),
        Shape::Url { collection } => ("Edm.String", *collection),
        Shape::Json | Shape::Nav { .. } => ("Edm.Untyped", false),
    };
    let mut p = Property::new(
        name,
        if collection {
            TypeRef::collection(ty)
        } else {
            TypeRef::single(ty)
        },
    );
    if ty == "Edm.Decimal" {
        p.scale = Some(Scale::Variable);
    }
    p.annotations.push(iri(slot_iri));
    p.annotations.push(Annotation::new(
        format!("{VOCABULARY_ALIAS}.Kind"),
        Expr::String(schema.kind(slot).to_owned()),
    ));
    let range = schema.range(slot);
    if matches!(shape, Shape::Url { .. })
        || matches!(range, Range::Type(Builtin::Uri | Builtin::Uriorcurie))
    {
        p.annotations.push(core("IsURL"));
    }
    if *shape == Shape::Language {
        p.annotations.push(core("IsLanguageDependent"));
    }
    if schema.kind(slot) == "ListProperty" {
        p.annotations.push(core("Ordered"));
    }
    match written(slot) {
        Written::Never => p.annotations.push(core("Computed")),
        Written::OnCreate => p.annotations.push(core("Immutable")),
        Written::Always => {}
    }
    p.annotations
        .extend(slot.description.as_deref().map(description));
    if let Range::Enum(e) = range {
        // `range` names an enum only when the schema holds it.
        let values = schema.schema().enums.get(e).into_iter();
        p.annotations.push(Annotation::new(
            "Validation.AllowedValues",
            Expr::Collection(
                values
                    .flat_map(|v| v.permissible_values.keys())
                    .map(|v| Expr::Record(vec![("Value".into(), Expr::String(v.clone()))]))
                    .collect(),
            ),
        ));
    }
    p
}

fn iri(value: &str) -> Annotation {
    Annotation::new(
        format!("{VOCABULARY_ALIAS}.Iri"),
        Expr::String(value.to_owned()),
    )
}

fn core(term: &str) -> Annotation {
    Annotation::new(format!("Core.{term}"), Expr::Bool(true))
}

/// The view's writes narrowed by the class's `odata_insertable`,
/// `odata_updatable` and `odata_deletable` (C10a): `"false"` takes one away,
/// nothing adds one.
fn narrowed(writable: Writable, class: Option<&linkml_model::Class>) -> Writable {
    let flag = |tag: &str| {
        class
            .and_then(|c| c.annotations.get(tag))
            .is_none_or(|a| a.value != "false")
    };
    Writable {
        insert: writable.insert && flag("odata_insertable"),
        update: writable.update && flag("odata_updatable"),
        delete: writable.delete && flag("odata_deletable"),
    }
}

/// `Core.Description` of a slot or a class (T-3722).
fn description(text: &str) -> Annotation {
    Annotation::new("Core.Description", Expr::String(text.to_owned()))
}

/// The class IRI of LinkML's `Any`, a value of no stated structure.
const LINKML_ANY: &str = "https://w3id.org/linkml/Any";

/// The [`Member`] a slot of `shape` maps to.
fn member_of(
    schema: &linkml_model::Model,
    slot: &linkml_model::Slot,
    iri: String,
    shape: Shape,
) -> Member {
    let kind = schema.kind(slot);
    Member {
        iri,
        list: kind.starts_with("List"),
        json: kind == "JsonProperty",
        untyped: kind == "Property"
            && shape == Shape::Json
            && matches!(schema.range(slot), Range::Class(c) if schema.class_iri(c) == LINKML_ANY),
        shape,
        written: written(slot),
    }
}

/// When a client may write the slot's member (T-3722); the model admits
/// only `"true"` and `"false"` as the flags' values. A slot of a kind the
/// mapping does not know is never written (T-5207).
fn written(slot: &linkml_model::Slot) -> Written {
    let set = |tag: &str| slot.annotations.get(tag).is_some_and(|a| a.value == "true");
    let unknown = slot
        .annotations
        .get(linkml_model::KIND_TAG)
        .is_some_and(|a| !linkml_model::known_kind(&a.value));
    if unknown || set("odata_computed") {
        Written::Never
    } else if set("odata_immutable") {
        Written::OnCreate
    } else {
        Written::Always
    }
}

/// C2 to C16: the writes the view offers, none of them deep or an upsert,
/// with the members no write may name ([`Shape::writable`], the computed
/// ones are `Core.Computed`); counting; filtering and sorting on the
/// members [`Shape::scalar`] admits and `id` (C11, C12), sorting also on
/// `createdAt` and `modifiedAt`; expanding one
/// level (C13). Advisory: every request is still gated (O25).
fn capabilities(
    members: &BTreeMap<String, Member>,
    writable: Writable,
    required: Vec<Expr>,
) -> Vec<Annotation> {
    let flag = |value: bool| Expr::Bool(value);
    let restriction = |term: &str, record: Vec<(&str, Expr)>| {
        Annotation::new(
            format!("Capabilities.{term}"),
            Expr::Record(record.into_iter().map(|(m, v)| (m.into(), v)).collect()),
        )
    };
    let unserved: Vec<Expr> = ["_types", "scope", "createdAt", "modifiedAt"]
        .into_iter()
        .map(str::to_owned)
        .chain(
            members
                .iter()
                .filter(|(_, m)| !m.shape.scalar() && !matches!(m.shape, Shape::Nav { .. }))
                .map(|(name, _)| name.clone()),
        )
        .map(Expr::PropertyPath)
        .collect();
    let refused = |create: bool| -> Vec<Expr> {
        members
            .iter()
            .filter(|(_, m)| {
                !m.shape.writable()
                    || m.written == Written::Never
                    || (!create && m.written == Written::OnCreate)
            })
            .map(|(name, _)| Expr::PropertyPath(name.clone()))
            .collect()
    };
    vec![
        restriction(
            "InsertRestrictions",
            vec![
                ("Insertable", flag(writable.insert)),
                ("NonInsertableProperties", Expr::Collection(refused(true))),
                // C5: a create names them (LinkML `required`)
                ("RequiredProperties", Expr::Collection(required)),
            ],
        ),
        restriction(
            "UpdateRestrictions",
            vec![
                ("Updatable", flag(writable.update)),
                ("Upsertable", flag(false)),
                ("DeltaUpdateSupported", flag(false)),
                ("NonUpdatableProperties", Expr::Collection(refused(false))),
            ],
        ),
        restriction(
            "DeleteRestrictions",
            vec![("Deletable", flag(writable.delete))],
        ),
        restriction("CountRestrictions", vec![("Countable", flag(true))]),
        restriction("SearchRestrictions", vec![("Searchable", flag(true))]),
        restriction(
            "FilterRestrictions",
            vec![
                ("Filterable", flag(true)),
                ("MaxLevels", Expr::Int(1)),
                (
                    "NonFilterableProperties",
                    Expr::Collection(
                        unserved
                            .iter()
                            .filter(|p| {
                                !matches!(p, Expr::PropertyPath(n) if members.get(n).is_some_and(|m| m.untyped))
                            })
                            .cloned()
                            .collect(),
                    ),
                ),
            ],
        ),
        restriction(
            "SortRestrictions",
            vec![
                ("Sortable", flag(true)),
                // `createdAt` and `modifiedAt` are sorted by the surface (T-3758)
                (
                    "NonSortableProperties",
                    Expr::Collection(
                        unserved
                            .into_iter()
                            .filter(|p| {
                                !matches!(p, Expr::PropertyPath(n) if n == "createdAt" || n == "modifiedAt")
                            })
                            .collect(),
                    ),
                ),
            ],
        ),
        restriction(
            "ExpandRestrictions",
            vec![("Expandable", flag(true)), ("MaxLevels", Expr::Int(1))],
        ),
        restriction("DeepInsertSupport", vec![("Supported", flag(false))]),
        restriction("DeepUpdateSupport", vec![("Supported", flag(false))]),
    ]
    .into_iter()
    .chain(paging(members))
    .collect()
}

/// A key, `$top` and `$skip` on the set; below one Entity, inside
/// `$expand(...)` too, only `$select` applies (Q31 to Q33, ADR-0024
/// decision 5): the rest of a navigation's options are 501.
fn paging(members: &BTreeMap<String, Member>) -> [Annotation; 4] {
    let restricted = members
        .iter()
        .filter(|(_, m)| matches!(m.shape, Shape::Nav { .. }))
        .map(|(name, _)| nested_only_select(name))
        .collect();
    [
        Annotation::new(
            "Capabilities.NavigationRestrictions",
            Expr::Record(vec![(
                "RestrictedProperties".into(),
                Expr::Collection(restricted),
            )]),
        ),
        Annotation::new("Capabilities.IndexableByKey", Expr::Bool(true)),
        Annotation::new("Capabilities.TopSupported", Expr::Bool(true)),
        Annotation::new("Capabilities.SkipSupported", Expr::Bool(true)),
    ]
}

/// One `NavigationRestrictions` row: the navigation `name` takes `$select`
/// and no other option. No `CountRestrictions` member: the record type in
/// the vocabulary the .NET client ships has none and refuses `$metadata`.
fn nested_only_select(name: &str) -> Expr {
    let off = |member: &str| Expr::Record(vec![(member.into(), Expr::Bool(false))]);
    Expr::Record(vec![
        (
            "NavigationProperty".into(),
            Expr::NavigationPropertyPath(name.into()),
        ),
        ("FilterRestrictions".into(), off("Filterable")),
        ("SearchRestrictions".into(), off("Searchable")),
        ("SortRestrictions".into(), off("Sortable")),
        ("TopSupported".into(), Expr::Bool(false)),
        ("SkipSupported".into(), Expr::Bool(false)),
        ("IndexableByKey".into(), Expr::Bool(false)),
    ])
}

/// C18 to C20.
fn container_capabilities() -> Vec<Annotation> {
    let flag = |term: &str, value: bool| {
        Annotation::new(format!("Capabilities.{term}"), Expr::Bool(value))
    };
    vec![
        // the lowest level: the pinned suite proves none higher (docs/src/odata.md section 6)
        Annotation::new(
            "Capabilities.ConformanceLevel",
            Expr::EnumMember {
                ty: "Capabilities.ConformanceLevelType".into(),
                members: vec!["Minimal".into()],
            },
        ),
        // C18: the surface serves `$batch` (T-3723); Content-ID references are 501.
        flag("BatchSupported", true),
        Annotation::new(
            "Capabilities.BatchSupport",
            Expr::Record(vec![
                ("Supported".into(), Expr::Bool(true)),
                ("ContinueOnErrorSupported".into(), Expr::Bool(true)),
                (
                    "ReferencesInRequestBodiesSupported".into(),
                    Expr::Bool(false),
                ),
                (
                    "ReferencesAcrossChangeSetsSupported".into(),
                    Expr::Bool(false),
                ),
                (
                    "SupportedFormats".into(),
                    Expr::Collection(vec![
                        Expr::String("multipart/mixed".into()),
                        Expr::String("application/json".into()),
                    ]),
                ),
            ]),
        ),
        flag("QuerySegmentSupported", true),
        // the operators and functions a `$filter` translates, nothing else (T-5907)
        Annotation::new(
            "Capabilities.FilterFunctions",
            Expr::Collection(
                crate::functions::filter_functions()
                    .into_iter()
                    .map(|f| Expr::String(f.into()))
                    .collect(),
            ),
        ),
        flag("CrossJoinSupported", false),
        flag("ComputeSupported", true),
        // Q19: the `$apply` transformations served (T-3937, T-3985)
        Annotation::new(
            "Aggregation.ApplySupported",
            Expr::Record(vec![
                (
                    "Transformations".into(),
                    Expr::Collection(
                        [
                            "identity",
                            "filter",
                            "orderby",
                            "top",
                            "skip",
                            "aggregate",
                            "groupby",
                            "topcount",
                            "topsum",
                            "toppercent",
                            "bottomcount",
                            "bottomsum",
                            "bottompercent",
                            "concat",
                            "compute",
                            "search",
                        ]
                        .into_iter()
                        .map(|t| Expr::String(t.into()))
                        .collect(),
                    ),
                ),
                // No `Rollup`: `Aggregation.RollupType/None` is an enum member of a vocabulary
                // the clients do not load, and Microsoft.OData.Edm's validator refuses the whole
                // $metadata over it (BadUnresolvedEnumMember); `rollup` answers 501 (T-3937).
            ]),
        ),
        flag("KeyAsSegmentSupported", false),
        flag("AsynchronousRequestsSupported", false),
    ]
}

fn references() -> Vec<Reference> {
    ["Core", "Capabilities", "Validation", "Aggregation"]
        .into_iter()
        .map(|alias| Reference {
            uri: format!("{VOCABULARIES}Org.OData.{alias}.V1.xml"),
            includes: vec![Include {
                namespace: format!("Org.OData.{alias}.V1"),
                alias: Some(alias.into()),
            }],
        })
        .collect()
}

/// `Antares.NgsiLd`: the terms the surface annotates with and the abstract
/// base that holds the key (E2), the types (E3), the scope (E5) and the
/// system members (E6).
fn vocabulary() -> Schema {
    let mut v = Schema::new(VOCABULARY);
    v.alias = Some(VOCABULARY_ALIAS.into());
    v.annotations.push(Annotation::new(
        "Core.SchemaVersion",
        Expr::String(VOCABULARY_VERSION.into()),
    ));
    v.terms = vec![
        Term::new("Iri", TypeRef::single("Edm.String")),
        Term::new("Kind", TypeRef::single("Edm.String")),
        Term::new("observedAt", TypeRef::single("Edm.DateTimeOffset")),
        Term::new("datasetId", TypeRef::single("Edm.String")),
        // T-5206: the instance's UN/CEFACT unit code, and why a stored value reads null.
        Term::new("unitCode", TypeRef::single("Edm.String")),
        Term::new("Unserved", TypeRef::single("Edm.String")),
    ];
    let mut base = EntityType::new("Entity");
    base.is_abstract = true;
    base.key = vec!["id".into()];
    base.properties = base_properties();
    v.entity_types = vec![base];
    v
}

/// The members of `NgsiLd.Entity` (E2, E3, E5, E6).
fn base_properties() -> Vec<Property> {
    let computed = |p: Property| {
        let mut p = p.required();
        p.annotations.push(core("Computed"));
        p
    };
    // W1: a create that leaves out `id` gets one minted
    let mut id = Property::new("id", TypeRef::single("Edm.String")).required();
    id.annotations.push(core("ComputedDefaultValue"));
    vec![
        id,
        computed(Property::new("_types", TypeRef::collection("Edm.String"))),
        Property::new("scope", TypeRef::collection("Edm.String")).required(),
        computed(Property::new(
            "createdAt",
            TypeRef::single("Edm.DateTimeOffset"),
        )),
        computed(Property::new(
            "modifiedAt",
            TypeRef::single("Edm.DateTimeOffset"),
        )),
    ]
}

/// The members every EntityType inherits from the base, as the surface
/// reads them.
#[must_use]
pub fn base_member(name: &str) -> bool {
    matches!(name, "id" | "_types" | "scope" | "createdAt" | "modifiedAt")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> linkml_model::Model {
        let json = br#"{
          "id": "https://example.org/city", "name": "city",
          "prefixes": {"city": {"prefix_prefix": "city", "prefix_reference": "https://example.org/city/"}},
          "default_prefix": "city",
          "classes": {
            "Car": {"slots": ["speed", "plate", "owner", "secret", "parkedIn", "colour", "notes", "spot", "label", "tags", "price", "photo",
                              "drivers", "route", "maker", "trailer", "service", "ratings", "shades", "palette", "kind", "dims", "stops", "makers"]},
            "Person": {"slots": ["name"]},
            "Garage": {"slots": ["name"]}
          },
          "slots": {
            "speed": {"range": "float"},
            "plate": {},
            "owner": {"range": "Person"},
            "secret": {"range": "integer"},
            "parkedIn": {"range": "Garage", "multivalued": true},
            "colour": {"range": "Colour"},
            "notes": {"annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "JsonProperty"}}},
            "spot": {"annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "GeoProperty"}}},
            "label": {"annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "LanguageProperty"}}},
            "tags": {"annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "ListProperty"}}},
            "price": {"range": "decimal"},
            "photo": {"range": "uri"},
            "name": {},
            "drivers": {"range": "Person", "multivalued": true},
            "route": {"range": "Person", "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "ListRelationship"}}},
            "maker": {"annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "Relationship"}}},
            "trailer": {"range": "Person", "inlined": true, "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "Relationship"}}},
            "service": {"range": "Person", "inlined": true},
            "ratings": {"range": "integer", "multivalued": true},
            "shades": {"range": "Colour", "multivalued": true},
            "palette": {"range": "Colour", "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "ListProperty"}}},
            "kind": {"range": "Colour", "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "VocabProperty"}}},
            "dims": {"range": "boolean"},
            "stops": {"annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "ListRelationship"}}},
            "makers": {"multivalued": true, "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "Relationship"}}}
          },
          "enums": {"Colour": {"permissible_values": {"red": {}, "pm2.5": {}}}}
        }"#;
        let limits = linkml_model::Limits {
            max_bytes: 1 << 16,
            max_depth: 16,
            max_elements: 1024,
        };
        linkml_model::Model::read(json, &limits).unwrap()
    }

    fn shown(s: &linkml_model::Model, types: &[(&str, &[&str])]) -> Shown {
        types
            .iter()
            .map(|(t, attrs)| {
                (
                    s.class_iri(t),
                    attrs.iter().map(|a| s.slot_iri(a)).collect(),
                )
            })
            .collect()
    }

    const CAR_ALL: &[&str] = &[
        "speed", "plate", "owner", "parkedIn", "colour", "notes", "spot", "label", "tags", "price",
        "photo", "drivers", "route", "maker", "trailer", "service", "ratings", "shades", "palette",
        "kind", "dims", "stops", "makers",
    ];

    fn xml(edm: &Edm) -> String {
        let mut out = Vec::new();
        odata_csdl::write_xml(&edm.csdl, &odata_csdl::Limits::strict(), &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn a_reference_annotation_is_a_constraint_only_when_the_view_shows_it_whole() {
        let json = br#"{
          "id": "https://example.org/shop", "name": "shop",
          "classes": {
            "Desc": {"slots": ["productId", "product", "products", "ownerRef", "owner", "maker", "boughtId", "bought",
                               "lost", "flat", "missing"]},
            "Product": {"slots": ["ID", "name"]},
            "Person": {"slots": ["name"]}
          },
          "slots": {
            "ID": {"identifier": true, "annotations": {"edm_type": {"tag": "edm_type", "value": "Edm.Guid"}}},
            "name": {},
            "productId": {"annotations": {"edm_type": {"tag": "edm_type", "value": "Edm.Guid"}}},
            "product": {"range": "Product", "annotations": {"odata_referential_constraint": {"tag": "odata_referential_constraint", "value": "productId"}}},
            "products": {"range": "Product", "multivalued": true, "annotations": {"odata_referential_constraint": {"tag": "odata_referential_constraint", "value": "productId"}}},
            "ownerRef": {},
            "owner": {"range": "Person", "annotations": {"odata_referential_constraint": {"tag": "odata_referential_constraint", "value": "ownerRef"}}},
            "maker": {"range": "Product", "annotations": {"odata_referential_constraint": {"tag": "odata_referential_constraint", "value": "ownerRef"}}},
            "boughtId": {"required": true, "annotations": {"edm_type": {"tag": "edm_type", "value": "Edm.Guid"}}},
            "bought": {"range": "Product", "annotations": {"odata_referential_constraint": {"tag": "odata_referential_constraint", "value": "boughtId"}}},
            "lost": {"range": "Product", "annotations": {"odata_referential_constraint": {"tag": "odata_referential_constraint", "value": "nothing"}}},
            "flat": {"annotations": {"odata_referential_constraint": {"tag": "odata_referential_constraint", "value": "ownerRef"}}},
            "missing": {"range": "Person", "annotations": {"odata_referential_constraint": {"tag": "odata_referential_constraint", "value": "ownerRef"}}}
          }
        }"#;
        let limits = linkml_model::Limits {
            max_bytes: 1 << 16,
            max_depth: 16,
            max_elements: 1024,
        };
        let s = linkml_model::Model::read(json, &limits).unwrap();
        let every: Vec<&str> =
            "productId product products ownerRef owner maker boughtId bought lost flat"
                .split(' ')
                .collect();
        let constraints = |shown: &Shown| {
            let edm = build("shop", &s, shown, Writable::default(), CsdlVersion::V4_01).unwrap();
            let desc = &edm.sets["Desc"];
            let ty = edm.csdl.schemas[1]
                .entity_types
                .iter()
                .find(|t| t.name == "Desc")
                .unwrap();
            let navs: BTreeMap<String, Vec<ReferentialConstraint>> = ty
                .navigation_properties
                .iter()
                .filter(|n| !n.referential_constraints.is_empty())
                .map(|n| (n.name.clone(), n.referential_constraints.clone()))
                .collect();
            assert_eq!(
                desc.bound.keys().collect::<Vec<_>>(),
                navs.keys().collect::<Vec<_>>()
            );
            (navs, desc.references.clone())
        };
        let (navs, references) = constraints(&shown(
            &s,
            &[
                ("Desc", &every),
                ("Product", &["ID", "name"]),
                ("Person", &["name"]),
            ],
        ));
        // `products` is a collection, `maker` an Edm.String for a Guid key,
        // `bought` required, `lost` names no slot, `flat` no class, and the
        // view does not show `missing`.
        assert_eq!(
            navs,
            BTreeMap::from([
                (
                    "owner".to_owned(),
                    vec![ReferentialConstraint::new("ownerRef", "id")]
                ),
                (
                    "product".to_owned(),
                    vec![ReferentialConstraint::new("productId", "ID")]
                ),
            ])
        );
        assert_eq!(
            references,
            BTreeMap::from([
                ("ownerRef".to_owned(), "Person".to_owned()),
                ("productId".to_owned(), "Product".to_owned()),
            ])
        );
        // The key, the Property or the target hidden: no constraint.
        for (desc, product) in [
            (&every[..], &["name"][..]),
            (&["product", "owner"][..], &["ID", "name"][..]),
        ] {
            let (navs, references) = constraints(&shown(
                &s,
                &[("Desc", desc), ("Product", product), ("Person", &["name"])],
            ));
            assert_eq!(
                (navs.get("product"), references.get("productId")),
                (None, None),
                "{desc:?} {product:?}"
            );
        }
        assert!(constraints(&shown(&s, &[("Desc", &every)])).0.is_empty());
    }

    #[test]
    fn a_history_set_stands_beside_each_class_set_keyed_by_id_and_observed_at() {
        let s = schema();
        let shown = shown(
            &s,
            &[("Car", CAR_ALL), ("Person", &["name"]), ("Garage", &[])],
        );
        for version in [CsdlVersion::V4_01, CsdlVersion::V4_0] {
            let edm =
                histories(build("city-cars", &s, &shown, Writable::default(), version).unwrap());
            odata_csdl::validate(&edm.csdl, &odata_csdl::Limits::strict()).unwrap();
            let names: Vec<&str> = edm.histories.keys().map(String::as_str).collect();
            assert_eq!(
                names,
                ["Car_history", "Person_history"],
                "Garage shows no member"
            );
            let car = edm.histories.get("Car_history").unwrap();
            let shape = |name: &str| car.members.get(name).map(|m| m.shape.clone());
            assert_eq!(shape("drivers"), Some(Shape::Url { collection: true }));
            assert_eq!(shape("owner"), Some(Shape::Url { collection: false }));
            assert_eq!(car.writable, Writable::default());
            let ty = edm
                .csdl
                .schemas
                .iter()
                .flat_map(|s| &s.entity_types)
                .find(|t| t.name == "Car_history")
                .unwrap();
            assert_eq!(ty.key, ["id", "observedAt"]);
            assert_eq!(ty.navigation_properties.len(), 0);
            let drivers = ty.properties.iter().find(|p| p.name == "drivers").unwrap();
            assert_eq!(drivers.ty, TypeRef::collection("Edm.String"));
        }
        let built = || {
            build(
                "city-cars",
                &s,
                &shown,
                Writable::default(),
                CsdlVersion::V4_01,
            )
            .unwrap()
        };
        let mut reserved = built();
        let name = reserved
            .sets
            .get("Person")
            .unwrap()
            .members
            .get("name")
            .unwrap()
            .clone();
        reserved
            .sets
            .get_mut("Person")
            .unwrap()
            .members
            .insert("observedAt".into(), name);
        let mut taken = built();
        let person = taken.sets.get("Person").unwrap().clone();
        taken.sets.insert("Car_history".into(), person);
        for (edm, kept) in [(reserved, "Car_history"), (taken, "Person_history")] {
            let names: Vec<String> = histories(edm).histories.into_keys().collect();
            assert_eq!(
                names,
                [kept],
                "a reserved member or a taken name makes no history"
            );
        }
        let mut bare = built();
        bare.csdl.schemas.clear();
        assert!(
            histories(bare).histories.is_empty(),
            "no container, no history"
        );
    }

    #[test]
    fn the_model_is_valid_csdl_in_both_versions() {
        let s = schema();
        let shown = shown(&s, &[("Car", CAR_ALL), ("Person", &["name"])]);
        for version in [CsdlVersion::V4_01, CsdlVersion::V4_0] {
            let edm = build("city-cars", &s, &shown, Writable::default(), version).unwrap();
            odata_csdl::validate(&edm.csdl, &odata_csdl::Limits::strict()).unwrap();
            let mut json = Vec::new();
            if version == CsdlVersion::V4_01 {
                odata_csdl::write_json(&edm.csdl, &odata_csdl::Limits::strict(), &mut json)
                    .unwrap();
            }
        }
    }

    #[test]
    fn each_slot_maps_by_its_kind_and_range() {
        let s = schema();
        let edm = build(
            "v",
            &s,
            &shown(&s, &[("Car", CAR_ALL), ("Person", &["name"])]),
            Writable::default(),
            CsdlVersion::V4_01,
        )
        .unwrap();
        let car = &edm.sets["Car"].members;
        let shape = |n: &str| car[n].shape.clone();
        assert_eq!(
            shape("speed"),
            Shape::Value {
                edm: "Edm.Double",
                collection: false
            }
        );
        assert_eq!(
            shape("plate"),
            Shape::Value {
                edm: "Edm.String",
                collection: false
            }
        );
        assert_eq!(
            shape("owner"),
            Shape::Nav {
                target: "Person".into(),
                collection: false
            },
            "R1: a visible target"
        );
        assert_eq!(
            shape("parkedIn"),
            Shape::Url { collection: true },
            "R5: Garage is hidden"
        );
        assert_eq!(
            shape("colour"),
            Shape::Value {
                edm: "Edm.String",
                collection: false
            }
        );
        assert_eq!(shape("notes"), Shape::Json);
        assert_eq!(shape("spot"), Shape::Geo);
        assert_eq!(shape("label"), Shape::Language);
        assert_eq!(
            shape("tags"),
            Shape::Value {
                edm: "Edm.String",
                collection: true
            }
        );
        assert_eq!(car["speed"].iri, s.slot_iri("speed"));
        let collection = |target: &str| Shape::Nav {
            target: target.into(),
            collection: true,
        };
        assert_eq!(shape("drivers"), collection("Person"), "R2");
        assert_eq!(shape("route"), collection("Person"), "R3");
        assert_eq!(
            shape("maker"),
            Shape::Url { collection: false },
            "R4: no target class"
        );
        assert_eq!(
            shape("trailer"),
            Shape::Json,
            "an inlined Relationship is its value"
        );
        assert_eq!(shape("service"), Shape::Json, "P2: an inlined class");
        let value = |edm, collection| Shape::Value { edm, collection };
        assert_eq!(shape("ratings"), value("Edm.Int64", true));
        assert_eq!(shape("shades"), value("Edm.String", true));
        assert_eq!(shape("palette"), value("Edm.String", true));
        assert_eq!(shape("kind"), Shape::Vocab);
        assert_eq!(
            shape("makers"),
            Shape::Url { collection: true },
            "R4, multivalued"
        );
        assert_eq!(
            shape("stops"),
            Shape::Url { collection: true },
            "R4 of a list"
        );
        assert_eq!(shape("dims"), value("Edm.Boolean", false));
        let x = xml(&edm);
        assert!(
            x.contains(r#"<NavigationPropertyBinding Path="owner" Target="Person"/>"#),
            "{x}"
        );
        assert!(x.contains(r#"Scale="variable""#), "{x}");
        assert!(x.contains(r#"Property="Value" String="pm2.5""#), "{x}");
        assert!(x.contains(r#"Term="Core.IsURL""#), "{x}");
        assert!(x.contains(r#"Term="Core.IsLanguageDependent""#), "{x}");
        assert!(x.contains(r#"Term="Core.Ordered""#), "{x}");
    }

    #[test]
    fn a_hidden_type_or_attribute_leaves_no_trace() {
        let s = schema();
        let edm = build(
            "v",
            &s,
            &shown(&s, &[("Car", &["speed", "parkedIn"])]),
            Writable::default(),
            CsdlVersion::V4_01,
        )
        .unwrap();
        let x = xml(&edm);
        for hidden in [
            "secret", "Person", "Garage", "owner", "plate", "/secret", "/Garage",
        ] {
            assert!(!x.contains(hidden), "{hidden} in {x}");
        }
        assert_eq!(edm.sets.keys().collect::<Vec<_>>(), vec!["Car"]);
    }

    #[test]
    fn a_4_0_model_has_no_untyped_property_and_an_open_type_serving_it_dynamically() {
        let s = schema();
        let edm = build(
            "v",
            &s,
            &shown(&s, &[("Car", CAR_ALL)]),
            Writable::default(),
            CsdlVersion::V4_0,
        )
        .unwrap();
        assert!(
            edm.sets["Car"].members.contains_key("notes"),
            "a dynamic property (T-5206)"
        );
        assert!(!xml(&edm).contains("Edm.Untyped"));
        assert!(xml(&edm).contains(r#"OpenType="true""#));
    }

    /// T-5206 finding 10: slots named like the base members give valid CSDL; an identifier
    /// slot `id` is the NGSI-LD id, the others are served escaped.
    #[test]
    fn a_slot_named_like_a_base_member_is_escaped_never_declared_twice() {
        let s = linkml_model::Model::read(
            br#"{"id": "https://e.org/s/", "name": "s",
                "classes": {"Car": {"slots": ["id", "scope", "createdAt", "speed"]}},
                "slots": {"id": {"identifier": true, "range": "uriorcurie"}, "scope": {},
                          "createdAt": {"range": "datetime"}, "speed": {"range": "float"}}}"#,
            &linkml_model::Limits {
                max_bytes: 4096,
                max_depth: 8,
                max_elements: 99,
            },
        )
        .unwrap();
        let shown = s.slot_iris_by_class();
        let edm = build("v", &s, &shown, Writable::default(), CsdlVersion::V4_01).unwrap();
        let members: Vec<&String> = edm.sets["Car"].members.keys().collect();
        assert_eq!(members, ["_x0063_reatedAt", "_x0073_cope", "speed"]);
        assert!(
            edm.sets["Car"].identifier.is_none(),
            "keyed by the NGSI-LD id"
        );
        assert!(odata_csdl::validate(&edm.csdl, &odata_csdl::Limits::strict()).is_ok());
        assert!(xml(&edm).contains(r#"Name="_x0073_cope""#));
    }

    #[test]
    fn names_are_mangled_and_a_view_id_without_a_namespace_form_is_refused() {
        let s = schema();
        let edm = build(
            "city-cars",
            &s,
            &shown(&s, &[("Car", &["speed"])]),
            Writable::default(),
            CsdlVersion::V4_01,
        )
        .unwrap();
        assert_eq!(edm.namespace, "city_x002D_cars");
        assert!(build(
            &"v".repeat(200),
            &s,
            &Shown::new(),
            Writable::default(),
            CsdlVersion::V4_01
        )
        .is_err());
    }

    #[test]
    fn a_view_of_every_name_shows_every_class_with_every_slot() {
        let s = schema();
        let every = s.slot_iris_by_class();
        let car = &every[&s.class_iri("Car")];
        assert!(
            shown(&s, &[("Car", CAR_ALL)])[&s.class_iri("Car")].is_subset(car),
            "every slot of Car, inherited ones too"
        );
        assert!(every.contains_key(&s.class_iri("Person")));
        let edm = build("all", &s, &every, Writable::default(), CsdlVersion::V4_01).unwrap();
        let xml = xml(&edm);
        for set in ["Car", "Person"] {
            assert!(xml.contains(&format!("<EntitySet Name=\"{set}\"")), "{xml}");
        }
    }

    #[test]
    fn an_output_is_a_read_only_set_of_its_class_and_one_without_a_class_is_none() {
        let s = schema();
        let edm = build(
            "city",
            &s,
            &shown(&s, &[("Car", CAR_ALL)]),
            Writable::default(),
            CsdlVersion::V4_01,
        )
        .unwrap();
        let long = "a".repeat(129);
        let list = serde_json::json!([
            {"name": "fast-cars", "filter": {"q": "speed>2"}, "sources": [{"type": "Car"}]},
            {"name": "people", "sources": [{"type": "Person"}]},
            {"name": "long", "sources": [{"type": long}]}
        ]);
        let listed: Vec<_> = antares_ops::outputs::listed(Some(&list), "odata")
            .into_iter()
            .filter_map(|(_, o)| o.ok())
            .collect();
        assert_eq!(listed.len(), 3);
        let named = outputs(edm.clone(), &listed);
        assert_eq!(
            named.sets.keys().collect::<Vec<_>>(),
            vec!["Car", "fast_cars"]
        );
        let fast = &named.sets["fast_cars"];
        assert_eq!(fast.writable, Writable::default());
        assert_eq!(
            fast.output.as_ref().and_then(|f| f.q.as_deref()),
            Some("speed>2")
        );
        assert_eq!(fast.members, edm.sets["Car"].members);
        // a model without a container takes no output
        let mut bare = edm;
        for schema in &mut bare.csdl.schemas {
            schema.entity_container = None;
        }
        assert_eq!(outputs(bare.clone(), &listed), bare);
    }

    #[test]
    fn every_builtin_has_its_edm_type() {
        use linkml_model::Builtin as B;
        let all = [
            (B::String, "Edm.String"),
            (B::Uri, "Edm.String"),
            (B::Uriorcurie, "Edm.String"),
            (B::Integer, "Edm.Int64"),
            (B::Float, "Edm.Double"),
            (B::Double, "Edm.Double"),
            (B::Decimal, "Edm.Decimal"),
            (B::Boolean, "Edm.Boolean"),
            (B::Date, "Edm.Date"),
            (B::Datetime, "Edm.DateTimeOffset"),
            (B::Time, "Edm.TimeOfDay"),
        ];
        for (b, edm) in all {
            assert_eq!(edm_type(b), edm, "{b:?}");
        }
    }

    #[test]
    fn shapes_say_what_filters_and_sorts() {
        assert!(Shape::Value {
            edm: "Edm.Int64",
            collection: false
        }
        .sortable());
        assert!(!Shape::Value {
            edm: "Edm.Int64",
            collection: true
        }
        .sortable());
        assert!(Shape::Value {
            edm: "Edm.Int64",
            collection: true
        }
        .filterable());
        assert!(!Shape::Json.filterable());
        assert!(!Shape::Geo.filterable());
        assert!(!Shape::Geo.sortable());
        assert!(!Shape::Nav {
            target: "A".into(),
            collection: false
        }
        .filterable());
        assert!(Shape::Url { collection: false }.sortable());
        assert!(base_member("_types"));
        assert!(!base_member("speed"));
    }

    /// T-5207: a kind a later NGSI-LD version adds degrades its own slot to a read-only,
    /// unfiltered `Edm.Untyped` with the kind stated; the other slots keep their types, and the
    /// schema states its version with the mapping's.
    #[test]
    fn a_new_kind_degrades_its_slot_and_the_schema_version_is_stated() {
        let json = br#"{"id": "https://e.org/s", "name": "s", "version": "2.1.0+b7",
          "classes": {"Car": {"slots": ["speed", "trace"]}},
          "slots": {"speed": {"range": "float"},
            "trace": {"range": "float", "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "TimeSeriesProperty"}}}}}"#;
        let limits = linkml_model::Limits {
            max_bytes: 1 << 16,
            max_depth: 16,
            max_elements: 64,
        };
        let s = linkml_model::Model::read(json, &limits).unwrap();
        let edm = build(
            "v",
            &s,
            &shown(&s, &[("Car", &["speed", "trace"])]),
            Writable::default(),
            CsdlVersion::V4_01,
        )
        .unwrap();
        let car = &edm.sets["Car"].members;
        assert_eq!(car["trace"].shape, Shape::Json);
        assert!(!car["trace"].shape.filterable());
        assert_eq!(car["trace"].written, Written::Never);
        assert_eq!(
            car["speed"].shape,
            Shape::Value {
                edm: "Edm.Double",
                collection: false
            }
        );
        assert_eq!(edm.schema_version, format!("2.1.0_b7.m{MAPPING_VERSION}"));
        let own = &edm.csdl.schemas[1];
        assert!(own
            .annotations
            .iter()
            .any(|a| a.term == "Core.SchemaVersion"));
    }
}
