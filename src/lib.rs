//! joinedcontext Portal library: the axum application, the resource API, and the embedded reconciler
//! (see docs/Architecture/09-portal.md). The binary in `main.rs` only starts it.

// The release build lays out the editing agent's conversation future 130 queries deep, past the
// default of 128 (ci-full readme, 534379bd).
#![recursion_limit = "256"]

pub mod access_requests;
pub mod activity;
pub mod agents;
pub mod alerts;
pub mod answers;
pub mod api;
pub mod apps;
pub mod artifact_store;
pub mod assets;
pub mod auth;
pub mod branding;
pub mod brokers;
pub mod cache;
pub mod capped;
pub mod catalog;
pub mod change;
pub mod config;
pub mod connectors;
pub mod dashboards;
pub mod dataset_score;
pub mod db;
pub mod domain_verification;
pub mod endpoint_slug;
pub mod endpoint_usage;
pub mod entity_comments;
pub mod entity_trash;
pub mod erasure;
pub mod error;
pub mod feedback;
pub mod flow_map;
pub mod git;
pub mod grafana;
pub mod groups;
pub mod health_log;
pub mod health_probe;
pub mod keyed;
pub mod link_check;
pub mod live;
pub mod mcp;
pub mod openapi;
pub mod ops;
pub mod personal_connectors;
pub use ops::drafts;
pub use ops::verdict;
pub mod costs;
pub mod csp;
pub mod mqtt_allowance;
pub mod mqtt_capture;
pub mod mqtt_roles;
pub mod paired_tabs;
pub mod people;
pub mod permissions;
pub mod personal_data;
pub mod personal_keys;
pub mod pipeline_expiry;
pub mod pipeline_health;
pub mod pipeline_history;
pub mod pipeline_lifetime;
pub mod pipeline_log;
pub mod pipeline_outcomes;
pub mod pipeline_rights;
pub mod pipeline_samples;
pub mod pipeline_secrets;
pub mod pipeline_source_deletes;
pub mod pipeline_stats;
pub mod pipeline_status;
pub mod pipeline_validation;
pub mod plan;
pub mod public_docs;
pub mod quality;
pub mod quality_checks;
pub mod quotas;
pub mod reconciler;
pub mod record_check;
pub mod references;
pub mod request_id;
pub mod resource;
pub mod secret_files;
pub mod secret_scan;
pub mod server;
pub mod space_move;
pub mod space_usage;
pub mod spaces;
pub mod state;
pub mod store;
pub mod sync;
pub mod telemetry;
pub mod temporal_audit;
pub mod tools;

/// Name reported by `/api/v1/health` and the process banner.
pub const APP_NAME: &str = "joinedcontext-portal";

/// Version reported by `/api/v1/health` and OpenAPI documentation.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    #[test]
    fn app_name_is_stable() {
        assert_eq!(super::APP_NAME, "joinedcontext-portal");
    }

    /// Cargo loads every workspace member's manifest before it builds anything, so a
    /// member the Dockerfile does not copy fails the image build with "failed to load
    /// manifest for workspace member". That break reaches `main` because the image lane
    /// runs after the merge; this check runs on the pull request.
    #[test]
    fn the_dockerfile_copies_every_workspace_member() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let manifest = std::fs::read_to_string(root.join("Cargo.toml")).expect("Cargo.toml");
        let dockerfile = std::fs::read_to_string(root.join("Dockerfile")).expect("Dockerfile");
        let copied: Vec<&str> = dockerfile
            .lines()
            .filter_map(|line| line.trim().strip_prefix("COPY "))
            .flat_map(str::split_whitespace)
            .collect();

        for member in workspace_members(&manifest) {
            // `apps/*` is copied by copying `apps`; either spelling counts.
            let root_dir = member.split(['/', '*']).next().unwrap_or(&member);
            assert!(
                copied.iter().any(|path| {
                    let path = path.trim_start_matches("./");
                    path == member || path == root_dir
                }),
                "workspace member `{member}` is not copied into the Dockerfile build stage; \
                 add `COPY {root_dir} ./{root_dir}` or the image build fails on main"
            );
        }
    }

    /// The `members = [...]` entries of a workspace manifest.
    fn workspace_members(manifest: &str) -> Vec<String> {
        let Some(rest) = manifest.split_once("members = [").map(|(_, rest)| rest) else {
            return Vec::new();
        };
        let list = rest.split_once(']').map(|(list, _)| list).unwrap_or(rest);
        list.split(',')
            .map(|entry| entry.trim().trim_matches('"').to_owned())
            .filter(|entry| !entry.is_empty())
            .collect()
    }

    #[test]
    fn a_workspace_with_no_members_asks_for_nothing() {
        assert!(workspace_members("[package]\nname = \"x\"\n").is_empty());
        assert_eq!(
            workspace_members("members = [\"apps/*\", \"tools/gen\"]\n"),
            vec!["apps/*".to_owned(), "tools/gen".to_owned()]
        );
    }
}
