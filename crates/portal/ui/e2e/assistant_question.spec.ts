import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// The assistant asks which endpoints to read, and the person answers by clicking two of them
// (UI-73, UI-74, T-1452): the built bundle, the API answered from the browser, and the run's
// stream fulfilled with the one question.

const PROJECT = "helsinki";
const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};
const RUN = {
  id: "01J8ZQ4T7K9M2N3P4Q5R6S7T8V",
  project: PROJECT,
  appName: "bikes-and-air",
  endpointName: "bikes",
  appClass: "fullstack",
  visibility: "project",
  prompt: "Build me a dashboard of bikes and air quality",
  status: "interviewing",
  steps: 2,
  tokensUsed: 1_200,
  createdBy: "jana.kovacova",
  createdAt: "2026-09-18T08:00:00Z",
};
const QUESTION = {
  seq: 1,
  questionId: "q-endpoints",
  pick: "endpoints",
  multiple: true,
  min: 1,
  schema: {
    type: "object",
    title: "Which endpoints should the app read?",
    properties: {
      answer: {
        type: "array",
        title: "Which endpoints should the app read?",
        items: {
          type: "string",
          oneOf: [
            { const: "bikes", title: "City bikes", description: "space helsinki · ngsi-ld, geojson · organization" },
            { const: "air", title: "Air quality", description: "space helsinki · csv · public" },
            { const: "noise", title: "Noise", description: "space helsinki · ngsi-ld · organization" },
          ],
        },
        uniqueItems: true,
        minItems: 1,
      },
    },
    required: ["answer"],
  },
};

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

async function stubApi(page: Page, answers: unknown[]): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (request.method() === "POST" && path.endsWith(`/agent-runs/${RUN.id}/answers`)) {
      answers.push(request.postDataJSON());
      return route.fulfill({ status: 204 });
    }
    if (request.method() !== "GET") {
      return json({ type: "about:blank", title: "Not stubbed", status: 404 }, 404);
    }
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ ...list([{ name: PROJECT }]), kind: "ProjectList" });
    if (path.endsWith(`/agent-runs/${RUN.id}/events`)) {
      return route.fulfill({
        status: 200,
        contentType: "text/event-stream",
        headers: { "cache-control": "no-cache" },
        body: `id: 1\nevent: question\ndata: ${JSON.stringify(QUESTION)}\n\n`,
      });
    }
    if (path.endsWith(`/agent-runs/${RUN.id}`)) return json(RUN);
    if (path.endsWith("/agent-runs")) return json(list([RUN]));
    if (/^\/api\/v1\/projects\/[a-z-]+\/[a-z-]+$/.test(path)) return json(list([]));
    return json({ type: "about:blank", title: "Not stubbed", status: 404 }, 404);
  });
}

test("the person answers the assistant's question by clicking two endpoints", async ({ page }) => {
  const answers: unknown[] = [];
  await stubApi(page, answers);
  await page.goto(`/projects/${PROJECT}/apps/${RUN.appName}?lang=en`, { waitUntil: "networkidle" });
  const bubble = page.getByRole("button", { name: "Open the assistant" });
  if (await bubble.count()) {
    await bubble.first().click();
  }

  const group = page.getByRole("group", { name: "Which endpoints should the app read?" });
  await expect(group).toBeVisible();
  await expect(group.getByText("space helsinki · csv · public")).toBeVisible();
  await expect(page.getByRole("button", { name: "Something else…" })).toHaveCount(0);

  await group.getByRole("button", { name: /City bikes/ }).click();
  await group.getByRole("button", { name: /Air quality/ }).click();
  await page.getByRole("button", { name: "Use these (2)" }).click();

  await expect.poll(() => answers).toEqual([{ questionId: "q-endpoints", answers: { answer: ["bikes", "air"] } }]);
  expect(await axeViolations(page)).toEqual([]);
});

