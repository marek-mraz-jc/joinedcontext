// SPDX-License-Identifier: EUPL-1.2
//! The request-body reader for the mapped writes (ADR-0024 decision 5, row `odata-json`): create
//! (W1, W2), update and replace (W4, W6), a property (W10, W11) and a single-valued or full
//! collection reference (W16, W20). Every name is checked against the model before the surface
//! sees the body (odata-linkml-mapping M4).

use odata_csdl::{Model, Property, TypeRef};
use odata_url::{EdmModel, Key, ODataUri, QueryOptions, Segment};
use serde_json::{Map, Value};

use crate::model::{computed, computed_default, Resource};
use crate::{json, BodyError, BodyLimit, Ctx, Feature};

/// The bounds of one request body (ADR-0024 decision 4, threat model O22). Every reader takes
/// them from the caller; there is no default on a request path.
///
/// ```
/// use odata_json::BodyLimits;
/// let limits = BodyLimits { max_bind_links: 4, ..BodyLimits::strict() };
/// assert_eq!(limits.max_bind_links, 4);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyLimits {
    /// Bytes of the body.
    pub max_bytes: usize,
    /// Nesting of objects and arrays; the body's own object is 1.
    pub max_depth: usize,
    /// Members of all objects of the body together, control information included.
    pub max_properties: usize,
    /// Bind links of one body.
    pub max_bind_links: usize,
    /// Items of one array.
    pub max_collection_len: usize,
    /// The bounds a bind URL is parsed under.
    pub bind_url: odata_url::Limits,
}

impl BodyLimits {
    /// Bounds for tests and documentation: 1 MiB, depth 8, 1000 properties, 100 bind links,
    /// 1000 items, the bind URL under [`odata_url::Limits::strict`]. The NGSI-LD side bounds a
    /// request at 1 MiB too (`MAX_BODY_BYTES`).
    #[must_use]
    pub fn strict() -> Self {
        Self {
            max_bytes: 1 << 20,
            max_depth: 8,
            max_properties: 1000,
            max_bind_links: 100,
            max_collection_len: 1000,
            bind_url: odata_url::Limits::strict(),
        }
    }
}

/// Which write a body is for.
///
/// ```
/// use odata_json::WriteKind;
/// assert_ne!(WriteKind::Create, WriteKind::Update);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteKind {
    /// `POST` to the entity set (W1, W2).
    Create,
    /// `PATCH` of an entity (W4).
    Update,
    /// `PUT` of an entity (W6).
    Replace,
}

/// The entity a bind addresses: an entity set and a key, as the bind URL wrote them.
///
/// ```
/// use odata_json::Target;
/// use odata_url::{Key, KeyValue, Literal};
/// let target = Target { set: "People".into(), key: Key::Single(KeyValue::Literal(Literal::String("p1".into()))) };
/// assert_eq!(target.set, "People");
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    /// The entity set.
    pub set: String,
    /// The key.
    pub key: Key,
}

/// A bind of a single-valued navigation property (`nav@odata.bind`, or `nav: {"@id": ...}` in
/// 4.01).
///
/// ```
/// use odata_json::{Bind, Target};
/// use odata_url::{Key, KeyValue, Literal};
/// let owner = Bind { navigation: "owner".into(), target: Target { set: "People".into(), key: Key::Single(KeyValue::Literal(Literal::Int(7))) } };
/// assert_eq!(owner.navigation, "owner");
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Bind {
    /// The navigation property.
    pub navigation: String,
    /// The entity it is bound to.
    pub target: Target,
}

/// A write body read against the model. Values are as the body wrote them, each checked against
/// its declared type; control information and instance annotations are dropped.
///
/// ```
/// use odata_json::EntityPayload;
/// assert!(EntityPayload::default().properties.is_empty());
/// ```
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EntityPayload {
    /// The key properties the body names: all of them in a create; in an update or replace the
    /// surface compares them with the URL's key.
    pub key: Vec<(String, Value)>,
    /// The other structural properties, in name order.
    pub properties: Vec<(String, Value)>,
    /// The binds, in navigation-property name order.
    pub binds: Vec<Bind>,
}

