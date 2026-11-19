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

// Administration → Repositories (T-7409) as an Organization administrator meets it: the
// repositories listed, a download of one, nothing yet, and a forge that does not answer, at 1440
// and 400 px. `vite preview` has no Portal API behind it, so the stub answers the door with the
// server's shape and the archive with bytes and the server's headers; the page is the real build.

const IDENTITY = { subject: "s1", username: "demo.admin", name: "Demo Admin", email: "admin@bbsk.sk", roles: [] };
const HEAD = "2c245c6bb1efa7a20938c5b11b70379be6838eec";
const LISTING = {
  organization: "bbsk",
  maxArchiveBytes: 67108864,
  repositories: [
    { name: "configuration", role: "configuration", project: null, app: null, defaultBranch: "main", updatedAt: "2026-11-19T08:12:00Z", sizeBytes: 412672, archived: false },
    { name: "air-quality", role: "project", project: "air-quality", app: null, defaultBranch: "main", updatedAt: "2026-11-18T09:30:00Z", sizeBytes: 20480, archived: false },
    { name: "air-quality_air-map", role: "application", project: "air-quality", app: "air-map", defaultBranch: "main", updatedAt: "2026-11-17T12:00:00Z", sizeBytes: 3145728, archived: false },
    { name: "mobility", role: "project", project: "mobility", app: null, defaultBranch: "main", updatedAt: "2026-11-02T15:45:00Z", sizeBytes: 98304, archived: false },
    { name: "city-scans", role: "project", project: "city-scans", app: null, defaultBranch: "main", updatedAt: "2026-11-10T07:20:00Z", sizeBytes: 94371840, archived: false },
    { name: "old-pilot", role: "project", project: "old-pilot", app: null, defaultBranch: "main", updatedAt: "2026-06-30T10:00:00Z", sizeBytes: 8192, archived: true },
  ],
};

type Mode = "filled" | "empty" | "error";

function json(body: unknown, status = 200) {
  return { status, contentType: "application/json", body: JSON.stringify(body) };
}

async function stubApi(page: Page, mode: Mode): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/v1/organization/repositories") {
      if (mode === "error")
        return route.fulfill(
          json({ type: "about:blank", title: "Service Unavailable", status: 503, detail: "The Git repository did not answer the Portal. Try again in a minute; if it lasts, ask the operator to check the forge and the Portal's forge token." }, 503),
        );
      return route.fulfill(json(mode === "empty" ? { ...LISTING, repositories: [] } : LISTING));
    }
    const archive = /^\/api\/v1\/organization\/repositories\/([^/]+)\/archive$/.exec(path);
    if (archive)
      return route.fulfill({
        status: 200,
        headers: { "content-type": "application/gzip", "content-disposition": `attachment; filename="${decodeURIComponent(archive[1])}-${HEAD}.tar.gz"`, "x-commit": HEAD },
        body: Buffer.from([0x1f, 0x8b, 0x08, 0x00]),
      });
    if (path.endsWith("/auth/me")) return route.fulfill(json(IDENTITY));
    if (path === "/api/v1/projects")
      return route.fulfill(json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "air-quality" }] }));
    if (path.endsWith("/permissions/me")) return route.fulfill(json({ project: "org", bootstrap: true, grants: [] }));
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

test.describe("Administration → Repositories (T-7409)", () => {
  for (const width of [1440, 400]) {
    test(`an administrator lists the repositories and downloads one at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, "filled");
      await page.goto("/organization/repositories?lang=en");
      const list = page.getByTestId("organization-repositories");
      await expect(list.getByRole("heading", { name: "Repositories" })).toBeVisible();
      await expect(list.getByText("air-quality_air-map")).toBeVisible();
      await expect(list.getByText("App air-map in air-quality").first()).toBeVisible();
      if (width === 1440) await expect(page.getByRole("link", { name: "Repositories", exact: true })).toHaveAttribute("aria-current", "page");
      await expect(list.getByText("Larger than 64 MB: clone it with git instead.").first()).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `repositories-filled-${width}`);

      const [download] = await Promise.all([
        page.waitForEvent("download"),
        list.getByRole("link", { name: "Download air-quality_air-map as .tar.gz" }).click(),
      ]);
      // The file name is the server's Content-Disposition (org_repositories_tests); vite preview
      // serves the download itself, so here the link's address is what the browser fetched.
      expect(new URL(download.url()).pathname).toBe("/api/v1/organization/repositories/air-quality_air-map/archive");
      await shot(page, `repositories-download-${width}`);
    });

    test(`an Organization with no repository says what to do at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, "empty");
      await page.goto("/organization/repositories?lang=en");
      await expect(page.getByText("No repositories yet")).toBeVisible();
      await expect(page.getByText(/Create a project under Projects first/)).toBeVisible();
      await expect(page.getByRole("link", { name: "Open Projects" })).toHaveAttribute("href", "/organization/projects");
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `repositories-empty-${width}`);
    });

    test(`a forge that does not answer is named at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, "error");
      await page.goto("/organization/repositories?lang=en");
      await expect(page.getByText(/The Git repository did not answer the Portal/)).toBeVisible();
      await shot(page, `repositories-error-${width}`);
    });
  }
});
