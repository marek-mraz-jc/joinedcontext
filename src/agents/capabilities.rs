//! The capabilities a person chose for a conversation (AG-92, T-2718, API/04 §8): a preset and
//! each endpoint's access. They only narrow what the profile and the person already allow
//! (AG-70); a conversation without them is narrowed by nothing more.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::agents::paths::Path;
use crate::resource::is_dns1123;

/// What the assistant may do in the conversation, in three steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum Preset {
    /// Read the data and the project, change nothing.
    Read,
    /// Also draft what the person proposes (AG-73).
    Propose,
    /// Also the paths that build an app, a dashboard, a pipeline or a KPI.
    Build,
}

/// What the assistant may prepare on one endpoint's entities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum EndpointAccess {
    Read,
    ReadWrite,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Capabilities {
    pub preset: Preset,
    /// An endpoint not named here is `read`.
    #[serde(default)]
    pub endpoints: BTreeMap<String, EndpointAccess>,
    /// The connector segment the conversation runs as (T-5958, API/14 §1), e.g. `pipeline`: the
    /// assistant calls that segment's registry tools natively, checked as a connector's calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment: Option<String>,
}

/// The conversation's own tools that only read or ask: every preset has them. A registry
/// operation reads when its lane is green, which the caller says.
pub const READING: [&str; 7] = [
    "jc_ask",
    "jc_ui_navigate",
    "jc_switch_path",
    "describe_tool",
    "search_catalog",
    "jc_catalog_search",
    "query_endpoint",
];

/// More endpoints than a conversation can read are no choice about them (AG-75: five).
const MAX_NAMED: usize = crate::agents::endpoints::MAX_ENDPOINTS;

impl Preset {
    fn word(self) -> &'static str {
        match self {
            Preset::Read => "read",
            Preset::Propose => "propose",
            Preset::Build => "build",
        }
    }
}

impl Capabilities {
    /// A request's choice, or why it is none: names are an endpoint's, and a conversation
    /// reads at most five.
    pub fn check(&self) -> Result<(), String> {
        if self.endpoints.len() > MAX_NAMED {
            return Err(format!(
                "access names at most {MAX_NAMED} endpoints, as a conversation reads"
            ));
        }
        if let Some(name) = self.endpoints.keys().find(|name| !is_dns1123(name)) {
            return Err(format!("access names '{name}', which is no endpoint name"));
        }
        match self.segment.as_deref() {
            Some(segment) if !crate::connectors::is_segment(segment) => Err(format!(
                "segment '{segment}' is no segment; one of data-read, context, pipeline, access, \
                 publish, apps, operate, admin"
            )),
            _ => Ok(()),
        }
    }

    pub fn allows_path(&self, path: Path) -> bool {
        match path {
            Path::FindData => true,
            Path::ShareData | Path::UploadData | Path::CreateDataModel => {
                self.preset != Preset::Read
            }
            Path::BuildApp | Path::BuildDashboard | Path::IntegratePipeline | Path::DefineKpi => {
                self.preset == Preset::Build
            }
        }
    }

    /// Whether `tool` may run; `reads` is whether it only reads.
    pub fn allows_tool(&self, tool: &str, reads: bool) -> bool {
        self.preset != Preset::Read || reads || READING.contains(&tool)
    }

    /// Whether a change to the entities of `endpoint` may be prepared.
    pub fn writes(&self, endpoint: &str) -> bool {
        self.preset != Preset::Read
            && self.endpoints.get(endpoint) == Some(&EndpointAccess::ReadWrite)
    }

    /// What the model reads when the choice refuses a tool or a path.
    pub fn refusal(&self, what: &str) -> String {
        format!(
            "the person chose '{}' for this conversation, which does not let you {what}; tell \
             them what to switch on in Capabilities to have it, and do nothing in its place",
            self.preset.word()
        )
    }

