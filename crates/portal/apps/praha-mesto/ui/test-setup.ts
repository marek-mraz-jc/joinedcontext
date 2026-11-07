import "@testing-library/jest-dom/vitest";
import { configure } from "@testing-library/react";
import { vi } from "vitest";

// jsdom has no layout: the charts resize on nothing, and that is all a test needs of it.
class NoResize {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= NoResize as unknown as typeof ResizeObserver;

// jsdom has no WebGL. The map is replaced by one that records what the screen hands it, so a test
// reads the points and colours the screen drew and nothing pretends to render tiles.
export const drawn: { data: unknown[]; style: unknown[] } = { data: [], style: [] };
vi.mock("maplibre-gl", () => {
  class FakeMap {
    private handlers: Record<string, () => void> = {};
    constructor(options: { style: unknown }) {
      drawn.style.push(options.style);
      queueMicrotask(() => this.handlers.load?.());
    }
    on(event: string, handler: () => void) {
      this.handlers[event] = handler;
      return this;
    }
    addSource() {}
    addLayer() {}
    getSource() {
      return { setData: (data: unknown) => drawn.data.push(data) };
    }
    remove() {}
  }
  return { Map: FakeMap, setWorkerUrl: () => {} };
});

// Every control a test file renders and which its tests use, for the Apps' coverage gate (T-3373).
import { afterAll } from "vitest";
import { recordControls } from "@joinedcontext/sdk/testing";
recordControls(afterAll);

// ci-full runs every App's suite under v8 coverage, where the library's 1 s default for
// findBy*/waitFor gives way: a different test missed it on each run (T-4350, as ui/tests/setup.ts).
configure({ asyncUtilTimeout: 5_000 });
