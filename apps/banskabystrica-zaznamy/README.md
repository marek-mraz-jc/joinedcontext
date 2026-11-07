# Záznamy mesta Banská Bystrica / Records of the city of Banská Bystrica

The city's published statistical rows, by the page, with a filter on every column — and one
attribute a person may write. The region's grid, `apps/bbsk-zaznamy`, is the same bundle with its
own manifest.

## Why only one column opens

Every figure of a `KeyPerformanceIndicator` is written by a pipeline: `kpiValue`, `indicator`,
`refArea`, `refPeriod` and the rest come from the publisher's own table and are replaced on the
next run. A screen that let a person correct one would lose what they typed, silently, the next
time the pipeline ran. So the grid opens `stewardNote` — the one attribute of the model that no
pipeline writes (`statistical-observation` 1.1.0, T-2434) — and the page says out loud why the
numbers cannot be edited.

The narrowing is in three places, and each one is the whole rule on its own:

| where | what it refuses |
|---|---|
| the Policy `mesto-steward-note` | `updateAttrs` on `stewardNote` alone: a PATCH of `kpiValue` is the gateway's own 403 |
| `spec.dataNeeds` | one type, one write operation, the listed attributes |
| `records.ts` | the screen sends the note and nothing else, and refuses a note the model would refuse |

## In the shell, and the entity panel

The screen sits in the SDK's `AppShell` (SDK-39) with its states. Each row's **Otvoriť záznam**
opens the record in the shell's entity panel (SDK-40), read fresh through the App's endpoint. The
panel asks the reader's own access document what they may change: a steward, whom the Policy grants
`updateAttrs` on `stewardNote`, gets **Upraviť** with the note as its one field, checked against the
model, shown before it is written, and a refusal (403) or a conflict (409) said in words; every
other reader gets the link to the record in the Portal. The write is the same `PATCH` through the
same endpoint as the grid's own. The panel edits a note a record already has; a first note is
written in the grid's open column. No write operation was added: `dataNeeds` already hold
`updateAttrs` for the note alone.

## Whose records a screen shows

The same `ui/src` is published twice. Which body's records it shows comes from the space the
served configuration names (`banskabystrica-mesto` → the city, `bbsk-kraj` → the region), and the
endpoint is found in the same document by space and never by position. A configuration naming a
space the application does not know says so instead of guessing — nothing is built for one project,
so the two screens cannot drift apart. `ui/src` of the two apps is byte-identical, and
`tests/reference_apps_tests.rs` holds it that way.

## What a person sees when a write is refused

The write is one `PATCH /api/endpoint/{slug}/ngsi-ld/v1/entities/{id}/attrs` through this
application's own endpoint, carried by the person's own session — the bundle holds no credential
and never sends an `Authorization` header. A refusal is the gateway's own problem document, shown
in its own words beside the row it belongs to, with the typed value kept; nothing reloads and
nothing is retried without the person (AP-40, AP-62).

A note longer than the model's 500 characters, or one holding `<` or `>`, is refused before it is
sent, in the same place a refusal appears. That bound is the model's own
(`^[^<>]{0,500}$`); the gateway does not yet validate writes against a space's published schema
(DM-27), so today this is where it holds.

## Charts and names (T-2966)

Above the table, the records of one cube at a time are drawn as charts: a line over the periods
(a month or quarter joins the period), bars across the territories at the latest period, or
columns across a key, such as the city's residents by age. The table draws the indicator and the
territory by name with the code beside it, the number with the publisher's unit text, and the
URN behind a disclosure; its filters still ask the endpoint about the codes.

The names are the statistics office's own category labels, in Slovak and English, in
`ui/src/labels.ts`. `labels.mjs` writes that file from the URLs the pipelines read; run it again
when a pipeline reads another cube:

```
node labels.mjs ../../../joinedcontext-deployment/components/context-gateway/seed
```

## No history, on purpose

The steward's grant is `retrieveOps` and `updateAttrs` (T-2434). It reaches no temporal
operation, so the grid's history is switched off rather than offered as a button that answers 403.

## Building it

```sh
cd ui
pnpm install
pnpm typecheck && pnpm test && pnpm build
pnpm e2e
```

`pnpm e2e` serves the built bundle under `/apps/banskabystrica-zaznamy/` with the static host's
Content Security Policy and the fixture endpoint, and holds the records and a note under review to
375, 768, 1440 and 2560 px: no sideways scroll, no overlapping blocks, no control cut off, axe clean
(T-2825).

## Related

- `docs/Development/10-banska-bystrica-contract.md` — the spaces, endpoints and id shapes
- T-2434 — the steward attribute and the four grants; T-2437 — the region's copy of this screen
