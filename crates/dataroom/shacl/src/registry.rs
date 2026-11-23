#![forbid(unsafe_code)]
//! Profile registry (`C17`): the installed profiles of `crates/dataroom/profiles` and each
//! catalogue's uploaded versions, their stacked shapes, form layout extraction, and dry-run
//! validation reports.

use crate::validator::{parse_shapes_graph, validate_graph_against_shapes, PathExpr, ShapesGraph};
use dataroom_core::ops::DatasetValidator;
use dataroom_core::{
    Catalogue, CatalogueId, DataroomError, Dataset, FormFieldLayout, OrgId, ProfileDryRunReport,
    ProfilePin, ProfileRecord, Publisher, Result, ShaclValidationReport,
};
use dataroom_rdf::{catalogue_to_graph, dataset_to_graph, DcatProfile, RdfLimits};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::hash::BuildHasher;
use std::sync::{Arc, PoisonError, RwLock};

/// Derives [`FormFieldLayout`] entries for the dataset editor from a [`ShapesGraph`] (C17).
pub fn derive_form_layout(shapes: &ShapesGraph, target_class: &str) -> Vec<FormFieldLayout> {
    let mut by_path: BTreeMap<String, FormFieldLayout> = BTreeMap::new();

    for ns_item in shapes.node_shapes.values() {
        if !ns_item.target_classes.iter().any(|c| c == target_class) {
            continue;
        }
        for ps in &ns_item.properties {
            let PathExpr::Predicate(ref pred_iri) = ps.path else {
                continue;
            };
            let entry = by_path
                .entry(pred_iri.clone())
                .or_insert_with(|| FormFieldLayout {
                    path: pred_iri.clone(),
                    field_name: predicate_to_field_name(pred_iri),
                    name: BTreeMap::new(),
                    description: BTreeMap::new(),
                    group: ps.group.clone(),
                    order: ps.order,
                    required: false,
                    max_count: None,
                    node_kind: None,
                    datatype: None,
                    class_iri: None,
                    allowed_values: Vec::new(),
                });

            for (lang, text) in &ps.name {
                entry
                    .name
                    .entry(lang.clone())
                    .or_insert_with(|| text.clone());
            }
            for (lang, text) in &ps.description {
                entry
                    .description
                    .entry(lang.clone())
                    .or_insert_with(|| text.clone());
            }
            if ps.group.is_some() {
                entry.group.clone_from(&ps.group);
            }
            if ps.order < entry.order {
                entry.order = ps.order;
            }
            if ps.constraints.min_count.is_some_and(|m| m >= 1) {
                entry.required = true;
            }
            if let Some(mc) = ps.constraints.max_count {
                entry.max_count = Some(mc);
            }
            if let Some(ref nk) = ps.constraints.node_kind {
                entry.node_kind = Some(nk.clone());
            }
            if let Some(dt) = ps.constraints.datatypes.first() {
                entry.datatype = Some(dt.clone());
            }
            if let Some(cls) = ps.constraints.classes.first() {
                entry.class_iri = Some(cls.clone());
            }
            if let Some(ref vals) = ps.constraints.in_values {
                entry.allowed_values = vals.iter().map(|t| t.string_value().to_owned()).collect();
            }
        }
    }

    let mut fields: Vec<FormFieldLayout> = by_path.into_values().collect();
    fields.sort_by(|a, b| {
        a.order
            .cmp(&b.order)
            .then_with(|| a.field_name.cmp(&b.field_name))
    });
    fields
}

/// The field a person fills in for a shape's path.
pub fn predicate_to_field_name(pred_iri: &str) -> String {
    match pred_iri {
        "http://purl.org/dc/terms/title" => "title".into(),
        "http://purl.org/dc/terms/description" => "notes".into(),
        "http://purl.org/dc/terms/publisher" => "publisher_id".into(),
        "http://www.w3.org/ns/dcat#theme" => "themes".into(),
        "http://www.w3.org/ns/dcat#keyword" => "keywords".into(),
        "http://purl.org/dc/terms/accrualPeriodicity" => "frequency".into(),
        "http://purl.org/dc/terms/spatial" => "spatial".into(),
        "http://purl.org/dc/terms/temporal" => "temporal".into(),
        "http://www.w3.org/ns/dcat#contactPoint" => "contact_point".into(),
        "http://www.w3.org/ns/dcat#distribution" => "distributions".into(),
        "http://data.europa.eu/r5r/applicableLegislation" => "applicable_legislation".into(),
        "http://purl.org/dc/terms/conformsTo" => "conforms_to".into(),
        "http://www.w3.org/ns/dcat#landingPage" => "landing_page".into(),
        other => national_field(other)
            .unwrap_or_else(|| other.rsplit(['/', '#']).next().unwrap_or(other))
            .to_owned(),
    }
}

/// The field behind an installed profile's terms predicate.
fn national_field(predicate: &str) -> Option<&'static str> {
    DcatProfile::installed().find_map(|profile| {
        let terms = profile.render.terms.as_ref()?;
        if predicate == terms.personal_data {
            return Some("personal_data");
        }
        [
            &terms.link,
            &terms.authors_work,
            &terms.database_as_work,
            &terms.sui_generis_database,
        ]
        .into_iter()
        .any(|member| member == predicate)
        .then_some("license_id")
    })
}

/// The installed profiles as registry records (C17), each from its manifest.
pub fn built_in_profiles() -> Vec<ProfileRecord> {
    DcatProfile::installed()
        .map(|profile| {
            let shapes_turtle = profile.shapes_turtle();
            let parsed =
                parse_shapes_graph(&shapes_turtle, &RdfLimits::default()).unwrap_or_default();
            ProfileRecord {
                id: profile.id.clone(),
                version: profile.version.clone(),
                uri: profile.uri.clone(),
                title: profile.title.clone(),
                base_profile: profile.bases.first().cloned(),
                form_fields: derive_form_layout(&parsed, "http://www.w3.org/ns/dcat#Dataset"),
                shapes_turtle,
                jsonld_context: profile
                    .render
                    .json_ld
                    .as_ref()
                    .map(|keys| keys.catalogue.context.clone()),
                required_languages: profile.languages.clone(),
                built_in: true,
            }
        })
        .collect()
}

/// The catalogue an uploaded profile belongs to; built-in profiles carry `None` and serve every
/// catalogue.
pub type ProfileScope = (OrgId, CatalogueId);

type ProfileKey = (Option<ProfileScope>, String, String);
type ProfileMap = BTreeMap<ProfileKey, (ProfileRecord, ShapesGraph)>;

/// A dry run's report that did not turn into JSON.
fn dry_run_error(e: serde_json::Error) -> DataroomError {
    DataroomError::Storage(format!("dry run: {e}"))
}

fn scope_of(catalogue: &Catalogue) -> ProfileScope {
    (catalogue.org_id, catalogue.id)
}

/// The record an unpinned reference to `id` means in `scope`: the built-in profile when one
/// exists, else the latest version `scope` uploaded. Another catalogue's upload never answers.
fn unpinned<'a>(
    map: &'a ProfileMap,
    scope: ProfileScope,
    id: &str,
) -> Option<&'a (ProfileRecord, ShapesGraph)> {
    let latest = |owner: Option<ProfileScope>| {
        map.iter()
            .filter(|((o, pid, _), _)| *o == owner && pid == id)
            .map(|(_, v)| v)
            .next_back()
    };
    latest(None).or_else(|| latest(Some(scope)))
}

