#![forbid(unsafe_code)]
//! The `Store` contract of dataroom-store-pg's tests, run against the memory store in this crate's
//! own tests as well: a mutant of the memory store is judged by the contract (T-7624).

#[path = "../../store-pg/tests/common/contract.rs"]
mod contract;
#[allow(dead_code)]
#[path = "../../store-pg/tests/common/fixtures.rs"]
mod fixtures;

use dataroom_store_memory::MemoryStore;
use fixtures::*;

macro_rules! memory_store {
    ($($name:ident),* $(,)?) => {
        $(
            #[tokio::test]
            async fn $name() {
                contract::$name(&MemoryStore::new()).await;
            }
        )*
    };
}

memory_store!(
    catalogues,
    publishers,
    datasets,
    records,
    harvest_lock,
    odata,
    isolation,
    deleted_catalogues,
    guarded_writes,
    audited_writes,
    edges,
    minted_and_redirected,
    twin_catalogues
);
