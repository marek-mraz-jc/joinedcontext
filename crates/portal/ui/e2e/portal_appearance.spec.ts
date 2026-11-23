import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";
import { manifestReadsAsRest } from "./manifestReads";
import { writePhoneReport } from "./phoneCheck.cts";
import { odataBody, odataCall } from "../tests/odata";

// The pages read their manifests through the OData service; this file stubs the REST reads (T-7073).
test.beforeEach(({ page }) => manifestReadsAsRest(page));

// T-7744: a portal's Appearance tab on stubbed routes, every state at 1440 and 400 px (inherited,
// set on the portal, reset to inherited, refused by the server), and one journey that sets header
// colours, outlined buttons and a text size and publishes them as one checked Change.

const HOSTS = { items: [{ organization: "bbsk", hosts: ["doprava.bbsk.sk"], opendata: "https://opendata.bbsk.sk" }] };
const ORGANIZATION = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Organization",
  metadata: { name: "bbsk", namespace: "org" },
  spec: { appearance: { colours: { primary: "#0b5394" }, width: "wide", font: "humanist" } },
};
const OWN = { instanceName: "Doprava BBSK", colours: { headerBackground: "#0b5394", headerText: "#ffffff", link: "#1d4ed8" }, fontSize: 17, buttonStyle: "outline", cardShadow: "none" };
const CHANGE = { apiVersion: "joinedcontext.com/v1alpha1", kind: "Change", metadata: { name: "chg-7744", namespace: "org" }, spec: {}, status: { phase: "PendingApproval" } };

async function stub(page: Page, appearance: unknown, refuse?: string): Promise<unknown[]> {
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
        spec: { organizationRef: "bbsk", path: "doprava", title: { sk: "Doprava", en: "Transport" }, languages: ["en", "sk"], ...(appearance ? { appearance } : {}) },
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

async function openAppearance(page: Page): Promise<void> {
  await page.goto("/portals/bbsk-doprava?lang=en");
  await page.getByRole("tab", { name: "Appearance" }).click();
  await expect(page.getByRole("heading", { name: "Type and layout" })).toBeVisible();
  await page.waitForFunction(() => document.getAnimations().every((animation) => animation.playState !== "running"));
}

async function noOverflow(page: Page): Promise<void> {
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  expect(overflow).toBeLessThanOrEqual(0);
}


for (const width of [1440, 400]) {
  test.describe(`at ${width} px`, () => {
    // The phone check reads a phone: touch and a mobile viewport at 400 px.
    test.use(width === 400 ? { hasTouch: true, isMobile: true } : {});
    test.beforeEach(async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
    });

    test("appearance: inherited, set on the portal, reset to inherited and refused", async ({ page }, info) => {
      await stub(page, undefined);
      await openAppearance(page);
      await expect(page.getByText("From the Organization").first()).toBeVisible();
      await shoot(page, width, info.outputPath(`appearance-default-${width}.png`));
      expect(await axeViolations(page)).toEqual([]);
      await noOverflow(page);

      await page.unrouteAll();
      await manifestReadsAsRest(page);
      await stub(page, OWN);
      await openAppearance(page);
      await expect(page.getByLabel(/^Header background/)).toHaveValue("#0b5394");
      await expect(page.getByLabel(/^Buttons/)).toHaveValue("outline");
      await shoot(page, width, info.outputPath(`appearance-overridden-${width}.png`));
      expect(await axeViolations(page)).toEqual([]);
      await noOverflow(page);

      for (const field of ["Header background", "Header text", "Link colour", "Text size (px)", "Buttons", "Card shadow"]) {
        await page.getByRole("button", { name: `Reset ${field} to inherited` }).click();
      }
      await expect(page.getByLabel(/^Header background/)).toHaveValue("");
      await expect(page.getByText("Unpublished changes: publish to propose them for approval.")).toBeVisible();
      await shoot(page, width, info.outputPath(`appearance-reset-${width}.png`));
      expect(await axeViolations(page)).toEqual([]);

      await page.unrouteAll();
      await manifestReadsAsRest(page);
      await stub(page, undefined, "spec.appearance.colours.headerText: must be a colour with a contrast of at least 4.5:1 on its background (WCAG AA)");
      await openAppearance(page);
      await page.getByLabel(/^Header background/).fill("#0b5394");
      await page.getByLabel(/^Header text/).fill("#ffffff");
      await page.getByRole("button", { name: "Publish", exact: true }).click();
      await expect(page.getByText("The appearance was refused")).toBeVisible();
      await expect(page.getByText(/contrast of at least 4.5:1 on its background/).first()).toBeVisible();
      await shoot(page, width, info.outputPath(`appearance-refusal-${width}.png`));
    });
  });
}

test("set header colours, outlined buttons and a text size and publish one checked Change", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const sent = await stub(page, undefined);
  await openAppearance(page);
  await page.getByLabel(/^Header background/).fill("#0b5394");
  await page.getByLabel(/^Header text/).fill("#ffffff");
  await page.getByLabel(/^Buttons/).selectOption("outline");
  await page.getByLabel(/^Text size/).fill("17");
  await page.getByLabel(/^Heading font/).selectOption("serif");
  await page.getByRole("button", { name: "Publish", exact: true }).click();
  await expect(page.getByText(/chg-7744/)).toBeVisible();
  expect(sent).toHaveLength(2);
  expect((sent[1] as { spec: { appearance: unknown } }).spec.appearance).toEqual({
    colours: { headerBackground: "#0b5394", headerText: "#ffffff" },
    buttonStyle: "outline",
    fontSize: 17,
    headingFont: "serif",
  });
});
