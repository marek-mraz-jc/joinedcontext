#![forbid(unsafe_code)]
//! SHACL Core validator (C8) covering:
//! `targetClass`, `targetNode`, `property`, `path` (predicate, sequence, inverse, alternative),
//! `nodeKind`, `class`, `datatype`, `minCount`, `maxCount`, `closed` + `ignoredProperties`,
//! `in`, `hasValue`, `pattern`, `node`, `or`, `and`, `not`, `minLength`, `maxLength`,
//! `languageIn`, `uniqueLang`, `severity`, and `message`.

use chrono::Utc;
use dataroom_core::{
    CatalogueId, OrgId, Result, ShaclFinding, ShaclSeverity, ShaclValidationReport, Texts,
};
use dataroom_rdf::turtle::parse_turtle_checked;
use dataroom_rdf::{ns, RdfGraph, RdfLimits, SubjectNode, Term};
use regex::RegexBuilder;
use std::collections::{BTreeMap, BTreeSet};

const SH: &str = ns::SHACL;
const SH_LEGACY: &str = "http://www.w3.org/ns/shacl#";

fn sh(local: &str) -> String {
    format!("{SH}{local}")
}

/// Property path expression in SHACL (`sh:path`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathExpr {
    Predicate(String),
    Inverse(Box<PathExpr>),
    Sequence(Vec<PathExpr>),
    Alternative(Vec<PathExpr>),
}

impl PathExpr {
    pub fn display_iri(&self) -> String {
        match self {
            Self::Predicate(p) => p.clone(),
            Self::Inverse(inner) => format!("^{}", inner.display_iri()),
            Self::Sequence(items) => items
                .iter()
                .map(Self::display_iri)
                .collect::<Vec<_>>()
                .join("/"),
            Self::Alternative(items) => items
                .iter()
                .map(Self::display_iri)
                .collect::<Vec<_>>()
                .join("|"),
        }
    }

    pub fn evaluate(&self, graph: &RdfGraph, focus: &Term) -> Vec<Term> {
        match self {
            Self::Predicate(pred) => match focus.as_subject() {
                Some(subj) => graph.objects_of(&subj, pred),
                None => Vec::new(),
            },
            Self::Inverse(inner) => match inner.as_ref() {
                Self::Predicate(pred) => graph
                    .subjects_for_object(pred, focus)
                    .into_iter()
                    .map(|s| s.to_term())
                    .collect(),
                _ => Vec::new(),
            },
            Self::Sequence(steps) => {
                let mut current = vec![focus.clone()];
                for step in steps {
                    let mut next = Vec::new();
                    for node in &current {
                        for v in step.evaluate(graph, node) {
                            if !next.contains(&v) {
                                next.push(v);
                            }
                        }
                    }
                    current = next;
                }
                current
            }
            Self::Alternative(branches) => {
                let mut out = Vec::new();
                for branch in branches {
                    for v in branch.evaluate(graph, focus) {
                        if !out.contains(&v) {
                            out.push(v);
                        }
                    }
                }
                out
            }
        }
    }
}

/// Parsed constraints common to NodeShapes and PropertyShapes.
#[derive(Debug, Clone, Default)]
pub struct ShapeConstraints {
    pub node_kind: Option<String>,
    pub classes: Vec<String>,
    pub datatypes: Vec<String>,
    pub min_count: Option<usize>,
    pub max_count: Option<usize>,
    pub min_length: Option<usize>,
    pub max_length: Option<usize>,
    pub pattern: Option<(String, Option<String>)>,
    pub in_values: Option<Vec<Term>>,
    pub has_value: Option<Term>,
    pub language_in: Option<Vec<String>>,
    pub unique_lang: bool,
    pub closed: bool,
    pub ignored_properties: Vec<String>,
    pub node_refs: Vec<SubjectNode>,
    pub or_shapes: Vec<Vec<SubjectNode>>,
    pub and_shapes: Vec<Vec<SubjectNode>>,
    pub not_shapes: Vec<SubjectNode>,
    pub severity: Option<ShaclSeverity>,
    pub message: Texts,
}

/// A parsed `sh:PropertyShape`.
#[derive(Debug, Clone)]
pub struct PropertyShape {
    pub id: SubjectNode,
    pub path: PathExpr,
    pub name: Texts,
    pub description: Texts,
    pub group: Option<String>,
    pub order: i32,
    pub constraints: ShapeConstraints,
}

/// A parsed `sh:NodeShape`.
#[derive(Debug, Clone)]
pub struct NodeShape {
    pub id: SubjectNode,
    pub target_classes: Vec<String>,
    pub target_nodes: Vec<Term>,
    pub properties: Vec<PropertyShape>,
    pub constraints: ShapeConstraints,
}

