import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";

// The Data models page as a person meets it (T-2765, T-2762; DM-61, DM-62): the list of the project's
// models, one click to a model's own page, and its diagram, form and YAML. `vite preview` has no
// Portal API behind it, so the API is answered in the browser; the pages are the real build.

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@hel.fi",
  roles: ["portal-editor"],
};

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

const LINKML = [
  "id: https://hel.fi/models/helsinki",
  "name: helsinki",
  "classes:",
  "  Alert:",
  "    slots: [category, refStation]",
  "  Station:",
  "    slots: [name]",
  "slots:",
  "  category: { range: AlertCategory }",
  "  refStation: { range: Station }",
  "  name: { range: string }",
  "enums:",
  "  AlertCategory:",
  "    permissible_values:",
  "      traffic: { title: { en: Traffic } }",
  "      weather: {}",
  "",
].join("\n");

const MODELS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataModel",
    metadata: { name: "helsinki", namespace: "helsinki", title: "Helsinki city context" },
    spec: { contextSpaceRef: "helsinki", version: "1.0.0", lifecycle: "published", classes: ["Alert", "Station"], linkml: LINKML },
  },
]);

const SPACES = list([
  { apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "helsinki", namespace: "helsinki" }, spec: {} },
]);

const ENDPOINTS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "helsinki-alerts", namespace: "helsinki" },
    spec: { contextSpaceRef: "helsinki", slug: "alerts0000000000000000000000000a", audience: "public" },
  },
]);

const ARTIFACTS = {
  jsonSchema: {
    definitions: {
      Alert: {
        type: "object",
        properties: { category: { type: "string", enum: ["traffic", "weather"], title: "category" } },
      },
      Station: { type: "object", properties: { name: { type: "string", title: "name" } } },
    },
  },
  example: { type: "Alert", category: "weather" },
};

async function stubApi(page: Page): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) =>
      route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/datamodels")) return json(MODELS);
    if (path.endsWith("/spaces")) return json(SPACES);
    if (path.endsWith("/spaces/helsinki")) return json(SPACES.items[0]);
    if (path.endsWith("/endpoints")) return json(ENDPOINTS);
    if (path === "/api/v1/tools/generate") return json(ARTIFACTS);
    return json(list([]));
  });
}

test.describe("the data models", () => {
  test("the list opens a model, and the model shows its diagram, form and YAML", async ({ page }) => {
    await stubApi(page);
    await page.goto("/projects/helsinki/models?lang=en");
    const table = page.getByRole("table", { name: "The project's data models" });
    await expect(table.getByRole("row")).toHaveCount(2);
    await expect(table).toContainText("1 endpoint");
    await table.getByRole("link", { name: "Helsinki city context" }).click();

    await expect(page).toHaveURL(/\/projects\/helsinki\/models\/helsinki$/);
    await expect(page.getByRole("heading", { level: 1, name: "Helsinki city context" })).toBeVisible();
    await expect(page.getByRole("group", { name: "The model's classes and what joins them" })).toBeVisible();
    await expect(page.getByRole("table", { name: "Fields of Alert" })).toContainText("Link to Station");

    await page.getByRole("tab", { name: "Form" }).click();
    // rjsf keys a select by index; the person reads the picked value.
    await expect(page.getByLabel("category").locator("option:checked")).toHaveText("weather");

    await page.getByRole("tab", { name: "LinkML" }).click();
    await expect(page.getByLabel("The model's source (LinkML YAML)")).toContainText("permissible_values:");

    await page.getByRole("tab", { name: "Where used" }).click();
    await expect(page.getByRole("tabpanel").getByRole("link", { name: "helsinki-alerts" })).toHaveAttribute(
      "href",
      "/projects/helsinki/endpoints/helsinki-alerts",
    );

    await page.getByRole("navigation", { name: "Breadcrumb" }).getByRole("link", { name: "Data models" }).click();
    await expect(page).toHaveURL(/\/projects\/helsinki\/models$/);
  });

  test("the space summarises its model, and Open data model opens the model's page", async ({ page }) => {
    await stubApi(page);
    // The model is the space's Data model tab since T-4825; since T-6090 the tab summarises the
    // model and links its one page instead of embedding a second modeler.
    await page.goto("/projects/helsinki/spaces/helsinki?tab=model&lang=en");
    const model = page.locator("section").filter({ has: page.getByRole("heading", { level: 2, name: "Data model" }) });
    await expect(model.getByText("2 classes")).toBeVisible();
    await expect(model.getByRole("group", { name: "The model's classes and what joins them" })).toHaveCount(0);
    await model.getByRole("link", { name: "Open data model" }).click();
    await expect(page).toHaveURL(/\/projects\/helsinki\/models\/helsinki$/);
    await expect(page.getByRole("group", { name: "The model's classes and what joins them" })).toBeVisible();
  });

  // T-6090 (owner walk): the catalog model's drawing sat below two paragraphs of notation, a strip
  // of marks and a minimap row, so the first screen showed no class. It is right under one toolbar now.
  for (const width of [1440, 400]) {
    test(`a model's drawing shows its classes on the first screen at ${width} px`, async ({ page }) => {
      await stubApi(page);
      await page.setViewportSize({ width, height: 900 });
      await page.goto("/projects/helsinki/models/helsinki?lang=en");
      const box = page.getByRole("button", { name: /^Open Alert/ });
      await expect(box).toBeVisible();
      expect((await box.boundingBox())?.y ?? Infinity).toBeLessThan(900);
      const notation = page.locator("summary", { hasText: "Notation" });
      await expect(notation).toBeVisible();
      await expect(page.getByRole("figure", { name: "Notation" })).toBeHidden();
      await notation.click();
      await expect(page.getByRole("figure", { name: "Notation" })).toBeVisible();
      if (process.env.SHOTS_DIR) {
        mkdirSync(process.env.SHOTS_DIR, { recursive: true });
        await page.screenshot({ path: join(process.env.SHOTS_DIR, `notation-${width}.png`) });
      }
    });
  }

  test("a model's address opens its page, and an unknown one says so", async ({ page }) => {
    await stubApi(page);
    await page.goto("/projects/helsinki/models/helsinki?lang=en");
    await expect(page.getByRole("heading", { level: 1, name: "Helsinki city context" })).toBeVisible();
    await page.goto("/projects/helsinki/models/nope?lang=en");
    await expect(page.getByText("No data model named nope")).toBeVisible();
  });
});

// The KPI model as B05 drew it wrong (T-4829): a slot over LinkML `Any` and an inline period.
const KPI_LINKML = [
  "id: https://hel.fi/models/helsinki-kpi",
  "name: helsinki-kpi",
  "prefixes:",
  "  jc: https://hel.fi/terms/",
  "  linkml: https://w3id.org/linkml/",
  "default_prefix: jc",
  "default_range: string",
  "slots:",
  "  name: { slot_uri: jc:name, title: { en: Name, fi: Nimi } }",
  "  currentValue: { slot_uri: jc:currentValue, range: Any, inlined: true, required: true }",
  "  calculationPeriod: { slot_uri: jc:calculationPeriod, range: CalculationPeriod, inlined: true }",
  "  startsAt: { slot_uri: jc:startsAt, range: datetime }",
  "  endsAt: { slot_uri: jc:endsAt, range: datetime }",
  "  computedBy: { slot_uri: jc:computedBy, range: uri, required: true }",
  "  location: { slot_uri: jc:location, annotations: { ngsi_ld_kind: GeoProperty } }",
  "classes:",
  "  KeyPerformanceIndicator:",
  "    class_uri: jc:KeyPerformanceIndicator",
  "    title: { en: Key performance indicator, fi: Avainmittari }",
  "    slots: [name, currentValue, calculationPeriod, computedBy, location]",
  "  CalculationPeriod:",
  "    class_uri: jc:CalculationPeriod",
  "    slots: [startsAt, endsAt]",
  "  Any:",
  "    class_uri: linkml:Any",
  "",
].join("\n");

