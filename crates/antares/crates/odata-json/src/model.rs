// SPDX-License-Identifier: EUPL-1.2
//! The entity set a request addresses, resolved once against the model, and the vocabulary
//! terms the reader enforces.

use odata_csdl::{Annotation, EntitySet, EntityType, Expr, Model, NavigationProperty, Property};

const CORE: &str = "Org.OData.Core.V1";
const CAPABILITIES: &str = "Org.OData.Capabilities.V1";

/// An entity set of a model with its entity type and base types resolved: what every writer and
/// reader of this crate works on.
///
/// ```
/// use odata_csdl::{CsdlVersion, EntityContainer, EntitySet, EntityType, Model, Schema};
/// use odata_json::Resource;
///
/// let mut city = Schema::new("City");
/// city.entity_types.push(EntityType::new("Building"));
/// let mut service = EntityContainer::new("Service");
/// service.entity_sets.push(EntitySet::new("Buildings", "City.Building"));
/// city.entity_container = Some(service);
/// let model = Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![city] };
///
/// let buildings = Resource::new(&model, "Buildings").ok_or("no set")?;
/// assert_eq!(buildings.set(), "Buildings");
/// assert!(Resource::new(&model, "Towers").is_none());
/// # Ok::<(), &str>(())
/// ```
#[derive(Debug, Clone)]
pub struct Resource<'m> {
    model: &'m Model,
    set: &'m EntitySet,
    type_name: String,
    lineage: Vec<&'m EntityType>,
    select: Option<&'m [&'m str]>,
}

impl<'m> Resource<'m> {
    /// The entity set `set` of `model`; `None` when the model has no such set or its entity type
    /// is not declared.
    #[must_use]
    pub fn new(model: &'m Model, set: &str) -> Option<Self> {
        let set = model.entity_set(set)?;
        let ty = model.entity_type(&set.entity_type)?;
        Some(Self {
            model,
            set,
            // Resolved above, so the qualifier names a namespace.
            type_name: model.full_name(&set.entity_type).unwrap_or_default(),
            lineage: model.lineage(ty),
            select: None,
        })
    }

    /// The same set, written projected by a `$select` list (its context URL names it).
    ///
    /// ```
    /// # use odata_csdl::{CsdlVersion, EntityContainer, EntitySet, EntityType, Model, Schema};
    /// # use odata_json::Resource;
    /// # let mut city = Schema::new("City");
    /// # city.entity_types.push(EntityType::new("Building"));
    /// # let mut service = EntityContainer::new("Service");
    /// # service.entity_sets.push(EntitySet::new("Buildings", "City.Building"));
    /// # city.entity_container = Some(service);
    /// # let model = Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![city] };
    /// let names = ["id"];
    /// let projected = Resource::new(&model, "Buildings").ok_or("no set")?.select(&names);
    /// assert_eq!(projected.set(), "Buildings");
    /// # Ok::<(), &str>(())
    /// ```
    #[must_use]
    pub fn select(self, names: &'m [&'m str]) -> Self {
        Self {
            select: Some(names),
            ..self
        }
    }

    /// The entity set's name.
    #[must_use]
    pub fn set(&self) -> &'m str {
        &self.set.name
    }

    pub(crate) fn model(&self) -> &'m Model {
        self.model
    }

    pub(crate) fn projection(&self) -> Option<&'m [&'m str]> {
        self.select
    }

    /// The entity type's namespace-qualified name.
    pub(crate) fn type_name(&self) -> &str {
        &self.type_name
    }

    /// A structural property of the type or a base type.
    pub(crate) fn property(&self, name: &str) -> Option<&'m Property> {
        self.lineage
            .iter()
            .flat_map(|t| &t.properties)
            .find(|p| p.name == name)
    }

    /// A navigation property of the type or a base type.
    pub(crate) fn navigation(&self, name: &str) -> Option<&'m NavigationProperty> {
        self.navigations().find(|n| n.name == name)
    }

    pub(crate) fn navigations(&self) -> impl Iterator<Item = &'m NavigationProperty> + '_ {
        self.lineage.iter().flat_map(|t| &t.navigation_properties)
    }

    /// The key: that of the most derived type in the lineage that declares one.
    pub(crate) fn key(&self) -> &'m [String] {
        self.lineage
            .iter()
            .map(|t| t.key.as_slice())
            .find(|key| !key.is_empty())
            .unwrap_or_default()
    }

    /// The paths a `record` field of a `Capabilities` restriction term on the set names:
    /// `NonInsertableProperties` of `InsertRestrictions`, for example.
    pub(crate) fn restricted(&self, term: &str, field: &str) -> Vec<&'m str> {
        let model = self.model;
        self.set
            .annotations
            .iter()
            .filter(|a| is_term(model, a, CAPABILITIES, term))
            .filter_map(|a| match &a.value {
                Expr::Record(fields) => Some(fields),
                _ => None,
            })
            .flatten()
            .filter(|(name, _)| name == field)
            .filter_map(|(_, value)| match value {
                Expr::Collection(items) => Some(items),
                _ => None,
            })
            .flatten()
            .filter_map(|item| match item {
                Expr::PropertyPath(path)
                | Expr::NavigationPropertyPath(path)
                | Expr::Path(path) => Some(path.as_str()),
                _ => None,
            })
            .collect()
    }
}

/// Whether `annotation` is `namespace.name`, however the model qualifies it.
fn is_term(model: &Model, annotation: &Annotation, namespace: &str, name: &str) -> bool {
    model.full_name(&annotation.term).is_some_and(|full| {
        full.strip_prefix(namespace)
            .and_then(|t| t.strip_prefix('.'))
            == Some(name)
    })
}

/// `Core.Computed` with the value `true`: the service computes the property.
pub(crate) fn computed(model: &Model, property: &Property) -> bool {
    annotated(model, property, "Computed")
}

/// `Core.ComputedDefaultValue` with the value `true`: a client may set the
/// property on insert, and the service computes it when the client does not.
pub(crate) fn computed_default(model: &Model, property: &Property) -> bool {
    annotated(model, property, "ComputedDefaultValue")
}

/// `Core.{term}` with the value `true` on `property`.
fn annotated(model: &Model, property: &Property, term: &str) -> bool {
    property
        .annotations
        .iter()
        .any(|a| is_term(model, a, CORE, term) && a.value == Expr::Bool(true))
}