/// The record `(id, version)` names in `scope` (built-in or this catalogue's upload), falling
/// back to the unpinned one when that version is unknown.
fn resolve<'a>(
    map: &'a ProfileMap,
    scope: ProfileScope,
    id: &str,
    version: Option<&str>,
) -> Option<&'a (ProfileRecord, ShapesGraph)> {
    version
        .and_then(|v| {
            map.get(&(None, id.to_owned(), v.to_owned()))
                .or_else(|| map.get(&(Some(scope), id.to_owned(), v.to_owned())))
        })
        .or_else(|| unpinned(map, scope, id))
}

/// A profile with the shapes of every base under it, merged once.
#[derive(Debug)]
pub struct Stack {
    pub record: ProfileRecord,
    pub shapes: ShapesGraph,
    /// The installed profile that renders it: itself, else the nearest installed base, else EU.
    pub renderer: DcatProfile,
    /// A hash of every shapes text stacked, keyed per process: equal keys, equal verdicts.
    pub key: u64,
}

/// Thread-safe registry of built-in and uploaded DCAT-AP profiles (C17).
pub struct ProfileRegistry {
    profiles: RwLock<ProfileMap>,
    /// Merged stacks by the record they end in; emptied whenever a profile is added.
    stacks: RwLock<BTreeMap<ProfileKey, Arc<Stack>>>,
    hasher: std::hash::RandomState,
}

impl Default for ProfileRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ProfileRegistry {
    pub fn new() -> Self {
        let mut map = BTreeMap::new();
        for p in built_in_profiles() {
            if let Ok(sg) = parse_shapes_graph(&p.shapes_turtle, &RdfLimits::default()) {
                map.insert((None, p.id.clone(), p.version.clone()), (p, sg));
            }
        }
        Self {
            profiles: RwLock::new(map),
            stacks: RwLock::default(),
            hasher: std::hash::RandomState::new(),
        }
    }

    /// A profile was added: a stack may end elsewhere now.
    fn forget_stacks(&self) {
        self.stacks
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    /// Lists the built-in profiles and `scope`'s own uploads, every version.
    pub fn list(&self, scope: ProfileScope) -> Vec<ProfileRecord> {
        self.profiles
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|((o, _, _), _)| o.is_none_or(|o| o == scope))
            .map(|(_, (p, _))| p.clone())
            .collect()
    }

    /// Resolves a profile by `id` and optional `version` in `scope` (choosing the built-in or
    /// the latest version when `None`).
    pub fn get(
        &self,
        scope: ProfileScope,
        id: &str,
        version: Option<&str>,
    ) -> Option<ProfileRecord> {
        let guard = self.profiles.read().unwrap_or_else(PoisonError::into_inner);
        resolve(&guard, scope, id, version).map(|(rec, _)| rec.clone())
    }

    /// Loads `scope`'s stored uploads the registry does not hold yet (after a restart, or on
    /// another replica), so a pinned uploaded profile keeps validating. A stored record whose
    /// shapes no longer parse stays out and its pin answers "not found".
    pub fn load_stored(&self, scope: ProfileScope, stored: &[ProfileRecord]) {
        let mut guard = self
            .profiles
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let before = guard.len();
        for rec in stored.iter().filter(|r| !r.built_in) {
            let key = (Some(scope), rec.id.clone(), rec.version.clone());
            if guard.contains_key(&key) {
                continue;
            }
            if let Ok(sg) = parse_shapes_graph(&rec.shapes_turtle, &RdfLimits::uploaded_shapes()) {
                guard.insert(key, (rec.clone(), sg));
            }
        }
        if guard.len() != before {
            self.forget_stacks();
        }
    }

    /// The profile `(id, version)` stacked on all its bases (C17), shared and merged once.
    pub fn stack(
        &self,
        scope: ProfileScope,
        id: &str,
        version: Option<&str>,
    ) -> Result<Arc<Stack>> {
        let guard = self.profiles.read().unwrap_or_else(PoisonError::into_inner);
        let (target, _) = resolve(&guard, scope, id, version)
            .ok_or_else(|| DataroomError::NotFound(format!("profile '{id}' not found")))?;
        let key = (
            (!target.built_in).then_some(scope),
            target.id.clone(),
            target.version.clone(),
        );
        if let Some(stack) = self
            .stacks
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
        {
            return Ok(stack.clone());
        }

        // Breadth first, so the first installed profile met is the nearest; each profile once.
        let mut chain = vec![resolve(&guard, scope, id, version)
            .ok_or_else(|| DataroomError::NotFound(format!("profile '{id}' not found")))?];
        let mut at = 0;
        while let Some((record, _)) = chain.get(at).copied() {
            at += 1;
            let bases: Vec<String> =
                match DcatProfile::by_id(&record.id).filter(|_| record.built_in) {
                    Some(installed) => installed.bases.clone(),
                    None => record.base_profile.iter().cloned().collect(),
                };
            for base in bases {
                let found = unpinned(&guard, scope, &base).ok_or_else(|| {
                    DataroomError::NotFound(format!(
                        "profile '{base}', a base of '{}', not found",
                        record.id
                    ))
                })?;
                if !chain.iter().any(|(held, _)| held.id == found.0.id) {
                    chain.push(found);
                }
            }
        }

        let renderer = chain
            .iter()
            .find(|(record, _)| record.built_in)
            .and_then(|(record, _)| DcatProfile::by_id(&record.id))
            .unwrap_or_else(DcatProfile::eu);
        let mut shapes = ShapesGraph::default();
        let mut texts = Vec::new();
        for (record, own) in chain.iter().rev() {
            shapes.merge(own);
            texts.push(record.shapes_turtle.as_str());
        }
        let stack = Arc::new(Stack {
            record: target.clone(),
            shapes,
            renderer,
            key: self.hasher.hash_one(&texts),
        });
        self.stacks
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, stack.clone());
        Ok(stack)
    }

    /// Uploads and registers a new profile or profile version (enforcing `RdfLimits::uploaded_shapes()`,
    /// refusing `owl:imports`, deriving form layout) and returns a [`ProfileDryRunReport`] against
    /// `existing_datasets` (C17).
    pub fn upload_and_dry_run(
        &self,
        mut record: ProfileRecord,
        catalogue: &Catalogue,
        existing_datasets: &[Dataset],
        publishers: &BTreeMap<String, Publisher>,
    ) -> Result<(ProfileRecord, ProfileDryRunReport)> {
        self.check_profile(&record)?;
        let sg = parse_shapes_graph(&record.shapes_turtle, &RdfLimits::uploaded_shapes())?;
        record.built_in = false;
        let scope = scope_of(catalogue);
        let key = (Some(scope), record.id.clone(), record.version.clone());

        // Register in memory, under the uploading catalogue only, so stacked_shapes resolves it.
        let replaced = {
            let mut guard = self
                .profiles
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            guard.insert(key.clone(), (record.clone(), sg))
        };
        self.forget_stacks();

        let checked = self
            .stack(scope, &record.id, Some(&record.version))
            .and_then(|stack| {
                record.form_fields =
                    derive_form_layout(&stack.shapes, "http://www.w3.org/ns/dcat#Dataset");
                {
                    let mut guard = self
                        .profiles
                        .write()
                        .unwrap_or_else(PoisonError::into_inner);
                    if let Some((stored_rec, _)) = guard.get_mut(&key) {
                        stored_rec.form_fields.clone_from(&record.form_fields);
                    }
                }
                self.dry_run(
                    &record.id,
                    &record.version,
                    catalogue,
                    existing_datasets,
                    publishers,
                )
            });
        match checked {
            Ok(dry_run) => Ok((record, dry_run)),
            Err(e) => {
                // A refused upload is stored nowhere, so it is not resolvable either.
                {
                    let mut guard = self
                        .profiles
                        .write()
                        .unwrap_or_else(PoisonError::into_inner);
                    match replaced {
                        Some(before) => guard.insert(key, before),
                        None => guard.remove(&key),
                    };
                }
                self.forget_stacks();
                Err(e)
            }
        }
    }

    /// Evaluates all `existing_datasets` against `(profile_id, version)` without modifying them,
    /// returning which datasets conform and which turn red (`newly_failing_datasets`, C17).
    pub fn dry_run(
        &self,
        profile_id: &str,
        version: &str,
        catalogue: &Catalogue,
        existing_datasets: &[Dataset],
        publishers: &BTreeMap<String, Publisher>,
    ) -> Result<ProfileDryRunReport> {
        let stack = self.stack(scope_of(catalogue), profile_id, Some(version))?;

        let mut reports = Vec::new();
        let mut conforming = 0usize;
        let mut failing = 0usize;
        let mut newly_failing = Vec::new();

        for ds in existing_datasets {
            let pub_rec = publishers.get(&ds.publisher_id);
            let g = dataset_to_graph(ds, catalogue, pub_rec, stack.renderer);
            let report = validate_graph_against_shapes(
                &g,
                &stack.shapes,
                catalogue.org_id,
                catalogue.id,
                &ds.id,
                profile_id,
                version,
            );
            if report.conforms {
                conforming += 1;
            } else {
                failing += 1;
                newly_failing.push(ds.name.clone());
            }
            reports.push(report);
        }

        Ok(ProfileDryRunReport {
            profile_id: profile_id.to_owned(),
            version: version.to_owned(),
            total_datasets: existing_datasets.len(),
            conforming_datasets: conforming,
            failing_datasets: failing,
            newly_failing_datasets: newly_failing,
            dataset_reports: reports,
        })
    }

    /// Validates an arbitrary RDF graph (e.g. a sample against a dataset's custom SHACL graph, C8).
    pub fn validate_custom_sample(
        &self,
        shapes_turtle: &str,
        sample_turtle: &str,
        org_id: OrgId,
        catalogue_id: dataroom_core::CatalogueId,
        dataset_id: &str,
    ) -> Result<ShaclValidationReport> {
        let shapes = parse_shapes_graph(shapes_turtle, &RdfLimits::uploaded_shapes())?;
        let data =
            dataroom_rdf::turtle::parse_turtle_checked(sample_turtle, &RdfLimits::default())?;
        Ok(validate_graph_against_shapes(
            &data,
            &shapes,
            org_id,
            catalogue_id,
            dataset_id,
            "custom-sample",
            "1",
        ))
    }
}

