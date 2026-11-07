import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";

// T-3057: the knowledge assistant's sources in a real browser, the API answered in the browser.
// What the list shows of `/knowledge/sources`, the source page's tree opened a level at a time,
// a branch left out of answers with what it removed, and what a viewer meets.

const IDENTITY = { subject: "b7c1e0f4", username: "eva.steward", name: "Eva Steward", email: "eva@hel.fi", roles: [] };

const grants = (verbs: string[]) => ({
  project: "helsinki",
  bootstrap: false,
  grants: [
    {
      role: verbs.includes("propose") ? "org-admin" : "viewer",
      binding: "helsinki-people",
      scope: "project:helsinki",
      rule: { kinds: ["KnowledgeSource", "AssistantDeployment"], verbs },
    },
  ],
});

const SOURCES = {
  items: [
    {
      source: "hel-web",
      type: "website",
      state: "crawled",
      startUrls: ["https://www.hel.fi/"],
      ckanInstanceRef: null,
      schedule: "0 3 * * *",
      visibility: "public",
      lastCrawl: "2026-10-06T03:00:00Z",
      pages: 40,
      pagesIncluded: 38,
      documents: 3,
      passages: 300,
      embedded: 300,
      job: { state: "done", attempts: 1, error: null },
    },
    { source: "hel-data", type: "ckan", state: "not-crawled", startUrls: [], ckanInstanceRef: "hel-fi", schedule: null, visibility: "public" },
  ],
};

const page = (id: number, url: string, extra: Record<string, unknown> = {}) => ({
  id, url, depth: 0, parentId: null, status: "fetched", included: true, excludedBy: null, language: "fi",
  fetchedAt: "2026-10-06T03:01:00Z", children: 0, documents: 0, passages: 5, ...extra,
});

// T-3058: one answer as jc-assistant streams it, with a passage and a tool cited.
const ANSWER = [
  'event: conversation\ndata: {"id":"6f1c0e9e-3b1a-4d7e-9b51-2c4f8f2a7c11"}',
  'event: tool\ndata: {"name":"query_entities","endpoint":"helsinki-weather","status":"done"}',
  'event: answer\ndata: {"text":"The roads are icy at Kamppi [1], -3 °C [2]."}',
  'event: citations\ndata: [{"n":1,"url":"https://www.hel.fi/en/roads"},{"n":2,"tool":"query_entities","endpoint":"helsinki-weather"}]',
  'event: done\ndata: {"tokens":700}',
].join("\n\n") + "\n\n";

const CONVERSATION = "6f1c0e9e-3b1a-4d7e-9b51-2c4f8f2a7c11";
const CONVERSATIONS = {
  items: [{ id: CONVERSATION, deployment: "staff", origin: null, startedAt: "2026-10-18T10:00:00Z", lastAt: "2026-10-18T10:01:00Z",
    firstQuestion: "Are the roads icy?", turns: 1, routes: ["data"], totalMs: 2400, errors: 0, timeouts: 0, costUsd: 0.0046,
    uncosted: 0, thumbsUp: 0, thumbsDown: 0 }],
  nextCursor: null,
};
const DETAIL = {
  id: CONVERSATION, deployment: "staff", origin: null,
  turns: [{ turn: 1, at: "2026-10-18T10:01:00Z", question: "Are the roads icy?", answer: "The roads are icy at Kamppi.", route: "data",
    firstTokenMs: 800, totalMs: 2400, error: null, timedOut: false, costUsd: 0.0046, uncosted: 0, verdict: null,
    steps: [{ kind: "tool", name: "query_entities", endpoint: "helsinki-weather", input: "{}", output: "[]", outputBytes: 2,
      status: "ok", startMs: 900, durationMs: 120, error: null, stage: null, model: null, provider: null, tokensIn: null,
      tokensOut: null, reasoningTokens: null, costUsd: null }] }],
};

