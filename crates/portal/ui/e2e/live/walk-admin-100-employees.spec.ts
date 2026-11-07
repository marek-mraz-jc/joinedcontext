/**
 * T-5535 — a municipality administrator onboards 100 employees into five departments on dev, timed.
 *
 * The steward (`org-admin`) opens the throwaway project `walk-admin` with one space per department,
 * then walks seven lapped steps: (1) 100 people through the people API and 3 through the form,
 * (2) five department Groups of 20, the traffic department added one by one on the Group page,
 * (3) a space-scope and a project-scope RoleBinding per Group, (4) one member of each department
 * signs in and is checked against the other departments' spaces, (5) one person moves department,
 * (6) the mayor's office (20 people) is disabled and deleted, its Changes counted, (7) the
 * activity feed is read back for an entry per create, disable and delete naming the steward.
 *
 * Every walk person is `walk-NNN@walk.invalid`; afterAll removes every person, binding, Group and
 * the project, so nothing of the demo seeds is touched. The temporary passwords the people API
 * answers (dev has no realm mail, PF-92) live in memory only and never reach a log, a shot or the
 * report. `SHOTS_DIR` receives the 1440 + 400 shots, `walk-report.json` (laps, p50/p95, counts) and
 * the contact sheet `index.html`. Assertions about isolation and audit are soft: a red one names
 * the finding and the walk goes on to measure the steps after it.
 */
import { randomBytes } from "node:crypto";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { Browser, BrowserContext, Page } from "@playwright/test";
import { STEWARD, approve, csrf, portalReady, signIn } from "./portal";
import { shot, writeIndex } from "./shoot";
import { submitted } from "./totp";

const PROJECT = "walk-admin";
const DOMAIN = "walk.invalid";
const DEPARTMENTS = ["traffic", "environment", "finance", "it", "mayor-office"] as const;
type Department = (typeof DEPARTMENTS)[number];
const PER_DEPARTMENT = 20;
const API_PEOPLE = PER_DEPARTMENT * DEPARTMENTS.length;
const UI_PEOPLE = 3;
const API_VERSION = "joinedcontext.com/v1alpha1";
const SHOTS = process.env.SHOTS_DIR;

const group = (department: Department) => `walk-dep-${department}`;
const space = (department: Department) => `walk-${department}`;
const email = (n: number) => `walk-${String(n).padStart(3, "0")}@${DOMAIN}`;

test.describe.configure({ mode: "serial" });
test.setTimeout(4 * 60 * 60_000);

type Session = { context: BrowserContext; page: Page };
interface Created {
  id: string;
  email: string;
  /** In memory only: the temporary password the realm set, never written anywhere. */
  password?: string;
}
interface Lap {
  step: string;
  ms: number;
  clicks: number;
  changes: number;
  approvals: number;
  shots: string[];
  verdict: string;
}

const laps: Lap[] = [];
const report: Record<string, unknown> = {
  task: "T-5535",
  project: PROJECT,
  laps,
};
const people = new Map<string, Created>();
let steward: Session | null = null;

function percentile(values: number[], p: number): number {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted.length ? sorted[Math.min(sorted.length - 1, Math.ceil((p / 100) * sorted.length) - 1)] : 0;
}

/** One lap: the step's time, its clicks, Changes and approvals, the shots it took, its verdict. */
async function lap(name: string, body: (tally: Lap) => Promise<void>): Promise<void> {
  const tally: Lap = {
    step: name,
    ms: 0,
    clicks: 0,
    changes: 0,
    approvals: 0,
    shots: [],
    verdict: "green",
  };
  const started = Date.now();
  try {
    await test.step(name, () => body(tally));
  } catch (err) {
    tally.verdict = `red: ${String(err).split("\n")[0].slice(0, 300)}`;
    throw err;
  } finally {
    tally.ms = Date.now() - started;
    laps.push(tally);
    saveReport();
  }
}

