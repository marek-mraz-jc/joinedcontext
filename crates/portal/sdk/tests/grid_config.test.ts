import { describe, expect, it } from "vitest";
import { parseGridConfig, DEFAULT_PAGE_SIZE, gridConfigSchema } from "../src/grid/config";

describe("parseGridConfig", () => {
  it("accepts minimal valid config with defaults", () => {
    const raw = { source: { kind: "endpoint", slug: "abc" }, type: "Bike" };
    const result = parseGridConfig(raw);
    expect(result.findings).toHaveLength(0);
    expect(result.config).toBeDefined();
    expect(result.config!.source).toEqual({ kind: "endpoint", slug: "abc" });
    expect(result.config!.type).toBe("Bike");
    expect(result.config!.columns).toEqual([]);
    expect(result.config!.entityTimestamps).toBe(false);
    expect(result.config!.filters).toEqual({});
    expect(result.config!.pageSize).toBe(DEFAULT_PAGE_SIZE);
    expect(result.config!.mode).toBe("view");
    expect(result.config!.editableAttrs).toEqual([]);
    expect(result.config!.history).toEqual({ enabled: false });
    expect(result.config!.density).toBe("comfortable");
    expect(result.config!.rowActions).toEqual([]);
    expect(result.config!.compareWith).toBeUndefined();
  });

  it("rejects unknown top-level key", () => {
    const raw = { source: { kind: "endpoint", slug: "abc" }, type: "Bike", extra: true };
    const result = parseGridConfig(raw);
    expect(result.findings).toHaveLength(1);
    expect(result.findings[0].path).toBe("/extra");
    expect(result.config).toBeUndefined();
  });

  it("rejects unknown column format", () => {
    const raw = {
      source: { kind: "endpoint", slug: "abc" },
      type: "Bike",
      columns: [{ attr: "speed", format: "color" }],
    };
    const result = parseGridConfig(raw);
    expect(result.findings).toHaveLength(1);
    expect(result.findings[0].path).toBe("/columns/0/format");
    expect(result.config).toBeUndefined();
  });

  it("rejects mode edit with empty editableAttrs", () => {
    const raw = {
      source: { kind: "endpoint", slug: "abc" },
      type: "Bike",
      mode: "edit",
      editableAttrs: [],
    };
    const result = parseGridConfig(raw);
    expect(result.findings).toHaveLength(1);
    expect(result.findings[0].path).toBe("/editableAttrs");
    expect(result.config).toBeUndefined();
  });

  it("rejects pageSize > 5000", () => {
    const raw = {
      source: { kind: "endpoint", slug: "abc" },
      type: "Bike",
      pageSize: 5001,
    };
    const result = parseGridConfig(raw);
    expect(result.findings).toHaveLength(1);
    expect(result.findings[0].path).toBe("/pageSize");
    expect(result.config).toBeUndefined();
  });

  it("rejects source with url key", () => {
    const raw = {
      source: { kind: "endpoint", slug: "abc", url: "http://evil.com" },
      type: "Bike",
    };
    const result = parseGridConfig(raw);
    expect(result.findings).toHaveLength(1);
    expect(result.findings[0].path).toBe("/source/url");
    expect(result.config).toBeUndefined();
  });

  it("accepts space source", () => {
    const raw = { source: { kind: "space", space: "helsinki-bikes" }, type: "Station" };
    const result = parseGridConfig(raw);
    expect(result.findings).toHaveLength(0);
    expect(result.config!.source).toEqual({ kind: "space", space: "helsinki-bikes" });
  });

  it("accepts fixture source", () => {
    const raw = { source: { kind: "fixture", name: "test-fixture" }, type: "T" };
    const result = parseGridConfig(raw);
    expect(result.findings).toHaveLength(0);
    expect(result.config!.source).toEqual({ kind: "fixture", name: "test-fixture" });
  });

  it("rejects source slug with slash", () => {
    const raw = { source: { kind: "endpoint", slug: "a/b" }, type: "T" };
    const result = parseGridConfig(raw);
    expect(result.findings).toHaveLength(1);
    expect(result.findings[0].path).toBe("/source/slug");
  });

  it("rejects source slug with question mark", () => {
    const raw = { source: { kind: "endpoint", slug: "a?b" }, type: "T" };
    const result = parseGridConfig(raw);
    expect(result.findings).toHaveLength(1);
    expect(result.findings[0].path).toBe("/source/slug");
  });

  it("survives JSON roundtrip and parses the same", () => {
    const raw = {
      source: { kind: "endpoint", slug: "abc" },
      type: "Bike",
      columns: [{ attr: "speed", label: "Speed", width: 100, pinned: true, show: { unit: true }, format: "number" }],
      entityTimestamps: true,
      filters: { allowed: ["speed"], preset: { q: "speed>10", attrs: ["speed"], idPattern: ".*", scopeQ: "public" } },
      pageSize: 100,
      mode: "view",
      editableAttrs: ["speed"],
      history: { enabled: true, maxPoints: 10 },
      compareWith: { kind: "endpoint", slug: "other" },
      density: "compact",
      rowActions: ["delete"],
    };
    const result1 = parseGridConfig(raw);
    expect(result1.findings).toHaveLength(0);
    const json = JSON.parse(JSON.stringify(raw));
    const result2 = parseGridConfig(json);
    expect(result2.findings).toHaveLength(0);
    expect(result2.config).toEqual(result1.config);
  });
});