/// A compiled collection of SHACL shapes.
#[derive(Debug, Clone, Default)]
pub struct ShapesGraph {
    pub node_shapes: BTreeMap<SubjectNode, NodeShape>,
    pub property_shapes: BTreeMap<SubjectNode, PropertyShape>,
}

impl ShapesGraph {
    /// Merges another `ShapesGraph` into `self` (for stacked profiles, C17).
    pub fn merge(&mut self, other: &ShapesGraph) {
        for (k, p) in &other.property_shapes {
            self.property_shapes.insert(k.clone(), p.clone());
        }
        for (k, ns_item) in &other.node_shapes {
            self.node_shapes.insert(k.clone(), ns_item.clone());
        }
    }
}

/// Parses a Turtle document of SHACL shapes into a [`ShapesGraph`].
pub fn parse_shapes_graph(turtle: &str, limits: &RdfLimits) -> Result<ShapesGraph> {
    let g = parse_turtle_checked(turtle, limits)?;
    compile_shapes_from_rdf(&g)
}

/// Compiles a [`ShapesGraph`] from an [`RdfGraph`].
pub fn compile_shapes_from_rdf(g: &RdfGraph) -> Result<ShapesGraph> {
    let mut prop_subjects: BTreeSet<SubjectNode> = g
        .instances_of(&format!("{SH_LEGACY}PropertyShape"))
        .into_iter()
        .collect();
    for t in &g.triples {
        if t.predicate == sh("property") {
            if let Some(s) = t.object.as_subject() {
                prop_subjects.insert(s);
            }
        }
        if t.predicate == sh("path") {
            prop_subjects.insert(t.subject.clone());
        }
    }

    let mut property_shapes = BTreeMap::new();
    for p_subj in prop_subjects {
        if let Some(ps) = parse_property_shape(g, &p_subj) {
            property_shapes.insert(p_subj, ps);
        }
    }

    let mut node_subjects: BTreeSet<SubjectNode> = g
        .instances_of(&format!("{SH_LEGACY}NodeShape"))
        .into_iter()
        .collect();
    for t in &g.triples {
        if matches!(
            t.predicate.as_str(),
            "http://www.w3.org/ns/shacl#targetClass"
                | "http://www.w3.org/ns/shacl#targetNode"
                | "http://www.w3.org/ns/shacl#property"
        ) {
            node_subjects.insert(t.subject.clone());
        }
    }

    let mut node_shapes = BTreeMap::new();
    for n_subj in node_subjects {
        if property_shapes.contains_key(&n_subj)
            && g.objects_of(&n_subj, &sh("targetClass")).is_empty()
        {
            continue;
        }
        let target_classes: Vec<String> = g
            .objects_of(&n_subj, &sh("targetClass"))
            .into_iter()
            .map(|t| t.string_value().to_owned())
            .collect();
        let target_nodes = g.objects_of(&n_subj, &sh("targetNode"));
        let mut props = Vec::new();
        for p_term in g.objects_of(&n_subj, &sh("property")) {
            if let Some(p_subj) = p_term.as_subject() {
                if let Some(ps) = property_shapes.get(&p_subj) {
                    props.push(ps.clone());
                }
            }
        }
        let constraints = parse_constraints(g, &n_subj);
        node_shapes.insert(
            n_subj.clone(),
            NodeShape {
                id: n_subj,
                target_classes,
                target_nodes,
                properties: props,
                constraints,
            },
        );
    }

    Ok(ShapesGraph {
        node_shapes,
        property_shapes,
    })
}

fn parse_property_shape(g: &RdfGraph, subj: &SubjectNode) -> Option<PropertyShape> {
    let path_term = g.first_object(subj, &sh("path"))?;
    let path = parse_path_expr(g, &path_term)?;
    let name = read_texts(g, subj, &sh("name"));
    let description = read_texts(g, subj, &sh("description"));
    let group = g
        .first_object(subj, &sh("group"))
        .map(|t| t.string_value().to_owned());
    let order = g
        .first_object(subj, &sh("order"))
        .and_then(|t| t.string_value().parse::<i32>().ok())
        .unwrap_or(100);
    let constraints = parse_constraints(g, subj);

    Some(PropertyShape {
        id: subj.clone(),
        path,
        name,
        description,
        group,
        order,
        constraints,
    })
}

