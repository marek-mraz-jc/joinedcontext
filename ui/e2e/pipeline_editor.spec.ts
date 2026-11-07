import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { greenVerdict, isCheck } from "./verdict";

// T-0497: a pipeline created from the form, clicking real controls (PL-04, PL-39, UI-01, AP-13).
// Same shape as the managers journey: `vite preview` has no portal API behind it, so the API
// is answered in the browser and everything above it — the dialog, the selects, Monaco — is
// the real thing. The root route goes to the first project `GET /api/v1/projects` lists.

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const BRANDING = {
  instanceName: "joinedcontext",
  shortName: "joinedcontext",
  city: "Helsinki",
  organisation: "City of Helsinki",
  orgDomain: "hel.fi",
  domain: "portal.hel.fi",
  contactEmail: "data@hel.fi",
  licenseDefault: "CC-BY-4.0",
  logo: "",
  favicon: "",
  colours: {
    primary: "#1d4ed8",
    secondary: "#0f766e",
    accent: "#f59e0b",
    background: "#ffffff",
    text: "#0f172a",
  },
  fonts: { heading: "system-ui, sans-serif", body: "system-ui, sans-serif" },
  languages: { default: "en", offered: ["en"] },
  primaryForeground: "#ffffff",
};

const list = (items: unknown[]) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items,
});

const PROJECTS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ProjectList",
  items: [{ name: "helsinki" }],
};

const DATASOURCES = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataSource",
    metadata: { name: "mqtt-city", namespace: "helsinki" },
    spec: { type: "mqtt", mqtt: { urls: ["tls://mqtt.hel.fi:8883"], topics: ["aq/#"] } },
  },
]);

const ENDPOINTS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "public-air", namespace: "helsinki" },
    spec: { contextSpaceRef: "air", slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa", audience: "public" },
  },
]);

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-77aa11bb", namespace: "helsinki" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 1 } },
};

/** A pipeline whose mapping lives in the bento.yaml beside its manifest (PL-41, T-4818). */
const BENTO_PIPELINE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Pipeline",
  metadata: { name: "air-quality", namespace: "helsinki" },
  spec: {
    class: "auto",
    source: { dataSourceRef: { kind: "DataSource", name: "mqtt-city" } },
    targetEndpoint: "urn:ngsi-ld:Endpoint:hel.fi:air:public-air",
  },
  status: { phase: "Live" },
};
const BENTO = 'pipeline:\n  processors:\n    - xml:\n        operator: to_json\n    - mapping: |\n        root = { "type": "AirQualityObserved" }\n';

/** The target space's model holds two classes, so the mapping step asks which one it writes (T-4971). */
const SPACES = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ContextSpace",
    metadata: { name: "air", namespace: "helsinki" },
    spec: { dataModelRef: { kind: "DataModel", name: "air" } },
  },
]);
const LINKML = [
  "id: https://hel.fi/models/air",
  "name: air",
  "classes:",
  "  AirQualityObserved:",
  "    slots: [pm10]",
  "  WeatherObserved:",
  "    slots: [temperature]",
  "slots:",
  "  pm10:",
  "    range: float",
  "  temperature:",
  "    range: float",
  "",
].join("\n");
const DATAMODELS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataModel",
    metadata: { name: "air", namespace: "helsinki" },
    spec: { linkml: LINKML, classes: ["AirQualityObserved", "WeatherObserved"] },
  },
]);

