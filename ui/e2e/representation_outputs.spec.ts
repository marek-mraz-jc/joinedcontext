import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

// The Endpoint form's named calendar and feed outputs (T-5218, API/11 EP-159) and the Endpoint page's
// subscription links (T-4900, EP-125), against a stubbed API as models.spec.ts does: `vite preview` has
// no Portal behind it. Shots of every state at 1440 and 400 go to test-results/representation-outputs.

const IDENTITY = { subject: "b7c1e0f4", username: "jana", name: "Jana Kováčová", email: "jana@example.org", roles: ["portal-editor"] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const LINKML = [
  "id: https://example.org/models/events",
  "name: events",
  "classes:",
  "  Event:",
  "    slots: [name, startDate, endDate, description, address, url, category, free]",
  "slots:",
  "  name: { required: true, annotations: { ngsi_ld_kind: LanguageProperty } }",
  "  startDate: { range: datetime, required: true }",
  "  endDate: { range: datetime }",
  "  description: { range: string }",
  "  address: { range: string }",
  "  url: { range: uri }",
  "  category: { range: EventCategory }",
  "  free: { range: boolean }",
  "enums:",
  "  EventCategory:",
  "    permissible_values: { sport: {}, music: {} }",
  "",
].join("\n");
const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";
const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "podujatia", namespace: "bbsk", title: "Podujatia v kraji" },
  spec: {
    contextSpaceRef: "podujatia",
    slug: SLUG,
    audience: "public",
    enabledRepresentations: ["ngsi-ld", "mcp", "ical"],
    representations: { ical: { preset: "sdm-event", mapping: { LOCATION: { join: ["address", "description"], separator: ", " } } } },
  },
};
const REFUSAL =
  "Endpoint bbsk/podujatia, space podujatia: spec.representations.feed: feed output `novinky`: `filter.q` is no NGSI-LD query: unexpected end of input";

const TABLE_REFUSAL =
  "Endpoint bbsk/podujatia, space podujatia: widget output `mapa`, Event: `table.filter` is not an NGSI-LD q of at most 1000 characters, such as `startDate>=today` or `category==\"concert\"` (EP-125)";
const MAP_REFUSAL =
  "Endpoint bbsk/podujatia, space podujatia: widget output `mapa`, Event: `mapping.colour` reads `name`, a LanguageProperty; colour takes an enum or a boolean (EP-125)";

async function stubApi(page: Page, answer: "refuse" | "forbid" | "accept" | "refuseMap"): Promise<string[]> {
  const posted: string[] = [];
  await page.route("**/api/**", async (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "bbsk" }] });
    if (path.endsWith("/datamodels"))
      return json(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "DataModel", metadata: { name: "events", namespace: "bbsk" }, spec: { contextSpaceRef: "podujatia", version: "1.0.0", lifecycle: "published", linkml: LINKML } }]));
    if (path.endsWith("/spaces")) return json(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "podujatia", namespace: "bbsk" }, spec: {} }]));
    if (path.endsWith("/endpoints") && route.request().method() === "POST") {
      posted.push(route.request().postData() ?? "");
      if (answer === "refuse")
        return json({ type: "https://joinedcontext.com/problems/unprocessable", title: "Unprocessable Content", status: 422, detail: REFUSAL, errors: [REFUSAL] }, 422);
      if (answer === "refuseMap")
        return json({ type: "https://joinedcontext.com/problems/unprocessable", title: "Unprocessable Content", status: 422, detail: MAP_REFUSAL, errors: [MAP_REFUSAL] }, 422);
      if (answer === "refuseTable")
        return json({ type: "https://joinedcontext.com/problems/unprocessable", title: "Unprocessable Content", status: 422, detail: TABLE_REFUSAL, errors: [TABLE_REFUSAL] }, 422);
      if (answer === "forbid")
        return json({ type: "https://joinedcontext.com/problems/forbidden", title: "Forbidden", status: 403, detail: "no role of yours grants create on endpoints in project bbsk" }, 403);
    }
    if (path.endsWith("/endpoints")) return json(list([ENDPOINT]));
    if (path.endsWith("/endpoints/podujatia")) return json(ENDPOINT);
    return json(list([]));
  });
  return posted;
}

const SHOTS = join(process.cwd(), "test-results", "representation-outputs");