/// The value of a control-information member, with or without the `odata.` prefix (4.01 lets
/// the client omit it).
fn control<'v>(members: &'v Map<String, Value>, name: &str) -> Option<&'v Value> {
    members
        .get(&format!("@odata.{name}"))
        .or_else(|| members.get(&format!("@{name}")))
}

/// An annotation member name `property@term`: the property and the term without `odata.`.
fn annotation(name: &str) -> Option<(&str, &str)> {
    let (property, term) = name.split_once('@')?;
    Some((property, term.strip_prefix("odata.").unwrap_or(term)))
}

/// An object holding only an entity id: the 4.01 bind form `{"@id": ...}`.
fn id_only(value: &Value) -> Option<&Value> {
    let Value::Object(members) = value else {
        return None;
    };
    let id = control(members, "id")?;
    (members.len() == 1).then_some(id)
}

/// Reads one body against one resource.
struct Reader<'r, 'm> {
    service_root: &'r str,
    resource: &'r Resource<'m>,
    limits: &'r BodyLimits,
    read_only: Vec<&'m str>,
    binds: usize,
}

impl<'m> Reader<'_, 'm> {
    fn model(&self) -> &'m Model {
        self.resource.model()
    }

    /// The entity a bind URL addresses, which must be of `target_type` in this service: absolute
    /// under the service root, or relative to it (JSON Format 4.01 section 8.5).
    fn target(
        &mut self,
        navigation: &str,
        target_type: &str,
        url: &Value,
    ) -> Result<Target, BodyError> {
        let invalid = || BodyError::InvalidBind {
            navigation: navigation.to_owned(),
        };
        let max = self.limits.max_bind_links;
        self.binds = self
            .binds
            .checked_add(1)
            .filter(|binds| *binds <= max)
            .ok_or(BodyError::LimitExceeded {
                limit: BodyLimit::BindLinks,
                max,
            })?;
        let url = url.as_str().ok_or_else(invalid)?;
        let relative = url.strip_prefix(self.service_root).unwrap_or(url);
        if relative.starts_with('/') || relative.contains("://") {
            return Err(invalid());
        }
        let model = self.model();
        let uri = odata_url::parse(relative, &EdmModel(model), &self.limits.bind_url)
            .map_err(|_| invalid())?;
        let ODataUri::Resource { path, options } = uri else {
            return Err(invalid());
        };
        let [Segment::EntitySet(set), Segment::Key(key)] =
            <[Segment; 2]>::try_from(path).map_err(|_| invalid())?
        else {
            return Err(invalid());
        };
        let set_type = model
            .entity_set(&set)
            .and_then(|s| model.full_name(&s.entity_type));
        let same_type = set_type.is_some_and(|t| Some(t) == model.full_name(target_type));
        if options != QueryOptions::default() || !same_type {
            return Err(invalid());
        }
        Ok(Target { set, key })
    }

    /// The navigation property `name`, unless the set's restrictions make it read-only.
    fn navigation(&self, name: &str) -> Result<&'m TypeRef, BodyError> {
        let navigation = self
            .resource
            .navigation(name)
            .ok_or(BodyError::UnknownProperty)?;
        if self.read_only.contains(&name) {
            return Err(BodyError::ReadOnly {
                property: navigation.name.clone(),
            });
        }
        Ok(&navigation.ty)
    }

    /// `nav@odata.bind` (4.0) or `nav: {...}` (4.01, `deep` the feature nested entities ask
    /// for): the bind of a single-valued navigation property.
    fn bind(
        &mut self,
        name: &str,
        value: &Value,
        deep: Option<Feature>,
    ) -> Result<Bind, BodyError> {
        let ty = self.navigation(name)?;
        let invalid = || BodyError::InvalidBind {
            navigation: name.to_owned(),
        };
        let url = match (deep, value) {
            (_, Value::Null) => return Err(BodyError::NotImplemented(Feature::Unbind)),
            (_, Value::Array(items)) if ty.collection => {
                let binds_only = deep.is_none() || items.iter().all(|item| id_only(item).is_some());
                let feature = deep
                    .filter(|_| !binds_only)
                    .unwrap_or(Feature::CollectionBind);
                return Err(BodyError::NotImplemented(feature));
            }
            (None, url @ Value::String(_)) => url,
            (Some(feature), object @ Value::Object(_)) => match id_only(object) {
                Some(url) => url,
                None => return Err(BodyError::NotImplemented(feature)),
            },
            _ => return Err(invalid()),
        };
        if ty.collection {
            return Err(invalid());
        }
        Ok(Bind {
            navigation: name.to_owned(),
            target: self.target(name, &ty.name, url)?,
        })
    }

    /// A value of a declared property at `path`.
    fn check(&self, path: &str, property: &Property, value: &Value) -> Result<(), BodyError> {
        if computed(self.model(), property) || self.read_only.contains(&path) {
            return Err(BodyError::ReadOnly {
                property: path.to_owned(),
            });
        }
        let invalid = || BodyError::InvalidValue {
            property: path.to_owned(),
        };
        let null = || {
            if property.nullable {
                Ok(())
            } else {
                Err(BodyError::NullNotAllowed {
                    property: path.to_owned(),
                })
            }
        };
        match value {
            Value::Null if property.ty.collection => Err(invalid()),
            Value::Null => null(),
            Value::Array(items) if property.ty.collection => {
                items.iter().try_for_each(|item| match item {
                    Value::Null => null(),
                    _ => self.single(path, &property.ty.name, item),
                })
            }
            _ if property.ty.collection => Err(invalid()),
            _ => self.single(path, &property.ty.name, value),
        }
    }

    /// A non-null value of one instance of type `name`.
    fn single(&self, path: &str, name: &str, value: &Value) -> Result<(), BodyError> {
        let integer = |min: i64, max: i64| value.as_i64().is_some_and(|v| (min..=max).contains(&v));
        let fits = match name {
            "Edm.String" => value.is_string(),
            "Edm.Date" | "Edm.DateTimeOffset" | "Edm.TimeOfDay" | "Edm.Duration" | "Edm.Guid"
            | "Edm.Binary" => value
                .as_str()
                .is_some_and(|text| odata_url::is_primitive_text(name, text)),
            "Edm.Boolean" => value.is_boolean(),
            "Edm.Byte" => integer(0, 255),
            "Edm.SByte" => integer(-128, 127),
            "Edm.Int16" => integer(i16::MIN.into(), i16::MAX.into()),
            "Edm.Int32" => integer(i32::MIN.into(), i32::MAX.into()),
            "Edm.Int64" => value.is_i64(),
            "Edm.Decimal" => value.is_number(),
            "Edm.Single" | "Edm.Double" => {
                value.is_number()
                    || ["INF", "-INF", "NaN"]
                        .iter()
                        .any(|s| value.as_str() == Some(s))
            }
            "Edm.Untyped" | "Edm.PrimitiveType" => true,
            _ if name.starts_with("Edm.Geography") || name.starts_with("Edm.Geometry") => {
                value.is_object()
            }
            _ => {
                return match (self.model().complex_type(name), value) {
                    (Some(complex), Value::Object(members)) => {
                        self.structured(path, name, members, |member| {
                            complex.properties.iter().find(|p| p.name == member)
                        })
                    }
                    _ => Err(BodyError::InvalidValue {
                        property: path.to_owned(),
                    }),
                };
            }
        };
        if fits {
            Ok(())
        } else {
            Err(BodyError::InvalidValue {
                property: path.to_owned(),
            })
        }
    }

    /// The members of a complex instance at `path` of type `type_name`.
    fn structured<'p>(
        &self,
        path: &str,
        type_name: &str,
        members: &Map<String, Value>,
        declared: impl Fn(&str) -> Option<&'p Property>,
    ) -> Result<(), BodyError> {
        self.check_type(members, type_name)?;
        for (name, value) in members {
            if name.contains('@') {
                continue;
            }
            let property = declared(name).ok_or(BodyError::UnknownProperty)?;
            self.check(&format!("{path}/{name}"), property, value)?;
        }
        Ok(())
    }

    /// `@odata.type` of an instance, when present, names `type_name`.
    fn check_type(&self, members: &Map<String, Value>, type_name: &str) -> Result<(), BodyError> {
        let Some(named) = control(members, "type") else {
            return Ok(());
        };
        let model = self.model();
        let named = named.as_str().map(|n| n.strip_prefix('#').unwrap_or(n));
        if named
            .and_then(|n| model.full_name(n))
            .is_some_and(|n| Some(n) == model.full_name(type_name))
        {
            Ok(())
        } else {
            Err(BodyError::TypeMismatch)
        }
    }

    fn entity(
        &mut self,
        kind: WriteKind,
        members: &Map<String, Value>,
    ) -> Result<EntityPayload, BodyError> {
        self.check_type(members, self.resource.type_name())?;
        let deep = if kind == WriteKind::Create {
            Feature::DeepInsert
        } else {
            Feature::DeepUpdate
        };
        let key = self.resource.key();
        let mut payload = EntityPayload::default();
        for (name, value) in members {
            if let Some((property, term)) = annotation(name) {
                if !property.is_empty() && term == "bind" {
                    payload.binds.push(self.bind(property, value, None)?);
                }
                continue;
            }
            if self.resource.navigation(name).is_some() {
                payload.binds.push(self.bind(name, value, Some(deep))?);
                continue;
            }
            let property = self
                .resource
                .property(name)
                .ok_or(BodyError::UnknownProperty)?;
            if key.contains(name) {
                // Core.Computed: the service sets the key, a body never does.
                if computed(self.model(), property) {
                    return Err(BodyError::ReadOnly {
                        property: name.clone(),
                    });
                }
                self.single(name, &property.ty.name, value)?;
                payload.key.push((name.clone(), value.clone()));
            } else {
                self.check(name, property, value)?;
                payload.properties.push((name.clone(), value.clone()));
            }
        }
        payload
            .binds
            .sort_by(|a, b| a.navigation.cmp(&b.navigation));
        if let Some(twice) = payload.binds.windows(2).find_map(|pair| match pair {
            [a, b] if a.navigation == b.navigation => Some(a),
            _ => None,
        }) {
            return Err(BodyError::InvalidBind {
                navigation: twice.navigation.clone(),
            });
        }
        if kind == WriteKind::Create {
            // A key the service computes (Core.Computed, or
            // Core.ComputedDefaultValue when the body leaves it out) is not
            // the client's to name.
            let named = |k: &String| payload.key.iter().any(|(name, _)| name == k);
            let service_sets = |k: &String| {
                self.resource
                    .property(k)
                    .is_some_and(|p| computed(self.model(), p) || computed_default(self.model(), p))
            };
            if let Some(missing) = key.iter().find(|k| !named(k) && !service_sets(k)) {
                return Err(BodyError::MissingKey {
                    property: missing.clone(),
                });
            }
            // C5: InsertRestrictions/RequiredProperties are named, and not null.
            let given = |p: &&str| members.get(*p).is_some_and(|v| !v.is_null());
            if let Some(missing) = self
                .resource
                .restricted("InsertRestrictions", "RequiredProperties")
                .into_iter()
                .find(|p| !given(p))
            {
                return Err(BodyError::MissingProperty {
                    property: missing.to_owned(),
                });
            }
        }
        Ok(payload)
    }
}