async function stubApi(page: Page, pipelines: unknown[] = []): Promise<{ writes: string[] }> {
  const writes: string[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (path.endsWith("/branding")) {
      return json(BRANDING);
    }
    // The Check is a dry run of the same collection the proposal posts to; it answers a verdict
    // and never a change (PF-57).
    if (isCheck(request.method(), request.url())) {
      return json(greenVerdict(request.postData()));
    }
    const draft = /^\/api\/v1\/projects\/([^/]+)\/drafts\/([^/]+)\/([^/]+)$/.exec(path);
    if (draft && request.method() === "PUT") {
      // The form saves its draft before it proposes (T-0769); the draft is not the change.
      const { manifest } = JSON.parse(request.postData() ?? "{}") as { manifest: unknown };
      const [, project, kind, name] = draft;
      return json({
        project,
        kind,
        name,
        manifest,
        touchedBy: "jana.kovacova",
        touchedKind: "person",
        version: 1,
        updatedAt: new Date().toISOString(),
      });
    }
    // The workbench's steps sample, try and check on the runner; they write nothing (PL-63).
    if (/\/ops\/jc_pipeline_(sample_source|try_mapping|validate)$/.test(path)) {
      return json({ records: [], fields: [], count: 0, truncated: false, errors: [] });
    }
    if (request.method() !== "GET") {
      writes.push(`${request.method()} ${path} ${request.postData() ?? ""}`);
      return json(CHANGE, 202);
    }
    if (path.endsWith("/pipelines/air-quality/bento")) {
      return route.fulfill({ status: 200, contentType: "text/yaml", body: BENTO });
    }
    if (path.endsWith("/pipelines")) {
      return json(list(pipelines));
    }
    if (path === "/api/v1/projects") {
      return json(PROJECTS);
    }
    if (path.endsWith("/datasources")) {
      return json(DATASOURCES);
    }
    if (path.endsWith("/endpoints")) {
      return json(ENDPOINTS);
    }
    if (path.endsWith("/spaces")) {
      return json(SPACES);
    }
    if (path.endsWith("/datamodels")) {
      return json(DATAMODELS);
    }
    return json(list([]));
  });
  return { writes };
}

