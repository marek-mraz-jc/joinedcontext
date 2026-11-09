import { expect, test } from "@playwright/test";
import type { Page, Route } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { axeViolations } from "./axe";

// The dashboard builder on the built bundle with the API answered from the browser: a reader,
// an empty new dashboard, a panel being added, the form of each of the four panel kinds with its
// preview, a save in flight, a dashboard waiting for approval, a conflict, a refused field and a
// failed preview. BUILDER_SHOTS=<dir> saves a full-height shot of each state at each width.

const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", email: "jana.kovacova@hel.fi", roles: [] };
const UID = "jcd-b1ce57a7105f0001";
const BUILDER = "/api/v1/projects/helsinki/grafana/builder";
const HOUR = 3_600_000;

const OPTIONS = {
  endpoints: [
    { name: "bikes", title: "Bike stations", slug: "bikes7yq2mfa3kd5pq4c6rtw2xnlb", public: true, usable: true },
    { name: "air", title: "Air quality", slug: "air4yq2mfa3kd5pq4c6rtw2xnlbz", public: true, usable: true },
  ],
  types: ["BikeHireDockingStation"],
  attributes: [
    { name: "availableBikeNumber", kinds: ["timeseries", "stat", "table"] },
    { name: "freeSlotNumber", kinds: ["timeseries", "stat", "table"] },
    { name: "status", kinds: ["table"] },
    { name: "name", kinds: ["table"] },
    { name: "location", kinds: ["table", "map"] },
  ],
};

const BOARD = {
  title: "Bike stations in the centre",
  description: "Free bikes and docks, read every five minutes.",
  from: "now-24h",
  to: "now",
  refresh: "5m",
  variables: [{ name: "status", label: "Station status", endpoint: "bikes", type: "BikeHireDockingStation", attribute: "status", multi: true, includeAll: true }],
  panels: [
    {
      id: 1,
      kind: "timeseries",
      title: "Free bikes, hourly average",
      grid: { x: 0, y: 0, w: 16, h: 9 },
      endpoint: "bikes",
      type: "BikeHireDockingStation",
      attribute: "availableBikeNumber",
      aggregation: "avg",
      interval: "PT1H",
      unit: "short",
      filters: [{ attribute: "status", op: "eq", variable: "status" }],
    },
    {
      id: 2,
      kind: "stat",
      title: "Free bikes now",
      grid: { x: 16, y: 0, w: 8, h: 9 },
      endpoint: "bikes",
      type: "BikeHireDockingStation",
      attribute: "availableBikeNumber",
      aggregation: "sum",
      interval: "auto",
      decimals: 0,
      thresholds: { base: "red", steps: [{ value: 20, color: "yellow" }, { value: 60, color: "green" }] },
    },
    {
      id: 3,
      kind: "table",
      title: "Stations with few docks",
      grid: { x: 0, y: 9, w: 16, h: 9 },
      endpoint: "bikes",
      type: "BikeHireDockingStation",
      attribute: "freeSlotNumber",
      aggregation: "none",
      interval: "auto",
      filters: [{ attribute: "freeSlotNumber", op: "lt", value: "3" }],
    },
    {
      id: 4,
      kind: "gauge",
      title: "Free docks, maximum",
      grid: { x: 16, y: 9, w: 8, h: 9 },
      endpoint: "bikes",
      type: "BikeHireDockingStation",
      attribute: "freeSlotNumber",
      aggregation: "max",
      interval: "auto",
      unit: "short",
      thresholds: { base: "green", steps: [{ value: 80, color: "orange" }] },
    },
  ],
};

interface World {
  mayEdit: boolean;
  pending: boolean;
  save: "ok" | "slow" | "conflict" | "refused";
  preview: "ok" | "fail";
  sent: { method: string; path: string; body: unknown }[];
}

const world = (over: Partial<World> = {}): World => ({ mayEdit: true, pending: false, save: "ok", preview: "ok", sent: [], ...over });

const wave = (n: number, base: number, swing: number) => Array.from({ length: n }, (_, i) => Math.round(base + swing * Math.sin(i / 3) + (swing / 3) * Math.cos(i / 1.7)));
const times = (n: number) => Array.from({ length: n }, (_, i) => Date.now() - 24 * HOUR + (i * 24 * HOUR) / (n - 1));

