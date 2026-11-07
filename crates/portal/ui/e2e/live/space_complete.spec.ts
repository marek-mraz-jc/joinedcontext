import { expect, test } from "@playwright/test";
import * as fs from "fs";
import * as path from "path";
import { fileURLToPath } from "url";
import { guideShot } from "./guide";
import { APPROVER, STEWARD, approve, proposedChange, signIn } from "./portal";

const PROJECT = "helsinki";

test("complete a space from sample file and propose change", async ({ browser }) => {
  const samplePath = path.resolve(
    path.dirname(fileURLToPath(import.meta.url)),
    "../../../../joinedcontext-presentation/record/samples/bike-stations.csv",
  );

  if (!fs.existsSync(samplePath)) {
    test.skip(true, `sample file not found at ${samplePath}`);
    return;
  }

  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/spaces/complete?lang=en`);
  const page = steward.page;

  await page.locator("#complete-space").fill(`bikes-${Date.now().toString().slice(-4)}`);
  await page.getByLabel("Files", { exact: true }).setInputFiles(samplePath);
  await guideShot(page, "space-1-name-and-file");

  const completeBtn = page.locator("#complete-btn");
  await expect(completeBtn).toBeEnabled();
  await completeBtn.click();

  await expect(page.locator("[data-testid='complete-draft-DataModel']")).toBeVisible({ timeout: 45_000 });
  await expect(page.locator("[data-testid='complete-draft-ContextSpace']")).toBeVisible({ timeout: 45_000 });
  await guideShot(page, "space-2-drafts");

  const proposeBtn = page.locator("#complete-propose");
  await expect(proposeBtn).toBeVisible({ timeout: 30_000 });
  await expect(proposeBtn).toBeEnabled();
  await proposeBtn.click();

  const changeId = await proposedChange(page);
  expect(changeId).toBeTruthy();
  await guideShot(page, "space-3-proposed");

  // The bundle is one Change like any other: listed in Approvals and approved by someone else (CC-34).
  const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
  await expect(approver.page.getByRole("row").filter({ hasText: changeId })).toBeVisible({ timeout: 30_000 });
  await guideShot(approver.page, "approval-1-waiting");
  await approve(approver.page, PROJECT, changeId);
  await guideShot(approver.page, "approval-2-approved");

  await approver.context.close();
  await steward.context.close();
});