function saveReport(): void {
  if (!SHOTS) return;
  mkdirSync(SHOTS, { recursive: true });
  writeFileSync(join(SHOTS, "walk-report.json"), `${JSON.stringify(report, null, 2)}\n`);
}

/** The page at 1440 and at 400, as the task asks for every page visited. */
async function both(page: Page, tally: Lap, route: string, state: string, who: "steward" | "viewer"): Promise<void> {
  const size = page.viewportSize() ?? { width: 1440, height: 900 };
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: 900 });
    const file = await shot(page, {
      area: "walk",
      route,
      state,
      who,
      lang: "en",
    });
    if (file) tally.shots.push(file.split("/").pop() ?? file);
  }
  await page.setViewportSize(size);
}

async function headers(session: Session): Promise<Record<string, string>> {
  return {
    "x-csrf-token": await csrf(session.context),
    "content-type": "application/json",
  };
}

/** Approves one change, typing back the name when the approval page asks for it (CC-19). */
async function approveAny(page: Page, change: { name: string; namespace: string }): Promise<void> {
  await page.goto(`/projects/${change.namespace}/approvals/${change.name}?lang=en`, { waitUntil: "load" });
  const input = page.locator("#confirm-resource-name");
  const button = page.getByRole("button", { name: "Approve", exact: true });
  await expect(button.and(page.locator(":enabled")).or(input).first()).toBeVisible({ timeout: 60_000 });
  const typed = (await input.count()) ? ((await input.getAttribute("placeholder")) ?? "") : undefined;
  await approve(page, change.namespace, change.name, typed);
}

function changeOf(body: unknown): { name: string; namespace: string } {
  const metadata = (body as { metadata?: { name?: string; namespace?: string } }).metadata;
  if (!metadata?.name || !metadata.namespace) throw new Error(`the answer names no change: ${JSON.stringify(body).slice(0, 300)}`);
  return { name: metadata.name, namespace: metadata.namespace };
}

/** Checks a manifest on its door, proposes it and approves the Change; returns the Change. */
async function propose(
  session: Session,
  project: string,
  plural: string,
  manifest: {
    kind: string;
    metadata: { name: string; namespace: string };
  } & Record<string, unknown>,
  create: boolean,
): Promise<{ name: string; namespace: string }> {
  const path = create ? `/api/v1/projects/${project}/${plural}` : `/api/v1/projects/${project}/${plural}/${manifest.metadata.name}`;
  const send = async (dryRun: boolean) => {
    const url = dryRun ? `${path}?dryRun=All` : path;
    const options = { headers: await headers(session), data: manifest };
    return create ? session.page.request.post(url, options) : session.page.request.put(url, options);
  };
  const check = await send(true);
  expect(check.ok(), `check of ${manifest.kind} ${manifest.metadata.name}: ${await check.text()}`).toBe(true);
  const answer = await send(false);
  expect(answer.status(), `${manifest.kind} ${manifest.metadata.name}: ${await answer.text()}`).toBe(202);
  return changeOf(await answer.json());
}

async function status(page: Page, path: string): Promise<number> {
  return (await page.request.get(path)).status();
}

async function waitStatus(page: Page, path: string, want: number, timeout = 300_000): Promise<void> {
  await expect
    .poll(() => status(page, path), {
      message: `${path} answers ${want}`,
      timeout,
      intervals: [5_000],
    })
    .toBe(want);
}

async function groupMembers(page: Page, name: string): Promise<string[]> {
  const answer = await page.request.get(`/api/v1/projects/org/groups/${name}`);
  if (!answer.ok()) return [];
  const spec = ((await answer.json()) as { spec?: { members?: { user: string }[] } }).spec;
  return (spec?.members ?? []).map((member) => member.user.toLowerCase());
}

