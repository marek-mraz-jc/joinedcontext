//! The forge teams of every project repository (PF-87, T-2655).
//!
//! Each registered project of layout 2 has two teams in the forge organization,
//! `{slug}-readers` (read) and `{slug}-writers` (write), each reaching that project's repository
//! and no other. Who is in them is Keycloak's: the groups of the same names, which
//! [`super::groups`] fills from the project's bindings, and the forge's group→team mapping. This
//! wave keeps the teams themselves: it creates them, takes away any other repository a team was
//! given, and removes the teams of a project that is gone.
//!
//! A team this wave made says so in its description. A team of the same name without that
//! description belongs to whoever made it and is never written or removed.

use std::collections::{BTreeMap, BTreeSet};

use crate::git::{GitError, GiteaClient, Team};

/// How a team this wave keeps starts its description.
pub const MARKER: &str = "joinedcontext project";

/// The two teams of a project, with the permission each carries.
pub fn teams_of(slug: &str) -> [(String, &'static str); 2] {
    [
        (format!("{slug}-readers"), "read"),
        (format!("{slug}-writers"), "write"),
    ]
}

fn managed(team: &Team) -> bool {
    team.description.starts_with(MARKER)
}

/// What one run did with one team.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamOutcome {
    pub team: String,
    /// What it changed; empty when the forge already matched.
    pub changes: Vec<String>,
    pub error: Option<String>,
}

impl TeamOutcome {
    fn of(team: &str) -> Self {
        Self {
            team: team.to_owned(),
            changes: Vec::new(),
            error: None,
        }
    }
}

/// What a run did with the project teams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Teams {
    /// The forge user owns the organization: each team, and what was done with it.
    Kept(Vec<TeamOutcome>),
    /// It does not, so it writes no team: the bootstrap keeps them (T-2647), and who is in them
    /// still follows the bindings through the Keycloak groups of the same names (T-3323).
    LeftToTheBootstrap,
}

/// Brings the organization's project teams to `projects`, slug → repository name, when the
/// forge user may; asks nothing more of a forge that would refuse every write (T-3323).
pub async fn converge(forge: &GiteaClient, projects: &BTreeMap<String, String>) -> Teams {
    match forge.owns_organization().await {
        Ok(true) => Teams::Kept(keep(forge, projects).await),
        Ok(false) => Teams::LeftToTheBootstrap,
        Err(error) => {
            let mut outcome = TeamOutcome::of("*");
            outcome.error = Some(format!(
                "the forge did not say whether this user owns the organization: {error}"
            ));
            Teams::Kept(vec![outcome])
        }
    }
}

async fn keep(forge: &GiteaClient, projects: &BTreeMap<String, String>) -> Vec<TeamOutcome> {
    let existing = match forge.org_teams().await {
        Ok(teams) => teams,
        Err(error) => {
            let mut outcome = TeamOutcome::of("*");
            outcome.error = Some(format!("the forge did not list its teams: {error}"));
            return vec![outcome];
        }
    };
    let mut outcomes = Vec::new();
    let mut wanted = BTreeSet::new();
    for (slug, repository) in projects {
        for (name, permission) in teams_of(slug) {
            wanted.insert(name.clone());
            let held = existing.iter().find(|team| team.name == name);
            outcomes.push(converge_one(forge, slug, repository, &name, permission, held).await);
        }
    }
    for team in existing
        .iter()
        .filter(|team| managed(team) && !wanted.contains(&team.name))
    {
        let mut outcome = TeamOutcome::of(&team.name);
        match forge.delete_team(team.id).await {
            Ok(()) => outcome
                .changes
                .push("its project is no longer registered; the team was removed".to_owned()),
            Err(error) => outcome.error = Some(error.to_string()),
        }
        outcomes.push(outcome);
    }
    outcomes
}

async fn converge_one(
    forge: &GiteaClient,
    slug: &str,
    repository: &str,
    name: &str,
    permission: &str,
    held: Option<&Team>,
) -> TeamOutcome {
    let mut outcome = TeamOutcome::of(name);
    let team = match held {
        Some(team) if !managed(team) => {
            outcome.error = Some(format!(
                "the forge has a team '{name}' this platform did not make; it is never written, \
                 so the project {slug}'s members do not reach its repository through it (PF-87). \
                 Rename or remove that team"
            ));
            return outcome;
        }
        Some(team) => team.clone(),
        None => {
            let description = format!("{MARKER} {slug}: {permission} on {repository} (PF-87)");
            match forge.create_team(name, &description, permission).await {
                Ok(team) => {
                    outcome.changes.push("the team was created".to_owned());
                    team
                }
                Err(error) => {
                    outcome.error = Some(error.to_string());
                    return outcome;
                }
            }
        }
    };
    let reaches = match forge.team_repositories(team.id).await {
        Ok(names) => names,
        Err(error) => {
            outcome.error = Some(error.to_string());
            return outcome;
        }
    };
    let mut result: Result<(), GitError> = Ok(());
    if !reaches.iter().any(|repo| repo == repository) {
        result = forge.set_team_repository(team.id, repository, true).await;
        if result.is_ok() {
            outcome
                .changes
                .push(format!("the team was given {repository}"));
        }
    }
    for other in reaches.iter().filter(|repo| *repo != repository) {
        if result.is_err() {
            break;
        }
        result = forge.set_team_repository(team.id, other, false).await;
        if result.is_ok() {
            outcome.changes.push(format!(
                "the team reached {other}, which is not its project's; it was taken away"
            ));
        }
    }
    if let Err(error) = result {
        outcome.error = Some(error.to_string());
    }
    outcome
}
