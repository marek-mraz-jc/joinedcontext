import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";
import { manifestReadsAsRest } from "./manifestReads";
import { odataBody, odataCall } from "../tests/odata";

// The pages read their manifests through the OData service; this file stubs the REST reads (T-7073).
test.beforeEach(({ page }) => manifestReadsAsRest(page));

// T-7434: a portal's Layout tab on stubbed routes, every state at 1440 and 400 px (the built-in
// home, a home and a page built from blocks with the preview, the incomplete fields named), and
// one journey that builds a home and a page and publishes them as one checked Change.

const HOSTS = { items: [{ organization: "bbsk", hosts: ["doprava.bbsk.sk"], opendata: "https://opendata.bbsk.sk" }] };
const sha = (c: string) => c.repeat(64);
const FILES = {
  items: [
    { id: "f-1", name: "titulka-doprava.jpg", contentType: "image/jpeg", size: 845_000, sha256: sha("a"), path: `/files/${sha("a")}/titulka-doprava.jpg`, uploadedBy: "Jana Admin", uploadedAt: "2026-11-19T18:40:00Z", usedBy: [] },
    { id: "f-2", name: "vyrocna-sprava-2025.pdf", contentType: "application/pdf", size: 3_400_000, sha256: sha("c"), path: `/files/${sha("c")}/vyrocna-sprava-2025.pdf`, uploadedBy: "Jana Admin", uploadedAt: "2026-11-19T18:40:00Z", usedBy: [] },
  ],
  limits: { files: 2, bytes: 4_245_000, maxFiles: 500, maxBytes: 200 * 1024 * 1024, maxFileBytes: 10 * 1024 * 1024 },
};
const DATASETS = {
  total: 3,
  items: [
    { name: "zastavky-mhd", title: "Zastávky MHD", source: "endpoints", hidden: false },
    { name: "ovzdusie-bb", title: "Kvalita ovzdušia", source: "endpoints", hidden: false },
    { name: "interne", title: "Interný zoznam", source: "endpoints", hidden: true },
  ],
};
const BUILT = [
  {
    slug: "home",
    rows: [
      { blocks: [{ type: "hero", title: { en: "Open data of Banská Bystrica region" }, intro: { en: "All the region's data in **one place**." }, image: `/files/${sha("a")}/titulka-doprava.jpg` }] },
      { blocks: [{ type: "numbers", title: { en: "In numbers" }, items: ["datasets", "publishers", "updated-this-month"] }, { type: "text", markdown: { en: "## About\n\nThe region publishes its data here." } }] },
      { blocks: [{ type: "dataset-gallery", title: { en: "Transport" }, tag: "doprava", limit: 6 }] },
      { blocks: [{ type: "map", title: { en: "Stops" }, dataset: "zastavky-mhd" }, { type: "chart", dataset: "ovzdusie-bb" }, { type: "file-list", title: { en: "Documents" }, files: [`/files/${sha("c")}/vyrocna-sprava-2025.pdf`] }] },
    ],
  },
  { slug: "doprava", title: { en: "Transport" }, rows: [{ blocks: [{ type: "link-cards", title: { en: "More" }, cards: [{ title: { en: "Timetables" }, text: { en: "Buses and trams" }, url: "https://www.dpmbb.sk" }] }] }] },
];
const CHANGE = { apiVersion: "joinedcontext.com/v1alpha1", kind: "Change", metadata: { name: "chg-7434", namespace: "org" }, spec: {}, status: { phase: "PendingApproval" } };

