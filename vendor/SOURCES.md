# Sources of joined and vendored code

| Path | Repository | Commit | How |
|---|---|---|---|
| `crates/ekiben` | marek-mraz-jc/ekiben (`/workspace/ekiben`) | `787dfe30642c1c8290d25bac9a0662957457a81f` | `git subtree add --squash` |
| `crates/antares` | Antares-NGSI-LD-Context-Broker (`/workspace/Antares-NGSI-LD-Context-Broker`, origin/dev) | `b683bfc3c59aa19a39c9e0b87e83feeb5625c0ae` | `git subtree add --squash` |
| `crates/portal` | joinedcontext-portal (`/workspace/joinedcontext-portal`) | `8d2241f901206be014e8b0ade83136708298f73e` | `git subtree add --squash` |
| `vendor/joinedcontext-platform` | joinedcontext-platform (`/workspace/joinedcontext-platform`, origin/main) | `e59ceb9f086c0e823fb476ec5228c1c1e4977f04` | `git archive` of `Cargo.toml`, `Cargo.lock` and the crates the Portal links (jc-core, jc-dcat, jc-manifest, jc-ngsi-input, jc-relationships, jc-tabular, context-gateway, jcctl); workspace `members` narrowed to them and the kreuzberg patch dropped. The Portal pinned rev `9b0b8594`; origin/main is 4 commits ahead (a calamine/pulldown-cmark bump and a gateway test wording). |
| `vendor/ngsild` | marek-mraz-jc/ngsild tag `v0.3.0` (generated from Antares `15c08b85`) | `b06b4145fda4ee1b2147b08853e73e66da1959bd` | `git archive` from the local cargo git cache (GitHub fetch refused); unchanged |

The root `Cargo.toml` and `crates/portal/Cargo.toml` patch the two git URLs to these paths.
