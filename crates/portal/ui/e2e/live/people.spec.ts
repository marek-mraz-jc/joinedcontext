/**
 * T-2688 (ADR-N-031, AP-113): a person's whole life in the organization, walked on dev as the
 * people it is for would walk it.
 *
 * 1. The steward, an organization administrator, creates `t2688-{run}@hel.fi`. dev's realm sends
 *    no e-mail, so the page hands over a temporary password once; the journey keeps it in memory
 *    for the new person's first sign-in and nowhere else.
 * 2. The steward adds them to `helsinki-alerts-steward`, the default group of the App's steward
 *    role, from the App's Roles and members. The approver approves the change when it waits for
 *    one (PF-58 approves an administrator's own change in the same call).
 * 3. The new person signs in to helsinki-alerts, replaces the temporary password, and sees what
 *    only a steward sees: the count of alerts stewards added, and `steward` among their roles.
 * 4. The steward disables them. Their next request is sent to the login, which refuses them.
 * 5. The steward deletes them; the change that takes them out of the group is approved, and the
 *    group no longer names them.
 *
 * Nothing is left behind: `afterAll` deletes the person and approves that change when a red step
 * stopped the walk before step 5. Trace is off and no screenshot is taken while a password is on
 * a page, since both would keep it (T-2746 Security).
 */
import { randomBytes } from "node:crypto";
import { expect, test } from "@playwright/test";
import type { BrowserContext, Page } from "@playwright/test";
import { APPROVER, STEWARD, approve, csrf, proposedChange, signIn } from "./portal";

type Session = { context: BrowserContext; page: Page };

const PROJECT = "helsinki";
const APP = "helsinki-alerts";
const ROLE = "Steward";
const GROUP = `${APP}-steward`;
const APPS_URL = process.env.APPS_URL ?? "https://dev.joinedcontext.com";
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
const PERSON = { email: `t2688-${SUFFIX}@hel.fi`, first: "People", last: `Walk ${SUFFIX}` };
/** The realm's policy: 12 characters, an upper and a lower case letter and a digit. */
const NEW_PASSWORD = `${randomBytes(18).toString("base64url")}Aa1`;
/** An access token lives 300 s on dev; the edge only asks the realm again once it has expired. */
const LOCKOUT_SECONDS = 420;

let steward: Session;
let approver: Session;
let personId = "";
let temporary = "";
let deleted = false;

test.describe.configure({ mode: "serial" });
test.use({ trace: "off" });

async function phaseOf(page: Page, change: string): Promise<string> {
  const answer = await page.request.get(`/api/v1/projects/org/changes/${change}`);
  return answer.ok() ? (((await answer.json()) as { status?: { phase?: string } }).status?.phase ?? "") : "";
}

/** Approves an organization change as the approver, unless PF-58 already approved it. */
async function approvedByTheApprover(change: string): Promise<void> {
  if ((await phaseOf(steward.page, change)) === "PendingApproval") {
    const typed = approver.page.locator("#confirm-resource-name");
    await approver.page.goto(`/projects/org/approvals/${change}?lang=en`, { waitUntil: "load" });
    const asked = await typed
      .waitFor({ state: "visible", timeout: 15_000 })
      .then(() => true)
      .catch(() => false);
    await approve(approver.page, "org", change, asked ? ((await typed.getAttribute("placeholder")) ?? "") : undefined);
  }
  await expect
    .poll(() => phaseOf(steward.page, change), { timeout: 300_000, intervals: [5_000], message: `${change} is merged` })
    .toMatch(/^(Merged|Applied|Deploying)$/);
}

interface Detail {
  person: { enabled: boolean };
  groups: { name: string }[];
  appRoles: { app: string; role: string }[];
}

async function detail(page: Page): Promise<Detail | null> {
  const answer = await page.request.get(`/api/v1/organization/people/${personId}`);
  return answer.ok() ? ((await answer.json()) as Detail) : null;
}

/**
 * Signs the new person in to the App: the realm's form with the temporary password the first
 * time, then its Update password form, and the new password every time after.
 */
