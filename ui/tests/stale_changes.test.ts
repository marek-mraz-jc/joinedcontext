/**
 * T-6081: the live check counts a pending Change as journey residue only when its resource carries
 * a journey's name and it has waited longer than a day; a person's proposal is never counted.
 */
// covers: e2e/live/staleChanges.ts
import { describe, expect, it } from "vitest";
import { DAY_MS, type ListedChange, isTestName, staleTestChanges } from "../e2e/live/staleChanges";

const NOW = Date.parse("2026-10-29T18:00:00Z");

function change(name: string, createdAt: string | undefined, phase = "PendingApproval"): ListedChange {
  return { metadata: { name: `chg-${name}` }, status: { phase }, summary: { params: { kind: "App", name } }, createdAt };
}

describe("staleTestChanges", () => {
  it("names what the journeys on dev left behind", () => {
    for (const name of ["checklist-mvtmi3lh", "history-mvtmv53g", "restore-mvtnj8ib", "t1585-mf1x2abc", "t2729-imp-mvtmi3lh", "newuser-policy-mvtmi3lh"]) {
      expect(isTestName(name), name).toBe(true);
    }
  });

  it("never names a person's resource", () => {
    for (const name of ["alerts-desk", "helsinki-events", "history", "restore-air-quality", "t15-bikes", "checklist-mvtmi3lh-old", ""]) {
      expect(isTestName(name), name).toBe(false);
    }
  });

  it("keeps only pending journey Changes older than a day", () => {
    const old = new Date(NOW - DAY_MS - 1).toISOString();
    const fresh = new Date(NOW - DAY_MS + 60_000).toISOString();
    const items = [
      change("checklist-mvtmi3lh", old),
      change("history-mvtmv53g", fresh),
      change("restore-mvtnj8ib", old, "Deploying"),
      change("alerts-desk", "2026-10-07T09:00:00Z"),
      change("t1585-mf1x2abc", undefined),
    ];
    expect(staleTestChanges(items, NOW).map((each) => each.metadata.name)).toEqual(["chg-checklist-mvtmi3lh", "chg-t1585-mf1x2abc"]);
  });

  it("reads a Change whose listing names no resource as no residue", () => {
    expect(staleTestChanges([{ metadata: { name: "chg-0000002b" }, status: { phase: "PendingApproval" } }], NOW)).toEqual([]);
  });
});
