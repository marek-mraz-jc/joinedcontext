# Ukazovatele kraja a mesta / Indicators of the region and the city

One screen showing the `KeyPerformanceIndicator` entities of two public bodies side by side: the
Banskobystrický samosprávny kraj and the city of Banská Bystrica. It reads and never writes.

It sits in the SDK's `AppShell` (SDK-39). A card's territory, a district's bar in a chart and a
district on the map open that indicator in the shell's entity panel (SDK-40), read fresh through
its body's endpoint.

**What a reader edits here: nothing.** The `dataNeeds` keep `queryEntity` and `retrieveEntity`. Every
indicator is computed by its pipeline (T-2307), which rewrites it on each run, so a value changed by
hand would be overwritten and lost, and a hand-typed figure under a publisher's heading is the
confusion this screen exists to prevent. The panel never offers Edit; it links the indicator to the
Portal.

## Why it is two sections and not one table

The region has roughly 607 581 inhabitants and the city 72 123. The same question — how many
people, how much dust — has two right answers here, eight times apart, and a reader who takes the
city's for the region's is wrong with no way to tell. So:

- every card names its territory, on the card and not in a page title;
- each body's cards sit under a heading naming that body, with a note saying what the numbers
  cover and what they do not;
- an indicator both bodies publish appears once under each, never merged into one number;
- the body is taken from the entity's own URN (`{orgDomain}`), which the writing pipeline mints
  from the project it runs in, so a row arriving on the wrong endpoint is dropped rather than
  relabelled.

## What it reads

Two endpoints, both through the reader's own token (AP-07):

| body | space | how it is reached |
|---|---|---|
| `bbsk` | `bbsk-kpi` | the app's own Endpoint, rendered from `spec.dataNeeds` |
| `banskabystrica` | `banskabystrica-kpi` | the `SharedSpaceReference` `mesto-kpi` the region declares (EP-15, `docs/Development/10` §8) |

The endpoints are found in the served configuration by **space**, never by position, so the
application behaves the same however the configuration lists them, and a configuration naming only
one leaves the other body marked unreachable instead of showing the wrong rows under its heading.

## What is the application's own, and what is the entity's

`PF-54` closes the `KeyPerformanceIndicator` attribute set, so the entity carries no territory and
no threshold state:

- the **territory** is the `{localId}` suffix (`docs/Development/10` §6), split at the territory
  token and not at the last hyphen, so `okres-ziar-nad-hronom` keeps its name;
- the **threshold state** is computed here from Directive 2008/50/EC, as `docs/Development/11` §2
  quotes it: PM10 amber at 35 and red at 50 µg/m³, PM2.5 amber at 17 and red at 25. An indicator
  with no published limit gets no state rather than an invented one;
- the **written unit** (`t/km²`, `l/os./deň`) is the locale's, because the UN/CEFACT code on the
  entity names the numerator only. A value whose code is not the one the contract fixes is shown
  with its raw code instead.

A window with no readings reads "not measured" with the reason beside it, never `0` and never a
bare dash.

## Building it

```sh
cd ui
pnpm install
pnpm typecheck && pnpm test && pnpm build
pnpm e2e
```

`pnpm e2e` serves the built bundle under `/apps/bbsk-ukazovatele/` with the static host's Content
Security Policy and both bodies' fixtures, and holds the indicators to 375, 768, 1440 and 2560 px:
no sideways scroll, no overlapping cards, no control cut off, axe clean (T-2825); and an indicator
in the entity panel at 375 and 1440 px, light and dark. The tests exercise every control and hold
the coverage gate of T-3373 (`sh scripts/app-coverage-run.sh bbsk-ukazovatele`).

`@joinedcontext/sdk` is linked from this repository, which is why `vite.config.ts` dedupes React:
the linked SDK carries its own copy and two Reacts leave every hook with a null dispatcher. A
published app resolves the SDK from the registry and has one.

## Related

- `docs/Development/10-banska-bystrica-contract.md` — the frozen contract: ids, spaces, endpoints,
  territories
- `docs/Development/11-banska-bystrica-kpis.md` — the five indicators and their limits
- T-2307 — the pipelines that compute the entities this screen shows
