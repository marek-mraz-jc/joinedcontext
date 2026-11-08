#![forbid(unsafe_code)]
//! `shako-core`: domain types, UUIDv7 identifiers, RFC 9457 / OData / Gitea error mappings,
//! `Store` and `Authorizer` traits, `OperationRegistry` (`OperationDef` with `compat_path`),
//! OData 4.01 `Service` definitions, `RefPolicy` (branch & tag protection), `MemoryBudget`
//! semaphore and quotas, `OutboundGuard` (SSRF protection), and `TokenVerifier` (JWKS + ES256).

pub mod auth;
pub mod error;
pub mod ids;
pub mod model;
pub mod ops;
pub mod outbound;
pub mod policy;
pub mod quota;
pub mod store;
#[cfg(feature = "testkit")]
pub mod testkit;
pub mod validate;

pub use auth::{
    Action, AudClaim, Authorizer, Decision, JwksFetcher, JwtClaims, LocalAuthorizer, ObjectTarget,
    RequestCtx, RoleBinding, ShakoSigningKey, StaticTokenBinding, Subject, TokenVerifier,
};
pub use error::{GiteaErrorBody, ProblemDetails, ShakoError, ShakoResult};
pub use ids::{
    InstanceId, OrgId, PackageId, RepoId, WebhookId, constant_time_eq, constant_time_eq_bytes,
    sha256_hex,
};
pub use model::*;
pub use ops::{
    ODATA_NAMESPACE, ODataKind, ODataOperation, ODataParam, OperationDef, OperationRegistry,
    scope_for_context, shako_odata_service,
};
pub use outbound::{MAX_OUTBOUND_RESPONSE_BYTES, OUTBOUND_TIMEOUT, OutboundGuard, OutboundPolicy};
pub use policy::{MergePolicyInput, RefPolicy};
pub use quota::{
    ARCHIVE_OPERATION_WEIGHT_BYTES, DEFAULT_MEMORY_SEMAPHORE_BYTES, MAX_DELTA_CHAIN_DEPTH,
    MAX_PKT_LINE_LEN, MAX_REFS_PER_PUSH, MAX_REFS_PER_REPO, MAX_SINGLE_OBJECT_BYTES,
    MAX_SINGLE_PUSH_BYTES, MERGE_OPERATION_WEIGHT_BYTES, MemoryBudget, PACK_OPERATION_WEIGHT_BYTES,
    check_repo_count_quota, remaining_storage_allowance,
};
pub use store::{MemorySecretStore, SecretStore, Store, StoreFuture};
pub use validate::{
    repo_disk_path, validate_branch_name, validate_full_ref_name, validate_package_segment,
    validate_repo_or_owner_name, validate_tag_name, validate_tree_path,
};