async function stubApi(p: Page, verbs: string[] = ["read", "propose"]): Promise<{ writes: { path: string; body: unknown }[] }> {
  const writes: { path: string; body: unknown }[] = [];
  await p.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: status >= 400 ? "application/problem+json" : "application/json", body: JSON.stringify(body) });
    const base = "/api/v1/projects/helsinki/knowledge";
    if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
    if (url.pathname.endsWith("/permissions/me")) return json(grants(verbs));
    if (request.method() !== "GET") {
      writes.push({ path: url.pathname, body: request.postDataJSON() as unknown });
      if (url.pathname.endsWith("/chat")) {
        return route.fulfill({ status: 200, contentType: "text/event-stream", body: ANSWER });
      }
      return json({ pages: 2, documents: 0, passagesRemoved: 12 });
    }
    if (url.pathname === "/api/v1/projects/helsinki/assistantdeployments") {
      return json({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "List",
        items: [{ apiVersion: "joinedcontext.com/v1alpha1", kind: "AssistantDeployment", metadata: { name: "staff", namespace: "helsinki" }, spec: { publicId: "helsinki-staff", channel: "internal", sources: ["hel-web"], connectors: [{ endpoint: "helsinki-weather", tools: ["query_entities"] }] } }],
      });
    }
    if (url.pathname === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [{ name: "helsinki" }] });
    if (url.pathname === `${base}/sources`) return json(SOURCES);
    if (url.pathname === `${base}/sources/hel-web/pages`) {
      return url.searchParams.get("parent") === "1"
        ? json({ items: [page(2, "https://www.hel.fi/en/culture", { depth: 1, parentId: 1 })] })
        : json({ items: [page(1, "https://www.hel.fi/", { children: 1 })] });
    }
    if (url.pathname === `${base}/sources/hel-web/documents`) return json({ items: [] });
    // T-4670: one stored conversation, a data turn with a model call and a tool call.
    if (url.pathname === `${base}/conversations`) return json(CONVERSATIONS);
    if (url.pathname === `${base}/conversations/${CONVERSATION}`) return json(DETAIL);
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  return { writes };
}