impl DatasetValidator for ProfileRegistry {
    fn load_stored_profiles(&self, catalogue: &Catalogue, stored: &[ProfileRecord]) {
        self.load_stored(scope_of(catalogue), stored);
    }

    fn check_profile(&self, profile: &ProfileRecord) -> Result<()> {
        if DcatProfile::by_id(&profile.id).is_some() {
            return Err(DataroomError::field_error(
                "profile.id",
                format!(
                    "{} ships with the release; upload under an id of your own, with it as \
                     baseProfile",
                    profile.id
                ),
            ));
        }
        parse_shapes_graph(&profile.shapes_turtle, &RdfLimits::uploaded_shapes())
            .map(|_| ())
            .map_err(|e| {
                DataroomError::field_error(
                    "profile.shapesTurtle",
                    format!("not SHACL in Turtle: {e}"),
                )
            })
    }

    fn check_profile_choice(
        &self,
        catalogue: &Catalogue,
        added: &[&str],
        managed: bool,
    ) -> Result<()> {
        for id in added {
            if managed && DcatProfile::by_id(id).is_some() {
                return Err(DataroomError::field_error(
                    "activeProfiles",
                    format!("{id} is chosen on the Portal manifest: set spec.profiles there"),
                ));
            }
            let stack = self.stack(scope_of(catalogue), id, None).map_err(|_| {
                DataroomError::field_error(
                    "activeProfiles",
                    format!("{id} is not an installed profile"),
                )
            })?;
            if stack.renderer.requires_publisher() && catalogue.declared_publisher_uri().is_none() {
                return Err(DataroomError::field_error(
                    "publisherUri",
                    format!("{id} needs the catalogue's publisher IRI; declare it first"),
                ));
            }
        }
        Ok(())
    }

    fn validate_dataset(
        &self,
        catalogue: &Catalogue,
        dataset: &Dataset,
        publisher: Option<&Publisher>,
    ) -> Result<Vec<ShaclValidationReport>> {
        let mut pins = catalogue.active_profiles.clone();
        if !pins.iter().any(|p| p.profile_id == catalogue.main_profile) {
            pins.insert(
                0,
                ProfilePin {
                    profile_id: catalogue.main_profile.clone(),
                    version: String::new(),
                },
            );
        }

        let mut reports = Vec::new();
        for pin in pins {
            let ver = (!pin.version.is_empty()).then_some(pin.version.as_str());
            let scope = scope_of(catalogue);
            let stack = match self.stack(scope, &pin.profile_id, ver) {
                Ok(stack) => stack,
                // A pin this build does not know blocks no write (T-7521 C7); its feed is closed.
                // The main profile decides a write, so that one still fails it.
                Err(error) if pin.profile_id != catalogue.main_profile => {
                    tracing::warn!(catalogue = %catalogue.id, profile = %pin.profile_id, %error, "pinned profile not installed");
                    continue;
                }
                Err(error) => return Err(error),
            };
            let data = dataset_to_graph(dataset, catalogue, publisher, stack.renderer);
            let report = validate_graph_against_shapes(
                &data,
                &stack.shapes,
                catalogue.org_id,
                catalogue.id,
                &dataset.id,
                &pin.profile_id,
                &stack.record.version,
            );
            reports.push(report);
        }
        Ok(reports)
    }

    fn check_catalogue_feed(
        &self,
        catalogue: &Catalogue,
        datasets: &[Dataset],
        publishers: &BTreeMap<String, Publisher>,
        profile_id: Option<&str>,
    ) -> Result<Value> {
        let pid = profile_id.unwrap_or(&catalogue.main_profile);
        let stack = self.stack(scope_of(catalogue), pid, None)?;
        let (rec, stacked, kind) = (&stack.record, &stack.shapes, stack.renderer);

        let mut valid_datasets = Vec::new();
        let mut hidden_from_profile = Vec::new();

        for ds in datasets {
            let pub_rec = publishers.get(&ds.publisher_id);
            let ds_graph = dataset_to_graph(ds, catalogue, pub_rec, kind);
            let report = validate_graph_against_shapes(
                &ds_graph,
                stacked,
                catalogue.org_id,
                catalogue.id,
                &ds.id,
                &rec.id,
                &rec.version,
            );
            if report.conforms {
                valid_datasets.push(ds.clone());
            } else {
                hidden_from_profile.push(json!({
                    "dataset": ds.name,
                    "violations": report.violation_count,
                    "findings": report.findings,
                }));
            }
        }

        let feed_graph = catalogue_to_graph(catalogue, &valid_datasets, publishers, kind, None);
        let feed_report = validate_graph_against_shapes(
            &feed_graph,
            stacked,
            catalogue.org_id,
            catalogue.id,
            "catalog",
            &rec.id,
            &rec.version,
        );

        Ok(json!({
            "profile": rec.id,
            "version": rec.version,
            "conforms": feed_report.conforms && hidden_from_profile.is_empty(),
            "datasetsChecked": datasets.len(),
            "datasetsValid": valid_datasets.len(),
            "hiddenDatasets": hidden_from_profile,
            "feedViolations": feed_report.violation_count,
            "findings": feed_report.findings,
        }))
    }

