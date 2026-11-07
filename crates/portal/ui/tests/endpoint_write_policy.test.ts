// covers: src/routes/EndpointsPage.tsx
/** F-UI3-003: each writable class of a projected Endpoint keeps its own scope and write filter. */
import { describe, expect, it } from "vitest";
import { writePolicyOf } from "../src/routes/EndpointsPage";

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