/** New data model is one window for the name (owner 2026-11-01, T-6344); the empty editor follows. */
async function nameNewModel(page: Page, name: string): Promise<void> {
  const dialog = page.getByRole("dialog", { name: "New data model" });
  await dialog.getByRole("textbox", { name: "Name", exact: true }).fill(name);
  await dialog.getByRole("button", { name: "Create model" }).click();
  await expect(dialog).toBeHidden();
}

/** Add type opens one dialog: a Smart Data Model, a spreadsheet, or an empty type by name. */
async function addType(page: Page, name: string): Promise<void> {
  await page.getByRole("button", { name: "Add type", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Add type" });
  await dialog.getByRole("textbox", { name: "New type" }).fill(name);
  await dialog.getByRole("button", { name: "Add", exact: true }).click();
}

/** Add field opens one form: what the field holds, then its name; the new field opens after. */
async function addField(page: Page, name: string): Promise<void> {
  await page.getByRole("button", { name: "Add field", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Add field" });
  await dialog.getByRole("textbox", { name: "Name", exact: true }).fill(name);
  await dialog.getByRole("button", { name: "Add", exact: true }).click();
}

async function stubKpi(page: Page, verbs: string[], linkml = KPI_LINKML): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/permissions/me")) {
      return json({ project: "helsinki", grants: [{ rule: { kinds: ["DataModel", "ContextSpace", "Endpoint"], verbs } }] });
    }
    // The editor reads the model's source here; a JSON list in its place parsed as a model with no
    // id and no name, so every edit there showed two errors (T-6162).
    if (path.endsWith("/datamodels/helsinki-kpi/source") && route.request().method() === "GET") {
      return route.fulfill({ status: 200, contentType: "text/yaml", body: linkml });
    }
    if (path.endsWith("/datamodels")) {
      return json(list([{
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "DataModel",
        metadata: { name: "helsinki-kpi", namespace: "helsinki", title: "Helsinki indicators" },
        spec: { contextSpaceRef: "helsinki-kpi", version: "1.0.0", lifecycle: "published", linkml },
      }]));
    }
    if (path === "/api/v1/tools/sdm-catalog") return json({ subjects: [] });
    if (path.endsWith("/organizations")) {
      return json(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "Organization", metadata: { name: "hel" }, spec: { domain: "hel.fi", locales: ["en", "fi"], defaultLocale: "en" } }]));
    }
    return json(list([]));
  });
}

/**
 * Screenshots for the task, where `JC_EVIDENCE_DIR` asks for them. A dialog is shot as the
 * viewport shows it: a full-page capture stretches the fixed dialog past the phone's height.
 */
async function shot(page: Page, name: string, fullPage = true): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage });
}

test.describe("one plain model editor (T-4829)", () => {
  for (const width of [1440, 400]) {
    test(`a steward reads the KPI in plain words and opens Add field at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
      await stubKpi(page, ["read", "propose"]);
      await page.goto("/projects/helsinki/datamodels/helsinki-kpi?lang=en");
      await expect(page).toHaveURL(/\/projects\/helsinki\/models\/helsinki-kpi(\?|$)/);
      const diagram = page.getByRole("group", { name: "The model's classes and what joins them" });
      await expect(diagram).toBeVisible();
      await expect(diagram.getByText("Any", { exact: true })).toHaveCount(0);
      await expect(diagram.getByText("CalculationPeriod", { exact: true })).toHaveCount(0);
      const fields = page.getByRole("table", { name: "Fields of Key performance indicator" });
      await expect(fields.getByRole("row", { name: /currentValue/ })).toContainText("Any value");
      await expect(fields.getByRole("row", { name: /calculationPeriod/ })).toContainText("Holds startsAt, endsAt");
      await expect(fields.getByRole("row", { name: /location/ })).toContainText("Location");
      await expect(page.getByRole("table", { name: "Fields of Any" })).toHaveCount(0);
      const overflow = await page.evaluate(() => (document.scrollingElement?.scrollWidth ?? 0) - window.innerWidth);
      expect(overflow).toBeLessThanOrEqual(0);
      await shot(page, `model-overview-steward-${width}`);

      await page.getByRole("button", { name: "Add field to Key performance indicator" }).click();
      const dialog = page.getByRole("dialog");
      await expect(dialog.getByRole("button", { name: "Propose field" })).toBeVisible();
      await expect(dialog.getByText(/IRI/)).toHaveCount(0);
      await shot(page, `model-add-field-steward-${width}`);
    });
  }

  test("the editor offers Smart Data Models first, mints the IRI, and keeps it under Advanced", async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await stubKpi(page, ["read", "propose"]);
    await page.goto("/projects/helsinki/models?new=blank&lang=en");
    await nameNewModel(page, "Schools");
    await page.getByRole("button", { name: "Add type", exact: true }).click();
    await expect(page.getByRole("dialog", { name: "Add type" }).getByRole("button", { name: /Import from Smart Data Models/ })).toBeVisible();
    await page.keyboard.press("Escape");
    await addType(page, "School");
    await expect(page.getByRole("heading", { level: 2, name: "School" })).toBeVisible();
    // Titles in the Organization's languages: the reader's row, fi for Helsinki under Translations (T-6254).
    await expect(page.getByLabel("Title (en)")).toBeVisible();
    await page.getByText("Translations: fi").click();
    await expect(page.getByLabel("Title (fi)")).toBeVisible();
    await expect(page.getByLabel("Class IRI")).toBeHidden();
    await shot(page, "model-editor-steward-1440");
    await page.getByText("Advanced (IRIs, mixins, profiles)").first().click();
    await expect(page.getByLabel("Class IRI")).toHaveValue(/^[a-z]+:School$/);
  });

  // T-6101: a unit measures a number, so a unit on a text field makes it a Number, said with Undo.
  for (const width of [1440, 400]) {
    test(`a unit on a text field makes it a number and offers Undo at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
      await stubKpi(page, ["read", "propose"]);
      await page.goto("/projects/helsinki/models?new=blank&lang=en");
      await nameNewModel(page, "Weather");
      await addType(page, "WeatherObserved");
      await addField(page, "temperature");
      await page.getByRole("dialog", { name: "temperature" }).getByRole("combobox", { name: "Unit" }).fill("CEL");
      await page.keyboard.press("Enter");
      const notice = page.getByRole("status").filter({ hasText: "A unit measures a number" });
      await expect(notice).toBeVisible();
      await expect(page.locator("#slot-range")).toHaveValue("float");
      await shot(page, `unit-made-number-${width}`);
      await notice.getByRole("button", { name: "Undo" }).click();
      await expect(page.locator("#slot-range")).toHaveValue("string");
      await expect(notice).toHaveCount(0);
    });
  }

  // T-6101: Model Tools answers a model it cannot convert with 200 and `errors`; the wizard
  // showed "Fetching the upstream model..." for good. The answer below is dev's for Event before
  // the Model Tools fix.
  for (const width of [1440, 400]) {
    test(`a catalogue model that cannot be imported says why at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
      await stubKpi(page, ["read", "propose"]);
      await page.route("**/api/v1/tools/**", async (route) => {
        const path = new URL(route.request().url()).pathname;
        const body = path.endsWith("/sdm-catalog")
          ? { subjects: [{ name: "dataModel.TourismDestinations", title: "TourismDestinations", models: [{ id: "dataModel.TourismDestinations/Event", name: "Event", description: "An event of interest to visitors.", attributes: ["subCategory"] }] }] }
          : { generatorVersion: "1.11.1", errors: ["garden: duplicate key"] };
        await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
      });
      await page.goto("/projects/helsinki/models?new=blank&lang=en");
      await nameNewModel(page, "Events");
      await page.getByRole("button", { name: "Add type", exact: true }).click();
      await page.getByRole("dialog", { name: "Add type" }).getByRole("button", { name: /Import from Smart Data Models/ }).click();
      await page.getByRole("button", { name: /Event/ }).click();
      const alert = page.getByRole("alert").filter({ hasText: "garden: duplicate key" });
      await expect(alert).toContainText("The catalogue's Event schema cannot be turned into a model.");
      await expect(alert.getByRole("button", { name: "Retry" })).toBeVisible();
      await expect(page.getByText("Fetching the upstream model…")).toHaveCount(0);
      // At 400 px the preview sits below the dialog's fold: the refusal scrolls into view (T-6106).
      await expect(alert).toBeInViewport({ ratio: 1 });
      const overflow = await page.evaluate(() => (document.scrollingElement?.scrollWidth ?? 0) - window.innerWidth);
      expect(overflow).toBeLessThanOrEqual(0);
      await shot(page, `sdm-import-error-${width}`, false);
    });
  }

  for (const width of [1440, 400]) {
    test(`a viewer reads the same page with no Add field at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
      await stubKpi(page, ["read"]);
      await page.goto("/projects/helsinki/models/helsinki-kpi?lang=en");
      await expect(page.getByRole("table", { name: "Fields of Key performance indicator" })).toBeVisible();
      await expect(page.getByRole("button", { name: /^Add field to/ })).toHaveCount(0);
      await shot(page, `model-overview-viewer-${width}`);
    });
  }
});

