//! A shako for the page and registry targets: one Organization whose reader may read, nothing
//! seeded, so every fuzzed request is a read and the forge never grows.

use std::sync::{Arc, LazyLock};

use shako_api::ShakoState;
use shako_core::{
    Action, InstanceId, LocalAuthorizer, OrgId, OutboundGuard, Principal, RoleBinding,
    ShakoSigningKey, StaticTokenBinding, Subject, TokenVerifier,
};
use shako_store_memory::MemoryStore;

pub const READER: &str = "reader-token";

pub static RT: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
});

static ROOT: LazyLock<tempfile::TempDir> =
    LazyLock::new(|| tempfile::tempdir().expect("a repository root"));

pub fn state() -> ShakoState {
    let org = OrgId::from_u128(10);
    ShakoState::new(
        InstanceId::from_u128(1),
        ROOT.path().to_path_buf(),
        Arc::new(MemoryStore::new()),
        Arc::new(LocalAuthorizer::new(vec![RoleBinding {
            org_id: org,
            principal: Principal::User("reader".into()),
            actions: vec![Action::Read],
            project_id: None,
            repo: None,
        }])),
        TokenVerifier::new(ShakoSigningKey::generate().expect("key")).with_static_tokens(vec![
            StaticTokenBinding::new(
                READER,
                Subject::authenticated("reader", "reader", vec![org]),
            ),
        ]),
        OutboundGuard::default(),
    )
}
