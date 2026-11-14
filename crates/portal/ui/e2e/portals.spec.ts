import { expect, test } from "@playwright/test";
import type { Page, Route } from "@playwright/test";
import { axeViolations } from "./axe";

// T-6875 (ODP-15): the Portals area on stubbed routes. Every state of the list and the editor at
// 1440 and 400 px, and one create, edit and delete journey whose requests are each a checked
// proposal (a dry run, then the write) on the Portal kind's own routes.

const HOSTS = { items: [{ organization: "bbsk", hosts: ["doprava.bbsk.sk", "skoly.bbsk.sk"], opendata: "https://opendata.bbsk.sk" }] };

function portal(path: string, title: Record<string, string>, extra: Record<string, unknown> = {}) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Portal",
    metadata: { name: `bbsk-${path}`, namespace: "org" },
    spec: { organizationRef: "bbsk", path, title, languages: Object.keys(title), ...extra },
  };
}

const FILLED = [
  portal("default", { sk: "Otvorené údaje BBSK", en: "BBSK open data" }, { sources: [{ name: "endpoints", type: "endpoints" }] }),
  portal("mobility", { sk: "Doprava", en: "Mobility" }, { host: "doprava.bbsk.sk", slug: "redirect", sources: [{ name: "kosice", type: "dcat-ap" }, { name: "nkod", type: "nkod-sparql" }] }),
  portal("skoly", { sk: "Školy", en: "Schools" }),
];

const CHANGE = {
  id: "c-1",
  title: "Propose Portal bbsk-mobility",
  lane: "Yellow",
  author: "jana",
  createdAt: "2026-11-14T10:00:00Z",
  status: { phase: "AwaitingApproval" },
  resources: [{ kind: "Portal", name: "bbsk-mobility", namespace: "org", operation: "create" }],
};

interface Stub {
  portals: unknown[];
  /** What a write of a portal answers; the default is a Change. */
  write?: (route: Route) => Promise<void>;
  /** The hosts read; a 403 for somebody who makes no portal. */
  hosts?: number;
}

async function stub(page: Page, options: Stub): Promise<string[]> {
  const sent: string[] = [];
  await page.route(/\/api\//, async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json({ subject: "s", username: "jana", name: "Jana Admin", email: "jana@bbsk.sk", roles: [] });
    if (path.endsWith("/permissions/me")) {
      return json({ project: "org", bootstrap: false, grants: [{ rule: { kinds: ["Portal", "Organization", "RoleBinding", "Group"], verbs: ["read", "propose", "approve", "delete"] }, organization: "bbsk" }] });
    }
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "doprava" }] });
    if (path === "/api/v1/organization/portal-hosts") {
      return options.hosts ? json({ type: "about:blank", title: "Forbidden", status: options.hosts, detail: "only an Organization's administrator" }, options.hosts) : json(HOSTS);
    }
    if (path.startsWith("/api/v1/projects/org/portals")) {
      if (request.method() !== "GET") {
        sent.push(`${request.method()} ${path}${url.search}`);
        if (options.write) return options.write(route);
        return url.searchParams.get("dryRun") ? json({ valid: true, verdict: { ok: true, findings: [] } }) : json(CHANGE, 202);
      }
      const name = path.split("/")[6];
      if (!name) return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: options.portals });
      const found = (options.portals as { metadata: { name: string } }[]).find((p) => p.metadata.name === name);
      return found ? json(found) : json({ type: "about:blank", title: "Not Found", status: 404, detail: `Portal '${name}' not found` }, 404);
    }
    if (path.endsWith("/events")) return route.fulfill({ status: 200, contentType: "text/event-stream", body: "" });
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  return sent;
}

async function shoot(page: Page, name: string, width: number, path: string): Promise<void> {
  // Full height in the viewport, so the sticky sidebar is not cut (knowledge/portal-ui.md).
  const height = await page.evaluate(() => document.documentElement.scrollHeight);
  await page.setViewportSize({ width, height: Math.max(height, 900) });
  await page.screenshot({ path });
}

