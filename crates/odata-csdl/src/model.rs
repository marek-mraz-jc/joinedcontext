// SPDX-License-Identifier: EUPL-1.2
//! The EDM model: the subset of CSDL 4.01 this crate writes (ADR-0024 decision 5).
//!
//! Everything is plain data with public fields; [`validate`](fn@crate::validate) decides whether a value is a
//! model the writers may print. Structures that the subset leaves out (actions, functions,
//! singletons, enum and type definitions, media and stream properties) have no field here, so a
//! caller cannot ask for them.

/// The CSDL version a document declares (`Version` / `$Version`).
///
/// ```
/// use odata_csdl::CsdlVersion;
/// assert_eq!(CsdlVersion::V4_01.as_str(), "4.01");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsdlVersion {
    /// OData 4.0: no `Edm.Untyped`, no CSDL JSON.
    V4_0,
    /// OData 4.01.
    V4_01,
}

impl CsdlVersion {
    /// The version string both representations carry.
    ///
    /// ```
    /// assert_eq!(odata_csdl::CsdlVersion::V4_0.as_str(), "4.0");
    /// ```
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::V4_0 => "4.0",
            Self::V4_01 => "4.01",
        }
    }
}

/// One metadata document: references to other documents and the schemas it defines.
///
/// ```
/// use odata_csdl::{CsdlVersion, Model};
/// let model = Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![] };
/// assert!(model.schemas.is_empty());
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    /// The declared version.
    pub version: CsdlVersion,
    /// `edmx:Reference` / `$Reference`, in order.
    pub references: Vec<Reference>,
    /// `Schema`, in order; at most one carries the entity container.
    pub schemas: Vec<Schema>,
}

impl Model {
    /// The namespace a qualifier names: a schema's namespace or alias, or a namespace included
    /// from a reference, by itself or by its alias.
    ///
    /// ```
    /// use odata_csdl::{CsdlVersion, Include, Model, Reference, Schema};
    /// let mut city = Schema::new("City");
    /// city.alias = Some("c".into());
    /// let core = Include { namespace: "Org.OData.Core.V1".into(), alias: Some("Core".into()) };
    /// let model = Model {
    ///     version: CsdlVersion::V4_01,
    ///     references: vec![Reference { uri: "core.xml".into(), includes: vec![core] }],
    ///     schemas: vec![city],
    /// };
    /// assert_eq!(model.namespace("c"), Some("City"));
    /// assert_eq!(model.namespace("Core"), Some("Org.OData.Core.V1"));
    /// assert_eq!(model.namespace("Other"), None);
    /// ```
    #[must_use]
    pub fn namespace(&self, qualifier: &str) -> Option<&str> {
        let local = self
            .schemas
            .iter()
            .map(|s| (s.namespace.as_str(), s.alias.as_deref()));
        let included = self
            .references
            .iter()
            .flat_map(|r| &r.includes)
            .map(|i| (i.namespace.as_str(), i.alias.as_deref()));
        local
            .chain(included)
            .find(|(namespace, alias)| *namespace == qualifier || *alias == Some(qualifier))
            .map(|(namespace, _)| namespace)
    }

    /// A qualified name (`Qualifier.Name`) with its qualifier replaced by the namespace it
    /// names: how two spellings of one type or term compare.
    ///
    /// ```
    /// # use odata_csdl::{CsdlVersion, Model, Schema};
    /// # let mut city = Schema::new("City");
    /// # city.alias = Some("c".into());
    /// # let model = Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![city] };
    /// assert_eq!(model.full_name("c.Building").as_deref(), Some("City.Building"));
    /// assert_eq!(model.full_name("Edm.String"), None);
    /// ```
    #[must_use]
    pub fn full_name(&self, qualified: &str) -> Option<String> {
        let (prefix, local) = qualified.rsplit_once('.')?;
        self.namespace(prefix)
            .map(|namespace| format!("{namespace}.{local}"))
    }