// The generated schema as Model Tools writes it for the Helsinki models (B06, T-4830): an optional
// enum as `anyOf [X, null]`, a value of two types as an untitled `anyOf`, GeoJSON coordinates with
// no `items`; and the model's published v1.0.0 as a merged Change.
const KPI_ARTIFACTS = {
  jsonSchema: {
    definitions: {
      Any: {},
      TransportMode: { type: "string", enum: ["bus", "tram"], title: "TransportMode" },
      KeyPerformanceIndicator: {
        type: "object",
        properties: {
          name: { type: "string", title: "Name" },
          currentValue: { $ref: "#/definitions/Any", anyOf: [{ type: "number" }, { type: "string" }], description: "The number." },
          mode: { anyOf: [{ $ref: "#/definitions/TransportMode" }, { type: "null" }], description: "What runs it." },
          location: {
            type: ["object", "null"],
            "x-ngsi-ld-kind": "GeoProperty",
            properties: {
              type: { type: "string", enum: ["Point", "Polygon"] },
              coordinates: { type: "array" },
            },
            required: ["type", "coordinates"],
          },
        },
      },
    },
  },
  example: { type: "KeyPerformanceIndicator", name: "Bus lines on time", currentValue: 94.5, location: { type: "Point", coordinates: [24.94, 60.17] } },
};

const PUBLISHED_CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-helsinki-00000007", namespace: "helsinki" },
  status: { lane: "yellow", phase: "Merged", plan: { create: 1, update: 0, delete: 0 } },
  summary: { key: "change.summary.create", params: { kind: "DataModel", name: "helsinki-kpi", fields: 6 } },
  author: { name: "Aino Virtanen" },
  createdAt: "2026-10-12T08:10:00Z",
  decision: { by: "mikko@hel.fi", at: "2026-10-12T09:02:00Z" },
};

test.describe("a model change is proposed, and read back (T-4830)", () => {
  for (const width of [1440, 400]) {
    test(`the Form tab has no schema error, History lists v1.0.0, Propose waits for an edit at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
      await stubKpi(page, ["read", "propose"]);
      await page.route("**/api/v1/tools/generate", (route) =>
        route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(KPI_ARTIFACTS) }),
      );
      await page.route("**/api/v1/projects/helsinki/changes/history**", (route) =>
        route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(list([PUBLISHED_CHANGE])) }),
      );
      await page.route("**/api/v1/projects/helsinki/datamodels/helsinki-kpi/source", (route) =>
        route.fulfill({ status: 200, contentType: "text/plain", body: KPI_LINKML }),
      );

      await page.goto("/projects/helsinki/models/helsinki-kpi?tab=form&lang=en");
      const panel = page.getByRole("tabpanel");
      await expect(panel.getByLabel("Name")).toHaveValue("Bus lines on time");
      await expect(panel).not.toContainText(/Unsupported field schema|Missing items definition|option \d/);
      await expect(panel).not.toContainText(/\bT-\d{4}\b|Development\/\d+/);
      await expect(panel.locator('[aria-invalid="true"]'), "a preview marks nothing invalid").toHaveCount(0);
      await expect(panel.getByRole("combobox").filter({ has: page.locator("option", { hasText: "Number" }) })).toBeVisible();
      await shot(page, `model-form-${width}`);

      await page.getByRole("tab", { name: "History" }).click();
      await expect(page.getByTestId("resource-history-item")).toHaveCount(1);
      await expect(page.getByTestId("resource-history-item")).toContainText("chg-helsinki-00000007");
      await shot(page, `model-history-${width}`);

      await page.goto("/projects/helsinki/models?edit=helsinki-kpi&lang=en");
      const propose = page.getByRole("button", { name: "Propose v1.0.0" });
      await expect(propose).toHaveAttribute("aria-disabled", "true");
      await expect(propose).toHaveAccessibleDescription("Nothing to propose: edit the model first.");
      const overflow = await page.evaluate(() => (document.scrollingElement?.scrollWidth ?? 0) - window.innerWidth);
      expect(overflow).toBeLessThanOrEqual(0);
      await shot(page, `model-propose-unchanged-${width}`);
    });
  }
});

// T-6162: the editor's name line. A new model is named, never placed in a space; a rename of an
// open model writes its title and leaves the id and the file name as they are.
test.describe("the model editor's name line (T-6162)", () => {
  for (const width of [1440, 400]) {
    test(`a new model is named with no space, and an open one renamed behind the pencil at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
      await stubKpi(page, ["read", "propose"]);
      await page.goto("/projects/helsinki/models?new=blank&lang=en");
      // Named in the New data model window (owner 2026-11-01, T-6344), never placed in a space.
      const window_ = page.getByRole("dialog", { name: "New data model" });
      await expect(window_.getByRole("button", { name: "Create model" })).toHaveAttribute("aria-disabled", "true");
      await expect(window_.getByRole("combobox", { name: /space/i })).toHaveCount(0);
      await shot(page, `new-model-window-${width}`, false);
      await window_.getByRole("textbox", { name: "Name", exact: true }).fill("helsinki-kpi");
      await expect(window_.getByRole("alert")).toContainText("helsinki-kpi");
      await shot(page, `new-model-window-taken-${width}`, false);
      await nameNewModel(page, "Street trees");
      await expect(page.getByRole("heading", { level: 1, name: "Street trees" })).toBeVisible();
      await expect(page.getByRole("main").getByText(/context space/i)).toHaveCount(0);
      await shot(page, `new-model-named-${width}`);

      await page.goto("/projects/helsinki/models?edit=helsinki-kpi&lang=en");
      await page.getByRole("button", { name: /^Rename / }).click();
      const name = page.getByRole("textbox", { name: "Name", exact: true });
      await name.fill("");
      await page.getByRole("button", { name: "Use this name" }).click();
      await expect(page.getByText("Type a name: the model needs one.")).toBeVisible();
      await shot(page, `rename-empty-${width}`);
      await name.fill("City indicators");
      await name.press("Enter");
      await expect(page.getByText("City indicators", { exact: true })).toBeVisible();
      // The file sits under Details since T-6254, out of the title row.
      await page.getByText("Details", { exact: true }).click();
      await expect(page.getByText("helsinki-kpi.linkml.yaml")).toBeVisible();
      await expect(page.getByRole("button", { name: /^Propose v/ })).not.toHaveAttribute("aria-disabled", "true");
      const overflow = await page.evaluate(() => (document.scrollingElement?.scrollWidth ?? 0) - window.innerWidth);
      expect(overflow).toBeLessThanOrEqual(0);
      await shot(page, `renamed-${width}`);
    });

    test(`a viewer reads the name line with no rename at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
      await stubKpi(page, ["read"]);
      await page.goto("/projects/helsinki/models?edit=helsinki-kpi&lang=en");
      await page.getByText("Details", { exact: true }).click();
      await expect(page.getByText("helsinki-kpi.linkml.yaml")).toBeVisible();
      // The stub's manifest carries the title the list shows; the LinkML has none.
      await expect(page.getByText("Helsinki indicators", { exact: true })).toBeVisible();
      await expect(page.getByRole("button", { name: /^Rename / })).toHaveCount(0);
      await shot(page, `name-line-viewer-${width}`);
    });
  }
});

// T-6162: an enumeration opens its values table from the rail: stored value fixed, label edited.
const ENUM_LINKML = `${KPI_LINKML}enums:\n  KpiStatus:\n    permissible_values:\n      active: { title: { en: Active, fi: Aktiivinen } }\n      retired: {}\n`;

test.describe("the model editor's enumerations (T-6162)", () => {
  for (const width of [1440, 400]) {
    test(`an enumeration's values are a table with fixed stored values at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
      await stubKpi(page, ["read", "propose"], ENUM_LINKML);
      await page.goto("/projects/helsinki/models?edit=helsinki-kpi&lang=en");
      await page.getByRole("button", { name: /^KpiStatus/ }).click();
      const table = page.getByRole("table", { name: "Values of KpiStatus" });
      await expect(table).toContainText("retired");
      await page.getByRole("textbox", { name: "Label of retired" }).fill("Retired");
      await page.getByRole("textbox", { name: "New value", exact: true }).fill("planned");
      await page.getByRole("button", { name: "Add value to KpiStatus" }).click();
      await expect(table).toContainText("planned");
      const overflow = await page.evaluate(() => (document.scrollingElement?.scrollWidth ?? 0) - window.innerWidth);
      expect(overflow).toBeLessThanOrEqual(0);
      await shot(page, `enum-values-${width}`);

      await page.getByRole("textbox", { name: "New enum" }).fill("Band");
      await page.getByRole("button", { name: "Add enum" }).click();
      await expect(page.getByRole("table", { name: "Values of Band" })).toContainText("No values yet");
      await shot(page, `enum-empty-${width}`);
    });
  }
});