test.describe("pipeline editor", () => {
  test("creates a pipeline from the form and proposes it as a change", async ({ page }) => {
    const { writes } = await stubApi(page);

    await page.goto("/projects/helsinki/pipelines?lang=en");
    // An empty list offers the action twice: in the page header and in the empty state
    // itself (UI-01). The header's is the one a person with pipelines also has.
    await page.getByRole("button", { name: "New pipeline" }).first().click();
    const dialog = page.getByTestId("form-page");

    await dialog.getByLabel(/^Name/).fill("aq-ingest");
    // The execution and the kind of step sit under More options, the studio under the Advanced
    // editor: both folded, both on the same draft as the workbench (PL-58, T-2709).
    await dialog.getByText("More options").click();
    await dialog.getByText("Advanced editor").click();
    // Every choice reads as words (T-2756), and the source's own fields follow the choice of
    // where to read from (T-2754).
    await dialog.getByLabel(/^Execution/).selectOption({ label: "Always running" });
    await dialog.getByLabel(/^Read from/).selectOption({ label: "A data source of this project (external feed)" });
    // The studio reads from the same draft as the workbench: its choice is the workbench's.
    await dialog.getByTestId("pipeline-studio").getByLabel(/^Data source/).selectOption("mqtt-city");
    await expect(dialog.locator("#workbench-source-pick")).toHaveValue("datasource:mqtt-city");
    // The options carry the endpoint's name; the value is the URN (T-0630).
    await dialog.getByLabel(/^Target endpoint/).selectOption({ label: "public-air" });
    await dialog.getByLabel(/^Kind/).selectOption({ label: "Reshape records (Bloblang)" });
    await expect(dialog.getByText(/bento\.yaml beside this manifest/)).toBeVisible();

    // The YAML view shows the manifest the form wrote, in the real editor.
    await dialog.getByRole("tab", { name: "YAML" }).click();
    await expect(dialog.locator(".monaco-editor")).toBeVisible();
    await expect(dialog.locator(".view-lines")).toContainText("aq-ingest");
    await expect(dialog.locator(".view-lines")).toContainText("targetEndpoint");
    await dialog.getByRole("tab", { name: "Form" }).click();
    await expect(dialog.getByLabel(/^Name/)).toHaveValue("aq-ingest");

    // Strict validation proposes nothing without a fresh green verdict (T-0779); the dialog runs
    // the check by itself once the form rests (T-4818), so no hidden step opens Propose.
    await expect(dialog.getByTestId("draft-verdict")).toContainText("Checked");
    await dialog.getByRole("button", { name: "Propose change" }).click();

    await expect(page.getByText("chg-77aa11bb")).toBeVisible();
    await expect(page.getByRole("link", { name: "Review it in Approvals" })).toBeVisible();

    expect(writes).toHaveLength(1);
    const [method, path, body] = writes[0].split(" ", 3);
    expect(`${method} ${path}`).toBe("POST /api/v1/projects/helsinki/pipelines");
    expect(JSON.parse(writes[0].slice(method.length + path.length + 2))).toEqual({
      // Whoever writes a Pipeline writes the second shape (PL-54, ADR-N-023).
      apiVersion: "joinedcontext.com/v1alpha2",
      kind: "Pipeline",
      metadata: { name: "aq-ingest", namespace: "helsinki" },
      spec: {
        class: "resident",
        sources: [{ dataSourceRef: { kind: "DataSource", name: "mqtt-city" } }],
        steps: [{ kind: "bloblang" }],
        outputs: [{ targetEndpoint: "urn:ngsi-ld:Endpoint:hel.fi:air:public-air" }],
      },
      // The change names the draft the form saved, so the approver reads what was typed (T-0769).
      draft: { kind: "Pipeline", name: "aq-ingest" },
    });
    void body;
  });

  test("shows the bento.yaml an existing pipeline runs and proposes the edited file", async ({ page }) => {
    const { writes } = await stubApi(page, [BENTO_PIPELINE]);
    await page.goto("/projects/helsinki/pipelines?lang=en");
    const row = page.getByRole("row").filter({ hasText: "air-quality" });
    await row.getByRole("button", { name: /More actions/ }).click();
    await page.getByRole("menuitem", { name: "Edit" }).click();
    const dialog = page.getByTestId("form-page");

    // The basics come first, above the six steps (T-4818).
    const name = await dialog.getByLabel(/^Name/).boundingBox();
    const steps = await dialog.getByTestId("pipeline-workbench").boundingBox();
    expect(name && steps && name.y < steps.y).toBe(true);

    const file = dialog.getByLabel("Mapping file (bento.yaml)");
    await expect(file).toHaveValue(BENTO);
    await expect(file).toHaveAttribute("readonly", "");
    await dialog.getByLabel(/^Edit the mapping file/).check();
    await file.fill(`${BENTO}# checked\n`);
    await expect(dialog.getByTestId("draft-verdict")).toContainText("Checked");
    await dialog.getByRole("button", { name: "Propose change" }).click();
    await expect(page.getByText("chg-77aa11bb")).toBeVisible();

    const put = writes.find((write) => write.startsWith("PUT /api/v1/projects/helsinki/pipelines/air-quality "));
    expect(put).toBeDefined();
    const body = JSON.parse((put ?? "").split(" ").slice(2).join(" ")) as { files?: Record<string, string>; draft?: unknown };
    expect(body.files).toEqual({ "bento.yaml": `${BENTO}# checked\n` });
    expect(body.draft).toBeUndefined();
  });

  // T-4971: the class picker of a model with several classes carries its one-line hint (UI-16),
  // as the live walker's field-hint check asks of every field on the edit page.
  test("the class a mapping writes is picked with a hint when the model has several", async ({ page }) => {
    await stubApi(page, [BENTO_PIPELINE]);
    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: 900 });
      await page.goto("/projects/helsinki/pipelines?lang=en");
      const row = page.getByRole("row").filter({ hasText: "air-quality" });
      await row.getByRole("button", { name: /More actions/ }).click();
      await page.getByRole("menuitem", { name: "Edit" }).click();
      const picker = page.getByTestId("form-page").getByRole("combobox", { name: "Type the mapping writes" });
      await expect(picker).toBeVisible();
      await expect(picker).toHaveAccessibleDescription(/pick the one each record becomes/);
      await picker.scrollIntoViewIfNeeded();
      if (process.env.SHOTS_DIR) await page.screenshot({ path: `${process.env.SHOTS_DIR}/t4971-class-picker-${width}.png` });
    }
  });
});
