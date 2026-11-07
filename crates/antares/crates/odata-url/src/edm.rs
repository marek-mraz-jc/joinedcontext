// SPDX-License-Identifier: EUPL-1.2
//! [`Model`] over an [`odata_csdl::Model`]: names resolved the way the `$metadata` document of
//! the same service declares them.

use odata_csdl::{ComplexType, EntityType, Property};

use crate::{Member, Model};

/// An [`odata_csdl::Model`] the parser resolves names against. Types are the qualified names the
/// model writes (`Namespace.Name` or `Alias.Name`); an entity type's members include those of
/// its base types.
///
/// ```
/// use odata_csdl::{CsdlVersion, EntityContainer, EntitySet, EntityType, Property, Schema, TypeRef};
/// use odata_url::{parse, EdmModel, Limits, ODataUri};
///
/// let mut schema = Schema::new("City");
/// let mut building = EntityType::new("Building");
/// building.key.push("id".into());
/// building.properties.push(Property::new("id", TypeRef::single("Edm.String")).required());
/// building.properties.push(Property::new("height", TypeRef::single("Edm.Int32")));
/// schema.entity_types.push(building);
/// let mut container = EntityContainer::new("Service");
/// container.entity_sets.push(EntitySet::new("Buildings", "City.Building"));
/// schema.entity_container = Some(container);
/// let model = odata_csdl::Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![schema] };
///
/// let uri = parse("Buildings?$filter=height gt 10", &EdmModel(&model), &Limits::strict())?;
/// assert!(matches!(uri, ODataUri::Resource { .. }));
/// assert!(parse("Buildings?$filter=width gt 10", &EdmModel(&model), &Limits::strict()).is_err());
/// # Ok::<(), odata_url::UrlError>(())
/// ```
#[derive(Debug, Clone, Copy)]
pub struct EdmModel<'m>(pub &'m odata_csdl::Model);

