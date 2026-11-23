#![forbid(unsafe_code)]
//! Bidirectional DCAT-AP graph mapping under any installed profile ([`crate::profile`]), Hydra
//! pagination, and W3C `Accept-Profile` / `Accept` content negotiation (C7, C17).

use crate::graph::{ns, LiteralValue, RdfGraph, RdfLimits, SubjectNode, Term};
use crate::jsonld::{
    parse_jsonld_bytes, serialize_compact_catalogue, serialize_compact_dataset,
    serialize_graph_jsonld,
};
use crate::profile::{DcatProfile, Subject, Terms, ValueKind};
use crate::rdfxml::{parse_rdfxml_bytes, serialize_rdfxml};
use crate::turtle::{
    parse_ntriples_bytes, parse_turtle_bytes, serialize_ntriples, serialize_turtle,
};
use dataroom_core::vocab::{
    eu_iri_to_language, find_licence, format_to_eu_iri, frequency_to_eu_iri,
    iana_iri_to_media_type, language_to_eu_iri, media_type_to_iana_iri, theme_code, theme_to_iri,
};
use dataroom_core::{
    is_declared_iri, AgentDetails, Catalogue, CatalogueId, ContactKind, ContactPoint, DataService,
    Dataset, Distribution, OrgId, Publisher, RecordState, TemporalPeriod, TermsOfUse, Texts,
    Visibility, PERSONAL_DATA, PERSONAL_DATA_CONTAINS, PERSONAL_DATA_NONE,
};
use jc_time::{parse_date, parse_rfc3339, Date, TimestampTz};
use std::collections::BTreeMap;
use uuid::Uuid;

/// Supported RDF serialization formats (C7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RdfFormat {
    Turtle,
    JsonLd,
    RdfXml,
    NTriples,
}

impl RdfFormat {
    pub const fn content_type(self) -> &'static str {
        match self {
            Self::Turtle => "text/turtle; charset=utf-8",
            Self::JsonLd => "application/ld+json; charset=utf-8",
            Self::RdfXml => "application/rdf+xml; charset=utf-8",
            Self::NTriples => "application/n-triples; charset=utf-8",
        }
    }

    /// The suffix of a feed in this format.
    pub const fn suffix(self) -> &'static str {
        match self {
            Self::Turtle => "ttl",
            Self::JsonLd => "jsonld",
            Self::RdfXml => "rdf",
            Self::NTriples => "nt",
        }
    }

    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
            "ttl" | "turtle" | "n3" => Some(Self::Turtle),
            "jsonld" | "json-ld" | "json" => Some(Self::JsonLd),
            "rdf" | "xml" => Some(Self::RdfXml),
            "nt" | "ntriples" => Some(Self::NTriples),
            _ => None,
        }
    }
}

/// The values of an `Accept`-style header, highest `q` first (ties keep their order), `q=0` and
/// unreadable weights dropped (RFC 9110 section 12.4.2).
fn by_quality(header: &str) -> Vec<&str> {
    let mut items: Vec<(f32, &str)> = header
        .split(',')
        .filter_map(|part| {
            let mut params = part.split(';');
            let value = params.next()?.trim();
            let q = match params.find_map(|p| {
                let (k, v) = p.split_once('=')?;
                k.trim().eq_ignore_ascii_case("q").then(|| v.trim())
            }) {
                Some(q) => q.parse::<f32>().ok().filter(|q| (0.0..=1.0).contains(q))?,
                None => 1.0,
            };
            (q > 0.0 && !value.is_empty()).then_some((q, value))
        })
        .collect();
    items.sort_by(|a, b| b.0.total_cmp(&a.0));
    items.into_iter().map(|(_, v)| v).collect()
}

/// Negotiates an [`RdfFormat`] from an optional URL extension or `Accept` header.
pub fn negotiate_rdf_format(ext: Option<&str>, accept: Option<&str>) -> Option<RdfFormat> {
    if let Some(ext) = ext {
        if let Some(fmt) = RdfFormat::from_extension(ext) {
            return Some(fmt);
        }
    }
    for media in by_quality(accept?) {
        match media.to_ascii_lowercase().as_str() {
            "text/turtle" | "application/x-turtle" | "text/n3" => return Some(RdfFormat::Turtle),
            "application/ld+json" => return Some(RdfFormat::JsonLd),
            "application/rdf+xml" | "application/xml" => return Some(RdfFormat::RdfXml),
            "application/n-triples" => return Some(RdfFormat::NTriples),
            "text/html" => return None,
            _ => {}
        }
    }
    None
}

/// The profiles an `Accept-Profile` header names, best first (W3C Content Negotiation by
/// Profile, C17).
pub fn accepted_profiles(header: &str) -> Vec<&str> {
    by_quality(header)
}

/// Hydra pagination metadata for paged catalogue feeds (`hydra:PartialCollectionView` / `hydra:PagedCollection`, C7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HydraPageInfo {
    pub collection_uri: String,
    pub current_page_uri: String,
    pub total_items: usize,
    pub items_per_page: usize,
    pub first_page_uri: String,
    pub last_page_uri: String,
    pub next_page_uri: Option<String>,
    pub prev_page_uri: Option<String>,
}

/// Converts a [`Dataset`] into an [`RdfGraph`] according to `profile`.
pub fn dataset_to_graph(
    dataset: &Dataset,
    catalogue: &Catalogue,
    publisher: Option<&Publisher>,
    profile: DcatProfile,
) -> RdfGraph {
    let mut g = RdfGraph::new();
    add_dataset_triples(&mut g, dataset, catalogue, publisher, profile);
    type_values(&mut g, profile);
    g
}

/// States the classes the profile wants on the values of some predicates.
fn type_values(g: &mut RdfGraph, profile: DcatProfile) {
    let Some(classes) = &profile.render.value_classes else {
        return;
    };
    let typed: Vec<(String, String)> = g
        .triples
        .iter()
        .filter_map(|t| match (&t.object, classes.get(&t.predicate)) {
            (Term::Iri(value), Some(class)) => Some((value.clone(), class.clone())),
            _ => None,
        })
        .collect();
    for (value, class) in typed {
        g.insert(SubjectNode::Named(value), ns::RDF_TYPE, Term::Iri(class));
    }
}

