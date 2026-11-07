/**
 * T-3057 — the knowledge sources on dev (API/01 §34): what the page shows is what
 * `/knowledge/sources` answers the same person. Where dev runs the assistant the list carries the
 * declared sources and a crawled one opens its page tree; where it does not yet, the page says
 * the sources could not be read with the Portal's own sentence, never that there are none.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { STEWARD, VIEWER, csrf, signIn } from "./portal";

test.setTimeout(180_000);

const PROJECT = "helsinki";

interface Sources {
  items: { source: string; state: string }[];
}

test("the knowledge sources and one source's page agree with what the API answers", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/knowledge?lang=en`);
  try {
    await expect(page.getByRole("heading", { level: 1, name: "Knowledge sources" })).toBeVisible({ timeout: 60_000 });
    const answer = await page.request.get(`/api/v1/projects/${PROJECT}/knowledge/sources`);
    if (!answer.ok()) {
      const problem = (await answer.json()) as { detail?: string };
      await expect(page.getByRole("alert")).toContainText(`The sources could not be read: ${problem.detail ?? ""}`);
      await expect(page.getByText("This project has no knowledge source")).toHaveCount(0);
      return;
    }
    const sources = (await answer.json()) as Sources;
    if (sources.items.length === 0) {
      await expect(page.getByText("This project has no knowledge source")).toBeVisible();
      return;
    }
    const table = page.getByRole("table", { name: "Knowledge sources" });
    for (const item of sources.items) {
      await expect(table.getByRole("row", { name: new RegExp(item.source) })).toBeVisible();
    }
    const crawled = sources.items.find((item) => item.state === "crawled");
    const opened = crawled?.source ?? sources.items[0].source;
    await page.goto(`/projects/${PROJECT}/knowledge/${opened}?lang=en`);
    await expect(page.getByRole("heading", { level: 1, name: `Source ${opened}` })).toBeVisible({ timeout: 60_000 });
    if (crawled) {
      await expect(page.getByRole("checkbox").first()).toBeVisible({ timeout: 60_000 });
    } else {
      await expect(page.getByRole("alert")).toContainText("has not been crawled");
    }
  } finally {
    await context.close();
  }
});

// T-3057 (reopened): the three writes of the knowledge workflow, each leaving dev as it found it.

async function post(page: Page, token: string, path: string, data: unknown) {
  return page.request.post(path, { headers: { "x-csrf-token": token }, data });
}

test("taking back a page already in the answers changes nothing, and a viewer may not leave one out", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/knowledge?lang=en`);
  const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/knowledge?lang=en`);
  try {
    const sources = await steward.page.request.get(`/api/v1/projects/${PROJECT}/knowledge/sources`);
    const listed = sources.ok() ? ((await sources.json()) as Sources).items : [];
    const crawled = listed.find((item) => item.state === "crawled")?.source ?? "web";
    const refused = await post(viewer.page, await csrf(viewer.context), `/api/v1/projects/${PROJECT}/knowledge/sources/${crawled}/inclusion`, {
      pages: [1],
      included: false,
    });
    // 403 where the viewer reads the sources, 404 where the seed grants them nothing: refused either way.
    expect([403, 404]).toContain(refused.status());
    if (!sources.ok()) {
      // dev runs no assistant yet: the steward meets the Portal's sentence, and nothing was asked.
      expect(sources.status()).toBe(503);
      return;
    }
    const roots = await steward.page.request.get(`/api/v1/projects/${PROJECT}/knowledge/sources/${crawled}/pages`);
    const page = (((await roots.json()) as { items?: { id: number; included: boolean }[] }).items ?? []).find((p) => p.included);
    if (!page) return;
    const answer = await post(steward.page, await csrf(steward.context), `/api/v1/projects/${PROJECT}/knowledge/sources/${crawled}/inclusion`, {
      pages: [page.id],
      included: true,
    });
    expect(answer.status()).toBe(200);
    expect(await answer.json()).toMatchObject({ pages: 0, documents: 0, passagesRemoved: 0 });
  } finally {
    await steward.context.close();
    await viewer.context.close();
  }
});

test("a viewer may not queue a crawl, and a steward's crawl now is queued once", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/knowledge?lang=en`);
  const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/knowledge?lang=en`);
  try {
    const sources = await steward.page.request.get(`/api/v1/projects/${PROJECT}/knowledge/sources`);
    const listed = sources.ok() ? ((await sources.json()) as Sources).items : [];
    const source = listed.find((item) => item.state === "crawled")?.source ?? "web";
    const path = `/api/v1/projects/${PROJECT}/knowledge/sources/${source}/recrawl`;
    expect([403, 404]).toContain((await post(viewer.page, await csrf(viewer.context), path, {})).status());
    if (!sources.ok()) {
      expect(sources.status()).toBe(503);
      return;
    }
    // The crawl the schedule runs anyway, brought forward: a second ask while it waits is 409.
    const token = await csrf(steward.context);
    const first = await post(steward.page, token, path, {});
    expect([202, 409]).toContain(first.status());
    expect((await post(steward.page, token, path, {})).status()).toBe(409);
  } finally {
    await steward.context.close();
    await viewer.context.close();
  }
});

// T-4185: a file of a `source: upload`, uploaded, listed and removed again, so dev keeps what it had.
test("a viewer may not upload a file, and a steward's file is read, listed and removed", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/knowledge?lang=en`);
  const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/knowledge?lang=en`);
  try {
    const sources = await steward.page.request.get(`/api/v1/projects/${PROJECT}/knowledge/sources`);
    const listed = sources.ok() ? ((await sources.json()) as { items: { source: string; type: string }[] }).items : [];
    const source = listed.find((item) => item.type === "upload")?.source ?? "files";
    const files = `/api/v1/projects/${PROJECT}/knowledge/sources/${source}/files`;
    const name = `e2e-${Date.now()}.md`;
    const body = Buffer.from("# Odvoz odpadu\n\nKomunálny odpad vyvážame v utorok.\n");
    const send = (page: Page, token: string) =>
      page.request.post(`${files}?name=${encodeURIComponent(name)}`, {
        headers: { "x-csrf-token": token, "content-type": "application/octet-stream" },
        data: body,
      });
    expect([403, 404]).toContain((await send(viewer.page, await csrf(viewer.context))).status());
    if (!sources.ok() || !listed.some((item) => item.type === "upload")) return;
    const token = await csrf(steward.context);
    const added = await send(steward.page, token);
    expect(added.status()).toBe(201);
    const file = (await added.json()) as { id: number; name: string; passages: number };
    expect(file).toMatchObject({ name });
    expect(file.passages).toBeGreaterThan(0);
    const after = (await (await steward.page.request.get(files)).json()) as { items: { id: number }[] };
    expect(after.items.map((item) => item.id)).toContain(file.id);
    const removed = await steward.page.request.delete(`${files}/${file.id}`, { headers: { "x-csrf-token": token } });
    expect(removed.status()).toBe(204);
    const gone = (await (await steward.page.request.get(files)).json()) as { items: { id: number }[] };
    expect(gone.items.map((item) => item.id)).not.toContain(file.id);
  } finally {
    await steward.context.close();
    await viewer.context.close();
  }
});

test("a question outside the contract and an unknown assistant are refused before anything is spent", async ({ browser }) => {
  const { context, page } = await signIn(browser, VIEWER, `/projects/${PROJECT}/knowledge?lang=en`);
  try {
    const token = await csrf(context);
    const unknown = await post(page, token, `/api/v1/projects/${PROJECT}/knowledge/deployments/no-such-assistant/chat`, { message: "Hello" });
    expect(unknown.status()).toBe(404);
    const deployments = await page.request.get(`/api/v1/projects/${PROJECT}/assistantdeployments`);
    const name = (((await deployments.json()) as { items?: { metadata: { name: string } }[] }).items ?? [])[0]?.metadata.name;
    if (!name) return;
    const outside = await post(page, token, `/api/v1/projects/${PROJECT}/knowledge/deployments/${name}/chat`, { message: "Hello", password: "x" });
    expect(outside.status()).toBeGreaterThanOrEqual(400);
    expect(outside.status()).toBeLessThan(500);
  } finally {
    await context.close();
  }
});

// T-4670: the stored conversations agree with the API for one who may read them, and a viewer is
// refused by the API and offered nothing on the page.
test("the stored conversations agree with the API, and a viewer reads none of them", async ({ browser }) => {
  const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/knowledge/conversations?lang=en`);
  try {
    await expect(viewer.page.getByRole("heading", { level: 1, name: "Conversations" })).toBeVisible({ timeout: 60_000 });
    expect((await viewer.page.request.get(`/api/v1/projects/${PROJECT}/knowledge/conversations`)).status()).toBe(403);
    await expect(viewer.page.getByRole("table")).toHaveCount(0);
  } finally {
    await viewer.context.close();
  }
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/knowledge/conversations?lang=en`);
  try {
    await expect(page.getByRole("heading", { level: 1, name: "Conversations" })).toBeVisible({ timeout: 60_000 });
    const answer = await page.request.get(`/api/v1/projects/${PROJECT}/knowledge/conversations?limit=50`);
    if (answer.status() === 403) {
      await expect(page.getByText("Only the assistant's administrator")).toBeVisible();
      return;
    }
    const list = (await answer.json()) as { items: { firstQuestion: string }[] };
    if (list.items.length === 0) {
      await expect(page.getByText("No stored conversations")).toBeVisible();
    } else {
      await expect(page.getByRole("table").getByRole("button", { name: list.items[0].firstQuestion })).toBeVisible();
    }
  } finally {
    await context.close();
  }
});

// T-4670: erasing a stored conversation, without erasing anything on dev: a viewer is refused,
// and an id that is no conversation's never reaches the assistant.
test("a viewer may not erase a stored conversation, and an id that is no conversation's is refused", async ({ browser }) => {
  const path = (id: string) => `/api/v1/projects/${PROJECT}/knowledge/conversations/${id}`;
  const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/knowledge/conversations?lang=en`);
  try {
    const token = await csrf(viewer.context);
    const refused = await viewer.page.request.delete(path("00000000-0000-4000-8000-000000000000"), { headers: { "x-csrf-token": token } });
    expect(refused.status()).toBe(403);
  } finally {
    await viewer.context.close();
  }
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/knowledge/conversations?lang=en`);
  try {
    const token = await csrf(context);
    const malformed = await page.request.delete(path("not-a-conversation"), { headers: { "x-csrf-token": token } });
    expect([400, 403]).toContain(malformed.status());
  } finally {
    await context.close();
  }
});

// T-4731: the kept answers of an assistant are forgotten only by an approver; on dev the purge
// costs one cold answer and nothing else, so the journey runs it.
test("a viewer may not make an assistant forget its kept answers, and an approver's purge says how many it forgot", async ({ browser }) => {
  const path = (name: string) => `/api/v1/projects/${PROJECT}/knowledge/deployments/${name}/answers`;
  const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/knowledge/conversations?lang=en`);
  try {
    const refused = await viewer.page.request.delete(path("public"), { headers: { "x-csrf-token": await csrf(viewer.context) } });
    expect(refused.status()).toBe(403);
  } finally {
    await viewer.context.close();
  }
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/knowledge/conversations?lang=en`);
  try {
    const token = await csrf(context);
    const unknown = await page.request.delete(path("no-such-assistant"), { headers: { "x-csrf-token": token } });
    if (unknown.status() === 403) return; // this steward is no approver on dev
    expect(unknown.status()).toBe(404);
    const listed = await page.request.get(`/api/v1/projects/${PROJECT}/assistantdeployments`);
    const names = listed.ok() ? ((await listed.json()) as { items: { metadata: { name: string } }[] }).items : [];
    if (names.length === 0) return;
    const purged = await page.request.delete(path(names[0].metadata.name), { headers: { "x-csrf-token": token } });
    expect(purged.status()).toBe(200);
    expect(typeof ((await purged.json()) as { forgotten: unknown }).forgotten).toBe("number");
  } finally {
    await context.close();
  }
});

// T-4693 (AG-121): the editor's preview resolves a manifest nobody saved, on a channel nobody signs
// in to public Endpoints alone, and changes nothing without `dryRun=All`.
test("a steward previews an unsaved assistant's Endpoints, public ones alone on a public channel", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/assistantdeployments?lang=en`);
  try {
    const manifest = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "AssistantDeployment",
      metadata: { name: "live-preview", namespace: PROJECT },
      spec: { publicId: "live-preview", channel: "public", select: {}, answer: { routes: "data" } },
    };
    const path = `/api/v1/projects/${PROJECT}/knowledge/deployments/context`;
    const token = await csrf(steward.context);
    expect((await post(steward.page, token, path, manifest)).status()).toBe(400);
    const answer = await post(steward.page, token, `${path}?dryRun=All&lang=en`, manifest);
    expect(answer.status()).toBe(200);
    const context = (await answer.json()) as { endpoints: { audience: string | null }[]; problems: string[] };
    expect(context.endpoints.every((endpoint) => endpoint.audience === "public")).toBe(true);
    // Nothing was saved: the name the preview read is no deployment of the project.
    expect((await steward.page.request.get(`/api/v1/projects/${PROJECT}/knowledge/deployments/live-preview/context`)).status()).toBe(404);
  } finally {
    await steward.context.close();
  }
});
