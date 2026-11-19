import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";
import { manifestReadsAsRest } from "./manifestReads";

// The pages read their manifests through the OData service; this file stubs the REST reads (T-7073).
test.beforeEach(({ page }) => manifestReadsAsRest(page));

// T-7417, T-7428: a portal's Files tab on stubbed routes, every state at 1440 and 400 px (empty,
// uploading, list, over the limit, delete refused while used), and one journey that uploads by
// drop, renames, and deletes an unused file after its confirm.

const HOSTS = { items: [{ organization: "bbsk", hosts: ["doprava.bbsk.sk"], opendata: "https://opendata.bbsk.sk" }] };
const PORTAL = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Portal",
  metadata: { name: "bbsk-mobility", namespace: "org" },
  spec: { organizationRef: "bbsk", path: "mobility", title: { sk: "Doprava", en: "Mobility" }, languages: ["sk", "en"] },
};
const sha = (c: string) => c.repeat(64);
const file = (id: string, name: string, contentType: string, size: number, c: string, usedBy: string[] = []) => ({
  id,
  name,
  contentType,
  size,
  sha256: sha(c),
  path: `/files/${sha(c)}/${encodeURIComponent(name)}`,
  uploadedBy: "Jana Admin",
  uploadedAt: "2026-11-19T18:40:00Z",
  usedBy,
});
const FILES = [
  file("f-1", "titulka-doprava.jpg", "image/jpeg", 845_000, "a", ["bbsk-mobility"]),
  file("f-2", "logo-bbsk.svg", "image/svg+xml", 6_200, "b", ["Organization bbsk"]),
  file("f-3", "vyrocna-sprava-2025.pdf", "application/pdf", 3_400_000, "c"),
  file("f-4", "zastavky-mhd.csv", "text/csv", 120_400, "d"),
  file("f-5", "rozpocet-2026.xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", 58_000, "e"),
];
const LIMITS = { files: 5, bytes: 4_429_600, maxFiles: 500, maxBytes: 200 * 1024 * 1024, maxFileBytes: 10 * 1024 * 1024 };
const problem = (status: number, detail: string) => ({ type: "about:blank", title: "Refused", status, detail });

interface Stub {
  items: unknown[];
  limits?: typeof LIMITS;
  /** The upload's answer: a status, or a promise the test resolves to hold the upload open. */
  upload?: number | Promise<void>;
}

async function stub(page: Page, options: Stub): Promise<string[]> {
  const sent: string[] = [];
  let items = [...options.items];
  await page.route(/\/api\//, async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json({ subject: "s", username: "jana", name: "Jana Admin", email: "jana@bbsk.sk", roles: [] });
    if (path.endsWith("/permissions/me")) {
      return json({ project: "org", bootstrap: false, grants: [{ rule: { kinds: ["Portal", "Organization", "RoleBinding", "Group"], verbs: ["read", "propose", "approve", "delete"] }, organization: "bbsk" }] });
    }
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "doprava", organization: "bbsk" }] });
    if (path === "/api/v1/organization/portal-hosts") return json(HOSTS);
    const base = "/api/v1/projects/org/portals/bbsk-mobility";
    if (path === base && request.method() === "GET") return json(PORTAL);
    if (path === `${base}/files` && request.method() === "POST") {
      sent.push(`POST ${url.search}`);
      if (options.upload instanceof Promise) await options.upload;
      if (typeof options.upload === "number") {
        return json(problem(options.upload, "file: the Organization's files would hold 214000000 bytes; its limit is 209715200 bytes (200 MiB)"), options.upload);
      }
      const added = file(`f-${items.length + 1}`, url.searchParams.get("name") ?? "x", "image/png", 2048, "f");
      items = [...items, added];
      return json(added, 201);
    }
    if (path === `${base}/files`) return json({ items, limits: options.limits ?? { ...LIMITS, files: items.length } });
    const one = path.match(/\/files\/([^/]+)$/);
    if (one && request.method() === "PATCH") {
      const body = JSON.parse(request.postData() ?? "{}") as { name: string };
      sent.push(`PATCH ${one[1]} ${body.name}`);
      items = items.map((f) => ((f as { id: string }).id === one[1] ? { ...(f as object), name: body.name } : f));
      return json(items.find((f) => (f as { id: string }).id === one[1]));
    }
    if (one && request.method() === "DELETE") {
      sent.push(`DELETE ${one[1]}`);
      items = items.filter((f) => (f as { id: string }).id !== one[1]);
      return route.fulfill({ status: 204 });
    }
    if (path.endsWith("/events")) return route.fulfill({ status: 200, contentType: "text/event-stream", body: "" });
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  return sent;
}

