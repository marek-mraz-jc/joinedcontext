//! Agent run state machine, models, and cryptographic ticket generation.

use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::{PasswordHasher, SaltString};
use argon2::Argon2;
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

const SLUG_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// The four kinds of builder runs (AG-67..AG-71).
pub const RUN_KINDS: [&str; 4] = ["conversation", "application", "dashboard", "analysis"];

/// Where one builder run stands (Architecture/19 §5). The wire name of every variant is the
/// name of the state in that chapter, which is why the rename is `snake_case` and not
/// `lowercase`: `awaiting_approval` is one state, not one word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunStatus {
    Queued,
    Starting,
    Interviewing,
    Building,
    Testing,
    Previewing,
    AwaitingApproval,
    Published,
    Failed,
    Cancelled,
    Expired,
}

impl AgentRunStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Starting => "starting",
            Self::Interviewing => "interviewing",
            Self::Building => "building",
            Self::Testing => "testing",
            Self::Previewing => "previewing",
            Self::AwaitingApproval => "awaiting_approval",
            Self::Published => "published",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
        }
    }

    /// The state one of these names, or `None` for anything else. The stored column is text, so
    /// a row written by a newer Portal is read as unknown rather than panicking here.
    pub fn parse(name: &str) -> Option<Self> {
        [
            Self::Queued,
            Self::Starting,
            Self::Interviewing,
            Self::Building,
            Self::Testing,
            Self::Previewing,
            Self::AwaitingApproval,
            Self::Published,
            Self::Failed,
            Self::Cancelled,
            Self::Expired,
        ]
        .into_iter()
        .find(|status| status.as_str() == name)
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Published | Self::Failed | Self::Cancelled | Self::Expired
        )
    }

    pub fn allows_transition_to(&self, next: Self) -> bool {
        // A run that is over is over, the state it ended in included: a second cancel of a
        // cancelled run is a conflict, not a second cancellation.
        if self.is_terminal() {
            return false;
        }
        // The same live state twice is not a transition. The workspace reports where it is
        // rather than what changed, so it says `building` more than once.
        if *self == next {
            return true;
        }
        // Architecture/19 §5, read as a table: every live state may fail, be cancelled or
        // expire, and the forward edges are the ones drawn there.
        matches!(
            (self, next),
            (Self::Queued, Self::Starting)
                | (Self::Starting, Self::Interviewing | Self::Building)
                | (Self::Interviewing, Self::Building)
                | (Self::Building, Self::Testing)
                | (Self::Testing, Self::Previewing)
                | (Self::Previewing, Self::AwaitingApproval)
                | (Self::AwaitingApproval, Self::Published)
                | (_, Self::Failed | Self::Cancelled | Self::Expired)
        )
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct AgentRun {
    pub id: String,
    pub project: String,
    pub app_name: String,
    /// The application's name for people, chosen with its first version; `app_name` is the id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub endpoint_name: String,
    pub endpoint_slug: String,
    /// Every endpoint the run reads, `[{name, slug, space}]`, the primary (`endpoint_name`)
    /// first; `[]` for a run of one endpoint recorded before several were possible (AP-44).
    #[serde(default = "empty_endpoints")]
    pub endpoints: serde_json::Value,
    pub profile: String,
    pub kind: String,
    pub unattended: bool,
    #[serde(default)]
    pub continues: Option<String>,
    pub app_class: String,
    pub visibility: String,
    pub prompt: String,
    pub prompt_digest: String,
    pub data_needs: serde_json::Value,
    pub allows_write: bool,
    pub branch: String,
    pub path_prefix: String,
    pub status: String,
    #[serde(skip_serializing)]
    pub ticket_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merge_request: Option<i32>,
    /// The `chg-…` id of the Change that publishes the application, read from `merge_request`
    /// when the API answers (AP-71).
    #[sqlx(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_id: Option<String>,
    /// The forge's public web address of the run's source, its `path_prefix` on its branch (AP-71).
    #[sqlx(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    /// The GitHub copy of the application's repository on the run's branch, when the installation
    /// keeps one (AP-79). The forge's `source_url` stays the repository of record.
    #[sqlx(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirror_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_url: Option<String>,
    /// Milliseconds from creation to the first preview, set once (AP-57).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_frame_ms: Option<i64>,
    /// Milliseconds from creation to the first generated version, set once.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_version_ms: Option<i64>,
    /// The files a run wrote, path to content; `{}` for a workspace run (AP-56).
    #[serde(skip_serializing, default = "empty_files")]
    pub files: serde_json::Value,
    pub steps: i32,
    pub tokens_used: i64,
    pub created_by: String,
    /// `person`, or `journey` for a run the Portal's own live journeys started (AG-93). Only a
    /// browser session sets `journey`; a list leaves those out unless it is asked for them.
    #[serde(default = "person_origin")]
    pub origin: String,
    /// The identity that started the run, as the Portal read it from that person's session.
    ///
    /// A run outlives the session that started it, and a workspace calling back through the
    /// proxy carries no session of its own. The operations registry judges a person by their
    /// bindings, and most bindings name a group — so a name alone would quietly refuse a steward
    /// their own grants. Never serialized: a run's readers have no business with somebody's
    /// groups (AG-64, PF-50).
    #[serde(skip_serializing, default)]
    pub starter: serde_json::Value,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    pub expires_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct AgentRunEvent {
    pub run_id: String,
    pub seq: i64,
    pub kind: String,
    pub payload: serde_json::Value,
    pub created_at: String,
}

fn person_origin() -> String {
    RunOrigin::Person.as_str().to_owned()
}

/// Who a run is for (AG-93): a person, or one of the Portal's own live journeys, whose test
/// runs would otherwise crowd the history people read. It hides; it never deletes (AG-45).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOrigin {
    Person,
    Journey,
}

impl RunOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Person => "person",
            Self::Journey => "journey",
        }
    }
}

fn empty_endpoints() -> serde_json::Value {
    serde_json::Value::Array(Vec::new())
}

fn empty_files() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

/// A random RFC 4122 version 4 identifier for one run.
pub fn mint_run_id() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

/// Generates a single-use 32-byte ticket and its Argon2id hash.
pub fn mint_ticket() -> (String, String) {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let ticket: String = bytes
        .iter()
        .map(|byte| char::from(SLUG_ALPHABET[usize::from(byte & 0x1f)]))
        .collect();

    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(ticket.as_bytes(), &salt)
        .expect("argon2 hashing succeeds")
        .to_string();

    (ticket, hash)
}

pub fn digest_prompt(prompt: &str) -> String {
    let hash = Sha256::digest(prompt.as_bytes());
    format!("sha256:{:x}", hash)
}