    fn upload_profile(
        &self,
        profile: ProfileRecord,
        catalogue: &Catalogue,
        datasets: &[Dataset],
        publishers: &BTreeMap<String, Publisher>,
    ) -> Result<(ProfileRecord, Value)> {
        let taken = self
            .list(scope_of(catalogue))
            .iter()
            .any(|p| p.id == profile.id && p.version == profile.version);
        if taken {
            return Err(DataroomError::Conflict(format!(
                "profile '{}' version '{}' already exists; upload a new version",
                profile.id, profile.version
            )));
        }
        let (saved, report) = self.upload_and_dry_run(profile, catalogue, datasets, publishers)?;
        let report = serde_json::to_value(report).map_err(dry_run_error)?;
        Ok((saved, report))
    }

    fn dry_run_profile(
        &self,
        profile_id: &str,
        version: &str,
        catalogue: &Catalogue,
        datasets: &[Dataset],
        publishers: &BTreeMap<String, Publisher>,
    ) -> Result<(Value, Vec<ShaclValidationReport>)> {
        let report = self.dry_run(profile_id, version, catalogue, datasets, publishers)?;
        let value = serde_json::to_value(&report).map_err(dry_run_error)?;
        Ok((value, report.dataset_reports))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dataroom_core::{
        AgentDetails, CatalogueId, Distribution, RecordState, ThemeTokens, Visibility,
    };
    use dataroom_rdf::{parse_rdf_graph, RdfFormat};
    use std::collections::BTreeSet;
    use uuid::Uuid;

    fn sk_shapes() -> &'static str {
        let sk = DcatProfile::by_id("dcat-ap-sk-2.0").expect("installed");
        sk.manifest().file("shapes.ttl")
    }

    impl ProfileRegistry {
        fn stacked_shapes(
            &self,
            scope: ProfileScope,
            id: &str,
            version: Option<&str>,
        ) -> Result<(ProfileRecord, ShapesGraph)> {
            self.stack(scope, id, version)
                .map(|stack| (stack.record.clone(), stack.shapes.clone()))
        }
    }

    fn test_ids() -> (OrgId, CatalogueId) {
        (
            OrgId::from_uuid(Uuid::from_u128(1)),
            CatalogueId::from_uuid(Uuid::from_u128(10)),
        )
    }

    #[test]
    fn shacl_validates_cz_and_sk_example_files_and_fails_mutated_with_expected_path() {
        let (org_id, catalogue_id) = test_ids();
        let registry = ProfileRegistry::new();

        // 1. Validate specs/dcat-ap-cz/priklady-katalog examples against DCAT-AP-CZ shapes
        let (_, cz_shapes) = registry
            .stacked_shapes(test_ids(), "dcat-ap-cz", None)
            .expect("cz shapes");
        let (_, cz_lkod_shapes) = registry
            .stacked_shapes(test_ids(), "dcat-ap-cz-lkod", None)
            .expect("cz lkod shapes");

        for (name, ttl, shapes) in [
            (
                "datova-sada.ttl",
                include_str!("../../specs/dcat-ap-cz/priklady-katalog/datova-sada.ttl"),
                &cz_shapes,
            ),
            (
                "datova-sada-v-serii.ttl",
                include_str!("../../specs/dcat-ap-cz/priklady-katalog/datova-sada-v-serii.ttl"),
                &cz_shapes,
            ),
            (
                "katalog.ttl",
                include_str!("../../specs/dcat-ap-cz/priklady-katalog/katalog.ttl"),
                &cz_lkod_shapes,
            ),
        ] {
            let g = parse_rdf_graph(ttl, RdfFormat::Turtle, &RdfLimits::default())
                .expect("parse example ttl");
            let report = validate_graph_against_shapes(
                &g,
                shapes,
                org_id,
                catalogue_id,
                name,
                "dcat-ap-cz",
                "2026-09-23",
            );
            assert!(
                report.conforms,
                "{name} should pass SHACL, got findings: {:?}",
                report.findings
            );
        }

        // Validate specs/dcat-ap-cz/priklady-katalog/katalog.jsonld
        let cz_cat_jsonld = include_str!("../../specs/dcat-ap-cz/priklady-katalog/katalog.jsonld");
        let g_cz_jsonld = parse_rdf_graph(cz_cat_jsonld, RdfFormat::JsonLd, &RdfLimits::default())
            .expect("parse cz katalog.jsonld");
        let r_cz_jsonld = validate_graph_against_shapes(
            &g_cz_jsonld,
            &cz_lkod_shapes,
            org_id,
            catalogue_id,
            "katalog.jsonld",
            "dcat-ap-cz-lkod",
            "2026-09-23",
        );
        assert!(
            r_cz_jsonld.conforms,
            "katalog.jsonld should pass SHACL, got: {:?}",
            r_cz_jsonld.findings
        );

        // 2. Validate specs/dcat-ap-sk/príklady examples against DCAT-AP-SK 2.0 shapes
        let (_, sk_shapes) = registry
            .stacked_shapes(test_ids(), "dcat-ap-sk-2.0", None)
            .expect("sk shapes");

        for (name, content, fmt) in [
            (
                "dataset.ttl",
                include_str!("../../specs/dcat-ap-sk/príklady/dataset.ttl"),
                RdfFormat::Turtle,
            ),
            (
                "dataset.jsonld",
                include_str!("../../specs/dcat-ap-sk/príklady/dataset.jsonld"),
                RdfFormat::JsonLd,
            ),
            (
                "katalóg.ttl",
                include_str!("../../specs/dcat-ap-sk/príklady/katalóg.ttl"),
                RdfFormat::Turtle,
            ),
            (
                "katalóg.jsonld",
                include_str!("../../specs/dcat-ap-sk/príklady/katalóg.jsonld"),
                RdfFormat::JsonLd,
            ),
        ] {
            let g = parse_rdf_graph(content, fmt, &RdfLimits::default()).expect("parse sk example");
            let report = validate_graph_against_shapes(
                &g,
                &sk_shapes,
                org_id,
                catalogue_id,
                name,
                "dcat-ap-sk-2.0",
                "2.0.1",
            );
            assert!(
                report.conforms,
                "SK {name} should pass SHACL, got findings: {:?}",
                report.findings
            );
        }

        // 3. Mutate a valid dataset (remove dct:title and add a non-decimal spatialResolutionInMeters)
        // and verify it fails with the exact expected result_path!
        let mutated_ttl = r#"
            @prefix dcat: <http://www.w3.org/ns/dcat#> .
            @prefix dct: <http://purl.org/dc/terms/> .
            <https://data.gov.cz/lkod/mdcr/datové-sady/broken> a dcat:Dataset ;
                dct:description "Chybí název a rozlišení není číslo"@cs ;
                dct:publisher <https://rpp-opendata.egon.gov.cz/odrpp/zdroj/orgán-veřejné-moci/66003008> ;
                dcat:spatialResolutionInMeters "not-a-decimal" .
        "#;
        let g_mutated =
            parse_rdf_graph(mutated_ttl, RdfFormat::Turtle, &RdfLimits::default()).unwrap();
        let bad_report = validate_graph_against_shapes(
            &g_mutated,
            &cz_shapes,
            org_id,
            catalogue_id,
            "broken",
            "dcat-ap-cz",
            "2026-09-23",
        );
        assert!(!bad_report.conforms);
        let paths: Vec<&str> = bad_report
            .findings
            .iter()
            .filter_map(|f| f.result_path.as_deref())
            .collect();
        assert!(
            paths.contains(&"http://purl.org/dc/terms/title"),
            "expected missing title path in {paths:?}"
        );
        assert!(
            paths.contains(&"http://www.w3.org/ns/dcat#spatialResolutionInMeters"),
            "expected spatialResolutionInMeters path in {paths:?}"
        );
    }