/// The restriction terms of a write: (term, properties field, navigation properties field).
fn restrictions(kind: WriteKind) -> (&'static str, &'static str, &'static str) {
    match kind {
        WriteKind::Create => (
            "InsertRestrictions",
            "NonInsertableProperties",
            "NonInsertableNavigationProperties",
        ),
        WriteKind::Update | WriteKind::Replace => (
            "UpdateRestrictions",
            "NonUpdatableProperties",
            "NonUpdatableNavigationProperties",
        ),
    }
}

fn reader<'r, 'm>(
    service_root: &'r str,
    resource: &'r Resource<'m>,
    kind: WriteKind,
    limits: &'r BodyLimits,
) -> Reader<'r, 'm> {
    let (term, properties, navigations) = restrictions(kind);
    let mut read_only = resource.restricted(term, properties);
    read_only.extend(resource.restricted(term, navigations));
    Reader {
        service_root,
        resource,
        limits,
        read_only,
        binds: 0,
    }
}

fn object(input: &[u8], limits: &BodyLimits) -> Result<Map<String, Value>, BodyError> {
    match json::parse(input, limits)? {
        Value::Object(members) => Ok(members),
        _ => Err(BodyError::NotAnObject),
    }
}

/// The body of a create, update or replace of an entity of `resource` (W1, W2, W4, W6).
///
/// Structural properties must be declared (open types are not read), have their declared type, and be
/// neither `Core.Computed` nor named by the set's `NonInsertableProperties` (create) or
/// `NonUpdatableProperties` (update, replace); a create names every key property but one the
/// service computes (`Core.Computed`, never in a body, or `Core.ComputedDefaultValue`). A
/// single-valued navigation property is bound by URL; nested entities, collection binds and
/// unbinding answer [`BodyError::NotImplemented`]. `@odata.type`, when present, names the set's
/// type; other control information and instance annotations are ignored.
///
/// ```
/// use odata_csdl::{CsdlVersion, EntityContainer, EntitySet, EntityType, Model, NavigationProperty, Property, Schema, TypeRef};
/// use odata_json::{read_entity_body, BodyError, BodyLimits, Ctx, MetadataLevel, ODataVersion, Resource, WriteKind};
///
/// let mut city = Schema::new("City");
/// let mut building = EntityType::new("Building");
/// building.key.push("id".into());
/// building.properties.push(Property::new("id", TypeRef::single("Edm.String")).required());
/// building.properties.push(Property::new("height", TypeRef::single("Edm.Int32")));
/// building.navigation_properties.push(NavigationProperty::new("owner", TypeRef::single("City.Building")));
/// city.entity_types.push(building);
/// let mut service = EntityContainer::new("Service");
/// service.entity_sets.push(EntitySet::new("Buildings", "City.Building"));
/// city.entity_container = Some(service);
/// let model = Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![city] };
///
/// let ctx = Ctx { service_root: "https://example.org/odata/", level: MetadataLevel::Minimal, version: ODataVersion::V4_01, metadata_etag: None };
/// let buildings = Resource::new(&model, "Buildings").ok_or(BodyError::UnknownProperty)?;
/// let body = br#"{"id":"b2","height":12,"owner@odata.bind":"Buildings('b1')"}"#;
/// let payload = read_entity_body(&ctx, &buildings, WriteKind::Create, body, &BodyLimits::strict())?;
/// assert_eq!(payload.binds[0].target.set, "Buildings");
/// let deep = br#"{"id":"b3","owner":{"id":"b4"}}"#;
/// assert!(matches!(read_entity_body(&ctx, &buildings, WriteKind::Create, deep, &BodyLimits::strict()), Err(BodyError::NotImplemented(_))));
/// # Ok::<(), BodyError>(())
/// ```
///
/// # Errors
///
/// [`BodyError`], by the first problem found.
pub fn read_entity_body(
    ctx: &Ctx<'_>,
    resource: &Resource<'_>,
    kind: WriteKind,
    input: &[u8],
    limits: &BodyLimits,
) -> Result<EntityPayload, BodyError> {
    let members = object(input, limits)?;
    reader(ctx.service_root, resource, kind, limits).entity(kind, &members)
}