// A type that specialises another, so the fields table shows what it inherits (T-6106).
const INHERITING_LINKML = KPI_LINKML.replace(
  "  KeyPerformanceIndicator:\n",
  "  Indicator:\n    title: { en: Indicator }\n    slots: [name, location]\n  KeyPerformanceIndicator:\n    is_a: Indicator\n",
).replace("    slots: [name, currentValue, calculationPeriod, computedBy, location]", "    slots: [currentValue, calculationPeriod, computedBy]");

test.describe("the model editor's import dialog, inherited fields and read-only source (T-6106)", () => {
  for (const width of [1440, 400]) {
    test(`a new model is named, Add type brings the import, and an open one reads its source first at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
      await stubKpi(page, ["read", "propose"], INHERITING_LINKML);
      // Registered after stubKpi, so it answers first: the editor reads the committed source.
      await page.route("**/datamodels/helsinki-kpi/source", (route) =>
        route.fulfill({ status: 200, contentType: "text/yaml", body: INHERITING_LINKML }),
      );
      await page.goto("/projects/helsinki/models?new=sdm&lang=en");
      await nameNewModel(page, "Indicators");
      await expect(page.getByRole("tab", { name: "Structure", selected: true })).toBeVisible();
      await shot(page, `blank-editor-${width}`);
      await page.getByRole("button", { name: "Add type", exact: true }).click();
      await shot(page, `add-type-dialog-${width}`, false);
      await page.getByRole("dialog", { name: "Add type" }).getByRole("button", { name: /Import from Smart Data Models/ }).click();
      const dialog = page.getByRole("dialog", { name: "Import a data model" });
      await expect(dialog).toBeVisible();
      // The empty catalogue, not its loading skeleton, is the state shot.
      await expect(dialog.getByText("The catalogue lists no model yet.")).toBeVisible();
      await shot(page, `import-dialog-${width}`, false);
      await page.keyboard.press("Escape");
      await expect(dialog).toBeHidden();

      await page.goto("/projects/helsinki/models?edit=helsinki-kpi&lang=en");
      await page.getByRole("button", { name: "KeyPerformanceIndicator", exact: true }).click();
      const fields = page.getByRole("table", { name: "Fields", exact: true });
      const inherited = fields.getByRole("row").filter({ hasText: "From Indicator" });
      await expect(inherited).toHaveCount(2);
      await expect(fields.getByRole("row").filter({ hasText: "This type" })).toHaveCount(3);
      await inherited.first().scrollIntoViewIfNeeded();
      await shot(page, `inherited-fields-${width}`);
      // An inherited field opens read-only: editing it would change the parent for every type.
      await inherited.filter({ hasText: "location" }).getByRole("button", { name: "location", exact: true }).click();
      const detail = page.getByRole("dialog", { name: "location" });
      await expect(detail).toContainText("Defined on Indicator");
      await expect(detail.getByRole("checkbox")).toHaveCount(0);
      await detail.scrollIntoViewIfNeeded();
      await shot(page, `inherited-field-read-only-${width}`);
      await detail.getByRole("button", { name: "Open Indicator" }).click();
      await expect(page.getByRole("dialog", { name: "location" }).getByRole("checkbox").first()).toBeVisible();
      await page.keyboard.press("Escape");
      await expect(page.getByRole("button", { name: "Indicator", exact: true })).toHaveAttribute("aria-current", "true");
      await page.getByRole("button", { name: "KeyPerformanceIndicator", exact: true }).click();

      await page.getByRole("tab", { name: "Source" }).click();
      const edit = page.getByRole("button", { name: "Edit source" });
      await expect(edit).toHaveAttribute("aria-pressed", "false");
      await expect(page.locator(".monaco-editor .view-lines").first()).toContainText(
        // The editor shows the first screen only (is_a sits below it at every width since the 80 % root size).
        "name: helsinki-kpi",
        { timeout: 60_000 },
      );
      await shot(page, `source-read-only-${width}`);
      await edit.click();
      await expect(page.getByRole("button", { name: "Done editing" })).toHaveAttribute("aria-pressed", "true");
      await shot(page, `source-editing-${width}`);
      const overflow = await page.evaluate(() => (document.scrollingElement?.scrollWidth ?? 0) - window.innerWidth);
      expect(overflow).toBeLessThanOrEqual(0);
    });
  }
});

/** The last daily quality run of the space, as GET …/spaces/{space}/quality answers it (DM-74). */
const QUALITY = {
  observedAt: "2026-10-21T03:00:00Z",
  checked: 120,
  invalid: 0,
  truncated: false,
  rules: [],
  freshness: [],
  types: [
    {
      type: "Alert",
      count: 120,
      newest: "2026-10-21T02:55:00Z",
      attributes: [
        { name: "category", present: 118, outliers: 0, outlierExamples: [] },
        { name: "refStation", present: 60, outliers: 0, outlierExamples: [] },
      ],
    },
    { type: "Station", count: 0, attributes: [] },
  ],
};

test.describe("a model's fields beside what its space holds (T-5066)", () => {
  for (const width of [1440, 400]) {
    test(`the Fields tab at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page);
      await page.route("**/api/v1/projects/helsinki/spaces/helsinki/quality", (route) =>
        route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(QUALITY) }),
      );
      await page.goto("/projects/helsinki/models/helsinki?tab=fields&lang=en");
      const alert = page.getByRole("table", { name: "Fields of Alert, what each allows and what the space holds" });
      const category = alert.getByRole("row").filter({ hasText: "category" });
      await expect(category).toContainText("traffic (Traffic), weather");
      await expect(category).toContainText("98%");
      await expect(page.getByRole("table", { name: "Fields of Station, what each allows and what the space holds" })).toContainText(
        "No entity of this type yet",
      );
      const query = category.getByRole("link", { name: "Open Explore on the Alert entities that have category" });
      await expect(query).toHaveAttribute("href", "/projects/helsinki/explore?space=helsinki&type=Alert&q=category");
      await shot(page, `model-fields-${width}`);
      await query.click();
      await expect(page).toHaveURL(/\/projects\/helsinki\/explore\?space=helsinki&type=Alert&q=category/);
    });
  }
});