/** Every walk binding of the organization: the ones that name a walk Group. */
async function walkBindings(page: Page): Promise<string[]> {
  const answer = await page.request.get("/api/v1/projects/org/rolebindings");
  if (!answer.ok()) return [];
  const items = ((await answer.json()) as { items?: { metadata: { name: string } }[] }).items ?? [];
  return items.map((item) => item.metadata.name).filter((name) => name.startsWith("walk-dep-"));
}

/** Every walk person the realm lists, paged. */
async function walkPeople(page: Page): Promise<{ id: string; email: string }[]> {
  const found: { id: string; email: string }[] = [];
  for (let first = 0; ;) {
    const answer = await page.request.get(`/api/v1/organization/people?search=${DOMAIN}&first=${first}&max=100`);
    expect(answer.ok(), `people search: ${answer.status()}`).toBe(true);
    const body = (await answer.json()) as {
      items: { id: string; email: string }[];
      next?: number;
    };
    found.push(...body.items.filter((item) => item.email.endsWith(`@${DOMAIN}`)));
    if (body.next === undefined) return found;
    first = body.next;
  }
}

/**
 * Signs a walk person in through Keycloak's form: their temporary password, then the new password
 * the realm's UPDATE_PASSWORD step asks for (random, in memory only). Returns null when the realm
 * refuses the sign-in, which is what an offboarded person must meet.
 */
async function memberSignIn(browser: Browser, who: Created, path: string): Promise<Session | null> {
  const context = await browser.newContext({
    viewport: { width: 1440, height: 900 },
  });
  const page = await context.newPage();
  await page.goto(path, { waitUntil: "load" });
  for (let step = 0; step < 6; step += 1) {
    if (await portalReady(page).count()) return { context, page };
    if (await page.locator("#password-new").count()) {
      const next = `${randomBytes(18).toString("base64url")}aA1!`;
      await page.fill("#password-new", next);
      await page.fill("#password-confirm", next);
      who.password = next;
      await submitted(page, () => page.locator("[type=submit]").first().click());
      continue;
    }
    if (await page.locator("#username").count()) {
      if (step > 0 && (await page.locator("#input-error, .kc-feedback-text, [class*=alert-error]").count())) break;
      await page.fill("#username", who.email);
      await page.fill("#password", who.password ?? "");
      await submitted(page, () => page.click("#kc-login"));
      continue;
    }
    const signInButton = page.getByRole("button", { name: "Sign in" });
    if (await signInButton.count()) {
      await submitted(page, () => signInButton.first().click());
      continue;
    }
    await page.waitForTimeout(2_000);
  }
  if (
    await portalReady(page)
      .isVisible()
      .catch(() => false)
  )
    return { context, page };
  await context.close();
  return null;
}

test.beforeAll(async ({ browser }) => {
  steward = await signIn(browser, STEWARD, "/organization/people?lang=en");
  const { page } = steward;
  expect(await walkPeople(page), "no walk person is left from an earlier run").toEqual([]);
  expect(await walkBindings(page), "no walk binding is left from an earlier run").toEqual([]);
  if ((await status(page, `/api/v1/projects/${PROJECT}`)) === 404) {
    const answer = await page.request.post("/api/v1/projects", {
      headers: await headers(steward),
      data: {
        name: PROJECT,
        displayName: "Walk: 100 employees",
        description: "Throwaway project of the T-5535 walk",
      },
    });
    expect(answer.status(), await answer.text()).toBe(202);
    await approveAny(page, changeOf(await answer.json()));
    await waitStatus(page, `/api/v1/projects/${PROJECT}`, 200);
  }
  for (const department of DEPARTMENTS) {
    if ((await status(page, `/api/v1/projects/${PROJECT}/spaces/${space(department)}`)) === 200) continue;
    const change = await propose(
      steward,
      PROJECT,
      "spaces",
      {
        apiVersion: API_VERSION,
        kind: "ContextSpace",
        metadata: { name: space(department), namespace: PROJECT },
        spec: {},
      },
      true,
    );
    await approveAny(page, change);
  }
  for (const department of DEPARTMENTS) {
    await waitStatus(page, `/api/v1/projects/${PROJECT}/spaces/${space(department)}`, 200);
  }
});

