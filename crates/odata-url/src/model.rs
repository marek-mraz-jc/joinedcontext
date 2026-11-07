// SPDX-License-Identifier: EUPL-1.2
//! What the parser asks about names. The OData URL grammar depends on the model: after
//! `Orders` a `(1)` is a key only when `Orders` is a collection, and `/Customer/` continues with
//! the members of the customer's type. The parser asks as it goes, so it parses one pass and
//! answers an unknown name where it stands.

/// Names of a service, as the parser needs them. Types are named by any string the
/// implementation chooses; the parser only hands them back.
///
/// ```
/// use odata_url::{parse, Limits, Member, Model};
///
/// // One entity set, `Sensors`, whose entities have a `value`.
/// struct Sensors;
/// impl Model for Sensors {
///     fn entity_set(&self, name: &str) -> Option<&str> {
///         (name == "Sensors").then_some("Sensor")
///     }
///     fn member(&self, ty: &str, name: &str) -> Option<Member<'_>> {
///         (ty == "Sensor" && name == "value").then_some(Member::Primitive { collection: false })
///     }
/// }
/// assert!(parse("Sensors?$filter=value gt 1", &Sensors, &Limits::strict()).is_ok());
/// assert!(parse("Sensors?$filter=level gt 1", &Sensors, &Limits::strict()).is_err());
/// ```
pub trait Model {
    /// The entity type of the entity set `name`, if there is one.
    fn entity_set(&self, name: &str) -> Option<&str>;

    /// The member `name` of the structured type `ty`, if there is one.
    fn member(&self, ty: &str, name: &str) -> Option<Member<'_>>;
}

/// What a member of a structured type is.
///
/// ```
/// use odata_url::Member;
/// let rooms = Member::Complex { ty: "Room", collection: true };
/// assert!(matches!(rooms, Member::Complex { collection: true, .. }));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Member<'m> {
    /// A primitive property, or a collection of primitives.
    Primitive {
        /// A collection.
        collection: bool,
    },
    /// A complex property of type `ty`, or a collection of them.
    Complex {
        /// Its complex type.
        ty: &'m str,
        /// A collection.
        collection: bool,
    },
    /// A navigation property to entities of type `ty`.
    Navigation {
        /// The target entity type.
        ty: &'m str,
        /// Collection-valued.
        collection: bool,
    },
}
