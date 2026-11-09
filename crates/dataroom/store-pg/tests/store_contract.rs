#![forbid(unsafe_code)]
//! The `Store` contract against the memory store and against the Postgres store reached
//! through the request role (no SUPERUSER, no BYPASSRLS), so forced RLS is in force for
//! every statement.

mod common;

use common::contract;
use dataroom_store_memory::MemoryStore;

macro_rules! both_stores {
    ($($name:ident),* $(,)?) => {
        mod memory {
            use super::*;
            $(
                #[tokio::test]
                async fn $name() {
                    contract::$name(&MemoryStore::new()).await;
                }
            )*
        }
        mod postgres {
            use super::*;
            $(
                #[tokio::test]
                async fn $name() {
                    let Some(pg) = common::pg(concat!("postgres::", stringify!($name))).await else {
                        return;
                    };
                    contract::$name(&pg.app_store().await).await;
                }
            )*
        }
    };
}

both_stores!(
    catalogues,
    publishers,
    datasets,
    records,
    harvest_lock,
    odata,
    isolation
);
