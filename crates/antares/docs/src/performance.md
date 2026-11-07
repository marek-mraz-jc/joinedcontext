# Performance

Two scheduled runs on `dev` produce every performance number this
project publishes; nothing in this chapter is typed in by hand. Both rent
a dedicated-vCPU machine at Hetzner for the run, register it as an
ephemeral GitHub runner, and delete it afterwards (`perf-janitor` sweeps a
leaked server or volume by its expiry label). Results land under
[`/reports/perf/latest/`](https://antaresbroker.joinedcontext.com/reports/perf/latest/)
with the raw CSVs next to the tables. One table is the exception:
[query plans under row-level security](#query-plans-under-row-level-security)
comes from a local script, named with its box beside it.

| run | box | what it measures | cadence |
|---|---|---|---|
| `perf-weekly` | ccx33 (8 dedicated vCPU, 32 GB), one hour | the request shapes other brokers publish, on the in-memory store; dispatched with `store=postgres` it adds the same tables against a PostgreSQL container on the box, at pool 20 and 100 (`pg-pool<N>/`) | Friday |
| `scale-weekly` | ccx53 (32 dedicated vCPU, 128 GB) + volume, one hour | the design targets on PostgreSQL, at scale 0.01 | Thursday |

`scale-weekly` sizes the database to the box it rents: `shared_buffers` a
quarter of RAM, `maintenance_work_mem` a sixteenth, the shared memory
segment an eighth, and the broker's connection pool eight per core with
the eight-core value pinned to the 100 it was measured at. A number from
one server type therefore does not compare with a number from another;
the box is named next to every table.

## The shapes (`perf-weekly`)

Every script lives in `dev/perf/` and runs on a laptop the same way it
runs in CI; `k6` is the only tool it needs.

| table | script | method |
|---|---|---|
| startup and idle footprint | `startup.sh` | exec to the first `200` from `/q/health`, median of five, `VmRSS` right after, per store |
| throughput per shape | `shapes.sh` | 100 five-attribute entities; `GET /entities?type=Vehicle&limit=20` at 50 and 200 concurrent clients, `GET /entities/{id}` at 50 (`SPECS` picks other rows, the PostgreSQL dispatch runs 64, 256 and 1 024 clients); five seconds, median of three runs, p99 from the same runs. The `facade` and `facade-twin` shapes run the same pair only when the binary under test serves `/x/example/things` — a shipped build does not |
| core scaling | `core-scale.sh` | broker pinned to 1, 2, 4, 8 physical cores with `taskset`, load generator on the remaining cores; refuses a step it cannot isolate. `cores used` is the broker's CPU time over the window against the cores it was allotted; `peak threads` is the largest thread count of the process, which is what a store driver that parks threads instead of awaiting shows up in |
| saturation knee | `saturate.sh` | open model, +500 rps every 30 s until p99 passes 50 ms or errors pass 0.1 %; the knee is the last stage that held, the curve is a CSV; `cores used` and `peak threads` as in core scaling, over the whole sweep |
| noise profile | `variance.py` | the same commit measured ten times; the fence for a future regression gate is `Q3 + 3·IQR` of each metric's own history |

The load generator shares the machine with the broker, as in every
published broker table; the numbers describe that shape and nothing
else, and quadrupling the concurrency shows the queue, not the broker.
Reproduce them on your own box before quoting them:

```bash
cargo build --release -p antares-broker
dev/perf/startup.sh && dev/perf/shapes.sh && dev/perf/core-scale.sh && dev/perf/saturate.sh
python3 dev/perf/report.py results/perf     # index.html + perf.json
```

## The measured ceiling

Two `perf-weekly` dispatches. The first, on ccx33 (8 logical, 4 physical
cores, three passes) at commit `b55d554`, could isolate only the 1- and
2-core steps: pinning the broker to 4 cores needs 8 physical, and to 8
needs 16. The second, on ccx53 (32 logical, 16 physical cores, two passes)
at commit `41610be`, runs the whole ladder, and the tables below are that
run. `store=postgres` in both; the load generator shares the machine, so
every row is broker plus generator.

Core scaling, one row per store and pool: the broker pinned to N physical
cores (SMT siblings excluded), the generator on the rest, query shape at
50 concurrent clients. `cores used` is the broker's CPU time over the
window against the cores it was allotted.

| store | 1 core | 2 cores | 4 cores | 8 cores | efficiency at 2 / 4 / 8 | cores used at 8 | peak threads at 8 |
|---|---|---|---|---|---|---|---|
| memory | 3 076 req/s | 6 050 req/s | 11 342 req/s | 12 554 req/s | 98 % / 92 % / 51 % | 7.64 | 16 |
| postgres, pool 20 | 2 097 req/s | 3 809 req/s | 7 093 req/s | 7 238 req/s | 91 % / 85 % / 43 % | 6.47 | 73 |
| postgres, pool 100 | 2 026 req/s | 3 665 req/s | 7 157 req/s | 7 837 req/s | 90 % / 88 % / 48 % | 7.03 | 85 |

The same rows as CPU spent per request (`cores used` over req/s), which is
what the efficiency column is measuring underneath:

| store | 1 core | 2 cores | 4 cores | 8 cores |
|---|---|---|---|---|
| memory | 0.31 ms | 0.32 ms | 0.34 ms | 0.61 ms |
| postgres, pool 20 | 0.47 ms | 0.49 ms | 0.50 ms | 0.89 ms |
| postgres, pool 100 | 0.48 ms | 0.50 ms | 0.51 ms | 0.90 ms |

The saturation knee, whole box, open model:

| store | shape | knee | p99 at the knee | first failing stage | cores used | peak threads |
|---|---|---|---|---|---|---|
| memory | query | 5 000 rps | 1.2 ms | none reached | 0.96 | 39 |
| memory | write | 5 000 rps | 0.6 ms | none reached | 0.47 | 62 |
| postgres, pool 20 | query | 5 000 rps | 3.0 ms | none reached | 2.17 | 77 |
| postgres, pool 20 | write | 1 000 rps | 2.9 ms | 1 500 rps | 1.79 | 4 038 |
| postgres, pool 100 | query | 5 000 rps | 2.6 ms | none reached | 2.18 | 64 |
| postgres, pool 100 | write | 1 000 rps | 2.8 ms | 1 500 rps | 1.80 | 4 038 |

Throughput per shape at 64, 256 and 1 024 concurrent clients, whole box:

| store | shape | c64 | c256 | c1024 |
|---|---|---|---|---|
| memory | query | 19 272 req/s, p99 7.7 ms | 21 254 req/s, p99 36.6 ms | 21 566 req/s, p99 123.4 ms |
| memory | retrieve | 39 766 req/s, p99 3.8 ms | 43 805 req/s, p99 19.7 ms | 45 425 req/s, p99 68.1 ms |
| postgres, pool 20 | query | 12 418 req/s, p99 7.3 ms | 11 948 req/s, p99 23.2 ms | 11 448 req/s, p99 93.2 ms |
| postgres, pool 20 | retrieve | 12 432 req/s, p99 5.9 ms | 12 233 req/s, p99 22.3 ms | 11 766 req/s, p99 90.0 ms |
| postgres, pool 100 | query | 12 426 req/s, p99 7.8 ms | 11 823 req/s, p99 28.5 ms | 10 292 req/s, p99 104.0 ms |
| postgres, pool 100 | retrieve | 13 974 req/s, p99 6.0 ms | 14 144 req/s, p99 20.4 ms | 13 696 req/s, p99 75.7 ms |

The run predates ADR-0022: the Postgres driver still parked a thread per
in-flight store call, which is what the four-figure `peak threads` column
records. The knees and the per-request cost are what that shape delivered.

What the run says, in the order it matters:

- Scaling is close to linear to four cores (98 %, 92 % on memory; 91 %,
  85 % and 90 %, 88 % on the two pools) and loses half of that at eight.
- The eighth core is used, not idle: 7.64 of 8 on memory, 7.03 and 6.47
  on the two pools. What changes is the price of a request, which is flat
  from one core to four and then rises by about 80 % on all three stores.
  A cost that appears identically in the in-process store, which never
  parks a thread on a socket, is not the storage driver: it is contention
  above the store, in the path the three shapes share.
- The blocking pool is nowhere near its ceiling. The Postgres write shape
  parks 4 038 live OS threads at its knee against a ceiling of 11 024
  (`ANTARES_MAX_CONNECTIONS` plus 1 024, `main.rs`), and the query shapes
  park 64 to 77. Nothing deadlocks and no shape reaches the cap.
- Pool size barely moves anything on this box. Pool 100 is 8 % faster at
  eight cores and 10 % slower at 1 024 concurrent query clients; both
  pools hold the same 1 000 rps write knee and fail at the same 1 500.
  The ccx33 run's "a larger pool is worse" reads as an artefact of four
  physical cores, not a property of the pool.
- The write path is what bends first. Both Postgres pools hold 1 000 rps
  and fail at 1 500 while every query shape holds the harness ceiling of
  5 000 rps; the in-memory write path holds 5 000 rps on 0.47 cores.

### The update shape

The ladder above is the query shape. `perf-weekly` run `33797374897`, same
ccx53 box, runs it again against the update shape — the write path with the
notification pipeline behind it — once per store:

| store | 1 core | 2 cores | 4 cores | 8 cores | efficiency at 2 / 4 / 8 | cores used at 8 | peak threads at 8 |
|---|---|---|---|---|---|---|---|
| memory | 13 635 req/s | 19 498 req/s | 11 573 req/s | 12 538 req/s | 71 % / 21 % / 11 % | 6.78 | 44 |
| file | 3 870 req/s | 4 492 req/s | 3 744 req/s | 3 617 req/s | 58 % / 24 % / 12 % | 0.94 | 17 |

Updates do not merely scale worse than queries: past two cores they scale
backwards, and eight cores serve fewer requests per second than one while
burning 6.78 of them. Something serializes and the cores spend their time
arriving at it.

Not the store's map lock, which is what a single-tenant ladder would blame
and what tenant sharding would answer: this ladder drives one tenant, so
sharding by tenant would leave every request on the same shard and change
nothing. What the write path does that the query path does not is hand the
worker's queue to another thread — `block_in_place` — for every document it
writes, so the store can commit without stalling an async worker. In `file`
mode that commit is an fsync and the hop is the point. In `memory` mode
there is no commit: the write is a lock and a map insert, and the hop is the
whole cost, paid once per write and paid more the more cores there are to
hand work between. The hop is now taken only where something blocks; the
paths that hold the write section for a whole scan (the 4.22 sweep, the two
purges) still take it in either mode.

### The exit criterion

`perf-weekly` run `33683839528` sets what the runtime work has to hold. A
change to the request runtime, the drivers or the store keeps all of it:

- Efficiency at eight allotted physical cores, query at c50: at least
  51 % on memory and 43 % on either Postgres pool.
- CPU per request at eight cores: no more than twice its one-core value
  on any store. The run itself sits at 1.97 (memory), 1.89 (pool 20) and
  1.87 (pool 100).
- Saturation, whole box: the knee at 5 000 rps or better for every query
  shape and 1 000 rps or better for the Postgres write shape, p99 at the
  knee within 3.0 ms.
- Live OS threads at the write knee: 4 038 or fewer, against the 11 024
  ceiling.

## Deployment scenarios

`dev/perf/scenarios.sh` starts one or more brokers from the release binary
(`dev/perf/fleet.sh`), seeds each world over the API and asks one question
per scenario. `MODE=check` runs the conformance assertions of
`dev/perf/scenario-check.py` against a memory-store fleet in seconds;
`MODE=load` runs the same assertions first and then a k6 rate ladder whose
every number comes from k6's summary or the sink's counters. A verdict is
computed from those numbers and its note names the failing assertion. The
fleet notifies a single-process `sink.py` of its own on port 9810: the load
rig's sink on 9800 runs multi-process, and its front door only folds the
workers' counters.

```bash
MODE=check STORE=memory   ./dev/perf/scenarios.sh              # all nine
MODE=load  STORE=postgres ./dev/perf/scenarios.sh loop fan-in  # a subset
```

The nine worlds, and what each one asks:

- **S1 `hot-entity`**: one broker, 1 000 vehicles. Concurrent partial
  updates on one entity (5.6.3, distinct `datasetId`s) versus the same
  load spread over all of them: are updates lost, and what does the
  contention cost in p99?
- **S2 `noisy-tenant`**: one broker, tenants `quiet` and `loud`. The quiet
  tenant's GET p99 is measured alone and under a write flood on `loud`
  (4.14 isolation, in time as well as in data).
- **S3 `slow-subscriber`**: one broker, ten fast endpoints and one that
  answers after 500 ms. Does the slow one hold the fast ones back, given
  `ANTARES_DELIVERY_WIDTH_PER_TENANT`?
- **S4 `fan-in`**: one broker, 50 subscriptions matching one entity. How
  many notifications per second does one update stream fan out to, and are
  all of them delivered?
- **S5 `hub-sources`**: a hub and two source brokers, registered with the
  `tenant` member (a forward never carries the client's tenant). Federated
  query and retrieve through the hub (4.3.6.1, 5.7.2): complete, merged,
  404 for an absent id.
- **S6 `collision`**: the same entity id in the hub and a source. 4.5.5
  merge of non-colliding attributes, 4.3.6.2 local data over an auxiliary
  source, 5.9.2.4 409 for an exclusive or redirect registration that
  overlaps data already held.
- **S7 `loop`**: brokers A and B registered to each other. A query
  terminates with each side's data once (6.3.18 `Via`); a write whose
  chain already names the receiver runs locally under an inclusive
  registration and answers 508 (6.3.17) when the only matching source is a
  redirect. An id-only write carries no type, so every type-only
  registration matches it: the 508 case needs a tenant whose only
  registration is the redirect.
- **S8 `distributed-subscription`**: a subscription at the hub, the entity
  at a source. 5.8.1.4: the hub plants a reduced copy at the source
  (registration `operations` includes `federationOps`) and a change at the
  source reaches the hub's subscriber, counted against the accepted
  updates.
- **S9 `ha-pair`**: two broker pods on one PostgreSQL database and one NATS
  JetStream bus (`NATS_URL`, `PG_URL_BASE`; skipped without them). Writes
  alternate between the pods; notifications are counted against accepted
  writes, so a duplicate or a loss shows as a mismatch.

The tables land in `scenarios/` of the run artifact; `report.py` folds them
into `index.html` and `pdf.py` gives each scenario a page (world, question,
what CIM 009 requires, verdict, measured numbers).

## The design targets (`scale-weekly`)

The README's target table is a design contract; this run is where each
row gets its measured column. `SCALE` scales every count linearly, so the
same rig runs at `0.0001` against a laptop's Postgres and at `1.0` on the
rented box:

| stage | script | at scale 1.0 |
|---|---|---|
| entities | `gen.py` streaming into `dev/bulk-load.sh` (one COPY stream) | 100,000,000 over 10,000 tenants, five attributes and a location each |
| subscriptions | `api-load.py subscriptions` | 100,000, one per tenant round-robin, HTTP to the sink, every tenth over MQTT |
| registrations | `api-load.py registrations` | 100,000, one id pattern each, endpoint at the sink |
| resident set | `rss.sh` | broker and Postgres backends sampled at 1 Hz for the whole run, peaks printed as the verdict table. Ceilings are opt-in through `BROKER_MIB` and `PG_GIB`, and only a run at scale 1.0 lets them fail the step; neither is set today, because a budget is read off these runs rather than asserted ahead of them |
| at load | `shapes.sh`, `saturate.sh` | throughput per tenant, the knee |
| subscriptions firing | `fire.sh` | update + delete streams over the loaded entities at 100, 200, 500, 1,000, 2,000 and 4,000 rps; every update fires each subscription of its tenant once, so the notifications due are known; the table shows due, delivered, the distinct subscriptions that fired, how long the sink kept receiving after the stream, failed operations by class (no HTTP answer / 4xx / 5xx) and the broker's own counters over the rate (changes the bounded matcher queue dropped, dead letters), so a delivery gap is attributed to the queue, the delivery policy or the receiver; the limit is the last rate that delivered 99% with no failed operation |
| per-class delivery | `fire.sh` → `fire-classes.md` | the subscriptions (10 000 by default, the `subs` dispatch input) fall into eight filter classes (type, q, watchedAttributes, idPattern, geoQ, scopeQ; `subs.md`) and every one is unique: p = k // tenants parametrises its q threshold, idPattern tail, polygon edge or scopeQ branch, and k6 evaluates the same rule, so due and delivered are reported per class |
| federated queries | `fed.sh` | five query shapes (type, q, geoQ, scopeQ, idPattern) on random tenants over the registrations (10 000 by default, the `regs` input; each with its own idPattern, polygon or scope) of eight classes (`csr.md`: mode, operations, csf properties, headers, expiry, location, scopes); every source is the sink; the row shows queries, failures, queries with a source warning, p99, source calls and calls per query |
| CPU and memory | `rss.sh` → `rss.csv`, `rss.md` | 1 Hz: broker and Postgres RSS, broker and Postgres CPU in cores, and whole-host busy cores against the core count — the saturation check; every fire.md / fed.md row carries the mean over its own window |
| PDF | `pdf.py` → `report.pdf` | the narrated report (what was stored, who called, the conditions of each number) built by `dev/perf/pdf.py`, next to `index.html` and `perf.json` in the downloadable results folder |

`dev/perf/sink.py` is the other end of every subscription and
registration: it counts notifications and answers forwarded queries with
an empty list, so the fan-out over 100,000 registrations costs the broker
the matching and the HTTP round trips and nothing else.

Every run is capped at one hour: the server's TTL, the job timeout and
the box's own shutdown timer agree on it. Scale `0.01` (1,000,000
entities, 100 tenants, 1,000 subscriptions, 1,000 registrations) fits with
margin and is what the schedule runs; `1.0` does not fit in an hour and
is a deliberate dispatch on a bigger box with the TTL raised in the
workflow. Bulk load bypasses the broker
(no notifications, no history), which is the documented path for initial
loads in [Operations](operations.md#bulk-load-postgres-timescale).

### The measured run

`scale-weekly` run `33863032274` on a ccx33, `SCALE=0.01` with the
subscription and registration counts raised to 10,000 each: 1,000,000
entities over 100 tenants. The load took 367 s for the entities (one COPY
stream), 8 s for the subscriptions and 159 s for the registrations.

| store | ready in (median of 5) | RSS after start |
|---|---|---|
| memory | 39 ms | 18 MiB |
| file | 51 ms | 19 MiB |
| postgres | 358 ms | 59 MiB |

| store | shape | concurrency | req/s | p99 |
|---|---|---|---|---|
| memory | query | c50 | 7 431 | 19.97 ms |
| memory | query | c200 | 7 821 | 70.19 ms |
| memory | retrieve | c50 | 29 913 | 7.47 ms |
| postgres | query | c50 | 912 | 61.37 ms |
| postgres | query | c200 | 933 | 207.85 ms |
| postgres | retrieve | c50 | 1 841 | 29.08 ms |

| store | shape | knee (rps held) | p99 at knee | first failing stage | broker cores | peak threads |
|---|---|---|---|---|---|---|
| postgres | query | 3 000 | 10.5 ms | 3 500 | 2.56 | 13 |
| postgres | write | 1 000 | 5.2 ms | 1 500 | 1.64 | 13 |

Subscriptions firing, over 10,000 subscriptions on 101 tenants:

| rate (rps) | due | delivered | delivered % | POSTs/s | dropped by broker | dead letters | PATCH p99 | broker cores | host busy |
|---|---|---|---|---|---|---|---|---|---|
| 100 | 96 492 | 96 492 | 100.0 | 1 637.1 | 0 | 0 | 25.2 ms | 1.6 | 3.5 |
| 200 | 193 406 | 193 406 | 100.0 | 3 250.8 | 0 | 0 | 28.1 ms | 3.0 | 6.0 |
| 500 | 481 526 | 40 330 | 8.4 | 561.3 | 26 398 | 0 | 1 184.1 ms | 4.5 | 7.1 |

Against run `33835261405`, the same shape on the same server type before the
per-drain `@context` memo landed:

| rate (rps) | delivered % | POSTs/s | dropped | PATCH p99 | quiet after |
|---|---|---|---|---|---|
| 200 before | 99.9 | 2 742.7 | 14 | 69.6 ms | 4 s |
| 200 after | 100.0 | 3 250.8 | 0 | 28.1 ms | 0 s |
| 500 before | 12.7 | 847.9 | 25 359 | 645.7 ms | 4 s |
| 500 after | 8.4 | 561.3 | 26 398 | 1 184.1 ms | 2 s |

Federated queries, over 10,000 registrations, every source the sink:

| rate (rps) | queries | failed (conn/4xx/5xx) | with a source warning | GET p99 | source calls | calls per query | broker cores | host busy |
|---|---|---|---|---|---|---|---|---|
| 50 | 1 487 | 0 (0/0/0) | 0 | 1 821.5 ms | 50 675 | 34.08 | 1.8 | 5.9 |
| 100 | 2 346 | 0 (0/0/0) | 0 | 12 697.0 ms | 80 502 | 34.31 | 2.2 | 7.9 |
| 200 | 3 615 | 0 (0/0/0) | 0 | 33 731.2 ms | 130 096 | 35.99 | 2.2 | 7.9 |
| 500 | 1 928 | 0 (0/0/0) | 0 | 53 754.9 ms | 159 943 | 82.96 | 1.9 | 8.0 |

Resident set and CPU over the whole run, 1 085 samples about 1.3 s apart:

| measure | value | ceiling |
|---|---|---|
| broker RSS peak | 3 181 MiB | no ceiling set |
| Postgres RSS peak | 11.86 GiB | no ceiling set |
| broker CPU peak / mean | 5.3 / 1.1 cores | of 8 |
| Postgres CPU peak / mean | 6.5 / 2.1 cores | of 8 |
| host busy peak / mean | 8.0 / 4.7 cores | of 8: saturated when peak ≈ 8 |

What the run says:

- Delivery is exact to 200 rps of writes: all 193 406 due notifications
  arrive, nothing is dropped, and the sink goes quiet in the same second the
  writes stop. The run before the memo delivered 99.9 % of the same shape,
  dropped 14 changes and took four seconds to drain. Write latency at that
  rate falls with it, 69.6 ms to 28.1 ms at the 99th percentile, because the
  matcher drain is no longer competing with the write path for a core.
- At 500 rps both runs are past the knee the run itself reports, and the
  newer one is further past it: the drop count barely moves (25 359 to
  26 398) but what survives is delivered more slowly (848 to 561 POSTs/s).
  Cheaper matching feeds the delivery stage faster, so the queue behind it
  fills sooner. The limit line reads 200 rps in both runs; what changed is
  that 200 rps is now met exactly instead of nearly.
- The registry narrowing holds. Each tenant carries 100 registrations and a
  `type=Vehicle` query reaches 34 of them — the three Vehicle-typed classes
  of the eight in `csr.md` — so the index decides the fan-out and the
  forward path is not a broadcast. The ratio is flat from 50 to 200 rps.
- The distributed path returns no 5xx, no 4xx and no `NGSILD-Warning` over
  9 376 queries: every one of the 421 216 source calls was answered and
  folded in.
- Where the broker bends, it is not out of CPU. At the Postgres query knee
  it holds 3 000 rps on 2.56 of 8 cores and at the write knee 1 000 rps on
  1.64; on the federated path it sits at 2.2 cores while Postgres takes 5.1
  and the host runs out at 7.9. The component that saturates is the
  database or the machine, never the broker alone.
- A federated query costs about eight times the Postgres work of a direct
  query of the same shape (42.5 against 5.5 mcores). The fan-out is 34
  source calls and a registry match over 10,000 rows; the direct row is one
  local read, so the two are not the same unit of work and the ratio is the
  price of federation, not a regression against a like-for-like baseline.
  The broker side of that comparison is not available from this run. The
  sampler matched broker processes against the whole of the first broker's
  `argv[0]`, and the `shapes`, `saturate` and `startup` stages start their
  own brokers from the same binary by a different path form, so it counted
  none of them and reported the idle first process instead — zero cores
  through windows serving 912 req/s. `saturate.md` disagrees with `rss.csv`
  for the same window (2.56 cores against zero) because that stage measures
  the process it started itself. The sampler now matches on the last two
  path components, so a later run carries a broker CPU column for every
  stage; the rows above are left as they were measured.

## Soak (`soak`)

`dev/soak.py` holds one broker for hours at a fixed low rate: 4 workers at
2 cycles a second each. Every worker owns its Entities, so the expected
state is known exactly. A cycle creates an Entity, PATCHes it, reads it
back, counts the worker's live Entities, batch creates, updates and deletes
three more, creates and deletes a throwaway Subscription, and deletes the
Entity. A Subscription per worker makes each PATCH one notification due at
the script's own receiver. The run fails on:

- an answer other than the operation's own status;
- a read-back or a count off what was written;
- a notification not received;
- RssAnon growing more than max(25 %, 16 MiB) past its value at the end of
  the warm-up (a tenth of the run);
- an ERROR or a panic in the broker log.

`soak.yml` runs it per store (memory, file, postgres) on a hosted runner,
120 minutes by default. `full.yml` calls it on `v*` tags and by hand, and
it runs by itself when the soak changes. `python3 dev/soak.py --selftest`
plants a failure in each judge.

## How each measurement works

No rig script is a unit test. Every one starts a real broker, drives it
over HTTP, and folds what came back into one Markdown table. The pieces
are the same in all of them:

```text
  dev/perf/<script>.sh
        │
        ├── starts ──► antares  (release binary, one store)  :9090
        │                 │
        │                 └── store: memory | file | postgres (docker)
        │
        ├── drives ───► k6 (a dev/perf/k6-*.js scenario)  or  python3
        │                 └── constant-arrival-rate: offered load, not
        │                     closed-loop, so a slow broker shows up as a
        │                     growing queue instead of a slower client
        │
        ├── receives ─► dev/perf/sink.py  :9800
        │                 ├── POST /…      one notification, counted
        │                 └── GET  /csr/k  one forwarded query, answered []
        │
        └── samples ──► dev/perf/rss.sh    1 Hz → rss.csv
                          reads /proc for broker, Postgres, k6, sink, host
```

`$OUT/phase` is the thread that ties them together: each script writes the
stage name into it before it starts, and `rss.sh` copies that string into
every sample it takes, so any row of `rss.csv` can be attributed to the
stage that caused it.

### `startup.sh` — how long a cold broker takes to answer

Runs the binary, polls `GET /q/health` until the first `200`, records the
elapsed time and reads `VmRSS` out of `/proc/<pid>/status` immediately
after. Five times per store, median reported. Nothing else is running, so
the number is the process itself: binary load, config parse, store open,
listener bind.

### `shapes.sh` — throughput per request shape

Starts one broker, seeds 100 five-attribute entities through the API, then
runs `k6-shapes.js` closed-loop at a fixed number of clients: a list query
at 50 and 200, a single retrieve at 50. Three runs of five seconds, median
reported, p99 from the same runs. Closed-loop on purpose — this table
answers "how fast is one shape", not "where does it break".

### `core-scale.sh` — does the broker use the cores it is given?

Pins the broker to 1, 2, 4 and 8 physical cores with `taskset` (SMT
siblings excluded) and keeps k6 on the cores left over, refusing any step
where the two would share one. The in-memory store on purpose: the
question is whether the broker's own work parallelises, so nothing waits
on a database.

### `saturate.sh` — where the knee is

Open model. `k6-saturate.js` raises the arrival rate by 500 rps every
stage and keeps going until p99 passes `P99_MS` or the error rate passes
`ERR`. The knee is the last stage that held both. A run that never fails a
stage reports `none reached`, which means the ladder is shorter than the
box — the answer is more stages, not a bigger conclusion.

### `load.sh` — building the dataset

The only stage that does not go through k6:

```text
load.sh
  ├─ sink.py 9800 8        one front door, eight worker processes behind it
  │                        (one CPython process tops out near 5 000 req/s)
  ├─ gen.py | bulk-load.sh entities, one COPY stream straight into Postgres,
  │                        bypassing the broker: no notifications, no history
  ├─ api-load.py subscriptions   through the API, eight filter classes
  └─ api-load.py registrations   through the API, eight CSR classes
```

Entities bypass the broker because a hundred million of them through the
API would measure the loader. Subscriptions and registrations do not: they
have to pass validation and land in the matcher's index, which is what the
later stages exercise.

### `fire.sh` — do the subscriptions fire, and up to what rate

```text
k6-fire.js                broker :9090                    sink.py :9800
    │                          │                                 │
    │ PATCH …/attrs (update)   │                                 │
    │ DELETE …/entities/{id}   │                                 │
    ├─────────────────────────►│                                 │
    │              204         │ 1. write the change              │
    │◄─────────────────────────┤ 2. match it against this         │
    │                          │    tenant's subscriptions        │
    │                          │ 3. queue it (changeQueue, 1024)  │
    │                          │ 4. deliver, deliveryWidth (64)   │
    │                          │    in flight, 8 per tenant       │
    │                          │                                  │
    │                          │  POST /  {"data": [entity, …]}   │
    │                          ├─────────────────────────────────►│
    │                          │                          204     │
    │                          │◄─────────────────────────────────┤
    │                          │                                  │
    └── k6 stops ──────────────┴── fire.sh polls /stats until the │
                                   count stops moving for 5 s     │
```

The count due is not measured, it is derived: `k6-fire.js` knows which
subscription classes each write should trigger and evaluates the same rule
`api-load.py` used to create them, so `delivered / due` is a real ratio and
not a guess. `quiet after` is how long the sink kept receiving once the
stream stopped — the drain. `dropped by broker` comes from the broker's own
`changesDropped` counter, read from `/q/health` before and after, so a
delivery gap can be charged to the queue, to the delivery policy or to the
receiver rather than left ambiguous.

The limit is the last rate that delivered 99 % with no failed operation,
and the ladder stops at the first rate that misses it.

### `fed.sh` — federated queries over the registrations

The registrations point at `sink.py`, which answers every forwarded query
with an empty array. That is deliberate: an empty answer costs the broker
the index lookup, the fan-out and the HTTP round trips and nothing else,
so the row measures the federation machinery rather than a source.

```text
k6-fed.js                    broker :9090                        sink.py :9800
    │                             │                                      │
    │ GET /entities?type=Vehicle  │                                      │
    │ NGSILD-Tenant: t42          │                                      │
    ├────────────────────────────►│                                      │
    │                             │ 1. expand the query against @context │
    │                             │                                      │
    │                             │ 2. csource_index lookup (5.12): one  │
    │                             │    SQL query, narrowing on entity     │
    │                             │    type and entity id alone. It may  │
    │                             │    only REMOVE registrations the     │
    │                             │    matcher would reject anyway, so   │
    │                             │    a NULL dimension always survives  │
    │                             │                                      │
    │                             │ 3. the matcher decides the rest in   │
    │                             │    Rust: geoQ, scopeQ, csf, the      │
    │                             │    intervals, the idPattern regex,   │
    │                             │    the Via chain                     │
    │                             │                                      │
    │                             │ 4. fold registrations naming the     │
    │                             │    same source into one request      │
    │                             │    (5.2.9: same endpoint, mode,      │
    │                             │    tenant, alias, headers, localOnly)│
    │                             │                                      │
    │                             │ 5. narrow each forward to what its   │
    │                             │    registration declares (4.3.6.1)   │
    │                             │                                      │
    │                             │ 6. fan out, ANTARES_FED_FANOUT (8)   │
    │                             │    in flight, each bounded by the    │
    │                             │    registration's timeout            │
    │                             │                                      │
    │                             │  GET /csr/17/entities?type=…&attrs=… │
    │                             │  Via: 1.1 broker-a                   │
    │                             ├─────────────────────────────────────►│
    │                             │                        200  []       │
    │                             │◄─────────────────────────────────────┤
    │                             │      × N per query (`calls per       │
    │                             │        query` in the table)          │
    │                             │                                      │
    │                             │ 7. book the outcome on each          │
    │                             │    registration: timesSent,          │
    │                             │    timesFailed, lastSuccess,         │
    │                             │    lastFailure, status               │
    │                             │    (Table 5.2.9-2)                   │
    │                             │                                      │
    │                             │ 8. merge the halves (4.5.5),         │
    │                             │    paginate, answer                  │
    │       200 + entity list     │                                      │
    │◄────────────────────────────┤                                      │
```

`calls per query` is the number that matters: it is the fan-out the
registry narrowing left behind, and it multiplies everything downstream. A
query that reaches 34 sources costs 34 HTTP round trips, 34 timeout
budgets and 34 bookkeeping writes.

Step 7 is where the federated path meets the database, once per source
call rather than once per query:

```text
one forwarded request
      │
      └─► note_forward (federation.rs)
             └─► CurrentStateDriver::record_forward
                    └─► record_forward_via_mutate (antares-store)
                           └─► PgDocStore::mutate(Kind::Registration)
                                  ├─ SELECT … FOR UPDATE   the row lock
                                  ├─ UPDATE csource_registrations
                                  └─ csource_index: rebuilt only when the
                                     write moved a member the index is
                                     built from — the counters are not
```

The `failed (conn/4xx/5xx)` and `with a source warning` columns are split
because they are different faults: a failed query is the broker not
answering, while a warning is the broker answering after a source did not
(6.3.17), which is the documented outcome and not an error.

### `rss.sh` — the CPU and memory column on every other table

One background sampler for the whole run. It resolves the broker by the
last two components of its `argv[0]`, the Postgres container by name, and
k6, the sink and mosquitto by their own names, then writes RSS and CPU for
each at 1 Hz along with whole-host busy cores. Every `fire.md` and `fed.md`
row folds the samples that fall inside its own window, which is why a
saturation claim can be checked against the phase that made it.

### `report.py` — the artefacts

Folds every table into `index.html`, `perf.json` and invokes `dev/perf/pdf.py`
to build `report.pdf` (the narrated report explaining what was stored, who
called, and the conditions of each measured number), next to the raw CSVs,
so a later run can be diffed against this one without rerunning anything.

## Measuring delivery without the rented runner

`fire.sh` needs k6, so until now the notification pipeline could only be
measured by dispatching `scale-weekly`. `dev/perf/deliver.py` drives the
same path with the standard library alone, against a memory-store broker
and `sink.py`:

```bash
ANTARES_STORE=memory ANTARES_EGRESS_ALLOW_PRIVATE=true antares &
python3 dev/perf/sink.py 9800 8 &
python3 dev/perf/deliver.py --seed --tenants 10 --entities 500
python3 dev/perf/api-load.py subscriptions --count 240 --tenants 10 \
  --sink-workers 8 --broker http://127.0.0.1:9090 --sink http://127.0.0.1:9800
python3 dev/perf/deliver.py --tenants 10 --entities 500 --rate 2000
```

The number to read is `matches_per_second`. One match is one
(subscription, entity) pair: it is what the matcher evaluates and what a
notification carries, so it is the unit both halves of the pipeline scale
with. `changes_per_second` divides that by how many subscriptions each
change fires, so it moves whenever the subscription set changes even
though the pipeline is doing identical work.

What it measures, on twelve cores with a release build, at an offered
2 000 changes per second over ten tenants and 500 entities:

| subscriptions over 10 tenants | matches/s | broker cores | changes dropped |
|---|---|---|---|
| 60 | 2 201 | 0.91 | 0 |
| 120 | 6 056 | 1.24 | 0 |
| 240 | 11 781 | 1.28 | 0 |
| 480 | 25 547 | 1.89 | 0 |
| 960 | 44 818 | 1.90 | 1 191 |

Matches per second rise with the subscription count, because a change is
evaluated against more subscriptions and fires more of them. The broker
carries the whole offered change rate up to 480 subscriptions over ten
tenants, and drops 0.15 per cent of it at 960.

Holding the subscription count at 240 and raising the offered rate
instead:

| offered changes/s | achieved | matches/s | broker cores | changes dropped |
|---|---|---|---|---|
| 2 000 | 2 000 | 12 074 | 1.72 | 0 |
| 3 000 | 3 001 | 17 998 | 2.18 | 0 |
| 4 000 | 4 004 | 23 948 | 2.48 | 0 |
| 6 000 | 5 131 | 30 683 | 2.74 | 0 |

The last row measures the harness rather than the broker: `deliver.py`
runs its writers as threads in one interpreter, and it cannot produce the
6 000 changes per second it was asked for. The broker absorbed every
change offered at every rate, dropped none of them, and held 2.74 of
twelve cores, so the table gives a floor for the delivery bound rather
than the bound itself. Reaching that bound needs the writers in separate
processes.

`ANTARES_DELIVERY_WIDTH` is not what governs this. Over a hundred-tenant
shape at 1 000 changes per second, widths of 8, 64, 256 and 1 024 deliver
7 026, 7 028, 7 026 and 7 028 matches per second, none of them dropping a
change: the offered load is absorbed whole at every width, so the knob
has nothing to arbitrate.

The same ladder against PostgreSQL, which is what a deployment runs
(`synchronous_commit=off`, 1 GiB of shared buffers, a pool of 50, the
database sharing the box with the broker, the harness and the sink):

| offered changes/s | achieved | matches/s | broker cores | changes dropped |
|---|---|---|---|---|
| 500 | 500 | 3 051 | 0.75 | 0 |
| 1 000 | 1 000 | 6 062 | 1.25 | 0 |
| 2 000 | 1 671 | 10 108 | 1.39 | 0 |
| 4 000 | 1 140 | 6 927 | 1.26 | 0 |

A change costs an entity update and a temporal-history insert here rather
than a map write, so the knee arrives near 1 671 changes per second
against the memory store's 7 800. Two things about that knee matter more
than its position. The broker holds 1.39 cores at it while PostgreSQL
holds 2.29 on average and peaks at 4.12, so the broker is not the
component that runs out — which is why a criterion written as broker core
utilization at saturation cannot be met on a rig that shares one box.
And goodput goes backwards past the knee: offering 4 000 changes per
second delivers less than offering 2 000, with broker CPU falling as it
does, because nothing sheds write load before it drives the database past
what it can commit. Neither is a property of the delivery path; both are
the write path underneath it.

## Setting up the rented runner (two repository secrets)

`perf-weekly` and `scale-weekly` rent a Hetzner Cloud server for the run
(a ccx33 and a ccx53; the design targets add a volume),
register it as an ephemeral GitHub runner, and delete it afterwards;
`perf-janitor` sweeps anything past its `expiry` label. Until the two
secrets below exist, both workflows stop at "Create server" with an empty
token, so the weekly runs stay red and no perf report is published.

### 1. `HCLOUD_TOKEN` — Hetzner Cloud API token

1. Sign in at <https://console.hetzner.cloud/>.
2. Create a project of its own for this (for example `antares-perf`) so the
   token cannot touch anything else, and set a spending alert on the
   project (Project → *Billing* → *Alerts*).
3. In that project open *Security* → *API tokens* → *Generate API token*.
   Name it `github-actions`, permission **Read & Write** (the workflow
   creates and deletes servers and volumes and reads the price list).
4. Copy the token once; Hetzner never shows it again.

### 2. `RUNNER_PAT` — GitHub token that can register a runner

The workflow asks the GitHub API for a runner registration token
(`POST /repos/<owner>/<repo>/actions/runners/registration-token`), which the
default `GITHUB_TOKEN` is not allowed to do.

1. GitHub → *Settings* (your account) → *Developer settings* →
   *Personal access tokens* → *Fine-grained tokens* → *Generate new token*.
2. Resource owner: the account that owns this repository. Repository
   access: **Only select repositories** → this repository.
3. Repository permissions: **Administration: Read and write**. Nothing
   else.
4. Expiration: one year is the maximum; put the renewal date in your
   calendar, the workflow fails with HTTP 401 when it lapses.
5. Copy the token once.

A classic token with the `repo` scope works too, but grants far more than
the workflow needs.

### 3. Store both as repository secrets

Repository → *Settings* → *Secrets and variables* → *Actions* →
*New repository secret*, twice:

| Name | Value |
|------|-------|
| `HCLOUD_TOKEN` | the Hetzner token from step 1 |
| `RUNNER_PAT` | the GitHub token from step 2 |

The names must match exactly; the workflows read them as
`${{ secrets.HCLOUD_TOKEN }}` and `${{ secrets.RUNNER_PAT }}`.

### 4. Limits on a fresh Hetzner project

A new project starts with small quotas. `perf-weekly` asks for a ccx33
(8 dedicated cores) and `scale-weekly` for a ccx53 (32);
`scale-weekly` at 1.0 would add a 500 GB volume, and Hetzner answers
`dedicated core limit exceeded` / `volumes size limit exceeded` until the
limits are raised: Project → *Limits* → request more dedicated cores and
volume storage (a short form, usually approved within a day). The 0.01
dry run needs the core limit only; it shrinks the volume to 10 GB.

### 5. First run

Actions → *scale-weekly* → *Run workflow* with `scale` = `0.01` (about an
hour, a few euros); read the step summary for the cost line and the
tables. Then Actions → *perf-weekly* → *Run workflow*. Both schedules take
over from there (perf-weekly Friday, scale-weekly Thursday, 06:17 UTC), and `pages`
folds the newest bundles into `/reports/perf/latest/`.

If you would rather not rent hardware, disable the two workflows (Actions
→ workflow → *⋯* → *Disable workflow*) so the weekly runs stop going red.

## The perf gate

Two runs fail on a regression against a baseline committed in
`ci/perf-baseline/` (T-4853). `dev/perf/gate.py` compares; `gate.py
selftest` checks the comparison itself and runs first in both.

| run | what it measures | baseline | fails when |
|---|---|---|---|
| `bench-surfaces`, nightly and weekly on a GitHub runner | `cargo bench -p antares-sql --bench core_paths`: `q`, type, `geoQ` and `scopeQ` parsing, the SQL each compiles to, JSON-LD compaction of a 100-entity page | `bench-core.json`, per CPU model | a case's p50, read as a ratio to a fixed CPU loop measured right before it, is more than 15 % above the baseline's ratio |
| `perf-weekly`, on the pinned Hetzner box | every row of `shapes.sh`, memory and (with `store: postgres`) PostgreSQL | `shapes.json`, per instance type | a row's p99 is more than 15 % up or its req/s more than 15 % down |

Both fail on a baseline row the run no longer produces. A CPU model or
instance type with no baseline is reported and not gated: its first run
prints the rows, and committing them is how a new runner joins the gate.
The ratio is what makes a shared runner usable for the bench: four runs of
the same commit on one box moved each case by at most 6.3 %, and a case
slowed on purpose (a 1 ns sleep in `ql_parse_q`, about 50 µs) failed the
gate at +6 315 %.

Accepting a change that moves a number is a reviewed commit of the
baseline:

```bash
BENCH_JSON=/tmp/core.json cargo bench -p antares-sql --bench core_paths
python3 dev/perf/gate.py bench /tmp/core.json ci/perf-baseline/bench-core.json --write
python3 dev/perf/gate.py shapes results/perf/shapes.md ci/perf-baseline/shapes.json --machine ccx33 --write
```

Each run's tables are committed to the `perf-history` branch by the
workflow's `history` job (`dev/perf/commit-history.sh`), under
`runs/<time>-<run>-<commit>/`, so a number this book cites does not expire
with the Actions artifact. `perf-weekly` stays dispatched by hand: it rents
the Hetzner box, and its schedule line is commented out until the owner
turns it on.

## Vanilla against the platform build

The joinedcontext platform runs Antares built with
`full,jc-policy,linkml,parquet,tabular,ical` and `ANTARES_POLICY=joinedcontext`.
`dev/perf/platform-overhead.sh` runs the same `shapes.sh` rows three times on
one machine: the vanilla binary; the platform binary as vanilla runs it
(`allow-all`, no view); and the platform binary under the policy engine,
with a rule document of 20 permissions, 12 Endpoint views loaded, and every
request carrying the `x-jc-subject` and `x-jc-roles` headers the gateway
sets. The caller's permission names the Vehicle type and its five
Attributes, so each read is narrowed as a platform read is. 1 000 Vehicles,
50 clients, five seconds, median of three runs (T-4853):

| store | shape | clients | vanilla req/s, p99 | platform build | change req/s, p99 | + policy and views | change req/s, p99 |
|---|---|---|---|---|---|---|---|
| memory | query | c50 | 19474, 28.50 ms | 18054, 12.18 ms | -7%, -57% | 14096, 13.10 ms | -28%, -54% |
| memory | query-q | c50 | 13269, 30.52 ms | 13198, 23.45 ms | -1%, -23% | 9171, 17.28 ms | -31%, -43% |
| memory | query-deep | c50 | 14120, 28.78 ms | 14152, 24.29 ms | +0%, -16% | 12615, 12.26 ms | -11%, -57% |
| memory | query-regex | c50 | 14434, 32.21 ms | 14634, 22.91 ms | +1%, -29% | 7231, 19.95 ms | -50%, -38% |
| memory | retrieve | c50 | 42385, 18.93 ms | 29588, 7.46 ms | -30%, -61% | 32604, 6.42 ms | -23%, -66% |
| memory | update | c50 | 16412, 7.44 ms | 14997, 8.15 ms | -9%, +10% | 14030, 8.97 ms | -15%, +21% |
| memory | hfp | c50 | 4303, 20.07 ms | 4178, 19.87 ms | -3%, -1% | 4245, 19.56 ms | -1%, -3% |
| postgres | query | c50 | 4369, 25.20 ms | 4116, 31.90 ms | -6%, +27% | 2470, 52.58 ms | -43%, +109% |
| postgres | query-q | c50 | 2863, 35.96 ms | 2358, 43.40 ms | -18%, +21% | 1631, 54.02 ms | -43%, +50% |
| postgres | query-deep | c50 | 1276, 65.85 ms | 1178, 75.42 ms | -8%, +15% | 1160, 69.14 ms | -9%, +5% |
| postgres | query-regex | c50 | 345, 204.06 ms | 330, 340.20 ms | -4%, +67% | 1198, 66.38 ms | +247%, -67% |
| postgres | retrieve | c50 | 7372, 25.12 ms | 6918, 16.42 ms | -6%, -35% | 7449, 14.78 ms | +1%, -41% |
| postgres | update | c50 | 4473, 19.84 ms | 4525, 19.80 ms | +1%, -0% | 2705, 36.10 ms | -40%, +82% |
| postgres | hfp | c50 | 2511, 28.54 ms | 2581, 25.84 ms | +3%, -9% | 2074, 37.47 ms | -17%, +31% |

Measured 2026-10-21 on the shared 16-vCPU AMD EPYC Genoa sandbox, load
average 20 to 45 from other jobs: read the req/s columns; the p99 columns
swing by half on that host and decide nothing. What the table shows:

- The build costs little: a request of the NGSI-LD API runs the same code
  as in vanilla, and most rows move −1 % to −9 % req/s. Memory `retrieve`
  (−30 %) and PostgreSQL `query-q` (−18 %) are the rows to measure again on
  the pinned box before reading them as a cost.
- The policy engine costs reads 23 % to 50 % of req/s on the memory store
  and 43 % on PostgreSQL `query` and `query-q`: each read is decided, then
  conjoined with the permission's rule and projected to its Attributes.
  Writes by id cost 15 % on memory and 17 % to 40 % on PostgreSQL.
- PostgreSQL `query-regex` runs 3.5 times faster under the policy than in
  vanilla (1 198 against 345 req/s). This run does not trace why; the
  vanilla `~=` path is the one T-4907 pushes down to PostgreSQL.

Reproduce:

```bash
cargo build --release -p antares-broker && cp target/release/antares /tmp/antares-vanilla
cargo build --release -p antares-broker --features full,jc-policy,linkml,parquet,tabular,ical
ENTITIES=1000 DATABASE_URL=postgres://… BIN_VANILLA=/tmp/antares-vanilla \
  BIN_PLATFORM=target/release/antares dev/perf/platform-overhead.sh
```

## What a façade costs

A façade for another standard answers by driving this broker's own NGSI-LD
router in process ([Façades for another
standard](extending.md#façades-for-another-standard)). The seam's own cost
is a JSON round trip: the inner answer is serialized to bytes, parsed, and
re-serialized into the façade's envelope. Everything else about the request
happens exactly once.

Measured with the reference façade (`GET /x/example/things?kind=Vehicle`)
against the NGSI-LD request it wraps
(`GET /entities?type=Vehicle&options=keyValues`), both through the same
router in the same process, 200 calls each, medians of five repetitions,
release build:

| answer | façade | the request it wraps | the round trip |
|---|---|---|---|
| 100 five-attribute Entities | 265-297 µs | 199-226 µs | **66-71 µs** |
| nothing matched | 112-127 µs | 107-157 µs | **under 10 µs** |

The comparison is in process on purpose: the number is about a serialize
and a parse, and a socket between the two halves would measure the socket.
`dev/perf/shapes.sh` runs the same pair end to end against a built binary
(the `facade` shape, skipped unless the binary was built with the reference
plugin); the table above is the per-call figure.

What it decides: the seam has almost no fixed cost — an empty answer's
round trip does not clear the noise — and what it does cost is
proportional to the answer, about 0.7 µs per Entity on this shape. A typed
operations layer, one where a façade reached the handlers through Rust
types instead of through JSON, would save exactly that and nothing else.
Sixty-six microseconds on a hundred-Entity page is not a reason to build
and maintain a second, typed API surface beside the HTTP one, so that box
stays closed until a façade measures this as its ceiling rather than as its
rounding error.

## Query plans under row-level security

The broker connects as a role without BYPASSRLS, so every read runs under
the tables' forced RLS. Postgres then takes an operator into an index
condition only when it is leakproof, and no PostGIS operator (`&&`,
`ST_*`), no jsonb operator (`@?`, `?|`, `@>`) and no array operator (`@>`,
`&&`) is: the GIST on `location` and the GIN indexes on `entity` and
`types` never serve the broker, whatever a superuser `EXPLAIN` shows.
Btree comparisons of text, numbers and timestamps are leakproof. T-4852
moved every shape the broker pushes down onto those:

- `type=A,B` and `type=A;B` read one `types[1] = T` arm per named type
  through `i_entities_first_type_id` plus one arm over the multi-type
  Entities (`i_entities_multi_type_id`), as one named type has since T-4781;
- a geoquery ANDs a geohash cover of its region (at most eight cells and
  their prefixes) over `location_cell`, the generated cell of each
  Entity's bounding box (migration 0015), through
  `i_entities_location_cell`; the PostGIS predicate still filters and the
  evaluator still decides;
- the temporal `type=` reads `types[1] = ANY(...)` and the multi-type
  Entities through two new `temporal_entities` indexes instead of `&&`.

`q` and `attrs` have no leakproof form of their own: the jsonb operators
are not, and neither are numeric's `=` and `<`. Since T-5303 the store
keys them in `entity_values` (migration 0024): per Entity and Attribute
each number as float8 and each string up to 1 024 bytes as
`text COLLATE "C"`, kept by statement triggers on `entities`. A top-level
`q` term (`==` a number, a string or a numeric range; `<`, `<=`, `>`, `>=`
a number) or `attrs=` first probes those btrees for at most 1 000 Entity
ids, and the page ANDs `id = ANY(...)` to its WHERE; the `@?` or `?|`
stays and decides. A term more than 1 000 Entities of the Tenant match
gets no id list and reads the type's rows as before, plus the probe
(about 1 ms). A planner-joined `id IN (SELECT ...)` was measured and
dropped: the planner takes `@?` and `?|` for selective and joined every
candidate on an unselective page (`attrs` of a 10 000-Entity Tenant
1.6 -> 11 ms).

The keys cost writes. Same box, 100 000 Entities of five Attributes in
one INSERT: 5.5 s without the triggers, 18.7 s with them; a one-Entity
INSERT 0.68 -> 0.86 ms and UPDATE 0.84 -> 1.75 ms (`pgbench`, one client,
2 000 runs). The table and its three btrees take up to as much room as
`entities` does.

The q-equality row asks for `speed == 42`, which no Entity of the type
holds: the worst case, all of the type's rows read. At 1M, 10 000
Entities of the Tenant hold 42, past the probe's 1 000, so that row keeps
its cost: a `q` whose value is common in the Tenant and rare in the type
still reads the type (ponytail: a `types[1]` column in `entity_values`
would key it, at another index per write). `q-range-all` and
`q-all-untyped` match every Entity: the probe's cost on a page it cannot
narrow.

`dev/rls-plan-bench.sh <url> <N> [R] [R_BEFORE]` seeds two Tenants of N
Entities each (the second one is there so that a walk of the table is not
also a walk of the Tenant), runs each shape R times through `pgbench` as
`antares_app`, and prints p50, p99 and the plan's scan nodes. The
statements are the store's own, the "before" rows are their shape before
T-4852. `crates/antares-sql/tests/pg_explain.rs` asserts the same plans at
100 000 Entities on every test run. Box: the 16-core sandbox host,
PostGIS 17.5-3.5 in a container with its defaults (`shared_buffers`
128 MB, `work_mem` 4 MB), one client, R = 200, R_BEFORE = 20 at 1M.
The q and attrs rows are T-5303's (2026-10-23, same box and image): "before"
without the `entity_values` probe, "after" the probe plus the page.
`q-range` asks `> 98` (1 % of the Tenant); the `-untyped` rows carry no
`type`, `attrs-untyped` names an Attribute no Entity has, `q-range-all`
and `q-all-untyped` ask `>= 0`, which every Entity holds.

| shape | form | 10k p50 / p99 ms | 100k p50 / p99 ms | 1M p50 / p99 ms | scan nodes at 1M |
|---|---|---|---|---|---|
| id | same | 0.14 / 0.22 | 0.14 / 0.22 | 0.45 / 0.64 | Index Scan entities_pkey |
| type | same | 0.34 / 0.62 | 0.78 / 0.96 | 5.05 / 14.16 | Index Scan i_entities_first_type_id, Index Scan i_entities_multi_type_id |
| type=A,B | before | 0.67 / 0.96 | 6.81 / 9.99 | 319.24 / 339.54 | Index Scan entities_pkey |
| type=A,B | after | 0.48 / 2.00 | 1.03 / 1.38 | 6.63 / 15.25 | Index Scan i_entities_first_type_id, Index Scan i_entities_multi_type_id |
| q-equality | before | 1.30 / 2.05 | 10.12 / 21.62 | 511.62 / 3114.95 | Index Scan i_entities_first_type_id, Index Scan i_entities_multi_type_id |
| q-equality | after | 0.85 / 1.42 | 4.78 / 8.18 | 474.63 / 792.37 | Index Scan i_entity_values_num (past 1 000: no ids), Index Scan i_entities_first_type_id, Index Scan i_entities_multi_type_id |
| q-range | before | 1.30 / 2.08 | 11.20 / 28.00 | 468.58 / 584.63 | Index Scan i_entities_first_type_id, Index Scan i_entities_multi_type_id |
| q-range | after | 0.99 / 1.62 | 13.88 / 25.29 | 498.20 / 745.78 | Bitmap Index Scan i_entity_values_num (past 1 000 from 100k on), Index Scan i_entities_first_type_id, Index Scan i_entities_multi_type_id |
| q-range-all | before | 1.84 / 2.57 | 1.45 / 2.86 | 12.91 / 20.11 | Index Scan i_entities_first_type_id, Index Scan i_entities_multi_type_id |
| q-range-all | after | 2.74 / 4.17 | 2.63 / 3.63 | 14.36 / 33.95 | Index Scan i_entity_values_num (no ids), Index Scan i_entities_first_type_id, Index Scan i_entities_multi_type_id |
| attrs | before | 1.49 / 3.66 | 1.25 / 1.76 | 8.30 / 14.41 | Index Scan i_entities_first_type_id, Index Scan i_entities_multi_type_id |
| attrs | after | 2.17 / 14.70 | 2.48 / 3.55 | 3.65 / 18.34 | Seq Scan entity_values (stops at 1 001 rows: no ids), Index Scan i_entities_first_type_id, Index Scan i_entities_multi_type_id |
| q-eq-untyped | before | 6.13 / 9.48 | 19.71 / 34.17 | 863.83 / 3621.25 | Index Scan entities_pkey |
| q-eq-untyped | after | 0.99 / 2.03 | 2.24 / 4.46 | 835.04 / 1152.29 | Index Scan i_entity_values_num, Index Scan entities_pkey (ids at 10k and 100k) |
| q-all-untyped | before | 10.13 / 13.80 | 0.18 / 1.14 | 0.19 / 0.24 | Index Scan entities_pkey |
| q-all-untyped | after | 11.74 / 15.91 | 1.25 / 1.98 | 0.77 / 4.42 | Index Scan i_entity_values_num (no ids), Index Scan entities_pkey |
| attrs-untyped | before | 3.21 / 3.82 | 67.59 / 120.80 | 3756.84 / 8989.36 | Index Scan entities_pkey |
| attrs-untyped | after | 0.30 / 0.54 | 0.29 / 0.38 | 0.31 / 0.44 | Index Scan i_entity_values_num |
| geo-near | before | 33.23 / 67.17 | 257.32 / 397.72 | 2823.12 / 2886.87 | Index Scan entities_pkey |
| geo-near | after | 0.28 / 0.41 | 0.34 / 0.80 | 0.72 / 1.12 | Bitmap Index Scan i_entities_loc_ambiguous, Bitmap Index Scan i_entities_location_cell |
| geo-within | before | 20.94 / 35.01 | 224.29 / 282.46 | 2005.86 / 2094.93 | Index Scan entities_pkey |
| geo-within | after | 0.23 / 0.30 | 0.34 / 1.08 | 0.57 / 1.54 | Bitmap Index Scan i_entities_loc_ambiguous, Bitmap Index Scan i_entities_location_cell |
| temporal-type | before | 17.58 / 78.73 | 21.83 / 25.59 | 22.33 / 38.19 | Index Only Scan attr_instances lookup btree, Index Scan temporal_entities_pkey |
| temporal-type | after | 14.20 / 68.98 | 21.55 / 30.18 | 22.12 / 32.94 | Index Only Scan attr_instances lookup btree, Index Scan temporal_entities_pkey |
| temporal-lastN | same | 0.11 / 0.19 | 0.30 / 0.46 | 0.34 / 0.53 | Index Scan attr_instances lookup btree |
| temporal-between | same | 0.15 / 0.23 | 0.42 / 0.64 | 0.45 / 0.70 | Index Scan Backward attr_instances lookup btree |

The paged temporal query is driven from the Entities that hold
instances (an index-only scan of `attr_instances`, then one primary-key
probe per Entity), so its time follows the instances, before and after.
The new indexes replace the Tenant read where the planner starts from
`temporal_entities` instead, which `pg_explain.rs` pins.

## Deep pages, the next cursor and regex filters

T-4907 measured a BBSK `Invoice` page at offset 5000 on dev at 3.8 s p50
against 0.5 s at offset 0, and `name~="zzz"` at 1.31 s against
0.39 s for `name=="zzz"`. Three changes in the store and the API:

- **Ids first.** A page past offset 0 skips its OFFSET over ids alone,
  which `i_entities_first_type_id` and `i_entities_multi_type_id` hand back
  as an index-only scan (their INCLUDE columns, 0013), then reads only the
  page's documents through `entities_pkey`. Before, OFFSET walked the
  jsonb documents it then threw away.
- **The next cursor.** An Entity query whose page is kept in id order and
  is not counted (no `count=true`, no `orderBy`, no federation, no
  `idPattern`) carries `cursor=` on its `rel="next"` Link beside `offset`.
  The next page then seeks `id >` the last id shown through the same
  indexes, so page 500 costs what page 1 costs. The cursor is opaque: the
  hex of the last id plus a SHA-256 tag over the view, the Tenant and every
  other parameter of the request, so a cursor sent with another view,
  Tenant, `q`, `limit` or `offset` is a 400 BadRequestData. It is no secret:
  a forged one only starts the same query at another id. A `prev` Link and
  a counted page keep plain `offset`, which the ETSI suite compares.
- **`~=` in Postgres.** A pattern that Postgres `like_regex` and the
  `regex` crate read the same way compiles into the jsonpath and is
  decided in SQL. That covers literals, `.`, `^`, `$`, `|`, groups,
  `*` `+` `?` and positive bracket lists, up to 128 characters. Anything
  else stays with the evaluator under its program caps: escapes
  (back-references are what makes Postgres backtrack), `{m,n}`, `[^…]`,
  POSIX classes, inline flags. `!~=` stays there too, because it needs
  every array element to miss. `pg_query_parity.rs` checks both arms
  against the evaluator on stored line breaks. There is no trigram index:
  `~` and pg_trgm's `%` are not leakproof (`pg_proc.proleakproof = f`), so
  under forced RLS a trigram index never serves the broker's role. The
  regex filters the type's rows inside Postgres instead of shipping them
  to the broker.

Local numbers, same box and seed as the table above (100 000 Entities per
Tenant, type T1 = 10 000), as `antares_app`, SQL execution time:

| shape | before | after |
|---|---|---|
| type page at offset 5000, limit 100 | 12-52 ms (OFFSET over documents) | 2.3 ms (Index Only Scan, 0 heap fetches) |
| type + `brand~="^zzz"` | 22 ms in SQL, then 10 000 documents decoded and evaluated in the broker | 10.6 ms, nothing shipped |

`pg_explain.rs` `deep_and_keyset_pages_read_ids_from_the_type_index` pins
both plans under RLS.

## 1 000 distinct queries a second

T-5062 asks for 1 000 requests a second for ten minutes, no two alike,
with p95 under 300 ms and errors under 0.1 %, on the share of the dev node
the broker and its PostgreSQL get. `tests/perf/distinct-1000/` is the rig;
`bench.yml` runs it on demand (`workflow_dispatch`, a runner label as
input).

- `seed.py` writes the 581 000 entities of `dev/perf/k6-surfaces.js`'s
  data set (ten types, a Point each, `level`, `category`) by COPY through
  `dev/bulk-load.sh`; `seed-extras.js` puts the view `load`, its LinkML
  schema and a 30-day pm25 history on 500 entities through the API. The
  API seed alone wrote 220 entities a second on this PostgreSQL, 45 min
  for the set.
- `queries.js` turns request number u into one request of the mix:
  retrieve 18 %, `q` on two attributes 14 % and on three 10 %, geo `near`
  10 % and `within` 8 %, a temporal window 8 %, OData `$filter` 10 %, OGC
  `bbox` 10 %, its CSV export 6 % and MCP `query_entities` 6 %. u rides in
  an id, a threshold's last digits or a coordinate's, so the 600 000
  requests of a run are distinct; `node queries.js` checks that.
- `run.sh` runs PostGIS in a container capped at 2 cores and 2 GiB, the
  platform binary (`full,jc-policy,linkml,parquet,tabular,ical`) pinned
  to 4 cores, as `antares_app` without BYPASSRLS (`ANTARES_REQUIRE_RLS=1`)
  under `ANTARES_POLICY=joinedcontext` with 20 permissions, every request
  carrying the subject headers the gateway sends. k6 sends from 64 loopback
  source addresses (`--local-ips`), so a per-address cap in front of the
  broker would see 64 callers.
- `distinct.js` is an open model (constant arrival rate) with the task's
  thresholds; the run writes k6's summary, the broker's and PostgreSQL's
  CPU and RSS, and the 15 costliest statements of `pg_stat_statements`.

`GATEWAY=1` puts the Context Gateway in front (T-5136): the platform image
by digest on 2 cores of its own, host network so it sees k6's 64 source
addresses, a manifest repository of one Context Space, one public Endpoint
and one Policy granting the role `public` the reads; the data then lives in
that space's Tenant and k6 asks every surface under
`/api/endpoint/{slug}/`. The gateway runs without an identity provider, so
its token check is not measured; its Endpoint lookup, Policy decision,
per-caller caps and proxying are. Box: the 16-core sandbox host, 3-minute
runs at 1 000 req/s on the seeded set; the generator shares the host.

| build | answered | req/s served | dropped arrivals | broker CPU | PostgreSQL CPU | costliest statement |
|---|---|---|---|---|---|---|
| dev `a60ea0d` | 9 % | 261 | 130 464 | 19 % | 194 % of 200 % | OData's `count(*)` per page: 802 calls, 2.4 s each, 80 % of PostgreSQL's time |
| + the two fixes below | 59 % | 277 | 127 363 | 27 % | 197 % of 200 % | `q` over a type's rows: 10 321 calls, 106 ms each |
| dev `5caec28` (T-5088's candidate walk) | 64 % | 259 | 130 706 | 29 % | 197 % of 200 % | the `q` page: 9 479 calls, 106 ms each, 21 rows each |
| + no parallel workers, 0018 (T-5136) | 80 % | 347 | 115 252 | 81 % | 195 % of 200 % | geo `near`/`within` pages: 28-63 ms each, about 1 000 rows read per page of `bbox` |
| the same, `GATEWAY=1` | 22 % | 218 | 0 | 74 % (gateway 19 %) | 200 % of 200 % | as above; the gateway answered 78 % with 503 at its 128 requests in flight |

The rig does not hold 1 000 distinct requests a second yet, nor 250
(64 % answered at 250 req/s): PostgreSQL's two cores are the wall, the
broker idles at a quarter of a core. Fixed so far, each with its own
before and after:

- **A view's geoquery reads the store's spatial index.** Under a view's
  rules the client's geoquery was judged on the shown Entity, so the store
  read every Entity of the type in doubling pages and the broker filtered
  them: an OGC `bbox`, an NGSI-LD `georel` under `/x/views/…`, an OData or
  MCP geo read. When every rule of the selected types shows `location`
  (and no rule hides it), the shown and the stored Entity answer the
  geoquery alike, so it goes to the store with the page, as T-4289 did for
  a client `q`. One `bbox` read on 1 666 Entities of a type: 5 statements
  and 1 620 rows before, 1 statement and the 27 matches after.
  `policy_filter_by_type.rs`
  `a_geoquery_on_a_shown_location_is_paged_in_the_store_and_a_hidden_one_is_not`
  pins both sides.
- **No count to find the next page.** OData without `$count` and MCP
  `query_entities` and temporal pages without `count` asked the broker for
  `count=true` on every page only to learn whether a next page exists.
  They now ask one Entity past the page; a page at the 1 000 ceiling still
  counts. At 1 000 req/s the count statements went from 80 % of
  PostgreSQL's time to none.

- **No parallel workers for a page** (T-5136). A `q` page of a type
  plans its `@?` as a filter under RLS (never an index condition) that the
  planner estimates selective, so it chose a Gather of two workers: two
  processes started for 21 rows. Idle, 14 ms with the workers and 2.5 ms
  without; at 1 000 req/s, 106 ms. Every pooled session now sets
  `max_parallel_workers_per_gather = 0`: the `q` page went to 11.7 ms.
  `pg_explain.rs` `a_pooled_session_starts_no_parallel_workers`.
- **A geoquery of one type reads that type's cells** (migration 0018). The
  cover over `(tenant_id, location_cell)` read every type in the cells and
  dropped the others after: 17 000-47 000 rows for a page of 20, 0.5-1.4 s
  a statement under load. `i_entities_type_location_cell` puts the type in
  the key: 28-63 ms. `pg_explain.rs` "geo near of a type".
- **`!location` reads only the Entities without a geometry** (0018). An OGC
  `bbox` page asks a second part for the Features without a geometry (C12);
  its negated `@?` read the whole type for no row (64 000 rows, 68 ms) on
  every `bbox` and CSV page. The store ANDs `location IS NULL` to a
  `!location` every match needs, leakproof and served by the partial
  `i_entities_type_unlocated`; the `@?` still decides.
  `pg_query_parity.rs` `a_missing_location_is_decided_exactly`.

Behind the gateway (`GATEWAY=1`) the broker serves what PostgreSQL
allows and the gateway refuses the rest at once: past 128 requests in
flight (`JC_GATEWAY_MAX_IN_FLIGHT`, its default) it answers 503 with a
retry hint in under a millisecond. The answered requests then keep a p95 of
396 ms where the broker alone queued them to 7.7 s, and the gateway uses a
fifth of a core at 1 000 req/s offered: the gateway is not the wall,
PostgreSQL's two cores are.

A side table of `q` keys kept by trigger, or leakproof wrappers, was the
plan for `q` (T-5088). Measured, the `q` page reads about 20 rows per
match (`level` spreads over the ids), and its cost was the parallel
workers, not a walk of the type; a side table would serve a `q` that
matches almost nothing (the walk then reads the whole type) at a write
cost on every Entity. Decided (worker-t5136, owner may override): no side
table now, because the measured cost is gone without one and the wall
moved to geo pages; options: a trigger-kept `(tenant, type, attr, value)`
table, LEAKPROOF wrappers (superuser-only on CNPG).

What holds the rig below its target, measured in the second run, in
order:

- **Geoqueries are never decided in SQL** (fixed in T-5088, rig rerun
  owed). `near` compares geography in PostGIS and haversine in the
  evaluator, and `location_ambiguous` rows need the evaluator, so the
  store dropped the page and read up to `MAX_UNDECIDED_ROWS` of the
  region: about 1 000 documents and 0.6-1.1 s per statement for a page of
  20 in the dense synthetic grid. A paged geoquery or `scopeQ` without
  `count` now reads its candidates through `query_candidates`: the store
  pushes the id-ordered page over what it cannot decide, and the API reads
  pages of its own, doubling up to 2 048, until the window and one match
  past it hold, as rules that are not by type already did; a walk past
  `ANTARES_DISCOVERY_SCAN_MAX` rows is refused. `bounded_reads_5_14_5_16.rs`
  `a_geoquery_page_reads_the_candidates_of_its_page`: page 3 of 2 reads 7
  of 12 candidates, where the flat read took all 12.
- **`q` has no leakproof index path** ([Query plans under row-level
  security](#query-plans-under-row-level-security)): `level>=x;level<y`
  reads the type's rows, 64 000 here, 106-164 ms a statement under load.

## Response compression

The broker binary compresses a response with gzip or br when the request's
`Accept-Encoding` asks for one of them (tower-http `CompressionLayer`,
`antares_api::compression` in `crates/antares-api/src/router.rs`, laid over
the router in `crates/antares-broker/src/main.rs`, T-5847). An in-process call
(`AppState::call`, a façade, a forward) runs the router without it, so it
always reads plain JSON and a façade chain spends no stack on the layer. An NGSI-LD page repeats its
attribute names in every entity: a 75 427 byte page of 100
`KeyPerformanceIndicator` entities packs to 1 578 bytes with gzip.

- A response is left plain when its size is known and is 32 bytes or less,
  when its type is `text/event-stream`, `image/*` or gRPC, or when it already
  carries `Content-Encoding` or `Content-Range`. A streamed entity list has no
  known size, so it is compressed whatever its length.
- Every response the layer judges gets `Vary: Accept-Encoding`, appended to
  any `Vary` the handler set.
- A request without `Accept-Encoding` gets the plain body, byte for byte what
  the decoded compressed body holds.
- Request bodies are never decompressed: a body sent with
  `Content-Encoding: gzip` is parsed as sent and refused with 400, so the
  4 MiB body bound counts the bytes the broker actually reads.
- Antares sets no cookie and reads none: every caller names itself in a
  header its own client adds, so a third-party page cannot make a browser send
  an authenticated request and measure the compressed length (BREACH). The
  broker issues no tokens or CSRF values, and its error bodies never echo a
  stored credential (4.3.6.5, `tests/csi_credential_4_3_6_5.rs`).

There is no switch: a client that wants plain bytes omits the header. A proxy
in front of the broker that rewrites response bodies either decodes first or
drops `Accept-Encoding` on its way up; the joinedcontext gateway drops it and
compresses toward its own client.

## What the numbers are not

They are one machine, one request shape, one week. A different instance
type invalidates the whole history, which is why both workflows pin one.
The gate ([The perf gate](#the-perf-gate)) compares a run with the
baseline of its own CPU model or instance type, never across them.
