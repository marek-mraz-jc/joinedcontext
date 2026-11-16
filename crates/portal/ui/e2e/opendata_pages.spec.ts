import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { asOData, doorAsRest } from "../tests/odata";
import { doorsAsRest } from "./manifestReads";

// T-7176: the stub answers permissions/me by its REST address; the page asks its Portal door.
test.beforeEach(({ page }) => doorsAsRest(page, ["portal_permissions_me", "portal_project_list"]));

// Administration → Open data pages (T-7041): every page per language, the editor with its toolbar
// beside the preview at 1440 and as a tab at 400, publish as a Change, and the empty, error (422),
// refused (403) and pending (409) states. `vite preview` has no Portal API behind it, so the API
// is answered in the browser; the preview answer imitates the catalogue renderer's HTML.
// JC_SHOTS=<dir> saves a desktop 1440 and a phone 400 shot of each state.

const IDENTITY = { subject: "b7c1e0f4", username: "ada", name: "Ada Admin", email: "ada@bbsk.sk", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const BUILT_IN: Record<string, string> = {
  connector:
    "# AI konektor\n\nPripojte svojho asistenta k otvoreným údajom.\n\n## Claude\n\n- Otvorte nastavenia\n- Pridajte konektor",
  privacy: "# Ochrana súkromia\n\nAké údaje portál spracúva.",
  support: "# Podpora\n\nNapíšte nám.",
};
const OWN =
  "# Náš AI konektor\n\nBanskobystrický kraj pripája asistentov k **otvoreným údajom**.\n\n## Ako na to\n\n- Otvorte nastavenia asistenta\n- Vložte adresu konektora\n\nViac na [podpora](https://www.bbsk.sk/podpora).";

const change = (name: string, phase: string) => ({
  apiVersion: "joinedcontext.com/v1",
  kind: "ChangeProposal",
  metadata: { name, namespace: "org" },
  author: { name: "Jana Kováčová", email: "jana@bbsk.sk" },
  createdAt: "2026-11-14T09:12:00Z",
  decision: phase === "Merged" ? { at: "2026-11-14T10:02:00Z", by: "eva@bbsk.sk" } : null,
  status: { phase, lane: "yellow", plan: {} },
  summary: { key: "update", params: { kind: "Organization", name: "bbsk" } },
});

/** A few lines of the catalogue renderer's output, enough to look like the page. */
function render(markdown: string): { title?: string; html: string } {
  const lines = markdown.split("\n");
  const title = lines[0]?.startsWith("# ") ? lines.shift()?.slice(2) : undefined;
  const inline = (text: string) =>
    text
      .replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>")
      .replace(/\[(.+?)\]\((.+?)\)/g, '<a href="$2" rel="noopener nofollow">$1</a>');
  const html = lines
    .filter((line) => line.trim() !== "")
    .map((line) =>
      line.startsWith("## ")
        ? `<h2>${inline(line.slice(3))}</h2>`
        : line.startsWith("- ")
          ? `<ul><li>${inline(line.slice(2))}</li></ul>`
          : `<p>${inline(line)}</p>`,
    )
    .join("");
  return { title, html };
}

interface Stub {
  own?: boolean;
  list?: number;
  write?: { status: number; detail: string };
  history?: boolean;
}

async function stubApi(page: Page, stub: Stub = {}): Promise<void> {
  let published = false;
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    // A Portal door over OData (T-7100) is answered by its REST door's stub, wrapped as the service does.
    const door = await doorAsRest(
      new Request(request.url(), { method: request.method(), body: request.postData() ?? undefined }),
    );
    const path = new URL(door?.url ?? request.url()).pathname;
    const sent = door ? ((await door.clone().json().catch(() => undefined)) as unknown) : request.postDataJSON();
    const json = async (body: unknown, status = 200) => {
      const rest = new Response(JSON.stringify(body), { status });
      const answer = door ? await asOData(rest) : rest;
      return route.fulfill({
        status: answer.status,
        contentType: answer.status >= 400 && !door ? "application/problem+json" : "application/json",
        body: await answer.text(),
      });
    };
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: "doprava" }] });
    if (path.endsWith("/permissions/me")) {
      return json({
        project: "org",
        bootstrap: false,
        grants: [
          {
            role: "r",
            binding: "b",
            rule: { kinds: ["RoleBinding", "Organization"], verbs: ["read", "propose", "approve", "delete"] },
          },
        ],
      });
    }
    if (path === "/api/v1/organization/opendata/pages") {
      if (stub.list)
        return json(
          {
            type: "about:blank",
            title: "Forbidden",
            status: stub.list,
            detail:
              "editing the open data portal's pages is for the Organization's administrators, on the Administration page (UI-87)",
          },
          stub.list,
        );
      const pages = ["connector", "privacy", "support"].flatMap((name) =>
        ["sk", "en"].map((lang) => {
          const own = stub.own && name === "connector" && lang === "sk";
          return {
            page: name,
            lang,
            title: (own ? OWN : BUILT_IN[name]).split("\n")[0].slice(2),
            builtIn: BUILT_IN[name],
            ...(own ? { markdown: OWN } : {}),
          };
        }),
      );
      return json({ organization: "bbsk", languages: ["sk", "en"], maxBytes: 16384, pages });
    }
    if (path === "/api/v1/organization/opendata/pages/preview") {
      return json(render((sent as { markdown: string }).markdown));
    }
    if (path.startsWith("/api/v1/organization/opendata/pages/")) {
      if (stub.write) return json({ type: "about:blank", title: "Refused", ...stub.write }, stub.write.status);
      published = true;
      return json(change("chg-7041a0b1", "PendingApproval"), 202);
    }
    if (path === "/api/v1/projects/org/changes")
      return json({ kind: "ChangeList", items: published ? [change("chg-7041a0b1", "PendingApproval")] : [] });
    if (path === "/api/v1/projects/org/changes/chg-7041a0b1/manifests") {
      return json({
        items: [
          {
            kind: "Organization",
            metadata: { name: "bbsk" },
            spec: { appearance: { pages: [{ page: "connector", lang: "sk", markdown: "# Nový text" }] } },
          },
        ],
      });
    }
    if (path === "/api/v1/projects/org/changes/history") {
      return json({ kind: "ChangeList", items: stub.history ? [change("chg-00c0ffee", "Merged")] : [] });
    }
    if (path === "/api/v1/projects/org/changes/chg-00c0ffee") {
      return json({ ...change("chg-00c0ffee", "Merged"), planFields: [{ path: "spec.appearance.pages", to: [] }] });
    }
    return json(list([]));
  });
}

