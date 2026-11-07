/**
 * Sweeps what takes and e2e left behind in helsinki (T-0667, T-0750): every resource whose name
 * carries a take's HHMM suffix, or the fixed name an act creates, is deleted through the
 * Portal's own delete, a Red change the janitor approves with the name typed back, so the
 * reconciler drops what it had deployed. Dependents first, then the spaces, then the models the spaces referenced.
 * Only these names, only this project; nothing to sweep is a pass. The janitor's role reaches these
 * names and no other (T-2627, deployment seed/helsinki/helsinki-role-janitor.yaml), so a name added
 * here that the role's pattern does not cover is refused at the approval, not swept by accident.
 *
 * A copy is swept too, and by its own door (T-1592): a workspace is not a resource under
 * `/projects/{project}/{plural}`, it is discarded, and an expired one is invisible to the list it
 * is no longer in. Until this, a copy a take or a journey left behind was residue nobody looked
 * at — the sweep knew seven plurals and not this one.
 */
import { expect, test } from "@playwright/test";
import type { BrowserContext, Page } from "@playwright/test";
import { JANITOR, STEWARD, approve, csrf, listedNames, signIn } from "./portal";
import { DAY_MS, type ListedChange, changeSubject, staleTestChanges } from "./staleChanges";

const PROJECT = "helsinki";
/** A take's suffix is the HH:MM it started at; an act with a fixed name is listed by it. */
const TAKE = /-\d{4}$/;
/**
 * A live journey names what it creates after its own task, so its leftovers are recognisable:
 * `t1585-…`, `t1588-…`, `t1589r-…`, `t4843-…`. A journey that approves a change cannot simply DELETE
 * what it made — a removal is a Red change that needs an approval too — so anything a journey
 * abandons is swept here (T-1592, and the six spaces this rule found on dev on 2026-09-18). Any
 * four-digit task counts: the t1xxx-only rule missed T-4668's models and T-4843's space (T-4993).
 */
const JOURNEY = /^t[0-9]{4}[a-z]?-/;
const SWEEP: { plural: string; residue: (name: string) => boolean }[] = [
  // An assistant names its knowledge sources, so it goes before them (T-4869's journey makes both).
  { plural: "assistantdeployments", residue: (name) => JOURNEY.test(name) },
  { plural: "knowledgesources", residue: (name) => JOURNEY.test(name) },
  { plural: "syncsources", residue: (name) => JOURNEY.test(name) },
  // The policy-new-user journey's policy (T-4993 found two on dev).
  { plural: "policies", residue: (name) => JOURNEY.test(name) || /^newuser-policy-[a-z0-9]+$/.test(name) },
  { plural: "endpoints", residue: (name) => JOURNEY.test(name) || /^bikes-regional-[a-z0-9]+$/.test(name) },
  { plural: "pipelines", residue: (name) => JOURNEY.test(name) || TAKE.test(name) },
  { plural: "datasources", residue: (name) => JOURNEY.test(name) || TAKE.test(name) },
  // `city-bikes`, which a take once created, is the seeded dashboard now: never residue.
  { plural: "dashboards", residue: (name) => JOURNEY.test(name) || TAKE.test(name) },
  // A dashboard names its layers, so the layers go after it (T-1546).
  { plural: "layers", residue: (name) => JOURNEY.test(name) },
  // What the kind journeys make (T-1539, T-1540): nothing refers to them.
  { plural: "csrs", residue: (name) => JOURNEY.test(name) },
  { plural: "ckaninstances", residue: (name) => JOURNEY.test(name) },
  { plural: "subscriptions", residue: (name) => JOURNEY.test(name) },
  // A journey's service account (T-1550): nothing it made refers to it once the journey ends.
  { plural: "serviceaccounts", residue: (name) => JOURNEY.test(name) },
  { plural: "apps", residue: (name) => JOURNEY.test(name) || TAKE.test(name) || name === "large-map-city" },
  // A space references its model (dataModelRef), so the space goes first.
  { plural: "spaces", residue: (name) => /^citybikes-\d{4}$/.test(name) || JOURNEY.test(name) },
  { plural: "datamodels", residue: (name) => /^citybikes-\d{4}$/.test(name) || JOURNEY.test(name) },
];

test.setTimeout(3_600_000);
test.use({ trace: "off" });

/**
 * The Portal's own delete through the session: a Red change, its id returned (CC-19). A removal of
 * the same resource a failed sweep left open is that change, approved now; any other open change
 * is a person's, and the sweep stops on it.
 */