/** What the Portal's preview answers for a panel: the panel as compiled, and its frames. */
function previewOf(board: typeof BOARD, id: number) {
  const panel = board.panels.find((each) => each.id === id) ?? board.panels[0];
  const type = panel.kind;
  const thresholds = panel.thresholds
    ? { mode: "absolute", steps: [{ color: panel.thresholds.base, value: null }, ...panel.thresholds.steps.map((step) => ({ color: step.color, value: step.value }))] }
    : undefined;
  const compiled = {
    id,
    type,
    title: panel.title,
    gridPos: panel.grid,
    fieldConfig: { defaults: { unit: panel.unit, decimals: panel.decimals, ...(thresholds ? { thresholds, color: { mode: "thresholds" } } : {}) }, overrides: [] },
    options: type === "stat" || type === "gauge" ? { reduceOptions: { calcs: ["lastNotNull"] } } : {},
  };
  if (type === "table") {
    return {
      panel: compiled,
      frames: [
        {
          name: "BikeHireDockingStation",
          refId: "A",
          fields: [
            { name: "Station", type: "string", values: ["Kamppi (M)", "Rautatientori", "Kaivopuisto", "Hakaniemi (M)"] },
            { name: "freeSlotNumber", type: "number", values: [0, 1, 2, 2] },
          ],
        },
      ],
    };
  }
  if (type === "timeseries") {
    return {
      panel: compiled,
      frames: [{ name: "", refId: "A", fields: [{ name: "Time", type: "time", values: times(24) }, { name: "availableBikeNumber", type: "number", values: wave(24, 640, 180) }] }],
    };
  }
  return { panel: compiled, frames: [{ name: "", refId: "A", fields: [{ name: "Time", type: "time", values: times(6) }, { name: panel.attribute, type: "number", values: type === "stat" ? [512, 530, 541, 548, 552, 557] : [61, 64, 70, 72, 74, 77] }] }] };
}

async function stubApi(page: Page, w: World): Promise<void> {
  await page.route("**/api/v1/**", async (route: Route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: status >= 400 ? "application/problem+json" : "application/json", body: JSON.stringify(body) });
    const problem = (status: number, detail: string, errors?: string[]) => json({ type: "about:blank", title: status === 409 ? "Conflict" : "Refused", status, detail, ...(errors ? { errors } : {}) }, status);
    if (path.startsWith(BUILDER) && request.method() !== "GET") w.sent.push({ method: request.method(), path, body: request.postDataJSON() as unknown });
    const pending = w.pending ? { uid: UID, title: BOARD.title, change: "chg-0000004f", url: "https://forge.example/helsinki/config/pulls/79" } : null;
    if (path === BUILDER && request.method() === "GET") {
      return json({ mayEdit: w.mayEdit, pending: pending ? [pending] : [], refreshes: ["", "1m", "5m", "15m", "1h"], units: ["", "short", "percent", "celsius", "none"], colors: ["green", "yellow", "orange", "red", "blue", "purple", "text"] });
    }
    if (path === `${BUILDER}/preview`) {
      if (w.preview === "fail") return problem(403, "panels[1].endpoint: bikes is not an Endpoint of helsinki you may read; choose another from the list, or ask a project owner for read access");
      const asked = request.postDataJSON() as { board: typeof BOARD; panel: number };
      return json(previewOf(asked.board, asked.panel));
    }
    if (path === `${BUILDER}/values`) return json({ values: ["closed", "full", "open"] });
    if (path === `${BUILDER}/${UID}` && request.method() === "GET") {
      if (!w.mayEdit) return problem(403, "you may not propose a Dashboard in helsinki");
      return json({ uid: UID, base: "blob-7", board: BOARD, pending });
    }
    if ((path === BUILDER && request.method() === "POST") || (path === `${BUILDER}/${UID}` && request.method() === "PUT")) {
      if (w.save === "slow") return new Promise<void>(() => undefined);
      if (w.save === "conflict") return problem(409, "the dashboard changed since you opened it; open it again to edit the saved version");
      if (w.save === "refused") return problem(422, "the dashboard was refused", ["panels[2].filters[0].value: freeSlotNumber holds numbers; write a number, for example 3", "variables[0].attribute: status holds no text in BikeHireDockingStation any more; choose another attribute"]);
      w.pending = true;
      return json({ uid: UID, change: { metadata: { name: "chg-0000004f" } } }, 202);
    }
    if (path.endsWith("/grafana/options")) {
      const endpoint = url.searchParams.get("endpoint");
      const type = url.searchParams.get("type");
      return json({ endpoints: OPTIONS.endpoints, types: endpoint ? OPTIONS.types : [], attributes: endpoint && type ? OPTIONS.attributes : [] });
    }
    if (path.endsWith("/grafana/dashboards")) return json({ ready: true, items: [] });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: "helsinki" }] });
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
}

