// SPDX-License-Identifier: EUPL-1.2
//! CSDL JSON (OData CSDL JSON Representation 4.01).

use std::io::Write;

use crate::error::{CsdlError, Feature};
use crate::model::{
    line_ends, Annotation, CsdlVersion, EntityContainer, EntityType, Expr, Model,
    NavigationProperty, Property, Schema, Term, TypeRef,
};
use crate::validate::{is_temporal, validate, Limits};
use crate::xml::{scale_text, srid_text};

/// Writes `model` as a CSDL JSON document, after [`validate`] accepts it under `limits`.
///
/// CSDL JSON exists for 4.01 only, so a 4.0 model is answered
/// [`CsdlError::NotImplemented`]`(`[`Feature::CsdlJson40`]`)`.
///
/// # Errors
///
/// Whatever [`validate`] answers, the 4.0 refusal above, or [`CsdlError::Io`] when `out` fails.
///
/// ```
/// use odata_csdl::{write_json, CsdlVersion, Limits, Model, Schema};
/// let model = Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![Schema::new("ns")] };
/// let mut out = Vec::new();
/// write_json(&model, &Limits::strict(), &mut out)?;
/// assert_eq!(String::from_utf8(out)?, "{\n  \"$Version\": \"4.01\",\n  \"ns\": {}\n}\n");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn write_json<W: Write>(model: &Model, limits: &Limits, mut out: W) -> Result<(), CsdlError> {
    if model.version == CsdlVersion::V4_0 {
        return Err(CsdlError::NotImplemented(Feature::CsdlJson40));
    }
    validate(model, limits)?;
    let mut document = vec![member("$Version", J::str(model.version.as_str()))];
    if let Some((schema, container)) = model
        .schemas
        .iter()
        .find_map(|s| s.entity_container.as_ref().map(|c| (s, c)))
    {
        document.push(member(
            "$EntityContainer",
            J::Str(format!("{}.{}", schema.namespace, container.name)),
        ));
    }
    if !model.references.is_empty() {
        let references = model
            .references
            .iter()
            .map(|reference| {
                let includes = reference
                    .includes
                    .iter()
                    .map(|include| {
                        let mut members = vec![member("$Namespace", J::str(&include.namespace))];
                        if let Some(alias) = &include.alias {
                            members.push(member("$Alias", J::str(alias)));
                        }
                        J::Obj(members)
                    })
                    .collect();
                (
                    json_uri(&reference.uri),
                    J::Obj(vec![member("$Include", J::Arr(includes))]),
                )
            })
            .collect();
        document.push(member("$Reference", J::Obj(references)));
    }
    for schema in &model.schemas {
        document.push((schema.namespace.clone(), schema_json(schema)));
    }
    J::Obj(document).write(&mut out, 0)?;
    out.write_all(b"\n")?;
    Ok(())
}

/// An ordered JSON value: CSDL JSON members keep the model's order.
enum J {
    Obj(Vec<(String, J)>),
    Arr(Vec<J>),
    Str(String),
    Bool(bool),
    Int(i64),
}

impl J {
    fn str(text: &str) -> Self {
        Self::Str(text.to_owned())
    }

    fn write<W: Write>(&self, out: &mut W, depth: usize) -> std::io::Result<()> {
        match self {
            Self::Obj(members) => {
                if members.is_empty() {
                    return out.write_all(b"{}");
                }
                out.write_all(b"{\n")?;
                let inner = depth.saturating_add(1);
                for (at, (name, value)) in members.iter().enumerate() {
                    if at > 0 {
                        out.write_all(b",\n")?;
                    }
                    indent(out, inner)?;
                    string(out, name)?;
                    out.write_all(b": ")?;
                    value.write(out, inner)?;
                }
                out.write_all(b"\n")?;
                indent(out, depth)?;
                out.write_all(b"}")
            }
            Self::Arr(items) => {
                if items.is_empty() {
                    return out.write_all(b"[]");
                }
                out.write_all(b"[\n")?;
                let inner = depth.saturating_add(1);
                for (at, item) in items.iter().enumerate() {
                    if at > 0 {
                        out.write_all(b",\n")?;
                    }
                    indent(out, inner)?;
                    item.write(out, inner)?;
                }
                out.write_all(b"\n")?;
                indent(out, depth)?;
                out.write_all(b"]")
            }
            Self::Str(text) => string(out, text),
            Self::Bool(b) => write!(out, "{b}"),
            Self::Int(i) => write!(out, "{i}"),
        }
    }
}

fn indent<W: Write>(out: &mut W, depth: usize) -> std::io::Result<()> {
    for _ in 0..depth {
        out.write_all(b"  ")?;
    }
    Ok(())
}

