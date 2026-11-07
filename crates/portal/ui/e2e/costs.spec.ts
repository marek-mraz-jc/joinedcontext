import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";

// Administration → Costs (T-5126) as an administrator meets it: a month split per project, a
// partial month, no price table yet, an organization with no project, and a member refused, in
// English and Slovak at 1440 and 400 px. `vite preview` has no Portal API behind it, so the API is
// answered in the browser; the page is the real build.

const IDENTITY = { subject: "s1", username: "demo.admin", name: "Demo Admin", email: "admin@bbsk.sk", roles: [] };
const GB = 1e9;

const project = (name: string, share: number, spaces: [string, number][], assistants: unknown[] = []) => {
  const storage = spaces.reduce((sum, [, bytes]) => sum + Math.round((bytes / GB) * 4.4) / 100, 0);
  const ai = assistants.reduce((sum: number, a) => sum + ((a as { cost: number }).cost ?? 0), 0);
  const node = Math.round(30.5 * share * 100) / 100;
  return {
    project: name,
    node,
    storage,
    ai,
    total: Math.round((node + storage + ai) * 100) / 100,
    share,
    spaces: spaces.map(([space, bytes]) => ({ space, bytes, storage: Math.round((bytes / GB) * 4.4) / 100 })),
    assistants,
  };
};

const COSTS = (month: string, broken = false) => ({
  month,
  currency: "EUR",
  measuredAt: "2026-10-22T01:00:00Z",
  prices: { currency: "EUR", perUsd: 0.86, nodes: [{ name: "cpx42 nbg1", monthly: 30 }], volumes: [], fixed: [] },
  projects: [
    project("bbsk", 0.62, [["podujatia", 48 * GB], ["doprava", 21 * GB]], [
      { assistant: "pomocnik", questions: 412, uncostedQuestions: 3, costUsd: 6.12, cost: 5.26 },
      broken
        ? { assistant: "verejny", questions: 0, uncostedQuestions: 0, costUsd: null, cost: 0, reason: "the assistant answered 503 Service Unavailable" }
        : { assistant: "verejny", questions: 96, uncostedQuestions: 0, costUsd: 1.4, cost: 1.2 },
    ]),
    project("helsinki", 0.38, [["air", 42 * GB]]),
  ],
  platform: { node: 0, storage: 8.32, total: 8.32 },
  infrastructure: 43.7,
  ai: broken ? 5.26 : 6.46,
  total: broken ? 48.96 : 50.16,
});

type Answer = (path: string, month: string) => { status: number; body: unknown } | undefined;

async function stubApi(page: Page, answer: Answer, admin = true): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    const answered = answer(url.pathname, url.searchParams.get("month") ?? "");
    if (answered) return json(answered.body, answered.status);
    if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
    if (url.pathname === "/api/v1/projects")
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "bbsk" }] });
    if (url.pathname.endsWith("/permissions/me")) return json({ project: "org", bootstrap: admin, grants: [] });
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: true });
}

const costs = (broken = false): Answer => (path, month) =>
  path === "/api/v1/organization/costs" ? { status: 200, body: COSTS(month, broken) } : undefined;

test.describe("Administration → Costs (T-5126)", () => {
  for (const width of [1440, 400]) {
    for (const lang of ["en", "sk"] as const) {
      test(`an administrator reads the month per project (${lang}, ${width} px)`, async ({ page }) => {
        await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
        await stubApi(page, costs());
        await page.goto(`/organization/costs?lang=${lang}`);
        await expect(page.getByText("bbsk", { exact: true }).first()).toBeVisible();
        await expect(page.getByText(lang === "en" ? "Infrastructure €43.70 + AI €6.46 = €50.16" : /Infraštruktúra 43,70/)).toBeVisible();
        await page.getByText(lang === "en" ? "2 spaces, 2 assistants" : "2 priestory, 2 asistenti").click();
        await expect(page.getByText(lang === "en" ? /Assistant pomocnik: 412 questions/ : /Asistent pomocnik: otázky 412/)).toBeVisible();
        expect(await axeViolations(page)).toEqual([]);
        await shot(page, `costs-filled-${lang}-${width}`);
      });
    }

    test(`a month a source did not answer is said to be partial at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
      await stubApi(page, costs(true));
      await page.goto("/organization/costs?lang=en");
      await expect(page.getByText(/Some spaces or assistants did not answer/)).toBeVisible();
      await page.getByText("2 spaces, 2 assistants").click();
      await expect(page.getByText("the assistant answered 503 Service Unavailable")).toBeVisible();
      await shot(page, `costs-partial-${width}`);
    });

    test(`no price table says what the operator sets at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
      const detail =
        "no price table is set: the operator sets portal.costPrices in the deployment values (JC_PORTAL_COST_PRICES_FILE) with each node, volume and fixed item";
      await stubApi(page, (path) =>
        path === "/api/v1/organization/costs"
          ? { status: 503, body: { type: "about:blank", title: "Service Unavailable", status: 503, detail } }
          : undefined,
      );
      await page.goto("/organization/costs?lang=en");
      await expect(page.getByText(/portal\.costPrices/)).toBeVisible();
      await shot(page, `costs-no-prices-${width}`);
    });

    test(`an organization without projects says what to do at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
      await stubApi(page, (path, month) =>
        path === "/api/v1/organization/costs" ? { status: 200, body: { ...COSTS(month), projects: [] } } : undefined,
      );
      await page.goto("/organization/costs?lang=en");
      await expect(page.getByText(/Create a Context Space in a project/)).toBeVisible();
      await shot(page, `costs-empty-${width}`);
    });

    test(`a member who does not administer is refused at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
      await stubApi(page, () => undefined, false);
      await page.goto("/organization/costs?lang=en");
      await expect(page.getByRole("main")).toBeVisible();
      await shot(page, `costs-refused-${width}`);
    });
  }
});
