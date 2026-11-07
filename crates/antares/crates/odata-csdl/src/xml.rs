// SPDX-License-Identifier: EUPL-1.2
//! CSDL XML (OData CSDL XML Representation 4.01).

use std::io::Write;

use crate::error::CsdlError;
use crate::model::{
    line_ends, Annotation, EntityContainer, EntityType, Expr, Model, NavigationProperty, Property,
    Scale, Schema, Srid, Term, TypeRef,
};
use crate::validate::{validate, Limits};

const EDMX: &str = "http://docs.oasis-open.org/odata/ns/edmx";
const EDM: &str = "http://docs.oasis-open.org/odata/ns/edm";

/// Writes `model` as a CSDL XML document, after [`validate`] accepts it under `limits`.
///
/// # Errors
///
/// Whatever [`validate`] answers, or [`CsdlError::Io`] when `out` fails.
///
/// ```
/// use odata_csdl::{write_xml, CsdlVersion, Limits, Model, Schema};
/// let model = Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![Schema::new("ns")] };
/// let mut out = Vec::new();
/// write_xml(&model, &Limits::strict(), &mut out)?;
/// let xml = String::from_utf8(out)?;
/// assert!(xml.contains(r#"<Schema xmlns="http://docs.oasis-open.org/odata/ns/edm" Namespace="ns"/>"#));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn write_xml<W: Write>(model: &Model, limits: &Limits, out: W) -> Result<(), CsdlError> {
    validate(model, limits)?;
    let mut xml = Xml { out, depth: 0 };
    xml.out
        .write_all(b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\n")?;
    xml.open(
        "edmx:Edmx",
        &[("xmlns:edmx", EDMX), ("Version", model.version.as_str())],
    )?;
    for reference in &model.references {
        xml.open("edmx:Reference", &[("Uri", &reference.uri)])?;
        for include in &reference.includes {
            let mut attrs = vec![("Namespace", include.namespace.as_str())];
            if let Some(alias) = &include.alias {
                attrs.push(("Alias", alias));
            }
            xml.empty("edmx:Include", &attrs)?;
        }
        xml.close("edmx:Reference")?;
    }
    xml.open("edmx:DataServices", &[])?;
    for schema in &model.schemas {
        xml.schema(schema)?;
    }
    xml.close("edmx:DataServices")?;
    xml.close("edmx:Edmx")?;
    Ok(())
}

struct Xml<W> {
    out: W,
    depth: usize,
}

impl<W: Write> Xml<W> {
    fn start(&mut self, tag: &str, attrs: &[(&str, &str)]) -> std::io::Result<()> {
        for _ in 0..self.depth {
            self.out.write_all(b"  ")?;
        }
        write!(self.out, "<{tag}")?;
        for (name, value) in attrs {
            write!(self.out, " {name}=\"")?;
            escape(&mut self.out, value, true)?;
            self.out.write_all(b"\"")?;
        }
        Ok(())
    }

    fn open(&mut self, tag: &str, attrs: &[(&str, &str)]) -> std::io::Result<()> {
        self.start(tag, attrs)?;
        self.out.write_all(b">\n")?;
        self.depth = self.depth.saturating_add(1);
        Ok(())
    }

    fn empty(&mut self, tag: &str, attrs: &[(&str, &str)]) -> std::io::Result<()> {
        self.start(tag, attrs)?;
        self.out.write_all(b"/>\n")
    }

    fn close(&mut self, tag: &str) -> std::io::Result<()> {
        self.depth = self.depth.saturating_sub(1);
        for _ in 0..self.depth {
            self.out.write_all(b"  ")?;
        }
        writeln!(self.out, "</{tag}>")
    }

    fn text(&mut self, tag: &str, text: &str) -> std::io::Result<()> {
        self.start(tag, &[])?;
        self.out.write_all(b">")?;
        escape(&mut self.out, text, false)?;
        writeln!(self.out, "</{tag}>")
    }

    /// An element with attributes and children: `open` when there are children, else `empty`.
    fn element(
        &mut self,
        tag: &str,
        attrs: &[(&str, &str)],
        has_children: bool,
    ) -> std::io::Result<()> {
        if has_children {
            self.open(tag, attrs)
        } else {
            self.empty(tag, attrs)
        }
    }

