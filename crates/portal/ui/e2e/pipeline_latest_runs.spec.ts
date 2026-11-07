import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";

// The pipelines list's Last run column (T-5818): one request reads every row's newest run,
// however many rows there are, and a failed read says so in each row. `vite preview` has no
// Portal API behind it, so the API is answered in the browser; the page is the real build.

const IDENTITY = { subject: "s1", username: "demo.steward", name: "Demo Steward", email: "steward@bbsk.sk", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const NAMES = Array.from({ length: 12 }, (_, i) => `podujatia-${String(i + 1).padStart(2, "0")}`);
const pipeline = (name: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Pipeline",
  metadata: { name, namespace: "bbsk" },
  spec: { class: "scheduled", schedule: "0 6 * * *" },
  status: { phase: "Live" },
});
const run = (sent: number, rejected: number, failed: number) => {
  const at = new Date(Date.now() - 4 * 60_000).toISOString();
  return { run: at, firstAt: at, lastAt: at, sent, rejected, failed };
};
const LATEST = [
  { pipeline: "podujatia-01", run: run(64, 0, 0) },
  { pipeline: "podujatia-02", run: run(12, 3, 0) },
  { pipeline: "podujatia-03", run: run(0, 0, 2) },
];

async function stubApi(page: Page, latest: "ok" | "down", asked: string[]): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    asked.push(path);
    const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "bbsk" }] });
    if (path.endsWith("/permissions/me")) {
      return json({ project: "bbsk", grants: [{ rule: { kinds: ["Pipeline"], verbs: ["read", "propose"] } }] });
    }
    if (path.endsWith("/pipeline-runs")) {
      return latest === "ok"
        ? json({ items: LATEST })
        : route.fulfill({
            status: 503,
            contentType: "application/problem+json",
            body: JSON.stringify({ title: "Unavailable", status: 503, detail: "the pipelines' runs could not be read; try again" }),
          });
    }
    if (path.endsWith("/pipelines")) return json(list(NAMES.map(pipeline)));
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: true });
}

test.describe("the pipelines list's last run (T-5818)", () => {
  for (const width of [1440, 400]) {
    test(`every row's last run comes from one request at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
      const asked: string[] = [];
      await stubApi(page, "ok", asked);
      await page.goto("/projects/bbsk/pipelines?lang=en");
      const row = (name: string) => page.getByRole("row").filter({ hasText: name });
      await expect(row("podujatia-01")).toContainText("64 written");
      await expect(row("podujatia-02")).toContainText("3 rejected");
      await expect(row("podujatia-03")).toContainText("Run failed");
      await expect(row("podujatia-12")).toContainText("Has not run yet");
      expect(asked.filter((path) => path.endsWith("/pipeline-runs"))).toHaveLength(1);
      expect(asked.filter((path) => /\/pipelines\/[^/]+\/runs$/.test(path))).toEqual([]);
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `latest-runs-filled-${width}`);
    });

    test(`a failed read says the runs could not be read at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
      await stubApi(page, "down", []);
      await page.goto("/projects/bbsk/pipelines?lang=en");
      await expect(page.getByRole("row").filter({ hasText: "podujatia-01" })).toContainText("Runs could not be read");
      await shot(page, `latest-runs-error-${width}`);
    });
  }
});
