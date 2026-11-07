/**
 * T-5541 — an employee builds an App for their department, timed, on dev (ADR-N-027, -028, -030,
 * -031; AP-18, AP-93, AP-96, AP-173).
 *
 * The employee is `demo.author`, who holds `app-editor` in project `sharing` and nothing else; the
 * approver is `demo.approver`; the other department is `demo.neighbour`, a reader of
 * `sharing-other` only. Each step is one lap in `walk-report.json` beside the shots in
 * `$SHOTS_DIR` (1440 and 400 px): (1) the model key, (2) the App generated over the sharing
 * Endpoint with the roles viewer and editor, (3) the publish checklist, (4) publish + approve,
 * (5) the forge build, approve → `Served`, (6) opened framed and in its own window, the editor's
 * write through the App's own Endpoint, the neighbour's 403 page naming the roles, (7) shared with
 * the department Group `walk-dep-traffic`, (8) retired: default groups, Keycloak client and host
 * gone. Visibility is `roles` throughout; public is never chosen.
 *
 * The generator needs the Portal's model key. When it is out (or WALK_MODEL_KEY=out), step 1 says
 * "not run: key" and the rest is skipped: an app-editor has no path to an App without the model
 * (the template gallery opens the same generator), which is a finding of its own.
 *
 * The steward (an organization administrator) only creates and removes the walk's Group and
 * removes the retired App at the end, as `removeCompletely` needs `delete` (portal.ts).
 * Findings are soft expectations: a red one is listed in the report, the walk goes on, the test
 * ends red. Every wait is on the thing the step needs, never on the network going idle (T-2452).
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { Browser, BrowserContext, Page } from "@playwright/test";
import { APPROVER, AUTHOR, NEIGHBOUR, STEWARD, approve, checkManifest, csrf, goSignedIn, proposedChange, removeCompletely, signIn } from "./portal";
import { shot, writeIndex } from "./shoot";
import type { Shot } from "./shoot";

const PROJECT = "sharing";
const GROUP = "walk-dep-traffic";
const NAME = `walk-app-${Date.now().toString(36)}`;
const PORTAL = new URL(process.env.PORTAL_URL ?? "https://portal.dev.joinedcontext.com").origin;
/** `{name}.apps.{domain}`: the Portal's host with `portal.` swapped for `{name}.apps.` (T-4915). */
const APP_ORIGIN = PORTAL.replace("://portal.", `://${NAME}.apps.`);
const PROMPT =
  "A page for the traffic department: a table and a map of the city-bike docking stations with " +
  "their names. Two roles: viewer reads the stations, editor may also add a station.";

test.setTimeout(3_600_000);
test.describe.configure({ mode: "serial" });

interface Lap {
  step: string;
  ms: number;
  verdict: "pass" | "finding" | "not run";
  note?: string;
  shots: string[];
}
const laps: Lap[] = [];
let approvals = 0;
let author: { context: BrowserContext; page: Page } | undefined;
let steward: { context: BrowserContext; page: Page } | undefined;
let groupMade = false;
let appMade = false;

/** Runs one step, timed; a soft expectation that failed inside it marks it a finding. */
async function lap(step: string, body: (shots: string[]) => Promise<string | void>): Promise<void> {
  const shots: string[] = [];
  const before = test.info().errors.length;
  const started = Date.now();
  let note: string | void = undefined;
  try {
    note = await test.step(step, () => body(shots));
  } finally {
    const finding = test.info().errors.length > before;
    laps.push({ step, ms: Date.now() - started, verdict: finding ? "finding" : "pass", note: note ?? undefined, shots });
  }
}

/** The page at 1440 and at 400 px, then back to 1440. */
async function both(page: Page, shots: string[], what: Omit<Shot, "width" | "lang">): Promise<void> {
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: 900 });
    const file = await shot(page, { ...what, lang: "en" });
    if (file) shots.push(file);
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

/** Approves one change as `demo.approver` (CC-34: never the proposer). */
async function approved(browser: Browser, change: string, confirm?: string): Promise<void> {
  const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
  try {
    await approve(approver.page, PROJECT, change, confirm);
    approvals += 1;
  } finally {
    await approver.context.close();
  }
}

/** What Keycloak's authorization endpoint says of client `app-{name}` (ADR-N-030), without any credential. */
async function clientKnown(page: Page): Promise<boolean> {
  const metadata = await page.request.get(`${PORTAL}/.well-known/oauth-protected-resource/api/v1/mcp`);
  const issuer = ((await metadata.json()) as { authorization_servers?: string[] }).authorization_servers?.[0] ?? "";
  const query = new URLSearchParams({ client_id: `app-${NAME}`, response_type: "code", scope: "openid", redirect_uri: `${APP_ORIGIN}/` });
  const answer = await page.request.get(`${issuer.replace(/\/$/, "")}/protocol/openid-connect/auth?${query}`, { maxRedirects: 0 });
  return !/Client not found/i.test(await answer.text());
}

