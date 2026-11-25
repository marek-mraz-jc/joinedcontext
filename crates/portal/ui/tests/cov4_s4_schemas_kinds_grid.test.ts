// T-7706: the dashboard form's grid schema when the SDK publishes a columns property without item properties.
import { describe, expect, it, vi } from "vitest";

vi.mock("@joinedcontext/sdk", async (importOriginal) => {
  const real = await importOriginal<typeof import("@joinedcontext/sdk")>();
  return {
    ...real,
    gridConfigSchema: { properties: { columns: { type: "array" }, source: { type: "string" } } },
  };
});

describe("the dashboard form's grid widget", () => {
  it("keeps a columns property that has no item properties as published, and drops the source", async () => {
    const { dashboardSchema } = await import("../src/schemas/kinds");
    const schema = dashboardSchema((key) => key, []);
    const text = JSON.stringify(schema);
    expect(text).toContain('"columns":{"type":"array"');
    expect(text).not.toContain('"source":{"type":"string"');
  });
});
