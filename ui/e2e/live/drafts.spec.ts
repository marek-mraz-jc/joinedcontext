import { expect, test } from "@playwright/test";
import { STEWARD, signIn } from "./portal";

const PROJECT = "helsinki";
const FEED = "https://gbfs.theta.fifteen.eu/gbfs/2.2/helsinki/en/free_bike_status.json";
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
const SOURCE = `hsl-citybikes-draft-${SUFFIX}`;

test("drafts shared across windows: typing in window A syncs to window B, check verdict syncs, propose drops draft", async ({
  browser,
}) => {
  // 1. Steward opens the Data sources page in window A
  const stewardA = await signIn(browser, STEWARD, `/projects/${PROJECT}/datasources?lang=en`);
  const pageA = stewardA.page;

  await pageA.getByLabel("Type").selectOption("http");
  await pageA.getByRole("button", { name: "New data source" }).click();
  const dialogA = pageA.getByTestId("form-page");
  await expect(dialogA).toBeVisible({ timeout: 10_000 });

  // 2. Type name and URL in window A
  await dialogA.getByLabel(/^Name/).fill(SOURCE);
  await dialogA.getByLabel(/^URL/).fill(FEED);

  // 3. Window B opens the page pointing at the draft
  const stewardB = await signIn(
    browser,
    STEWARD,
    `/projects/${PROJECT}/datasources?draft=${encodeURIComponent(SOURCE)}&lang=en`,
  );
  const pageB = stewardB.page;
  const dialogB = pageB.getByTestId("form-page");

  // Window B sees the URL typed in A within 10 s
  await expect(dialogB).toBeVisible({ timeout: 10_000 });
  await expect(dialogB.getByLabel(/^URL/)).toHaveValue(FEED, { timeout: 10_000 });

  // 4. Window A runs Check
  await dialogA.getByRole("button", { name: "Check" }).click();
  await expect(dialogA.getByText("Planned change")).toBeVisible({ timeout: 30_000 });

  // 5. Window B sees the green verdict chip
  const verdictB = dialogB.getByTestId("draft-verdict");
  await expect(verdictB).toBeVisible({ timeout: 15_000 });
  await expect(verdictB).toHaveText(/Checked/i, { timeout: 15_000 });

  // 6. Window A proposes the change
  await dialogA.getByRole("button", { name: "Propose change" }).click();
  await expect(pageA.getByRole("link", { name: "Review it in Approvals" })).toBeVisible({
    timeout: 30_000,
  });

  // 7. Window B's dialog shows the draft is gone (closed, status cleared, or Approvals link visible)
  await expect(async () => {
    const isClosed = !(await dialogB.isVisible());
    const draftGone = !(await dialogB.getByTestId("draft-status").isVisible().catch(() => false));
    const hasApprovals = await pageB
      .getByText(/Review it in Approvals/)
      .first()
      .isVisible()
      .catch(() => false);
    expect(isClosed || draftGone || hasApprovals).toBe(true);
  }).toPass({ timeout: 15_000 });

  await stewardA.context.close();
  await stewardB.context.close();
});