async function personOpensTheApp(page: Page): Promise<void> {
  await page.goto(`${APPS_URL}/apps/${APP}/`, { waitUntil: "load" });
  await expect(page.locator("#username")).toBeVisible({ timeout: 60_000 });
  await page.fill("#username", PERSON.email);
  await page.fill("#password", temporary || NEW_PASSWORD);
  await Promise.all([page.waitForURL(() => true, { waitUntil: "load" }), page.click("#kc-login")]);
  if (await page.locator("#password-new").count()) {
    await page.fill("#password-new", NEW_PASSWORD);
    await page.fill("#password-confirm", NEW_PASSWORD);
    await Promise.all([page.waitForURL(() => true, { waitUntil: "load" }), page.press("#password-confirm", "Enter")]);
    temporary = "";
  }
}

test.beforeAll(async ({ browser }) => {
  test.setTimeout(300_000);
  steward = await signIn(browser, STEWARD, "/organization/people?lang=en");
  approver = await signIn(browser, APPROVER, "/projects/org/approvals?lang=en");
});

test.afterAll(async () => {
  test.setTimeout(900_000);
  try {
    if (personId && !deleted && steward) {
      const gone = await steward.page.request.delete(`/api/v1/organization/people/${personId}`, {
        headers: { "x-csrf-token": await csrf(steward.context) },
      });
      expect([202, 204, 404], await gone.text()).toContain(gone.status());
      if (gone.status() === 202) {
        await approvedByTheApprover(((await gone.json()) as { metadata: { name: string } }).metadata.name);
      }
    }
  } finally {
    await steward?.context.close();
    await approver?.context.close();
  }
});

test.afterEach(async () => {
  const info = test.info();
  if (info.status !== info.expectedStatus && steward) {
    if (await steward.page.getByRole("dialog", { name: "Temporary password" }).count()) {
      info.annotations.push({ type: "screenshot", description: "withheld: a temporary password was on the page" });
      return;
    }
    await info.attach("screenshot", { body: await steward.page.screenshot({ fullPage: true }), contentType: "image/png" });
  }
});

test("1. the administrator creates a person, and the page hands over a temporary password once", async () => {
  const page = steward.page;
  await page.goto("/organization/people?lang=en", { waitUntil: "load" });
  await page.getByRole("button", { name: "New person" }).click();
  const form = page.getByTestId("form-page").or(page.getByRole("dialog")).first();
  await form.getByLabel(/^E-mail/).fill(PERSON.email);
  await form.getByLabel(/^First name/).fill(PERSON.first);
  await form.getByLabel(/^Last name/).fill(PERSON.last);
  const created = page.waitForResponse(
    (response) => response.url().endsWith("/api/v1/organization/people") && response.request().method() === "POST",
  );
  await form.getByRole("button", { name: "Create person" }).click();
  const answer = await created;
  expect(answer.status(), "the person is created").toBe(201);
  personId = ((await answer.json()) as { person: { id: string } }).person.id;

  const handOver = page.getByRole("dialog", { name: "Temporary password" });
  await expect(handOver, "dev's realm sends no e-mail, so the page hands the password over").toBeVisible({ timeout: 60_000 });
  temporary = await handOver.getByLabel("Temporary password").inputValue();
  expect(temporary.length, "a temporary password was handed over").toBeGreaterThanOrEqual(12);
  await handOver.getByRole("button", { name: "Done" }).click();
  await expect(handOver).toHaveCount(0);
});

test("2. the administrator adds the person to the App's steward group, and the change is approved", async () => {
  test.setTimeout(600_000);
  const page = steward.page;
  await page.goto(`/projects/${PROJECT}/apps/${APP}?lang=en`, { waitUntil: "load" });
  const role = page.getByRole("region", { name: "Roles and members" }).getByRole("listitem", { name: ROLE });
  await expect(role.getByText(`Default group ${GROUP}`)).toBeVisible({ timeout: 60_000 });
  await role.getByLabel("E-mail").fill(PERSON.email);
  await role.getByRole("button", { name: "Add person" }).click();
  await approvedByTheApprover(await proposedChange(page));

  await expect
    .poll(
      async () => {
        const held = await detail(page);
        return Boolean(
          held?.groups.some((group) => group.name === GROUP) &&
            held.appRoles.some((granted) => granted.app === APP && granted.role === "steward"),
        );
      },
      { timeout: 300_000, intervals: [5_000], message: `the person is in ${GROUP} and holds the App's steward role` },
    )
    .toBe(true);
  await page.goto(`/organization/people/${personId}?lang=en`, { waitUntil: "load" });
  await expect(page.getByRole("region", { name: "Groups" }).getByRole("link", { name: GROUP })).toBeVisible({ timeout: 60_000 });
});

