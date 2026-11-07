import { describe, expect, it } from "vitest";
import { allowsIn } from "../src/api/permissions";
import type { Effective } from "../src/api/permissions";
import type { Manifest } from "../src/api/manifest";
import { reachableEndpoints } from "../src/pages/pipelines/PipelineEditor";

// PL-94, T-4893: the pipeline editor offers only Endpoints the person reads through.
function endpoint(name: string, space: string, audience?: string): Manifest {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name, namespace: "helsinki" },
    spec: { contextSpaceRef: space, slug: name, ...(audience ? { audience } : {}) },
  } as Manifest;
}

const DEVELOPER = {
  project: "helsinki",
  bootstrap: false,
  grants: [
    { role: "pipeline-editor", binding: "dev", scope: "project:helsinki", rule: { kinds: ["Pipeline"], verbs: ["propose"] } },
    { role: "space-reader", binding: "dev-a", scope: "contextSpace:a", space: "a", rule: { kinds: ["Entity"], verbs: ["read"] } },
  ],
} as unknown as Effective;

describe("reachableEndpoints", () => {
  const all = [endpoint("ep-a", "a", "internal"), endpoint("ep-b", "b", "internal"), endpoint("ep-b-public", "b", "public"), endpoint("ep-b-default", "b")];

  it("keeps public Endpoints and those into a space the person reads", () => {
    expect(reachableEndpoints(DEVELOPER, all).map((e) => e.metadata.name)).toEqual(["ep-a", "ep-b-public"]);
  });

  it("counts an Endpoint without an audience as closed, as jc-core requires one (F-R2UI3-001)", () => {
    expect(reachableEndpoints(DEVELOPER, [endpoint("ep-b-default", "b")])).toEqual([]);
  });

  it("keeps every Endpoint while the permissions are unknown or for the bootstrap group", () => {
    expect(reachableEndpoints(undefined, all)).toHaveLength(4);
    expect(reachableEndpoints({ ...DEVELOPER, bootstrap: true }, all)).toHaveLength(4);
  });

  it("drops an Endpoint whose space is not named", () => {
    expect(reachableEndpoints(DEVELOPER, [endpoint("ep-none", "", "internal")])).toEqual([]);
  });
});

describe("allowsIn", () => {
  it("reads a space-bound grant only in its space, and propose implies read", () => {
    expect(allowsIn(DEVELOPER, "Entity", "read", "a")).toBe(true);
    expect(allowsIn(DEVELOPER, "Entity", "read", "b")).toBe(false);
    expect(allowsIn(DEVELOPER, "Pipeline", "read", "b")).toBe(true);
    expect(allowsIn(DEVELOPER, "Pipeline", "approve", "a")).toBe(false);
  });
});