async function stub(page: Page, pages: unknown[] | undefined, refuse?: string): Promise<unknown[]> {
  const sent: unknown[] = [];
  await page.route(/\/api\//, async (route) => {
    const call = odataCall(route.request().url());
    if (call?.name === "portal_organization_change_history") {
      return route.fulfill({ status: 200, contentType: "application/json", body: odataBody({ kind: "ChangeList", items: [], next: null }) });
    }
    if (call?.name === "portal_opendata_page_preview") {
      const body = JSON.parse(route.request().postData() ?? "{}") as { markdown?: string; body?: { markdown?: string } };
      // The portal's renderer in small: a `## ` line is a heading, a blank line ends a paragraph.
      const html = (body.markdown ?? body.body?.markdown ?? "")
        .replace(/</g, "&lt;")
        .replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>")
        .split(/\n{2,}/)
        .map((part) => (part.startsWith("## ") ? `<h2>${part.slice(3)}</h2>` : `<p>${part}</p>`))
        .join("");
      return route.fulfill({ status: 200, contentType: "application/json", body: odataBody({ html }) });
    }
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
    if (path === "/api/v1/organization/opendata/pages/preview") {
      const body = JSON.parse(request.postData() ?? "{}") as { markdown: string };
      const plain = body.markdown.replace(/^#+ /gm, "").replace(/\*\*/g, "");
      return json({ html: `<p>${plain.replace(/</g, "&lt;")}</p>` });
    }
    const base = "/api/v1/projects/org/portals/bbsk-mobility";
    if (path === base && request.method() === "GET") {
      return json({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "Portal",
        metadata: { name: "bbsk-mobility", namespace: "org" },
        spec: { organizationRef: "bbsk", path: "mobility", title: { sk: "Doprava", en: "Mobility" }, languages: ["en", "sk"], ...(pages ? { pages } : {}) },
      });
    }
    if (path === base && request.method() === "PUT") {
      if (refuse) return json({ type: "about:blank", title: "Refused", status: 422, detail: refuse, errors: [refuse] }, 422);
      sent.push(JSON.parse(request.postData() ?? "{}"));
      return json(url.search.includes("dryRun") ? { valid: true, verdict: { ok: true } } : CHANGE);
    }
    if (path === `${base}/files`) return json(FILES);
    if (path === `${base}/datasets`) return json(DATASETS);
    if (path === "/api/v1/projects/org/changes/history") return json({ kind: "ChangeList", items: [] });
    if (path.endsWith("/events")) return route.fulfill({ status: 200, contentType: "text/event-stream", body: "" });
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  return sent;
}

async function shoot(page: Page, width: number, path: string): Promise<void> {
  await page.waitForFunction(() => document.getAnimations().every((animation) => animation.playState !== "running"));
  const height = await page.evaluate(() => document.documentElement.scrollHeight);
  await page.setViewportSize({ width, height: Math.max(height, 900) });
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.screenshot({ path });
  await page.setViewportSize({ width, height: 900 });
}

async function openLayout(page: Page): Promise<void> {
  await page.goto("/portals/bbsk-mobility?lang=en");
  await page.getByRole("tab", { name: "Layout" }).click();
  await page.waitForFunction(() => document.getAnimations().every((animation) => animation.playState !== "running"));
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

    test("layout: built-in home, a built home with its preview, a page, incomplete fields and a refusal", async ({ page }, info) => {
      await stub(page, undefined);
      await openLayout(page);
      await expect(page.getByText("This portal shows the built-in home")).toBeVisible();
      await shoot(page, width, info.outputPath(`layout-empty-${width}.png`));
      expect(await axeViolations(page)).toEqual([]);
      await noOverflow(page);

      await page.unrouteAll();
      await manifestReadsAsRest(page);
      await stub(page, BUILT);
      await openLayout(page);
      await expect(page.getByRole("heading", { name: "Map", level: 4 })).toBeVisible();
      await expect(page.getByText("No unpublished changes.")).toBeVisible();
      await shoot(page, width, info.outputPath(`layout-filled-${width}.png`));
      expect(await axeViolations(page)).toEqual([]);
      await noOverflow(page);

      await page.getByRole("radio", { name: /400/ }).check();
      await expect(page.locator("[data-width='400']")).toBeVisible();
      // The phone preview is as wide as a 400 px phone (28rem of content plus its padding and border).
      if (width > 1000) expect(Math.abs(((await page.locator("[data-width='400']").boundingBox())?.width ?? 0) - 400)).toBeLessThanOrEqual(4);
      await page.locator("[data-width='400']").screenshot({ path: info.outputPath(`layout-preview-400-${width}.png`) });
      await page.getByRole("button", { name: "Transport", exact: true }).click();
      await expect(page.getByRole("heading", { name: "/page/doprava" })).toBeVisible();
      await shoot(page, width, info.outputPath(`layout-page-${width}.png`));

      await page.getByLabel(/^Block type/).selectOption("chart");
      await page.getByRole("button", { name: "Add a row" }).click();
      await page.getByRole("button", { name: "Publish layout" }).click();
      await expect(page.getByText("Some blocks are not complete")).toBeVisible();
      await expect(page.getByText("Choose a dataset of this portal.")).toBeVisible();
      await shoot(page, width, info.outputPath(`layout-error-${width}.png`));
      expect(await axeViolations(page)).toEqual([]);

      await page.unrouteAll();
      await manifestReadsAsRest(page);
      await stub(page, BUILT, "spec.pages[0].rows[0].blocks[0].title.en: one line of 1 to 120 characters without < or >");
      await openLayout(page);
      await page.getByLabel(/^Heading/).first().fill("Open data <Banská Bystrica>");
      await page.getByRole("button", { name: "Publish layout" }).click();
      await expect(page.getByText("The portal refused the layout")).toBeVisible();
      await expect(page.getByText("Fix the marked field, then publish again.")).toBeVisible();
      await expect(page.getByText("one line of 1 to 120 characters without < or >")).toBeVisible();
      await shoot(page, width, info.outputPath(`layout-refusal-${width}.png`));
    });
  });
}

test("build a home and a page from blocks and publish them as one checked Change", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const sent = await stub(page, undefined);
  await openLayout(page);
  await page.getByRole("button", { name: "Start from the built-in home" }).click();
  await page.getByLabel(/^Heading/).first().fill("Open data of the region");
  await page.getByLabel("Image").selectOption({ label: "titulka-doprava.jpg" });
  await page.getByLabel(/^Block type/).selectOption("featured-datasets");
  await page.getByRole("button", { name: "Add a row" }).click();
  await expect(page.getByLabel(/^Add a dataset/).locator("option")).toHaveText(["Choose a dataset", "Zastávky MHD", "Kvalita ovzdušia"]);
  await page.getByLabel(/^Add a dataset/).selectOption("ovzdusie-bb");
  await page.getByRole("button", { name: "Add a page" }).click();
  await page.getByLabel(/^Address/).fill("doprava");
  await page.getByLabel(/^Title and menu entry/).fill("Transport");
  await page.getByRole("button", { name: "Create the page" }).click();
  await page.getByLabel(/^Text/).fill("## Buses and trams\n\nTimetables and stops.");
  await page.getByRole("button", { name: "Publish layout" }).click();
  await expect(page.getByText(/chg-7434/)).toBeVisible();
  expect(sent).toHaveLength(2);
  const pages = (sent[1] as { spec: { pages: { slug: string; rows: { blocks: { type: string }[] }[] }[] } }).spec.pages;
  expect(pages.map((p) => p.slug)).toEqual(["home", "doprava"]);
  expect(pages[0]?.rows.map((r) => r.blocks[0]?.type)).toEqual(["hero", "numbers", "dataset-gallery", "featured-datasets"]);
  expect(pages[0]?.rows[0]?.blocks[0]).toEqual({ type: "hero", title: { en: "Open data of the region" }, image: `/files/${sha("a")}/titulka-doprava.jpg` });
});