pub fn add_dataset_triples(
    g: &mut RdfGraph,
    dataset: &Dataset,
    catalogue: &Catalogue,
    publisher: Option<&Publisher>,
    profile: DcatProfile,
) -> SubjectNode {
    let base = catalogue.base_url();
    let ds_iri = dataset.canonical_uri(&base);
    let ds_subj = SubjectNode::Named(ds_iri.clone());

    let rdf_class = if dataset.dataset_type == "dataset_series" {
        "http://www.w3.org/ns/dcat#DatasetSeries"
    } else {
        "http://www.w3.org/ns/dcat#Dataset"
    };
    g.insert(
        ds_subj.clone(),
        ns::RDF_TYPE,
        Term::Iri(rdf_class.to_owned()),
    );

    for (lang, text) in &dataset.title {
        if !text.is_empty() {
            g.insert(
                ds_subj.clone(),
                format!("{}title", ns::DCT),
                Term::Literal(LiteralValue::lang(text.clone(), lang.clone())),
            );
        }
    }
    for (lang, text) in &dataset.description {
        if !text.is_empty() {
            g.insert(
                ds_subj.clone(),
                format!("{}description", ns::DCT),
                Term::Literal(LiteralValue::lang(text.clone(), lang.clone())),
            );
        }
    }

    // Identifier
    let ident = dataset.identifier.as_deref().unwrap_or(&dataset.name);
    let rules = &profile.render;
    if rules.identifier == Some(true) {
        g.insert(
            ds_subj.clone(),
            format!("{}identifier", ns::DCT),
            Term::Literal(LiteralValue::plain(ident)),
        );
    }

    // Publisher (`dct:publisher`)
    if let Some((iri, names)) = dataset_publisher(dataset, catalogue, publisher) {
        insert_agent(g, &ds_subj, iri, &names);
    }

    // Contact point (`dcat:contactPoint`)
    if let Some(cp) = &dataset.contact_point {
        let cp_subj = match &cp.uri {
            Some(u) if !u.is_empty() => SubjectNode::Named(u.clone()),
            _ => g.fresh_blank_node(),
        };
        g.insert(
            ds_subj.clone(),
            format!("{}contactPoint", ns::DCAT),
            cp_subj.to_term(),
        );
        // vcard:Organization and vcard:Individual are subclasses of the vcard:Kind every
        // profile asks for; with vcard:Kind alone an Individual read back as an Organization.
        let vcard_class = match cp.kind {
            ContactKind::Organization => format!("{}Organization", ns::VCARD),
            ContactKind::Individual => format!("{}Individual", ns::VCARD),
        };
        g.insert(cp_subj.clone(), ns::RDF_TYPE, Term::Iri(vcard_class));
        g.insert(
            cp_subj.clone(),
            ns::RDF_TYPE,
            Term::Iri(format!("{}Kind", ns::VCARD)),
        );
        for (lang, name) in &cp.name {
            if !name.is_empty() {
                g.insert(
                    cp_subj.clone(),
                    format!("{}fn", ns::VCARD),
                    Term::Literal(LiteralValue::lang(name.clone(), lang.clone())),
                );
            }
        }
        if !cp.email.is_empty() {
            let mailto = if cp.email.starts_with("mailto:") {
                cp.email.clone()
            } else {
                format!("mailto:{}", cp.email)
            };
            g.insert(cp_subj, format!("{}hasEmail", ns::VCARD), Term::Iri(mailto));
        }
    }

    // Keywords (`dcat:keyword`)
    if !dataset.keywords.is_empty() {
        for (lang, words) in &dataset.keywords {
            for w in words {
                g.insert(
                    ds_subj.clone(),
                    format!("{}keyword", ns::DCAT),
                    Term::Literal(LiteralValue::lang(w.clone(), lang.clone())),
                );
            }
        }
    } else {
        for t in &dataset.tags {
            g.insert(
                ds_subj.clone(),
                format!("{}keyword", ns::DCAT),
                Term::Literal(LiteralValue::lang(
                    t.clone(),
                    catalogue.default_language.clone(),
                )),
            );
        }
    }

    // Themes (`dcat:theme`)
    for theme in &dataset.themes {
        let t_iri = theme_to_iri(theme);
        let t_subj = SubjectNode::Named(t_iri.clone());
        g.insert(
            ds_subj.clone(),
            format!("{}theme", ns::DCAT),
            Term::Iri(t_iri),
        );
        g.insert(
            t_subj.clone(),
            ns::RDF_TYPE,
            Term::Iri(format!("{}Concept", ns::SKOS)),
        );
        let code = theme_code(theme);
        g.insert(
            t_subj,
            format!("{}prefLabel", ns::SKOS),
            Term::Literal(LiteralValue::lang(
                dataroom_core::vocab::theme_label(&code, "en"),
                "en",
            )),
        );
    }
    for ev in &dataset.eurovoc_themes {
        g.insert(
            ds_subj.clone(),
            format!("{}theme", ns::DCAT),
            Term::Iri(ev.clone()),
        );
    }

    // Frequency (`dct:accrualPeriodicity`)
    if let Some(freq) = &dataset.frequency {
        let f_iri = frequency_to_eu_iri(freq);
        let f_subj = SubjectNode::Named(f_iri.clone());
        g.insert(
            ds_subj.clone(),
            format!("{}accrualPeriodicity", ns::DCT),
            Term::Iri(f_iri),
        );
        g.insert(
            f_subj,
            ns::RDF_TYPE,
            Term::Iri(format!("{}Frequency", ns::DCT)),
        );
    }

    // Spatial (`dct:spatial`)
    for sp in &dataset.spatial {
        let sp_iri = if sp.starts_with("http://") || sp.starts_with("https://") {
            sp.clone()
        } else {
            format!("http://data.europa.eu/nuts/code/{sp}")
        };
        let sp_subj = SubjectNode::Named(sp_iri.clone());
        g.insert(
            ds_subj.clone(),
            format!("{}spatial", ns::DCT),
            Term::Iri(sp_iri),
        );
        g.insert(
            sp_subj,
            ns::RDF_TYPE,
            Term::Iri(format!("{}Location", ns::DCT)),
        );
    }

    // Temporal (`dct:temporal`)
    if let Some(temp) = &dataset.temporal {
        if temp.start.is_some() || temp.end.is_some() {
            let period = g.fresh_blank_node();
            g.insert(
                ds_subj.clone(),
                format!("{}temporal", ns::DCT),
                period.to_term(),
            );
            g.insert(
                period.clone(),
                ns::RDF_TYPE,
                Term::Iri(format!("{}PeriodOfTime", ns::DCT)),
            );
            if let Some(start) = temp.start {
                g.insert(
                    period.clone(),
                    format!("{}startDate", ns::DCAT),
                    Term::Literal(LiteralValue::typed(start.to_string(), ns::XSD_DATE)),
                );
            }
            if let Some(end) = temp.end {
                g.insert(
                    period,
                    format!("{}endDate", ns::DCAT),
                    Term::Literal(LiteralValue::typed(end.to_string(), ns::XSD_DATE)),
                );
            }
        }
    }

    // Resolutions
    if let Some(meters) = dataset.spatial_resolution_in_meters {
        let formatted = if meters.fract() == 0.0 {
            format!("{meters:.1}")
        } else {
            meters.to_string()
        };
        g.insert(
            ds_subj.clone(),
            format!("{}spatialResolutionInMeters", ns::DCAT),
            Term::Literal(LiteralValue::typed(formatted, ns::XSD_DECIMAL)),
        );
    }
    if let Some(dur) = &dataset.temporal_resolution {
        g.insert(
            ds_subj.clone(),
            format!("{}temporalResolution", ns::DCAT),
            Term::Literal(LiteralValue::typed(dur.clone(), ns::XSD_DURATION)),
        );
    }

    // Landing page, documentation, conformsTo, applicableLegislation, inSeries
    if let Some(lp) = &dataset.landing_page {
        g.insert(
            ds_subj.clone(),
            format!("{}landingPage", ns::DCAT),
            Term::Iri(lp.clone()),
        );
        g.insert(
            SubjectNode::Named(lp.clone()),
            ns::RDF_TYPE,
            Term::Iri(format!("{}Document", ns::FOAF)),
        );
    }
    for doc in &dataset.documentation {
        g.insert(
            ds_subj.clone(),
            format!("{}page", ns::FOAF),
            Term::Iri(doc.clone()),
        );
        g.insert(
            SubjectNode::Named(doc.clone()),
            ns::RDF_TYPE,
            Term::Iri(format!("{}Document", ns::FOAF)),
        );
    }
    for std_iri in &dataset.conforms_to {
        g.insert(
            ds_subj.clone(),
            format!("{}conformsTo", ns::DCT),
            Term::Iri(std_iri.clone()),
        );
        g.insert(
            SubjectNode::Named(std_iri.clone()),
            ns::RDF_TYPE,
            Term::Iri(format!("{}Standard", ns::DCT)),
        );
    }
    for law in &dataset.applicable_legislation {
        g.insert(
            ds_subj.clone(),
            format!("{}applicableLegislation", ns::DCATAP),
            Term::Iri(law.clone()),
        );
        g.insert(
            SubjectNode::Named(law.clone()),
            ns::RDF_TYPE,
            Term::Iri(format!("{}LegalResource", ns::ELI)),
        );
    }
    if let Some(series) = &dataset.in_series {
        g.insert(
            ds_subj.clone(),
            format!("{}inSeries", ns::DCAT),
            Term::Iri(series.clone()),
        );
    }
    for lang in &dataset.languages {
        let l_iri = language_to_eu_iri(lang);
        g.insert(
            ds_subj.clone(),
            format!("{}language", ns::DCT),
            Term::Iri(l_iri.clone()),
        );
        g.insert(
            SubjectNode::Named(l_iri),
            ns::RDF_TYPE,
            Term::Iri(format!("{}LinguisticSystem", ns::DCT)),
        );
    }

    if rules.issued_modified == Some(true) {
        g.insert(
            ds_subj.clone(),
            format!("{}issued", ns::DCT),
            Term::Literal(LiteralValue::typed(
                dataset.issued.0.strftime("%Y-%m-%dT%H:%M:%SZ").to_string(),
                ns::XSD_DATE_TIME,
            )),
        );
        g.insert(
            ds_subj.clone(),
            format!("{}modified", ns::DCT),
            Term::Literal(LiteralValue::typed(
                dataset
                    .modified
                    .0
                    .strftime("%Y-%m-%dT%H:%M:%SZ")
                    .to_string(),
                ns::XSD_DATE_TIME,
            )),
        );
    }

    // Distributions (`dcat:distribution`)
    for dist in &dataset.distributions {
        let dist_iri = dist
            .uri
            .clone()
            .unwrap_or_else(|| format!("{ds_iri}/resource/{}", dist.id));
        let dist_subj = SubjectNode::Named(dist_iri.clone());
        g.insert(
            ds_subj.clone(),
            format!("{}distribution", ns::DCAT),
            Term::Iri(dist_iri),
        );
        g.insert(
            dist_subj.clone(),
            ns::RDF_TYPE,
            Term::Iri(format!("{}Distribution", ns::DCAT)),
        );

        for (lang, t) in &dist.title {
            if !t.is_empty() {
                g.insert(
                    dist_subj.clone(),
                    format!("{}title", ns::DCT),
                    Term::Literal(LiteralValue::lang(t.clone(), lang.clone())),
                );
            }
        }
        for (lang, d) in &dist.description {
            if !d.is_empty() {
                g.insert(
                    dist_subj.clone(),
                    format!("{}description", ns::DCT),
                    Term::Literal(LiteralValue::lang(d.clone(), lang.clone())),
                );
            }
        }

        g.insert(
            dist_subj.clone(),
            format!("{}accessURL", ns::DCAT),
            Term::Iri(dist.access_url.clone()),
        );
        if let Some(dl) = &dist.download_url {
            g.insert(
                dist_subj.clone(),
                format!("{}downloadURL", ns::DCAT),
                Term::Iri(dl.clone()),
            );
        }
        if let Some(fmt) = &dist.format {
            let f_iri = format_to_eu_iri(fmt);
            g.insert(
                dist_subj.clone(),
                format!("{}format", ns::DCT),
                Term::Iri(f_iri.clone()),
            );
            g.insert(
                SubjectNode::Named(f_iri),
                ns::RDF_TYPE,
                Term::Iri(format!("{}MediaTypeOrExtent", ns::DCT)),
            );
        }
        if let Some(mt) = &dist.media_type {
            let m_iri = media_type_to_iana_iri(mt);
            g.insert(
                dist_subj.clone(),
                format!("{}mediaType", ns::DCAT),
                Term::Iri(m_iri.clone()),
            );
            g.insert(
                SubjectNode::Named(m_iri),
                ns::RDF_TYPE,
                Term::Iri(format!("{}MediaType", ns::DCT)),
            );
        }
        if let Some(cf) = &dist.compress_format {
            let c_iri = media_type_to_iana_iri(cf);
            g.insert(
                dist_subj.clone(),
                format!("{}compressFormat", ns::DCAT),
                Term::Iri(c_iri.clone()),
            );
            g.insert(
                SubjectNode::Named(c_iri),
                ns::RDF_TYPE,
                Term::Iri(format!("{}MediaType", ns::DCT)),
            );
        }
        if let Some(pf) = &dist.package_format {
            let p_iri = media_type_to_iana_iri(pf);
            g.insert(
                dist_subj.clone(),
                format!("{}packageFormat", ns::DCAT),
                Term::Iri(p_iri.clone()),
            );
            g.insert(
                SubjectNode::Named(p_iri),
                ns::RDF_TYPE,
                Term::Iri(format!("{}MediaType", ns::DCT)),
            );
        }
        if let Some(bytes) = dist.byte_size {
            g.insert(
                dist_subj.clone(),
                format!("{}byteSize", ns::DCAT),
                Term::Literal(LiteralValue::typed(
                    bytes.to_string(),
                    ns::XSD_NON_NEGATIVE_INTEGER,
                )),
            );
        }
        // A checksum without its algorithm cannot be checked, and no shape admits the node.
        let algorithm = checksum_algorithm(dist).filter(|algorithm| {
            rules
                .checksum_algorithms
                .as_ref()
                .is_none_or(|admitted| admitted.contains(algorithm))
        });
        if let (Some(hash), Some(algo_iri)) = (&dist.hash, algorithm) {
            if !hash.is_empty() {
                let cs_node = g.fresh_blank_node();
                g.insert(
                    dist_subj.clone(),
                    format!("{}checksum", ns::SPDX),
                    cs_node.to_term(),
                );
                g.insert(
                    cs_node.clone(),
                    ns::RDF_TYPE,
                    Term::Iri(format!("{}Checksum", ns::SPDX)),
                );
                g.insert(
                    cs_node.clone(),
                    format!("{}checksumValue", ns::SPDX),
                    Term::Literal(LiteralValue::typed(hash.clone(), ns::XSD_HEX_BINARY)),
                );
                g.insert(
                    cs_node,
                    format!("{}algorithm", ns::SPDX),
                    Term::Iri(algo_iri.clone()),
                );
                g.insert(
                    SubjectNode::Named(algo_iri),
                    ns::RDF_TYPE,
                    Term::Iri(format!("{}ChecksumAlgorithm", ns::SPDX)),
                );
            }
        }
        for c in &dist.conforms_to {
            g.insert(
                dist_subj.clone(),
                format!("{}conformsTo", ns::DCT),
                Term::Iri(c.clone()),
            );
            g.insert(
                SubjectNode::Named(c.clone()),
                ns::RDF_TYPE,
                Term::Iri(format!("{}Standard", ns::DCT)),
            );
        }
        // A profile may repeat the dataset's legislation on each distribution and service.
        let inherited_laws: &[String] = if rules.inherit_legislation == Some(true) {
            &dataset.applicable_legislation
        } else {
            &[]
        };
        for law in dist.applicable_legislation.iter().chain(inherited_laws) {
            g.insert(
                dist_subj.clone(),
                format!("{}applicableLegislation", ns::DCATAP),
                Term::Iri(law.clone()),
            );
            g.insert(
                SubjectNode::Named(law.clone()),
                ns::RDF_TYPE,
                Term::Iri(format!("{}LegalResource", ns::ELI)),
            );
        }

        // The licence, as the profile states it
        if rules.licence == Some(true) {
            let stated = distribution_licence(dataset, dist);
            let iri = match stated.and_then(find_licence) {
                Some(known) => Some(known.eu_iri),
                None => stated.filter(|s| s.contains("://")).map(str::to_owned),
            };
            if let Some(iri) = iri {
                g.insert(
                    dist_subj.clone(),
                    format!("{}license", ns::DCT),
                    Term::Iri(iri.clone()),
                );
                g.insert(
                    SubjectNode::Named(iri),
                    ns::RDF_TYPE,
                    Term::Iri(format!("{}LicenseDocument", ns::DCT)),
                );
            }
        }
        if let Some(node) = &rules.terms {
            if let Some(stated) = terms_of_use(dataset, dist, profile) {
                insert_terms(g, &dist_subj, node, &stated);
            }
        }
        insert_properties(g, &dist_subj, Subject::Distribution, dataset, profile);

        // DataServices (`dcat:accessService`)
        for svc in &dist.access_services {
            let svc_iri = svc
                .uri
                .clone()
                .unwrap_or_else(|| format!("{ds_iri}/service/{}", svc.id));
            let svc_subj = SubjectNode::Named(svc_iri.clone());
            g.insert(
                dist_subj.clone(),
                format!("{}accessService", ns::DCAT),
                Term::Iri(svc_iri),
            );
            g.insert(
                svc_subj.clone(),
                ns::RDF_TYPE,
                Term::Iri(format!("{}DataService", ns::DCAT)),
            );
            for (lang, stitle) in &svc.title {
                if !stitle.is_empty() {
                    g.insert(
                        svc_subj.clone(),
                        format!("{}title", ns::DCT),
                        Term::Literal(LiteralValue::lang(stitle.clone(), lang.clone())),
                    );
                }
            }
            for ep_url in &svc.endpoint_urls {
                g.insert(
                    svc_subj.clone(),
                    format!("{}endpointURL", ns::DCAT),
                    Term::Iri(ep_url.clone()),
                );
            }
            for ep_desc in &svc.endpoint_descriptions {
                g.insert(
                    svc_subj.clone(),
                    format!("{}endpointDescription", ns::DCAT),
                    Term::Iri(ep_desc.clone()),
                );
            }
            for std_iri in &svc.conforms_to {
                g.insert(
                    svc_subj.clone(),
                    format!("{}conformsTo", ns::DCT),
                    Term::Iri(std_iri.clone()),
                );
                g.insert(
                    SubjectNode::Named(std_iri.clone()),
                    ns::RDF_TYPE,
                    Term::Iri(format!("{}Standard", ns::DCT)),
                );
            }
            for law in svc.applicable_legislation.iter().chain(inherited_laws) {
                g.insert(
                    svc_subj.clone(),
                    format!("{}applicableLegislation", ns::DCATAP),
                    Term::Iri(law.clone()),
                );
                g.insert(
                    SubjectNode::Named(law.clone()),
                    ns::RDF_TYPE,
                    Term::Iri(format!("{}LegalResource", ns::ELI)),
                );
            }
        }
    }
    insert_properties(g, &ds_subj, Subject::Dataset, dataset, profile);

    ds_subj
}

/// The profile's terms-of-use node, of the members the record states.
fn insert_terms(g: &mut RdfGraph, subject: &SubjectNode, node: &Terms, stated: &TermsOfUse) {
    let terms = g.fresh_blank_node();
    g.insert(subject.clone(), node.link.clone(), terms.to_term());
    g.insert(terms.clone(), ns::RDF_TYPE, Term::Iri(node.class.clone()));
    let iri = |g: &mut RdfGraph, predicate: &String, value: &Option<String>| {
        if let Some(value) = value {
            g.insert(terms.clone(), predicate.clone(), Term::Iri(value.clone()));
        }
    };
    let names = |g: &mut RdfGraph, predicate: &String, value: &Texts| {
        for (lang, name) in value {
            g.insert(
                terms.clone(),
                predicate.clone(),
                Term::Literal(LiteralValue::lang(name.clone(), lang.clone())),
            );
        }
    };
    iri(g, &node.authors_work, &stated.authors_work);
    names(g, &node.author_name, &stated.author_name);
    iri(g, &node.database_as_work, &stated.database_as_work);
    names(g, &node.database_author_name, &stated.database_author_name);
    iri(g, &node.sui_generis_database, &stated.sui_generis_database);
    iri(g, &node.personal_data, &stated.personal_data);
}