async function shot(page: Page, name: string, locator?: ReturnType<Page["locator"]>): Promise<void> {
  mkdirSync(SHOTS, { recursive: true });
  const width = page.viewportSize()?.width ?? 0;
  const path = join(SHOTS, `${name}-${width}.png`);
  if (locator) await locator.screenshot({ path });
  else await page.screenshot({ path, fullPage: true });
}

/** The form up to the outputs, with the Endpoint's name and type filled and iCal on. */
async function openForm(page: Page): Promise<ReturnType<Page["locator"]>> {
  await page.goto("/projects/bbsk/endpoints/new?lang=en");
  await page.getByRole("textbox", { name: "Name", exact: true }).fill("podujatia");
  await page.getByRole("checkbox", { name: "Event" }).check();
  return page.getByTestId("representation-outputs");
}

for (const width of [1440, 400]) {
  test.describe(`calendar and feed outputs at ${width}`, () => {
    test.use({ viewport: { width, height: 900 } });

    test("empty, a calendar and a feed added and filled, a duplicate name, then the server's 422", async ({ page }) => {
      const posted = await stubApi(page, "refuse");
      const section = await openForm(page);
      await expect(section).toContainText("No news feed yet");
      await expect(section).toContainText("Turn on the iCalendar feed");
      await shot(page, "1-empty", section);

      await page.getByLabel("iCalendar feed").check();
      await section.getByRole("button", { name: "Add a calendar" }).click();
      await section.getByLabel("Title in Slovak").first().fill("Kalendár podujatí");
      await section.getByRole("button", { name: "Add a news feed" }).click();
      const feed = section.locator("section").filter({ hasText: "News feeds" });
      await feed.getByLabel("Title in Slovak").fill("Novinky z kultúry");
      await feed.getByLabel("Only entities matching (optional)").fill('eventCategory=="kultura"');
      await expect(feed.getByLabel("Title", { exact: true }).locator("option")).toContainText(["name", "startDate"]);
      await feed.getByLabel("Title", { exact: true }).selectOption("name");
      await shot(page, "2-filled", section);

      await feed.getByLabel(/^Name in the address/).fill("kalendar");
      await section.getByRole("button", { name: "Add a news feed" }).click();
      await feed.getByLabel(/^Name in the address/).last().fill("novinky");
      await expect(feed.getByText("Each output needs its own name")).toHaveCount(0);
      await feed.getByLabel(/^Name in the address/).first().fill("novinky");
      await expect(feed.getByText("Each output needs its own name; these occur twice: novinky")).toBeVisible();
      await shot(page, "3-duplicate", section);
      await feed.getByRole("button", { name: /^Remove output/ }).last().click();

      await page.getByRole("button", { name: "Check", exact: true }).click();
      const refusal = page.getByRole("alert").filter({ hasText: "filter.q" });
      await expect(refusal).toBeVisible();
      expect(posted.at(-1)).toContain('"feed":[{"name":"novinky"');
      expect(posted.at(-1)).toContain('"ical":[{"name":"kalendar"');
      await shot(page, "4-refused", refusal.locator("xpath=.."));
    });

    test("map defaults: empty, colour, popup and clustering picked from the model, then the server's 422 naming the member (T-5239)", async ({ page }) => {
      const posted = await stubApi(page, "refuseMap");
      const section = await openForm(page);
      const map = section.locator("section").filter({ hasText: "Catalogue map defaults" });
      await expect(map.getByText("The map uses its built-in defaults")).toBeVisible();
      await shot(page, "map-empty", map);
      await map.getByRole("button", { name: "Add map defaults" }).click();
      const colour = map.getByLabel("Colour by");
      await expect(colour.locator("option")).toHaveText(["One colour per type", "category", "free"]);
      await colour.selectOption("category");
      const popup = map.getByRole("group", { name: "Popup shows" });
      await popup.getByRole("checkbox", { name: "startDate" }).check();
      await popup.getByRole("checkbox", { name: "url" }).check();
      await map.getByLabel("Group nearby markers").selectOption("off");
      await shot(page, "map-filled", map);
      await page.getByRole("button", { name: "Check", exact: true }).click();
      const refusal = page.getByRole("alert").filter({ hasText: "mapping.colour" });
      await expect(refusal).toBeVisible();
      await shot(page, "map-refused");
      expect(posted.find((body) => body.includes('"widget"'))).toContain('"widget":[{"name":"mapa","sources":[{"type":"Event","mapping":{"colour":"category","popup":["startDate","url"]},"cluster":false}]}]');
    });

    test("table start: filter, sort, page length and columns from the model, then the server's 422 naming the member (T-5197)", async ({ page }) => {
      const posted = await stubApi(page, "refuseTable");
      const section = await openForm(page);
      const map = section.locator("section").filter({ hasText: "Catalogue map defaults" });
      await map.getByRole("button", { name: "Add map defaults" }).click();
      const table = map.getByRole("group", { name: "Table start" });
      await expect(table.getByText("Example: startDate>=today")).toBeVisible();
      await expect(table.getByLabel("Direction")).toBeDisabled();
      await shot(page, "table-empty", table);
      await table.getByLabel("Default filter").fill("startDate>=today");
      await table.getByLabel("Sort by").selectOption("startDate");
      await table.getByLabel("Direction").selectOption("desc");
      await table.getByLabel("Rows per page").selectOption("50");
      const columns = table.getByRole("group", { name: "Columns shown" });
      await columns.getByRole("checkbox", { name: "name" }).check();
      await columns.getByRole("checkbox", { name: "startDate" }).check();
      await shot(page, "table-filled", table);
      await page.getByRole("button", { name: "Check", exact: true }).click();
      await expect(page.getByRole("alert").filter({ hasText: "table.filter" })).toBeVisible();
      await shot(page, "table-refused");
      expect(posted.find((body) => body.includes('"table"'))).toContain('"table":{"filter":"startDate>=today","orderBy":"startDate;desc","pageLength":50,"columns":["name","startDate"]}');
    });

    test("a person without the Endpoint right is refused with the reason", async ({ page }) => {
      await stubApi(page, "forbid");
      const section = await openForm(page);
      await section.getByRole("button", { name: "Add a news feed" }).click();
      await page.getByRole("button", { name: "Check", exact: true }).click();
      const refusal = page.getByRole("alert").filter({ hasText: /grants create on endpoints|permission|Forbidden/i });
      await expect(refusal.first()).toBeVisible();
      await shot(page, "5-forbidden");
    });

    test("the Endpoint page offers the calendar", async ({ page }) => {
      await stubApi(page, "accept");
      await page.goto("/projects/bbsk/endpoints/podujatia?lang=en");
      await expect(page.getByRole("link", { name: "ical", exact: true })).toBeVisible();
      await shot(page, "6-detail");
    });
  });
}

