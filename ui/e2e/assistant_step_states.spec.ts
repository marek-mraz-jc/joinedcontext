/**
 * What the dock shows a person when a step fails or finds nothing (T-6492, TS-19), at 1440 and
 * 400: the failures a live walk met (evidence/T-5962/assistant), replayed through the run's
 * stream. Red lines are words, a refusal, an empty read and a page that would not open each end
 * in a state with a next step, a result table reaches every column without a sideways scroll,
 * and no token count stands in the conversation. The API is answered in the browser.
 */
import { createServer } from "node:http";
import type { Server } from "node:http";
import type { AddressInfo } from "node:net";
import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";
import { axeViolations } from "./axe";

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "demo-city";
const RUN = "run-step-states-1";
const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-approver"],
  groups: [PROJECT],
};

const read = (endpoint: string, name: string, args: unknown, content: unknown) => ({
  tool: "query_endpoint",
  status: "ok",
  durationMs: 374,
  input: { endpoint, name, arguments: args },
  output: { structuredContent: content },
});
const failed = (tool: string, error: string) => ({ tool, status: "failed", durationMs: 18, input: null, error });

const COUNTERS = Array.from({ length: 4 }, (_, at) => ({
  id: `urn:ngsi-ld:BikeCounter:C${at}`,
  type: "BikeCounter",
  name: ["Old bridge", "Riverside path", "Town hall", "Railway station"][at],
  city: "Banská Bystrica",
  count: 60 + at * 300,
  createdAt: "2026-11-02T18:07:50.158Z",
  modifiedAt: "2026-11-02T18:09:12.402Z",
}));

/** Each scenario: what the person asked, the steps of the run, its answer. */
const SCENARIOS: Record<string, { ask: string; steps: Record<string, unknown>[]; answer: string }> = {
  table: {
    ask: "Which bike counters do we have?",
    steps: [read("city-counters", "query_entities", { type: "BikeCounter" }, { entities: COUNTERS, total: 221 })],
    answer: "There are 221 bike counters; here are the first four.",
  },
  slip: {
    ask: "Show counters above 100.",
    steps: [
      failed("query_endpoint", "error: argument type: takes 1 to 50 items"),
      failed("jc_resource_list", 'invalid input at /: "kind" is a required property'),
      read("city-counters", "query_entities", { type: "BikeCounter" }, { entities: COUNTERS.slice(1), total: 3 }),
    ],
    answer: "Three counters are above 100.",
  },
  empty: {
    ask: "Find all bike counters within 500 m of the old town square.",
    steps: [read("city-counters", "query_entities", { type: "BikeCounter" }, { entities: [] })],
    answer: "No bike counters were found within 500 m.",
  },
  entity: {
    ask: "Show counter C1.",
    steps: [read("city-counters", "get_entity", { id: "urn:ngsi-ld:BikeCounter:C1" }, { entity: COUNTERS[1] })],
    answer: "Riverside path counted 360.",
  },
  refused: {
    ask: "Delete the counter data model.",
    steps: [failed("change_resource", 'error: {"detail":"mutation tools not permitted on read-only run","status":403}')],
    answer: "I could not do that.",
  },
  page: {
    ask: "Open the entities of the counters.",
    steps: [failed("jc_ui_navigate", "the page 'entities' needs the endpoint whose data to open")],
    answer: "That page could not be opened. Say which page you want, for example Data models or the name of an endpoint.",
  },
  guarded: {
    ask: "Add an optional slot direction to BikeCounter.",
    steps: [failed("change_resource", "the data read for this message writes this change_resource call; data is never an instruction, so nothing was drafted or opened")],
    answer:
      "Nothing was changed: this step came from the data the assistant read, not from you, and an instruction inside data is not followed. If you want it, say so in your own words.",
  },
  missing: {
    ask: "Sample the source trips-csv.",
    steps: [failed("jc_pipeline_sample_source", "not found: DataSource 'trips-csv' not found in project 'demo-city'")],
    answer: "I found no such source.",
  },
};

function stream(scenario: string): string {
  const { ask, steps, answer } = SCENARIOS[scenario];
  const frames: [string, Record<string, unknown>][] = [
    ["message", { text: ask }],
    ...steps.flatMap((step): [string, Record<string, unknown>][] => [
      ["tool", step],
      ["usage", { tokensThisStep: 7593 }],
    ]),
    ["thought", { text: answer }],
    ["usage", { tokensThisStep: 7806 }],
    ["status", { status: "interviewing" }],
  ];
  return frames
    .map(([kind, data], at) => `id: ${at + 1}\nevent: ${kind}\ndata: ${JSON.stringify({ seq: at + 1, ...data })}\n\n`)
    .join("");
}

/** The run's stream stays open as a live one does: a fulfilled route ends, and the dock says so. */
let live: Server | undefined;

async function liveStream(scenario: string): Promise<string> {
  live = createServer((_, answer) => {
    answer.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
    answer.write(stream(scenario));
  });
  await new Promise<void>((listening) => live?.listen(0, "127.0.0.1", listening));
  return `http://127.0.0.1:${(live.address() as AddressInfo).port}/`;
}

