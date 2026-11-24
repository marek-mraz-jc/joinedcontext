/** T-7589: coverage for dashboardJson.ts lines 112 and 115 and edge case manifest structures. */
import { describe, expect, it } from "vitest";
import {
  insertWidget,
  parseManifest,
  schemaWithEndpoints,
  starter,
  toText,
  widgetSnippet,
} from "../src/pages/dashboards/dashboardJson";

describe("dashboardJson validation and helper coverage", () => {
  it("rejects when spec.pages is missing, not an object or not an array (line 112)", () => {
    const noSpec = JSON.stringify({ kind: "Dashboard" });
    expect(parseManifest(noSpec)).toEqual({
      error: { message: "spec.pages must be a list of pages" },
    });

    const specNotObject = JSON.stringify({ kind: "Dashboard", spec: "invalid" });
    expect(parseManifest(specNotObject)).toEqual({
      error: { message: "spec.pages must be a list of pages" },
    });

    const pagesNotArray = JSON.stringify({ kind: "Dashboard", spec: { pages: "not-array" } });
    expect(parseManifest(pagesNotArray)).toEqual({
      error: { message: "spec.pages must be a list of pages" },
    });
  });

  it("rejects when metadata or metadata.name is missing or not a string (line 115)", () => {
    const noMetadata = JSON.stringify({ kind: "Dashboard", spec: { pages: [] } });
    expect(parseManifest(noMetadata)).toEqual({
      error: { message: "metadata.name must be the dashboard's name" },
    });

    const emptyMetadata = JSON.stringify({ kind: "Dashboard", spec: { pages: [] }, metadata: {} });
    expect(parseManifest(emptyMetadata)).toEqual({
      error: { message: "metadata.name must be the dashboard's name" },
    });

    const numericName = JSON.stringify({ kind: "Dashboard", spec: { pages: [] }, metadata: { name: 1234 } });
    expect(parseManifest(numericName)).toEqual({
      error: { message: "metadata.name must be the dashboard's name" },
    });
  });

  it("accepts a minimal valid Dashboard manifest", () => {
    const valid = JSON.stringify({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Dashboard",
      metadata: { name: "test-dash", namespace: "helsinki" },
      spec: { title: "Test", pages: [{ title: "Overview", widgets: [] }] },
    });
    const parsed = parseManifest(valid);
    expect(parsed.manifest?.metadata.name).toBe("test-dash");
    expect(parsed.error).toBeUndefined();
  });

  it("handles insertWidget with empty pages array or missing widgets property", () => {
    const emptyPages = JSON.stringify({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Dashboard",
      metadata: { name: "dash" },
      spec: { title: "T", pages: [] },
    });
    const widget = widgetSnippet("grid", "ep-1", "Sensor");
    const inserted1 = insertWidget(emptyPages, widget);
    expect(inserted1).not.toBeNull();
    const parsed1 = parseManifest(inserted1!);
    expect(parsed1.manifest?.spec.pages[0].widgets).toEqual([widget]);

    const pageNoWidgets = JSON.stringify({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Dashboard",
      metadata: { name: "dash" },
      spec: { title: "T", pages: [{ title: "P1" }] },
    });
    const inserted2 = insertWidget(pageNoWidgets, widget);
    const parsed2 = parseManifest(inserted2!);
    expect(parsed2.manifest?.spec.pages[0].widgets).toEqual([widget]);
  });

  it("handles schemaWithEndpoints without endpoints or without Widget definition", () => {
    const schemaWithoutWidget = {
      type: "object",
      definitions: { Other: { type: "object" } },
    };
    const untouched = schemaWithEndpoints(schemaWithoutWidget, ["ep-1"]);
    expect(untouched).toEqual(schemaWithoutWidget);

    const schemaWithWidget = {
      type: "object",
      definitions: { Widget: { properties: { endpointRef: { type: "string" } } } },
    };
    const emptyEndpoints = schemaWithEndpoints(schemaWithWidget, []);
    expect(emptyEndpoints).toEqual(schemaWithWidget);
  });

  it("handles starter and toText utilities", () => {
    const s = starter("my-project");
    expect(s.metadata.namespace).toBe("my-project");
    expect(s.spec.visibility).toBe("project");

    const text = toText(s);
    expect(text.endsWith("\n")).toBe(true);
    expect(JSON.parse(text)).toEqual(s);
  });
});