/// The profile's additional properties the record declares as extras: one value, or a JSON
/// array of them.
fn insert_properties(
    g: &mut RdfGraph,
    subject: &SubjectNode,
    level: Subject,
    dataset: &Dataset,
    profile: DcatProfile,
) {
    for property in profile.properties.iter().filter(|p| p.subject == level) {
        let Some(stated) = dataset.extras.get(&property.extra) else {
            continue;
        };
        let values =
            serde_json::from_str::<Vec<String>>(stated).unwrap_or_else(|_| vec![stated.to_owned()]);
        for value in values.iter().map(|v| v.trim()).filter(|v| !v.is_empty()) {
            let object = match property.kind {
                ValueKind::Literal => Term::Literal(LiteralValue::plain(value)),
                ValueKind::Iri => {
                    let labels = property.values.get(value);
                    if !is_declared_iri(value) || (labels.is_none() && !property.values.is_empty())
                    {
                        continue;
                    }
                    for (lang, label) in labels.into_iter().flatten() {
                        g.insert(
                            SubjectNode::Named(value.to_owned()),
                            format!("{}prefLabel", ns::SKOS),
                            Term::Literal(LiteralValue::lang(label.clone(), lang.clone())),
                        );
                    }
                    if let Some(class) = &property.class {
                        g.insert(
                            SubjectNode::Named(value.to_owned()),
                            ns::RDF_TYPE,
                            Term::Iri(class.clone()),
                        );
                    }
                    Term::Iri(value.to_owned())
                }
            };
            g.insert(subject.clone(), property.predicate.clone(), object);
        }
    }
}

/// Converts a [`Catalogue`], its [`Dataset`]s, [`Publisher`]s and optional [`HydraPageInfo`]
/// into an [`RdfGraph`].
pub fn catalogue_to_graph(
    catalogue: &Catalogue,
    datasets: &[Dataset],
    publishers: &BTreeMap<String, Publisher>,
    profile: DcatProfile,
    hydra: Option<&HydraPageInfo>,
) -> RdfGraph {
    let mut g = RdfGraph::new();
    let base = catalogue.base_url();
    let cat_iri = format!("{base}/catalog");
    let cat_subj = SubjectNode::Named(cat_iri.clone());

    g.insert(
        cat_subj.clone(),
        ns::RDF_TYPE,
        Term::Iri(format!("{}Catalog", ns::DCAT)),
    );
    for (lang, t) in &catalogue.title {
        if !t.is_empty() {
            g.insert(
                cat_subj.clone(),
                format!("{}title", ns::DCT),
                Term::Literal(LiteralValue::lang(t.clone(), lang.clone())),
            );
        }
    }
    for (lang, d) in &catalogue.description {
        if !d.is_empty() {
            g.insert(
                cat_subj.clone(),
                format!("{}description", ns::DCT),
                Term::Literal(LiteralValue::lang(d.clone(), lang.clone())),
            );
        }
    }

    if let Some((iri, names)) = catalogue_publisher(catalogue) {
        insert_agent(&mut g, &cat_subj, iri, &names);
    }

    let homepage = catalogue.homepage.clone().unwrap_or_else(|| base.clone());
    g.insert(
        cat_subj.clone(),
        format!("{}homepage", ns::FOAF),
        Term::Iri(homepage.clone()),
    );
    g.insert(
        SubjectNode::Named(homepage),
        ns::RDF_TYPE,
        Term::Iri(format!("{}Document", ns::FOAF)),
    );

    if let Some(cp) = &catalogue.contact_point {
        let cp_node = g.fresh_blank_node();
        g.insert(
            cat_subj.clone(),
            format!("{}contactPoint", ns::DCAT),
            cp_node.to_term(),
        );
        g.insert(
            cp_node.clone(),
            ns::RDF_TYPE,
            Term::Iri(format!("{}Organization", ns::VCARD)),
        );
        g.insert(
            cp_node.clone(),
            ns::RDF_TYPE,
            Term::Iri(format!("{}Kind", ns::VCARD)),
        );
        for (lang, name) in &cp.name {
            if !name.is_empty() {
                g.insert(
                    cp_node.clone(),
                    format!("{}fn", ns::VCARD),
                    Term::Literal(LiteralValue::lang(name.clone(), lang.clone())),
                );
            }
        }
        let email = if cp.email.starts_with("mailto:") {
            cp.email.clone()
        } else {
            format!("mailto:{}", cp.email)
        };
        g.insert(cp_node, format!("{}hasEmail", ns::VCARD), Term::Iri(email));
    }

    for ds in datasets {
        let pub_rec = publishers.get(&ds.publisher_id);
        let ds_subj = add_dataset_triples(&mut g, ds, catalogue, pub_rec, profile);
        g.insert(
            cat_subj.clone(),
            format!("{}dataset", ns::DCAT),
            ds_subj.to_term(),
        );
    }

    if let Some(page) = hydra {
        let view_subj = SubjectNode::Named(page.current_page_uri.clone());
        g.insert(
            view_subj.clone(),
            ns::RDF_TYPE,
            Term::Iri(format!("{}PartialCollectionView", ns::HYDRA)),
        );
        g.insert(
            view_subj.clone(),
            ns::RDF_TYPE,
            Term::Iri(format!("{}PagedCollection", ns::HYDRA)),
        );
        g.insert(
            view_subj.clone(),
            format!("{}totalItems", ns::HYDRA),
            Term::Literal(LiteralValue::typed(
                page.total_items.to_string(),
                ns::XSD_INTEGER,
            )),
        );
        g.insert(
            view_subj.clone(),
            format!("{}itemsPerPage", ns::HYDRA),
            Term::Literal(LiteralValue::typed(
                page.items_per_page.to_string(),
                ns::XSD_INTEGER,
            )),
        );
        g.insert(
            view_subj.clone(),
            format!("{}first", ns::HYDRA),
            Term::Iri(page.first_page_uri.clone()),
        );
        g.insert(
            view_subj.clone(),
            format!("{}firstPage", ns::HYDRA),
            Term::Iri(page.first_page_uri.clone()),
        );
        g.insert(
            view_subj.clone(),
            format!("{}last", ns::HYDRA),
            Term::Iri(page.last_page_uri.clone()),
        );
        g.insert(
            view_subj.clone(),
            format!("{}lastPage", ns::HYDRA),
            Term::Iri(page.last_page_uri.clone()),
        );
        if let Some(next) = &page.next_page_uri {
            g.insert(
                view_subj.clone(),
                format!("{}next", ns::HYDRA),
                Term::Iri(next.clone()),
            );
            g.insert(
                view_subj.clone(),
                format!("{}nextPage", ns::HYDRA),
                Term::Iri(next.clone()),
            );
        }
        if let Some(prev) = &page.prev_page_uri {
            g.insert(
                view_subj.clone(),
                format!("{}previous", ns::HYDRA),
                Term::Iri(prev.clone()),
            );
            g.insert(
                view_subj,
                format!("{}previousPage", ns::HYDRA),
                Term::Iri(prev.clone()),
            );
        }
    }

    type_values(&mut g, profile);
    g
}

/// The licence a distribution is published under: its own, else the dataset's. CKAN's
/// `notspecified` declares none.
pub(crate) fn distribution_licence<'a>(
    dataset: &'a Dataset,
    dist: &'a Distribution,
) -> Option<&'a str> {
    [&dist.licence, &dataset.licence_id, &dataset.licence_url]
        .into_iter()
        .filter_map(|l| l.as_deref())
        .find(|l| !l.trim().is_empty() && !l.eq_ignore_ascii_case("notspecified"))
}

/// Whether the record declares that the data holds personal data: the dataset's own statement,
/// or the national code a harvested distribution carries. Another value is the record's own IRI.
fn personal_data(dataset: &Dataset, dist: &Distribution, profile: DcatProfile) -> Option<String> {
    let own = dist
        .terms_of_use
        .as_ref()
        .and_then(|t| t.personal_data.as_deref());
    let stated = own.or_else(|| dataset.extras.get(PERSONAL_DATA).map(String::as_str))?;
    let holds = std::iter::once((PERSONAL_DATA_CONTAINS, PERSONAL_DATA_NONE))
        // Every installed code list (API/08 section 5): a harvested record carries its country's.
        .chain(DcatProfile::installed().filter_map(|installed| {
            let codes = installed.personal_data_codes()?;
            Some((codes.contains.as_str(), codes.does_not_contain.as_str()))
        }))
        .find_map(|(yes, no)| {
            (stated == yes)
                .then_some(true)
                .or_else(|| (stated == no).then_some(false))
        });
    let codes = profile.render.personal_data.as_ref()?;
    match holds {
        Some(true) => Some(codes.contains.clone()),
        Some(false) => Some(codes.does_not_contain.clone()),
        None => stated.contains("://").then(|| stated.to_owned()),
    }
}

/// The terms of use a distribution declares: its own, the three rights it leaves out
/// from the declared licence (API/08 section 6), and personal data only as declared (CMP-08).
/// `None` when nothing is known. Both the graph and the JSON-LD serializers state these.
pub(crate) fn terms_of_use(
    dataset: &Dataset,
    dist: &Distribution,
    profile: DcatProfile,
) -> Option<TermsOfUse> {
    let mut tou = dist.terms_of_use.clone().unwrap_or_default();
    let licence = distribution_licence(dataset, dist).and_then(|l| match find_licence(l) {
        Some(known) => (!known.url.is_empty()).then_some(known.url),
        None => l.contains("://").then(|| l.to_owned()),
    });
    for right in [
        &mut tou.authors_work,
        &mut tou.database_as_work,
        &mut tou.sui_generis_database,
    ] {
        if right.is_none() {
            right.clone_from(&licence);
        }
    }
    tou.personal_data = personal_data(dataset, dist, profile);
    (tou != TermsOfUse::default()).then_some(tou)
}

/// Every installed profile's terms vocabulary, each once.
fn known_terms() -> Vec<&'static Terms> {
    let mut known: Vec<&'static Terms> = Vec::new();
    for terms in DcatProfile::installed().filter_map(DcatProfile::terms) {
        if !known.iter().any(|held| held.link == terms.link) {
            known.push(terms);
        }
    }
    known
}

/// The SPDX IRI of the checksum algorithm the record names.
fn checksum_algorithm(dist: &Distribution) -> Option<String> {
    let named = dist.hash_algorithm.as_deref()?.trim();
    if named.contains("://") {
        return Some(named.to_owned());
    }
    let name = named.to_ascii_lowercase().replace('-', "");
    ["md5", "sha1", "sha224", "sha256", "sha384", "sha512"]
        .contains(&name.as_str())
        .then(|| format!("http://spdx.org/rdf/terms#checksumAlgorithm_{name}"))
}

/// A dataset's publisher as declared, `(IRI, names)`: the Publisher record's own IRI, else the
/// IRI the dataset names, else the record's page in this catalogue, else the dataset's name of
/// it alone. A bare `publisher_id` is no declaration.
pub(crate) fn dataset_publisher(
    dataset: &Dataset,
    catalogue: &Catalogue,
    publisher: Option<&Publisher>,
) -> Option<(Option<String>, Texts)> {
    let named = dataset.publisher.as_ref();
    let names = named.map(|a| a.name.clone()).unwrap_or_default();
    if let Some(record) = publisher.filter(|p| p.has_own_iri(catalogue)) {
        return Some((Some(record.uri.clone()), record.title.clone()));
    }
    if let Some(uri) = named
        .and_then(|a| a.uri.as_deref())
        .filter(|uri| is_declared_iri(uri))
    {
        let names = match publisher {
            Some(record) if names.is_empty() => record.title.clone(),
            _ => names,
        };
        return Some((Some(uri.to_owned()), names));
    }
    match publisher {
        Some(record) if is_declared_iri(&record.uri) => {
            Some((Some(record.uri.clone()), record.title.clone()))
        }
        _ => (!names.is_empty()).then_some((None, names)),
    }
}

/// The catalogue's publisher as declared: its IRI, or its name alone.
fn catalogue_publisher(catalogue: &Catalogue) -> Option<(Option<String>, Texts)> {
    let iri = catalogue.declared_publisher_uri().map(str::to_owned);
    let names = catalogue.publisher_name.clone().unwrap_or_default();
    (iri.is_some() || !names.is_empty()).then_some((iri, names))
}

/// `dct:publisher`: the agent's IRI, or a blank node carrying the declared names.
fn insert_agent(g: &mut RdfGraph, subject: &SubjectNode, iri: Option<String>, names: &Texts) {
    let agent = match iri {
        Some(iri) => SubjectNode::Named(iri),
        None => g.fresh_blank_node(),
    };
    g.insert(
        subject.clone(),
        format!("{}publisher", ns::DCT),
        agent.to_term(),
    );
    g.insert(
        agent.clone(),
        ns::RDF_TYPE,
        Term::Iri(format!("{}Agent", ns::FOAF)),
    );
    for (lang, name) in names {
        if !name.is_empty() {
            g.insert(
                agent.clone(),
                format!("{}name", ns::FOAF),
                Term::Literal(LiteralValue::lang(name.clone(), lang.clone())),
            );
        }
    }
}