test.describe("removing a model from its page (T-5066)", () => {
  for (const width of [1440, 400]) {
    test(`proposes the removal with the name typed back at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page);
      const deletes: string[] = [];
      await page.route("**/api/v1/projects/helsinki/datamodels/helsinki?*", (route) => {
        if (route.request().method() !== "DELETE") return route.fallback();
        const url = new URL(route.request().url());
        deletes.push(url.search);
        const body = url.searchParams.has("dryRun")
          ? { goesWith: [] }
          : {
              apiVersion: "joinedcontext.com/v1alpha1",
              kind: "ChangeProposal",
              metadata: { name: "chg-5066aaaa", namespace: "helsinki" },
              status: { lane: "red", phase: "PendingApproval" },
            };
        return route.fulfill({ status: url.searchParams.has("dryRun") ? 200 : 202, contentType: "application/json", body: JSON.stringify(body) });
      });
      await page.goto("/projects/helsinki/models/helsinki?lang=en");
      await page.getByRole("button", { name: /^Remove / }).click();
      const dialog = page.getByRole("dialog");
      const propose = dialog.getByRole("button", { name: "Propose removal" });
      await expect(propose).toHaveAttribute("aria-disabled", "true");
      await shot(page, `model-remove-empty-${width}`);
      await dialog.getByLabel("Type helsinki to confirm").fill("helsink");
      await shot(page, `model-remove-mismatch-${width}`);
      await dialog.getByLabel("Type helsinki to confirm").fill("helsinki");
      await propose.click();
      await expect(dialog.getByText("chg-5066aaaa")).toBeVisible();
      await shot(page, `model-remove-proposed-${width}`);
      expect(deletes).toEqual(["?dryRun=All", "?confirm=helsinki"]);
    });
  }
});

for (const width of [1440, 400]) {
  test(`a project of the second Organization writes titles in that Organization's languages (F-UI1-011, ${width} px)`, async ({ page }) => {
    await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
    await stubKpi(page, ["read", "propose"]);
    const org = (name: string, domain: string, locales: string[]) => ({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Organization",
      metadata: { name },
      spec: { domain, locales, defaultLocale: locales[0] },
    });
    await page.route(
      (url) => url.pathname === "/api/v1/projects" || url.pathname.endsWith("/organizations"),
      (route) =>
        route.fulfill({
          contentType: "application/json",
          body: JSON.stringify(
            route.request().url().endsWith("/organizations")
              ? list([org("hel", "hel.fi", ["en", "fi"]), org("zilina", "zilina.sk", ["sk", "en"])])
              : {
                  apiVersion: "joinedcontext.com/v1alpha1",
                  kind: "ProjectList",
                  items: [{ name: "helsinki", organization: "zilina" }],
                  organizations: [{ name: "hel" }, { name: "zilina" }],
                },
          ),
        }),
    );
    await page.goto("/projects/helsinki/models?new=blank&lang=en");
    await nameNewModel(page, "Schools");
    await addType(page, "School");
    await expect(page.getByLabel("Title (en)")).toBeVisible();
    await page.getByText("Translations: sk").click();
    await expect(page.getByLabel("Title (sk)")).toBeVisible();
    await expect(page.getByLabel("Title (fi)")).toHaveCount(0);
    await shot(page, `model-editor-second-organization-${width}`);
  });
}


// T-6094: the model as a full-screen canvas, with the Endpoints overlay the owner asked for. One
// model with the three Endpoints of the task's Done line, each bound to its own Policy.
const CITY = [
  "id: https://hel.fi/models/city",
  "name: city",
  "default_range: string",
  "classes:",
  "  Place:",
  "    slots: [name, address]",
  "  BikeHireDockingStation:",
  "    is_a: Place",
  "    slots: [capacity, freeBikes, contactPoint, operator]",
  "  Event:",
  "    slots: [name, startDate, location, organizer, category]",
  "  Organization:",
  "    slots: [name, homepage]",
  "  Dataset:",
  "    slots: [title, publisher, distribution, theme]",
  "  DataService:",
  "    slots: [title, publisher, servesDataset]",
  "  Distribution:",
  "    slots: [title, accessURL]",
  "  Person:",
  "    annotations: { personalData: true }",
  "    slots: [name, email, memberOf]",
  "slots:",
  "  name: { range: string, required: true }",
  "  address: { range: string }",
  "  capacity: { range: integer }",
  "  freeBikes: { range: integer }",
  "  contactPoint: { range: string }",
  "  operator: { range: Organization }",
  "  startDate: { range: datetime, required: true }",
  "  location: { range: Place }",
  "  organizer: { range: Organization }",
  "  category: { range: EventCategory, multivalued: true }",
  "  homepage: { range: uri }",
  "  title: { range: string, required: true }",
  "  publisher: { range: Organization }",
  "  distribution: { range: Distribution, multivalued: true }",
  "  theme: { range: string }",
  "  servesDataset: { range: Dataset, multivalued: true }",
  "  accessURL: { range: uri }",
  "  email: { range: string }",
  "  memberOf: { range: Organization }",
  "enums:",
  "  EventCategory:",
  "    permissible_values:",
  "      concert: {}",
  "      sport: {}",
  "      theatre: {}",
  "",
].join("\n");

const meta = (kind: string, name: string, spec: Record<string, unknown>) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind,
  metadata: { name, namespace: "helsinki" },
  spec,
});
const urn = (name: string) => `urn:ngsi-ld:Policy:hel.fi:helsinki-helsinki:${name}`;
const policy = (name: string, information: unknown[]) =>
  meta("Policy", name, { contextSpaceRef: "helsinki", assignee: { kind: "role", id: "public" }, operations: ["retrieveOps"], information });

