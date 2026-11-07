// SPDX-License-Identifier: EUPL-1.2
//! Model validation: the CSDL rules the writers rely on, under the caller's limits.

use std::collections::{HashMap, HashSet};

use crate::error::{CsdlError, Feature, LimitKind, Problem, ValidationError};
use crate::model::{
    Annotation, CsdlVersion, EntityContainer, EntitySet, EntityType, Expr, Model,
    NavigationProperty, Property, ReferentialConstraint, Schema, Term, TypeRef,
};
use crate::names::{is_namespace, is_simple_identifier, split_qualified};

/// The bounds a model must stay within before anything is written (ADR-0024 decision 4).
///
/// ```
/// let limits = odata_csdl::Limits::strict();
/// assert!(limits.max_elements > 0 && limits.max_expr_depth > 0);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Model elements in total: schemas, references, includes, types, properties, terms, sets,
    /// bindings, annotations and annotation expression nodes.
    pub max_elements: usize,
    /// Nesting of collection and record expressions in one annotation value.
    pub max_expr_depth: usize,
    /// Bytes of one string constant, path or reference URI.
    pub max_string_len: usize,
}

impl Limits {
    /// Conservative values for tests and documentation; a service passes its own.
    ///
    /// ```
    /// assert_eq!(odata_csdl::Limits::strict().max_expr_depth, 8);
    /// ```
    #[must_use]
    pub fn strict() -> Self {
        Self {
            max_elements: 100_000,
            max_expr_depth: 8,
            max_string_len: 4096,
        }
    }
}

/// The primitive types of the subset; `Edm.Stream` is answered `NotImplemented`.
const PRIMITIVES: &[&str] = &[
    "Edm.Binary",
    "Edm.Boolean",
    "Edm.Byte",
    "Edm.Date",
    "Edm.DateTimeOffset",
    "Edm.Decimal",
    "Edm.Double",
    "Edm.Duration",
    "Edm.Guid",
    "Edm.Int16",
    "Edm.Int32",
    "Edm.Int64",
    "Edm.SByte",
    "Edm.Single",
    "Edm.String",
    "Edm.TimeOfDay",
    "Edm.Untyped",
    "Edm.Geography",
    "Edm.GeographyPoint",
    "Edm.GeographyLineString",
    "Edm.GeographyPolygon",
    "Edm.GeographyMultiPoint",
    "Edm.GeographyMultiLineString",
    "Edm.GeographyMultiPolygon",
    "Edm.GeographyCollection",
    "Edm.Geometry",
    "Edm.GeometryPoint",
    "Edm.GeometryLineString",
    "Edm.GeometryPolygon",
    "Edm.GeometryMultiPoint",
    "Edm.GeometryMultiLineString",
    "Edm.GeometryMultiPolygon",
    "Edm.GeometryCollection",
];

/// The primitive types a key property may have (CSDL XML 6.5).
const KEY_TYPES: &[&str] = &[
    "Edm.Boolean",
    "Edm.Byte",
    "Edm.Date",
    "Edm.DateTimeOffset",
    "Edm.Decimal",
    "Edm.Duration",
    "Edm.Guid",
    "Edm.Int16",
    "Edm.Int32",
    "Edm.Int64",
    "Edm.SByte",
    "Edm.String",
    "Edm.TimeOfDay",
];

/// The element names `AppliesTo` may list (`csdl.schema.json`, `Term.$AppliesTo`).
const ELEMENTS: &[&str] = &[
    "Action",
    "ActionImport",
    "Annotation",
    "Apply",
    "Cast",
    "Collection",
    "ComplexType",
    "EntityContainer",
    "EntitySet",
    "EntityType",
    "EnumType",
    "Function",
    "FunctionImport",
    "If",
    "Include",
    "IsOf",
    "LabeledElement",
    "Member",
    "NavigationProperty",
    "Null",
    "OnDelete",
    "Parameter",
    "Property",
    "PropertyValue",
    "Record",
    "Reference",
    "ReferentialConstraint",
    "ReturnType",
    "Schema",
    "Singleton",
    "Term",
    "TypeDefinition",
    "UrlRef",
];

/// Checks `model` against the CSDL rules the writers rely on and the caller's `limits`.
///
/// Every broken rule is listed in [`CsdlError::Invalid`]; a limit or a construct outside the
/// subset stops the walk at once.
///
/// # Errors
///
/// [`CsdlError::Invalid`], [`CsdlError::LimitExceeded`] or [`CsdlError::NotImplemented`].
///
/// ```
/// use odata_csdl::{validate, CsdlError, CsdlVersion, EntityType, Limits, Model, Problem, Schema};
/// let mut schema = Schema::new("ns");
/// schema.entity_types.push(EntityType::new("Keyless"));
/// let model = Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![schema] };
/// let Err(CsdlError::Invalid(problems)) = validate(&model, &Limits::strict()) else { panic!() };
/// assert_eq!(problems[0].problem, Problem::MissingKey);
/// ```
pub fn validate(model: &Model, limits: &Limits) -> Result<(), CsdlError> {
    let mut check = Check {
        model,
        limits,
        qualifiers: HashMap::new(),
        problems: Vec::new(),
        elements: 0,
    };
    check.model()?;
    if check.problems.is_empty() {
        Ok(())
    } else {
        Err(CsdlError::Invalid(check.problems))
    }
}

/// What a qualified type name resolves to.
enum Resolved<'m> {
    Primitive(&'m str),
    Entity(&'m EntityType),
    Complex,
    /// A type of an included namespace, which this model cannot see into.
    External,
}

/// The use a type reference is checked for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Use {
    Structural,
    Navigation,
    Term,
}

struct Check<'m> {
    model: &'m Model,
    limits: &'m Limits,
    /// Every namespace and alias: the schema it names, `None` for an included one.
    qualifiers: HashMap<&'m str, Option<&'m Schema>>,
    problems: Vec<ValidationError>,
    elements: usize,
}

impl<'m> Check<'m> {
    fn problem(&mut self, at: impl Into<String>, problem: Problem) {
        self.problems.push(ValidationError {
            at: at.into(),
            problem,
        });
    }

    fn count(&mut self) -> Result<(), CsdlError> {
        let max = self.limits.max_elements;
        match self.elements.checked_add(1) {
            Some(next) if next <= max => {
                self.elements = next;
                Ok(())
            }
            _ => Err(CsdlError::LimitExceeded {
                limit: LimitKind::Elements,
                max,
            }),
        }
    }