    /// The schema of a qualified name and the name in it.
    fn declared<'q>(&self, qualified: &'q str) -> Option<(&Schema, &'q str)> {
        let (prefix, local) = qualified.rsplit_once('.')?;
        let schema = self
            .schemas
            .iter()
            .find(|s| s.namespace == prefix || s.alias.as_deref() == Some(prefix))?;
        Some((schema, local))
    }

    /// The entity type a qualified name (namespace or alias) names.
    ///
    /// ```
    /// # use odata_csdl::{CsdlVersion, EntityType, Model, Schema};
    /// # let mut city = Schema::new("City");
    /// # city.entity_types.push(EntityType::new("Building"));
    /// # let model = Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![city] };
    /// assert!(model.entity_type("City.Building").is_some());
    /// assert!(model.entity_type("City.Tower").is_none());
    /// ```
    #[must_use]
    pub fn entity_type(&self, qualified: &str) -> Option<&EntityType> {
        let (schema, name) = self.declared(qualified)?;
        schema.entity_types.iter().find(|t| t.name == name)
    }

    /// The complex type a qualified name (namespace or alias) names.
    ///
    /// ```
    /// # use odata_csdl::{ComplexType, CsdlVersion, Model, Schema};
    /// # let mut city = Schema::new("City");
    /// # city.complex_types.push(ComplexType::new("Address"));
    /// # let model = Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![city] };
    /// assert!(model.complex_type("City.Address").is_some());
    /// assert!(model.complex_type("Nowhere.Address").is_none());
    /// ```
    #[must_use]
    pub fn complex_type(&self, qualified: &str) -> Option<&ComplexType> {
        let (schema, name) = self.declared(qualified)?;
        schema.complex_types.iter().find(|t| t.name == name)
    }

    /// The entity set `name` of the model's entity container.
    ///
    /// ```
    /// # use odata_csdl::{CsdlVersion, EntityContainer, EntitySet, Model, Schema};
    /// # let mut city = Schema::new("City");
    /// # let mut service = EntityContainer::new("Service");
    /// # service.entity_sets.push(EntitySet::new("Buildings", "City.Building"));
    /// # city.entity_container = Some(service);
    /// # let model = Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![city] };
    /// assert_eq!(model.entity_set("Buildings").map(|s| s.entity_type.as_str()), Some("City.Building"));
    /// assert!(model.entity_set("Towers").is_none());
    /// ```
    #[must_use]
    pub fn entity_set(&self, name: &str) -> Option<&EntitySet> {
        self.schemas
            .iter()
            .filter_map(|s| s.entity_container.as_ref())
            .flat_map(|c| &c.entity_sets)
            .find(|set| set.name == name)
    }

    /// `ty` and its base types, most derived first. The walk stops at a base type this model
    /// does not declare as an entity type, and after as many steps as there are entity types
    /// (a base-type cycle is a validation error of the model).
    ///
    /// ```
    /// # use odata_csdl::{CsdlVersion, EntityType, Model, Schema};
    /// # let mut city = Schema::new("City");
    /// # let mut tower = EntityType::new("Tower");
    /// # tower.base_type = Some("City.Building".into());
    /// # city.entity_types = vec![EntityType::new("Building"), tower];
    /// # let model = Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![city] };
    /// let tower = model.entity_type("City.Tower").ok_or("no tower")?;
    /// let names: Vec<&str> = model.lineage(tower).iter().map(|t| t.name.as_str()).collect();
    /// assert_eq!(names, ["Tower", "Building"]);
    /// # Ok::<(), &str>(())
    /// ```
    #[must_use]
    pub fn lineage<'m>(&'m self, ty: &'m EntityType) -> Vec<&'m EntityType> {
        let types: usize = self.schemas.iter().map(|s| s.entity_types.len()).sum();
        let mut lineage = vec![ty];
        let mut current = ty;
        while lineage.len() < types {
            match current
                .base_type
                .as_deref()
                .and_then(|base| self.entity_type(base))
            {
                Some(base) => {
                    lineage.push(base);
                    current = base;
                }
                None => break,
            }
        }
        lineage
    }
}