    fn schema(&mut self, schema: &Schema) -> std::io::Result<()> {
        let mut attrs = vec![("xmlns", EDM), ("Namespace", schema.namespace.as_str())];
        if let Some(alias) = &schema.alias {
            attrs.push(("Alias", alias));
        }
        let has_children = [
            schema.entity_types.is_empty(),
            schema.complex_types.is_empty(),
            schema.terms.is_empty(),
            schema.entity_container.is_none(),
            schema.annotations.is_empty(),
        ]
        .contains(&false);
        self.element("Schema", &attrs, has_children)?;
        if !has_children {
            return Ok(());
        }
        for ty in &schema.entity_types {
            self.entity_type(ty)?;
        }
        for ty in &schema.complex_types {
            let attrs = structured_attrs(&ty.name, None, false, ty.open);
            let has_children = !ty.properties.is_empty() || !ty.annotations.is_empty();
            self.element("ComplexType", &attrs, has_children)?;
            if has_children {
                for property in &ty.properties {
                    self.property(property)?;
                }
                self.annotations(&ty.annotations)?;
                self.close("ComplexType")?;
            }
        }
        for term in &schema.terms {
            self.term(term)?;
        }
        if let Some(container) = &schema.entity_container {
            self.container(container)?;
        }
        self.annotations(&schema.annotations)?;
        self.close("Schema")
    }

    fn entity_type(&mut self, ty: &EntityType) -> std::io::Result<()> {
        let attrs = structured_attrs(&ty.name, ty.base_type.as_deref(), ty.is_abstract, ty.open);
        let has_children = !ty.key.is_empty()
            || !ty.properties.is_empty()
            || !ty.navigation_properties.is_empty()
            || !ty.annotations.is_empty();
        self.element("EntityType", &attrs, has_children)?;
        if !has_children {
            return Ok(());
        }
        if !ty.key.is_empty() {
            self.open("Key", &[])?;
            for key in &ty.key {
                self.empty("PropertyRef", &[("Name", key)])?;
            }
            self.close("Key")?;
        }
        for property in &ty.properties {
            self.property(property)?;
        }
        for navigation in &ty.navigation_properties {
            self.navigation(navigation)?;
        }
        self.annotations(&ty.annotations)?;
        self.close("EntityType")
    }

    fn property(&mut self, property: &Property) -> std::io::Result<()> {
        let ty = type_name(&property.ty);
        let mut attrs = vec![("Name", property.name.as_str()), ("Type", ty.as_str())];
        attrs.extend(nullable(&property.ty, property.nullable));
        let precision = property.precision.map(|p| p.to_string());
        if let Some(precision) = &precision {
            attrs.push(("Precision", precision));
        }
        let scale = property.scale.map(scale_text);
        if let Some(scale) = &scale {
            attrs.push(("Scale", scale));
        }
        let srid = property.srid.map(srid_text);
        if let Some(srid) = &srid {
            attrs.push(("SRID", srid));
        }
        self.annotated("Property", &attrs, &property.annotations)
    }

    fn navigation(&mut self, navigation: &NavigationProperty) -> std::io::Result<()> {
        let ty = type_name(&navigation.ty);
        let mut attrs = vec![("Name", navigation.name.as_str()), ("Type", ty.as_str())];
        if !navigation.ty.collection && !navigation.nullable {
            attrs.push(("Nullable", "false"));
        }
        if let Some(partner) = &navigation.partner {
            attrs.push(("Partner", partner));
        }
        let constraints = &navigation.referential_constraints;
        if constraints.is_empty() {
            return self.annotated("NavigationProperty", &attrs, &navigation.annotations);
        }
        self.open("NavigationProperty", &attrs)?;
        for c in constraints {
            self.empty(
                "ReferentialConstraint",
                &[
                    ("Property", &c.property),
                    ("ReferencedProperty", &c.referenced_property),
                ],
            )?;
        }
        self.annotations(&navigation.annotations)?;
        self.close("NavigationProperty")
    }