for (const [label, width, height] of [["desktop-1440", 1440, 900], ["phone-400", 400, 860]] as const) {
  test(`an app-build frame on the stream shows the Build card (F-UI2-010, ${label})`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await stubApi(page, []);
    const build = { seq: 2, endpoints: ["bikes"], access: "read", visibility: "project", prompt: "A map of the city bikes" };
    await page.route(
      (url) => url.pathname.endsWith(`/agent-runs/${RUN.id}/events`),
      (route) =>
        route.fulfill({
          status: 200,
          contentType: "text/event-stream",
          headers: { "cache-control": "no-cache" },
          body: `id: 1\nevent: message\ndata: ${JSON.stringify({ seq: 1, text: "Here is what the app will do." })}\n\nid: 2\nevent: app-build\ndata: ${JSON.stringify(build)}\n\n`,
        }),
    );
    await page.route(
      (url) => url.pathname === `/api/v1/projects/${PROJECT}/endpoints`,
      (route) =>
        route.fulfill({
          contentType: "application/json",
          body: JSON.stringify(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "Endpoint", metadata: { name: "bikes", namespace: PROJECT, title: "City bikes" }, spec: { contextSpaceRef: "helsinki", slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa", audience: "organization" }, status: { phase: "Live" } }])),
        }),
    );
    await page.goto(`/projects/${PROJECT}/apps/${RUN.appName}?lang=en`, { waitUntil: "networkidle" });
    const bubble = page.getByRole("button", { name: "Open the assistant" });
    if (await bubble.count()) await bubble.first().click();
    await expect(page.getByText("Build a-map-of-the-city-bikes", { exact: false }).or(page.getByRole("heading", { name: /^Build / })).first()).toBeVisible();
    if (process.env.SHOTS_DIR) await page.screenshot({ path: `${process.env.SHOTS_DIR}/app-build-card-${label}.png`, fullPage: true });
  });
}


// T-6104 (owner walk): "cancel this" typed under a question closed nothing, and the Banská
// Bystrica air question from Helsinki found nothing although Helsinki references that city's
// Endpoint and the open-data catalogue holds the data. The whole conversation, at both widths.
for (const [label, width, height] of [["desktop-1440", 1440, 1100], ["phone-400", 400, 1400]] as const) {
  test(`a typed message withdraws the open question, and the search shows shared endpoints and open datasets (${label})`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await stubApi(page, []);
    const source = {
      seq: 2,
      questionId: "q-source",
      schema: {
        type: "object",
        title: "Where does the data come from?",
        properties: {
          answer: {
            type: "string",
            title: "Where does the data come from?",
            oneOf: [
              { const: "datasource", title: "A data source" },
              { const: "space", title: "A context space" },
            ],
          },
        },
        required: ["answer"],
      },
    };
    const found = {
      q: "airquality banska bystrica",
      items: [
        {
          kind: "Endpoint",
          name: "app-banskabystrica-ovzdusie",
          space: "banskabystrica-verejne",
          owner: "banskabystrica",
          title: "Kvalita ovzdušia v Banskej Bystrici",
          endpointSlug: "k7m2qz4tv6xh3n5jb2ryd3wcfa",
          matchReason: ["name", "shared with this project by banskabystrica"],
          access: { verdict: "allowed", reason: "audience organization: every signed-in member" },
          freshness: null,
        },
      ],
      datasets: [
        { title: "Kvalita ovzdušia", url: "https://opendata.dev.joinedcontext.com/dataset/public-air", publisher: "Mesto Banská Bystrica", formats: ["CSV", "GeoJSON"] },
      ],
    };
    const frames = [
      ["message", { seq: 1, text: "set up a pipeline for parking", sentBy: "demo.steward" }],
      ["question", source],
      ["message", { seq: 3, text: "cancel this", sentBy: "demo.steward" }],
      ["thought", { seq: 4, text: "Pipeline setup cancelled." }],
      ["message", { seq: 5, text: "what is the airquality right now in banska bystrica??", sentBy: "demo.steward" }],
      ["tool", { seq: 6, tool: "search_catalog", status: "ok", input: { q: "airquality banska bystrica" }, output: found }],
      ["thought", { seq: 7, text: "PM10 at Štefánikovo nábrežie is 18 µg/m³ (observed 20:00), read from app-banskabystrica-ovzdusie, shared with Helsinki by Banská Bystrica." }],
    ] as const;
    await page.route(
      (url) => url.pathname.endsWith(`/agent-runs/${RUN.id}/events`),
      (route) =>
        route.fulfill({
          status: 200,
          contentType: "text/event-stream",
          headers: { "cache-control": "no-cache" },
          body: frames.map(([kind, data]) => `id: ${String(data.seq)}\nevent: ${kind}\ndata: ${JSON.stringify(data)}\n\n`).join(""),
        }),
    );
    // A conversation, not a build: the dock draws what a search found only outside a build.
    await page.route(
      (url) => url.pathname.endsWith(`/agent-runs/${RUN.id}`),
      (route) =>
        route.fulfill({
          contentType: "application/json",
          body: JSON.stringify({ ...RUN, kind: "conversation", appName: "", prompt: "set up a pipeline for parking" }),
        }),
    );
    await page.goto(`/projects/${PROJECT}/apps/${RUN.appName}?lang=en`, { waitUntil: "networkidle" });
    const bubble = page.getByRole("button", { name: "Open the assistant" });
    if (await bubble.count()) await bubble.first().click();

    await expect(page.getByText("Pipeline setup cancelled.")).toBeVisible();
    await expect(page.getByRole("heading", { name: "The agent is asking" })).toHaveCount(0);
    await expect(page.getByText(/Question withdrawn/)).toBeVisible();
    await expect(page.getByText("shared by banskabystrica")).toBeVisible();
    const datasets = page.getByRole("region", { name: "Open data catalogue" });
    await expect(datasets.getByRole("link", { name: /Kvalita ovzdušia/ })).toBeVisible();
    if (process.env.SHOTS_DIR) await page.screenshot({ path: `${process.env.SHOTS_DIR}/t6104-conversation-${label}.png`, fullPage: true });
  });
}
