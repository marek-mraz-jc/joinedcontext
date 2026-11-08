import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { dirname, join } from "node:path";
import { existsSync, mkdirSync, readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { pageFindings } from "../tests/pageChecks";

// The deployment's nine dashboards and the three range fixtures, drawn by the Portal over the
// same recorded frames the reference Grafana 12.4.14 drew (scripts/grafana-reference.mjs): the
// same absolute range, Europe/Bratislava, 1440 and 400 px. PARITY_SHOTS=<dir> saves each page as
// portal-<dashboard>-<width>.png beside grafana-<dashboard>-<width>.png for the pairwise read.

const here = dirname(fileURLToPath(import.meta.url));
const NAMESPACES = Object.keys(JSON.parse(readFileSync(join(here, "../src/locales/en.json"), "utf8")) as object);
const FIXTURES = join(here, "../tests/fixtures");
const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", email: "jana.kovacova@hel.fi", roles: ["portal-approver"] };
// scripts/panel-frames.mjs: every recorded range ends here, and the runs the k6 variable offers.
const TO = Date.UTC(2026, 10, 8, 6, 0, 0);
const K6_RUNS = ["k6-2026-11-07T21", "k6-2026-11-08T05"];
const UNIT: Record<string, number> = { m: 60_000, h: 3_600_000, d: 86_400_000 };

type Board = Record<string, unknown> & { uid: string; title: string; panels?: Record<string, unknown>[]; time?: { from?: string; to?: string } };
const files = readdirSync(join(FIXTURES, "dashboards")).filter((file) => file.endsWith(".json"));
const BOARDS = new Map<string, { stem: string; board: Board }>(
  files.map((file) => {
    const board = JSON.parse(readFileSync(join(FIXTURES, "dashboards", file), "utf8")) as Board;
    return [board.uid, { stem: file.replace(/\.json$/, ""), board }];
  }),
);

function rangeOf(board: Board): { from: number; to: number } {
  const match = /^now-(\d+)([mhd])$/.exec(board.time?.from ?? "now-6h");
  return { from: TO - (match ? Number(match[1]) * UNIT[match[2]] : 6 * UNIT.h), to: TO };
}

/** The frames a panel's datasource answered, after daiya's transformations where they were recorded. */
function frames(uid: string, id: number): unknown[] {
  const { stem } = BOARDS.get(uid)!;
  const own = join(FIXTURES, "frames", `${stem}-${id}.json`);
  const path = existsSync(own) ? own : join(FIXTURES, "frames", `context-broker-${id}.json`);
  const fixture = JSON.parse(readFileSync(path, "utf8")) as { frames: unknown[]; served?: unknown[] };
  return fixture.served ?? fixture.frames;
}

/** The dashboard as the Portal's route answers it (src/api/grafana_dashboards.rs dashboard_view). */
function view(board: Board) {
  const keep = ["id", "type", "title", "description", "gridPos", "fieldConfig", "options", "collapsed", "maxDataPoints", "timeFrom", "timeShift", "hideTimeOverride"];
  const stripped = (panel: Record<string, unknown>): Record<string, unknown> => {
    const out = Object.fromEntries(Object.entries(panel).filter(([key]) => keep.includes(key)));
    if (Array.isArray(panel.panels)) out.panels = (panel.panels as Record<string, unknown>[]).map(stripped);
    return out;
  };
  const list = ((board.templating as { list?: Record<string, unknown>[] } | undefined)?.list ?? []).map((v) => {
    const current = (v.current as { value?: string | string[] } | undefined)?.value;
    return {
      name: v.name,
      label: v.label ?? "",
      kind: v.type,
      multi: v.multi === true,
      includeAll: v.includeAll === true,
      options: ((v.options as { text: string; value: string }[] | undefined) ?? []).map((o) => ({ text: o.text, value: o.value })),
      // Nothing saved: All where the variable offers it, as the server's view picks (Grafana's pick).
      current: current === undefined ? (v.includeAll === true ? ["$__all"] : []) : Array.isArray(current) ? current : [current],
    };
  });
  return {
    uid: board.uid,
    orgId: 4,
    title: board.title,
    description: board.description ?? null,
    from: board.time?.from ?? "now-6h",
    to: board.time?.to ?? "now",
    refresh: board.refresh || null,
    timePickerHidden: (board.timepicker as { hidden?: boolean } | undefined)?.hidden === true,
    timezone: board.timezone || null,
    variables: list,
    panels: (board.panels ?? []).map(stripped),
  };
}

async function stubApi(page: Page, asked: string[]): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    const panel = /\/grafana\/dashboards\/([^/]+)\/panels\/(\d+)$/.exec(url.pathname);
    if (panel) {
      asked.push(`${panel[1]}/${panel[2]}${url.search}`);
      return json({ frames: frames(decodeURIComponent(panel[1]), Number(panel[2])) });
    }
    if (/\/grafana\/dashboards\/[^/]+\/variables\/[^/]+$/.test(url.pathname)) return json({ options: K6_RUNS.map((run) => ({ text: run, value: run })) });
    const one = /\/grafana\/dashboards\/([^/]+)$/.exec(url.pathname);
    if (one) {
      const found = BOARDS.get(decodeURIComponent(one[1]));
      return found ? json(view(found.board)) : json({ status: 404, title: "Not Found", detail: "no such dashboard" }, 404);
    }
    if (url.pathname.endsWith("/grafana/dashboards")) return json({ ready: true, items: LIST });
    if (url.pathname.endsWith("/grafana")) return json({ ready: true, mayCreate: false, items: [] });
    if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
    if (url.pathname === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: "helsinki" }] });
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
}