/// Serializes one [`Dataset`] in the requested [`RdfFormat`] and [`DcatProfile`] (C7, C17).
pub fn serialize_dataset(
    dataset: &Dataset,
    catalogue: &Catalogue,
    publisher: Option<&Publisher>,
    format: RdfFormat,
    profile: DcatProfile,
) -> String {
    if let (RdfFormat::JsonLd, Some(keys)) = (format, &profile.render.json_ld) {
        return serialize_compact_dataset(dataset, catalogue, publisher, profile, keys);
    }
    let g = dataset_to_graph(dataset, catalogue, publisher, profile);
    match format {
        RdfFormat::Turtle => serialize_turtle(&g),
        RdfFormat::JsonLd => serialize_graph_jsonld(&g),
        RdfFormat::RdfXml => serialize_rdfxml(&g),
        RdfFormat::NTriples => serialize_ntriples(&g),
    }
}

/// Serializes a [`Catalogue`] in the requested [`RdfFormat`] and [`DcatProfile`] (C7, C17).
pub fn serialize_catalogue(
    catalogue: &Catalogue,
    datasets: &[Dataset],
    publishers: &BTreeMap<String, Publisher>,
    format: RdfFormat,
    profile: DcatProfile,
    hydra: Option<&HydraPageInfo>,
) -> String {
    if let (RdfFormat::JsonLd, Some(keys), None) = (format, &profile.render.json_ld, hydra) {
        return serialize_compact_catalogue(catalogue, datasets, keys);
    }
    let g = catalogue_to_graph(catalogue, datasets, publishers, profile, hydra);
    match format {
        RdfFormat::Turtle => serialize_turtle(&g),
        RdfFormat::JsonLd => serialize_graph_jsonld(&g),
        RdfFormat::RdfXml => serialize_rdfxml(&g),
        RdfFormat::NTriples => serialize_ntriples(&g),
    }
}

/// Unified entry point `parse(bytes, format, RdfLimits)` backed by `oxttl`, `oxrdfxml`, and
/// `oxjsonld` with streaming byte, triple, nesting-depth, and literal-length caps (A1).
pub fn parse(
    bytes: &[u8],
    format: RdfFormat,
    limits: RdfLimits,
) -> dataroom_core::Result<RdfGraph> {
    match format {
        RdfFormat::Turtle => parse_turtle_bytes(bytes, &limits),
        RdfFormat::NTriples => parse_ntriples_bytes(bytes, &limits),
        RdfFormat::JsonLd => parse_jsonld_bytes(bytes, &limits),
        RdfFormat::RdfXml => parse_rdfxml_bytes(bytes, &limits),
    }
}

/// Parses an RDF document in any supported format into an [`RdfGraph`].
pub fn parse_rdf_graph(
    input: &str,
    format: RdfFormat,
    limits: &RdfLimits,
) -> dataroom_core::Result<RdfGraph> {
    parse(input.as_bytes(), format, *limits)
}

/// Extracts all [`Dataset`]s (and [`DatasetSeries`]) from an [`RdfGraph`] (DCAT-AP 3.0.1, CZ, SK).
pub fn graph_to_datasets(
    graph: &RdfGraph,
    org_id: OrgId,
    catalogue_id: CatalogueId,
    default_publisher_id: &str,
    default_lang: &str,
) -> Vec<Dataset> {
    let mut subjects = graph.instances_of("http://www.w3.org/ns/dcat#Dataset");
    let series_subjects = graph.instances_of("http://www.w3.org/ns/dcat#DatasetSeries");
    for s in &series_subjects {
        if !subjects.contains(s) {
            subjects.push(s.clone());
        }
    }

    let mut out = Vec::new();
    for ds_subj in subjects {
        let is_series = series_subjects.contains(&ds_subj);
        let uri = ds_subj.as_iri().map(str::to_owned);
        let slug = uri
            .as_deref()
            .and_then(|u| u.trim_end_matches('/').rsplit('/').next())
            .map(slugify)
            .filter(|s| s.len() >= 2)
            .unwrap_or_else(|| format!("dataset-{}", &Uuid::now_v7().to_string()[..8]));

        let title = extract_lang_map(
            graph,
            &ds_subj,
            "http://purl.org/dc/terms/title",
            default_lang,
        );
        let description = extract_lang_map(
            graph,
            &ds_subj,
            "http://purl.org/dc/terms/description",
            default_lang,
        );

        let mut keywords: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut tags: Vec<String> = Vec::new();
        for kw_term in graph.objects_of(&ds_subj, "http://www.w3.org/ns/dcat#keyword") {
            if let Term::Literal(lit) = kw_term {
                let lang = lit
                    .language
                    .clone()
                    .unwrap_or_else(|| default_lang.to_owned());
                keywords.entry(lang).or_default().push(lit.value.clone());
                if let Some(clean_tag) = clean_ckan_tag(&lit.value) {
                    if !tags.contains(&clean_tag) {
                        tags.push(clean_tag);
                    }
                }
            }
        }

        let mut themes = Vec::new();
        let mut eurovoc_themes = Vec::new();
        for th_term in graph.objects_of(&ds_subj, "http://www.w3.org/ns/dcat#theme") {
            let val = th_term.string_value().to_owned();
            if val.contains("eurovoc.europa.eu") {
                eurovoc_themes.push(val);
            } else {
                themes.push(val);
            }
        }

        let frequency = graph
            .first_object(&ds_subj, "http://purl.org/dc/terms/accrualPeriodicity")
            .map(|t| t.string_value().to_owned());

        let spatial: Vec<String> = graph
            .objects_of(&ds_subj, "http://purl.org/dc/terms/spatial")
            .into_iter()
            .map(|t| t.string_value().to_owned())
            .collect();

        let temporal = graph
            .first_object(&ds_subj, "http://purl.org/dc/terms/temporal")
            .and_then(|t| t.as_subject())
            .map(|p_subj| {
                let start = graph
                    .first_object(&p_subj, "http://www.w3.org/ns/dcat#startDate")
                    .and_then(|v| v.string_value().get(..10).and_then(parse_date).map(Date));
                let end = graph
                    .first_object(&p_subj, "http://www.w3.org/ns/dcat#endDate")
                    .and_then(|v| v.string_value().get(..10).and_then(parse_date).map(Date));
                TemporalPeriod { start, end }
            })
            .filter(|tp| tp.start.is_some() || tp.end.is_some());

        let spatial_resolution_in_meters = graph
            .first_object(
                &ds_subj,
                "http://www.w3.org/ns/dcat#spatialResolutionInMeters",
            )
            .and_then(|t| t.string_value().parse::<f64>().ok());

        let temporal_resolution = graph
            .first_object(&ds_subj, "http://www.w3.org/ns/dcat#temporalResolution")
            .map(|t| t.string_value().to_owned());

        let publisher = graph
            .first_object(&ds_subj, "http://purl.org/dc/terms/publisher")
            .map(|p_term| {
                let p_uri = p_term.as_iri().map(str::to_owned);
                let mut names = BTreeMap::new();
                if let Some(p_subj) = p_term.as_subject() {
                    names = extract_lang_map(
                        graph,
                        &p_subj,
                        "http://xmlns.com/foaf/0.1/name",
                        default_lang,
                    );
                }
                AgentDetails {
                    uri: p_uri,
                    name: names,
                    ..AgentDetails::default()
                }
            });

        let contact_point = graph
            .first_object(&ds_subj, "http://www.w3.org/ns/dcat#contactPoint")
            .and_then(|cp_term| cp_term.as_subject())
            .map(|cp_subj| {
                let names = extract_lang_map(
                    graph,
                    &cp_subj,
                    "http://www.w3.org/2006/vcard/ns#fn",
                    default_lang,
                );
                let email = graph
                    .first_object(&cp_subj, "http://www.w3.org/2006/vcard/ns#hasEmail")
                    .map(|t| t.string_value().trim_start_matches("mailto:").to_owned())
                    .unwrap_or_default();
                let is_ind = graph
                    .objects_of(&cp_subj, ns::RDF_TYPE)
                    .iter()
                    .any(|t| t.string_value().ends_with("Individual"));
                ContactPoint {
                    uri: cp_subj.as_iri().map(str::to_owned),
                    kind: if is_ind {
                        ContactKind::Individual
                    } else {
                        ContactKind::Organization
                    },
                    name: names,
                    email,
                    url: None,
                    identifier: None,
                }
            });

        let landing_page = graph
            .first_object(&ds_subj, "http://www.w3.org/ns/dcat#landingPage")
            .map(|t| t.string_value().to_owned());
        let documentation: Vec<String> = graph
            .objects_of(&ds_subj, "http://xmlns.com/foaf/0.1/page")
            .into_iter()
            .map(|t| t.string_value().to_owned())
            .collect();
        let conforms_to: Vec<String> = graph
            .objects_of(&ds_subj, "http://purl.org/dc/terms/conformsTo")
            .into_iter()
            .map(|t| t.string_value().to_owned())
            .collect();
        let applicable_legislation: Vec<String> = graph
            .objects_of(&ds_subj, "http://data.europa.eu/r5r/applicableLegislation")
            .into_iter()
            .map(|t| t.string_value().to_owned())
            .collect();
        let in_series = graph
            .first_object(&ds_subj, "http://www.w3.org/ns/dcat#inSeries")
            .or_else(|| graph.first_object(&ds_subj, "http://purl.org/dc/terms/isPartOf"))
            .map(|t| t.string_value().to_owned());
        let relations: Vec<String> = graph
            .objects_of(&ds_subj, "http://purl.org/dc/terms/relation")
            .into_iter()
            .map(|t| t.string_value().to_owned())
            .collect();
        let languages: Vec<String> = graph
            .objects_of(&ds_subj, "http://purl.org/dc/terms/language")
            .into_iter()
            .map(|t| eu_iri_to_language(t.string_value()))
            .collect();
        let dcat_type = graph
            .first_object(&ds_subj, "http://purl.org/dc/terms/type")
            .map(|t| t.string_value().to_owned());
        let identifier = graph
            .first_object(&ds_subj, "http://purl.org/dc/terms/identifier")
            .map(|t| t.string_value().to_owned());

        let mut distributions = Vec::new();
        let mut stated_licences: Vec<Option<String>> = Vec::new();

        for (idx, dist_term) in graph
            .objects_of(&ds_subj, "http://www.w3.org/ns/dcat#distribution")
            .into_iter()
            .enumerate()
        {
            let Some(dist_subj) = dist_term.as_subject() else {
                continue;
            };
            let dist_uri = dist_subj.as_iri().map(str::to_owned);
            let dist_title = extract_lang_map(
                graph,
                &dist_subj,
                "http://purl.org/dc/terms/title",
                default_lang,
            );
            let dist_desc = extract_lang_map(
                graph,
                &dist_subj,
                "http://purl.org/dc/terms/description",
                default_lang,
            );
            let access_url = graph
                .first_object(&dist_subj, "http://www.w3.org/ns/dcat#accessURL")
                .or_else(|| graph.first_object(&dist_subj, "http://www.w3.org/ns/dcat#downloadURL"))
                .map(|t| t.string_value().to_owned())
                .unwrap_or_default();
            let download_url = graph
                .first_object(&dist_subj, "http://www.w3.org/ns/dcat#downloadURL")
                .map(|t| t.string_value().to_owned());
            let format = graph
                .first_object(&dist_subj, "http://purl.org/dc/terms/format")
                .map(|t| {
                    t.string_value()
                        .trim_end_matches('/')
                        .rsplit('/')
                        .next()
                        .unwrap_or(t.string_value())
                        .to_owned()
                });
            let media_type = graph
                .first_object(&dist_subj, "http://www.w3.org/ns/dcat#mediaType")
                .map(|t| iana_iri_to_media_type(t.string_value()));
            let compress_format = graph
                .first_object(&dist_subj, "http://www.w3.org/ns/dcat#compressFormat")
                .map(|t| t.string_value().to_owned());
            let package_format = graph
                .first_object(&dist_subj, "http://www.w3.org/ns/dcat#packageFormat")
                .map(|t| t.string_value().to_owned());
            let byte_size = graph
                .first_object(&dist_subj, "http://www.w3.org/ns/dcat#byteSize")
                .and_then(|t| t.string_value().parse::<u64>().ok());

            let mut hash = None;
            let mut hash_algorithm = None;
            if let Some(cs_subj) = graph
                .first_object(&dist_subj, "http://spdx.org/rdf/terms#checksum")
                .and_then(|t| t.as_subject())
            {
                hash = graph
                    .first_object(&cs_subj, "http://spdx.org/rdf/terms#checksumValue")
                    .map(|t| t.string_value().to_owned());
                hash_algorithm = graph
                    .first_object(&cs_subj, "http://spdx.org/rdf/terms#algorithm")
                    .map(|t| t.string_value().to_owned());
            }

            let dist_licence = graph
                .first_object(&dist_subj, "http://purl.org/dc/terms/license")
                .map(|t| t.string_value().to_owned());
            stated_licences.push(dist_licence.clone());

            // Terms of use, under any installed profile's predicates
            let first = |subject: &SubjectNode, member: fn(&Terms) -> &String| {
                known_terms()
                    .into_iter()
                    .find_map(|node| graph.first_object(subject, member(node)))
            };
            let names = |subject: &SubjectNode, member: fn(&Terms) -> &String| {
                let mut all = Texts::new();
                for node in known_terms() {
                    all.extend(extract_lang_map(graph, subject, member(node), default_lang));
                }
                all
            };
            let terms_of_use = first(&dist_subj, |t| &t.link)
                .and_then(|t| t.as_subject())
                .map(|tou_subj| {
                    let value = |member: fn(&Terms) -> &String| {
                        first(&tou_subj, member).map(|t| t.string_value().to_owned())
                    };
                    let authors_work = value(|t| &t.authors_work);
                    if let (Some(aw), Some(stated)) = (&authors_work, stated_licences.last_mut()) {
                        if stated.is_none() && find_licence(aw).is_some() {
                            *stated = Some(aw.clone());
                        }
                    }
                    TermsOfUse {
                        authors_work,
                        author_name: names(&tou_subj, |t| &t.author_name),
                        database_as_work: value(|t| &t.database_as_work),
                        database_author_name: names(&tou_subj, |t| &t.database_author_name),
                        sui_generis_database: value(|t| &t.sui_generis_database),
                        personal_data: value(|t| &t.personal_data),
                    }
                });

            let dist_conforms: Vec<String> = graph
                .objects_of(&dist_subj, "http://purl.org/dc/terms/conformsTo")
                .into_iter()
                .map(|t| t.string_value().to_owned())
                .collect();
            let dist_laws: Vec<String> = graph
                .objects_of(
                    &dist_subj,
                    "http://data.europa.eu/r5r/applicableLegislation",
                )
                .into_iter()
                .map(|t| t.string_value().to_owned())
                .collect();

            let mut access_services = Vec::new();
            for (s_idx, svc_term) in graph
                .objects_of(&dist_subj, "http://www.w3.org/ns/dcat#accessService")
                .into_iter()
                .enumerate()
            {
                let Some(svc_subj) = svc_term.as_subject() else {
                    continue;
                };
                let svc_title = extract_lang_map(
                    graph,
                    &svc_subj,
                    "http://purl.org/dc/terms/title",
                    default_lang,
                );
                let ep_urls: Vec<String> = graph
                    .objects_of(&svc_subj, "http://www.w3.org/ns/dcat#endpointURL")
                    .into_iter()
                    .map(|t| t.string_value().to_owned())
                    .collect();
                let ep_descs: Vec<String> = graph
                    .objects_of(&svc_subj, "http://www.w3.org/ns/dcat#endpointDescription")
                    .into_iter()
                    .map(|t| t.string_value().to_owned())
                    .collect();
                let svc_conforms: Vec<String> = graph
                    .objects_of(&svc_subj, "http://purl.org/dc/terms/conformsTo")
                    .into_iter()
                    .map(|t| t.string_value().to_owned())
                    .collect();
                let svc_laws: Vec<String> = graph
                    .objects_of(&svc_subj, "http://data.europa.eu/r5r/applicableLegislation")
                    .into_iter()
                    .map(|t| t.string_value().to_owned())
                    .collect();
                access_services.push(DataService {
                    id: format!("svc-{slug}-{idx}-{s_idx}"),
                    uri: svc_subj.as_iri().map(str::to_owned),
                    title: svc_title,
                    endpoint_urls: ep_urls,
                    endpoint_descriptions: ep_descs,
                    conforms_to: svc_conforms,
                    applicable_legislation: svc_laws,
                    ..DataService::default()
                });
            }

            distributions.push(Distribution {
                id: format!("res-{slug}-{idx}"),
                dataset_id: format!("pkg-{slug}"),
                uri: dist_uri,
                title: dist_title,
                description: dist_desc,
                access_url,
                download_url,
                format,
                media_type,
                compress_format,
                package_format,
                byte_size,
                hash,
                hash_algorithm,
                licence: stated_licences.last().cloned().flatten(),
                rights: BTreeMap::new(),
                status: None,
                availability: None,
                resource_type: None,
                conforms_to: dist_conforms,
                documentation: Vec::new(),
                languages: Vec::new(),
                applicable_legislation: dist_laws,
                temporal_resolution: None,
                spatial_resolution_in_meters: None,
                terms_of_use,
                access_services,
                issued: None,
                modified: None,
                extras: BTreeMap::new(),
            });
        }

        // The dataset is under a licence only when all its distributions state that one; a
        // sibling's licence is no statement about a distribution without one.
        let (mut derived_licence_id, mut derived_licence_url) = (None, None);
        if let Some(Some(first)) = stated_licences.first() {
            if stated_licences.iter().all(|l| l.as_ref() == Some(first)) {
                match find_licence(first) {
                    Some(entry) => {
                        derived_licence_id = Some(entry.id);
                        derived_licence_url = Some(entry.url);
                    }
                    None => derived_licence_url = Some(first.clone()),
                }
            }
        }

        let now = TimestampTz::now();
        let issued = graph
            .first_object(&ds_subj, "http://purl.org/dc/terms/issued")
            .and_then(|t| parse_any_datetime(t.string_value()))
            .unwrap_or(now);
        let modified = graph
            .first_object(&ds_subj, "http://purl.org/dc/terms/modified")
            .and_then(|t| parse_any_datetime(t.string_value()))
            .unwrap_or(now);

        out.push(Dataset {
            org_id,
            catalogue_id,
            id: format!("pkg-{slug}"),
            name: slug,
            uri,
            dataset_type: if is_series {
                "dataset_series".into()
            } else {
                "dataset".into()
            },
            title,
            description,
            publisher_id: default_publisher_id.to_owned(),
            publisher,
            creator: None,
            contact_point,
            visibility: Visibility::Public,
            state: RecordState::Active,
            licence_id: derived_licence_id,
            licence_url: derived_licence_url,
            themes,
            eurovoc_themes,
            keywords,
            tags,
            spatial,
            spatial_resolution_in_meters,
            temporal,
            temporal_resolution,
            frequency,
            landing_page,
            documentation,
            conforms_to,
            applicable_legislation,
            in_series,
            relations,
            sources: Vec::new(),
            languages,
            version: None,
            version_notes: BTreeMap::new(),
            access_rights: None,
            provenance: BTreeMap::new(),
            dcat_type,
            identifier,
            alternate_identifiers: Vec::new(),
            endpoint_url: None,
            generated_by: None,
            harvest_source_id: None,
            harvest_source_item_id: None,
            harvest_hash: None,
            custom_shacl_shapes: None,
            distributions,
            extras: BTreeMap::new(),
            restricted_grants: Vec::new(),
            issued,
            modified,
        });
    }

    out
}