const CANVAS_LISTS: Record<string, unknown> = {
  datamodels: list([meta("DataModel", "city", { contextSpaceRef: "helsinki", version: "1.0.0", linkml: CITY, classes: ["Event"] })]),
  spaces: SPACES,
  endpoints: list([
    meta("Endpoint", "helsinki-bikes", { contextSpaceRef: "helsinki", slug: "bikes00000000000000000000000000a", policyRef: urn("public-bikes"), projection: { hiddenAttributes: ["contactPoint"] } }),
    meta("Endpoint", "helsinki-events", { contextSpaceRef: "helsinki", slug: "events0000000000000000000000000a", policyRef: urn("public-events") }),
    meta("Endpoint", "catalog", { contextSpaceRef: "helsinki", slug: "catalog000000000000000000000000a", policyRef: urn("catalog-read") }),
  ]),
  policies: list([
    policy("public-bikes", [{ entities: [{ type: "BikeHireDockingStation" }] }]),
    policy("public-events", [{ entities: [{ type: "Event" }], propertyNames: ["name", "startDate", "category"], relationshipNames: ["location"] }, { entities: [{ type: "Place" }] }]),
    policy("catalog-read", [{ entities: [{ type: "Dataset" }, { type: "DataService" }, { type: "Distribution" }, { type: "Organization" }] }]),
  ]),
  pipelines: list([meta("Pipeline", "gbfs-stations", { output: { type: "BikeHireDockingStation" } })]),
};

async function stubCanvas(page: Page): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] });
    const plural = /^\/api\/v1\/projects\/helsinki\/([a-z]+)$/.exec(path)?.[1];
    return json(plural && CANVAS_LISTS[plural] ? CANVAS_LISTS[plural] : list([]));
  });
}

const opened = (page: Page, name: string) => page.getByRole("button", { name: `Open ${name} in the structure view`, exact: true });

test.describe("the model canvas (T-6094)", () => {
  test("opens full screen from the model page and lights what each Endpoint serves", async ({ page }) => {
    await stubCanvas(page);
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto("/projects/helsinki/models/city?lang=en");
    await page.getByRole("link", { name: "Open full screen" }).click();
    await expect(page).toHaveURL(/\/projects\/helsinki\/models\/city\/diagram$/);
    await expect(page.getByRole("heading", { level: 1, name: "Diagram of city" })).toBeVisible();
    await page.getByRole("button", { name: "Fit" }).click();
    // Fit never shrinks the canvas below reading size (11 px slot text); a wider model pans.
    const percent = Number((await page.getByText(/^Zoom \d+ %$/).textContent())?.match(/\d+/)?.[0]);
    expect(percent).toBeGreaterThanOrEqual(110);
    for (const name of ["Place", "BikeHireDockingStation", "Event", "Organization", "Dataset", "DataService", "Distribution"]) {
      await expect(opened(page, name)).toBeVisible();
    }
    // Person is personal data (DM-120): its own tab, its header in the danger tint.
    await expect(opened(page, "Person")).toHaveCount(0);
    await shot(page, "canvas-1440");
    await page.getByRole("tab", { name: "Personal data" }).click();
    await expect(page.getByRole("tab", { name: "Personal data" })).toHaveAttribute("aria-selected", "true");
    await expect(opened(page, "Person")).toBeVisible();
    // The shot waits for the tab underline's colour transition and the pointer's hover to end.
    await page.mouse.move(700, 450);
    await expect(page.getByRole("tab", { name: "Main" })).toHaveCSS("border-bottom-color", "rgba(0, 0, 0, 0)");
    // One class fits the frame: no minimap, whose viewport box was a solid grey block (T-6164).
    await expect(page.getByTestId("diagram-minimap")).toHaveCount(0);
    await shot(page, "canvas-personal-1440");
    await page.getByRole("tab", { name: "Main" }).click();

    const overlay = page.getByLabel("Show what serves the classes");
    const lit = async (expected: string[]) => {
      for (const name of ["Place", "BikeHireDockingStation", "Event", "Organization", "Dataset", "DataService", "Distribution"]) {
        if (expected.includes(name)) await expect(opened(page, name)).not.toHaveAttribute("opacity", "0.45");
        else await expect(opened(page, name)).toHaveAttribute("opacity", "0.45");
      }
    };
    // The how-to is a ? beside the label (T-6164): the header holds no paragraph under the pick.
    await expect(page.getByRole("tooltip")).toHaveClass(/sr-only/);
    await page.getByRole("button", { name: "How the highlight works" }).hover();
    await expect(page.getByRole("tooltip")).not.toHaveClass(/sr-only/);
    await expect(overlay).toHaveAccessibleDescription(/Pick an Endpoint \(e\.g\. helsinki-bikes\)/);
    await shot(page, "canvas-hint-1440");
    await page.mouse.move(700, 450);
    await expect(page.getByRole("tooltip")).toHaveClass(/sr-only/);
    await overlay.selectOption("endpoint:helsinki-bikes");
    await expect(page.getByText("1 class of 8 is reached; the rest is faded.")).toBeVisible();
    await lit(["BikeHireDockingStation"]);
    await expect(opened(page, "BikeHireDockingStation").locator('[data-served="false"]')).toHaveCount(1);
    await shot(page, "canvas-bikes-1440");

    await overlay.selectOption("endpoint:helsinki-events");
    await expect(page.getByText("2 classes of 8 are reached; the rest is faded.")).toBeVisible();
    await lit(["Event", "Place"]);
    await expect(opened(page, "Event").locator('[data-served="false"]')).toHaveCount(1);
    await shot(page, "canvas-events-1440");

    await overlay.selectOption("endpoint:catalog");
    await expect(page.getByText("4 classes of 8 are reached; the rest is faded.")).toBeVisible();
    await lit(["Dataset", "DataService", "Distribution", "Organization"]);

    await opened(page, "Dataset").click();
    const panel = page.getByRole("complementary");
    await expect(panel.getByRole("table", { name: "Fields of Dataset" })).toContainText("distribution");
    await expect(panel.getByRole("link", { name: "catalog" })).toHaveAttribute("href", "/projects/helsinki/endpoints/catalog");
    await shot(page, "canvas-catalog-details-1440");

    await opened(page, "BikeHireDockingStation").click();
    await expect(panel.getByRole("link", { name: "gbfs-stations" })).toBeVisible();
    await expect(panel.getByText("from Place").first()).toBeVisible();
  });

  test("a phone gets the classes as a list and the canvas one press away", async ({ page }) => {
    await stubCanvas(page);
    await page.setViewportSize({ width: 400, height: 860 });
    await page.goto("/projects/helsinki/models/city/diagram?lang=en");
    const classes = page.getByRole("region", { name: "Classes" });
    await expect(classes.getByRole("button")).toHaveCount(8);
    await expect(classes.getByRole("button", { name: /Person/ })).toContainText("Personal data");
    await page.getByLabel("Show what serves the classes").selectOption("endpoint:catalog");
    // A faded card says the pick does not reach it, not how many Endpoints serve it.
    await expect(classes.getByRole("button", { name: /^Place/ })).toContainText("Not reached by catalog");
    await classes.getByRole("button", { name: /Dataset/ }).first().click();
    await expect(page.getByRole("table", { name: "Fields of Dataset" })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(400);
    // Each card holds its name and its badge line: a card cut to one control's height overlapped the next (T-6164).
    const card = await classes.getByRole("button", { name: /Dataset/ }).first().boundingBox();
    expect(card?.height ?? 0).toBeGreaterThan(48);
    await shot(page, "canvas-list-400");
    await page.getByRole("button", { name: "Show diagram" }).click();
    await expect(opened(page, "Dataset")).toBeVisible();
    // The canvas opens on the first class the overlay lights, not on a faded one (T-6094 UI judge).
    await shot(page, "canvas-diagram-400");
    await expect(page.getByTestId("diagram-badge-Dataset")).toBeInViewport();
    // A press on a box on the phone canvas brings its details into view (T-6164).
    await opened(page, "Dataset").click();
    const details = page.getByRole("heading", { level: 2, name: "Dataset" });
    await expect(details).toBeFocused();
    await expect(details).toBeInViewport({ ratio: 1 });
    await expect(page.getByRole("table", { name: "Fields of Dataset" })).toBeInViewport();
    await shot(page, "canvas-diagram-details-400", false);
    await page.getByRole("button", { name: "Back to the diagram" }).click();
    await expect(page.getByTestId("diagram-badge-Dataset")).toBeInViewport();
  });
});

// T-6146 (owner): the Overview is one toolbar row ending in Open full screen, the canvas across the
// content width and the relationships below it; the header names no space (a DataModel binds none).
const CITY_LINKED = CITY.replace("    slots: [name, address]", "    slots: [name, address, events]")
  .replace("  location: { range: Place }", "  location: { range: Place, inverse: events, annotations: { ngsi_ld_kind: Relationship } }")
  .replace(
    "  organizer: { range: Organization }",
    "  organizer: { range: Organization }\n  events: { range: Event, multivalued: true, inverse: location, annotations: { ngsi_ld_kind: Relationship } }",
  );

const OVERVIEW_MODELS: Record<string, string> = {
  city: CITY_LINKED,
  helsinki: LINKML,
  empty: "id: https://hel.fi/models/empty\nname: empty\n",
};

async function stubOverview(page: Page, verbs: string[]): Promise<void> {
  // A URL test, not a glob: the route gate (tests/gates.ts) reads a slash and two stars as a comment.
  await page.route((url) => url.pathname.startsWith("/api/"), async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/permissions/me")) return json({ project: "helsinki", grants: [{ rule: { kinds: ["DataModel", "ContextSpace", "Endpoint"], verbs } }] });
    if (path.endsWith("/datamodels")) {
      return json(list(Object.entries(OVERVIEW_MODELS).map(([name, linkml]) => meta("DataModel", name, { contextSpaceRef: "helsinki", version: "1.0.0", linkml }))));
    }
    if (path === "/api/v1/tools/generate") return json(ARTIFACTS);
    const plural = /^\/api\/v1\/projects\/helsinki\/([a-z]+)$/.exec(path)?.[1];
    return json(plural && CANVAS_LISTS[plural] && plural !== "datamodels" ? CANVAS_LISTS[plural] : list([]));
  });
}