test("3. the person signs in, replaces the temporary password and sees the steward's view of the App", async ({ browser }) => {
  test.setTimeout(600_000);
  // The realm's group reaches the person's token at the next sign-in; the reconciler writes it a
  // little after the mirror shows it, so a sign-in that came too early is tried again.
  await expect
    .poll(
      async () => {
        const context = await browser.newContext();
        try {
          const page = await context.newPage();
          await personOpensTheApp(page);
          const overview = page.getByRole("region", { name: "Overview" });
          const summary = overview.getByText(/^Alerts stewards added: \d+$/);
          const roles = overview.getByText(/^Signed in as .* · .*\bsteward\b/);
          // A token without the role meets the App's refusal instead of the Overview: false, and
          // the next attempt signs in again.
          const seen = await summary
            .waitFor({ state: "visible", timeout: 60_000 })
            .then(() => true)
            .catch(() => false);
          return seen && (await roles.count()) > 0;
        } finally {
          await context.close();
        }
      },
      { timeout: 480_000, intervals: [20_000], message: "the person sees the steward's count and role in the App" },
    )
    .toBe(true);
});

test("4. the administrator disables the person, and their next request meets a login that refuses them", async ({ browser }) => {
  test.setTimeout(900_000);
  const context = await browser.newContext();
  try {
    const person = await context.newPage();
    await personOpensTheApp(person);
    await expect(person.getByRole("region", { name: "Overview" })).toBeVisible({ timeout: 60_000 });

    const page = steward.page;
    await page.goto(`/organization/people/${personId}?lang=en`, { waitUntil: "load" });
    await page.getByRole("button", { name: "Disable", exact: true }).click();
    await page.getByTestId("confirm-accept").click();
    await expect(page.getByText("Disabled. Every session of the person ended.")).toBeVisible({ timeout: 60_000 });

    // The edge keeps the session's token until it expires, then asks the realm, which ended it.
    await expect
      .poll(
        async () => {
          await person.reload({ waitUntil: "load" });
          return person.locator("#username").count();
        },
        { timeout: LOCKOUT_SECONDS * 1_000, intervals: [15_000], message: "the disabled person is sent to the login" },
      )
      .toBeGreaterThan(0);
    await person.fill("#username", PERSON.email);
    await person.fill("#password", NEW_PASSWORD);
    await Promise.all([person.waitForURL(() => true, { waitUntil: "load" }), person.click("#kc-login")]);
    await expect(person.locator("#username"), "the login refuses a disabled person").toBeVisible();
    await expect(person.locator('.kc-feedback-text, [id^="input-error"]').first()).not.toBeEmpty();
    expect(person.url(), "the person never reaches the App").not.toContain(`/apps/${APP}/`);
    expect((await detail(page))?.person.enabled).toBe(false);
  } finally {
    await context.close();
  }
});

test("5. the administrator deletes the person, and the approved change takes them out of the group", async () => {
  test.setTimeout(600_000);
  const page = steward.page;
  await page.goto(`/organization/people/${personId}?lang=en`, { waitUntil: "load" });
  await page.getByRole("button", { name: "Delete", exact: true }).click();
  await page.getByTestId("confirm-accept").click();
  await approvedByTheApprover(await proposedChange(page));
  deleted = true;

  await expect
    .poll(
      async () => {
        const answer = await page.request.get(`/api/v1/projects/org/groups/${GROUP}`);
        const members = answer.ok()
          ? (((await answer.json()) as { spec?: { members?: { user: string }[] } }).spec?.members ?? [])
          : [];
        return members.some((member) => member.user.toLowerCase() === PERSON.email);
      },
      { timeout: 300_000, intervals: [5_000], message: `${GROUP} no longer names the person` },
    )
    .toBe(false);
  await expect
    .poll(async () => (await page.request.get(`/api/v1/organization/people/${personId}`)).status(), {
      timeout: 300_000,
      intervals: [5_000],
      message: "the account goes once the change is merged",
    })
    .toBe(404);
});
