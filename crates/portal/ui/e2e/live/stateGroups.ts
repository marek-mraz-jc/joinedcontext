/**
 * T-6404: which live specs share state, so the sweep may run the rest on several Playwright workers.
 * A spec named nowhere here runs beside any other. A group's specs run one at a time, beside other
 * groups and the free specs. The exclusive specs run alone, one at a time, after everything else has
 * finished, failures included (a Playwright teardown project). tests/live_state_groups.test.ts reads
 * every spec and fails when two specs outside one group share a fixed object name, or when a spec
 * that grants a demo person a right is not exclusive and not listed in NARROW_GRANTS.
 */

export interface StateGroup {
  /** What the specs share, so a reader can tell when the group may go. */
  shares: string;
  /** Spec file names under e2e/live. */
  specs: string[];
}

/** Run alone at the end: each changes what every other spec sees. */
export const EXCLUSIVE: StateGroup = {
  shares:
    "helsinki's journey residue and demo.viewer's rights: residue deletes every t<task>- resource, so it would " +
    "remove what a running spec just made; organization and project-settings grant demo.viewer a role for a " +
    "while, and every viewer-refusal spec would meet a viewer who may write",
  specs: ["residue.spec.ts", "organization.spec.ts", "project-settings.spec.ts"],
};

/** Specs that share one object or one per-person limit, each group on one worker at a time. */
export const GROUPS: Record<string, StateGroup> = {
  "helsinki-alerts": {
    shares: "the helsinki-alerts App: people grants its steward role, readiness opens it as the viewer and a stranger",
    specs: ["people.spec.ts", "readiness.spec.ts"],
  },
  "helsinki-mobility": {
    shares: "the helsinki-mobility project and its forge repository: kind-bundles imports into it, project-repos clones and pushes it",
    specs: ["kind-bundles.spec.ts", "project-repos.spec.ts"],
  },
  clips: {
    shares: "the t6454- names and drafts of the clip journeys, which record one person's screen in turn",
    specs: ["clips_connector.spec.ts", "clips_dashboard.spec.ts", "clips_space_complete.spec.ts"],
  },
  "mcp-minute": {
    shares: "the Portal MCP door's per-person minute (60 calls; the seeded people sign in to it)",
    specs: [
      "an-mcp-client-does-the-load-journey-with.spec.ts",
      "audit-forms-access.spec.ts",
      "connect.spec.ts",
      "copy-agent.spec.ts",
      "mcp-question.spec.ts",
      "mcp-servers.spec.ts",
      "walk-app-builder.spec.ts",
      "walk-approver-queue.spec.ts",
      "walk-citizen.spec.ts",
    ],
  },
};

/**
 * Specs that name a demo person in a grant and still run beside the others, with the reason the grant
 * reaches nothing another spec reads.
 */
export const NARROW_GRANTS: Record<string, string> = {
  "roles-refusals.spec.ts": "every grant it sends is refused at the door; none is approved",
  "walk-steward-dataset.spec.ts": "demo.viewer's group reads only the walk's own walk-steward- space",
  "walk-approver-queue.spec.ts": "its team group is bound by no role and rejected at the end",
};

/**
 * Fixed names several free specs may hold, with the reason none of them changes the object: the
 * seeded project and the seeded objects specs only read or write their own t<task>- objects into.
 */
export const SHARED_SEEDS: Record<string, string> = {
  helsinki: "the seeded project every journey works in; each names what it makes after its task or its start time",
  "helsinki-events": "a seeded Endpoint and App the specs only read",
  "urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all": "the seeded write target; the pipelines aimed at it are each spec's own",
};

/** A Playwright project per group, one for the free specs and the exclusive one last. */
export function projects<T extends object>(use: T) {
  const grouped = [...EXCLUSIVE.specs, ...Object.values(GROUPS).flatMap((group) => group.specs)];
  const match = (specs: string[]) => specs.map((spec) => new RegExp(`/${spec.replace(/\./g, "\\.")}$`));
  return [
    { name: "chromium", use, testIgnore: match(grouped), teardown: "exclusive" },
    ...Object.entries(GROUPS).map(([name, group]) => ({
      name,
      use,
      testMatch: match(group.specs),
      workers: 1,
      teardown: "exclusive",
    })),
    { name: "exclusive", use, testMatch: match(EXCLUSIVE.specs), workers: 1 },
  ];
}
