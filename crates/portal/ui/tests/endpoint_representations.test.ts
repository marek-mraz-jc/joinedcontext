import { describe, expect, it } from "vitest";
import { servedRepresentations } from "../src/components/endpoints/links";
import {
  checkedRepresentations,
  representationSpec,
  servesMcp,
  toEnvelope,
  toForm,
} from "../src/routes/EndpointsPage";
import type { Manifest } from "../src/api/manifest";

const EVERY_BUT_STA = ["ngsi-ld", "csv", "ogc-features", "odata", "websocket", "mcp"];

function endpoint(spec: Record<string, unknown>): Manifest {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "air", namespace: "helsinki" },
    spec: { contextSpaceRef: "air", slug: "air", audience: "public", ...spec },
  };
}

/** EP-104: no list serves every standard but sta, less the disabled ones (T-3942). */
describe("the Endpoint's standards", () => {
  it("serves every standard but sta when the spec lists none, as jc-core does", () => {
    expect(servedRepresentations({})).toEqual(["ngsi-ld", "mcp", "csv", "ogc-features", "odata", "websocket"]);
    expect(servedRepresentations({ disabledRepresentations: ["odata", "csv"], mcp: false })).toEqual([
      "ngsi-ld",
      "ogc-features",
      "websocket",
    ]);
    expect(servedRepresentations({ enabledRepresentations: ["sta"] })).toEqual(["sta", "ngsi-ld", "mcp"]);
  });

  it("checks every standard but sta on a new Endpoint and writes no field for it", () => {
    expect(checkedRepresentations({})).toEqual(EVERY_BUT_STA);
    expect(representationSpec(EVERY_BUT_STA)).toEqual({});
  });

  it("writes an unchecked standard as disabledRepresentations and leaves the list unset", () => {
    const checked = EVERY_BUT_STA.filter((rep) => rep !== "odata" && rep !== "csv");
    expect(representationSpec(checked)).toEqual({ disabledRepresentations: ["csv", "odata"] });
  });

  it("never writes an always-on standard as disabled, even unchecked", () => {
    expect(representationSpec(["csv"])).toEqual({
      disabledRepresentations: ["ogc-features", "odata", "websocket"],
    });
  });

  it("turns sta on with a list, since only a list serves it", () => {
    expect(representationSpec([...EVERY_BUT_STA, "sta"])).toEqual({
      enabledRepresentations: ["csv", "ogc-features", "odata", "websocket", "sta"],
    });
  });

  it("keeps an existing list as checked, in its order, and never widens it on save", () => {
    const stored = endpoint({ enabledRepresentations: ["csv", "ngsi-ld"] });
    const form = toForm(stored);
    // Public and not opted in: MCP is unchecked (T-6761).
    expect(form.enabledRepresentations).toEqual(["ngsi-ld", "csv"]);
    expect(toEnvelope("helsinki", form, "air", [], undefined, stored).spec).toEqual(stored.spec);

    form.enabledRepresentations = [...form.enabledRepresentations, "odata"];
    expect(toEnvelope("helsinki", form, "air", [], undefined, stored).spec).toMatchObject({
      enabledRepresentations: ["csv", "ngsi-ld", "odata"],
    });
    form.enabledRepresentations = ["ngsi-ld"];
    const narrowed = toEnvelope("helsinki", form, "air", [], undefined, stored).spec as Record<string, unknown>;
    expect(narrowed.enabledRepresentations).toEqual(["ngsi-ld"]);
    expect(narrowed).not.toHaveProperty("disabledRepresentations");
  });

  it("drops a retired file a stored list still names on save (T-4600)", () => {
    const stored = endpoint({ enabledRepresentations: ["geojson", "ngsi-ld", "csv", "xlsx"] });
    const form = toForm(stored);
    expect(form.enabledRepresentations).toEqual(["ngsi-ld", "csv"]);
    expect(toEnvelope("helsinki", form, "air", [], undefined, stored).spec).toMatchObject({
      enabledRepresentations: ["ngsi-ld", "csv"],
    });
  });

  it("round-trips a disabled set and drops it once everything is checked again", () => {
    const stored = endpoint({ disabledRepresentations: ["websocket"] });
    const form = toForm(stored);
    expect(form.enabledRepresentations).not.toContain("websocket");
    expect(toEnvelope("helsinki", form, "air", [], undefined, stored).spec).toEqual(stored.spec);

    form.enabledRepresentations = EVERY_BUT_STA;
    const spec = toEnvelope("helsinki", form, "air", [], undefined, stored).spec as Record<string, unknown>;
    expect(spec).not.toHaveProperty("disabledRepresentations");
    expect(spec).not.toHaveProperty("enabledRepresentations");
  });

  it("leaves mcp unchecked when the Endpoint turned it off, and keeps it off", () => {
    const stored = endpoint({ mcp: false });
    const form = toForm(stored);
    expect(form.enabledRepresentations).not.toContain("mcp");
    expect(toEnvelope("helsinki", form, "air", [], undefined, stored).spec).toEqual(stored.spec);
  });
});

