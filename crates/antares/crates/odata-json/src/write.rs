// SPDX-License-Identifier: EUPL-1.2
//! The response writers (JSON Format 4.01 sections 5, 11, 12, 14): straight to a
//! `std::io::Write`, members in the order the format asks for, control information by the
//! negotiated level and version.

use std::io::{self, Write};

use odata_csdl::{Model, Property, TypeRef};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::model::Resource;
use crate::{context_url, ContextTarget, Ctx, MetadataLevel};

/// One entity to write: its id and its property values in the order to write them. Values are
/// the JSON Format representations (section 7); the caller has applied `$select`.
///
/// ```
/// use odata_json::EntityValue;
/// use serde_json::json;
/// let building = EntityValue { id: "Buildings('b1')".into(), properties: vec![("height".into(), json!(42))] };
/// assert_eq!(building.properties.len(), 1);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct EntityValue {
    /// The entity id relative to the service root, the canonical URL: `Buildings('b1')`.
    pub id: String,
    /// Property name and value pairs.
    pub properties: Vec<(String, Value)>,
}

/// The paging control information of a collection.
///
/// ```
/// use odata_json::Page;
/// let page = Page { count: Some(120), next_link: Some("https://example.org/odata/Buildings?$skiptoken=50") };
/// assert_eq!(Page::default().count, None);
/// # let _ = page;
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Page<'a> {
    /// `@odata.count`, written when `$count=true` asked for it.
    pub count: Option<u64>,
    /// `@odata.nextLink`, an absolute URL.
    pub next_link: Option<&'a str>,
}

fn json<W: Write, T: Serialize + ?Sized>(out: &mut W, value: &T) -> io::Result<()> {
    serde_json::to_writer(out, value).map_err(io::Error::from)
}

/// The members of one JSON object being written.
struct Members<'w, W> {
    out: &'w mut W,
    first: bool,
}

impl<'w, W: Write> Members<'w, W> {
    fn open(out: &'w mut W) -> io::Result<Self> {
        out.write_all(b"{")?;
        Ok(Self { out, first: true })
    }

    /// Writes the member name; the value follows on the returned writer.
    fn name(&mut self, name: &str) -> io::Result<&mut W> {
        if !self.first {
            self.out.write_all(b",")?;
        }
        self.first = false;
        json(self.out, name)?;
        self.out.write_all(b":")?;
        Ok(self.out)
    }

    fn member<T: Serialize + ?Sized>(&mut self, name: &str, value: &T) -> io::Result<()> {
        let out = self.name(name)?;
        json(out, value)
    }

    fn close(self) -> io::Result<()> {
        self.out.write_all(b"}")
    }
}

/// Writes one response under a context.
struct Writer<'c> {
    ctx: &'c Ctx<'c>,
    model: &'c Model,
}