for (const width of [1440, 400]) {
  test.describe(`at ${width} px`, () => {
    test.beforeEach(async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
    });

    test("the list: empty, filled, and refused", async ({ page }, info) => {
      await stub(page, { portals: [] });
      await page.goto("/portals?lang=en");
      await expect(page.getByRole("heading", { level: 1, name: "Open data portals" })).toBeVisible();
      await expect(page.getByText("No portal yet")).toBeVisible();
      await shoot(page, "empty", width, info.outputPath(`portals-list-empty-${width}.png`));
      expect(await axeViolations(page)).toEqual([]);

      await page.unrouteAll();
      await stub(page, { portals: FILLED });
      await page.reload();
      await expect(page.getByRole("link", { name: "Mobility" })).toBeVisible();
      await expect(page.getByText("https://doprava.bbsk.sk/")).toBeVisible();
      await shoot(page, "filled", width, info.outputPath(`portals-list-filled-${width}.png`));
      expect(await axeViolations(page)).toEqual([]);
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
      expect(overflow).toBeLessThanOrEqual(0);

      await page.unrouteAll();
      await stub(page, { portals: [], hosts: 403 });
      await page.reload();
      await expect(page.getByText(/made by an Organization's administrators/)).toBeVisible();
      await shoot(page, "refusal", width, info.outputPath(`portals-list-refusal-${width}.png`));
    });

    test("the editor: filled, the server's refusal on its field, and another Organization's portal", async ({ page }, info) => {
      await stub(page, {
        portals: FILLED,
        write: (route) =>
          route.fulfill({
            status: 422,
            contentType: "application/problem+json",
            body: JSON.stringify({ type: "about:blank", title: "Unprocessable", status: 422, detail: "portals.max (6) reached", errors: ["spec.host: skoly.bbsk.sk is already the host of portal bbsk-skoly2"] }),
          }),
      });
      await page.goto("/portals/bbsk-mobility?lang=en");
      await expect(page.getByLabel(/^Domain/)).toHaveValue("doprava.bbsk.sk");
      await shoot(page, "edit", width, info.outputPath(`portals-editor-filled-${width}.png`));
      expect(await axeViolations(page)).toEqual([]);

      await page.getByLabel(/^Domain/).selectOption("skoly.bbsk.sk");
      await page.getByRole("button", { name: "Propose the changes" }).click();
      await expect(page.getByText("skoly.bbsk.sk is already the host of portal bbsk-skoly2")).toBeVisible();
      await shoot(page, "error", width, info.outputPath(`portals-editor-error-${width}.png`));

      await page.goto("/portals/praha-skoly?lang=en");
      await expect(page.getByText("No portal praha-skoly in your Organizations")).toBeVisible();
      await shoot(page, "refusal", width, info.outputPath(`portals-editor-refusal-${width}.png`));
    });
  });
}

test("create, edit and delete a portal: each write a checked proposal", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const sent = await stub(page, { portals: FILLED });
  await page.goto("/portals?lang=en");
  await page.getByRole("link", { name: "New portal" }).first().click();
  await page.getByLabel(/Name in the address/).fill("mobility2");
  await page.getByLabel(/Title in Slovak/).fill("Doprava 2");
  await page.getByRole("radio", { name: "A domain of its own" }).check();
  await page.getByLabel(/^Domain/).selectOption("skoly.bbsk.sk");
  await page.getByLabel("Also keep the usual slug address").check();
  await expect(page.getByText("The portal will answer at https://skoly.bbsk.sk/")).toBeVisible();
  const created = page.waitForRequest((r) => r.method() === "POST" && !r.url().includes("dryRun"));
  await page.getByRole("button", { name: "Propose the portal" }).click();
  const body = (await created).postDataJSON() as { metadata: { name: string }; spec: Record<string, unknown> };
  expect(body.metadata.name).toBe("bbsk-mobility2");
  expect(body.spec).toMatchObject({ organizationRef: "bbsk", path: "mobility2", host: "skoly.bbsk.sk", slug: "keep", title: { sk: "Doprava 2" } });
  await expect(page.getByText(/waiting|approv/i).first()).toBeVisible();

  await page.goto("/portals/bbsk-skoly?lang=en");
  await page.getByLabel(/Title in English/).fill("Schools and kindergartens");
  await page.getByRole("button", { name: "Propose the changes" }).click();
  await expect.poll(() => sent.filter((s) => s.startsWith("PUT /api/v1/projects/org/portals/bbsk-skoly")).length).toBe(2);

  await page.goto("/portals/bbsk-skoly?lang=en");
  await page.getByRole("button", { name: /Remove portal/ }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("textbox").fill("bbsk-skoly");
  await dialog.getByRole("button", { name: "Propose removal" }).click();
  await expect.poll(() => sent.some((s) => s.startsWith("DELETE /api/v1/projects/org/portals/bbsk-skoly"))).toBe(true);
  expect(sent.filter((s) => s.startsWith("POST")).map((s) => s.includes("dryRun"))).toEqual([true, false]);
});