    /// What the path request answers when the preset leaves the path out.
    pub fn path_refusal(&self, path: Path) -> String {
        format!(
            "the capabilities '{}' do not include the path '{}'",
            self.preset.word(),
            path.id()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chosen(preset: Preset, endpoints: &[(&str, EndpointAccess)]) -> Capabilities {
        Capabilities {
            preset,
            endpoints: endpoints
                .iter()
                .map(|(name, access)| ((*name).to_owned(), *access))
                .collect(),
            segment: None,
        }
    }

    #[test]
    fn each_preset_takes_the_paths_of_the_one_before_and_its_own() {
        let taken = |preset| {
            Path::ALL
                .iter()
                .filter(|path| chosen(preset, &[]).allows_path(**path))
                .map(|path| path.id())
                .collect::<Vec<_>>()
        };
        assert_eq!(taken(Preset::Read), ["find-data"]);
        let propose = taken(Preset::Propose);
        assert_eq!(propose.len(), 4, "{propose:?}");
        assert!(propose.contains(&"share-data") && !propose.contains(&"build-app"));
        assert_eq!(taken(Preset::Build).len(), Path::ALL.len());
    }

    #[test]
    fn read_keeps_the_reading_tools_and_refuses_every_other() {
        let read = chosen(Preset::Read, &[("bikes", EndpointAccess::ReadWrite)]);
        for tool in READING {
            assert!(read.allows_tool(tool, false), "{tool}");
        }
        assert!(read.allows_tool("jc_resource_get", true));
        for tool in [
            "change_resource",
            "propose_endpoint",
            "write_entities",
            "jc_draft_put",
        ] {
            assert!(!read.allows_tool(tool, false), "{tool}");
        }
        // A write on an endpoint needs more than its name: the preset has to prepare changes.
        assert!(!read.writes("bikes"));
        assert!(read.refusal("call change_resource").contains("'read'"));
    }

    #[test]
    fn a_change_to_entities_needs_the_endpoints_own_read_write() {
        let propose = chosen(
            Preset::Propose,
            &[
                ("bikes", EndpointAccess::ReadWrite),
                ("air", EndpointAccess::Read),
            ],
        );
        assert!(propose.allows_tool("write_entities", false));
        assert!(propose.writes("bikes"));
        assert!(!propose.writes("air"));
        assert!(!propose.writes("unnamed"));
    }

    #[test]
    fn a_choice_names_endpoints_and_at_most_five() {
        assert!(chosen(Preset::Build, &[("bikes", EndpointAccess::Read)])
            .check()
            .is_ok());
        let odd = chosen(Preset::Build, &[("Bikes!", EndpointAccess::Read)]);
        assert!(odd.check().is_err());
        let many: Vec<(String, EndpointAccess)> = (0..6)
            .map(|n| (format!("e{n}"), EndpointAccess::Read))
            .collect();
        let many = Capabilities {
            preset: Preset::Read,
            endpoints: many.into_iter().collect(),
            segment: None,
        };
        assert!(many.check().is_err());
        let parsed: Result<Capabilities, _> =
            serde_json::from_str(r#"{"preset":"read","extra":1}"#);
        assert!(parsed.is_err(), "an unknown field is refused");
        let parsed: Capabilities =
            serde_json::from_str(r#"{"preset":"propose","endpoints":{"bikes":"readWrite"}}"#)
                .expect("the documented shape");
        assert!(parsed.writes("bikes"));
        // T-5958: a segment is one a connector may name, nothing else.
        let segment = |name: &str| -> Capabilities {
            serde_json::from_value(serde_json::json!({ "preset": "read", "segment": name }))
                .expect("the documented shape")
        };
        assert_eq!(segment("pipeline").check(), Ok(()));
        assert_eq!(segment("admin").check(), Ok(()));
        let refused = segment("root").check().expect_err("an unknown segment");
        assert!(refused.contains("'root'"), "{refused}");
    }
}