fn parse_path_expr(g: &RdfGraph, term: &Term) -> Option<PathExpr> {
    match term {
        Term::Iri(iri) => Some(PathExpr::Predicate(iri.clone())),
        Term::Blank(_) => {
            let subj = term.as_subject()?;
            if let Some(inv) = g.first_object(&subj, &sh("inversePath")) {
                return Some(PathExpr::Inverse(Box::new(parse_path_expr(g, &inv)?)));
            }
            if let Some(alt) = g.first_object(&subj, &sh("alternativePath")) {
                let list = g
                    .rdf_list(&alt)
                    .iter()
                    .filter_map(|item| parse_path_expr(g, item))
                    .collect();
                return Some(PathExpr::Alternative(list));
            }
            let seq: Vec<PathExpr> = g
                .rdf_list(term)
                .iter()
                .filter_map(|item| parse_path_expr(g, item))
                .collect();
            if !seq.is_empty() {
                Some(PathExpr::Sequence(seq))
            } else {
                None
            }
        }
        Term::Literal(_) => None,
    }
}

fn parse_constraints(g: &RdfGraph, subj: &SubjectNode) -> ShapeConstraints {
    let node_kind = g
        .first_object(subj, &sh("nodeKind"))
        .map(|t| t.string_value().to_owned());
    let classes: Vec<String> = g
        .objects_of(subj, &sh("class"))
        .into_iter()
        .map(|t| t.string_value().to_owned())
        .collect();
    let datatypes: Vec<String> = g
        .objects_of(subj, &sh("datatype"))
        .into_iter()
        .map(|t| t.string_value().to_owned())
        .collect();
    let min_count = g
        .first_object(subj, &sh("minCount"))
        .and_then(|t| t.string_value().parse::<usize>().ok());
    let max_count = g
        .first_object(subj, &sh("maxCount"))
        .and_then(|t| t.string_value().parse::<usize>().ok());
    let min_length = g
        .first_object(subj, &sh("minLength"))
        .and_then(|t| t.string_value().parse::<usize>().ok());
    let max_length = g
        .first_object(subj, &sh("maxLength"))
        .and_then(|t| t.string_value().parse::<usize>().ok());
    let pattern = g.first_object(subj, &sh("pattern")).map(|p| {
        let flags = g
            .first_object(subj, &sh("flags"))
            .map(|f| f.string_value().to_owned());
        (p.string_value().to_owned(), flags)
    });
    let in_values = g
        .first_object(subj, &sh("in"))
        .map(|head| g.rdf_list(&head));
    let has_value = g.first_object(subj, &sh("hasValue"));
    let language_in = g.first_object(subj, &sh("languageIn")).map(|head| {
        g.rdf_list(&head)
            .into_iter()
            .map(|t| t.string_value().to_ascii_lowercase())
            .collect()
    });
    let unique_lang = g
        .first_object(subj, &sh("uniqueLang"))
        .is_some_and(|t| t.string_value() == "true");
    let closed = g
        .first_object(subj, &sh("closed"))
        .is_some_and(|t| t.string_value() == "true");
    let ignored_properties = g
        .first_object(subj, &sh("ignoredProperties"))
        .map(|head| {
            g.rdf_list(&head)
                .into_iter()
                .map(|t| t.string_value().to_owned())
                .collect()
        })
        .unwrap_or_default();

    let node_refs: Vec<SubjectNode> = g
        .objects_of(subj, &sh("node"))
        .into_iter()
        .filter_map(|t| t.as_subject())
        .collect();
    let or_shapes: Vec<Vec<SubjectNode>> = g
        .objects_of(subj, &sh("or"))
        .into_iter()
        .map(|head| {
            g.rdf_list(&head)
                .into_iter()
                .filter_map(|t| t.as_subject())
                .collect()
        })
        .collect();
    let and_shapes: Vec<Vec<SubjectNode>> = g
        .objects_of(subj, &sh("and"))
        .into_iter()
        .map(|head| {
            g.rdf_list(&head)
                .into_iter()
                .filter_map(|t| t.as_subject())
                .collect()
        })
        .collect();
    let not_shapes: Vec<SubjectNode> = g
        .objects_of(subj, &sh("not"))
        .into_iter()
        .filter_map(|t| t.as_subject())
        .collect();

    let severity = g
        .first_object(subj, &sh("severity"))
        .map(|t| match t.string_value() {
            "http://www.w3.org/ns/shacl#Info" => ShaclSeverity::Info,
            "http://www.w3.org/ns/shacl#Warning" => ShaclSeverity::Warning,
            _ => ShaclSeverity::Violation,
        });
    let message = read_texts(g, subj, &sh("message"));

    ShapeConstraints {
        node_kind,
        classes,
        datatypes,
        min_count,
        max_count,
        min_length,
        max_length,
        pattern,
        in_values,
        has_value,
        language_in,
        unique_lang,
        closed,
        ignored_properties,
        node_refs,
        or_shapes,
        and_shapes,
        not_shapes,
        severity,
        message,
    }
}