test.describe("the model Overview (T-6146)", () => {
  for (const width of [1440, 400]) {
    test(`one toolbar row, the canvas full width, relationships below, at ${width}`, async ({ page }) => {
      await stubOverview(page, ["read", "propose", "delete"]);
      await page.setViewportSize({ width, height: 900 });
      await page.goto("/projects/helsinki/models/city?lang=en");
      const toolbar = page.getByRole("toolbar", { name: "Diagram tools" });
      const open = toolbar.getByRole("link", { name: "Open full screen" });
      await expect(open).toBeVisible();
      await expect(page.getByRole("tooltip").first()).not.toContainText("Context space");
      const drawing = page.getByRole("group", { name: "The model's classes and what joins them" });
      const table = page.getByRole("table", { name: "Relationships in the drawing" });
      const [frame, rows] = [await drawing.locator("xpath=..").boundingBox(), await table.boundingBox()];
      expect(frame && rows && rows.y >= frame.y + frame.height).toBe(true);
      if (width === 1440) {
        // The canvas takes the content column, not a share of it beside the table.
        const toolbarBox = await toolbar.boundingBox();
        expect(frame && toolbarBox && frame.width >= toolbarBox.width - 2).toBe(true);
        // Open full screen sits on the toolbar's own row, at its end.
        const [openBox, fit] = [await open.boundingBox(), await toolbar.getByRole("button", { name: "Fit" }).boundingBox()];
        expect(openBox && fit && Math.abs(openBox.y - fit.y) < 8 && openBox.x > fit.x).toBe(true);
      }
      await shot(page, `overview-large-${width}`);

      await page.goto("/projects/helsinki/models/helsinki?lang=en");
      await expect(page.getByRole("toolbar", { name: "Diagram tools" }).getByRole("link", { name: "Open full screen" })).toBeVisible();
      await shot(page, `overview-small-${width}`);

      await page.goto("/projects/helsinki/models/empty?lang=en");
      await expect(page.getByRole("heading", { level: 1, name: "empty" })).toBeVisible();
      // Nothing to draw: no toolbar and no way to an empty full-screen canvas.
      await expect(page.getByText("This model declares no class yet, so there is nothing to draw.")).toBeVisible();
      await expect(page.getByRole("link", { name: "Open full screen" })).toHaveCount(0);
      await shot(page, `overview-empty-${width}`);
    });

    test(`a viewer reads the Overview with Edit disabled, at ${width}`, async ({ page }) => {
      await stubOverview(page, ["read"]);
      await page.setViewportSize({ width, height: 900 });
      await page.goto("/projects/helsinki/models/city?lang=en");
      await expect(page.getByRole("toolbar", { name: "Diagram tools" }).getByRole("link", { name: "Open full screen" })).toBeVisible();
      await expect(page.getByRole("tooltip").first()).not.toContainText("Context space");
      await expect(page.getByRole("button", { name: "Edit model" })).toHaveAttribute("aria-disabled", "true");
      await shot(page, `overview-viewer-${width}`);
    });
  }
});

// T-6254 (UI judge on T-6207): the catalogue diagram opens fitted and centred, the editor shows one
// Title row and the id and file under Details, a disabled Propose says why in the open, the list caps
// Types at six names, the LinkML wraps inside its box, and a new model offers fields only after a type.
const CATALOG_SLOTS: Record<string, string[]> = {
  Dataset: ["title", "description", "name", "identifier", "url", "keyword", "theme", "spatial", "temporal", "frequency", "license", "issued", "modified", "source", "nextQuery", "searchText", "refPublisher", "refDataService"],
  DataService: ["title", "description", "identifier", "endpointURL", "accessURL", "accessRights", "license", "accessLevel", "visibleBecause", "entityTypes", "nextQuery", "refDataset"],
  Organization: ["name", "identifier", "url", "datasetCount", "publishedDatasets", "searchText"],
};
const CATALOG_LINKML = [
  "id: https://hel.fi/models/helsinki/catalog",
  "name: catalog",
  "title: { en: Open data catalogue }",
  "description: >-",
  "  One dataset of the catalogue: a public CKAN dataset by its CKAN name, or a public Endpoint no CKAN dataset names, with its publisher, its access service and the search text the assistant reads.",
  "prefixes:",
  "  jc: https://hel.fi/terms/",
  "  linkml: https://w3id.org/linkml/",
  "default_prefix: jc",
  "default_range: string",
  "classes:",
  ...Object.entries(CATALOG_SLOTS).flatMap(([name, slots]) => [`  ${name}:`, `    class_uri: jc:${name}`, `    slots: [${slots.join(", ")}]`]),
  "slots:",
  ...[...new Set(Object.values(CATALOG_SLOTS).flat())].map((slot) =>
    slot === "refPublisher" || slot === "publishedDatasets"
      ? `  ${slot}: { slot_uri: jc:${slot}, range: ${slot === "refPublisher" ? "Organization" : "Dataset"}, inverse: ${slot === "refPublisher" ? "publishedDatasets" : "refPublisher"}, multivalued: ${slot === "publishedDatasets"}, annotations: { ngsi_ld_kind: Relationship } }`
      : slot === "refDataService" || slot === "refDataset"
        ? `  ${slot}: { slot_uri: jc:${slot}, range: ${slot === "refDataService" ? "DataService" : "Dataset"}, inverse: ${slot === "refDataService" ? "refDataset" : "refDataService"}, multivalued: ${slot === "refDataset"}, annotations: { ngsi_ld_kind: Relationship } }`
        : `  ${slot}: { slot_uri: jc:${slot}, range: string }`,
  ),
  "",
].join("\n");
const CITY_TYPES = Array.from({ length: 27 }, (_, index) => `CityType${index + 1}`);