    #[test]
    fn uploading_new_profile_version_produces_dry_run_report_and_forbids_owl_imports() {
        let (org_id, catalogue_id) = test_ids();
        let registry = ProfileRegistry::new();
        let cat = Catalogue {
            org_id,
            id: catalogue_id,
            name: "city".into(),
            host: "data.city.example".into(),
            title: BTreeMap::from([("en".into(), "City Catalogue".into())]),
            description: BTreeMap::from([("en".into(), "Desc".into())]),
            homepage: None,
            default_language: "en".into(),
            languages: vec!["en".into()],
            main_profile: "dcat-ap-3.0.1".into(),
            active_profiles: Vec::new(),
            default_licence: Some("cc-by".into()),
            publisher_uri: Some("https://city.example/org".into()),
            publisher_name: None,
            contact_point: None,
            theme: ThemeTokens::default(),
            branding: None,
            hidden_datasets: Default::default(),
            allow_cross_org_harvest: true,
            allowed_outbound_hosts: Vec::new(),
            applicable_legislation: Vec::new(),
            modified: jc_time::TimestampTz::now(),
            deleted: None,
            public_base: None,
            portal_base: None,
            left_hosts: Vec::new(),
        };

        let ds_with_landing = Dataset {
            org_id,
            catalogue_id,
            id: "pkg-a".into(),
            name: "with-landing".into(),
            uri: None,
            dataset_type: "dataset".into(),
            title: BTreeMap::from([("en".into(), "Dataset A".into())]),
            description: BTreeMap::from([("en".into(), "Description A".into())]),
            publisher_id: "city".into(),
            publisher: Some(AgentDetails {
                uri: Some("https://city.example/org".into()),
                name: BTreeMap::from([("en".into(), "City".into())]),
                ..AgentDetails::default()
            }),
            creator: None,
            contact_point: None,
            visibility: Visibility::Public,
            state: RecordState::Active,
            licence_id: Some("cc-by".into()),
            licence_url: None,
            themes: vec!["TRAN".into()],
            eurovoc_themes: Vec::new(),
            keywords: BTreeMap::new(),
            tags: vec!["transport".into()],
            spatial: Vec::new(),
            spatial_resolution_in_meters: None,
            temporal: None,
            temporal_resolution: None,
            frequency: None,
            landing_page: Some("https://city.example/datasets/a".into()),
            documentation: Vec::new(),
            conforms_to: Vec::new(),
            applicable_legislation: Vec::new(),
            in_series: None,
            relations: Vec::new(),
            sources: Vec::new(),
            languages: Vec::new(),
            version: None,
            version_notes: BTreeMap::new(),
            access_rights: None,
            provenance: BTreeMap::new(),
            dcat_type: None,
            identifier: None,
            alternate_identifiers: Vec::new(),
            endpoint_url: None,
            generated_by: None,
            harvest_source_id: None,
            harvest_source_item_id: None,
            harvest_hash: None,
            custom_shacl_shapes: None,
            distributions: vec![Distribution {
                id: "res-1".into(),
                dataset_id: "pkg-a".into(),
                uri: None,
                title: BTreeMap::from([("en".into(), "CSV".into())]),
                description: BTreeMap::new(),
                access_url: "https://city.example/a.csv".into(),
                download_url: None,
                format: Some("CSV".into()),
                media_type: None,
                compress_format: None,
                package_format: None,
                byte_size: None,
                hash: None,
                hash_algorithm: None,
                licence: None,
                rights: BTreeMap::new(),
                status: None,
                availability: None,
                resource_type: None,
                conforms_to: Vec::new(),
                documentation: Vec::new(),
                languages: Vec::new(),
                applicable_legislation: Vec::new(),
                temporal_resolution: None,
                spatial_resolution_in_meters: None,
                terms_of_use: None,
                access_services: Vec::new(),
                issued: None,
                modified: None,
                extras: BTreeMap::new(),
            }],
            extras: BTreeMap::new(),
            restricted_grants: Vec::new(),
            issued: jc_time::TimestampTz::now(),
            modified: jc_time::TimestampTz::now(),
        };

        let mut ds_without_landing = ds_with_landing.clone();
        ds_without_landing.id = "pkg-b".into();
        ds_without_landing.name = "without-landing".into();
        ds_without_landing.landing_page = None;

        // Upload a new version of a profile that requires dcat:landingPage (minCount 1)
        let custom_shapes = r#"
            @prefix dcat: <http://www.w3.org/ns/dcat#> .
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            <https://city.example/shapes#LandingPageRequired> a sh:NodeShape ;
                sh:targetClass dcat:Dataset ;
                sh:property [
                    sh:path dcat:landingPage ;
                    sh:name "Landing page"@en ;
                    sh:order 15 ;
                    sh:minCount 1 ;
                    sh:nodeKind sh:BlankNodeOrIRI
                ] .
        "#;
        let new_profile = ProfileRecord {
            id: "dcat-ap-city".into(),
            version: "2.0.0".into(),
            uri: "https://city.example/profile/2.0.0".into(),
            title: BTreeMap::from([("en".into(), "City Profile 2.0".into())]),
            base_profile: Some("dcat-ap-3.0.1".into()),
            shapes_turtle: custom_shapes.into(),
            jsonld_context: None,
            required_languages: vec!["en".into()],
            built_in: false,
            form_fields: Vec::new(),
        };

        let (saved_prof, dry_run) = registry
            .upload_and_dry_run(
                new_profile,
                &cat,
                &[ds_with_landing, ds_without_landing],
                &BTreeMap::new(),
            )
            .expect("upload and dry run");

        assert_eq!(dry_run.total_datasets, 2);
        assert_eq!(dry_run.conforming_datasets, 1);
        assert_eq!(dry_run.failing_datasets, 1);
        assert_eq!(dry_run.newly_failing_datasets, vec!["without-landing"]);
        assert!(saved_prof
            .form_fields
            .iter()
            .any(|f| f.field_name == "landing_page" && f.required));

        // Verify owl:imports is refused on uploaded shapes (C17)
        let bad_owl = ProfileRecord {
            id: "bad-owl".into(),
            version: "1.0.0".into(),
            uri: "https://evil.example/profile".into(),
            title: BTreeMap::new(),
            base_profile: None,
            shapes_turtle: "<https://evil.example/s> <http://www.w3.org/2002/07/owl#imports> <https://evil.example/remote.ttl> .".into(),
            jsonld_context: None,
            required_languages: Vec::new(),
            built_in: false,
            form_fields: Vec::new(),
        };
        assert!(registry
            .upload_and_dry_run(bad_owl, &cat, &[], &BTreeMap::new())
            .is_err());
    }

