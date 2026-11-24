//! Input: an entity-set index byte, then the OData query string an admin sends. It is parsed the
//! way kaisatsu-admin parses it and run against a seeded in-memory store: rows or an error.
#![no_main]

use std::sync::LazyLock;

use kaisatsu_core::{
    kaisatsu_odata_service, GroupRepresentation, LocalAuthorizer, Principal, RequestCtx, Store,
    UserRepresentation,
};
use kaisatsu_store_memory::MemoryStore;
use libfuzzer_sys::fuzz_target;

const SETS: [&str; 8] = [
    "People",
    "Groups",
    "GroupMembers",
    "Roles",
    "Applications",
    "Realms",
    "Organizations",
    "Events",
];

static RT: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime")
});

static STORE: LazyLock<MemoryStore> = LazyLock::new(|| {
    let store = MemoryStore::default();
    RT.block_on(async {
        for (id, name) in [("u1", "ada"), ("u2", "bob"), ("u3", "Čech")] {
            store
                .create_user(UserRepresentation {
                    id: id.into(),
                    realm_id: "r".into(),
                    username: name.into(),
                    email: Some(format!("{name}@example.org")),
                    ..Default::default()
                })
                .await
                .expect("user");
        }
        let g = store
            .upsert_group(GroupRepresentation {
                realm_id: "r".into(),
                name: "staff".into(),
                path: "/staff".into(),
                ..Default::default()
            })
            .await
            .expect("group");
        store
            .add_user_to_group("r", "u1", &g.id)
            .await
            .expect("member");
    });
    store
});

fuzz_target!(|data: &[u8]| {
    let Some((&which, query)) = data.split_first() else {
        return;
    };
    let set = SETS[usize::from(which) % SETS.len()];
    let query = String::from_utf8_lossy(query);
    let service = kaisatsu_odata_service();
    let model = service.model();
    let edm = odata_url::EdmModel(&model);
    let Ok(odata_url::ODataUri::Resource { options, .. }) = odata_url::parse(
        &format!("{set}?{query}"),
        &edm,
        &odata_url::Limits::strict(),
    ) else {
        return;
    };
    let ctx = RequestCtx::new("r", Principal::realm_admin("root"));
    let _ = RT.block_on(STORE.query_entity_set(&ctx, &LocalAuthorizer, set, &options, 50));
});
