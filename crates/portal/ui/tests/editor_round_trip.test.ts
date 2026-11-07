import { describe, expect, it } from "vitest";
import { toEnvelope as endpointEnvelope, toForm as endpointForm } from "../src/routes/EndpointsPage";
import { fromManifest, toEnvelope as pipelineEnvelope } from "../src/pages/pipelines/PipelineEditor";
import type { Manifest } from "../src/api/manifest";

/** One field edited in a form proposes one field (T-0885): the seed manifests of dev, round-tripped. */
describe("the Endpoint form", () => {
  const stored: Manifest = {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "helsinki-bikes", namespace: "helsinki", title: { en: "Helsinki city bike stations" } },
    spec: {
      contextSpaceRef: "helsinki",
      slug: "helsinki-bikes",
      audience: "public",
      enabledRepresentations: ["ngsi-ld", "csv"],
      rateLimits: { requestsPerMinute: 600 },
      policyRef: "urn:ngsi-ld:Policy:hel.fi:helsinki:public-bikes",
      publish: { ckan: { instanceRef: { kind: "CkanInstance", name: "hel-fi" } } },
    },
  };

  it("proposes the one field it changed and nothing else", () => {
    const form = endpointForm(stored);
    form.rateLimits = { requestsPerMinute: 300 };
    const out = endpointEnvelope("helsinki", form, "helsinki-bikes", [], undefined, stored);
    expect(out).toEqual({ ...stored, spec: { ...stored.spec, rateLimits: { requestsPerMinute: 300 } } });
  });

  it("removes what the YAML view's document no longer holds", () => {
    // The typed document is the base: a policyRef deleted there is proposed as a removal.
    const { policyRef: _gone, ...spec } = stored.spec;
    void _gone;
    const typed = { ...stored, spec };
    const out = endpointEnvelope("helsinki", endpointForm(typed), "helsinki-bikes", [], undefined, typed);
    expect(out.spec).not.toHaveProperty("policyRef");
    expect(out.spec.publish).toEqual(stored.spec.publish);
  });

  it("drops a title the form cleared and keeps one it never showed", () => {
    const form = endpointForm(stored);
    form.title = "";
    expect(endpointEnvelope("helsinki", form, "helsinki-bikes", [], undefined, stored).metadata).not.toHaveProperty("title");
    const described = { ...stored, metadata: { ...stored.metadata, description: { en: "Dock stations" } } };
    expect(endpointEnvelope("helsinki", endpointForm(described), "helsinki-bikes", [], undefined, described).metadata.description).toEqual({ en: "Dock stations" });
  });

  it("writes a new endpoint from the form alone", () => {
    const out = endpointEnvelope("helsinki", endpointForm(stored), "helsinki-bikes", []);
    expect(out.metadata.labels).toEqual({ "joinedcontext.com/space": "helsinki" });
    expect(out.spec).not.toHaveProperty("policyRef");
  });
});

describe("the Pipeline form", () => {
  const stored: Manifest = {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Pipeline",
    metadata: { name: "hel-news", namespace: "helsinki", title: { en: "City news from the hel.fi RSS feed" } },
    spec: {
      class: "batch",
      schedule: "*/15 * * * *",
      source: { dataSourceRef: { kind: "DataSource", name: "hel-rss" } },
      output: { type: "NewsArticle", mode: "upsert" },
      enabled: true,
    },
  };

  it("keeps enabled from the manifest it edits", () => {
    const out = pipelineEnvelope("helsinki", fromManifest(stored), stored);
    expect(out).toEqual(stored);
  });

  it("takes enabled: false typed in the YAML view as the pause it is", () => {
    const typed = { ...stored, spec: { ...stored.spec, enabled: false } };
    const out = pipelineEnvelope("helsinki", fromManifest(typed), typed);
    expect(out.spec.enabled).toBe(false);
    expect(out.metadata.title).toEqual({ en: "City news from the hel.fi RSS feed" });
  });

  it("carries spec.lifetime both ways and drops a lifetime the form emptied (T-4839)", () => {
    const lifetime = [{ type: "NewsArticle", entity: { from: "datePublished", plus: "P30D" } }];
    const timed = { ...stored, spec: { ...stored.spec, lifetime } };
    const form = fromManifest(timed);
    expect(form.lifetime).toEqual(lifetime);
    expect(pipelineEnvelope("helsinki", form, timed)).toEqual(timed);
    expect(pipelineEnvelope("helsinki", { ...form, lifetime: undefined }, timed).spec.lifetime).toBeUndefined();
    expect(pipelineEnvelope("helsinki", { ...form, lifetime: [] }, timed).spec.lifetime).toBeUndefined();
  });
});