    fn text(&mut self, at: &str, text: &str) -> Result<(), CsdlError> {
        if text.len() > self.limits.max_string_len {
            return Err(CsdlError::LimitExceeded {
                limit: LimitKind::StringLen,
                max: self.limits.max_string_len,
            });
        }
        if text.chars().any(|c| {
            (c < ' ' && !matches!(c, '\t' | '\n' | '\r')) || matches!(c, '\u{FFFE}' | '\u{FFFF}')
        }) {
            self.problem(at, Problem::InvalidCharacter);
        }
        Ok(())
    }

    fn identifier(&mut self, at: &str, name: &str) {
        if !is_simple_identifier(name) {
            self.problem(at, Problem::InvalidIdentifier);
        }
    }

    fn qualifier(&mut self, at: &str, name: &'m str, schema: Option<&'m Schema>) {
        if self.qualifiers.insert(name, schema).is_some() {
            self.problem(at, Problem::Duplicate);
        }
    }

    fn model(&mut self) -> Result<(), CsdlError> {
        let model = self.model;
        for reference in &model.references {
            self.count()?;
            self.text(&reference.uri, &reference.uri)?;
            if reference.includes.is_empty() {
                self.problem(reference.uri.clone(), Problem::EmptyReference);
            }
            for include in &reference.includes {
                self.count()?;
                if !is_namespace(&include.namespace) {
                    self.problem(include.namespace.clone(), Problem::InvalidNamespace);
                }
                self.qualifier(&include.namespace, &include.namespace, None);
                if let Some(alias) = &include.alias {
                    self.identifier(alias, alias);
                    self.qualifier(alias, alias, None);
                }
            }
        }
        for schema in &model.schemas {
            self.count()?;
            if !is_namespace(&schema.namespace) {
                self.problem(schema.namespace.clone(), Problem::InvalidNamespace);
            }
            self.qualifier(&schema.namespace, &schema.namespace, Some(schema));
            if let Some(alias) = &schema.alias {
                self.identifier(alias, alias);
                self.qualifier(alias, alias, Some(schema));
            }
        }
        let mut containers = 0_usize;
        for schema in &model.schemas {
            if let Some(container) = &schema.entity_container {
                containers = containers.saturating_add(1);
                if containers > 1 {
                    self.problem(qualified(schema, &container.name), Problem::SecondContainer);
                }
            }
            self.schema(schema)?;
        }
        Ok(())
    }

    fn schema(&mut self, schema: &'m Schema) -> Result<(), CsdlError> {
        let mut names = HashSet::new();
        let declared = schema
            .entity_types
            .iter()
            .map(|t| t.name.as_str())
            .chain(schema.complex_types.iter().map(|t| t.name.as_str()))
            .chain(schema.terms.iter().map(|t| t.name.as_str()))
            .chain(schema.entity_container.iter().map(|c| c.name.as_str()));
        for name in declared {
            let at = qualified(schema, name);
            self.identifier(&at, name);
            if !names.insert(name) {
                self.problem(at, Problem::Duplicate);
            }
        }
        for entity_type in &schema.entity_types {
            self.entity_type(schema, entity_type)?;
        }
        for complex in &schema.complex_types {
            self.count()?;
            let at = qualified(schema, &complex.name);
            let mut members = HashSet::new();
            for property in &complex.properties {
                self.property(&at, &mut members, property)?;
            }
            self.annotations(&at, &complex.annotations)?;
        }
        for term in &schema.terms {
            self.term(schema, term)?;
        }
        if let Some(container) = &schema.entity_container {
            self.container(schema, container)?;
        }
        self.annotations(&schema.namespace, &schema.annotations)
    }

    fn entity_type(&mut self, schema: &'m Schema, ty: &'m EntityType) -> Result<(), CsdlError> {
        self.count()?;
        let at = qualified(schema, &ty.name);
        let base = match ty.base_type.as_deref().map(|name| self.resolve(name)) {
            Some(Ok(Resolved::Entity(base))) => {
                if !base.is_abstract || base.base_type.is_some() {
                    return Err(CsdlError::NotImplemented(Feature::DerivedType));
                }
                Some(base)
            }
            Some(_) => {
                self.problem(at.clone(), Problem::BaseTypeNotFound);
                None
            }
            None => None,
        };
        if ty.base_type.is_some() && !ty.key.is_empty() {
            self.problem(at.clone(), Problem::KeyOnDerivedType);
        }
        if ty.base_type.is_none() && ty.key.is_empty() && !ty.is_abstract {
            self.problem(at.clone(), Problem::MissingKey);
        }
        for key in &ty.key {
            match ty.properties.iter().find(|p| &p.name == key) {
                None => self.problem(format!("{at}/{key}"), Problem::KeyNotFound),
                Some(p)
                    if p.nullable
                        || p.ty.collection
                        || !KEY_TYPES.contains(&p.ty.name.as_str()) =>
                {
                    self.problem(format!("{at}/{key}"), Problem::InvalidKeyProperty);
                }
                Some(_) => {}
            }
        }
        let mut members: HashSet<&str> = base.iter().flat_map(|b| member_names(b)).collect();
        for property in &ty.properties {
            self.property(&at, &mut members, property)?;
        }
        for navigation in &ty.navigation_properties {
            self.navigation(&at, &mut members, ty, base, navigation)?;
        }
        self.annotations(&at, &ty.annotations)
    }

    fn member(&mut self, at: &str, members: &mut HashSet<&'m str>, name: &'m str) -> String {
        let at = format!("{at}/{name}");
        self.identifier(&at, name);
        if !members.insert(name) {
            self.problem(at.clone(), Problem::Duplicate);
        }
        at
    }

    fn property(
        &mut self,
        at: &str,
        members: &mut HashSet<&'m str>,
        property: &'m Property,
    ) -> Result<(), CsdlError> {
        self.count()?;
        let at = self.member(at, members, &property.name);
        self.type_ref(&at, &property.ty, Use::Structural)?;
        let name = property.ty.name.as_str();
        self.precision(&at, name, property.precision);
        if property.scale.is_some() && name != "Edm.Decimal" {
            self.problem(at.clone(), Problem::FacetNotApplicable);
        }
        if property.srid.is_some() && !is_spatial(name) {
            self.problem(at.clone(), Problem::FacetNotApplicable);
        }
        self.annotations(&at, &property.annotations)
    }

