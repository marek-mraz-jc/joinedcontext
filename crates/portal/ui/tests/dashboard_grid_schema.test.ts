/**
 * The grid widget's form schema (UI-71, T-2756) from a published grid schema of another shape:
 * a choice offered as words, a list kept a list, the fields naming a source left out.
 */
import { describe, expect, it, vi } from "vitest";

vi.mock("@joinedcontext/sdk", async (actual) => ({
  ...(await actual<typeof import("@joinedcontext/sdk")>()),
  gridConfigSchema: {
    properties: {
      source: { type: "string" },
      compareWith: { type: "string" },
      mode: { enum: ["table", "map"] },
      density: { enum: [1, 2] },
      title: { anyOf: [{ type: "string", enum: ["a"] }, { type: "null" }] },
      columns: { type: "array", items: { properties: { name: { type: "string" }, formula: { type: "string" } } } },
    },
  },
}));

const { dashboardSchema } = await import("../src/schemas/kinds");

function findGrid(node: unknown): Record<string, unknown> | undefined {
  if (node === null || typeof node !== "object") return undefined;
  const record = node as Record<string, unknown>;
  if (record.title === "dashboards.field.grid") return record;
  for (const value of Object.values(record)) {
    const found = findGrid(value);
    if (found) return found;
  }
  return undefined;
}

describe("the grid widget's schema", () => {
  it("words the grid's choices, keeps lists as lists and leaves out what names a source", () => {
    const grid = findGrid(dashboardSchema((key) => key, []));
    expect(grid?.additionalProperties).toBe(false);
    const properties = grid?.properties as Record<string, Record<string, unknown>>;
    expect(Object.keys(properties).sort()).toEqual(["columns", "density", "mode", "title"]);
    expect(properties.mode).toEqual({ oneOf: [{ const: "table", title: "choice.gridMode.table" }, { const: "map", title: "choice.gridMode.map" }] });
    // Choices that are not words stay the schema's own values.
    expect(properties.density).toEqual({ enum: [1, 2] });
    // A nested enum is no grid choice of its own, and the alternatives stay a list.
    expect(properties.title).toEqual({ anyOf: [{ type: "string", enum: ["a"] }, { type: "null" }] });
    expect(properties.columns).toEqual({ type: "array", items: { properties: { name: { type: "string" } } } });
  });
});