async function shoot(page: Page, width: number, path: string): Promise<void> {
  const height = await page.evaluate(() => document.documentElement.scrollHeight);
  await page.setViewportSize({ width, height: Math.max(height, 900) });
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.screenshot({ path });
  await page.setViewportSize({ width, height: 900 });
}

async function openFiles(page: Page): Promise<void> {
  await page.goto("/portals/bbsk-mobility?lang=en");
  await page.getByRole("tab", { name: "Files" }).click();
  await page.waitForFunction(() => document.getAnimations().every((animation) => animation.playState !== "running"));
}

async function drop(page: Page, name: string): Promise<void> {
  const transfer = await page.evaluateHandle((fileName) => {
    const data = new DataTransfer();
    data.items.add(new File([new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])], fileName, { type: "image/png" }));
    return data;
  }, name);
  await page.getByTestId("file-drop-zone").dispatchEvent("drop", { dataTransfer: transfer });
}

async function noOverflow(page: Page): Promise<void> {
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  expect(overflow).toBeLessThanOrEqual(0);
}

for (const width of [1440, 400]) {
  test.describe(`at ${width} px`, () => {
    test.beforeEach(async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
    });

    test("files: empty, uploading, list, over the limit and delete refused while used", async ({ page }, info) => {
      let release = () => {};
      const held = new Promise<void>((resolve) => (release = resolve));
      await stub(page, { items: [], limits: { ...LIMITS, files: 0, bytes: 0 }, upload: held });
      await openFiles(page);
      await expect(page.getByText("No file yet")).toBeVisible();
      await shoot(page, width, info.outputPath(`files-empty-${width}.png`));
      expect(await axeViolations(page)).toEqual([]);
      await noOverflow(page);

      await drop(page, "titulka-doprava.png");
      await expect(page.getByRole("status").filter({ hasText: "Uploading 1 of 1" })).toBeVisible();
      await shoot(page, width, info.outputPath(`files-uploading-${width}.png`));
      release();

      await page.unrouteAll();
      await stub(page, { items: FILES });
      await openFiles(page);
      await expect(page.getByRole("table", { name: "Files" })).toBeVisible();
      await expect(page.getByRole("button", { name: "Delete titulka-doprava.jpg" })).toBeDisabled();
      await shoot(page, width, info.outputPath(`files-list-${width}.png`));
      expect(await axeViolations(page)).toEqual([]);
      await noOverflow(page);

      await page.unrouteAll();
      await stub(page, { items: FILES, upload: 422 });
      await openFiles(page);
      await drop(page, "letecka-snimka.png");
      await expect(page.getByRole("alert")).toContainText("its limit is 209715200 bytes (200 MiB)");
      await shoot(page, width, info.outputPath(`files-error-${width}.png`));
    });
  });
}

test("drop, rename and delete: each on the portal's own files routes", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const sent = await stub(page, { items: FILES });
  await openFiles(page);
  await drop(page, "mapa-liniek.png");
  await expect(page.getByText("1 files uploaded.")).toBeVisible();
  await expect(page.getByText("mapa-liniek.png")).toBeVisible();
  await page.getByRole("button", { name: "Rename zastavky-mhd.csv" }).click();
  await page.getByLabel(/^New name/).fill("zastavky-mhd-2026.csv");
  await page.getByRole("button", { name: "Rename", exact: true }).click();
  await expect(page.getByText("The file was renamed.")).toBeVisible();
  await page.getByRole("button", { name: "Delete vyrocna-sprava-2025.pdf" }).click();
  await expect(page.getByText("Delete vyrocna-sprava-2025.pdf?")).toBeVisible();
  await page.getByRole("button", { name: "Delete the file" }).click();
  await expect(page.getByText("vyrocna-sprava-2025.pdf was deleted.")).toBeVisible();
  expect(sent).toEqual(["POST ?name=mapa-liniek.png", "PATCH f-4 zastavky-mhd-2026.csv", "DELETE f-3"]);
});
