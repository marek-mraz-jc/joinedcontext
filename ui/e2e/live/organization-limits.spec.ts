/**
 * T-2717 — Organization limits on dev: what an organization admin sets in Organization settings
 * is enforced at the Portal's doors once the change is approved, then restored (ADR-N-035,
 * PF-101, PF-103, PF-73).
 *
 * `demo.steward` administers the organization (`org-admin`), so they approve their own red-lane
 * Organization change as the administrator exception (PF-58), as `project-settings.spec.ts` does.
 *
 * 1. Public apps refused (`spec.policies.apps.public`): making one of helsinki's Apps public is
 *    refused, naming the policy.
 * 2. A lowered quota (`spec.projects.quota.contextSpaces`): helsinki keeps no quota of its own,
 *    so the organization's default is in force there (PF-73). It is set to exactly the spaces
 *    helsinki holds, so nothing already standing goes beyond it, and the next space is refused
 *    with the quota named.
 *
 * Each refusal may come from the check or from the proposal: every door refuses it, and a door
 * that answers must name the limit. Nothing is proposed that could stand: both refusals happen
 * before a Change exists. The Organization goes back to the spec it had, approved, in `finally`.
 */
import { isDeepStrictEqual } from "node:util";
import { expect, test } from "@playwright/test";
import type { BrowserContext, Page } from "@playwright/test";
import { STEWARD, approve, csrf, signIn } from "./portal";

const PROJECT = "helsinki";
const SPACE = "t2717-over-quota";

test.setTimeout(900_000);

type Session = { context: BrowserContext; page: Page };
type Manifest = { apiVersion: string; kind: string; metadata: { name: string; namespace?: string }; spec: Record<string, unknown> };

/** Every item of a list route, following its continue token. */
async function all(page: Page, route: string): Promise<Manifest[]> {
  const items: Manifest[] = [];
  let next = "";
  do {
    const answer = await page.request.get(`${route}?limit=500${next ? `&continue=${encodeURIComponent(next)}` : ""}`);
    expect(answer.ok(), `${route}: ${answer.status()}`).toBe(true);
    const body = (await answer.json()) as { items: Manifest[]; metadata?: { continue?: string; continueToken?: string } };
    items.push(...body.items);
    next = body.metadata?.continue ?? body.metadata?.continueToken ?? "";
  } while (next);
  return items;
}

/** The organization's one Organization manifest, as stored. */
async function organization(page: Page): Promise<Manifest> {
  const [org] = await all(page, "/api/v1/projects/org/organizations");
  expect(org, "the organization has its manifest").toBeTruthy();
  return { apiVersion: org.apiVersion, kind: org.kind, metadata: { name: org.metadata.name, namespace: "org" }, spec: org.spec };
}

/** Proposes the Organization with `spec` and approves it, typing its name back (a red lane, CC-19). */
async function setOrganization(owner: Session, current: Manifest, spec: Record<string, unknown>): Promise<void> {
  const answer = await owner.page.request.put(`/api/v1/projects/org/organizations/${current.metadata.name}`, {
    headers: { "x-csrf-token": await csrf(owner.context) },
    data: { ...current, spec },
  });
  expect(answer.status(), await answer.text()).toBe(202);
  const change = ((await answer.json()) as { metadata?: { name?: string } }).metadata?.name ?? "";
  expect(change, "the proposal names its change").not.toBe("");
  await owner.page.goto(`/projects/org/approvals/${change}?lang=en`, { waitUntil: "load" });
  const input = owner.page.locator("#confirm-resource-name");
  await expect(input).toBeVisible({ timeout: 60_000 });
  await approve(owner.page, "org", change, (await input.getAttribute("placeholder")) ?? "");
}

/**
 * Proposes `manifest` the way a client does, its check first, and expects a refusal at one of
 * the two doors that names `rule`. Returns what the refusing door said.
 */