/// Parses an RDF document directly into [`Dataset`] records.
pub fn parse_datasets_from_rdf(
    input: &str,
    format: RdfFormat,
    org_id: OrgId,
    catalogue_id: CatalogueId,
    default_publisher_id: &str,
    default_lang: &str,
    limits: &RdfLimits,
) -> dataroom_core::Result<Vec<Dataset>> {
    let g = parse_rdf_graph(input, format, limits)?;
    Ok(graph_to_datasets(
        &g,
        org_id,
        catalogue_id,
        default_publisher_id,
        default_lang,
    ))
}

fn extract_lang_map(
    graph: &RdfGraph,
    subj: &SubjectNode,
    predicate: &str,
    default_lang: &str,
) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for term in graph.objects_of(subj, predicate) {
        if let Term::Literal(lit) = term {
            let lang = lit.language.unwrap_or_else(|| default_lang.to_owned());
            map.insert(lang, lit.value);
        }
    }
    map
}

fn parse_any_datetime(s: &str) -> Option<TimestampTz> {
    if let Some(ts) = parse_rfc3339(s) {
        return Some(TimestampTz(ts));
    }
    if let Ok(ts) = jiff::fmt::strtime::parse("%Y-%m-%dT%H:%M:%S", s)
        .and_then(|t| t.to_datetime())
        .and_then(|dt| jiff::tz::Offset::UTC.to_timestamp(dt))
    {
        return Some(TimestampTz(ts));
    }
    s.get(..10)
        .and_then(parse_date)
        .and_then(TimestampTz::from_date)
}

fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').chars().take(90).collect()
}

