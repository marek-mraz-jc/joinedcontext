/**
 * The pipeline form's target choices (UI judge 2026-11-09): a saved target the minted URN does
 * not match showed as `urn:urn:ngsi-ld:Endpoint:…` instead of its Endpoint's name.
 */
import { describe, expect, it } from "vitest";
import type { Manifest } from "../src/api/manifest";
import { endpointOptionsFor } from "../src/pages/pipelines/PipelineEditor";

const endpoint = (name: string, space: string): Manifest =>
  ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "Endpoint", metadata: { name }, spec: { contextSpaceRef: space } }) as Manifest;

const LIST = [endpoint("banskabystrica-verejne", "banskabystrica-verejne"), endpoint("senzory", "senzory")];
const SAVED = "urn:ngsi-ld:Endpoint:banskabystrica.sk:banskabystrica-verejne:banskabystrica-verejne";

describe("endpointOptionsFor", () => {
  it("offers the minted URN of each Endpoint, and a matching target as it is", () => {
    expect(endpointOptionsFor(LIST, "banskabystrica.sk", undefined, SAVED)).toEqual([
      { name: "banskabystrica-verejne", urn: SAVED },
      { name: "senzory", urn: "urn:ngsi-ld:Endpoint:banskabystrica.sk:senzory:senzory" },
    ]);
  });

  it("keeps a target minted under another domain on its Endpoint's name, never as urn:urn:", () => {
    const options = endpointOptionsFor(LIST, "banskabystrica", undefined, SAVED);
    expect(options).toEqual([
      { name: "banskabystrica-verejne", urn: SAVED },
      { name: "senzory", urn: "urn:ngsi-ld:Endpoint:banskabystrica:senzory:senzory" },
    ]);
    expect(options.some((option) => option.name.startsWith("urn:urn:"))).toBe(false);
  });

  it("shows a target no Endpoint of the list ends as the URN itself", () => {
    const other = "urn:ngsi-ld:Endpoint:example.org:elsewhere:gone";
    expect(endpointOptionsFor(LIST, "banskabystrica.sk", undefined, other).at(-1)).toEqual({ name: other, urn: other });
  });

  it("adds a source Endpoint the list does not hold", () => {
    expect(endpointOptionsFor(LIST, "banskabystrica.sk", "old-source", undefined).at(-1)).toEqual({ name: "old-source" });
  });
});