async function stubCatalog(page: Page): Promise<void> {
  const model = (name: string, title: string, classes: string[], linkml: string) => ({
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataModel",
    metadata: { name, namespace: "helsinki", title },
    spec: { contextSpaceRef: name, version: "1.14.0", lifecycle: "published", classes, linkml },
  });
  // A RegExp, not a glob: tests/gates.ts strips block comments and pairs the globs of the other stubs.
  await page.route(/\/api\//, async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/permissions/me")) return json({ project: "helsinki", grants: [{ rule: { kinds: ["DataModel", "ContextSpace", "Endpoint"], verbs: ["read", "propose"] } }] });
    if (path.endsWith("/datamodels/catalog/source") && route.request().method() === "GET") {
      return route.fulfill({ status: 200, contentType: "text/yaml", body: CATALOG_LINKML });
    }
    if (path.endsWith("/datamodels")) {
      return json(list([
        model("catalog", "Open data catalogue", Object.keys(CATALOG_SLOTS), CATALOG_LINKML),
        model("helsinki", "Helsinki city context", CITY_TYPES, "./helsinki.linkml.yaml"),
      ]));
    }
    if (path === "/api/v1/tools/generate") return json(ARTIFACTS);
    if (path === "/api/v1/tools/sdm-catalog") return json({ subjects: [] });
    if (path.endsWith("/organizations")) {
      return json(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "Organization", metadata: { name: "hel" }, spec: { domain: "hel.fi", locales: ["en", "fi", "sk"], defaultLocale: "en" } }]));
    }
    return json(list([]));
  });
}

test.describe("the model pages after the T-6207 judge (T-6254)", () => {
  for (const width of [1440, 400]) {
    test(`list, diagram, LinkML, editor and new model read cleanly at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
      await stubCatalog(page);
      const noSideScroll = async () =>
        expect(await page.evaluate(() => (document.scrollingElement?.scrollWidth ?? 0) - window.innerWidth)).toBeLessThanOrEqual(0);

      await page.goto("/projects/helsinki/models?lang=en");
      await expect(page.getByRole("link", { name: "21 more types of Helsinki city context" })).toBeVisible();
      await expect(page.getByRole("link", { name: "CityType7", exact: true })).toHaveCount(0);
      await noSideScroll();
      await shot(page, `models-list-${width}`, false);

      await page.goto("/projects/helsinki/models/catalog?lang=en");
      const drawing = page.getByRole("group", { name: "The model's classes and what joins them" });
      await expect(drawing).toBeVisible();
      if (width > 600) {
        // Fitted: the whole drawing inside its frame, the lowest class not cut, and centred.
        const fits = await drawing.evaluate((svg) => {
          const frame = svg.parentElement!;
          const box = svg.getBoundingClientRect();
          const around = frame.getBoundingClientRect();
          return {
            inside: box.bottom <= around.bottom && box.top >= around.top && box.right <= around.right,
            centred: Math.abs(box.left - around.left - (around.right - box.right)) < 24,
          };
        });
        expect(fits).toEqual({ inside: true, centred: true });
      }
      await shot(page, `model-catalog-${width}`, false);

      await page.goto("/projects/helsinki/models/catalog?tab=yaml&lang=en");
      const source = page.getByRole("group", { name: "The model's source (LinkML YAML)" });
      await expect(source).toContainText("public Endpoint no CKAN dataset names");
      // The long description line wraps inside the box: nothing scrolls sideways.
      expect(await source.evaluate((pre) => pre.scrollWidth - pre.clientWidth)).toBeLessThanOrEqual(0);
      await shot(page, `model-linkml-${width}`, false);

      await page.goto("/projects/helsinki/models?edit=catalog&lang=en");
      await expect(page.getByRole("heading", { level: 1, name: "Open data catalogue" })).toBeVisible();
      await expect(page.getByRole("textbox", { name: "Title (en)" })).toBeVisible();
      await expect(page.getByRole("textbox", { name: "Title (fi)" })).toBeHidden();
      await expect(page.getByText("catalog.linkml.yaml")).toBeHidden();
      await expect(page.getByText("v1.14.0", { exact: true })).toBeVisible();
      await expect(page.getByText("Nothing to propose: edit the model first.", { exact: true }).first()).toBeVisible();
      await noSideScroll();
      await shot(page, `model-editor-${width}`, false);

      await page.goto("/projects/helsinki/models?new=blank&lang=en");
      await expect(page.getByText("Fields belong to a type: add the first type under Types, then its fields here.")).toBeVisible();
      await expect(page.getByRole("textbox", { name: "New field" })).toHaveCount(0);
      await noSideScroll();
      await shot(page, `model-new-${width}`, false);
    });
  }
});

// T-6344: a structured value's shape (CalculationPeriod) sits folded under "Value structures", apart
// from the entity types; a link is added through Add field, whose relationship form fills the dialog.
test.describe("value structures and the link form of Add field (T-6344)", () => {
  for (const width of [1440, 400]) {
    test(`the rail folds value structures and Add field > link shows the relationship form at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
      await stubKpi(page, ["read", "propose"]);
      // A regex, not a glob: tests/gates.ts strips comments by regex, and a glob there ends one.
      await page.route(/\/datamodels\/helsinki-kpi\/source$/, (route) =>
        route.fulfill({ status: 200, contentType: "text/yaml", body: KPI_LINKML }),
      );
      await page.goto("/projects/helsinki/models?edit=helsinki-kpi&lang=en");
      const folded = page.getByText("Value structures (1)", { exact: true });
      await expect(folded).toBeVisible();
      await expect(page.getByRole("button", { name: "CalculationPeriod", exact: true })).toBeHidden();
      await shot(page, `value-structures-folded-${width}`, false);
      await folded.click();
      await expect(page.getByRole("button", { name: "CalculationPeriod", exact: true })).toBeVisible();
      await shot(page, `value-structures-open-${width}`, false);

      await page.getByRole("button", { name: "KeyPerformanceIndicator", exact: true }).click();
      await page.getByRole("button", { name: "Add field", exact: true }).first().click();
      await page.getByRole("radio", { name: /A link to another type/ }).click();
      const form = page.getByRole("form", { name: "Add a relationship from KeyPerformanceIndicator" });
      await expect(form).toBeVisible();
      await expect(form.getByRole("button", { name: "Add relationship" })).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `add-relationship-form-${width}`, false);
    });
  }
});
