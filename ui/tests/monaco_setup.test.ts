/**
 * UI-15, UI-16 (T-2137, OPS-46): the editor is the copy in this bundle, never a CDN's.
 *
 * `@monaco-editor/react` fetches Monaco from jsDelivr unless it is told otherwise. A municipal
 * installation runs behind its own Content Security Policy and sometimes with no route to the
 * public internet at all, so an editor that quietly reaches for a CDN is an editor that never
 * opens there — and, where the route does exist, third-party code on the page that edits the
 * organization's manifests.
 *
 * T-5513: the setup registers YAML and no other language, so the build carries no TypeScript,
 * CSS, HTML or JSON service and no worker of theirs.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";

const config = vi.fn();

vi.mock("@monaco-editor/react", () => ({ loader: { config } }));
// The real editor API and language registry, not a mock: Monaco's clipboard feature asks
// `document.queryCommandSupported`, which jsdom lacks.
Object.assign(document, { queryCommandSupported: () => false });
vi.mock("monaco-editor/editor/editor.worker.js?worker", () => ({
  default: class {
    static created = 0;
    constructor() {
      (this.constructor as typeof Object & { created: number }).created += 1;
    }
  },
}));

describe("the editor's setup", () => {
  beforeEach(() => {
    config.mockClear();
    vi.resetModules();
    delete (globalThis as { MonacoEnvironment?: unknown }).MonacoEnvironment;
  });

  it("points the loader at the bundled Monaco rather than at a CDN", async () => {
    await import("../src/pages/models/monaco-setup");
    const monaco = await import("monaco-editor/editor/editor.api");
    expect(config).toHaveBeenCalledTimes(1);
    const passed = config.mock.calls[0][0] as {
      monaco?: { editor?: unknown };
      paths?: { vs?: string };
    };
    expect(passed.monaco?.editor).toBe(monaco.editor);
    // No `paths.vs`, which is the one option that would send it to a CDN after all.
    expect(passed.paths?.vs).toBeUndefined();
    expect(Object.keys(passed)).toEqual(["monaco"]);
  });

  it("registers YAML and none of the languages the Portal never shows", async () => {
    await import("../src/pages/models/monaco-setup");
    const monaco = await import("monaco-editor/editor/editor.api");
    const ids = monaco.languages.getLanguages().map((language) => language.id);
    expect(ids).toContain("yaml");
    for (const unused of ["typescript", "javascript", "css", "html", "json"]) {
      expect(ids).not.toContain(unused);
    }
  });

  it("gives the editor a worker of its own", async () => {
    // Without `MonacoEnvironment.getWorker` the editor loads its worker from the same default
    // CDN path, so the setup is only half done and the highlighting silently dies.
    await import("../src/pages/models/monaco-setup");
    const environment = (globalThis as { MonacoEnvironment?: { getWorker: () => unknown } })
      .MonacoEnvironment;
    expect(environment).toBeDefined();
    expect(typeof environment?.getWorker).toBe("function");
    expect(environment?.getWorker()).toBeInstanceOf(Object);
  });
});