const SHOTS = process.env.BUILDER_SHOTS;
if (SHOTS) mkdirSync(SHOTS, { recursive: true });

/** The whole page at full height, so the sticky sidebar is not cut (KNOWLEDGE T-6423). */
async function shot(page: Page, name: string, width: number): Promise<void> {
  if (!SHOTS) return;
  await page.waitForTimeout(300);
  const height = await page.evaluate(() => document.documentElement.scrollHeight);
  await page.setViewportSize({ width, height: Math.min(Math.max(height, 700), 7000) });
  await page.waitForTimeout(500);
  await page.screenshot({ path: `${SHOTS}/${name}-${width}.png` });
  await page.setViewportSize({ width, height: 900 });
}

const save = (page: Page) => page.getByRole("button", { name: /^(Propose dashboard|Proposing…)$/ });
const panelForm = (page: Page) => page.locator("section").filter({ has: page.getByRole("heading", { level: 2, name: /^Panel: / }) });

for (const width of [1440, 400]) {
  test.describe(`at ${width} px`, () => {
    test.use({ viewport: { width, height: 900 } });

    test("a reader meets no edit control and reads whom to ask", async ({ page }) => {
      await stubApi(page, world({ mayEdit: false }));
      await page.goto(`/projects/helsinki/grafana/dashboards/${UID}/edit`);
      await expect(page.getByText("You cannot build dashboards in this project")).toBeVisible();
      await expect(save(page)).toHaveCount(0);
      await expect(page.getByRole("button", { name: "Add panel" })).toHaveCount(0);
      await shot(page, "readonly", width);
    });

    test("a new dashboard starts empty and a panel is added in the first free spot", async ({ page }) => {
      await stubApi(page, world());
      await page.goto("/projects/helsinki/grafana/new");
      await expect(page.getByText("This dashboard has no panels yet")).toBeVisible();
      await expect(save(page)).toHaveAttribute("aria-disabled", "true");
      await shot(page, "empty", width);
      await page.locator("#builder-title").fill("Bike stations in the centre");
      await page.getByRole("button", { name: "Add panel" }).click();
      const form = panelForm(page);
      await expect(form.getByRole("heading", { name: "Panel: Panel 1" })).toBeVisible();
      await expect(form.locator("#builder-panel-1-endpoint")).toBeVisible();
      await shot(page, "adding", width);
      await form.locator("#builder-panel-1-endpoint").selectOption("bikes");
      await form.locator("#builder-panel-1-type").selectOption("BikeHireDockingStation");
      await form.locator("#builder-panel-1-attribute").selectOption("availableBikeNumber");
      await expect(save(page)).not.toHaveAttribute("aria-disabled", "true");
    });

    for (const [index, kind] of (["timeseries", "stat", "table", "gauge"] as const).entries()) {
      test(`the ${kind} form reads its choices and previews as the person`, async ({ page }) => {
        const w = world();
        await stubApi(page, w);
        await page.goto(`/projects/helsinki/grafana/dashboards/${UID}/edit`);
        const title = BOARD.panels[index].title;
        await page.getByRole("button", { name: new RegExp(`^${title}`) }).click();
        const form = panelForm(page);
        await expect(form.getByRole("heading", { name: `Panel: ${title}` })).toBeVisible();
        await expect(form.locator(`#builder-panel-${index + 1}-attribute`)).toHaveValue(BOARD.panels[index].attribute);
        await form.getByRole("button", { name: "Preview" }).click();
        await expect(form.locator(`section[data-panel-type="${kind}"]`)).toBeVisible();
        const asked = w.sent.find((each) => each.path.endsWith("/preview"));
        expect(JSON.stringify(asked?.body)).not.toMatch(/customQuery|targets|"q"/);
        await shot(page, `form-${kind}`, width);
      });
    }

    test("a save in flight, then the dashboard waits for approval", async ({ page }) => {
      const w = world({ save: "slow" });
      await stubApi(page, w);
      await page.goto(`/projects/helsinki/grafana/dashboards/${UID}/edit`);
      await page.locator("#builder-title").fill("Bike stations in the centre, weekdays");
      await save(page).click();
      await expect(page.getByRole("button", { name: "Proposing…" })).toBeVisible();
      await shot(page, "saving", width);
      await page.unrouteAll({ behavior: "ignoreErrors" });
      await stubApi(page, world({ pending: true }));
      await page.reload();
      await expect(page.getByText("Waiting for approval").first()).toBeVisible();
      await expect(page.getByRole("link", { name: "Open the change" })).toHaveAttribute("href", "/projects/helsinki/approvals/chg-0000004f");
      await expect(save(page)).toHaveAttribute("aria-disabled", "true");
      await shot(page, "pending", width);
    });

    test("a conflict offers the saved version", async ({ page }) => {
      await stubApi(page, world({ save: "conflict" }));
      await page.goto(`/projects/helsinki/grafana/dashboards/${UID}/edit`);
      await save(page).click();
      await expect(page.getByText("This dashboard changed since you opened it")).toBeVisible();
      await expect(page.getByRole("button", { name: "Open the saved version" })).toBeVisible();
      await shot(page, "conflict", width);
    });

    test("a refusal names each field in the form's words", async ({ page }) => {
      await stubApi(page, world({ save: "refused" }));
      await page.goto(`/projects/helsinki/grafana/dashboards/${UID}/edit`);
      // A value the server refuses: words where the attribute holds numbers.
      await page.getByRole("button", { name: /^Stations with few docks/ }).click();
      await page.locator("#builder-filter-3-0-value").fill("a few");
      await save(page).click();
      await expect(page.getByText("The dashboard was not proposed")).toBeVisible();
      await expect(page.getByText(/Panel “Stations with few docks”, Filter: Value/)).toBeVisible();
      await expect(page.getByText(/panels\[2\]/)).toHaveCount(0);
      await shot(page, "refused", width);
      // Each refused field is a way to it: the panel opens and the field takes the focus.
      await page.getByRole("button", { name: "Panel “Stations with few docks”, Filter: Value:" }).click();
      await expect(page.locator("#builder-filter-3-0-value")).toBeFocused();
      // The field carries its refusal until it is edited.
      await expect(page.locator("#builder-filter-3-0-value")).toHaveAttribute("aria-invalid", "true");
      await expect(page.locator("#builder-var-0-attribute")).toHaveAttribute("aria-invalid", "true");
      await page.locator("#builder-filter-3-0-value").fill("3");
      await expect(page.locator("#builder-filter-3-0-value")).not.toHaveAttribute("aria-invalid", "true");
    });

    test("a failed preview says why", async ({ page }) => {
      await stubApi(page, world({ preview: "fail" }));
      await page.goto(`/projects/helsinki/grafana/dashboards/${UID}/edit`);
      await page.getByRole("button", { name: /^Free bikes now/ }).click();
      await panelForm(page).getByRole("button", { name: "Preview" }).click();
      await expect(page.getByText("The preview could not be read")).toBeVisible();
      await expect(page.getByText("Panel “Free bikes now”, Endpoint: bikes is not an Endpoint of helsinki you may read; choose another from the list, or ask a project owner for read access")).toBeVisible();
      await shot(page, "preview-failed", width);
    });
  });
}