/** The project's dashboards as daiya's search lists them, in folders as the deployment files them. */
const LIST = [
  { uid: "project-helsinki", title: "Helsinki air and traffic", tags: ["helsinki", "air"], folder: "Helsinki" },
  { uid: "helsinki-history", title: "Helsinki pm25 history", tags: ["air"], folder: "Helsinki" },
  { uid: "project-bbsk", title: "Banská Bystrica region", tags: ["bbsk"], folder: "Banská Bystrica" },
  { uid: "jc-k6-load", title: "k6 load", tags: ["load", "platform"], folder: "Platform" },
  { uid: "jc-context-broker", title: "Context broker", tags: ["platform"], folder: "Platform" },
  { uid: "jc-context-gateway", title: "Context gateway", tags: ["platform"], folder: "Platform" },
  { uid: "jc-slo", title: "Service levels", tags: ["platform", "slo"], folder: "Platform" },
  { uid: "scratch", title: "Scratch board", tags: [] },
];

const SHOTS = process.env.PARITY_SHOTS;
if (SHOTS) mkdirSync(SHOTS, { recursive: true });

async function open(page: Page, board: Board, asked: string[]): Promise<void> {
  await page.clock.setFixedTime(TO);
  await stubApi(page, asked);
  const { from, to } = rangeOf(board);
  await page.goto(`/projects/helsinki/grafana/dashboards/${board.uid}${board.time ? `?from=${from}&to=${to}` : ""}`);
  await expect(page.getByRole("heading", { level: 1, name: board.title })).toBeVisible();
  await expect(page.getByRole("status", { name: /^Reading / })).toHaveCount(0);
  await page.waitForTimeout(800);
}

async function shot(page: Page, name: string, width: number): Promise<void> {
  if (!SHOTS) return;
  const height = await page.evaluate(() => document.documentElement.scrollHeight);
  await page.setViewportSize({ width, height: Math.min(Math.max(height, 600), 9000) });
  await page.waitForTimeout(400);
  await page.screenshot({ path: `${SHOTS}/portal-${name}-${width}.png` });
}

for (const width of [1440, 400]) {
  test.describe(`parity at ${width} px`, () => {
    test.use({ viewport: { width, height: 900 }, timezoneId: "Europe/Bratislava", locale: "en-GB" });

    for (const [uid, { stem, board }] of BOARDS) {
      test(`${stem} draws over Grafana's frames`, async ({ page }) => {
        const asked: string[] = [];
        await open(page, board, asked);
        await shot(page, stem, width);
        expect(asked.length).toBeGreaterThan(0);
        expect(await page.evaluate(pageFindings, { namespaces: NAMESPACES, layout: true })).toEqual([]);
        void uid;
      });
    }

    test("the time picker opens Grafana's quick ranges and an absolute range", async ({ page }) => {
      const { board } = BOARDS.get("jc-context-broker")!;
      await open(page, board, []);
      await page.getByText("2026-11-08 01:00:00 to 2026-11-08 07:00:00", { exact: true }).first().click();
      await expect(page.getByRole("button", { name: "This fiscal year", exact: true })).toBeVisible();
      await shot(page, "context-broker-timepicker", width);
      await page.keyboard.press("Escape");
      await page.getByLabel("Refresh", { exact: true }).focus();
      await shot(page, "context-broker-refresh", width);
    });
  });
}