describe("the map beside the rows (UI-72)", () => {
  const base = { source: { kind: "fixture", name: "x" }, type: "Vehicle" };

  it("is off unless the manifest asks for it", () => {
    const { config, findings } = parseGridConfig(base);
    expect(findings).toHaveLength(0);
    expect(config!.map).toEqual({ enabled: false });
  });

  it("takes the attribute and the side the manifest names", () => {
    const { config, findings } = parseGridConfig({
      ...base,
      map: { enabled: true, attr: "location", position: "bottom" },
    });
    expect(findings).toHaveLength(0);
    expect(config!.map).toEqual({ enabled: true, attr: "location", position: "bottom" });
  });

  it("names what is wrong instead of dropping it", () => {
    expect(parseGridConfig({ ...base, map: "yes" }).findings).toEqual([
      { path: "/map", message: "must be an object" },
    ]);
    expect(parseGridConfig({ ...base, map: { enabled: "true" } }).findings).toEqual([
      { path: "/map/enabled", message: "must be a boolean" },
    ]);
    expect(parseGridConfig({ ...base, map: { attr: "" } }).findings).toEqual([
      { path: "/map/attr", message: "must be a non-empty string" },
    ]);
    expect(parseGridConfig({ ...base, map: { position: "left" } }).findings).toEqual([
      { path: "/map/position", message: 'must be "right" or "bottom"' },
    ]);
    expect(parseGridConfig({ ...base, map: { zoom: 12 } }).findings).toEqual([
      { path: "/map/zoom", message: "unknown property" },
    ]);
  });
});

describe("gridConfigSchema", () => {
  it("has additionalProperties false at top level", () => {
    expect(gridConfigSchema.additionalProperties).toBe(false);
  });

  // 5000 rows a page since owner 2026-11-01; the source reads them in broker pages of 1000.
  it("has pageSize maximum 5000", () => {
    const pageSizeProp = (gridConfigSchema.properties as Record<string, unknown>).pageSize as Record<string, unknown>;
    expect(pageSizeProp.maximum).toBe(5000);
  });
});

describe("grid.config.schema.json", () => {
  it("is the committed copy of gridConfigSchema (regenerate: see the file's test)", async () => {
    // npx tsx -e 'import { gridConfigSchema } from "./src/grid/config.ts"; import { writeFileSync } from "node:fs";
    //   writeFileSync("grid.config.schema.json", JSON.stringify(gridConfigSchema, null, 2) + "\n");'
    const committed = (await import("../grid.config.schema.json")).default;
    expect(committed).toEqual(gridConfigSchema);
  });
});
