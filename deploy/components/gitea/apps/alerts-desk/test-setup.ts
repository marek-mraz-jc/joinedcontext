import "@testing-library/jest-dom/vitest";
import { configure } from "@testing-library/react";
import { afterAll } from "vitest";
import { recordControls } from "@joinedcontext/sdk/testing";

// The Apps' coverage gate reads which controls the tests rendered and which they exercised (T-3373).
recordControls(afterAll);

// ci-full runs every App's suite under v8 coverage, where the library's 1 s default for
// findBy*/waitFor gives way: a different test missed it on each run (T-4350, as ui/tests/setup.ts).
configure({ asyncUtilTimeout: 5_000 });