test.describe("the knowledge sources", () => {
  test("the assistant's administrator opens the stored conversations from the page and reads one with its steps (T-4670)", async ({ page: p }) => {
    await stubApi(p, ["read", "propose", "approve"]);
    await p.goto("/projects/helsinki/knowledge?lang=en");
    await p.getByRole("link", { name: "Conversations" }).click();
    await expect(p).toHaveURL(/\/projects\/helsinki\/knowledge\/conversations/);
    await p.getByRole("link", { name: "Are the roads icy?" }).click();
    await expect(p.getByText("The roads are icy at Kamppi.")).toBeVisible();
    await expect(p.getByText(/helsinki-weather/)).toBeVisible();
    expect(await axeViolations(p)).toEqual([]);
  });

  test("a steward without approve is offered no conversations link", async ({ page: p }) => {
    await stubApi(p);
    await p.goto("/projects/helsinki/knowledge?lang=en");
    await expect(p.getByRole("heading", { level: 1, name: "Knowledge sources" })).toBeVisible();
    await expect(p.getByRole("link", { name: "Conversations" })).toHaveCount(0);
    await p.goto("/projects/helsinki/knowledge/conversations?lang=en");
    await expect(p.getByText("Only the assistant's administrator")).toBeVisible();
    await expect(p.getByRole("table")).toHaveCount(0);
  });

  // T-5833: the walker's viewer pressed "Add a source" on a slow Portal before its rights were read.
  test("a viewer is offered no add while the rights are read, then told why", async ({ page: p }) => {
    await stubApi(p, ["read"]);
    let release: () => void = () => undefined;
    const held = new Promise<void>((resolve) => (release = resolve));
    await p.route("**/api/v1/projects/helsinki/permissions/me", async (route) => {
      await held;
      await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(grants(["read"])) });
    });
    for (const width of [1440, 400]) {
      await p.setViewportSize({ width, height: 900 });
      if (width === 1440) await p.goto("/projects/helsinki/knowledge?lang=en");
      const add = p.getByRole("button", { name: "Add a source" }).first();
      await expect(add).toHaveAttribute("aria-disabled", "true");
      await expect(add).toHaveAccessibleDescription("Checking your access…");
      await p.screenshot({ path: test.info().outputPath(`knowledge-viewer-checking-${width}.png`), fullPage: true });
    }
    await p.getByRole("button", { name: "Add a source" }).first().click({ force: true });
    await expect(p).toHaveURL(/\/projects\/helsinki\/knowledge(\?|$)/);
    release();
    for (const width of [1440, 400]) {
      await p.setViewportSize({ width, height: 900 });
      const add = p.getByRole("button", { name: "Add a source" }).first();
      await expect(add).toHaveAccessibleDescription(/does not permit 'propose' on 'KnowledgeSource'/);
      await p.screenshot({ path: test.info().outputPath(`knowledge-viewer-refused-${width}.png`), fullPage: true });
    }
  });

  test("lists each source with what the assistant holds, and a source not crawled yet says so", async ({ page: p }) => {
    await stubApi(p);
    await p.goto("/projects/helsinki/knowledge?lang=en");
    await expect(p.getByRole("heading", { level: 1, name: "Knowledge sources" })).toBeVisible();
    const table = p.getByRole("table", { name: "Knowledge sources" });
    await expect(table.getByRole("row", { name: /hel-web/ }).getByText("38 of 40")).toBeVisible();
    await expect(table.getByRole("row", { name: /hel-data/ }).getByText("Not crawled yet")).toBeVisible();
    expect(await axeViolations(p)).toEqual([]);
  });

  // T-5328: the live walk found no source row opening its page, and the refused conversations page
  // without the line under its title.
  test("a click on any source row opens its page, a source not crawled yet too; the refused conversations page says what it is for", async ({ page: p }) => {
    await stubApi(p);
    await p.goto("/projects/helsinki/knowledge?lang=en");
    const table = p.getByRole("table", { name: "Knowledge sources" });
    await expect(table.getByRole("row", { name: /hel-data/ })).toBeVisible();
    await expect(table.locator("tbody tr:not(:has(a[data-row-link]))")).toHaveCount(0);
    await keep(p, "knowledge-sources");
    await table.getByRole("row", { name: /hel-data/ }).getByText("Not crawled yet").click();
    await expect(p).toHaveURL(/\/projects\/helsinki\/knowledge\/hel-data$/);
    await expect(p.getByRole("heading", { level: 1 })).toContainText("hel-data");
    await keep(p, "knowledge-source-not-crawled");
    await p.goto("/projects/helsinki/knowledge/conversations?lang=en");
    await expect(p.getByText("Only the assistant's administrator")).toBeVisible();
    await expect(p.locator("h1 + p")).toContainText("What visitors asked the project's assistants");
    await keep(p, "conversations-refused");
    expect(await axeViolations(p)).toEqual([]);
  });

  // T-4265 (photo audit T-4160 #23): at 375 px both tables ran past their cards, the crawl time one
  // letter a line and the row's Ask off the screen. A phone keeps the name, the state and the actions.
  test("both tables fit a phone in Slovak, their actions on the screen", async ({ page: p }) => {
    await p.setViewportSize({ width: 375, height: 812 });
    await stubApi(p);
    await p.goto("/projects/helsinki/knowledge?lang=sk");
    for (const name of ["Zdroje znalostí", "Asistenti"]) {
      const frame = p.getByRole("group", { name });
      await expect(frame).toBeVisible();
      const wider = await frame.evaluate((el) => el.scrollWidth - el.clientWidth);
      expect(wider, `${name} is wider than its frame`).toBeLessThanOrEqual(1);
    }
    const sideways = await p.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(sideways, "the page scrolls sideways").toBeLessThanOrEqual(0);
  });

  test("opens a branch of the tree and leaves it out of answers with everything below it", async ({ page: p }) => {
    const { writes } = await stubApi(p);
    await p.goto("/projects/helsinki/knowledge/hel-web?lang=en");
    await p.getByRole("button", { name: "Show the 1 pages below https://www.hel.fi/" }).click();
    await expect(p.getByRole("link", { name: /^https:\/\/www\.hel\.fi\/en\/culture/ })).toBeVisible();
    await p.getByRole("checkbox", { name: "Select https://www.hel.fi/", exact: true }).check();
    await p.getByRole("button", { name: "Leave out of answers" }).click();
    await expect(p.getByText("Left out: 2 pages and 0 documents; 12 passages removed.")).toBeVisible();
    expect(writes).toEqual([
      { path: "/api/v1/projects/helsinki/knowledge/sources/hel-web/inclusion", body: { pages: [1], documents: [], subtree: true, included: false } },
    ]);
    expect(await axeViolations(p)).toEqual([]);
  });

  test("Add an assistant opens the deployment form at its own address, with its fields labelled and no axe violations", async ({ page: p }) => {
    await stubApi(p);
    await p.goto("/projects/helsinki/knowledge?lang=en");
    await p.getByRole("button", { name: "Add an assistant" }).click();
    await expect(p).toHaveURL(/\/projects\/helsinki\/assistantdeployments\/new/);
    await expect(p.getByLabel("Public id")).toBeVisible();
    await expect(p.getByLabel("Channel")).toBeVisible();
    expect(await axeViolations(p)).toEqual([]);
  });

  test("a viewer finds leaving out disabled with the reason, and pressing it sends nothing", async ({ page: p }) => {
    const { writes } = await stubApi(p, ["read"]);
    await p.goto("/projects/helsinki/knowledge/hel-web?lang=en");
    await p.getByRole("checkbox", { name: "Select https://www.hel.fi/", exact: true }).check();
    const exclude = p.getByRole("button", { name: "Leave out of answers" });
    await expect(exclude).toHaveAttribute("aria-disabled", "true");
    await expect(exclude).toHaveAccessibleDescription("Disabled: your role does not permit 'propose' on 'KnowledgeSource' in this project");
    await exclude.click({ force: true });
    expect(writes).toEqual([]);
  });

  test("asks a staff assistant by keyboard and reads its answer with the sources, with no axe violations (T-3058)", async ({ page: p }) => {
    const { writes } = await stubApi(p);
    await p.goto("/projects/helsinki/knowledge?lang=en");
    await p.getByRole("button", { name: "Ask staff" }).click();
    const dialog = p.getByRole("dialog", { name: "Ask staff" });
    await expect(dialog.getByText(/answers you as yourself/)).toBeVisible();
    await dialog.getByRole("textbox", { name: "Your question" }).fill("Are the roads icy?");
    await dialog.getByRole("textbox", { name: "Your question" }).press("Enter");
    // The answer's markers are links to its sources since T-3325, not the text "[1]".
    await expect(dialog.getByText(/The roads are icy at Kamppi/)).toBeVisible();
    await expect(dialog.getByRole("link", { name: "Source 1" }).first()).toBeVisible();
    await expect(dialog.getByRole("list", { name: "Sources" }).getByRole("link", { name: /hel\.fi\/en\/roads/ })).toBeVisible();
    await expect(dialog.getByRole("textbox", { name: "Your question" })).toBeFocused();
    expect(writes).toEqual([
      { path: "/api/v1/projects/helsinki/knowledge/deployments/staff/chat", body: { message: "Are the roads icy?", history: [], connectors: ["helsinki-weather"] } },
    ]);
    await expect(p.getByRole("button", { name: "Embed staff" })).toHaveCount(0);
    expect(await axeViolations(p)).toEqual([]);
  });
});

