/**
 * Changes made through the Portal's forms, end to end on the stack: a personal AI connector
 * created and revoked, a role granted to a person of the organization, and
 * a Context Space proposed by the administrator, approved by a second administrator (nobody
 * approves their own change, CC-34) and then live in the list. Every name is new to the run
 * (`unique`), so journeys on one stack never meet each other's data.
 */
import type { Page } from "@playwright/test";
import { expect, shoot, test, unique } from "./fixtures";

const REVIEW = /^(Review it in Approvals|Open the change)$/;

/** The change a form's proposal opened: its id from the notice's link (e2e/live/portal.ts proposedChange). */
async function proposedChange(page: Page): Promise<string> {
  const review = page.getByRole("link", { name: REVIEW }).first();
  await expect(review).toBeVisible({ timeout: 60_000 });
  const id = ((await review.getAttribute("href")) ?? "").split("/approvals/")[1]?.split(/[?#]/)[0];
  if (!id) throw new Error("the proposal notice links to no change");
  return id;
}

/** The form's own proposal button: Check first where the form asks for it, then Propose. */
async function propose(page: Page): Promise<void> {
  const form = page.getByTestId("form-page").or(page.getByRole("main")).first();
  const button = form.getByRole("button", { name: /^Propose/ }).first();
  if (await button.isDisabled()) await form.getByRole("button", { name: "Check", exact: true }).click();
  await expect(button).toBeEnabled({ timeout: 60_000 });
  await button.click();
}

test("J060 a person creates a personal AI connector, finds it listed, and revokes it", async ({ as }, info) => {
  const page = await as("admin", "/ai-connectors");
  const name = unique("rel-conn");
  await page.getByRole("main").getByRole("button", { name: "New AI connector" }).first().click();
  const dialog = page.getByRole("dialog");
  await dialog.locator("#ai-connector-name").fill(name);
  await dialog.getByRole("checkbox", { name: /^Data read/ }).first().check();
  await dialog.getByRole("checkbox", { name: "helsinki: the whole project" }).check();
  await shoot(page, info, "form");
  await dialog.getByRole("button", { name: "Create", exact: true }).click();
  await expect(dialog).toBeHidden({ timeout: 30_000 });
  await expect(page.getByRole("main").getByText(name).first()).toBeVisible();
  await shoot(page, info, "listed");
  await page.getByRole("main").getByRole("button", { name: new RegExp(`^Revoke.*${name}`) }).first().click();
  await page.getByRole("alertdialog").getByRole("button", { name: "Revoke now" }).click();
  await expect(page.getByRole("main").getByRole("button", { name: new RegExp(`^Revoke.*${name}`) })).toHaveCount(0);
});

test("J061 the administrator grants a person of the organization the viewer role in Helsinki, proposed for approval", async ({ as, person }, info) => {
  test.setTimeout(300_000);
  const page = await as("admin", "/projects/helsinki/settings/members");
  await page.getByRole("main").getByRole("button", { name: "Grant a role" }).click();
  const form = page.getByRole("main");
  await form.getByRole("combobox").first().selectOption({ label: "A person" });
  // The field offers only the organization's people: type, then pick from the choices.
  const who = person("outsider").user;
  await form.getByRole("combobox", { name: "Username or e-mail" }).fill(who);
  await page.getByRole("listbox", { name: "Choices" }).getByRole("option", { name: new RegExp(who) }).first().click();
  await form.getByRole("combobox", { name: /^Role/ }).selectOption({ label: "viewer" });
  await shoot(page, info, "form");
  // One open change per RoleBinding: while another journey's grant of the same role and place
  // waits ("already open ... approve or reject it first"), propose again once it is decided.
  await expect(async () => {
    await form.getByRole("button", { name: "Propose grant" }).click();
    await expect(page.getByRole("link", { name: REVIEW }).first()).toBeVisible({ timeout: 10_000 });
  }).toPass({ timeout: 120_000, intervals: [5_000] });
  const href = (await page.getByRole("link", { name: REVIEW }).first().getAttribute("href")) ?? "";
  await shoot(page, info, "proposed");

  // The approver reads the grant in Approvals and rejects it with a reason, so the binding is free
  // for the next journey and the outsider stays an outsider.
  const approver = await as("approver", href);
  await approver.getByRole("button", { name: "Reject", exact: true }).click();
  const dialog = approver.getByRole("dialog");
  await dialog.getByLabel("Why are you rejecting this?").fill("Rejected by the release journey J061: proposed only to prove the form.");
  await dialog.getByRole("button", { name: "Reject the change" }).click();
  await expect(approver.getByText(/Rejected/).first()).toBeVisible({ timeout: 60_000 });
});

test("J062 the administrator proposes a Context Space, the approver approves it, and it is listed", async ({ as }, info) => {
  test.setTimeout(300_000);
  const page = await as("admin", "/projects/helsinki/spaces/new");
  const name = unique("rel-space");
  await page.getByLabel(/^Name/).first().fill(name);
  await page.getByLabel(/^Title/).first().fill(`Release journey ${name}`);
  await shoot(page, info, "form");
  await propose(page);
  const change = await proposedChange(page);

  const approver = await as("approver", `/projects/helsinki/approvals/${change}`);
  const approve = approver.getByRole("button", { name: "Approve", exact: true });
  await expect(approve).toBeEnabled({ timeout: 60_000 });
  await shoot(approver, info, "approval");
  await approve.click();
  await expect(approver.getByText(/Deploying|Merged|Applied|Live/).first()).toBeVisible({ timeout: 90_000 });

  // Merged, then the reconciler deploys it (a pass every 10 s on the stack): the list shows it.
  await expect(async () => {
    await page.goto("/projects/helsinki/spaces");
    await expect(page.getByRole("main").getByText(`Release journey ${name}`).first()).toBeVisible({ timeout: 10_000 });
  }).toPass({ timeout: 150_000 });
  await shoot(page, info, "listed");
});

test("J063 the administrator cannot approve their own change", async ({ as }) => {
  const page = await as("admin", "/projects/helsinki/spaces/new");
  const name = unique("rel-self");
  await page.getByLabel(/^Name/).first().fill(name);
  await propose(page);
  const change = await proposedChange(page);
  await page.goto(`/projects/helsinki/approvals/${change}`);
  const approve = page.getByRole("button", { name: "Approve", exact: true });
  await expect(approve.or(page.getByText(/your own change|approve your own|another person/i)).first()).toBeVisible({ timeout: 60_000 });
  if (await approve.count()) await expect(approve).toBeDisabled();
});
