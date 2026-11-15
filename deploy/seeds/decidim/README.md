# Decidim harvester: citizen participation as NGSI-LD (T-5065)

The towns' participation platforms (Decidim) as the space `slovensko-participacia` of the Slovakia
project: participatory processes and assemblies, proposals, participatory budget projects, results,
meetings, debates and surveys. Read anonymously, no login, no personal data.

## Files

| file | role |
|---|---|
| `instances.csv` | one row per Decidim instance: name, base URL, language, scope, town, provider |
| `pipeline.yaml` | the Bento mapping every instance runs, unchanged (comments in it explain each step) |
| `generate.py` | writes each row's DataSource, daily Pipeline, hourly budgets-only Pipeline, both mappings, the index lines and the catalogue source; `--check` fails on a stale file |
| `inventory.py` | every public Decidim instance of the decidim.org list: robots.txt, version, spaces, record counts by component (ids only), whether `author` reaches an anonymous client; plus the harvest's counts on dev and prod. `python3 -I seeds/decidim/inventory.py --out <dir>` (~40 min, 1 request/s per host; `--host <h>` for one instance) |
| `../../tests/test_decidim.py` | the mapping run in Bento against a stand-in Decidim (0.29 and 0.27 answers), the model's JSON Schema, robots.txt, the personal-data filter, the seed |

Add an instance: one row in `instances.csv`, then `python3 seeds/decidim/generate.py`. Nothing else
changes; the space, its model, Endpoints and Policy are shared.

The row `participacia` is the platform's own Decidim (T-6449), whose host is each environment's: its
files go to `components/decidim/seed/` with dev's host, which the forge seed's rewrites move, and
only an environment listing the `decidim` component loads them. Its one full
pass runs hourly, and the shared Endpoint's title and source list keep naming the towns alone.

## How a run reads an instance

1. `robots.txt` (a 5xx fails the run; `/api` disallowed for `*` or `joinedcontext-platform` fails it).
2. `{ decidim { version } }`; 0.28 and later add `withdrawn` to proposals and meetings.
3. `participatoryProcesses` and `assemblies`, then per space its components by type, then each
   component's records 100 a page by cursor.
4. A component whose records the API answers as `null` is read from its public list page instead
   (title, state label, support, votes, amount only).

Every request waits a second after the last one and carries the User-Agent
`joinedcontext-platform/1.0 (+https://dev.joinedcontext.com; public participation data)`. Every URL is
the row's base URL plus a path the mapping builds, never a link taken from an answer. The daily pass
reads everything; the hourly pass reads only the budgets of processes running that day, so vote counts
move within an hour during a vote. Each entity carries `expiresAt` three days ahead, so a record the
platform hides or deletes leaves within three days.

## Instances found (2026-10-21, inventory 2026-11-01 in T-6315)

| instance | Decidim | API | note |
|---|---|---|---|
| tvormespolu.banskabystrica.sk (Banská Bystrica, "Spoločne tvorme Banskú Bystricu") | 0.29.5 | public `/api` | 19 processes, 0 assemblies. Proposals and budget projects answer `null` to an anonymous client, so they come from the list pages. `/open-data/download` redirects to a blob that answers `NoSuchKey`. The old host spolupracovna.banskabystrica.sk redirects here. |
| Trnava ("Tvorme spoločne Trnavu") | n/a | n/a | not found: participacia.trnava.sk answers 404 on `/api`, tvormespolocnetrnavu.sk does not resolve. |
| bbsk.sk/participativne-planovanie | n/a | n/a | the region runs no Decidim. |
| decidim.spotter.cz (Czech, "Rozhodování") | 0.26.2 | public `/api` | 23 processes, 5 assemblies, 9 proposals, 21 debates; budgets answer `null`. Not harvested yet. |
| zapojsa.trencin.sk (Trenčín) | n/a | n/a | gone: the decidim.org list points at a 2023 web.archive.org copy. |

GLOBSEC's 2025 report on participatory democracy in Slovakia names Banská Bystrica as the Slovak town
on Decidim. A further instance goes in as a row once its host answers `{ decidim { version } }`.

## A new process takes two runs

The gateway looks up a relationship's target in the store, not in the batch it holds (`HeldWrite::check`), and answers a 207 that leaves the run green. The first run over a new process writes the process alone; the next run writes its proposals, projects, results and meetings. A Result under a new parent Result takes one run more. New records in a known process take one run.

## The derived dataset

`slovensko-pipeline-participacia-statistika.yaml` (T-6505) reads Proposal and ParticipatoryProcess through
the public Endpoint each hour, after the harvest, and writes one ProposalStatistic per process and state
into the same space. The Endpoint `slovensko-participacia-statistika` serves that one type under its own
Policy and publishes it to CKAN as links; the rows stay in the space. A state with no proposal left leaves
within three hours (`expiresAt`), so the counts equal the Proposal records once the proposals' own three-day
lease has passed. The state words are `OWN_STATE_LABELS` (a test keeps them equal).

## Not in the data, and why

- Authors, profiles, comments and endorsement lists: personal data, never asked for.
- A Result's linked proposals: Decidim's API has no field for them (0.27 to 0.29).
- Proposal coordinates and body on 0.29.5: the API hides the proposal, and the list page shows neither.
- Survey questions and answers: only the survey's title and link.
- A meeting's online link on 0.32: `onlineMeetingUrl` is non-null there and nulls a meeting without one, so it stays unread; the Meeting's `url` leads to the link.
- Meetings as calendar Events of the events store (T-5047): not done here; the Meeting entities carry
  `startDate`, `endDate`, `address` and `location`, ready for a later mapping.
