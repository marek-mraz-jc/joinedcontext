import { describe, expect, it } from "vitest";
import { overlay } from "../src/api/manifest";
import type { Manifest } from "../src/api/manifest";

/** An edit through a form keeps what the form has no control for (T-0885, AG-77, PF-57). */
const base: Manifest = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "bikes", namespace: "helsinki", title: { en: "Bikes", fi: "Pyörät" } },
  spec: {
    contextSpaceRef: "helsinki",
    rateLimits: { requestsPerMinute: 600 },
    caching: { maxAgeSeconds: 30 },
    policyRef: "urn:ngsi-ld:Policy:hel.fi:helsinki:public-bikes",
    publish: { ckan: { instanceRef: { kind: "CkanInstance", name: "hel-fi" } } },
  },
};

const next: Manifest = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "bikes", namespace: "helsinki", title: "Bikes", labels: { "joinedcontext.com/space": "helsinki" } },
  spec: { contextSpaceRef: "helsinki", rateLimits: { requestsPerMinute: 300 } },
};

describe("overlay", () => {
  it("keeps the spec keys the form does not own and replaces the ones it does", () => {
    const out = overlay(base, next, ["contextSpaceRef", "rateLimits", "caching"]);
    expect(out.spec.policyRef).toBe("urn:ngsi-ld:Policy:hel.fi:helsinki:public-bikes");
    expect(out.spec.publish).toEqual(base.spec.publish);
    expect(out.spec.rateLimits).toEqual({ requestsPerMinute: 300 });
    // Owned and absent from the form: the person cleared it.
    expect(out.spec).not.toHaveProperty("caching");
  });

  it("keeps a localized title as a map, the shown entry replaced", () => {
    const out = overlay(base, { ...next, metadata: { ...next.metadata, title: "City bikes" } }, []);
    expect(out.metadata.title).toEqual({ en: "City bikes", fi: "Pyörät" });
  });

  it("drops the title the form cleared", () => {
    const { title: _title, ...metadata } = next.metadata;
    void _title;
    expect(overlay(base, { ...next, metadata }, []).metadata).not.toHaveProperty("title");
  });

  it("adds no label to a manifest that had none, and merges into one that had", () => {
    expect(overlay(base, next, []).metadata).not.toHaveProperty("labels");
    const labelled = { ...base, metadata: { ...base.metadata, labels: { tier: "gold" } } };
    expect(overlay(labelled, next, []).metadata.labels).toEqual({ tier: "gold", "joinedcontext.com/space": "helsinki" });
  });

  it("is the form's manifest when there is nothing to edit", () => {
    expect(overlay(undefined, next, ["rateLimits"])).toBe(next);
  });

  it("never carries a status", () => {
    const withStatus = { ...base, status: { phase: "Live" } } as Manifest;
    expect(overlay(withStatus, next, [])).not.toHaveProperty("status");
  });
});