/// A reference to another CSDL document and the namespaces taken from it.
///
/// ```
/// use odata_csdl::{Include, Reference};
/// let core = Reference {
///     uri: "https://oasis-tcs.github.io/odata-vocabularies/vocabularies/Org.OData.Core.V1.xml".into(),
///     includes: vec![Include { namespace: "Org.OData.Core.V1".into(), alias: Some("Core".into()) }],
/// };
/// assert_eq!(core.includes.len(), 1);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Reference {
    /// The document's URI.
    pub uri: String,
    /// The namespaces included from it.
    pub includes: Vec<Include>,
}

/// One namespace included from a referenced document, with its optional alias.
///
/// ```
/// let include = odata_csdl::Include { namespace: "Org.OData.Capabilities.V1".into(), alias: None };
/// assert!(include.alias.is_none());
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Include {
    /// The included namespace.
    pub namespace: String,
    /// The alias annotations and type names may use in its place.
    pub alias: Option<String>,
}

/// A CSDL schema: one namespace and what it defines.
///
/// ```
/// let schema = odata_csdl::Schema::new("Antares.NgsiLd");
/// assert_eq!(schema.namespace, "Antares.NgsiLd");
/// assert!(schema.entity_container.is_none());
/// ```
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Schema {
    /// The namespace, a dotted list of simple identifiers.
    pub namespace: String,
    /// The alias the document may use in place of the namespace.
    pub alias: Option<String>,
    /// Entity types, in order.
    pub entity_types: Vec<EntityType>,
    /// Complex types, in order.
    pub complex_types: Vec<ComplexType>,
    /// Term definitions, in order.
    pub terms: Vec<Term>,
    /// The entity container; at most one schema of a model has one.
    pub entity_container: Option<EntityContainer>,
    /// Annotations on the schema itself (for example `Core.SchemaVersion`).
    pub annotations: Vec<Annotation>,
}

impl Schema {
    /// An empty schema of `namespace`.
    ///
    /// ```
    /// assert!(odata_csdl::Schema::new("ns").entity_types.is_empty());
    /// ```
    #[must_use]
    pub fn new(namespace: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            ..Self::default()
        }
    }
}

/// An entity type. Derivation is limited to one abstract base (ADR-0024 decision 5).
///
/// ```
/// use odata_csdl::{EntityType, Property, TypeRef};
/// let mut base = EntityType::new("Entity");
/// base.is_abstract = true;
/// base.key = vec!["id".into()];
/// base.properties.push(Property::new("id", TypeRef::single("Edm.String")).required());
/// assert_eq!(base.key, ["id"]);
/// ```
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EntityType {
    /// A simple identifier, unique in its schema.
    pub name: String,
    /// The qualified name of the abstract base type, if any.
    pub base_type: Option<String>,
    /// `Abstract`: no entity is of exactly this type.
    pub is_abstract: bool,
    /// `OpenType`: instances may carry dynamic properties.
    pub open: bool,
    /// The key's property names, declared on this type; empty when inherited.
    pub key: Vec<String>,
    /// Structural properties, in order.
    pub properties: Vec<Property>,
    /// Navigation properties, in order.
    pub navigation_properties: Vec<NavigationProperty>,
    /// Annotations on the type.
    pub annotations: Vec<Annotation>,
}

impl EntityType {
    /// A concrete, closed entity type named `name` with nothing in it yet.
    ///
    /// ```
    /// assert!(!odata_csdl::EntityType::new("Building").is_abstract);
    /// ```
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }
}

/// A complex type: structured, keyless, without derivation.
///
/// ```
/// let address = odata_csdl::ComplexType::new("Address");
/// assert!(address.properties.is_empty());
/// ```
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ComplexType {
    /// A simple identifier, unique in its schema.
    pub name: String,
    /// `OpenType`.
    pub open: bool,
    /// Structural properties, in order.
    pub properties: Vec<Property>,
    /// Annotations on the type.
    pub annotations: Vec<Annotation>,
}