    fn navigation(
        &mut self,
        at: &str,
        members: &mut HashSet<&'m str>,
        owner: &'m EntityType,
        base: Option<&'m EntityType>,
        navigation: &'m NavigationProperty,
    ) -> Result<(), CsdlError> {
        self.count()?;
        let at = self.member(at, members, &navigation.name);
        if navigation.ty.collection && navigation.nullable {
            self.problem(at.clone(), Problem::NullableCollectionNavigation);
        }
        let target = self.type_ref(&at, &navigation.ty, Use::Navigation)?;
        let principal = match &target {
            Some(Resolved::Entity(t)) => Some((self.base_of(t), *t)),
            _ => None,
        };
        if let (Some(partner), Some(Resolved::Entity(target))) = (&navigation.partner, target) {
            let back = all_navigations(self.base_of(target), target).find(|n| &n.name == partner);
            let leads_back = back.is_some_and(|back| {
                let to_owner = match self.resolve(&back.ty.name) {
                    // Entity types are compared by address: one model, one type per name.
                    Ok(Resolved::Entity(t)) => {
                        std::ptr::eq(t, owner) || base.is_some_and(|b| std::ptr::eq(t, b))
                    }
                    _ => false,
                };
                to_owner && back.partner.as_ref().is_none_or(|p| p == &navigation.name)
            });
            if !leads_back {
                self.problem(at.clone(), Problem::InvalidPartner);
            }
        } else if navigation.partner.is_some() {
            self.problem(at.clone(), Problem::InvalidPartner);
        }
        for constraint in &navigation.referential_constraints {
            self.count()?;
            if !holds(navigation, (base, owner), principal, constraint) {
                let at = format!("{at}/{}", constraint.property);
                self.problem(at, Problem::InvalidReferentialConstraint);
            }
        }
        self.annotations(&at, &navigation.annotations)
    }

    /// `Precision`: at least 1 on `Edm.Decimal`, 0 to 12 on a temporal type, nowhere else
    /// (CSDL XML 7.2.3).
    fn precision(&mut self, at: &str, ty: &str, precision: Option<u32>) {
        let fits = match precision {
            None => true,
            Some(p) if ty == "Edm.Decimal" => p >= 1,
            Some(p) => is_temporal(ty) && p <= 12,
        };
        if !fits {
            self.problem(at, Problem::FacetNotApplicable);
        }
    }

    fn term(&mut self, schema: &'m Schema, term: &'m Term) -> Result<(), CsdlError> {
        self.count()?;
        let at = qualified(schema, &term.name);
        self.type_ref(&at, &term.ty, Use::Term)?;
        self.precision(&at, &term.ty.name, term.precision);
        for element in &term.applies_to {
            if !ELEMENTS.contains(&element.as_str()) {
                self.problem(at.clone(), Problem::InvalidAppliesTo);
            }
        }
        self.annotations(&at, &term.annotations)
    }

    fn container(
        &mut self,
        schema: &'m Schema,
        container: &'m EntityContainer,
    ) -> Result<(), CsdlError> {
        self.count()?;
        let at = qualified(schema, &container.name);
        let mut names = HashSet::new();
        for set in &container.entity_sets {
            let set_at = format!("{at}/{}", set.name);
            self.identifier(&set_at, &set.name);
            if !names.insert(set.name.as_str()) {
                self.problem(set_at, Problem::Duplicate);
            }
        }
        for set in &container.entity_sets {
            self.entity_set(&at, container, set)?;
        }
        self.annotations(&at, &container.annotations)
    }

    fn entity_set(
        &mut self,
        at: &str,
        container: &'m EntityContainer,
        set: &'m EntitySet,
    ) -> Result<(), CsdlError> {
        self.count()?;
        let at = format!("{at}/{}", set.name);
        let ty = if let Ok(Resolved::Entity(ty)) = self.resolve(&set.entity_type) {
            Some(ty)
        } else {
            self.problem(at.clone(), Problem::UnknownEntitySetType);
            None
        };
        for binding in &set.bindings {
            self.count()?;
            let binding_at = format!("{at}/{}", binding.path);
            if !container
                .entity_sets
                .iter()
                .any(|s| s.name == binding.target)
            {
                self.problem(binding_at.clone(), Problem::UnknownBindingTarget);
            }
            if let Some(ty) = ty {
                if !self.binding_path(ty, &binding.path) {
                    self.problem(binding_at, Problem::InvalidBindingPath);
                }
            }
        }
        self.annotations(&at, &set.annotations)
    }

    /// `nav` or `nav/<ns>.<Type>` where the cast type is the navigation's target or derives
    /// from it (CSDL XML 13.4.1).
    fn binding_path(&self, ty: &'m EntityType, path: &str) -> bool {
        let (nav, cast) = match path.split_once('/') {
            Some((nav, cast)) => (nav, Some(cast)),
            None => (path, None),
        };
        let Some(navigation) = all_navigations(self.base_of(ty), ty).find(|n| n.name == nav) else {
            return false;
        };
        let Some(cast) = cast else { return true };
        let (Ok(Resolved::Entity(target)), Ok(Resolved::Entity(cast_to))) =
            (self.resolve(&navigation.ty.name), self.resolve(cast))
        else {
            return false;
        };
        std::ptr::eq(target, cast_to)
            || self
                .base_of(cast_to)
                .is_some_and(|b| std::ptr::eq(b, target))
    }

