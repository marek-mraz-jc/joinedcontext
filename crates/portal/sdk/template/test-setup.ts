import "@testing-library/jest-dom/vitest";
import { configure } from "@testing-library/react";

// ci-full runs every App's suite under v8 coverage, where the library's 1 s default for
// findBy*/waitFor gives way: a different test missed it on each run (T-4350, as ui/tests/setup.ts).
configure({ asyncUtilTimeout: 5_000 });