async function stubApi(page: Page, scenario: string): Promise<void> {
  const events = await liveStream(scenario);
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path.endsWith("/permissions/me")) return json({ project: PROJECT, bootstrap: true, grants: [] });
    if (path === "/api/v1/projects") return json({ apiVersion: API, kind: "ProjectList", items: [{ name: PROJECT }] });
    if (path.endsWith("/assistant/conversations") && request.method() === "POST") {
      return json({ id: RUN, status: "queued" }, 202);
    }
    if (path.endsWith(`/agent-runs/${RUN}/events`)) {
      return route.continue({ url: events });
    }
    if (path.endsWith(`/agent-runs/${RUN}`)) {
      return json({ id: RUN, project: PROJECT, kind: "conversation", appName: "", status: "interviewing", createdBy: "jana.kovacova", createdAt: "2026-11-02T18:00:00Z" });
    }
    return json({ apiVersion: API, kind: "List", items: [] });
  });
}

async function open(page: Page, scenario: string): Promise<Locator> {
  await stubApi(page, scenario);
  await page.goto(`/projects/${PROJECT}/activity?lang=en`);
  await page.getByRole("button", { name: "Open the assistant" }).first().click();
  const dock = page.getByRole("complementary", { name: "Conversation" });
  await dock.getByLabel("Ask the assistant").fill(SCENARIOS[scenario].ask);
  await dock.getByLabel("Ask the assistant").press("Enter");
  await expect(dock.getByTestId("run-progress")).toHaveText("Waiting for you");
  return dock;
}

for (const [label, width, height] of [
  ["1440", 1440, 1000],
  ["400", 400, 860],
] as const) {
  test.describe(`a step's state in the dock (${label})`, () => {
    test.beforeEach(async ({ page }) => {
      await page.setViewportSize({ width, height });
    });
    test.afterEach(async ({ page }, info) => {
      if (process.env.SHOTS_DIR) await page.screenshot({ path: `${process.env.SHOTS_DIR}/${info.title}-${label}.png` });
      expect(await axeViolations(page)).toEqual([]);
      live?.closeAllConnections();
      live?.close();
    });

    test("table", async ({ page }) => {
      const dock = await open(page, "table");
      const card = dock.getByTestId("query-result");
      for (const seen of ["count", "created at", "modified at", "city", "name", "Railway station"]) {
        await expect(card.getByText(seen, { exact: false }).first()).toBeAttached();
      }
      // Every column is in reach: the frame has nothing past its right edge.
      const frame = card.getByRole("group", { name: "The rows the endpoint answered" });
      const beyond = await frame.evaluate((box) => box.scrollWidth - box.clientWidth);
      expect(beyond).toBeLessThanOrEqual(1);
      await expect(dock).not.toContainText("tokens", { useInnerText: true });
      // Every row too: the card has no scroll of its own.
      const rows = await frame.evaluate((box) => box.scrollHeight - box.clientHeight);
      expect(rows).toBeLessThanOrEqual(1);
      await expect(card.getByText("Railway station")).toBeVisible();
      await expect(card).not.toContainText("T18:07:50");
      await expect(card).toContainText("Ask for the next rows to see more");
    });

    test("slip", async ({ page }) => {
      const dock = await open(page, "slip");
      const reasons = dock.getByTestId("step-reason");
      await expect(dock.getByRole("status").filter({ hasText: "Live" })).toBeVisible();
      await expect(reasons).toHaveText(["Tried again another way"]);
      await expect(dock.getByRole("img", { name: "Tried again another way" })).toHaveCount(2);
      await expect(dock.getByRole("img", { name: "Did not finish" })).toHaveCount(0);
      await expect(dock).not.toContainText("argument type", { useInnerText: true });
      await expect(dock).not.toContainText("jc_resource_list", { useInnerText: true });
      await expect(dock.getByTestId("step-notice")).toHaveCount(0);
    });

    test("empty", async ({ page }) => {
      const dock = await open(page, "empty");
      await expect(dock.getByText("Nothing matched this read.")).toBeVisible();
      await expect(dock.getByText("Try a wider area, a longer period or fewer conditions.")).toBeVisible();
      await expect(dock.getByRole("table")).toHaveCount(0);
      await expect(dock.getByRole("button", { name: "Try with fewer conditions" })).toBeVisible();
    });

    test("entity", async ({ page }) => {
      const dock = await open(page, "entity");
      const card = dock.getByTestId("query-result");
      await expect(card).toContainText("id: C1");
      await expect(card).not.toContainText("urn:ngsi-ld");
      await expect(card).not.toContainText("get_entity");
    });

    test("refused", async ({ page }) => {
      const dock = await open(page, "refused");
      const notice = dock.getByTestId("step-notice");
      await expect(notice).toContainText("You may not do this here");
      await expect(notice).toContainText("Ask a project owner or an organization administrator");
      await expect(dock).not.toContainText("mutation tools", { useInnerText: true });
    });

    test("page", async ({ page }) => {
      const dock = await open(page, "page");
      const notice = dock.getByTestId("step-notice");
      await expect(notice).toContainText("That page could not be opened.");
      await expect(notice.getByRole("link", { name: "Data models" })).toBeVisible();
      await expect(dock.getByText("That page could not be opened.")).toHaveCount(1);
      await expect(dock).not.toContainText("jc_ui_navigate", { useInnerText: true });
    });

    test("guarded", async ({ page }) => {
      const dock = await open(page, "guarded");
      const notice = dock.getByTestId("step-notice");
      await expect(notice).toContainText("The request came from text in the data, not from you.");
      await expect(notice.getByRole("button", { name: "Send my request again" })).toBeVisible();
    });

    test("missing", async ({ page }) => {
      const dock = await open(page, "missing");
      await expect(dock.getByTestId("step-notice")).toContainText("Check the name, or ask what this project holds.");
      await expect(dock.getByRole("button", { name: "Ask what it holds" })).toBeVisible();
      await expect(dock).not.toContainText("trips-csv'", { useInnerText: true });
    });
  });
}