impl ComplexType {
    /// A closed complex type named `name` with nothing in it yet.
    ///
    /// ```
    /// assert!(!odata_csdl::ComplexType::new("Address").open);
    /// ```
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }
}

/// A type reference: a qualified type name, single- or collection-valued.
///
/// ```
/// use odata_csdl::TypeRef;
/// assert!(TypeRef::collection("Edm.String").collection);
/// assert!(!TypeRef::single("Edm.Int64").collection);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeRef {
    /// `Edm.<primitive>` or `<namespace or alias>.<Name>`.
    pub name: String,
    /// `Collection(...)` in XML, `$Collection` in JSON.
    pub collection: bool,
}

impl TypeRef {
    /// A single-valued reference to `name`.
    ///
    /// ```
    /// assert_eq!(odata_csdl::TypeRef::single("Edm.Date").name, "Edm.Date");
    /// ```
    #[must_use]
    pub fn single(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            collection: false,
        }
    }

    /// A collection of `name`.
    ///
    /// ```
    /// assert!(odata_csdl::TypeRef::collection("ns.Address").collection);
    /// ```
    #[must_use]
    pub fn collection(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            collection: true,
        }
    }
}

/// The `Scale` facet of an `Edm.Decimal`.
///
/// ```
/// assert_eq!(odata_csdl::Scale::Variable, odata_csdl::Scale::Variable);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    /// At most this many fractional digits.
    Digits(u32),
    /// `variable`: any number of fractional digits up to the precision.
    Variable,
    /// `floating`: a decimal floating-point number.
    Floating,
}

/// The `SRID` facet of a geography or geometry property.
///
/// ```
/// assert_eq!(odata_csdl::Srid::Id(4326), odata_csdl::Srid::Id(4326));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Srid {
    /// One coordinate reference system, by EPSG code.
    Id(u32),
    /// `variable`: each value names its own.
    Variable,
}

/// A structural property.
///
/// ```
/// use odata_csdl::{Property, TypeRef};
/// let p = Property::new("id", TypeRef::single("Edm.String")).required();
/// assert!(!p.nullable);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    /// A simple identifier, unique in its type (base type included).
    pub name: String,
    /// A primitive or complex type.
    pub ty: TypeRef,
    /// Whether the value (or a collection's item) may be null.
    pub nullable: bool,
    /// `Precision`: significant digits of an `Edm.Decimal` (at least 1), or fractional-second
    /// digits of an `Edm.DateTimeOffset`, `Edm.Duration` or `Edm.TimeOfDay` (0 to 12; absent
    /// means 0 there, CSDL XML 7.2.3).
    pub precision: Option<u32>,
    /// `Scale`, on `Edm.Decimal` only; absent means 0 (CSDL XML 7.2.4).
    pub scale: Option<Scale>,
    /// `SRID`, on geography and geometry types only.
    pub srid: Option<Srid>,
    /// Annotations on the property.
    pub annotations: Vec<Annotation>,
}

impl Property {
    /// A nullable property of type `ty` with no facets.
    ///
    /// ```
    /// use odata_csdl::{Property, TypeRef};
    /// assert!(Property::new("name", TypeRef::single("Edm.String")).nullable);
    /// ```
    #[must_use]
    pub fn new(name: impl Into<String>, ty: TypeRef) -> Self {
        Self {
            name: name.into(),
            ty,
            nullable: true,
            precision: None,
            scale: None,
            srid: None,
            annotations: Vec::new(),
        }
    }

    /// The same property, not nullable.
    ///
    /// ```
    /// use odata_csdl::{Property, TypeRef};
    /// assert!(!Property::new("id", TypeRef::single("Edm.String")).required().nullable);
    /// ```
    #[must_use]
    pub fn required(mut self) -> Self {
        self.nullable = false;
        self
    }
}