test("a panel dragged by its top bar moves on the grid and pushes the one it lands on down", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await stubApi(page, world());
  await page.goto(`/projects/helsinki/grafana/dashboards/${UID}/edit`);
  const handle = page.locator("[data-drag-handle]").nth(1);
  const box = await handle.boundingBox();
  if (!box) throw new Error("no drag handle");
  await page.mouse.move(box.x + 20, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x - 600, box.y + box.height / 2, { steps: 8 });
  await page.mouse.up();
  await page.getByRole("button", { name: /^Free bikes now/ }).click();
  await expect(panelForm(page).getByLabel("Column")).toHaveValue("0");
  await expect(panelForm(page).getByLabel("Row")).toHaveValue("0");
  // The time series sat where the stat landed: it moved below it, nothing overlaps.
  await page.getByRole("button", { name: /^Free bikes, hourly average/ }).click();
  await expect(panelForm(page).getByLabel("Row")).toHaveValue("9");
});

test("the builder has no axe violations", async ({ page }) => {
  await stubApi(page, world());
  await page.goto(`/projects/helsinki/grafana/dashboards/${UID}/edit`);
  await page.getByRole("button", { name: /^Free bikes now/ }).click();
  await expect(panelForm(page)).toBeVisible();
  expect(await axeViolations(page)).toEqual([]);
});