impl Writer<'_> {
    fn full(&self) -> bool {
        self.ctx.level == MetadataLevel::Full
    }

    /// The name of a control information member, `@odata.id` or `@id`, after `annotated` (the
    /// property it annotates, or empty).
    fn control(&self, annotated: &str, name: &str) -> String {
        format!("{annotated}{}{name}", self.ctx.version.prefix())
    }

    fn context<W: Write>(
        &self,
        members: &mut Members<'_, W>,
        target: &ContextTarget<'_>,
    ) -> io::Result<()> {
        if self.ctx.level == MetadataLevel::None {
            return Ok(());
        }
        members.member(&self.control("", "context"), &context_url(self.ctx, target))?;
        match self.ctx.metadata_etag {
            Some(tag) => members.member(&self.control("", "metadataEtag"), tag),
            None => Ok(()),
        }
    }

    /// The type name a full-metadata `@odata.type` names for a property, or `None` when a client
    /// tells it from the JSON (JSON Format 4.01 section 4.5.3: string, boolean, double) or the
    /// value names it itself (a complex instance, an untyped value).
    fn type_of(&self, ty: &TypeRef) -> Option<String> {
        let name = match ty.name.strip_prefix("Edm.") {
            Some("Untyped") => return None,
            Some(primitive) => primitive.to_owned(),
            None => self
                .model
                .full_name(&ty.name)
                .unwrap_or_else(|| ty.name.clone()),
        };
        if ty.collection {
            return Some(format!("Collection({name})"));
        }
        let heuristic = ["String", "Boolean", "Double"].contains(&name.as_str());
        let complex = self.model.complex_type(&ty.name).is_some();
        (!heuristic && !complex).then_some(name)
    }

    /// A property, preceded by its type in full metadata.
    fn property<W: Write>(
        &self,
        members: &mut Members<'_, W>,
        name: &str,
        value: &Value,
        declared: Option<&Property>,
    ) -> io::Result<()> {
        let ty = declared.map(|p| &p.ty);
        if self.full() && !value.is_null() {
            if let Some(type_name) = ty.and_then(|t| self.type_of(t)) {
                members.member(&self.control(name, "type"), &format!("#{type_name}"))?;
            }
        }
        let out = members.name(name)?;
        self.value(out, value, ty)
    }

    /// A value; in full metadata a complex instance carries its type and its members' types.
    fn value<W: Write>(&self, out: &mut W, value: &Value, ty: Option<&TypeRef>) -> io::Result<()> {
        let complex = ty
            .filter(|_| self.full())
            .and_then(|t| self.model.complex_type(&t.name).map(|c| (t, c)));
        let Some((ty, complex)) = complex else {
            return json(out, value);
        };
        let instance = |out: &mut W, item: &Value| match item {
            Value::Object(members) => self.complex(out, &ty.name, &complex.properties, members),
            _ => json(out, item),
        };
        match value {
            Value::Array(items) => {
                out.write_all(b"[")?;
                for (at, item) in items.iter().enumerate() {
                    if at > 0 {
                        out.write_all(b",")?;
                    }
                    instance(out, item)?;
                }
                out.write_all(b"]")
            }
            _ => instance(out, value),
        }
    }

    fn complex<W: Write>(
        &self,
        out: &mut W,
        type_name: &str,
        properties: &[Property],
        values: &Map<String, Value>,
    ) -> io::Result<()> {
        let mut members = Members::open(out)?;
        // Found by `complex_type`, so the qualifier names a namespace.
        let full_name = self.model.full_name(type_name).unwrap_or_default();
        members.member(&self.control("", "type"), &format!("#{full_name}"))?;
        for (name, value) in values {
            let declared = properties.iter().find(|p| &p.name == name);
            self.property(&mut members, name, value, declared)?;
        }
        members.close()
    }

    /// The members of an entity: in full metadata its type, id and navigation links.
    fn entity<W: Write>(
        &self,
        members: &mut Members<'_, W>,
        resource: &Resource<'_>,
        entity: &EntityValue,
    ) -> io::Result<()> {
        if self.full() {
            members.member(
                &self.control("", "type"),
                &format!("#{}", resource.type_name()),
            )?;
            members.member(&self.control("", "id"), &entity.id)?;
        }
        for (name, value) in &entity.properties {
            self.property(members, name, value, resource.property(name))?;
        }
        if self.full() {
            for navigation in resource.navigations() {
                let link = format!("{}/{}", entity.id, navigation.name);
                members.member(&self.control(&navigation.name, "navigationLink"), &link)?;
            }
        }
        Ok(())
    }

    /// `count`, the `value` array, `nextLink` (sections 12, 13.1).
    fn page<W: Write, T>(
        &self,
        members: &mut Members<'_, W>,
        items: &[T],
        page: &Page<'_>,
        mut item: impl FnMut(&mut W, &T) -> io::Result<()>,
    ) -> io::Result<()> {
        if let Some(count) = page.count {
            members.member(&self.control("", "count"), &count)?;
        }
        let out = members.name("value")?;
        out.write_all(b"[")?;
        for (at, each) in items.iter().enumerate() {
            if at > 0 {
                out.write_all(b",")?;
            }
            item(out, each)?;
        }
        out.write_all(b"]")?;
        if let Some(next_link) = page.next_link {
            members.member(&self.control("", "nextLink"), next_link)?;
        }
        Ok(())
    }
}

