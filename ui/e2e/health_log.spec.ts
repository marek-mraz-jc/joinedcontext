import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";

// The health log (T-5067): a project's Health page as a steward meets it, errors first with the
// fix under each, in English and Slovak, light and dark, at 1440 and 400 px; an empty log; a
// member of another project refused; and the administrator's view across projects. `vite preview`
// has no Portal API behind it, so the API is answered in the browser; the page is the real build.

const IDENTITY = { subject: "s1", username: "demo.steward", name: "Demo Steward", email: "steward@bbsk.sk", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const days = (statuses: string[]) =>
  statuses.map((status, i) => ({ at: new Date(Date.UTC(2026, 9, 15, i * 4)).toISOString(), status }));

const none = { code: "", args: {}, text: "" };
const said = (code: string, text: string, args: Record<string, string> = {}) => ({ code, args, text });

const check = (over: Record<string, unknown>) => ({
  project: "bbsk",
  kind: "pipeline",
  check: "runs",
  status: "ok",
  since: "2026-10-20T06:00:00Z",
  lastOk: "2026-10-21T08:00:00Z",
  checkedAt: "2026-10-21T08:00:00Z",
  detail: none,
  fix: none,
  history: days(Array(30).fill("ok")),
  ...over,
});

const CHECKS = [
  ...["podujatia-brezno", "podujatia-detva", "podujatia-revuca"].map((name) =>
    check({
      name,
      status: "error",
      since: "2026-10-19T06:00:00Z",
      lastOk: "2026-10-18T06:00:00Z",
      detail: said("", "3 runs in a row sent nothing: 403 no policy grants this operation"),
      fix: said("policyWrite", `The broker refused the write: the target Endpoint's Policy must grant pl-${name} create and update.`, { name }),
      evidenceUrl: `/projects/bbsk/pipelines/${name}`,
      history: days([...Array(20).fill("ok"), ...Array(10).fill("error")]),
    }),
  ),
  check({
    kind: "endpoint",
    name: "podujatia-api",
    check: "reconcile",
    status: "warn",
    detail: said("waitingApproval", "waiting for approval"),
    fix: said("approve", "Approve its change on the Approvals page; the next sync applies it."),
    evidenceUrl: "/projects/bbsk/endpoints/podujatia-api",
    history: days(["warn"]),
  }),
  check({
    kind: "endpoint",
    name: "podujatia-api",
    check: "odata",
    status: "error",
    detail: said("anonymousRefused", "odata refused an anonymous reader (401) though the Endpoint is public", { surface: "odata", status: "401", ms: "42" }),
    fix: said("publicRefused", "The Endpoint is public but its surface asks for a token."),
    evidenceUrl: "/projects/bbsk/endpoints/podujatia-api",
    history: days([...Array(28).fill("ok"), "warn", "error"]),
  }),
  check({
    kind: "space",
    name: "podujatia",
    check: "storage",
    status: "warn",
    detail: said("counted", "1520 entities, 48211 history rows", { entities: "1520", history: "48211" }),
    fix: said("retention", "The history grows without a bound: set spec.retention on the Context Space."),
    evidenceUrl: "/projects/bbsk/spaces/podujatia",
  }),
  check({ name: "podujatia-zvolen", check: "freshness", evidenceUrl: "/projects/bbsk/pipelines/podujatia-zvolen" }),
  check({ kind: "space", name: "podujatia", check: "reconcile", evidenceUrl: "/projects/bbsk/spaces/podujatia" }),
];

async function stubApi(page: Page, answer: (path: string) => { status: number; body: unknown } | undefined): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    const answered = answer(path);
    if (answered) return json(answered.body, answered.status);
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "bbsk" }] });
    if (path.endsWith("/permissions/me")) return json({ project: "bbsk", bootstrap: true, grants: [] });
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: true });
}

const filled = (path: string) => (path.endsWith("/health-checks") ? { status: 200, body: { items: CHECKS } } : undefined);

test.describe("the health log (T-5067)", () => {
  for (const width of [1440, 400]) {
    for (const [lang, scheme] of [["en", "light"], ["sk", "dark"]] as const) {
      test(`a steward reads the errors first with their fix (${lang}, ${scheme}, ${width} px)`, async ({ page }) => {
        await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
        await page.emulateMedia({ colorScheme: scheme });
        await stubApi(page, filled);
        await page.goto(`/projects/bbsk/health?lang=${lang}`);
        const caption = lang === "en" ? "Health checks" : "Kontroly zdravia";
        const rows = page.getByRole("table", { name: caption }).getByRole("row").filter({ hasNot: page.getByRole("columnheader") });
        // The log opens on what needs attention; the two healthy checks wait behind the filter.
        await expect(rows).toHaveCount(CHECKS.filter((item) => item.status !== "ok").length);
        await expect(rows.nth(0)).toContainText("403 no policy grants this operation");
        await expect(rows.nth(0)).toContainText("pl-podujatia-brezno");
        await expect(rows.nth(0)).toContainText(lang === "en" ? "Endpoint's Policy must grant" : "Policy cieľového Endpointu");
        expect(await axeViolations(page)).toEqual([]);
        await shot(page, `health-filled-${lang}-${scheme}-${width}`);
      });
    }

    test(`the status filter narrows the log at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
      await stubApi(page, filled);
      await page.goto("/projects/bbsk/health?lang=en");
      await page.locator("#health-status").selectOption("warn");
      const rows = page.getByRole("table", { name: "Health checks" }).getByRole("row").filter({ hasNot: page.getByRole("columnheader") });
      await expect(rows).toHaveCount(2);
      await shot(page, `health-filtered-${width}`);
    });

    test(`an empty log says what to do at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
      await stubApi(page, (path) => (path.endsWith("/health-checks") ? { status: 200, body: { items: [] } } : undefined));
      await page.goto("/projects/bbsk/health?lang=en");
      await expect(page.getByText("Nothing checked yet")).toBeVisible();
      await shot(page, `health-empty-${width}`);
    });

    test(`a member of another project is refused at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
      await stubApi(page, (path) =>
        path.endsWith("/health-checks")
          ? { status: 404, body: { type: "about:blank", title: "Not Found", status: 404, detail: "project 'bbsk' not found" } }
          : undefined,
      );
      await page.goto("/projects/bbsk/health?lang=en");
      await expect(page.getByRole("alert").first()).toContainText("project 'bbsk' not found");
      await expect(page.getByText("403 no policy grants")).toHaveCount(0);
      await shot(page, `health-refused-${width}`);
    });

    test(`the administrator sees every project at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
      await stubApi(page, (path) =>
        path.endsWith("/organization/health-checks")
          ? { status: 200, body: { items: [...CHECKS, check({ project: "helsinki", kind: "model", name: "air", check: "reconcile", evidenceUrl: "/projects/helsinki/models/air" })] } }
          : path.endsWith("/organization/health")
            ? { status: 200, body: { checks: [] } }
            : undefined,
      );
      await page.goto("/organization/health?lang=en");
      await page.locator("#health-status").selectOption("all");
      await expect(page.getByText("Project: helsinki").filter({ visible: true })).toHaveCount(1);
      await shot(page, `health-admin-${width}`);
    });
  }
});