/// A navigation property to an entity type of the model.
///
/// ```
/// use odata_csdl::{NavigationProperty, TypeRef};
/// let owner = NavigationProperty::new("owner", TypeRef::single("ns.Person"));
/// assert!(owner.nullable && owner.partner.is_none());
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct NavigationProperty {
    /// A simple identifier, unique in its type (base type included).
    pub name: String,
    /// The target entity type, single- or collection-valued.
    pub ty: TypeRef,
    /// Whether a single-valued navigation may be absent; must stay `true` on a collection,
    /// whose `Nullable` is never written (CSDL XML 8.2: the collection always exists).
    pub nullable: bool,
    /// The navigation property of the target type that leads back here.
    pub partner: Option<String>,
    /// The dependent properties whose values key the target (CSDL XML 8.5).
    pub referential_constraints: Vec<ReferentialConstraint>,
    /// Annotations on the navigation property.
    pub annotations: Vec<Annotation>,
}

/// A referential constraint of a single-valued navigation property (CSDL XML 8.5): `property`
/// of the declaring type holds the value of `referenced_property` of the target type.
///
/// ```
/// use odata_csdl::ReferentialConstraint;
/// let c = ReferentialConstraint::new("ProductID", "ID");
/// assert_eq!((c.property.as_str(), c.referenced_property.as_str()), ("ProductID", "ID"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferentialConstraint {
    /// The dependent property: a primitive property of the declaring type.
    pub property: String,
    /// The principal property of the target type, of the same type.
    pub referenced_property: String,
}

impl ReferentialConstraint {
    /// `property` references `referenced_property`.
    #[must_use]
    pub fn new(property: impl Into<String>, referenced_property: impl Into<String>) -> Self {
        Self {
            property: property.into(),
            referenced_property: referenced_property.into(),
        }
    }
}

impl NavigationProperty {
    /// A nullable navigation property without a partner.
    ///
    /// ```
    /// use odata_csdl::{NavigationProperty, TypeRef};
    /// assert!(NavigationProperty::new("parts", TypeRef::collection("ns.Part")).ty.collection);
    /// ```
    #[must_use]
    pub fn new(name: impl Into<String>, ty: TypeRef) -> Self {
        Self {
            name: name.into(),
            ty,
            nullable: true,
            partner: None,
            referential_constraints: Vec::new(),
            annotations: Vec::new(),
        }
    }
}

/// A term definition: the vocabulary half of an annotation.
///
/// ```
/// use odata_csdl::{Term, TypeRef};
/// let iri = Term::new("Iri", TypeRef::single("Edm.String"));
/// assert!(iri.applies_to.is_empty());
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Term {
    /// A simple identifier, unique in its schema.
    pub name: String,
    /// The type an annotation value has.
    pub ty: TypeRef,
    /// Whether an annotation value may be null.
    pub nullable: bool,
    /// `Precision`, as on [`Property::precision`].
    pub precision: Option<u32>,
    /// The model elements the term applies to (`AppliesTo`), by CSDL element name; empty for any.
    pub applies_to: Vec<String>,
    /// Annotations on the term (for example `Core.Description`).
    pub annotations: Vec<Annotation>,
}

impl Term {
    /// A nullable term of type `ty` that applies to anything.
    ///
    /// ```
    /// use odata_csdl::{Term, TypeRef};
    /// assert!(Term::new("Kind", TypeRef::single("Edm.String")).nullable);
    /// ```
    #[must_use]
    pub fn new(name: impl Into<String>, ty: TypeRef) -> Self {
        Self {
            name: name.into(),
            ty,
            nullable: true,
            precision: None,
            applies_to: Vec::new(),
            annotations: Vec::new(),
        }
    }
}

/// The entity container: the entity sets a service exposes.
///
/// ```
/// let container = odata_csdl::EntityContainer::new("Container");
/// assert!(container.entity_sets.is_empty());
/// ```
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EntityContainer {
    /// A simple identifier, unique in its schema.
    pub name: String,
    /// Entity sets, in order.
    pub entity_sets: Vec<EntitySet>,
    /// Annotations on the container.
    pub annotations: Vec<Annotation>,
}