async function proposeDelete(page: Page, context: BrowserContext, plural: string, name: string): Promise<string> {
  const csrf = (await context.cookies()).find((cookie) => cookie.name === "__Host-jc_csrf")?.value ?? "";
  const answer = await page.request.delete(`/api/v1/projects/${PROJECT}/${plural}/${name}`, {
    headers: { "x-csrf-token": csrf },
  });
  const open = answer.status() === 409 ? /already open: (chg-[0-9a-f]+)/.exec(await answer.text())?.[1] : undefined;
  if (open) {
    const change = (await (await page.request.get(`/api/v1/projects/${PROJECT}/changes/${open}`)).json()) as {
      summary?: { key?: string; params?: { name?: string } };
    };
    expect(change.summary, `${open} is open on ${plural}/${name} and is not its removal`).toMatchObject({
      key: "change.summary.delete",
      params: { name },
    });
    return open;
  }
  expect(answer.status(), `delete ${plural}/${name}: ${await answer.text()}`).toBe(202);
  return ((await answer.json()) as { metadata: { name: string } }).metadata.name;
}

/** The copies of this project, as their own route answers them (T-1592). */
async function listedCopies(page: Page): Promise<{ name: string; expiresAt: string }[]> {
  const answer = await page.request.get(`/api/v1/projects/${PROJECT}/workspaces`);
  expect(answer.ok(), `list workspaces: ${await answer.text()}`).toBe(true);
  const body = (await answer.json()) as { items?: { name: string; expiresAt: string }[] };
  return body.items ?? [];
}

/**
 * A copy a take or a journey left behind: its name carries a take's suffix or a journey's prefix,
 * or it is already past its own `expiresAt` and still listed — which is residue whichever name it
 * carries, because a copy nobody discarded holds a branch and may hold a preview slot.
 */
function copyResidue(copy: { name: string; expiresAt: string }): boolean {
  return (
    TAKE.test(copy.name) ||
    JOURNEY.test(copy.name) ||
    /^(e2e|journey)-/.test(copy.name) ||
    Date.parse(copy.expiresAt) < Date.now()
  );
}

test("no copy of a take or a journey is left in helsinki", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/workspaces?lang=en`);
  try {
    const found = (await listedCopies(page)).filter(copyResidue).map((copy) => copy.name);
    test.info().annotations.push({ type: "copies", description: found.join(", ") || "none" });
    const token = (await context.cookies()).find((each) => each.name === "__Host-jc_csrf")?.value ?? "";
    expect(token, "no __Host-jc_csrf cookie: the session did not complete").not.toBe("");
    for (const name of found) {
      // A copy is discarded, not proposed away: no Change, no approval, and its branch goes with
      // it (`discarding_removes_the_branch_and_the_record`).
      const answer = await page.request.delete(`/api/v1/projects/${PROJECT}/workspaces/${name}`, {
        headers: { "x-csrf-token": token },
      });
      expect([204, 404], `discard ${name}: ${await answer.text()}`).toContain(answer.status());
    }
    await expect
      .poll(async () => (await listedCopies(page)).filter(copyResidue).map((copy) => copy.name), {
        message: "copies still listed after their discard",
        timeout: 120_000,
        intervals: [5_000],
      })
      .toEqual([]);
  } finally {
    await context.close();
  }
});

test("no residue of takes or e2e is left in helsinki", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/endpoints?lang=en`);
  const found: string[] = [];
  for (const { plural, residue } of SWEEP) {
    const names = (await listedNames(steward.page, PROJECT, plural)).filter(residue);
    found.push(...names.map((name) => `${plural}/${name}`));
  }
  test.info().annotations.push({ type: "residue", description: found.join(", ") || "none" });
  if (found.length > 0) {
    const approver = await signIn(browser, JANITOR, `/projects/${PROJECT}/approvals?lang=en`);
    for (const item of found) {
      const [plural, name] = item.split("/");
      const change = await proposeDelete(steward.page, steward.context, plural, name);
      await approve(approver.page, PROJECT, change, name);
    }
    await approver.context.close();
  }
  for (const { plural, residue } of SWEEP) {
    await expect
      .poll(async () => (await listedNames(steward.page, PROJECT, plural)).filter(residue), {
        message: `${plural} still list residue`,
        timeout: 300_000,
        intervals: [10_000],
      })
      .toEqual([]);
  }
  await steward.context.close();
});