/// The body of a `PUT` or `PATCH` of one structural property (W10, W11): `{"value": ...}` for a
/// primitive or collection property, the instance itself for a complex one. The property must
/// be declared and writable as in [`read_entity_body`]; a key property is read-only.
///
/// ```
/// # use odata_csdl::{CsdlVersion, EntityContainer, EntitySet, EntityType, Model, Property, Schema, TypeRef};
/// use odata_json::{read_property_body, BodyError, BodyLimits, Resource};
/// # let mut city = Schema::new("City");
/// # let mut building = EntityType::new("Building");
/// # building.properties.push(Property::new("height", TypeRef::single("Edm.Int32")));
/// # city.entity_types.push(building);
/// # let mut service = EntityContainer::new("Service");
/// # service.entity_sets.push(EntitySet::new("Buildings", "City.Building"));
/// # city.entity_container = Some(service);
/// # let model = Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![city] };
/// let buildings = Resource::new(&model, "Buildings").ok_or(BodyError::UnknownProperty)?;
/// let value = read_property_body(&buildings, "height", br#"{"value":12}"#, &BodyLimits::strict())?;
/// assert_eq!(value, 12);
/// # Ok::<(), BodyError>(())
/// ```
///
/// # Errors
///
/// [`BodyError`], by the first problem found.
pub fn read_property_body(
    resource: &Resource<'_>,
    property: &str,
    input: &[u8],
    limits: &BodyLimits,
) -> Result<Value, BodyError> {
    // A property body binds nothing, so no service root is needed.
    let reader = reader("", resource, WriteKind::Update, limits);
    let declared = resource
        .property(property)
        .ok_or(BodyError::UnknownProperty)?;
    if resource.key().iter().any(|k| k == property) {
        return Err(BodyError::ReadOnly {
            property: declared.name.clone(),
        });
    }
    let mut members = object(input, limits)?;
    let complex =
        !declared.ty.collection && resource.model().complex_type(&declared.ty.name).is_some();
    let value = if complex {
        Value::Object(members)
    } else {
        members.retain(|name, _| !name.starts_with('@'));
        match (members.remove("value"), members.is_empty()) {
            (Some(value), true) => value,
            _ => return Err(BodyError::UnknownProperty),
        }
    };
    reader.check(&declared.name, declared, &value)?;
    Ok(value)
}