const TWO_CLASSES = LINKML.replace("classes:\n", "classes:\n  Place:\n    slots: [name, address]\n");

for (const width of [1440, 400]) {
  test(`two writable classes with their own scopes write one rule each (F-UI3-003, ${width})`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await stubApi(page, "accept");
    await page.route(
      (url) => url.pathname.endsWith("/datamodels"),
      (route) =>
        route.fulfill({
          contentType: "application/json",
          body: JSON.stringify(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "DataModel", metadata: { name: "events", namespace: "bbsk" }, spec: { contextSpaceRef: "podujatia", version: "1.0.0", lifecycle: "published", linkml: TWO_CLASSES } }])),
        }),
    );
    await page.goto("/projects/bbsk/endpoints/new?lang=en");
    await page.getByRole("textbox", { name: "Name", exact: true }).fill("podujatia");
    for (const [cls, scope] of [["Event", "/bbsk/events"], ["Place", "/bbsk/places"]]) {
      await page.getByRole("checkbox", { name: cls, exact: true }).check();
      await page.getByRole("checkbox", { name: `${cls} Writable` }).check();
      await page.getByRole("textbox", { name: `${cls} Scope` }).fill(scope);
    }
    await page.getByText("Show the manifests this proposes").click();
    const preview = page.getByTestId("endpoint-preview");
    await expect(preview).toContainText('"rules"');
    const manifests = JSON.parse((await preview.textContent()) ?? "[]") as { kind: string; metadata: { name: string }; spec: Record<string, unknown> }[];
    const write = manifests.find((m) => m.metadata.name === "podujatia-write")!;
    expect((write.spec.rules as { scopeQ: string }[]).map((rule) => rule.scopeQ)).toEqual(["/bbsk/events", "/bbsk/places"]);
    expect(write.spec).not.toHaveProperty("scopeQ");
    await preview.scrollIntoViewIfNeeded();
    await shot(page, "write-policy-rules");
  });
}