    fn base_of(&self, ty: &'m EntityType) -> Option<&'m EntityType> {
        match self.resolve(ty.base_type.as_deref()?) {
            Ok(Resolved::Entity(base)) => Some(base),
            _ => None,
        }
    }

    fn resolve(&self, name: &'m str) -> Result<Resolved<'m>, Problem> {
        if let Some(primitive) = PRIMITIVES.iter().find(|p| **p == name) {
            return Ok(Resolved::Primitive(primitive));
        }
        let (qualifier, simple) = split_qualified(name).ok_or(Problem::InvalidQualifiedName)?;
        if qualifier == "Edm" {
            return Err(Problem::UnknownType);
        }
        let Some(schema) = self
            .qualifiers
            .get(qualifier)
            .ok_or(Problem::UnknownNamespace)?
        else {
            return Ok(Resolved::External);
        };
        if let Some(ty) = schema.entity_types.iter().find(|t| t.name == simple) {
            return Ok(Resolved::Entity(ty));
        }
        if schema.complex_types.iter().any(|t| t.name == simple) {
            return Ok(Resolved::Complex);
        }
        Err(Problem::UnknownType)
    }

    fn type_ref(
        &mut self,
        at: &str,
        ty: &'m TypeRef,
        usage: Use,
    ) -> Result<Option<Resolved<'m>>, CsdlError> {
        if ty.name == "Edm.Stream" {
            return Err(CsdlError::NotImplemented(Feature::StreamProperty));
        }
        let resolved = match self.resolve(&ty.name) {
            Ok(resolved) => resolved,
            Err(problem) => {
                self.problem(at, problem);
                return Ok(None);
            }
        };
        let problem = match (&resolved, usage) {
            (Resolved::Primitive("Edm.Untyped"), _) if self.model.version == CsdlVersion::V4_0 => {
                Some(Problem::UntypedIn40)
            }
            (Resolved::Entity(..), Use::Structural) => Some(Problem::EntityTypedProperty),
            (Resolved::Primitive(_) | Resolved::Complex | Resolved::External, Use::Navigation) => {
                Some(Problem::NotAnEntityType)
            }
            _ => None,
        };
        if let Some(problem) = problem {
            self.problem(at, problem);
        }
        Ok(Some(resolved))
    }

    fn annotations(&mut self, at: &str, annotations: &'m [Annotation]) -> Result<(), CsdlError> {
        for annotation in annotations {
            self.count()?;
            let at = format!("{at}@{}", annotation.term);
            match split_qualified(&annotation.term) {
                None => self.problem(at.clone(), Problem::InvalidQualifiedName),
                Some((qualifier, term)) => match self.qualifiers.get(qualifier) {
                    None => self.problem(at.clone(), Problem::UnknownNamespace),
                    Some(Some(schema)) if !schema.terms.iter().any(|t| t.name == term) => {
                        self.problem(at.clone(), Problem::UnknownTerm);
                    }
                    Some(_) => {}
                },
            }
            if let Some(qualifier) = &annotation.qualifier {
                self.identifier(&at, qualifier);
            }
            self.expr(&at, &annotation.value, 0)?;
        }
        Ok(())
    }

    fn expr(&mut self, at: &str, expr: &'m Expr, depth: usize) -> Result<(), CsdlError> {
        self.count()?;
        match expr {
            Expr::Bool(_) | Expr::Int(_) => {}
            Expr::String(text) => self.text(at, text)?,
            Expr::EnumMember { ty, members } => {
                if split_qualified(ty).is_none()
                    || members.is_empty()
                    || !members.iter().all(|m| is_simple_identifier(m))
                {
                    self.problem(at, Problem::InvalidEnumMember);
                }
            }
            Expr::Path(path) | Expr::PropertyPath(path) | Expr::NavigationPropertyPath(path) => {
                self.text(at, path)?;
                let segment_ok = |s: &str| is_simple_identifier(s) || split_qualified(s).is_some();
                if !path.split('/').all(segment_ok) {
                    self.problem(at, Problem::InvalidPath);
                }
            }
            Expr::Collection(items) => {
                let depth = self.deeper(depth)?;
                for item in items {
                    self.expr(at, item, depth)?;
                }
            }
            Expr::Record(values) => {
                let depth = self.deeper(depth)?;
                for (property, value) in values {
                    let at = format!("{at}/{property}");
                    self.identifier(&at, property);
                    self.expr(&at, value, depth)?;
                }
            }
        }
        Ok(())
    }

    fn deeper(&self, depth: usize) -> Result<usize, CsdlError> {
        let max = self.limits.max_expr_depth;
        depth
            .checked_add(1)
            .filter(|d| *d <= max)
            .ok_or(CsdlError::LimitExceeded {
                limit: LimitKind::ExprDepth,
                max,
            })
    }
}

fn qualified(schema: &Schema, name: &str) -> String {
    format!("{}.{name}", schema.namespace)
}

pub(crate) fn is_temporal(name: &str) -> bool {
    matches!(
        name,
        "Edm.DateTimeOffset" | "Edm.Duration" | "Edm.TimeOfDay"
    )
}

fn is_spatial(name: &str) -> bool {
    name.starts_with("Edm.Geography") || name.starts_with("Edm.Geometry")
}

fn member_names(ty: &EntityType) -> impl Iterator<Item = &str> {
    ty.properties
        .iter()
        .map(|p| p.name.as_str())
        .chain(ty.navigation_properties.iter().map(|n| n.name.as_str()))
}

/// CSDL XML 8.5 on a single-valued navigation: the dependent property is a primitive of the
/// declaring type; the principal one, of the target type, has the same type; the dependent is
/// nullable when the navigation or the principal is, and only then. A target that is no entity
/// type of the model is its own problem already.
fn holds(
    navigation: &NavigationProperty,
    (base, owner): (Option<&EntityType>, &EntityType),
    principal: Option<(Option<&EntityType>, &EntityType)>,
    constraint: &ReferentialConstraint,
) -> bool {
    fn named<'a>(
        base: Option<&'a EntityType>,
        ty: &'a EntityType,
        name: &str,
    ) -> Option<&'a Property> {
        base.into_iter()
            .flat_map(|b| b.properties.iter())
            .chain(ty.properties.iter())
            .find(|p| p.name == name)
    }
    let Some(dependent) = named(base, owner, &constraint.property) else {
        return false;
    };
    if navigation.ty.collection
        || dependent.ty.collection
        || !PRIMITIVES.contains(&dependent.ty.name.as_str())
    {
        return false;
    }
    let Some((base, target)) = principal else {
        return true;
    };
    named(base, target, &constraint.referenced_property).is_some_and(|p| {
        p.ty == dependent.ty && dependent.nullable == (navigation.nullable || p.nullable)
    })
}

