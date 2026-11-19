import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { asOData, doorAsRest } from "../tests/odata";
import { doorsAsRest } from "./manifestReads";

test.beforeEach(({ page }) => doorsAsRest(page, ["portal_permissions_me", "portal_org_permissions_me", "portal_project_list"]));

// Administration → Open data appearance (T-7413): the open data portal's look for the
// Organization or one of its portals, the home and dataset page at 1440 and 400 drawn from the
// form, contrast while typing, publish as a Change, and the refused (403) and below-AA states.
// `vite preview` has no Portal API behind it, so the API is answered in the browser.
// JC_SHOTS=<dir> saves a desktop 1440 and a phone 400 shot of each state.

const IDENTITY = { subject: "b7c1e0f4", username: "ada", name: "Ada Admin", email: "ada@bbsk.sk", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

const LOOK = {
  instanceName: "Banskobystrický kraj",
  colours: { primary: "#0b5394", accent: "#e69138", background: "#fbfaf7", text: "#1f2937" },
  font: "serif",
  radius: 12,
  hero: { title: "Otvorené údaje Banskobystrického kraja", intro: "Údaje, ktoré kraj zverejňuje na **voľné použitie**." },
  datasetList: "table",
  navLinks: [{ label: "Kraj", url: "https://www.bbsk.sk/" }, { label: "O portáli", url: "/about" }],
  footerLinks: [{ label: "Kontakt", url: "https://www.bbsk.sk/kontakt" }],
};

const organization = (appearance?: Record<string, unknown>) => ({
  apiVersion: "joinedcontext.com/v1",
  kind: "Organization",
  metadata: { name: "bbsk" },
  spec: { domain: "bbsk.sk", installation: true, ...(appearance ? { appearance } : {}) },
});

const MOBILITY = {
  apiVersion: "joinedcontext.com/v1",
  kind: "Portal",
  metadata: { name: "bbsk-doprava" },
  spec: {
    organizationRef: "bbsk",
    host: "doprava.bbsk.sk",
    appearance: { colours: { primary: "#7f1d1d", background: "#ffffff" }, font: "humanist", radius: 0, datasetList: "cards" },
  },
};

const CHANGE = (name: string, phase: string) => ({
  apiVersion: "joinedcontext.com/v1",
  kind: "ChangeProposal",
  metadata: { name, namespace: "org" },
  author: { name: "Jana Kováčová", email: "jana@bbsk.sk" },
  createdAt: "2026-11-19T09:12:00Z",
  decision: phase === "Merged" ? { at: "2026-11-19T10:02:00Z", by: "eva@bbsk.sk" } : null,
  spec: {},
  status: { phase, lane: "yellow", plan: {} },
  summary: { key: "update", params: { kind: "Organization", name: "bbsk" } },
});

interface Stub {
  look?: boolean;
  portals?: boolean;
  write?: { status: number; detail: string };
}

async function stubApi(page: Page, stub: Stub = {}): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const door = await doorAsRest(
      new Request(request.url(), { method: request.method(), body: request.postData() ?? undefined }),
    );
    const url = new URL(door?.url ?? request.url());
    const path = url.pathname;
    const method = door?.method ?? request.method();
    const json = async (body: unknown, status = 200) => {
      const rest = new Response(JSON.stringify(body), { status });
      const answer = door ? await asOData(rest) : rest;
      return route.fulfill({
        status: answer.status,
        contentType: answer.status >= 400 && !door ? "application/problem+json" : "application/json",
        body: await answer.text(),
      });
    };
    if (decodeURIComponent(path) === "/api/v1/Organization") {
      const manifest = organization(stub.look ? LOOK : undefined);
      return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ "@odata.etag": 'W/"bbsk-1"', name: "bbsk", ...manifest }) });
    }
    if (path === "/api/v1/portals" && method === "GET") {
      const value = (stub.portals ? [MOBILITY] : []).map((portal) => ({ name: portal.metadata.name, status: null, ...portal }));
      return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ value }) });
    }
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: "doprava" }] });
    if (path.endsWith("/permissions/me")) {
      return json({
        project: "org",
        bootstrap: false,
        grants: [
          {
            role: "r",
            binding: "b",
            rule: { kinds: ["RoleBinding", "Organization", "Portal"], verbs: ["read", "propose", "approve", "delete"] },
          },
        ],
      });
    }
    if (path === "/api/v1/projects/org/organizations") return json(list([organization(stub.look ? LOOK : undefined)]));
    if (path === "/api/v1/projects/org/portals" && method === "GET") return json(list(stub.portals ? [MOBILITY] : []));
    if (path === "/api/v1/organization/opendata/pages/preview") {
      return json({ html: "<p>Údaje, ktoré kraj zverejňuje na <strong>voľné použitie</strong>.</p>" });
    }
    if (method === "PUT" && /^\/api\/v1\/projects\/org\/(organizations|portals)\//.test(path)) {
      if (url.search.includes("dryRun")) return json({ valid: true, verdict: { ok: true } });
      if (stub.write) return json({ type: "about:blank", title: "Refused", ...stub.write }, stub.write.status);
      return json(CHANGE("chg-7413a0b1", "PendingApproval"), 202);
    }
    if (path === "/api/v1/projects/org/changes") return json({ kind: "ChangeList", items: [] });
    if (path === "/api/v1/projects/org/changes/history") {
      return json({ kind: "ChangeList", items: stub.look ? [CHANGE("chg-00c0ffee", "Merged")] : [] });
    }
    if (path === "/api/v1/projects/org/changes/chg-00c0ffee") {
      return json({ ...CHANGE("chg-00c0ffee", "Merged"), planFields: [{ path: "spec.appearance.colours", to: {} }] });
    }
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_SHOTS;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  for (const [width, device] of [[1440, "desktop"], [400, "phone"]] as const) {
    await page.setViewportSize({ width, height: 900 });
    await page.evaluate(() => window.scrollTo(0, 0));
    await page.screenshot({ path: join(dir, `opendata-appearance-${name}-${device}.png`), fullPage: true, animations: "disabled" });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

test.describe("Administration → Open data appearance", () => {
  test("no look of its own: the built-in look and what to do next", async ({ page }) => {
    await stubApi(page);
    await page.goto("/organization/opendata-appearance?lang=en");
    await expect(page.getByTestId("preview-1440")).toBeVisible();
    await expect(page.getByText(/Add a portal under Portals/)).toBeVisible();
    await expect(page.getByText("No approved or rejected appearance change yet.")).toBeVisible();
    await shot(page, "empty");
  });

  test("the Organization's look and one portal's own look, each drawn at 1440 and 400", async ({ page }) => {
    await stubApi(page, { look: true, portals: true });
    await page.goto("/organization/opendata-appearance?lang=en");
    await expect(page.getByTestId("preview-1440")).toContainText("Otvorené údaje Banskobystrického kraja");
    await expect(page.getByTestId("contrast-text")).toContainText("readable");
    await shot(page, "filled");
    await page.getByRole("tab", { name: "Dataset page" }).click();
    await page.getByRole("tab", { name: "400 px wide" }).click();
    await expect(page.getByTestId("preview-400")).toContainText("Download");
    await shot(page, "dataset");
    await page.getByRole("tab", { name: "Home page" }).click();
    await page.getByRole("tab", { name: "1440 px wide" }).click();
    await page.getByLabel("Portal to style").selectOption("bbsk-doprava");
    await expect(page.getByRole("textbox", { name: /^Primary/ })).toHaveValue("#7f1d1d");
    await shot(page, "portal");
    // Nothing widens the page on a phone.
    await page.setViewportSize({ width: 400, height: 900 });
    const wide = await page.evaluate(() =>
      [...document.querySelectorAll("main *")]
        .filter((element) => element.getBoundingClientRect().right > document.documentElement.clientWidth + 1)
        .slice(0, 5)
        .map((element) => `${element.tagName}.${element.className}`.slice(0, 120)),
    );
    expect(wide).toEqual([]);
  });

  test("publishing proposes a change that waits for approval", async ({ page }) => {
    await stubApi(page, { look: true });
    await page.goto("/organization/opendata-appearance?lang=en");
    await page.getByRole("textbox", { name: "Heading" }).fill("Dáta kraja");
    await expect(page.getByTestId("preview-1440")).toContainText("Dáta kraja");
    await page.getByRole("button", { name: "Publish appearance" }).click();
    await expect(page.getByText(/shows the new look within seconds/)).toBeVisible();
    await expect(page.getByText("chg-7413a0b1").first()).toBeVisible();
    await shot(page, "published");
  });

  test("a text colour below AA and a javascript: link are refused before anything is sent", async ({ page }) => {
    await stubApi(page, { look: true });
    await page.goto("/organization/opendata-appearance?lang=en");
    await page.getByRole("textbox", { name: /^Text/ }).fill("#b0b0b0");
    await expect(page.getByTestId("contrast-text")).toContainText("below 4.5:1");
    const footer = page.getByRole("group", { name: "Footer links" });
    await footer.getByRole("textbox", { name: "Address" }).fill("javascript:alert(1)");
    await expect(page.getByRole("button", { name: "Publish appearance" })).toBeDisabled();
    await shot(page, "error");
  });

  test("the server's refusal says who may", async ({ page }) => {
    await stubApi(page, { write: { status: 403, detail: "propose on Organization bbsk is not granted to ada" } });
    await page.goto("/organization/opendata-appearance?lang=en");
    await page.getByRole("button", { name: "Publish appearance" }).click();
    await expect(page.getByText(/An administrator of the Organization can/)).toBeVisible();
    await shot(page, "refused");
  });
});