    #[test]
    fn a4_sk_shapes_have_source_comments_and_fail_each_mandatory_property_mutation() {
        // Verify header and rdfs:comment source line annotations on every shape and constraint
        assert!(sk_shapes().starts_with(
            "# Derived by Argon (2026-11-07) from DCAT-AP-SK 2.0 (index.bs, schémy/*.json): the SK profile publishes no SHACL."
        ));
        assert!(sk_shapes().contains("rdfs:comment \"Source: specs/dcat-ap-sk/"));

        let (org_id, catalogue_id) = test_ids();
        let sk_shapes =
            parse_shapes_graph(sk_shapes(), &RdfLimits::default()).expect("parse sk 2.0");

        // Every mandatory property in DCAT-AP-SK 2.0 across Catalog, Dataset, Distribution, TermsOfUse, DataService:
        let sk_ds_ttl = include_str!("../../specs/dcat-ap-sk/príklady/dataset.ttl");
        let base_ds_graph =
            parse_rdf_graph(sk_ds_ttl, RdfFormat::Turtle, &RdfLimits::default()).unwrap();
        let sk_cat_ttl = include_str!("../../specs/dcat-ap-sk/príklady/katalóg.ttl");
        let base_cat_graph =
            parse_rdf_graph(sk_cat_ttl, RdfFormat::Turtle, &RdfLimits::default()).unwrap();

        // 1. Mutate Catalog mandatory properties: dc:title, dc:description, dc:publisher, dcat:dataset
        for mandatory_pred in [
            "http://purl.org/dc/terms/title",
            "http://purl.org/dc/terms/description",
            "http://purl.org/dc/terms/publisher",
            "http://www.w3.org/ns/dcat#dataset",
        ] {
            let mut mutated = base_cat_graph.clone();
            mutated.triples.retain(|t| t.predicate != mandatory_pred);
            let rep = validate_graph_against_shapes(
                &mutated,
                &sk_shapes,
                org_id,
                catalogue_id,
                "cat-mut",
                "dcat-ap-sk-2.0",
                "2.0.1",
            );
            assert!(
                !rep.conforms,
                "removing mandatory Catalog property {mandatory_pred} must fail SHACL"
            );
            assert!(
                rep.findings
                    .iter()
                    .any(|f| f.result_path.as_deref() == Some(mandatory_pred)),
                "expected finding with result_path={mandatory_pred}, got {:?}",
                rep.findings
            );
        }

        // 2. Mutate Dataset, Distribution, and TermsOfUse mandatory properties:
        for mandatory_pred in [
            "http://purl.org/dc/terms/title",
            "http://purl.org/dc/terms/description",
            "http://purl.org/dc/terms/publisher",
            "http://www.w3.org/ns/dcat#theme",
            "http://purl.org/dc/terms/accrualPeriodicity",
            "http://www.w3.org/ns/dcat#keyword",
            "http://www.w3.org/ns/dcat#distribution",
            "http://www.w3.org/ns/dcat#accessURL",
            "https://data.gov.sk/def/ontology/legislation/termsOfUse",
            "https://data.gov.sk/def/ontology/legislation/authorsWorkType",
            "https://data.gov.sk/def/ontology/legislation/originalDatabaseType",
            "https://data.gov.sk/def/ontology/legislation/databaseProtectedBySpecialRightsType",
            "https://data.gov.sk/def/ontology/legislation/personalDataContainmentType",
        ] {
            let mut mutated = base_ds_graph.clone();
            mutated.triples.retain(|t| t.predicate != mandatory_pred);
            let rep = validate_graph_against_shapes(
                &mutated,
                &sk_shapes,
                org_id,
                catalogue_id,
                "ds-mut",
                "dcat-ap-sk-2.0",
                "2.0.1",
            );
            assert!(
                !rep.conforms,
                "removing mandatory SK property {mandatory_pred} must fail SHACL"
            );
            assert!(
                rep.findings
                    .iter()
                    .any(|f| f.result_path.as_deref() == Some(mandatory_pred)),
                "expected finding with result_path={mandatory_pred}, got {:?}",
                rep.findings
            );
        }
    }

    #[test]
    fn e3_every_shacl_constraint_component_passes_and_fails() {
        let (org_id, catalogue_id) = test_ids();
        let shapes_ttl = r#"
            @prefix ex: <https://example.org/> .
            @prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

            ex:ChildShape a sh:NodeShape ;
                sh:property [
                    sh:path ex:childCode ;
                    sh:minCount 1 ;
                    sh:hasValue "REQUIRED_CODE"
                ] .

            ex:BranchAShape a sh:NodeShape ;
                sh:property [
                    sh:path ex:branchProp ;
                    sh:minCount 1
                ] .

            ex:BranchBShape a sh:NodeShape ;
                sh:property [
                    sh:path ex:altProp ;
                    sh:minCount 1
                ] .

            ex:ForbiddenShape a sh:NodeShape ;
                sh:property [
                    sh:path ex:forbiddenProp ;
                    sh:minCount 1
                ] .

            ex:ItemShape a sh:NodeShape ;
                sh:targetClass ex:Item ;
                sh:targetNode ex:TargetedNode ;
                sh:closed true ;
                sh:ignoredProperties ( rdf:type ex:code ex:title ex:child ex:parent ex:seqStart ex:status ex:opt ) ;
                sh:property [
                    sh:path ex:code ;
                    sh:minCount 1 ;
                    sh:maxCount 1 ;
                    sh:nodeKind sh:Literal ;
                    sh:datatype xsd:string ;
                    sh:minLength 3 ;
                    sh:maxLength 10 ;
                    sh:pattern "^[A-Z0-9_]+$" ;
                    sh:severity sh:Violation ;
                    sh:message "Invalid code"@en
                ] ;
                sh:property [
                    sh:path ex:title ;
                    sh:minCount 1 ;
                    sh:datatype rdf:langString ;
                    sh:languageIn ( "sk" "en" ) ;
                    sh:uniqueLang true
                ] ;
                sh:property [
                    sh:path ex:status ;
                    sh:in ( "active" "draft" )
                ] ;
                sh:property [
                    sh:path ex:child ;
                    sh:class ex:Child ;
                    sh:node ex:ChildShape ;
                    sh:or ( ex:BranchAShape ex:BranchBShape ) ;
                    sh:and ( ex:ChildShape ex:BranchAShape ) ;
                    sh:not ex:ForbiddenShape
                ] ;
                sh:property [
                    sh:path ( ex:seqStart ex:seqEnd ) ;
                    sh:minCount 1
                ] ;
                sh:property [
                    sh:path [ sh:inversePath ex:linksTo ] ;
                    sh:minCount 1
                ] .
        "#;

        let sg = parse_shapes_graph(shapes_ttl, &RdfLimits::default()).expect("parse e3 shapes");

        let valid_data_ttl = r#"
            @prefix ex: <https://example.org/> .
            @prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .

            ex:TargetedNode a ex:Item ;
                ex:code "ABC_1" ;
                ex:title "Názov"@sk, "Title"@en ;
                ex:status "active" ;
                ex:child ex:Child1 ;
                ex:seqStart ex:Mid .

            ex:Mid ex:seqEnd "done" .
            ex:Linker ex:linksTo ex:TargetedNode .

            ex:Child1 a ex:Child ;
                ex:childCode "REQUIRED_CODE" ;
                ex:branchProp "ok" .
        "#;
        let g_valid =
            parse_rdf_graph(valid_data_ttl, RdfFormat::Turtle, &RdfLimits::default()).unwrap();
        let rep_ok = validate_graph_against_shapes(
            &g_valid,
            &sg,
            org_id,
            catalogue_id,
            "e3-ok",
            "test",
            "1",
        );
        assert!(
            rep_ok.conforms,
            "valid graph must pass all E3 constraint components: {:?}",
            rep_ok.findings
        );

        // Failing graph exercising each component:
        // - ClosedConstraintComponent (ex:undeclaredPred)
        // - MaxCountConstraintComponent / MinLength / MaxLength / Pattern / Datatype / NodeKind
        // - LanguageInConstraintComponent (@de) & UniqueLangConstraintComponent (duplicate @sk)
        // - InConstraintComponent ("archived")
        // - HasValue / Class / Node / Or / And / Not
        // - Sequence path & Inverse path minCount
        let invalid_data_ttl = r#"
            @prefix ex: <https://example.org/> .

            ex:TargetedNode a ex:Item ;
                ex:undeclaredPred "disallowed" ;
                ex:code "a", "TOO_LONG_STRING_12345" ;
                ex:title "Prvý"@sk, "Druhý"@sk, "Deutsch"@de ;
                ex:status "archived" ;
                ex:child ex:BadChild .

            ex:BadChild a ex:WrongClass ;
                ex:forbiddenProp "bad" .
        "#;
        let g_invalid =
            parse_rdf_graph(invalid_data_ttl, RdfFormat::Turtle, &RdfLimits::default()).unwrap();
        let rep_bad = validate_graph_against_shapes(
            &g_invalid,
            &sg,
            org_id,
            catalogue_id,
            "e3-bad",
            "test",
            "1",
        );
        assert!(!rep_bad.conforms);
        let components: BTreeSet<&str> = rep_bad
            .findings
            .iter()
            .map(|f| f.source_constraint_component.as_str())
            .collect();
        for expected in [
            "http://www.w3.org/ns/shacl#ClosedConstraintComponent",
            "http://www.w3.org/ns/shacl#MaxCountConstraintComponent",
            "http://www.w3.org/ns/shacl#MinLengthConstraintComponent",
            "http://www.w3.org/ns/shacl#MaxLengthConstraintComponent",
            "http://www.w3.org/ns/shacl#PatternConstraintComponent",
            "http://www.w3.org/ns/shacl#LanguageInConstraintComponent",
            "http://www.w3.org/ns/shacl#UniqueLangConstraintComponent",
            "http://www.w3.org/ns/shacl#InConstraintComponent",
            "http://www.w3.org/ns/shacl#ClassConstraintComponent",
            "http://www.w3.org/ns/shacl#NodeConstraintComponent",
            "http://www.w3.org/ns/shacl#OrConstraintComponent",
            "http://www.w3.org/ns/shacl#AndConstraintComponent",
            "http://www.w3.org/ns/shacl#NotConstraintComponent",
            "http://www.w3.org/ns/shacl#MinCountConstraintComponent",
        ] {
            assert!(
                components.contains(expected),
                "missing expected SHACL violation component {expected} in {components:?}"
            );
        }
    }

