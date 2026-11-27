// covers: src/routes/EndpointsPage.tsx
/** F-UI3-003: each writable class of a projected Endpoint keeps its own scope and write filter. */
import { describe, expect, it } from "vitest";
import { withoutBurst, writePolicyOf } from "../src/routes/EndpointsPage";

const form = { name: "parking-api", contextSpaceRef: "parking" };

describe("the write Policy of a projected Endpoint", () => {
  it("gives two classes with different scopes one rule each", () => {
    const spec = writePolicyOf("helsinki", form, [
      { name: "ParkingSpot", scope: "/kallio/#" },
      { name: "ParkingGroup", writeQ: "status==\"open\"" },
    ]).spec as { rules: Record<string, unknown>[]; scopeQ?: string };
    expect(spec.scopeQ).toBeUndefined();
    expect(spec.rules).toEqual([
      expect.objectContaining({ scopeQ: "/kallio/#", information: [{ entities: [{ type: "ParkingSpot" }] }] }),
      expect.objectContaining({ q: "status==\"open\"", information: [{ entities: [{ type: "ParkingGroup" }] }] }),
    ]);
    expect(spec.rules[1]).not.toHaveProperty("scopeQ");
  });

  it("keeps classes that share their narrowing in one rule", () => {
    const spec = writePolicyOf("helsinki", form, [
      { name: "ParkingSpot", scope: "/kallio/#" },
      { name: "ParkingGroup", scope: "/kallio/#", idPattern: "^urn:ngsi-ld:ParkingGroup:hel" },
    ]).spec as Record<string, unknown>;
    expect(spec).not.toHaveProperty("rules");
    expect(spec.scopeQ).toBe("/kallio/#");
    expect(spec.assignee).toEqual({ kind: "role", id: "helsinki-writers" });
    expect(spec.information).toHaveLength(2);
  });
});

/** A test schema as the page's JSON Schema type. */
const asSchema = (value: unknown) => value as Parameters<typeof withoutBurst>[0];

describe("the burst field of an endpoint's rate limits", () => {
  it("drops only the burst and keeps the rate and every other field", () => {
    const schema = {
      type: "object",
      properties: {
        name: { type: "string" },
        rateLimits: { type: "object", title: "Limits", properties: { requestsPerMinute: { type: "integer" }, burst: { type: "integer" } } },
      },
    };
    const out = withoutBurst(asSchema(schema));
    expect(Object.keys((out.properties?.rateLimits as { properties: object }).properties)).toEqual(["requestsPerMinute"]);
    expect((out.properties?.rateLimits as { title?: string }).title).toBe("Limits");
    expect(out.properties?.name).toEqual({ type: "string" });
    // The input is left as it was: the page builds its schema once per render.
    expect(Object.keys(schema.properties.rateLimits.properties)).toEqual(["requestsPerMinute", "burst"]);
  });

  it("returns a schema without rate limits, or with limits that list no fields, as it is", () => {
    const bare = { type: "object", properties: { name: { type: "string" } } };
    expect(withoutBurst(asSchema(bare))).toBe(bare);
    const empty = { type: "object", properties: { rateLimits: { type: "object" } } };
    expect(withoutBurst(asSchema(empty))).toBe(empty);
    const none = { type: "object" };
    expect(withoutBurst(asSchema(none))).toBe(none);
  });
});
