/**
 * Two hands on one resource, on dev (T-1588; CC-19, CC-34, PF-42).
 *
 * `tests/resource_mutate_tests.rs::a_second_proposal_while_a_change_is_open_names_it_and_writes_nothing`
 * proves the rule against a stub: the second proposal is a 409 naming the open change. What no test
 * proved is that a person meets it — that the second attempt says *which* change is in the way, that
 * the page offers it rather than swallowing it, and that deciding the first one lets the second
 * through.
 *
 * **One person plays both hands, and this is why.** Dev seeds three people and only
 * `demo.steward@hel.fi` holds `propose`: `demo.approver` holds `approve` alone and `demo.viewer`
 * holds neither (`helsinki-role-*.yaml`). A genuine collision between two proposers needs a fourth
 * person (T-2231). The rule under test does not care who the second hand belongs to — the resource
 * takes one open change, whoever opens it — so the refusal is played by the same session twice, and
 * the two-people case is named in T-2231 rather than pretended here.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { APPROVER, STEWARD, approve, proposedChange, removeCompletely, signIn } from "./portal";

const PROJECT = "helsinki";

test.setTimeout(900_000);

/** The phase of a change, as the approvals route reads it. */
async function phase(page: Page, change: string): Promise<string> {
  const answer = await page.request.get(`/api/v1/projects/${PROJECT}/changes/${change}`);
  expect(answer.ok(), `read change ${change}`).toBe(true);
  return String((await answer.json()).status?.phase ?? "");
}

/**
 * Fills the new-space form and presses Propose, exactly as a person does. The form is the door
 * here and not the REST route, because a proposal needs a fresh green check of its own
 * (PF-57, `strict_gate_tests.rs`) and the form is what runs it; a REST-only client has to put a
 * draft and check it first, which is T-1596's journey and not this one.
 */
async function proposeSpaceInTheForm(page: Page, name: string): Promise<void> {
  await page.goto(`/projects/${PROJECT}/spaces?lang=en`, { waitUntil: "load" });
  await page.getByRole("button", { name: /^New (context )?space/i }).first().click();
  const form = page.getByTestId("form-page");
  await form.getByLabel(/^Name/).fill(name);
  const propose = form.getByRole("button", { name: /^Propose/ });
  if (await propose.isDisabled()) {
    await form.getByRole("button", { name: "Check", exact: true }).click();
  }
  await expect(propose).toBeEnabled({ timeout: 120_000 });
  await propose.click();
}

test("a second change on the same resource is refused by the name of the first, and lands once that one is decided", async ({
  browser,
}) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/spaces?lang=en`);
  const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
  const name = `t1588-${Date.now().toString(36)}`;
  let first = "";
  let second = "";
  try {
    await proposeSpaceInTheForm(steward.page, name);
    first = await proposedChange(steward.page);

    // The second hand, while the first change is open: the form says which change is in the way,
    // so the person can go and decide it. A dialog that closed on a silent failure would leave
    // them believing they had proposed twice.
    await proposeSpaceInTheForm(steward.page, name);
    await expect(
      steward.page.getByText(new RegExp(`already open|${first}`, "i")).first(),
      "the form names the open change instead of swallowing the refusal",
    ).toBeVisible({ timeout: 120_000 });

    // And only one change exists for this resource: the refused attempt opened nothing.
    const changes = await steward.page.request.get(`/api/v1/projects/${PROJECT}/changes`);
    expect(changes.ok()).toBe(true);
    const mine = ((await changes.json()).items ?? []).filter((change: { summary?: { params?: { name?: string } } }) =>
      change.summary?.params?.name === name,
    );
    expect(mine, `one change for ${name}, not two`).toHaveLength(1);
    await steward.page.keyboard.press("Escape");

    // Decide the first one, and the second hand goes through: the rule is one change at a time,
    // not one change ever.
    // A space is a Yellow lane: no typed confirmation, unlike a public Endpoint (CC-19).
    await approve(approver.page, PROJECT, first);
    await expect
      .poll(() => phase(steward.page, first), { timeout: 300_000, intervals: [10_000] })
      .toMatch(/Merged|Applied/);
    first = "";

    await proposeSpaceInTheForm(steward.page, name);
    second = await proposedChange(steward.page);
    expect(second).toMatch(/^chg-/);
  } finally {
    // Reject what is still open first, and wait for it: a resource takes one open change, so the
    // removal below is refused while either of these is open — the very rule this journey tests.
    for (const change of [second, first]) {
      if (!change || (await phase(steward.page, change)) !== "PendingApproval") {
        continue;
      }
      await approver.page.goto(`/projects/${PROJECT}/approvals/${change}?lang=en`, {
        waitUntil: "load",
      });
      await approver.page.getByRole("button", { name: "Reject", exact: true }).click();
      const dialog = approver.page.getByRole("dialog");
      const why = dialog.getByRole("textbox").first();
      if (await why.count()) {
        await why.fill("withdrawn by the live journey of T-1588");
      }
      await dialog.getByRole("button", { name: /^(Reject|Confirm)/ }).click();
      await expect.poll(() => phase(steward.page, change), { timeout: 60_000 }).toBe("Rejected");
    }
    // The space the approved change created goes with it, its removal approved as well. Not
    // swallowed: a cleanup that fails quietly is how residue reaches dev (T-2236).
    await removeCompletely(steward, PROJECT, "spaces", name);
    await steward.context.close();
    await approver.context.close();
  }
});
