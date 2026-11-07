import "@testing-library/jest-dom/vitest";
import { configure } from "@testing-library/react";
import { afterAll } from "vitest";
import { recordControls } from "@joinedcontext/sdk/testing";

// Every control a test file renders and which its tests use, for the Apps' coverage gate (T-3373).
recordControls(afterAll);

// ci-full runs every App's suite under v8 coverage, where the library's 1 s default for
// findBy*/waitFor gives way: a different test missed it on each run (T-4350, as ui/tests/setup.ts).
configure({ asyncUtilTimeout: 5_000 });
