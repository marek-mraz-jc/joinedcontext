#![forbid(unsafe_code)]
//! Bidirectional DCAT-AP 3.0.1, DCAT-AP-CZ and DCAT-AP-SK 2.0 graph mapping, Hydra pagination,
//! and W3C `Accept-Profile` / `Accept` content negotiation (C7, C17).

use crate::graph::{ns, LiteralValue, RdfGraph, RdfLimits, SubjectNode, Term};
use crate::jsonld::{
    parse_jsonld, serialize_cz_catalogue_jsonld, serialize_cz_dataset_jsonld,
    serialize_graph_jsonld, serialize_sk_catalogue_jsonld, serialize_sk_dataset_jsonld,
};
use crate::rdfxml::{parse_rdfxml, serialize_rdfxml};
use crate::turtle::{parse_turtle_checked, serialize_ntriples, serialize_turtle};
use chrono::{DateTime, NaiveDate, Utc};
use dataroom_core::vocab::{
    eu_iri_to_language, find_licence, format_to_eu_iri, frequency_to_eu_iri,
    iana_iri_to_media_type, language_to_eu_iri, media_type_to_iana_iri, theme_code, theme_to_iri,
};
use dataroom_core::{
    AgentDetails, Catalogue, CatalogueId, ContactKind, ContactPoint, DataService, DataroomError,
    Dataset, Distribution, OrgId, Publisher, RecordState, TemporalPeriod, TermsOfUse, Visibility,
};
use std::collections::BTreeMap;
use uuid::Uuid;

/// Supported RDF serialization formats (C7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// Negotiates an [`RdfFormat`] from an optional URL extension or `Accept` header.
pub fn negotiate_rdf_format(ext: Option<&str>, accept: Option<&str>) -> Option<RdfFormat> {
    if let Some(ext) = ext {
        if let Some(fmt) = RdfFormat::from_extension(ext) {
            return Some(fmt);
        }
    }
    let accept = accept?;
    for part in accept.split(',') {
        let media = part.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
        match media.as_str() {
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

/// Built-in profile families for DCAT-AP rendering (C7, C17).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DcatProfileKind {
    Eu301,
    Cz,
    Sk20,
}

impl DcatProfileKind {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Eu301 => "dcat-ap-3.0.1",
            Self::Cz => "dcat-ap-cz",
            Self::Sk20 => "dcat-ap-sk-2.0",
        }
    }

    pub const fn profile_uri(self) -> &'static str {
        match self {
            Self::Eu301 => "https://semiceu.github.io/DCAT-AP/releases/3.0.1",
            Self::Cz => "https://ofn.gov.cz/dcat-ap-cz",
            Self::Sk20 => "https://datova-kancelaria.github.io/dcat-ap-sk-2.0/",
        }
    }

    pub fn from_token(token: &str) -> Option<Self> {
        let clean = token
            .trim()
            .trim_matches(|c| c == '<' || c == '>' || c == '"');
        if clean.eq_ignore_ascii_case("dcat-ap-cz")
            || clean.eq_ignore_ascii_case("cz")
            || clean.contains("ofn.gov.cz/dcat-ap-cz")
        {
            Some(Self::Cz)
        } else if clean.eq_ignore_ascii_case("dcat-ap-sk-2.0")
            || clean.eq_ignore_ascii_case("dcat-ap-sk")
            || clean.eq_ignore_ascii_case("sk")
            || clean.contains("dcat-ap-sk")
        {
            Some(Self::Sk20)
        } else if clean.eq_ignore_ascii_case("dcat-ap-3.0.1")
            || clean.eq_ignore_ascii_case("eu")
            || clean.eq_ignore_ascii_case("euro_dcat_ap_3")
            || clean.contains("semiceu.github.io/DCAT-AP")
        {
            Some(Self::Eu301)
        } else {
            None
        }
    }
}