fn read_texts(g: &RdfGraph, subj: &SubjectNode, predicate: &str) -> Texts {
    let mut out = BTreeMap::new();
    for term in g.objects_of(subj, predicate) {
        if let Term::Literal(lit) = term {
            let lang = lit.language.unwrap_or_else(|| "en".into());
            out.insert(lang, lit.value);
        }
    }
    out
}

/// Validates a data [`RdfGraph`] against a [`ShapesGraph`] and produces a [`ShaclValidationReport`].
pub fn validate_graph_against_shapes(
    data: &RdfGraph,
    shapes: &ShapesGraph,
    org_id: OrgId,
    catalogue_id: CatalogueId,
    dataset_id: &str,
    profile_id: &str,
    profile_version: &str,
) -> ShaclValidationReport {
    let mut findings = Vec::new();

    for shape in shapes.node_shapes.values() {
        let mut focus_nodes: Vec<Term> = Vec::new();
        for class_iri in &shape.target_classes {
            for s in find_instances_with_subclasses(data, class_iri) {
                let term = s.to_term();
                if !focus_nodes.contains(&term) {
                    focus_nodes.push(term);
                }
            }
        }
        for tn in &shape.target_nodes {
            if !focus_nodes.contains(tn) {
                focus_nodes.push(tn.clone());
            }
        }

        for focus in focus_nodes {
            validate_node_shape(data, shapes, shape, &focus, &mut findings);
        }
    }

    let violation_count = findings
        .iter()
        .filter(|f| f.severity == ShaclSeverity::Violation)
        .count();
    let warning_count = findings
        .iter()
        .filter(|f| f.severity == ShaclSeverity::Warning)
        .count();

    ShaclValidationReport {
        org_id,
        catalogue_id,
        id: format!("{dataset_id}:{profile_id}:{profile_version}"),
        dataset_id: dataset_id.to_owned(),
        profile_id: profile_id.to_owned(),
        profile_version: profile_version.to_owned(),
        conforms: violation_count == 0,
        violation_count,
        warning_count,
        findings,
        checked_at: Utc::now(),
    }
}

fn validate_node_shape(
    data: &RdfGraph,
    shapes: &ShapesGraph,
    shape: &NodeShape,
    focus: &Term,
    findings: &mut Vec<ShaclFinding>,
) {
    let shape_id_str = shape.id.to_string();
    let severity = shape
        .constraints
        .severity
        .unwrap_or(ShaclSeverity::Violation);

    check_value_constraints(
        data,
        shapes,
        &shape.constraints,
        focus,
        focus,
        None,
        &shape_id_str,
        severity,
        findings,
    );

    if shape.constraints.closed {
        if let Some(subj) = focus.as_subject() {
            let mut allowed_preds: BTreeSet<String> = shape
                .constraints
                .ignored_properties
                .iter()
                .cloned()
                .collect();
            for ps in &shape.properties {
                if let PathExpr::Predicate(ref p) = ps.path {
                    allowed_preds.insert(p.clone());
                }
            }
            for t in data.triples.iter().filter(|t| t.subject == subj) {
                if !allowed_preds.contains(&t.predicate) {
                    findings.push(ShaclFinding {
                        focus_node: focus.string_value().to_owned(),
                        result_path: Some(t.predicate.clone()),
                        value: Some(t.object.string_value().to_owned()),
                        source_constraint_component: sh("ClosedConstraintComponent"),
                        source_shape: shape_id_str.clone(),
                        severity,
                        message: default_msg(
                            &shape.constraints.message,
                            format!("Predicate <{}> is not allowed on closed shape", t.predicate),
                        ),
                    });
                }
            }
        }
    }

    // Evaluate all property shapes on `shape`.
    // Note: in `dcat-ap-SHACL.ttl` and `dcat-ap-cz.shacl.ttl`, multiple PropertyShapes on the
    // same NodeShape share the same `sh:path` (e.g. one for `minCount`, one for `maxCount`,
    // one for `nodeKind`). We also check whether any PropertyShape on the same `(NodeShape, path)`
    // declares `sh:datatype rdf:langString`, in which case Dataspecer's `sh:maxCount 1` means
    // at most 1 value per language tag!
    let mut lang_string_paths: BTreeSet<String> = BTreeSet::new();
    for ps in &shape.properties {
        if ps
            .constraints
            .datatypes
            .iter()
            .any(|dt| dt == ns::RDF_LANG_STRING)
        {
            lang_string_paths.insert(ps.path.display_iri());
        }
    }

    for ps in &shape.properties {
        let is_lang_string_prop = lang_string_paths.contains(&ps.path.display_iri());
        validate_property_shape(data, shapes, ps, focus, is_lang_string_prop, findings);
    }
}