impl EntityContainer {
    /// An empty container named `name`.
    ///
    /// ```
    /// assert_eq!(odata_csdl::EntityContainer::new("C").name, "C");
    /// ```
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }
}

/// An entity set of one entity type.
///
/// ```
/// let set = odata_csdl::EntitySet::new("Buildings", "ns.Building");
/// assert!(set.bindings.is_empty());
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct EntitySet {
    /// A simple identifier, unique in the container.
    pub name: String,
    /// The qualified name of its entity type.
    pub entity_type: String,
    /// Where each navigation property of its entities leads.
    pub bindings: Vec<NavigationPropertyBinding>,
    /// Annotations on the set (the Capabilities terms, for example).
    pub annotations: Vec<Annotation>,
}

impl EntitySet {
    /// A set of `entity_type` without bindings or annotations.
    ///
    /// ```
    /// assert_eq!(odata_csdl::EntitySet::new("S", "ns.T").entity_type, "ns.T");
    /// ```
    #[must_use]
    pub fn new(name: impl Into<String>, entity_type: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            entity_type: entity_type.into(),
            bindings: Vec::new(),
            annotations: Vec::new(),
        }
    }
}

/// A navigation property binding: `path` (a navigation property, optionally followed by a type
/// cast `/<ns>.<Type>`) leads into the entity set `target` of the same container.
///
/// ```
/// let b = odata_csdl::NavigationPropertyBinding { path: "owner".into(), target: "People".into() };
/// assert_eq!(b.target, "People");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationPropertyBinding {
    /// The navigation property, with an optional type-cast segment.
    pub path: String,
    /// The target entity set's name.
    pub target: String,
}

/// One annotation: a term applied with a value.
///
/// ```
/// use odata_csdl::{Annotation, Expr};
/// let a = Annotation::new("Core.Computed", Expr::Bool(true));
/// assert!(a.qualifier.is_none());
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    /// The term's qualified name (namespace or alias).
    pub term: String,
    /// An optional qualifier, a simple identifier.
    pub qualifier: Option<String>,
    /// The value.
    pub value: Expr,
}

impl Annotation {
    /// `term` applied with `value`, unqualified.
    ///
    /// ```
    /// use odata_csdl::{Annotation, Expr};
    /// assert_eq!(Annotation::new("Core.Description", Expr::String("x".into())).term, "Core.Description");
    /// ```
    #[must_use]
    pub fn new(term: impl Into<String>, value: Expr) -> Self {
        Self {
            term: term.into(),
            qualifier: None,
            value,
        }
    }
}

/// `text` with every CR LF and lone CR as LF (XML 1.0 section 2.11).
pub(crate) fn line_ends(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// An annotation value: the constant, path, collection and record expressions of the subset.
///
/// ```
/// use odata_csdl::Expr;
/// let insert = Expr::Record(vec![("Insertable".into(), Expr::Bool(false))]);
/// assert!(matches!(insert, Expr::Record(_)));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// `Bool`.
    Bool(bool),
    /// `Int`.
    Int(i64),
    /// `String`. Both writers write its line ends as LF: XML 1.0 section 2.11 reads a literal
    /// CR or CR LF as LF, and the OASIS converter carries that into CSDL JSON.
    String(String),
    /// `EnumMember`: the enum type's qualified name and one or more member names.
    EnumMember {
        /// The enum type, for example `Capabilities.ConformanceLevelType`.
        ty: String,
        /// Member names; more than one only for a flags enum.
        members: Vec<String>,
    },
    /// `Path`.
    Path(String),
    /// `PropertyPath`.
    PropertyPath(String),
    /// `NavigationPropertyPath`.
    NavigationPropertyPath(String),
    /// `Collection`.
    Collection(Vec<Expr>),
    /// `Record`, its property values in order; the record type is the term's.
    Record(Vec<(String, Expr)>),
}
