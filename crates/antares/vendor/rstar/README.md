# rstar 0.12.2, on heapless 0.9.4 (T-7429)

`src/` is rstar 0.12.2 from crates.io byte for byte except `src/rtree.rs` (`.crate` sha256
421400d13ccfd26dfa5858199c30a5d76f9c54e0dba7575273025b43c5175dbb, the checksum in
Cargo.lock before this copy), MIT OR Apache-2.0 (the published crate ships no license file).
`src/rtree.rs` differs in 19 signatures only: each iterator return type spells its elided
lifetime (`LocateInEnvelope<'_, T>`, `NearestNeighborIterator<'_, T>`, ...), rustc's own
`cargo fix` suggestion and what rstar 0.13 ships. rustc 1.99 warns on the hidden lifetime
(mismatched_lifetime_syntaxes), and workspace.yml checks antares-sql, which reaches rstar,
with `RUSTFLAGS="-D warnings"`, where a path crate's warning is an error.
`Cargo.toml` is the published manifest with its dev-dependencies removed and one change:
`heapless` 0.8 -> 0.9.4.

Why: heapless before 0.9.4 carries RUSTSEC-2026-0574..0582 (unsound: pool race, panicking
`Drop`, `Deque`/`IndexMap`/`SortedLinkedList` invariants, `Spsc::peek` ordering,
`FindMutView::pop` use after free), and `cargo audit --deny warnings` fails on them in
workspace.yml and on the image binary in build-image.yml. rstar uses one heapless type,
`binary_heap::BinaryHeap<T, Max, 32>` (`src/algorithm/nearest_neighbor.rs`, the inline
nearest-neighbour heap); its `new`, `push`, `pop`, `len`, `capacity` and `into_vec` keep
their signatures in 0.9, which is why rstar 0.14 moved to heapless 0.9 without touching that
file. geo 0.33.1, the newest geo release, still requires rstar ^0.12; geo main took rstar
0.14 (georust/geo#1641, 2026-11-12) but no release has it yet.

Delete this directory, `vendor/geo-types` and their two `[patch.crates-io]` lines in the
root Cargo.toml once a geo release depends on rstar 0.14.