fn validate_property_shape(
    data: &RdfGraph,
    shapes: &ShapesGraph,
    ps: &PropertyShape,
    focus: &Term,
    is_lang_string_prop: bool,
    findings: &mut Vec<ShaclFinding>,
) {
    let values = ps.path.evaluate(data, focus);
    let path_str = ps.path.display_iri();
    let shape_id_str = ps.id.to_string();
    let severity = ps.constraints.severity.unwrap_or(ShaclSeverity::Violation);

    if let Some(min_c) = ps.constraints.min_count {
        if values.len() < min_c {
            findings.push(ShaclFinding {
                focus_node: focus.string_value().to_owned(),
                result_path: Some(path_str.clone()),
                value: None,
                source_constraint_component: sh("MinCountConstraintComponent"),
                source_shape: shape_id_str.clone(),
                severity,
                message: default_msg(
                    &ps.constraints.message,
                    format!(
                        "Expected at least {min_c} value(s) for <{path_str}>, found {}",
                        values.len()
                    ),
                ),
            });
        }
    }

    if let Some(max_c) = ps.constraints.max_count {
        if is_lang_string_prop {
            // Dataspecer / multilingual `rdf:langString` rule: `maxCount` applies per language tag.
            let mut by_lang: BTreeMap<String, usize> = BTreeMap::new();
            for v in &values {
                let lang = v
                    .as_literal()
                    .and_then(|l| l.language.clone())
                    .unwrap_or_default();
                *by_lang.entry(lang).or_insert(0) += 1;
            }
            for (lang, count) in by_lang {
                if count > max_c {
                    findings.push(ShaclFinding {
                        focus_node: focus.string_value().to_owned(),
                        result_path: Some(path_str.clone()),
                        value: Some(format!("@{lang} ({count})")),
                        source_constraint_component: sh("MaxCountConstraintComponent"),
                        source_shape: shape_id_str.clone(),
                        severity,
                        message: default_msg(
                            &ps.constraints.message,
                            format!(
                                "Expected at most {max_c} value(s) per language for <{path_str}>, found {count} for @{lang}"
                            ),
                        ),
                    });
                }
            }
        } else if values.len() > max_c {
            findings.push(ShaclFinding {
                focus_node: focus.string_value().to_owned(),
                result_path: Some(path_str.clone()),
                value: None,
                source_constraint_component: sh("MaxCountConstraintComponent"),
                source_shape: shape_id_str.clone(),
                severity,
                message: default_msg(
                    &ps.constraints.message,
                    format!(
                        "Expected at most {max_c} value(s) for <{path_str}>, found {}",
                        values.len()
                    ),
                ),
            });
        }
    }

    if ps.constraints.unique_lang {
        let mut seen_langs = BTreeSet::new();
        for v in &values {
            if let Some(lang) = v.as_literal().and_then(|l| l.language.as_deref()) {
                if !lang.is_empty() && !seen_langs.insert(lang.to_ascii_lowercase()) {
                    findings.push(ShaclFinding {
                        focus_node: focus.string_value().to_owned(),
                        result_path: Some(path_str.clone()),
                        value: Some(format!("@{lang}")),
                        source_constraint_component: sh("UniqueLangConstraintComponent"),
                        source_shape: shape_id_str.clone(),
                        severity,
                        message: default_msg(
                            &ps.constraints.message,
                            format!("Duplicate language tag @{lang} on <{path_str}>"),
                        ),
                    });
                }
            }
        }
    }

    if let Some(ref required_val) = ps.constraints.has_value {
        if !values.contains(required_val) {
            findings.push(ShaclFinding {
                focus_node: focus.string_value().to_owned(),
                result_path: Some(path_str.clone()),
                value: None,
                source_constraint_component: sh("HasValueConstraintComponent"),
                source_shape: shape_id_str.clone(),
                severity,
                message: default_msg(
                    &ps.constraints.message,
                    format!("Missing required value {required_val} on <{path_str}>"),
                ),
            });
        }
    }

    for val in &values {
        check_value_constraints(
            data,
            shapes,
            &ps.constraints,
            focus,
            val,
            Some(&path_str),
            &shape_id_str,
            severity,
            findings,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn check_value_constraints(
    data: &RdfGraph,
    shapes: &ShapesGraph,
    c: &ShapeConstraints,
    focus: &Term,
    val: &Term,
    path: Option<&str>,
    shape_id: &str,
    severity: ShaclSeverity,
    findings: &mut Vec<ShaclFinding>,
) {
    // 1. nodeKind
    if let Some(ref nk) = c.node_kind {
        let ok = match nk.as_str() {
            "http://www.w3.org/ns/shacl#IRI" => matches!(val, Term::Iri(_)),
            "http://www.w3.org/ns/shacl#BlankNode" => matches!(val, Term::Blank(_)),
            "http://www.w3.org/ns/shacl#Literal" => matches!(val, Term::Literal(_)),
            "http://www.w3.org/ns/shacl#BlankNodeOrIRI" => {
                matches!(val, Term::Iri(_) | Term::Blank(_))
            }
            "http://www.w3.org/ns/shacl#BlankNodeOrLiteral" => {
                matches!(val, Term::Blank(_) | Term::Literal(_))
            }
            "http://www.w3.org/ns/shacl#IRIOrLiteral" => {
                matches!(val, Term::Iri(_) | Term::Literal(_))
            }
            _ => true,
        };
        if !ok {
            findings.push(ShaclFinding {
                focus_node: focus.string_value().to_owned(),
                result_path: path.map(str::to_owned),
                value: Some(val.string_value().to_owned()),
                source_constraint_component: sh("NodeKindConstraintComponent"),
                source_shape: shape_id.to_owned(),
                severity,
                message: default_msg(
                    &c.message,
                    format!("Value {val} does not match nodeKind <{nk}>"),
                ),
            });
        }
    }

    // 2. datatype
    for dt in &c.datatypes {
        let ok = match val {
            Term::Literal(lit) => datatype_matches(lit, dt),
            _ => false,
        };
        if !ok {
            findings.push(ShaclFinding {
                focus_node: focus.string_value().to_owned(),
                result_path: path.map(str::to_owned),
                value: Some(val.string_value().to_owned()),
                source_constraint_component: sh("DatatypeConstraintComponent"),
                source_shape: shape_id.to_owned(),
                severity,
                message: default_msg(
                    &c.message,
                    format!("Value {val} does not have datatype <{dt}>"),
                ),
            });
        }
    }

    // 3. class
    for expected_class in &c.classes {
        if expected_class == "http://www.w3.org/2000/01/rdf-schema#Resource" {
            continue;
        }
        let ok = match val.as_subject() {
            Some(subj) => node_satisfies_class(data, &subj, expected_class),
            None => false,
        };
        if !ok {
            findings.push(ShaclFinding {
                focus_node: focus.string_value().to_owned(),
                result_path: path.map(str::to_owned),
                value: Some(val.string_value().to_owned()),
                source_constraint_component: sh("ClassConstraintComponent"),
                source_shape: shape_id.to_owned(),
                severity,
                message: default_msg(
                    &c.message,
                    format!("Value {val} is not an instance of <{expected_class}>"),
                ),
            });
        }
    }

    // 4. minLength / maxLength
    let char_len = val.string_value().chars().count();
    if let Some(min_l) = c.min_length {
        if char_len < min_l {
            findings.push(ShaclFinding {
                focus_node: focus.string_value().to_owned(),
                result_path: path.map(str::to_owned),
                value: Some(val.string_value().to_owned()),
                source_constraint_component: sh("MinLengthConstraintComponent"),
                source_shape: shape_id.to_owned(),
                severity,
                message: default_msg(
                    &c.message,
                    format!("Value length {char_len} is less than minLength {min_l}"),
                ),
            });
        }
    }
    if let Some(max_l) = c.max_length {
        if char_len > max_l {
            findings.push(ShaclFinding {
                focus_node: focus.string_value().to_owned(),
                result_path: path.map(str::to_owned),
                value: Some(val.string_value().to_owned()),
                source_constraint_component: sh("MaxLengthConstraintComponent"),
                source_shape: shape_id.to_owned(),
                severity,
                message: default_msg(
                    &c.message,
                    format!("Value length {char_len} exceeds maxLength {max_l}"),
                ),
            });
        }
    }

    // 5. pattern
    if let Some((ref pat, ref flags)) = c.pattern {
        let mut builder = RegexBuilder::new(pat);
        if let Some(fl) = flags {
            if fl.contains('i') {
                builder.case_insensitive(true);
            }
        }
        if let Ok(re) = builder.build() {
            if !re.is_match(val.string_value()) {
                findings.push(ShaclFinding {
                    focus_node: focus.string_value().to_owned(),
                    result_path: path.map(str::to_owned),
                    value: Some(val.string_value().to_owned()),
                    source_constraint_component: sh("PatternConstraintComponent"),
                    source_shape: shape_id.to_owned(),
                    severity,
                    message: default_msg(
                        &c.message,
                        format!(
                            "Value '{}' does not match pattern /{pat}/",
                            val.string_value()
                        ),
                    ),
                });
            }
        }
    }

    // 6. sh:in
    if let Some(ref allowed) = c.in_values {
        if !allowed.contains(val) {
            findings.push(ShaclFinding {
                focus_node: focus.string_value().to_owned(),
                result_path: path.map(str::to_owned),
                value: Some(val.string_value().to_owned()),
                source_constraint_component: sh("InConstraintComponent"),
                source_shape: shape_id.to_owned(),
                severity,
                message: default_msg(
                    &c.message,
                    format!("Value {val} is not in the allowed sh:in list"),
                ),
            });
        }
    }

    // 7. sh:languageIn
    if let Some(ref langs) = c.language_in {
        let ok = match val.as_literal().and_then(|l| l.language.as_deref()) {
            Some(l) => langs.iter().any(|allowed| {
                l.eq_ignore_ascii_case(allowed)
                    || l.to_ascii_lowercase()
                        .starts_with(&format!("{}-", allowed.to_ascii_lowercase()))
            }),
            None => false,
        };
        if !ok {
            findings.push(ShaclFinding {
                focus_node: focus.string_value().to_owned(),
                result_path: path.map(str::to_owned),
                value: Some(val.string_value().to_owned()),
                source_constraint_component: sh("LanguageInConstraintComponent"),
                source_shape: shape_id.to_owned(),
                severity,
                message: default_msg(
                    &c.message,
                    format!("Value {val} does not have a language in {langs:?}"),
                ),
            });
        }
    }

    // 8. sh:node
    for target_node_shape in &c.node_refs {
        if !conforms_to_shape_ref(data, shapes, target_node_shape, val) {
            findings.push(ShaclFinding {
                focus_node: focus.string_value().to_owned(),
                result_path: path.map(str::to_owned),
                value: Some(val.string_value().to_owned()),
                source_constraint_component: sh("NodeConstraintComponent"),
                source_shape: shape_id.to_owned(),
                severity,
                message: default_msg(
                    &c.message,
                    format!("Value {val} does not conform to shape {target_node_shape}"),
                ),
            });
        }
    }

    // 9. sh:or
    for branch_list in &c.or_shapes {
        if !branch_list
            .iter()
            .any(|s| conforms_to_shape_ref(data, shapes, s, val))
        {
            findings.push(ShaclFinding {
                focus_node: focus.string_value().to_owned(),
                result_path: path.map(str::to_owned),
                value: Some(val.string_value().to_owned()),
                source_constraint_component: sh("OrConstraintComponent"),
                source_shape: shape_id.to_owned(),
                severity,
                message: default_msg(
                    &c.message,
                    format!("Value {val} does not satisfy any branch of sh:or"),
                ),
            });
        }
    }

    // 10. sh:and
    for branch_list in &c.and_shapes {
        if !branch_list
            .iter()
            .all(|s| conforms_to_shape_ref(data, shapes, s, val))
        {
            findings.push(ShaclFinding {
                focus_node: focus.string_value().to_owned(),
                result_path: path.map(str::to_owned),
                value: Some(val.string_value().to_owned()),
                source_constraint_component: sh("AndConstraintComponent"),
                source_shape: shape_id.to_owned(),
                severity,
                message: default_msg(
                    &c.message,
                    format!("Value {val} does not satisfy all branches of sh:and"),
                ),
            });
        }
    }

    // 11. sh:not
    for neg_shape in &c.not_shapes {
        if conforms_to_shape_ref(data, shapes, neg_shape, val) {
            findings.push(ShaclFinding {
                focus_node: focus.string_value().to_owned(),
                result_path: path.map(str::to_owned),
                value: Some(val.string_value().to_owned()),
                source_constraint_component: sh("NotConstraintComponent"),
                source_shape: shape_id.to_owned(),
                severity,
                message: default_msg(
                    &c.message,
                    format!("Value {val} must not conform to shape {neg_shape}"),
                ),
            });
        }
    }
}

fn conforms_to_shape_ref(
    data: &RdfGraph,
    shapes: &ShapesGraph,
    shape_ref: &SubjectNode,
    val: &Term,
) -> bool {
    let mut temp_findings = Vec::new();
    if let Some(ns_item) = shapes.node_shapes.get(shape_ref) {
        validate_node_shape(data, shapes, ns_item, val, &mut temp_findings);
    } else if let Some(ps_item) = shapes.property_shapes.get(shape_ref) {
        validate_property_shape(data, shapes, ps_item, val, false, &mut temp_findings);
    }
    !temp_findings
        .iter()
        .any(|f| f.severity == ShaclSeverity::Violation)
}

fn datatype_matches(lit: &dataroom_rdf::LiteralValue, expected_dt: &str) -> bool {
    if expected_dt == ns::RDF_LANG_STRING {
        return lit.language.as_ref().is_some_and(|l| !l.is_empty());
    }
    if lit.datatype != expected_dt {
        return false;
    }
    match expected_dt {
        ns::XSD_INTEGER => lit.value.parse::<i128>().is_ok(),
        ns::XSD_NON_NEGATIVE_INTEGER => lit.value.parse::<u128>().is_ok(),
        ns::XSD_DECIMAL | ns::XSD_DOUBLE => lit.value.parse::<f64>().is_ok(),
        ns::XSD_BOOLEAN => matches!(lit.value.as_str(), "true" | "false" | "1" | "0"),
        ns::XSD_DATE => lit
            .value
            .get(..10)
            .is_some_and(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()),
        ns::XSD_HEX_BINARY => {
            lit.value.len().is_multiple_of(2) && lit.value.bytes().all(|b| b.is_ascii_hexdigit())
        }
        _ => true,
    }
}

fn subclasses_of(class_iri: &str) -> Vec<&str> {
    match class_iri {
        "http://www.w3.org/2006/vcard/ns#Kind" => vec![
            "http://www.w3.org/2006/vcard/ns#Kind",
            "http://www.w3.org/2006/vcard/ns#Organization",
            "http://www.w3.org/2006/vcard/ns#Individual",
            "http://www.w3.org/2006/vcard/ns#Group",
            "http://www.w3.org/2006/vcard/ns#Location",
        ],
        "http://www.w3.org/ns/dcat#Resource" => vec![
            "http://www.w3.org/ns/dcat#Resource",
            "http://www.w3.org/ns/dcat#Dataset",
            "http://www.w3.org/ns/dcat#DatasetSeries",
            "http://www.w3.org/ns/dcat#DataService",
            "http://www.w3.org/ns/dcat#Catalog",
        ],
        // Fix A3: in DCAT-AP 3.0.1 and DCAT-AP-SK 2.0, `dcat:Catalog` is a subclass of `dcat:Resource`
        // (and in OWL of dcat:Dataset), but SHACL shapes in `dcat-ap-SHACL.ttl` and
        // `dcat-ap-sk-2.0.ttl` define separate `CatalogShape` (`sh:targetClass dcat:Catalog`) and
        // `DatasetShape` (`sh:targetClass dcat:Dataset`). Treating `dcat:Catalog` as a target of
        // `DatasetShape` wrongly applied dataset-only mandatory properties (`dcat:keyword`,
        // `dcat:theme`, `dct:accrualPeriodicity`, `dcat:distribution`) to `dcat:Catalog` nodes,
        // causing `specs/dcat-ap-sk/príklady/katalóg.ttl` to fail `dcat:keyword minCount 1`!
        "http://www.w3.org/ns/dcat#Dataset" => vec!["http://www.w3.org/ns/dcat#Dataset"],
        "http://xmlns.com/foaf/0.1/Agent" => vec![
            "http://xmlns.com/foaf/0.1/Agent",
            "http://xmlns.com/foaf/0.1/Organization",
            "http://xmlns.com/foaf/0.1/Person",
            "http://xmlns.com/foaf/0.1/Group",
        ],
        "http://purl.org/dc/terms/LicenseDocument" => vec![
            "http://purl.org/dc/terms/LicenseDocument",
            "https://data.gov.cz/slovník/podmínky-užití/Specifikace",
            "https://data.gov.sk/def/ontology/legislation/TermsOfUse",
        ],
        "http://purl.org/dc/terms/RightsStatement" => vec![
            "http://purl.org/dc/terms/RightsStatement",
            "http://purl.org/dc/terms/LicenseDocument",
            "https://data.gov.cz/slovník/podmínky-užití/Specifikace",
            "https://data.gov.sk/def/ontology/legislation/TermsOfUse",
        ],
        "http://purl.org/dc/terms/MediaTypeOrExtent" => vec![
            "http://purl.org/dc/terms/MediaTypeOrExtent",
            "http://purl.org/dc/terms/MediaType",
        ],
        other => vec![other],
    }
}

fn find_instances_with_subclasses(data: &RdfGraph, class_iri: &str) -> Vec<SubjectNode> {
    let mut out = Vec::new();
    for cls in subclasses_of(class_iri) {
        for s in data.instances_of(cls) {
            if !out.contains(&s) {
                out.push(s);
            }
        }
    }
    out
}

fn node_satisfies_class(data: &RdfGraph, subj: &SubjectNode, expected_class: &str) -> bool {
    let actual_types = data.objects_of(subj, ns::RDF_TYPE);
    if actual_types.is_empty() {
        // An untyped external IRI (e.g. EU authority code or OVM IRI) satisfies open-world
        // class references in DCAT-AP graphs unless it is an untyped blank node.
        return matches!(subj, SubjectNode::Named(_));
    }
    let allowed = subclasses_of(expected_class);
    actual_types
        .iter()
        .any(|t| allowed.contains(&t.string_value()))
}

fn default_msg(msg: &Texts, fallback: String) -> Texts {
    if msg.is_empty() {
        BTreeMap::from([("en".into(), fallback)])
    } else {
        msg.clone()
    }
}
