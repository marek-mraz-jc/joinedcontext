import "@testing-library/jest-dom/vitest";
import { afterAll, vi } from "vitest";
import { recordControls } from "@joinedcontext/sdk/testing";

vi.mock("maplibre-gl", () => import("./maplibre"));

// The Apps' coverage gate reads which controls the tests rendered and which they exercised (T-3373).
recordControls(afterAll);