/// The body of a `PUT` to `{nav}/$ref` (W16, W20): `{"@odata.id": ...}` for a single-valued
/// navigation property, `{"value": [{"@odata.id": ...}, ...]}` (the full collection) for a
/// collection-valued one.
///
/// ```
/// # use odata_csdl::{CsdlVersion, EntityContainer, EntitySet, EntityType, Model, NavigationProperty, Property, Schema, TypeRef};
/// use odata_json::{read_reference_body, BodyError, BodyLimits, Ctx, MetadataLevel, ODataVersion, Resource};
/// # let mut city = Schema::new("City");
/// # let mut building = EntityType::new("Building");
/// # building.key.push("id".into());
/// # building.properties.push(Property::new("id", TypeRef::single("Edm.String")).required());
/// # building.navigation_properties.push(NavigationProperty::new("owner", TypeRef::single("City.Building")));
/// # city.entity_types.push(building);
/// # let mut service = EntityContainer::new("Service");
/// # service.entity_sets.push(EntitySet::new("Buildings", "City.Building"));
/// # city.entity_container = Some(service);
/// # let model = Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![city] };
/// let ctx = Ctx { service_root: "https://example.org/odata/", level: MetadataLevel::Minimal, version: ODataVersion::V4_0, metadata_etag: None };
/// let buildings = Resource::new(&model, "Buildings").ok_or(BodyError::UnknownProperty)?;
/// let body = br#"{"@odata.id":"https://example.org/odata/Buildings('b1')"}"#;
/// let targets = read_reference_body(&ctx, &buildings, "owner", body, &BodyLimits::strict())?;
/// assert_eq!(targets.len(), 1);
/// # Ok::<(), BodyError>(())
/// ```
///
/// # Errors
///
/// [`BodyError`], by the first problem found.
pub fn read_reference_body(
    ctx: &Ctx<'_>,
    resource: &Resource<'_>,
    navigation: &str,
    input: &[u8],
    limits: &BodyLimits,
) -> Result<Vec<Target>, BodyError> {
    let mut reader = reader(ctx.service_root, resource, WriteKind::Update, limits);
    let ty = reader.navigation(navigation)?;
    let members = object(input, limits)?;
    let invalid = || BodyError::InvalidBind {
        navigation: navigation.to_owned(),
    };
    if !ty.collection {
        let url = id_only(&Value::Object(members))
            .ok_or_else(invalid)?
            .clone();
        return Ok(vec![reader.target(navigation, &ty.name, &url)?]);
    }
    let Some(Value::Array(items)) = members.get("value") else {
        return Err(invalid());
    };
    items
        .iter()
        .map(|item| {
            let url = id_only(item).ok_or_else(invalid)?;
            reader.target(navigation, &ty.name, url)
        })
        .collect()
}