test.afterAll(async () => {
  if (!steward) return;
  const { page } = steward;
  const left: string[] = [];
  const attempt = async (what: string, run: () => Promise<void>) => {
    try {
      await run();
    } catch (err) {
      left.push(`${what}: ${String(err).split("\n")[0]}`);
    }
  };
  const remove = async (path: string, name: string) => {
    const answer = await page.request.delete(path, {
      headers: await headers(steward as Session),
      data: { confirm: name },
    });
    if (answer.status() === 404 || answer.status() === 204) return;
    if (answer.status() !== 202) throw new Error(`${answer.status()} ${await answer.text()}`);
    await approveAny(page, changeOf(await answer.json()));
    await waitStatus(page, path, 404);
  };
  for (const name of await walkBindings(page)) {
    await attempt(`binding ${name}`, () => remove(`/api/v1/projects/org/rolebindings/${name}`, name));
  }
  for (const department of DEPARTMENTS) {
    await attempt(`group ${group(department)}`, () => remove(`/api/v1/projects/org/groups/${group(department)}`, group(department)));
  }
  // Nothing names a walk person once their Groups are gone, so each deletion is immediate (204).
  for (const person of await walkPeople(page)) {
    await attempt(`person ${person.email}`, async () => {
      const answer = await page.request.delete(`/api/v1/organization/people/${person.id}`, { headers: await headers(steward as Session) });
      if (answer.status() === 202) await approveAny(page, changeOf(await answer.json()));
      else if (![204, 404].includes(answer.status())) throw new Error(`${answer.status()} ${await answer.text()}`);
    });
  }
  await attempt(`project ${PROJECT}`, () => remove(`/api/v1/projects/${PROJECT}`, PROJECT));
  report.cleanup = {
    left,
    peopleLeft: (await walkPeople(page)).length,
    bindingsLeft: (await walkBindings(page)).length,
  };
  saveReport();
  if (SHOTS) writeIndex(SHOTS);
  await steward.context.close();
  expect(left, "afterAll left walk resources on dev").toEqual([]);
});