for (const width of [1440, 400]) {
  test(`the dashboard list browses folders, searches, filters by tag and sorts at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await stubApi(page, []);
    await page.goto("/projects/helsinki/grafana");
    const tree = page.getByRole("list", { name: "Dashboards by folder" });
    await expect(tree).toBeVisible();
    await expect(tree.getByRole("button")).toHaveText([/^Banská Bystrica/, /^Helsinki/, /^Platform/]);
    await expect(tree.getByRole("link", { name: "Scratch board" })).toBeVisible();
    await shot(page, "list-folders", width);
    await tree.getByRole("button", { name: /^Platform/ }).click();
    await expect(tree.getByRole("link", { name: "k6 load" })).toBeVisible();
    await shot(page, "list-folder-open", width);
    await page.getByLabel("Search dashboards").fill("helsinki");
    const results = page.getByRole("list", { name: "Search results" });
    await expect(results.getByRole("link")).toHaveText(["Helsinki air and traffic", "Helsinki pm25 history"]);
    await shot(page, "list-search", width);
    await page.getByLabel("Search dashboards").fill("");
    await page.getByLabel("Tag").selectOption("platform");
    await page.getByLabel("Sort").selectOption("alpha-desc");
    await expect(page.getByRole("list", { name: "Search results" }).getByRole("link")).toHaveText(["Service levels", "k6 load", "Context gateway", "Context broker"]);
    await shot(page, "list-tag", width);
    await page.getByLabel("Search dashboards").fill("no such board");
    await expect(page.getByRole("status").filter({ hasText: "No dashboard matches this search." })).toBeVisible();
    await shot(page, "list-no-match", width);
    expect(await page.evaluate(pageFindings, { namespaces: NAMESPACES, layout: true })).toEqual([]);
  });
}

test.describe("parity of the range rules", () => {
  test.use({ viewport: { width: 1440, height: 900 }, timezoneId: "Europe/Bratislava", locale: "en-GB" });

  test("a hidden time picker leaves no range or refresh control", async ({ page }) => {
    await open(page, BOARDS.get("parity-hidden-picker")!.board, []);
    await expect(page.getByLabel("Refresh", { exact: true })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Refresh now" })).toHaveCount(0);
  });

  test("a dashboard without a saved time reads the last six hours", async ({ page }) => {
    const asked: string[] = [];
    await open(page, BOARDS.get("parity-no-time")!.board, asked);
    await expect(page.getByText("Last 6 hours", { exact: true }).first()).toBeVisible();
    const first = new URLSearchParams(asked[0].split("?")[1]);
    expect(Number(first.get("to")) - Number(first.get("from"))).toBe(6 * UNIT.h);
  });

  test("a panel's shift moves its own read a day back and names it, unless the panel hides it", async ({ page }) => {
    const asked: string[] = [];
    const { board } = BOARDS.get("parity-time-override")!;
    await open(page, board, asked);
    const { from, to } = rangeOf(board);
    const read = (id: number) => new URLSearchParams(asked.find((each) => each.startsWith(`parity-time-override/${id}?`))!.split("?")[1]);
    expect([Number(read(1).get("from")), Number(read(1).get("to"))]).toEqual([from - UNIT.d, to - UNIT.d]);
    // Under an absolute range Grafana 12.4 reads no timeFrom: panel 3 keeps the dashboard's range.
    expect([Number(read(3).get("from")), Number(read(3).get("to"))]).toEqual([from, to]);
    await expect(page.getByRole("region", { name: /a day back\)$/ }).getByText("Timeshift -1d")).toBeVisible();
    await expect(page.getByRole("region", { name: /override hidden/ }).locator("[data-time-info]")).toHaveCount(0);
  });

  test("the k6 run variable sends one var-testid per picked run", async ({ page }) => {
    const asked: string[] = [];
    await open(page, BOARDS.get("jc-k6-load")!.board, asked);
    await page.locator("summary", { hasText: "All" }).first().click();
    await page.getByRole("checkbox", { name: K6_RUNS[0] }).click();
    await page.getByRole("checkbox", { name: K6_RUNS[1] }).click();
    await expect.poll(() => asked.some((each) => new URLSearchParams(each.split("?")[1]).getAll("var-testid").join() === K6_RUNS.join())).toBe(true);
    await shot(page, "k6-load-picked", 1440);
  });
});
