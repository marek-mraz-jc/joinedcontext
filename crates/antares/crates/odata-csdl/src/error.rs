// SPDX-License-Identifier: EUPL-1.2
//! What validation and the writers answer instead of a document.

use std::fmt;

/// Why a model cannot be written.
///
/// ```
/// use odata_csdl::{CsdlError, Feature};
/// let e = CsdlError::NotImplemented(Feature::CsdlJson40);
/// assert!(e.to_string().contains("not implemented"));
/// ```
#[derive(Debug)]
pub enum CsdlError {
    /// The model breaks one or more CSDL rules; every problem found is listed.
    Invalid(Vec<ValidationError>),
    /// The model is larger than the caller's [`crate::Limits`] allow.
    LimitExceeded {
        /// Which limit.
        limit: LimitKind,
        /// Its value.
        max: usize,
    },
    /// The model asks for something outside the subset of ADR-0024 decision 5.
    NotImplemented(Feature),
    /// Writing to the sink failed.
    Io(std::io::Error),
}

impl fmt::Display for CsdlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(problems) => {
                write!(f, "invalid CSDL model ({} problems)", problems.len())?;
                for problem in problems {
                    write!(f, "; {problem}")?;
                }
                Ok(())
            }
            Self::LimitExceeded { limit, max } => {
                write!(f, "limit exceeded: {} (max {max})", limit.as_str())
            }
            Self::NotImplemented(feature) => write!(f, "not implemented: {}", feature.as_str()),
            Self::Io(err) => write!(f, "write failed: {err}"),
        }
    }
}

impl std::error::Error for CsdlError {}

impl From<std::io::Error> for CsdlError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

/// One limit of [`crate::Limits`].
///
/// ```
/// assert_eq!(odata_csdl::LimitKind::ExprDepth.as_str(), "annotation expression depth");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitKind {
    /// Model elements in total.
    Elements,
    /// Nesting of collection and record expressions.
    ExprDepth,
    /// Bytes of one string constant or reference URI.
    StringLen,
}

impl LimitKind {
    /// A fixed description.
    ///
    /// ```
    /// assert_eq!(odata_csdl::LimitKind::Elements.as_str(), "model elements");
    /// ```
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Elements => "model elements",
            Self::ExprDepth => "annotation expression depth",
            Self::StringLen => "string length",
        }
    }
}

/// A construct outside the subset (ADR-0024 decision 5), answered as 501 by a service.
///
/// ```
/// assert_eq!(odata_csdl::Feature::StreamProperty.as_str(), "stream properties");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    /// A base type that is not abstract, or that itself derives from another.
    DerivedType,
    /// A property of type `Edm.Stream`.
    StreamProperty,
    /// CSDL JSON of a 4.0 model: the JSON representation is 4.01 only.
    CsdlJson40,
}

impl Feature {
    /// A fixed description.
    ///
    /// ```
    /// assert_eq!(odata_csdl::Feature::CsdlJson40.as_str(), "CSDL JSON for OData 4.0");
    /// ```
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DerivedType => "derived types beyond one abstract base",
            Self::StreamProperty => "stream properties",
            Self::CsdlJson40 => "CSDL JSON for OData 4.0",
        }
    }
}

/// One broken rule, and the model element it is on.
///
/// ```
/// use odata_csdl::{Problem, ValidationError};
/// let e = ValidationError { at: "ns.T/p".into(), problem: Problem::UnknownType };
/// assert_eq!(e.to_string(), "ns.T/p: unknown type");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    /// The element: `namespace`, `namespace.Type`, `namespace.Type/member`, and so on.
    pub at: String,
    /// The rule.
    pub problem: Problem,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.at, self.problem.as_str())
    }
}