/// A JSON string (RFC 8259 section 7): `"` and `\` escaped, control characters as `\uXXXX`.
fn string<W: Write>(out: &mut W, text: &str) -> std::io::Result<()> {
    out.write_all(b"\"")?;
    let mut buf = [0; 4];
    for c in text.chars() {
        match c {
            '"' => out.write_all(b"\\\"")?,
            '\\' => out.write_all(b"\\\\")?,
            c if c < ' ' => write!(out, "\\u{:04x}", u32::from(c))?,
            c => out.write_all(c.encode_utf8(&mut buf).as_bytes())?,
        }
    }
    out.write_all(b"\"")
}

fn member(name: &str, value: J) -> (String, J) {
    (name.to_owned(), value)
}

fn schema_json(schema: &Schema) -> J {
    let mut members = Vec::new();
    if let Some(alias) = &schema.alias {
        members.push(member("$Alias", J::str(alias)));
    }
    for ty in &schema.entity_types {
        members.push((ty.name.clone(), entity_type(ty)));
    }
    for ty in &schema.complex_types {
        let mut type_members = vec![member("$Kind", J::str("ComplexType"))];
        if ty.open {
            type_members.push(member("$OpenType", J::Bool(true)));
        }
        type_members.extend(ty.properties.iter().map(property));
        annotations(&mut type_members, &ty.annotations);
        members.push((ty.name.clone(), J::Obj(type_members)));
    }
    for term in &schema.terms {
        members.push((term.name.clone(), term_json(term)));
    }
    if let Some(container) = &schema.entity_container {
        members.push((container.name.clone(), container_json(container)));
    }
    annotations(&mut members, &schema.annotations);
    J::Obj(members)
}

fn entity_type(ty: &EntityType) -> J {
    let mut members = vec![member("$Kind", J::str("EntityType"))];
    if let Some(base) = &ty.base_type {
        members.push(member("$BaseType", J::str(base)));
    }
    if ty.is_abstract {
        members.push(member("$Abstract", J::Bool(true)));
    }
    if ty.open {
        members.push(member("$OpenType", J::Bool(true)));
    }
    if !ty.key.is_empty() {
        members.push(member(
            "$Key",
            J::Arr(ty.key.iter().map(|k| J::str(k)).collect()),
        ));
    }
    members.extend(ty.properties.iter().map(property));
    members.extend(ty.navigation_properties.iter().map(navigation));
    annotations(&mut members, &ty.annotations);
    J::Obj(members)
}

/// The JSON form of an OASIS or SAP vocabulary reference names the `.json` file, as the OASIS
/// converter `odata-csdl-xml2json` writes it; any other URI stays as given.
fn json_uri(uri: &str) -> String {
    const VOCABULARIES: [&str; 2] = [
        "https://oasis-tcs.github.io/odata-vocabularies/vocabularies/",
        "https://sap.github.io/odata-vocabularies/vocabularies/",
    ];
    match uri.strip_suffix(".xml") {
        Some(stem) if VOCABULARIES.iter().any(|v| uri.starts_with(v)) => format!("{stem}.json"),
        _ => uri.to_owned(),
    }
}

/// `$Precision` where the model gives one; on a temporal type without one, `0`, since CSDL XML
/// reads that absence as 0 and CSDL JSON as arbitrary precision (CSDL JSON 7.2.3).
fn precision(members: &mut Vec<(String, J)>, ty: &TypeRef, precision: Option<u32>) {
    match precision {
        Some(p) => members.push(member("$Precision", J::Int(i64::from(p)))),
        None if is_temporal(&ty.name) => members.push(member("$Precision", J::Int(0))),
        None => {}
    }
}

/// `$Type` (absent for `Edm.String`), `$Collection` and `$Nullable` (absent means `false`).
fn typed(members: &mut Vec<(String, J)>, ty: &TypeRef, nullable: bool) {
    if ty.name != "Edm.String" {
        members.push(member("$Type", J::str(&ty.name)));
    }
    if ty.collection {
        members.push(member("$Collection", J::Bool(true)));
    }
    if nullable {
        members.push(member("$Nullable", J::Bool(true)));
    }
}

fn property(property: &Property) -> (String, J) {
    let mut members = Vec::new();
    typed(&mut members, &property.ty, property.nullable);
    precision(&mut members, &property.ty, property.precision);
    // CSDL XML reads an absent Scale as 0, CSDL JSON as `variable` (CSDL JSON 7.2.4).
    match property.scale {
        Some(crate::Scale::Variable) => {}
        Some(crate::Scale::Digits(digits)) => {
            members.push(member("$Scale", J::Int(i64::from(digits))));
        }
        Some(other) => members.push(member("$Scale", J::Str(scale_text(other)))),
        None if property.ty.name == "Edm.Decimal" => members.push(member("$Scale", J::Int(0))),
        None => {}
    }
    if let Some(srid) = property.srid {
        members.push(member("$SRID", J::Str(srid_text(srid))));
    }
    annotations(&mut members, &property.annotations);
    (property.name.clone(), J::Obj(members))
}