async function shot(page: Page, name: string, phoneStep?: (page: Page) => Promise<void>): Promise<void> {
  const dir = process.env.JC_SHOTS;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.screenshot({
    path: join(dir, `opendata-pages-${name}-desktop.png`),
    fullPage: true,
    animations: "disabled",
  });
  await page.setViewportSize({ width: 400, height: 900 });
  if (phoneStep) await phoneStep(page);
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.screenshot({
    path: join(dir, `opendata-pages-${name}-phone.png`),
    fullPage: true,
    animations: "disabled",
  });
  await page.setViewportSize({ width: 1440, height: 900 });
}

const editConnectorSk = (page: Page) =>
  page
    .getByTestId("opendata-pages-list")
    .getByRole("button", { name: /\(sk\)$/ })
    .first()
    .click();

test.describe("Administration → Open data pages", () => {
  test("every page shows the built-in text and the list says what to do", async ({ page }) => {
    await stubApi(page);
    await page.goto("/organization/opendata-pages?lang=en");
    await expect(page.getByText(/Every page shows the built-in text/)).toBeVisible();
    await expect(page.getByText("No page text was changed yet.")).toBeVisible();
    await shot(page, "empty");
  });

  test("an own text, its history, and the editor beside its preview", async ({ page }) => {
    await stubApi(page, { own: true, history: true });
    await page.goto("/organization/opendata-pages?lang=en");
    await expect(page.getByText("Your text")).toBeVisible();
    await expect(page.getByTestId("opendata-pages-history")).toContainText("Jana Kováčová");
    await shot(page, "filled");
    await editConnectorSk(page);
    await expect(page.locator("strong", { hasText: "otvoreným údajom" })).toBeVisible();
    await shot(page, "preview", async (phone) => {
      await phone.getByRole("tab", { name: "Preview" }).click();
      await expect(phone.locator("strong", { hasText: "otvoreným údajom" })).toBeVisible();
    });
  });

  test("publish proposes a change that waits for approval", async ({ page }) => {
    await stubApi(page);
    await page.goto("/organization/opendata-pages?lang=en");
    await editConnectorSk(page);
    const box = page.getByRole("textbox", { name: /Page text in Markdown/ });
    await box.fill("# Nový text\n\nKonektor kraja.");
    await page.getByRole("button", { name: "Bold" }).click();
    await expect(page.locator("strong", { hasText: "text" })).toBeVisible();
    await page.getByRole("button", { name: "Publish" }).click();
    await expect(page.getByText(/once an administrator approves the change/)).toBeVisible();
    await expect(page.getByText("chg-7041a0b1").first()).toBeVisible();
    await shot(page, "published");
  });

  test("a 422 marks the field with the server's words", async ({ page }) => {
    await stubApi(page, {
      write: {
        status: 422,
        detail: "spec.appearance.pages[0].markdown: the text holds a control character (U+0007) at line 3; remove it",
      },
    });
    await page.goto("/organization/opendata-pages?lang=en");
    await editConnectorSk(page);
    // Text pasted from a word processor, with a bell character the eye does not see.
    await page
      .getByRole("textbox", { name: /Page text in Markdown/ })
      .fill("# Podpora\n\nNapíšte nám\u0007 na podpora@bbsk.sk.");
    await expect(page.locator("article").getByText("Podpora", { exact: true })).toBeVisible();
    await page.getByRole("button", { name: "Publish" }).click();
    await expect(page.getByText(/holds a control character/)).toBeVisible();
    await shot(page, "error");
  });

  test("a text over the limit cannot be published and the counter says why", async ({ page }) => {
    await stubApi(page);
    await page.goto("/organization/opendata-pages?lang=en");
    await editConnectorSk(page);
    await page
      .getByRole("textbox", { name: /Page text in Markdown/ })
      .fill(`# Dlhý text\n\n${"Odsek o otvorených údajoch. ".repeat(615)}`);
    await expect(page.getByRole("button", { name: "Publish" })).toBeDisabled();
    await expect(page.getByText("18464 of 16384 bytes")).toBeVisible();
    await shot(page, "toolong");
  });

  test("a 409 says to decide the open change first", async ({ page }) => {
    await stubApi(page, { write: { status: 409, detail: "a Change on Organization bbsk is open: chg-11112222" } });
    await page.goto("/organization/opendata-pages?lang=en");
    await editConnectorSk(page);
    await page.getByRole("button", { name: "Publish" }).click();
    await expect(page.getByText(/Approve or reject it, then publish again/)).toBeVisible();
    await shot(page, "pending");
  });

  test("somebody who does not administer the Organization is told whose page it is", async ({ page }) => {
    await stubApi(page, { list: 403 });
    await page.goto("/organization/opendata-pages?lang=en");
    await expect(page.getByText(/Only the Organization's administrators edit/)).toBeVisible();
    await shot(page, "refused");
  });
});
