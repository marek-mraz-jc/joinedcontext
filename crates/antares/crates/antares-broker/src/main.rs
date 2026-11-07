// SPDX-License-Identifier: EUPL-1.2
//! The `antares` binary: the broker library's [`antares_broker::main_entry`].

/// Jemalloc with decay-based purging — RSS returns to ~live×1.2 when idle;
/// tune via MALLOC_CONF (e.g. dirty_decay_ms). Deliberately not glibc
/// malloc, whose arena fragmentation never gives memory back.
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    antares_broker::main_entry()
}
