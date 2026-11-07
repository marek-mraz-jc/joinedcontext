# The sample gallery (T-2778, SDK-24)

Reference applications the agent builds from. Each is an **overlay on the template**: only the
files a model would write over `sdk/template/` (`src/App.tsx`, pages, `src/app.css`,
`src/design-tokens.json`, tests, functions), so a sample reads exactly like a good answer.
A relative import of a file the sample does not hold is the template's file at the same place
(`sampleOverlay` in `vite.config.ts`; `rootDirs` in each sample's `tsconfig.json`).

Each folder holds:

| file | what |
|---|---|
| `sample.json` | purpose, archetype, audience, layout, keywords the agent matches a request against, data needs, access, SDK parts, the text that says it is on screen (`ready`) |
| `README.md` | what it is for, which SDK parts it uses, what to copy |
| `model.linkml.yaml` | the data model it reads |
| `src/fixtures.ts` | `ROWS` (and `SCHEMA`, `FUNCTIONS` where it needs them): the endpoint's answer for its tests and the gallery's browser check |
| `src/…` | the overlay; its tests beside every page |

Checks: its tests run with the SDK's (`pnpm test`), it typechecks with the template
(`pnpm typecheck`), and `e2e/responsive.spec.ts` holds it to 375, 768, 1440 and 2560 px (no
sideways scroll, no overlapping blocks, axe clean).

No sample holds a secret, and each reads through the least access its purpose needs.