    fn small_catalogue(main_profile: &str) -> Catalogue {
        let (org_id, catalogue_id) = test_ids();
        serde_json::from_value(json!({
            "orgId": org_id, "id": catalogue_id, "name": "city", "host": "data.city.example",
            "title": {"en": "City"}, "description": {"en": "Desc"},
            "defaultLanguage": "en", "languages": ["en"],
            "mainProfile": main_profile, "activeProfiles": [],
            "modified": "2026-11-01T10:00:00Z"
        }))
        .expect("catalogue fixture")
    }

    fn small_dataset(name: &str, description: bool) -> Dataset {
        serde_json::from_value(json!({
            "orgId": test_ids().0, "catalogueId": test_ids().1, "id": format!("pkg-{name}"),
            "name": name, "title": {"en": "T"},
            "description": if description { json!({"en": "D"}) } else { json!({}) },
            "publisherId": "city", "visibility": "public", "state": "active",
            "issued": "2026-11-01T10:00:00Z", "modified": "2026-11-02T10:00:00Z"
        }))
        .expect("dataset fixture")
    }

    fn profile(id: &str, version: &str, base: Option<&str>, shapes: &str) -> ProfileRecord {
        serde_json::from_value(json!({
            "id": id, "version": version, "uri": format!("https://e.org/profile/{id}"),
            "title": {"en": id}, "baseProfile": base, "shapesTurtle": shapes, "builtIn": true
        }))
        .expect("profile fixture")
    }

    const NEEDS_KEYWORD: &str = r#"
        @prefix sh: <http://www.w3.org/ns/shacl#> .
        @prefix dcat: <http://www.w3.org/ns/dcat#> .
        <https://e.org/shape> a sh:NodeShape ; sh:targetClass dcat:Dataset ;
          sh:property [ sh:path dcat:keyword ; sh:minCount 1 ; sh:name "Keyword"@en ] .
    "#;

    #[test]
    fn an_upload_of_a_built_in_id_never_changes_what_other_catalogues_validate_against() {
        let registry = ProfileRegistry::new();
        let cat = small_catalogue("dcat-ap-3.0.1");
        let bad = small_dataset("no-description", false);
        let before = registry.validate_dataset(&cat, &bad, None).unwrap();
        assert!(
            !before[0].conforms,
            "the built-in EU profile needs dct:description"
        );

        // Another catalogue uploads an empty "new version" of the EU profile.
        let empty = profile(
            "dcat-ap-3.0.1",
            "9.9",
            None,
            "@prefix sh: <http://www.w3.org/ns/shacl#> .",
        );
        let refused = registry
            .upload_and_dry_run(
                empty.clone(),
                &small_catalogue("other"),
                &[],
                &BTreeMap::new(),
            )
            .expect_err("a built-in's id is taken");
        assert!(matches!(refused, DataroomError::Validation { .. }));
        // One stored before uploads were refused still changes nothing unasked.
        registry.load_stored(
            test_ids(),
            &[ProfileRecord {
                built_in: false,
                ..empty
            }],
        );
        let after = registry.validate_dataset(&cat, &bad, None).unwrap();
        assert_eq!(
            after[0].profile_version, before[0].profile_version,
            "unpinned means built-in"
        );
        assert!(
            !after[0].conforms,
            "an empty upload must not wave the dataset through"
        );
        assert_eq!(
            registry
                .get(test_ids(), "dcat-ap-3.0.1", None)
                .map(|p| p.built_in),
            Some(true)
        );
        // A catalogue that pins the uploaded version gets it.
        let pinned = registry
            .get(test_ids(), "dcat-ap-3.0.1", Some("9.9"))
            .unwrap();
        assert_eq!(pinned.version, "9.9");
        let mut pinning = small_catalogue("dcat-ap-3.0.1");
        pinning.active_profiles = vec![ProfilePin {
            profile_id: "dcat-ap-3.0.1".into(),
            version: "9.9".into(),
        }];
        let reports = registry.validate_dataset(&pinning, &bad, None).unwrap();
        assert_eq!(reports.len(), 1, "the main profile pin is not added twice");
        assert!(reports[0].conforms && reports[0].profile_version == "9.9");
    }