/// One entity (JSON Format 4.01 section 5): the context URL, then in full metadata its type and
/// id, its properties (each non-string primitive preceded by its type in full metadata), and in
/// full metadata a navigation link per navigation property.
///
/// ```
/// use odata_csdl::{CsdlVersion, EntityContainer, EntitySet, EntityType, Model, Property, Schema, TypeRef};
/// use odata_json::{write_entity, Ctx, EntityValue, MetadataLevel, ODataVersion, Resource};
/// use serde_json::json;
///
/// let mut city = Schema::new("City");
/// let mut building = EntityType::new("Building");
/// building.properties.push(Property::new("height", TypeRef::single("Edm.Int32")));
/// city.entity_types.push(building);
/// let mut service = EntityContainer::new("Service");
/// service.entity_sets.push(EntitySet::new("Buildings", "City.Building"));
/// city.entity_container = Some(service);
/// let model = Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![city] };
///
/// let ctx = Ctx { service_root: "https://example.org/odata/", level: MetadataLevel::Full, version: ODataVersion::V4_01, metadata_etag: None };
/// let b1 = EntityValue { id: "Buildings('b1')".into(), properties: vec![("height".into(), json!(42))] };
/// let mut body = Vec::new();
/// write_entity(&ctx, &Resource::new(&model, "Buildings").ok_or("no set")?, &b1, &mut body).map_err(|_| "io")?;
/// assert_eq!(
///     String::from_utf8_lossy(&body),
///     r##"{"@context":"https://example.org/odata/$metadata#Buildings/$entity","@type":"#City.Building","@id":"Buildings('b1')","height@type":"#Int32","height":42}"##
/// );
/// # Ok::<(), &str>(())
/// ```
///
/// # Errors
///
/// What `out` answers.
pub fn write_entity<W: Write>(
    ctx: &Ctx<'_>,
    resource: &Resource<'_>,
    entity: &EntityValue,
    mut out: W,
) -> io::Result<()> {
    let writer = Writer {
        ctx,
        model: resource.model(),
    };
    let mut members = Members::open(&mut out)?;
    let target = ContextTarget::Entity {
        set: resource.set(),
        select: resource.projection(),
    };
    writer.context(&mut members, &target)?;
    writer.entity(&mut members, resource, entity)?;
    members.close()
}

/// A collection of entities (JSON Format 4.01 section 12): the context URL, `count` when given,
/// the entities in `value`, `nextLink` when given. Without control information
/// (`metadata=none`) `count` and `nextLink` are still written (section 3.1.3).
///
/// ```
/// # use odata_csdl::{CsdlVersion, EntityContainer, EntitySet, EntityType, Model, Schema};
/// use odata_json::{write_collection, Ctx, EntityValue, MetadataLevel, ODataVersion, Page, Resource};
/// # let mut city = Schema::new("City");
/// # city.entity_types.push(EntityType::new("Building"));
/// # let mut service = EntityContainer::new("Service");
/// # service.entity_sets.push(EntitySet::new("Buildings", "City.Building"));
/// # city.entity_container = Some(service);
/// # let model = Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![city] };
/// let ctx = Ctx { service_root: "https://example.org/odata/", level: MetadataLevel::None, version: ODataVersion::V4_0, metadata_etag: None };
/// let page = Page { count: Some(1), next_link: None };
/// let mut body = Vec::new();
/// let b1 = EntityValue { id: "Buildings('b1')".into(), properties: vec![] };
/// write_collection(&ctx, &Resource::new(&model, "Buildings").ok_or("no set")?, &[b1], &page, &mut body).map_err(|_| "io")?;
/// assert_eq!(body, br#"{"@odata.count":1,"value":[{}]}"#);
/// # Ok::<(), &str>(())
/// ```
///
/// # Errors
///
/// What `out` answers.
pub fn write_collection<W: Write>(
    ctx: &Ctx<'_>,
    resource: &Resource<'_>,
    entities: &[EntityValue],
    page: &Page<'_>,
    mut out: W,
) -> io::Result<()> {
    write_collection_start(ctx, resource, page.count, &mut out)?;
    for (at, entity) in entities.iter().enumerate() {
        write_collection_entity(ctx, resource, entity, at == 0, &mut out)?;
    }
    write_collection_end(ctx, page.next_link, out)
}

