/**
 * T-2631, EP-14, EP-15: a page reads only through an endpoint whose audience admits the person, the
 * gateway's own rule (context-gateway `identify` + `Endpoint::admits`), so it never fetches to
 * meet a 403: the approver's walk met exactly that on the space and assistant pages of a project
 * whose group they are not in.
 */
import { describe, expect, it } from "vitest";
import type { Manifest } from "../src/api/manifest";
import { admitsPerson } from "../src/components/endpoints/sharing";
import { pickReadEndpoint } from "../src/pages/spaces/spaceFacts";

const endpoint = (name: string, spec: Record<string, unknown>, namespace?: string): Manifest =>
  ({
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name, ...(namespace ? { namespace } : {}) },
    spec: { slug: `${name}-slug`, ...spec },
  }) as unknown as Manifest;

describe("whether an endpoint admits a person", () => {
  it.each([
    ["public", { audience: "public" }, [], true],
    ["organization, no group", { audience: "organization" }, [], true],
    ["project-list, a member of the owner", { audience: "project-list" }, ["bbsk"], true],
    ["project-list, a member of a listed project", { audience: "project-list", allowedProjects: ["mesto"] }, ["mesto"], true],
    ["project-list, a member of another project", { audience: "project-list", allowedProjects: ["mesto"] }, ["helsinki"], false],
    ["no audience is project-list", {}, [], false],
  ])("%s", (_, spec, groups, admitted) => {
    expect(admitsPerson(endpoint("e", spec, "bbsk"), groups, "bbsk")).toBe(admitted);
  });

  it("takes the page's project as the owner of a manifest that names no namespace", () => {
    expect(admitsPerson(endpoint("e", {}), ["bbsk"], "bbsk")).toBe(true);
    expect(admitsPerson(endpoint("e", {}), ["bbsk"], "helsinki")).toBe(false);
  });
});

describe("the endpoint a space is read through", () => {
  const closed = endpoint("closed", { audience: "project-list" }, "bbsk");
  const open = endpoint("open", { audience: "public", policyRef: { name: "p" } }, "bbsk");

  it("skips an endpoint that would refuse the person", () => {
    expect(pickReadEndpoint([closed, open], ["helsinki"], "bbsk")?.metadata.name).toBe("open");
    expect(pickReadEndpoint([closed, open], ["bbsk"], "bbsk")?.metadata.name).toBe("closed");
    expect(pickReadEndpoint([closed], ["helsinki"], "bbsk")).toBeUndefined();
  });

  it("picks none while the person is not known yet, and every one when no one is asked about", () => {
    expect(pickReadEndpoint([closed, open], null, "bbsk")).toBeUndefined();
    expect(pickReadEndpoint([closed, open])?.metadata.name).toBe("closed");
  });
});