test("a municipality admin onboards 100 employees into five departments and offboards one", async ({ browser }) => {
  const owner = steward as Session;
  const { page } = owner;
  const members: Record<Department, Created[]> = {
    traffic: [],
    environment: [],
    finance: [],
    it: [],
    "mayor-office": [],
  };

  await lap("1 create 100 people by API and 3 by the form", async (tally) => {
    const times: number[] = [];
    let limited = 0;
    for (let n = 1; n <= API_PEOPLE; n += 1) {
      const started = Date.now();
      for (;;) {
        const answer = await page.request.post("/api/v1/organization/people", {
          headers: await headers(owner),
          data: {
            email: email(n),
            firstName: "Walk",
            lastName: `Employee ${n}`,
            project: PROJECT,
          },
        });
        if (answer.status() === 429) {
          limited += 1;
          await page.waitForTimeout(1_000 * Number(answer.headers()["retry-after"] ?? "5"));
          continue;
        }
        expect(answer.status(), `create ${email(n)}: ${answer.status()}`).toBe(201);
        const body = (await answer.json()) as {
          person: { id: string };
          emailSent: boolean;
          temporaryPassword?: string;
        };
        const created: Created = {
          id: body.person.id,
          email: email(n),
          password: body.temporaryPassword,
        };
        people.set(created.email, created);
        members[DEPARTMENTS[Math.floor((n - 1) / PER_DEPARTMENT)]].push(created);
        break;
      }
      times.push(Date.now() - started);
    }
    report.create = {
      people: API_PEOPLE,
      p50Ms: percentile(times, 50),
      p95Ms: percentile(times, 95),
      maxMs: Math.max(...times),
      throttled429: limited,
      temporaryPasswords: [...people.values()].filter((one) => one.password).length,
    };

    await page.goto("/organization/people?lang=en", { waitUntil: "load" });
    await both(page, tally, "/organization/people", "100-created", "steward");
    for (let n = API_PEOPLE + 1; n <= API_PEOPLE + UI_PEOPLE; n += 1) {
      await page.getByRole("button", { name: "New person" }).click();
      const dialog = page.getByRole("region", { name: "New person" });
      await dialog.getByRole("textbox", { name: /E-mail/ }).fill(email(n));
      await dialog.getByRole("textbox", { name: /First name/ }).fill("Walk");
      await dialog.getByRole("textbox", { name: /Last name/ }).fill(`Employee ${n}`);
      await dialog.getByRole("combobox", { name: /Project/ }).selectOption(PROJECT);
      if (n === API_PEOPLE + 1) await both(page, tally, "/organization/people", "form-filled", "steward");
      await dialog.getByRole("button", { name: "Create" }).click();
      tally.clicks += 3; // New person, the project, Create; typing is not a click
      const password = page.getByRole("dialog", {
        name: /temporary password/i,
      });
      if (await password.isVisible({ timeout: 10_000 }).catch(() => false)) {
        // The password dialog is masked in the shot only if it is an input: close it unshot.
        await password.getByRole("button").first().click();
        tally.clicks += 1;
      }
    }
    await expect.poll(async () => (await walkPeople(page)).length, { timeout: 120_000 }).toBe(API_PEOPLE + UI_PEOPLE);
    for (const one of await walkPeople(page)) {
      if (!people.has(one.email)) people.set(one.email, { id: one.id, email: one.email });
    }
  });

  await lap("2 five department Groups of 20", async (tally) => {
    // Traffic: as a person does it, one member at a time on the Group page. The Group is created
    // with its first member (a Group page needs a Group), the other 19 are added there.
    const [first, ...rest] = members.traffic;
    tally.changes += 1;
    await approveAny(
      page,
      await propose(
        owner,
        "org",
        "groups",
        {
          apiVersion: API_VERSION,
          kind: "Group",
          metadata: { name: group("traffic"), namespace: "org" },
          spec: {
            description: "Walk department: traffic",
            members: [{ user: first.email }],
          },
        },
        true,
      ),
    );
    tally.approvals += 1;
    await expect.poll(() => groupMembers(page, group("traffic")), { timeout: 300_000 }).toContain(first.email);

    // The other 19 in one Change (T-5605): pasted as a list into the person field, proposed once.
    await page.goto(`/organization/groups/${group("traffic")}?lang=en`, {
      waitUntil: "load",
    });
    const field = page.getByLabel("E-mail of the person");
    await field.click();
    await field.fill(rest.map((who) => who.email).join(", "));
    await page.getByRole("option", { name: `Add the e-mails of the list (${rest.length})` }).click();
    tally.clicks += 2;
    await page.getByRole("button", { name: `Add members (${rest.length})` }).click();
    tally.clicks += 1;
    const link = page.getByRole("link", {
      name: /^(Review it in Approvals|Open the change)$/,
    });
    await expect(link).toBeVisible({ timeout: 60_000 });
    const id = ((await link.getAttribute("href")) ?? "").split("/approvals/")[1]?.split(/[?#]/)[0] ?? "";
    tally.changes += 1;
    // A second edit while this one waits would be computed from the stored list and drop the 19:
    // the page holds it and names the open change instead.
    await page.goto(`/organization/groups/${group("traffic")}?lang=en`, {
      waitUntil: "load",
    });
    await expect(page.getByRole("link", { name: `Open change ${id}` })).toBeVisible({ timeout: 60_000 });
    await expect(page.getByRole("button", { name: "Add member" })).toHaveAttribute("aria-disabled", "true");
    await both(page, tally, "/organization/groups/walk-dep-traffic", "member-proposed", "steward");
    await approveAny(page, { name: id, namespace: "org" });
    tally.approvals += 1;
    await expect
      .poll(() => groupMembers(page, group("traffic")), {
        timeout: 300_000,
        message: "the 19 added in one Change all land",
      })
      .toEqual(expect.arrayContaining(rest.map((who) => who.email)));
    await page.goto(`/organization/groups/${group("traffic")}?lang=en`, {
      waitUntil: "load",
    });
    await both(page, tally, "/organization/groups/walk-dep-traffic", "20-members", "steward");

    // The other four: one manifest each, the only bulk door there is.
    for (const department of DEPARTMENTS.slice(1)) {
      tally.changes += 1;
      await approveAny(
        page,
        await propose(
          owner,
          "org",
          "groups",
          {
            apiVersion: API_VERSION,
            kind: "Group",
            metadata: { name: group(department), namespace: "org" },
            spec: {
              description: `Walk department: ${department}`,
              members: members[department].map((one) => ({ user: one.email })),
            },
          },
          true,
        ),
      );
      tally.approvals += 1;
    }
    for (const department of DEPARTMENTS) {
      await expect.poll(async () => (await groupMembers(page, group(department))).length, { timeout: 300_000 }).toBe(PER_DEPARTMENT);
    }
    await page.goto("/organization/groups?lang=en", { waitUntil: "load" });
    await both(page, tally, "/organization/groups", "5-departments", "steward");
  });

  await lap("3 a space-scope and a project-scope RoleBinding per Group", async (tally) => {
    for (const department of DEPARTMENTS) {
      for (const [suffix, role, scope] of [
        ["space", "editor", { contextSpace: space(department) }],
        ["project", "viewer", { project: PROJECT }],
      ] as const) {
        const name = `${group(department)}-${suffix}`;
        tally.changes += 1;
        await approveAny(
          page,
          await propose(
            owner,
            "org",
            "rolebindings",
            {
              apiVersion: API_VERSION,
              kind: "RoleBinding",
              metadata: { name, namespace: "org" },
              spec: { subjects: [{ group: group(department) }], role, scope },
            },
            true,
          ),
        );
        tally.approvals += 1;
      }
    }
    await expect
      .poll(async () => (await walkBindings(page)).length, {
        timeout: 300_000,
      })
      .toBe(DEPARTMENTS.length * 2);
    await page.goto(`/organization/groups/${group("finance")}?lang=en`, {
      waitUntil: "load",
    });
    await both(page, tally, "/organization/groups/walk-dep-finance", "bindings", "steward");
    await page.goto(`/projects/${PROJECT}/settings/members?lang=en`, {
      waitUntil: "load",
    });
    await both(page, tally, `/projects/${PROJECT}/settings/members`, "bindings", "steward");
  });

  const sampled = new Map<Department, Created>();
  await lap("4 one member of each department signs in", async (tally) => {
    const seen: Record<string, unknown> = {};
    for (const department of DEPARTMENTS) {
      const who = members[department][1];
      if (!who.password) {
        tally.verdict = "unreached: the realm sent mail, so no temporary password was answered to sign in with";
        return;
      }
      const member = await memberSignIn(browser, who, `/projects/${PROJECT}/spaces?lang=en`);
      expect(member, `${who.email} signs in`).not.toBeNull();
      if (!member) continue;
      sampled.set(department, who);
      try {
        const grants = await member.page.request.get(`/api/v1/projects/${PROJECT}/permissions/me`);
        const spaces = await member.page.request.get(`/api/v1/projects/${PROJECT}/spaces`);
        const listed = spaces.ok()
          ? (
              (
                (await spaces.json()) as {
                  items?: { metadata: { name: string } }[];
                }
              ).items ?? []
            ).map((one) => one.metadata.name)
          : [];
        const others: Record<string, number> = {};
        for (const other of DEPARTMENTS.filter((one) => one !== department)) {
          others[space(other)] = await status(member.page, `/api/v1/projects/${PROJECT}/spaces/${space(other)}`);
        }
        seen[department] = {
          spacesListed: listed,
          otherSpaceStatus: others,
          grants: grants.ok() ? await grants.json() : grants.status(),
        };
        expect.soft(listed, `${department} sees its own space`).toContain(space(department));
        for (const [other, code] of Object.entries(others)) {
          expect.soft([403, 404], `${department} reading ${other}`).toContain(code);
          expect.soft(listed, `${department} listing ${other}`).not.toContain(other);
        }
        await member.page.reload({ waitUntil: "load" });
        await both(member.page, tally, `/projects/${PROJECT}/spaces`, `member-${department}`, "viewer");
        await member.page.goto(`/projects/${PROJECT}/endpoints?lang=en`, {
          waitUntil: "load",
        });
        await both(member.page, tally, `/projects/${PROJECT}/endpoints`, `member-${department}`, "viewer");
        const disabled = member.page.locator("[aria-disabled=true][aria-describedby]");
        const reasons: string[] = [];
        for (const control of await disabled.all()) {
          const reason = await control.getAttribute("aria-describedby");
          reasons.push(
            `${(await control.innerText()).trim()}: ${
              reason
                ? (
                    await member.page
                      .locator(`[id="${reason}"]`)
                      .first()
                      .innerText()
                      .catch(() => "")
                  ).trim()
                : ""
            }`,
          );
        }
        (seen[department] as Record<string, unknown>).disabledControls = reasons;
      } finally {
        await member.context.close();
      }
    }
    report.members = seen;
  });

  await lap("5 move one person from traffic to environment", async (tally) => {
    const who = members.traffic[2];
    await page.goto(`/organization/groups/${group("traffic")}?lang=en`, {
      waitUntil: "load",
    });
    await page.getByRole("button", { name: `Remove ${who.email} from the group` }).click();
    tally.clicks += 1;
    const link = page.getByRole("link", {
      name: /^(Review it in Approvals|Open the change)$/,
    });
    await expect(link).toBeVisible({ timeout: 60_000 });
    const out = ((await link.getAttribute("href")) ?? "").split("/approvals/")[1]?.split(/[?#]/)[0] ?? "";
    tally.changes += 1;
    await page.goto(`/organization/groups/${group("environment")}?lang=en`, {
      waitUntil: "load",
    });
    await page.getByLabel("E-mail of the person").fill(who.email);
    await page.getByRole("button", { name: "Add member" }).click();
    tally.clicks += 1;
    await expect(link).toBeVisible({ timeout: 60_000 });
    const into = ((await link.getAttribute("href")) ?? "").split("/approvals/")[1]?.split(/[?#]/)[0] ?? "";
    tally.changes += 1;
    for (const name of [out, into]) {
      await approveAny(page, { name, namespace: "org" });
      tally.approvals += 1;
    }
    await expect
      .poll(() => groupMembers(page, group("environment")), {
        timeout: 300_000,
      })
      .toContain(who.email);
    await expect.poll(() => groupMembers(page, group("traffic")), { timeout: 300_000 }).not.toContain(who.email);
    await page.goto(`/organization/people/${who.id}?lang=en`, {
      waitUntil: "load",
    });
    await both(page, tally, "/organization/people/person", "moved", "steward");
  });

  await lap("6 offboard the mayor's office: disable, then delete 20 people in one Change", async (tally) => {
    const leaving = members["mayor-office"];
    const disableTimes: number[] = [];
    for (const who of leaving) {
      const started = Date.now();
      const answer = await page.request.post(`/api/v1/organization/people/${who.id}/disable`, { headers: await headers(owner) });
      expect(answer.status(), `disable ${who.email}`).toBe(200);
      disableTimes.push(Date.now() - started);
    }
    await page.goto(`/organization/people?search=${DOMAIN}&lang=en`, {
      waitUntil: "load",
    });
    await both(page, tally, "/organization/people", "20-disabled", "steward");
    // T-5606: the department leaves in ONE removal Change, not one per person.
    const changes: { name: string; namespace: string }[] = [];
    let immediate = 0;
    const removal = await page.request.post("/api/v1/organization/people/remove", {
      headers: await headers(owner),
      data: { ids: leaving.map((who) => who.id) },
    });
    if (removal.status() === 204) {
      immediate = leaving.length;
    } else {
      expect(removal.status(), `remove the department: ${await removal.text()}`).toBe(202);
      changes.push(changeOf(await removal.json()));
    }
    expect(changes.length, "one Change for the whole department").toBeLessThanOrEqual(1);
    tally.changes += changes.length;
    await page.goto("/projects/org/approvals?lang=en", { waitUntil: "load" });
    await both(page, tally, "/projects/org/approvals", "1-removal-open", "steward");
    const refused: string[] = [];
    for (const change of changes) {
      try {
        await approveAny(page, change);
        tally.approvals += 1;
      } catch (err) {
        refused.push(`${change.name}: ${String(err).split("\n")[0].slice(0, 200)}`);
      }
    }
    report.offboarding = {
      people: leaving.length,
      disableP50Ms: percentile(disableTimes, 50),
      disableP95Ms: percentile(disableTimes, 95),
      changes: changes.length,
      approvals: tally.approvals,
      deletedAtOnce: immediate,
      approvalsRefused: refused,
    };
    expect.soft(refused, "every removal Change of the department is approvable").toEqual([]);
    await expect
      .poll(async () => (await groupMembers(page, group("mayor-office"))).length, {
        timeout: 600_000,
        message: "the department's Group is empty after its removals merged",
      })
      .toBe(0);
    await expect
      .poll(async () => (await walkPeople(page)).filter((one) => leaving.some((who) => who.id === one.id)).length, { timeout: 600_000 })
      .toBe(0);
    const gone = sampled.get("mayor-office");
    if (gone) {
      const again = await memberSignIn(browser, gone, `/projects/${PROJECT}/spaces?lang=en`);
      expect.soft(again, "an offboarded person cannot sign in again").toBeNull();
      await again?.context.close();
    }
  });

  await lap("7 the activity feed attributes every action to the admin", async (tally) => {
    const events: {
      kind: string;
      details?: { action?: string; person?: string; actor?: string };
    }[] = [];
    for (let cursor: string | undefined, round = 0; round < 50; round += 1) {
      const query = new URLSearchParams({
        kind: "person.changed",
        limit: "500",
        ...(cursor ? { cursor } : {}),
      });
      const answer = await page.request.get(`/api/v1/projects/org/activity?${query}`);
      expect(answer.ok(), `activity: ${answer.status()}`).toBe(true);
      const body = (await answer.json()) as {
        items: typeof events;
        next?: string;
      };
      events.push(...body.items);
      if (!body.next) break;
      cursor = body.next;
    }
    const ours = new Set([...people.values()].map((one) => one.id));
    const count = (action: string) =>
      events.filter(
        (event) => event.details?.action === action && ours.has(event.details.person ?? "") && event.details.actor === STEWARD.user,
      ).length;
    const audit = {
      created: count("created"),
      disabled: count("disabled"),
      deleted: count("deleted"),
      eventsRead: events.length,
    };
    report.audit = audit;
    expect.soft(audit.created, "one created entry per person, naming the admin").toBe(API_PEOPLE + UI_PEOPLE);
    expect.soft(audit.disabled, "one disabled entry per offboarded person").toBe(PER_DEPARTMENT);
    expect.soft(audit.deleted, "one deleted entry per offboarded person").toBe(PER_DEPARTMENT);
    await page.goto(`/projects/${PROJECT}/activity?lang=en`, {
      waitUntil: "load",
    });
    await both(page, tally, `/projects/${PROJECT}/activity`, "after-walk", "steward");
    await page.goto("/organization/agents/activity?lang=en", {
      waitUntil: "load",
    });
    await both(page, tally, "/organization/agents/activity", "after-walk", "steward");
  });
});