    #[test]
    fn custom_profiles_stack_on_their_base_and_report() {
        let registry = ProfileRegistry::new();
        assert!(registry.list(test_ids()).len() >= 7);
        assert!(registry.get(test_ids(), "nope", None).is_none());
        assert!(matches!(
            registry.stacked_shapes(test_ids(), "nope", None),
            Err(DataroomError::NotFound(_))
        ));
        assert!(matches!(
            registry.stacked_shapes(test_ids(), "nope", Some("1")),
            Err(DataroomError::NotFound(_))
        ));
        assert!(registry
            .dry_run("nope", "1", &small_catalogue("x"), &[], &BTreeMap::new())
            .is_err());

        // A city profile stacked on the EU profile adds a keyword requirement.
        let city = profile("city-ext", "1", Some("dcat-ap-3.0.1"), NEEDS_KEYWORD);
        let cat = small_catalogue("city-ext");
        let datasets = [small_dataset("ok", true)];
        let (saved, dry) = registry
            .upload_and_dry_run(city, &cat, &datasets, &BTreeMap::new())
            .unwrap();
        assert!(saved
            .form_fields
            .iter()
            .any(|f| f.path.ends_with("#keyword") && f.required));
        assert!(
            saved
                .form_fields
                .iter()
                .any(|f| f.path.ends_with("description")),
            "base shapes stack in"
        );
        assert_eq!((dry.total_datasets, dry.failing_datasets), (1, 1));
        assert_eq!(dry.newly_failing_datasets, ["ok"]);
        assert_eq!(
            registry
                .get(test_ids(), "city-ext", Some("missing"))
                .map(|p| p.version),
            Some("1".into()),
            "an unknown version falls back"
        );

        let reports = registry.validate_dataset(&cat, &datasets[0], None).unwrap();
        assert!(!reports[0].conforms);
        let feed = registry
            .check_catalogue_feed(&cat, &datasets, &BTreeMap::new(), None)
            .unwrap();
        assert_eq!(feed["profile"], "city-ext");
        assert_eq!(feed["conforms"], false);
        assert_eq!(feed["hiddenDatasets"][0]["dataset"], "ok");
        let eu = registry
            .check_catalogue_feed(&cat, &datasets, &BTreeMap::new(), Some("dcat-ap-3.0.1"))
            .unwrap();
        assert_eq!(eu["datasetsChecked"], 1);
        assert!(registry
            .check_catalogue_feed(&cat, &[], &BTreeMap::new(), Some("nope"))
            .is_err());

        // Shapes over the upload limits or with owl:imports are refused, as is a broken file.
        let imports = profile(
            "evil",
            "1",
            None,
            "<https://e.org/s> <http://www.w3.org/2002/07/owl#imports> <https://evil.example/x> .",
        );
        assert!(registry
            .upload_and_dry_run(imports, &cat, &[], &BTreeMap::new())
            .is_err());
        let broken = profile("broken", "1", None, "this is not turtle");
        assert!(registry
            .upload_and_dry_run(broken, &cat, &[], &BTreeMap::new())
            .is_err());
        assert!(
            registry.get(test_ids(), "broken", None).is_none(),
            "a refused upload is not registered"
        );
        let orphan = profile("orphan", "1", Some("no-such-base"), "");
        assert!(registry
            .upload_and_dry_run(orphan, &cat, &[], &BTreeMap::new())
            .is_err());
        assert!(
            registry.get(test_ids(), "orphan", None).is_none(),
            "an upload whose base does not resolve is not registered"
        );
    }

    #[test]
    fn an_upload_serves_its_own_catalogue_and_survives_a_restart() {
        let registry = ProfileRegistry::new();
        let cat = small_catalogue("city-ext");
        let (saved, _) = registry
            .upload_and_dry_run(
                profile("city-ext", "1", Some("dcat-ap-3.0.1"), NEEDS_KEYWORD),
                &cat,
                &[],
                &BTreeMap::new(),
            )
            .unwrap();
        // Another catalogue neither resolves nor lists it, pinned or not.
        let mut other = small_catalogue("city-ext");
        other.id = CatalogueId::from_uuid(Uuid::from_u128(11));
        let other_scope = (other.org_id, other.id);
        assert!(registry.get(other_scope, "city-ext", Some("1")).is_none());
        assert!(registry.get(other_scope, "city-ext", None).is_none());
        assert!(!registry
            .list(other_scope)
            .iter()
            .any(|p| p.id == "city-ext"));
        assert!(registry
            .validate_dataset(&other, &small_dataset("ok", true), None)
            .is_err());
        // A fresh process holds only the built-ins until the store's records are loaded.
        let restarted = ProfileRegistry::new();
        assert!(restarted
            .validate_dataset(&cat, &small_dataset("ok", true), None)
            .is_err());
        restarted.load_stored(test_ids(), std::slice::from_ref(&saved));
        let reports = restarted
            .validate_dataset(&cat, &small_dataset("ok", true), None)
            .unwrap();
        assert_eq!(reports[0].profile_id, "city-ext");
        assert!(restarted.get(other_scope, "city-ext", None).is_none());
    }

    #[test]
    fn a_report_that_is_no_json_is_a_storage_error_naming_the_dry_run() {
        let not_json = serde_json::from_str::<u8>("x").expect_err("x is no number");
        let e = dry_run_error(not_json);
        assert!(
            matches!(&e, DataroomError::Storage(m) if m.starts_with("dry run: expected value")),
            "{e:?}"
        );
    }

    #[test]
    fn custom_sample_validation() {
        let registry = ProfileRegistry::default();
        let (org, cat) = test_ids();
        let ok = r#"<https://e.org/d> a <http://www.w3.org/ns/dcat#Dataset> ; <http://www.w3.org/ns/dcat#keyword> "k" ."#;
        let report = registry
            .validate_custom_sample(NEEDS_KEYWORD, ok, org, cat, "pkg")
            .unwrap();
        assert!(report.conforms);
        let missing = r#"<https://e.org/d> a <http://www.w3.org/ns/dcat#Dataset> ."#;
        let report = registry
            .validate_custom_sample(NEEDS_KEYWORD, missing, org, cat, "pkg")
            .unwrap();
        assert_eq!(report.violation_count, 1);
        assert_eq!(report.profile_id, "custom-sample");
        assert!(registry
            .validate_custom_sample("not shapes", ok, org, cat, "pkg")
            .is_err());
        assert!(registry
            .validate_custom_sample(NEEDS_KEYWORD, "not turtle", org, cat, "pkg")
            .is_err());
    }
    #[test]
    fn a_poisoned_lock_and_a_missing_pinned_version_keep_the_registry_serving() {
        let registry = std::sync::Arc::new(ProfileRegistry::new());
        let poisoner = std::sync::Arc::clone(&registry);
        let crashed = std::thread::spawn(move || {
            let _guard = poisoner.profiles.write().unwrap();
            panic!("a writer crashed while holding the profile lock");
        })
        .join();
        assert!(crashed.is_err());
        assert!(registry.profiles.is_poisoned());

        assert!(registry
            .list(test_ids())
            .iter()
            .any(|p| p.id == "dcat-ap-3.0.1"));
        assert!(registry.get(test_ids(), "dcat-ap-3.0.1", None).is_some());

        let states = r#"
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix dcat: <http://www.w3.org/ns/dcat#> .
            @prefix ex: <https://e.org/> .
            <https://e.org/shape> a sh:NodeShape ; sh:targetClass dcat:Dataset ;
              sh:property [ sh:path ex:state ; sh:in ( "open" "closed" ) ] .
        "#;
        let (stored, _) = registry
            .upload_and_dry_run(
                profile("states", "1", None, states),
                &small_catalogue("states"),
                &[],
                &BTreeMap::new(),
            )
            .expect("upload works after the lock was poisoned");
        let field = stored
            .form_fields
            .iter()
            .find(|f| f.path == "https://e.org/state")
            .expect("sh:in field in the form layout");
        assert_eq!(field.allowed_values, ["open", "closed"]);

        // A catalogue pinned to a version that is gone validates against the unpinned record.
        let (rec, _) = registry
            .stacked_shapes(test_ids(), "states", Some("9"))
            .unwrap();
        assert_eq!(rec.version, "1");
        assert!(matches!(
            registry.stacked_shapes(test_ids(), "nobody", Some("1")),
            Err(DataroomError::NotFound(_))
        ));
    }
}
