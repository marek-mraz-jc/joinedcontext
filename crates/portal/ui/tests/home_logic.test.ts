// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/home/home.ts,
// the first-run steps and the role cards of the project home (T-3233, T-3235).
import { describe, expect, it } from "vitest";
import { cardsFor, draftPage, firstRunSteps, roleOf, sampleToTry, welcomeStep } from "../src/pages/home/home";
import type { Effective } from "../src/api/permissions";

const grants = (...verbs: string[]): Effective =>
  ({ project: "helsinki", grants: [{ rule: { kinds: ["Pipeline"], verbs } }] }) as unknown as Effective;

describe("the first-dataset steps (T-3233, T-5109)", () => {
  it("tick themselves from what the project holds, never from a click", () => {
    const empty = firstRunSteps("helsinki", { endpoints: [], published: 0 });
    expect(empty.map((step) => step.key)).toEqual(["upload", "share", "publish"]);
    expect(empty.map((step) => step.done)).toEqual([false, false, false]);
    expect(empty.map((step) => step.to)).toEqual([
      "/projects/helsinki/spaces/upload",
      "/projects/helsinki/endpoints/new",
      "/projects/helsinki/endpoints",
    ]);
    // The upload proposes the Endpoint with the space; it may land before the rows do.
    const proposed = firstRunSteps("helsinki", { endpoints: ["parking lots"], published: 0 });
    expect(proposed.map((step) => step.done)).toEqual([false, true, false]);
    expect(proposed[2].to).toBe("/projects/helsinki/endpoints/parking%20lots");
    const all = firstRunSteps("helsinki", { endpoints: ["parking"], published: 1, spaceWithData: "parking" });
    expect(all.every((step) => step.done)).toBe(true);
  });
});

describe("the role and its cards (T-3235)", () => {
  it("reads the role from what the grants let the person do", () => {
    expect(roleOf(undefined)).toBeUndefined();
    expect(roleOf(grants("read"))).toBe("viewer");
    expect(roleOf(grants("read", "propose"))).toBe("editor");
    expect(roleOf(grants("propose", "approve"))).toBe("steward");
    expect(roleOf({ bootstrap: true, grants: [] } as unknown as Effective)).toBe("steward");
  });

  it("shows each role what it acts on, one click away, and no card that would say zero", () => {
    expect(cardsFor("helsinki", "steward", { approvalsWaiting: 2, failingPipelines: 0, invitesPending: 1 })).toEqual([
      { key: "approvals", count: 2, to: "/projects/helsinki/approvals" },
      { key: "invites", count: 1, to: "/organization/people" },
    ]);
    expect(cardsFor("helsinki", "editor", { drafts: { count: 3, to: "/x" }, failingPipelines: 1 }).map((card) => card.key)).toEqual(["drafts", "failing"]);
    expect(cardsFor("helsinki", "viewer", { spaceWithData: "air", dashboards: ["overview"] })).toEqual([
      { key: "data", count: 1, to: "/projects/helsinki/spaces/air" },
      { key: "dashboards", count: 1, to: "/projects/helsinki/dashboards/overview" },
    ]);
    expect(cardsFor("helsinki", "viewer", {})).toEqual([]);
  });

  it("resumes a draft on its kind's page, and names no page for a kind without one", () => {
    expect(draftPage("helsinki", "Pipeline", "air feed")).toBe("/projects/helsinki/pipelines?draft=air%20feed");
    expect(draftPage("helsinki", "Role", "x")).toBeUndefined();
  });
});

describe("a newcomer's first step (PF-108)", () => {
  it("takes each role where it starts", () => {
    expect(welcomeStep("helsinki", "viewer")).toBe("/projects/helsinki/spaces");
    expect(welcomeStep("helsinki", "editor")).toBe("/projects/helsinki/datasources/new");
    expect(welcomeStep("helsinki", "steward")).toBe("/projects/helsinki/approvals");
  });
});

describe("the sample to try (PF-109)", () => {
  it("offers the first sample other than the project in hand, and none when there is none", () => {
    expect(sampleToTry("helsinki", ["banskabystrica", "praha"])).toBe("banskabystrica");
    expect(sampleToTry("banskabystrica", ["banskabystrica", "praha"])).toBe("praha");
    expect(sampleToTry("banskabystrica", ["banskabystrica"])).toBeUndefined();
    expect(sampleToTry("helsinki", [])).toBeUndefined();
  });
});