/**
 * A proposal a journey made and nobody decided (T-4993: Journey 44's KnowledgeSource t4869-files-…
 * waited on dev's approvals page for days; T-6081: 23 checklist-, history-, restore- and t1xxx-
 * proposals in helsinki back to 2026-10-07). Every project the steward reads is listed, since a
 * journey may propose outside helsinki; a create, an update and a removal count alike. One hour
 * is longer than any journey runs, so a journey still at work keeps its own.
 */
const SWEPT_AFTER_MS = 60 * 60 * 1000;

async function openJourneyChanges(page: Page, maxAgeMs: number): Promise<{ project: string; change: string; name: string }[]> {
  const projects = await page.request.get("/api/v1/projects");
  expect(projects.ok(), `list projects: ${await projects.text()}`).toBe(true);
  const found: { project: string; change: string; name: string }[] = [];
  for (const { name: project } of ((await projects.json()) as { items?: { name: string }[] }).items ?? []) {
    const answer = await page.request.get(`/api/v1/projects/${project}/changes`);
    expect(answer.ok(), `list changes of ${project}: ${await answer.text()}`).toBe(true);
    const items = ((await answer.json()) as { items?: ListedChange[] }).items ?? [];
    for (const change of staleTestChanges(items, Date.now(), maxAgeMs)) {
      found.push({ project, change: change.metadata.name, name: changeSubject(change) });
    }
  }
  return found;
}

test("no open proposal of a journey is left in any project", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/approvals?lang=en`);
  try {
    // The count the check reports: journey proposals that waited longer than a day (T-6081).
    const stale = await openJourneyChanges(steward.page, DAY_MS);
    test.info().annotations.push({
      type: "stale journey proposals (> 24 h)",
      description: `${stale.length}${stale.length ? `: ${stale.map((each) => `${each.project}/${each.name} ${each.change}`).join(", ")}` : ""}`,
    });
    // The steward rejects, through the approver door every person uses: the janitor's Role is
    // bound in helsinki only and its name pattern predates the checklist-/history-/restore-
    // journeys. Only names `isTestName` reads as a journey's are ever touched.
    for (const { project, change, name } of await openJourneyChanges(steward.page, SWEPT_AFTER_MS)) {
      const answer = await steward.page.request.post(`/api/v1/projects/${project}/changes/${change}/reject`, {
        headers: { "x-csrf-token": await csrf(steward.context), "content-type": "application/json" },
        data: { reason: `A live journey ended before this proposal of ${name} was decided; the residue sweep rejects it (T-6081).` },
      });
      expect(answer.status(), `reject ${project}/${change}: ${await answer.text()}`).toBe(202);
    }
    expect(await openJourneyChanges(steward.page, SWEPT_AFTER_MS)).toEqual([]);
  } finally {
    await steward.context.close();
  }
});

/**
 * A journey's draft is the steward's own: the drafts page listed twenty t4869-…, t4866-… and
 * t4824-… drafts on dev (T-4993). Each is dropped through the same route the page's Discard uses;
 * a draft of another person, or one without a journey's name, is left alone.
 */
test("no draft of a journey is left in helsinki", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/drafts?lang=en`);
  const mine = async (): Promise<{ kind: string; name: string }[]> => {
    const answer = await steward.page.request.get(`/api/v1/projects/${PROJECT}/drafts`);
    expect(answer.ok(), `list drafts: ${await answer.text()}`).toBe(true);
    const body = (await answer.json()) as { items?: { kind: string; name: string; touchedBy?: string }[] };
    return (body.items ?? []).filter((draft) => draft.touchedBy === STEWARD.user && JOURNEY.test(draft.name));
  };
  try {
    const found = await mine();
    test.info().annotations.push({ type: "drafts", description: found.map((d) => `${d.kind}/${d.name}`).join(", ") || "none" });
    const token = await csrf(steward.context);
    for (const draft of found) {
      const answer = await steward.page.request.delete(
        `/api/v1/projects/${PROJECT}/drafts/${encodeURIComponent(draft.kind)}/${encodeURIComponent(draft.name)}`,
        { headers: { "x-csrf-token": token } },
      );
      expect([200, 204, 404], `drop draft ${draft.kind}/${draft.name}: ${await answer.text()}`).toContain(answer.status());
    }
    expect(await mine()).toEqual([]);
  } finally {
    await steward.context.close();
  }
});