/// W3C Content Negotiation by Profile (`Accept-Profile` header or `?_profile=` query param, C17).
pub fn negotiate_profile(
    query_profile: Option<&str>,
    accept_profile_header: Option<&str>,
    catalogue_default: &str,
) -> (DcatProfileKind, String) {
    if let Some(qp) = query_profile {
        if let Some(kind) = DcatProfileKind::from_token(qp) {
            return (kind, kind.profile_uri().to_owned());
        }
    }
    if let Some(hdr) = accept_profile_header {
        for part in hdr.split(',') {
            let token = part.split(';').next().unwrap_or("").trim();
            if let Some(kind) = DcatProfileKind::from_token(token) {
                return (kind, kind.profile_uri().to_owned());
            }
        }
    }
    let kind = DcatProfileKind::from_token(catalogue_default).unwrap_or(DcatProfileKind::Eu301);
    (kind, kind.profile_uri().to_owned())
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

/// Converts a [`Dataset`] into an [`RdfGraph`] according to `profile` (`Eu301`, `Cz`, `Sk20`).
pub fn dataset_to_graph(
    dataset: &Dataset,
    catalogue: &Catalogue,
    publisher: Option<&Publisher>,
    profile: DcatProfileKind,
) -> RdfGraph {
    let mut g = RdfGraph::new();
    add_dataset_triples(&mut g, dataset, catalogue, publisher, profile);
    g
}

pub fn add_dataset_triples(
    g: &mut RdfGraph,
    dataset: &Dataset,
    catalogue: &Catalogue,
    publisher: Option<&Publisher>,
    profile: DcatProfileKind,
) -> SubjectNode {
    let base = catalogue.base_url();
    let ds_iri = dataset.canonical_uri(&base);
    let ds_subj = SubjectNode::Named(ds_iri.clone());

    let rdf_class = if dataset.dataset_type == "dataset_series" {
        "http://www.w3.org/ns/dcat#DatasetSeries"
    } else {
        "http://www.w3.org/ns/dcat#Dataset"
    };
    g.insert(ds_subj.clone(), ns::RDF_TYPE, Term::Iri(rdf_class.to_owned()));

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
    let ident = dataset
        .identifier
        .as_deref()
        .unwrap_or(&dataset.name);
    if profile == DcatProfileKind::Eu301 {
        g.insert(
            ds_subj.clone(),
            format!("{}identifier", ns::DCT),
            Term::Literal(LiteralValue::plain(ident)),
        );
    }

    // Publisher (`dct:publisher`)
    let pub_iri = publisher
        .map(|p| p.uri.clone())
        .or_else(|| dataset.publisher.as_ref().and_then(|a| a.uri.clone()))
        .unwrap_or_else(|| format!("{base}/publisher/{}", dataset.publisher_id));
    let pub_subj = SubjectNode::Named(pub_iri.clone());
    g.insert(
        ds_subj.clone(),
        format!("{}publisher", ns::DCT),
        Term::Iri(pub_iri),
    );
    g.insert(
        pub_subj.clone(),
        ns::RDF_TYPE,
        Term::Iri(format!("{}Agent", ns::FOAF)),
    );
    let pub_names = publisher
        .map(|p| &p.title)
        .or_else(|| dataset.publisher.as_ref().map(|a| &a.name));
    if let Some(names) = pub_names {
        for (lang, name) in names {
            if !name.is_empty() {
                g.insert(
                    pub_subj.clone(),
                    format!("{}name", ns::FOAF),
                    Term::Literal(LiteralValue::lang(name.clone(), lang.clone())),
                );
            }
        }
    } else {
        g.insert(
            pub_subj.clone(),
            format!("{}name", ns::FOAF),
            Term::Literal(LiteralValue::lang(
                dataset.publisher_id.clone(),
                catalogue.default_language.clone(),
            )),
        );
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
        let vcard_class = match (profile, cp.kind) {
            (DcatProfileKind::Eu301, _) => format!("{}Kind", ns::VCARD),
            (_, ContactKind::Organization) => format!("{}Organization", ns::VCARD),
            (_, ContactKind::Individual) => format!("{}Individual", ns::VCARD),
        };
        g.insert(cp_subj.clone(), ns::RDF_TYPE, Term::Iri(vcard_class));
        if profile != DcatProfileKind::Eu301 {
            g.insert(
                cp_subj.clone(),
                ns::RDF_TYPE,
                Term::Iri(format!("{}Kind", ns::VCARD)),
            );
        }
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
            g.insert(
                cp_subj,
                format!("{}hasEmail", ns::VCARD),
                Term::Iri(mailto),
            );
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

    if profile != DcatProfileKind::Cz {
        g.insert(
            ds_subj.clone(),
            format!("{}issued", ns::DCT),
            Term::Literal(LiteralValue::typed(
                dataset.issued.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                ns::XSD_DATE_TIME,
            )),
        );
        g.insert(
            ds_subj.clone(),
            format!("{}modified", ns::DCT),
            Term::Literal(LiteralValue::typed(
                dataset.modified.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
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
        if let Some(hash) = &dist.hash {
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
                let algo_iri = "http://spdx.org/rdf/terms#checksumAlgorithm_sha256";
                g.insert(
                    cs_node,
                    format!("{}algorithm", ns::SPDX),
                    Term::Iri(algo_iri.to_owned()),
                );
                g.insert(
                    SubjectNode::Named(algo_iri.to_owned()),
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
        for law in &dist.applicable_legislation {
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

        // Profile-specific licence / terms-of-use representation
        let dist_lic = dist
            .licence
            .as_deref()
            .or(dataset.licence_id.as_deref())
            .or(dataset.licence_url.as_deref());
        let resolved_lic = dist_lic.and_then(find_licence);

        match profile {
            DcatProfileKind::Eu301 => {
                if let Some(lic) = resolved_lic {
                    g.insert(
                        dist_subj.clone(),
                        format!("{}license", ns::DCT),
                        Term::Iri(lic.eu_iri.clone()),
                    );
                    g.insert(
                        SubjectNode::Named(lic.eu_iri),
                        ns::RDF_TYPE,
                        Term::Iri(format!("{}LicenseDocument", ns::DCT)),
                    );
                } else if let Some(raw_lic) = dist_lic.filter(|s| s.contains("://")) {
                    g.insert(
                        dist_subj.clone(),
                        format!("{}license", ns::DCT),
                        Term::Iri(raw_lic.to_owned()),
                    );
                    g.insert(
                        SubjectNode::Named(raw_lic.to_owned()),
                        ns::RDF_TYPE,
                        Term::Iri(format!("{}LicenseDocument", ns::DCT)),
                    );
                }
            }
            DcatProfileKind::Cz => {
                let spec_node = g.fresh_blank_node();
                g.insert(
                    dist_subj.clone(),
                    format!("{}specifikace", ns::CZ_PU),
                    spec_node.to_term(),
                );
                g.insert(
                    spec_node.clone(),
                    ns::RDF_TYPE,
                    Term::Iri(format!("{}Specifikace", ns::CZ_PU)),
                );
                let tou = dist.terms_of_use.clone().unwrap_or_else(|| {
                    let cc_url = resolved_lic
                        .as_ref()
                        .map(|l| l.url.clone())
                        .filter(|u| !u.is_empty())
                        .unwrap_or_else(|| {
                            "https://data.gov.cz/podmínky-užití/neobsahuje-autorská-díla/".into()
                        });
                    TermsOfUse {
                        authors_work: Some(cc_url.clone()),
                        author_name: BTreeMap::new(),
                        database_as_work: Some(
                            "https://data.gov.cz/podmínky-užití/není-autorskoprávně-chráněnou-databází/"
                                .into(),
                        ),
                        database_author_name: BTreeMap::new(),
                        sui_generis_database: Some(
                            "https://data.gov.cz/podmínky-užití/není-chráněna-zvláštním-právem-pořizovatele-databáze/"
                                .into(),
                        ),
                        personal_data: Some(
                            "https://data.gov.cz/podmínky-užití/neobsahuje-osobní-údaje/".into(),
                        ),
                    }
                });
                if let Some(aw) = &tou.authors_work {
                    g.insert(
                        spec_node.clone(),
                        format!("{}autorské-dílo", ns::CZ_PU),
                        Term::Iri(aw.clone()),
                    );
                }
                for (lang, author) in &tou.author_name {
                    g.insert(
                        spec_node.clone(),
                        format!("{}autor", ns::CZ_PU),
                        Term::Literal(LiteralValue::lang(author.clone(), lang.clone())),
                    );
                }
                if let Some(db) = &tou.database_as_work {
                    g.insert(
                        spec_node.clone(),
                        format!("{}databáze-jako-autorské-dílo", ns::CZ_PU),
                        Term::Iri(db.clone()),
                    );
                }
                for (lang, db_author) in &tou.database_author_name {
                    g.insert(
                        spec_node.clone(),
                        format!("{}autor-databáze", ns::CZ_PU),
                        Term::Literal(LiteralValue::lang(db_author.clone(), lang.clone())),
                    );
                }
                if let Some(sg) = &tou.sui_generis_database {
                    g.insert(
                        spec_node.clone(),
                        format!("{}databáze-chráněná-zvláštními-právy", ns::CZ_PU),
                        Term::Iri(sg.clone()),
                    );
                }
                if let Some(pd) = &tou.personal_data {
                    g.insert(
                        spec_node,
                        format!("{}osobní-údaje", ns::CZ_PU),
                        Term::Iri(pd.clone()),
                    );
                }
            }
            DcatProfileKind::Sk20 => {
                let tou_node = g.fresh_blank_node();
                g.insert(
                    dist_subj.clone(),
                    format!("{}termsOfUse", ns::SK_LEG),
                    tou_node.to_term(),
                );
                g.insert(
                    tou_node.clone(),
                    ns::RDF_TYPE,
                    Term::Iri(format!("{}TermsOfUse", ns::SK_LEG)),
                );
                let tou = dist.terms_of_use.clone().unwrap_or_else(|| {
                    let cc_url = resolved_lic
                        .as_ref()
                        .map(|l| l.url.clone())
                        .filter(|u| !u.is_empty())
                        .unwrap_or_else(|| "https://creativecommons.org/licenses/by/4.0/".into());
                    TermsOfUse {
                        authors_work: Some(cc_url.clone()),
                        author_name: BTreeMap::new(),
                        database_as_work: Some(cc_url),
                        database_author_name: BTreeMap::new(),
                        sui_generis_database: Some(
                            "https://creativecommons.org/publicdomain/zero/1.0/".into(),
                        ),
                        personal_data: Some(
                            "https://data.gov.sk/def/personal-data-occurence-type/2".into(),
                        ),
                    }
                });
                if let Some(aw) = &tou.authors_work {
                    g.insert(
                        tou_node.clone(),
                        format!("{}authorsWorkType", ns::SK_LEG),
                        Term::Iri(aw.clone()),
                    );
                }
                for (lang, author) in &tou.author_name {
                    g.insert(
                        tou_node.clone(),
                        format!("{}authorName", ns::SK_LEG),
                        Term::Literal(LiteralValue::lang(author.clone(), lang.clone())),
                    );
                }
                if let Some(db) = &tou.database_as_work {
                    g.insert(
                        tou_node.clone(),
                        format!("{}originalDatabaseType", ns::SK_LEG),
                        Term::Iri(db.clone()),
                    );
                }
                for (lang, db_author) in &tou.database_author_name {
                    g.insert(
                        tou_node.clone(),
                        format!("{}originalDatabaseAuthorName", ns::SK_LEG),
                        Term::Literal(LiteralValue::lang(db_author.clone(), lang.clone())),
                    );
                }
                if let Some(sg) = &tou.sui_generis_database {
                    g.insert(
                        tou_node.clone(),
                        format!("{}databaseProtectedBySpecialRightsType", ns::SK_LEG),
                        Term::Iri(sg.clone()),
                    );
                }
                if let Some(pd) = &tou.personal_data {
                    g.insert(
                        tou_node,
                        format!("{}personalDataContainmentType", ns::SK_LEG),
                        Term::Iri(pd.clone()),
                    );
                }
            }
        }

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
            for law in &svc.applicable_legislation {
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

    ds_subj
}

/// Converts a [`Catalogue`], its [`Dataset`]s, [`Publisher`]s and optional [`HydraPageInfo`]
/// into an [`RdfGraph`].
pub fn catalogue_to_graph(
    catalogue: &Catalogue,
    datasets: &[Dataset],
    publishers: &BTreeMap<String, Publisher>,
    profile: DcatProfileKind,
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

    let pub_iri = catalogue
        .publisher_uri
        .clone()
        .or_else(|| publishers.values().next().map(|p| p.uri.clone()))
        .unwrap_or_else(|| format!("{base}/publisher/default"));
    let pub_subj = SubjectNode::Named(pub_iri.clone());
    g.insert(
        cat_subj.clone(),
        format!("{}publisher", ns::DCT),
        Term::Iri(pub_iri),
    );
    g.insert(
        pub_subj.clone(),
        ns::RDF_TYPE,
        Term::Iri(format!("{}Agent", ns::FOAF)),
    );
    if let Some(names) = &catalogue.publisher_name {
        for (lang, n) in names {
            if !n.is_empty() {
                g.insert(
                    pub_subj.clone(),
                    format!("{}name", ns::FOAF),
                    Term::Literal(LiteralValue::lang(n.clone(), lang.clone())),
                );
            }
        }
    } else {
        g.insert(
            pub_subj,
            format!("{}name", ns::FOAF),
            Term::Literal(LiteralValue::lang(
                catalogue.theme.organisation.clone(),
                catalogue.default_language.clone(),
            )),
        );
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

    g
}

/// Serializes one [`Dataset`] in the requested [`RdfFormat`] and [`DcatProfileKind`] (C7, C17).
pub fn serialize_dataset(
    dataset: &Dataset,
    catalogue: &Catalogue,
    publisher: Option<&Publisher>,
    format: RdfFormat,
    profile: DcatProfileKind,
) -> String {
    if format == RdfFormat::JsonLd {
        match profile {
            DcatProfileKind::Cz => {
                return serialize_cz_dataset_jsonld(dataset, catalogue, publisher)
            }
            DcatProfileKind::Sk20 => {
                return serialize_sk_dataset_jsonld(dataset, catalogue, publisher)
            }
            DcatProfileKind::Eu301 => {}
        }
    }
    let g = dataset_to_graph(dataset, catalogue, publisher, profile);
    match format {
        RdfFormat::Turtle => serialize_turtle(&g),
        RdfFormat::JsonLd => serialize_graph_jsonld(&g),
        RdfFormat::RdfXml => serialize_rdfxml(&g),
        RdfFormat::NTriples => serialize_ntriples(&g),
    }
}

/// Serializes a [`Catalogue`] in the requested [`RdfFormat`] and [`DcatProfileKind`] (C7, C17).
pub fn serialize_catalogue(
    catalogue: &Catalogue,
    datasets: &[Dataset],
    publishers: &BTreeMap<String, Publisher>,
    format: RdfFormat,
    profile: DcatProfileKind,
    hydra: Option<&HydraPageInfo>,
) -> String {
    if format == RdfFormat::JsonLd && hydra.is_none() {
        match profile {
            DcatProfileKind::Cz => {
                return serialize_cz_catalogue_jsonld(catalogue, datasets, publishers)
            }
            DcatProfileKind::Sk20 => {
                return serialize_sk_catalogue_jsonld(catalogue, datasets, publishers)
            }
            DcatProfileKind::Eu301 => {}
        }
    }
    let g = catalogue_to_graph(catalogue, datasets, publishers, profile, hydra);
    match format {
        RdfFormat::Turtle => serialize_turtle(&g),
        RdfFormat::JsonLd => serialize_graph_jsonld(&g),
        RdfFormat::RdfXml => serialize_rdfxml(&g),
        RdfFormat::NTriples => serialize_ntriples(&g),
    }
}

/// Parses an RDF document in any supported format into an [`RdfGraph`].
pub fn parse_rdf_graph(
    input: &str,
    format: RdfFormat,
    limits: &RdfLimits,
) -> dataroom_core::Result<RdfGraph> {
    match format {
        RdfFormat::Turtle | RdfFormat::NTriples => parse_turtle_checked(input, limits),
        RdfFormat::JsonLd => parse_jsonld(input, limits),
        RdfFormat::RdfXml => parse_rdfxml(input, limits),
    }
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

        let title = extract_lang_map(graph, &ds_subj, "http://purl.org/dc/terms/title", default_lang);
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
                    .and_then(|v| v.string_value().get(..10).and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()));
                let end = graph
                    .first_object(&p_subj, "http://www.w3.org/ns/dcat#endDate")
                    .and_then(|v| v.string_value().get(..10).and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()));
                TemporalPeriod { start, end }
            })
            .filter(|tp| tp.start.is_some() || tp.end.is_some());

        let spatial_resolution_in_meters = graph
            .first_object(&ds_subj, "http://www.w3.org/ns/dcat#spatialResolutionInMeters")
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
        let mut derived_licence_id = None;
        let mut derived_licence_url = None;

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
            if let Some(ref dl) = dist_licence {
                if let Some(lic_entry) = find_licence(dl) {
                    derived_licence_id = Some(lic_entry.id);
                    derived_licence_url = Some(lic_entry.url);
                } else {
                    derived_licence_url = Some(dl.clone());
                }
            }

            // Terms of use (`pu:specifikace` in CZ or `leg:termsOfUse` in SK)
            let terms_of_use = graph
                .first_object(
                    &dist_subj,
                    "https://data.gov.cz/slovník/podmínky-užití/specifikace",
                )
                .or_else(|| {
                    graph.first_object(
                        &dist_subj,
                        "https://data.gov.sk/def/ontology/legislation/termsOfUse",
                    )
                })
                .and_then(|t| t.as_subject())
                .map(|tou_subj| {
                    let authors_work = graph
                        .first_object(
                            &tou_subj,
                            "https://data.gov.cz/slovník/podmínky-užití/autorské-dílo",
                        )
                        .or_else(|| {
                            graph.first_object(
                                &tou_subj,
                                "https://data.gov.sk/def/ontology/legislation/authorsWorkType",
                            )
                        })
                        .map(|t| t.string_value().to_owned());
                    if let Some(ref aw) = authors_work {
                        if let Some(lic_entry) = find_licence(aw) {
                            derived_licence_id.get_or_insert(lic_entry.id);
                            derived_licence_url.get_or_insert(lic_entry.url);
                        }
                    }
                    let author_name = {
                        let mut m = extract_lang_map(
                            graph,
                            &tou_subj,
                            "https://data.gov.cz/slovník/podmínky-užití/autor",
                            default_lang,
                        );
                        m.extend(extract_lang_map(
                            graph,
                            &tou_subj,
                            "https://data.gov.sk/def/ontology/legislation/authorName",
                            default_lang,
                        ));
                        m
                    };
                    let database_as_work = graph
                        .first_object(
                            &tou_subj,
                            "https://data.gov.cz/slovník/podmínky-užití/databáze-jako-autorské-dílo",
                        )
                        .or_else(|| {
                            graph.first_object(
                                &tou_subj,
                                "https://data.gov.sk/def/ontology/legislation/originalDatabaseType",
                            )
                        })
                        .map(|t| t.string_value().to_owned());
                    let database_author_name = {
                        let mut m = extract_lang_map(
                            graph,
                            &tou_subj,
                            "https://data.gov.cz/slovník/podmínky-užití/autor-databáze",
                            default_lang,
                        );
                        m.extend(extract_lang_map(
                            graph,
                            &tou_subj,
                            "https://data.gov.sk/def/ontology/legislation/originalDatabaseAuthorName",
                            default_lang,
                        ));
                        m
                    };
                    let sui_generis_database = graph
                        .first_object(
                            &tou_subj,
                            "https://data.gov.cz/slovník/podmínky-užití/databáze-chráněná-zvláštními-právy",
                        )
                        .or_else(|| {
                            graph.first_object(
                                &tou_subj,
                                "https://data.gov.sk/def/ontology/legislation/databaseProtectedBySpecialRightsType",
                            )
                        })
                        .map(|t| t.string_value().to_owned());
                    let personal_data = graph
                        .first_object(
                            &tou_subj,
                            "https://data.gov.cz/slovník/podmínky-užití/osobní-údaje",
                        )
                        .or_else(|| {
                            graph.first_object(
                                &tou_subj,
                                "https://data.gov.sk/def/ontology/legislation/personalDataContainmentType",
                            )
                        })
                        .map(|t| t.string_value().to_owned());
                    TermsOfUse {
                        authors_work,
                        author_name,
                        database_as_work,
                        database_author_name,
                        sui_generis_database,
                        personal_data,
                    }
                });

            let dist_conforms: Vec<String> = graph
                .objects_of(&dist_subj, "http://purl.org/dc/terms/conformsTo")
                .into_iter()
                .map(|t| t.string_value().to_owned())
                .collect();
            let dist_laws: Vec<String> = graph
                .objects_of(&dist_subj, "http://data.europa.eu/r5r/applicableLegislation")
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
                licence: dist_licence,
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

        let now = Utc::now();
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
            let lang = lit
                .language
                .unwrap_or_else(|| default_lang.to_owned());
            map.insert(lang, lit.value);
        }
    }
    map
}

fn parse_any_datetime(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Some(naive.and_utc());
    }
    if let Some(d_str) = s.get(..10) {
        if let Ok(d) = NaiveDate::parse_from_str(d_str, "%Y-%m-%d") {
            return d.and_hms_opt(0, 0, 0).map(|dt| dt.and_utc());
        }
    }
    None
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
    use dataroom_core::ThemeTokens;

    fn sample_catalogue(org_id: OrgId, catalogue_id: CatalogueId) -> Catalogue {
        Catalogue {
            org_id,
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
            modified: Utc::now(),
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

        for profile in [DcatProfileKind::Eu301, DcatProfileKind::Cz, DcatProfileKind::Sk20] {
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
        let sk_emitted =
            serialize_dataset(ds_sk, &cat, None, RdfFormat::Turtle, DcatProfileKind::Sk20);
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
}
