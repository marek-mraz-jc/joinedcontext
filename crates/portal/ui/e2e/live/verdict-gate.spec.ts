/**
 * Nothing is proposed without a fresh green verdict, on dev, in the form (T-1589; PF-57, UI-23).
 *
 * The server half is `tests/strict_gate_tests.rs::strict_mode_propose_without_verdict_refused_409`
 * and `::strict_mode_stale_verdict_refused_409`; the green path is waited for in three mocked specs.
 * What no browser test walked is the gate itself: unchecked refuses with the reason, a check opens
 * the door, **editing the draft closes it again**, and the second check opens it once more.
 *
 * The refused half is here too, since T-2233: a `Data model` naming a model that does not exist used
 * to be checked **green** and is now refused with the field named. It is an error beside the buttons
 * rather than a red verdict chip — `result.valid` is the literal `true`, so a `ContextSpace` check
 * records a green verdict or none (T-2234 carries the one-shape question) — and what matters to the
 * person is the same either way: the door stays shut, the reason names the field, and fixing the
 * field opens it.
 *
 * The one proposal at the end is rejected again, so dev keeps what it had.
 */
import { expect, test } from "@playwright/test";
import { APPROVER, STEWARD, proposedChange, signIn } from "./portal";

const PROJECT = "helsinki";
/** The model the seed commits, so the draft names something that is really there. */
const MODEL = "helsinki";

test.setTimeout(900_000);

test("the verdict gate: unchecked refuses, a check opens it, an edit closes it again", async ({
  browser,
}) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/spaces?lang=en`);
  const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
  const name = `t1589-${Date.now().toString(36)}`;
  let change = "";
  try {
    await steward.page.getByRole("button", { name: /^New (context )?space/i }).first().click();
    const form = steward.page.getByTestId("form-page");
    const verdict = form.getByTestId("draft-verdict");
    const propose = form.getByRole("button", { name: /^Propose/ });
    const reason = form.getByTestId("propose-reason");
    await form.getByLabel(/^Name/).fill(name);

    // Unchecked: the reason says a check is what is missing, not that something is broken.
    await expect(verdict).toContainText("Not checked yet");
    await expect(propose).toBeDisabled();
    await expect(reason).toContainText(/check first|fresh green/i);

    // Checked: the door opens, and the chip says how fresh the verdict is.
    await form.getByRole("button", { name: "Check", exact: true }).click();
    await expect(verdict).toContainText("Checked", { timeout: 120_000 });
    await expect(propose).toBeEnabled({ timeout: 60_000 });

    // Edited after the check: the verdict is stale, not silently still green, and the door shuts.
    // This is the case a person walks into by fixing a typo after checking, and the one
    // `strict_mode_stale_verdict_refused_409` refuses at the door.
    await form.getByLabel("Data model").fill(MODEL);
    await expect(verdict, "an edit makes the verdict stale").toContainText(
      "Changed since the check",
      { timeout: 60_000 },
    );
    await expect(propose).toBeDisabled();
    await expect(reason).toContainText(/changed since|run the check again/i);

    // Checked again: open once more, and this time the draft is proposed.
    await form.getByRole("button", { name: "Check", exact: true }).click();
    await expect(verdict).toContainText("Checked", { timeout: 120_000 });
    await expect(propose).toBeEnabled({ timeout: 60_000 });
    await propose.click();
    change = await proposedChange(steward.page);
    expect(change).toMatch(/^chg-/);
  } finally {
    if (change) {
      await approver.page.goto(`/projects/${PROJECT}/approvals/${change}?lang=en`, {
        waitUntil: "load",
      });
      const reject = approver.page.getByRole("button", { name: "Reject", exact: true });
      if (await reject.isEnabled().catch(() => false)) {
        await reject.click();
        const dialog = approver.page.getByRole("dialog");
        const why = dialog.getByRole("textbox").first();
        if (await why.count()) {
          await why.fill("withdrawn by the live journey of T-1589");
        }
        await dialog.getByRole("button", { name: /^(Reject|Confirm)/ }).click();
      }
    }
    await steward.context.close();
    await approver.context.close();
  }
});

/**
 * MF-13, T-2233: a check that cannot resolve what the manifest names refuses, names the **field**,
 * and leaves the door shut; naming a model that is really there opens it.
 */
test("a check refuses a model that does not exist, names the field, and a fix clears it", async ({
  browser,
}) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/spaces?lang=en`);
  const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
  const name = `t1589r-${Date.now().toString(36)}`;
  const missing = `no-such-model-${name}`;
  let change = "";
  try {
    await steward.page.getByRole("button", { name: /^New (context )?space/i }).first().click();
    const form = steward.page.getByTestId("form-page");
    const propose = form.getByRole("button", { name: /^Propose/ });
    await form.getByLabel(/^Name/).fill(name);
    await form.getByLabel("Data model").fill(missing);

    await form.getByRole("button", { name: "Check", exact: true }).click();
    const refusal = form.getByTestId("footer-error");
    await expect(refusal, "the check refuses what it cannot resolve").toBeVisible({
      timeout: 120_000,
    });
    // The field and the name, because "invalid manifest" is not something a person can act on.
    await expect(refusal).toContainText("spec.dataModelRef");
    await expect(refusal).toContainText(missing);
    await expect(propose, "a refused check leaves the door shut").toBeDisabled();

    // The fix: a model the project really holds.
    await form.getByLabel("Data model").fill(MODEL);
    await form.getByRole("button", { name: "Check", exact: true }).click();
    await expect(form.getByTestId("draft-verdict")).toContainText("Checked", { timeout: 120_000 });
    await expect(propose).toBeEnabled({ timeout: 60_000 });
    await propose.click();
    change = await proposedChange(steward.page);
    expect(change).toMatch(/^chg-/);
  } finally {
    if (change) {
      await approver.page.goto(`/projects/${PROJECT}/approvals/${change}?lang=en`, {
        waitUntil: "load",
      });
      const reject = approver.page.getByRole("button", { name: "Reject", exact: true });
      if (await reject.isEnabled().catch(() => false)) {
        await reject.click();
        const dialog = approver.page.getByRole("dialog");
        const why = dialog.getByRole("textbox").first();
        if (await why.count()) {
          await why.fill("withdrawn by the live journey of T-1589");
        }
        await dialog.getByRole("button", { name: /^(Reject|Confirm)/ }).click();
      }
    }
    await steward.context.close();
    await approver.context.close();
  }
});