async function refused(owner: Session, method: "post" | "put", route: string, manifest: Manifest, rule: RegExp): Promise<string> {
  const headers = { "x-csrf-token": await csrf(owner.context), "content-type": "application/json" };
  const check = await owner.page.request.post(`/api/v1/projects/${PROJECT}/ops/jc_manifest_dry_run`, {
    headers,
    data: { manifest },
  });
  const checked = await check.text();
  const green = check.ok() && (JSON.parse(checked) as { verdict?: { ok?: boolean } }).verdict?.ok === true;
  if (!green) {
    expect(checked, "the check names the limit").toMatch(rule);
    return checked;
  }
  const answer = await owner.page.request[method](route, { headers, data: manifest });
  const said = await answer.text();
  expect(answer.status(), said).toBeGreaterThanOrEqual(400);
  expect(answer.status(), said).toBeLessThan(500);
  expect(said, "the proposal names the limit").toMatch(rule);
  return said;
}

test("an organization admin refuses public apps and lowers a quota; both are enforced, then restored", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, "/organization/settings?lang=en");
  const { page } = steward;
  const original = await organization(page);
  let changed = false;
  try {
    expect(
      (await all(page, `/api/v1/projects/${PROJECT}/spaces`)).some((space) => space.metadata.name === SPACE),
      "nothing is left over from an earlier run",
    ).toBe(false);
    // helsinki keeps no quota of its own, so the organization's default is the one in force.
    const project = await page.request.get(`/api/v1/projects/org/projects/${PROJECT}`);
    expect(project.ok()).toBe(true);
    expect(((await project.json()) as Manifest).spec.quotas, `${PROJECT} has its own quotas`).toBeUndefined();
    const held = (await all(page, `/api/v1/projects/${PROJECT}/spaces`)).length;
    expect(held, "helsinki holds spaces").toBeGreaterThan(0);

    const spec = structuredClone(original.spec) as {
      policies?: { apps?: Record<string, unknown> };
      projects?: { quota?: Record<string, unknown> };
    };
    spec.policies = { ...spec.policies, apps: { ...spec.policies?.apps, public: "refused" } };
    spec.projects = { ...spec.projects, quota: { ...spec.projects?.quota, contextSpaces: held } };
    changed = true;
    await setOrganization(steward, original, spec);

    // Settings shows what is now in force.
    await page.goto("/organization/settings?lang=en", { waitUntil: "load" });
    const policy = page.getByRole("row", { name: /Public apps/ });
    await expect(policy).toContainText("Refused", { timeout: 120_000 });

    // 1. A steward's app made public is refused, naming the policy.
    const apps = await all(page, `/api/v1/projects/${PROJECT}/apps`);
    const app = apps.find((one) => one.spec.visibility !== "public");
    expect(app, `${PROJECT} has an app that is not public`).toBeTruthy();
    if (app) {
      await expect
        .poll(
          async () =>
            refused(
              steward,
              "put",
              `/api/v1/projects/${PROJECT}/apps/${app.metadata.name}`,
              { apiVersion: app.apiVersion, kind: app.kind, metadata: app.metadata, spec: { ...app.spec, visibility: "public" } },
              /spec\.policies\.apps\.public/,
            ).then(() => true, () => false),
          { timeout: 120_000, message: "the approved policy reaches the Portal's doors" },
        )
        .toBe(true);
    }

    // 2. The next space is refused with the quota named.
    const said = await refused(
      steward,
      "post",
      `/api/v1/projects/${PROJECT}/spaces`,
      {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "ContextSpace",
        metadata: { name: SPACE, namespace: PROJECT },
        spec: { isSandbox: true },
      },
      /contextSpaces/,
    );
    expect(said).toContain(`${held + 1} of ${held}`);
  } finally {
    if (changed) {
      const now = await organization(page);
      if (!isDeepStrictEqual(now.spec, original.spec)) {
        await setOrganization(steward, now, original.spec);
      }
      await expect
        .poll(async () => (await organization(page)).spec, {
          timeout: 300_000,
          message: "the organization is back as the journey found it",
        })
        .toEqual(original.spec);
    }
    await steward.context.close();
  }
});
