/**
 * T-3217 — a person who has never seen the platform grants a group read access to a space, with
 * only what the screen gives them: the space, the kind of grantee and the grantee picked from
 * what exists, the operations ticked by name, the dates picked; the proposal is a Change a second
 * person approves, and the history shows it. Its validity ended in 2020, so it grants nothing
 * while it exists, and it is removed again, so dev is as it was.
 */
import { expect, test } from "@playwright/test";
import type { BrowserContext, Page } from "@playwright/test";
import { guideShot } from "./guide";
import { proposeFrom } from "./kindJourney";
import { APPROVER, STEWARD, approve, proposedChange, removeCompletely, signIn } from "./portal";

test.setTimeout(300_000);
test.describe.configure({ mode: "serial" });

const PROJECT = "helsinki";
const NAME = `newuser-policy-${Date.now().toString(36)}`;

let steward: { context: BrowserContext; page: Page };
let change = "";

test.afterAll(async () => {
  if (steward) {
    await removeCompletely(steward, PROJECT, "policies", NAME).catch(() => undefined);
    await steward.context.close();
  }
});

test("a new user fills the Policy form from what it offers and proposes it", async ({ browser }) => {
  steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/policies?lang=en`);
  const page = steward.page;
  await page.getByRole("button", { name: "New Policy" }).click();
  const form = page.getByRole("dialog").or(page.getByRole("main")).first();
  await form.getByLabel(/^Name/).fill(NAME);
  // The space is a list of what exists; the first one the person may read is picked.
  const space = form.getByLabel(/^Context Space/);
  const firstSpace = await space.locator("option:not([value=''])").first().getAttribute("value");
  expect(firstSpace, "the form offers a space to pick").toBeTruthy();
  await space.selectOption(firstSpace ?? "");
  // The grantee: a group, offered by name from the groups that exist (T-3217).
  await form.getByLabel(/^Kind of grantee/).selectOption({ label: "A group" });
  const grantee = form.getByLabel(/^Grantee/);
  await expect(grantee).toHaveAttribute("list", /.+/, { timeout: 30_000 });
  const offered = await grantee.evaluate((input) => {
    const list = (input as HTMLInputElement).list;
    return list ? [...list.options].map((option) => option.value) : [];
  });
  expect(offered.length, "the groups that exist are offered").toBeGreaterThan(0);
  await grantee.fill(offered[0]);
  // The operations by the names a person reads, not by an id.
  await form.getByRole("checkbox", { name: "Read", exact: true }).check();
  // The validity with the date picker: it ended long ago, so the grant never reads anything
  // while it exists on dev for the minute this journey keeps it.
  // Asked again after each click: opening one fold can reveal or remove others.
  const folded = form.locator("details:not([open]) > summary");
  for (let opened = 0; opened < 20 && (await folded.count()) > 0; opened += 1) {
    await folded.first().click();
  }
  await form.getByLabel(/^Valid to/).fill("2020-01-01T00:00");
  // T-3276: what the form grants, in words, as it is filled.
  await expect(form.getByTestId("policy-sentence")).toContainText(`Members of the group ${offered[0]} may read`);
  await guideShot(page, "policy-1-form-in-words");
  // A policy is proposed only after a fresh green check.
  await proposeFrom(form);
  change = await proposedChange(page);
  expect(change).toMatch(/\S/);
  await guideShot(page, "policy-2-proposed");
});

test("a second person approves the change", async ({ browser }) => {
  expect(change, "the step above proposed a change").toMatch(/\S/);
  const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
  try {
    await expect(approver.page.getByRole("heading", { level: 1, name: "Approvals" })).toBeVisible({ timeout: 60_000 });
    await approve(approver.page, PROJECT, change, NAME);
  } finally {
    await approver.context.close();
  }
});

test("the change's page shows it merged with the policy it made, and the policy is stored as proposed", async () => {
  expect(change, "the step above proposed a change").toMatch(/\S/);
  const page = steward.page;
  await page.goto(`/projects/${PROJECT}/approvals/${change}?lang=en`);
  await expect(page.getByText(/Merged|Applied|Live/).first()).toBeVisible({ timeout: 90_000 });
  await expect(page.getByText(NAME).first()).toBeVisible({ timeout: 60_000 });
  // And the Approvals history lists it, found by the policy's name, with who approved it (T-3292).
  await page.goto(`/projects/${PROJECT}/approvals?lang=en`);
  await page.getByRole("tab", { name: "History" }).click();
  const search = page.getByRole("search", { name: "Filter the history" });
  await search.getByLabel("Name contains").fill(NAME);
  await search.getByRole("button", { name: "Filter", exact: true }).click();
  const row = page.getByRole("table", { name: "Closed changes" }).getByRole("row").filter({ hasText: change });
  await expect(row).toBeVisible({ timeout: 60_000 });
  await expect(row.getByText("Merged")).toBeVisible();
  await expect(row.getByText(APPROVER.user)).toBeVisible();
  await guideShot(page, "approval-3-history", { reloads: true });
  // A merged change reaches the store with the reconciler's next pass, not with the merge.
  const read = () => page.request.get(`/api/v1/projects/${PROJECT}/policies/${NAME}`);
  await expect.poll(async () => (await read()).status(), { timeout: 120_000, intervals: [3_000] }).toBe(200);
  const stored = await read();
  const spec = ((await stored.json()) as { spec?: { validity?: { to?: string } } }).spec;
  expect(spec?.validity?.to ?? "", "the picked date is stored as a UTC instant").toMatch(/^2020-01-01T|^2019-12-31T/);
});