/// The CSDL rules [`validate`](fn@crate::validate) checks.
///
/// ```
/// assert_eq!(odata_csdl::Problem::MissingKey.as_str(), "entity type without a key");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    /// Not a simple identifier (CSDL XML 15.2).
    InvalidIdentifier,
    /// Not a namespace: dotted simple identifiers, at most 511 characters.
    InvalidNamespace,
    /// Not `<namespace or alias>.<simple identifier>`.
    InvalidQualifiedName,
    /// The name is taken already in the same scope.
    Duplicate,
    /// The qualifier names no schema and no included namespace.
    UnknownNamespace,
    /// The type is neither a primitive nor a type of the model.
    UnknownType,
    /// The annotation's term is not defined in the schema its qualifier names.
    UnknownTerm,
    /// A structural property typed with an entity type.
    EntityTypedProperty,
    /// A navigation property whose type is not an entity type of the model.
    NotAnEntityType,
    /// `Edm.Untyped` in a 4.0 model.
    UntypedIn40,
    /// `Scale` on a type other than `Edm.Decimal`, or `SRID` on a non-spatial type.
    FacetNotApplicable,
    /// A concrete entity type with neither a key nor a base type.
    MissingKey,
    /// A key on a type that also has a base type.
    KeyOnDerivedType,
    /// A key names no property of the type.
    KeyNotFound,
    /// A key property that is nullable, a collection, or not primitive.
    InvalidKeyProperty,
    /// The base type is not an entity type of the model.
    BaseTypeNotFound,
    /// A collection-valued navigation property marked nullable.
    NullableCollectionNavigation,
    /// The partner is not a navigation property of the target leading back.
    InvalidPartner,
    /// A referential constraint on a collection-valued navigation, or whose properties are
    /// missing, not primitive, of different types or of a nullability CSDL XML 8.5 forbids.
    InvalidReferentialConstraint,
    /// A binding path names no navigation property of the set's type, or casts to a type that
    /// does not derive from its target.
    InvalidBindingPath,
    /// A binding target names no entity set of the container.
    UnknownBindingTarget,
    /// The entity set's type is not an entity type of the model.
    UnknownEntitySetType,
    /// A second entity container in the model.
    SecondContainer,
    /// A string that XML 1.0 cannot carry (a control character, U+FFFE or U+FFFF).
    InvalidCharacter,
    /// An enum member value without members, or with a member that is not an identifier.
    InvalidEnumMember,
    /// A path expression that is empty or has a segment that is not an identifier.
    InvalidPath,
    /// An `AppliesTo` entry that is no CSDL element name.
    InvalidAppliesTo,
    /// A reference without includes.
    EmptyReference,
}

impl Problem {
    /// A fixed description.
    ///
    /// ```
    /// assert_eq!(odata_csdl::Problem::Duplicate.as_str(), "name already taken");
    /// ```
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidIdentifier => "not a simple identifier",
            Self::InvalidNamespace => "not a namespace",
            Self::InvalidQualifiedName => "not a qualified name",
            Self::Duplicate => "name already taken",
            Self::UnknownNamespace => "unknown namespace or alias",
            Self::UnknownType => "unknown type",
            Self::UnknownTerm => "unknown term",
            Self::EntityTypedProperty => "structural property of an entity type",
            Self::NotAnEntityType => "navigation to a type that is not an entity type",
            Self::UntypedIn40 => "Edm.Untyped needs OData 4.01",
            Self::FacetNotApplicable => "facet does not apply to the type",
            Self::MissingKey => "entity type without a key",
            Self::KeyOnDerivedType => "key on a derived entity type",
            Self::KeyNotFound => "key names no property of the type",
            Self::InvalidKeyProperty => "key property must be a non-nullable primitive",
            Self::BaseTypeNotFound => "base type is not an entity type of the model",
            Self::NullableCollectionNavigation => "collection-valued navigation marked nullable",
            Self::InvalidPartner => "partner does not lead back",
            Self::InvalidReferentialConstraint => "referential constraint does not hold",
            Self::InvalidBindingPath => "binding path is not a navigation of the set's type",
            Self::UnknownBindingTarget => "binding target is not an entity set",
            Self::UnknownEntitySetType => "entity set type is not an entity type of the model",
            Self::SecondContainer => "a model has one entity container",
            Self::InvalidCharacter => "character XML cannot carry",
            Self::InvalidEnumMember => "invalid enum member",
            Self::InvalidPath => "invalid path",
            Self::InvalidAppliesTo => "invalid AppliesTo entry",
            Self::EmptyReference => "reference without includes",
        }
    }
}