/** The App host's status for a stranger, or the network error's first line when nothing answers. */
async function hostAnswer(browser: Browser): Promise<string> {
  const stranger = await browser.newContext();
  try {
    return String((await stranger.request.get(`${APP_ORIGIN}/`, { maxRedirects: 0 })).status());
  } catch (error) {
    return `no answer: ${String(error).split("\n")[0]}`;
  } finally {
    await stranger.close();
  }
}

test.afterAll(async () => {
  if (steward) {
    if (appMade) await removeCompletely(steward, PROJECT, "apps", NAME).catch(() => undefined);
    if (groupMade) await removeCompletely(steward, "org", "groups", GROUP).catch(() => undefined);
    await steward.context.close();
  }
  await author?.context.close();
  const dir = process.env.SHOTS_DIR;
  if (dir) {
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, "walk-report.json"), `${JSON.stringify({ app: NAME, approvals, laps }, null, 2)}\n`);
    writeIndex(dir);
  }
});

test("an employee builds an App for their department, shares it with the department and retires it", async ({ browser }) => {
  // The department Group the App is shared with in step 7: an administrator's, made first.
  steward = await signIn(browser, STEWARD, "/organization/groups?lang=en");
  const existing = await steward.page.request.get(`/api/v1/projects/org/groups/${GROUP}`);
  expect(existing.status(), `${GROUP} is left over from an earlier walk: remove it first`).toBe(404);
  const groupManifest = {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Group",
    metadata: { name: GROUP, namespace: "org" },
    spec: { description: "Walkthrough T-5541: the traffic department.", members: [{ user: "demo.member@hel.fi" }] },
  };
  await checkManifest(steward.page, steward.context, "org", groupManifest);
  const proposedGroup = await steward.page.request.post("/api/v1/projects/org/groups", {
    headers: { "x-csrf-token": await csrf(steward.context), "content-type": "application/json" },
    data: groupManifest,
  });
  expect(proposedGroup.status(), await proposedGroup.text()).toBe(202);
  groupMade = true;
  const groupChange = ((await proposedGroup.json()) as { metadata?: { name?: string } }).metadata?.name ?? "";
  const approver = await signIn(browser, APPROVER, "/projects/org/approvals?lang=en");
  await approve(approver.page, "org", groupChange, GROUP).finally(() => approver.context.close());

  author = await signIn(browser, AUTHOR, `/projects/${PROJECT}/apps?lang=en`);
  const page = author.page;

  let keyOut = process.env.WALK_MODEL_KEY === "out";
  await lap("1 model key: generator or template", async () => (keyOut ? "not run: key (WALK_MODEL_KEY=out)" : "generator"));

  await lap("2 create the App over one department Endpoint (data needs, roles viewer and editor)", async (shots) => {
    if (keyOut) return "not run: key";
    await page.getByRole("button", { name: "Generate your own app" }).first().click();
    const endpoint = page.getByLabel("Endpoint").first();
    await endpoint.selectOption({ index: 1 });
    await page.getByLabel("What should the app do?").first().fill(PROMPT);
    await page.getByText("Details:", { exact: false }).first().click();
    await page.getByLabel("App name").fill(NAME);
    await both(page, shots, { area: "walk-app", route: `/projects/${PROJECT}/apps`, state: "generator-filled", who: "author" });
    const started = page.waitForResponse((answer) => answer.request().method() === "POST" && answer.url().endsWith(`/projects/${PROJECT}/agent-runs`));
    await page.getByRole("button", { name: "Generate the app" }).click();
    const answer = await started;
    if (answer.status() === 402 || answer.status() === 503) {
      keyOut = true;
      return `not run: key (${answer.status()} ${(await answer.text()).slice(0, 200)})`;
    }
    expect(answer.status(), await answer.text()).toBeLessThan(300);
    appMade = true;
    await page.waitForURL(new RegExp(`/projects/${PROJECT}/apps/${NAME}`), { timeout: 60_000 });
    await expect(page.getByTestId("run-progress").filter({ hasText: "Waiting for you" })).toBeVisible({ timeout: 900_000 });
    const manifest = (await (await page.request.get(`/api/v1/projects/${PROJECT}/apps/${NAME}`)).json()) as { spec?: { roles?: { name: string }[] } };
    expect.soft((manifest.spec?.roles ?? []).map((role) => role.name).sort(), "the App declares the roles the prompt asked for").toEqual(["editor", "viewer"]);
    await both(page, shots, { area: "walk-app", route: `/projects/${PROJECT}/apps/${NAME}`, state: "first-version", who: "author" });
  });
  test.skip(keyOut, "not run: key — the generator is the only way an app-editor makes an App");

  await lap("3 publish checklist", async (shots) => {
    await page.goto(`/projects/${PROJECT}/apps?lang=en`, { waitUntil: "load" });
    const card = page.locator("li").filter({ hasText: NAME }).first();
    await expect(card).toBeVisible({ timeout: 120_000 });
    await card.getByRole("button", { name: new RegExp(`^More actions for ${NAME}`) }).click();
    const item = page.getByRole("menuitem", { name: /^Publish/ });
    if (!(await item.isEnabled())) {
      expect.soft(await item.isEnabled(), "the checklist is reachable before the App is published").toBe(true);
      await page.keyboard.press("Escape");
      return "menu item disabled";
    }
    await item.click();
    const dialog = page.getByRole("dialog");
    await expect(dialog.getByRole("region", { name: "Before it is published" })).toBeVisible({ timeout: 60_000 });
    await both(page, shots, { area: "walk-app", route: `/projects/${PROJECT}/apps`, state: "publish-checklist", who: "author" });
    await dialog.getByRole("button", { name: "Cancel" }).click();
  });

  let approvedAt = 0;
  await lap("4 publish, approved by another person", async (shots) => {
    await page.goto(`/projects/${PROJECT}/apps/${NAME}?lang=en`, { waitUntil: "load" });
    await page.getByRole("button", { name: "Publish this app" }).click();
    const notice = page.getByRole("link", { name: /review/i }).first();
    await expect(notice).toBeVisible({ timeout: 120_000 });
    const change = new URL((await notice.getAttribute("href")) ?? "", page.url()).pathname.split("/").pop() ?? "";
    await both(page, shots, { area: "walk-app", route: `/projects/${PROJECT}/apps/${NAME}`, state: "publish-proposed", who: "author" });
    await approved(browser, change, NAME);
    approvedAt = Date.now();
  });

  await lap("5 forge build: approve to Served", async (shots) => {
    const card = page.locator("li").filter({ hasText: NAME }).first();
    await expect
      .poll(
        async () => {
          await page.goto(`/projects/${PROJECT}/apps?lang=en`, { waitUntil: "load" });
          return (await card.getByText(/^Served [0-9a-f]{7}/).count()) > 0;
        },
        { timeout: 1_800_000, intervals: [15_000] },
      )
      .toBe(true);
    const manifest = (await (await page.request.get(`/api/v1/projects/${PROJECT}/apps/${NAME}`)).json()) as { status?: { build?: unknown } };
    expect(manifest.status?.build, "status.build names the build the forge made").toBeTruthy();
    await both(page, shots, { area: "walk-app", route: `/projects/${PROJECT}/apps`, state: "build-served", who: "author" });
    return `build ${Math.round((Date.now() - approvedAt) / 1000)} s from approve to Served`;
  });

  await lap("6a only role holders open it (visibility roles), approved", async () => {
    await page.goto(`/projects/${PROJECT}/apps/${NAME}?lang=en`, { waitUntil: "load" });
    await page.getByRole("button", { name: "Share", exact: true }).first().click();
    const dialog = page.getByRole("dialog");
    await dialog.getByRole("radio", { name: /^People with a role/ }).check();
    await dialog.getByRole("button", { name: "Propose the change" }).click();
    await approved(browser, await proposedChange(page), NAME);
    // The author holds editor through the App's default group, proposed from Roles and members.
    await page.goto(`/projects/${PROJECT}/apps/${NAME}?lang=en`, { waitUntil: "load" });
    const editor = page.getByRole("listitem", { name: /^Editor$/i });
    await editor.getByLabel("E-mail").fill(AUTHOR.user);
    await editor.getByRole("button", { name: "Add person" }).click();
    await approved(browser, await proposedChange(page));
  });

  await lap("6b opened framed and in its own window; the editor writes through the App's Endpoint", async (shots) => {
    await page.goto(`/projects/${PROJECT}/apps/${NAME}/open?lang=en`, { waitUntil: "load" });
    await expect(page.frameLocator("iframe[sandbox]").locator("body")).not.toBeEmpty({ timeout: 120_000 });
    await both(page, shots, { area: "walk-app", route: `/projects/${PROJECT}/apps/${NAME}/open`, state: "opened-framed", who: "author" });
    const popup = page.waitForEvent("popup");
    await page.getByRole("link", { name: /Open in new window/ }).click();
    const own = await popup;
    await own.waitForURL((url) => url.origin === APP_ORIGIN, { timeout: 60_000 });
    await expect(own.locator("#jc-config")).toBeAttached({ timeout: 60_000 });
    await both(own, shots, { area: "walk-app", route: "/", state: "opened-own-window", who: "author" });
    const config = JSON.parse((await own.locator("#jc-config").textContent()) ?? "{}") as { slug?: string; user?: { roles?: string[] } };
    expect.soft(config.user?.roles ?? [], "the author holds editor inside the App").toContain("editor");
    expect.soft(config.slug, "the App reads through its own Endpoint app-{name}").toBeTruthy();
    expect(JSON.stringify(config).toLowerCase(), "no platform credential in the served page").not.toMatch(/bearer|access_token|eyj/);
    const written = await own.evaluate(
      async ([slug, id]) => {
        const token = document.cookie.split("; ").find((each) => each.startsWith("__Host-jc_csrf="))?.split("=")[1] ?? "";
        const base = `/api/endpoint/${slug}/ngsi-ld/v1/entities`;
        const headers = { "content-type": "application/json", "x-csrf-token": token };
        const entity = { id, type: "BikeHireDockingStation", name: { type: "Property", value: "Walkthrough T-5541" } };
        const created = await fetch(base, { method: "POST", credentials: "include", headers, body: JSON.stringify(entity) });
        const removed = await fetch(`${base}/${encodeURIComponent(id)}`, { method: "DELETE", credentials: "include", headers });
        return [created.status, removed.status];
      },
      [config.slug ?? "", `urn:ngsi-ld:BikeHireDockingStation:${NAME}`],
    );
    expect.soft(written, "the editor's write and its removal go through the App's Endpoint and Policy").toEqual([201, 204]);
    await own.close();
    return `write ${written.join(" / ")}`;
  });

  await lap("6c the other department gets the 403 page naming the roles", async (shots) => {
    const neighbour = await browser.newContext();
    try {
      const other = await neighbour.newPage();
      const refused = other.waitForResponse((answer) => answer.url() === `${APP_ORIGIN}/` && answer.request().resourceType() === "document" && answer.status() === 403, { timeout: 120_000 });
      await goSignedIn(other, NEIGHBOUR, `${APP_ORIGIN}/`, (opened) => opened.getByRole("heading", { level: 1 }));
      expect.soft((await refused.catch(() => null))?.status(), "the neighbour is refused").toBe(403);
      const roles = other.getByRole("listitem");
      expect.soft(await roles.allInnerTexts(), "the 403 page names the App's roles").toEqual(expect.arrayContaining([expect.stringMatching(/viewer/i), expect.stringMatching(/editor/i)]));
      await both(other, shots, { area: "walk-app", route: "/", state: "refused-403", who: "neighbour" });
    } finally {
      await neighbour.close();
    }
  });

  await lap(`7 shared with the department Group ${GROUP}`, async (shots) => {
    await page.goto(`/projects/${PROJECT}/apps/${NAME}?lang=en`, { waitUntil: "load" });
    const viewer = page.getByRole("listitem", { name: /^Viewer$/i });
    await viewer.getByLabel("Group").selectOption(GROUP);
    await viewer.getByRole("button", { name: "Give this role to a group" }).click();
    await approved(browser, await proposedChange(page), NAME);
    await page.goto(`/projects/${PROJECT}/apps/${NAME}?lang=en`, { waitUntil: "load" });
    await expect(page.getByRole("listitem", { name: /^Viewer$/i }).getByText(`Group ${GROUP}`)).toBeVisible({ timeout: 120_000 });
    await both(page, shots, { area: "walk-app", route: `/projects/${PROJECT}/apps/${NAME}`, state: "shared-with-group", who: "author" });
  });

  await lap("8 retired: default groups, client and address are gone", async (shots) => {
    expect.soft(await clientKnown(page), `Keycloak knows app-${NAME} before retirement`).toBe(true);
    await page.goto(`/projects/${PROJECT}/apps?lang=en`, { waitUntil: "load" });
    const card = page.locator("li").filter({ hasText: NAME }).first();
    await card.getByRole("button", { name: new RegExp(`^More actions for ${NAME}`) }).click();
    await page.getByRole("menuitem", { name: /^Retire/ }).click();
    await page.getByRole("dialog").getByRole("button", { name: "Open the merge request" }).click();
    await approved(browser, await proposedChange(page), NAME);
    for (const role of ["viewer", "editor"]) {
      await expect
        .poll(async () => (await page.request.get(`/api/v1/projects/org/groups/${NAME}-${role}`)).status(), { message: `default group ${NAME}-${role} after retirement`, timeout: 600_000, intervals: [10_000] })
        .toBe(404);
    }
    await expect.poll(() => clientKnown(page), { message: `Keycloak client app-${NAME} after retirement`, timeout: 600_000, intervals: [10_000] }).toBe(false);
    const host = await hostAnswer(browser);
    expect.soft(host, "the retired App's address answers 404").toBe("404");
    await page.goto(`/projects/${PROJECT}/apps/${NAME}?lang=en`, { waitUntil: "load" });
    await both(page, shots, { area: "walk-app", route: `/projects/${PROJECT}/apps/${NAME}`, state: "retired", who: "author" });
    return `address ${host}`;
  });
});