fn clean_ckan_tag(word: &str) -> Option<String> {
    let allowed = |c: char| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.');
    let spaced: String = word
        .chars()
        .map(|c| if allowed(c) { c } else { ' ' })
        .collect();
    let tag: String = spaced
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(100)
        .collect();
    let tag = tag.trim().to_owned();
    (2..=100).contains(&tag.chars().count()).then_some(tag)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dataroom_core::DataroomError;
    use dataroom_core::ThemeTokens;

    fn sample_catalogue(org_id: OrgId, catalogue_id: CatalogueId) -> Catalogue {
        Catalogue {
            org_id,
            branding: None,
            hidden_datasets: Default::default(),
            id: catalogue_id,
            name: "opendata".into(),
            host: "opendata.example.org".into(),
            title: BTreeMap::from([
                ("cs".into(), "Katalog otevřených dat".into()),
                ("sk".into(), "Katalóg otvorených dát".into()),
                ("en".into(), "Open Data Catalogue".into()),
            ]),
            description: BTreeMap::from([
                ("cs".into(), "Popis katalogu".into()),
                ("sk".into(), "Popis katalógu".into()),
                ("en".into(), "Catalogue description".into()),
            ]),
            homepage: Some("https://opendata.example.org".into()),
            default_language: "cs".into(),
            languages: vec!["cs".into(), "sk".into(), "en".into()],
            main_profile: "dcat-ap-3.0.1".into(),
            active_profiles: Vec::new(),
            default_licence: Some("cc-by".into()),
            publisher_uri: Some(
                "https://rpp-opendata.egon.gov.cz/odrpp/zdroj/orgán-veřejné-moci/66003008".into(),
            ),
            publisher_name: Some(BTreeMap::from([
                ("cs".into(), "Ministerstvo dopravy".into()),
                ("en".into(), "Ministry of Transport".into()),
            ])),
            contact_point: None,
            theme: ThemeTokens::default(),
            allow_cross_org_harvest: true,
            allowed_outbound_hosts: Vec::new(),
            applicable_legislation: Vec::new(),
            modified: TimestampTz::now(),
            deleted: None,
            public_base: None,
            portal_base: None,
            left_hosts: Vec::new(),
        }
    }

    #[test]
    fn round_trip_dcat_ap_turtle_to_dataset_to_turtle_per_profile() {
        let org_id = OrgId::from_uuid(Uuid::from_u128(1));
        let catalogue_id = CatalogueId::from_uuid(Uuid::from_u128(10));
        let cat = sample_catalogue(org_id, catalogue_id);

        // 1. CZ example Turtle -> Dataset -> Turtle -> Dataset
        let cz_ttl = include_str!("../../specs/dcat-ap-cz/priklady-katalog/datova-sada.ttl");
        let parsed_cz = parse_datasets_from_rdf(
            cz_ttl,
            RdfFormat::Turtle,
            org_id,
            catalogue_id,
            "mdcr",
            "cs",
            &RdfLimits::default(),
        )
        .expect("parse cz ttl");
        assert_eq!(parsed_cz.len(), 1);
        let ds_cz = &parsed_cz[0];
        assert_eq!(
            ds_cz.title.get("cs").map(String::as_str),
            Some("Jízdní řády veřejné linkové dopravy")
        );
        assert_eq!(
            ds_cz.title.get("en").map(String::as_str),
            Some("Public transport timetables")
        );
        assert_eq!(ds_cz.distributions.len(), 2);

        for profile in [
            DcatProfile::eu(),
            DcatProfile::from_token("dcat-ap-cz").expect("installed"),
            DcatProfile::from_token("dcat-ap-sk-2.0").expect("installed"),
        ] {
            let emitted_ttl = serialize_dataset(ds_cz, &cat, None, RdfFormat::Turtle, profile);
            let reparsed = parse_datasets_from_rdf(
                &emitted_ttl,
                RdfFormat::Turtle,
                org_id,
                catalogue_id,
                "mdcr",
                "cs",
                &RdfLimits::default(),
            )
            .expect("reparse emitted ttl");
            assert_eq!(reparsed.len(), 1);
            assert_eq!(reparsed[0].title, ds_cz.title);
            assert_eq!(reparsed[0].description, ds_cz.description);
            assert_eq!(reparsed[0].distributions.len(), ds_cz.distributions.len());
            assert_eq!(reparsed[0].temporal, ds_cz.temporal);
            assert_eq!(
                reparsed[0].spatial_resolution_in_meters,
                ds_cz.spatial_resolution_in_meters
            );
        }

        // 2. SK example Turtle -> Dataset -> Turtle -> Dataset
        let sk_ttl = include_str!("../../specs/dcat-ap-sk/príklady/dataset.ttl");
        let parsed_sk = parse_datasets_from_rdf(
            sk_ttl,
            RdfFormat::Turtle,
            org_id,
            catalogue_id,
            "mindop",
            "sk",
            &RdfLimits::default(),
        )
        .expect("parse sk ttl");
        assert_eq!(parsed_sk.len(), 1);
        let ds_sk = &parsed_sk[0];
        assert_eq!(
            ds_sk.title.get("sk").map(String::as_str),
            Some("Cestovné poriadky")
        );
        let sk_emitted = serialize_dataset(
            ds_sk,
            &cat,
            None,
            RdfFormat::Turtle,
            DcatProfile::from_token("dcat-ap-sk-2.0").expect("installed"),
        );
        let sk_reparsed = parse_datasets_from_rdf(
            &sk_emitted,
            RdfFormat::Turtle,
            org_id,
            catalogue_id,
            "mindop",
            "sk",
            &RdfLimits::default(),
        )
        .expect("reparse sk ttl");
        assert_eq!(sk_reparsed[0].title, ds_sk.title);
        assert_eq!(sk_reparsed[0].description, ds_sk.description);
    }

    #[test]
    fn a1_rdf_limits_refuse_bytes_triples_nesting_literal_length_billion_laughs_and_remote_context()
    {
        // 1. Byte cap refused with kind == "bytes"
        let small_byte_limit = RdfLimits {
            max_bytes: 32,
            ..RdfLimits::default()
        };
        let ttl = "<https://example.org/s> <https://example.org/p> \"hello world\" .";
        let err = parse(ttl.as_bytes(), RdfFormat::Turtle, small_byte_limit).unwrap_err();
        assert!(
            matches!(&err, DataroomError::RdfLimitExceeded { kind, .. } if kind == "bytes"),
            "expected bytes limit error, got {err:?}"
        );

        // 2. Triple cap refused with kind == "triples"
        let small_triple_limit = RdfLimits {
            max_triples: 2,
            ..RdfLimits::default()
        };
        let three_triples = r#"
            <https://example.org/s> <https://example.org/p1> "v1" .
            <https://example.org/s> <https://example.org/p2> "v2" .
            <https://example.org/s> <https://example.org/p3> "v3" .
        "#;
        let err = parse(
            three_triples.as_bytes(),
            RdfFormat::Turtle,
            small_triple_limit,
        )
        .unwrap_err();
        assert!(
            matches!(&err, DataroomError::RdfLimitExceeded { kind, .. } if kind == "triples"),
            "expected triples limit error, got {err:?}"
        );

        // 3. Nesting-depth cap refused with kind == "nesting-depth" (Turtle and JSON-LD and RDF/XML)
        let small_depth_limit = RdfLimits {
            max_depth: 3,
            ..RdfLimits::default()
        };
        let deep_ttl = "<https://example.org/s> <https://example.org/p> [ <https://example.org/p> [ <https://example.org/p> [ <https://example.org/p> [ <https://example.org/p> \"deep\" ] ] ] ] .";
        let err = parse(deep_ttl.as_bytes(), RdfFormat::Turtle, small_depth_limit).unwrap_err();
        assert!(
            matches!(&err, DataroomError::RdfLimitExceeded { kind, .. } if kind == "nesting-depth"),
            "expected nesting-depth limit error in Turtle, got {err:?}"
        );

        let deep_jsonld = r#"{"@id":"https://example.org/1","http://example.org/p":{"http://example.org/p":{"http://example.org/p":{"http://example.org/p":"deep"}}}}"#;
        let err = parse(deep_jsonld.as_bytes(), RdfFormat::JsonLd, small_depth_limit).unwrap_err();
        assert!(
            matches!(&err, DataroomError::RdfLimitExceeded { kind, .. } if kind == "nesting-depth"),
            "expected nesting-depth limit error in JSON-LD, got {err:?}"
        );

        // 4. Literal-length cap refused with kind == "literal-length"
        let small_lit_limit = RdfLimits {
            max_literal_length: 10,
            ..RdfLimits::default()
        };
        let long_lit_ttl = "<https://example.org/s> <https://example.org/p> \"0123456789abcdef\" .";
        let err = parse(long_lit_ttl.as_bytes(), RdfFormat::Turtle, small_lit_limit).unwrap_err();
        assert!(
            matches!(&err, DataroomError::RdfLimitExceeded { kind, .. } if kind == "literal-length"),
            "expected literal-length error, got {err:?}"
        );

        // 5. Billion-laughs JSON-LD context bomb refused with kind == "context-bomb"
        let billion_laughs_jsonld = r#"{
            "@context": {
                "a0": "http://example.org/ns#",
                "a1": "a0:b1",
                "a2": "a1:b2",
                "a3": "a2:b3",
                "a4": "a3:b4",
                "a5": "a4:b5"
            },
            "@id": "a5:node",
            "@type": "a5:Type"
        }"#;
        let err = parse(
            billion_laughs_jsonld.as_bytes(),
            RdfFormat::JsonLd,
            RdfLimits::default(),
        )
        .unwrap_err();
        assert!(
            matches!(&err, DataroomError::RdfLimitExceeded { kind, .. } if kind == "context-bomb"),
            "expected context-bomb error, got {err:?}"
        );

        // 6. Remote @context URL refused with kind == "remote-context" (no fetch)
        let remote_ctx_jsonld = r#"{
            "@context": "https://evil.example.org/context.jsonld",
            "@id": "https://example.org/ds",
            "@type": "http://www.w3.org/ns/dcat#Dataset"
        }"#;
        let err = parse(
            remote_ctx_jsonld.as_bytes(),
            RdfFormat::JsonLd,
            RdfLimits::default(),
        )
        .unwrap_err();
        assert!(
            matches!(&err, DataroomError::RdfLimitExceeded { kind, .. } if kind == "remote-context"),
            "expected remote-context error, got {err:?}"
        );

        // 7. CZ and SK example files still parse in both Turtle and JSON-LD
        for (content, fmt) in [
            (
                include_str!("../../specs/dcat-ap-cz/priklady-katalog/datova-sada.ttl"),
                RdfFormat::Turtle,
            ),
            (
                include_str!("../../specs/dcat-ap-cz/priklady-katalog/katalog.jsonld"),
                RdfFormat::JsonLd,
            ),
            (
                include_str!("../../specs/dcat-ap-sk/príklady/dataset.ttl"),
                RdfFormat::Turtle,
            ),
            (
                include_str!("../../specs/dcat-ap-sk/príklady/dataset.jsonld"),
                RdfFormat::JsonLd,
            ),
            (
                include_str!("../../specs/dcat-ap-sk/príklady/katalóg.ttl"),
                RdfFormat::Turtle,
            ),
            (
                include_str!("../../specs/dcat-ap-sk/príklady/katalóg.jsonld"),
                RdfFormat::JsonLd,
            ),
        ] {
            let g = parse(content.as_bytes(), fmt, RdfLimits::default()).expect("example parses");
            assert!(!g.is_empty());
        }
    }

    /// A dataset whose free-text URL fields carry IRI-breaking characters.
    fn hostile_dataset() -> Dataset {
        serde_json::from_value(serde_json::json!({
            "orgId": Uuid::nil(), "catalogueId": Uuid::nil(), "id": "pkg-x", "name": "x",
            "title": {"cs": "X"}, "description": {}, "publisherId": "city",
            "visibility": "public", "state": "active",
            "landingPage": "https://e.org/lp> . <https://victim.example/ds> <http://purl.org/dc/terms/publisher> <https://evil.example/p",
            "distributions": [{
                "id": "res-1", "datasetId": "pkg-x", "title": {},
                "accessUrl": "https://e.org/a\"b {c}|^`\\d e\n<https://evil.example/s> <https://evil.example/p> <https://evil.example/o> .\n",
                "downloadUrl": "https://e.org/dl?q=<x>"
            }],
            "issued": "2026-11-01T10:00:00Z", "modified": "2026-11-02T10:00:00Z"
        }))
        .expect("hostile dataset fixture")
    }

    #[test]
    fn a1_iri_breaking_characters_neither_inject_triples_nor_break_the_feed() {
        let org = OrgId::from_uuid(Uuid::from_u128(1));
        let cid = CatalogueId::from_uuid(Uuid::from_u128(10));
        let cat = sample_catalogue(org, cid);
        let ds = hostile_dataset();
        let expected = dataset_to_graph(&ds, &cat, None, DcatProfile::eu()).len();
        for format in [
            RdfFormat::Turtle,
            RdfFormat::NTriples,
            RdfFormat::RdfXml,
            RdfFormat::JsonLd,
        ] {
            let text = serialize_dataset(&ds, &cat, None, format, DcatProfile::eu());
            let g = match parse(text.as_bytes(), format, RdfLimits::default()) {
                Ok(v) => v,
                Err(e) => panic!("{format:?} output must parse: {e}\n{text}"),
            };
            assert_eq!(
                g.len(),
                expected,
                "{format:?} added or lost triples:\n{text}"
            );
            assert!(
                !g.triples
                    .iter()
                    .any(|t| t.predicate.starts_with("https://evil")
                        || t.subject.as_iri() == Some("https://victim.example/ds")),
                "{format:?} let a URL field write its own triple:\n{text}"
            );
        }
        // The CZ and SK JSON-LD documents carry the same URLs as plain JSON strings.
        for profile in [
            DcatProfile::from_token("dcat-ap-cz").expect("installed"),
            DcatProfile::from_token("dcat-ap-sk-2.0").expect("installed"),
        ] {
            let text = serialize_dataset(&ds, &cat, None, RdfFormat::JsonLd, profile);
            let g = match parse(text.as_bytes(), RdfFormat::JsonLd, RdfLimits::default()) {
                Ok(v) => v,
                Err(e) => panic!("{profile:?} JSON-LD must parse: {e}\n{text}"),
            };
            let access = format!("{}accessURL", ns::DCAT);
            assert_eq!(
                g.triples.iter().filter(|t| t.predicate == access).count(),
                1,
                "{profile:?} JSON-LD lost the access URL:\n{text}"
            );
            assert!(!g
                .triples
                .iter()
                .any(|t| t.predicate.starts_with("https://evil")));
        }
    }

    /// A dataset with every optional DCAT field filled.
    fn full_dataset() -> Dataset {
        serde_json::from_str(
            r#"{
            "orgId": "00000000-0000-0000-0000-000000000000", "catalogueId": "00000000-0000-0000-0000-000000000000", "id": "pkg-full", "name": "full",
            "uri": "https://data.example.org/ds/full",
            "title": {"cs": "Úplná datová sada", "en": "Full dataset"},
            "description": {"cs": "Popis", "en": "Description"},
            "publisherId": "city",
            "publisher": {"uri": "https://data.example.org/org/city", "name": {"cs": "Město"},
                          "email": "info@city.example", "homepage": "https://city.example",
                          "agentType": "http://purl.org/adms/publishertype/LocalAuthority", "identifier": "00313271"},
            "creator": {"uri": "https://data.example.org/org/author", "name": {"cs": "Autor"}},
            "contactPoint": {"kind": "Individual", "name": {"cs": "Jan"}, "email": "jan@city.example"},
            "visibility": "public", "state": "active",
            "licenceId": "cc-by",
            "themes": ["ENVI", "TRAN"],
            "eurovocThemes": ["http://eurovoc.europa.eu/100141"],
            "keywords": {"cs": ["ovzduší", "doprava"], "en": ["air"]},
            "spatial": ["https://linked.cuzk.cz/resource/ruian/obec/554782"],
            "spatialResolutionInMeters": 25.0,
            "temporal": {"start": "2020-01-01", "end": "2025-12-31"},
            "temporalResolution": "P1D",
            "frequency": "daily",
            "landingPage": "https://city.example/air",
            "documentation": ["https://city.example/air/doc"],
            "conformsTo": ["https://city.example/air/schema"],
            "applicableLegislation": ["http://data.europa.eu/eli/reg_impl/2023/138/oj"],
            "inSeries": "https://data.example.org/series/air",
            "relations": ["https://data.example.org/ds/other"],
            "sources": ["https://data.example.org/ds/source"],
            "languages": ["http://publications.europa.eu/resource/authority/language/CES"],
            "version": "2.1",
            "versionNotes": {"en": "Second release"},
            "accessRights": "http://publications.europa.eu/resource/authority/access-right/PUBLIC",
            "provenance": {"en": "Measured by sensors"},
            "dcatType": "http://publications.europa.eu/resource/authority/dataset-type/STATISTICAL",
            "identifier": "AIR-1",
            "alternateIdentifiers": ["urn:x:air"],
            "distributions": [{
                "id": "res-1", "datasetId": "pkg-full", "uri": "https://data.example.org/dist/1",
                "title": {"cs": "CSV"}, "description": {"cs": "Měření"},
                "accessUrl": "https://files.example.org/air", "downloadUrl": "https://files.example.org/air.csv",
                "format": "CSV", "mediaType": "text/csv", "compressFormat": "application/gzip",
                "packageFormat": "application/zip", "byteSize": 1234, "hash": "abc123",
                "hashAlgorithm": "http://spdx.org/rdf/terms#checksumAlgorithm_sha256",
                "licence": "cc-zero", "rights": {"en": "Free"},
                "status": "http://publications.europa.eu/resource/authority/distribution-status/COMPLETED",
                "availability": "http://data.europa.eu/r5r/availability/stable",
                "conformsTo": ["https://city.example/air/csv-schema"],
                "documentation": ["https://city.example/air/csv-doc"],
                "languages": ["http://publications.europa.eu/resource/authority/language/CES"],
                "applicableLegislation": ["http://data.europa.eu/eli/reg_impl/2023/138/oj"],
                "temporalResolution": "PT1H", "spatialResolutionInMeters": 10.0,
                "issued": "2026-01-01T00:00:00Z", "modified": "2026-02-01T00:00:00Z"
            }, {
                "id": "res-2", "datasetId": "pkg-full", "uri": "https://data.example.org/dist/2",
                "title": {"cs": "API"}, "accessUrl": "https://api.example.org/air",
                "accessServices": [{
                    "id": "svc", "uri": "https://data.example.org/svc/air", "title": {"cs": "Služba"},
                    "endpointUrls": ["https://api.example.org/air"],
                    "endpointDescriptions": ["https://api.example.org/air/openapi.json"],
                    "conformsTo": ["https://www.ogc.org/standard/sensorthings/"],
                    "applicableLegislation": ["http://data.europa.eu/eli/reg_impl/2023/138/oj"]
                }]
            }],
            "issued": "2026-11-01T10:00:00Z", "modified": "2026-11-02T10:00:00Z"
        }"#,
        )
        .expect("full dataset fixture")
    }

    #[test]
    fn every_profile_and_format_round_trips_a_full_dataset() {
        let org = OrgId::from_uuid(Uuid::from_u128(1));
        let cid = CatalogueId::from_uuid(Uuid::from_u128(10));
        let cat = sample_catalogue(org, cid);
        let src = full_dataset();
        for profile in [
            DcatProfile::eu(),
            DcatProfile::from_token("dcat-ap-cz").expect("installed"),
            DcatProfile::from_token("dcat-ap-sk-2.0").expect("installed"),
        ] {
            for format in [
                RdfFormat::Turtle,
                RdfFormat::NTriples,
                RdfFormat::RdfXml,
                RdfFormat::JsonLd,
            ] {
                let text = serialize_dataset(&src, &cat, None, format, profile);
                let back = match parse_datasets_from_rdf(
                    &text,
                    format,
                    org,
                    cid,
                    "city",
                    "cs",
                    &RdfLimits::default(),
                ) {
                    Ok(v) => v,
                    Err(e) => panic!("{profile:?} {format:?}: {e}\n{text}"),
                };
                let ctx = format!("{profile:?} {format:?}");
                assert_eq!(back.len(), 1, "{ctx}\n{text}");
                let ds = &back[0];
                assert_eq!(ds.title, src.title, "{ctx}");
                assert_eq!(ds.uri, src.uri, "{ctx}");
                assert_eq!(ds.spatial, src.spatial, "{ctx}");
                assert_eq!(ds.distributions.len(), 2, "{ctx}\n{text}");
                let csv = ds
                    .distributions
                    .iter()
                    .find(|d| d.access_url == "https://files.example.org/air")
                    .expect(&ctx);
                assert_eq!(
                    csv.download_url.as_deref(),
                    Some("https://files.example.org/air.csv"),
                    "{ctx}"
                );
                if format == RdfFormat::JsonLd && profile != DcatProfile::eu() {
                    continue; // the CZ and SK JSON-LD documents carry the profile's core fields only
                }
                assert_eq!(ds.description, src.description, "{ctx}");
                let sorted = |k: &BTreeMap<String, Vec<String>>| {
                    k.iter()
                        .map(|(l, v)| {
                            let mut v = v.clone();
                            v.sort();
                            (l.clone(), v)
                        })
                        .collect::<BTreeMap<_, _>>()
                };
                assert_eq!(
                    sorted(&ds.keywords),
                    sorted(&src.keywords),
                    "RDF keeps no keyword order: {ctx}"
                );
                assert_eq!(ds.temporal, src.temporal, "{ctx}");
                assert_eq!(ds.landing_page, src.landing_page, "{ctx}");
                assert_eq!(ds.documentation, src.documentation, "{ctx}");
                assert_eq!(ds.conforms_to, src.conforms_to, "{ctx}");
                assert_eq!(
                    ds.applicable_legislation, src.applicable_legislation,
                    "{ctx}"
                );
                assert_eq!(ds.in_series, src.in_series, "{ctx}");
                if profile == DcatProfile::eu() {
                    assert_eq!(
                        ds.identifier, src.identifier,
                        "only DCAT-AP writes dct:identifier"
                    );
                }
                assert_eq!(
                    ds.contact_point.as_ref().map(|c| c.kind),
                    Some(ContactKind::Individual),
                    "{ctx}"
                );
                assert_eq!(
                    ds.contact_point.as_ref().map(|c| &c.email),
                    Some(&"jan@city.example".to_owned()),
                    "{ctx}"
                );
                assert_eq!(csv.byte_size, Some(1234), "{ctx}");
                assert_eq!(csv.title, src.distributions[0].title, "{ctx}");
                assert!(
                    csv.terms_of_use.is_some() || profile == DcatProfile::eu(),
                    "{ctx}"
                );
                let api = ds
                    .distributions
                    .iter()
                    .find(|d| d.access_url == "https://api.example.org/air")
                    .expect(&ctx);
                assert_eq!(api.access_services.len(), 1, "{ctx}\n{text}");
                assert_eq!(
                    api.access_services[0].endpoint_urls,
                    ["https://api.example.org/air"],
                    "{ctx}"
                );
            }
        }
    }

    /// The literal and IRI objects of every triple with `predicate`, as text.
    fn objects_named(g: &RdfGraph, predicate: &str) -> Vec<String> {
        g.triples
            .iter()
            .filter(|t| t.predicate == predicate)
            .map(|t| t.object.string_value().to_owned())
            .collect()
    }

    /// The writer's own rules for edge values: an empty text is no triple, a series is typed as
    /// one, a contact point keeps its IRI and a `mailto:` it already has, a bare code becomes a
    /// NUTS IRI, a half-open period writes only its end, a fractional resolution keeps its
    /// digits, and a named checksum algorithm becomes its SPDX IRI when the profile admits it.
    #[test]
    fn edge_values_follow_the_writer_rules() {
        let org = OrgId::from_uuid(Uuid::from_u128(1));
        let cid = CatalogueId::from_uuid(Uuid::from_u128(10));
        let mut cat = sample_catalogue(org, cid);
        cat.title.insert("de".into(), String::new());
        cat.description.insert("de".into(), String::new());
        cat.publisher_name = Some(BTreeMap::from([
            ("cs".into(), "Ministerstvo dopravy".into()),
            ("de".into(), String::new()),
        ]));
        cat.contact_point = Some(
            serde_json::from_value(serde_json::json!({
                "kind": "Organization", "name": {"cs": "Úřad", "de": ""}, "email": "mailto:urad@e.org"
            }))
            .expect("catalogue contact point"),
        );
        let mut ds = full_dataset();
        ds.dataset_type = "dataset_series".into();
        ds.title.insert("de".into(), String::new());
        ds.description.insert("de".into(), String::new());
        ds.contact_point = Some(
            serde_json::from_value(serde_json::json!({
                "uri": "https://e.org/contact/jan", "kind": "Individual",
                "name": {"cs": "Jan", "de": ""}, "email": "mailto:jan@e.org"
            }))
            .expect("dataset contact point"),
        );
        ds.spatial = vec!["CZ01".into()];
        ds.temporal =
            Some(serde_json::from_value(serde_json::json!({"end": "2025-12-31"})).expect("period"));
        ds.spatial_resolution_in_meters = Some(2.5);
        ds.distributions[0].title.insert("de".into(), String::new());
        ds.distributions[0]
            .description
            .insert("de".into(), String::new());
        ds.distributions[0].hash_algorithm = Some(" SHA-1 ".into());
        ds.distributions[1].access_services[0]
            .title
            .insert("de".into(), String::new());
        ds.publisher
            .as_mut()
            .expect("fixture publisher")
            .name
            .insert("de".into(), String::new());

        let fi = DcatProfile::from_token("dcat-ap-fi-2.0").expect("installed");
        let g = catalogue_to_graph(&cat, std::slice::from_ref(&ds), &BTreeMap::new(), fi, None);
        assert!(
            !g.triples
                .iter()
                .any(|t| t.object.as_literal().is_some_and(|l| l.value.is_empty())),
            "an empty text wrote a triple"
        );
        let series = format!("{}DatasetSeries", ns::DCAT);
        assert!(objects_named(&g, ns::RDF_TYPE).contains(&series));
        assert!(objects_named(&g, &format!("{}contactPoint", ns::DCAT))
            .contains(&"https://e.org/contact/jan".to_owned()));
        let mut emails = objects_named(&g, &format!("{}hasEmail", ns::VCARD));
        emails.sort();
        assert_eq!(emails, ["mailto:jan@e.org", "mailto:urad@e.org"]);
        assert_eq!(
            objects_named(&g, &format!("{}spatial", ns::DCT)),
            ["http://data.europa.eu/nuts/code/CZ01"]
        );
        assert!(objects_named(&g, &format!("{}startDate", ns::DCAT)).is_empty());
        assert_eq!(
            objects_named(&g, &format!("{}endDate", ns::DCAT)),
            ["2025-12-31"]
        );
        assert_eq!(
            objects_named(&g, &format!("{}spatialResolutionInMeters", ns::DCAT)),
            ["2.5"]
        );
        let sha1 = "http://spdx.org/rdf/terms#checksumAlgorithm_sha1";
        assert_eq!(objects_named(&g, &format!("{}algorithm", ns::SPDX)), [sha1]);

        // FI admits SHA-1 alone: a SHA-256 checksum leaves no node there, but does under EU; an
        // algorithm SPDX does not name leaves none anywhere.
        ds.temporal = Some(
            serde_json::from_value(serde_json::json!({"start": "2020-01-01"})).expect("period"),
        );
        ds.distributions[0].hash_algorithm = Some("sha256".into());
        let g = dataset_to_graph(&ds, &cat, None, fi);
        assert!(objects_named(&g, &format!("{}checksum", ns::SPDX)).is_empty());
        assert_eq!(
            objects_named(&g, &format!("{}startDate", ns::DCAT)),
            ["2020-01-01"]
        );
        assert!(objects_named(&g, &format!("{}endDate", ns::DCAT)).is_empty());
        let g = dataset_to_graph(&ds, &cat, None, DcatProfile::eu());
        assert_eq!(
            objects_named(&g, &format!("{}algorithm", ns::SPDX)),
            ["http://spdx.org/rdf/terms#checksumAlgorithm_sha256"]
        );
        ds.distributions[0].hash_algorithm = Some("crc32".into());
        let g = dataset_to_graph(&ds, &cat, None, DcatProfile::eu());
        assert!(objects_named(&g, &format!("{}checksum", ns::SPDX)).is_empty());
    }

    /// Harvested graphs as other catalogues write them: a node typed both Dataset and
    /// DatasetSeries is one series, a non-literal title or a literal distribution or service is
    /// skipped, a tag repeated after cleaning is kept once, a literal publisher has no names,
    /// and the dataset's licence is the one every distribution states, or none.
    #[test]
    fn harvested_edge_shapes_are_read_by_the_reader_rules() {
        let org = OrgId::from_uuid(Uuid::from_u128(1));
        let cid = CatalogueId::from_uuid(Uuid::from_u128(10));
        let ttl = r#"
            @prefix dcat: <http://www.w3.org/ns/dcat#> .
            @prefix dct: <http://purl.org/dc/terms/> .
            <https://e.org/ds/a--b> a dcat:Dataset, dcat:DatasetSeries ;
               dct:title "Both"@en, <https://e.org/not-a-text> ;
               dcat:keyword "air"@en, "air!"@en ;
               dct:publisher "Město" ;
               dcat:distribution "not a node", <https://e.org/dist/1>, <https://e.org/dist/2> .
            <https://e.org/dist/1> a dcat:Distribution ;
               dct:license <https://creativecommons.org/licenses/by/4.0/> ;
               dcat:accessService "not a node" .
            <https://e.org/dist/2> a dcat:Distribution ;
               dct:license <https://creativecommons.org/licenses/by/4.0/> .
            <https://e.org/ds/custom> a dcat:Dataset ; dcat:distribution <https://e.org/dist/3>, <https://e.org/dist/4> .
            <https://e.org/dist/3> dct:license <https://licence.example/own> .
            <https://e.org/dist/4> dct:license <https://licence.example/own> .
            <https://e.org/ds/mixed> a dcat:Dataset ; dcat:distribution <https://e.org/dist/5>, <https://e.org/dist/6> .
            <https://e.org/dist/5> dct:license <https://licence.example/own> .
            <https://e.org/dist/6> dct:license <https://creativecommons.org/licenses/by/4.0/> .
        "#;
        let mut ds = parse_datasets_from_rdf(
            ttl,
            RdfFormat::Turtle,
            org,
            cid,
            "city",
            "en",
            &RdfLimits::default(),
        )
        .expect("harvested turtle");
        ds.sort_by(|a, b| a.uri.cmp(&b.uri));
        assert_eq!(ds.len(), 3, "the series node counts once");
        let (both, custom, mixed) = (&ds[0], &ds[1], &ds[2]);
        assert_eq!(both.name, "a-b", "a run of separators is one dash");
        assert_eq!(both.dataset_type, "dataset_series");
        assert_eq!(both.title.get("en").map(String::as_str), Some("Both"));
        assert_eq!(both.title.len(), 1, "an IRI is no title");
        assert_eq!(both.tags, ["air"], "one tag after cleaning");
        let publisher = both
            .publisher
            .as_ref()
            .expect("a literal publisher is kept");
        assert!(publisher.uri.is_none() && publisher.name.is_empty());
        assert_eq!(
            both.distributions.len(),
            2,
            "a literal distribution is skipped"
        );
        assert!(both.distributions[0].access_services.is_empty());
        assert_eq!(both.licence_id.as_deref(), Some("cc-by"));
        assert_eq!(custom.licence_id, None, "no known licence");
        assert_eq!(
            custom.licence_url.as_deref(),
            Some("https://licence.example/own")
        );
        assert_eq!(
            (mixed.licence_id.as_ref(), mixed.licence_url.as_ref()),
            (None, None),
            "distributions disagree"
        );
    }

    #[test]
    fn catalogue_graph_carries_contact_point_publishers_and_hydra_paging() {
        let org = OrgId::from_uuid(Uuid::from_u128(1));
        let cid = CatalogueId::from_uuid(Uuid::from_u128(10));
        let mut cat = sample_catalogue(org, cid);
        cat.contact_point = Some(
            serde_json::from_value(serde_json::json!(
            {"kind": "Organization", "name": {"cs": "Odbor"}, "email": "data@city.example"}))
            .unwrap(),
        );
        let hydra = HydraPageInfo {
            collection_uri: "https://opendata.example.org/catalog".into(),
            current_page_uri: "https://opendata.example.org/catalog?page=2".into(),
            total_items: 30,
            items_per_page: 10,
            first_page_uri: "https://opendata.example.org/catalog?page=1".into(),
            last_page_uri: "https://opendata.example.org/catalog?page=3".into(),
            next_page_uri: Some("https://opendata.example.org/catalog?page=3".into()),
            prev_page_uri: Some("https://opendata.example.org/catalog?page=1".into()),
        };
        let pubs = BTreeMap::new();
        for profile in [
            DcatProfile::eu(),
            DcatProfile::from_token("dcat-ap-cz").expect("installed"),
            DcatProfile::from_token("dcat-ap-sk-2.0").expect("installed"),
        ] {
            for format in [
                RdfFormat::Turtle,
                RdfFormat::NTriples,
                RdfFormat::RdfXml,
                RdfFormat::JsonLd,
            ] {
                let text = serialize_catalogue(
                    &cat,
                    &[full_dataset()],
                    &pubs,
                    format,
                    profile,
                    Some(&hydra),
                );
                let g = match parse(text.as_bytes(), format, RdfLimits::default()) {
                    Ok(v) => v,
                    Err(e) => panic!("{profile:?} {format:?}: {e}\n{text}"),
                };
                let page = SubjectNode::Named(hydra.current_page_uri.clone());
                let next = g.first_object(&page, &format!("{}next", ns::HYDRA));
                assert_eq!(
                    next.as_ref().and_then(Term::as_iri),
                    hydra.next_page_uri.as_deref(),
                    "{profile:?} {format:?}\n{text}"
                );
                let cat_subj = SubjectNode::Named("https://opendata.example.org/catalog".into());
                assert!(
                    g.first_object(&cat_subj, &format!("{}contactPoint", ns::DCAT))
                        .is_some(),
                    "{profile:?} {format:?}"
                );
                assert_eq!(
                    g.objects_of(&cat_subj, &format!("{}dataset", ns::DCAT)),
                    [Term::Iri("https://data.example.org/ds/full".into())],
                    "{profile:?} {format:?}"
                );
            }
        }
        // The first and last page have no previous and next link.
        let first = HydraPageInfo {
            prev_page_uri: None,
            next_page_uri: None,
            ..hydra
        };
        let g = catalogue_to_graph(&cat, &[], &pubs, DcatProfile::eu(), Some(&first));
        let page = SubjectNode::Named(first.current_page_uri.clone());
        assert!(g
            .first_object(&page, &format!("{}next", ns::HYDRA))
            .is_none());
        assert!(g
            .first_object(&page, &format!("{}previous", ns::HYDRA))
            .is_none());
    }

    #[test]
    fn content_negotiation_by_format_and_profile() {
        for (ext, f) in [
            ("ttl", RdfFormat::Turtle),
            (".JSONLD", RdfFormat::JsonLd),
            ("rdf", RdfFormat::RdfXml),
            ("nt", RdfFormat::NTriples),
        ] {
            assert_eq!(RdfFormat::from_extension(ext), Some(f));
            assert_eq!(
                RdfFormat::from_extension(f.suffix()),
                Some(f),
                "a feed's suffix reads back"
            );
            assert!(f.content_type().ends_with("charset=utf-8"));
        }
        assert_eq!(RdfFormat::from_extension("csv"), None);
        assert_eq!(
            negotiate_rdf_format(Some("nt"), Some("text/turtle")),
            Some(RdfFormat::NTriples),
            "the extension wins"
        );
        assert_eq!(
            negotiate_rdf_format(Some("csv"), Some("application/ld+json")),
            Some(RdfFormat::JsonLd)
        );
        assert_eq!(negotiate_rdf_format(None, None), None);
        assert_eq!(negotiate_rdf_format(None, Some("*/*")), None);
        assert_eq!(
            negotiate_rdf_format(
                None,
                Some("text/html,application/xhtml+xml;q=0.9,*/*;q=0.8")
            ),
            None,
            "a browser gets HTML"
        );
        assert_eq!(
            negotiate_rdf_format(None, Some("application/xml;q=0.1, Application/N-Triples")),
            Some(RdfFormat::NTriples)
        );
        // Weights decide, and q=0 means "not acceptable" (RFC 9110 12.4.2).
        assert_eq!(
            negotiate_rdf_format(None, Some("text/turtle;q=0.2, application/ld+json;q=0.9")),
            Some(RdfFormat::JsonLd)
        );
        assert_eq!(
            negotiate_rdf_format(None, Some("text/turtle;q=0, application/rdf+xml")),
            Some(RdfFormat::RdfXml)
        );
        assert_eq!(negotiate_rdf_format(None, Some("text/turtle;q=bad")), None);
        assert_eq!(
            negotiate_rdf_format(None, Some("text/html;q=0.1, text/turtle")),
            Some(RdfFormat::Turtle)
        );

        for k in DcatProfile::installed() {
            assert_eq!(DcatProfile::from_token(k.id()), Some(k));
            assert_eq!(
                DcatProfile::from_token(&format!("<{}>", k.profile_uri())),
                Some(k)
            );
        }
        assert_eq!(DcatProfile::from_token("dcat-ap-de"), None);
        assert_eq!(
            accepted_profiles("cz;q=0.1, sk;q=0.8, eu;q=0"),
            ["sk", "cz"]
        );
    }

    #[test]
    fn harvested_graphs_fill_slugs_languages_and_dates() {
        let org = OrgId::from_uuid(Uuid::from_u128(1));
        let cid = CatalogueId::from_uuid(Uuid::from_u128(10));
        let ttl = r#"
            @prefix dcat: <http://www.w3.org/ns/dcat#> .
            @prefix dct: <http://purl.org/dc/terms/> .
            [] a dcat:Dataset ; dct:title "Bez jazyka" ; dcat:keyword "slovo" ;
               dct:issued "2026-03-04" ; dct:modified "2026-03-04T05:06:07" .
            <https://e.org/ds/Air_Quality/> a dcat:Dataset ; dct:title "Two"@en ;
               dct:issued "not a date" .
        "#;
        let mut ds = parse_datasets_from_rdf(
            ttl,
            RdfFormat::Turtle,
            org,
            cid,
            "city",
            "sk",
            &RdfLimits::default(),
        )
        .expect("harvested turtle");
        ds.sort_by(|a, b| a.uri.cmp(&b.uri));
        let blank = &ds[0];
        assert!(
            blank.name.starts_with("dataset-"),
            "a blank node gets a generated slug: {}",
            blank.name
        );
        assert_eq!(
            blank.title.get("sk").map(String::as_str),
            Some("Bez jazyka")
        );
        assert_eq!(blank.keywords.get("sk"), Some(&vec!["slovo".to_owned()]));
        assert_eq!(blank.issued.to_rfc3339(), "2026-03-04T00:00:00+00:00");
        assert_eq!(blank.modified.to_rfc3339(), "2026-03-04T05:06:07+00:00");
        assert_eq!(ds[1].name, "air-quality", "the IRI tail becomes the slug");
        assert_ne!(ds[1].issued.to_rfc3339(), "not a date");
    }

    // DCAT dates as chrono 0.4.45 wrote and read them, pinned before the move to jiff (T-7238).
    #[test]
    fn dcat_dates_keep_chrono_bytes() {
        let mut v = serde_json::to_value(full_dataset()).expect("json");
        v["issued"] = serde_json::json!("2026-03-29T03:30:00.120+02:00");
        v["modified"] = serde_json::json!("2026-10-25T01:30:59.999+03:00");
        v["temporal"] = serde_json::json!({"start": "0024-03-01", "end": "2026-10-25"});
        let ds: Dataset = serde_json::from_value(v).expect("dataset");
        let cid = ds.catalogue_id;
        let cat = sample_catalogue(ds.org_id, cid);
        let ttl = serialize_dataset(&ds, &cat, None, RdfFormat::NTriples, DcatProfile::eu());
        let lines: Vec<&str> = ttl
            .lines()
            .filter(|l| l.contains("issued") || l.contains("modified") || l.contains("Date>"))
            .collect();
        let xsd = "^^<http://www.w3.org/2001/XMLSchema#";
        assert_eq!(
            lines,
            [
                format!("<https://data.example.org/ds/full> <http://purl.org/dc/terms/issued> \"2026-03-29T01:30:00Z\"{xsd}dateTime> ."),
                format!("<https://data.example.org/ds/full> <http://purl.org/dc/terms/modified> \"2026-10-24T22:30:59Z\"{xsd}dateTime> ."),
                format!("_:b2 <http://www.w3.org/ns/dcat#endDate> \"2026-10-25\"{xsd}date> ."),
                format!("_:b2 <http://www.w3.org/ns/dcat#startDate> \"0024-03-01\"{xsd}date> ."),
            ]
        );
        for (input, want) in [
            (
                "2026-03-29T03:30:00.5+02:00",
                Some("2026-03-29T01:30:00.500+00:00"),
            ),
            ("2026-03-29T03:30:00", Some("2026-03-29T03:30:00+00:00")),
            ("2026-3-29T3:30:00", Some("2026-03-29T03:30:00+00:00")),
            ("2026-03-29T03:30", Some("2026-03-29T00:00:00+00:00")),
            ("2026-03-29garbage", Some("2026-03-29T00:00:00+00:00")),
            ("2026-02-30T00:00:00", None),
            ("2026-02-30", None),
            ("26-03-29", None),
        ] {
            assert_eq!(
                parse_any_datetime(input).map(|t| t.to_rfc3339()).as_deref(),
                want,
                "{input}"
            );
        }
    }

    #[test]
    fn publisher_licence_and_service_fallbacks() {
        let org = OrgId::from_uuid(Uuid::from_u128(1));
        let cid = CatalogueId::from_uuid(Uuid::from_u128(10));
        let mut cat = sample_catalogue(org, cid);
        let mut ds = full_dataset();
        ds.licence_id = Some("https://licence.example/custom".into());
        ds.distributions[0].licence = None;
        ds.distributions[1].access_services[0].uri = None;
        let publisher: Publisher = serde_json::from_value(serde_json::json!({
            "orgId": Uuid::nil(), "catalogueId": Uuid::nil(), "id": "p", "name": "city",
            "title": {"cs": "Město"}, "description": {}, "uri": "https://e.org/publisher/city",
            "state": "active", "editorGrants": [],
            "created": "2026-11-01T10:00:00Z", "modified": "2026-11-01T10:00:00Z"
        }))
        .unwrap();
        let g = dataset_to_graph(&ds, &cat, Some(&publisher), DcatProfile::eu());
        let ds_subj = SubjectNode::Named("https://data.example.org/ds/full".into());
        assert_eq!(
            g.first_object(&ds_subj, &format!("{}publisher", ns::DCT)),
            Some(Term::Iri("https://e.org/publisher/city".into())),
            "the catalogue's publisher record wins over the dataset's own"
        );
        let dist = SubjectNode::Named("https://data.example.org/dist/1".into());
        assert_eq!(
            g.first_object(&dist, &format!("{}license", ns::DCT)),
            Some(Term::Iri("https://licence.example/custom".into())),
            "an unknown licence URL is written as it is"
        );
        let svc = format!("{}/service/svc", ds.canonical_uri(&cat.base_url()));
        assert!(
            !g.objects_of(&SubjectNode::Named(svc), ns::RDF_TYPE)
                .is_empty(),
            "a service without IRI gets one"
        );
        let text = serialize_dataset(
            &ds,
            &cat,
            Some(&publisher),
            RdfFormat::JsonLd,
            DcatProfile::from_token("dcat-ap-cz").expect("installed"),
        );
        assert!(text.contains("https://e.org/publisher/city"));

        // A catalogue's publisher is the one it declares, never the first of its datasets'.
        cat.publisher_uri = None;
        cat.publisher_name = None;
        cat.homepage = None;
        let pubs = BTreeMap::from([("city".to_owned(), publisher)]);
        let cat_subj = SubjectNode::Named("https://opendata.example.org/catalog".into());
        let g = catalogue_to_graph(&cat, &[], &pubs, DcatProfile::eu(), None);
        assert_eq!(
            g.first_object(&cat_subj, &format!("{}publisher", ns::DCT)),
            None
        );
        assert_eq!(
            g.first_object(&cat_subj, &format!("{}homepage", ns::FOAF)),
            Some(Term::Iri("https://opendata.example.org".into()))
        );
        let sk = serialize_catalogue(
            &cat,
            &[],
            &BTreeMap::new(),
            RdfFormat::JsonLd,
            DcatProfile::from_token("sk").expect("installed"),
            None,
        );
        assert!(!sk.contains("poskytovateľ"));

        cat.publisher_uri = Some("https://data.gov.sk/id/legal-subject/00000000".into());
        cat.publisher_name = Some(BTreeMap::from([("sk".into(), "Mesto".into())]));
        let g = catalogue_to_graph(&cat, &[], &pubs, DcatProfile::eu(), None);
        let agent = g.first_object(&cat_subj, &format!("{}publisher", ns::DCT));
        assert!(
            matches!(&agent, Some(term) if term.as_iri().is_none()),
            "a placeholder is no IRI, the declared name stays: {agent:?}"
        );
    }
}