    fn term(&mut self, term: &Term) -> std::io::Result<()> {
        let ty = type_name(&term.ty);
        let mut attrs = vec![("Name", term.name.as_str()), ("Type", ty.as_str())];
        attrs.extend(nullable(&term.ty, term.nullable));
        let precision = term.precision.map(|p| p.to_string());
        if let Some(precision) = &precision {
            attrs.push(("Precision", precision));
        }
        let applies_to = term.applies_to.join(" ");
        if !applies_to.is_empty() {
            attrs.push(("AppliesTo", &applies_to));
        }
        self.annotated("Term", &attrs, &term.annotations)
    }

    fn container(&mut self, container: &EntityContainer) -> std::io::Result<()> {
        let attrs = [("Name", container.name.as_str())];
        let has_children = [
            container.entity_sets.is_empty(),
            container.annotations.is_empty(),
        ]
        .contains(&false);
        self.element("EntityContainer", &attrs, has_children)?;
        if !has_children {
            return Ok(());
        }
        for set in &container.entity_sets {
            let attrs = [
                ("Name", set.name.as_str()),
                ("EntityType", set.entity_type.as_str()),
            ];
            let has_children = !set.bindings.is_empty() || !set.annotations.is_empty();
            self.element("EntitySet", &attrs, has_children)?;
            if has_children {
                for binding in &set.bindings {
                    self.empty(
                        "NavigationPropertyBinding",
                        &[("Path", &binding.path), ("Target", &binding.target)],
                    )?;
                }
                self.annotations(&set.annotations)?;
                self.close("EntitySet")?;
            }
        }
        self.annotations(&container.annotations)?;
        self.close("EntityContainer")
    }

    /// An element whose only children are its annotations.
    fn annotated(
        &mut self,
        tag: &str,
        attrs: &[(&str, &str)],
        annotations: &[Annotation],
    ) -> std::io::Result<()> {
        self.element(tag, attrs, !annotations.is_empty())?;
        if annotations.is_empty() {
            return Ok(());
        }
        self.annotations(annotations)?;
        self.close(tag)
    }

    fn annotations(&mut self, annotations: &[Annotation]) -> std::io::Result<()> {
        for annotation in annotations {
            let mut attrs = vec![("Term", annotation.term.as_str())];
            if let Some(qualifier) = &annotation.qualifier {
                attrs.push(("Qualifier", qualifier));
            }
            self.valued("Annotation", &attrs, &annotation.value)?;
        }
        Ok(())
    }

    /// `Annotation` or `PropertyValue`: a constant or path as an attribute, a collection or
    /// record as the one child element.
    fn valued(&mut self, tag: &str, attrs: &[(&str, &str)], value: &Expr) -> std::io::Result<()> {
        if let Shape::Inline(name, text) = shape(value) {
            let mut all = attrs.to_vec();
            all.push((name, &text));
            return self.empty(tag, &all);
        }
        self.open(tag, attrs)?;
        self.expr(value)?;
        self.close(tag)
    }

    /// An expression as an element (inside a collection, or a record under an annotation).
    fn expr(&mut self, value: &Expr) -> std::io::Result<()> {
        match shape(value) {
            Shape::Inline(name, text) => self.text(name, &text),
            Shape::Collection([]) => self.empty("Collection", &[]),
            Shape::Collection(items) => {
                self.open("Collection", &[])?;
                for item in items {
                    self.expr(item)?;
                }
                self.close("Collection")
            }
            Shape::Record([]) => self.empty("Record", &[]),
            Shape::Record(values) => {
                self.open("Record", &[])?;
                for (property, value) in values {
                    self.valued("PropertyValue", &[("Property", property)], value)?;
                }
                self.close("Record")
            }
        }
    }
}