/// The start of a collection written as its entities arrive (a streamed answer): what
/// [`write_collection`] writes up to the `value` array's `[`. [`write_collection_entity`] writes
/// each entity, [`write_collection_end`] the rest; the three write the bytes of
/// [`write_collection`].
///
/// ```
/// # use odata_csdl::{CsdlVersion, EntityContainer, EntitySet, EntityType, Model, Schema};
/// use odata_json::{write_collection_end, write_collection_entity, write_collection_start, Ctx, EntityValue, MetadataLevel, ODataVersion, Resource};
/// # let mut city = Schema::new("City");
/// # city.entity_types.push(EntityType::new("Building"));
/// # let mut service = EntityContainer::new("Service");
/// # service.entity_sets.push(EntitySet::new("Buildings", "City.Building"));
/// # city.entity_container = Some(service);
/// # let model = Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![city] };
/// let ctx = Ctx { service_root: "https://example.org/odata/", level: MetadataLevel::None, version: ODataVersion::V4_0, metadata_etag: None };
/// let buildings = Resource::new(&model, "Buildings").ok_or("no set")?;
/// let b1 = EntityValue { id: "Buildings('b1')".into(), properties: vec![] };
/// let mut body = Vec::new();
/// write_collection_start(&ctx, &buildings, Some(2), &mut body).map_err(|_| "io")?;
/// write_collection_entity(&ctx, &buildings, &b1, true, &mut body).map_err(|_| "io")?;
/// write_collection_entity(&ctx, &buildings, &b1, false, &mut body).map_err(|_| "io")?;
/// write_collection_end(&ctx, Some("https://example.org/odata/Buildings?$skiptoken=2"), &mut body).map_err(|_| "io")?;
/// assert_eq!(
///     String::from_utf8_lossy(&body),
///     r#"{"@odata.count":2,"value":[{},{}],"@odata.nextLink":"https://example.org/odata/Buildings?$skiptoken=2"}"#
/// );
/// # Ok::<(), &str>(())
/// ```
///
/// # Errors
///
/// What `out` answers.
pub fn write_collection_start<W: Write>(
    ctx: &Ctx<'_>,
    resource: &Resource<'_>,
    count: Option<u64>,
    mut out: W,
) -> io::Result<()> {
    let writer = Writer {
        ctx,
        model: resource.model(),
    };
    let mut members = Members::open(&mut out)?;
    let target = ContextTarget::Collection {
        set: resource.set(),
        select: resource.projection(),
    };
    writer.context(&mut members, &target)?;
    if let Some(count) = count {
        members.member(&writer.control("", "count"), &count)?;
    }
    members.name("value")?.write_all(b"[")
}

/// One entity of a collection begun by [`write_collection_start`], after a comma unless it is
/// the `first`.
///
/// # Errors
///
/// What `out` answers.
pub fn write_collection_entity<W: Write>(
    ctx: &Ctx<'_>,
    resource: &Resource<'_>,
    entity: &EntityValue,
    first: bool,
    mut out: W,
) -> io::Result<()> {
    if !first {
        out.write_all(b",")?;
    }
    let writer = Writer {
        ctx,
        model: resource.model(),
    };
    let mut members = Members::open(&mut out)?;
    writer.entity(&mut members, resource, entity)?;
    members.close()
}

/// The end of a collection begun by [`write_collection_start`]: the `value` array's `]`, the
/// `nextLink` when given, the closing brace.
///
/// # Errors
///
/// What `out` answers.
pub fn write_collection_end<W: Write>(
    ctx: &Ctx<'_>,
    next_link: Option<&str>,
    mut out: W,
) -> io::Result<()> {
    out.write_all(b"]")?;
    let mut members = Members {
        out: &mut out,
        first: false,
    };
    if let Some(next_link) = next_link {
        let name = format!("{}nextLink", ctx.version.prefix());
        members.member(&name, next_link)?;
    }
    members.close()
}