// T-4869: an upload source as journey 41 met it on dev, after the fixes: its state in words, its
// file on a phone, a search hit by file and page, and an answer that cites the file. With
// JC_EVIDENCE_DIR set, each screen is kept at 1440 and 400 px.
const FILES_SOURCE = {
  source: "hel-files", type: "upload", state: "crawled", startUrls: [], ckanInstanceRef: null, schedule: null,
  visibility: "internal", pages: 0, pagesIncluded: 0, documents: 1, passages: 1, embedded: 1, pending: 0,
};
const UPLOAD_ANSWER = [
  `event: conversation\ndata: {"id":"${CONVERSATION}"}`,
  'event: answer\ndata: {"text":"The committee meets on the third Thursday of each month at 18:40 in the Liljankukka pavilion [1]."}',
  'event: citations\ndata: [{"n":1,"url":"upload:hel-files/kumpula-garden-rules.pdf#page=1"}]',
  'event: done\ndata: {"tokens":400}',
].join("\n\n") + "\n\n";

async function keep(p: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  for (const width of [1440, 400]) {
    await p.setViewportSize({ width, height: 900 });
    await p.screenshot({ path: join(dir, `${name}-${width}.png`), fullPage: true });
  }
  await p.setViewportSize({ width: 1440, height: 900 });
}