/** T-6761 (EP-24, deny by default): MCP on a public Endpoint is the person's switch. */
describe("MCP on a public Endpoint", () => {
  it("is not served until the spec opts in, as jc-core's mcp_on", () => {
    expect(servedRepresentations({ audience: "public", enabledRepresentations: ["csv"] })).toEqual(["csv", "ngsi-ld"]);
    expect(servedRepresentations({ audience: "public" })).not.toContain("mcp");
    expect(servedRepresentations({ audience: "public", mcp: true })).toContain("mcp");
    expect(servedRepresentations({ audience: "public", enabledRepresentations: ["mcp"] })).toContain("mcp");
    // An internal Endpoint keeps today's default.
    expect(servedRepresentations({ audience: "project-list" })).toContain("mcp");
  });

  it("writes mcp: true when checked and nothing when unchecked", () => {
    const stored = endpoint({});
    const form = toForm(stored);
    expect(form.enabledRepresentations).not.toContain("mcp");
    expect(toEnvelope("helsinki", form, "air", [], undefined, stored).spec).not.toHaveProperty("mcp");

    form.enabledRepresentations = [...form.enabledRepresentations, "mcp"];
    expect(toEnvelope("helsinki", form, "air", [], undefined, stored).spec).toMatchObject({ mcp: true });
  });

  it("lists mcp on a listing Endpoint, and drops a stored switch once unchecked", () => {
    const listing = endpoint({ enabledRepresentations: ["ngsi-ld", "csv"] });
    const form = toForm(listing);
    form.enabledRepresentations = [...form.enabledRepresentations, "mcp"];
    const opted = toEnvelope("helsinki", form, "air", [], undefined, listing).spec as Record<string, unknown>;
    expect(opted.enabledRepresentations).toEqual(["ngsi-ld", "csv", "mcp"]);
    expect(opted).not.toHaveProperty("mcp");

    const switched = endpoint({ mcp: true });
    const kept = toForm(switched);
    expect(kept.enabledRepresentations).toContain("mcp");
    expect(toEnvelope("helsinki", kept, "air", [], undefined, switched).spec).toEqual(switched.spec);
    kept.enabledRepresentations = kept.enabledRepresentations.filter((rep) => rep !== "mcp");
    expect(toEnvelope("helsinki", kept, "air", [], undefined, switched).spec).not.toHaveProperty("mcp");
  });

  it("says on in the grid's MCP column only where the Endpoint serves it", () => {
    expect(servesMcp({ audience: "public", enabledRepresentations: ["ngsi-ld", "csv"] })).toBe(false);
    expect(servesMcp({ audience: "public" })).toBe(false);
    expect(servesMcp({ audience: "public", mcp: true })).toBe(true);
    expect(servesMcp({ audience: "public", enabledRepresentations: ["ngsi-ld", "mcp"] })).toBe(true);
    expect(servesMcp({ audience: "organization" })).toBe(true);
    expect(servesMcp({ audience: "organization", mcp: false })).toBe(false);
  });

  it("keeps an internal Endpoint's mcp: false on save", () => {
    const stored = endpoint({ audience: "project-list", allowedProjects: ["helsinki"], mcp: false });
    const form = toForm(stored);
    expect(toEnvelope("helsinki", form, "air", [], undefined, stored).spec).toMatchObject({ mcp: false });
  });
});
