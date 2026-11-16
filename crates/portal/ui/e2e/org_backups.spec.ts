import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";
import { doorsAsRest, manifestReadsAsRest } from "./manifestReads";

test.beforeEach(({ page }) => {
  manifestReadsAsRest(page);
  doorsAsRest(page);
});

// Administration → Backups (T-7154) as an Organization administrator meets it: nothing set up,
// a bucket field refused, the bucket saved, a run, the versions, a restore check and a soft
// delete, at 1440 and 400 px. `vite preview` has no Portal API behind it, so a small in-browser
// bucket answers the backup doors with the server's shapes; the page is the real build.

const IDENTITY = { subject: "s1", username: "demo.admin", name: "Demo Admin", email: "admin@bbsk.sk", roles: [] };
const DOOR = "/api/v1/organization/backups/configuration";

interface Bucket {
  target: Record<string, unknown> | null;
  log: Record<string, unknown>[];
  versions: Record<string, unknown>[];
}

function json(body: unknown, status = 200) {
  return { status, contentType: "application/json", body: JSON.stringify(body) };
}

async function stubApi(page: Page, bucket: Bucket, admin = true): Promise<void> {
  let tick = 0;
  const now = () => `2026-11-16T09:${String(10 + tick++).padStart(2, "0")}:00Z`;
  const log = (action: string, ok: boolean, message = "uploaded", bytes = 0) =>
    bucket.log.unshift({ id: bucket.log.length + 1, organization: "bbsk", kind: "configuration", action, actor: "demo.admin", at: now(), ok, bytes, objectKey: "jc/bbsk/configuration/bundle.zip", versionId: "", message });
  await page.route("**/api/**", async (route) => {
    const url = new URL(route.request().url());
    const method = route.request().method();
    const path = url.pathname;
    if (path === DOOR && method === "GET") return route.fulfill(json({ target: bucket.target, log: bucket.log }));
    if (path === DOOR && method === "PUT") {
      const input = route.request().postDataJSON() as Record<string, unknown>;
      if (!/^[a-z0-9.-]{3,63}$/.test(String(input.bucket))) {
        const detail = "bucket: 3 to 63 lower-case letters, digits, '.' and '-', e.g. bb-backups";
        return route.fulfill(json({ type: "about:blank", title: "Unprocessable Content", status: 422, detail, errors: [{ path: "bucket", message: "3 to 63 lower-case letters, digits, '.' and '-', e.g. bb-backups" }] }, 422));
      }
      const { secretAccessKey: _secret, ...kept } = input;
      bucket.target = { ...kept, organization: "bbsk", kind: "configuration", updatedBy: "demo.admin", updatedAt: now(), lastRunAt: null };
      return route.fulfill(json(bucket.target));
    }
    if (path === `${DOOR}/run`) {
      for (const version of bucket.versions) version.isLatest = false;
      bucket.versions.unshift({ versionId: `v${bucket.versions.length + 1}`, lastModified: now(), size: 48213, isLatest: true, deleteMarker: false });
      log("run", true, "uploaded", 48213);
      return route.fulfill(json(bucket.log[0]));
    }
    if (path === `${DOOR}/versions`) return route.fulfill(json({ versioning: "Enabled", objectLock: false, versions: bucket.versions }));
    if (path === `${DOOR}/bundle` && method === "DELETE") {
      for (const version of bucket.versions) version.isLatest = false;
      bucket.versions.unshift({ versionId: "m1", lastModified: now(), size: 0, isLatest: true, deleteMarker: true });
      log("delete", true, "delete marker written");
      return route.fulfill({ status: 204, body: "" });
    }
    if (path.endsWith("/restore")) {
      log("restore-plan", true, "0 change(s) planned");
      return route.fulfill(
        json({
          organization: "bbsk",
          versionId: "v1",
          takenAt: "2026-11-16T09:11:00Z",
          changes: 0,
          groups: [
            { name: "bbsk", created: [], changed: [], unchanged: 14 },
            { name: "organization", created: [], changed: [], unchanged: 6 },
          ],
        }),
      );
    }
    if (path.endsWith("/auth/me")) return route.fulfill(json(IDENTITY));
    if (path === "/api/v1/projects")
      return route.fulfill(json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "bbsk" }] }));
    if (path.endsWith("/permissions/me")) return route.fulfill(json({ project: "org", bootstrap: admin, grants: [] }));
    return route.fulfill(json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] }));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  const height = await page.evaluate(() => document.documentElement.scrollHeight);
  await page.setViewportSize({ width: page.viewportSize()?.width ?? 1440, height });
  await page.screenshot({ path: join(dir, `${name}.png`) });
}

test.describe("Administration → Backups (T-7154)", () => {
  for (const width of [1440, 400]) {
    test(`an administrator sets up, runs, checks and deletes a backup at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      const bucket: Bucket = { target: null, log: [], versions: [] };
      await stubApi(page, bucket);
      await page.goto("/organization/backups?lang=en");
      await expect(page.getByText("No backup is set up yet")).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `backups-empty-${width}`);

      await page.getByLabel("S3 address").fill("https://s3.eu-central-1.amazonaws.com");
      await page.getByLabel("Region").fill("eu-central-1");
      await page.getByLabel("Bucket", { exact: true }).fill("City_Backups");
      await page.getByLabel("Folder").fill("joinedcontext/");
      await page.getByLabel("Access key id").fill("AKIAIOSFODNN7EXAMPLE");
      await page.getByLabel("Secret access key").fill("not-shown-again");
      await page.getByRole("button", { name: "Save" }).click();
      await expect(page.getByText(/3 to 63 lower-case letters/)).toBeVisible();
      await shot(page, `backups-refused-${width}`);

      await page.getByLabel("Bucket", { exact: true }).fill("city-backups");
      await page.getByRole("switch", { name: "Scheduled backups on" }).click();
      await page.getByRole("button", { name: "Save" }).click();
      await expect(page.getByText("Runs every 24 hours, and whenever you press Run now.")).toBeVisible();
      await expect(page.getByText("No version in the bucket yet")).toBeVisible();

      await page.getByRole("button", { name: "Run now" }).click();
      await expect(page.getByTestId("backup-last-run")).toContainText("47.1 KiB uploaded");
      await expect(page.getByText("Versioning on")).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `backups-filled-${width}`);

      await page.getByRole("button", { name: "Restore this version" }).click();
      await expect(page.getByText("Nothing to restore: the Organization is as it was in this version.")).toBeVisible();
      await shot(page, `backups-restore-${width}`);
      await page.getByRole("button", { name: "Close" }).first().click();

      await page.getByRole("button", { name: "Delete backup" }).click();
      await page.getByRole("alertdialog").getByRole("button", { name: "Delete backup" }).click();
      await expect(page.getByText("Deleted", { exact: true })).toBeVisible();
      await expect(page.getByText(/The backup is deleted. Every earlier version is still kept/)).toBeVisible();
      await shot(page, `backups-deleted-${width}`);
    });

    test(`a member who does not administer is refused at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, { target: null, log: [], versions: [] }, false);
      await page.goto("/organization/backups?lang=en");
      await expect(page.getByText(/This page is for organization administrators/).first()).toBeVisible();
      await expect(page.getByText("No backup is set up yet")).toHaveCount(0);
      await shot(page, `backups-refused-member-${width}`);
    });
  }
});