/// One entity reference (JSON Format 4.01 section 14): the context URL and the id. The id is
/// written absolute (service root and `id`), so it means the same without a context URL.
///
/// ```
/// use odata_json::{write_entity_reference, Ctx, MetadataLevel, ODataVersion};
/// let ctx = Ctx { service_root: "https://example.org/odata/", level: MetadataLevel::Minimal, version: ODataVersion::V4_0, metadata_etag: None };
/// let mut body = Vec::new();
/// write_entity_reference(&ctx, "People('p1')", &mut body)?;
/// assert_eq!(
///     String::from_utf8_lossy(&body),
///     r#"{"@odata.context":"https://example.org/odata/$metadata#$ref","@odata.id":"https://example.org/odata/People('p1')"}"#
/// );
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// What `out` answers.
pub fn write_entity_reference<W: Write>(ctx: &Ctx<'_>, id: &str, mut out: W) -> io::Result<()> {
    let writer = Writer { ctx, model: &EMPTY };
    let mut members = Members::open(&mut out)?;
    writer.context(&mut members, &ContextTarget::Reference)?;
    members.member(
        &writer.control("", "id"),
        &format!("{}{id}", ctx.service_root),
    )?;
    members.close()
}

/// A collection of entity references (JSON Format 4.01 section 14), ids absolute as in
/// [`write_entity_reference`].
///
/// ```
/// use odata_json::{write_entity_references, Ctx, MetadataLevel, ODataVersion, Page};
/// let ctx = Ctx { service_root: "https://example.org/odata/", level: MetadataLevel::None, version: ODataVersion::V4_01, metadata_etag: None };
/// let mut body = Vec::new();
/// write_entity_references(&ctx, &["People('p1')"], &Page::default(), &mut body)?;
/// assert_eq!(String::from_utf8_lossy(&body), r#"{"value":[{"@id":"https://example.org/odata/People('p1')"}]}"#);
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// What `out` answers.
pub fn write_entity_references<W: Write>(
    ctx: &Ctx<'_>,
    ids: &[&str],
    page: &Page<'_>,
    mut out: W,
) -> io::Result<()> {
    let writer = Writer { ctx, model: &EMPTY };
    let mut members = Members::open(&mut out)?;
    writer.context(&mut members, &ContextTarget::References)?;
    let id = writer.control("", "id");
    writer.page(&mut members, ids, page, |out, each| {
        let mut reference = Members::open(out)?;
        reference.member(&id, &format!("{}{each}", ctx.service_root))?;
        reference.close()
    })?;
    members.close()
}

/// The model the reference writers resolve no names against.
static EMPTY: Model = Model {
    version: odata_csdl::CsdlVersion::V4_01,
    references: Vec::new(),
    schemas: Vec::new(),
};

/// The service document (JSON Format 4.01 section 5): the context URL and one entry per entity
/// set of the model's entity container, in order.
///
/// ```
/// use odata_csdl::{CsdlVersion, EntityContainer, EntitySet, Model, Schema};
/// use odata_json::{write_service_document, Ctx, MetadataLevel, ODataVersion};
///
/// let mut city = Schema::new("City");
/// let mut service = EntityContainer::new("Service");
/// service.entity_sets.push(EntitySet::new("Buildings", "City.Building"));
/// city.entity_container = Some(service);
/// let model = Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![city] };
///
/// let ctx = Ctx { service_root: "https://example.org/odata/", level: MetadataLevel::Minimal, version: ODataVersion::V4_01, metadata_etag: None };
/// let mut body = Vec::new();
/// write_service_document(&ctx, &model, &mut body)?;
/// assert_eq!(
///     String::from_utf8_lossy(&body),
///     r#"{"@context":"https://example.org/odata/$metadata","value":[{"name":"Buildings","kind":"EntitySet","url":"Buildings"}]}"#
/// );
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// What `out` answers.
pub fn write_service_document<W: Write>(
    ctx: &Ctx<'_>,
    model: &Model,
    mut out: W,
) -> io::Result<()> {
    let writer = Writer { ctx, model };
    let mut members = Members::open(&mut out)?;
    writer.context(&mut members, &ContextTarget::ServiceDocument)?;
    let sets: Vec<&str> = model
        .schemas
        .iter()
        .filter_map(|s| s.entity_container.as_ref())
        .flat_map(|c| &c.entity_sets)
        .map(|set| set.name.as_str())
        .collect();
    writer.page(&mut members, &sets, &Page::default(), |out, name| {
        let mut entry = Members::open(out)?;
        entry.member("name", name)?;
        entry.member("kind", "EntitySet")?;
        entry.member("url", name)?;
        entry.close()
    })?;
    members.close()
}