/// A structured type found by its qualified name.
enum Structured<'m> {
    Entity(&'m EntityType),
    Complex(&'m ComplexType),
}

impl<'m> EdmModel<'m> {
    /// The entity or complex type `type_name` names, by namespace or alias.
    fn structured(self, type_name: &str) -> Option<Structured<'m>> {
        let (qualifier, name) = type_name.rsplit_once('.')?;
        let schema = self
            .0
            .schemas
            .iter()
            .find(|s| s.namespace == qualifier || s.alias.as_deref() == Some(qualifier))?;
        if let Some(entity) = schema.entity_types.iter().find(|t| t.name == name) {
            return Some(Structured::Entity(entity));
        }
        schema
            .complex_types
            .iter()
            .find(|t| t.name == name)
            .map(Structured::Complex)
    }

    /// A property as a member: complex when its type is a complex type here.
    fn property(self, property: &'m Property) -> Member<'m> {
        let collection = property.ty.collection;
        match self.structured(&property.ty.name) {
            Some(Structured::Complex(_)) => Member::Complex {
                ty: &property.ty.name,
                collection,
            },
            _ => Member::Primitive { collection },
        }
    }
}

impl Model for EdmModel<'_> {
    fn entity_set(&self, name: &str) -> Option<&str> {
        self.0
            .schemas
            .iter()
            .filter_map(|s| s.entity_container.as_ref())
            .flat_map(|c| &c.entity_sets)
            .find(|set| set.name == name)
            .map(|set| set.entity_type.as_str())
    }

    fn member(&self, ty: &str, name: &str) -> Option<Member<'_>> {
        let mut entity = match self.structured(ty)? {
            Structured::Complex(complex) => {
                return complex
                    .properties
                    .iter()
                    .find(|p| p.name == name)
                    .map(|p| self.property(p));
            }
            Structured::Entity(entity) => entity,
        };
        // A base-type cycle is a validation error of the model; the walk stops after as many
        // steps as there are entity types.
        let types: usize = self.0.schemas.iter().map(|s| s.entity_types.len()).sum();
        for _ in 0..types {
            if let Some(p) = entity.properties.iter().find(|p| p.name == name) {
                return Some(self.property(p));
            }
            if let Some(nav) = entity.navigation_properties.iter().find(|n| n.name == name) {
                return Some(Member::Navigation {
                    ty: &nav.ty.name,
                    collection: nav.ty.collection,
                });
            }
            match self.structured(entity.base_type.as_deref()?)? {
                Structured::Entity(base) => entity = base,
                Structured::Complex(_) => return None,
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use odata_csdl::{
        ComplexType, CsdlVersion, EntityContainer, EntitySet, EntityType, NavigationProperty,
        Property, Schema, TypeRef,
    };

    use super::EdmModel;
    use crate::{Member, Model};

    /// `City` (alias `c`): Place, Building from Place, Tower from `c.Building`, a cycle
    /// Loop from Loop, Odd from the complex Address; `Other` holds nothing.
    fn city() -> odata_csdl::Model {
        let mut city = Schema::new("City");
        city.alias = Some("c".into());
        let mut place = EntityType::new("Place");
        place
            .properties
            .push(Property::new("id", TypeRef::single("Edm.String")));
        place
            .properties
            .push(Property::new("address", TypeRef::single("City.Address")));
        place
            .properties
            .push(Property::new("tags", TypeRef::collection("Edm.String")));
        let mut building = EntityType::new("Building");
        building.base_type = Some("City.Place".into());
        building
            .properties
            .push(Property::new("homes", TypeRef::collection("c.Address")));
        building.navigation_properties.push(NavigationProperty::new(
            "owner",
            TypeRef::single("City.Person"),
        ));
        building.navigation_properties.push(NavigationProperty::new(
            "parts",
            TypeRef::collection("City.Place"),
        ));
        let mut tower = EntityType::new("Tower");
        tower.base_type = Some("c.Building".into());
        let mut looped = EntityType::new("Loop");
        looped.base_type = Some("City.Loop".into());
        let mut odd = EntityType::new("Odd");
        odd.base_type = Some("City.Address".into());
        let mut lost = EntityType::new("Lost");
        lost.base_type = Some("Nowhere.Thing".into());
        city.entity_types = vec![place, building, tower, looped, odd, lost];
        let mut address = ComplexType::new("Address");
        address
            .properties
            .push(Property::new("street", TypeRef::single("Edm.String")));
        city.complex_types.push(address);
        let mut service = EntityContainer::new("Service");
        service
            .entity_sets
            .push(EntitySet::new("Towers", "City.Tower"));
        city.entity_container = Some(service);
        odata_csdl::Model {
            version: CsdlVersion::V4_01,
            references: vec![],
            schemas: vec![Schema::new("Other"), city],
        }
    }

    #[test]
    fn names_resolve_through_aliases_and_base_types() {
        let model = city();
        let edm = EdmModel(&model);
        assert_eq!(edm.entity_set("Towers"), Some("City.Tower"));
        assert_eq!(edm.entity_set("Places"), None);
        let single = Member::Primitive { collection: false };
        assert_eq!(edm.member("City.Tower", "id"), Some(single));
        assert_eq!(
            edm.member("c.Tower", "tags"),
            Some(Member::Primitive { collection: true })
        );
        assert_eq!(
            edm.member("City.Tower", "address"),
            Some(Member::Complex {
                ty: "City.Address",
                collection: false
            })
        );
        assert_eq!(
            edm.member("City.Building", "homes"),
            Some(Member::Complex {
                ty: "c.Address",
                collection: true
            })
        );
        assert_eq!(
            edm.member("City.Tower", "owner"),
            Some(Member::Navigation {
                ty: "City.Person",
                collection: false
            })
        );
        assert_eq!(
            edm.member("City.Tower", "parts"),
            Some(Member::Navigation {
                ty: "City.Place",
                collection: true
            })
        );
        assert_eq!(edm.member("City.Address", "street"), Some(single));
        assert_eq!(edm.member("City.Address", "city"), None);
        for (ty, name) in [
            ("City.Tower", "width"),
            ("City.Loop", "id"),
            ("City.Odd", "street"),
            ("City.Lost", "id"),
            ("City.Nothing", "id"),
            ("Nowhere.Tower", "id"),
            ("Tower", "id"),
        ] {
            assert_eq!(edm.member(ty, name), None, "{ty} {name}");
        }
    }

    #[test]
    fn the_parser_reads_a_model_through_it() {
        let model = city();
        for text in [
            "Towers('t')/address/street",
            "Towers('t')?$filter=parts/any(p:p/id eq 'x')&$expand=owner",
        ] {
            let uri = crate::parse(text, &EdmModel(&model), &crate::Limits::strict());
            assert!(uri.is_ok(), "{text}: {uri:?}");
        }
    }
}
