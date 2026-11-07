import { describe, expect, it } from "vitest";
import { servedRepresentations } from "../src/components/endpoints/links";
import {
  checkedRepresentations,
  representationSpec,
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
    expect(form.enabledRepresentations).toEqual(["ngsi-ld", "csv", "mcp"]);
    expect(toEnvelope("helsinki", form, "air", [], undefined, stored).spec).toEqual(stored.spec);

    form.enabledRepresentations = [...form.enabledRepresentations, "odata"];
    expect(toEnvelope("helsinki", form, "air", [], undefined, stored).spec).toMatchObject({
      enabledRepresentations: ["csv", "ngsi-ld", "odata"],
    });
    form.enabledRepresentations = ["ngsi-ld", "mcp"];
    const narrowed = toEnvelope("helsinki", form, "air", [], undefined, stored).spec as Record<string, unknown>;
    expect(narrowed.enabledRepresentations).toEqual(["ngsi-ld"]);
    expect(narrowed).not.toHaveProperty("disabledRepresentations");
  });

  it("drops a retired file a stored list still names on save (T-4600)", () => {
    const stored = endpoint({ enabledRepresentations: ["geojson", "ngsi-ld", "csv", "xlsx"] });
    const form = toForm(stored);
    expect(form.enabledRepresentations).toEqual(["ngsi-ld", "csv", "mcp"]);
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