fn navigation(navigation: &NavigationProperty) -> (String, J) {
    let mut members = vec![
        member("$Kind", J::str("NavigationProperty")),
        member("$Type", J::str(&navigation.ty.name)),
    ];
    if navigation.ty.collection {
        members.push(member("$Collection", J::Bool(true)));
    } else if navigation.nullable {
        members.push(member("$Nullable", J::Bool(true)));
    }
    if let Some(partner) = &navigation.partner {
        members.push(member("$Partner", J::str(partner)));
    }
    if !navigation.referential_constraints.is_empty() {
        let pairs = navigation
            .referential_constraints
            .iter()
            .map(|c| member(&c.property, J::str(&c.referenced_property)))
            .collect();
        members.push(member("$ReferentialConstraint", J::Obj(pairs)));
    }
    annotations(&mut members, &navigation.annotations);
    (navigation.name.clone(), J::Obj(members))
}

fn term_json(term: &Term) -> J {
    let mut members = vec![member("$Kind", J::str("Term"))];
    typed(&mut members, &term.ty, term.nullable);
    precision(&mut members, &term.ty, term.precision);
    // A term has no Scale in the model; CSDL XML reads its absence as 0 (see `property`).
    if term.ty.name == "Edm.Decimal" {
        members.push(member("$Scale", J::Int(0)));
    }
    if !term.applies_to.is_empty() {
        members.push(member(
            "$AppliesTo",
            J::Arr(term.applies_to.iter().map(|a| J::str(a)).collect()),
        ));
    }
    annotations(&mut members, &term.annotations);
    J::Obj(members)
}

fn container_json(container: &EntityContainer) -> J {
    let mut members = vec![member("$Kind", J::str("EntityContainer"))];
    for set in &container.entity_sets {
        let mut set_members = vec![
            member("$Collection", J::Bool(true)),
            member("$Type", J::str(&set.entity_type)),
        ];
        if !set.bindings.is_empty() {
            set_members.push(member(
                "$NavigationPropertyBinding",
                J::Obj(
                    set.bindings
                        .iter()
                        .map(|b| (b.path.clone(), J::str(&b.target)))
                        .collect(),
                ),
            ));
        }
        annotations(&mut set_members, &set.annotations);
        members.push((set.name.clone(), J::Obj(set_members)));
    }
    annotations(&mut members, &container.annotations);
    J::Obj(members)
}

fn annotations(members: &mut Vec<(String, J)>, annotations: &[Annotation]) {
    for annotation in annotations {
        let name = match &annotation.qualifier {
            Some(qualifier) => format!("@{}#{qualifier}", annotation.term),
            None => format!("@{}", annotation.term),
        };
        members.push((name, expr(&annotation.value)));
    }
}

fn expr(value: &Expr) -> J {
    match value {
        Expr::Bool(b) => J::Bool(*b),
        Expr::Int(i) => J::Int(*i),
        Expr::String(s) => J::Str(line_ends(s)),
        Expr::EnumMember { members, .. } => J::Str(members.join(",")),
        Expr::Path(p) => J::Obj(vec![member("$Path", J::str(p))]),
        // CSDL JSON 14.4.1.5 and 14.4.1.6: "represented as a string containing a path".
        Expr::PropertyPath(p) | Expr::NavigationPropertyPath(p) => J::str(p),
        Expr::Collection(items) => J::Arr(items.iter().map(expr).collect()),
        Expr::Record(values) => J::Obj(values.iter().map(|(k, v)| (k.clone(), expr(v))).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(value: &J) -> String {
        let mut out = Vec::new();
        value.write(&mut out, 0).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn strings_escape_quotes_backslashes_and_controls_only() {
        assert_eq!(
            written(&J::str("a\"b\\c\u{1}d\n")),
            "\"a\\\"b\\\\c\\u0001d\\u000a\""
        );
        assert_eq!(written(&J::str("ünï ☃ </")), "\"ünï ☃ </\"");
        assert_eq!(written(&J::str("")), "\"\"");
    }

    #[test]
    fn empty_containers_stay_on_one_line() {
        assert_eq!(written(&J::Obj(vec![])), "{}");
        assert_eq!(written(&J::Arr(vec![])), "[]");
        assert_eq!(
            written(&J::Arr(vec![J::Int(-1), J::Bool(false)])),
            "[\n  -1,\n  false\n]"
        );
    }
}
