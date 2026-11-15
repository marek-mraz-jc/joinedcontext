import "@testing-library/jest-dom/vitest";
import { configure } from "@testing-library/react";
import { afterAll } from "vitest";
import { recordControls } from "@joinedcontext/sdk/testing";
// The module as the build lane binds it into wasm/pkg (builder/build-wasm.sh, or `pnpm wasm`),
// loaded once: jsdom has no worker, so the page calls it in place, and every test runs the real
// WebAssembly. Vite inlines its bytes, so no file is read and no network is asked.
import wasm from "./wasm/pkg/bike_rebalancing_bg.wasm?url&inline";
import { initSync } from "./wasm/pkg/bike_rebalancing.js";

initSync({ module: Uint8Array.from(atob(wasm.slice(wasm.indexOf(",") + 1)), (c) => c.charCodeAt(0)) });

// The Apps' coverage gate reads which controls the tests rendered and which they exercised (T-3373).
recordControls(afterAll);

// ci-full runs every App's suite under v8 coverage, where the library's 1 s default for
// findBy*/waitFor gives way: a different test missed it on each run (T-4350, as ui/tests/setup.ts).
configure({ asyncUtilTimeout: 5_000 });
