# Portal UI

Vite + React + TypeScript. Generated API client from `../target/openapi.json` (`openapi-typescript`); no hand-written fetch calls.

## Commands

- `pnpm dev` — Start the Vite development server on port 5173 with proxy to the backend API (`http://localhost:8080`).
- `pnpm build` — Typecheck project references (`tsc -b`) and build production assets into `dist/`.
- `pnpm preview` — Preview the production build locally.
- `pnpm lint` — Lint source files using ESLint.
- `pnpm typecheck` — Run TypeScript compiler check without emitting files (`tsc --noEmit`).
- `pnpm test` — Run all unit and component tests with Vitest.
- `pnpm test:unit` — Run unit and component tests with Vitest.
  Outside CI Vitest uses at most 3 workers, so parallel runs on a shared host leave CPU for builds; set
  `VITEST_MAX_WORKERS` to change it (for example `VITEST_MAX_WORKERS=8 pnpm test`). CI keeps one worker per core.
- `pnpm generate:api` — Generate TypeScript types from OpenAPI 3.1 specification.
