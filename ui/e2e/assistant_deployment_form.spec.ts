import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

// T-4282 (T-4281 items 8, 9): the deployment form picks the region's and the city's pages and is
// laid out in capabilities, at a phone's and a desktop's width. `vite preview` has no Portal API
// behind it, so the API is answered in the browser.

const PROJECT = "bbsk";

const source = (project: string, name: string, title: string, visibility: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "KnowledgeSource",
  metadata: { name, namespace: project, title: { en: title, sk: title } },
  spec: { source: "website", startUrls: [`https://www.${project}.sk/`], visibility },
});

const SOURCES: Record<string, unknown[]> = {
  bbsk: [source("bbsk", "web", "Region pages", "public"), source("bbsk", "intranet", "Region intranet", "internal")],
  banskabystrica: [source("banskabystrica", "web", "City pages", "public")],
};

type Proposal = { spec?: { sources?: string[]; select?: unknown } };

// T-4693 (AG-121): what the resolver would answer, a data-only assistant's Endpoints or a RAG-only
// assistant's sources, from what the form sends.
function contextOf(proposal: Proposal) {
  const spec = proposal.spec ?? {};
  const transport = [{ code: "TRAN", label: "Transport" }];
  return {
    publicId: "bbsk-doprava",
    channel: "public",
    title: null,
    languages: ["sk", "en"],
    categories: spec.select ? transport : [],
    limits: { perClientPerHour: null, perClientPerDay: null, messagesPerConversation: null, questionChars: null },
    endpoints: spec.select
      ? [{ slug: "aaaaaaaaaaaaaaaaaaaaaaaaaa", project: PROJECT, name: "roads", title: "Regional roads", audience: "public", themes: transport, types: ["Road"], tools: [], timeoutSeconds: 20 }]
      : [],
    omitted: [],
    sources: (spec.sources ?? []).map((name) => ({ project: PROJECT, name, kind: "website", visibility: "public", sites: ["www.bbsk.sk"], categories: [] })),
    problems: [],
  };
}

async function stubApi(page: Page): Promise<void> {
  await page.route(
    (url) => url.pathname.startsWith("/api/"),
    async (route) => {
      const path = new URL(route.request().url()).pathname;
      const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
      const list = (items: unknown[]) => json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
      if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "jana", name: "Jana Kováčová", roles: ["portal-editor"] });
      if (path.endsWith("/permissions/me")) return json({ project: PROJECT, bootstrap: true, grants: [] });
      if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: PROJECT }, { name: "banskabystrica" }] });
      if (path.endsWith("/events")) return route.fulfill({ status: 200, contentType: "text/event-stream", body: "" });
      const sources = /^\/api\/v1\/projects\/([^/]+)\/knowledgesources$/.exec(path);
      if (sources) return list(SOURCES[sources[1]] ?? []);
      if (path.endsWith("/knowledge/deployments/context")) return json(contextOf(route.request().postDataJSON() as Proposal));
      return list([]);
    },
  );
}

for (const width of [375, 1440]) {
  test(`at ${width} px the form picks both projects' pages and keeps internal ones off a public channel`, async ({ page }) => {
    await page.setViewportSize({ width, height: width < 768 ? 812 : 1000 });
    await stubApi(page);
    await page.goto(`/projects/${PROJECT}/assistantdeployments/new?lang=en`);
    const picker = page.getByTestId("knowledge-sources");
    await expect(picker.getByRole("checkbox", { name: /City pages/ })).toBeVisible();
    await page.getByLabel(/^Channel/).selectOption({ label: "Public" });
    await expect(picker.getByRole("checkbox", { name: /Region intranet/ })).toBeDisabled();
    await picker.getByRole("checkbox", { name: /Region pages/ }).check();
    await picker.getByRole("checkbox", { name: /City pages/ }).check();
    // The widget, its languages and its sites are filled by default, so their groups fold (T-4824).
    for (const fold of ["Widget", "Language", "Where it may appear"]) {
      await page.getByRole("group", { name: fold, exact: true }).locator("summary").click();
    }
    await page.getByLabel(/^Answers from/).selectOption({ label: "Knowledge only" });
    // A suggested question per language, as a steward writes the chips.
    const chips = page.getByRole("group", { name: /^Suggested questions/ });
    await chips.getByRole("button", { name: "Add a language" }).click();
    await chips.getByLabel("Language").fill("en");
    await chips.getByLabel("Language").blur();
    // T-4311: an entry's question and a site, so the judge sees a filled entry of each kind.
    await chips.getByRole("group", { name: "en" }).getByRole("button", { name: "Add" }).click();
    await chips.getByRole("group", { name: "en" }).getByRole("textbox").first().fill("When is the waste collected?");
    const sites = page.getByRole("group", { name: /^Allowed sites/ });
    await sites.getByLabel(/^Allowed sites 1/).fill("https://www.bbsk.sk");
    for (const group of ["Knowledge", "Live data", "How it answers", "Where it may appear"]) {
      await expect(page.getByRole("group", { name: group, exact: true })).toBeVisible();
    }
    const sideways = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(sideways, "the page scrolls sideways").toBeLessThanOrEqual(0);
    // The page as a visitor scrolls it, a screen at a time: a full-page shot paints a sticky header
    // and the assistant's bubble once, over whatever sits there.
    const height = await page.evaluate(() => document.documentElement.scrollHeight);
    const screen = page.viewportSize()?.height ?? 800;
    for (let y = 0, at = 0; y < height; y += screen - 120, at++) {
      await page.evaluate((top) => window.scrollTo(0, top), y);
      await page.screenshot({ path: `${process.env.SHOTS_DIR ?? "test-results"}/t4282-form-${width}-${String(at).padStart(2, "0")}.png` });
    }
  });
}

test("a RAG-only assistant previews its sources and no Endpoint", async ({ page }) => {
  await stubApi(page);
  await page.goto(`/projects/${PROJECT}/assistantdeployments/new?lang=en`);
  await page.getByTestId("knowledge-sources").getByRole("checkbox", { name: /Region pages/ }).check();
  const preview = page.getByRole("region", { name: "What the assistant will know" });
  // The source's own answer, not the first one before the box was ticked.
  await expect(preview.getByRole("list", { name: /sources/i })).toContainText("web");
  await expect(preview.getByRole("table")).toHaveCount(0);
});

test("a data-only assistant previews the Endpoints its selector reaches", async ({ page }) => {
  await stubApi(page);
  await page.route(
    (url) => url.pathname.endsWith("/knowledge/deployments/context"),
    (route) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(contextOf({ spec: { select: {} } })) }),
  );
  await page.goto(`/projects/${PROJECT}/assistantdeployments/new?lang=en`);
  const preview = page.getByRole("region", { name: "What the assistant will know" });
  await expect(preview.getByRole("cell", { name: "Regional roads" })).toBeVisible();
  await expect(preview.getByText("Transport").first()).toBeVisible();
  await page.screenshot({ path: `${process.env.SHOTS_DIR ?? "test-results"}/t4693-preview-data.png`, fullPage: false });
});