/// How an expression is written: a constant or path inline, as `(element name, text)`.
enum Shape<'a> {
    Inline(&'static str, String),
    Collection(&'a [Expr]),
    Record(&'a [(String, Expr)]),
}

fn shape(value: &Expr) -> Shape<'_> {
    match value {
        Expr::Bool(b) => Shape::Inline("Bool", b.to_string()),
        Expr::Int(i) => Shape::Inline("Int", i.to_string()),
        Expr::String(s) => Shape::Inline("String", line_ends(s)),
        Expr::EnumMember { ty, members } => Shape::Inline(
            "EnumMember",
            members
                .iter()
                .map(|m| format!("{ty}/{m}"))
                .collect::<Vec<_>>()
                .join(" "),
        ),
        Expr::Path(p) => Shape::Inline("Path", p.clone()),
        Expr::PropertyPath(p) => Shape::Inline("PropertyPath", p.clone()),
        Expr::NavigationPropertyPath(p) => Shape::Inline("NavigationPropertyPath", p.clone()),
        Expr::Collection(items) => Shape::Collection(items),
        Expr::Record(values) => Shape::Record(values),
    }
}

fn structured_attrs<'a>(
    name: &'a str,
    base: Option<&'a str>,
    is_abstract: bool,
    open: bool,
) -> Vec<(&'a str, &'a str)> {
    let mut attrs = vec![("Name", name)];
    if let Some(base) = base {
        attrs.push(("BaseType", base));
    }
    if is_abstract {
        attrs.push(("Abstract", "true"));
    }
    if open {
        attrs.push(("OpenType", "true"));
    }
    attrs
}

/// `Nullable` as CSDL XML 7.2.1 reads it: absent is `true` for a single value, so only `false`
/// is written there; for a collection absence means "unknown" in 4.01, so both are written.
fn nullable(ty: &TypeRef, nullable: bool) -> Option<(&'static str, &'static str)> {
    match (ty.collection, nullable) {
        (_, false) => Some(("Nullable", "false")),
        (true, true) => Some(("Nullable", "true")),
        (false, true) => None,
    }
}

fn type_name(ty: &TypeRef) -> String {
    if ty.collection {
        format!("Collection({})", ty.name)
    } else {
        ty.name.clone()
    }
}

pub(crate) fn scale_text(scale: Scale) -> String {
    match scale {
        Scale::Digits(digits) => digits.to_string(),
        Scale::Variable => "variable".to_owned(),
        Scale::Floating => "floating".to_owned(),
    }
}

pub(crate) fn srid_text(srid: Srid) -> String {
    match srid {
        Srid::Id(id) => id.to_string(),
        Srid::Variable => "variable".to_owned(),
    }
}

/// XML-escapes `text`; in an attribute, whitespace other than the space is written as a
/// character reference so attribute-value normalisation cannot change it.
fn escape<W: Write>(out: &mut W, text: &str, attribute: bool) -> std::io::Result<()> {
    let mut buf = [0; 4];
    for c in text.chars() {
        let replacement = match c {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            '\r' => "&#xD;",
            '"' if attribute => "&quot;",
            '\t' if attribute => "&#x9;",
            '\n' if attribute => "&#xA;",
            c => c.encode_utf8(&mut buf),
        };
        out.write_all(replacement.as_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn escaped(text: &str, attribute: bool) -> String {
        let mut out = Vec::new();
        escape(&mut out, text, attribute).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn markup_and_whitespace_are_escaped_where_xml_would_change_them() {
        assert_eq!(escaped("a&b<c>d\"e", true), "a&amp;b&lt;c&gt;d&quot;e");
        assert_eq!(escaped("a&b<c>d\"e", false), "a&amp;b&lt;c&gt;d\"e");
        assert_eq!(escaped("\t\n\r", true), "&#x9;&#xA;&#xD;");
        assert_eq!(escaped("\t\n\r", false), "\t\n&#xD;");
        assert_eq!(escaped("plain ünïcode", true), "plain ünïcode");
        assert_eq!(escaped("", true), "");
    }

    #[test]
    fn nullable_is_written_where_its_absence_would_mean_something_else() {
        assert_eq!(nullable(&TypeRef::single("Edm.String"), true), None);
        assert_eq!(
            nullable(&TypeRef::single("Edm.String"), false),
            Some(("Nullable", "false"))
        );
        assert_eq!(
            nullable(&TypeRef::collection("Edm.String"), true),
            Some(("Nullable", "true"))
        );
        assert_eq!(
            nullable(&TypeRef::collection("Edm.String"), false),
            Some(("Nullable", "false"))
        );
    }

    #[test]
    fn facets_have_their_csdl_spelling() {
        assert_eq!(scale_text(Scale::Digits(3)), "3");
        assert_eq!(scale_text(Scale::Variable), "variable");
        assert_eq!(scale_text(Scale::Floating), "floating");
        assert_eq!(srid_text(Srid::Id(4326)), "4326");
        assert_eq!(srid_text(Srid::Variable), "variable");
    }
}
