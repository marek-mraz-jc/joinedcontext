/**
 * T-6081: which open Changes a live journey left behind. A journey names what it proposes after
 * itself (`t1585-…`, `checklist-<base36 time>`), so a pending Change whose resource carries such a
 * name and is older than a day is residue nobody will approve: on dev it buried real proposals
 * under 23 test ones in helsinki's Approvals. Pure, so a unit test reads it without a browser.
 */

/** One item of `GET /api/v1/projects/{project}/changes`, the fields this check reads. */
export interface ListedChange {
  metadata: { name: string };
  status: { phase: string };
  summary?: { params?: { name?: unknown; kind?: unknown } };
  createdAt?: string;
}

/**
 * The names live journeys give what they propose: a task prefix (`t1585-`, `t2729-imp-`) or a
 * journey's own word followed by the base36 time it started (`Date.now().toString(36)`).
 */
const TEST_NAME =
  /^(t[0-9]{4}[a-z]?-.+|(checklist|history|restore|inbox|journey|journey-paths|walk-app|walk-analyst|newuser-policy)-[0-9a-z]{8})$/;

export const DAY_MS = 24 * 60 * 60 * 1000;

/** The resource name a Change proposes, or "" when the listing names none. */
export function changeSubject(change: ListedChange): string {
  const name = change.summary?.params?.name;
  return typeof name === "string" ? name : "";
}

/** Whether a resource name is one a live journey made. */
export function isTestName(name: string): boolean {
  return TEST_NAME.test(name);
}

/**
 * The pending Changes a journey made that have waited longer than `maxAgeMs` at `now`. A Change
 * without a readable `createdAt` counts as stale: an unknown age is no reason to keep it hidden.
 */
export function staleTestChanges(items: ListedChange[], now: number, maxAgeMs = DAY_MS): ListedChange[] {
  return items.filter((change) => {
    if (change.status.phase !== "PendingApproval" || !isTestName(changeSubject(change))) return false;
    const created = Date.parse(change.createdAt ?? "");
    return Number.isNaN(created) || now - created > maxAgeMs;
  });
}
