import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";
import { manifestReadsAsRest } from "./manifestReads";
import { writePhoneReport } from "./phoneCheck.cts";
import { odataBody, odataCall } from "../tests/odata";

// The pages read their manifests through the OData service; this file stubs the REST reads (T-7073).
test.beforeEach(({ page }) => manifestReadsAsRest(page));

// T-7743: a portal's Display tab on stubbed routes, every state at 1440 and 400 px (inherited,
// set on the portal, reset to inherited, refused by the server), and one journey that hides the
// map, renames a heading, reorders the about rows and publishes it as one checked Change.

const HOSTS = { items: [{ organization: "bbsk", hosts: ["doprava.bbsk.sk"], opendata: "https://opendata.bbsk.sk" }] };
const ORGANIZATION = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Organization",
  metadata: { name: "bbsk", namespace: "org" },
  spec: { appearance: { display: { dataset: { about: ["license", "publisher", "modified"] } } } },
};
const OWN = {
  dataset: {
    sections: [{ name: "publisher" }, { name: "about", heading: { sk: "O dátach", en: "About the data" } }, { name: "resources" }, { name: "visualizations" }],
  },
  datasetList: { fields: ["format", "modified"] },
};
const CHANGE = { apiVersion: "joinedcontext.com/v1alpha1", kind: "Change", metadata: { name: "chg-7743", namespace: "org" }, spec: {}, status: { phase: "PendingApproval" } };

async function stub(page: Page, display: unknown, refuse?: string): Promise<unknown[]> {
  const sent: unknown[] = [];
  await page.route(/\/api\//, async (route) => {
    const call = odataCall(route.request().url());
    if (call?.name === "portal_organization_change_history") {
      return route.fulfill({ status: 200, contentType: "application/json", body: odataBody({ kind: "ChangeList", items: [], next: null }) });
    }
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json({ subject: "s", username: "jana", name: "Jana Admin", email: "jana@bbsk.sk", roles: [] });
    if (path.endsWith("/permissions/me")) {
      return json({ project: "org", bootstrap: false, grants: [{ rule: { kinds: ["Portal", "Organization"], verbs: ["read", "propose", "approve", "delete"] }, organization: "bbsk" }] });
    }
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "doprava", organization: "bbsk" }] });
    if (path === "/api/v1/projects/org/organizations") return json({ kind: "List", items: [ORGANIZATION] });
    if (path === "/api/v1/organization/portal-hosts") return json(HOSTS);
    const base = "/api/v1/projects/org/portals/bbsk-doprava";
    if (path === base && request.method() === "GET") {
      return json({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "Portal",
        metadata: { name: "bbsk-doprava", namespace: "org" },
        spec: { organizationRef: "bbsk", path: "doprava", title: { sk: "Doprava", en: "Transport" }, languages: ["en", "sk"], ...(display ? { appearance: { display } } : {}) },
      });
    }
    if (path === base && request.method() === "PUT") {
      if (refuse) return json({ type: "about:blank", title: "Refused", status: 422, detail: refuse, errors: [refuse] }, 422);
      sent.push(JSON.parse(request.postData() ?? "{}"));
      return json(url.search.includes("dryRun") ? { valid: true, verdict: { ok: true } } : CHANGE);
    }
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
  if (width === 400) expect(await writePhoneReport(page, path)).toEqual([]);
  await page.setViewportSize({ width, height: 900 });
}

async function openDisplay(page: Page): Promise<void> {
  await page.goto("/portals/bbsk-doprava?lang=en");
  await page.getByRole("tab", { name: "Display" }).click();
  await expect(page.getByRole("heading", { name: "About table rows" })).toBeVisible();
  await page.waitForFunction(() => document.getAnimations().every((animation) => animation.playState !== "running"));
}

async function noOverflow(page: Page): Promise<void> {
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  expect(overflow).toBeLessThanOrEqual(0);
}

const section = (page: Page, title: string) => page.locator("section").filter({ has: page.getByRole("heading", { name: title }) });

for (const width of [1440, 400]) {
  test.describe(`at ${width} px`, () => {
    // The phone check reads a phone: touch and a mobile viewport at 400 px.
    test.use(width === 400 ? { hasTouch: true, isMobile: true } : {});
    test.beforeEach(async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
    });

    test("display: inherited, set on the portal, reset to inherited and refused", async ({ page }, info) => {
      await stub(page, undefined);
      await openDisplay(page);
      await expect(section(page, "Dataset page sections").getByText("Inherited from the installation default")).toBeVisible();
      await expect(section(page, "About table rows").getByText("Inherited from the Organization")).toBeVisible();
      await shoot(page, width, info.outputPath(`display-default-${width}.png`));
      expect(await axeViolations(page)).toEqual([]);
      await noOverflow(page);

      await page.unrouteAll();
      await manifestReadsAsRest(page);
      await stub(page, OWN);
      await openDisplay(page);
      await expect(section(page, "Dataset page sections").getByText("Set on this portal")).toBeVisible();
      await expect(page.locator("#display-heading-about-sk")).toHaveValue("O dátach");
      await shoot(page, width, info.outputPath(`display-overridden-${width}.png`));
      expect(await axeViolations(page)).toEqual([]);
      await noOverflow(page);

      await section(page, "Dataset page sections").getByRole("button", { name: "Reset to inherited" }).click();
      await expect(section(page, "Dataset page sections").getByText("Inherited from the installation default")).toBeVisible();
      await expect(section(page, "Dataset page sections").getByRole("checkbox", { name: "Map" })).toBeChecked();
      await expect(page.getByText("Unpublished changes: publish to propose them for approval.")).toBeVisible();
      await shoot(page, width, info.outputPath(`display-reset-${width}.png`));
      expect(await axeViolations(page)).toEqual([]);

      await page.unrouteAll();
      await manifestReadsAsRest(page);
      await stub(page, undefined, "spec.appearance.display.dataset.sections[0].heading.de: must be in a language of the Portal's spec.languages");
      await openDisplay(page);
      await section(page, "Dataset page sections").getByRole("checkbox", { name: "Map" }).uncheck();
      await page.getByRole("button", { name: "Publish", exact: true }).click();
      await expect(page.getByText("The display was refused")).toBeVisible();
      await expect(page.getByText(/must be in a language of the Portal's spec.languages/)).toBeVisible();
      await shoot(page, width, info.outputPath(`display-refusal-${width}.png`));
    });
  });
}

test("hide the map, rename a heading, reorder the about rows and publish one checked Change", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const sent = await stub(page, undefined);
  await openDisplay(page);
  const sections = section(page, "Dataset page sections");
  await sections.getByRole("checkbox", { name: "Map" }).uncheck();
  const about = sections.locator("li").filter({ has: page.getByRole("checkbox", { name: "About", exact: true }) });
  await about.getByText("Change the heading").click();
  await about.getByLabel("Heading (SK)").fill("O dátach");
  await section(page, "About table rows").getByRole("button", { name: "Move Publisher up" }).click();
  await page.getByRole("button", { name: "Publish", exact: true }).click();
  await expect(page.getByText(/chg-7743/)).toBeVisible();
  expect(sent).toHaveLength(2);
  const display = (sent[1] as { spec: { appearance: { display: { dataset: { sections: { name: string; heading?: Record<string, string> }[]; about: string[] } } } } }).spec.appearance.display;
  expect(display.dataset.sections.map((s) => s.name)).not.toContain("map");
  expect(display.dataset.sections.find((s) => s.name === "about")).toEqual({ name: "about", heading: { sk: "O dátach" } });
  expect(display.dataset.about.slice(0, 2)).toEqual(["publisher", "license"]);
});