test.describe("an upload source (T-4869)", () => {
  test("says its state in words, names its file and page, and an answer cites the file", async ({ page: p }) => {
    await stubApi(p);
    // A RegExp, not a glob: the route gate reads a glob that opens with two stars and a slash as a comment end.
    await p.route(/\/api\/v1\/projects\/helsinki\//, async (route) => {
      const url = new URL(route.request().url());
      const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
      const base = "/api/v1/projects/helsinki/knowledge";
      if (url.pathname === `${base}/sources`) return json({ items: [...SOURCES.items, FILES_SOURCE] });
      if (url.pathname === `${base}/sources/hel-files/files`) {
        return json({ items: [{ id: 1, name: "kumpula-garden-rules.pdf", mime: "application/pdf", bytes: 1362, passages: 1, status: "fetched" }] });
      }
      if (url.pathname === `${base}/sources/hel-files/search`) {
        return json({ hits: [{ title: null, url: "upload:hel-files/kumpula-garden-rules.pdf#page=1", publisher: null, description: null,
          published: null, modified: null, language: "en", text: "The committee meets on the third Thursday of each month at 18:40.", score: 0.02 }] });
      }
      if (url.pathname.endsWith("/deployments/staff/chat")) return route.fulfill({ status: 200, contentType: "text/event-stream", body: UPLOAD_ANSWER });
      return route.fallback();
    });
    await p.goto("/projects/helsinki/knowledge?lang=en");
    const row = p.getByRole("table", { name: "Knowledge sources" }).getByRole("row").filter({ hasText: "hel-files" });
    await expect(row).toContainText("Read");
    await expect(row).not.toContainText("Crawled");
    await keep(p, "after-list");

    await row.getByRole("link", { name: "hel-files" }).click();
    await expect(p.getByRole("table", { name: "Files" })).toContainText("kumpula-garden-rules.pdf");
    await p.getByRole("searchbox", { name: "Question or words" }).fill("Liljankukka");
    await p.getByRole("button", { name: "Try the search" }).click();
    const hits = p.getByRole("list", { name: /Hits for/ });
    await expect(hits).toContainText("kumpula-garden-rules.pdf, page 1");
    await expect(hits).not.toContainText("upload:");
    await p.setViewportSize({ width: 400, height: 900 });
    // At a phone's width Remove stays inside the screen, beside the file's name (UI-27).
    const remove = p.getByRole("button", { name: "Remove kumpula-garden-rules.pdf" });
    await remove.scrollIntoViewIfNeeded();
    const box = await remove.boundingBox();
    expect((box?.x ?? 999) + (box?.width ?? 0)).toBeLessThanOrEqual(400);
    await p.setViewportSize({ width: 1440, height: 900 });
    await keep(p, "after-source");

    await p.goto("/projects/helsinki/knowledge?lang=en");
    await p.getByRole("button", { name: "Ask staff" }).click();
    const dialog = p.getByRole("dialog", { name: "Ask staff" });
    await dialog.getByRole("textbox", { name: "Your question" }).fill("When does the garden committee meet?");
    await dialog.getByRole("textbox", { name: "Your question" }).press("Enter");
    await expect(dialog.getByRole("list", { name: "Sources" })).toContainText("kumpula-garden-rules.pdf");
    await keep(p, "after-answer");
  });
});