fn all_navigations<'a>(
    base: Option<&'a EntityType>,
    ty: &'a EntityType,
) -> impl Iterator<Item = &'a NavigationProperty> {
    base.into_iter()
        .flat_map(|b| b.navigation_properties.iter())
        .chain(ty.navigation_properties.iter())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        ComplexType, EntitySet, Include, NavigationPropertyBinding, Reference, Scale, Srid,
    };

    /// A valid model: an abstract base with the key, two types related both ways, a complex
    /// type, a term, a container with both sets bound, one reference.
    fn valid() -> Model {
        let mut base = EntityType::new("Entity");
        base.is_abstract = true;
        base.key = vec!["id".into()];
        base.properties = vec![Property::new("id", TypeRef::single("Edm.String")).required()];
        let mut a = EntityType::new("A");
        a.base_type = Some("ns.Entity".into());
        a.properties = vec![Property::new("p", TypeRef::single("Edm.Int64"))];
        let mut to_b = NavigationProperty::new("b", TypeRef::single("ns.B"));
        to_b.partner = Some("as".into());
        a.navigation_properties = vec![to_b];
        let mut b = EntityType::new("B");
        b.key = vec!["k".into()];
        b.properties = vec![Property::new("k", TypeRef::single("Edm.Int32")).required()];
        let mut to_a = NavigationProperty::new("as", TypeRef::collection("ns.A"));
        to_a.nullable = false;
        to_a.partner = Some("b".into());
        let mut any = NavigationProperty::new("any", TypeRef::single("ns.Entity"));
        any.nullable = false;
        b.navigation_properties = vec![to_a, any];
        let mut container = EntityContainer::new("C");
        let mut set_a = EntitySet::new("As", "ns.A");
        set_a.bindings = vec![NavigationPropertyBinding {
            path: "b".into(),
            target: "Bs".into(),
        }];
        let mut set_b = EntitySet::new("Bs", "ns.B");
        set_b.bindings = vec![
            NavigationPropertyBinding {
                path: "as".into(),
                target: "As".into(),
            },
            NavigationPropertyBinding {
                path: "any/ns.A".into(),
                target: "As".into(),
            },
        ];
        container.entity_sets = vec![set_a, set_b];
        let mut schema = Schema::new("ns");
        schema.alias = Some("self".into());
        schema.entity_types = vec![base, a, b];
        schema.complex_types = vec![ComplexType::new("Address")];
        schema.terms = vec![Term::new("Kind", TypeRef::single("Edm.String"))];
        schema.entity_container = Some(container);
        Model {
            version: CsdlVersion::V4_01,
            references: vec![Reference {
                uri: "https://example.org/Core.xml".into(),
                includes: vec![Include {
                    namespace: "Org.OData.Core.V1".into(),
                    alias: Some("Core".into()),
                }],
            }],
            schemas: vec![schema],
        }
    }

    fn schema(model: &mut Model) -> &mut Schema {
        &mut model.schemas[0]
    }

    fn ty<'m>(model: &'m mut Model, name: &str) -> &'m mut EntityType {
        schema(model)
            .entity_types
            .iter_mut()
            .find(|t| t.name == name)
            .unwrap()
    }

    fn sets(model: &mut Model) -> &mut Vec<EntitySet> {
        &mut schema(model).entity_container.as_mut().unwrap().entity_sets
    }

    /// The problems `change` causes, in order.
    fn problems(change: impl FnOnce(&mut Model)) -> Vec<Problem> {
        let mut model = valid();
        change(&mut model);
        match validate(&model, &Limits::strict()) {
            Err(CsdlError::Invalid(found)) => found.into_iter().map(|e| e.problem).collect(),
            other => {
                assert!(other.is_ok(), "{other:?}");
                vec![]
            }
        }
    }

    /// The first problem `change` causes; later ones are its consequences.
    fn first(change: impl FnOnce(&mut Model)) -> Problem {
        problems(change)[0]
    }

    fn one(change: impl FnOnce(&mut Model)) -> Problem {
        let found = problems(change);
        assert_eq!(found.len(), 1, "{found:?}");
        found[0]
    }

    #[test]
    fn the_fixture_model_is_valid() {
        assert_eq!(problems(|_| {}), vec![]);
    }

    #[test]
    fn names_must_be_identifiers_and_namespaces() {
        // Every qualified name of the schema stops resolving after it.
        assert_eq!(
            first(|m| schema(m).namespace = "ns..x".into()),
            Problem::InvalidNamespace
        );
        assert_eq!(
            one(|m| schema(m).alias = Some("a.b".into())),
            Problem::InvalidIdentifier
        );
        assert_eq!(
            one(|m| m.references[0].includes[0].namespace = "1x".into()),
            Problem::InvalidNamespace
        );
        assert_eq!(
            one(|m| m.references[0].includes[0].alias = Some("-".into())),
            Problem::InvalidIdentifier
        );
        assert_eq!(
            one(|m| schema(m).complex_types[0].name = "a b".into()),
            Problem::InvalidIdentifier
        );
        assert_eq!(
            one(|m| sets(m).push(EntitySet::new("S-1", "ns.A"))),
            Problem::InvalidIdentifier
        );
        assert_eq!(
            one(|m| ty(m, "A").properties[0].name = "ngsi-ld:p".into()),
            Problem::InvalidIdentifier
        );
    }

    #[test]
    fn names_are_unique_in_their_scope() {
        assert_eq!(
            one(|m| schema(m).terms[0].name = "A".into()),
            Problem::Duplicate
        );
        assert_eq!(
            one(|m| schema(m).alias = Some("ns".into())),
            Problem::Duplicate
        );
        assert_eq!(
            one(|m| m.references[0].includes[0].alias = Some("self".into())),
            Problem::Duplicate
        );
        // A member of the base type, and one of the type itself.
        assert_eq!(
            one(|m| ty(m, "A")
                .properties
                .push(Property::new("id", TypeRef::single("Edm.String")))),
            Problem::Duplicate
        );
        assert_eq!(
            one(|m| ty(m, "A").properties[0].name = "b".into()),
            Problem::Duplicate
        );
        // A navigation property of the base type, and a property of the type itself.
        assert_eq!(
            one(|m| {
                let to_entity = NavigationProperty::new("link", TypeRef::single("ns.Entity"));
                ty(m, "Entity").navigation_properties.push(to_entity);
                ty(m, "A").properties[0].name = "link".into();
            }),
            Problem::Duplicate
        );
        assert_eq!(
            one(|m| {
                let mut copy = sets(m)[1].clone();
                copy.bindings.clear();
                sets(m).push(copy);
            }),
            Problem::Duplicate
        );
    }

    #[test]
    fn type_references_resolve_by_namespace_or_alias() {
        assert_eq!(
            problems(|m| ty(m, "A").properties[0].ty = TypeRef::single("self.Address")),
            vec![]
        );
        assert_eq!(
            problems(|m| ty(m, "A").properties[0].ty = TypeRef::single("Core.Tag")),
            vec![]
        );
        assert_eq!(
            one(|m| ty(m, "A").properties[0].ty = TypeRef::single("Edm.Text")),
            Problem::UnknownType
        );
        assert_eq!(
            one(|m| ty(m, "A").properties[0].ty = TypeRef::single("ns.Nope")),
            Problem::UnknownType
        );
        assert_eq!(
            one(|m| ty(m, "A").properties[0].ty = TypeRef::single("other.T")),
            Problem::UnknownNamespace
        );
        assert_eq!(
            one(|m| ty(m, "A").properties[0].ty = TypeRef::single("String")),
            Problem::InvalidQualifiedName
        );
        assert_eq!(
            one(|m| ty(m, "A").properties[0].ty = TypeRef::single("ns.B")),
            Problem::EntityTypedProperty
        );
        // Neither side of the partnership leads back any more.
        assert_eq!(
            problems(|m| ty(m, "A").navigation_properties[0].ty = TypeRef::single("ns.Address")),
            vec![
                Problem::NotAnEntityType,
                Problem::InvalidPartner,
                Problem::InvalidPartner
            ]
        );
        // The binding `any/ns.A` casts from a target that is no entity type any more.
        for target in ["Core.Tag", "Edm.String"] {
            assert_eq!(
                problems(|m| ty(m, "B").navigation_properties[1].ty = TypeRef::single(target)),
                vec![Problem::NotAnEntityType, Problem::InvalidBindingPath]
            );
        }
        assert_eq!(
            problems(|m| ty(m, "A").navigation_properties[0].ty = TypeRef::single("ns.Nope")),
            vec![
                Problem::UnknownType,
                Problem::InvalidPartner,
                Problem::InvalidPartner
            ]
        );
    }

    #[test]
    fn untyped_needs_4_01() {
        let untyped = |m: &mut Model| ty(m, "A").properties[0].ty = TypeRef::single("Edm.Untyped");
        assert_eq!(problems(untyped), vec![]);
        assert_eq!(
            one(|m| {
                untyped(m);
                m.version = CsdlVersion::V4_0;
            }),
            Problem::UntypedIn40
        );
    }

    #[test]
    fn facets_apply_to_their_types_only() {
        let facet = |ty_name: &str, change: fn(&mut Property)| {
            problems(|m| {
                let p = &mut ty(m, "A").properties[0];
                p.ty = TypeRef::single(ty_name);
                change(p);
            })
        };
        assert_eq!(
            facet("Edm.Decimal", |p| p.scale = Some(Scale::Floating)),
            vec![]
        );
        assert_eq!(facet("Edm.Decimal", |p| p.precision = Some(1)), vec![]);
        assert_eq!(
            facet("Edm.GeometryPolygon", |p| p.srid = Some(Srid::Variable)),
            vec![]
        );
        assert_eq!(facet("Edm.TimeOfDay", |p| p.precision = Some(12)), vec![]);
        assert_eq!(facet("Edm.Duration", |p| p.precision = Some(0)), vec![]);
        let bad = vec![Problem::FacetNotApplicable];
        assert_eq!(
            facet("Edm.Double", |p| p.scale = Some(Scale::Digits(2))),
            bad
        );
        assert_eq!(facet("Edm.String", |p| p.srid = Some(Srid::Id(4326))), bad);
        assert_eq!(facet("Edm.Decimal", |p| p.precision = Some(0)), bad);
        assert_eq!(facet("Edm.DateTimeOffset", |p| p.precision = Some(13)), bad);
        assert_eq!(facet("Edm.Int64", |p| p.precision = Some(3)), bad);
        assert_eq!(
            one(|m| {
                let term = &mut schema(m).terms[0];
                term.precision = Some(3);
            }),
            Problem::FacetNotApplicable
        );
    }

    #[test]
    fn keys_are_declared_once_on_non_nullable_primitives() {
        assert_eq!(one(|m| ty(m, "B").key.clear()), Problem::MissingKey);
        assert_eq!(problems(|m| ty(m, "Entity").key.clear()), vec![]);
        assert_eq!(
            problems(|m| ty(m, "A").key = vec!["p".into()]),
            vec![Problem::KeyOnDerivedType, Problem::InvalidKeyProperty]
        );
        assert_eq!(
            one(|m| ty(m, "B").key = vec!["nope".into()]),
            Problem::KeyNotFound
        );
        assert_eq!(
            one(|m| ty(m, "B").properties[0].nullable = true),
            Problem::InvalidKeyProperty
        );
        assert_eq!(
            one(|m| ty(m, "B").properties[0].ty = TypeRef::collection("Edm.Int32")),
            Problem::InvalidKeyProperty
        );
        assert_eq!(
            one(|m| ty(m, "B").properties[0].ty = TypeRef::single("Edm.Double")),
            Problem::InvalidKeyProperty
        );
    }

    #[test]
    fn derivation_stops_at_one_abstract_base() {
        // Without its base, A no longer derives from the target `any/ns.A` casts from.
        assert_eq!(
            problems(|m| ty(m, "A").base_type = Some("ns.Nope".into())),
            vec![Problem::BaseTypeNotFound, Problem::InvalidBindingPath]
        );
        assert_eq!(
            first(|m| ty(m, "A").base_type = Some("ns.Address".into())),
            Problem::BaseTypeNotFound
        );
        for change in [
            (|m: &mut Model| ty(m, "Entity").is_abstract = false) as fn(&mut Model),
            |m| ty(m, "A").base_type = Some("ns.B".into()),
            |m| {
                let mut deeper = EntityType::new("Deeper");
                deeper.is_abstract = true;
                deeper.base_type = Some("ns.Entity".into());
                schema(m).entity_types.push(deeper);
                let mut leaf = EntityType::new("Leaf");
                leaf.base_type = Some("ns.Deeper".into());
                schema(m).entity_types.push(leaf);
            },
        ] {
            let mut model = valid();
            change(&mut model);
            assert!(matches!(
                validate(&model, &Limits::strict()),
                Err(CsdlError::NotImplemented(Feature::DerivedType))
            ));
        }
    }

    #[test]
    fn stream_properties_are_not_implemented() {
        let mut model = valid();
        ty(&mut model, "A").properties[0].ty = TypeRef::single("Edm.Stream");
        assert!(matches!(
            validate(&model, &Limits::strict()),
            Err(CsdlError::NotImplemented(Feature::StreamProperty))
        ));
    }

    #[test]
    fn navigation_partners_lead_back() {
        assert_eq!(
            one(|m| ty(m, "B").navigation_properties[0].nullable = true),
            Problem::NullableCollectionNavigation
        );
        // A partner that is not there, and the other side, whose partner no longer answers.
        assert_eq!(
            problems(|m| ty(m, "A").navigation_properties[0].partner = Some("nope".into())),
            vec![Problem::InvalidPartner, Problem::InvalidPartner]
        );
        // `any` leads back to A's base, which is enough for A; `as` loses its answer.
        assert_eq!(
            problems(|m| ty(m, "A").navigation_properties[0].partner = Some("any".into())),
            vec![Problem::InvalidPartner]
        );
        assert_eq!(
            problems(|m| ty(m, "B").navigation_properties[0].partner = Some("p".into())),
            vec![Problem::InvalidPartner, Problem::InvalidPartner]
        );
        assert_eq!(
            problems(|m| ty(m, "B").navigation_properties[1].partner = Some("b".into())),
            vec![Problem::InvalidPartner]
        );
        assert_eq!(
            problems(|m| {
                ty(m, "A").navigation_properties[0].ty = TypeRef::single("ns.Nope");
                ty(m, "A").navigation_properties[0].partner = Some("as".into());
            }),
            vec![
                Problem::UnknownType,
                Problem::InvalidPartner,
                Problem::InvalidPartner
            ]
        );
    }

    #[test]
    fn referential_constraints_pair_like_properties() {
        let constrained = |m: &mut Model, dependent: Property, property: &str, referenced: &str| {
            ty(m, "A").properties.push(dependent);
            ty(m, "A").navigation_properties[0].referential_constraints =
                vec![ReferentialConstraint::new(property, referenced)];
        };
        let bk = || Property::new("bk", TypeRef::single("Edm.Int32"));
        assert_eq!(problems(|m| constrained(m, bk(), "bk", "k")), vec![]);
        // A target that is no entity type is that problem alone.
        assert_eq!(
            problems(|m| {
                constrained(m, bk(), "bk", "k");
                ty(m, "A").navigation_properties[0].ty = TypeRef::single("ns.Address");
                ty(m, "A").navigation_properties[0].partner = None;
                ty(m, "B").navigation_properties[0].partner = None;
            }),
            vec![Problem::NotAnEntityType]
        );
        let bad = Problem::InvalidReferentialConstraint;
        for (dependent, property, referenced) in [
            (bk(), "nope", "k"),
            (bk(), "bk", "nope"),
            // p is an Int64, k an Int32
            (bk(), "p", "k"),
            // the navigation is nullable, so the dependent must be
            (bk().required(), "bk", "k"),
            (
                Property::new("bks", TypeRef::collection("Edm.Int32")),
                "bks",
                "k",
            ),
            (
                Property::new("at", TypeRef::single("ns.Address")),
                "at",
                "k",
            ),
        ] {
            assert_eq!(
                problems(|m| constrained(m, dependent, property, referenced)),
                vec![bad],
                "{property} -> {referenced}"
            );
        }
        // Both sides non-nullable: the dependent may not be nullable either.
        assert_eq!(
            problems(|m| {
                constrained(m, bk(), "bk", "k");
                ty(m, "A").navigation_properties[0].nullable = false;
            }),
            vec![bad]
        );
        assert_eq!(
            problems(|m| {
                constrained(m, bk().required(), "bk", "k");
                ty(m, "A").navigation_properties[0].nullable = false;
            }),
            vec![]
        );
        // The dependent may be the base's; a collection-valued navigation takes none.
        assert_eq!(
            first(|m| {
                ty(m, "B").navigation_properties[0].referential_constraints =
                    vec![ReferentialConstraint::new("k", "id")];
            }),
            bad
        );
        assert_eq!(
            problems(|m| {
                ty(m, "A")
                    .properties
                    .push(Property::new("p2", TypeRef::single("Edm.String")));
                let mut to_any = NavigationProperty::new("e", TypeRef::single("ns.Entity"));
                to_any.referential_constraints = vec![ReferentialConstraint::new("id", "id")];
                ty(m, "A").navigation_properties.push(to_any);
            }),
            vec![bad],
            "id is the base's and not nullable, the navigation is"
        );
    }

    #[test]
    fn bindings_name_a_navigation_and_a_set() {
        assert_eq!(
            one(|m| sets(m)[0].bindings[0].target = "Nope".into()),
            Problem::UnknownBindingTarget
        );
        assert_eq!(
            one(|m| sets(m)[0].bindings[0].path = "p".into()),
            Problem::InvalidBindingPath
        );
        assert_eq!(
            one(|m| sets(m)[1].bindings[1].path = "any/ns.B".into()),
            Problem::InvalidBindingPath
        );
        assert_eq!(
            problems(|m| sets(m)[1].bindings[1].path = "any/ns.Entity".into()),
            vec![]
        );
        assert_eq!(
            one(|m| sets(m)[1].bindings[1].path = "any/ns.Nope".into()),
            Problem::InvalidBindingPath
        );
        assert_eq!(
            one(|m| sets(m)[1].bindings[1].path = "any/ns.A/x".into()),
            Problem::InvalidBindingPath
        );
        assert_eq!(
            one(|m| sets(m)[0].entity_type = "ns.Address".into()),
            Problem::UnknownEntitySetType
        );
    }

    #[test]
    fn a_model_has_one_container() {
        assert_eq!(
            one(|m| {
                let mut other = Schema::new("other");
                other.entity_container = Some(EntityContainer::new("C2"));
                m.schemas.push(other);
            }),
            Problem::SecondContainer
        );
    }

    #[test]
    fn references_and_strings_are_checked() {
        assert_eq!(
            one(|m| m.references[0].includes.clear()),
            Problem::EmptyReference
        );
        assert_eq!(
            one(|m| m.references[0].uri = "a\u{0}b".into()),
            Problem::InvalidCharacter
        );
        let string = |text: &'static str| {
            problems(move |m| {
                schema(m).annotations =
                    vec![Annotation::new("self.Kind", Expr::String(text.into()))];
            })
        };
        assert_eq!(string("tab\tline\nreturn\r"), vec![]);
        assert_eq!(string("\u{FFFE}"), vec![Problem::InvalidCharacter]);
        assert_eq!(string("\u{FFFF}"), vec![Problem::InvalidCharacter]);
        assert_eq!(string("\u{1F}"), vec![Problem::InvalidCharacter]);
    }

    #[test]
    fn terms_apply_to_csdl_elements() {
        assert_eq!(
            problems(
                |m| schema(m).terms[0].applies_to = vec!["Property".into(), "EntitySet".into()]
            ),
            vec![]
        );
        assert_eq!(
            one(|m| schema(m).terms[0].applies_to = vec!["Entity".into()]),
            Problem::InvalidAppliesTo
        );
    }

    #[test]
    fn annotations_name_known_terms() {
        let annotate = |term: &'static str, value: Expr, qualifier: Option<&'static str>| {
            problems(move |m| {
                let mut a = Annotation::new(term, value);
                a.qualifier = qualifier.map(Into::into);
                ty(m, "A").annotations = vec![a];
            })
        };
        assert_eq!(
            annotate("Core.Computed", Expr::Bool(true), Some("q1")),
            vec![]
        );
        assert_eq!(annotate("self.Kind", Expr::Int(1), None), vec![]);
        assert_eq!(
            annotate("Computed", Expr::Bool(true), None),
            vec![Problem::InvalidQualifiedName]
        );
        assert_eq!(
            annotate("Nope.Computed", Expr::Bool(true), None),
            vec![Problem::UnknownNamespace]
        );
        assert_eq!(
            annotate("ns.Nope", Expr::Bool(true), None),
            vec![Problem::UnknownTerm]
        );
        assert_eq!(
            annotate("Core.Computed", Expr::Bool(true), Some("a b")),
            vec![Problem::InvalidIdentifier]
        );
    }

    #[test]
    fn annotation_values_are_checked() {
        let value = |value: Expr| {
            problems(move |m| {
                ty(m, "A").properties[0].annotations = vec![Annotation::new("self.Kind", value)];
            })
        };
        let member = |ty: &str, members: &[&str]| Expr::EnumMember {
            ty: ty.into(),
            members: members.iter().map(|m| (*m).to_owned()).collect(),
        };
        assert_eq!(value(member("Core.Permission", &["Read", "Write"])), vec![]);
        assert_eq!(
            value(member("Permission", &["Read"])),
            vec![Problem::InvalidEnumMember]
        );
        assert_eq!(
            value(member("Core.Permission", &[])),
            vec![Problem::InvalidEnumMember]
        );
        assert_eq!(
            value(member("Core.Permission", &["Re ad"])),
            vec![Problem::InvalidEnumMember]
        );
        for path in [Expr::Path, Expr::PropertyPath, Expr::NavigationPropertyPath] {
            assert_eq!(value(path("b/ns.B/k".into())), vec![]);
            assert_eq!(value(path(String::new())), vec![Problem::InvalidPath]);
            assert_eq!(value(path("a//b".into())), vec![Problem::InvalidPath]);
        }
        assert_eq!(
            value(Expr::Collection(vec![Expr::Record(vec![(
                "Value".into(),
                Expr::Bool(true)
            )])])),
            vec![]
        );
        assert_eq!(
            value(Expr::Record(vec![("not ok".into(), Expr::Bool(true))])),
            vec![Problem::InvalidIdentifier]
        );
    }

    fn limited(limits: Limits, change: impl FnOnce(&mut Model)) -> Option<LimitKind> {
        let mut model = valid();
        change(&mut model);
        limit_hit(&limits, &model)
    }

    /// The limit `model` exceeds, if any; not generic, so one copy sees every arm.
    fn limit_hit(limits: &Limits, model: &Model) -> Option<LimitKind> {
        match validate(model, limits) {
            Err(CsdlError::LimitExceeded { limit, max }) => {
                let expected = match limit {
                    LimitKind::Elements => limits.max_elements,
                    LimitKind::ExprDepth => limits.max_expr_depth,
                    LimitKind::StringLen => limits.max_string_len,
                };
                assert_eq!(max, expected);
                Some(limit)
            }
            other => {
                assert!(other.is_ok(), "{other:?}");
                None
            }
        }
    }

    /// The fixture's element count: 1 reference, 1 include, 1 schema, 3 entity types, 1 complex
    /// type, 3 properties, 3 navigations, 1 term, the container, 2 sets, 3 bindings.
    const ELEMENTS: usize = 20;

    #[test]
    fn every_limit_answers_at_limit_plus_one() {
        let at = |max_elements| Limits {
            max_elements,
            ..Limits::strict()
        };
        assert_eq!(limited(at(ELEMENTS), |_| {}), None);
        assert_eq!(limited(at(ELEMENTS - 1), |_| {}), Some(LimitKind::Elements));
        assert_eq!(limited(at(usize::MAX), |_| {}), None);

        let nested = |depth: usize| {
            let mut value = Expr::Bool(true);
            for _ in 0..depth {
                value = Expr::Collection(vec![value]);
            }
            move |m: &mut Model| schema(m).annotations = vec![Annotation::new("self.Kind", value)]
        };
        let depth = Limits {
            max_expr_depth: 2,
            ..Limits::strict()
        };
        assert_eq!(limited(depth, nested(2)), None);
        assert_eq!(limited(depth, nested(3)), Some(LimitKind::ExprDepth));
        let record = |m: &mut Model| {
            let inner = Expr::Record(vec![("V".into(), Expr::Record(vec![]))]);
            schema(m).annotations = vec![Annotation::new(
                "self.Kind",
                Expr::Record(vec![("V".into(), inner)]),
            )];
        };
        assert_eq!(limited(depth, record), Some(LimitKind::ExprDepth));

        let string = Limits {
            max_string_len: 4,
            ..Limits::strict()
        };
        let text = |text: &'static str| {
            move |m: &mut Model| {
                m.references[0].uri = "u".into();
                schema(m).annotations =
                    vec![Annotation::new("self.Kind", Expr::String(text.into()))];
            }
        };
        assert_eq!(limited(string, text("four")), None);
        assert_eq!(limited(string, text("fiver")), Some(LimitKind::StringLen));
        assert_eq!(
            limited(string, |m| m.references[0].uri = "u".repeat(5)),
            Some(LimitKind::StringLen)
        );
        assert_eq!(
            limited(string, |m| {
                m.references[0].uri = "u".into();
                schema(m).annotations =
                    vec![Annotation::new("self.Kind", Expr::Path("abcde".into()))];
            }),
            Some(LimitKind::StringLen)
        );
    }

    #[test]
    fn strict_limits_are_positive() {
        let strict = Limits::strict();
        assert!(strict.max_elements > ELEMENTS);
        assert!(strict.max_expr_depth > 0);
        assert!(strict.max_string_len > 0);
    }
}
